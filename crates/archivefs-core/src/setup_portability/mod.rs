//! Local setup export and read-only import preview.
//!
//! A setup manifest is a typed projection of selected configuration, not a
//! backup archive or a new settings store. No ROM, credential, save, emulator
//! configuration bytes, cache, or recovery journal is copied. There is no
//! import-apply API. Paths remain evidence from the source machine until a
//! person selects a complete replacement field on the destination machine.

mod collect;
mod io;
mod preview;

pub use collect::{collect_setup, collect_setup_default};
pub use io::{export_setup_new, read_setup_manifest};
pub use preview::{SetupImportPreview, SetupPathReview, preview_setup_import};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dat::policy::DatPolicyConfig;
use crate::dat::sources::{DatSourceKind, DatSourceOwnership};
use crate::emulator_inventory::InventoryEmulator;
use crate::identity_source::path_map::{PathMapping, ProviderPathKind};
use crate::identity_source::romm::media_mapping::RommMediaMapping;

pub const SETUP_FORMAT_VERSION: u32 = 1;
pub const MAX_SETUP_BYTES: u64 = 1024 * 1024;
pub const MAX_SETUP_ITEMS: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupLibrary {
    pub sources: Vec<SetupSource>,
    pub mount_root: Option<PathBuf>,
    pub master_rom_root: Option<PathBuf>,
    pub ratarmount_bin: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupSource {
    pub path: PathBuf,
    pub enabled: bool,
}

/// Registration metadata only. Ownership describes the original registration;
/// it never grants update/replacement authority on the destination machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupDatSource {
    pub id: String,
    pub display_name: String,
    pub path: PathBuf,
    pub kind: DatSourceKind,
    pub ownership: DatSourceOwnership,
    pub enabled: Option<bool>,
    pub priority: Option<u32>,
    pub platform: Option<String>,
}

/// Reuses the installed-emulator vocabulary. Automatic detection and version
/// probes are deliberately deferred to the destination's existing Setup page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupEmulator {
    pub emulator: InventoryEmulator,
    pub executable: Option<PathBuf>,
    pub configuration_folder: Option<PathBuf>,
}

/// Deliberately excludes tokens, token-file paths, arbitrary provider fields,
/// URL userinfo/query/fragment and even the URL base path. That path can carry
/// credentials too; it must be entered again on the destination machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupRomm {
    pub enabled: bool,
    pub server_origin: Option<String>,
    pub mappings: Vec<PathMapping>,
    pub media_mapping: Option<RommMediaMapping>,
    pub provider_path_kind: ProviderPathKind,
    pub page_size: Option<u32>,
    pub import_timeout_seconds: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupArea {
    Library,
    Emulators,
    Dat,
    Providers,
    Artwork,
    Controllers,
    Launch,
    Conversion,
    CheatsMods,
    SavesConfigs,
    OtherSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupCoverage {
    Included,
    RequiresAttention,
    NotIncluded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupNotice {
    pub area: SetupArea,
    pub coverage: SetupCoverage,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupManifest {
    pub format_version: u32,
    pub emuwiz_version: String,
    pub source_os: String,
    pub library: SetupLibrary,
    pub dat_sources: Vec<SetupDatSource>,
    pub dat_policy: Option<DatPolicyConfig>,
    pub emulators: Vec<SetupEmulator>,
    pub retroarch_core_directory: Option<PathBuf>,
    pub romm: Option<SetupRomm>,
    pub notices: Vec<SetupNotice>,
}

impl Default for SetupManifest {
    fn default() -> Self {
        Self {
            format_version: SETUP_FORMAT_VERSION,
            emuwiz_version: env!("CARGO_PKG_VERSION").into(),
            source_os: std::env::consts::OS.into(),
            library: SetupLibrary::default(),
            dat_sources: Vec::new(),
            dat_policy: None,
            emulators: Vec::new(),
            retroarch_core_directory: None,
            romm: None,
            notices: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupPathKind {
    Directory,
    File,
    Executable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupPath {
    /// Stable field identity, not a filename or an inferred identity match.
    pub id: String,
    pub label: String,
    pub path: PathBuf,
    pub kind: SetupPathKind,
}

impl SetupManifest {
    pub fn paths(&self) -> Vec<SetupPath> {
        let mut paths = Vec::new();
        let mut add = |id: String, label: String, path: &Path, kind| {
            paths.push(SetupPath {
                id,
                label,
                path: path.to_path_buf(),
                kind,
            });
        };
        for (index, source) in self.library.sources.iter().enumerate() {
            add(
                format!("library.source.{index}"),
                format!(
                    "Game source {}{}",
                    index + 1,
                    if source.enabled { "" } else { " (disabled)" }
                ),
                &source.path,
                SetupPathKind::Directory,
            );
        }
        for (id, label, path) in [
            (
                "library.mount_root",
                "Mount folder",
                &self.library.mount_root,
            ),
            (
                "library.master_rom_root",
                "Organised game folder",
                &self.library.master_rom_root,
            ),
            (
                "retroarch.cores",
                "RetroArch core folder",
                &self.retroarch_core_directory,
            ),
        ] {
            if let Some(path) = path {
                add(id.into(), label.into(), path, SetupPathKind::Directory);
            }
        }
        if let Some(binary) = &self.library.ratarmount_bin {
            // A bare command is resolved separately by the destination's Setup
            // checks; it is not interpreted relative to our process directory.
            if Path::new(binary).is_absolute() || binary.contains(['/', '\\']) {
                add(
                    "library.ratarmount".into(),
                    "Archive mount tool".into(),
                    Path::new(binary),
                    SetupPathKind::Executable,
                );
            }
        }
        for (index, dat) in self.dat_sources.iter().enumerate() {
            add(
                format!("dat.source.{index}"),
                format!("DAT: {}", dat.display_name),
                &dat.path,
                if dat.kind == DatSourceKind::Folder {
                    SetupPathKind::Directory
                } else {
                    SetupPathKind::File
                },
            );
        }
        for emulator in &self.emulators {
            let key = format!("{:?}", emulator.emulator);
            if let Some(path) = &emulator.executable {
                add(
                    format!("emulator.{key}.executable"),
                    format!("{} executable", emulator.emulator.label()),
                    path,
                    SetupPathKind::Executable,
                );
            }
            if let Some(path) = &emulator.configuration_folder {
                add(
                    format!("emulator.{key}.configuration"),
                    format!("{} settings folder", emulator.emulator.label()),
                    path,
                    SetupPathKind::Directory,
                );
            }
        }
        if let Some(romm) = &self.romm {
            for (index, mapping) in romm.mappings.iter().enumerate() {
                add(
                    format!("romm.mapping.{index}"),
                    format!("RomM game folder {}", index + 1),
                    &mapping.archivefs_prefix,
                    SetupPathKind::Directory,
                );
            }
            if let Some(mapping) = &romm.media_mapping {
                add(
                    "romm.media".into(),
                    "RomM pictures and documents folder".into(),
                    &mapping.local_root,
                    SetupPathKind::Directory,
                );
            }
        }
        paths.sort_by(|a, b| a.id.cmp(&b.id));
        paths
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != SETUP_FORMAT_VERSION {
            return Err("This setup file uses an unsupported format version.".into());
        }
        let mut emulators = std::collections::BTreeSet::new();
        if self
            .emulators
            .iter()
            .any(|entry| !emulators.insert(entry.emulator))
        {
            return Err("This setup file repeats an emulator selection.".into());
        }
        let mut dat_ids = std::collections::BTreeSet::new();
        if self
            .dat_sources
            .iter()
            .any(|entry| entry.id.trim().is_empty() || !dat_ids.insert(&entry.id))
        {
            return Err(
                "This setup file contains an empty or repeated DAT source identity.".into(),
            );
        }
        let paths = self.paths();
        if paths.len() > MAX_SETUP_ITEMS || self.notices.len() > MAX_SETUP_ITEMS {
            return Err("This setup file contains too many settings.".into());
        }
        if paths.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err("This setup file repeats an emulator selection.".into());
        }
        // DAT's shared tolerant model retains unknown settings, but a setup
        // export must not smuggle arbitrary fields into a portable manifest.
        if self.dat_policy.as_ref().is_some_and(|p| {
            !p.unknown_fields.is_empty()
                || p.platforms.as_ref().is_some_and(|platforms| {
                    platforms.values().any(|p| !p.unknown_fields.is_empty())
                })
        }) {
            return Err("This setup file contains unsupported DAT policy fields.".into());
        }
        if let Some(origin) = self.romm.as_ref().and_then(|r| r.server_origin.as_ref()) {
            let parsed =
                url::Url::parse(origin).map_err(|_| "Invalid server origin in setup file.")?;
            if !matches!(parsed.scheme(), "http" | "https")
                || origin != &parsed.origin().ascii_serialization()
            {
                return Err("The setup file must contain only a server origin, without credentials or URL paths.".into());
            }
        }
        Ok(())
    }
}

/// Exact field replacements only. No prefix substitution, filesystem search,
/// fuzzy association, settings application, or modification of source data.
pub type SetupPathRemaps = BTreeMap<String, PathBuf>;
