//! Read-only, channel-aware update availability checks for E1 installations.
//!
//! Metadata is the only remote material involved.  No release asset is ever
//! requested.  The provider boundary keeps tests entirely local and makes
//! network failure an ordinary `Offline`/`LatestUnknown` result.

use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::emulator_inventory::{
    BuildChannel, EmulatorInstallation, InstallationType, InventoryEmulator, SaveStateRisk,
};

pub const MAX_METADATA_BYTES: usize = 64 * 1024;
pub const METADATA_TIMEOUT: Duration = Duration::from_secs(5);
pub const CACHE_TTL: Duration = Duration::from_secs(15 * 60);

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
}
