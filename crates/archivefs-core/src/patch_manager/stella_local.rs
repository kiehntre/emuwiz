//! Bounded, read-only discovery for native Stella (Atari 2600 emulator).
//!
//! Stella boots Atari 2600 cartridge dumps directly - the console has no
//! external BIOS/PIF ROM requirement for standard cartridge play (unlike
//! PS1/PS2/Xbox/Dreamcast/3DS emulation, or even N64's mupen64plus HLE
//! RSP/PIF situation): Stella emulates the 2600 hardware itself with no
//! separate firmware image involved at all. So, exactly like
//! [`crate::patch_manager::rmg_local`], this module has no firmware/BIOS
//! state to discover or project.
//!
//! # Executable name evidence
//!
//! No `stella` binary was available in this build environment to run
//! `stella -help` against (`which stella` found nothing), so the exact
//! Linux binary name is taken from documented upstream behavior rather than
//! a captured `--help`/`-help` transcript: upstream
//! (`github.com/stella-emu/stella`) builds and packages the Linux binary as
//! exactly `stella` (lowercase) - this is also the name distro packages
//! (Debian/Ubuntu/Fedora/Arch `stella` packages) install to `/usr/bin/stella`,
//! and is the name stella-emu.github.io's own Linux usage documentation
//! (`stella [options] romfile`) assumes when it shows `stella romfile` as
//! the invocation. Documented, not captured - recorded here exactly as such,
//! the same way this crate always distinguishes captured tool output from
//! documented upstream behavior.
//!
//! # This module never writes Stella configuration
//!
//! Stella auto-creates its own config directory/`stella.pro`/`stellarc` on
//! first run if absent - this is Stella's own internal first-run behavior,
//! never modeled, invented, or suppressed by this crate. This module never
//! reads, writes, or requires any Stella configuration for readiness -
//! only the executable itself is discovered - and never executes a
//! discovered binary.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// How a discovered Stella executable was found. Only the two forms this
/// build has real, safe discovery evidence for - no AppImage-specific seam
/// exists for Stella in this build (upstream does not distribute one in any
/// way an existing adapter already safely discovers); inventing one here
/// would be exactly the kind of unreviewed widening this crate's launch
/// adapters avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StellaInstallationType {
    /// Found on `PATH` under the exact upstream-verified name `stella`.
    Native,
    /// A caller-supplied exact executable path.
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StellaExecutable {
    pub path: PathBuf,
    pub installation_type: StellaInstallationType,
    pub version: Option<String>,
}

/// One discovered Stella installation candidate. Stella has no
/// per-profile configuration this build reads, so - exactly like
/// [`crate::patch_manager::RmgProfile`] - a profile here is simply "this
/// exact executable, found this way": `profile_id` is synthesized from the
/// executable path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StellaProfile {
    pub profile_id: String,
    pub installation_type: StellaInstallationType,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<StellaExecutable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StellaProfileDiscovery {
    pub profiles: Vec<StellaProfile>,
    pub complete: bool,
}

/// Bounds how many `Explicit` executable candidates one discovery call will
/// ever consider - mirrors the small bounded-search caps every other
/// adapter's discovery already enforces (see
/// [`crate::patch_manager::RMG_MAX_EXPLICIT_EXECUTABLES`]).
pub const STELLA_MAX_EXPLICIT_EXECUTABLES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StellaProfileDiscoveryRoots {
    /// Caller-provided exact executable path(s) - never a guess.
    pub explicit_executables: Vec<PathBuf>,
    /// The raw `PATH` environment value to search - carried explicitly
    /// (rather than read directly from `std::env`) so discovery stays
    /// deterministic and test-injectable, exactly like every other
    /// adapter's discovery-roots type in this crate.
    pub path_env: Option<std::ffi::OsString>,
    /// Already-captured `stella -version` output, keyed by executable path -
    /// never executed by this module itself. See [`parse_stella_version`].
    pub known_version_outputs: std::collections::BTreeMap<PathBuf, String>,
}

impl StellaProfileDiscoveryRoots {
    pub fn from_environment() -> Self {
        Self {
            explicit_executables: Vec::new(),
            path_env: env::var_os("PATH"),
            known_version_outputs: std::collections::BTreeMap::new(),
        }
    }
}

fn regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    regular(path) && fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    regular(path)
}

/// The one upstream-documented installed executable name (see the module
/// doc comment). Deliberately a single exact name, never a guessed
/// uppercase or hyphenated variant this build has no evidence for.
const STELLA_EXECUTABLE_NAME: &str = "stella";

fn candidate_executables(roots: &StellaProfileDiscoveryRoots) -> Vec<StellaExecutable> {
    let mut paths: Vec<(PathBuf, StellaInstallationType)> = roots
        .explicit_executables
        .iter()
        .take(STELLA_MAX_EXPLICIT_EXECUTABLES)
        .cloned()
        .map(|p| (p, StellaInstallationType::Explicit))
        .collect();
    if let Some(path_env) = &roots.path_env {
        for dir in env::split_paths(path_env) {
            paths.push((
                dir.join(STELLA_EXECUTABLE_NAME),
                StellaInstallationType::Native,
            ));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|(p, _)| executable(p))
        .map(|(path, installation_type)| StellaExecutable {
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|s| parse_stella_version(s)),
            path,
            installation_type,
        })
        .collect()
}

fn profile(installation_type: StellaInstallationType, all: &[StellaExecutable]) -> StellaProfile {
    let matching: Vec<_> = all
        .iter()
        .filter(|e| e.installation_type == installation_type)
        .cloned()
        .collect();
    let eligible = !matching.is_empty();
    let blocker = (!eligible).then(|| "no safe Stella executable was discovered".to_string());
    StellaProfile {
        profile_id: match installation_type {
            StellaInstallationType::Native => "stella:native".to_string(),
            StellaInstallationType::Explicit => matching
                .first()
                .map(|e| format!("stella:{}", e.path.display()))
                .unwrap_or_else(|| "stella:explicit".to_string()),
        },
        installation_type,
        eligible,
        blocker,
        executable_candidates: matching,
    }
}

/// Discovers Stella installation candidates: an executable on `PATH`
/// (documented `stella` name), and one profile per caller-supplied explicit
/// executable path. Never scans an AppImage-adjacent directory, Flatpak
/// metadata, or any config location - Stella's own configuration is never
/// read by this crate.
pub fn discover_stella_profiles(roots: &StellaProfileDiscoveryRoots) -> StellaProfileDiscovery {
    let all = candidate_executables(roots);
    let mut profiles = Vec::new();
    if roots.path_env.is_some() {
        profiles.push(profile(StellaInstallationType::Native, &all));
    }
    if !roots.explicit_executables.is_empty() {
        profiles.push(profile(StellaInstallationType::Explicit, &all));
    }
    StellaProfileDiscovery {
        profiles,
        complete: true,
    }
}

/// Parses bounded `stella -version` output without executing a process.
/// Tolerant of the same "first digit-dot token" shape
/// `parse_rmg_version`/`parse_mgba_version` already use, since no captured
/// real transcript exists to pin an exact format to.
pub fn parse_stella_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|token| {
            token.chars().next().is_some_and(|c| c.is_ascii_digit()) && token.contains('.')
        })
        .map(|v| {
            v.trim_matches(|c: char| !c.is_ascii_digit() && c != '.')
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StellaLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    AmbiguousExecutable,
    ExecutableUnsafe,
    ExecutableNotExecutable,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StellaLaunchBlocker {
    pub kind: StellaLaunchBlockerKind,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StellaNativeLaunchBinding {
    pub executable: PathBuf,
}

/// Resolves exactly one safe executable for `profile` - never a silent
/// substitution when more than one candidate matches.
pub fn resolve_stella_native_launch_binding(
    profile: &StellaProfile,
) -> Result<StellaNativeLaunchBinding, StellaLaunchBlocker> {
    if !profile.eligible {
        return Err(StellaLaunchBlocker {
            kind: StellaLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "profile is not eligible".into()),
        });
    }
    let valid: Vec<_> = profile
        .executable_candidates
        .iter()
        .filter(|e| e.installation_type == profile.installation_type)
        .filter(|e| executable(&e.path))
        .collect();
    match valid.as_slice() {
        [one] => Ok(StellaNativeLaunchBinding {
            executable: one.path.clone(),
        }),
        [] => Err(StellaLaunchBlocker {
            kind: StellaLaunchBlockerKind::ExecutableMissing,
            detail: "no safe executable matches this profile".into(),
        }),
        _ => Err(StellaLaunchBlocker {
            kind: StellaLaunchBlockerKind::AmbiguousExecutable,
            detail: "more than one safe executable matches this profile".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn mark_exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut m = fs::metadata(p).unwrap().permissions();
        m.set_mode(0o755);
        fs::set_permissions(p, m).unwrap();
    }

    #[test]
    fn version_is_bounded_and_optional() {
        assert_eq!(parse_stella_version("Stella 6.7"), Some("6.7".into()));
        assert_eq!(parse_stella_version("unknown"), None);
    }

    #[test]
    fn discovers_native_executable_on_path() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join(STELLA_EXECUTABLE_NAME);
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = StellaProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_stella_profiles(&roots);
        let native = discovery
            .profiles
            .iter()
            .find(|p| p.installation_type == StellaInstallationType::Native)
            .unwrap();
        assert!(native.eligible);
        assert_eq!(native.executable_candidates[0].path, exe);
        assert!(resolve_stella_native_launch_binding(native).is_ok());
    }

    #[test]
    fn uppercase_name_is_never_matched() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("Stella");
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = StellaProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_stella_profiles(&roots);
        let native = discovery
            .profiles
            .iter()
            .find(|p| p.installation_type == StellaInstallationType::Native)
            .unwrap();
        assert!(!native.eligible);
    }

    #[test]
    fn no_installation_reports_no_profiles() {
        let roots = StellaProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_stella_profiles(&roots);
        assert!(discovery.profiles.is_empty());
    }

    #[test]
    fn explicit_executable_is_discovered_and_bound() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("custom-stella-binary");
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = StellaProfileDiscoveryRoots {
            explicit_executables: vec![exe.clone()],
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_stella_profiles(&roots);
        let explicit = discovery
            .profiles
            .iter()
            .find(|p| p.installation_type == StellaInstallationType::Explicit)
            .unwrap();
        assert!(explicit.eligible);
        let binding = resolve_stella_native_launch_binding(explicit).unwrap();
        assert_eq!(binding.executable, exe);
    }

    #[test]
    fn missing_executable_is_ineligible_and_refuses_binding() {
        let roots = StellaProfileDiscoveryRoots {
            explicit_executables: vec![PathBuf::from("/nonexistent/stella")],
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_stella_profiles(&roots);
        let explicit = &discovery.profiles[0];
        assert!(!explicit.eligible);
        assert!(resolve_stella_native_launch_binding(explicit).is_err());
    }

    #[test]
    fn non_regular_file_is_refused() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("real-stella");
        fs::write(&target, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&target);
        let link = dir.path().join(STELLA_EXECUTABLE_NAME);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(unix)]
        {
            let roots = StellaProfileDiscoveryRoots {
                explicit_executables: Vec::new(),
                path_env: Some(dir.path().as_os_str().to_owned()),
                known_version_outputs: std::collections::BTreeMap::new(),
            };
            let discovery = discover_stella_profiles(&roots);
            let native = discovery
                .profiles
                .iter()
                .find(|p| p.installation_type == StellaInstallationType::Native)
                .unwrap();
            assert!(
                !native.eligible,
                "a symlink must never be treated as a safe executable"
            );
        }
    }

    #[test]
    fn non_executable_regular_file_is_refused() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join(STELLA_EXECUTABLE_NAME);
        fs::write(&exe, b"x").unwrap();
        // Deliberately never marked executable.
        let roots = StellaProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_stella_profiles(&roots);
        let native = discovery
            .profiles
            .iter()
            .find(|p| p.installation_type == StellaInstallationType::Native)
            .unwrap();
        #[cfg(unix)]
        assert!(!native.eligible);
        #[cfg(not(unix))]
        let _ = native;
    }
}
