//! Converts an [`ImportedMameListxmlSource`] hash lookup into
//! [`EvidenceObservation`]s.
//!
//! Every observation stays [`SourceFamily::MAMEArcade`] - an independent
//! evidence lane - never [`SourceFamily::MAMERedump`]. Turning a MAME disk's
//! CHD identity into `MAMERedump`-derived evidence is a distinct, separately
//! reviewed job:
//! [`crate::platform_evidence_fusion::mame_redump_bridge::observations_from_mame_redump_chd_identity`].

use super::import::ImportedMameListxmlSource;
use crate::dat::index::DatRomRef;
use crate::dat::model::ChecksumAlgorithm;
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope, LineageRelation,
    Provenance, Representation, SourceArtifactIdentity, SourceFamily,
};

fn provenance(source: &ImportedMameListxmlSource, representation: Representation) -> Provenance {
    Provenance {
        channel: EvidenceChannel::LocalMame,
        upstream_source: SourceFamily::MAMEArcade,
        upstream_version: None,
        source_artifact: Some(SourceArtifactIdentity {
            source_family: SourceFamily::MAMEArcade,
            upstream_version: None,
            artifact_sha256: Some(source.artifact_sha256.clone()),
            artifact_name: Some(source.artifact_name.clone()),
        }),
        imported_at_unix: None,
        retrieved_at_unix: None,
        generator_version: None,
        lineage: LineageRelation::Independent,
        representation,
    }
}

fn has_status(rom: &DatRomRef, status: &str) -> bool {
    rom.status
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case(status))
}

/// A CRC32-only match is corroboration, never a standalone strong claim (the
/// algorithm is too collision-prone); a `baddump` status likewise never
/// counts as a clean strong match regardless of algorithm.
fn claim_strength(rom: &DatRomRef, algorithm: ChecksumAlgorithm) -> ClaimStrength {
    if has_status(rom, "baddump") || matches!(algorithm, ChecksumAlgorithm::Crc32) {
        ClaimStrength::Corroborated
    } else {
        ClaimStrength::Strong
    }
}

/// Matches `hash` against the imported dump's ROM index and returns one
/// observation per matching MAME machine ROM. A `nodump` declaration carries
/// no real bytes to match against and is always excluded, even if the DAT
/// contradictorily also published a hash alongside it.
pub fn observations_from_mame_listxml_matches(
    source: &ImportedMameListxmlSource,
    algorithm: ChecksumAlgorithm,
    hash: &str,
) -> Vec<EvidenceObservation> {
    let rows = match algorithm {
        ChecksumAlgorithm::Crc32 => source.index.lookup_crc32(hash),
        ChecksumAlgorithm::Md5 => source.index.lookup_md5(hash),
        ChecksumAlgorithm::Sha1 => source.index.lookup_sha1(hash),
        ChecksumAlgorithm::Sha256 => source.index.lookup_sha256(hash),
    };
    rows.iter()
        .filter(|rom| !has_status(rom, "nodump"))
        .map(|rom| EvidenceObservation {
            provenance: provenance(source, Representation::PhysicalFile),
            claim: ClaimType::ExactBytesMatch,
            claim_strength: claim_strength(rom, algorithm),
            identity_scope: IdentityScope::DumpIdentity,
            hash_or_value: Some(hash.to_string()),
            platform_candidate: None,
            release_candidate: Some(rom.game_name.clone()),
            notes: Some(format!(
                "MAME listxml machine match in {}",
                source.artifact_name
            )),
        })
        .collect()
}

/// Matches `sha1` against the imported dump's CHD disk index and returns one
/// [`Representation::LogicalChd`] observation per matching machine disk.
pub fn observations_from_mame_listxml_disk_matches(
    source: &ImportedMameListxmlSource,
    sha1: &str,
) -> Vec<EvidenceObservation> {
    source
        .disk_index
        .lookup_disk_sha1(sha1)
        .iter()
        .map(|disk| EvidenceObservation {
            provenance: provenance(source, Representation::LogicalChd),
            claim: ClaimType::ExactLogicalDiscMatch,
            claim_strength: ClaimStrength::Strong,
            identity_scope: IdentityScope::DumpIdentity,
            hash_or_value: Some(sha1.to_string()),
            platform_candidate: None,
            release_candidate: Some(disk.game_name.clone()),
            notes: Some(format!(
                "MAME listxml logical CHD match in {}",
                source.artifact_name
            )),
        })
        .collect()
}
