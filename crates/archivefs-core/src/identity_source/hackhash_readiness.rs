//! Offline readiness analysis for applying a HackHash patch.
//!
//! This module joins three independently inspected things: a local base-ROM
//! identity, a local patch inspection, and an active HackHash snapshot.  It
//! never applies a patch and never promotes the external claim to native
//! identity.

use serde::{Deserialize, Serialize};

use super::hackhash::{HackHashExport, HackHashRecord};
use super::hackhash_identity::{
    HackHashIdentityResult, HackHashObservedHashes, HackHashOutputHashes, HackHashSnapshotState,
};
use crate::standalone_patch::{PatchInspectionState, StandalonePatchFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HackHashBaseIdentityState {
    Verified,
    Identified,
    Missing,
    Conflicting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HackHashPatchExecutorAvailability {
    /// A bounded standalone applier exists, but it is not connected to the
    /// HackHash evidence and shared history/undo transaction yet.
    StandaloneOnly,
    /// A future exact-format transaction executor can permit direct apply.
    Transactional,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HackHashPatchReadinessStatus {
    ReadyToPatch,
    ReadyToStage,
    PossiblyReady,
    NotReady,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashPatchReadinessEvidence {
    pub status: HackHashPatchReadinessStatus,
    pub base_hash_match: bool,
    pub expected_base_identity: Option<String>,
    pub patch_hash_match: bool,
    pub patch_format: StandalonePatchFormat,
    pub expected_output_hashes: Vec<String>,
    pub expected_output: Option<HackHashOutputHashes>,
    pub hack_titles: Vec<String>,
    pub versions: Vec<String>,
    pub family_versions: Vec<String>,
    pub snapshot_sha256: Option<String>,
    pub provider_provenance: String,
    pub conflicts: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub refusal_reasons: Vec<String>,
    pub explanation: String,
}

#[derive(Debug, Clone)]
pub struct HackHashPatchReadinessRequest<'a> {
    pub base_identity_state: HackHashBaseIdentityState,
    pub base_hashes: HackHashObservedHashes,
    pub patch_hashes: HackHashObservedHashes,
    pub patch_format: StandalonePatchFormat,
    pub patch_inspection_state: PatchInspectionState,
    pub executor: HackHashPatchExecutorAvailability,
    pub snapshot_state: HackHashSnapshotState,
    pub snapshot_sha256: Option<&'a str>,
    pub provider_provenance: &'a str,
    pub export: &'a HackHashExport,
    pub identity: &'a HackHashIdentityResult,
}

impl HackHashPatchReadinessEvidence {
    pub fn is_applyable(&self) -> bool {
        self.status == HackHashPatchReadinessStatus::ReadyToPatch
    }
}

pub fn assess_hackhash_patch_readiness(
    request: &HackHashPatchReadinessRequest<'_>,
) -> HackHashPatchReadinessEvidence {
    let mut result = HackHashPatchReadinessEvidence {
        status: HackHashPatchReadinessStatus::NotReady,
        base_hash_match: false,
        expected_base_identity: None,
        patch_hash_match: false,
        patch_format: request.patch_format,
        expected_output_hashes: Vec::new(),
        expected_output: None,
        hack_titles: Vec::new(),
        versions: Vec::new(),
        family_versions: Vec::new(),
        snapshot_sha256: request.snapshot_sha256.map(str::to_owned),
        provider_provenance: request.provider_provenance.to_owned(),
        conflicts: request
            .identity
            .conflicts
            .iter()
            .map(|conflict| conflict.reason.clone())
            .collect(),
        missing_evidence: Vec::new(),
        refusal_reasons: Vec::new(),
        explanation: String::new(),
    };

    if request.snapshot_state != HackHashSnapshotState::Active {
        result.refusal_reasons.push(match request.snapshot_state {
            HackHashSnapshotState::Inactive => "HackHash snapshot is inactive".into(),
            HackHashSnapshotState::Stale => "HackHash snapshot is stale".into(),
            HackHashSnapshotState::Active => unreachable!(),
        });
        result.missing_evidence.push("active snapshot".into());
        return finish(
            result,
            "Readiness refused until an active HackHash snapshot is selected.",
        );
    }
    if request.base_identity_state == HackHashBaseIdentityState::Missing {
        result
            .missing_evidence
            .push("locally verified or identified base ROM".into());
    }
    if request.base_identity_state == HackHashBaseIdentityState::Conflicting {
        result
            .refusal_reasons
            .push("base ROM identity is conflicting".into());
    }
    if request.patch_inspection_state != PatchInspectionState::Valid {
        result
            .refusal_reasons
            .push("patch inspection did not produce a valid bounded patch".into());
    }
    if request.patch_format == StandalonePatchFormat::Unknown {
        result
            .refusal_reasons
            .push("patch format is unsupported".into());
    }
    if !result.missing_evidence.is_empty() || !result.refusal_reasons.is_empty() {
        return finish(
            result,
            "Readiness is blocked by missing or unsafe evidence.",
        );
    }

    let base_matches: Vec<&HackHashRecord> = request
        .export
        .machines
        .iter()
        .filter(|record| hash_matches_record(&request.base_hashes, record))
        .collect();
    result.base_hash_match = !base_matches.is_empty();
    if !result.base_hash_match {
        result
            .refusal_reasons
            .push("selected base hash does not match a HackHash base-ROM claim".into());
        return finish(
            result,
            "The selected base ROM is not an exact HackHash base match.",
        );
    }

    let patch_matches: Vec<&HackHashRecord> = base_matches
        .into_iter()
        .filter(|record| {
            record
                .details
                .patch
                .sha1
                .as_deref()
                .is_some_and(|sha1| request.patch_hashes.sha1.as_deref() == Some(sha1))
        })
        .collect();
    result.patch_hash_match = !patch_matches.is_empty();
    if !result.patch_hash_match {
        result
            .refusal_reasons
            .push("selected patch hash does not match the HackHash patch claim".into());
        return finish(
            result,
            "The selected patch is not an exact match for the selected base-ROM claim.",
        );
    }

    for record in &patch_matches {
        result
            .expected_output_hashes
            .push(format_output_hash(record));
        result.hack_titles.push(record.details.hack_name.clone());
        result.versions.push(record.details.version.clone());
        if let Some(family) = &record.details.hack_family {
            result
                .family_versions
                .push(format!("{} / {}", family.name, record.details.version));
        }
        if result.expected_base_identity.is_none() {
            result.expected_base_identity = Some(
                record
                    .details
                    .base_rom
                    .as_ref()
                    .map_or_else(|| record.description.clone(), |base| base.name.clone()),
            );
        }
    }
    if patch_matches.len() == 1 {
        let record = patch_matches[0];
        result.expected_output = Some(HackHashOutputHashes {
            sha1: record.sha1.clone(),
            md5: record.md5.clone(),
            crc32: record.crc32.clone(),
        });
    }
    result.expected_output_hashes.sort();
    result.expected_output_hashes.dedup();
    result.hack_titles.sort();
    result.hack_titles.dedup();
    result.versions.sort();
    result.versions.dedup();

    if result.expected_output_hashes.len() > 1 {
        result.status = HackHashPatchReadinessStatus::Ambiguous;
        result
            .conflicts
            .push("multiple expected patched-output hashes remain for this base and patch".into());
        return finish(
            result,
            "Readiness is ambiguous because HackHash publishes conflicting outputs.",
        );
    }
    if patch_matches.len() > 1 {
        result.status = HackHashPatchReadinessStatus::Ambiguous;
        result
            .conflicts
            .push("multiple HackHash records claim the same base and patch".into());
        return finish(
            result,
            "Readiness is ambiguous because multiple HackHash records remain.",
        );
    }
    if result.expected_output_hashes.is_empty() {
        result
            .missing_evidence
            .push("expected patched-output hash".into());
        return finish(
            result,
            "Readiness cannot proceed without an expected patched-output hash.",
        );
    }

    result.status = match request.executor {
        HackHashPatchExecutorAvailability::Transactional => {
            HackHashPatchReadinessStatus::ReadyToPatch
        }
        HackHashPatchExecutorAvailability::StandaloneOnly => {
            HackHashPatchReadinessStatus::ReadyToStage
        }
        HackHashPatchExecutorAvailability::Unavailable => {
            result
                .refusal_reasons
                .push("no safe executor is available for this exact patch format".into());
            HackHashPatchReadinessStatus::PossiblyReady
        }
    };
    let explanation = match result.status {
        HackHashPatchReadinessStatus::ReadyToPatch => {
            "Exact base, patch, and output evidence is present and the transactional executor is available."
        }
        HackHashPatchReadinessStatus::ReadyToStage => {
            "Exact base, patch, and output evidence is present; stage for a transactional apply because the standalone executor is not wired to HackHash history/undo."
        }
        HackHashPatchReadinessStatus::PossiblyReady => {
            "Evidence is exact, but the patch executor is unavailable for this format."
        }
        _ => unreachable!(),
    };
    finish(result, explanation)
}

fn finish(
    mut result: HackHashPatchReadinessEvidence,
    explanation: &str,
) -> HackHashPatchReadinessEvidence {
    result.explanation = explanation.into();
    result
}

fn hash_matches_record(hashes: &HackHashObservedHashes, record: &HackHashRecord) -> bool {
    hashes.sha1.as_deref()
        == Some(
            record
                .details
                .base_rom
                .as_ref()
                .map_or(record.sha1.as_str(), |base| base.sha1.as_str()),
        )
        || hashes.md5.as_deref()
            == Some(
                record
                    .details
                    .base_rom
                    .as_ref()
                    .map_or(record.md5.as_str(), |base| base.md5.as_str()),
            )
        || hashes.crc32.as_deref()
            == Some(
                record
                    .details
                    .base_rom
                    .as_ref()
                    .map_or(record.crc32.as_str(), |base| base.crc32.as_str()),
            )
}

fn format_output_hash(record: &HackHashRecord) -> String {
    format!(
        "SHA-1 {} / MD5 {} / CRC32 {}",
        record.sha1, record.md5, record.crc32
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::hackhash::{HackHashDetails, HackHashPatch};

    fn record() -> HackHashRecord {
        HackHashRecord {
            machine_name: "base".into(),
            description: "Hack".into(),
            rom_name: "hack.rom".into(),
            platform: "SNES".into(),
            file_size: "1".into(),
            crc32: "11111111".into(),
            md5: "11111111111111111111111111111111".into(),
            sha1: "2222222222222222222222222222222222222222".into(),
            details: HackHashDetails {
                hack_name: "Hack".into(),
                version: "1.0".into(),
                patch: HackHashPatch {
                    patch_type: Some("IPS".into()),
                    filename: Some("hack.ips".into()),
                    sha1: Some("3333333333333333333333333333333333333333".into()),
                },
                base_rom: Some(crate::identity_source::hackhash::HackHashBaseRom {
                    name: "Base ROM".into(),
                    platform: "SNES".into(),
                    file_extension: None,
                    crc32: "aaaaaaaa".into(),
                    md5: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                    sha1: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                    status: "verified".into(),
                }),
                approved_at: "now".into(),
                ..Default::default()
            },
        }
    }

    fn request<'a>(
        export: &'a HackHashExport,
        identity: &'a HackHashIdentityResult,
    ) -> HackHashPatchReadinessRequest<'a> {
        HackHashPatchReadinessRequest {
            base_identity_state: HackHashBaseIdentityState::Verified,
            base_hashes: HackHashObservedHashes::new(
                Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
                None,
                None,
            ),
            patch_hashes: HackHashObservedHashes::new(
                Some("3333333333333333333333333333333333333333"),
                None,
                None,
            ),
            patch_format: StandalonePatchFormat::Ips,
            patch_inspection_state: PatchInspectionState::Valid,
            executor: HackHashPatchExecutorAvailability::StandaloneOnly,
            snapshot_state: HackHashSnapshotState::Active,
            snapshot_sha256: Some("snapshot"),
            provider_provenance: "local immutable snapshot",
            export,
            identity,
        }
    }

    fn identity() -> HackHashIdentityResult {
        HackHashIdentityResult {
            snapshot_state: HackHashSnapshotState::Active,
            snapshot_sha256: "snapshot".into(),
            matches: vec![],
            conflicts: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn exact_evidence_is_ready_to_stage_without_transactional_executor() {
        let export = HackHashExport {
            header: None,
            machines: vec![record()],
            generated: None,
            count: Some(1),
        };
        let identity = identity();
        let result = assess_hackhash_patch_readiness(&request(&export, &identity));
        assert_eq!(result.status, HackHashPatchReadinessStatus::ReadyToStage);
        assert!(result.base_hash_match && result.patch_hash_match);
        assert!(!result.is_applyable());
    }

    #[test]
    fn wrong_base_and_patch_fail_closed() {
        let export = HackHashExport {
            header: None,
            machines: vec![record()],
            generated: None,
            count: Some(1),
        };
        let identity = identity();
        let mut request = request(&export, &identity);
        request.base_hashes = HackHashObservedHashes::new(
            Some("cccccccccccccccccccccccccccccccccccccccc"),
            None,
            None,
        );
        let result = assess_hackhash_patch_readiness(&request);
        assert_eq!(result.status, HackHashPatchReadinessStatus::NotReady);
        assert!(!result.base_hash_match);
    }

    #[test]
    fn inactive_snapshot_is_not_ready() {
        let export = HackHashExport {
            header: None,
            machines: vec![record()],
            generated: None,
            count: Some(1),
        };
        let identity = identity();
        let mut request = request(&export, &identity);
        request.snapshot_state = HackHashSnapshotState::Inactive;
        let result = assess_hackhash_patch_readiness(&request);
        assert_eq!(result.status, HackHashPatchReadinessStatus::NotReady);
        assert!(result.explanation.contains("active HackHash"));
    }

    #[test]
    fn wrong_patch_and_unsupported_format_fail_closed() {
        let export = HackHashExport {
            header: None,
            machines: vec![record()],
            generated: None,
            count: Some(1),
        };
        let identity = identity();
        let mut request = request(&export, &identity);
        request.patch_hashes = HackHashObservedHashes::new(
            Some("dddddddddddddddddddddddddddddddddddddddd"),
            None,
            None,
        );
        let result = assess_hackhash_patch_readiness(&request);
        assert_eq!(result.status, HackHashPatchReadinessStatus::NotReady);
        assert!(!result.patch_hash_match);

        request.patch_inspection_state = PatchInspectionState::Unsupported;
        let result = assess_hackhash_patch_readiness(&request);
        assert!(
            result
                .refusal_reasons
                .iter()
                .any(|reason| reason.contains("valid bounded patch"))
        );
    }

    #[test]
    fn conflicting_outputs_are_ambiguous() {
        let mut second = record();
        second.sha1 = "4444444444444444444444444444444444444444".into();
        let export = HackHashExport {
            header: None,
            machines: vec![record(), second],
            generated: None,
            count: Some(2),
        };
        let identity = identity();
        let result = assess_hackhash_patch_readiness(&request(&export, &identity));
        assert_eq!(result.status, HackHashPatchReadinessStatus::Ambiguous);
        assert!(result.explanation.contains("conflicting outputs"));
    }

    #[test]
    fn unavailable_executor_is_not_reported_as_applyable() {
        let export = HackHashExport {
            header: None,
            machines: vec![record()],
            generated: None,
            count: Some(1),
        };
        let identity = identity();
        let mut request = request(&export, &identity);
        request.executor = HackHashPatchExecutorAvailability::Unavailable;
        let result = assess_hackhash_patch_readiness(&request);
        assert_eq!(result.status, HackHashPatchReadinessStatus::PossiblyReady);
        assert!(!result.is_applyable());
    }
}
