//! Read-only, channel-aware update availability checks for E1 installations.
//!
//! Metadata is the only remote material involved.  No release asset is ever
//! requested.  The provider boundary keeps tests entirely local and makes
//! network failure an ordinary `Offline`/`LatestUnknown` result.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::emulator_inventory::{
    BuildChannel, EmulatorInstallation, InstallationType, InventoryEmulator, SaveStateRisk,
    UpdateCapability,
};

pub const MAX_METADATA_BYTES: usize = 64 * 1024;
pub const METADATA_TIMEOUT: Duration = Duration::from_secs(5);
pub const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
pub const MAX_UPDATE_BYTES: u64 = 512 * 1024 * 1024;
pub const UPDATE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable,
    InstalledNewer,
    VersionUnknown,
    LatestUnknown,
    ChannelMismatch,
    ComparisonUnsupported,
    Offline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateMetadataSource {
    OfficialReleaseApi,
    PackageMetadata,
    FlatpakMetadata,
    LocalCache,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AvailableVersion {
    pub version: String,
    pub channel: BuildChannel,
    pub source: UpdateMetadataSource,
    pub provenance: String,
    pub checked_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateResult {
    pub emulator: InventoryEmulator,
    pub executable_path: std::path::PathBuf,
    pub installed_version: Option<String>,
    pub installed_channel: BuildChannel,
    pub available_version: Option<String>,
    pub available_channel: BuildChannel,
    pub status: UpdateStatus,
    pub source: UpdateMetadataSource,
    pub provenance: String,
    pub checked_unix_seconds: u64,
    pub warning: Option<String>,
    pub save_state_warning: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateReport {
    pub results: Vec<UpdateResult>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateExecutionEligibility {
    Ready,
    RunningBlocked,
    UnsupportedInstallType,
    VersionUnknown,
    StaleMetadata,
    InvalidTarget,
    VerificationUnavailable,
    ReviewRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateVerificationLevel {
    Sha256,
    Unverified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum UpdateTransactionState {
    Planned,
    Downloading,
    Verifying,
    Staged,
    Published,
    Failed,
    RolledBack,
    NeedsReconciliation,
    Stale,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateArtifact {
    pub version: String,
    pub channel: BuildChannel,
    pub url: String,
    pub sha256: Option<String>,
    pub source: UpdateMetadataSource,
    pub provenance: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateExecutionPlan {
    pub transaction_id: String,
    pub emulator: InventoryEmulator,
    pub installation_type: InstallationType,
    pub target_path: PathBuf,
    pub target_sha256: String,
    pub installed_version: String,
    pub installed_channel: BuildChannel,
    pub new_version: String,
    pub new_channel: BuildChannel,
    pub artifact: UpdateArtifact,
    pub verification: UpdateVerificationLevel,
    pub rollback_path: PathBuf,
    pub eligibility: UpdateExecutionEligibility,
    pub save_state_warning: bool,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateJournal {
    pub transaction_id: String,
    pub emulator: InventoryEmulator,
    pub target_path: PathBuf,
    pub rollback_path: PathBuf,
    pub old_version: String,
    pub new_version: String,
    pub channel: BuildChannel,
    pub source_url: String,
    pub provenance: String,
    pub verification: UpdateVerificationLevel,
    pub staged_path: Option<PathBuf>,
    pub state: UpdateTransactionState,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UpdateExecutionError {
    Ineligible(UpdateExecutionEligibility),
    Stale,
    Download(String),
    Verification(String),
    Io(String),
    NeedsReconciliation(String),
}

impl std::fmt::Display for UpdateExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ineligible(reason) => write!(f, "update is not executable: {reason:?}"),
            Self::Stale => f.write_str("update preview is stale; review it again"),
            Self::Download(message) => write!(f, "update download failed: {message}"),
            Self::Verification(message) => write!(f, "update verification failed: {message}"),
            Self::Io(message) => write!(f, "update filesystem operation failed: {message}"),
            Self::NeedsReconciliation(message) => {
                write!(f, "update needs reconciliation: {message}")
            }
        }
    }
}

impl std::error::Error for UpdateExecutionError {}

pub trait UpdateDownloader {
    fn download(&mut self, url: &str, destination: &Path) -> Result<(), UpdateExecutionError>;
}

#[derive(Default)]
pub struct HttpsUpdateDownloader;

impl UpdateDownloader for HttpsUpdateDownloader {
    fn download(&mut self, url: &str, destination: &Path) -> Result<(), UpdateExecutionError> {
        if !url.starts_with("https://") {
            return Err(UpdateExecutionError::Download(
                "only HTTPS sources are allowed".into(),
            ));
        }
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(UPDATE_TIMEOUT))
            .build()
            .new_agent();
        let mut response = agent
            .get(url)
            .call()
            .map_err(|error| UpdateExecutionError::Download(error.to_string()))?;
        if !(200..300).contains(&response.status().as_u16()) {
            return Err(UpdateExecutionError::Download(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
        let mut limited = response.body_mut().as_reader().take(MAX_UPDATE_BYTES + 1);
        let copied = std::io::copy(&mut limited, &mut output)
            .map_err(|error| UpdateExecutionError::Download(error.to_string()))?;
        if copied > MAX_UPDATE_BYTES {
            return Err(UpdateExecutionError::Download(
                "download exceeded size limit".into(),
            ));
        }
        output
            .flush()
            .map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
        output
            .sync_all()
            .map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
        Ok(())
    }
}

fn update_transaction_id(path: &Path, version: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(version.as_bytes());
    hasher.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..24]
        .to_string()
}

pub fn plan_staged_update(
    installation: &EmulatorInstallation,
    update: &UpdateResult,
    artifact: UpdateArtifact,
    emulator_running: bool,
) -> UpdateExecutionPlan {
    let installed_version = installation.version.clone().unwrap_or_default();
    let verification = if artifact.sha256.is_some() {
        UpdateVerificationLevel::Sha256
    } else {
        UpdateVerificationLevel::Unverified
    };
    let target_sha256 = hash_file(&installation.executable_path).unwrap_or_default();
    let eligibility = if emulator_running {
        UpdateExecutionEligibility::RunningBlocked
    } else if !matches!(
        installation.installation_type,
        InstallationType::AppImage | InstallationType::Portable | InstallationType::Managed
    ) {
        UpdateExecutionEligibility::UnsupportedInstallType
    } else if installation.update_capability != UpdateCapability::PortableManaged {
        UpdateExecutionEligibility::UnsupportedInstallType
    } else if installation.version.is_none() {
        UpdateExecutionEligibility::VersionUnknown
    } else if update.status != UpdateStatus::UpdateAvailable
        || update.available_version.as_deref() != Some(artifact.version.as_str())
        || update.available_channel != artifact.channel
    {
        UpdateExecutionEligibility::StaleMetadata
    } else if target_sha256.is_empty()
        || !installation.executable_path.is_file()
        || artifact.url.is_empty()
        || !artifact.url.starts_with("https://")
    {
        UpdateExecutionEligibility::InvalidTarget
    } else if verification == UpdateVerificationLevel::Unverified {
        UpdateExecutionEligibility::VerificationUnavailable
    } else {
        UpdateExecutionEligibility::Ready
    };
    let rollback_path = installation
        .installation_root
        .join(".emuwiz-update-rollback")
        .join(format!(
            "{}-{}",
            installation.emulator.label(),
            installed_version
        ));
    UpdateExecutionPlan { transaction_id: update_transaction_id(&installation.executable_path, &artifact.version), emulator: installation.emulator, installation_type: installation.installation_type, target_path: installation.executable_path.clone(), target_sha256, installed_version, installed_channel: installation.channel, new_version: artifact.version.clone(), new_channel: artifact.channel, artifact, verification, rollback_path, eligibility, save_state_warning: update.save_state_warning, warning: (verification == UpdateVerificationLevel::Unverified).then_some("No published checksum/signature was supplied; execution is not safe to claim verified.".into()) }
}

fn hash_file(path: &Path) -> Result<String, UpdateExecutionError> {
    let mut file = File::open(path).map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > MAX_UPDATE_BYTES {
            return Err(UpdateExecutionError::Verification(
                "staged update exceeds size limit".into(),
            ));
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn sync_directory(path: &Path) -> Result<(), UpdateExecutionError> {
    File::open(path)
        .map_err(|error| UpdateExecutionError::Io(error.to_string()))?
        .sync_all()
        .map_err(|error| UpdateExecutionError::Io(error.to_string()))
}

pub fn execute_staged_update<D: UpdateDownloader>(
    plan: &UpdateExecutionPlan,
    installation: &EmulatorInstallation,
    update: &UpdateResult,
    emulator_running: bool,
    downloader: &mut D,
) -> Result<UpdateJournal, UpdateExecutionError> {
    if plan.eligibility != UpdateExecutionEligibility::Ready {
        return Err(UpdateExecutionError::Ineligible(plan.eligibility));
    }
    if plan.artifact.sha256.is_none() {
        return Err(UpdateExecutionError::Ineligible(
            UpdateExecutionEligibility::VerificationUnavailable,
        ));
    }
    if emulator_running
        || installation.executable_path != plan.target_path
        || installation.installation_type != plan.installation_type
        || installation.version.as_deref() != Some(plan.installed_version.as_str())
        || installation.channel != plan.installed_channel
        || update.status != UpdateStatus::UpdateAvailable
        || update.available_version.as_deref() != Some(plan.new_version.as_str())
        || update.available_channel != plan.new_channel
    {
        return Err(UpdateExecutionError::Stale);
    }
    let metadata = fs::symlink_metadata(&plan.target_path)
        .map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(UpdateExecutionError::Stale);
    }
    if hash_file(&plan.target_path)? != plan.target_sha256 {
        return Err(UpdateExecutionError::Stale);
    }
    let staging = plan
        .target_path
        .with_file_name(format!(".emuwiz-update-staging-{}", plan.transaction_id));
    if fs::symlink_metadata(&staging).is_ok() {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "staging path already exists".into(),
        ));
    }
    let mut journal = UpdateJournal {
        transaction_id: plan.transaction_id.clone(),
        emulator: plan.emulator,
        target_path: plan.target_path.clone(),
        rollback_path: plan.rollback_path.clone(),
        old_version: plan.installed_version.clone(),
        new_version: plan.new_version.clone(),
        channel: plan.new_channel,
        source_url: plan.artifact.url.clone(),
        provenance: plan.artifact.provenance.clone(),
        verification: plan.verification,
        staged_path: Some(staging.clone()),
        state: UpdateTransactionState::Downloading,
        failure: None,
    };
    if let Err(error) = downloader.download(&plan.artifact.url, &staging) {
        journal.state = UpdateTransactionState::Failed;
        journal.failure = Some(error.to_string());
        let _ = fs::remove_file(&staging);
        return Err(error);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        let permissions = fs::Permissions::from_mode(mode);
        fs::set_permissions(&staging, permissions)
            .map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
    }
    journal.state = UpdateTransactionState::Verifying;
    let hash = hash_file(&staging)?;
    if plan.artifact.sha256.as_deref() != Some(hash.as_str()) {
        journal.state = UpdateTransactionState::Failed;
        let _ = fs::remove_file(&staging);
        return Err(UpdateExecutionError::Verification(
            "SHA-256 does not match published artifact".into(),
        ));
    }
    journal.state = UpdateTransactionState::Staged;
    if let Some(parent) = plan.rollback_path.parent() {
        fs::create_dir_all(parent).map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
    }
    fs::rename(&plan.target_path, &plan.rollback_path)
        .map_err(|error| UpdateExecutionError::Io(error.to_string()))?;
    if let Err(error) = fs::rename(&staging, &plan.target_path) {
        let _ = fs::rename(&plan.rollback_path, &plan.target_path);
        return Err(UpdateExecutionError::NeedsReconciliation(error.to_string()));
    }
    if let Some(parent) = plan.target_path.parent() {
        sync_directory(parent)
            .map_err(|error| UpdateExecutionError::NeedsReconciliation(error.to_string()))?;
    }
    journal.state = UpdateTransactionState::Published;
    journal.staged_path = None;
    Ok(journal)
}

pub fn rollback_staged_update(
    journal: &UpdateJournal,
) -> Result<UpdateJournal, UpdateExecutionError> {
    if journal.state != UpdateTransactionState::Published {
        return Err(UpdateExecutionError::Ineligible(
            UpdateExecutionEligibility::ReviewRequired,
        ));
    }
    if !journal.rollback_path.is_file() || journal.target_path.is_symlink() {
        return Err(UpdateExecutionError::NeedsReconciliation(
            "rollback target or backup is not trustworthy".into(),
        ));
    }
    let failed_new = journal
        .target_path
        .with_file_name(format!(".emuwiz-update-failed-{}", journal.transaction_id));
    fs::rename(&journal.target_path, &failed_new)
        .map_err(|error| UpdateExecutionError::NeedsReconciliation(error.to_string()))?;
    if let Err(error) = fs::rename(&journal.rollback_path, &journal.target_path) {
        let _ = fs::rename(&failed_new, &journal.target_path);
        return Err(UpdateExecutionError::NeedsReconciliation(error.to_string()));
    }
    if let Some(parent) = journal.target_path.parent() {
        sync_directory(parent)
            .map_err(|error| UpdateExecutionError::NeedsReconciliation(error.to_string()))?;
    }
    let _ = fs::remove_file(failed_new);
    let mut restored = journal.clone();
    restored.state = UpdateTransactionState::RolledBack;
    Ok(restored)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetadataError {
    Offline(String),
    Malformed(String),
    Unsupported(String),
}

pub trait UpdateMetadataProvider {
    fn latest(
        &mut self,
        emulator: InventoryEmulator,
        channel: BuildChannel,
    ) -> Result<AvailableVersion, MetadataError>;
}

#[derive(Clone, Debug, Default)]
pub struct LocalMetadataProvider {
    pub versions: std::collections::BTreeMap<InventoryEmulator, AvailableVersion>,
    pub error: Option<MetadataError>,
}

#[derive(Clone, Debug)]
pub struct OfficialMetadataProvider {
    agent: ureq::Agent,
}

impl Default for OfficialMetadataProvider {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(METADATA_TIMEOUT))
            .build();
        Self {
            agent: config.new_agent(),
        }
    }
}

impl UpdateMetadataProvider for OfficialMetadataProvider {
    fn latest(
        &mut self,
        emulator: InventoryEmulator,
        channel: BuildChannel,
    ) -> Result<AvailableVersion, MetadataError> {
        let url = official_metadata_url(emulator, InstallationType::Manual)
            .ok_or_else(|| MetadataError::Unsupported("No official metadata endpoint".into()))?;
        let mut response = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header(
                "User-Agent",
                concat!("archivefs/", env!("CARGO_PKG_VERSION")),
            )
            .call()
            .map_err(|e| MetadataError::Offline(e.to_string()))?;
        if !(200..300).contains(&response.status().as_u16()) {
            return Err(MetadataError::Offline(format!(
                "official metadata returned HTTP {}",
                response.status()
            )));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take((MAX_METADATA_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| MetadataError::Offline(e.to_string()))?;
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(MetadataError::Malformed(
                "metadata response exceeded size limit".into(),
            ));
        }
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| MetadataError::Malformed(e.to_string()))?;
        let tag = json
            .get("tag_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MetadataError::Malformed("release has no tag_name".into()))?;
        let detected = if json
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            BuildChannel::Development
        } else {
            BuildChannel::Stable
        };
        let _requested_channel = channel;
        Ok(AvailableVersion {
            version: tag.trim_start_matches('v').into(),
            channel: detected,
            source: UpdateMetadataSource::OfficialReleaseApi,
            provenance: url.into(),
            checked_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        })
    }
}

impl UpdateMetadataProvider for LocalMetadataProvider {
    fn latest(
        &mut self,
        emulator: InventoryEmulator,
        _channel: BuildChannel,
    ) -> Result<AvailableVersion, MetadataError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        self.versions
            .get(&emulator)
            .cloned()
            .ok_or_else(|| MetadataError::Offline("No metadata fixture available".into()))
    }
}

/// Compare only versions whose numeric ordering is explicit.  This handles
/// semantic versions, build numbers, and date/build strings without lexical
/// comparison; revision-only identifiers intentionally remain unsupported.
pub fn compare_versions(installed: &str, available: &str) -> Option<std::cmp::Ordering> {
    fn numbers(value: &str) -> Option<Vec<u64>> {
        let result: Vec<u64> = value
            .split(|c: char| !c.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        (!result.is_empty()).then_some(result)
    }
    let left = numbers(installed)?;
    let right = numbers(available)?;
    Some(left.cmp(&right))
}

pub fn compare_installation(
    installation: &EmulatorInstallation,
    latest: Result<AvailableVersion, MetadataError>,
) -> UpdateResult {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (available_version, available_channel, source, provenance, status, warning) = match latest {
        Ok(metadata) => {
            let channel = metadata.channel;
            if installation.channel != BuildChannel::Unknown
                && channel != BuildChannel::Unknown
                && installation.channel != channel
            {
                (
                    Some(metadata.version),
                    channel,
                    metadata.source,
                    metadata.provenance,
                    UpdateStatus::ChannelMismatch,
                    Some("The available metadata describes a different release channel.".into()),
                )
            } else if installation.version.is_none() {
                (
                    Some(metadata.version),
                    channel,
                    metadata.source,
                    metadata.provenance,
                    UpdateStatus::VersionUnknown,
                    Some("Installed version is unknown, so no comparison was made.".into()),
                )
            } else if let Some(ordering) = compare_versions(
                installation.version.as_deref().unwrap_or_default(),
                &metadata.version,
            ) {
                let status = match ordering {
                    std::cmp::Ordering::Less => UpdateStatus::UpdateAvailable,
                    std::cmp::Ordering::Equal => UpdateStatus::UpToDate,
                    std::cmp::Ordering::Greater => UpdateStatus::InstalledNewer,
                };
                (
                    Some(metadata.version),
                    channel,
                    metadata.source,
                    metadata.provenance,
                    status,
                    None,
                )
            } else {
                (
                    Some(metadata.version),
                    channel,
                    metadata.source,
                    metadata.provenance,
                    UpdateStatus::ComparisonUnsupported,
                    Some("These version identifiers do not have a proven ordering.".into()),
                )
            }
        }
        Err(MetadataError::Offline(message)) => (
            None,
            BuildChannel::Unknown,
            UpdateMetadataSource::Unknown,
            "official metadata unavailable".into(),
            UpdateStatus::Offline,
            Some(message),
        ),
        Err(error) => (
            None,
            BuildChannel::Unknown,
            UpdateMetadataSource::Unknown,
            "metadata unavailable".into(),
            UpdateStatus::LatestUnknown,
            Some(format!("{error:?}")),
        ),
    };
    UpdateResult {
        emulator: installation.emulator,
        executable_path: installation.executable_path.clone(),
        installed_version: installation.version.clone(),
        installed_channel: installation.channel,
        available_version,
        available_channel,
        status,
        source,
        provenance,
        checked_unix_seconds: now,
        warning,
        save_state_warning: matches!(
            installation.save_state_risk,
            SaveStateRisk::VersionSensitive
        ) && matches!(status, UpdateStatus::UpdateAvailable),
    }
}

pub fn check_updates<P: UpdateMetadataProvider>(
    installations: &[EmulatorInstallation],
    provider: &mut P,
) -> UpdateReport {
    let mut results: Vec<_> = installations
        .iter()
        .map(|installation| {
            compare_installation(
                installation,
                provider.latest(installation.emulator, installation.channel),
            )
        })
        .collect();
    results.sort_by(|a, b| (a.emulator, &a.executable_path).cmp(&(b.emulator, &b.executable_path)));
    UpdateReport { results }
}

/// Official endpoints are intentionally metadata-only and are not used by
/// tests. Package/Flatpak installations remain on their own provenance lane.
pub fn official_metadata_url(
    emulator: InventoryEmulator,
    kind: InstallationType,
) -> Option<&'static str> {
    if matches!(
        kind,
        InstallationType::SystemPackage | InstallationType::Flatpak
    ) {
        return None;
    }
    match emulator {
        InventoryEmulator::Dolphin => {
            Some("https://api.github.com/repos/dolphin-emu/dolphin/releases/latest")
        }
        InventoryEmulator::Rpcs3 => {
            Some("https://api.github.com/repos/RPCS3/rpcs3-binaries-linux/releases/latest")
        }
        InventoryEmulator::Pcsx2 => {
            Some("https://api.github.com/repos/PCSX2/pcsx2/releases/latest")
        }
        InventoryEmulator::Ppsspp => {
            Some("https://api.github.com/repos/hrydgard/ppsspp/releases/latest")
        }
        InventoryEmulator::DuckStation => {
            Some("https://api.github.com/repos/stenzek/duckstation/releases/latest")
        }
        InventoryEmulator::Xemu => {
            Some("https://api.github.com/repos/xemu-project/xemu/releases/latest")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_inventory::{InventoryCandidate, VersionConfidence, VersionSource};
    use std::fs;
    fn install(
        version: Option<&str>,
        channel: BuildChannel,
        kind: InstallationType,
    ) -> EmulatorInstallation {
        let candidate = InventoryCandidate {
            emulator: InventoryEmulator::Dolphin,
            executable_path: "/emu/dolphin".into(),
            installation_root: "/emu".into(),
            version_output: version.map(str::to_string),
            installation_type: kind,
            update_capability: crate::emulator_inventory::UpdateCapability::UpstreamRelease,
            preferred: None,
        };
        let mut item = crate::emulator_inventory::inventory_from_candidates(vec![candidate])
            .installations
            .remove(0);
        item.channel = channel;
        item.version_confidence = VersionConfidence::VerifiedCommand;
        item.version_source = VersionSource::VersionCommand;
        item
    }
    fn metadata(version: &str, channel: BuildChannel) -> AvailableVersion {
        AvailableVersion {
            version: version.into(),
            channel,
            source: UpdateMetadataSource::OfficialReleaseApi,
            provenance: "fixture".into(),
            checked_unix_seconds: 1,
        }
    }
    #[test]
    fn statuses_are_channel_aware() {
        assert_eq!(
            compare_installation(
                &install(Some("1.0"), BuildChannel::Stable, InstallationType::Manual),
                Ok(metadata("1.0", BuildChannel::Stable))
            )
            .status,
            UpdateStatus::UpToDate
        );
        assert_eq!(
            compare_installation(
                &install(Some("1.0"), BuildChannel::Stable, InstallationType::Manual),
                Ok(metadata("2.0", BuildChannel::Stable))
            )
            .status,
            UpdateStatus::UpdateAvailable
        );
        assert_eq!(
            compare_installation(
                &install(Some("9.0"), BuildChannel::Stable, InstallationType::Manual),
                Ok(metadata("2.0", BuildChannel::Stable))
            )
            .status,
            UpdateStatus::InstalledNewer
        );
        assert_eq!(
            compare_installation(
                &install(Some("1.0"), BuildChannel::Stable, InstallationType::Manual),
                Ok(metadata("2.0", BuildChannel::Development))
            )
            .status,
            UpdateStatus::ChannelMismatch
        );
    }
    #[test]
    fn unknown_and_offline_fail_closed() {
        assert_eq!(
            compare_installation(
                &install(None, BuildChannel::Unknown, InstallationType::Manual),
                Err(MetadataError::Offline("x".into()))
            )
            .status,
            UpdateStatus::Offline
        );
        assert_eq!(
            compare_installation(
                &install(
                    Some("revision"),
                    BuildChannel::Stable,
                    InstallationType::Manual
                ),
                Ok(metadata("nightly", BuildChannel::Stable))
            )
            .status,
            UpdateStatus::ComparisonUnsupported
        );
    }
    #[test]
    fn update_warns_about_save_states() {
        let result = compare_installation(
            &install(Some("1.0"), BuildChannel::Stable, InstallationType::Manual),
            Ok(metadata("2.0", BuildChannel::Stable)),
        );
        assert!(result.save_state_warning);
    }

    fn executable_install(root: &Path) -> EmulatorInstallation {
        let path = root.join("dolphin");
        fs::write(&path, b"old emulator").unwrap();
        let mut item = install(
            Some("1.0"),
            BuildChannel::Stable,
            InstallationType::Portable,
        );
        item.executable_path = path;
        item.installation_root = root.to_path_buf();
        item.update_capability = UpdateCapability::PortableManaged;
        item
    }

    fn update_result(install: &EmulatorInstallation) -> UpdateResult {
        UpdateResult {
            emulator: install.emulator,
            executable_path: install.executable_path.clone(),
            installed_version: install.version.clone(),
            installed_channel: install.channel,
            available_version: Some("2.0".into()),
            available_channel: BuildChannel::Stable,
            status: UpdateStatus::UpdateAvailable,
            source: UpdateMetadataSource::OfficialReleaseApi,
            provenance: "fixture metadata".into(),
            checked_unix_seconds: 1,
            warning: None,
            save_state_warning: true,
        }
    }

    struct FixtureDownloader {
        bytes: Vec<u8>,
    }

    impl UpdateDownloader for FixtureDownloader {
        fn download(&mut self, _url: &str, destination: &Path) -> Result<(), UpdateExecutionError> {
            fs::write(destination, &self.bytes).map_err(|e| UpdateExecutionError::Io(e.to_string()))
        }
    }

    fn artifact(bytes: &[u8]) -> UpdateArtifact {
        UpdateArtifact {
            version: "2.0".into(),
            channel: BuildChannel::Stable,
            url: "https://example.invalid/dolphin".into(),
            sha256: Some(
                Sha256::digest(bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            ),
            source: UpdateMetadataSource::OfficialReleaseApi,
            provenance: "fixture artifact".into(),
        }
    }

    #[test]
    fn staged_update_publishes_and_rolls_back_without_source_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let mut installation = executable_install(directory.path());
        let update = update_result(&installation);
        let bytes = b"new emulator";
        let plan = plan_staged_update(&installation, &update, artifact(bytes), false);
        assert_eq!(plan.eligibility, UpdateExecutionEligibility::Ready);
        assert!(plan.save_state_warning);
        let source_hash = hash_file(&installation.executable_path).unwrap();
        let mut downloader = FixtureDownloader {
            bytes: bytes.to_vec(),
        };
        let journal =
            execute_staged_update(&plan, &installation, &update, false, &mut downloader).unwrap();
        assert_eq!(journal.state, UpdateTransactionState::Published);
        assert_eq!(fs::read(&installation.executable_path).unwrap(), bytes);
        assert_eq!(fs::read(&plan.rollback_path).unwrap(), b"old emulator");
        let restored = rollback_staged_update(&journal).unwrap();
        assert_eq!(restored.state, UpdateTransactionState::RolledBack);
        assert_eq!(
            hash_file(&installation.executable_path).unwrap(),
            source_hash
        );
        assert!(!plan.rollback_path.exists());
        installation.version = Some("2.0".into());
    }

    #[test]
    fn plan_and_execution_fail_closed_for_unsafe_or_stale_state() {
        let directory = tempfile::tempdir().unwrap();
        let installation = executable_install(directory.path());
        let update = update_result(&installation);
        let mut unverified_artifact = artifact(b"new emulator");
        unverified_artifact.sha256 = None;
        assert_eq!(
            plan_staged_update(&installation, &update, unverified_artifact, false).eligibility,
            UpdateExecutionEligibility::VerificationUnavailable
        );
        assert_eq!(
            plan_staged_update(&installation, &update, artifact(b"new emulator"), true).eligibility,
            UpdateExecutionEligibility::RunningBlocked
        );
        let mut changed = installation.clone();
        changed.version = Some("1.1".into());
        let plan = plan_staged_update(&installation, &update, artifact(b"new emulator"), false);
        let mut downloader = FixtureDownloader {
            bytes: b"new emulator".to_vec(),
        };
        assert_eq!(
            execute_staged_update(&plan, &changed, &update, false, &mut downloader),
            Err(UpdateExecutionError::Stale)
        );
        assert_eq!(
            fs::read(&installation.executable_path).unwrap(),
            b"old emulator"
        );
    }

    #[test]
    fn checksum_mismatch_never_publishes() {
        let directory = tempfile::tempdir().unwrap();
        let installation = executable_install(directory.path());
        let update = update_result(&installation);
        let plan = plan_staged_update(&installation, &update, artifact(b"expected"), false);
        let mut downloader = FixtureDownloader {
            bytes: b"wrong".to_vec(),
        };
        assert!(matches!(
            execute_staged_update(&plan, &installation, &update, false, &mut downloader),
            Err(UpdateExecutionError::Verification(_))
        ));
        assert_eq!(
            fs::read(&installation.executable_path).unwrap(),
            b"old emulator"
        );
        assert!(!plan.rollback_path.exists());
    }
}
