//! Safe native FinalBurn Neo planning from FBNeo-specific DAT evidence.
//!
//! FBNeo is not treated as a MAME alias: the set evidence must come from the
//! explicitly branded FBNeo DAT ecosystem. The command uses the trusted
//! FBNeo driver/set name, while the selected archive remains separate
//! provenance and is never guessed from its filename.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::dat::dependency::DependencyState;
use crate::dat::model::DatEcosystem;
use crate::dat::set::{SetResolution, SetState};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};

pub const FBNEO_SUPPORTED_PLATFORM_ID: &str = "Arcade";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FbneoIdentityEvidence {
    /// A local DAT explicitly identified itself as FBNeo. The DAT's
    /// collision-preserving hash match is what authorizes this evidence;
    /// this value does not authorize a MAME-only match.
    VerifiedDat {
        source_id: String,
        ecosystem: DatEcosystem,
    },
    /// A MAME/listXML match is deliberately not enough for FBNeo.
    MameOnly { source_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbneoSetEvidence {
    pub driver_name: String,
    pub resolution: SetResolution,
    pub identity_evidence: FbneoIdentityEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbneoCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub driver_name: String,
    pub selected_content: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbneoCommandPlan {
    pub command: Option<FbneoCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

pub fn build_fbneo_command_plan(
    identity: &CanonicalIdentityStatus,
    set: &FbneoSetEvidence,
    executable: Option<&std::path::Path>,
) -> FbneoCommandPlan {
    let mut blockers = Vec::new();
    let canonical = match identity {
        CanonicalIdentityStatus::Resolved(identity)
            if identity.platform_id == FBNEO_SUPPORTED_PLATFORM_ID =>
        {
            Some(identity)
        }
        CanonicalIdentityStatus::Resolved(identity) => {
            blockers.push(blocker(
                LaunchBlockerKind::FbneoPlatformMismatch,
                format!("resolved platform is {}, not Arcade", identity.platform_id),
            ));
            None
        }
        CanonicalIdentityStatus::Unknown => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityUnresolved,
                "Arcade platform identity is unresolved",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityConflict,
                "Arcade platform evidence is conflicting",
            ));
            None
        }
    };

    if !matches!(
        set.identity_evidence,
        FbneoIdentityEvidence::VerifiedDat {
            ecosystem: DatEcosystem::FBNeo,
            ..
        }
    ) {
        blockers.push(blocker(
            LaunchBlockerKind::FbneoCompatibilityUnavailable,
            "no verified FBNeo-specific DAT evidence is available",
        ));
    }
    if set.resolution.identity.source_id.trim().is_empty()
        || set.driver_name.trim().is_empty()
        || set.driver_name != set.resolution.identity.game_name
    {
        blockers.push(blocker(
            LaunchBlockerKind::FbneoIdentityUnavailable,
            "the trusted FBNeo driver identity is missing or does not match the set evidence",
        ));
    }
    if let Some(identity) = canonical
        && identity.game_key != set.driver_name
    {
        blockers.push(blocker(
            LaunchBlockerKind::FbneoIdentityUnavailable,
            "the canonical Arcade identity does not match the FBNeo driver name",
        ));
    }
    match set.resolution.state {
        SetState::Complete => {}
        SetState::Incomplete => blockers.push(blocker(
            LaunchBlockerKind::FbneoSetIncomplete,
            "the FBNeo set is incomplete",
        )),
        SetState::BadMetadata(_) | SetState::NeedsReview(_) => blockers.push(blocker(
            LaunchBlockerKind::FbneoCompatibilityUnavailable,
            "the FBNeo set verdict requires review and is not launch-authorizing",
        )),
    }
    if !set.resolution.dependencies.state.permits_complete()
        && set.resolution.dependencies.state != DependencyState::NotApplicable
    {
        blockers.push(blocker(
            LaunchBlockerKind::FbneoDependencyBlocked,
            format!(
                "FBNeo dependency state is {:?}",
                set.resolution.dependencies.state
            ),
        ));
    }
    let Some(executable) = executable else {
        blockers.push(blocker(
            LaunchBlockerKind::FbneoEmulatorUnavailable,
            "no explicit standalone FBNeo executable binding is available",
        ));
        return FbneoCommandPlan {
            command: None,
            blockers,
        };
    };
    if !blockers.is_empty() {
        return FbneoCommandPlan {
            command: None,
            blockers,
        };
    }
    FbneoCommandPlan {
        command: Some(FbneoCommand {
            executable: executable.to_path_buf(),
            arguments: vec![OsString::from(set.driver_name.as_str())],
            working_directory: None,
            driver_name: set.driver_name.clone(),
            selected_content: set.resolution.archive_path.clone(),
        }),
        blockers,
    }
}

#[cfg(test)]
mod tests;
