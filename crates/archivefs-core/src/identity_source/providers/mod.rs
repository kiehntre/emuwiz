//! Two explicit official-tool providers, not a marketplace or a replacement DAT stack.
pub mod discovery;
pub mod mame;
pub mod scummvm;
mod tool;

use super::model::IdentityProvider;
use crate::dat::model::ParsedDat;
use crate::identity_source::managed_snapshot::{
    ActivationPreview, ActivationResult, ManagedSourceDescriptor, ManagedSourceKind,
    ManagedSourceMetadata, ManagedSourceReference, ManagedSourceSnapshot, ManagedSourceStore,
    ManagedSourceTrust, ValidatedCandidate, ValidationReport,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub type ProviderResult<T> = Result<T, String>;
pub const PARSER_VERSION: u32 = 1;
pub const MAX_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;

/// Provider-facing adapter over the generic immutable managed-source
/// lifecycle. Provider payloads remain provider-owned JSON; this type only
/// supplies the descriptor, validation summary, activation, and safe reload.
#[derive(Debug, Clone)]
pub struct ManagedProviderStore {
    provider: IdentityProvider,
    store: ManagedSourceStore,
}

impl ManagedProviderStore {
    pub fn new(
        root: PathBuf,
        provider: IdentityProvider,
        executable: &Path,
    ) -> ProviderResult<Self> {
        if !matches!(provider, IdentityProvider::Mame | IdentityProvider::ScummVm) {
            return Err("Only MAME and ScummVM use the managed provider store".into());
        }
        let descriptor = ManagedSourceDescriptor {
            provider_id: provider.slug().into(),
            display_name: format!("{} official provider snapshot", provider.label()),
            source_kind: ManagedSourceKind::Local,
            source: ManagedSourceReference::LocalPath(executable.to_path_buf()),
            expected_media_type: "application/vnd.emuwiz.identity-provider+json".into(),
            maximum_size_bytes: MAX_SNAPSHOT_BYTES,
            attribution_url: None,
            parser_schema_version: PARSER_VERSION.to_string(),
            trust: ManagedSourceTrust::Official,
        };
        ManagedSourceStore::new(root, descriptor)
            .map(|store| Self { provider, store })
            .map_err(|error| error.to_string())
    }

    pub fn store(&self) -> &ManagedSourceStore {
        &self.store
    }

    pub fn stage_snapshot(
        &self,
        snapshot: &ProviderSnapshot,
    ) -> ProviderResult<ValidatedCandidate> {
        if snapshot.provider != self.provider {
            return Err("provider snapshot does not match managed provider store".into());
        }
        snapshot.validate()?;
        let bytes = serde_json::to_vec(snapshot).map_err(|error| error.to_string())?;
        let staged = self
            .store
            .stage_bytes(
                &bytes,
                ManagedSourceMetadata {
                    provider_version: Some(snapshot.version.clone()),
                    ..ManagedSourceMetadata::default()
                },
            )
            .map_err(|error| error.to_string())?;
        self.store
            .validate_candidate(
                staged,
                ValidationReport {
                    valid: true,
                    summary: format!(
                        "{} provider snapshot with {} records",
                        self.provider.label(),
                        snapshot.record_count()
                    ),
                    record_count: Some(snapshot.record_count() as u64),
                    warnings: snapshot.warnings.clone(),
                },
            )
            .map_err(|error| error.to_string())
    }

    pub fn preview_activation(
        &self,
        candidate: &ValidatedCandidate,
    ) -> ProviderResult<ActivationPreview> {
        self.store
            .preview_activation(candidate)
            .map_err(|error| error.to_string())
    }

    pub fn activate_snapshot(
        &self,
        candidate: &ValidatedCandidate,
        expected_active: Option<&str>,
    ) -> ProviderResult<ActivationResult> {
        self.store
            .activate_snapshot(candidate, expected_active)
            .map_err(|error| error.to_string())
    }

    pub fn rollback_snapshot(&self, hash: &str) -> ProviderResult<ActivationResult> {
        self.store
            .rollback_snapshot(hash)
            .map_err(|error| error.to_string())
    }

    pub fn active_snapshot(&self) -> ProviderResult<Option<ProviderSnapshot>> {
        let Some(bytes) = self
            .store
            .active_snapshot_bytes()
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let snapshot: ProviderSnapshot =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        snapshot.validate()?;
        if snapshot.provider != self.provider || snapshot.sha256()? != sha256(&bytes) {
            return Err("active provider snapshot failed payload integrity validation".into());
        }
        Ok(Some(snapshot))
    }

    pub fn active_managed_snapshot(&self) -> ProviderResult<Option<ManagedSourceSnapshot>> {
        self.store
            .active_snapshot()
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum ProviderRecords {
    DatLike {
        catalogue: ParsedDat,
        listxml: String,
    },
    NativeDetection(Vec<scummvm::DetectionRecord>),
}

/// Immutable payload. Import time is stored by the catalogue independently of
/// content identity, so checking the same tool twice does not invent an update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub provider: IdentityProvider,
    pub version: String,
    pub source_identifier: String,
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub source_sha256: String,
    pub parser_version: u32,
    pub records: ProviderRecords,
    pub warnings: Vec<String>,
}

impl ProviderSnapshot {
    pub fn sha256(&self) -> ProviderResult<String> {
        serde_json::to_vec(self)
            .map(|b| sha256(&b))
            .map_err(|e| e.to_string())
    }
    pub fn record_count(&self) -> usize {
        match &self.records {
            ProviderRecords::DatLike { catalogue, .. } => catalogue.games.len(),
            ProviderRecords::NativeDetection(records) => records.len(),
        }
    }
    pub fn validate(&self) -> ProviderResult<()> {
        if self.parser_version != PARSER_VERSION
            || self.version.trim().is_empty()
            || self.record_count() == 0
        {
            return Err("Unsupported or empty provider snapshot".into());
        }
        match (&self.provider, &self.records) {
            (IdentityProvider::Mame, ProviderRecords::DatLike { catalogue: dat, .. })
                if dat.source.ecosystem == crate::dat::model::DatEcosystem::MAMEArcade => {}
            (IdentityProvider::ScummVm, ProviderRecords::NativeDetection(_)) => {}
            _ => return Err("Provider and evidence model disagree".into()),
        }
        Ok(())
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchStatus {
    Exact,
    Probable,
    Ambiguous,
    NoMatch,
    NeedsRecheck,
}

/// ScummVM result semantics are intentionally finer than a boolean match.
/// A runtime ID, a table signature, and EmuWiz filename clues are different
/// kinds of evidence and must remain distinguishable to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionClass {
    OfficialExact,
    OfficialFallback,
    OfficialDetectionCoverageGap,
    EmuwizDerivedProbable,
    Unknown,
}
impl DetectionClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::OfficialExact => "Official exact",
            Self::OfficialFallback => "Official fallback",
            Self::OfficialDetectionCoverageGap => "Official detection coverage gap",
            Self::EmuwizDerivedProbable => "EmuWiz-derived probable",
            Self::Unknown => "Unknown",
        }
    }
}
impl MatchStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Exact => "Exact",
            Self::Probable => "Probable",
            Self::Ambiguous => "Ambiguous",
            Self::NoMatch => "No match",
            Self::NeedsRecheck => "Needs re-check",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchOrigin {
    OfficialMame,
    OfficialScummVm,
    EmuwizDiscovery,
}
impl MatchOrigin {
    pub fn label(self) -> &'static str {
        match self {
            Self::OfficialMame => "MAME official",
            Self::OfficialScummVm => "Official ScummVM match",
            Self::EmuwizDiscovery => "EmuWiz discovery — not an official match",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderIdentityResult {
    pub provider: IdentityProvider,
    pub snapshot_sha256: String,
    pub path: PathBuf,
    pub status: MatchStatus,
    pub detection_class: DetectionClass,
    pub origin: MatchOrigin,
    pub detector_method: Option<String>,
    pub official_game_id: Option<String>,
    pub configured_target_id: Option<String>,
    pub configured_target_evidence: Option<String>,
    pub possible_base_game_id: Option<String>,
    pub cleaned_local_title: Option<String>,
    pub candidates: Vec<String>,
    pub details: Vec<String>,
    pub discovery: Option<discovery::DiscoveryResult>,
}
impl ProviderIdentityResult {
    /// Presentation only. The original evidence and source attribution survive.
    pub fn status_against(&self, active_snapshot: Option<&str>) -> MatchStatus {
        if active_snapshot != Some(self.snapshot_sha256.as_str()) {
            MatchStatus::NeedsRecheck
        } else {
            self.status
        }
    }
}

/// Explicit check only: captures metadata into memory. It does not publish a
/// snapshot, update the user's emulator, run a game or touch its configuration.
pub fn check_provider(
    provider: IdentityProvider,
    executable: &Path,
) -> ProviderResult<ProviderSnapshot> {
    match provider {
        IdentityProvider::Mame => mame::capture(executable),
        IdentityProvider::ScummVm => scummvm::capture(executable),
        _ => Err("Only MAME and ScummVM belong to this proof of concept".into()),
    }
}

pub fn verify(snapshot: &ProviderSnapshot, path: &Path) -> ProviderResult<ProviderIdentityResult> {
    snapshot.validate()?;
    match snapshot.provider {
        IdentityProvider::Mame => mame::verify(snapshot, path),
        IdentityProvider::ScummVm => scummvm::verify(snapshot, path),
        _ => Err("Unsupported provider".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn fixture(version: &str) -> ProviderSnapshot {
        ProviderSnapshot {
            provider: IdentityProvider::ScummVm,
            version: version.into(),
            source_identifier: "fixture:scummvm/detection-export".into(),
            executable: PathBuf::from("/opt/scummvm/scummvm"),
            executable_sha256: "11".repeat(32),
            source_sha256: "22".repeat(32),
            parser_version: PARSER_VERSION,
            records: ProviderRecords::NativeDetection(vec![scummvm::DetectionRecord {
                engine: "scumm".into(),
                game_id: "monkey2".into(),
                title: "Monkey Island 2".into(),
                platform: "dos".into(),
                language: "en".into(),
                variant: "CD".into(),
                files: Vec::new(),
                fields: BTreeMap::new(),
            }]),
            warnings: vec!["fixture coverage is intentionally narrow".into()],
        }
    }

    #[test]
    fn provider_snapshots_use_explicit_activation_and_rollback() {
        let root = tempfile::tempdir().unwrap();
        let store = ManagedProviderStore::new(
            root.path().join("managed"),
            IdentityProvider::ScummVm,
            Path::new("/opt/scummvm/scummvm"),
        )
        .unwrap();
        let first = fixture("2.8.0");
        let first_candidate = store.stage_snapshot(&first).unwrap();
        assert!(store.preview_activation(&first_candidate).unwrap().changed);
        let first_activation = store.activate_snapshot(&first_candidate, None).unwrap();
        assert_eq!(
            first_activation.change.freshness,
            crate::identity_source::managed_snapshot::VerificationFreshness::NeedsRecheck
        );
        assert_eq!(store.active_snapshot().unwrap().unwrap().version, "2.8.0");

        let second = fixture("2.9.0");
        let second_candidate = store.stage_snapshot(&second).unwrap();
        let preview = store.preview_activation(&second_candidate).unwrap();
        assert!(preview.changed);
        let second_activation = store
            .activate_snapshot(&second_candidate, Some(&first.sha256().unwrap()))
            .unwrap();
        assert_eq!(
            second_activation.change.freshness,
            crate::identity_source::managed_snapshot::VerificationFreshness::NeedsRecheck
        );
        assert_eq!(
            ProviderIdentityResult {
                provider: IdentityProvider::ScummVm,
                snapshot_sha256: first.sha256().unwrap(),
                path: PathBuf::from("fixture"),
                status: MatchStatus::Exact,
                detection_class: DetectionClass::OfficialExact,
                origin: MatchOrigin::OfficialScummVm,
                detector_method: None,
                official_game_id: None,
                configured_target_id: None,
                configured_target_evidence: None,
                possible_base_game_id: None,
                cleaned_local_title: None,
                candidates: Vec::new(),
                details: Vec::new(),
                discovery: None,
            }
            .status_against(Some(&second.sha256().unwrap())),
            MatchStatus::NeedsRecheck
        );
        let rolled_back = store.rollback_snapshot(&first.sha256().unwrap()).unwrap();
        assert_eq!(
            rolled_back.active.provider_version.as_deref(),
            Some("2.8.0")
        );
        assert_eq!(store.active_snapshot().unwrap().unwrap().version, "2.8.0");
    }

    #[test]
    fn active_provider_snapshot_is_available_offline_without_network() {
        let root = tempfile::tempdir().unwrap();
        let store = ManagedProviderStore::new(
            root.path().join("managed"),
            IdentityProvider::ScummVm,
            Path::new("/opt/scummvm/scummvm"),
        )
        .unwrap();
        let snapshot = fixture("2.8.0");
        let candidate = store.stage_snapshot(&snapshot).unwrap();
        store.activate_snapshot(&candidate, None).unwrap();
        let check = store
            .store()
            .check_for_update(true, &OfflineTransport)
            .unwrap();
        assert!(matches!(
            check,
            crate::identity_source::managed_snapshot::UpdateCheck::Offline { active: Some(_) }
        ));
        assert_eq!(store.active_snapshot().unwrap().unwrap().version, "2.8.0");
    }

    #[test]
    fn mame_provider_snapshots_use_the_same_managed_lifecycle() {
        use crate::dat::model::{
            DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatSource, ParsedDat,
        };

        let root = tempfile::tempdir().unwrap();
        let store = ManagedProviderStore::new(
            root.path().join("managed"),
            IdentityProvider::Mame,
            Path::new("/opt/mame/mame"),
        )
        .unwrap();
        let snapshot = ProviderSnapshot {
            provider: IdentityProvider::Mame,
            version: "0.264".into(),
            source_identifier: "official-local:mame/-listxml/puckman".into(),
            executable: PathBuf::from("/opt/mame/mame"),
            executable_sha256: "33".repeat(32),
            source_sha256: "44".repeat(32),
            parser_version: PARSER_VERSION,
            records: ProviderRecords::DatLike {
                catalogue: ParsedDat {
                    source: DatSource {
                        format: DatFormat::Logiqx,
                        ecosystem: DatEcosystem::MAMEArcade,
                        file_path: "official-local:mame/-listxml/puckman".into(),
                        name: Some("MAME official".into()),
                        description: None,
                        version: Some("0.264".into()),
                        author: None,
                        homepage: None,
                        clrmamepro_header: None,
                        entry_count: 1,
                        rom_count: 0,
                        parse_warnings: Vec::new(),
                        packing_policy: DatPackingPolicy::Standard,
                    },
                    games: vec![DatGameEntry {
                        name: "puckman".into(),
                        ..DatGameEntry::default()
                    }],
                },
                listxml: "<mame build=\"0.264\"><machine name=\"puckman\"/></mame>".into(),
            },
            warnings: Vec::new(),
        };
        let candidate = store.stage_snapshot(&snapshot).unwrap();
        let activation = store.activate_snapshot(&candidate, None).unwrap();
        assert_eq!(activation.active.provider_id, "mame");
        assert_eq!(store.active_snapshot().unwrap().unwrap().version, "0.264");
    }

    struct OfflineTransport;
    impl crate::identity_source::managed_snapshot::ManagedSourceTransport for OfflineTransport {
        fn metadata(
            &self,
            _url: &str,
            _headers: &[(String, String)],
        ) -> std::result::Result<
            crate::identity_source::managed_snapshot::SourceResponseMetadata,
            String,
        > {
            Err("network must not be used in offline mode".into())
        }

        fn fetch(
            &self,
            _url: &str,
            _headers: &[(String, String)],
            _maximum_size: u64,
            _destination: &mut dyn std::io::Write,
        ) -> std::result::Result<
            crate::identity_source::managed_snapshot::SourceResponseMetadata,
            String,
        > {
            Err("network must not be used in offline mode".into())
        }
    }
}
