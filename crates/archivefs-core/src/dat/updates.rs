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
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dat::firmware_evidence::FirmwareSystem;
use crate::dat::limits::DEFAULT_MAX_FILE_SIZE;
use crate::dat::model::DatEcosystem;
use crate::dat::sources::DatSourceOwnership;
use crate::{ArchiveFsError, Result};

/// The app-owned directory under EmuWiz's effective data directory.
pub const MANAGED_DAT_DIRECTORY: &str = "managed-dats";
const OBJECTS_DIRECTORY: &str = "objects";
const STATE_FILE_NAME: &str = "state.json";
const MAME_REPOSITORY: &str = "mamedev/mame";
const STAGING_DIRECTORY: &str = "staging";
const GITHUB_API_HOST: &str = "api.github.com";
const GITHUB_RAW_HOST: &str = "raw.githubusercontent.com";
const REDUMP_HOST: &str = "redump.info";
/// Redump BIOS DATs are a handful of records at most - a few KiB. This cap
/// is generous while still being a real, enforced bound rather than reusing
/// the multi-hundred-MiB general DAT ceiling MAME software lists need.
const REDUMP_BIOS_MAX_PAYLOAD: u64 = 1024 * 1024;
const MANAGED_DAT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MANAGED_DAT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MANAGED_DAT_OVERALL_TIMEOUT: Duration = Duration::from_secs(90);
const MANAGED_DAT_HEADER_LIMIT: usize = 32 * 1024;
const MANAGED_DAT_NETWORK_CHUNK: usize = 64 * 1024;

/// The built-in managed-DAT providers this model supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDatProvider {
    MameSoftwareList,
    /// Redump's dedicated machine-readable BIOS DATs - see
    /// [`RedumpBiosSystem`] for the fixed, closed set of systems this
    /// provider may name.
    RedumpBios,
    /// Redump's ordinary per-system game/disc DATs - see
    /// [`RedumpGameSystem`] for the fixed, closed set of systems this
    /// provider may name. A deliberately narrower set than
    /// [`RedumpBiosSystem`]'s three systems is not required - both draw
    /// from the same proven `redump.info` host and `/datfile/<slug>/` path
    /// family (see [`RedumpGameSystem::fixed_url`]'s own doc comment) - but
    /// only systems this codebase has actual evidence for are ever added.
    RedumpGames,
}

impl ManagedDatProvider {
    fn storage_component(self) -> &'static str {
        match self {
            Self::MameSoftwareList => "mame-software-list",
            Self::RedumpBios => "redump-bios",
            Self::RedumpGames => "redump-games",
        }
    }
}

/// The fixed, closed set of systems Redump publishes a dedicated
/// machine-readable BIOS DAT for. There is intentionally no way to
/// construct a [`ManagedDatSourceDescriptor`] naming any other Redump
/// dataset, host, or path - see [`ManagedDatSourceDescriptor::redump_bios`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedumpBiosSystem {
    PlayStation,
    PlayStation2,
    Xbox,
}

impl RedumpBiosSystem {
    fn slug(self) -> &'static str {
        match self {
            Self::PlayStation => "playstation",
            Self::PlayStation2 => "playstation2",
            Self::Xbox => "xbox",
        }
    }

    fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "playstation" => Some(Self::PlayStation),
            "playstation2" => Some(Self::PlayStation2),
            "xbox" => Some(Self::Xbox),
            _ => None,
        }
    }

    /// The one fixed, approved HTTPS URL for this system's Redump BIOS DAT.
    /// Never caller-supplied, never inferred from a URL a caller passed in
    /// - this is the entire "approved endpoint" surface for this provider.
    fn fixed_url(self) -> &'static str {
        match self {
            Self::PlayStation => "https://redump.info/datfile/psx-bios/",
            Self::PlayStation2 => "https://redump.info/datfile/ps2-bios/",
            Self::Xbox => "https://redump.info/datfile/xbox-bios/",
        }
    }

    /// Which [`FirmwareSystem`] this Redump dataset produces
    /// [`crate::dat::firmware_evidence::FirmwareIdentityRecord`] evidence
    /// for - see [`FirmwareSystem::Xbox`]'s own doc comment for why the
    /// Xbox mapping in particular names only the BIOS/flash component.
    pub fn firmware_system(self) -> FirmwareSystem {
        match self {
            Self::PlayStation => FirmwareSystem::PlayStation,
            Self::PlayStation2 => FirmwareSystem::PlayStation2,
            Self::Xbox => FirmwareSystem::Xbox,
        }
    }
}

/// The fixed, closed set of systems this build has actual evidence for
/// Redump's ordinary (non-BIOS) per-system game/disc DAT.
///
/// Deliberately limited to the same three systems [`RedumpBiosSystem`]
/// already proves a working `redump.info` contract for - not because other
/// Redump systems (Saturn, Dreamcast, GameCube, Wii, ...) lack a game DAT,
/// but because this codebase has no proven slug for any of them: Redump's
/// own BIOS URLs (`.../datfile/psx-bios/`, `.../datfile/ps2-bios/`,
/// `.../datfile/xbox-bios/`) are the only place a `/datfile/<slug>/`-shaped
/// path has ever been confirmed correct in this repository, and the
/// ordinary-dataset slug for each is exactly that BIOS slug with the
/// `-bios` suffix removed - not a separately invented guess. A system whose
/// slug cannot be derived this way (Saturn included) is intentionally left
/// unsupported rather than guessed at; see the module's managed-provider
/// task notes for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedumpGameSystem {
    PlayStation,
    PlayStation2,
    Xbox,
}

impl RedumpGameSystem {
    fn slug(self) -> &'static str {
        match self {
            Self::PlayStation => "playstation",
            Self::PlayStation2 => "playstation2",
            Self::Xbox => "xbox",
        }
    }

    fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "playstation" => Some(Self::PlayStation),
            "playstation2" => Some(Self::PlayStation2),
            "xbox" => Some(Self::Xbox),
            _ => None,
        }
    }

    /// The one fixed, approved HTTPS URL for this system's ordinary Redump
    /// game DAT - the [`RedumpBiosSystem::fixed_url`] URL for the same
    /// system with the `-bios` path segment removed, never a separately
    /// guessed slug (see this enum's own doc comment).
    fn fixed_url(self) -> &'static str {
        match self {
            Self::PlayStation => "https://redump.info/datfile/psx/",
            Self::PlayStation2 => "https://redump.info/datfile/ps2/",
            Self::Xbox => "https://redump.info/datfile/xbox/",
        }
    }

    /// A plain descriptive label for provenance/error text only - this is
    /// never asserted to appear verbatim in a downloaded DAT's header; see
    /// [`header_identifies_redump_game_dataset`] for the actual (tolerant,
    /// substring-based) match.
    fn dataset_label(self) -> &'static str {
        match self {
            Self::PlayStation => "Sony - PlayStation",
            Self::PlayStation2 => "Sony - PlayStation 2",
            Self::Xbox => "Microsoft - Xbox",
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

    /// Creates the stable ID for one of the fixed Redump BIOS systems.
    /// Infallible: `system` is a closed enum, so there is no free-text
    /// input to validate.
    pub fn redump_bios(system: RedumpBiosSystem) -> Self {
        Self {
            provider: ManagedDatProvider::RedumpBios,
            source_key: system.slug().to_string(),
        }
    }

    /// Creates the stable ID for one of the fixed Redump ordinary
    /// game-DAT systems. Infallible: `system` is a closed enum, so there is
    /// no free-text input to validate.
    pub fn redump_games(system: RedumpGameSystem) -> Self {
        Self {
            provider: ManagedDatProvider::RedumpGames,
            source_key: system.slug().to_string(),
        }
    }

    fn validate(&self) -> Result<()> {
        match self.provider {
            ManagedDatProvider::MameSoftwareList => {
                validate_mame_software_list_name(&self.source_key)
            }
            ManagedDatProvider::RedumpBios => {
                if RedumpBiosSystem::from_slug(&self.source_key).is_none() {
                    return Err(config_error(
                        "Redump BIOS source key must name one of the fixed supported systems",
                    ));
                }
                Ok(())
            }
            ManagedDatProvider::RedumpGames => {
                if RedumpGameSystem::from_slug(&self.source_key).is_none() {
                    return Err(config_error(
                        "Redump game DAT source key must name one of the fixed supported systems",
                    ));
                }
                Ok(())
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

/// How a descriptor's bytes are actually fetched. Private: a caller can
/// only ever get one of these values through
/// [`ManagedDatSourceDescriptor::mame_software_list`] or
/// [`ManagedDatSourceDescriptor::redump_bios`], never by constructing a URL
/// or hostname themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ManagedDatRemote {
    /// MAME's model: an immutable commit SHA is resolved first (via the
    /// GitHub API), then the file is downloaded from GitHub raw content
    /// pinned to that exact commit.
    GithubCommitPinned {
        repository: &'static str,
        repository_relative_path: PathBuf,
    },
    /// Redump's model: one fixed HTTPS URL serves the current DAT directly.
    /// There is no separate "resolve a revision" step - see
    /// [`check_redump_bios_update`]/[`update_redump_bios`].
    DirectHttps { url: &'static str },
}

/// Which authoritative dataset a downloaded DAT's parsed header must
/// identify itself as, before its bytes are trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpectedDataset {
    /// MAME: exact `<softwarelist name="...">` match.
    MameSoftwareList(String),
    /// Redump: the fixed system's BIOS Images dataset, matched via
    /// [`crate::dat::firmware_evidence::header_identifies_redump_bios_dataset`]
    /// rather than exact string equality (see that function's own doc
    /// comment for why).
    RedumpBios(RedumpBiosSystem),
    /// Redump: the fixed system's ordinary game/disc dataset, matched via
    /// [`header_identifies_redump_game_dataset`] - tolerant substring
    /// matching, and an explicit rejection of anything that looks like a
    /// BIOS dataset instead.
    RedumpGames(RedumpGameSystem),
}

/// A built-in, validated future source contract.
///
/// Construction is intentionally limited to MAME software lists and
/// Redump's fixed BIOS datasets.  This keeps an arbitrary URL, hostname, or
/// provider-looking local DAT from becoming updater authority by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatSourceDescriptor {
    source_id: ManagedDatSourceId,
    remote: ManagedDatRemote,
    expected_ecosystem: DatEcosystem,
    expected_dataset: ExpectedDataset,
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
            remote: ManagedDatRemote::GithubCommitPinned {
                repository: MAME_REPOSITORY,
                repository_relative_path: PathBuf::from("hash").join(format!("{source_key}.xml")),
            },
            expected_ecosystem: DatEcosystem::MAMESoftwareList,
            expected_dataset: ExpectedDataset::MameSoftwareList(source_key),
            max_payload_size: DEFAULT_MAX_FILE_SIZE,
            update_policy: ManagedDatUpdatePolicy::Disabled,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// Constructs the fixed contract for one of Redump's dedicated BIOS
    /// DATs. The caller supplies only a closed [`RedumpBiosSystem`] enum
    /// value - never a URL, hostname, or free-text dataset name - and each
    /// variant maps internally to exactly one approved endpoint (see
    /// [`RedumpBiosSystem::fixed_url`]).
    pub fn redump_bios(system: RedumpBiosSystem) -> Result<Self> {
        let descriptor = Self {
            source_id: ManagedDatSourceId::redump_bios(system),
            remote: ManagedDatRemote::DirectHttps {
                url: system.fixed_url(),
            },
            expected_ecosystem: DatEcosystem::Redump,
            expected_dataset: ExpectedDataset::RedumpBios(system),
            max_payload_size: REDUMP_BIOS_MAX_PAYLOAD,
            update_policy: ManagedDatUpdatePolicy::Disabled,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// Constructs the fixed contract for one of Redump's ordinary
    /// per-system game/disc DATs. The caller supplies only a closed
    /// [`RedumpGameSystem`] enum value - never a URL, hostname, or
    /// free-text dataset name - and each variant maps internally to exactly
    /// one approved endpoint (see [`RedumpGameSystem::fixed_url`]). The
    /// downloaded body may be a bare DAT/XML file or a ZIP archive
    /// containing exactly one - see [`fetch_and_validate_redump_games`] for
    /// how that is detected and safely unwrapped.
    pub fn redump_games(system: RedumpGameSystem) -> Result<Self> {
        let descriptor = Self {
            source_id: ManagedDatSourceId::redump_games(system),
            remote: ManagedDatRemote::DirectHttps {
                url: system.fixed_url(),
            },
            expected_ecosystem: DatEcosystem::Redump,
            expected_dataset: ExpectedDataset::RedumpGames(system),
            max_payload_size: DEFAULT_MAX_FILE_SIZE,
            update_policy: ManagedDatUpdatePolicy::Disabled,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn source_id(&self) -> &ManagedDatSourceId {
        &self.source_id
    }

    /// The GitHub repository this descriptor downloads from, for the MAME
    /// contract only. `None` for a Redump descriptor.
    pub fn repository(&self) -> Option<&'static str> {
        match &self.remote {
            ManagedDatRemote::GithubCommitPinned { repository, .. } => Some(repository),
            ManagedDatRemote::DirectHttps { .. } => None,
        }
    }

    /// The repository-relative path this descriptor downloads, for the MAME
    /// contract only. `None` for a Redump descriptor.
    pub fn repository_relative_path(&self) -> Option<&Path> {
        match &self.remote {
            ManagedDatRemote::GithubCommitPinned {
                repository_relative_path,
                ..
            } => Some(repository_relative_path),
            ManagedDatRemote::DirectHttps { .. } => None,
        }
    }

    pub fn expected_ecosystem(&self) -> DatEcosystem {
        self.expected_ecosystem
    }

    /// The exact expected `<softwarelist name="...">`, for the MAME
    /// contract only. Empty for a Redump descriptor - use
    /// [`Self::redump_bios_system`] there instead.
    pub fn expected_softwarelist_name(&self) -> &str {
        match &self.expected_dataset {
            ExpectedDataset::MameSoftwareList(name) => name,
            ExpectedDataset::RedumpBios(_) | ExpectedDataset::RedumpGames(_) => "",
        }
    }

    /// The fixed Redump BIOS system this descriptor represents, for the
    /// Redump BIOS contract only. `None` otherwise.
    pub fn redump_bios_system(&self) -> Option<RedumpBiosSystem> {
        match &self.expected_dataset {
            ExpectedDataset::RedumpBios(system) => Some(*system),
            ExpectedDataset::MameSoftwareList(_) | ExpectedDataset::RedumpGames(_) => None,
        }
    }

    /// The fixed Redump ordinary game-DAT system this descriptor
    /// represents, for the Redump game contract only. `None` otherwise.
    pub fn redump_games_system(&self) -> Option<RedumpGameSystem> {
        match &self.expected_dataset {
            ExpectedDataset::RedumpGames(system) => Some(*system),
            ExpectedDataset::MameSoftwareList(_) | ExpectedDataset::RedumpBios(_) => None,
        }
    }

    /// A meaningful authoritative-dataset label for [`ManagedDatState`]
    /// provenance, for either provider - MAME's exact software-list name,
    /// or Redump's human-readable dataset label (e.g.
    /// `"Sony - PlayStation 2 - BIOS Images"`), never the empty string
    /// [`Self::expected_softwarelist_name`] returns for a Redump
    /// descriptor.
    fn expected_authoritative_name(&self) -> String {
        match &self.expected_dataset {
            ExpectedDataset::MameSoftwareList(name) => name.clone(),
            ExpectedDataset::RedumpBios(system) => {
                system.firmware_system().redump_dataset_label().to_string()
            }
            ExpectedDataset::RedumpGames(system) => system.dataset_label().to_string(),
        }
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

    /// Validates that this still represents one of the built-in contracts.
    pub fn validate(&self) -> Result<()> {
        self.source_id.validate()?;
        match (
            &self.source_id.provider,
            &self.remote,
            &self.expected_dataset,
        ) {
            (
                ManagedDatProvider::MameSoftwareList,
                ManagedDatRemote::GithubCommitPinned {
                    repository,
                    repository_relative_path,
                },
                ExpectedDataset::MameSoftwareList(name),
            ) => {
                if *repository != MAME_REPOSITORY
                    || self.expected_ecosystem != DatEcosystem::MAMESoftwareList
                    || name != &self.source_id.source_key
                {
                    return Err(config_error(
                        "managed DAT descriptor is not the fixed MAME software-list contract",
                    ));
                }
                validate_repository_relative_path(repository_relative_path)?;
                let expected_path =
                    PathBuf::from("hash").join(format!("{}.xml", self.source_id.source_key));
                if repository_relative_path != &expected_path {
                    return Err(config_error(
                        "managed MAME software-list path does not match its typed source ID",
                    ));
                }
            }
            (
                ManagedDatProvider::RedumpBios,
                ManagedDatRemote::DirectHttps { url },
                ExpectedDataset::RedumpBios(system),
            ) => {
                if system.slug() != self.source_id.source_key
                    || self.expected_ecosystem != DatEcosystem::Redump
                    || *url != system.fixed_url()
                {
                    return Err(config_error(
                        "managed DAT descriptor is not a fixed Redump BIOS contract",
                    ));
                }
            }
            (
                ManagedDatProvider::RedumpGames,
                ManagedDatRemote::DirectHttps { url },
                ExpectedDataset::RedumpGames(system),
            ) => {
                if system.slug() != self.source_id.source_key
                    || self.expected_ecosystem != DatEcosystem::Redump
                    || *url != system.fixed_url()
                {
                    return Err(config_error(
                        "managed DAT descriptor is not a fixed Redump game contract",
                    ));
                }
            }
            _ => {
                return Err(config_error(
                    "managed DAT descriptor fields do not match a known provider contract",
                ));
            }
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
            authoritative_name: descriptor.expected_authoritative_name(),
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
            || self.authoritative_name != descriptor.expected_authoritative_name()
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

/// One immutable managed object that is not named by the source's current or
/// retained previous state. This is intentionally an inspection result only:
/// no API here deletes, moves, or promotes an orphan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatOrphanedObject {
    pub snapshot: ManagedDatSnapshot,
    pub path: PathBuf,
    pub size_bytes: u64,
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

/// Reconstructs the fixed built-in descriptor a source ID names, regardless
/// of which provider it belongs to. This is how [`save_managed_dat_state`]/
/// [`validate_managed_snapshot_ownership`] can revalidate a state record
/// without knowing in advance whether it is MAME or Redump - the exact
/// provider is read from `source_id.provider`, never guessed or accepted
/// from unrelated caller input.
fn descriptor_from_source_id(source_id: &ManagedDatSourceId) -> Result<ManagedDatSourceDescriptor> {
    match source_id.provider {
        ManagedDatProvider::MameSoftwareList => {
            ManagedDatSourceDescriptor::mame_software_list(source_id.source_key.clone())
        }
        ManagedDatProvider::RedumpBios => {
            let system = RedumpBiosSystem::from_slug(&source_id.source_key).ok_or_else(|| {
                config_error("managed DAT state names an unknown Redump BIOS system")
            })?;
            ManagedDatSourceDescriptor::redump_bios(system)
        }
        ManagedDatProvider::RedumpGames => {
            let system = RedumpGameSystem::from_slug(&source_id.source_key).ok_or_else(|| {
                config_error("managed DAT state names an unknown Redump game system")
            })?;
            ManagedDatSourceDescriptor::redump_games(system)
        }
    }
}

/// Atomically saves a state record below the managed root.  It never accepts a
/// state-file destination outside the typed source's storage directory.
pub fn save_managed_dat_state(managed_root: &Path, state: &ManagedDatState) -> Result<()> {
    let descriptor = descriptor_from_source_id(&state.source_id)?;
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
    let descriptor = descriptor_from_source_id(&state.source_id)?;
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
    resolve_managed_dat_snapshot_source(managed_root, state, &state.current_snapshot)
}

/// Resolves either the current or retained previous snapshot as a normal
/// read-only DAT input.  Callers cannot supply an arbitrary filesystem path:
/// the snapshot must be named by the validated managed state.
pub fn resolve_managed_dat_snapshot_source(
    managed_root: &Path,
    state: &ManagedDatState,
    snapshot: &ManagedDatSnapshot,
) -> Result<ManagedDatReadOnlySource> {
    let path = validate_managed_snapshot_ownership(managed_root, state, snapshot)?;
    Ok(ManagedDatReadOnlySource {
        source_id: state.source_id.clone(),
        path,
    })
}

/// Lists unreferenced immutable objects for one typed managed source.
///
/// This is wholly offline and read-only. Every entry is required to be a
/// regular non-symlink file with a valid SHA-256 filename under the source's
/// app-owned `objects` directory. A malformed entry or a path/symlink escape
/// is rejected instead of being silently ignored or treated as deletable.
///
/// If no state exists yet, every valid immutable object is reported as
/// unreferenced. Callers must make any cleanup decision explicitly; this
/// function intentionally offers no deletion operation.
pub fn list_managed_dat_orphaned_objects(
    managed_root: &Path,
    descriptor: &ManagedDatSourceDescriptor,
) -> Result<Vec<ManagedDatOrphanedObject>> {
    descriptor.validate()?;
    let source_dir = managed_source_dir(managed_root, descriptor.source_id())?;
    let objects_dir = source_dir.join(OBJECTS_DIRECTORY);
    ensure_existing_path_is_not_symlinked(managed_root, &objects_dir)?;
    let entries = match fs::read_dir(&objects_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ArchiveFsError::io(objects_dir, error)),
    };
    let objects_metadata = fs::symlink_metadata(&objects_dir)
        .map_err(|source| ArchiveFsError::io(objects_dir.clone(), source))?;
    if !objects_metadata.is_dir() || objects_metadata.file_type().is_symlink() {
        return Err(config_error(format!(
            "managed DAT objects directory is not a real directory: {}",
            objects_dir.display()
        )));
    }

    let state = load_optional_managed_dat_state(managed_root, descriptor)?;
    let current = state
        .as_ref()
        .map(|state| state.current_snapshot.sha256.as_str());
    let previous = state
        .as_ref()
        .and_then(|state| state.previous_snapshot.as_ref())
        .map(|snapshot| snapshot.sha256.as_str());
    let mut orphans = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ArchiveFsError::io(objects_dir.clone(), source))?;
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(config_error(format!(
                "managed DAT object filename is not valid UTF-8: {}",
                path.display()
            )));
        };
        let snapshot = ManagedDatSnapshot::new(name)?;
        let expected = objects_dir.join(&snapshot.sha256);
        if path != expected {
            return Err(config_error("managed DAT object path was not canonical"));
        }
        ensure_existing_path_is_not_symlinked(managed_root, &path)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| ArchiveFsError::io(path.clone(), source))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(config_error(format!(
                "managed DAT object is not a regular non-symlink file: {}",
                path.display()
            )));
        }
        if current == Some(snapshot.sha256.as_str()) || previous == Some(snapshot.sha256.as_str()) {
            continue;
        }
        orphans.push(ManagedDatOrphanedObject {
            snapshot,
            path,
            size_bytes: metadata.len(),
        });
    }
    orphans.sort_by(|left, right| left.snapshot.sha256.cmp(&right.snapshot.sha256));
    Ok(orphans)
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

/// The HTTP request shape used by the managed-DAT updater.  URLs are created
/// internally from a validated built-in descriptor; callers never supply one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatHttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// Response metadata retained for explicit update decisions and provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatHttpResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub retry_after_seconds: Option<u64>,
    pub downloaded_bytes: u64,
}

/// Transport failures are classified rather than made actionable by matching
/// prose.  A GUI may present `detail`, but logic should use `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedDatTransportFailureKind {
    Offline,
    Timeout,
    Network,
    Tls,
    InvalidResponse,
    Destination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatTransportError {
    pub kind: ManagedDatTransportFailureKind,
    pub detail: String,
}

impl ManagedDatTransportError {
    fn new(kind: ManagedDatTransportFailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

/// Narrow synchronous seam for tests and the future GUI worker.  It performs
/// one serial request and streams any successful response into `destination`.
pub trait ManagedDatTransport {
    fn get(
        &self,
        request: &ManagedDatHttpRequest,
        maximum_bytes: u64,
        destination: &mut dyn Write,
    ) -> std::result::Result<ManagedDatHttpResponse, ManagedDatTransportError>;
}

/// Rustls-backed HTTPS transport.  It has no redirect policy because both
/// approved hosts and immutable URL shapes are fixed by this module.
#[derive(Debug, Clone)]
pub struct HttpsManagedDatTransport {
    agent: ureq::Agent,
}

impl HttpsManagedDatTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(MANAGED_DAT_OVERALL_TIMEOUT))
            .timeout_resolve(Some(MANAGED_DAT_CONNECT_TIMEOUT))
            .timeout_connect(Some(MANAGED_DAT_CONNECT_TIMEOUT))
            .timeout_recv_response(None)
            .timeout_recv_body(Some(MANAGED_DAT_IDLE_TIMEOUT))
            .build();
        Self {
            agent: config.new_agent(),
        }
    }
}

impl Default for HttpsManagedDatTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ManagedDatTransport for HttpsManagedDatTransport {
    fn get(
        &self,
        request: &ManagedDatHttpRequest,
        maximum_bytes: u64,
        destination: &mut dyn Write,
    ) -> std::result::Result<ManagedDatHttpResponse, ManagedDatTransportError> {
        validate_managed_dat_http_url(&request.url)?;
        let mut call = self
            .agent
            .get(&request.url)
            .config()
            .timeout_global(Some(MANAGED_DAT_OVERALL_TIMEOUT))
            .build()
            .header("Accept-Encoding", "identity")
            .header("User-Agent", concat!("EmuWiz/", env!("CARGO_PKG_VERSION")));
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        let mut response = call.call().map_err(classify_managed_dat_ureq_error)?;
        let header_bytes = response
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum::<usize>();
        if header_bytes > MANAGED_DAT_HEADER_LIMIT {
            return Err(ManagedDatTransportError::new(
                ManagedDatTransportFailureKind::InvalidResponse,
                "response headers exceed the managed DAT limit",
            ));
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let content_length = match header("content-length") {
            Some(value) => Some(value.parse::<u64>().map_err(|_| {
                ManagedDatTransportError::new(
                    ManagedDatTransportFailureKind::InvalidResponse,
                    "response Content-Length is invalid",
                )
            })?),
            None => None,
        };
        let etag = header("etag");
        let last_modified = header("last-modified");
        let retry_after_seconds = header("retry-after").and_then(|value| value.parse::<u64>().ok());
        if content_length.is_some_and(|size| size > maximum_bytes) {
            return Err(ManagedDatTransportError::new(
                ManagedDatTransportFailureKind::InvalidResponse,
                "response Content-Length exceeds the managed DAT limit",
            ));
        }
        let status = response.status().as_u16();
        let mut downloaded_bytes = 0u64;
        if (200..300).contains(&status) {
            let mut reader = response.body_mut().as_reader();
            let mut buffer = [0u8; MANAGED_DAT_NETWORK_CHUNK];
            loop {
                let count = reader.read(&mut buffer).map_err(|error| {
                    ManagedDatTransportError::new(
                        if error.kind() == std::io::ErrorKind::TimedOut {
                            ManagedDatTransportFailureKind::Timeout
                        } else {
                            ManagedDatTransportFailureKind::Network
                        },
                        error.to_string(),
                    )
                })?;
                if count == 0 {
                    break;
                }
                downloaded_bytes = downloaded_bytes.saturating_add(count as u64);
                if downloaded_bytes > maximum_bytes {
                    return Err(ManagedDatTransportError::new(
                        ManagedDatTransportFailureKind::InvalidResponse,
                        "received body exceeds the managed DAT limit",
                    ));
                }
                destination.write_all(&buffer[..count]).map_err(|error| {
                    ManagedDatTransportError::new(
                        ManagedDatTransportFailureKind::Destination,
                        error.to_string(),
                    )
                })?;
            }
        }
        Ok(ManagedDatHttpResponse {
            status,
            content_length,
            etag,
            last_modified,
            retry_after_seconds,
            downloaded_bytes,
        })
    }
}

/// Structured result of a manual check or update request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedDatUpdateOutcome {
    Disabled,
    UpToDate {
        upstream_revision: Option<String>,
    },
    UpdateAvailable {
        upstream_revision: String,
    },
    Updated {
        upstream_revision: String,
        sha256: String,
    },
    Offline,
    RateLimited {
        retry_after_seconds: Option<u64>,
    },
    Failed {
        kind: ManagedDatUpdateFailureKind,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedDatUpdateFailureKind {
    Network,
    Timeout,
    Tls,
    Forbidden,
    NotFound,
    HttpStatus,
    InvalidResponse,
    DownloadTooLarge,
    EmptyDownload,
    TruncatedDownload,
    Parser,
    WrongEcosystem,
    WrongAuthoritativeName,
    EmptyCatalogue,
    Storage,
}

/// Options shared by the explicit check and update APIs.  `offline` is an
/// affirmative no-network mode, never inferred from a failure.
#[derive(Debug, Clone)]
pub struct ManagedDatUpdateOptions {
    pub managed_root: PathBuf,
    pub offline: bool,
    pub now_unix_seconds: u64,
}

impl ManagedDatUpdateOptions {
    pub fn new(managed_root: PathBuf, now_unix_seconds: u64) -> Self {
        Self {
            managed_root,
            offline: false,
            now_unix_seconds,
        }
    }
}

/// Checks whether an update is available for `descriptor`, dispatching by
/// provider - see [`check_mame_update`] for MAME's immutable-commit model
/// and [`check_redump_bios_update`] for Redump's single-URL model. Neither
/// branch downloads a full XML/DAT into the managed object store or changes
/// a current/previous snapshot; each only persists check metadata for an
/// already installed state.
pub fn check_managed_dat_update(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    descriptor.validate()?;
    if descriptor.update_policy() == ManagedDatUpdatePolicy::Disabled {
        return Ok(ManagedDatUpdateOutcome::Disabled);
    }
    if options.offline {
        return Ok(ManagedDatUpdateOutcome::Offline);
    }
    match descriptor.source_id().provider {
        ManagedDatProvider::MameSoftwareList => check_mame_update(descriptor, options, transport),
        ManagedDatProvider::RedumpBios => check_redump_bios_update(descriptor, options, transport),
        ManagedDatProvider::RedumpGames => {
            check_redump_games_update(descriptor, options, transport)
        }
    }
}

fn check_mame_update(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    let existing = load_optional_managed_dat_state(&options.managed_root, descriptor)?;
    let revision = match resolve_mame_revision(descriptor, existing.as_ref(), transport) {
        Ok(revision) => revision,
        Err(outcome) => return Ok(outcome),
    };
    if revision.not_modified
        || existing.as_ref().is_some_and(|state| {
            state.upstream_revision.as_deref() == Some(revision.label.as_str())
        })
    {
        if let Some(mut state) = existing {
            state.last_checked_at_unix_seconds = Some(options.now_unix_seconds);
            state.etag = revision.etag.or(state.etag);
            state.last_modified = revision.last_modified.or(state.last_modified);
            if let Err(error) = save_managed_dat_state(&options.managed_root, &state) {
                return Ok(storage_failure(error));
            }
            return Ok(ManagedDatUpdateOutcome::UpToDate {
                upstream_revision: state.upstream_revision,
            });
        }
        return Ok(ManagedDatUpdateOutcome::UpToDate {
            upstream_revision: None,
        });
    }
    Ok(ManagedDatUpdateOutcome::UpdateAvailable {
        upstream_revision: revision.label,
    })
}

/// Downloads and validates the XML at a revision resolved during this call,
/// dispatching by provider - see [`update_mame_dat`]/[`update_redump_bios`].
/// No bytes become current until they are parsed, validated, content-
/// addressed, and the next state record has been atomically written.
pub fn update_managed_dat(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    descriptor.validate()?;
    if descriptor.update_policy() == ManagedDatUpdatePolicy::Disabled {
        return Ok(ManagedDatUpdateOutcome::Disabled);
    }
    if options.offline {
        return Ok(ManagedDatUpdateOutcome::Offline);
    }
    match descriptor.source_id().provider {
        ManagedDatProvider::MameSoftwareList => update_mame_dat(descriptor, options, transport),
        ManagedDatProvider::RedumpBios => update_redump_bios(descriptor, options, transport),
        ManagedDatProvider::RedumpGames => update_redump_games(descriptor, options, transport),
    }
}

fn update_mame_dat(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    let existing = load_optional_managed_dat_state(&options.managed_root, descriptor)?;
    let revision = match resolve_mame_revision(descriptor, existing.as_ref(), transport) {
        Ok(revision) => revision,
        Err(outcome) => return Ok(outcome),
    };
    if revision.not_modified {
        return mark_up_to_date(existing, options, revision);
    }
    if existing
        .as_ref()
        .is_some_and(|state| state.upstream_revision.as_deref() == Some(revision.label.as_str()))
    {
        return mark_up_to_date(existing, options, revision);
    }

    let source_dir = match create_managed_source_dir(&options.managed_root, descriptor.source_id())
    {
        Ok(()) => managed_source_dir(&options.managed_root, descriptor.source_id())?,
        Err(error) => return Ok(storage_failure(error)),
    };
    let staging = match create_private_staging_file(&options.managed_root, &source_dir) {
        Ok(file) => file,
        Err(error) => return Ok(storage_failure(error)),
    };
    let _cleanup = ManagedDatStagingCleanup(staging.path.clone());
    let sha256 = match download_mame_xml(descriptor, &revision.label, &staging.path, transport) {
        Ok(sha256) => sha256,
        Err(outcome) => return Ok(outcome),
    };
    let parsed = match crate::dat::parsers::parse_dat_file(
        &staging.path,
        crate::dat::limits::DatLimits::default(),
    ) {
        Ok(outcome) => outcome.dat,
        Err(error) => {
            return Ok(ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::Parser,
                detail: error.to_string(),
            });
        }
    };
    if parsed.source.ecosystem != descriptor.expected_ecosystem() {
        return Ok(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::WrongEcosystem,
            detail: format!(
                "expected {}, received {}",
                descriptor.expected_ecosystem().label(),
                parsed.source.ecosystem.label()
            ),
        });
    }
    if parsed.source.name.as_deref() != Some(descriptor.expected_softwarelist_name()) {
        return Ok(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
            detail: "parsed MAME software-list name differs from typed descriptor".to_string(),
        });
    }
    if parsed.games.is_empty() || parsed.source.entry_count == 0 {
        return Ok(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::EmptyCatalogue,
            detail: "MAME software-list contains no software records".to_string(),
        });
    }
    publish_validated_snapshot(
        descriptor,
        options,
        existing,
        revision,
        sha256,
        staging.path,
        format!(
            "validated MAME software-list with {} records",
            parsed.source.entry_count
        ),
    )
}

/// Resolved provenance metadata for one check/update pass - MAME's
/// immutable commit SHA, or Redump's parsed DAT header version (or, absent
/// that, a digest-derived label - see [`update_redump_bios`]'s own doc
/// comment). Despite the field name, `label` is never treated as itself
/// proving bytes changed - see [`ManagedDatState::sha256`]'s own doc
/// comment: the content digest is always the authority for that.
#[derive(Debug)]
struct ResolvedRevisionMeta {
    label: String,
    etag: Option<String>,
    last_modified: Option<String>,
    not_modified: bool,
}

fn resolve_mame_revision(
    descriptor: &ManagedDatSourceDescriptor,
    existing: Option<&ManagedDatState>,
    transport: &dyn ManagedDatTransport,
) -> std::result::Result<ResolvedRevisionMeta, ManagedDatUpdateOutcome> {
    let mut headers = vec![(
        "Accept".to_string(),
        "application/vnd.github+json".to_string(),
    )];
    if let Some(state) = existing {
        if let Some(etag) = &state.etag {
            headers.push(("If-None-Match".to_string(), etag.clone()));
        }
        if let Some(last_modified) = &state.last_modified {
            headers.push(("If-Modified-Since".to_string(), last_modified.clone()));
        }
    }
    let repository = descriptor
        .repository()
        .expect("resolve_mame_revision is only ever called with a MAME descriptor");
    let request = ManagedDatHttpRequest {
        url: format!("https://{GITHUB_API_HOST}/repos/{repository}/commits/master"),
        headers,
    };
    let mut bytes = Vec::new();
    let response = transport
        .get(&request, 64 * 1024, &mut bytes)
        .map_err(transport_failure)?;
    match response.status {
        304 => Ok(ResolvedRevisionMeta {
            label: existing
                .and_then(|state| state.upstream_revision.clone())
                .unwrap_or_default(),
            etag: response.etag,
            last_modified: response.last_modified,
            not_modified: true,
        }),
        200 => {
            if response
                .content_length
                .is_some_and(|length| length != response.downloaded_bytes)
                || bytes.len() as u64 != response.downloaded_bytes
            {
                return Err(ManagedDatUpdateOutcome::Failed {
                    kind: ManagedDatUpdateFailureKind::TruncatedDownload,
                    detail: "revision response was truncated".to_string(),
                });
            }
            let commit = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|value| {
                    value
                        .get("sha")
                        .and_then(|sha| sha.as_str())
                        .map(str::to_owned)
                })
                .filter(|sha| is_git_commit_sha(sha))
                .ok_or_else(|| ManagedDatUpdateOutcome::Failed {
                    kind: ManagedDatUpdateFailureKind::InvalidResponse,
                    detail: "GitHub revision response did not contain a commit SHA".to_string(),
                })?;
            Ok(ResolvedRevisionMeta {
                label: commit,
                etag: response.etag,
                last_modified: response.last_modified,
                not_modified: false,
            })
        }
        status => Err(http_failure(status, response.retry_after_seconds)),
    }
}

fn download_mame_xml(
    descriptor: &ManagedDatSourceDescriptor,
    commit: &str,
    staging_path: &Path,
    transport: &dyn ManagedDatTransport,
) -> std::result::Result<String, ManagedDatUpdateOutcome> {
    if !is_git_commit_sha(commit) {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::InvalidResponse,
            detail: "resolved revision is not a commit SHA".to_string(),
        });
    }
    let repository = descriptor
        .repository()
        .expect("download_mame_xml is only ever called with a MAME descriptor");
    let repository_relative_path = descriptor
        .repository_relative_path()
        .expect("download_mame_xml is only ever called with a MAME descriptor");
    let request = ManagedDatHttpRequest {
        url: format!(
            "https://{GITHUB_RAW_HOST}/{repository}/{commit}/{}",
            repository_relative_path.to_string_lossy()
        ),
        headers: Vec::new(),
    };
    let (response, sha256) = download_to_staging(
        &request,
        staging_path,
        descriptor.max_payload_size(),
        "MAME software-list",
        transport,
    )?;
    match response.status {
        200 => {}
        status => return Err(http_failure(status, response.retry_after_seconds)),
    }
    Ok(sha256)
}

/// Downloads `request` into `staging_path` through a SHA-256-hashing
/// writer, then validates it is non-empty, within `max_payload_size`, and
/// not truncated relative to any declared `Content-Length`. Shared by both
/// [`download_mame_xml`] and [`update_redump_bios`]/
/// [`check_redump_bios_update`] - the only provider-specific behavior is
/// the resource label used in error details.
fn download_to_staging(
    request: &ManagedDatHttpRequest,
    staging_path: &Path,
    max_payload_size: u64,
    resource_label: &str,
    transport: &dyn ManagedDatTransport,
) -> std::result::Result<(ManagedDatHttpResponse, String), ManagedDatUpdateOutcome> {
    let file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(staging_path)
        .map_err(|error| storage_failure(ArchiveFsError::io(staging_path.to_path_buf(), error)))?;
    let mut writer = HashingWriter::new(file);
    let response = transport
        .get(request, max_payload_size, &mut writer)
        .map_err(transport_failure)?;
    writer
        .flush()
        .map_err(|error| storage_failure(ArchiveFsError::io(staging_path.to_path_buf(), error)))?;
    let sha256 = digest_hex(writer.hasher.finalize());
    let actual = fs::metadata(staging_path)
        .map_err(|error| storage_failure(ArchiveFsError::io(staging_path.to_path_buf(), error)))?
        .len();
    // Status is classified before any body-size validation, exactly the
    // order the original MAME-only download function used - a 403/404/429
    // error page body must never be misreported as an empty/oversized/
    // truncated *successful* download. 304 (conditional GET, Redump only)
    // carries no body to validate and is returned as-is for the caller to
    // interpret; every other non-200 status is a hard transport failure.
    if response.status != 200 && response.status != 304 {
        return Err(http_failure(response.status, response.retry_after_seconds));
    }
    if response.status == 304 {
        return Ok((response, sha256));
    }
    if actual == 0 || response.downloaded_bytes == 0 {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::EmptyDownload,
            detail: format!("{resource_label} response was empty"),
        });
    }
    if response
        .content_length
        .is_some_and(|length| length > max_payload_size)
    {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::DownloadTooLarge,
            detail: format!(
                "{resource_label} declared Content-Length exceeds the configured limit"
            ),
        });
    }
    if actual > max_payload_size || response.downloaded_bytes > max_payload_size {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::DownloadTooLarge,
            detail: format!("{resource_label} response exceeded the configured limit"),
        });
    }
    if response
        .content_length
        .is_some_and(|length| length != response.downloaded_bytes)
        || actual != response.downloaded_bytes
    {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::TruncatedDownload,
            detail: format!("{resource_label} response length did not match bytes received"),
        });
    }
    Ok((response, sha256))
}

/// Writes a validated DAT's bytes into the managed object store and
/// atomically updates state to name it as current, retaining the prior
/// current snapshot as previous. Shared by MAME and Redump - the caller has
/// already parsed and fully validated the DAT before this is called; this
/// function only ever performs content-addressed storage and state
/// bookkeeping, never parsing or dataset-identity validation itself.
fn publish_validated_snapshot(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    existing: Option<ManagedDatState>,
    revision: ResolvedRevisionMeta,
    sha256: String,
    staging_path: PathBuf,
    validation_summary: String,
) -> Result<ManagedDatUpdateOutcome> {
    let snapshot = ManagedDatSnapshot::new(sha256.clone())?;
    let source_dir = managed_source_dir(&options.managed_root, descriptor.source_id())?;
    let objects = source_dir.join(OBJECTS_DIRECTORY);
    fs::create_dir_all(&objects).map_err(|error| ArchiveFsError::io(objects.clone(), error))?;
    ensure_existing_path_is_not_symlinked(&options.managed_root, &objects)?;
    let object_path = objects.join(&sha256);
    if object_path.exists() {
        let existing_digest = sha256_file(&object_path)?;
        if existing_digest != sha256 {
            return Ok(ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::Storage,
                detail: "existing managed DAT object does not match its content-addressed name"
                    .to_string(),
            });
        }
    } else {
        fs::rename(&staging_path, &object_path)
            .map_err(|error| ArchiveFsError::io(object_path.clone(), error))?;
    }

    let old_current = existing
        .as_ref()
        .map(|state| state.current_snapshot.clone());
    let old_previous = existing
        .as_ref()
        .and_then(|state| state.previous_snapshot.clone());
    let mut state = existing.unwrap_or_else(|| {
        ManagedDatState::new(descriptor, snapshot.clone()).expect("validated descriptor and digest")
    });
    if state.current_snapshot != snapshot {
        state.previous_snapshot = Some(state.current_snapshot.clone());
        state.current_snapshot = snapshot.clone();
        state.sha256 = sha256.clone();
    }
    state.upstream_revision = Some(revision.label.clone());
    state.etag = revision.etag;
    state.last_modified = revision.last_modified;
    state.retrieved_at_unix_seconds = Some(options.now_unix_seconds);
    state.last_checked_at_unix_seconds = Some(options.now_unix_seconds);
    state.parsed_ecosystem = descriptor.expected_ecosystem();
    state.authoritative_name = descriptor.expected_authoritative_name();
    state.validation_summary = Some(validation_summary);
    state.last_failure = None;
    if let Err(error) = save_managed_dat_state(&options.managed_root, &state) {
        return Ok(storage_failure(error));
    }
    // Only after the state pointer is durable may an object older than the
    // retained previous snapshot be forgotten.  A failed cleanup leaves an
    // unreachable immutable object, never damages the active source.
    if let Some(old_previous) = old_previous {
        if old_current.as_ref() != Some(&snapshot) && old_previous != snapshot {
            let obsolete = objects.join(old_previous.sha256);
            let _ = fs::remove_file(obsolete);
        }
    }
    Ok(ManagedDatUpdateOutcome::Updated {
        upstream_revision: revision.label,
        sha256,
    })
}

fn mark_up_to_date(
    existing: Option<ManagedDatState>,
    options: &ManagedDatUpdateOptions,
    revision: ResolvedRevisionMeta,
) -> Result<ManagedDatUpdateOutcome> {
    let Some(mut state) = existing else {
        return Ok(ManagedDatUpdateOutcome::UpToDate {
            upstream_revision: None,
        });
    };
    state.last_checked_at_unix_seconds = Some(options.now_unix_seconds);
    state.etag = revision.etag.or(state.etag);
    state.last_modified = revision.last_modified.or(state.last_modified);
    if let Err(error) = save_managed_dat_state(&options.managed_root, &state) {
        return Ok(storage_failure(error));
    }
    Ok(ManagedDatUpdateOutcome::UpToDate {
        upstream_revision: state.upstream_revision,
    })
}

/// The validated result of [`fetch_and_validate_redump_bios`].
struct FetchedRedumpBios {
    response: ManagedDatHttpResponse,
    sha256: String,
    upstream_version: Option<String>,
    entry_count: usize,
}

/// Downloads `descriptor`'s fixed Redump BIOS URL into a private staging
/// file and fully validates it (parse, ecosystem, dataset identity,
/// non-empty usable evidence) - but never renames it into the managed
/// object store or touches `objects/`. Returns `Ok(None)` for a `304 Not
/// Modified` conditional response (no body to validate). Used by both
/// [`check_redump_bios_update`] (which only ever wants the resulting digest
/// and header version to decide `UpToDate`/`UpdateAvailable`) and
/// [`update_redump_bios`] (which additionally promotes the validated
/// staging file on success). The caller's [`ManagedDatStagingCleanup`]
/// guard is what actually deletes the staging file in either case.

fn fetch_and_validate_redump_bios(
    descriptor: &ManagedDatSourceDescriptor,
    system: RedumpBiosSystem,
    existing: Option<&ManagedDatState>,
    staging_path: &Path,
    transport: &dyn ManagedDatTransport,
) -> std::result::Result<Option<FetchedRedumpBios>, ManagedDatUpdateOutcome> {
    let mut headers = Vec::new();
    if let Some(state) = existing {
        if let Some(etag) = &state.etag {
            headers.push(("If-None-Match".to_string(), etag.clone()));
        }
        if let Some(last_modified) = &state.last_modified {
            headers.push(("If-Modified-Since".to_string(), last_modified.clone()));
        }
    }
    let request = ManagedDatHttpRequest {
        url: system.fixed_url().to_string(),
        headers,
    };
    let (response, sha256) = download_to_staging(
        &request,
        staging_path,
        descriptor.max_payload_size(),
        "Redump BIOS DAT",
        transport,
    )?;
    if response.status == 304 {
        return Ok(None);
    }
    if response.status != 200 {
        return Err(http_failure(response.status, response.retry_after_seconds));
    }
    let parsed =
        crate::dat::parsers::parse_dat_file(staging_path, crate::dat::limits::DatLimits::default())
            .map_err(|error| ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::Parser,
                detail: error.to_string(),
            })?
            .dat;
    if parsed.source.ecosystem != descriptor.expected_ecosystem() {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::WrongEcosystem,
            detail: format!(
                "expected {}, received {}",
                descriptor.expected_ecosystem().label(),
                parsed.source.ecosystem.label()
            ),
        });
    }
    if !crate::dat::firmware_evidence::header_identifies_redump_bios_dataset(
        &parsed.source,
        system.firmware_system(),
    ) {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
            detail: format!(
                "downloaded DAT does not identify itself as the {} dataset",
                system.firmware_system().redump_dataset_label()
            ),
        });
    }
    let evidence = crate::dat::firmware_evidence::redump_bios_evidence_from_dat(
        &parsed,
        system.firmware_system(),
    )
    .map_err(|error| ManagedDatUpdateOutcome::Failed {
        kind: ManagedDatUpdateFailureKind::EmptyCatalogue,
        detail: error.to_string(),
    })?;
    Ok(Some(FetchedRedumpBios {
        response,
        sha256,
        upstream_version: parsed.source.version.clone(),
        entry_count: evidence.len(),
    }))
}

/// Checks Redump's fixed BIOS URL for `descriptor`. Redump exposes no
/// separate cheap version endpoint, so this genuinely downloads the (small,
/// bounded) DAT into private staging and fully validates it exactly like
/// [`update_redump_bios`] does - the only difference is that it never
/// renames the staging file into the managed object store and never
/// updates `current_snapshot`/`previous_snapshot`; only check-timestamp/
/// conditional-header metadata on an already-installed state may be
/// persisted. See [`ManagedDatState::sha256`]'s own doc comment: the
/// content digest, not the parsed header version, is what actually decides
/// `UpToDate` vs. `UpdateAvailable` here.
fn check_redump_bios_update(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    let system = descriptor
        .redump_bios_system()
        .expect("check_redump_bios_update is only ever called with a Redump descriptor");
    let existing = load_optional_managed_dat_state(&options.managed_root, descriptor)?;
    let source_dir = match create_managed_source_dir(&options.managed_root, descriptor.source_id())
    {
        Ok(()) => managed_source_dir(&options.managed_root, descriptor.source_id())?,
        Err(error) => return Ok(storage_failure(error)),
    };
    let staging = match create_private_staging_file(&options.managed_root, &source_dir) {
        Ok(file) => file,
        Err(error) => return Ok(storage_failure(error)),
    };
    let _cleanup = ManagedDatStagingCleanup(staging.path.clone());
    let fetched = match fetch_and_validate_redump_bios(
        descriptor,
        system,
        existing.as_ref(),
        &staging.path,
        transport,
    ) {
        Ok(fetched) => fetched,
        Err(outcome) => return Ok(outcome),
    };
    let Some(fetched) = fetched else {
        // 304 Not Modified: bytes are provably unchanged without a body.
        let revision = ResolvedRevisionMeta {
            label: existing
                .as_ref()
                .and_then(|state| state.upstream_revision.clone())
                .unwrap_or_default(),
            etag: None,
            last_modified: None,
            not_modified: true,
        };
        return mark_up_to_date(existing, options, revision);
    };
    let up_to_date = existing
        .as_ref()
        .is_some_and(|state| state.sha256 == fetched.sha256);
    if up_to_date {
        let revision = ResolvedRevisionMeta {
            label: fetched
                .upstream_version
                .unwrap_or_else(|| fetched.sha256[..12].to_string()),
            etag: fetched.response.etag,
            last_modified: fetched.response.last_modified,
            not_modified: false,
        };
        return mark_up_to_date(existing, options, revision);
    }
    Ok(ManagedDatUpdateOutcome::UpdateAvailable {
        upstream_revision: fetched
            .upstream_version
            .unwrap_or_else(|| fetched.sha256[..12].to_string()),
    })
}

/// Downloads, validates, and (only on a genuine content change) promotes
/// Redump's fixed BIOS DAT for `descriptor`'s system. If the downloaded
/// bytes' SHA-256 matches the already-current snapshot, this leaves
/// `current`/`previous` untouched even when the parsed header version
/// differs from what is recorded - see [`ManagedDatState::sha256`]'s own
/// doc comment on why the digest, not the header, is authoritative for
/// whether bytes changed, and the module doc comment on why an upstream
/// header/version change is never treated as forcing a new snapshot.
fn update_redump_bios(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    let system = descriptor
        .redump_bios_system()
        .expect("update_redump_bios is only ever called with a Redump descriptor");
    let existing = load_optional_managed_dat_state(&options.managed_root, descriptor)?;
    let source_dir = match create_managed_source_dir(&options.managed_root, descriptor.source_id())
    {
        Ok(()) => managed_source_dir(&options.managed_root, descriptor.source_id())?,
        Err(error) => return Ok(storage_failure(error)),
    };
    let staging = match create_private_staging_file(&options.managed_root, &source_dir) {
        Ok(file) => file,
        Err(error) => return Ok(storage_failure(error)),
    };
    let _cleanup = ManagedDatStagingCleanup(staging.path.clone());
    let fetched = match fetch_and_validate_redump_bios(
        descriptor,
        system,
        existing.as_ref(),
        &staging.path,
        transport,
    ) {
        Ok(fetched) => fetched,
        Err(outcome) => return Ok(outcome),
    };
    let Some(fetched) = fetched else {
        let revision = ResolvedRevisionMeta {
            label: existing
                .as_ref()
                .and_then(|state| state.upstream_revision.clone())
                .unwrap_or_default(),
            etag: None,
            last_modified: None,
            not_modified: true,
        };
        return mark_up_to_date(existing, options, revision);
    };
    let revision = ResolvedRevisionMeta {
        label: fetched
            .upstream_version
            .clone()
            .unwrap_or_else(|| fetched.sha256[..12].to_string()),
        etag: fetched.response.etag,
        last_modified: fetched.response.last_modified,
        not_modified: false,
    };
    if existing
        .as_ref()
        .is_some_and(|state| state.sha256 == fetched.sha256)
    {
        // Bytes are byte-for-byte identical to the current snapshot - never
        // churn a new object/snapshot just because the upstream header
        // version text changed.
        return mark_up_to_date(existing, options, revision);
    }
    publish_validated_snapshot(
        descriptor,
        options,
        existing,
        revision,
        fetched.sha256,
        staging.path,
        format!(
            "validated Redump {} BIOS DAT with {} usable record(s)",
            system.firmware_system().redump_dataset_label(),
            fetched.entry_count
        ),
    )
}

/// Whether `parsed`'s header text identifies it as Redump's ordinary
/// (non-BIOS) game/disc dataset for `system` - checked across the same
/// header fields [`crate::dat::firmware_evidence::header_identifies_redump_bios_dataset`]
/// checks, using the same PS1-vs-PS2 disambiguation, but requiring "bios"
/// to be *absent* rather than present (task requirement: an ordinary game
/// DAT that happens to look like the BIOS dataset must never be accepted
/// here - the BIOS provider already owns that dataset).
fn header_identifies_redump_game_dataset(
    source: &crate::dat::model::DatSource,
    system: RedumpGameSystem,
) -> bool {
    let fields = [
        &source.name,
        &source.description,
        &source.author,
        &source.version,
    ];
    let joined: String = fields
        .iter()
        .filter_map(|field| field.as_deref())
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if joined.contains("bios") {
        return false;
    }
    let mentions_ps2 = joined.contains("playstation 2")
        || joined.contains("playstation2")
        || joined.contains("ps2");
    match system {
        RedumpGameSystem::PlayStation2 => mentions_ps2,
        RedumpGameSystem::PlayStation => joined.contains("playstation") && !mentions_ps2,
        RedumpGameSystem::Xbox => joined.contains("xbox") && !joined.contains("360"),
    }
}

/// Whether the bytes staged at `path` begin with a ZIP local-file-header
/// signature (`PK\x03\x04`) - the one detail this function inspects. It
/// never trusts a `Content-Type` header or file extension; only these first
/// four bytes decide whether [`fetch_and_validate_redump_games`] attempts a
/// bounded ZIP unwrap before parsing.
fn staged_file_looks_like_zip(path: &Path) -> std::result::Result<bool, ManagedDatUpdateOutcome> {
    let mut file = fs::File::open(path)
        .map_err(|error| storage_failure(ArchiveFsError::io(path.to_path_buf(), error)))?;
    let mut magic = [0_u8; 4];
    let read = file
        .read(&mut magic)
        .map_err(|error| storage_failure(ArchiveFsError::io(path.to_path_buf(), error)))?;
    Ok(read == 4 && magic == *b"PK\x03\x04")
}

/// The validated result of [`fetch_and_validate_redump_games`].
struct FetchedRedumpGames {
    response: ManagedDatHttpResponse,
    sha256: String,
    upstream_version: Option<String>,
    entry_count: usize,
}

/// Downloads `descriptor`'s fixed Redump game-DAT URL into a private
/// staging file, unwraps a ZIP-wrapped response to its single DAT/XML
/// member if present (see [`crate::dat::archive::zip::extract_sole_zip_member`],
/// reusing the same bounded ZIP metadata scan and member-safety checks
/// every other ZIP consumer in this crate relies on - never a second,
/// independent extraction path), and fully validates the resulting bytes
/// (parse, ecosystem, dataset identity for the requested system, not a
/// BIOS dataset, non-empty game list). An HTML/login/error page - which is
/// neither a ZIP nor a well-formed DAT - fails closed at the parse step
/// with [`ManagedDatUpdateFailureKind::Parser`], exactly like any other
/// malformed response. Never renames the staging file into the managed
/// object store; see [`update_redump_games`] for promotion. Returns `Ok(None)`
/// for a `304 Not Modified` conditional response.
fn fetch_and_validate_redump_games(
    descriptor: &ManagedDatSourceDescriptor,
    system: RedumpGameSystem,
    existing: Option<&ManagedDatState>,
    staging_path: &Path,
    transport: &dyn ManagedDatTransport,
) -> std::result::Result<Option<FetchedRedumpGames>, ManagedDatUpdateOutcome> {
    let mut headers = Vec::new();
    if let Some(state) = existing {
        if let Some(etag) = &state.etag {
            headers.push(("If-None-Match".to_string(), etag.clone()));
        }
        if let Some(last_modified) = &state.last_modified {
            headers.push(("If-Modified-Since".to_string(), last_modified.clone()));
        }
    }
    let request = ManagedDatHttpRequest {
        url: system.fixed_url().to_string(),
        headers,
    };
    let (response, downloaded_sha256) = download_to_staging(
        &request,
        staging_path,
        descriptor.max_payload_size(),
        "Redump game DAT",
        transport,
    )?;
    if response.status == 304 {
        return Ok(None);
    }
    if response.status != 200 {
        return Err(http_failure(response.status, response.retry_after_seconds));
    }

    // The content digest that ends up in `ManagedDatState`/no-update-churn
    // comparisons must always be over the *DAT bytes themselves*, never the
    // outer ZIP container (whose own metadata/compression could vary
    // without the actual game data changing) - so a ZIP response replaces
    // the staged bytes with its unwrapped content before hashing continues.
    let sha256 = if staged_file_looks_like_zip(staging_path)? {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let extracted = crate::dat::archive::zip::extract_sole_zip_member(
            staging_path,
            &crate::dat::archive::limits::ArchiveLimits::default(),
            &cancel,
        )
        .map_err(|error| ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::InvalidResponse,
            detail: format!("Redump game DAT ZIP could not be safely unwrapped: {error:?}"),
        })?;
        fs::write(staging_path, &extracted).map_err(|error| {
            storage_failure(ArchiveFsError::io(staging_path.to_path_buf(), error))
        })?;
        sha256_file(staging_path).map_err(storage_failure)?
    } else {
        downloaded_sha256
    };

    let parsed =
        crate::dat::parsers::parse_dat_file(staging_path, crate::dat::limits::DatLimits::default())
            .map_err(|error| ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::Parser,
                detail: error.to_string(),
            })?
            .dat;
    if parsed.source.ecosystem != descriptor.expected_ecosystem() {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::WrongEcosystem,
            detail: format!(
                "expected {}, received {}",
                descriptor.expected_ecosystem().label(),
                parsed.source.ecosystem.label()
            ),
        });
    }
    if !header_identifies_redump_game_dataset(&parsed.source, system) {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
            detail: format!(
                "downloaded DAT does not identify itself as the {} game dataset (or looks like \
                 a BIOS dataset)",
                system.dataset_label()
            ),
        });
    }
    if parsed.games.is_empty() {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::EmptyCatalogue,
            detail: "downloaded Redump game DAT contains no game records".to_string(),
        });
    }
    Ok(Some(FetchedRedumpGames {
        response,
        sha256,
        upstream_version: parsed.source.version.clone(),
        entry_count: parsed.games.len(),
    }))
}

/// Checks Redump's fixed game-DAT URL for `descriptor` - structurally
/// identical to [`check_redump_bios_update`] (same fully-validate-then-
/// compare-digest model, since Redump exposes no separate cheap version
/// endpoint here either), dispatching to [`fetch_and_validate_redump_games`]
/// instead of the BIOS fetch.
fn check_redump_games_update(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    let system = descriptor
        .redump_games_system()
        .expect("check_redump_games_update is only ever called with a Redump games descriptor");
    let existing = load_optional_managed_dat_state(&options.managed_root, descriptor)?;
    let source_dir = match create_managed_source_dir(&options.managed_root, descriptor.source_id())
    {
        Ok(()) => managed_source_dir(&options.managed_root, descriptor.source_id())?,
        Err(error) => return Ok(storage_failure(error)),
    };
    let staging = match create_private_staging_file(&options.managed_root, &source_dir) {
        Ok(file) => file,
        Err(error) => return Ok(storage_failure(error)),
    };
    let _cleanup = ManagedDatStagingCleanup(staging.path.clone());
    let fetched = match fetch_and_validate_redump_games(
        descriptor,
        system,
        existing.as_ref(),
        &staging.path,
        transport,
    ) {
        Ok(fetched) => fetched,
        Err(outcome) => return Ok(outcome),
    };
    let Some(fetched) = fetched else {
        let revision = ResolvedRevisionMeta {
            label: existing
                .as_ref()
                .and_then(|state| state.upstream_revision.clone())
                .unwrap_or_default(),
            etag: None,
            last_modified: None,
            not_modified: true,
        };
        return mark_up_to_date(existing, options, revision);
    };
    let up_to_date = existing
        .as_ref()
        .is_some_and(|state| state.sha256 == fetched.sha256);
    if up_to_date {
        let revision = ResolvedRevisionMeta {
            label: fetched
                .upstream_version
                .unwrap_or_else(|| fetched.sha256[..12].to_string()),
            etag: fetched.response.etag,
            last_modified: fetched.response.last_modified,
            not_modified: false,
        };
        return mark_up_to_date(existing, options, revision);
    }
    Ok(ManagedDatUpdateOutcome::UpdateAvailable {
        upstream_revision: fetched
            .upstream_version
            .unwrap_or_else(|| fetched.sha256[..12].to_string()),
    })
}

/// Downloads, validates, and (only on a genuine content change) promotes
/// Redump's fixed game DAT for `descriptor`'s system - structurally
/// identical to [`update_redump_bios`]: bytes identical to the current
/// snapshot's SHA-256 leave `current`/`previous` untouched even when the
/// parsed header version text differs.
fn update_redump_games(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    let system = descriptor
        .redump_games_system()
        .expect("update_redump_games is only ever called with a Redump games descriptor");
    let existing = load_optional_managed_dat_state(&options.managed_root, descriptor)?;
    let source_dir = match create_managed_source_dir(&options.managed_root, descriptor.source_id())
    {
        Ok(()) => managed_source_dir(&options.managed_root, descriptor.source_id())?,
        Err(error) => return Ok(storage_failure(error)),
    };
    let staging = match create_private_staging_file(&options.managed_root, &source_dir) {
        Ok(file) => file,
        Err(error) => return Ok(storage_failure(error)),
    };
    let _cleanup = ManagedDatStagingCleanup(staging.path.clone());
    let fetched = match fetch_and_validate_redump_games(
        descriptor,
        system,
        existing.as_ref(),
        &staging.path,
        transport,
    ) {
        Ok(fetched) => fetched,
        Err(outcome) => return Ok(outcome),
    };
    let Some(fetched) = fetched else {
        let revision = ResolvedRevisionMeta {
            label: existing
                .as_ref()
                .and_then(|state| state.upstream_revision.clone())
                .unwrap_or_default(),
            etag: None,
            last_modified: None,
            not_modified: true,
        };
        return mark_up_to_date(existing, options, revision);
    };
    let revision = ResolvedRevisionMeta {
        label: fetched
            .upstream_version
            .clone()
            .unwrap_or_else(|| fetched.sha256[..12].to_string()),
        etag: fetched.response.etag,
        last_modified: fetched.response.last_modified,
        not_modified: false,
    };
    if existing
        .as_ref()
        .is_some_and(|state| state.sha256 == fetched.sha256)
    {
        return mark_up_to_date(existing, options, revision);
    }
    publish_validated_snapshot(
        descriptor,
        options,
        existing,
        revision,
        fetched.sha256,
        staging.path,
        format!(
            "validated Redump {} game DAT with {} record(s)",
            system.dataset_label(),
            fetched.entry_count
        ),
    )
}

fn load_optional_managed_dat_state(
    root: &Path,
    descriptor: &ManagedDatSourceDescriptor,
) -> Result<Option<ManagedDatState>> {
    match load_managed_dat_state(root, descriptor) {
        Ok(state) => Ok(Some(state)),
        Err(ArchiveFsError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

struct PrivateStagingFile {
    path: PathBuf,
}

struct ManagedDatStagingCleanup(PathBuf);

impl Drop for ManagedDatStagingCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn create_private_staging_file(root: &Path, source_dir: &Path) -> Result<PrivateStagingFile> {
    let staging = source_dir.join(STAGING_DIRECTORY);
    fs::create_dir_all(&staging).map_err(|error| ArchiveFsError::io(staging.clone(), error))?;
    ensure_existing_path_is_not_symlinked(root, &staging)?;
    for sequence in 0..128u32 {
        let path = staging.join(format!("{}-{sequence}.part", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(PrivateStagingFile { path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ArchiveFsError::io(path, error)),
        }
    }
    Err(config_error(
        "could not allocate a private managed DAT staging file",
    ))
}

struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; MANAGED_DAT_NETWORK_CHUNK];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(digest_hex(hasher.finalize()))
}

fn digest_hex(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn validate_managed_dat_http_url(value: &str) -> std::result::Result<(), ManagedDatTransportError> {
    let url = url::Url::parse(value).map_err(|error| {
        ManagedDatTransportError::new(
            ManagedDatTransportFailureKind::InvalidResponse,
            error.to_string(),
        )
    })?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some(GITHUB_API_HOST | GITHUB_RAW_HOST | REDUMP_HOST)
        )
    {
        return Err(ManagedDatTransportError::new(
            ManagedDatTransportFailureKind::InvalidResponse,
            "managed DAT transport rejected a non-approved HTTPS host",
        ));
    }
    Ok(())
}

fn classify_managed_dat_ureq_error(error: ureq::Error) -> ManagedDatTransportError {
    let detail = error.to_string();
    let lower = detail.to_ascii_lowercase();
    let kind = if lower.contains("timed out") || lower.contains("timeout") {
        ManagedDatTransportFailureKind::Timeout
    } else if lower.contains("tls") || lower.contains("certificate") {
        ManagedDatTransportFailureKind::Tls
    } else {
        ManagedDatTransportFailureKind::Network
    };
    ManagedDatTransportError::new(kind, detail)
}

fn transport_failure(error: ManagedDatTransportError) -> ManagedDatUpdateOutcome {
    ManagedDatUpdateOutcome::Failed {
        kind: match error.kind {
            ManagedDatTransportFailureKind::Timeout => ManagedDatUpdateFailureKind::Timeout,
            ManagedDatTransportFailureKind::Tls => ManagedDatUpdateFailureKind::Tls,
            ManagedDatTransportFailureKind::Offline
            | ManagedDatTransportFailureKind::Network
            | ManagedDatTransportFailureKind::Destination => ManagedDatUpdateFailureKind::Network,
            ManagedDatTransportFailureKind::InvalidResponse => {
                ManagedDatUpdateFailureKind::InvalidResponse
            }
        },
        detail: error.detail,
    }
}

fn http_failure(status: u16, retry_after_seconds: Option<u64>) -> ManagedDatUpdateOutcome {
    match status {
        403 => ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::Forbidden,
            detail: "managed DAT source returned HTTP 403".to_string(),
        },
        404 => ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::NotFound,
            detail: "managed DAT source returned HTTP 404".to_string(),
        },
        429 => ManagedDatUpdateOutcome::RateLimited {
            retry_after_seconds,
        },
        _ => ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::HttpStatus,
            detail: format!("managed DAT source returned HTTP {status}"),
        },
    }
}

fn storage_failure(error: ArchiveFsError) -> ManagedDatUpdateOutcome {
    ManagedDatUpdateOutcome::Failed {
        kind: ManagedDatUpdateFailureKind::Storage,
        detail: error.to_string(),
    }
}

fn is_git_commit_sha(value: &str) -> bool {
    (40..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use crate::dat::limits::DatLimits;
    use crate::dat::parsers::parse_dat_file;
    use crate::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn descriptor() -> ManagedDatSourceDescriptor {
        ManagedDatSourceDescriptor::mame_software_list("gamecom")
            .unwrap()
            .with_update_policy(ManagedDatUpdatePolicy::Manual)
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

    const REV_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const REV_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const REV_C: &str = "cccccccccccccccccccccccccccccccccccccccc";

    #[derive(Clone)]
    struct FakeReply {
        status: u16,
        body: Vec<u8>,
        content_length: Option<u64>,
        downloaded_bytes: Option<u64>,
        etag: Option<String>,
        last_modified: Option<String>,
        retry_after_seconds: Option<u64>,
    }

    impl FakeReply {
        fn ok(body: impl Into<Vec<u8>>) -> Self {
            let body = body.into();
            Self {
                status: 200,
                content_length: Some(body.len() as u64),
                downloaded_bytes: None,
                body,
                etag: None,
                last_modified: None,
                retry_after_seconds: None,
            }
        }

        fn status(status: u16) -> Self {
            Self {
                status,
                body: Vec::new(),
                content_length: Some(0),
                downloaded_bytes: Some(0),
                etag: None,
                last_modified: None,
                retry_after_seconds: None,
            }
        }
    }

    struct FakeTransport {
        replies: RefCell<Vec<std::result::Result<FakeReply, ManagedDatTransportError>>>,
        calls: RefCell<Vec<ManagedDatHttpRequest>>,
    }

    impl FakeTransport {
        fn new(replies: Vec<std::result::Result<FakeReply, ManagedDatTransportError>>) -> Self {
            Self {
                replies: RefCell::new(replies),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl ManagedDatTransport for FakeTransport {
        fn get(
            &self,
            request: &ManagedDatHttpRequest,
            maximum_bytes: u64,
            destination: &mut dyn Write,
        ) -> std::result::Result<ManagedDatHttpResponse, ManagedDatTransportError> {
            self.calls.borrow_mut().push(request.clone());
            let reply = self.replies.borrow_mut().remove(0)?;
            if (200..300).contains(&reply.status) && reply.body.len() as u64 > maximum_bytes {
                return Err(ManagedDatTransportError::new(
                    ManagedDatTransportFailureKind::InvalidResponse,
                    "mock body too large",
                ));
            }
            if (200..300).contains(&reply.status) {
                destination.write_all(&reply.body).map_err(|error| {
                    ManagedDatTransportError::new(
                        ManagedDatTransportFailureKind::Destination,
                        error.to_string(),
                    )
                })?;
            }
            Ok(ManagedDatHttpResponse {
                status: reply.status,
                content_length: reply.content_length,
                etag: reply.etag,
                last_modified: reply.last_modified,
                retry_after_seconds: reply.retry_after_seconds,
                downloaded_bytes: reply.downloaded_bytes.unwrap_or(reply.body.len() as u64),
            })
        }
    }

    fn revision(sha: &str) -> FakeReply {
        FakeReply::ok(format!(r#"{{"sha":"{sha}"}}"#))
    }

    fn mame_xml(name: &str, game: &str) -> Vec<u8> {
        format!(
            r#"<softwarelist name="{name}" description="Test"><software name="{game}"><description>Test</description><year>1997</year><publisher>Test</publisher><part name="cart" interface="cart"><dataarea name="rom" size="1"><rom name="{game}.bin" size="1" crc="00000000"/></dataarea></part></software></softwarelist>"#
        )
        .into_bytes()
    }

    fn update_options(root: PathBuf) -> ManagedDatUpdateOptions {
        ManagedDatUpdateOptions::new(root, 1_700_000_000)
    }

    fn install(root: PathBuf, revision_sha: &str, body: Vec<u8>) -> ManagedDatState {
        let transport =
            FakeTransport::new(vec![Ok(revision(revision_sha)), Ok(FakeReply::ok(body))]);
        let outcome =
            update_managed_dat(&descriptor(), &update_options(root.clone()), &transport).unwrap();
        assert!(matches!(outcome, ManagedDatUpdateOutcome::Updated { .. }));
        load_managed_dat_state(&root, &descriptor()).unwrap()
    }

    // -----------------------------------------------------------------
    // Redump BIOS provider
    // -----------------------------------------------------------------

    fn redump_descriptor(system: RedumpBiosSystem) -> ManagedDatSourceDescriptor {
        ManagedDatSourceDescriptor::redump_bios(system)
            .unwrap()
            .with_update_policy(ManagedDatUpdatePolicy::Manual)
    }

    fn redump_header(system: RedumpBiosSystem) -> &'static str {
        match system {
            RedumpBiosSystem::PlayStation => "Sony - PlayStation - BIOS Images",
            RedumpBiosSystem::PlayStation2 => "Sony - PlayStation 2 - BIOS Images",
            RedumpBiosSystem::Xbox => "Microsoft - Xbox - BIOS Images",
        }
    }

    /// A synthetic, self-hashing Redump BIOS DAT body - never a real Redump
    /// hash or a real BIOS dump.
    fn redump_bios_dat(system: RedumpBiosSystem, game: &str, bytes: &[u8]) -> Vec<u8> {
        let crc32 = crate::identity_source::hashing::Crc32::of(bytes);
        let md5 = {
            use md5::{Digest as _, Md5};
            let digest = Md5::digest(bytes);
            digest_hex(digest)
        };
        let sha1 = {
            use sha1::{Digest as _, Sha1};
            let digest = Sha1::digest(bytes);
            digest_hex(digest)
        };
        let header = redump_header(system);
        format!(
            r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>{header}</name>
        <description>{header}</description>
        <version>20240115</version>
        <author>Redump.info</author>
    </header>
    <game name="{game}">
        <description>{game}</description>
        <rom name="bios.bin" size="{}" crc="{crc32}" md5="{md5}" sha1="{sha1}"/>
    </game>
</datafile>"#,
            bytes.len()
        )
        .into_bytes()
    }

    fn redump_bios_bytes(seed: &str) -> Vec<u8> {
        format!("synthetic managed redump bios bytes - {seed}").into_bytes()
    }

    /// A single-request Redump update: unlike MAME's two-phase resolve/
    /// download, Redump's fixed URL serves the whole DAT in one GET.
    fn install_redump(
        system: RedumpBiosSystem,
        root: PathBuf,
        body: Vec<u8>,
    ) -> ManagedDatUpdateOutcome {
        let transport = FakeTransport::new(vec![Ok(FakeReply::ok(body))]);
        update_redump_dat(system, root, &transport)
    }

    fn update_redump_dat(
        system: RedumpBiosSystem,
        root: PathBuf,
        transport: &FakeTransport,
    ) -> ManagedDatUpdateOutcome {
        update_managed_dat(&redump_descriptor(system), &update_options(root), transport).unwrap()
    }

    #[test]
    fn typed_descriptor_builds_the_correct_internal_provider_for_all_three_systems() {
        for (system, slug, label) in [
            (
                RedumpBiosSystem::PlayStation,
                "playstation",
                "Sony - PlayStation - BIOS Images",
            ),
            (
                RedumpBiosSystem::PlayStation2,
                "playstation2",
                "Sony - PlayStation 2 - BIOS Images",
            ),
            (
                RedumpBiosSystem::Xbox,
                "xbox",
                "Microsoft - Xbox - BIOS Images",
            ),
        ] {
            let descriptor = ManagedDatSourceDescriptor::redump_bios(system).unwrap();
            assert_eq!(
                descriptor.source_id().provider,
                ManagedDatProvider::RedumpBios
            );
            assert_eq!(descriptor.source_id().source_key, slug);
            assert_eq!(descriptor.expected_ecosystem(), DatEcosystem::Redump);
            assert_eq!(descriptor.redump_bios_system(), Some(system));
            assert_eq!(
                descriptor.source_id().to_string(),
                format!("redump-bios/{slug}")
            );
            assert_eq!(system.firmware_system().redump_dataset_label(), label);
            descriptor.validate().unwrap();
        }
    }

    /// Compile-time proof: the only way to build a Redump BIOS descriptor
    /// is through this closed enum - there is no overload or parameter
    /// that accepts a URL, hostname, or free-text dataset name.
    #[test]
    fn redump_bios_constructor_takes_only_the_closed_system_enum() {
        fn assert_signature(_: fn(RedumpBiosSystem) -> Result<ManagedDatSourceDescriptor>) {}
        assert_signature(ManagedDatSourceDescriptor::redump_bios);
    }

    #[test]
    fn arbitrary_redump_source_key_fails_validation() {
        let bogus = ManagedDatSourceId {
            provider: ManagedDatProvider::RedumpBios,
            source_key: "gamecube-bios".to_string(),
        };
        assert!(bogus.validate().is_err());
    }

    #[test]
    fn manual_and_disabled_policy_persist_for_all_three_systems() {
        for system in [
            RedumpBiosSystem::PlayStation,
            RedumpBiosSystem::PlayStation2,
            RedumpBiosSystem::Xbox,
        ] {
            for policy in [
                ManagedDatUpdatePolicy::Manual,
                ManagedDatUpdatePolicy::Disabled,
            ] {
                let descriptor = ManagedDatSourceDescriptor::redump_bios(system)
                    .unwrap()
                    .with_update_policy(policy);
                assert_eq!(descriptor.update_policy(), policy);
                descriptor.validate().unwrap();
            }
        }
    }

    #[test]
    fn valid_synthetic_dat_is_accepted_for_each_system() {
        for system in [
            RedumpBiosSystem::PlayStation,
            RedumpBiosSystem::PlayStation2,
            RedumpBiosSystem::Xbox,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(MANAGED_DAT_DIRECTORY);
            let body = redump_bios_dat(system, "BIOS", &redump_bios_bytes("valid"));
            let outcome = install_redump(system, root, body);
            assert!(
                matches!(outcome, ManagedDatUpdateOutcome::Updated { .. }),
                "{system:?}: expected Updated, got {outcome:?}"
            );
        }
    }

    #[test]
    fn ps1_descriptor_rejects_a_ps2_bios_dat_at_update_time() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let body = redump_bios_dat(
            RedumpBiosSystem::PlayStation2,
            "PS2 BIOS",
            &redump_bios_bytes("cross-system"),
        );
        let outcome = install_redump(RedumpBiosSystem::PlayStation, root, body);
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
                ..
            }
        ));
    }

    #[test]
    fn arbitrary_redump_game_dat_is_rejected_at_update_time() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let body = br#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Sony - PlayStation 2</name>
        <description>Sony - PlayStation 2 Datfile (full game discs)</description>
        <author>Redump.org</author>
    </header>
    <game name="Some Game">
        <rom name="game.bin" size="1" crc="00000000" md5="00000000000000000000000000000000" sha1="0000000000000000000000000000000000000000"/>
    </game>
</datafile>"#
            .to_vec();
        let outcome = install_redump(RedumpBiosSystem::PlayStation2, root, body);
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
                ..
            }
        ));
    }

    #[test]
    fn missing_hashes_are_rejected_as_authoritative_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let header = redump_header(RedumpBiosSystem::PlayStation2);
        let body = format!(
            r#"<?xml version="1.0"?>
<datafile>
    <header><name>{header}</name><description>{header}</description><author>Redump.org</author></header>
    <game name="incomplete"><rom name="bios.bin" size="4" crc="aabbccdd"/></game>
</datafile>"#
        )
        .into_bytes();
        let outcome = install_redump(RedumpBiosSystem::PlayStation2, root, body);
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::EmptyCatalogue,
                ..
            }
        ));
    }

    #[test]
    fn empty_dataset_is_rejected_at_update_time() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let header = redump_header(RedumpBiosSystem::Xbox);
        let body = format!(
            r#"<?xml version="1.0"?>
<datafile><header><name>{header}</name><description>{header}</description><author>Redump.org</author></header></datafile>"#
        )
        .into_bytes();
        let outcome = install_redump(RedumpBiosSystem::Xbox, root, body);
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::EmptyCatalogue,
                ..
            }
        ));
    }

    #[test]
    fn malformed_dat_is_rejected_at_update_time() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        // A `<?xml`/`<datafile` prefix commits the sniffer to the Logiqx
        // path (plain unrecognised text is instead accepted as an empty
        // ClrMamePro catalogue - a different, already-covered rejection
        // reason, and the streaming parser tolerates a body simply cut
        // short). A mismatched closing tag is what genuinely trips the
        // streaming XML parser's own `MalformedXml` error path.
        let outcome = install_redump(
            RedumpBiosSystem::PlayStation,
            root,
            b"<?xml version=\"1.0\"?><datafile><header></wrong></datafile>".to_vec(),
        );
        assert!(
            matches!(
                outcome,
                ManagedDatUpdateOutcome::Failed {
                    kind: ManagedDatUpdateFailureKind::Parser,
                    ..
                }
            ),
            "expected Parser failure, got {outcome:?}"
        );
    }

    #[test]
    fn redump_first_install_up_to_date_changed_bytes_and_no_churn_on_header_only_change() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpBiosSystem::PlayStation2;
        let descriptor = redump_descriptor(system);
        let bytes_v1 = redump_bios_bytes("v1");

        // 20: first install.
        let body_v1 = redump_bios_dat(system, "PS2 BIOS v1", &bytes_v1);
        let outcome = install_redump(system, root.clone(), body_v1.clone());
        let (first_sha256, first_upstream) = match outcome {
            ManagedDatUpdateOutcome::Updated {
                sha256,
                upstream_revision,
            } => (sha256, upstream_revision),
            other => panic!("expected Updated, got {other:?}"),
        };
        let state_after_install = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(state_after_install.current_snapshot.sha256, first_sha256);
        assert!(state_after_install.previous_snapshot.is_none());
        assert_eq!(state_after_install.parsed_ecosystem, DatEcosystem::Redump);

        // 21: identical bytes -> UpToDate, no state churn.
        let outcome = install_redump(system, root.clone(), body_v1.clone());
        assert!(matches!(outcome, ManagedDatUpdateOutcome::UpToDate { .. }));
        let state_unchanged = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(state_unchanged.current_snapshot.sha256, first_sha256);
        assert!(state_unchanged.previous_snapshot.is_none());

        // 24: the recorded state's `upstream_revision` label is mutated to
        // look like a different header version was seen last time (as if a
        // prior check had observed a header/version change), but the
        // actual downloaded bytes handed to this update are byte-for-byte
        // identical to what is already current. The content digest, not
        // the stale label, must still decide - this must not churn a new
        // object/snapshot, proving the digest is genuinely authoritative
        // rather than the header text.
        let mut mutated_label_state = load_managed_dat_state(&root, &descriptor).unwrap();
        mutated_label_state.upstream_revision = Some("99999999".to_string());
        save_managed_dat_state(&root, &mutated_label_state).unwrap();
        let outcome = install_redump(system, root.clone(), body_v1.clone());
        assert!(
            matches!(outcome, ManagedDatUpdateOutcome::UpToDate { .. }),
            "identical downloaded bytes must never churn a snapshot, regardless of the \
             previously recorded header/version label"
        );
        let state_still_unchanged = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(state_still_unchanged.current_snapshot.sha256, first_sha256);
        assert!(state_still_unchanged.previous_snapshot.is_none());

        // 22 + 23: genuinely different bytes -> promotes current, retains
        // the old current as previous.
        let bytes_v2 = redump_bios_bytes("v2 - genuinely different content");
        let body_v2 = redump_bios_dat(system, "PS2 BIOS v2", &bytes_v2);
        let outcome = install_redump(system, root.clone(), body_v2);
        let second_sha256 = match outcome {
            ManagedDatUpdateOutcome::Updated { sha256, .. } => sha256,
            other => panic!("expected Updated, got {other:?}"),
        };
        assert_ne!(second_sha256, first_sha256);
        let state_after_v2 = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(state_after_v2.current_snapshot.sha256, second_sha256);
        assert_eq!(
            state_after_v2.previous_snapshot.as_ref().unwrap().sha256,
            first_sha256
        );
        assert_eq!(
            first_upstream, "20240115",
            "the recorded upstream revision is the DAT's own parsed header version"
        );
    }

    #[test]
    fn redump_403_404_429_and_offline_are_structured_and_do_not_create_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpBiosSystem::Xbox;
        let descriptor = redump_descriptor(system);

        for (reply, expected) in [
            (
                FakeReply::status(403),
                ManagedDatUpdateFailureKind::Forbidden,
            ),
            (
                FakeReply::status(404),
                ManagedDatUpdateFailureKind::NotFound,
            ),
        ] {
            let transport = FakeTransport::new(vec![Ok(reply)]);
            let outcome =
                update_managed_dat(&descriptor, &update_options(root.clone()), &transport).unwrap();
            assert!(matches!(
                outcome,
                ManagedDatUpdateOutcome::Failed { kind, .. } if kind == expected
            ));
            assert!(
                load_optional_managed_dat_state(&root, &descriptor)
                    .unwrap()
                    .is_none()
            );
        }

        let mut rate_limited = FakeReply::status(429);
        rate_limited.retry_after_seconds = Some(120);
        let transport = FakeTransport::new(vec![Ok(rate_limited)]);
        let outcome =
            update_managed_dat(&descriptor, &update_options(root.clone()), &transport).unwrap();
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::RateLimited {
                retry_after_seconds: Some(120)
            }
        ));

        // Offline: zero transport calls, regardless of provider.
        let transport = FakeTransport::new(Vec::new());
        let mut options = update_options(root.clone());
        options.offline = true;
        let outcome = update_managed_dat(&descriptor, &options, &transport).unwrap();
        assert!(matches!(outcome, ManagedDatUpdateOutcome::Offline));
        assert!(transport.calls.borrow().is_empty());
        assert!(
            load_optional_managed_dat_state(&root, &descriptor)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn redump_validation_failure_preserves_the_existing_current_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpBiosSystem::PlayStation;
        let descriptor = redump_descriptor(system);
        let good_bytes = redump_bios_bytes("known-good");
        let good_body = redump_bios_dat(system, "PS1 BIOS", &good_bytes);
        let outcome = install_redump(system, root.clone(), good_body);
        let good_sha256 = match outcome {
            ManagedDatUpdateOutcome::Updated { sha256, .. } => sha256,
            other => panic!("expected Updated, got {other:?}"),
        };

        // A wrong-dataset DAT must never replace the known-good current
        // snapshot, even though it is bytewise well-formed XML.
        let bad_body = redump_bios_dat(RedumpBiosSystem::Xbox, "Xbox BIOS", b"unrelated bytes");
        let transport = FakeTransport::new(vec![Ok(FakeReply::ok(bad_body))]);
        let outcome =
            update_managed_dat(&descriptor, &update_options(root.clone()), &transport).unwrap();
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
                ..
            }
        ));
        let state = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(state.current_snapshot.sha256, good_sha256);
    }

    // -----------------------------------------------------------------
    // Redump game/disc DAT provider
    // -----------------------------------------------------------------

    fn redump_games_descriptor(system: RedumpGameSystem) -> ManagedDatSourceDescriptor {
        ManagedDatSourceDescriptor::redump_games(system)
            .unwrap()
            .with_update_policy(ManagedDatUpdatePolicy::Manual)
    }

    fn redump_game_header(system: RedumpGameSystem) -> &'static str {
        match system {
            RedumpGameSystem::PlayStation => "Sony - PlayStation",
            RedumpGameSystem::PlayStation2 => "Sony - PlayStation 2",
            RedumpGameSystem::Xbox => "Microsoft - Xbox",
        }
    }

    /// A synthetic, self-hashing Redump ordinary game DAT body - deliberate
    /// never real Redump content, and deliberately carries no "bios"
    /// wording anywhere.
    fn redump_game_dat(system: RedumpGameSystem, game: &str, bytes: &[u8]) -> Vec<u8> {
        let crc32 = crate::identity_source::hashing::Crc32::of(bytes);
        let md5 = {
            use md5::{Digest as _, Md5};
            digest_hex(Md5::digest(bytes))
        };
        let sha1 = {
            use sha1::{Digest as _, Sha1};
            digest_hex(Sha1::digest(bytes))
        };
        let header = redump_game_header(system);
        format!(
            r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>{header}</name>
        <description>{header} Datfile</description>
        <version>20240115</version>
        <author>Redump.org</author>
    </header>
    <game name="{game}">
        <description>{game}</description>
        <rom name="track.bin" size="{}" crc="{crc32}" md5="{md5}" sha1="{sha1}"/>
    </game>
</datafile>"#,
            bytes.len()
        )
        .into_bytes()
    }

    fn redump_game_bytes(seed: &str) -> Vec<u8> {
        format!("synthetic managed redump game bytes - {seed}").into_bytes()
    }

    /// Wraps `entries` in an in-memory ZIP - mirrors exactly what a
    /// ZIP-wrapped Redump datfile download would deliver as its body.
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Cursor;
        use std::io::Write as _;
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn install_redump_games(
        system: RedumpGameSystem,
        root: PathBuf,
        body: Vec<u8>,
    ) -> ManagedDatUpdateOutcome {
        let transport = FakeTransport::new(vec![Ok(FakeReply::ok(body))]);
        update_managed_dat(
            &redump_games_descriptor(system),
            &update_options(root),
            &transport,
        )
        .unwrap()
    }

    #[test]
    fn typed_game_descriptor_derives_its_url_from_the_proven_bios_url_for_all_three_systems() {
        for (system, slug, bios_system) in [
            (
                RedumpGameSystem::PlayStation,
                "playstation",
                RedumpBiosSystem::PlayStation,
            ),
            (
                RedumpGameSystem::PlayStation2,
                "playstation2",
                RedumpBiosSystem::PlayStation2,
            ),
            (RedumpGameSystem::Xbox, "xbox", RedumpBiosSystem::Xbox),
        ] {
            let descriptor = ManagedDatSourceDescriptor::redump_games(system).unwrap();
            assert_eq!(
                descriptor.source_id().provider,
                ManagedDatProvider::RedumpGames
            );
            assert_eq!(descriptor.source_id().source_key, slug);
            assert_eq!(descriptor.expected_ecosystem(), DatEcosystem::Redump);
            assert_eq!(descriptor.redump_games_system(), Some(system));
            assert_eq!(descriptor.redump_bios_system(), None);
            assert_eq!(
                descriptor.source_id().to_string(),
                format!("redump-games/{slug}")
            );
            // Every game-DAT URL is exactly its proven BIOS sibling's URL
            // with the `-bios` path segment removed - never a separately
            // invented slug (see `RedumpGameSystem`'s own doc comment).
            let expected_url = bios_system.fixed_url().replace("-bios", "");
            assert_eq!(system.fixed_url(), expected_url);
            descriptor.validate().unwrap();
        }
    }

    #[test]
    fn every_fixed_redump_url_uses_the_current_info_host_without_changing_paths() {
        for (system, expected) in [
            (
                RedumpBiosSystem::PlayStation,
                "https://redump.info/datfile/psx-bios/",
            ),
            (
                RedumpBiosSystem::PlayStation2,
                "https://redump.info/datfile/ps2-bios/",
            ),
            (
                RedumpBiosSystem::Xbox,
                "https://redump.info/datfile/xbox-bios/",
            ),
        ] {
            assert_eq!(system.fixed_url(), expected);
            validate_managed_dat_http_url(system.fixed_url()).unwrap();
        }
        for (system, expected) in [
            (
                RedumpGameSystem::PlayStation,
                "https://redump.info/datfile/psx/",
            ),
            (
                RedumpGameSystem::PlayStation2,
                "https://redump.info/datfile/ps2/",
            ),
            (RedumpGameSystem::Xbox, "https://redump.info/datfile/xbox/"),
        ] {
            assert_eq!(system.fixed_url(), expected);
            validate_managed_dat_http_url(system.fixed_url()).unwrap();
        }
    }

    /// Compile-time proof: the only way to build a Redump game-DAT
    /// descriptor is through this closed enum.
    #[test]
    fn redump_games_constructor_takes_only_the_closed_system_enum() {
        fn assert_signature(_: fn(RedumpGameSystem) -> Result<ManagedDatSourceDescriptor>) {}
        assert_signature(ManagedDatSourceDescriptor::redump_games);
    }

    #[test]
    fn arbitrary_redump_games_source_key_fails_validation() {
        let bogus = ManagedDatSourceId {
            provider: ManagedDatProvider::RedumpGames,
            source_key: "saturn".to_string(),
        };
        assert!(
            bogus.validate().is_err(),
            "an unproven system (e.g. Saturn) must never validate as a game-DAT source key"
        );
    }

    #[test]
    fn manual_and_disabled_policy_persist_for_all_three_game_systems() {
        for system in [
            RedumpGameSystem::PlayStation,
            RedumpGameSystem::PlayStation2,
            RedumpGameSystem::Xbox,
        ] {
            for policy in [
                ManagedDatUpdatePolicy::Manual,
                ManagedDatUpdatePolicy::Disabled,
            ] {
                let descriptor = ManagedDatSourceDescriptor::redump_games(system)
                    .unwrap()
                    .with_update_policy(policy);
                assert_eq!(descriptor.update_policy(), policy);
                descriptor.validate().unwrap();
            }
        }
    }

    #[test]
    fn valid_raw_xml_dat_is_accepted_for_each_system() {
        for system in [
            RedumpGameSystem::PlayStation,
            RedumpGameSystem::PlayStation2,
            RedumpGameSystem::Xbox,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(MANAGED_DAT_DIRECTORY);
            let body = redump_game_dat(system, "Some Game (USA)", &redump_game_bytes("raw"));
            let outcome = install_redump_games(system, root, body);
            assert!(
                matches!(outcome, ManagedDatUpdateOutcome::Updated { .. }),
                "{system:?}: expected Updated, got {outcome:?}"
            );
        }
    }

    #[test]
    fn valid_zip_wrapped_dat_is_safely_unwrapped_and_the_content_hash_is_of_the_xml_not_the_zip() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpGameSystem::PlayStation2;
        let xml = redump_game_dat(system, "Zipped Game (Europe)", &redump_game_bytes("zip"));
        let zip_body = build_zip(&[("ps2.dat", &xml)]);
        assert_ne!(
            zip_body, xml,
            "the test must genuinely exercise the ZIP path"
        );

        let outcome = install_redump_games(system, root.clone(), zip_body);
        let sha256 = match outcome {
            ManagedDatUpdateOutcome::Updated { sha256, .. } => sha256,
            other => panic!("expected Updated, got {other:?}"),
        };
        let expected_dir = tempfile::tempdir().unwrap();
        let expected_path = expected_dir.path().join("expected.dat");
        fs::write(&expected_path, &xml).unwrap();
        let expected_sha256 = sha256_file(&expected_path).unwrap();
        assert_eq!(
            sha256, expected_sha256,
            "the published content hash must be over the unwrapped XML bytes, not the ZIP bytes"
        );

        let descriptor = redump_games_descriptor(system);
        let state = load_managed_dat_state(&root, &descriptor).unwrap();
        let parsed = parse_dat_file(
            resolve_current_managed_dat_source(&root, &state)
                .unwrap()
                .path(),
            DatLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.dat.games.len(), 1);
        assert_eq!(parsed.dat.games[0].name, "Zipped Game (Europe)");
    }

    #[test]
    fn raw_xml_and_zip_wrapped_identical_content_hash_identically() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpGameSystem::Xbox;
        let xml = redump_game_dat(system, "Same Game", &redump_game_bytes("same"));

        let raw_outcome = install_redump_games(system, root.clone(), xml.clone());
        let raw_sha256 = match raw_outcome {
            ManagedDatUpdateOutcome::Updated { sha256, .. } => sha256,
            other => panic!("expected Updated, got {other:?}"),
        };

        // The very next check with the ZIP-wrapped equivalent of the exact
        // same bytes must be UpToDate, never a new snapshot - proving the
        // format wrapper never affects the content-addressed identity.
        let zip_body = build_zip(&[("xbox.dat", &xml)]);
        let outcome = install_redump_games(system, root.clone(), zip_body);
        assert!(
            matches!(outcome, ManagedDatUpdateOutcome::UpToDate { .. }),
            "expected UpToDate (same content, different wrapper), got {outcome:?}"
        );
        let descriptor = redump_games_descriptor(system);
        let state = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(state.current_snapshot.sha256, raw_sha256);
        assert!(state.previous_snapshot.is_none());
    }

    #[test]
    fn a_bios_dataset_is_rejected_by_the_games_provider() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpGameSystem::PlayStation2;
        // Genuinely a BIOS dataset - the games provider must never accept
        // this, even though it is otherwise well-formed Redump XML for the
        // same system (task requirement: "must not be a BIOS dataset").
        let body = redump_bios_dat(
            RedumpBiosSystem::PlayStation2,
            "PS2 BIOS",
            &redump_bios_bytes("cross-provider"),
        );
        let outcome = install_redump_games(system, root, body);
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
                ..
            }
        ));
    }

    #[test]
    fn wrong_system_game_dataset_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let body = redump_game_dat(
            RedumpGameSystem::PlayStation2,
            "PS2 Game",
            &redump_game_bytes("cross-system"),
        );
        let outcome = install_redump_games(RedumpGameSystem::PlayStation, root, body);
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
                ..
            }
        ));
    }

    #[test]
    fn empty_game_list_is_rejected_at_update_time() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpGameSystem::Xbox;
        let header = redump_game_header(system);
        let body = format!(
            r#"<?xml version="1.0"?>
<datafile><header><name>{header}</name><description>{header} Datfile</description><author>Redump.org</author></header></datafile>"#
        )
        .into_bytes();
        let outcome = install_redump_games(system, root, body);
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::EmptyCatalogue,
                ..
            }
        ));
    }

    #[test]
    fn html_error_page_fails_closed_never_installs_and_never_panics() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let body = b"<!DOCTYPE html><html><body>Please log in</body></html>".to_vec();
        let descriptor = redump_games_descriptor(RedumpGameSystem::PlayStation);
        let outcome = install_redump_games(RedumpGameSystem::PlayStation, root.clone(), body);
        // An HTML/login page is not valid XML and identifies no ecosystem -
        // it is sniffed as an empty generic DAT rather than tripping a hard
        // XML syntax error, so the exact failure kind is either `Parser` or
        // a downstream content-validation rejection (`WrongEcosystem`,
        // `EmptyCatalogue`); what matters is that it is always a structured
        // `Failed`, never `Updated`, and never a panic.
        assert!(
            matches!(
                outcome,
                ManagedDatUpdateOutcome::Failed {
                    kind: ManagedDatUpdateFailureKind::Parser
                        | ManagedDatUpdateFailureKind::WrongEcosystem
                        | ManagedDatUpdateFailureKind::EmptyCatalogue,
                    ..
                }
            ),
            "expected a structured fail-closed outcome, got {outcome:?}"
        );
        assert!(
            load_optional_managed_dat_state(&root, &descriptor)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn malformed_zip_bytes_fail_closed_without_installing_anything() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        // Begins with the ZIP local-file-header signature but is otherwise
        // garbage - must never panic or silently accept partial content.
        let mut body = b"PK\x03\x04".to_vec();
        body.extend_from_slice(&[0u8; 64]);
        let outcome = install_redump_games(RedumpGameSystem::PlayStation, root.clone(), body);
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::InvalidResponse,
                ..
            }
        ));
        assert!(
            load_optional_managed_dat_state(
                &root,
                &redump_games_descriptor(RedumpGameSystem::PlayStation)
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn a_zip_with_more_than_one_member_is_refused_rather_than_guessed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpGameSystem::PlayStation;
        let xml = redump_game_dat(system, "Ambiguous", &redump_game_bytes("ambiguous"));
        let zip_body = build_zip(&[("psx.dat", &xml), ("readme.txt", b"extra file")]);
        let outcome = install_redump_games(system, root, zip_body);
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::InvalidResponse,
                ..
            }
        ));
    }

    #[test]
    fn game_dat_no_churn_on_header_only_change_with_identical_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpGameSystem::PlayStation;
        let descriptor = redump_games_descriptor(system);
        let bytes_v1 = redump_game_bytes("v1");
        let body_v1 = redump_game_dat(system, "Game v1", &bytes_v1);

        let outcome = install_redump_games(system, root.clone(), body_v1.clone());
        let first_sha256 = match outcome {
            ManagedDatUpdateOutcome::Updated { sha256, .. } => sha256,
            other => panic!("expected Updated, got {other:?}"),
        };

        let mut mutated_label_state = load_managed_dat_state(&root, &descriptor).unwrap();
        mutated_label_state.upstream_revision = Some("stale-label".to_string());
        save_managed_dat_state(&root, &mutated_label_state).unwrap();

        let outcome = install_redump_games(system, root.clone(), body_v1);
        assert!(
            matches!(outcome, ManagedDatUpdateOutcome::UpToDate { .. }),
            "identical downloaded bytes must never churn a snapshot"
        );
        let state = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(state.current_snapshot.sha256, first_sha256);
        assert!(state.previous_snapshot.is_none());
    }

    #[test]
    fn game_dat_changed_bytes_promote_current_and_retain_previous() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpGameSystem::PlayStation;
        let descriptor = redump_games_descriptor(system);

        let body_v1 = redump_game_dat(system, "Game v1", &redump_game_bytes("v1"));
        let first_sha256 = match install_redump_games(system, root.clone(), body_v1) {
            ManagedDatUpdateOutcome::Updated { sha256, .. } => sha256,
            other => panic!("expected Updated, got {other:?}"),
        };

        let body_v2 = redump_game_dat(system, "Game v2", &redump_game_bytes("v2 different"));
        let second_sha256 = match install_redump_games(system, root.clone(), body_v2) {
            ManagedDatUpdateOutcome::Updated { sha256, .. } => sha256,
            other => panic!("expected Updated, got {other:?}"),
        };
        assert_ne!(first_sha256, second_sha256);
        let state = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(state.current_snapshot.sha256, second_sha256);
        assert_eq!(
            state.previous_snapshot.as_ref().unwrap().sha256,
            first_sha256
        );
    }

    #[test]
    fn game_dat_403_404_429_and_offline_are_structured_and_do_not_create_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let descriptor = redump_games_descriptor(RedumpGameSystem::Xbox);

        for (reply, expected) in [
            (
                FakeReply::status(403),
                ManagedDatUpdateFailureKind::Forbidden,
            ),
            (
                FakeReply::status(404),
                ManagedDatUpdateFailureKind::NotFound,
            ),
        ] {
            let transport = FakeTransport::new(vec![Ok(reply)]);
            let outcome =
                update_managed_dat(&descriptor, &update_options(root.clone()), &transport).unwrap();
            assert!(matches!(
                outcome,
                ManagedDatUpdateOutcome::Failed { kind, .. } if kind == expected
            ));
            assert!(
                load_optional_managed_dat_state(&root, &descriptor)
                    .unwrap()
                    .is_none()
            );
        }

        let transport = FakeTransport::new(Vec::new());
        let mut options = update_options(root.clone());
        options.offline = true;
        let outcome = update_managed_dat(&descriptor, &options, &transport).unwrap();
        assert!(matches!(outcome, ManagedDatUpdateOutcome::Offline));
        assert!(transport.calls.borrow().is_empty());
    }

    #[test]
    fn game_dat_validation_failure_preserves_the_existing_current_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let system = RedumpGameSystem::PlayStation;
        let descriptor = redump_games_descriptor(system);
        let good_body = redump_game_dat(system, "Known Good", &redump_game_bytes("known-good"));
        let good_sha256 = match install_redump_games(system, root.clone(), good_body) {
            ManagedDatUpdateOutcome::Updated { sha256, .. } => sha256,
            other => panic!("expected Updated, got {other:?}"),
        };

        let bad_body = redump_game_dat(
            RedumpGameSystem::Xbox,
            "Wrong System",
            b"unrelated bytes for the wrong system",
        );
        let transport = FakeTransport::new(vec![Ok(FakeReply::ok(bad_body))]);
        let outcome =
            update_managed_dat(&descriptor, &update_options(root.clone()), &transport).unwrap();
        assert!(matches!(
            outcome,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
                ..
            }
        ));
        let state = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(state.current_snapshot.sha256, good_sha256);
    }

    #[test]
    fn user_local_dat_source_entry_cannot_be_passed_to_the_updater() {
        // `check_managed_dat_update`/`update_managed_dat` accept only a
        // `&ManagedDatSourceDescriptor` - there is no overload or code path
        // that accepts a `DatSourceEntry`/local path and treats it as
        // managed-updater authority. Compile-time proof, not a runtime
        // check: this would fail to compile if such a code path existed
        // with a different signature shape.
        fn assert_signature(
            _: fn(
                &ManagedDatSourceDescriptor,
                &ManagedDatUpdateOptions,
                &dyn ManagedDatTransport,
            ) -> Result<ManagedDatUpdateOutcome>,
        ) {
        }
        assert_signature(check_managed_dat_update);
        assert_signature(update_managed_dat);
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
        let descriptor = ManagedDatSourceDescriptor::mame_software_list("gamecom").unwrap();
        assert_eq!(
            descriptor.source_id().to_string(),
            "mame-software-list/gamecom"
        );
        assert_eq!(descriptor.repository(), Some(MAME_REPOSITORY));
        assert_eq!(
            descriptor.repository_relative_path(),
            Some(Path::new("hash/gamecom.xml"))
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

    #[test]
    fn first_install_records_exact_immutable_revision_and_sha256() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let body = mame_xml("gamecom", "first");
        let expected_sha = {
            let mut hasher = Sha256::new();
            hasher.update(&body);
            let digest = hasher.finalize();
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let transport = FakeTransport::new(vec![Ok(revision(REV_A)), Ok(FakeReply::ok(body))]);
        let result =
            update_managed_dat(&descriptor(), &update_options(root.clone()), &transport).unwrap();
        assert_eq!(
            result,
            ManagedDatUpdateOutcome::Updated {
                upstream_revision: REV_A.to_string(),
                sha256: expected_sha.clone(),
            }
        );
        let state = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(state.upstream_revision.as_deref(), Some(REV_A));
        assert_eq!(state.sha256, expected_sha);
        assert!(transport.calls.borrow()[1].url.contains(REV_A));
        assert!(!transport.calls.borrow()[1].url.contains("/master/"));
    }

    #[test]
    fn wrong_name_malformed_and_truncated_downloads_never_install() {
        let cases = vec![
            (
                FakeReply::ok(mame_xml("wrong", "game")),
                ManagedDatUpdateFailureKind::WrongAuthoritativeName,
            ),
            (
                FakeReply::ok(b"<softwarelist".to_vec()),
                ManagedDatUpdateFailureKind::Parser,
            ),
            (
                {
                    let mut reply = FakeReply::ok(mame_xml("gamecom", "game"));
                    reply.content_length = Some(reply.body.len() as u64 + 1);
                    reply
                },
                ManagedDatUpdateFailureKind::TruncatedDownload,
            ),
        ];
        for (body_reply, kind) in cases {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(MANAGED_DAT_DIRECTORY);
            let transport = FakeTransport::new(vec![Ok(revision(REV_A)), Ok(body_reply)]);
            let result =
                update_managed_dat(&descriptor(), &update_options(root.clone()), &transport)
                    .unwrap();
            assert!(
                matches!(result, ManagedDatUpdateOutcome::Failed { kind: actual, .. } if actual == kind)
            );
            assert!(
                load_optional_managed_dat_state(&root, &descriptor())
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn oversized_response_is_rejected_before_install() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let mut reply = FakeReply::ok(mame_xml("gamecom", "large"));
        reply.content_length = Some(DEFAULT_MAX_FILE_SIZE + 1);
        let transport = FakeTransport::new(vec![Ok(revision(REV_A)), Ok(reply)]);
        let result =
            update_managed_dat(&descriptor(), &update_options(root.clone()), &transport).unwrap();
        assert!(matches!(
            result,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::DownloadTooLarge,
                ..
            }
        ));
        assert!(
            load_optional_managed_dat_state(&root, &descriptor())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unchanged_304_is_up_to_date_and_check_uses_conditional_headers() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let mut state = install(root.clone(), REV_A, mame_xml("gamecom", "first"));
        state.etag = Some("\"revision-etag\"".into());
        state.last_modified = Some("Wed, 21 Oct 2015 07:28:00 GMT".into());
        save_managed_dat_state(&root, &state).unwrap();
        let transport = FakeTransport::new(vec![Ok(FakeReply::status(304))]);
        let result =
            check_managed_dat_update(&descriptor(), &update_options(root.clone()), &transport)
                .unwrap();
        assert!(matches!(result, ManagedDatUpdateOutcome::UpToDate { .. }));
        let request = &transport.calls.borrow()[0];
        assert!(
            request
                .headers
                .iter()
                .any(|(name, _)| name == "If-None-Match")
        );
        assert!(
            request
                .headers
                .iter()
                .any(|(name, _)| name == "If-Modified-Since")
        );
    }

    #[test]
    fn changed_revision_same_bytes_does_not_churn_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let body = mame_xml("gamecom", "same");
        let first = install(root.clone(), REV_A, body.clone());
        let transport = FakeTransport::new(vec![Ok(revision(REV_B)), Ok(FakeReply::ok(body))]);
        let result =
            update_managed_dat(&descriptor(), &update_options(root.clone()), &transport).unwrap();
        assert!(matches!(result, ManagedDatUpdateOutcome::Updated { .. }));
        let second = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(second.current_snapshot, first.current_snapshot);
        assert_eq!(second.previous_snapshot, first.previous_snapshot);
        assert_eq!(second.upstream_revision.as_deref(), Some(REV_B));
    }

    #[test]
    fn changed_bytes_promote_current_keep_previous_and_forget_third_old_object() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let first = install(root.clone(), REV_A, mame_xml("gamecom", "one"));
        let transport_b = FakeTransport::new(vec![
            Ok(revision(REV_B)),
            Ok(FakeReply::ok(mame_xml("gamecom", "two"))),
        ]);
        update_managed_dat(&descriptor(), &update_options(root.clone()), &transport_b).unwrap();
        let second = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(
            second.previous_snapshot.as_ref(),
            Some(&first.current_snapshot)
        );
        let transport_c = FakeTransport::new(vec![
            Ok(revision(REV_C)),
            Ok(FakeReply::ok(mame_xml("gamecom", "three"))),
        ]);
        update_managed_dat(&descriptor(), &update_options(root.clone()), &transport_c).unwrap();
        let third = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(
            third.previous_snapshot.as_ref(),
            Some(&second.current_snapshot)
        );
        let objects = root
            .join(third.source_id.storage_relative_path())
            .join(OBJECTS_DIRECTORY);
        assert!(!objects.join(first.current_snapshot.sha256).exists());
    }

    #[test]
    fn http_rate_limit_and_network_failures_preserve_current() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let before = install(root.clone(), REV_A, mame_xml("gamecom", "first"));
        let mut limited = FakeReply::status(429);
        limited.retry_after_seconds = Some(60);
        let rate_transport = FakeTransport::new(vec![Ok(limited)]);
        assert_eq!(
            update_managed_dat(
                &descriptor(),
                &update_options(root.clone()),
                &rate_transport
            )
            .unwrap(),
            ManagedDatUpdateOutcome::RateLimited {
                retry_after_seconds: Some(60)
            }
        );
        let network_transport = FakeTransport::new(vec![Err(ManagedDatTransportError::new(
            ManagedDatTransportFailureKind::Timeout,
            "timeout",
        ))]);
        assert!(matches!(
            update_managed_dat(
                &descriptor(),
                &update_options(root.clone()),
                &network_transport
            )
            .unwrap(),
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::Timeout,
                ..
            }
        ));
        assert_eq!(
            load_managed_dat_state(&root, &descriptor())
                .unwrap()
                .current_snapshot,
            before.current_snapshot
        );
    }

    #[test]
    fn forbidden_and_not_found_are_structured_and_do_not_create_state() {
        for (status, expected) in [
            (403, ManagedDatUpdateFailureKind::Forbidden),
            (404, ManagedDatUpdateFailureKind::NotFound),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(MANAGED_DAT_DIRECTORY);
            let transport = FakeTransport::new(vec![Ok(FakeReply::status(status))]);
            let result =
                update_managed_dat(&descriptor(), &update_options(root.clone()), &transport)
                    .unwrap();
            assert!(
                matches!(result, ManagedDatUpdateOutcome::Failed { kind, .. } if kind == expected)
            );
            assert!(
                load_optional_managed_dat_state(&root, &descriptor())
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn offline_mode_performs_zero_requests_and_no_background_work_exists() {
        let temp = tempfile::tempdir().unwrap();
        let mut options = update_options(temp.path().join(MANAGED_DAT_DIRECTORY));
        options.offline = true;
        let transport = FakeTransport::new(Vec::new());
        assert_eq!(
            check_managed_dat_update(&descriptor(), &options, &transport).unwrap(),
            ManagedDatUpdateOutcome::Offline
        );
        assert_eq!(
            update_managed_dat(&descriptor(), &options, &transport).unwrap(),
            ManagedDatUpdateOutcome::Offline
        );
        assert!(transport.calls.borrow().is_empty());
    }

    #[test]
    fn disabled_descriptor_performs_zero_requests() {
        let temp = tempfile::tempdir().unwrap();
        let transport = FakeTransport::new(Vec::new());
        let disabled = ManagedDatSourceDescriptor::mame_software_list("gamecom").unwrap();
        let result = update_managed_dat(
            &disabled,
            &update_options(temp.path().join(MANAGED_DAT_DIRECTORY)),
            &transport,
        )
        .unwrap();
        assert_eq!(result, ManagedDatUpdateOutcome::Disabled);
        assert!(transport.calls.borrow().is_empty());
    }

    #[test]
    fn orphan_inspection_reports_only_objects_outside_current_and_previous() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let mut state = state();
        state.previous_snapshot = Some(ManagedDatSnapshot::new(SHA_B).unwrap());
        write_current_object(&root, &state);
        let previous = root
            .join(state.source_id.storage_relative_path())
            .join(OBJECTS_DIRECTORY)
            .join(SHA_B);
        fs::create_dir_all(previous.parent().unwrap()).unwrap();
        fs::write(&previous, b"previous").unwrap();
        let orphan_sha = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let orphan = previous.with_file_name(orphan_sha);
        fs::write(&orphan, b"orphan").unwrap();
        save_managed_dat_state(&root, &state).unwrap();

        let found = list_managed_dat_orphaned_objects(&root, &descriptor()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].snapshot.sha256, orphan_sha);
        assert_eq!(found[0].path, orphan);
        assert_eq!(found[0].size_bytes, 6);
        assert!(
            previous.exists(),
            "inspection must not delete previous data"
        );
        assert!(
            root.join(state.source_id.storage_relative_path())
                .join(OBJECTS_DIRECTORY)
                .join(SHA_A)
                .exists(),
            "inspection must not delete current data"
        );
    }

    #[test]
    fn orphan_inspection_is_read_only_and_reports_objects_without_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let orphan_sha = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let object = root
            .join(descriptor().source_id().storage_relative_path())
            .join(OBJECTS_DIRECTORY)
            .join(orphan_sha);
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        fs::write(&object, b"orphan").unwrap();
        let before = fs::read(&object).unwrap();

        let found = list_managed_dat_orphaned_objects(&root, &descriptor()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, object);
        assert_eq!(fs::read(&object).unwrap(), before);
        assert!(
            load_optional_managed_dat_state(&root, &descriptor())
                .unwrap()
                .is_none(),
            "inspection must not create state"
        );
    }

    #[cfg(unix)]
    #[test]
    fn orphan_inspection_rejects_symlinked_objects() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let object = root
            .join(descriptor().source_id().storage_relative_path())
            .join(OBJECTS_DIRECTORY)
            .join(SHA_A);
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        let external = temp.path().join("external");
        fs::write(&external, b"not managed").unwrap();
        symlink(&external, &object).unwrap();

        assert!(list_managed_dat_orphaned_objects(&root, &descriptor()).is_err());
        assert!(external.exists(), "inspection must not touch external data");
    }
}
