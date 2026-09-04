//! Safe native MAME command planning from existing Arcade/DAT evidence.
//!
//! MAME receives one exact set shortname as one argv component. The planner
//! never expands an archive, rebuilds a set, or turns filenames into identity.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::dat::dependency::DependencyState;
use crate::dat::set::{SetResolution, SetState};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};

/// Canonical platforms whose verified DAT-backed sets may be launched by the
/// native MAME adapter. NeoGeo cartridge/MVS/AES sets use the same MAME
/// shortname and dependency gates as Arcade; Neo Geo CD is deliberately not
/// included because it is a separate optical-media platform.
pub const MAME_SUPPORTED_PLATFORM_IDS: &[&str] = &["Arcade", "NeoGeo"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MameCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub set_name: String,
    /// Exact archive/ROM evidence associated with the selected set. MAME
    /// launches by `set_name`; this path is deliberately not placed in argv.
    pub selected_content: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MameCommandPlan {
    pub command: Option<MameCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn blocked(blockers: Vec<LaunchBlocker>) -> MameCommandPlan {
    MameCommandPlan {
        command: None,
        blockers,
    }
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

/// Builds the minimal native MAME invocation from exactly one current set
/// verdict. `set_resolutions` is intentionally an input slice: no verdict
/// means blocked, and multiple distinct identities never get silently picked.
pub fn build_mame_command_plan(
    identity: &CanonicalIdentityStatus,
    set_resolutions: &[SetResolution],
    executable: Option<&std::path::Path>,
    rom_search_path_configured: bool,
) -> MameCommandPlan {
    let mut blockers = Vec::new();
    match identity {
        CanonicalIdentityStatus::Resolved(identity)
            if MAME_SUPPORTED_PLATFORM_IDS.contains(&identity.platform_id.as_str()) => {}
        CanonicalIdentityStatus::Resolved(identity) => blockers.push(blocker(
            LaunchBlockerKind::MamePlatformMismatch,
            format!(
                "resolved platform is {}, not supported by native MAME",
                identity.platform_id
            ),
        )),
        CanonicalIdentityStatus::Unknown => blockers.push(blocker(
            LaunchBlockerKind::IdentityUnresolved,
            "Arcade platform identity is unresolved",
        )),
        CanonicalIdentityStatus::Conflicting => blockers.push(blocker(
            LaunchBlockerKind::IdentityConflict,
            "Arcade platform evidence is conflicting",
        )),
    }

    let identity = match identity {
        CanonicalIdentityStatus::Resolved(identity)
            if MAME_SUPPORTED_PLATFORM_IDS.contains(&identity.platform_id.as_str()) =>
        {
            Some(identity)
        }
        _ => None,
    };
    if set_resolutions.is_empty() {
        blockers.push(blocker(
            LaunchBlockerKind::MameSetVerdictUnavailable,
            "no current DAT-backed MAME set verdict is available",
        ));
    } else if set_resolutions.len() != 1 {
        blockers.push(blocker(
            LaunchBlockerKind::MameSetIdentityAmbiguous,
            "more than one MAME set resolution is available",
        ));
    }
    let resolution = set_resolutions
        .first()
        .filter(|_| set_resolutions.len() == 1);
    if let Some(resolution) = resolution {
        if resolution.identity.game_name.trim().is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::MameSetIdentityUnavailable,
                "the MAME set has no usable unique shortname",
            ));
        }
        if let Some(identity) = identity
            && identity.game_key != resolution.identity.game_name
        {
            blockers.push(blocker(
                LaunchBlockerKind::MameSetIdentityUnavailable,
                "the authorized Arcade identity does not match the DAT set name",
            ));
        }
        match resolution.state {
            SetState::Complete => {}
            SetState::Incomplete => blockers.push(blocker(
                LaunchBlockerKind::MameSetIncomplete,
                "the MAME set is incomplete",
            )),
            SetState::BadMetadata(_) | SetState::NeedsReview(_) => blockers.push(blocker(
                LaunchBlockerKind::MameSetVerdictUnavailable,
                "the MAME set verdict requires review and is not launch-authorizing",
            )),
        }
        if !resolution.dependencies.state.permits_complete()
            && resolution.dependencies.state != DependencyState::NotApplicable
        {
            blockers.push(blocker(
                LaunchBlockerKind::MameDependencyBlocked,
                format!(
                    "MAME dependency state is {:?}",
                    resolution.dependencies.state
                ),
            ));
        }
    }
    if executable.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::MameEmulatorUnavailable,
            "no discovered MAME executable binding is available",
        ));
    }
    if !rom_search_path_configured {
        blockers.push(blocker(
            LaunchBlockerKind::MameSearchPathUnconfigured,
            "the MAME ROM/search path arrangement is not explicitly configured",
        ));
    }
    if !blockers.is_empty() {
        return blocked(blockers);
    }
    let resolution = resolution.expect("resolution exists when no blockers exist");
    let executable = executable.expect("executable exists when no blockers exist");
    let set_name = resolution.identity.game_name.trim().to_string();
    MameCommandPlan {
        command: Some(MameCommand {
            executable: executable.to_path_buf(),
            arguments: vec![OsString::from(set_name.as_str())],
            working_directory: None,
            set_name,
            selected_content: resolution.archive_path.clone(),
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
