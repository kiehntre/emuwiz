//! Record evidence extending the existing cheat-provider boundary. Source
//! quality never guarantees cheat behaviour. Identity and lineage remain
//! separate, using the shared evidence vocabulary.

use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, LineageRelation, SourceArtifactIdentity, SourceFamily,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatSourceKind {
    LocalFile,
    RetroArchPack,
    Bundled,
    ManualUserEntry,
    ImportedDatabase,
    CommunityDatabase,
    EmulatorNative,
    GeneratedDerivative,
    #[default]
    Unknown,
}

/// Classification of the source history, never a safety rating.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatSourceQuality {
    UserAuthored,
    LocalKnown,
    BundledVerifiedSource,
    CommunityCurated,
    ImportedUnverified,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatApplicabilityKind {
    VerifiedGameIdentity,
    DatHashMatch,
    ContentHashMatch,
    SerialMatch,
    TitleMatch,
    Region,
    Revision,
    FilenameAssociation,
    ManualAssociation,
    SourcePackAssociation,
}

/// Source declarations use Weak/DisplayOnly. A match may be Strong only
/// when the caller actually established it against the selected game.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CheatApplicabilityEvidence {
    pub kind: CheatApplicabilityKind,
    pub value: String,
    pub strength: ClaimStrength,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatNormalizationStatus {
    #[default]
    Unchanged,
    ComparisonOnly,
    Derived,
}

/// One source record. Missing metadata stays missing; legacy strings are
/// never parsed into claims of authority. Paths are internal audit data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(default)]
pub struct CheatRecordProvenance {
    pub source_kind: CheatSourceKind,
    pub source_quality: CheatSourceQuality,
    pub source_path: Option<PathBuf>,
    pub provider_id: Option<String>,
    pub provider_name: Option<String>,
    pub record_key: Option<String>,
    pub record_index: Option<u32>,
    pub source_format: Option<String>,
    pub artifact: Option<SourceArtifactIdentity>,
    pub lineage: LineageRelation,
    /// Known original source/evidence identity, when a provider supplies it.
    pub upstream_evidence_key: Option<String>,
    pub original_description: Option<String>,
    pub original_code: Option<String>,
    pub normalized_description: Option<String>,
    pub normalized_code: Option<String>,
    pub normalization: CheatNormalizationStatus,
    pub applicability: Vec<CheatApplicabilityEvidence>,
}

impl Default for CheatRecordProvenance {
    fn default() -> Self {
        Self {
            source_kind: CheatSourceKind::Unknown,
            source_quality: CheatSourceQuality::Unknown,
            source_path: None,
            provider_id: None,
            provider_name: None,
            record_key: None,
            record_index: None,
            source_format: None,
            artifact: None,
            lineage: LineageRelation::Unknown,
            upstream_evidence_key: None,
            original_description: None,
            original_code: None,
            normalized_description: None,
            normalized_code: None,
            normalization: CheatNormalizationStatus::Unchanged,
            applicability: Vec::new(),
        }
    }
}

impl CheatRecordProvenance {
    pub fn original(description: Option<String>, code: Option<String>) -> Self {
        Self {
            original_description: description,
            original_code: code,
            ..Self::default()
        }
    }

    /// Selected local input is known locally, but is not thereby user-authored.
    pub fn local(path: &Path, bytes: &[u8], format: &str) -> Self {
        let sha256: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Self::local_with_sha256(path, &sha256, format)
    }

    /// As [`Self::local`] for callers that already hold the artifact digest
    /// (for example a pack preview that hashed the file while inspecting it).
    pub fn local_with_sha256(path: &Path, sha256: &str, format: &str) -> Self {
        Self {
            source_kind: CheatSourceKind::LocalFile,
            source_quality: CheatSourceQuality::LocalKnown,
            source_path: Some(path.to_path_buf()),
            source_format: Some(format.into()),
            artifact: Some(SourceArtifactIdentity {
                source_family: SourceFamily::Unknown,
                upstream_version: None,
                artifact_sha256: (!sha256.is_empty()).then(|| sha256.to_string()),
                artifact_name: path.file_name().and_then(|s| s.to_str()).map(str::to_owned),
            }),
            ..Self::default()
        }
    }

    pub fn manual(description: &str, code: &str, game: &str) -> Self {
        Self {
            source_kind: CheatSourceKind::ManualUserEntry,
            source_quality: CheatSourceQuality::UserAuthored,
            original_description: Some(description.into()),
            original_code: Some(code.into()),
            applicability: vec![CheatApplicabilityEvidence {
                kind: CheatApplicabilityKind::ManualAssociation,
                value: game.into(),
                strength: ClaimStrength::Weak,
            }],
            ..Self::default()
        }
    }

    /// Filename-only display: normal GUI projections must not expose audit paths.
    pub fn display_filename(&self) -> Option<&str> {
        self.source_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|p| p.to_str())
            .or_else(|| self.artifact.as_ref()?.artifact_name.as_deref())
    }

    pub fn note_comparison(&mut self, description: Option<String>, code: Option<String>) {
        self.normalized_description = description;
        self.normalized_code = code;
        self.normalization = CheatNormalizationStatus::ComparisonOnly;
    }
}

/// Stable audit ordering, without selecting a winner or dropping observations.
pub fn order_cheat_provenance(evidence: &mut [CheatRecordProvenance]) {
    evidence.sort();
}

/// Counts identifiable delivered records, collapsing known mirrors/relays.
/// This is not a count of independent confirmations. Unknown records cannot
/// contribute an invented source identity.
pub fn cheat_evidence_source_count(evidence: &[CheatRecordProvenance]) -> usize {
    use std::collections::BTreeSet;
    let mut keys = BTreeSet::new();
    for item in evidence {
        if let Some(key) = &item.upstream_evidence_key {
            keys.insert(("upstream", key.clone(), String::new()));
        } else if let Some(hash) = item
            .artifact
            .as_ref()
            .and_then(|a| a.artifact_sha256.as_ref())
        {
            keys.insert((
                "artifact",
                hash.clone(),
                format!("{:?}:{:?}", item.record_key, item.record_index),
            ));
        } else if let (Some(provider), Some(record)) = (&item.provider_id, &item.record_key) {
            keys.insert(("provider", provider.clone(), record.clone()));
        }
    }
    keys.len()
}

#[cfg(test)]
mod tests;
