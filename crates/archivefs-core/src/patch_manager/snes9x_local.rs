//! Bounded, read-only discovery for standalone Snes9x (Super Nintendo /
//! Super Famicom).
//!
//! Snes9x boots SNES/SFC cartridge images with **no external BIOS or
//! firmware of any kind** - an ordinary SNES cartridge launch needs none and
//! the enhancement chips (SuperFX, SA-1, S-DD1, CX4, ...) are emulated
//! internally.  This module therefore models no BIOS/firmware prerequisite,
//! never reads or writes Snes9x configuration, and never executes a
//! discovered binary.
//!
//! CLI shape is confirmed against `snes9xgit/snes9x` (`snes9x.cpp`):
//! `S9xParseArgs` takes every argv entry that does not begin with `-` as the
//! ROM filename (last one wins), and `S9xUsage` documents
//! `usage: snes9x [options] <ROM image filename>`.  The GTK port
//! (`gtk/src/gtk_s9x.cpp` `main`) calls the same `S9xParseArgs` and then
//! `S9xOpenROM(rom_filename)`; its desktop entry is `Exec=snes9x-gtk %F`.
//! No option flag is ever added by this adapter.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

pub const SNES9X_MAX_PROFILES: usize = 16;

/// The PATH binary names the three current Linux ports install, in
/// discovery-preference order: the GTK port (`snes9x-gtk`, by far the most
/// commonly packaged), the newer Qt port (`snes9x-qt`), and the SDL/X11
/// "unix" port (`snes9x`).
pub const SNES9X_NATIVE_BINARY_NAMES: &[&str] = &["snes9x-gtk", "snes9x-qt", "snes9x"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Snes9xInstallationType {
    /// Found on `PATH` under one of [`SNES9X_NATIVE_BINARY_NAMES`].
    Native,
    /// A user-supplied explicit executable path.
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xExecutable {
    pub path: PathBuf,
    pub installation_type: Snes9xInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xProfile {
    pub profile_id: String,
    pub installation_type: Snes9xInstallationType,
    pub executable: PathBuf,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xProfileDiscovery {
    pub profiles: Vec<Snes9xProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snes9xProfileDiscoveryRoots {
    /// User-supplied executable paths, tried before `PATH`.
    pub explicit_executables: Vec<PathBuf>,
    /// When `Some`, searched instead of the process `PATH` for the native
    /// binary names - lets tests exercise native discovery deterministically
    /// without touching the ambient environment.
    pub path_override: Option<OsString>,
    /// Bounded, caller-captured `--version`-style output, keyed by
    /// executable path.  This module never runs a process to obtain it.
    pub known_version_outputs: BTreeMap<PathBuf, String>,
}

impl Snes9xProfileDiscoveryRoots {
    pub fn from_environment() -> Self {
        Self::default()
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

fn native_candidates(roots: &Snes9xProfileDiscoveryRoots) -> Vec<PathBuf> {
    let path_value = roots.path_override.clone().or_else(|| env::var_os("PATH"));
    let Some(path_value) = path_value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in env::split_paths(&path_value) {
        for name in SNES9X_NATIVE_BINARY_NAMES {
            out.push(dir.join(name));
        }
    }
    out
}

fn version_for(roots: &Snes9xProfileDiscoveryRoots, path: &Path) -> Option<String> {
    roots
        .known_version_outputs
        .get(path)
        .and_then(|raw| parse_snes9x_version(raw))
}

fn make_profile(exe: &Snes9xExecutable) -> Snes9xProfile {
    let usable = executable(&exe.path);
    Snes9xProfile {
        profile_id: format!("snes9x:{}", exe.path.display()),
        installation_type: exe.installation_type,
        executable: exe.path.clone(),
        eligible: usable,
        blocker: (!usable).then(|| {
            if regular(&exe.path) {
                "the discovered Snes9x executable is not marked executable".to_string()
            } else {
                "the discovered Snes9x path is not a regular file".to_string()
            }
        }),
        version: exe.version.clone(),
    }
}

/// Bounded, deterministic, read-only discovery.  Explicit executables are
/// tried first, then `PATH` (or [`Snes9xProfileDiscoveryRoots::path_override`]).
/// A path that exists but is not a usable regular executable still becomes a
/// profile - `eligible == false`, with a `blocker` - so a caller can tell
/// "installation found but unusable" apart from "no installation candidate"
/// (an empty `profiles` list).
pub fn discover_snes9x_profiles(roots: &Snes9xProfileDiscoveryRoots) -> Snes9xProfileDiscovery {
    let mut ordered: Vec<(PathBuf, Snes9xInstallationType)> = roots
        .explicit_executables
        .iter()
        .cloned()
        .map(|p| (p, Snes9xInstallationType::Explicit))
        .collect();
    ordered.extend(
        native_candidates(roots)
            .into_iter()
            .map(|p| (p, Snes9xInstallationType::Native)),
    );

    let mut seen = std::collections::BTreeSet::new();
    let mut profiles = Vec::new();
    for (path, installation_type) in ordered {
        if !seen.insert(path.clone()) {
            continue;
        }
        // A native (PATH) candidate that simply does not exist is not
        // evidence of anything - skip it silently.  An explicit path the
        // caller named is always reported, usable or not.
        if installation_type == Snes9xInstallationType::Native
            && fs::symlink_metadata(&path).is_err()
        {
            continue;
        }
        let version = version_for(roots, &path);
        profiles.push(make_profile(&Snes9xExecutable {
            path,
            installation_type,
            version,
        }));
        if profiles.len() >= SNES9X_MAX_PROFILES {
            break;
        }
    }

    Snes9xProfileDiscovery {
        profiles,
        complete: true,
    }
}

/// Parses a bounded, caller-captured version string without executing
/// anything.  Accepts shapes like `Snes9x 1.63` / `snes9x-gtk v1.63`.
pub fn parse_snes9x_version(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let start = lower.find("snes9x")? + "snes9x".len();
    let tail = output[start..]
        .trim_start_matches(['-', 'g', 't', 'k', 'q', ' '])
        .trim_start()
        .trim_start_matches(['v', 'V']);
    let value: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!value.is_empty() && value.starts_with(|c: char| c.is_ascii_digit())).then_some(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snes9xLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    ExecutableUnsafe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xLaunchBlocker {
    pub kind: Snes9xLaunchBlockerKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xNativeLaunchBinding {
    pub executable: PathBuf,
}

/// Re-verifies a discovered profile's executable at binding time.  Never
/// falls back to a different executable and never consults RetroArch.
pub fn resolve_snes9x_native_launch_binding(
    profile: &Snes9xProfile,
) -> Result<Snes9xNativeLaunchBinding, Snes9xLaunchBlocker> {
    if !profile.eligible {
        return Err(Snes9xLaunchBlocker {
            kind: Snes9xLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "profile is not eligible".into()),
        });
    }
    if fs::symlink_metadata(&profile.executable).is_err() {
        return Err(Snes9xLaunchBlocker {
            kind: Snes9xLaunchBlockerKind::ExecutableMissing,
            detail: "the authorized Snes9x executable no longer exists".into(),
        });
    }
    if !executable(&profile.executable) {
        return Err(Snes9xLaunchBlocker {
            kind: Snes9xLaunchBlockerKind::ExecutableUnsafe,
            detail: "the authorized Snes9x executable is no longer a regular executable file"
                .into(),
        });
    }
    Ok(Snes9xNativeLaunchBinding {
        executable: profile.executable.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn mark_exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut m = fs::metadata(p).unwrap().permissions();
        m.set_mode(0o755);
        fs::set_permissions(p, m).unwrap();
    }

    #[test]
    fn native_path_discovery_finds_a_usable_binary() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("snes9x-gtk");
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = Snes9xProfileDiscoveryRoots {
            path_override: Some(dir.path().as_os_str().to_os_string()),
            ..Default::default()
        };
        let discovery = discover_snes9x_profiles(&roots);
        assert_eq!(discovery.profiles.len(), 1);
        let profile = &discovery.profiles[0];
        assert_eq!(profile.installation_type, Snes9xInstallationType::Native);
        assert_eq!(profile.executable, exe);
        assert!(profile.eligible);
        assert!(resolve_snes9x_native_launch_binding(profile).is_ok());
    }

    #[test]
    fn no_installation_candidate_yields_empty_profiles() {
        let dir = tempdir().unwrap();
        let roots = Snes9xProfileDiscoveryRoots {
            path_override: Some(dir.path().as_os_str().to_os_string()),
            ..Default::default()
        };
        let discovery = discover_snes9x_profiles(&roots);
        assert!(discovery.profiles.is_empty());
        assert!(discovery.complete);
    }

    #[test]
    fn explicit_but_non_executable_path_is_reported_ineligible() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("snes9x");
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut m = fs::metadata(&exe).unwrap().permissions();
            m.set_mode(0o644);
            fs::set_permissions(&exe, m).unwrap();
        }
        let roots = Snes9xProfileDiscoveryRoots {
            explicit_executables: vec![exe.clone()],
            path_override: Some(dir.path().as_os_str().to_os_string()),
            ..Default::default()
        };
        let discovery = discover_snes9x_profiles(&roots);
        // The non-executable file is discovered once (explicit), reported
        // unusable, and never silently dropped.
        let ours = discovery
            .profiles
            .iter()
            .find(|p| p.executable == exe)
            .expect("explicit path is always reported");
        #[cfg(unix)]
        {
            assert!(!ours.eligible);
            assert!(ours.blocker.is_some());
            assert!(resolve_snes9x_native_launch_binding(ours).is_err());
        }
        let _ = ours;
    }

    #[test]
    fn version_parsing_is_bounded_and_optional() {
        assert_eq!(parse_snes9x_version("Snes9x 1.63"), Some("1.63".into()));
        assert_eq!(
            parse_snes9x_version("snes9x-gtk v1.63"),
            Some("1.63".into())
        );
        assert_eq!(parse_snes9x_version("no version here"), None);
    }
}
