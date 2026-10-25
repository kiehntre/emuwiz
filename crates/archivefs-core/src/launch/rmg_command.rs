//! Read-only native RMG (Rosalie's Mupen GUI) command planning for direct
//! Nintendo 64 cartridge dumps.
//!
//! # Scope
//!
//! Only `N64` platform content, a direct loose `.z64`/`.n64`/`.v64` file
//! (never a `.ndd` 64DD disk image - RMG's `--disk` option pairs a disk with
//! a ROM and is a separate, unverified content shape this build does not
//! model), and an exact eligible [`RmgNativeLaunchBinding`]. Mounted/archive
//! content and any installation type other than [`RmgInstallationType`]'s
//! `Native`/`Explicit` are refused here, never silently widened.
//!
//! # Exact argv contract, and why `--quit-after-emulation`
//!
//! `[RMG] --quit-after-emulation -- [content]`. Proven from upstream
//! `Rosalie241/RMG`'s `Source/RMG/main.cpp`: a `QCommandLineOption`
//! `{"q", "quit-after-emulation"}` documented as "Quits RMG when emulation
//! has finished". Without it, closing a game leaves RMG's own GUI window
//! open rather than exiting the process, so a watcher modeled on "the
//! process exits when the play session ends" (the same model
//! RetroArch/Dolphin/PCSX2/DuckStation already use - see
//! [`crate::launch::duckstation_command`]'s own module doc comment for the
//! identical reasoning with `-batch`) would never observe a normal exit.
//! `--quit-after-emulation` does not change anything about the emulated game
//! itself while it is running, only what the frontend does after it closes,
//! so this is a safe, non-behavior-altering choice for the watcher's
//! benefit. `--` is Qt's own built-in `QCommandLineParser` end-of-options
//! marker (no special handling required in RMG's own argument-parsing code -
//! see the module's upstream citation) and is always included so a future
//! content path that happens to start with `-` is still parsed as the ROM
//! positional argument, never as a flag.
//!
//! Every argument is carried as its own `OsString` - spaces, quotes, and
//! shell-looking characters in a path are inert data, never shell syntax.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{RmgLaunchBlocker, RmgNativeLaunchBinding};

/// The only platform this native launch slice supports.
pub const RMG_SUPPORTED_PLATFORM_ID: &str = "N64";

/// The only direct content extensions this slice supports (lowercase, no
/// dot) - matches `crate::platform::PLATFORMS`'s N64
/// `strong_extensions` minus `.ndd` (a 64DD disk image, which RMG only
/// opens paired with a cartridge ROM via `--disk` - a separate, unverified
/// content shape this build does not model; see the module doc comment).
const RMG_SUPPORTED_EXTENSIONS: &[&str] = &["z64", "n64", "v64"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: RmgCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub game_key: String,
    pub content_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgCommandPlan {
    pub command: Option<RmgCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

/// Whether `path` has one of the exact direct N64 cartridge extensions this
/// native launch slice supports.
pub(crate) fn direct_n64_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            RMG_SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

pub fn build_rmg_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<RmgNativeLaunchBinding, RmgLaunchBlocker>,
) -> RmgCommandPlan {
    let mut blockers = Vec::new();

    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(resolved) => Some(resolved),
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
                "canonical game identity evidence conflicts and was not resolved to one answer",
            ));
            None
        }
    };
    if let Some(resolved) = resolved
        && resolved.platform_id != RMG_SUPPORTED_PLATFORM_ID
    {
        blockers.push(block(
            LaunchBlockerKind::RmgPlatformMismatch,
            format!(
                "resolved identity targets {}, but only {RMG_SUPPORTED_PLATFORM_ID} is supported \
                 by this native RMG launch slice",
                resolved.platform_id
            ),
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(block(
            LaunchBlockerKind::RmgCandidateRequired,
            "the supplied launch candidate does not target a standalone adapter",
        ));
        return RmgCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "rmg" {
        blockers.push(block(
            LaunchBlockerKind::RmgCandidateRequired,
            format!("the supplied launch candidate targets adapter `{adapter_id}`, not `rmg`"),
        ));
    }

    if candidate.readiness == crate::launch::readiness::LaunchReadiness::Blocked
        || !candidate.blockers.is_empty()
    {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(block(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied RMG launch candidate is marked blocked",
            ));
        }
    }

    let content_path = match candidate
        .content
        .resolved_path
        .as_ref()
        .filter(|_| !candidate.content.requires_mount)
    {
        Some(path) => Some(path.clone()),
        None => {
            blockers.push(block(
                LaunchBlockerKind::ContentNotResolved,
                "no resolved runnable game/content path is available",
            ));
            None
        }
    };
    if let Some(path) = &content_path
        && (crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input())
            || !direct_n64_extension(path))
    {
        blockers.push(block(
            LaunchBlockerKind::RmgContentFormatUnsupported,
            "only a direct .z64, .n64, or .v64 cartridge file is supported by this native RMG \
             launch slice",
        ));
    }

    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(block(
                LaunchBlockerKind::RmgBindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return RmgCommandPlan {
            command: None,
            blockers,
        };
    }

    let resolved = resolved.expect("identity is Resolved when no blockers exist");
    let content_path =
        content_path.expect("a resolved content path is required when no blockers exist");
    let binding = binding.expect("a launch binding is required when no blockers exist");

    let arguments = vec![
        OsString::from("--quit-after-emulation"),
        OsString::from("--"),
        content_path.clone().into_os_string(),
    ];

    RmgCommandPlan {
        command: Some(RmgCommand {
            executable: binding.executable.clone(),
            arguments,
            working_directory: None,
            selection: RmgCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: resolved.platform_id.clone(),
                game_key: resolved.game_key.clone(),
                content_path,
            },
        }),
        blockers: Vec::new(),
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
            game_key: "z64sha".into(),
        })
    }

    fn candidate(path: &'static str, adapter_id: &'static str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id,
                profile_id: "rmg:native".into(),
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

    fn binding() -> Result<RmgNativeLaunchBinding, RmgLaunchBlocker> {
        Ok(RmgNativeLaunchBinding {
            executable: "/usr/bin/RMG".into(),
        })
    }

    #[test]
    fn exact_argv_for_supported_extensions() {
        for path in ["/roms/Mario64.z64", "/roms/game.n64", "/roms/game.v64"] {
            let plan = build_rmg_command_plan(&id("N64"), &candidate(path, "rmg"), &binding());
            let command = plan.command.expect("command");
            assert_eq!(
                command.arguments,
                vec![
                    OsString::from("--quit-after-emulation"),
                    OsString::from("--"),
                    OsString::from(path),
                ]
            );
            assert_eq!(command.executable, PathBuf::from("/usr/bin/RMG"));
        }
    }

    #[test]
    fn exact_selected_rom_path_is_preserved_verbatim() {
        let path = "/roms/Legend of Zelda - Ocarina of Time (USA).z64";
        let plan = build_rmg_command_plan(&id("N64"), &candidate(path, "rmg"), &binding());
        let command = plan.command.expect("command");
        assert_eq!(command.selection.content_path, PathBuf::from(path));
        assert_eq!(command.arguments[2], OsString::from(path));
    }

    #[test]
    fn no_shell_wrapping_every_argument_is_its_own_os_string() {
        let path = "/roms/weird`; rm -rf ~`.z64";
        let plan = build_rmg_command_plan(&id("N64"), &candidate(path, "rmg"), &binding());
        let command = plan.command.expect("command");
        // The dangerous-looking text is carried as one inert OsString, never
        // concatenated into anything resembling a shell command line.
        assert_eq!(command.arguments.last().unwrap(), &OsString::from(path));
        assert_eq!(command.arguments.len(), 3);
    }

    #[test]
    fn non_n64_platform_is_refused() {
        let plan =
            build_rmg_command_plan(&id("PSX"), &candidate("/roms/game.z64", "rmg"), &binding());
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::RmgPlatformMismatch)
        );
    }

    #[test]
    fn unsupported_content_extension_is_refused() {
        let plan =
            build_rmg_command_plan(&id("N64"), &candidate("/roms/game.ndd", "rmg"), &binding());
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::RmgContentFormatUnsupported)
        );
    }

    #[test]
    fn wrong_adapter_candidate_is_refused_no_fallback() {
        let plan = build_rmg_command_plan(
            &id("N64"),
            &candidate("/roms/game.z64", "retroarch"),
            &binding(),
        );
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::RmgCandidateRequired)
        );
    }

    #[test]
    fn missing_executable_binding_is_refused() {
        let broken_binding: Result<RmgNativeLaunchBinding, RmgLaunchBlocker> =
            Err(RmgLaunchBlocker {
                kind: crate::patch_manager::RmgLaunchBlockerKind::ExecutableMissing,
                detail: "no safe executable matches this profile".into(),
            });
        let plan = build_rmg_command_plan(
            &id("N64"),
            &candidate("/roms/game.z64", "rmg"),
            &broken_binding,
        );
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::RmgBindingUnavailable)
        );
    }

    #[test]
    fn conflicting_identity_is_refused() {
        let plan = build_rmg_command_plan(
            &CanonicalIdentityStatus::Conflicting,
            &candidate("/roms/game.z64", "rmg"),
            &binding(),
        );
        assert!(plan.command.is_none());
    }
}
