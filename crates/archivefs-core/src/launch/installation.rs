//! How one resolved emulator installation is started: Native, AppImage or
//! Flatpak.
//!
//! Emulator adapters build only *emulator* arguments. The installation kind
//! decides how those arguments are wrapped into the argv of the single
//! process the watched-process layer spawns:
//!
//! ```text
//! Native    <exe> <args...>                              (unchanged)
//! AppImage  <appimage> <args...>                         (no shell, no wrapper)
//! Flatpak   flatpak run [--filesystem=<dir>:ro ...] <app-id> <args...>
//! ```
//!
//! Nothing here spawns a process, edits a Flatpak override or touches an
//! emulator profile. Flatpak file visibility is *transient*: it exists only as
//! `--filesystem=` options on this one `flatpak run`. Game content is exposed
//! read-only by default; write access must be asked for explicitly with a
//! reason.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

pub const MAX_APP_ID_CHARS: usize = 255;
const MAX_GRANTS: usize = 16;

/// How an installation is launched. The executable (or the `flatpak` binary)
/// is carried separately by the adapter's binding so existing executable
/// drift checks keep working unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LaunchInstallation {
    /// Plain executable. The adapter's own argv is used byte for byte.
    #[default]
    Native,
    /// An AppImage run directly. `extract_and_run` adds
    /// `--appimage-extract-and-run` and is only set when explicitly chosen
    /// for a host without FUSE; it is never a default.
    AppImage { extract_and_run: bool },
    /// A Flatpak application, started with `flatpak run`.
    Flatpak { app_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallationKind {
    Native,
    AppImage,
    Flatpak,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallationError {
    InvalidAppId(String),
    /// A visibility path that is relative, contains separators Flatpak would
    /// misparse, or is a broad host location.
    UnsafeGrant {
        path: String,
        why: &'static str,
    },
    TooManyGrants,
}

impl std::fmt::Display for InstallationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAppId(id) => write!(f, "`{id}` is not a valid Flatpak application id"),
            Self::UnsafeGrant { path, why } => {
                write!(f, "refusing to expose `{path}` to a Flatpak sandbox: {why}")
            }
            Self::TooManyGrants => f.write_str("too many Flatpak visibility grants"),
        }
    }
}

impl std::error::Error for InstallationError {}

/// Reverse-DNS application id: at least two dot-separated elements of
/// `[A-Za-z0-9_-]`, the first character of each a letter or `_`.
#[must_use]
pub fn valid_flatpak_app_id(id: &str) -> bool {
    if id.is_empty() || id.chars().count() > MAX_APP_ID_CHARS {
        return false;
    }
    let mut elements = 0;
    for element in id.split('.') {
        let mut chars = element.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first.is_ascii_alphabetic() || first == '_')
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return false;
        }
        elements += 1;
    }
    elements >= 2
}

impl LaunchInstallation {
    pub fn flatpak(app_id: &str) -> Result<Self, InstallationError> {
        if valid_flatpak_app_id(app_id) {
            Ok(Self::Flatpak {
                app_id: app_id.to_string(),
            })
        } else {
            Err(InstallationError::InvalidAppId(app_id.to_string()))
        }
    }

    #[must_use]
    pub fn kind(&self) -> InstallationKind {
        match self {
            Self::Native => InstallationKind::Native,
            Self::AppImage { .. } => InstallationKind::AppImage,
            Self::Flatpak { .. } => InstallationKind::Flatpak,
        }
    }

    /// Wraps the adapter's emulator arguments for this installation kind.
    /// `visibility` only matters for Flatpak; Native and AppImage ignore it.
    pub fn wrap_arguments(
        &self,
        visibility: &VisibilityPlan,
        emulator_arguments: Vec<OsString>,
    ) -> Result<Vec<OsString>, InstallationError> {
        match self {
            Self::Native => Ok(emulator_arguments),
            Self::AppImage { extract_and_run } => {
                if *extract_and_run {
                    let mut out = vec![OsString::from("--appimage-extract-and-run")];
                    out.extend(emulator_arguments);
                    Ok(out)
                } else {
                    Ok(emulator_arguments)
                }
            }
            Self::Flatpak { app_id } => {
                if !valid_flatpak_app_id(app_id) {
                    return Err(InstallationError::InvalidAppId(app_id.clone()));
                }
                let mut out = vec![OsString::from("run")];
                out.extend(visibility.flatpak_options()?);
                out.push(OsString::from(app_id));
                out.extend(emulator_arguments);
                Ok(out)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

/// One path an emulator must be able to see, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibilityGrant {
    pub path: PathBuf,
    pub access: Access,
    pub reason: &'static str,
}

/// The few resources a launch needs visible. Native and AppImage launches
/// need no grants; Flatpak translates them into transient options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VisibilityPlan {
    grants: Vec<VisibilityGrant>,
}

impl VisibilityPlan {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Game content: its containing directory, read-only, so sibling files
    /// (CUE + BIN, M3U discs, sidecars) are reachable.
    pub fn content(&mut self, content_file: &Path) -> &mut Self {
        if let Some(parent) = content_file.parent() {
            self.push(VisibilityGrant {
                path: parent.to_path_buf(),
                access: Access::ReadOnly,
                reason: "game content and sibling files",
            });
        }
        self
    }

    /// An external directory the emulator only reads (BIOS, firmware).
    pub fn read_only(&mut self, path: &Path, reason: &'static str) -> &mut Self {
        self.push(VisibilityGrant {
            path: path.to_path_buf(),
            access: Access::ReadOnly,
            reason,
        });
        self
    }

    /// Write access, only for an explicitly configured path that needs it.
    pub fn read_write(&mut self, path: &Path, reason: &'static str) -> &mut Self {
        self.push(VisibilityGrant {
            path: path.to_path_buf(),
            access: Access::ReadWrite,
            reason,
        });
        self
    }

    fn push(&mut self, grant: VisibilityGrant) {
        if let Some(existing) = self.grants.iter_mut().find(|g| g.path == grant.path) {
            if grant.access == Access::ReadWrite {
                existing.access = Access::ReadWrite;
            }
        } else {
            self.grants.push(grant);
        }
    }

    #[must_use]
    pub fn grants(&self) -> &[VisibilityGrant] {
        &self.grants
    }

    /// `--filesystem=<path>[:ro]` options, validated. Each is a single argv
    /// element, so spaces in a path are inert.
    pub fn flatpak_options(&self) -> Result<Vec<OsString>, InstallationError> {
        if self.grants.len() > MAX_GRANTS {
            return Err(InstallationError::TooManyGrants);
        }
        self.grants
            .iter()
            .map(|grant| {
                check_grant_path(&grant.path)?;
                let mut option = OsString::from("--filesystem=");
                option.push(grant.path.as_os_str());
                if grant.access == Access::ReadOnly {
                    option.push(":ro");
                }
                Ok(option)
            })
            .collect()
    }
}

/// Top-level locations that are never exposed, even read-only: this is a
/// narrow grant for one game folder, not host access.
const BROAD_ROOTS: &[&str] = &[
    "/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/media", "/mnt", "/opt",
    "/proc", "/root", "/run", "/sbin", "/sys", "/tmp", "/usr", "/var",
];

fn check_grant_path(path: &Path) -> Result<(), InstallationError> {
    let shown = path.display().to_string();
    let unsafe_grant = |why| InstallationError::UnsafeGrant {
        path: shown.clone(),
        why,
    };
    if !path.is_absolute() {
        return Err(unsafe_grant("path is not absolute"));
    }
    if path
        .components()
        .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(unsafe_grant("path is not normalised"));
    }
    let text = path
        .to_str()
        .ok_or_else(|| unsafe_grant("path is not UTF-8"))?;
    if text.contains([':', '\0', '\n']) {
        return Err(unsafe_grant(
            "path contains a character Flatpak would misparse",
        ));
    }
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() || BROAD_ROOTS.contains(&trimmed) {
        return Err(unsafe_grant("path is a broad host location"));
    }
    if let Some(home) = std::env::var_os("HOME")
        && Path::new(&home) == path
    {
        return Err(unsafe_grant("path is the home directory"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
