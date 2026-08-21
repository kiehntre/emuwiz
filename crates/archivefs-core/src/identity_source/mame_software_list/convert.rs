//! Turns MAME software-list index hits into lineage-aware observations.

use crate::dat::index::{DatDiskRef, DatIndex, DatRomRef};
use crate::dat::model::ChecksumAlgorithm;
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope, LineageRelation,
    Provenance, Representation, SourceArtifactIdentity, SourceFamily,
};

use super::import::ImportedMameSoftwareListSource;

fn source_artifact(source: &ImportedMameSoftwareListSource) -> SourceArtifactIdentity {
    SourceArtifactIdentity {
        source_family: SourceFamily::MAMESoftwareList,
        upstream_version: source.upstream_version.clone(),
        artifact_sha256: Some(source.artifact_sha256.clone()),
        artifact_name: Some(source.artifact_name.clone()),
    }
}

fn provenance(
    source: &ImportedMameSoftwareListSource,
    representation: Representation,
) -> Provenance {
    Provenance {
        channel: EvidenceChannel::LocalMame,
        upstream_source: SourceFamily::MAMESoftwareList,
        upstream_version: source.upstream_version.clone(),
        source_artifact: Some(source_artifact(source)),
        imported_at_unix: None,
        retrieved_at_unix: None,
        generator_version: None,
        lineage: LineageRelation::Independent,
        representation,
    }
}

fn status_is(rom: &DatRomRef, expected: &str) -> bool {
    rom.status
        .as_deref()
        .is_some_and(|status| status.eq_ignore_ascii_case(expected))
}

fn strength_for(rom: &DatRomRef, algorithm: ChecksumAlgorithm) -> ClaimStrength {
    if status_is(rom, "baddump") {
        return ClaimStrength::Corroborated;
    }
    match algorithm {
        ChecksumAlgorithm::Sha1 | ChecksumAlgorithm::Md5 | ChecksumAlgorithm::Sha256 => {
            ClaimStrength::Strong
        }
        // A CRC32 remains corroboration even when the same row also carries a
        // stronger checksum. A caller that has that stronger digest should use
        // its matching lane rather than promote CRC globally.
        ChecksumAlgorithm::Crc32 => ClaimStrength::Corroborated,
    }
}

/// Collision-preserving lookup of ROM members. Entries are kept in the index
/// even when `nodump` so callers can audit the source, but conversion below
/// refuses to turn a nodump row into identity evidence.
pub fn lookup_mame_software_list<'a>(
    index: &'a DatIndex,
    algorithm: ChecksumAlgorithm,
    hash_value: &str,
) -> &'a [DatRomRef] {
    let table = match algorithm {
        ChecksumAlgorithm::Sha1 => &index.by_sha1,
        ChecksumAlgorithm::Md5 => &index.by_md5,
        ChecksumAlgorithm::Crc32 => &index.by_crc32,
        ChecksumAlgorithm::Sha256 => &index.by_sha256,
    };
    table.get(hash_value).map(Vec::as_slice).unwrap_or(&[])
}

/// Produces one exact byte observation and separate display metadata for each
/// matching non-nodump member. Software entry names remain release metadata;
/// no MAME shortname or list name is emitted as a canonical platform value.
pub fn observations_from_mame_software_list_matches(
    source: &ImportedMameSoftwareListSource,
    algorithm: ChecksumAlgorithm,
    hash_value: &str,
) -> Vec<EvidenceObservation> {
    lookup_mame_software_list(&source.index, algorithm, hash_value)
        .iter()
        .filter(|rom| !status_is(rom, "nodump"))
        .flat_map(|rom| {
            [
                EvidenceObservation {
                    provenance: provenance(source, Representation::SoftwareListMember),
                    claim: ClaimType::ExactBytesMatch,
                    claim_strength: strength_for(rom, algorithm),
                    identity_scope: IdentityScope::DumpIdentity,
                    hash_or_value: Some(hash_value.to_string()),
                    platform_candidate: None,
                    release_candidate: Some(rom.game_name.clone()),
                    notes: Some(format!(
                        "MAME software-list member match in {} (list {})",
                        source.artifact_name,
                        source.software_list_name.as_deref().unwrap_or("unknown")
                    )),
                },
                EvidenceObservation {
                    provenance: Provenance {
                        lineage: LineageRelation::MetadataOnly,
                        representation: Representation::Unknown,
                        ..provenance(source, Representation::Unknown)
                    },
                    claim: ClaimType::DisplayMetadata,
                    claim_strength: ClaimStrength::DisplayOnly,
                    identity_scope: IdentityScope::ReleaseIdentity,
                    hash_or_value: None,
                    platform_candidate: None,
                    release_candidate: Some(rom.game_name.clone()),
                    notes: None,
                },
            ]
        })
        .collect()
}

/// Separate MAME `<disk sha1>` index lane. This is MAME's documented logical
/// CHD combined SHA-1, never a physical CHD-file SHA-256, raw SHA-1, or a
/// Redump track SHA-1.
pub fn lookup_mame_software_list_disk_sha1<'a>(
    source: &'a ImportedMameSoftwareListSource,
    sha1: &str,
) -> &'a [DatDiskRef] {
    source.disk_index.lookup_disk_sha1(sha1)
}

pub fn observations_from_mame_software_list_disk_matches(
    source: &ImportedMameSoftwareListSource,
    sha1: &str,
) -> Vec<EvidenceObservation> {
    lookup_mame_software_list_disk_sha1(source, sha1)
        .iter()
        .flat_map(|disk| {
            [
                EvidenceObservation {
                    provenance: provenance(source, Representation::LogicalChd),
                    claim: ClaimType::ExactLogicalDiscMatch,
                    claim_strength: ClaimStrength::Strong,
                    identity_scope: IdentityScope::DumpIdentity,
                    hash_or_value: Some(sha1.to_string()),
                    platform_candidate: None,
                    release_candidate: Some(disk.game_name.clone()),
                    notes: Some(format!(
                        "MAME software-list logical CHD match in {}",
                        source.artifact_name
                    )),
                },
                EvidenceObservation {
                    provenance: Provenance {
                        lineage: LineageRelation::MetadataOnly,
                        representation: Representation::Unknown,
                        ..provenance(source, Representation::Unknown)
                    },
                    claim: ClaimType::DisplayMetadata,
                    claim_strength: ClaimStrength::DisplayOnly,
                    identity_scope: IdentityScope::ReleaseIdentity,
                    hash_or_value: None,
                    platform_candidate: None,
                    release_candidate: Some(disk.game_name.clone()),
                    notes: None,
                },
            ]
        })
        .collect()
}
