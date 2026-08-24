//! Persistent configuration for explicitly managed DAT sources.
//!
//! This is intentionally separate from `dat_sources.toml`: that file remains
//! wholly user-local paths.  The only persisted authority here is a MAME
//! software-list's authoritative name plus its Disabled/Manual policy.  URLs,
//! repositories, provider names, and transport settings are not configurable.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dat::updates::{
    ManagedDatReadOnlySource, ManagedDatSourceDescriptor, ManagedDatState, ManagedDatUpdatePolicy,
    RedumpBiosSystem, RedumpGameSystem, load_managed_dat_state, managed_dat_root,
    resolve_current_managed_dat_source, resolve_managed_dat_snapshot_source,
};
use crate::{ArchiveFsError, Result};

/// Dedicated user configuration, deliberately unrelated to `dat_sources.toml`.
pub const MANAGED_DAT_SOURCES_CONFIG_FILE: &str = "managed_dat_sources.toml";

/// The complete managed-DAT configuration.  A named table is used rather than
/// a provider discriminator: MAME software lists and Redump's fixed BIOS
/// datasets are the only supported entry types, so no free-text provider,
/// URL, or endpoint string can ever be supplied or reinterpreted later.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedDatSourcesConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mame_software_lists: Vec<ManagedMameSoftwareListConfigEntry>,
    /// Persists only the fixed typed system and Disabled/Manual policy -
    /// never a URL or endpoint. See [`ManagedRedumpBiosConfigEntry`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redump_bios: Vec<ManagedRedumpBiosConfigEntry>,
    /// Persists only the fixed typed system and Disabled/Manual policy -
    /// never a URL or endpoint. See [`ManagedRedumpGamesConfigEntry`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redump_games: Vec<ManagedRedumpGamesConfigEntry>,
}

/// One explicitly configured MAME software list.  `authoritative_name` is
/// validated by the fixed core descriptor constructor on every load and edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedMameSoftwareListConfigEntry {
    pub authoritative_name: String,
    #[serde(default)]
    pub update_policy: ManagedDatUpdatePolicy,
}

impl ManagedMameSoftwareListConfigEntry {
    pub fn descriptor(&self) -> Result<ManagedDatSourceDescriptor> {
        ManagedDatSourceDescriptor::mame_software_list(self.authoritative_name.clone())
            .map(|descriptor| descriptor.with_update_policy(self.update_policy))
    }
}

/// One explicitly configured Redump BIOS system. `system` is the only
/// identity persisted - never a URL, hostname, or free-text dataset name -
/// so a hand-edited config file can select one of the three fixed systems
/// and a policy, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedRedumpBiosConfigEntry {
    pub system: RedumpBiosSystem,
    #[serde(default)]
    pub update_policy: ManagedDatUpdatePolicy,
}

impl ManagedRedumpBiosConfigEntry {
    pub fn descriptor(&self) -> Result<ManagedDatSourceDescriptor> {
        ManagedDatSourceDescriptor::redump_bios(self.system)
            .map(|descriptor| descriptor.with_update_policy(self.update_policy))
    }
}

/// One explicitly configured Redump ordinary game-DAT system. `system` is
/// the only identity persisted - never a URL, hostname, or free-text
/// dataset name - the game-DAT sibling of [`ManagedRedumpBiosConfigEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedRedumpGamesConfigEntry {
    pub system: RedumpGameSystem,
    #[serde(default)]
    pub update_policy: ManagedDatUpdatePolicy,
}

impl ManagedRedumpGamesConfigEntry {
    pub fn descriptor(&self) -> Result<ManagedDatSourceDescriptor> {
        ManagedDatSourceDescriptor::redump_games(self.system)
            .map(|descriptor| descriptor.with_update_policy(self.update_policy))
    }
}

/// A registry-like in-memory store with deterministic ordering and explicit
/// add/remove behavior.  Removing an entry changes configuration only; it
/// never deletes state or immutable managed objects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedDatSources {
    entries: Vec<ManagedMameSoftwareListConfigEntry>,
    redump_bios_entries: Vec<ManagedRedumpBiosConfigEntry>,
    redump_games_entries: Vec<ManagedRedumpGamesConfigEntry>,
}

impl ManagedDatSources {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_config(config: ManagedDatSourcesConfig) -> Result<Self> {
        let mut sources = Self::new();
        for entry in config.mame_software_lists {
            sources.add_mame_software_list(entry.authoritative_name, entry.update_policy)?;
        }
        for entry in config.redump_bios {
            sources.add_redump_bios(entry.system, entry.update_policy)?;
        }
        for entry in config.redump_games {
            sources.add_redump_games(entry.system, entry.update_policy)?;
        }
        Ok(sources)
    }

    pub fn to_config(&self) -> ManagedDatSourcesConfig {
        ManagedDatSourcesConfig {
            mame_software_lists: self.entries.clone(),
            redump_bios: self.redump_bios_entries.clone(),
            redump_games: self.redump_games_entries.clone(),
        }
    }

    pub fn entries(&self) -> &[ManagedMameSoftwareListConfigEntry] {
        &self.entries
    }

    pub fn redump_bios_entries(&self) -> &[ManagedRedumpBiosConfigEntry] {
        &self.redump_bios_entries
    }

    pub fn redump_games_entries(&self) -> &[ManagedRedumpGamesConfigEntry] {
        &self.redump_games_entries
    }

    /// Adds only a source constructible through the typed MAME descriptor.
    pub fn add_mame_software_list(
        &mut self,
        authoritative_name: impl Into<String>,
        update_policy: ManagedDatUpdatePolicy,
    ) -> Result<()> {
        let descriptor = ManagedDatSourceDescriptor::mame_software_list(authoritative_name.into())
            .map(|descriptor| descriptor.with_update_policy(update_policy))?;
        let authoritative_name = descriptor.expected_softwarelist_name().to_string();
        if self
            .entries
            .iter()
            .any(|entry| entry.authoritative_name == authoritative_name)
        {
            return Err(ArchiveFsError::Config(format!(
                "MAME software list '{}' is already configured",
                authoritative_name
            )));
        }
        self.entries.push(ManagedMameSoftwareListConfigEntry {
            authoritative_name,
            update_policy,
        });
        self.entries
            .sort_by(|left, right| left.authoritative_name.cmp(&right.authoritative_name));
        Ok(())
    }

    /// Removes configuration only.  Existing downloaded snapshots and state
    /// are intentionally retained for a later explicit maintenance feature.
    pub fn remove_mame_software_list(
        &mut self,
        authoritative_name: &str,
    ) -> Option<ManagedMameSoftwareListConfigEntry> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.authoritative_name == authoritative_name)?;
        Some(self.entries.remove(index))
    }

    /// Adds only a source constructible through the typed Redump BIOS
    /// descriptor - `system` is a closed enum, so there is no free-text
    /// input that could name an unapproved dataset. Each of the three
    /// systems may be configured at most once.
    pub fn add_redump_bios(
        &mut self,
        system: RedumpBiosSystem,
        update_policy: ManagedDatUpdatePolicy,
    ) -> Result<()> {
        // Constructing (and immediately discarding) the descriptor proves
        // this is still one of the fixed contracts before it is persisted,
        // the same reasoning `add_mame_software_list` applies.
        ManagedDatSourceDescriptor::redump_bios(system)?;
        if self
            .redump_bios_entries
            .iter()
            .any(|entry| entry.system == system)
        {
            return Err(ArchiveFsError::Config(format!(
                "Redump BIOS system '{system:?}' is already configured"
            )));
        }
        self.redump_bios_entries.push(ManagedRedumpBiosConfigEntry {
            system,
            update_policy,
        });
        self.redump_bios_entries
            .sort_by_key(|entry| format!("{:?}", entry.system));
        Ok(())
    }

    /// Removes configuration only.  Existing downloaded snapshots and state
    /// are intentionally retained for a later explicit maintenance feature -
    /// the same reasoning as [`Self::remove_mame_software_list`].
    pub fn remove_redump_bios(
        &mut self,
        system: RedumpBiosSystem,
    ) -> Option<ManagedRedumpBiosConfigEntry> {
        let index = self
            .redump_bios_entries
            .iter()
            .position(|entry| entry.system == system)?;
        Some(self.redump_bios_entries.remove(index))
    }

    /// Adds only a source constructible through the typed Redump game-DAT
    /// descriptor - `system` is a closed enum, so there is no free-text
    /// input that could name an unapproved dataset. Each of the fixed
    /// systems may be configured at most once, the same reasoning as
    /// [`Self::add_redump_bios`].
    pub fn add_redump_games(
        &mut self,
        system: RedumpGameSystem,
        update_policy: ManagedDatUpdatePolicy,
    ) -> Result<()> {
        ManagedDatSourceDescriptor::redump_games(system)?;
        if self
            .redump_games_entries
            .iter()
            .any(|entry| entry.system == system)
        {
            return Err(ArchiveFsError::Config(format!(
                "Redump game system '{system:?}' is already configured"
            )));
        }
        self.redump_games_entries
            .push(ManagedRedumpGamesConfigEntry {
                system,
                update_policy,
            });
        self.redump_games_entries
            .sort_by_key(|entry| format!("{:?}", entry.system));
        Ok(())
    }

    /// Removes configuration only.  Existing downloaded snapshots and state
    /// are intentionally retained for a later explicit maintenance feature -
    /// the same reasoning as [`Self::remove_mame_software_list`].
    pub fn remove_redump_games(
        &mut self,
        system: RedumpGameSystem,
    ) -> Option<ManagedRedumpGamesConfigEntry> {
        let index = self
            .redump_games_entries
            .iter()
            .position(|entry| entry.system == system)?;
        Some(self.redump_games_entries.remove(index))
    }

    /// Produces typed descriptors for every configured source of all three
    /// providers, preserving each source's explicit policy.
    pub fn descriptors(&self) -> Result<Vec<ManagedDatSourceDescriptor>> {
        let mame = self.entries.iter().map(|entry| entry.descriptor());
        let redump_bios = self
            .redump_bios_entries
            .iter()
            .map(|entry| entry.descriptor());
        let redump_games = self
            .redump_games_entries
            .iter()
            .map(|entry| entry.descriptor());
        mame.chain(redump_bios).chain(redump_games).collect()
    }
}

/// A configured source plus its entirely local installation state.  `current`
/// is suitable for the existing parser/audit APIs via its `path()` method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedManagedDatSource {
    pub config: ManagedMameSoftwareListConfigEntry,
    pub descriptor: ManagedDatSourceDescriptor,
    pub state: Option<ManagedDatState>,
    pub current: Option<ManagedDatReadOnlySource>,
    pub previous: Option<ManagedDatReadOnlySource>,
}

impl ResolvedManagedDatSource {
    pub fn is_installed(&self) -> bool {
        self.current.is_some()
    }
}

/// The effective path for the dedicated managed-DAT configuration file.
pub fn default_managed_dat_sources_config_path() -> Result<PathBuf> {
    crate::app_dirs::config_path(MANAGED_DAT_SOURCES_CONFIG_FILE)
}

/// Missing or empty configuration means no configured managed sources.
pub fn load_managed_dat_sources_from(path: impl AsRef<Path>) -> Result<ManagedDatSources> {
    let path = path.as_ref();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedDatSources::new());
        }
        Err(error) => return Err(ArchiveFsError::io(path.to_path_buf(), error)),
    };
    if text.trim().is_empty() {
        return Ok(ManagedDatSources::new());
    }
    let config = toml::from_str::<ManagedDatSourcesConfig>(&text).map_err(|error| {
        ArchiveFsError::Config(format!(
            "failed to parse managed DAT sources {}: {error}",
            path.display()
        ))
    })?;
    ManagedDatSources::from_config(config)
}

pub fn load_managed_dat_sources_default() -> Result<ManagedDatSources> {
    load_managed_dat_sources_from(default_managed_dat_sources_config_path()?)
}

/// Durably saves the exact typed source set.  It never touches the local DAT
/// registry or managed objects/state.
pub fn save_managed_dat_sources_to(
    path: impl AsRef<Path>,
    sources: &ManagedDatSources,
) -> Result<()> {
    let body = toml::to_string_pretty(&sources.to_config()).map_err(|error| {
        ArchiveFsError::Config(format!("failed to serialize managed DAT sources: {error}"))
    })?;
    crate::atomic_write_text(
        path.as_ref(),
        &format!(
            "# EmuWiz managed DAT source configuration\n# Only typed MAME software-list names and fixed Redump BIOS systems are accepted.\n\n{body}"
        ),
    )
}

pub fn save_managed_dat_sources_default(sources: &ManagedDatSources) -> Result<()> {
    save_managed_dat_sources_to(default_managed_dat_sources_config_path()?, sources)
}

/// Resolves every configured source against its local managed state.  This
/// function performs no network operation and makes no filesystem mutation.
pub fn resolve_managed_dat_sources(
    sources: &ManagedDatSources,
    managed_root: &Path,
) -> Result<Vec<ResolvedManagedDatSource>> {
    let mut resolved = Vec::with_capacity(sources.entries.len());
    for config in &sources.entries {
        let descriptor = config.descriptor()?;
        let state = match load_managed_dat_state(managed_root, &descriptor) {
            Ok(state) => Some(state),
            Err(ArchiveFsError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let (current, previous) = match &state {
            None => (None, None),
            Some(state) => {
                let current = Some(resolve_current_managed_dat_source(managed_root, state)?);
                let previous = match &state.previous_snapshot {
                    Some(snapshot) => Some(resolve_managed_dat_snapshot_source(
                        managed_root,
                        state,
                        snapshot,
                    )?),
                    None => None,
                };
                (current, previous)
            }
        };
        resolved.push(ResolvedManagedDatSource {
            config: config.clone(),
            descriptor,
            state,
            current,
            previous,
        });
    }
    Ok(resolved)
}

/// Resolves configured sources using the normal app-owned managed-DAT root.
/// Like [`resolve_managed_dat_sources`], this is wholly offline and read-only.
pub fn resolve_managed_dat_sources_default(
    sources: &ManagedDatSources,
) -> Result<Vec<ResolvedManagedDatSource>> {
    resolve_managed_dat_sources(sources, &managed_dat_root()?)
}

/// A configured Redump BIOS source plus its entirely local installation
/// state - the Redump-provider sibling of [`ResolvedManagedDatSource`],
/// kept as a distinct type (rather than widening `ResolvedManagedDatSource`
/// itself) so existing MAME-only callers of that type never need to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRedumpBiosSource {
    pub config: ManagedRedumpBiosConfigEntry,
    pub descriptor: ManagedDatSourceDescriptor,
    pub state: Option<ManagedDatState>,
    pub current: Option<ManagedDatReadOnlySource>,
    pub previous: Option<ManagedDatReadOnlySource>,
}

impl ResolvedRedumpBiosSource {
    pub fn is_installed(&self) -> bool {
        self.current.is_some()
    }
}

/// Resolves every configured Redump BIOS source against its local managed
/// state - the Redump-provider sibling of [`resolve_managed_dat_sources`].
/// Performs no network operation and makes no filesystem mutation.
pub fn resolve_redump_bios_sources(
    sources: &ManagedDatSources,
    managed_root: &Path,
) -> Result<Vec<ResolvedRedumpBiosSource>> {
    let mut resolved = Vec::with_capacity(sources.redump_bios_entries.len());
    for config in &sources.redump_bios_entries {
        let descriptor = config.descriptor()?;
        let state = match load_managed_dat_state(managed_root, &descriptor) {
            Ok(state) => Some(state),
            Err(ArchiveFsError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let (current, previous) = match &state {
            None => (None, None),
            Some(state) => {
                let current = Some(resolve_current_managed_dat_source(managed_root, state)?);
                let previous = match &state.previous_snapshot {
                    Some(snapshot) => Some(resolve_managed_dat_snapshot_source(
                        managed_root,
                        state,
                        snapshot,
                    )?),
                    None => None,
                };
                (current, previous)
            }
        };
        resolved.push(ResolvedRedumpBiosSource {
            config: *config,
            descriptor,
            state,
            current,
            previous,
        });
    }
    Ok(resolved)
}

pub fn resolve_redump_bios_sources_default(
    sources: &ManagedDatSources,
) -> Result<Vec<ResolvedRedumpBiosSource>> {
    resolve_redump_bios_sources(sources, &managed_dat_root()?)
}

/// A configured Redump ordinary game-DAT source plus its entirely local
/// installation state - the game-DAT sibling of [`ResolvedRedumpBiosSource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRedumpGamesSource {
    pub config: ManagedRedumpGamesConfigEntry,
    pub descriptor: ManagedDatSourceDescriptor,
    pub state: Option<ManagedDatState>,
    pub current: Option<ManagedDatReadOnlySource>,
    pub previous: Option<ManagedDatReadOnlySource>,
}

impl ResolvedRedumpGamesSource {
    pub fn is_installed(&self) -> bool {
        self.current.is_some()
    }
}

/// Resolves every configured Redump game-DAT source against its local
/// managed state - the game-DAT sibling of [`resolve_redump_bios_sources`].
/// Performs no network operation and makes no filesystem mutation.
pub fn resolve_redump_games_sources(
    sources: &ManagedDatSources,
    managed_root: &Path,
) -> Result<Vec<ResolvedRedumpGamesSource>> {
    let mut resolved = Vec::with_capacity(sources.redump_games_entries.len());
    for config in &sources.redump_games_entries {
        let descriptor = config.descriptor()?;
        let state = match load_managed_dat_state(managed_root, &descriptor) {
            Ok(state) => Some(state),
            Err(ArchiveFsError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let (current, previous) = match &state {
            None => (None, None),
            Some(state) => {
                let current = Some(resolve_current_managed_dat_source(managed_root, state)?);
                let previous = match &state.previous_snapshot {
                    Some(snapshot) => Some(resolve_managed_dat_snapshot_source(
                        managed_root,
                        state,
                        snapshot,
                    )?),
                    None => None,
                };
                (current, previous)
            }
        };
        resolved.push(ResolvedRedumpGamesSource {
            config: *config,
            descriptor,
            state,
            current,
            previous,
        });
    }
    Ok(resolved)
}

pub fn resolve_redump_games_sources_default(
    sources: &ManagedDatSources,
) -> Result<Vec<ResolvedRedumpGamesSource>> {
    resolve_redump_games_sources(sources, &managed_dat_root()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::limits::DatLimits;
    use crate::dat::parsers::parse_dat_file;
    use crate::dat::sources::{DatSourceEntry, DatSourceKind};
    use crate::dat::updates::{ManagedDatSnapshot, ManagedDatState, save_managed_dat_state};

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn manual_sources() -> ManagedDatSources {
        let mut sources = ManagedDatSources::new();
        sources
            .add_mame_software_list("gamecom", ManagedDatUpdatePolicy::Manual)
            .unwrap();
        sources
    }

    fn descriptor() -> ManagedDatSourceDescriptor {
        ManagedDatSourceDescriptor::mame_software_list("gamecom")
            .unwrap()
            .with_update_policy(ManagedDatUpdatePolicy::Manual)
    }

    fn write_snapshot(root: &Path, sha256: &str, game: &str) {
        let path = root
            .join(descriptor().source_id().storage_relative_path())
            .join("objects")
            .join(sha256);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                r#"<softwarelist name="gamecom"><software name="{game}"><description>Test</description><year>1997</year><publisher>Test</publisher><part name="cart" interface="cart"><dataarea name="rom" size="1"><rom name="{game}.bin" size="1" crc="00000000"/></dataarea></part></software></softwarelist>"#
            ),
        )
        .unwrap();
    }

    fn installed_state() -> ManagedDatState {
        let mut state =
            ManagedDatState::new(&descriptor(), ManagedDatSnapshot::new(SHA_A).unwrap()).unwrap();
        state.previous_snapshot = Some(ManagedDatSnapshot::new(SHA_B).unwrap());
        state
    }

    #[test]
    fn missing_config_is_empty_and_first_install_resolves_without_a_path() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("managed_dat_sources.toml");
        assert!(
            load_managed_dat_sources_from(&config)
                .unwrap()
                .entries()
                .is_empty()
        );
        let sources = manual_sources();
        let root = temp.path().join("managed-dats");
        let resolved = resolve_managed_dat_sources(&sources, &root).unwrap();
        assert_eq!(resolved.len(), 1);
        assert!(!resolved[0].is_installed());
        assert!(resolved[0].state.is_none());
        assert!(resolved[0].current.is_none());
    }

    #[test]
    fn add_reload_policy_and_name_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("managed_dat_sources.toml");
        let mut sources = ManagedDatSources::new();
        sources
            .add_mame_software_list("gamecom", ManagedDatUpdatePolicy::Manual)
            .unwrap();
        sources
            .add_mame_software_list("a2600", ManagedDatUpdatePolicy::Disabled)
            .unwrap();
        save_managed_dat_sources_to(&config, &sources).unwrap();
        let reloaded = load_managed_dat_sources_from(&config).unwrap();
        assert_eq!(reloaded, sources);
        assert_eq!(reloaded.entries()[0].authoritative_name, "a2600");
        assert_eq!(
            reloaded.entries()[0].update_policy,
            ManagedDatUpdatePolicy::Disabled
        );
        assert_eq!(reloaded.entries()[1].authoritative_name, "gamecom");
        assert_eq!(
            reloaded.entries()[1].update_policy,
            ManagedDatUpdatePolicy::Manual
        );
    }

    #[test]
    fn duplicate_is_rejected_and_remove_only_changes_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("managed_dat_sources.toml");
        let root = temp.path().join("managed-dats");
        let mut sources = manual_sources();
        assert!(
            sources
                .add_mame_software_list("gamecom", ManagedDatUpdatePolicy::Manual)
                .is_err()
        );
        let state = installed_state();
        write_snapshot(&root, SHA_A, "current");
        write_snapshot(&root, SHA_B, "previous");
        save_managed_dat_state(&root, &state).unwrap();
        assert!(sources.remove_mame_software_list("gamecom").is_some());
        save_managed_dat_sources_to(&config, &sources).unwrap();
        assert!(
            load_managed_dat_sources_from(&config)
                .unwrap()
                .entries()
                .is_empty()
        );
        assert!(
            root.join(descriptor().source_id().storage_relative_path())
                .join("state.json")
                .exists()
        );
        assert!(
            root.join(descriptor().source_id().storage_relative_path())
                .join("objects")
                .join(SHA_A)
                .exists()
        );
    }

    #[test]
    fn installed_current_and_previous_resolve_as_read_only_parser_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("managed-dats");
        let state = installed_state();
        write_snapshot(&root, SHA_A, "current");
        write_snapshot(&root, SHA_B, "previous");
        save_managed_dat_state(&root, &state).unwrap();
        let resolved = resolve_managed_dat_sources(&manual_sources(), &root).unwrap();
        let source = &resolved[0];
        assert!(source.is_installed());
        assert_eq!(
            source.current.as_ref().unwrap().path().file_name().unwrap(),
            SHA_A
        );
        assert_eq!(
            source
                .previous
                .as_ref()
                .unwrap()
                .path()
                .file_name()
                .unwrap(),
            SHA_B
        );
        let parsed = parse_dat_file(
            source.current.as_ref().unwrap().path(),
            DatLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.dat.source.name.as_deref(), Some("gamecom"));
    }

    #[test]
    fn malformed_state_or_missing_object_never_falls_back_to_an_arbitrary_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("managed-dats");
        let state = installed_state();
        save_managed_dat_state(&root, &state).unwrap();
        let outside = temp.path().join("outside.dat");
        fs::write(&outside, "not a managed DAT").unwrap();
        assert!(resolve_managed_dat_sources(&manual_sources(), &root).is_err());
        assert!(outside.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_managed_object_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("managed-dats");
        let state = installed_state();
        let outside = temp.path().join("outside.dat");
        fs::write(&outside, "outside").unwrap();
        let object = root
            .join(descriptor().source_id().storage_relative_path())
            .join("objects")
            .join(SHA_A);
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        symlink(&outside, &object).unwrap();
        save_managed_dat_state(&root, &state).unwrap();
        assert!(resolve_managed_dat_sources(&manual_sources(), &root).is_err());
    }

    #[test]
    fn persisted_schema_rejects_urls_providers_and_local_origin_is_unrelated() {
        let temp = tempfile::tempdir().unwrap();
        for body in [
            "url = 'https://example.invalid/x'",
            "provider = 'redump'",
            "[[mame_software_lists]]\nauthoritative_name = 'gamecom'\nurl = 'https://example.invalid/x'",
        ] {
            let path = temp.path().join(format!("{}.toml", body.len()));
            fs::write(&path, body).unwrap();
            assert!(load_managed_dat_sources_from(&path).is_err());
        }
        let local = DatSourceEntry {
            origin: Some("MAME".to_string()),
            ..DatSourceEntry::new(
                "local".into(),
                "Local".into(),
                PathBuf::from("/tmp/local.dat"),
                DatSourceKind::File,
            )
        };
        assert!(local.is_user_local());
    }

    #[test]
    fn managed_config_never_touches_local_dat_registry_or_network() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("dat_sources.toml");
        let managed = temp.path().join("managed_dat_sources.toml");
        let local_body = "# local registry remains separate\n";
        fs::write(&local, local_body).unwrap();
        save_managed_dat_sources_to(&managed, &manual_sources()).unwrap();
        assert_eq!(fs::read_to_string(&local).unwrap(), local_body);
        // The config/load/resolve APIs expose no transport and this test calls
        // no updater function; they are deterministic filesystem-only code.
        assert!(
            resolve_managed_dat_sources(&manual_sources(), &temp.path().join("managed-dats"))
                .is_ok()
        );
    }

    // -----------------------------------------------------------------
    // Redump BIOS config persistence
    // -----------------------------------------------------------------

    #[test]
    fn redump_bios_add_reload_and_policy_round_trip_for_all_three_systems() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("managed_dat_sources.toml");
        let mut sources = ManagedDatSources::new();
        sources
            .add_redump_bios(
                RedumpBiosSystem::PlayStation2,
                ManagedDatUpdatePolicy::Manual,
            )
            .unwrap();
        sources
            .add_redump_bios(RedumpBiosSystem::Xbox, ManagedDatUpdatePolicy::Disabled)
            .unwrap();
        sources
            .add_redump_bios(
                RedumpBiosSystem::PlayStation,
                ManagedDatUpdatePolicy::Manual,
            )
            .unwrap();
        save_managed_dat_sources_to(&config, &sources).unwrap();
        let reloaded = load_managed_dat_sources_from(&config).unwrap();
        assert_eq!(reloaded, sources);
        assert_eq!(reloaded.redump_bios_entries().len(), 3);
        let xbox_entry = reloaded
            .redump_bios_entries()
            .iter()
            .find(|entry| entry.system == RedumpBiosSystem::Xbox)
            .unwrap();
        assert_eq!(xbox_entry.update_policy, ManagedDatUpdatePolicy::Disabled);
    }

    #[test]
    fn redump_bios_duplicate_system_is_rejected_and_remove_only_changes_configuration() {
        let mut sources = ManagedDatSources::new();
        sources
            .add_redump_bios(
                RedumpBiosSystem::PlayStation2,
                ManagedDatUpdatePolicy::Manual,
            )
            .unwrap();
        assert!(
            sources
                .add_redump_bios(
                    RedumpBiosSystem::PlayStation2,
                    ManagedDatUpdatePolicy::Manual
                )
                .is_err()
        );
        assert!(
            sources
                .remove_redump_bios(RedumpBiosSystem::PlayStation2)
                .is_some()
        );
        assert!(sources.redump_bios_entries().is_empty());
    }

    #[test]
    fn persisted_toml_never_carries_a_url_for_a_redump_bios_entry() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("managed_dat_sources.toml");
        let mut sources = ManagedDatSources::new();
        sources
            .add_redump_bios(
                RedumpBiosSystem::PlayStation2,
                ManagedDatUpdatePolicy::Manual,
            )
            .unwrap();
        save_managed_dat_sources_to(&config, &sources).unwrap();
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("play_station2"));
        assert!(!text.to_ascii_lowercase().contains("redump.org"));
        assert!(!text.to_ascii_lowercase().contains("http"));
    }

    #[test]
    fn a_config_file_with_only_mame_entries_still_loads_with_no_redump_entries() {
        // Proves the MAME-only schema this file's existing config used
        // before this task still loads unchanged now that `redump_bios` is
        // an additional, optional table.
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("managed_dat_sources.toml");
        fs::write(
            &config,
            "[[mame_software_lists]]\nauthoritative_name = \"gamecom\"\nupdate_policy = \"manual\"\n",
        )
        .unwrap();
        let sources = load_managed_dat_sources_from(&config).unwrap();
        assert_eq!(sources.entries().len(), 1);
        assert!(sources.redump_bios_entries().is_empty());
    }

    // -----------------------------------------------------------------
    // Redump game/disc DAT config persistence
    // -----------------------------------------------------------------

    #[test]
    fn redump_games_add_reload_and_policy_round_trip_for_all_three_systems() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("managed_dat_sources.toml");
        let mut sources = ManagedDatSources::new();
        sources
            .add_redump_games(
                RedumpGameSystem::PlayStation2,
                ManagedDatUpdatePolicy::Manual,
            )
            .unwrap();
        sources
            .add_redump_games(RedumpGameSystem::Xbox, ManagedDatUpdatePolicy::Disabled)
            .unwrap();
        sources
            .add_redump_games(
                RedumpGameSystem::PlayStation,
                ManagedDatUpdatePolicy::Manual,
            )
            .unwrap();
        save_managed_dat_sources_to(&config, &sources).unwrap();
        let reloaded = load_managed_dat_sources_from(&config).unwrap();
        assert_eq!(reloaded, sources);
        assert_eq!(reloaded.redump_games_entries().len(), 3);
        let xbox_entry = reloaded
            .redump_games_entries()
            .iter()
            .find(|entry| entry.system == RedumpGameSystem::Xbox)
            .unwrap();
        assert_eq!(xbox_entry.update_policy, ManagedDatUpdatePolicy::Disabled);
    }

    #[test]
    fn redump_games_duplicate_system_is_rejected_and_remove_only_changes_configuration() {
        let mut sources = ManagedDatSources::new();
        sources
            .add_redump_games(
                RedumpGameSystem::PlayStation2,
                ManagedDatUpdatePolicy::Manual,
            )
            .unwrap();
        assert!(
            sources
                .add_redump_games(
                    RedumpGameSystem::PlayStation2,
                    ManagedDatUpdatePolicy::Manual
                )
                .is_err()
        );
        assert!(
            sources
                .remove_redump_games(RedumpGameSystem::PlayStation2)
                .is_some()
        );
        assert!(sources.redump_games_entries().is_empty());
    }

    #[test]
    fn persisted_toml_never_carries_a_url_for_a_redump_games_entry() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("managed_dat_sources.toml");
        let mut sources = ManagedDatSources::new();
        sources
            .add_redump_games(
                RedumpGameSystem::PlayStation2,
                ManagedDatUpdatePolicy::Manual,
            )
            .unwrap();
        save_managed_dat_sources_to(&config, &sources).unwrap();
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("play_station2"));
        assert!(!text.to_ascii_lowercase().contains("redump.org"));
        assert!(!text.to_ascii_lowercase().contains("http"));
    }

    #[test]
    fn a_config_with_only_bios_entries_still_loads_with_no_game_entries() {
        // Proves the existing BIOS-only schema still loads unchanged now
        // that `redump_games` is an additional, optional table - the same
        // backward-compatibility property `redump_bios` itself already had
        // to preserve for MAME-only configs.
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("managed_dat_sources.toml");
        fs::write(
            &config,
            "[[redump_bios]]\nsystem = \"xbox\"\nupdate_policy = \"manual\"\n",
        )
        .unwrap();
        let sources = load_managed_dat_sources_from(&config).unwrap();
        assert_eq!(sources.redump_bios_entries().len(), 1);
        assert!(sources.redump_games_entries().is_empty());
    }

    #[test]
    fn resolved_redump_games_source_installs_and_resolves_as_a_read_only_dat_input() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("managed-dats");
        let system = RedumpGameSystem::PlayStation2;
        let redump_descriptor = ManagedDatSourceDescriptor::redump_games(system)
            .unwrap()
            .with_update_policy(ManagedDatUpdatePolicy::Manual);
        let snapshot = ManagedDatSnapshot::new(SHA_A).unwrap();
        let state = ManagedDatState::new(&redump_descriptor, snapshot).unwrap();
        let object_path = root
            .join(state.source_id.storage_relative_path())
            .join("objects")
            .join(SHA_A);
        fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        fs::write(
            &object_path,
            r#"<?xml version="1.0"?><datafile><header><name>Sony - PlayStation 2</name><description>Sony - PlayStation 2 Datfile</description><author>Redump.org</author></header><game name="Some Game"><rom name="track.bin" size="1" crc="00000000" md5="00000000000000000000000000000000" sha1="0000000000000000000000000000000000000000"/></game></datafile>"#,
        )
        .unwrap();
        save_managed_dat_state(&root, &state).unwrap();

        let mut sources = ManagedDatSources::new();
        sources
            .add_redump_games(system, ManagedDatUpdatePolicy::Manual)
            .unwrap();
        let resolved = resolve_redump_games_sources(&sources, &root).unwrap();
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].is_installed());
        let parsed = parse_dat_file(
            resolved[0].current.as_ref().unwrap().path(),
            DatLimits::default(),
        )
        .unwrap();
        assert_eq!(
            parsed.dat.source.ecosystem,
            crate::dat::model::DatEcosystem::Redump
        );
        assert_eq!(parsed.dat.games.len(), 1);

        // Both providers' descriptors are chained by `descriptors()`.
        assert_eq!(sources.descriptors().unwrap().len(), 1);
    }

    #[test]
    fn resolved_redump_bios_source_installs_and_resolves_as_a_read_only_dat_input() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("managed-dats");
        let system = RedumpBiosSystem::PlayStation2;
        let redump_descriptor = ManagedDatSourceDescriptor::redump_bios(system)
            .unwrap()
            .with_update_policy(ManagedDatUpdatePolicy::Manual);
        let snapshot = ManagedDatSnapshot::new(SHA_A).unwrap();
        let state = ManagedDatState::new(&redump_descriptor, snapshot).unwrap();
        let object_path = root
            .join(state.source_id.storage_relative_path())
            .join("objects")
            .join(SHA_A);
        fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        fs::write(
            &object_path,
            r#"<?xml version="1.0"?><datafile><header><name>Sony - PlayStation 2 - BIOS Images</name><description>Sony - PlayStation 2 - BIOS Images</description><author>Redump.org</author></header><game name="BIOS"><rom name="bios.bin" size="1" crc="00000000" md5="00000000000000000000000000000000" sha1="0000000000000000000000000000000000000000"/></game></datafile>"#,
        )
        .unwrap();
        save_managed_dat_state(&root, &state).unwrap();

        let mut sources = ManagedDatSources::new();
        sources
            .add_redump_bios(system, ManagedDatUpdatePolicy::Manual)
            .unwrap();
        let resolved = resolve_redump_bios_sources(&sources, &root).unwrap();
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].is_installed());
        let parsed = parse_dat_file(
            resolved[0].current.as_ref().unwrap().path(),
            DatLimits::default(),
        )
        .unwrap();
        assert_eq!(
            parsed.dat.source.ecosystem,
            crate::dat::model::DatEcosystem::Redump
        );
        let evidence = crate::dat::firmware_evidence::redump_bios_evidence_from_dat(
            &parsed.dat,
            system.firmware_system(),
        )
        .unwrap();
        assert_eq!(evidence.len(), 1);
    }
}
