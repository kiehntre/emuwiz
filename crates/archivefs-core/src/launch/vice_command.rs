//! Exact, no-shell VICE C64 command planning.
use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{ViceLaunchBlocker, ViceNativeLaunchBinding};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const VICE_SUPPORTED_PLATFORM_ID: &str = "Commodore 64";
pub const VICE_DISABLE_SAVE_RESOURCES: &str = "+saveres";
pub const VICE_AUTOSTART: &str = "-autostart";
pub const VICE_ATTACH_CRT: &str = "-cartcrt";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViceContentKind {
    Autostart,
    Cartridge,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: ViceCommandSelection,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub game_key: String,
    pub content_path: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceCommandPlan {
    pub command: Option<ViceCommand>,
    pub blockers: Vec<LaunchBlocker>,
}
fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}
/// Only the registry's strong C64 forms: CRT uses VICE's documented cartridge
/// switch; T64/G64/D81NS use documented autostart. Weak/ambiguous forms refuse.
pub(crate) fn vice_content_kind(path: &Path) -> Option<ViceContentKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "t64" | "g64" | "d81ns" => Some(ViceContentKind::Autostart),
        "crt" => Some(ViceContentKind::Cartridge),
        _ => None,
    }
}
pub fn build_vice_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<ViceNativeLaunchBinding, ViceLaunchBlocker>,
) -> ViceCommandPlan {
    let mut blockers = Vec::new();
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(x) => Some(x),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(block(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical game identity could not be resolved",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(block(
                LaunchBlockerKind::IdentityConflict,
                "canonical game identity conflicts",
            ));
            None
        }
    };
    if resolved.is_some_and(|x| x.platform_id != VICE_SUPPORTED_PLATFORM_ID) {
        blockers.push(block(
            LaunchBlockerKind::VicePlatformMismatch,
            "VICE C64 accepts only canonical Commodore 64 content",
        ));
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        return ViceCommandPlan {
            command: None,
            blockers: vec![block(
                LaunchBlockerKind::ViceCandidateRequired,
                "candidate is not a standalone VICE target",
            )],
        };
    };
    if *adapter_id != "vice" {
        blockers.push(block(
            LaunchBlockerKind::ViceCandidateRequired,
            format!("candidate targets `{adapter_id}`, not `vice`"),
        ));
    }
    if candidate.readiness == crate::launch::readiness::LaunchReadiness::Blocked {
        blockers.extend(candidate.blockers.iter().cloned());
    }
    let Some(content) = candidate
        .content
        .resolved_path
        .as_ref()
        .filter(|_| !candidate.content.requires_mount)
    else {
        blockers.push(block(
            LaunchBlockerKind::ContentNotResolved,
            "no direct runnable content path is available",
        ));
        return ViceCommandPlan {
            command: None,
            blockers,
        };
    };
    if crate::archive_kind(content).is_some_and(|k| k.is_mount_input())
        || vice_content_kind(content).is_none()
    {
        blockers.push(block(
            LaunchBlockerKind::ViceContentFormatUnsupported,
            "VICE accepts only direct strong C64 .t64, .g64, .d81ns, or .crt content",
        ));
    }
    let binding = match binding {
        Ok(x) => Some(x),
        Err(x) => {
            blockers.push(block(
                LaunchBlockerKind::ViceBindingUnavailable,
                x.detail.clone(),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return ViceCommandPlan {
            command: None,
            blockers,
        };
    }
    let arguments = match vice_content_kind(content).unwrap() {
        ViceContentKind::Autostart => vec![
            VICE_DISABLE_SAVE_RESOURCES.into(),
            VICE_AUTOSTART.into(),
            content.clone().into_os_string(),
        ],
        ViceContentKind::Cartridge => vec![
            VICE_DISABLE_SAVE_RESOURCES.into(),
            "+cart".into(),
            VICE_ATTACH_CRT.into(),
            content.clone().into_os_string(),
        ],
    };
    let resolved = resolved.unwrap();
    ViceCommandPlan {
        command: Some(ViceCommand {
            executable: binding.unwrap().executable.clone(),
            arguments,
            working_directory: None,
            selection: ViceCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: resolved.platform_id.clone(),
                game_key: resolved.game_key.clone(),
                content_path: content.clone(),
            },
        }),
        blockers: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::{
        CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
        ResolvedIdentity,
    };
    use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
    use crate::patch_manager::ViceC64ExecutableKind;
    fn identity(platform_id: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform_id.into(),
            game_key: "key".into(),
        })
    }
    fn candidate(path: &str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "vice",
                profile_id: "vice:native:/usr/bin/x64sc".into(),
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::Cartridge),
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(path.into()),
                requires_mount: false,
                provenance: "test".into(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: vec![],
            warnings: vec![],
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::SoleEligible,
        }
    }
    fn binding() -> Result<ViceNativeLaunchBinding, ViceLaunchBlocker> {
        Ok(ViceNativeLaunchBinding {
            executable: "/usr/bin/x64sc".into(),
            kind: ViceC64ExecutableKind::X64sc,
        })
    }
    #[test]
    fn exact_direct_argv_and_no_shell_for_each_allowed_form() {
        for extension in ["t64", "g64", "d81ns"] {
            let command = build_vice_command_plan(
                &identity("Commodore 64"),
                &candidate(&format!("/roms/game.{extension}")),
                &binding(),
            )
            .command
            .unwrap();
            assert_eq!(
                command.arguments,
                vec![
                    OsString::from("+saveres"),
                    OsString::from("-autostart"),
                    OsString::from(format!("/roms/game.{extension}"))
                ]
            );
            assert_ne!(command.executable, PathBuf::from("sh"));
        }
        let command = build_vice_command_plan(
            &identity("Commodore 64"),
            &candidate("/roms/game.crt"),
            &binding(),
        )
        .command
        .unwrap();
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("+saveres"),
                OsString::from("+cart"),
                OsString::from("-cartcrt"),
                OsString::from("/roms/game.crt")
            ]
        );
    }
    #[test]
    fn refuses_nonstrong_forms_and_all_non_c64_platforms() {
        for ext in ["d64", "d71", "d81", "tap", "prg", "p00", "zip"] {
            assert!(
                build_vice_command_plan(
                    &identity("Commodore 64"),
                    &candidate(&format!("/roms/game.{ext}")),
                    &binding()
                )
                .command
                .is_none()
            );
        }
        for id in ["VIC-20", "Commodore 128", "PET", "Plus/4"] {
            assert!(
                build_vice_command_plan(&identity(id), &candidate("/roms/game.t64"), &binding())
                    .command
                    .is_none()
            );
        }
    }
}
