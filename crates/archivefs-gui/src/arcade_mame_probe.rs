//! Blue-side wiring for per-set MAME routing of extracted Arcade set folders.
//!
//! The readiness card and the Play button call the same cached probe, so the
//! card can only say Ready when launch preflight sees the same verdict. The
//! probe is read-only (metadata queries, bounded, no config writes); results
//! are cached in memory only, keyed on the executable, the set folder, its
//! modification time and the extra ROM paths. Nothing is persisted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use archivefs_core::arcade_mame_routing::{
    ArcadeBlockReason, ArcadeMameVerdict, ArcadeReadiness, is_probe_safe_set_name,
    probe_arcade_set, read_extra_rompath_file, validate_extra_rompaths,
};
use archivefs_core::dat::dependency::{DependencyState, SetDependencyReport};
use archivefs_core::dat::set::{SetIdentity, SetResolution, SetState};
use archivefs_core::launch::{
    LaunchBlocker, LaunchBlockerKind, LaunchPlan, LaunchReadiness, LaunchTarget, LaunchWarning,
    LaunchWarningKind,
};

/// File (read only, optional) listing extra read-only MAME ROM search paths,
/// one absolute folder per line. Absent means none. This app never writes it.
pub(crate) const EXTRA_ROMPATHS_FILE: &str = "mame_extra_rompaths.txt";
const PROBE_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug, Clone)]
pub(crate) struct ArcadeSetProbe {
    pub(crate) set: String,
    pub(crate) resolution: SetResolution,
    pub(crate) verdict: ArcadeMameVerdict,
    pub(crate) extra_rompaths: Vec<PathBuf>,
}

type Key = (PathBuf, PathBuf, Option<SystemTime>, Vec<PathBuf>);

fn cache() -> &'static Mutex<HashMap<Key, Arc<ArcadeSetProbe>>> {
    static CACHE: OnceLock<Mutex<HashMap<Key, Arc<ArcadeSetProbe>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The validated extra rompaths from the optional read-only list file.
pub(crate) fn configured_extra_rompaths() -> Vec<PathBuf> {
    let Ok(file) = archivefs_core::app_dirs::config_path(EXTRA_ROMPATHS_FILE) else {
        return Vec::new();
    };
    validate_extra_rompaths(&read_extra_rompath_file(&file)).0
}

/// Probes `set_dir` (an extracted set folder) with the given MAME. `None`
/// when the path is not a plain set folder this route handles.
pub(crate) fn probe_for_set_directory(
    executable: &Path,
    set_dir: &Path,
) -> Option<Arc<ArcadeSetProbe>> {
    probe_with(executable, set_dir, &configured_extra_rompaths())
}

pub(crate) fn probe_with(
    executable: &Path,
    set_dir: &Path,
    extra: &[PathBuf],
) -> Option<Arc<ArcadeSetProbe>> {
    let metadata = std::fs::symlink_metadata(set_dir).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    let set = set_dir.file_name()?.to_str()?.to_string();
    if !is_probe_safe_set_name(&set) {
        return None;
    }
    let key: Key = (
        executable.to_path_buf(),
        set_dir.to_path_buf(),
        metadata.modified().ok(),
        extra.to_vec(),
    );
    if let Some(hit) = cache().lock().ok()?.get(&key) {
        return Some(hit.clone());
    }
    let probe = probe_arcade_set(
        executable,
        &set,
        set_dir,
        extra,
        PROBE_TIMEOUT,
        &AtomicBool::new(false),
    );
    let blocked = probe.verdict.readiness == ArcadeReadiness::Blocked;
    let result = Arc::new(ArcadeSetProbe {
        resolution: SetResolution {
            identity: SetIdentity {
                source_id: "mame-installed".to_string(),
                game_name: set.clone(),
            },
            archive_path: set_dir.to_path_buf(),
            state: if blocked {
                SetState::Incomplete
            } else {
                SetState::Complete
            },
            members_required: Vec::new(),
            members_verified: Vec::new(),
            members_bad: Vec::new(),
            members_optional: Vec::new(),
            members_borrowed: Vec::new(),
            disks_required: Vec::new(),
            disks_identity_matched: Vec::new(),
            disks_parent_required: Vec::new(),
            dependencies: SetDependencyReport {
                state: DependencyState::NotApplicable,
                requirements: Vec::new(),
            },
        },
        set,
        verdict: probe.verdict,
        extra_rompaths: extra.to_vec(),
    });
    if let Ok(mut guard) = cache().lock() {
        if guard.len() > 512 {
            guard.clear();
        }
        guard.insert(key, result.clone());
    }
    Some(result)
}

/// The launch blocker for a Blocked verdict, with MAME's exact reason.
pub(crate) fn blocker_for(verdict: &ArcadeMameVerdict) -> LaunchBlocker {
    let kind = match verdict.block_reason {
        Some(ArcadeBlockReason::RomsIncomplete | ArcadeBlockReason::RomsWrongVersion) => {
            LaunchBlockerKind::MameSetIncomplete
        }
        Some(ArcadeBlockReason::BiosOrDeviceMissing) => LaunchBlockerKind::MameDependencyBlocked,
        _ => LaunchBlockerKind::MameSetVerdictUnavailable,
    };
    LaunchBlocker::new(kind, verdict.blockers.join("; "))
}

/// Adds the verdict's named warnings to the MAME candidate (never removes or
/// adds a blocker) and refreshes readiness and the summary counts.
pub(crate) fn apply_warnings(plan: &mut LaunchPlan, probe: &ArcadeSetProbe) {
    if probe.verdict.warnings.is_empty() {
        return;
    }
    for candidate in &mut plan.candidates {
        if !matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "mame",
                ..
            }
        ) || candidate.readiness == LaunchReadiness::Blocked
        {
            continue;
        }
        for warning in &probe.verdict.warnings {
            candidate.warnings.push(LaunchWarning::new(
                LaunchWarningKind::MameMachineFlagged,
                warning.clone(),
            ));
        }
        candidate.readiness = LaunchReadiness::ReadyWithWarnings;
    }
    let count = |r: LaunchReadiness| plan.candidates.iter().filter(|c| c.readiness == r).count();
    plan.summary.ready = count(LaunchReadiness::Ready);
    plan.summary.ready_with_warnings = count(LaunchReadiness::ReadyWithWarnings);
    plan.summary.blocked = count(LaunchReadiness::Blocked);
}

#[cfg(test)]
mod tests;
