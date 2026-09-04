//! Read-only native mGBA command planning for direct GB/GBC/GBA files.

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{MgbaLaunchBlocker, MgbaNativeLaunchBinding};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const MGBA_SUPPORTED_PLATFORM_IDS: &[&str] =
    &["Game Boy", "Game Boy Color", "Game Boy Advance"];
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: MgbaCommandSelection,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_game_key: String,
    pub content_path: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaCommandPlan {
    pub command: Option<MgbaCommand>,
    pub blockers: Vec<LaunchBlocker>,
}
fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}
pub(crate) fn direct_mgba_extension(path: &Path, platform: &str) -> bool {
    match platform {
        "Game Boy" => path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gb")),
        "Game Boy Color" => path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gbc")),
        "Game Boy Advance" => path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gba")),
        _ => false,
    }
}

pub fn build_mgba_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<MgbaNativeLaunchBinding, MgbaLaunchBlocker>,
) -> MgbaCommandPlan {
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
    if let Some(r) = resolved {
        if !MGBA_SUPPORTED_PLATFORM_IDS.contains(&r.platform_id.as_str()) {
            blockers.push(block(
                LaunchBlockerKind::MgbaPlatformMismatch,
                "resolved platform is not supported by native mGBA",
            ));
        }
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(block(
            LaunchBlockerKind::MgbaCandidateRequired,
            "candidate is not a standalone mGBA target",
        ));
        return MgbaCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "mgba" {
        blockers.push(block(
            LaunchBlockerKind::MgbaCandidateRequired,
            format!("candidate targets `{adapter_id}`, not `mgba`"),
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
        return MgbaCommandPlan {
            command: None,
            blockers,
        };
    };
    let platform = resolved.map(|r| r.platform_id.as_str()).unwrap_or("");
    if crate::archive_kind(content).is_some_and(|k| k.is_mount_input())
        || !direct_mgba_extension(content, platform)
    {
        blockers.push(block(LaunchBlockerKind::MgbaContentFormatUnsupported, "native mGBA accepts only direct .gb, .gbc, or .gba content matching the resolved platform"));
    }
    let binding = match binding {
        Ok(b) => Some(b),
        Err(e) => {
            blockers.push(block(
                LaunchBlockerKind::MgbaBindingUnavailable,
                format!("{:?}: {}", e.kind, e.detail),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return MgbaCommandPlan {
            command: None,
            blockers,
        };
    }
    let r = resolved.expect("resolved when unblocked");
    let b = binding.expect("binding when unblocked");
    MgbaCommandPlan {
        command: Some(MgbaCommand {
            executable: b.executable.clone(),
            arguments: vec![content.clone().into_os_string()],
            working_directory: None,
            selection: MgbaCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: r.platform_id.clone(),
                verified_game_key: r.game_key.clone(),
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
            game_key: "sha".into(),
        })
    }
    fn c(p: &str, a: &'static str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: a,
                profile_id: "p".into(),
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::Cartridge),
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(p.into()),
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
    #[test]
    fn distinct_platform_extensions_and_spaces_are_exact_argv() {
        let b = Ok(MgbaNativeLaunchBinding {
            executable: "/opt/mGBA".into(),
        });
        for (p, e) in [
            ("Game Boy", "/x/Game.gb"),
            ("Game Boy Color", "/x/Game.gbc"),
            ("Game Boy Advance", "/x/Game with spaces.gba"),
        ] {
            let x = build_mgba_command_plan(&id(p), &c(e, "mgba"), &b);
            assert!(x.command.is_some());
            assert_eq!(x.command.unwrap().arguments, vec![OsString::from(e)]);
        }
    }
    #[test]
    fn conflicting_missing_and_wrong_content_fail_closed() {
        let b = Ok(MgbaNativeLaunchBinding {
            executable: "/x/mgba".into(),
        });
        assert!(
            build_mgba_command_plan(
                &CanonicalIdentityStatus::Conflicting,
                &c("/x/a.gb", "mgba"),
                &b
            )
            .command
            .is_none()
        );
        assert!(
            build_mgba_command_plan(&id("Game Boy"), &c("/x/a.gba", "mgba"), &b)
                .command
                .is_none()
        );
        assert!(
            build_mgba_command_plan(&id("Game Boy"), &c("/x/a.gb", "retroarch"), &b)
                .command
                .is_none()
        );
    }
}
