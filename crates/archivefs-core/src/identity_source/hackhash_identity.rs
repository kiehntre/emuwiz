//! Deterministic matching of inspected local ROM hashes against an active
//! HackHash snapshot.
//!
//! This is deliberately an evidence adapter, not an identity authority.  A
//! result can explain a community claim and can be fed to the generic evidence
//! resolver, but it can never manufacture native `Verified` identity.

use serde::{Deserialize, Serialize};

use super::hackhash::{HackHashBaseRom, HackHashExport, HackHashPatch, HackHashRecord};
use super::managed_snapshot::ManagedSourceSnapshot;
use super::model::HashAlgorithm;
use crate::evidence_resolution::{
    ClaimProperty, EvidenceClaim, EvidenceProvenance, EvidenceSource, EvidenceStrength,
    EvidenceValue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HackHashSnapshotState {
    Active,
    Inactive,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HackHashEvidenceClass {
    ExactPatchedOutput,
    ProbableHackFamily,
    BaseRomRelationship,
    KnownPatchRelationship,
    ConflictingExternalClaims,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HackHashMatchConfidence {
    Exact,
    StrongRelationship,
    Probable,
    Ambiguous,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashObservedHashes {
    pub sha1: Option<String>,
    pub md5: Option<String>,
    pub crc32: Option<String>,
}

impl HackHashObservedHashes {
    pub fn new(sha1: Option<&str>, md5: Option<&str>, crc32: Option<&str>) -> Self {
        Self {
            sha1: sha1.map(normalize),
            md5: md5.map(normalize),
            crc32: crc32.map(normalize),
        }
    }

    fn value(&self, algorithm: HashAlgorithm) -> Option<&str> {
        match algorithm {
            HashAlgorithm::Sha1 => self.sha1.as_deref(),
            HashAlgorithm::Md5 => self.md5.as_deref(),
            HashAlgorithm::Crc32 => self.crc32.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashOutputHashes {
    pub sha1: String,
    pub md5: String,
    pub crc32: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashMatch {
    pub record_index: usize,
    pub evidence_class: HackHashEvidenceClass,
    pub confidence: HackHashMatchConfidence,
    pub matched_algorithm: HashAlgorithm,
    pub hack_title: String,
    pub family: Option<String>,
    pub version: String,
    pub author: Option<String>,
    pub base_rom: Option<HackHashBaseRom>,
    pub patch: HackHashPatch,
    pub patched_output_hashes: HackHashOutputHashes,
    pub provider_snapshot_sha256: String,
    pub provider_provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashConflict {
    pub record_indices: Vec<usize>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashIdentityResult {
    pub snapshot_state: HackHashSnapshotState,
    pub snapshot_sha256: String,
    pub matches: Vec<HackHashMatch>,
    pub conflicts: Vec<HackHashConflict>,
    pub warnings: Vec<String>,
}

impl HackHashIdentityResult {
    pub fn has_exact_match(&self) -> bool {
        self.matches
            .iter()
            .any(|item| item.evidence_class == HackHashEvidenceClass::ExactPatchedOutput)
    }

    /// Adapt the result to the existing resolver.  All claims from one
    /// provider snapshot share one derivation root, so duplicate provider
    /// records cannot independently promote a title to native Verified.
    pub fn evidence_claims(&self, subject: &str) -> Vec<EvidenceClaim> {
        if self.snapshot_state != HackHashSnapshotState::Active {
            return Vec::new();
        }
        self.matches
            .iter()
            .map(|item| {
                let value = format!(
                    "{}:{}:{}",
                    item.hack_title, item.version, item.provider_snapshot_sha256
                );
                let mut claim = EvidenceClaim::new(
                    format!("hackhash:{}", item.record_index),
                    subject,
                    ClaimProperty::GameIdentity,
                    EvidenceValue::GameIdentity {
                        namespace: "hackhash".into(),
                        value,
                    },
                    EvidenceSource::StructuredMetadata,
                    match item.evidence_class {
                        HackHashEvidenceClass::ExactPatchedOutput => EvidenceStrength::Strong,
                        _ => EvidenceStrength::Corroborated,
                    },
                    EvidenceProvenance::new(
                        item.provider_provenance.clone(),
                        format!("hackhash-snapshot:{}", self.snapshot_sha256),
                    ),
                );
                claim.scope.source_identity = Some("HackHash community evidence".into());
                claim
            })
            .collect()
    }
}

pub fn match_snapshot(
    snapshot: &ManagedSourceSnapshot,
    export: &HackHashExport,
    observed: &HackHashObservedHashes,
    state: HackHashSnapshotState,
) -> HackHashIdentityResult {
    let mut result = HackHashIdentityResult {
        snapshot_state: state,
        snapshot_sha256: snapshot.sha256.clone(),
        matches: Vec::new(),
        conflicts: Vec::new(),
        warnings: Vec::new(),
    };
    if state != HackHashSnapshotState::Active {
        result.warnings.push(match state {
            HackHashSnapshotState::Inactive => "HackHash snapshot is inactive.".into(),
            HackHashSnapshotState::Stale => "HackHash snapshot is stale.".into(),
            HackHashSnapshotState::Active => unreachable!(),
        });
        return result;
    }

    let algorithms = [
        HashAlgorithm::Sha1,
        HashAlgorithm::Md5,
        HashAlgorithm::Crc32,
    ];
    let mut selected: Option<(HashAlgorithm, Vec<usize>)> = None;
    let mut lower_matches = Vec::new();
    for algorithm in algorithms {
        let Some(value) = observed.value(algorithm).filter(|value| !value.is_empty()) else {
            continue;
        };
        let indices: Vec<usize> = export
            .machines
            .iter()
            .enumerate()
            .filter_map(|(index, record)| {
                (output_hash(record, algorithm) == value).then_some(index)
            })
            .collect();
        if selected.is_none() && !indices.is_empty() {
            selected = Some((algorithm, indices));
        } else if selected.is_some() && !indices.is_empty() {
            lower_matches.extend(indices);
        }
    }

    if let Some((algorithm, indices)) = selected {
        for index in &indices {
            result.matches.push(make_match(
                snapshot,
                &export.machines[*index],
                *index,
                HackHashEvidenceClass::ExactPatchedOutput,
                if indices.len() > 1 {
                    HackHashMatchConfidence::Ambiguous
                } else {
                    HackHashMatchConfidence::Exact
                },
                algorithm,
            ));
        }
        if !lower_matches.is_empty() {
            lower_matches.sort_unstable();
            lower_matches.dedup();
            result.conflicts.push(HackHashConflict {
                record_indices: lower_matches,
                reason: format!(
                    "stronger {algorithm:?} output evidence disagrees with a weaker hash match"
                ),
            });
        }
    } else {
        // Relationship claims are deliberately weaker than an output match.
        for algorithm in algorithms {
            let Some(value) = observed.value(algorithm).filter(|value| !value.is_empty()) else {
                continue;
            };
            for (index, record) in export.machines.iter().enumerate() {
                if base_hash(record, algorithm).as_deref() == Some(value) {
                    result.matches.push(make_match(
                        snapshot,
                        record,
                        index,
                        HackHashEvidenceClass::BaseRomRelationship,
                        HackHashMatchConfidence::StrongRelationship,
                        algorithm,
                    ));
                }
            }
        }
        if let Some(value) = observed.sha1.as_deref() {
            for (index, record) in export.machines.iter().enumerate() {
                if record
                    .details
                    .patch
                    .sha1
                    .as_deref()
                    .map(normalize)
                    .as_deref()
                    == Some(value)
                {
                    result.matches.push(make_match(
                        snapshot,
                        record,
                        index,
                        HackHashEvidenceClass::KnownPatchRelationship,
                        HackHashMatchConfidence::Probable,
                        HashAlgorithm::Sha1,
                    ));
                }
            }
        }
    }

    result.matches.sort_by_key(|item| {
        (
            class_rank(item.evidence_class),
            algorithm_rank(item.matched_algorithm),
            item.record_index,
        )
    });
    if result.matches.len() > 1
        && result
            .matches
            .iter()
            .map(|item| (&item.hack_title, &item.version, &item.family))
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 1
    {
        result.conflicts.push(HackHashConflict {
            record_indices: result
                .matches
                .iter()
                .map(|item| item.record_index)
                .collect(),
            reason: "multiple HackHash claims matched; family or version claims conflict".into(),
        });
    }
    result
}

fn make_match(
    snapshot: &ManagedSourceSnapshot,
    record: &HackHashRecord,
    record_index: usize,
    evidence_class: HackHashEvidenceClass,
    confidence: HackHashMatchConfidence,
    matched_algorithm: HashAlgorithm,
) -> HackHashMatch {
    HackHashMatch {
        record_index,
        evidence_class,
        confidence,
        matched_algorithm,
        hack_title: record.details.hack_name.clone(),
        family: record
            .details
            .hack_family
            .as_ref()
            .map(|family| family.name.clone()),
        version: record.details.version.clone(),
        author: record.details.author.clone(),
        base_rom: record.details.base_rom.clone(),
        patch: record.details.patch.clone(),
        patched_output_hashes: HackHashOutputHashes {
            sha1: normalize(&record.sha1),
            md5: normalize(&record.md5),
            crc32: normalize(&record.crc32),
        },
        provider_snapshot_sha256: snapshot.sha256.clone(),
        provider_provenance: "HackHash external community evidence".into(),
    }
}

fn output_hash(record: &HackHashRecord, algorithm: HashAlgorithm) -> String {
    normalize(match algorithm {
        HashAlgorithm::Sha1 => &record.sha1,
        HashAlgorithm::Md5 => &record.md5,
        HashAlgorithm::Crc32 => &record.crc32,
    })
}

fn base_hash(record: &HackHashRecord, algorithm: HashAlgorithm) -> Option<String> {
    record.details.base_rom.as_ref().map(|base| {
        normalize(match algorithm {
            HashAlgorithm::Sha1 => &base.sha1,
            HashAlgorithm::Md5 => &base.md5,
            HashAlgorithm::Crc32 => &base.crc32,
        })
    })
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn class_rank(class: HackHashEvidenceClass) -> u8 {
    match class {
        HackHashEvidenceClass::ExactPatchedOutput => 0,
        HackHashEvidenceClass::ProbableHackFamily => 1,
        HackHashEvidenceClass::BaseRomRelationship => 2,
        HackHashEvidenceClass::KnownPatchRelationship => 3,
        HackHashEvidenceClass::ConflictingExternalClaims => 4,
    }
}

fn algorithm_rank(algorithm: HashAlgorithm) -> u8 {
    match algorithm {
        HashAlgorithm::Sha1 => 0,
        HashAlgorithm::Md5 => 1,
        HashAlgorithm::Crc32 => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::hackhash::{HackHashDetails, HackHashExport};
    use crate::identity_source::managed_snapshot::{ManagedSourceReference, ManagedSourceTrust};
    use std::path::PathBuf;

    fn record(sha1: &str, md5: &str, crc32: &str, name: &str) -> HackHashRecord {
        HackHashRecord {
            machine_name: name.into(),
            description: name.into(),
            rom_name: name.into(),
            platform: "NES".into(),
            file_size: "1".into(),
            crc32: crc32.into(),
            md5: md5.into(),
            sha1: sha1.into(),
            details: HackHashDetails {
                hack_name: name.into(),
                version: "1.0".into(),
                approved_at: "now".into(),
                ..Default::default()
            },
        }
    }

    fn snapshot() -> ManagedSourceSnapshot {
        ManagedSourceSnapshot {
            provider_id: "hackhash".into(),
            source: ManagedSourceReference::LocalPath(PathBuf::from("import")),
            provider_version: None,
            retrieved_at_unix_seconds: 1,
            sha256: "a".repeat(64),
            size_bytes: 1,
            etag: None,
            last_modified: None,
            expected_media_type: "application/json".into(),
            attribution_url: None,
            parser_schema_version: "1".into(),
            trust: ManagedSourceTrust::UserProvided,
            validation_summary: "ok".into(),
            record_count: Some(1),
            warnings: Vec::new(),
        }
    }

    fn export(records: Vec<HackHashRecord>) -> HackHashExport {
        HackHashExport {
            header: None,
            machines: records,
            generated: None,
            count: None,
        }
    }

    #[test]
    fn strongest_hash_wins_and_md5_crc_fallbacks_work() {
        let one = record(&"a".repeat(40), &"b".repeat(32), "c", "One");
        let two = record(&"d".repeat(40), &"e".repeat(32), "f", "Two");
        let three = record(&"1".repeat(40), &"2".repeat(32), "12345678", "Three");
        let source = snapshot();
        assert_eq!(
            match_snapshot(
                &source,
                &export(vec![one.clone()]),
                &HackHashObservedHashes::new(Some(&"a".repeat(40)), None, None),
                HackHashSnapshotState::Active
            )
            .matches[0]
                .matched_algorithm,
            HashAlgorithm::Sha1
        );
        assert_eq!(
            match_snapshot(
                &source,
                &export(vec![two]),
                &HackHashObservedHashes::new(None, Some(&"e".repeat(32)), None),
                HackHashSnapshotState::Active
            )
            .matches[0]
                .matched_algorithm,
            HashAlgorithm::Md5
        );
        assert_eq!(
            match_snapshot(
                &source,
                &export(vec![three]),
                &HackHashObservedHashes::new(None, None, Some("12345678")),
                HackHashSnapshotState::Active
            )
            .matches[0]
                .matched_algorithm,
            HashAlgorithm::Crc32
        );
    }

    #[test]
    fn inactive_and_stale_snapshots_are_not_matches() {
        let source = snapshot();
        let result = match_snapshot(
            &source,
            &export(vec![record(&"a".repeat(40), "b", "c", "One")]),
            &HackHashObservedHashes::new(Some(&"a".repeat(40)), None, None),
            HackHashSnapshotState::Inactive,
        );
        assert!(result.matches.is_empty());
        assert!(result.evidence_claims("rom").is_empty());
    }

    #[test]
    fn duplicate_and_disagreeing_claims_are_exposed_without_election() {
        let source = snapshot();
        let result = match_snapshot(
            &source,
            &export(vec![
                record(&"a".repeat(40), "b", "c", "One"),
                record(&"a".repeat(40), "d", "e", "Two"),
            ]),
            &HackHashObservedHashes::new(Some(&"a".repeat(40)), None, None),
            HackHashSnapshotState::Active,
        );
        assert_eq!(result.matches.len(), 2);
        assert!(!result.conflicts.is_empty());
        assert!(
            result
                .evidence_claims("rom")
                .iter()
                .all(|claim| claim.source != EvidenceSource::NativeVerified)
        );
    }

    #[test]
    fn base_and_patch_relationships_are_weaker_than_output_matches() {
        let mut item = record(&"a".repeat(40), "b", "c", "One");
        item.details.base_rom = Some(crate::identity_source::hackhash::HackHashBaseRom {
            name: "Base".into(),
            platform: "NES".into(),
            file_extension: None,
            crc32: "11".into(),
            md5: "22".into(),
            sha1: "33".into(),
            status: "good".into(),
        });
        item.details.patch.sha1 = Some("44".into());
        let source = snapshot();
        let base = match_snapshot(
            &source,
            &export(vec![item.clone()]),
            &HackHashObservedHashes::new(None, None, Some("11")),
            HackHashSnapshotState::Active,
        );
        assert_eq!(
            base.matches[0].evidence_class,
            HackHashEvidenceClass::BaseRomRelationship
        );
        let patch = match_snapshot(
            &source,
            &export(vec![item]),
            &HackHashObservedHashes::new(Some("44"), None, None),
            HackHashSnapshotState::Active,
        );
        assert_eq!(
            patch.matches[0].evidence_class,
            HackHashEvidenceClass::KnownPatchRelationship
        );
    }

    #[test]
    fn absent_hash_is_not_a_match() {
        let source = snapshot();
        let result = match_snapshot(
            &source,
            &export(vec![record(&"a".repeat(40), "b", "c", "One")]),
            &HackHashObservedHashes::new(Some(&"f".repeat(40)), None, None),
            HackHashSnapshotState::Active,
        );
        assert!(result.matches.is_empty());
        assert!(result.conflicts.is_empty());
    }
}
