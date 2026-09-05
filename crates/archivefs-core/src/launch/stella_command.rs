//! Read-only native Stella command planning for direct Atari 2600
//! cartridge dumps.
//!
//! # Scope
//!
//! Only `Atari2600` platform content, a direct loose `.a26` file (the one
//! strong/canonical Atari 2600 extension in
//! `crate::platform::PLATFORMS` - see this module's own
//! `STELLA_SUPPORTED_EXTENSIONS` doc comment for why weaker/shared
//! extensions and archive containers are refused), and an exact eligible
//! [`StellaNativeLaunchBinding`]. Mounted/archive content and any
//! installation type other than [`StellaInstallationType`]'s
//! `Native`/`Explicit` are refused here, never silently widened.
//!
//! # Exact argv contract, and why there is no `--` separator
//!
//! `[stella, content]`. Unlike RMG (whose Qt `QCommandLineParser` needs an
//! explicit `--` end-of-options marker before a positional argument that
//! might start with `-`), Stella's own command-line usage is documented
//! upstream (stella-emu.github.io / `github.com/stella-emu/stella`) as
//! `stella [options] romfile` - a plain positional ROM-file argument with
//! no required flags for a normal launch, and no documented end-of-options
//! marker in Stella's own argument parser. So this module never invents
//! one: the exact selected ROM path is passed as the sole positional
//! argument, immediately after the executable, with nothing else - no
//! guessed flags to suppress Stella's own first-run UI or otherwise alter
//! its behavior (see `crate::patch_manager::stella_local`'s own module doc
//! comment for why this crate never configures Stella at all).
//!
//! Every argument is carried as its own `OsString` - spaces, quotes, and
//! shell-looking characters in a path are inert data, never shell syntax.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{StellaLaunchBlocker, StellaNativeLaunchBinding};

/// The only platform this native launch slice supports.
pub const STELLA_SUPPORTED_PLATFORM_ID: &str = "Atari2600";

/// The only direct content extension this slice supports (lowercase, no
/// dot) - matches `crate::platform::PLATFORMS`'s Atari 2600
/// `strong_extensions` exactly (`&["a26"]`). The registry's Atari 2600
/// `weak_extensions` (`"bin"`, `"rom"`, `"zip"`) are deliberately never
/// accepted here: `.bin`/`.rom` are shared with most other cartridge
/// systems (the registry's own `explanation` field says as much) and
/// `.zip` is an archive/mount-input container, not direct content - Stella
/// being technically able to open more formats is not evidence this build
/// has reviewed any of them as safe direct content.
const STELLA_SUPPORTED_EXTENSIONS: &[&str] = &["a26"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StellaCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: StellaCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StellaCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub game_key: String,
    pub content_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StellaCommandPlan {
    pub command: Option<StellaCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

/// Whether `path` has the exact direct Atari 2600 cartridge extension this
/// native launch slice supports.
pub(crate) fn direct_atari2600_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            STELLA_SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

pub fn build_stella_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<StellaNativeLaunchBinding, StellaLaunchBlocker>,
) -> StellaCommandPlan {
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
        && resolved.platform_id != STELLA_SUPPORTED_PLATFORM_ID
    {
        blockers.push(block(
            LaunchBlockerKind::StellaPlatformMismatch,
            format!(
                "resolved identity targets {}, but only {STELLA_SUPPORTED_PLATFORM_ID} is \
                 supported by this native Stella launch slice",
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
            LaunchBlockerKind::StellaCandidateRequired,
            "the supplied launch candidate does not target a standalone adapter",
        ));
        return StellaCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "stella" {
        blockers.push(block(
            LaunchBlockerKind::StellaCandidateRequired,
            format!("the supplied launch candidate targets adapter `{adapter_id}`, not `stella`"),
        ));
    }

    if candidate.readiness == crate::launch::readiness::LaunchReadiness::Blocked
        || !candidate.blockers.is_empty()
    {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(block(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied Stella launch candidate is marked blocked",
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
            || !direct_atari2600_extension(path))
    {
        blockers.push(block(
            LaunchBlockerKind::StellaContentFormatUnsupported,
            "only a direct .a26 cartridge file is supported by this native Stella launch slice",
        ));
    }

    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(block(
                LaunchBlockerKind::StellaBindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return StellaCommandPlan {
            command: None,
            blockers,
        };
    }

    let resolved = resolved.expect("identity is Resolved when no blockers exist");
    let content_path =
        content_path.expect("a resolved content path is required when no blockers exist");
    let binding = binding.expect("a launch binding is required when no blockers exist");

    let arguments = vec![content_path.clone().into_os_string()];

    StellaCommandPlan {
        command: Some(StellaCommand {
            executable: binding.executable.clone(),
            arguments,
            working_directory: None,
            selection: StellaCommandSelection {
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
            game_key: "a26sha".into(),
        })
    }

    fn candidate(path: &'static str, adapter_id: &'static str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id,
                profile_id: "stella:native".into(),
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

    fn binding() -> Result<StellaNativeLaunchBinding, StellaLaunchBlocker> {
        Ok(StellaNativeLaunchBinding {
            executable: "/usr/bin/stella".into(),
        })
    }

    #[test]
    fn exact_argv_for_supported_extension_no_separator() {
        let path = "/roms/Pitfall.a26";
        let plan =
            build_stella_command_plan(&id("Atari2600"), &candidate(path, "stella"), &binding());
        let command = plan.command.expect("command");
        assert_eq!(command.arguments, vec![OsString::from(path)]);
        assert_eq!(command.executable, PathBuf::from("/usr/bin/stella"));
    }

    #[test]
    fn exact_selected_rom_path_is_preserved_verbatim() {
        let path = "/roms/Adventure (USA).a26";
        let plan =
            build_stella_command_plan(&id("Atari2600"), &candidate(path, "stella"), &binding());
        let command = plan.command.expect("command");
        assert_eq!(command.selection.content_path, PathBuf::from(path));
        assert_eq!(command.arguments[0], OsString::from(path));
    }

    #[test]
    fn no_shell_wrapping_every_argument_is_its_own_os_string() {
        let path = "/roms/weird`; rm -rf ~`.a26";
        let plan =
            build_stella_command_plan(&id("Atari2600"), &candidate(path, "stella"), &binding());
        let command = plan.command.expect("command");
        assert_eq!(command.arguments.last().unwrap(), &OsString::from(path));
        assert_eq!(command.arguments.len(), 1);
    }

    #[test]
    fn non_atari2600_platform_is_refused() {
        let plan = build_stella_command_plan(
            &id("Atari5200"),
            &candidate("/roms/game.a26", "stella"),
            &binding(),
        );
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::StellaPlatformMismatch)
        );
    }

    #[test]
    fn unsupported_content_extension_is_refused() {
        for path in ["/roms/game.bin", "/roms/game.rom", "/roms/game.zip"] {
            let plan =
                build_stella_command_plan(&id("Atari2600"), &candidate(path, "stella"), &binding());
            assert!(plan.command.is_none(), "{path}");
            assert!(
                plan.blockers
                    .iter()
                    .any(|b| b.kind == LaunchBlockerKind::StellaContentFormatUnsupported),
                "{path}"
            );
        }
    }

    #[test]
    fn wrong_adapter_candidate_is_refused_no_fallback() {
        let plan = build_stella_command_plan(
            &id("Atari2600"),
            &candidate("/roms/game.a26", "retroarch"),
            &binding(),
        );
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::StellaCandidateRequired)
        );
    }

    #[test]
    fn missing_executable_binding_is_refused() {
        let broken_binding: Result<StellaNativeLaunchBinding, StellaLaunchBlocker> =
            Err(StellaLaunchBlocker {
                kind: crate::patch_manager::StellaLaunchBlockerKind::ExecutableMissing,
                detail: "no safe executable matches this profile".into(),
            });
        let plan = build_stella_command_plan(
            &id("Atari2600"),
            &candidate("/roms/game.a26", "stella"),
            &broken_binding,
        );
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::StellaBindingUnavailable)
        );
    }

    #[test]
    fn conflicting_identity_is_refused() {
        let plan = build_stella_command_plan(
            &CanonicalIdentityStatus::Conflicting,
            &candidate("/roms/game.a26", "stella"),
            &binding(),
        );
        assert!(plan.command.is_none());
    }
}
