//! Bounded, read-only discovery for native RMG (Rosalie's Mupen GUI).
//!
//! RMG is a mupen64plus front-end that boots Nintendo 64 cartridge dumps
//! directly - it needs no external BIOS/PIF ROM the way PS1/PS2/Xbox/
//! Dreamcast/3DS emulation does (mupen64plus's HLE RSP/PIF implementation is
//! built in), so unlike `duckstation_local`/`flycast_local`/`hatari_local`
//! this module has no firmware/BIOS state to discover or project at all.
//!
//! # Executable name evidence
//!
//! Upstream (`github.com/Rosalie241/RMG`, `Source/RMG/CMakeLists.txt`)
//! builds the executable target as exactly `add_executable(RMG
//! ${RMG_SOURCES})` with no `OUTPUT_NAME` override, and the top-level
//! `CMakeLists.txt`'s `install(TARGETS RMG DESTINATION
//! ${RMG_INSTALL_PATH})` installs it under that same name with no rename or
//! symlink step for either the portable (`Bin/${CMAKE_BUILD_TYPE}`) or
//! system (`${CMAKE_INSTALL_BINDIR}`, i.e. `/usr/bin`) install path. The
//! installed Linux binary name is therefore exactly `RMG` (capital,
//! case-sensitive) - never guessed, never a lowercase `rmg` variant this
//! build has no evidence for.
//!
//! # This module never writes RMG configuration
//!
//! RMG's own Qt-settings-based configuration is never read, written, or
//! required for readiness here - only the executable itself is discovered.
//! This module never executes a discovered binary.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// How a discovered RMG executable was found. Only the two forms this build
/// has real, safe discovery evidence for - no AppImage-specific seam exists
/// for RMG in this build (unlike DuckStation, which upstream officially
/// distributes as an AppImage and whose adapter carries real evidence for
/// that shape); inventing one here would be exactly the kind of unreviewed
/// widening this crate's launch adapters avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RmgInstallationType {
    /// Found on `PATH` under the exact upstream-verified name `RMG`.
    Native,
    /// A caller-supplied exact executable path.
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgExecutable {
    pub path: PathBuf,
    pub installation_type: RmgInstallationType,
    pub version: Option<String>,
}

/// One discovered RMG installation candidate. RMG has no per-profile
/// configuration this build reads, so - unlike the config-directory-rooted
/// `MgbaProfile`/`DuckStationProfile` - a profile here is simply "this exact
/// executable, found this way": `profile_id` is synthesized from the
/// executable path (the same pattern `crate::patch_manager::AzaharProfile`'s
/// projection already uses for an executable-only adapter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgProfile {
    pub profile_id: String,
    pub installation_type: RmgInstallationType,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<RmgExecutable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgProfileDiscovery {
    pub profiles: Vec<RmgProfile>,
    pub complete: bool,
}

/// Bounds how many `Explicit` executable candidates one discovery call will
/// ever consider - mirrors the small bounded-search caps every other
/// adapter's discovery already enforces (see e.g.
/// [`crate::patch_manager::MGBA_MAX_PROFILES`]).
pub const RMG_MAX_EXPLICIT_EXECUTABLES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RmgProfileDiscoveryRoots {
    /// Caller-provided exact executable path(s) - never a guess.
    pub explicit_executables: Vec<PathBuf>,
    /// The raw `PATH` environment value to search - carried explicitly
    /// (rather than read directly from `std::env`) so discovery stays
    /// deterministic and test-injectable, exactly like every other
    /// adapter's discovery-roots type in this crate.
    pub path_env: Option<std::ffi::OsString>,
    /// Already-captured `RMG --version` output, keyed by executable path -
    /// never executed by this module itself. See [`parse_rmg_version`].
    pub known_version_outputs: std::collections::BTreeMap<PathBuf, String>,
}

impl RmgProfileDiscoveryRoots {
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

/// The one upstream-verified installed executable name (see the module doc
/// comment's CMake citation). Deliberately a single exact name, never a
/// guessed lowercase or hyphenated variant this build has no evidence for.
const RMG_EXECUTABLE_NAME: &str = "RMG";

fn candidate_executables(roots: &RmgProfileDiscoveryRoots) -> Vec<RmgExecutable> {
    let mut paths: Vec<(PathBuf, RmgInstallationType)> = roots
        .explicit_executables
        .iter()
        .take(RMG_MAX_EXPLICIT_EXECUTABLES)
        .cloned()
        .map(|p| (p, RmgInstallationType::Explicit))
        .collect();
    if let Some(path_env) = &roots.path_env {
        for dir in env::split_paths(path_env) {
            paths.push((dir.join(RMG_EXECUTABLE_NAME), RmgInstallationType::Native));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|(p, _)| executable(p))
        .map(|(path, installation_type)| RmgExecutable {
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|s| parse_rmg_version(s)),
            path,
            installation_type,
        })
        .collect()
}

fn profile(installation_type: RmgInstallationType, all: &[RmgExecutable]) -> RmgProfile {
    let matching: Vec<_> = all
        .iter()
        .filter(|e| e.installation_type == installation_type)
        .cloned()
        .collect();
    let eligible = !matching.is_empty();
    let blocker = (!eligible).then(|| "no safe RMG executable was discovered".to_string());
    RmgProfile {
        profile_id: match installation_type {
            RmgInstallationType::Native => "rmg:native".to_string(),
            RmgInstallationType::Explicit => matching
                .first()
                .map(|e| format!("rmg:{}", e.path.display()))
                .unwrap_or_else(|| "rmg:explicit".to_string()),
        },
        installation_type,
        eligible,
        blocker,
        executable_candidates: matching,
    }
}

/// Discovers RMG installation candidates: an executable on `PATH` (exact
/// upstream-verified `RMG` name), and one profile per caller-supplied
/// explicit executable path. Never scans an AppImage-adjacent directory,
/// Flatpak metadata, or any config location - RMG's own configuration is
/// never read by this crate.
pub fn discover_rmg_profiles(roots: &RmgProfileDiscoveryRoots) -> RmgProfileDiscovery {
    let all = candidate_executables(roots);
    let mut profiles = Vec::new();
    if roots.path_env.is_some() {
        profiles.push(profile(RmgInstallationType::Native, &all));
    }
    if !roots.explicit_executables.is_empty() {
        profiles.push(profile(RmgInstallationType::Explicit, &all));
    }
    RmgProfileDiscovery {
        profiles,
        complete: true,
    }
}

/// Parses bounded `RMG --version` output without executing a process. Qt's
/// `QCommandLineParser::addVersionOption()` (which upstream `main.cpp` calls)
/// prints the application name followed by its version, so this looks for
/// the first digit-dot token after the executable name text, the same
/// tolerant shape `parse_mgba_version`/`parse_azahar_version` already use.
pub fn parse_rmg_version(output: &str) -> Option<String> {
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
pub enum RmgLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    AmbiguousExecutable,
    ExecutableUnsafe,
    ExecutableNotExecutable,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgLaunchBlocker {
    pub kind: RmgLaunchBlockerKind,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgNativeLaunchBinding {
    pub executable: PathBuf,
}

/// Resolves exactly one safe executable for `profile` - never a silent
/// substitution when more than one candidate matches.
pub fn resolve_rmg_native_launch_binding(
    profile: &RmgProfile,
) -> Result<RmgNativeLaunchBinding, RmgLaunchBlocker> {
    if !profile.eligible {
        return Err(RmgLaunchBlocker {
            kind: RmgLaunchBlockerKind::ProfileIneligible,
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
        [one] => Ok(RmgNativeLaunchBinding {
            executable: one.path.clone(),
        }),
        [] => Err(RmgLaunchBlocker {
            kind: RmgLaunchBlockerKind::ExecutableMissing,
            detail: "no safe executable matches this profile".into(),
        }),
        _ => Err(RmgLaunchBlocker {
            kind: RmgLaunchBlockerKind::AmbiguousExecutable,
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
        assert_eq!(parse_rmg_version("RMG 0.5.0"), Some("0.5.0".into()));
        assert_eq!(parse_rmg_version("unknown"), None);
    }

    #[test]
    fn discovers_native_executable_on_path() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join(RMG_EXECUTABLE_NAME);
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = RmgProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_rmg_profiles(&roots);
        let native = discovery
            .profiles
            .iter()
            .find(|p| p.installation_type == RmgInstallationType::Native)
            .unwrap();
        assert!(native.eligible);
        assert_eq!(native.executable_candidates[0].path, exe);
        assert!(resolve_rmg_native_launch_binding(native).is_ok());
    }

    #[test]
    fn lowercase_name_is_never_matched() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("rmg");
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = RmgProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_rmg_profiles(&roots);
        let native = discovery
            .profiles
            .iter()
            .find(|p| p.installation_type == RmgInstallationType::Native)
            .unwrap();
        assert!(!native.eligible);
    }

    #[test]
    fn no_installation_reports_no_profiles() {
        let roots = RmgProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_rmg_profiles(&roots);
        assert!(discovery.profiles.is_empty());
    }

    #[test]
    fn explicit_executable_is_discovered_and_bound() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("custom-rmg-binary");
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = RmgProfileDiscoveryRoots {
            explicit_executables: vec![exe.clone()],
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_rmg_profiles(&roots);
        let explicit = discovery
            .profiles
            .iter()
            .find(|p| p.installation_type == RmgInstallationType::Explicit)
            .unwrap();
        assert!(explicit.eligible);
        let binding = resolve_rmg_native_launch_binding(explicit).unwrap();
        assert_eq!(binding.executable, exe);
    }

    #[test]
    fn missing_executable_is_ineligible_and_refuses_binding() {
        let roots = RmgProfileDiscoveryRoots {
            explicit_executables: vec![PathBuf::from("/nonexistent/RMG")],
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_rmg_profiles(&roots);
        let explicit = &discovery.profiles[0];
        assert!(!explicit.eligible);
        assert!(resolve_rmg_native_launch_binding(explicit).is_err());
    }
}
