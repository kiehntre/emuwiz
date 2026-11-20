//! Read-only check: can a Flatpak application reach the host's NVIDIA graphics driver?
//!
//! Flatpak applications never load the host's graphics libraries. For the proprietary
//! NVIDIA driver, Flatpak mounts a runtime extension named after the *exact* version of the
//! kernel module that is loaded: it reads `/sys/module/nvidia/version` (for example
//! `580.196.07`) and looks for `org.freedesktop.Platform.GL.nvidia-580-196-07`. When the
//! host driver has been updated and the matching extension has not been installed, the
//! application silently gets Mesa only: OpenGL is drawn by the processor (llvmpipe) and
//! there is no NVIDIA Vulkan driver at all.
//!
//! Observed on the maintainer's machine on 2026-11-20 (host driver 580.196.07; extensions
//! for 580.65.06, 580.173.02 and 580.178.04 installed): inside both the xemu and the RPCS3
//! Flatpak only the `default` (Mesa) driver directory was mounted and no NVIDIA Vulkan
//! driver was listed. xemu then renders in software, far below full speed, which is heard
//! as stuttering sound; RPCS3's Vulkan renderer has no usable device, while its OpenGL
//! renderer still starts, in software.
//!
//! This module reads two kinds of path and never runs `flatpak`: the module version file,
//! and `<installation>/runtime/<extension>/<arch>/<branch>/active`. It makes no claim about
//! any driver other than NVIDIA's: for every other driver Flatpak's bundled Mesa applies and
//! there is nothing to match. Not covered: installations configured outside the two
//! standard locations, and a `FLATPAK_GL_DRIVERS` override.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Flatpak's name for an NVIDIA driver extension, without the version.
pub const FLATPAK_NVIDIA_EXTENSION_PREFIX: &str = "org.freedesktop.Platform.GL.nvidia-";

/// The longest version text accepted from the module version file.
const MAX_VERSION_LENGTH: usize = 32;

/// How many installed driver versions a result lists at most.
const MAX_LISTED_DRIVERS: usize = 6;

/// Where to look. Every path is supplied by the caller so tests never read the real machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatpakGraphicsRoots {
    /// Holds the version of the loaded NVIDIA kernel module; absent without that driver.
    pub nvidia_module_version_file: PathBuf,
    /// Flatpak installation directories (each has a `runtime` directory).
    pub flatpak_installations: Vec<PathBuf>,
    /// Flatpak's name for this machine's architecture (`x86_64`, `aarch64`).
    pub arch: String,
}

impl FlatpakGraphicsRoots {
    /// The standard locations: the system installation and this user's installation.
    pub fn for_home(home: &Path) -> Self {
        Self {
            nvidia_module_version_file: PathBuf::from("/sys/module/nvidia/version"),
            flatpak_installations: vec![
                PathBuf::from("/var/lib/flatpak"),
                home.join(".local/share/flatpak"),
            ],
            arch: flatpak_arch().to_string(),
        }
    }
}

/// Flatpak's architecture name for the machine this program was built for.
fn flatpak_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "i386",
        other => other,
    }
}

/// Whether Flatpak applications get the host's NVIDIA driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlatpakGraphics {
    /// No NVIDIA kernel module is loaded: nothing has to match.
    NotNvidia,
    /// The extension for the loaded driver version is installed.
    Matching { host_driver: String },
    /// The NVIDIA driver is loaded, but its Flatpak extension is not installed: Flatpak
    /// applications fall back to software rendering and have no NVIDIA Vulkan driver.
    Missing {
        /// The loaded driver version, for example `580.196.07`.
        host_driver: String,
        /// The exact extension Flatpak looks for.
        wanted_extension: String,
        /// Driver versions whose extension *is* installed, newest first.
        installed_drivers: Vec<String>,
    },
    /// The driver version could not be read as a version; nothing is concluded.
    Unknown { detail: String },
}

impl FlatpakGraphics {
    /// True only for the proven mismatch.
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing { .. })
    }
}

/// Reads the loaded NVIDIA driver version and looks for its Flatpak extension.
pub fn inspect_flatpak_graphics(roots: &FlatpakGraphicsRoots) -> FlatpakGraphics {
    let text = match fs::read_to_string(&roots.nvidia_module_version_file) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return FlatpakGraphics::NotNvidia,
        Err(error) => {
            return FlatpakGraphics::Unknown {
                detail: format!("the NVIDIA driver version could not be read: {error}"),
            };
        }
    };
    let host_driver = text.trim();
    if !is_driver_version(host_driver) {
        return FlatpakGraphics::Unknown {
            detail: "the NVIDIA driver version file does not hold a version".to_string(),
        };
    }
    let wanted_extension = format!(
        "{FLATPAK_NVIDIA_EXTENSION_PREFIX}{}",
        host_driver.replace('.', "-")
    );
    let installed_drivers = installed_nvidia_drivers(roots);
    if installed_drivers
        .iter()
        .any(|version| version == host_driver)
    {
        return FlatpakGraphics::Matching {
            host_driver: host_driver.to_string(),
        };
    }
    FlatpakGraphics::Missing {
        host_driver: host_driver.to_string(),
        wanted_extension,
        installed_drivers: installed_drivers
            .into_iter()
            .take(MAX_LISTED_DRIVERS)
            .collect(),
    }
}

/// Digits separated by single dots, as NVIDIA writes its versions (`580.196.07`).
fn is_driver_version(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_VERSION_LENGTH
        && text
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Every NVIDIA driver version with a deployed extension for this architecture, newest first.
fn installed_nvidia_drivers(roots: &FlatpakGraphicsRoots) -> Vec<String> {
    let mut versions: Vec<String> = Vec::new();
    for installation in &roots.flatpak_installations {
        let Ok(entries) = fs::read_dir(installation.join("runtime")) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(suffix) = name
                .to_str()
                .and_then(|name| name.strip_prefix(FLATPAK_NVIDIA_EXTENSION_PREFIX))
            else {
                continue;
            };
            let version = suffix.replace('-', ".");
            if is_driver_version(&version)
                && has_active_deployment(&entry.path().join(&roots.arch))
                && !versions.contains(&version)
            {
                versions.push(version);
            }
        }
    }
    versions.sort_by_key(|version| std::cmp::Reverse(version_key(version)));
    versions
}

/// A deployed extension has `<arch>/<branch>/active` pointing at a directory. A folder left
/// behind without it (an interrupted or removed install) is not usable.
fn has_active_deployment(arch_directory: &Path) -> bool {
    let Ok(branches) = fs::read_dir(arch_directory) else {
        return false;
    };
    branches
        .flatten()
        .any(|branch| branch.path().join("active").is_dir())
}

fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Machine {
        dir: tempfile::TempDir,
    }

    impl Machine {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn roots(&self) -> FlatpakGraphicsRoots {
            FlatpakGraphicsRoots {
                nvidia_module_version_file: self.dir.path().join("sys/module/nvidia/version"),
                flatpak_installations: vec![
                    self.dir.path().join("var/lib/flatpak"),
                    self.dir.path().join("home/.local/share/flatpak"),
                ],
                arch: "x86_64".to_string(),
            }
        }

        fn host_driver(&self, text: &str) {
            let file = self.roots().nvidia_module_version_file;
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }

        /// Deploys an extension the way Flatpak lays it out on disk.
        fn extension(&self, installation: usize, dashed: &str, arch: &str, active: bool) {
            let branch = self.roots().flatpak_installations[installation]
                .join("runtime")
                .join(format!("{FLATPAK_NVIDIA_EXTENSION_PREFIX}{dashed}"))
                .join(arch)
                .join("1.4");
            let commit = branch.join("0123abcd");
            fs::create_dir_all(&commit).unwrap();
            if active {
                std::os::unix::fs::symlink("0123abcd", branch.join("active")).unwrap();
            }
        }

        fn inspect(&self) -> FlatpakGraphics {
            inspect_flatpak_graphics(&self.roots())
        }
    }

    #[test]
    fn a_machine_without_the_nvidia_driver_has_nothing_to_match() {
        let machine = Machine::new();
        machine.extension(0, "580-178-04", "x86_64", true);
        assert_eq!(machine.inspect(), FlatpakGraphics::NotNvidia);
    }

    #[test]
    fn the_extension_for_the_loaded_driver_matches() {
        let machine = Machine::new();
        machine.host_driver("580.196.07\n");
        machine.extension(0, "580-178-04", "x86_64", true);
        machine.extension(0, "580-196-07", "x86_64", true);
        assert_eq!(
            machine.inspect(),
            FlatpakGraphics::Matching {
                host_driver: "580.196.07".to_string()
            }
        );
    }

    #[test]
    fn an_extension_in_the_users_own_installation_counts() {
        let machine = Machine::new();
        machine.host_driver("580.196.07\n");
        machine.extension(1, "580-196-07", "x86_64", true);
        assert!(matches!(
            machine.inspect(),
            FlatpakGraphics::Matching { .. }
        ));
    }

    /// The maintainer's machine on 2026-11-20: the host driver moved on, Flatpak did not.
    #[test]
    fn an_updated_host_driver_without_its_extension_is_reported_with_what_is_installed() {
        let machine = Machine::new();
        machine.host_driver("580.196.07\n");
        machine.extension(0, "580-65-06", "x86_64", true);
        machine.extension(0, "580-173-02", "x86_64", true);
        machine.extension(0, "580-178-04", "x86_64", true);
        let result = machine.inspect();
        assert!(result.is_missing());
        assert_eq!(
            result,
            FlatpakGraphics::Missing {
                host_driver: "580.196.07".to_string(),
                wanted_extension: "org.freedesktop.Platform.GL.nvidia-580-196-07".to_string(),
                installed_drivers: vec![
                    "580.178.04".to_string(),
                    "580.173.02".to_string(),
                    "580.65.06".to_string(),
                ],
            }
        );
    }

    #[test]
    fn an_extension_folder_that_is_not_deployed_does_not_count() {
        let machine = Machine::new();
        machine.host_driver("580.196.07");
        machine.extension(0, "580-196-07", "x86_64", false);
        assert!(machine.inspect().is_missing());
    }

    #[test]
    fn an_extension_for_another_architecture_does_not_count() {
        let machine = Machine::new();
        machine.host_driver("580.196.07");
        machine.extension(0, "580-196-07", "aarch64", true);
        assert!(machine.inspect().is_missing());
    }

    #[test]
    fn no_flatpak_installation_at_all_is_still_a_missing_extension() {
        let machine = Machine::new();
        machine.host_driver("580.196.07");
        assert_eq!(
            machine.inspect(),
            FlatpakGraphics::Missing {
                host_driver: "580.196.07".to_string(),
                wanted_extension: "org.freedesktop.Platform.GL.nvidia-580-196-07".to_string(),
                installed_drivers: Vec::new(),
            }
        );
    }

    #[test]
    fn text_that_is_not_a_version_concludes_nothing() {
        for text in [
            "",
            "\n",
            "580..07",
            "580.196.07; rm -rf",
            "../../etc",
            "v580",
        ] {
            let machine = Machine::new();
            machine.host_driver(text);
            assert!(
                matches!(machine.inspect(), FlatpakGraphics::Unknown { .. }),
                "{text:?}"
            );
        }
    }

    #[test]
    fn folders_that_only_look_like_extensions_are_ignored() {
        let machine = Machine::new();
        machine.host_driver("580.196.07");
        machine.extension(0, "580-196-07-debug", "x86_64", true);
        machine.extension(0, "latest", "x86_64", true);
        let FlatpakGraphics::Missing {
            installed_drivers, ..
        } = machine.inspect()
        else {
            panic!("expected a missing extension");
        };
        assert!(installed_drivers.is_empty(), "{installed_drivers:?}");
    }
}
