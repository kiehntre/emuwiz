//! Explicit native Saturn core authority; never ranks cores or changes firmware.
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::planning::{CandidatePreference, LaunchPlan, LaunchPlanSummary, LaunchTarget};
use super::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::emulator_environment::retroarch::{
    CoreFinding, ProfileKind, RetroArchEnvironmentReport, RetroArchProfile,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileBinding {
    path: PathBuf,
    device: u64,
    inode: u64,
    size: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    links: u64,
}

impl FileBinding {
    fn capture(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("The selected Saturn backend path is not absolute.".into());
        }
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("The selected Saturn backend is missing or is not a regular file.".into());
        }
        let id = super::process_spawn::CapturedFileIdentity::capture(&metadata);
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            path: path.to_owned(),
            device: id.device,
            inode: id.inode,
            size: id.size,
            modified: id.modified,
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            #[cfg(unix)]
            mode: metadata.mode(),
            #[cfg(unix)]
            links: metadata.nlink(),
        })
    }
    fn revalidate(&self) -> Result<(), String> {
        if Self::capture(&self.path)? != *self {
            return Err(
                "The selected Saturn core or RetroArch program changed; select it again.".into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaturnCoreSelection {
    pub core_stem: String,
    configuration: PathBuf,
    core: FileBinding,
    executable: FileBinding,
    metadata_sha256: String,
}

fn metadata_hash(core: &CoreFinding) -> Result<String, String> {
    serde_json::to_vec(&core.info)
        .map(|bytes| {
            Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        })
        .map_err(|e| e.to_string())
}

impl SaturnCoreSelection {
    pub fn capture(profile: &RetroArchProfile, core: &CoreFinding) -> Result<Self, String> {
        if profile.profile_kind != ProfileKind::Native
            || profile.config_file.path.lossy
            || core.full_path.lossy
            || !super::platform_map::retroarch_platform_matches(&core.info, "Saturn")
        {
            return Err("Choose a discovered native Saturn core with readable metadata.".into());
        }
        let [program] = profile.evidence.executables.as_slice() else {
            return Err("Choose one native RetroArch installation in Emulator Setup first.".into());
        };
        if program.lossy {
            return Err("The native RetroArch path is not exact.".into());
        }
        let selection = Self {
            core_stem: core.core_stem.clone(),
            configuration: PathBuf::from(&profile.config_file.path.display),
            core: FileBinding::capture(Path::new(&core.full_path.display))?,
            executable: FileBinding::capture(Path::new(&program.display))?,
            metadata_sha256: metadata_hash(core)?,
        };
        selection.revalidate_files()?;
        Ok(selection)
    }

    pub fn revalidate_files(&self) -> Result<(), String> {
        self.core.revalidate()?;
        self.executable.revalidate()?;
        use crate::emulator_environment::retroarch::installation_selection::{
            InstallationExecutable, classify_installation_executable,
        };
        if classify_installation_executable(
            &crate::emulator_environment::HostReadOnlyFilesystem,
            &self.executable.path,
        ) != InstallationExecutable::Native
        {
            return Err("The selected RetroArch program is no longer native.".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&self.executable.path)
                .map_err(|e| e.to_string())?
                .permissions()
                .mode()
                & 0o111
                == 0
            {
                return Err("The selected RetroArch program is not executable.".into());
            }
        }
        Ok(())
    }

    pub fn validate(&self, environment: &RetroArchEnvironmentReport) -> Result<(), String> {
        self.revalidate_files()?;
        let matches = environment
            .profiles
            .iter()
            .filter(|p| {
                p.profile_kind == ProfileKind::Native
                    && !p.config_file.path.lossy
                    && Path::new(&p.config_file.path.display) == self.configuration
            })
            .flat_map(|p| p.cores.iter().map(move |c| (p, c)))
            .filter(|(_, c)| c.core_stem == self.core_stem)
            .collect::<Vec<_>>();
        let [(profile, core)] = matches.as_slice() else {
            return Err(
                "The selected Saturn core is missing or ambiguous; select it again.".into(),
            );
        };
        if Self::capture(profile, core)? != *self {
            return Err(
                "The selected Saturn core metadata or installation changed; select it again."
                    .into(),
            );
        }
        Ok(())
    }

    pub fn matches_command(&self, command: &super::retroarch_command::RetroArchCommand) -> bool {
        command.selection.platform_id == "Saturn"
            && command.selection.profile.profile_kind == ProfileKind::Native
            && command.selection.core_stem == self.core_stem
            && command.selection.core_library == self.core.path
            && command.executable == self.executable.path
    }
}

/// Only explicit, current authority clears core ambiguity. Every other blocker
/// and warning survives, including BIOS/content/identity readiness failures.
pub fn apply(
    plan: &mut LaunchPlan,
    environment: &RetroArchEnvironmentReport,
    selection: Result<Option<&SaturnCoreSelection>, String>,
) {
    if plan.platform_id.as_deref() != Some("Saturn") {
        return;
    }
    let chosen = selection.and_then(|s| {
        let s = s.ok_or("Choose a native Saturn core in Launch details before Play.")?;
        s.validate(environment)?;
        Ok(s)
    });
    for candidate in &mut plan.candidates {
        let selected = chosen.as_ref().is_ok_and(|s| {
            matches!(&candidate.target,
            LaunchTarget::RetroArchCore { profile, core_stem, .. }
                if profile.profile_kind == ProfileKind::Native && core_stem == &s.core_stem)
        });
        candidate.preference = CandidatePreference::Undetermined;
        if chosen.is_ok() {
            candidate
                .blockers
                .retain(|b| b.kind != LaunchBlockerKind::AmbiguousCore);
            // A validated explicit choice resolves both core and profile
            // ambiguity across this plan, including excluded alternatives.
            candidate.warnings.retain(|w| {
                w.kind != super::readiness::LaunchWarningKind::MultipleEligibleProfiles
            });
        }
        if selected {
            candidate.preference = CandidatePreference::Remembered;
        } else {
            candidate.blockers.push(LaunchBlocker::new(
                if chosen.is_ok() {
                    LaunchBlockerKind::CandidateBlocked
                } else {
                    LaunchBlockerKind::AmbiguousCore
                },
                chosen
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| "A different Saturn core was explicitly selected.".into()),
            ));
        }
        candidate.readiness = if !candidate.blockers.is_empty() {
            LaunchReadiness::Blocked
        } else if !candidate.warnings.is_empty() {
            LaunchReadiness::ReadyWithWarnings
        } else {
            LaunchReadiness::Ready
        };
    }
    // Readiness explanations must describe the chosen core before alternatives.
    plan.candidates
        .sort_by_key(|c| c.preference != CandidatePreference::Remembered);
    plan.summary = LaunchPlanSummary {
        candidates: plan.candidates.len(),
        ..Default::default()
    };
    for c in &plan.candidates {
        match c.readiness {
            LaunchReadiness::Ready => plan.summary.ready += 1,
            LaunchReadiness::ReadyWithWarnings => plan.summary.ready_with_warnings += 1,
            LaunchReadiness::Blocked => plan.summary.blocked += 1,
        }
    }
}

#[cfg(test)]
mod tests;
