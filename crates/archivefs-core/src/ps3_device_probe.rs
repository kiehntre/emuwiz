//! Read-only PS3 device probing over an injected transport.
//!
//! The transport deliberately exposes no mutation methods. This phase only
//! probes capabilities and reads one bounded PARAM.SFO path supplied by the
//! caller's reviewed remote root.

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;

use crate::game_identity::{IdentityKind, IdentityPlatform, IdentityStatus};
use crate::mod_package::SelectedGameForMod;
use crate::param_sfo::{MAX_SFO_BYTES, SfoValue, parse_param_sfo};

const PARAM_SFO_RELATIVE_PATH: &str = "PS3_GAME/PARAM.SFO";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ps3TransportProbe {
    pub endpoint: String,
    pub can_read_files: bool,
    pub can_stat_files: bool,
    pub can_list_directories: bool,
    pub can_read_param_sfo: bool,
    pub plaintext_lan_warning: bool,
}

/// Read-only transport boundary. There are intentionally no upload, rename,
/// mkdir, delete, backup, restore, or command-execution methods.
pub trait Ps3ReadTransport {
    type Error: fmt::Display;

    fn connect_and_probe(&mut self) -> Result<Ps3TransportProbe, Self::Error>;
    fn read_bounded(&mut self, remote_path: &str, max_bytes: usize)
    -> Result<Vec<u8>, Self::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ps3DeviceCapabilities {
    pub can_read_files: bool,
    pub can_stat_files: bool,
    pub can_list_directories: bool,
    pub can_read_param_sfo: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ps3RemoteGameIdentity {
    pub candidate_root: String,
    pub param_sfo_path: String,
    pub status: IdentityStatus,
    pub title_id: Option<String>,
    pub app_version: Option<String>,
    pub category: Option<String>,
    pub provenance: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ps3RemoteIdentityMatch {
    Confirmed,
    Blocked,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ps3DeviceProbeResult {
    pub platform: IdentityPlatform,
    pub endpoint: Option<String>,
    pub capabilities: Option<Ps3DeviceCapabilities>,
    pub candidate_root: String,
    pub param_sfo_path: Option<String>,
    pub remote_identity: Option<Ps3RemoteGameIdentity>,
    pub identity_match: Ps3RemoteIdentityMatch,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

pub struct Ps3DeviceProbe<T> {
    transport: T,
}

impl<T> Ps3DeviceProbe<T>
where
    T: Ps3ReadTransport,
{
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn probe(
        &mut self,
        candidate_root: &str,
        selected_game: &SelectedGameForMod,
    ) -> Ps3DeviceProbeResult {
        let mut result = base_result(candidate_root);
        let Some(root) = safe_remote_root(candidate_root) else {
            result.blockers.push(
                "candidate remote game root must be absolute, non-empty, and traversal-free".into(),
            );
            return finish(result);
        };
        let probe = match self.transport.connect_and_probe() {
            Ok(probe) => probe,
            Err(error) => {
                result
                    .blockers
                    .push(format!("PS3 transport probe failed: {error}"));
                return finish(result);
            }
        };
        result.endpoint = Some(probe.endpoint.clone());
        result.capabilities = Some(Ps3DeviceCapabilities {
            can_read_files: probe.can_read_files,
            can_stat_files: probe.can_stat_files,
            can_list_directories: probe.can_list_directories,
            can_read_param_sfo: probe.can_read_param_sfo,
        });
        if probe.plaintext_lan_warning {
            result
                .warnings
                .push("PS3 FTP may be plaintext and relies on trusted LAN access".into());
        }
        if !probe.can_read_files || !probe.can_read_param_sfo {
            result
                .blockers
                .push("transport does not prove bounded PARAM.SFO read capability".into());
            return finish(result);
        }

        let param_sfo_path = format!("{root}/{PARAM_SFO_RELATIVE_PATH}");
        result.param_sfo_path = Some(param_sfo_path.clone());
        let bytes = match self.transport.read_bounded(&param_sfo_path, MAX_SFO_BYTES) {
            Ok(bytes) => bytes,
            Err(error) => {
                result
                    .blockers
                    .push(format!("remote PARAM.SFO is unavailable: {error}"));
                return finish(result);
            }
        };
        let Some(sfo) = parse_param_sfo(&bytes) else {
            result.blockers.push(
                "remote PARAM.SFO is malformed, truncated, or exceeds the bounded parser limit"
                    .into(),
            );
            return finish(result);
        };
        let title_id = text_field(&sfo, "TITLE_ID");
        let app_version = text_field(&sfo, "APP_VER");
        let category = text_field(&sfo, "CATEGORY");
        let status = if title_id.is_some() {
            IdentityStatus::Verified
        } else {
            IdentityStatus::Missing
        };
        result.remote_identity = Some(Ps3RemoteGameIdentity {
            candidate_root: root.to_string(),
            param_sfo_path,
            status,
            title_id: title_id.clone(),
            app_version,
            category,
            provenance: "bounded remote PS3_GAME/PARAM.SFO read".into(),
        });
        result.identity_match =
            compare_selected_identity(title_id.as_deref(), selected_game, &mut result);
        finish(result)
    }
}

fn base_result(candidate_root: &str) -> Ps3DeviceProbeResult {
    Ps3DeviceProbeResult {
        platform: IdentityPlatform::PlayStation3,
        endpoint: None,
        capabilities: None,
        candidate_root: candidate_root.to_string(),
        param_sfo_path: None,
        remote_identity: None,
        identity_match: Ps3RemoteIdentityMatch::Unknown,
        warnings: Vec::new(),
        blockers: Vec::new(),
    }
}

fn compare_selected_identity(
    remote_title_id: Option<&str>,
    selected_game: &SelectedGameForMod,
    result: &mut Ps3DeviceProbeResult,
) -> Ps3RemoteIdentityMatch {
    let selected_ids: BTreeSet<_> = selected_game
        .identity
        .evidence
        .iter()
        .filter(|item| {
            item.kind == IdentityKind::Ps3TitleId && item.status == IdentityStatus::Verified
        })
        .filter_map(|item| item.value.as_deref())
        .collect();
    if selected_game.identity.platform != IdentityPlatform::PlayStation3 {
        result
            .blockers
            .push("selected game is not verified as PlayStation 3 content".into());
        return Ps3RemoteIdentityMatch::Unknown;
    }
    if selected_ids.len() != 1 {
        result
            .blockers
            .push("selected game lacks exactly one verified PS3 Title ID".into());
        return Ps3RemoteIdentityMatch::Unknown;
    }
    let selected_id = selected_ids.iter().next().expect("length checked");
    match remote_title_id {
        Some(remote_id) if remote_id == *selected_id => Ps3RemoteIdentityMatch::Confirmed,
        Some(_) => {
            result
                .blockers
                .push("remote PS3 Title ID does not match the selected game".into());
            Ps3RemoteIdentityMatch::Blocked
        }
        None => {
            result
                .warnings
                .push("remote PARAM.SFO has no verified PS3 Title ID".into());
            Ps3RemoteIdentityMatch::Unknown
        }
    }
}

fn text_field(sfo: &crate::param_sfo::SfoObservation, key: &str) -> Option<String> {
    match sfo.get(key) {
        Some(SfoValue::Text(value)) if !value.is_empty() => Some(value.to_string()),
        _ => None,
    }
}

fn safe_remote_root(root: &str) -> Option<&str> {
    if root.is_empty()
        || root.contains('\0')
        || root.contains('\\')
        || root.contains("//")
        || root.split('/').any(|component| component == "..")
        || !root.starts_with('/')
        || root == "/"
    {
        return None;
    }
    Some(root.trim_end_matches('/'))
}

fn finish(mut result: Ps3DeviceProbeResult) -> Ps3DeviceProbeResult {
    result.warnings.sort();
    result.warnings.dedup();
    result.blockers.sort();
    result.blockers.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_identity::{
        GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat,
        IdentityProvenance,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    struct FakeTransport {
        probe: Result<Ps3TransportProbe, String>,
        files: BTreeMap<String, Vec<u8>>,
        reads: usize,
    }

    impl Ps3ReadTransport for FakeTransport {
        type Error = String;

        fn connect_and_probe(&mut self) -> Result<Ps3TransportProbe, Self::Error> {
            self.probe.clone()
        }

        fn read_bounded(
            &mut self,
            remote_path: &str,
            max_bytes: usize,
        ) -> Result<Vec<u8>, Self::Error> {
            self.reads += 1;
            self.files
                .get(remote_path)
                .cloned()
                .map(|bytes| bytes.into_iter().take(max_bytes).collect())
                .ok_or_else(|| "not found".into())
        }
    }

    fn selected(title_id: Option<&str>) -> SelectedGameForMod {
        let evidence = title_id
            .into_iter()
            .map(|value| IdentityEvidence {
                kind: IdentityKind::Ps3TitleId,
                status: IdentityStatus::Verified,
                value: Some(value.into()),
                confidence: IdentityConfidence::ExactBytes,
                provenance: IdentityProvenance {
                    archive_path: PathBuf::from("/fixture"),
                    member_path: None,
                    member_index: None,
                    method: "fixture".into(),
                },
                diagnostic: String::new(),
            })
            .collect();
        SelectedGameForMod {
            game_root: PathBuf::from("/fixture"),
            identity: GameIdentityReport {
                archive_path: PathBuf::from("/fixture"),
                platform: IdentityPlatform::PlayStation3,
                format: IdentityImageFormat::LooseCartridgeRom,
                evidence,
                warnings: Vec::new(),
                bytes_read: 0,
                archive_members_inspected: 0,
                metadata_paths_inspected: 0,
                nested_container_depth: 0,
                complete: true,
            },
        }
    }

    fn sfo(entries: &[(&str, &str)]) -> Vec<u8> {
        let key_start = 20 + entries.len() as u32 * 16;
        let keys: Vec<_> = entries
            .iter()
            .map(|(key, _)| format!("{key}\0").into_bytes())
            .collect();
        let values: Vec<_> = entries
            .iter()
            .map(|(_, value)| format!("{value}\0").into_bytes())
            .collect();
        let data_start = key_start + keys.iter().map(Vec::len).sum::<usize>() as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\0PSF");
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&key_start.to_le_bytes());
        bytes.extend_from_slice(&data_start.to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        let mut key_offset = 0_u16;
        let mut data_offset = 0_u32;
        for (index, value) in values.iter().enumerate() {
            bytes.extend_from_slice(&key_offset.to_le_bytes());
            bytes.extend_from_slice(&0x0204_u16.to_le_bytes());
            bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&data_offset.to_le_bytes());
            key_offset += keys[index].len() as u16;
            data_offset += value.len() as u32;
        }
        for key in keys {
            bytes.extend_from_slice(&key);
        }
        for value in values {
            bytes.extend_from_slice(&value);
        }
        bytes
    }

    fn fake(bytes: Vec<u8>) -> FakeTransport {
        FakeTransport {
            probe: Ok(Ps3TransportProbe {
                endpoint: "fixture".into(),
                can_read_files: true,
                can_stat_files: true,
                can_list_directories: false,
                can_read_param_sfo: true,
                plaintext_lan_warning: true,
            }),
            files: [("/dev_hdd0/game/BLES30000/PS3_GAME/PARAM.SFO".into(), bytes)]
                .into_iter()
                .collect(),
            reads: 0,
        }
    }

    #[test]
    fn matching_identity_projects_metadata_and_capabilities() {
        let mut probe = Ps3DeviceProbe::new(fake(sfo(&[
            ("TITLE_ID", "BLES30000"),
            ("APP_VER", "01.02"),
            ("CATEGORY", "DG"),
        ])));
        let result = probe.probe("/dev_hdd0/game/BLES30000", &selected(Some("BLES30000")));
        assert_eq!(result.identity_match, Ps3RemoteIdentityMatch::Confirmed);
        assert_eq!(
            result
                .remote_identity
                .as_ref()
                .unwrap()
                .app_version
                .as_deref(),
            Some("01.02")
        );
        assert_eq!(
            result.remote_identity.as_ref().unwrap().category.as_deref(),
            Some("DG")
        );
        assert!(result.capabilities.as_ref().unwrap().can_read_param_sfo);
        assert_eq!(
            result.warnings,
            vec!["PS3 FTP may be plaintext and relies on trusted LAN access"]
        );
    }

    #[test]
    fn mismatch_is_blocked_and_missing_selected_identity_is_unknown() {
        let mut mismatch = Ps3DeviceProbe::new(fake(sfo(&[("TITLE_ID", "BLES30001")])));
        let result = mismatch.probe("/dev_hdd0/game/BLES30000", &selected(Some("BLES30000")));
        assert_eq!(result.identity_match, Ps3RemoteIdentityMatch::Blocked);
        assert!(
            result
                .blockers
                .iter()
                .any(|item| item.contains("does not match"))
        );
        let mut unknown = Ps3DeviceProbe::new(fake(sfo(&[("TITLE_ID", "BLES30000")])));
        let result = unknown.probe("/dev_hdd0/game/BLES30000", &selected(None));
        assert_eq!(result.identity_match, Ps3RemoteIdentityMatch::Unknown);
    }

    #[test]
    fn malformed_missing_and_capability_failures_are_read_only_refusals() {
        let mut malformed = Ps3DeviceProbe::new(fake(vec![0; 8]));
        let result = malformed.probe("/dev_hdd0/game/BLES30000", &selected(Some("BLES30000")));
        assert!(
            result
                .blockers
                .iter()
                .any(|item| item.contains("malformed"))
        );
        let mut missing = Ps3DeviceProbe::new(fake(sfo(&[("CATEGORY", "DG")])));
        let result = missing.probe("/dev_hdd0/game/BLES30000", &selected(Some("BLES30000")));
        assert_eq!(result.identity_match, Ps3RemoteIdentityMatch::Unknown);
        assert!(result.remote_identity.unwrap().title_id.is_none());
        let mut unavailable = fake(Vec::new());
        unavailable.probe = Ok(Ps3TransportProbe {
            endpoint: "fixture".into(),
            can_read_files: false,
            can_stat_files: false,
            can_list_directories: false,
            can_read_param_sfo: false,
            plaintext_lan_warning: false,
        });
        let result = Ps3DeviceProbe::new(unavailable)
            .probe("/dev_hdd0/game/BLES30000", &selected(Some("BLES30000")));
        assert!(
            result
                .blockers
                .iter()
                .any(|item| item.contains("capability"))
        );
    }

    #[test]
    fn unsafe_root_is_refused_before_transport_read() {
        let result = Ps3DeviceProbe::new(fake(sfo(&[("TITLE_ID", "BLES30000")])))
            .probe("/dev_hdd0/game/../escape", &selected(Some("BLES30000")));
        assert_eq!(result.identity_match, Ps3RemoteIdentityMatch::Unknown);
        assert!(
            result
                .blockers
                .iter()
                .any(|item| item.contains("traversal"))
        );
    }
}
