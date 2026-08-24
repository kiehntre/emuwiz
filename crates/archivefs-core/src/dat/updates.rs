//! Typed, offline-only ownership and provenance for managed DAT snapshots.
//!
//! The user-editable DAT registry deliberately remains a registry of local
//! paths.  This module is the narrow, separate authority a later downloader
//! will need before it can replace any bytes: one built-in MAME software-list
//! descriptor, one state file, and at most current plus previous snapshots.
//!
//! There is intentionally no transport, URL fetching, scheduler, or generic
//! provider registration here.  A repository-relative path is delivery
//! metadata only; it is never used as a filesystem path.

use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dat::limits::DEFAULT_MAX_FILE_SIZE;
use crate::dat::model::DatEcosystem;
use crate::dat::sources::DatSourceOwnership;
use crate::{ArchiveFsError, Result};

/// The app-owned directory under EmuWiz's effective data directory.
pub const MANAGED_DAT_DIRECTORY: &str = "managed-dats";
const OBJECTS_DIRECTORY: &str = "objects";
const STATE_FILE_NAME: &str = "state.json";
const MAME_REPOSITORY: &str = "mamedev/mame";

/// The sole built-in managed-DAT provider initially supported by this model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDatProvider {
    MameSoftwareList,
}

impl ManagedDatProvider {
    fn storage_component(self) -> &'static str {
        match self {
            Self::MameSoftwareList => "mame-software-list",
        }
    }
}

/// A stable provider-scoped identity, not a filename-derived local-source ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManagedDatSourceId {
    pub provider: ManagedDatProvider,
    pub source_key: String,
}

impl ManagedDatSourceId {
    /// Creates the stable ID for one authoritative MAME software-list name.
    pub fn mame_software_list(source_key: impl Into<String>) -> Result<Self> {
        let source_key = source_key.into();
        validate_mame_software_list_name(&source_key)?;
        Ok(Self {
            provider: ManagedDatProvider::MameSoftwareList,
            source_key,
        })
    }

    fn validate(&self) -> Result<()> {
        match self.provider {
            ManagedDatProvider::MameSoftwareList => {
                validate_mame_software_list_name(&self.source_key)
            }
        }
    }

    /// A stable, app-owned relative storage path.  It never accepts a local
    /// filesystem path from configuration or a provider response.
    pub fn storage_relative_path(&self) -> PathBuf {
        PathBuf::from(self.provider.storage_component()).join(&self.source_key)
    }
}

impl fmt::Display for ManagedDatSourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}",
            self.provider.storage_component(),
            self.source_key
        )
    }
}

/// Managed update policy deliberately has no automatic mode yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDatUpdatePolicy {
    #[default]
    Disabled,
    Manual,
}

/// A built-in, validated future source contract.
///
/// Construction is intentionally limited to MAME software lists.  This keeps
/// an arbitrary URL or a provider-looking local DAT from becoming updater
/// authority by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatSourceDescriptor {
    source_id: ManagedDatSourceId,
    repository: &'static str,
    repository_relative_path: PathBuf,
    expected_ecosystem: DatEcosystem,
    expected_softwarelist_name: String,
    max_payload_size: u64,
    update_policy: ManagedDatUpdatePolicy,
}

impl ManagedDatSourceDescriptor {
    /// Constructs the fixed official MAME contract for one software list.
    ///
    /// It represents `mamedev/mame`, `hash/<name>.xml`, and a matching
    /// `<softwarelist name="<name>">`; it does not perform I/O or networking.
    pub fn mame_software_list(source_key: impl Into<String>) -> Result<Self> {
        let source_id = ManagedDatSourceId::mame_software_list(source_key)?;
        let source_key = source_id.source_key.clone();
        let descriptor = Self {
            source_id,
            repository: MAME_REPOSITORY,
            repository_relative_path: PathBuf::from("hash").join(format!("{source_key}.xml")),
            expected_ecosystem: DatEcosystem::MAMESoftwareList,
            expected_softwarelist_name: source_key,
            max_payload_size: DEFAULT_MAX_FILE_SIZE,
            update_policy: ManagedDatUpdatePolicy::Disabled,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn source_id(&self) -> &ManagedDatSourceId {
        &self.source_id
    }

    pub fn repository(&self) -> &'static str {
        self.repository
    }

    pub fn repository_relative_path(&self) -> &Path {
        &self.repository_relative_path
    }

    pub fn expected_ecosystem(&self) -> DatEcosystem {
        self.expected_ecosystem
    }

    pub fn expected_softwarelist_name(&self) -> &str {
        &self.expected_softwarelist_name
    }

    pub fn max_payload_size(&self) -> u64 {
        self.max_payload_size
    }

    pub fn update_policy(&self) -> ManagedDatUpdatePolicy {
        self.update_policy
    }

    /// Enables only the explicit, non-scheduled manual policy.
    pub fn with_update_policy(mut self, policy: ManagedDatUpdatePolicy) -> Self {
        self.update_policy = policy;
        self
    }

    /// Validates that this still represents the one built-in contract.
    pub fn validate(&self) -> Result<()> {
        self.source_id.validate()?;
        if self.source_id.provider != ManagedDatProvider::MameSoftwareList
            || self.repository != MAME_REPOSITORY
            || self.expected_ecosystem != DatEcosystem::MAMESoftwareList
            || self.expected_softwarelist_name != self.source_id.source_key
        {
            return Err(config_error(
                "managed DAT descriptor is not the fixed MAME software-list contract",
            ));
        }
        validate_repository_relative_path(&self.repository_relative_path)?;
        let expected_path =
            PathBuf::from("hash").join(format!("{}.xml", self.source_id.source_key));
        if self.repository_relative_path != expected_path {
            return Err(config_error(
                "managed MAME software-list path does not match its typed source ID",
            ));
        }
        if self.max_payload_size == 0 || self.max_payload_size > DEFAULT_MAX_FILE_SIZE {
            return Err(config_error(
                "managed DAT payload limit must be between one byte and the DAT parser limit",
            ));
        }
        Ok(())
    }
}

/// Rejects repository paths that could be interpreted as local filesystem
/// authority.  They are retained as remote metadata only after this check.
pub fn validate_repository_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(config_error(
            "repository-relative path must be non-empty and relative",
        ));
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(config_error(
                "repository-relative path must contain normal components only",
            ));
        }
    }
    Ok(())
}

/// One immutable managed object name.  It is a digest, never an upstream
/// filename, and is placed only below a source's `objects` directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedDatSnapshot {
    pub sha256: String,
}

impl ManagedDatSnapshot {
    pub fn new(sha256: impl Into<String>) -> Result<Self> {
        let snapshot = Self {
            sha256: sha256.into(),
        };
        validate_sha256(&snapshot.sha256)?;
        Ok(snapshot)
    }

    fn validate(&self) -> Result<()> {
        validate_sha256(&self.sha256)
    }
}

/// Durable provenance for exactly one current and one optional previous
/// validated snapshot.  This record is only state; it has no network behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedDatState {
    pub source_id: ManagedDatSourceId,
    pub current_snapshot: ManagedDatSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_snapshot: Option<ManagedDatSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// Repeated deliberately for simple provenance inspection without
    /// dereferencing `current_snapshot`.
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at_unix_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at_unix_seconds: Option<u64>,
    pub parsed_ecosystem: DatEcosystem,
    pub authoritative_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
}

impl ManagedDatState {
    /// Creates state for a validated current snapshot.  A future downloader
    /// fills provenance fields only after validating bytes and parser output.
    pub fn new(
        descriptor: &ManagedDatSourceDescriptor,
        current_snapshot: ManagedDatSnapshot,
    ) -> Result<Self> {
        descriptor.validate()?;
        let state = Self {
            source_id: descriptor.source_id.clone(),
            sha256: current_snapshot.sha256.clone(),
            current_snapshot,
            previous_snapshot: None,
            upstream_revision: None,
            etag: None,
            last_modified: None,
            retrieved_at_unix_seconds: None,
            last_checked_at_unix_seconds: None,
            parsed_ecosystem: descriptor.expected_ecosystem,
            authoritative_name: descriptor.expected_softwarelist_name.clone(),
            validation_summary: None,
            last_failure: None,
        };
        state.validate_for(descriptor)?;
        Ok(state)
    }

    /// Ensures state belongs to this descriptor and can only name current plus
    /// one previous snapshot.
    pub fn validate_for(&self, descriptor: &ManagedDatSourceDescriptor) -> Result<()> {
        descriptor.validate()?;
        self.source_id.validate()?;
        self.current_snapshot.validate()?;
        if let Some(previous) = &self.previous_snapshot {
            previous.validate()?;
            if previous.sha256 == self.current_snapshot.sha256 {
                return Err(config_error(
                    "managed DAT previous snapshot must differ from current snapshot",
                ));
            }
        }
        validate_sha256(&self.sha256)?;
        if self.sha256 != self.current_snapshot.sha256 {
            return Err(config_error(
                "managed DAT SHA-256 must name the current snapshot",
            ));
        }
        if self.source_id != descriptor.source_id
            || self.parsed_ecosystem != descriptor.expected_ecosystem
            || self.authoritative_name != descriptor.expected_softwarelist_name
        {
            return Err(config_error(
                "managed DAT state does not match its typed descriptor",
            ));
        }
        validate_optional_metadata("upstream revision", &self.upstream_revision)?;
        validate_optional_metadata("ETag", &self.etag)?;
        validate_optional_metadata("Last-Modified", &self.last_modified)?;
        validate_optional_metadata("validation summary", &self.validation_summary)?;
        validate_optional_metadata("last failure", &self.last_failure)?;
        Ok(())
    }
}

/// A validated read-only snapshot path suitable for the existing DAT parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatReadOnlySource {
    source_id: ManagedDatSourceId,
    path: PathBuf,
}

impl ManagedDatReadOnlySource {
    pub fn ownership(&self) -> DatSourceOwnership {
        DatSourceOwnership::EmuWizManaged
    }

    pub fn source_id(&self) -> &ManagedDatSourceId {
        &self.source_id
    }

    /// The ordinary regular-file input path for `parse_dat_file` and other
    /// read-only DAT consumers.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Returns the managed DAT root under EmuWiz's effective data directory.
/// This merely resolves a path; it does not create it.
pub fn managed_dat_root() -> Result<PathBuf> {
    crate::app_dirs::data_path(MANAGED_DAT_DIRECTORY)
}

/// The durable state-file location for a managed source.  The returned path is
/// always lexically below `managed_root`; it is never caller-supplied.
pub fn managed_dat_state_path(
    managed_root: &Path,
    source_id: &ManagedDatSourceId,
) -> Result<PathBuf> {
    let source_dir = managed_source_dir(managed_root, source_id)?;
    Ok(source_dir.join(STATE_FILE_NAME))
}

/// Loads one managed state record after checking its path for symlink escape.
pub fn load_managed_dat_state(
    managed_root: &Path,
    descriptor: &ManagedDatSourceDescriptor,
) -> Result<ManagedDatState> {
    descriptor.validate()?;
    let path = managed_dat_state_path(managed_root, descriptor.source_id())?;
    ensure_existing_path_is_not_symlinked(managed_root, &path)?;
    let text =
        fs::read_to_string(&path).map_err(|source| ArchiveFsError::io(path.clone(), source))?;
    let state: ManagedDatState = serde_json::from_str(&text).map_err(|error| {
        config_error(format!(
            "failed to parse managed DAT state {}: {error}",
            path.display()
        ))
    })?;
    state.validate_for(descriptor)?;
    Ok(state)
}

/// Atomically saves a state record below the managed root.  It never accepts a
/// state-file destination outside the typed source's storage directory.
pub fn save_managed_dat_state(managed_root: &Path, state: &ManagedDatState) -> Result<()> {
    let descriptor =
        ManagedDatSourceDescriptor::mame_software_list(state.source_id.source_key.clone())?;
    state.validate_for(&descriptor)?;
    create_managed_source_dir(managed_root, &state.source_id)?;
    let path = managed_dat_state_path(managed_root, &state.source_id)?;
    ensure_existing_path_is_not_symlinked(managed_root, &path)?;
    let body = serde_json::to_string_pretty(state)
        .map_err(|error| config_error(format!("failed to serialize managed DAT state: {error}")))?;
    crate::atomic_write_text(&path, &format!("{body}\n"))
}

/// Validates ownership of a current or previous snapshot and returns its
/// ordinary regular-file path.  There is no API that accepts an arbitrary
/// external file and labels it managed.
pub fn validate_managed_snapshot_ownership(
    managed_root: &Path,
    state: &ManagedDatState,
    snapshot: &ManagedDatSnapshot,
) -> Result<PathBuf> {
    let descriptor =
        ManagedDatSourceDescriptor::mame_software_list(state.source_id.source_key.clone())?;
    state.validate_for(&descriptor)?;
    let known = snapshot == &state.current_snapshot
        || state
            .previous_snapshot
            .as_ref()
            .is_some_and(|previous| previous == snapshot);
    if !known {
        return Err(config_error(
            "managed DAT snapshot is not current or previous state",
        ));
    }
    managed_snapshot_path(managed_root, &state.source_id, snapshot)
}

/// Resolves the current managed snapshot as a normal read-only DAT parser
/// input, after all ownership checks have passed.
pub fn resolve_current_managed_dat_source(
    managed_root: &Path,
    state: &ManagedDatState,
) -> Result<ManagedDatReadOnlySource> {
    let path = validate_managed_snapshot_ownership(managed_root, state, &state.current_snapshot)?;
    Ok(ManagedDatReadOnlySource {
        source_id: state.source_id.clone(),
        path,
    })
}

fn managed_snapshot_path(
    managed_root: &Path,
    source_id: &ManagedDatSourceId,
    snapshot: &ManagedDatSnapshot,
) -> Result<PathBuf> {
    snapshot.validate()?;
    let source_dir = managed_source_dir(managed_root, source_id)?;
    let path = source_dir.join(OBJECTS_DIRECTORY).join(&snapshot.sha256);
    ensure_existing_path_is_not_symlinked(managed_root, &path)?;
    let metadata =
        fs::symlink_metadata(&path).map_err(|source| ArchiveFsError::io(path.clone(), source))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(config_error(format!(
            "managed DAT snapshot is not a regular non-symlink file: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn managed_source_dir(managed_root: &Path, source_id: &ManagedDatSourceId) -> Result<PathBuf> {
    validate_managed_root_path(managed_root)?;
    source_id.validate()?;
    let path = managed_root.join(source_id.storage_relative_path());
    if !path.starts_with(managed_root) {
        return Err(config_error("managed DAT source path escaped managed root"));
    }
    Ok(path)
}

fn create_managed_source_dir(managed_root: &Path, source_id: &ManagedDatSourceId) -> Result<()> {
    let source_dir = managed_source_dir(managed_root, source_id)?;
    fs::create_dir_all(&source_dir)
        .map_err(|source| ArchiveFsError::io(source_dir.clone(), source))?;
    ensure_existing_path_is_not_symlinked(managed_root, &source_dir)?;
    let metadata = fs::symlink_metadata(&source_dir)
        .map_err(|source| ArchiveFsError::io(source_dir.clone(), source))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(config_error(format!(
            "managed DAT source directory is not a real directory: {}",
            source_dir.display()
        )));
    }
    Ok(())
}

fn validate_managed_root_path(root: &Path) -> Result<()> {
    if !root.is_absolute() {
        return Err(config_error("managed DAT root must be absolute"));
    }
    let mut current = PathBuf::new();
    for component in root.components() {
        match component {
            Component::CurDir | Component::ParentDir => {
                return Err(config_error(
                    "managed DAT root must not contain traversal components",
                ));
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
                // Check every existing ancestor, rather than only `root`.
                // `symlink_metadata(root)` alone would follow a symlinked
                // parent and could falsely make an outside directory look
                // app-owned.
                check_non_symlink_if_present(&current)?;
            }
        }
    }
    Ok(())
}

/// Rejects a symlink at any existing component between the managed root and an
/// expected descendant.  This avoids using canonicalisation as ownership
/// evidence: a symlink is never accepted as an owned directory or object.
fn ensure_existing_path_is_not_symlinked(root: &Path, path: &Path) -> Result<()> {
    validate_managed_root_path(root)?;
    if !path.starts_with(root) {
        return Err(config_error("managed DAT path is outside managed root"));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| config_error("managed DAT path escaped root"))?;
    let mut current = root.to_path_buf();
    check_non_symlink_if_present(&current)?;
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(config_error(
                "managed DAT path contains non-normal descendant component",
            ));
        };
        current.push(part);
        check_non_symlink_if_present(&current)?;
    }
    Ok(())
}

fn check_non_symlink_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(config_error(format!(
            "managed DAT path must not use symlinks: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ArchiveFsError::io(path.to_path_buf(), error)),
    }
}

fn validate_mame_software_list_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(config_error(
            "MAME software-list name must be 1-64 lowercase ASCII letters, digits, or underscores",
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(config_error(
            "managed DAT SHA-256 must be exactly 64 hexadecimal characters",
        ));
    }
    Ok(())
}

fn validate_optional_metadata(label: &str, value: &Option<String>) -> Result<()> {
    if value
        .as_ref()
        .is_some_and(|value| value.len() > 4096 || value.contains('\0'))
    {
        return Err(config_error(format!(
            "managed DAT {label} is too long or contains NUL"
        )));
    }
    Ok(())
}

fn config_error(detail: impl Into<String>) -> ArchiveFsError {
    ArchiveFsError::Config(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::limits::DatLimits;
    use crate::dat::parsers::parse_dat_file;
    use crate::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn descriptor() -> ManagedDatSourceDescriptor {
        ManagedDatSourceDescriptor::mame_software_list("gamecom").unwrap()
    }

    fn state() -> ManagedDatState {
        ManagedDatState::new(&descriptor(), ManagedDatSnapshot::new(SHA_A).unwrap()).unwrap()
    }

    fn write_current_object(root: &Path, state: &ManagedDatState) -> PathBuf {
        let path = root
            .join(state.source_id.storage_relative_path())
            .join(OBJECTS_DIRECTORY)
            .join(&state.current_snapshot.sha256);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"<softwarelist name="gamecom" description="Game.com">
<software name="test"><description>Test</description><year>1997</year><publisher>Test</publisher>
<part name="cart" interface="cart"><dataarea name="rom" size="1"><rom name="test.bin" size="1" crc="00000000"/></dataarea></part>
</software></softwarelist>"#,
        )
        .unwrap();
        path
    }

    #[test]
    fn old_toml_entries_are_user_local() {
        let config: crate::dat::sources::DatSourcesConfig = toml::from_str(
            r#"[[sources]]
id = "old"
display_name = "Old"
path = "/tmp/old.dat"
kind = "file"
"#,
        )
        .unwrap();
        let (registry, problems) = DatSourceRegistry::from_config(&config);
        assert!(problems.is_empty());
        assert!(registry.entries()[0].is_user_local());
    }

    #[test]
    fn normal_new_sources_are_user_local() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("normal.dat");
        fs::write(&path, "clrmamepro ( name \"test\" )").unwrap();
        let mut registry = DatSourceRegistry::new();
        registry
            .add(DatSourceEntry::new(
                "normal".into(),
                "Normal".into(),
                path,
                DatSourceKind::File,
            ))
            .unwrap();
        assert!(registry.entries()[0].is_user_local());
    }

    #[test]
    fn origin_text_never_changes_local_ownership() {
        for origin in ["MAME", "https://github.com/mamedev/mame"] {
            let entry = DatSourceEntry {
                origin: Some(origin.into()),
                ..DatSourceEntry::new(
                    "local".into(),
                    "Local".into(),
                    PathBuf::from("/tmp/local.dat"),
                    DatSourceKind::File,
                )
            };
            assert!(entry.is_user_local());
        }
    }

    #[test]
    fn managed_source_id_is_stable_and_descriptor_is_fixed_mame_contract() {
        let descriptor = descriptor();
        assert_eq!(
            descriptor.source_id().to_string(),
            "mame-software-list/gamecom"
        );
        assert_eq!(descriptor.repository(), MAME_REPOSITORY);
        assert_eq!(
            descriptor.repository_relative_path(),
            Path::new("hash/gamecom.xml")
        );
        assert_eq!(
            descriptor.expected_ecosystem(),
            DatEcosystem::MAMESoftwareList
        );
        assert_eq!(descriptor.expected_softwarelist_name(), "gamecom");
        assert_eq!(descriptor.update_policy(), ManagedDatUpdatePolicy::Disabled);
        descriptor.validate().unwrap();
    }

    #[test]
    fn repository_path_rejects_traversal_and_absolute_paths() {
        assert!(validate_repository_relative_path(Path::new("hash/../gamecom.xml")).is_err());
        assert!(validate_repository_relative_path(Path::new("/etc/passwd")).is_err());
        assert!(ManagedDatSourceId::mame_software_list("../gamecom").is_err());
    }

    #[test]
    fn managed_object_path_is_below_root_and_external_paths_cannot_be_claimed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let state = state();
        let expected = write_current_object(&root, &state);
        let resolved = resolve_current_managed_dat_source(&root, &state).unwrap();
        assert!(resolved.path().starts_with(&root));
        assert_eq!(resolved.path(), expected);
        assert_eq!(resolved.ownership(), DatSourceOwnership::EmuWizManaged);

        let external = temp.path().join("outside.dat");
        fs::write(&external, "not managed").unwrap();
        let missing =
            ManagedDatState::new(&descriptor(), ManagedDatSnapshot::new(SHA_B).unwrap()).unwrap();
        assert!(resolve_current_managed_dat_source(&root, &missing).is_err());
        assert!(external.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_managed_object_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let state = state();
        let external = temp.path().join("outside.dat");
        fs::write(&external, "outside").unwrap();
        let target = root
            .join(state.source_id.storage_relative_path())
            .join(OBJECTS_DIRECTORY)
            .join(&state.current_snapshot.sha256);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        symlink(&external, &target).unwrap();
        assert!(resolve_current_managed_dat_source(&root, &state).is_err());
    }

    #[test]
    fn current_previous_and_provenance_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let mut state = state();
        state.previous_snapshot = Some(ManagedDatSnapshot::new(SHA_B).unwrap());
        state.upstream_revision = Some("a1b2c3d4".into());
        state.etag = Some("\"etag\"".into());
        state.last_modified = Some("Wed, 21 Oct 2015 07:28:00 GMT".into());
        state.retrieved_at_unix_seconds = Some(1_700_000_000);
        state.last_checked_at_unix_seconds = Some(1_700_000_100);
        state.validation_summary = Some("parsed cleanly".into());
        state.last_failure = Some("previous timeout".into());
        save_managed_dat_state(&root, &state).unwrap();
        let loaded = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(loaded, state);
    }

    #[test]
    fn managed_current_snapshot_is_an_ordinary_read_only_dat_input() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let state = state();
        write_current_object(&root, &state);
        let source = resolve_current_managed_dat_source(&root, &state).unwrap();
        let parsed = parse_dat_file(source.path(), DatLimits::default()).unwrap();
        assert_eq!(parsed.dat.source.ecosystem, DatEcosystem::MAMESoftwareList);
    }

    #[test]
    fn user_local_source_has_no_managed_replacement_authority() {
        let source = DatSourceEntry::new(
            "local".into(),
            "Local".into(),
            PathBuf::from("/tmp/local.dat"),
            DatSourceKind::File,
        );
        assert!(source.is_user_local());
    }
}
