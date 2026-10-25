//! Bounded, read-only discovery and profile inspection for native SameBoy.
//!
//! # Verified against upstream, not assumed
//!
//! SameBoy ships no official Linux binary release (its releases are macOS
//! and Windows only; Linux users build from source or use a distro
//! package), so nothing here invents a packaging story it has not verified.
//! What is verified, from `LIJI32/SameBoy` (`SDL/main.c`, `Makefile`):
//!
//! - The installed binary is named `sameboy` (lowercase) -
//!   `install -m 755 $(BIN)/SDL/sameboy $(DESTDIR)$(PREFIX)/bin/sameboy`.
//! - `main()` accepts a bare ROM path with no other flags
//!   (`argc == 2 && argv[1][0] != '-'`); any other argument shape (an extra
//!   argument, or a leading `-`) prints a usage line to stderr and exits
//!   with status 1 *before* SDL is ever initialised - which is also how
//!   this module obtains a version string, see [`parse_sameboy_version`].
//! - SameBoy ships its own built-in boot ROMs (`dmg_boot.bin`,
//!   `cgb_boot.bin`, ...) alongside the SDL binary and loads them
//!   automatically (`load_boot_rom` in `SDL/main.c`) whenever no custom
//!   `bootrom_path` is configured, or whenever loading from a configured
//!   path fails. A custom boot ROM directory (real console dumps, for
//!   maximum accuracy) is optional evidence, never a launch requirement -
//!   see [`SameBoyBootRomState`].
//! - Preferences (`prefs.bin`) live next to the executable in a portable
//!   layout, or under `SDL_GetPrefPath("", "SameBoy")` (`$XDG_DATA_HOME/SameBoy/`
//!   on Linux) otherwise - a small binary file, inspected here for
//!   presence/readability only, never parsed.
//!
//! Nothing in this module writes `prefs.bin`, downloads or installs a boot
//! ROM, or executes a discovered binary.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const SAMEBOY_MAX_PROFILES: usize = 16;
pub const SAMEBOY_MAX_CONFIG_BYTES: u64 = 256 * 1024;
const PREFS_FILE_NAME: &str = "prefs.bin";
const BOOT_ROM_NAMES: &[&str] = &["dmg_boot.bin", "cgb_boot.bin"];

// ---------------------------------------------------------------------------
// Executable discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SameBoyInstallationType {
    Native,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyExecutable {
    pub path: PathBuf,
    pub installation_type: SameBoyInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub portable_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
}

impl SameBoyProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, SameBoyDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(SameBoyDiscoveryError::HomeUnavailable)?;
        let xdg_data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Ok(Self {
            home,
            xdg_data_home,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameBoyDiscoveryError {
    HomeUnavailable,
}
impl std::fmt::Display for SameBoyDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HOME is not set")
    }
}
impl std::error::Error for SameBoyDiscoveryError {}

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

fn executable_candidates(roots: &SameBoyProfileDiscoveryRoots) -> Vec<SameBoyExecutable> {
    let mut paths: Vec<(PathBuf, SameBoyInstallationType)> = roots
        .explicit_executables
        .iter()
        .cloned()
        .map(|p| (p, SameBoyInstallationType::Explicit))
        .collect();
    for root in &roots.portable_configuration_roots {
        paths.push((root.join("sameboy"), SameBoyInstallationType::Portable));
    }
    if let Some(path_env) = env::var_os("PATH") {
        for dir in env::split_paths(&path_env) {
            paths.push((dir.join("sameboy"), SameBoyInstallationType::Native));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|(p, _)| executable(p))
        .map(|(path, installation_type)| {
            let version = roots
                .known_version_outputs
                .get(&path)
                .and_then(|output| parse_sameboy_version(output));
            SameBoyExecutable {
                path,
                installation_type,
                version,
            }
        })
        .collect()
}

/// Parses SameBoy's own bounded, GUI-free version output.
///
/// SameBoy has no `--version` flag, but its argument parser
/// (`SDL/main.c: int main`) prints `SameBoy v<GB_VERSION>` to stderr and
/// exits with status 1 *before* `SDL_Init` whenever it is given any
/// argument shape other than exactly one bare, non-dash-prefixed path -
/// including an unrecognised flag such as `--version`. A caller runs
/// `sameboy --version` with a timeout and bounded output capture (exactly
/// as every sibling adapter's `known_version_outputs` map expects) and
/// passes the captured stderr text in here; this function performs no
/// process execution itself.
pub fn parse_sameboy_version(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let start = lower.find("sameboy")? + "sameboy".len();
    let tail = output[start..].trim_start().trim_start_matches(['v', 'V']);
    let value: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!value.is_empty() && value.starts_with(|c: char| c.is_ascii_digit())).then_some(value)
}

// ---------------------------------------------------------------------------
// Boot ROM evidence
// ---------------------------------------------------------------------------

/// Whether a *custom* boot ROM directory (real console dumps, for maximum
/// accuracy) is configured. SameBoy ships its own built-in boot ROMs and
/// uses them automatically whenever this is absent or unreadable - see the
/// module doc comment - so this state is never a launch blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameBoyBootRomState {
    NotConfigured,
    PresentUnverified,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyBootRomEvidence {
    pub directory: Option<PathBuf>,
    pub state: SameBoyBootRomState,
}

/// Inspects an optional custom boot ROM directory. `None` means "SameBoy's
/// own built-in boot ROMs will be used" - not evidence of a problem.
fn boot_rom_evidence(directory: Option<&Path>) -> SameBoyBootRomEvidence {
    let Some(directory) = directory else {
        return SameBoyBootRomEvidence {
            directory: None,
            state: SameBoyBootRomState::NotConfigured,
        };
    };
    let state = if !directory.is_dir() {
        SameBoyBootRomState::Missing
    } else if BOOT_ROM_NAMES
        .iter()
        .any(|name| regular(&directory.join(name)))
    {
        SameBoyBootRomState::PresentUnverified
    } else {
        SameBoyBootRomState::Unknown
    };
    SameBoyBootRomEvidence {
        directory: Some(directory.to_path_buf()),
        state,
    }
}

// ---------------------------------------------------------------------------
// Config inspection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyConfigInspection {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub oversized: bool,
}

fn inspect_config(path: &Path) -> SameBoyConfigInspection {
    let metadata = fs::symlink_metadata(path).ok();
    let exists = metadata
        .as_ref()
        .is_some_and(|m| m.is_file() && !m.file_type().is_symlink());
    let oversized = metadata
        .as_ref()
        .is_some_and(|m| m.len() > SAMEBOY_MAX_CONFIG_BYTES);
    let readable = exists && !oversized && fs::read(path).is_ok();
    SameBoyConfigInspection {
        path: path.to_path_buf(),
        exists,
        readable,
        oversized,
    }
}

fn config_path_for(root: &Path) -> PathBuf {
    root.join(PREFS_FILE_NAME)
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyProfile {
    pub profile_id: String,
    pub installation_type: SameBoyInstallationType,
    pub configuration_path: PathBuf,
    pub config: SameBoyConfigInspection,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<SameBoyExecutable>,
    pub boot_rom: SameBoyBootRomEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyProfileDiscovery {
    pub profiles: Vec<SameBoyProfile>,
    pub complete: bool,
}

fn profile(
    root: PathBuf,
    installation_type: SameBoyInstallationType,
    all: &[SameBoyExecutable],
    boot_rom_directory: Option<&Path>,
) -> SameBoyProfile {
    let config = inspect_config(&config_path_for(&root));
    let matching: Vec<SameBoyExecutable> = all
        .iter()
        .filter(|e| {
            e.installation_type == installation_type
                || installation_type == SameBoyInstallationType::Explicit
        })
        .cloned()
        .collect();
    // A never-written prefs.bin (SameBoy's first run) is not a fault - only
    // an existing-but-unreadable/oversized file is.
    let config_blocks = config.exists && (!config.readable || config.oversized);
    let eligible = !matching.is_empty() && !config_blocks;
    let blocker = (!eligible).then(|| {
        if matching.is_empty() {
            "no safe SameBoy executable was discovered".to_string()
        } else {
            "SameBoy preferences file is unreadable or oversized".to_string()
        }
    });
    SameBoyProfile {
        profile_id: format!("sameboy:{}", root.display()),
        installation_type,
        configuration_path: root,
        config,
        eligible,
        blocker,
        executable_candidates: matching,
        boot_rom: boot_rom_evidence(boot_rom_directory),
    }
}

pub fn discover_sameboy_profiles(roots: &SameBoyProfileDiscoveryRoots) -> SameBoyProfileDiscovery {
    let mut candidates = vec![(
        roots.xdg_data_home.join("SameBoy"),
        SameBoyInstallationType::Native,
    )];
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|p| (p, SameBoyInstallationType::Portable)),
    );
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|p| (p, SameBoyInstallationType::Explicit)),
    );
    candidates.sort();
    candidates.dedup_by(|a, b| a.0 == b.0);
    let all = executable_candidates(roots);
    let profiles = candidates
        .into_iter()
        .filter(|(p, k)| {
            p.is_dir()
                || matches!(
                    k,
                    SameBoyInstallationType::Explicit | SameBoyInstallationType::Portable
                )
        })
        .take(SAMEBOY_MAX_PROFILES)
        .map(|(p, k)| profile(p, k, &all, None))
        .collect();
    SameBoyProfileDiscovery {
        profiles,
        complete: true,
    }
}

// ---------------------------------------------------------------------------
// Launch binding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameBoyLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    AmbiguousExecutable,
    ExecutableUnsafe,
    ExecutableNotExecutable,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyLaunchBlocker {
    pub kind: SameBoyLaunchBlockerKind,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyNativeLaunchBinding {
    pub executable: PathBuf,
}

pub fn resolve_sameboy_native_launch_binding(
    profile: &SameBoyProfile,
) -> Result<SameBoyNativeLaunchBinding, SameBoyLaunchBlocker> {
    if !profile.eligible {
        return Err(SameBoyLaunchBlocker {
            kind: SameBoyLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "profile is not eligible".into()),
        });
    }
    let valid: Vec<_> = profile
        .executable_candidates
        .iter()
        .filter(|e| {
            e.installation_type == profile.installation_type
                || profile.installation_type == SameBoyInstallationType::Explicit
        })
        .filter(|e| executable(&e.path))
        .collect();
    match valid.as_slice() {
        [one] => Ok(SameBoyNativeLaunchBinding {
            executable: one.path.clone(),
        }),
        [] => Err(SameBoyLaunchBlocker {
            kind: SameBoyLaunchBlockerKind::ExecutableMissing,
            detail: "no safe executable matches this profile".into(),
        }),
        _ => Err(SameBoyLaunchBlocker {
            kind: SameBoyLaunchBlockerKind::AmbiguousExecutable,
            detail: "more than one safe executable matches this profile".into(),
        }),
    }
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

    fn write_exe(path: &Path) {
        fs::write(path, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(path);
    }

    #[test]
    fn version_is_parsed_from_the_bounded_usage_stderr_text() {
        assert_eq!(
            parse_sameboy_version(
                "SameBoy v1.0.3\nUsage: sameboy [--fullscreen|-f] [--nogl] [--stop-debugger|-s] [--model <model>] <rom>\n"
            ),
            Some("1.0.3".into())
        );
        assert_eq!(parse_sameboy_version("unknown"), None);
    }

    #[test]
    fn discovers_explicit_executable_and_profile() {
        let d = tempdir().unwrap();
        let root = d.path().join("profile");
        fs::create_dir_all(&root).unwrap();
        let exe = d.path().join("sameboy");
        write_exe(&exe);
        let roots = SameBoyProfileDiscoveryRoots {
            home: d.path().into(),
            xdg_data_home: d.path().join("none"),
            explicit_configuration_roots: vec![root],
            portable_configuration_roots: vec![],
            explicit_executables: vec![exe],
            known_version_outputs: BTreeMap::new(),
        };
        let discovery = discover_sameboy_profiles(&roots);
        let p = &discovery.profiles[0];
        assert!(p.eligible);
        assert!(resolve_sameboy_native_launch_binding(p).is_ok());
    }

    #[test]
    fn missing_prefs_file_is_not_a_blocker() {
        let d = tempdir().unwrap();
        let config = inspect_config(&d.path().join("prefs.bin"));
        assert!(!config.exists);
        assert!(!config.readable);
    }

    #[test]
    fn no_boot_rom_directory_configured_is_not_a_blocker() {
        let evidence = boot_rom_evidence(None);
        assert_eq!(evidence.state, SameBoyBootRomState::NotConfigured);
    }

    #[test]
    fn configured_boot_rom_directory_with_files_present_is_reported() {
        let d = tempdir().unwrap();
        fs::write(d.path().join("dmg_boot.bin"), b"boot").unwrap();
        let evidence = boot_rom_evidence(Some(d.path()));
        assert_eq!(evidence.state, SameBoyBootRomState::PresentUnverified);
    }

    #[test]
    fn configured_boot_rom_directory_missing_is_reported_distinctly() {
        let evidence = boot_rom_evidence(Some(Path::new("/does/not/exist")));
        assert_eq!(evidence.state, SameBoyBootRomState::Missing);
    }

    #[test]
    fn no_config_file_is_ever_written_by_discovery() {
        let d = tempdir().unwrap();
        let root = d.path().join("profile");
        fs::create_dir_all(&root).unwrap();
        let roots = SameBoyProfileDiscoveryRoots {
            home: d.path().into(),
            xdg_data_home: d.path().join("none"),
            explicit_configuration_roots: vec![root.clone()],
            portable_configuration_roots: vec![],
            explicit_executables: vec![],
            known_version_outputs: BTreeMap::new(),
        };
        let _ = discover_sameboy_profiles(&roots);
        assert!(fs::read_dir(&root).unwrap().next().is_none());
    }
}
