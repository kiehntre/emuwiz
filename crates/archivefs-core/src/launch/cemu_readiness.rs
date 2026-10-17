//! Read-only composition of recovered Wii U evidence with the existing Cemu backend.
//! Metadata is structural evidence, never canonical or cryptographic verification.
use super::cemu_command::CemuCommand;
use super::cemu_execution::{CemuPreflightRequest, preflight_cemu_launch};
use super::{
    LaunchBlocker, LaunchBlockerKind, LaunchPlan, LaunchPlanSummary, LaunchReadiness, LaunchTarget,
};
use crate::patch_manager::{
    CemuProfileDiscoveryRoots, CemuTitleKind, classify_title_kind, discover_cemu_profiles,
};
use crate::wiiu_content_readiness::{
    self as content, WiiUContentProjection, WiiUContentReadiness, WiiUContentRelationship,
    WiiUContentRole, WiiUContentStamp, WiiUKeyStoreEvidence,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CemuProfilePreview {
    pub profile_id: String,
    pub profile_path: PathBuf,
    pub executable: Option<PathBuf>,
    pub related: Vec<WiiUContentProjection>,
    pub command: Option<CemuCommand>,
    pub stamps: Vec<WiiUContentStamp>,
    pub refusals: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct CemuReadinessInspection {
    pub content: WiiUContentProjection,
    pub stamp: WiiUContentStamp,
    pub profiles: Vec<CemuProfilePreview>,
    pub refusals: Vec<String>,
}

/// Only exact, already configured MLC locations are inspected. Presence does not
/// mean Cemu applied an update or that a DLC entitlement is valid.
pub(crate) fn inspect_related_with_stamps(
    mlc: &Path,
    base_title_id: &str,
) -> (Vec<WiiUContentProjection>, Vec<WiiUContentStamp>) {
    let base_title_id = base_title_id.trim();
    if classify_title_kind(base_title_id) != CemuTitleKind::Base {
        return (Vec::new(), Vec::new());
    }
    ["0005000e", "0005000c"]
        .into_iter()
        .filter_map(|high| {
            let path = mlc
                .join("usr/title")
                .join(high)
                .join(base_title_id[8..].to_ascii_lowercase());
            match std::fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                _ => {
                    let stamp = WiiUContentStamp::capture(&path);
                    let mut projection = content::project_wiiu_content_readiness(
                        &path,
                        WiiUKeyStoreEvidence::NotChecked,
                        Some(base_title_id),
                    );
                    let expected = format!("{high}{}", &base_title_id[8..]);
                    if !projection
                        .identity
                        .as_ref()
                        .and_then(|i| i.title_id.as_ref())
                        .is_some_and(|id| id.eq_ignore_ascii_case(&expected))
                    {
                        projection
                            .issues
                            .push(content::WiiUContentIssue::ConflictingMetadata);
                        projection.readiness = WiiUContentReadiness::Ambiguous;
                    }
                    Some((projection, stamp))
                }
            }
        })
        .unzip()
}

/// Structural related-content projection; callers authorizing a launch also
/// retain the stamps from `inspect_related_with_stamps`.
pub fn inspect_related_content(mlc: &Path, base_title_id: &str) -> Vec<WiiUContentProjection> {
    inspect_related_with_stamps(mlc, base_title_id).0
}

/// Validates caller-selected related content without choosing a version or
/// installing anything. Duplicate role/title pairs are ambiguous even at the
/// same version; different declared versions are explicitly conflicting.
pub fn related_content_refusals(related: &[WiiUContentProjection]) -> Vec<String> {
    let mut refusals = Vec::new();
    let mut versions = std::collections::BTreeMap::new();
    for projection in related {
        if !matches!(
            projection.role,
            WiiUContentRole::Update | WiiUContentRole::Dlc
        ) || !matches!(
            projection.relationship,
            WiiUContentRelationship::Linked { .. }
        ) || !projection.structural_complete
            || !projection.issues.is_empty()
            || projection.representation != content::WiiUContentRepresentation::LoadiineExtracted
            || projection.encryption_state != content::WiiUEncryptionState::Decrypted
        {
            refusals.push(format!(
                "Related content {} has incomplete, conflicting or mismatched evidence",
                projection.source.display()
            ));
        }
        if let Some(id) = projection
            .identity
            .as_ref()
            .and_then(|i| i.title_id.as_ref())
        {
            let version = projection
                .evidence
                .directory
                .as_ref()
                .and_then(|d| d.title_version.clone());
            if let Some(previous) = versions.insert(id.clone(), version.clone()) {
                refusals.push(format!("Conflicting or duplicate versions for {id}: {previous:?} and {version:?}; no version selected"));
            }
        }
    }
    refusals
}

pub fn inspect_cemu_readiness(
    path: &Path,
    expected_title_id: Option<&str>,
    roots: &CemuProfileDiscoveryRoots,
) -> CemuReadinessInspection {
    let stamp = WiiUContentStamp::capture(path);
    let projection =
        content::project_wiiu_content_readiness(path, WiiUKeyStoreEvidence::NotChecked, None);
    let mut refusals = Vec::new();
    if projection.readiness != WiiUContentReadiness::DirectlyLaunchable {
        refusals.push(format!(
            "Content {:?}: {:?}; issues: {:?}",
            projection.representation, projection.readiness, projection.issues
        ));
    }
    let title_id = projection
        .identity
        .as_ref()
        .and_then(|i| i.title_id.as_deref());
    if let Some(expected) = expected_title_id {
        if !title_id.is_some_and(|id| id.eq_ignore_ascii_case(expected)) {
            refusals.push("TITLE_ID conflicts with the verified launch identity".into());
        }
    }
    if stamp.is_stale() {
        refusals.push(
            "Content changed or exceeds bounded freshness inspection; refresh required".into(),
        );
    }
    let mut profiles: Vec<CemuProfilePreview> = discover_cemu_profiles(roots)
        .profiles
        .iter()
        .map(|profile| {
            let binding = crate::patch_manager::resolve_cemu_native_launch_binding(profile);
            let executable = binding.as_ref().ok().map(|b| b.executable.clone());
            let (related, mut stamps) = profile
                .config
                .as_ref()
                .and_then(|c| c.mlc.path.as_ref())
                .zip(title_id)
                .map(|(mlc, id)| inspect_related_with_stamps(mlc, id))
                .unwrap_or_default();
            if let Some(path) = &executable {
                stamps.push(WiiUContentStamp::capture(path));
            }
            if let Some(path) = &profile.config_path {
                stamps.push(WiiUContentStamp::capture(path));
            }
            let mut profile_refusals = related_content_refusals(&related);
            if profile
                .config_path
                .as_ref()
                .map(|path| crate::patch_manager::cemu_inspect_config(path))
                != profile.config
            {
                profile_refusals
                    .push("Cemu configuration changed during discovery; refresh required".into());
            }
            let mut command = match &executable {
                Some(executable) => match preflight_cemu_launch(
                    &CemuPreflightRequest {
                        selected_content_path: path.to_path_buf(),
                        profile_id: profile.profile_id.clone(),
                        expected_executable: executable.clone(),
                    },
                    roots,
                ) {
                    Ok(command) if refusals.is_empty() && profile_refusals.is_empty() => {
                        Some(command)
                    }
                    Ok(_) => None,
                    Err(e) => {
                        profile_refusals.push(format!("{:?}: {}", e.kind, e.detail));
                        None
                    }
                },
                None => {
                    profile_refusals.push(format!("Cemu binding unavailable: {:?}", binding.err()));
                    None
                }
            };
            if stamps.iter().any(WiiUContentStamp::is_stale) {
                profile_refusals
                    .push("Cemu binding or related content changed during inspection".into());
                command = None;
            }
            CemuProfilePreview {
                profile_id: profile.profile_id.clone(),
                profile_path: profile.configuration_path.clone(),
                executable,
                related,
                command,
                stamps,
                refusals: profile_refusals,
            }
        })
        .collect();
    if stamp.is_stale() {
        refusals.push("Content changed during inspection; refresh required".into());
        for profile in &mut profiles {
            profile.command = None;
        }
    }
    CemuReadinessInspection {
        content: projection,
        stamp,
        profiles,
        refusals,
    }
}

/// Adds evidence to existing candidates; never selects a different emulator or
/// clears a prior refusal. No filesystem I/O; suitable for GUI rendering.
pub fn apply_cemu_readiness(plan: &mut LaunchPlan, inspection: Option<&CemuReadinessInspection>) {
    for candidate in &mut plan.candidates {
        let LaunchTarget::Standalone {
            adapter_id: "cemu",
            profile_id,
            ..
        } = &candidate.target
        else {
            continue;
        };
        let mut reasons = Vec::new();
        match inspection {
            None => reasons.push("Wii U content readiness has not been inspected".to_string()),
            Some(inspection) => {
                reasons.extend(inspection.refusals.clone());
                if candidate.content.resolved_path.as_ref() != Some(&inspection.content.source) {
                    reasons.push("Wii U inspection is for a different content path".into());
                }
                if let Some(key) = &plan.game_key {
                    let id = inspection
                        .content
                        .identity
                        .as_ref()
                        .and_then(|i| i.title_id.as_ref());
                    if !id.is_some_and(|id| id.eq_ignore_ascii_case(key)) {
                        reasons.push("TITLE_ID conflicts with the launch identity".into());
                    }
                }
                match inspection
                    .profiles
                    .iter()
                    .find(|p| &p.profile_id == profile_id)
                {
                    Some(profile) => {
                        reasons.extend(profile.refusals.clone());
                        if profile.command.is_none() {
                            reasons
                                .push("Cemu preflight did not produce a supported command".into());
                        }
                    }
                    None => reasons.push("Selected Cemu profile was not inspected".into()),
                }
            }
        }
        if !reasons.is_empty() {
            candidate.blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::CemuLayoutInvalid,
                reasons.join("; "),
            ));
            candidate.readiness = LaunchReadiness::Blocked;
        }
    }
    plan.summary = LaunchPlanSummary {
        candidates: plan.candidates.len(),
        ..Default::default()
    };
    for candidate in &plan.candidates {
        match candidate.readiness {
            LaunchReadiness::Ready => plan.summary.ready += 1,
            LaunchReadiness::ReadyWithWarnings => plan.summary.ready_with_warnings += 1,
            LaunchReadiness::Blocked => plan.summary.blocked += 1,
        }
    }
}

/// Revalidates an inspected preview immediately before using main's existing
/// executor. A stale preview is refused, including changed nested metadata/RPX,
/// changed related content, and a different profile or executable binding.
pub fn launch_previewed_cemu(
    inspection: &CemuReadinessInspection,
    profile_id: &str,
    roots: &CemuProfileDiscoveryRoots,
) -> Result<
    super::cemu_execution::LaunchedCemuProcess,
    super::cemu_execution::CemuLaunchExecutionError,
> {
    use super::cemu_execution::{CemuLaunchPreflightError, CemuLaunchPreflightErrorKind as Kind};
    let refused = |detail: &str| CemuLaunchPreflightError {
        kind: Kind::ContentChangedBeforeSpawn,
        detail: detail.into(),
    };
    let previous = inspection
        .profiles
        .iter()
        .find(|p| p.profile_id == profile_id)
        .and_then(|p| p.command.as_ref())
        .ok_or_else(|| refused("preview has no supported Cemu command"))?;
    if inspection.stamp.is_stale()
        || inspection
            .profiles
            .iter()
            .find(|p| p.profile_id == profile_id)
            .is_some_and(|p| p.stamps.iter().any(WiiUContentStamp::is_stale))
    {
        return Err(refused("preview content changed; refresh required").into());
    }
    let fresh = inspect_cemu_readiness(
        &inspection.content.source,
        previous.selection.title_id.as_deref(),
        roots,
    );
    let profile = fresh
        .profiles
        .iter()
        .find(|p| p.profile_id == profile_id)
        .ok_or_else(|| refused("previewed Cemu profile is no longer available"))?;
    if profile.command.as_ref() != Some(previous)
        || inspection
            .profiles
            .iter()
            .find(|p| p.profile_id == profile_id)
            .map(|p| &p.related)
            != Some(&profile.related)
        || !fresh.refusals.is_empty()
        || !profile.refusals.is_empty()
    {
        return Err(refused("Cemu readiness or related content changed; refresh required").into());
    }
    let command = super::cemu_execution::preflight_cemu_launch(
        &CemuPreflightRequest {
            selected_content_path: inspection.content.source.clone(),
            profile_id: profile_id.into(),
            expected_executable: previous.executable.clone(),
        },
        roots,
    )?;
    if command != *previous
        || inspection.stamp.is_stale()
        || inspection
            .profiles
            .iter()
            .find(|p| p.profile_id == profile_id)
            .is_some_and(|p| p.stamps.iter().any(WiiUContentStamp::is_stale))
    {
        return Err(refused("preview changed during revalidation; refresh required").into());
    }
    Ok(super::cemu_execution::spawn_cemu(command)?)
}

#[cfg(test)]
#[path = "cemu_readiness_tests.rs"]
mod tests;
