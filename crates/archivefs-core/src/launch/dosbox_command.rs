//! Pure DOSBox / DOSBox Staging launch-command planning for a DOS game
//! directory that carries a verified `dosbox.conf`.
//!
//! # Scope
//!
//! This module plans exactly one, reviewed launch form:
//!
//! ```text
//! <dosbox executable> -conf <verified config path>        (cwd = game directory)
//! ```
//!
//! and nothing else. It does **not**:
//!
//! - execute or interpret anything in the config's `[autoexec]` section -
//!   the commands there (`mount`, `imgmount`, `boot`, `cd`, `game.exe`,
//!   `call`) are never read, parsed, followed, or synthesized;
//! - guess a launch executable from the config;
//! - synthesize `mount` / `imgmount` / `boot` / `cd` arguments of its own;
//! - spawn a process, open a shell, download DOSBox, or write any file.
//!
//! If no verified `dosbox.conf` with a real `[autoexec]` section exists, it
//! fails closed. Identity is consumed as an already-resolved fact
//! ([`CanonicalIdentityStatus`]); a bare Weak MZ signature or an
//! ambiguous / unknown platform never reaches a resolved `DOS` identity and
//! so is never authorized here.
//!
//! # Executables and the `-conf` option - verified, not assumed
//!
//! - **Classic DOSBox** (Debian `dosbox(1)` man page): executable `dosbox`;
//!   `-conf configfile` - "Start dosbox with the options specified in
//!   configfile."
//! - **DOSBox Staging** (official `dosbox(1)` man page, dosbox-staging.org):
//!   synopsis executable `dosbox`; `--conf <configfile>` documented, and
//!   the single-dash `-conf` legacy form is accepted for DOSBox
//!   compatibility. Linux distributions and the `io.github.dosbox-staging`
//!   Flatpak install the binary as `dosbox-staging` so it can coexist with
//!   classic DOSBox.
//!
//! Both variants therefore accept `-conf <file>`, which is the one flag
//! this module ever emits.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::dosbox_config_evidence::{DosboxConfigInspection, inspect_dosbox_config};
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::safe_read::TrustedRoots;

/// The only canonical platform this launch slice serves.
pub const DOSBOX_SUPPORTED_PLATFORM_ID: &str = "DOS";

/// The one command-line flag this module emits, accepted by both classic
/// DOSBox and DOSBox Staging.
pub const DOSBOX_CONFIG_FLAG: &str = "-conf";

/// The `dosbox.conf` file name the DOS layout rule expects (see
/// [`crate::dosbox_config_evidence`] and the DOS platform registry entry).
pub const DOSBOX_CONFIG_FILE_NAME: &str = "dosbox.conf";

/// A supported DOSBox family. DOSBox-X and every other fork fail closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DosBoxVariant {
    /// Classic DOSBox (`dosbox`).
    Classic,
    /// DOSBox Staging (`dosbox-staging`).
    Staging,
}

impl DosBoxVariant {
    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "DOSBox",
            Self::Staging => "DOSBox Staging",
        }
    }

    /// Executable base names to look for, most specific first. Classic
    /// DOSBox is only ever `dosbox`; DOSBox Staging is `dosbox-staging` on
    /// every current Linux packaging (the bare `dosbox` name is not used
    /// for Staging here because it is ambiguous with the classic build).
    pub fn executable_names(self) -> &'static [&'static str] {
        match self {
            Self::Classic => &["dosbox"],
            Self::Staging => &["dosbox-staging"],
        }
    }

    /// The stable adapter/id string for this variant.
    pub fn id(self) -> &'static str {
        match self {
            Self::Classic => "dosbox",
            Self::Staging => "dosbox-staging",
        }
    }
}

/// Maps a variant id / name to a supported [`DosBoxVariant`], or `None`
/// (fail closed) for anything this build does not model - notably
/// `dosbox-x`.
pub fn dosbox_variant_from_id(id: &str) -> Option<DosBoxVariant> {
    match id
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '_'], "-")
        .as_str()
    {
        "dosbox" | "dosbox-classic" | "classic" => Some(DosBoxVariant::Classic),
        "dosbox-staging" | "staging" => Some(DosBoxVariant::Staging),
        _ => None,
    }
}

/// A resolved, safe native DOSBox executable and which variant it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DosBoxNativeLaunchBinding {
    pub executable: PathBuf,
    pub variant: DosBoxVariant,
}

/// Why a DOSBox binding could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DosBoxBindingRefusal {
    /// The requested variant is not one this build supports.
    VariantUnsupported(String),
    /// A supported variant, but no safe executable was found for it.
    ExecutableUnavailable(String),
}

impl DosBoxBindingRefusal {
    pub fn detail(&self) -> String {
        match self {
            Self::VariantUnsupported(id) => {
                format!("`{id}` is not a supported DOSBox variant (only DOSBox and DOSBox Staging)")
            }
            Self::ExecutableUnavailable(detail) => detail.clone(),
        }
    }
}

fn is_safe_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The bounded candidate paths for one variant: the two standard system
/// locations plus every `PATH` component, joined with each executable name.
fn executable_candidates(variant: DosBoxVariant) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for name in variant.executable_names() {
        candidates.push(PathBuf::from("/usr/games").join(name));
        candidates.push(PathBuf::from("/usr/bin").join(name));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for component in std::env::split_paths(&path).take(128) {
            for name in variant.executable_names() {
                candidates.push(component.join(name));
            }
        }
    }
    candidates
}

/// Discovers a native DOSBox executable, preferring DOSBox Staging (the
/// maintained continuation) and falling back to classic DOSBox. Returns a
/// binding only when an actual regular, non-symlink, executable file is
/// found - never from a package or folder name.
pub fn resolve_dosbox_native_launch_binding()
-> Result<DosBoxNativeLaunchBinding, DosBoxBindingRefusal> {
    for variant in [DosBoxVariant::Staging, DosBoxVariant::Classic] {
        if let Some(executable) = executable_candidates(variant)
            .into_iter()
            .find(|candidate| is_safe_executable(candidate))
        {
            return Ok(DosBoxNativeLaunchBinding {
                executable,
                variant,
            });
        }
    }
    Err(DosBoxBindingRefusal::ExecutableUnavailable(
        "no DOSBox or DOSBox Staging executable was found on PATH or in the standard locations"
            .to_string(),
    ))
}

/// Discovers the executable for one explicitly requested variant id. An
/// unrecognised id (e.g. `dosbox-x`) fails closed with
/// [`DosBoxBindingRefusal::VariantUnsupported`].
pub fn resolve_dosbox_native_launch_binding_from_id(
    variant_id: &str,
) -> Result<DosBoxNativeLaunchBinding, DosBoxBindingRefusal> {
    let Some(variant) = dosbox_variant_from_id(variant_id) else {
        return Err(DosBoxBindingRefusal::VariantUnsupported(
            variant_id.to_string(),
        ));
    };
    match executable_candidates(variant)
        .into_iter()
        .find(|candidate| is_safe_executable(candidate))
    {
        Some(executable) => Ok(DosBoxNativeLaunchBinding {
            executable,
            variant,
        }),
        None => Err(DosBoxBindingRefusal::ExecutableUnavailable(format!(
            "no {} executable was found on PATH or in the standard locations",
            variant.label()
        ))),
    }
}

/// Test / caller seam: validates one explicit executable path for a known
/// variant. Rejects symlinks, non-regular files, and non-executable files.
pub fn resolve_dosbox_native_launch_binding_at(
    executable: &Path,
    variant: DosBoxVariant,
) -> Result<DosBoxNativeLaunchBinding, DosBoxBindingRefusal> {
    if !is_safe_executable(executable) {
        return Err(DosBoxBindingRefusal::ExecutableUnavailable(format!(
            "{} is not a regular, non-symlink, executable file",
            executable.display()
        )));
    }
    Ok(DosBoxNativeLaunchBinding {
        executable: executable.to_path_buf(),
        variant,
    })
}

/// The state of the `dosbox.conf` a DOS game directory does or does not
/// carry. Only [`Self::Verified`] authorizes a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DosBoxConfigStatus {
    /// No `dosbox.conf` in the game directory.
    Missing,
    /// A `dosbox.conf` is present but did not parse as a DOSBox config.
    Malformed(String),
    /// A structurally valid DOSBox config, but with no `[autoexec]` section.
    ValidNoAutoexec,
    /// A verified DOSBox config with a real `[autoexec]` section.
    Verified {
        config_path: PathBuf,
        autoexec_command_lines: usize,
    },
}

impl DosBoxConfigStatus {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }
}

/// Pure mapping from an already-run [`DosboxConfigInspection`] to a
/// [`DosBoxConfigStatus`], given the path that was inspected. A seam so the
/// planner never does I/O.
pub fn dosbox_config_status_from_inspection(
    config_path: &Path,
    inspection: &DosboxConfigInspection,
) -> DosBoxConfigStatus {
    match &inspection.fact {
        Some(fact) if fact.is_verified_dos_layout() => DosBoxConfigStatus::Verified {
            config_path: config_path.to_path_buf(),
            autoexec_command_lines: fact.autoexec_command_lines,
        },
        Some(_) => DosBoxConfigStatus::ValidNoAutoexec,
        None => match &inspection.refusal {
            Some(refusal) => DosBoxConfigStatus::Malformed(refusal.detail()),
            None => DosBoxConfigStatus::Malformed("dosbox.conf could not be read".to_string()),
        },
    }
}

/// Looks for `dosbox.conf` (case-insensitively) directly in `game_directory`
/// and classifies it. One bounded `read_dir` plus one bounded config read
/// via [`inspect_dosbox_config`]; never recurses, never executes anything.
pub fn discover_dosbox_config(game_directory: &Path, trusted: &TrustedRoots) -> DosBoxConfigStatus {
    let Ok(read_dir) = std::fs::read_dir(game_directory) else {
        return DosBoxConfigStatus::Missing;
    };
    let mut config_path: Option<PathBuf> = None;
    for entry in read_dir.filter_map(Result::ok).take(4096) {
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(DOSBOX_CONFIG_FILE_NAME)
        {
            config_path = Some(entry.path());
            break;
        }
    }
    let Some(config_path) = config_path else {
        return DosBoxConfigStatus::Missing;
    };
    let inspection = inspect_dosbox_config(&config_path, trusted, None);
    dosbox_config_status_from_inspection(&config_path, &inspection)
}

/// The reviewed DOSBox launch command: `<executable> -conf <config path>`,
/// run with the game directory as the working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DosBoxCommand {
    pub executable: PathBuf,
    /// Each element is a separate argv component; there is no shell and no
    /// concatenated command string. `["-conf", "<config path>"]`.
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: DosBoxCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DosBoxCommandSelection {
    pub platform_id: String,
    pub variant: DosBoxVariant,
    pub game_directory: PathBuf,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DosBoxCommandPlan {
    pub command: Option<DosBoxCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

impl DosBoxCommandPlan {
    /// [`LaunchReadiness::Blocked`] while any blocker stands, else
    /// [`LaunchReadiness::Ready`]. This slice models no non-blocking
    /// warnings.
    pub fn readiness(&self) -> LaunchReadiness {
        if self.blockers.is_empty() {
            LaunchReadiness::Ready
        } else {
            LaunchReadiness::Blocked
        }
    }
}

fn blocked(blockers: Vec<LaunchBlocker>) -> DosBoxCommandPlan {
    debug_assert!(!blockers.is_empty());
    DosBoxCommandPlan {
        command: None,
        blockers,
    }
}

/// Builds the DOSBox launch plan. Pure: every input is an already-
/// established fact and no filesystem or process access happens here.
///
/// Blocks (fails closed) on any of: unresolved / conflicting identity, a
/// resolved identity that is not `DOS`, a non-absolute game directory, a
/// missing / malformed / `[autoexec]`-less `dosbox.conf`, an unavailable
/// DOSBox executable, or an unsupported DOSBox variant.
pub fn build_dosbox_command_plan(
    identity: &CanonicalIdentityStatus,
    game_directory: &Path,
    config: &DosBoxConfigStatus,
    binding: &Result<DosBoxNativeLaunchBinding, DosBoxBindingRefusal>,
) -> DosBoxCommandPlan {
    let mut blockers = Vec::new();

    let resolved: Option<&ResolvedIdentity> = match identity {
        CanonicalIdentityStatus::Resolved(value) => Some(value),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical DOS identity could not be resolved - a bare MZ signature or an \
                 ambiguous platform never authorizes a DOSBox launch",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::IdentityConflict,
                "DOS identity evidence conflicts and was not resolved",
            ));
            None
        }
    };

    if let Some(resolved) = resolved
        && resolved.platform_id != DOSBOX_SUPPORTED_PLATFORM_ID
    {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::DosBoxPlatformMismatch,
            format!(
                "resolved identity targets {}, not {DOSBOX_SUPPORTED_PLATFORM_ID}",
                resolved.platform_id
            ),
        ));
    }

    if !game_directory.is_absolute() {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::DosBoxContentUnsupported,
            "the DOS game directory must be an absolute path",
        ));
    }

    let verified_config_path = match config {
        DosBoxConfigStatus::Verified { config_path, .. } => Some(config_path.clone()),
        DosBoxConfigStatus::Missing => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::DosBoxConfigMissing,
                "no dosbox.conf was found in the DOS game directory",
            ));
            None
        }
        DosBoxConfigStatus::Malformed(detail) => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::DosBoxConfigMalformed,
                format!("dosbox.conf did not parse as a DOSBox configuration: {detail}"),
            ));
            None
        }
        DosBoxConfigStatus::ValidNoAutoexec => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::DosBoxConfigNoAutoexec,
                "dosbox.conf is a valid DOSBox config but has no [autoexec] section, so there is \
                 no reviewed configuration to launch with",
            ));
            None
        }
    };

    let binding = match binding {
        Ok(value) => Some(value),
        Err(DosBoxBindingRefusal::VariantUnsupported(id)) => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::DosBoxVariantUnsupported,
                DosBoxBindingRefusal::VariantUnsupported(id.clone()).detail(),
            ));
            None
        }
        Err(DosBoxBindingRefusal::ExecutableUnavailable(detail)) => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::DosBoxBindingUnavailable,
                detail.clone(),
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return blocked(blockers);
    }

    let resolved = resolved.expect("resolved identity when unblocked");
    let config_path = verified_config_path.expect("verified config path when unblocked");
    let binding = binding.expect("binding when unblocked");

    DosBoxCommandPlan {
        command: Some(DosBoxCommand {
            executable: binding.executable.clone(),
            arguments: vec![
                OsString::from(DOSBOX_CONFIG_FLAG),
                config_path.as_os_str().to_os_string(),
            ],
            working_directory: Some(game_directory.to_path_buf()),
            selection: DosBoxCommandSelection {
                platform_id: resolved.platform_id.clone(),
                variant: binding.variant,
                game_directory: game_directory.to_path_buf(),
                config_path,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
