//! The DAT source registry on disk: `~/.config/archivefs/dat_sources.toml`.
//!
//! # This file keeps what it does not understand
//!
//! `cheat_sources.toml` is `#[serde(deny_unknown_fields)]`, which means every
//! already-released binary refuses to read it the moment a new key appears -
//! including a `format_version` key that could have said so politely. That
//! constraint is now permanent for that file.
//!
//! This one is new, so it does not have to repeat the mistake. Unknown keys are
//! captured with `#[serde(flatten)]` at both the document and the entry level
//! and re-emitted verbatim on save. A future build can therefore add a field,
//! and this build will carry it through a load/edit/save cycle untouched
//! instead of deleting the user's line.
//!
//! There is deliberately still no `format_version`. A version number is only
//! useful to a reader that would otherwise misinterpret the file, and a reader
//! that preserves what it does not know has nothing to misinterpret. Adding one
//! now would be a key with no consumer.
//!
//! # Durability
//!
//! Saving goes through [`crate::atomic_write_text`]: temporary file in the same
//! directory, `sync_all`, permissions carried across, atomic rename, parent
//! directory synced, temporary file removed on any failure. A failed save
//! therefore leaves the previous file exactly as it was.

#[cfg(test)]
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    ArcadeCatalogueRevision, DatHealthState, DatSourceEntry, DatSourceHealth, DatSourceKind,
    DatSourceOwnership, MIN_DAT_PRIORITY,
};
use crate::ArchiveFsError;
use crate::dat::policy::config::DatPolicyConfig;

/// The whole preferences document.
///
/// Alongside the registered sources it carries one optional `[policy]` table:
/// the DAT matching preferences. The policy is deliberately part of this file
/// rather than a second document, so there is a single answer to "what is this
/// user's DAT matching policy" and a single durable-write path for both.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatSourcesConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<DatSourceConfigEntry>>,

    /// The DAT matching policy, present only when the user has set at least
    /// one preference. Absent means the safe defaults apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<DatPolicyConfig>,

    /// Top-level keys a newer build wrote. Never interpreted, never dropped.
    #[serde(flatten)]
    pub unknown_fields: toml::Table,
}

/// One `[[sources]]` entry.
///
/// Health is stored as flat, prefixed scalars rather than a nested
/// `[sources.health]` table. TOML requires every scalar in a table to precede
/// any sub-table, and a flattened catch-all can contain either; keeping the
/// entry free of sub-tables of our own removes that ordering hazard entirely
/// and leaves the file readable at a glance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatSourceConfigEntry {
    pub id: String,
    pub display_name: String,
    pub path: String,
    pub kind: DatSourceKind,

    /// Typed provenance for entries created by EmuWiz workflows. Missing in
    /// old configs means UserLocal; origin text never influences this field.
    #[serde(default, skip_serializing_if = "DatSourceOwnership::is_user_local")]
    pub ownership: DatSourceOwnership,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,

    /// The header name a recognised match was applied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_recognised_from_header: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_unix_seconds: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_state: Option<DatHealthState>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_last_validated_unix_seconds: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_detail: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_entry_count: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_rom_count: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_file_count: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_formats: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_observed_size_bytes: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_observed_modified_unix_seconds: Option<i64>,

    /// Sub-second part of the validated file's modification time. A new
    /// optional key: builds that predate it keep it as an unknown field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_observed_modified_subsec_nanos: Option<u32>,

    /// Arcade DAT catalogue `<version>` headers observed at validation time,
    /// one flat `"<ecosystem-key>=<version>"` string each (see
    /// [`ArcadeCatalogueRevision`]). Kept as scalars, not a sub-table, so the
    /// health block's TOML ordering rules are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_arcade_catalogue_revisions: Option<Vec<String>>,

    /// The header `<name>` the last validation read (file sources only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_header_name: Option<String>,

    /// The parsed ecosystem of that header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_header_ecosystem: Option<crate::dat::model::DatEcosystem>,

    /// The `size:seconds:nanoseconds` observation the header was read with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_header_observed: Option<String>,

    /// Keys inside this entry that a newer build wrote.
    #[serde(flatten)]
    pub unknown_fields: toml::Table,
}

impl DatSourceConfigEntry {
    /// Turns a persisted entry into a live one.
    ///
    /// `id` is passed in already validated, so this cannot construct an entry
    /// whose identity the registry would refuse.
    ///
    /// A priority outside the accepted range is clamped rather than rejected,
    /// matching how the cheat registry treats a hand-edited file: refusing to
    /// start over one bad number would be worse than correcting it, and the
    /// correction is visible because the file is rewritten with the clamped
    /// value on the next save.
    pub(super) fn into_entry(self, id: String) -> DatSourceEntry {
        DatSourceEntry {
            id,
            display_name: self.display_name,
            path: PathBuf::from(self.path),
            kind: self.kind,
            ownership: self.ownership,
            enabled: self.enabled.unwrap_or(true),
            priority: self
                .priority
                .unwrap_or(super::DEFAULT_DAT_PRIORITY)
                .clamp(MIN_DAT_PRIORITY, super::MAX_DAT_PRIORITY),
            platform: self.platform.filter(|value| !value.trim().is_empty()),
            platform_recognised_from: self.platform_recognised_from_header,
            origin: self.origin,
            added_unix_seconds: self.added_unix_seconds,
            health: DatSourceHealth {
                state: self.health_state,
                last_validated_unix_seconds: self.health_last_validated_unix_seconds,
                detail: self.health_detail,
                entry_count: self.health_entry_count,
                rom_count: self.health_rom_count,
                file_count: self.health_file_count,
                formats: self.health_formats,
                observed_size_bytes: self.health_observed_size_bytes,
                observed_modified_unix_seconds: self.health_observed_modified_unix_seconds,
                observed_modified_subsec_nanos: self.health_observed_modified_subsec_nanos,
                arcade_catalogue_revisions: ArcadeCatalogueRevision::decode_all(
                    self.health_arcade_catalogue_revisions
                        .as_deref()
                        .unwrap_or(&[]),
                ),
                header: self
                    .health_header_observed
                    .map(|observed| super::RecordedDatHeader {
                        name: self.health_header_name,
                        ecosystem: self.health_header_ecosystem,
                        observed,
                    }),
            },
            unknown_fields: self.unknown_fields,
        }
    }

    pub(super) fn from_entry(entry: &DatSourceEntry) -> Self {
        Self {
            id: entry.id.clone(),
            display_name: entry.display_name.clone(),
            path: entry.path.to_string_lossy().into_owned(),
            kind: entry.kind,
            ownership: entry.ownership.clone(),
            enabled: Some(entry.enabled),
            priority: Some(entry.priority),
            platform: entry.platform.clone(),
            platform_recognised_from_header: entry.platform_recognised_from.clone(),
            origin: entry.origin.clone(),
            added_unix_seconds: entry.added_unix_seconds,
            health_state: entry.health.state,
            health_last_validated_unix_seconds: entry.health.last_validated_unix_seconds,
            health_detail: entry.health.detail.clone(),
            health_entry_count: entry.health.entry_count,
            health_rom_count: entry.health.rom_count,
            health_file_count: entry.health.file_count,
            health_formats: entry.health.formats.clone(),
            health_observed_size_bytes: entry.health.observed_size_bytes,
            health_observed_modified_unix_seconds: entry.health.observed_modified_unix_seconds,
            health_observed_modified_subsec_nanos: entry.health.observed_modified_subsec_nanos,
            health_arcade_catalogue_revisions: ArcadeCatalogueRevision::encode_all(
                &entry.health.arcade_catalogue_revisions,
            ),
            health_header_name: entry
                .health
                .header
                .as_ref()
                .and_then(|header| header.name.clone()),
            health_header_ecosystem: entry
                .health
                .header
                .as_ref()
                .and_then(|header| header.ecosystem),
            health_header_observed: entry
                .health
                .header
                .as_ref()
                .map(|header| header.observed.clone()),
            unknown_fields: entry.unknown_fields.clone(),
        }
    }
}

/// Where the registry lives for the current user.
pub fn default_dat_sources_config_path() -> Result<PathBuf, ArchiveFsError> {
    crate::app_dirs::config_path("dat_sources.toml")
}

/// The registry path under an injected `home`, mirroring
/// [`default_dat_sources_config_path`]'s EmuWiz-first/ArchiveFS-fallback
/// resolution without reading the process environment.
#[cfg(test)]
pub(super) fn dat_sources_config_path_in(
    home: Option<OsString>,
) -> Result<PathBuf, ArchiveFsError> {
    let home = home.ok_or_else(|| ArchiveFsError::Config("HOME is not set".to_string()))?;
    Ok(crate::app_dirs::config_path_in(
        Path::new(&home),
        "dat_sources.toml",
    ))
}

pub fn load_dat_sources_config_default() -> Result<DatSourcesConfig, ArchiveFsError> {
    load_dat_sources_config_from(default_dat_sources_config_path()?)
}

/// Reads the registry. An absent or empty file means "no DAT sources yet".
pub fn load_dat_sources_config_from(
    path: impl AsRef<Path>,
) -> Result<DatSourcesConfig, ArchiveFsError> {
    let path = path.as_ref();
    let text = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DatSourcesConfig::default());
        }
        Err(source) => return Err(ArchiveFsError::io(path.to_path_buf(), source)),
    };
    if text.trim().is_empty() {
        return Ok(DatSourcesConfig::default());
    }
    toml::from_str(&text).map_err(|error| {
        ArchiveFsError::Config(format!("failed to parse {}: {error}", path.display()))
    })
}

/// What the registry file held when it was read: the SHA-256 of its bytes,
/// or `None` when there was no file.
///
/// A page that loads the registry keeps this and passes it back when it
/// saves. The save is refused if the file no longer holds exactly those
/// bytes, so an edit made elsewhere in the meantime (another window, a pack
/// import, a hand edit) is never silently overwritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatSourcesRevision(Option<String>);

impl DatSourcesRevision {
    fn of(bytes: Option<&[u8]>) -> Self {
        use sha2::{Digest, Sha256};
        Self(bytes.map(|bytes| {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        }))
    }

    /// The revision of the registry file at `path` as it is now.
    pub fn current(path: impl AsRef<Path>) -> Result<Self, ArchiveFsError> {
        let path = path.as_ref();
        match fs::read(path) {
            Ok(bytes) => Ok(Self::of(Some(&bytes))),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Self::of(None)),
            Err(source) => Err(ArchiveFsError::io(path.to_path_buf(), source)),
        }
    }
}

/// Why a conditional save did not happen.
#[derive(Debug)]
pub enum DatSourcesSaveError {
    /// The file changed since it was read. Nothing was written.
    ChangedElsewhere,
    /// Another save held the registry lock for too long. Nothing was written.
    Busy,
    Failed(ArchiveFsError),
}

impl std::fmt::Display for DatSourcesSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChangedElsewhere => f.write_str(
                "your DAT setup was changed somewhere else after this page read it \
                 (for example in another EmuWiz window). Nothing was saved. Reload the \
                 setup to see the latest version, then try again",
            ),
            Self::Busy => f.write_str(
                "another save of your DAT setup is still in progress. Nothing was saved. \
                 Try again in a moment",
            ),
            Self::Failed(error) => write!(f, "{error}"),
        }
    }
}

impl From<ArchiveFsError> for DatSourcesSaveError {
    fn from(error: ArchiveFsError) -> Self {
        Self::Failed(error)
    }
}

/// Reads the registry together with the revision it was read at. The bytes
/// are read once, so the revision describes exactly what was parsed.
pub fn load_dat_sources_config_with_revision(
    path: impl AsRef<Path>,
) -> Result<(DatSourcesConfig, DatSourcesRevision), ArchiveFsError> {
    let path = path.as_ref();
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok((DatSourcesConfig::default(), DatSourcesRevision::of(None)));
        }
        Err(source) => return Err(ArchiveFsError::io(path.to_path_buf(), source)),
    };
    let revision = DatSourcesRevision::of(Some(&bytes));
    let text = String::from_utf8(bytes).map_err(|error| {
        ArchiveFsError::Config(format!("failed to parse {}: {error}", path.display()))
    })?;
    if text.trim().is_empty() {
        return Ok((DatSourcesConfig::default(), revision));
    }
    let config = toml::from_str(&text).map_err(|error| {
        ArchiveFsError::Config(format!("failed to parse {}: {error}", path.display()))
    })?;
    Ok((config, revision))
}

/// Writes the registry only if it still holds `expected`, and returns the
/// new revision.
///
/// The check and the write happen while an exclusive lock on a sibling
/// `dat_sources.toml.lock` file is held, so two EmuWiz saves cannot
/// interleave. A writer that does not take the lock is still caught by the
/// byte comparison unless it writes in the instant between the check and
/// the atomic rename.
pub fn save_dat_sources_config_if_unchanged(
    path: impl AsRef<Path>,
    expected: &DatSourcesRevision,
    config: &DatSourcesConfig,
) -> Result<DatSourcesRevision, DatSourcesSaveError> {
    let path = path.as_ref();
    let _lock = lock_registry(path)?;
    if &DatSourcesRevision::current(path)? != expected {
        return Err(DatSourcesSaveError::ChangedElsewhere);
    }
    save_dat_sources_config_to(path, config)?;
    Ok(DatSourcesRevision::current(path)?)
}

/// Holds the registry's sibling lock file exclusively until dropped. Waits a
/// short, bounded time for another save to finish.
fn lock_registry(path: &Path) -> Result<fs::File, DatSourcesSaveError> {
    let lock_path = match path.file_name() {
        Some(name) => {
            let mut name = name.to_os_string();
            name.push(".lock");
            path.with_file_name(name)
        }
        None => {
            return Err(DatSourcesSaveError::Failed(ArchiveFsError::Config(
                format!("{} is not a file path", path.display()),
            )));
        }
    };
    if let Some(parent) = lock_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|source| ArchiveFsError::io(parent.to_path_buf(), source))?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|source| ArchiveFsError::io(lock_path.clone(), source))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(fs::TryLockError::WouldBlock) => return Err(DatSourcesSaveError::Busy),
            Err(fs::TryLockError::Error(source)) => {
                return Err(DatSourcesSaveError::Failed(ArchiveFsError::io(
                    lock_path, source,
                )));
            }
        }
    }
}

pub fn save_dat_sources_config_default(config: &DatSourcesConfig) -> Result<(), ArchiveFsError> {
    save_dat_sources_config_to(default_dat_sources_config_path()?, config)
}

/// Writes the registry durably.
pub fn save_dat_sources_config_to(
    path: impl AsRef<Path>,
    config: &DatSourcesConfig,
) -> Result<(), ArchiveFsError> {
    let path = path.as_ref();
    let header = "# EmuWiz DAT source registry\n\
                  # Managed by the DAT Sources page. Keys this build does not\n\
                  # recognise are preserved rather than removed.\n\n";
    let body = toml::to_string_pretty(config).map_err(|error| {
        ArchiveFsError::Config(format!("failed to serialize DAT source registry: {error}"))
    })?;
    crate::atomic_write_text(path, &format!("{header}{body}"))
}
