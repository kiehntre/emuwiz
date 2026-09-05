//! Read-only native Snes9x command planning for direct `.sfc`/`.smc` files.
//!
//! Argv is exactly `<snes9x executable> <rom path>` - one bare, non-dash
//! path and nothing else.  Verified against `snes9xgit/snes9x` `snes9x.cpp`:
//! `S9xParseArgs` records every argv entry that does not begin with `-` as
//! the ROM filename, and `S9xUsage` prints
//! `usage: snes9x [options] <ROM image filename>`.  The GTK port's `main`
//! (`gtk/src/gtk_s9x.cpp`) uses that same parser then `S9xOpenROM`.  No
//! option flag (`-fullscreen`, `-hirom`, `-conf`, ...) is ever added: the
//! adapter never configures the emulator and never rewrites its own argv.

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{Snes9xLaunchBlocker, Snes9xNativeLaunchBinding};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The one canonical platform this adapter serves.  Deliberately not
/// broadened to NES, Satellaview, or any other system Snes9x can technically
/// touch - EmuWiz's SNES identity is `.sfc`/`.smc` SNES/SFC cartridges only.
pub const SNES9X_SUPPORTED_PLATFORM_IDS: &[&str] = &["SNES"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: Snes9xCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_game_key: String,
    pub content_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xCommandPlan {
    pub command: Option<Snes9xCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

/// The only two content forms EmuWiz's SNES platform model treats as
/// canonical (`Platform { id: "SNES", strong_extensions: &["sfc", "smc"] }`).
/// The weak, ambiguous extensions (`.bin`/`.rom`/`.zip`/`.fig`/`.swc`) are
/// deliberately not accepted here even though Snes9x itself can open some of
/// them.
pub(crate) fn direct_snes9x_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("sfc") || e.eq_ignore_ascii_case("smc"))
}

pub fn build_snes9x_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<Snes9xNativeLaunchBinding, Snes9xLaunchBlocker>,
) -> Snes9xCommandPlan {
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
        && !SNES9X_SUPPORTED_PLATFORM_IDS.contains(&r.platform_id.as_str())
    {
        blockers.push(block(
            LaunchBlockerKind::Snes9xPlatformMismatch,
            "resolved platform is not SNES",
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(block(
            LaunchBlockerKind::Snes9xCandidateRequired,
            "candidate is not a standalone Snes9x target",
        ));
        return Snes9xCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "snes9x" {
        blockers.push(block(
            LaunchBlockerKind::Snes9xCandidateRequired,
            format!("candidate targets `{adapter_id}`, not `snes9x`"),
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
        return Snes9xCommandPlan {
            command: None,
            blockers,
        };
    };
    if crate::archive_kind(content).is_some_and(|k| k.is_mount_input())
        || !direct_snes9x_extension(content)
    {
        blockers.push(block(
            LaunchBlockerKind::Snes9xContentFormatUnsupported,
            "native Snes9x accepts only a direct, non-archived `.sfc` or `.smc` file",
        ));
    }

    let binding = match binding {
        Ok(b) => Some(b),
        Err(e) => {
            blockers.push(block(
                LaunchBlockerKind::Snes9xBindingUnavailable,
                format!("{:?}: {}", e.kind, e.detail),
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return Snes9xCommandPlan {
            command: None,
            blockers,
        };
    }
    let r = resolved.expect("resolved when unblocked");
    let b = binding.expect("binding when unblocked");
    Snes9xCommandPlan {
        command: Some(Snes9xCommand {
            executable: b.executable.clone(),
            arguments: vec![content.clone().into_os_string()],
            working_directory: None,
            selection: Snes9xCommandSelection {
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

    fn id(platform: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: "sha".into(),
        })
    }

    fn candidate(path: &str, adapter: &'static str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: adapter,
                profile_id: "snes9x:/opt/snes9x-gtk".into(),
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

    fn binding() -> Result<Snes9xNativeLaunchBinding, Snes9xLaunchBlocker> {
        Ok(Snes9xNativeLaunchBinding {
            executable: "/opt/snes9x-gtk".into(),
        })
    }

    #[test]
    fn canonical_snes_content_is_exact_single_argv() {
        for path in ["/roms/Chrono Trigger.sfc", "/roms/Super Metroid.smc"] {
            let plan =
                build_snes9x_command_plan(&id("SNES"), &candidate(path, "snes9x"), &binding());
            let command = plan.command.expect("command");
            assert_eq!(command.executable, PathBuf::from("/opt/snes9x-gtk"));
            assert_eq!(command.arguments, vec![OsString::from(path)]);
            assert_eq!(command.arguments.len(), 1);
            assert_eq!(command.working_directory, None);
            assert_eq!(command.selection.content_path, PathBuf::from(path));
        }
    }

    #[test]
    fn non_snes_platform_is_refused() {
        let plan = build_snes9x_command_plan(
            &id("NES"),
            &candidate("/roms/game.sfc", "snes9x"),
            &binding(),
        );
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::Snes9xPlatformMismatch)
        );
    }

    #[test]
    fn unsupported_extension_is_refused() {
        for path in ["/roms/game.zip", "/roms/game.bin", "/roms/game.swc"] {
            let plan =
                build_snes9x_command_plan(&id("SNES"), &candidate(path, "snes9x"), &binding());
            assert!(plan.command.is_none(), "{path}");
            assert!(
                plan.blockers
                    .iter()
                    .any(|b| b.kind == LaunchBlockerKind::Snes9xContentFormatUnsupported),
                "{path}"
            );
        }
    }

    #[test]
    fn a_retroarch_candidate_is_refused_no_fallback() {
        let plan = build_snes9x_command_plan(
            &id("SNES"),
            &candidate("/roms/game.sfc", "retroarch"),
            &binding(),
        );
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::Snes9xCandidateRequired)
        );
    }

    #[test]
    fn missing_binding_fails_closed() {
        let err: Result<Snes9xNativeLaunchBinding, Snes9xLaunchBlocker> =
            Err(Snes9xLaunchBlocker {
                kind: crate::patch_manager::Snes9xLaunchBlockerKind::ExecutableMissing,
                detail: "gone".into(),
            });
        let plan =
            build_snes9x_command_plan(&id("SNES"), &candidate("/roms/game.sfc", "snes9x"), &err);
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::Snes9xBindingUnavailable)
        );
    }

    #[test]
    fn conflicting_identity_fails_closed() {
        let plan = build_snes9x_command_plan(
            &CanonicalIdentityStatus::Conflicting,
            &candidate("/roms/game.sfc", "snes9x"),
            &binding(),
        );
        assert!(plan.command.is_none());
    }
}
