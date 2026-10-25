//! Read-only native Fuse command planning for direct ZX Spectrum media.

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{FuseLaunchBlocker, FuseNativeLaunchBinding};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const FUSE_SUPPORTED_PLATFORM_ID: &str = "ZX Spectrum";
const FUSE_SUPPORTED_EXTENSIONS: &[&str] = &["tap", "tzx", "sna", "z80", "szx"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuseCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: FuseCommandSelection,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuseCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub game_key: String,
    pub content_path: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuseCommandPlan {
    pub command: Option<FuseCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}
pub(crate) fn direct_fuse_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        FUSE_SUPPORTED_EXTENSIONS
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    })
}

pub fn build_fuse_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<FuseNativeLaunchBinding, FuseLaunchBlocker>,
) -> FuseCommandPlan {
    let mut blockers = Vec::new();
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(r) => Some(r),
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
    if let Some(r) = resolved
        && r.platform_id != FUSE_SUPPORTED_PLATFORM_ID
    {
        blockers.push(block(
            LaunchBlockerKind::CandidateBlocked,
            "resolved platform is not ZX Spectrum",
        ));
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(block(
            LaunchBlockerKind::CandidateBlocked,
            "candidate is not a standalone Fuse target",
        ));
        return FuseCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "fuse" {
        blockers.push(block(
            LaunchBlockerKind::CandidateBlocked,
            "candidate does not target Fuse",
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
        return FuseCommandPlan {
            command: None,
            blockers,
        };
    };
    if crate::archive_kind(content).is_some_and(|k| k.is_mount_input())
        || !direct_fuse_extension(content)
    {
        blockers.push(block(
            LaunchBlockerKind::CandidateBlocked,
            "native Fuse supports only direct .tap, .tzx, .sna, .z80 or .szx content",
        ));
    }
    let binding = match binding {
        Ok(b) => Some(b),
        Err(e) => {
            blockers.push(block(
                LaunchBlockerKind::ProfileIneligible,
                e.detail.clone(),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return FuseCommandPlan {
            command: None,
            blockers,
        };
    }
    let r = resolved.expect("resolved when unblocked");
    let b = binding.expect("binding when unblocked");
    FuseCommandPlan {
        command: Some(FuseCommand {
            executable: b.executable.clone(),
            arguments: vec![content.clone().into_os_string()],
            working_directory: None,
            selection: FuseCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: r.platform_id.clone(),
                game_key: r.game_key.clone(),
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
    fn id(p: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: p.into(),
            game_key: "key".into(),
        })
    }
    fn candidate(path: &str, adapter: &'static str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: adapter,
                profile_id: "fuse:/usr/bin/fuse".into(),
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::Executable),
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
    fn binding() -> Result<FuseNativeLaunchBinding, FuseLaunchBlocker> {
        Ok(FuseNativeLaunchBinding {
            executable: "/usr/bin/fuse".into(),
        })
    }
    #[test]
    fn supported_tap_has_exact_typed_argv() {
        let p = build_fuse_command_plan(
            &id("ZX Spectrum"),
            &candidate("/roms/a.tap", "fuse"),
            &binding(),
        );
        let c = p.command.unwrap();
        assert_eq!(c.arguments, vec![OsString::from("/roms/a.tap")]);
        assert_eq!(c.executable, PathBuf::from("/usr/bin/fuse"));
    }
    #[test]
    fn tzx_and_snapshots_are_supported_but_other_media_refused() {
        for ext in ["tzx", "sna", "z80", "szx"] {
            assert!(
                build_fuse_command_plan(
                    &id("ZX Spectrum"),
                    &candidate(&format!("/roms/a.{ext}"), "fuse"),
                    &binding()
                )
                .command
                .is_some()
            );
        }
        assert!(
            build_fuse_command_plan(
                &id("ZX Spectrum"),
                &candidate("/roms/a.dsk", "fuse"),
                &binding()
            )
            .command
            .is_none()
        );
    }
    #[test]
    fn wrong_platform_and_missing_binding_refuse() {
        assert!(
            build_fuse_command_plan(&id("C64"), &candidate("/roms/a.tap", "fuse"), &binding())
                .command
                .is_none()
        );
        assert!(
            build_fuse_command_plan(
                &id("ZX Spectrum"),
                &candidate("/roms/a.tap", "fuse"),
                &Err(FuseLaunchBlocker {
                    kind: crate::patch_manager::FuseLaunchBlockerKind::ExecutableMissing,
                    detail: "missing".into()
                })
            )
            .command
            .is_none()
        );
    }
}
