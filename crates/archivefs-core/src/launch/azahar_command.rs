//! Pure Azahar Phase 1 command planning for loose Nintendo 3DS homebrew.

use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{AzaharContentForm, AzaharEvidenceState, AzaharTitleIdentity};
use std::ffi::OsString;
use std::path::PathBuf;

pub const AZAHAR_SUPPORTED_PLATFORM_ID: &str = "Nintendo 3DS";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzaharLaunchRequest {
    pub executable: PathBuf,
    pub platform: String,
    pub content: PathBuf,
    pub content_form: AzaharContentForm,
    /// Optional existing Azahar configuration. Phase 1 never synthesizes or
    /// writes this file, but an unreadable/oversized selected profile is not
    /// safe to launch with.
    pub profile: Option<PathBuf>,
    pub profile_state: AzaharEvidenceState,
    pub profile_identity: Option<CapturedFileIdentity>,
    pub title_identity: Option<AzaharTitleIdentity>,
    pub keys_state: AzaharEvidenceState,
    pub system_data_state: AzaharEvidenceState,
    pub captured_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzaharCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzaharCommandPlan {
    pub command: Option<AzaharCommand>,
    pub blockers: Vec<LaunchBlocker>,
    pub readiness: LaunchReadiness,
}

pub fn build_azahar_command_plan(
    identity: &CanonicalIdentityStatus,
    request: &AzaharLaunchRequest,
) -> AzaharCommandPlan {
    let mut blockers = Vec::new();
    match identity {
        CanonicalIdentityStatus::Resolved(r) if r.platform_id == AZAHAR_SUPPORTED_PLATFORM_ID => {}
        CanonicalIdentityStatus::Resolved(_) => blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::AzaharPlatformMismatch,
            "identity is not Nintendo 3DS",
        )),
        CanonicalIdentityStatus::Unknown => blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::IdentityUnresolved,
            "Nintendo 3DS identity is unresolved",
        )),
        CanonicalIdentityStatus::Conflicting => blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::IdentityConflict,
            "Nintendo 3DS identity conflicts",
        )),
    }
    if request.platform != AZAHAR_SUPPORTED_PLATFORM_ID {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::AzaharPlatformMismatch,
            "request is not Nintendo 3DS",
        ));
    }
    if request.content_form != AzaharContentForm::ThreeDsx {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::AzaharContentFormatUnsupported,
            "only loose .3dsx homebrew is supported",
        ));
    }
    if matches!(
        request.profile_state,
        AzaharEvidenceState::Unreadable | AzaharEvidenceState::Oversized
    ) {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::AzaharProfileUnavailable,
            "selected Azahar profile is unreadable or oversized",
        ));
    }
    if blockers.is_empty() {
        AzaharCommandPlan {
            command: Some(AzaharCommand {
                executable: request.executable.clone(),
                arguments: vec![request.content.clone().into_os_string()],
            }),
            blockers,
            readiness: if request.title_identity.is_none() {
                LaunchReadiness::ReadyWithWarnings
            } else {
                LaunchReadiness::Ready
            },
        }
    } else {
        AzaharCommandPlan {
            command: None,
            blockers,
            readiness: LaunchReadiness::Blocked,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::ResolvedIdentity;
    #[test]
    fn three_dsx_plan_is_minimal_and_shell_free() {
        let id = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Nintendo 3DS".into(),
            game_key: "homebrew".into(),
        });
        let request = AzaharLaunchRequest {
            executable: "/opt/azahar".into(),
            platform: "Nintendo 3DS".into(),
            content: "/Games/My Homebrew.3dsx".into(),
            content_form: AzaharContentForm::ThreeDsx,
            profile: None,
            profile_state: AzaharEvidenceState::Absent,
            profile_identity: None,
            title_identity: None,
            keys_state: AzaharEvidenceState::Absent,
            system_data_state: AzaharEvidenceState::Unknown,
            captured_identity: CapturedFileIdentity {
                device: 1,
                inode: 2,
                size: 3,
                modified: None,
            },
            executable_identity: CapturedFileIdentity {
                device: 1,
                inode: 2,
                size: 3,
                modified: None,
            },
        };
        let plan = build_azahar_command_plan(&id, &request);
        assert_eq!(
            plan.command.unwrap().arguments,
            vec![OsString::from("/Games/My Homebrew.3dsx")]
        );
    }
}
