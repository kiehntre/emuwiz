//! Turns direct Redump index hits into lineage-aware observations.

use crate::dat::index::{DatDiskRef, DatIndex, DatRomRef};
use crate::dat::model::ChecksumAlgorithm;
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope, LineageRelation,
    Provenance, Representation, SourceArtifactIdentity, SourceFamily,
};

use super::import::ImportedRedumpSource;

pub fn claim_for_representation(representation: Representation) -> ClaimType {
    match representation {
        Representation::DiscTrack => ClaimType::ExactTrackMatch,
        Representation::LogicalChd => ClaimType::ExactLogicalDiscMatch,
        _ => ClaimType::PlatformCandidate,
    }
}

fn source_artifact(source: &ImportedRedumpSource) -> SourceArtifactIdentity {
    SourceArtifactIdentity {
        source_family: SourceFamily::Redump,
        upstream_version: source.upstream_version.clone(),
        artifact_sha256: Some(source.artifact_sha256.clone()),
        artifact_name: Some(source.artifact_name.clone()),
    }
}

fn provenance(source: &ImportedRedumpSource, representation: Representation) -> Provenance {
    Provenance {
        channel: EvidenceChannel::LocalRedump,
        upstream_source: SourceFamily::Redump,
        upstream_version: source.upstream_version.clone(),
        source_artifact: Some(source_artifact(source)),
        imported_at_unix: None,
        retrieved_at_unix: None,
        generator_version: None,
        lineage: LineageRelation::Independent,
        representation,
    }
}

fn track_strength(rom: &DatRomRef, algorithm: ChecksumAlgorithm) -> ClaimStrength {
    match algorithm {
        ChecksumAlgorithm::Sha1 | ChecksumAlgorithm::Md5 | ChecksumAlgorithm::Sha256 => {
            ClaimStrength::Strong
        }
        ChecksumAlgorithm::Crc32
            if rom.checksums.iter().any(|checksum| {
                matches!(
                    checksum.algorithm,
                    ChecksumAlgorithm::Sha1 | ChecksumAlgorithm::Md5 | ChecksumAlgorithm::Sha256
                )
            }) =>
        {
            ClaimStrength::Strong
        }
        ChecksumAlgorithm::Crc32 => ClaimStrength::Corroborated,
    }
}

/// A CUE sheet records disc layout and references; it is never one of the
/// audio/data tracks it describes.  Extension is used only to classify the
/// already-DAT-matched member's representation, never as identity evidence.
fn is_cue_sheet(rom: &DatRomRef) -> bool {
    rom.rom_name
        .rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("cue"))
}

fn cue_layout_observation(
    source: &ImportedRedumpSource,
    rom: &DatRomRef,
    hash_value: &str,
) -> EvidenceObservation {
    EvidenceObservation {
        provenance: provenance(source, Representation::StructuralMetadata),
        // There is no existing exact-layout claim type.  Retain the matched
        // hash as source-attributed layout metadata instead of pretending the
        // control file is an audio/data track.
        claim: ClaimType::DisplayMetadata,
        claim_strength: ClaimStrength::Corroborated,
        identity_scope: IdentityScope::DumpIdentity,
        hash_or_value: Some(hash_value.to_string()),
        platform_candidate: source.system_name.clone(),
        release_candidate: Some(rom.game_name.clone()),
        notes: Some(format!(
            "Redump CUE layout/control-file match in {}",
            source.artifact_name
        )),
    }
}

/// Collision-preserving ordinary track lookup.  It deliberately does not
/// include DAT `<disk>` identities: those have their own logical-CHD lane.
pub fn lookup_redump<'a>(
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

/// The separate DAT `<disk sha1>` lookup lane for a CHD's combined/logical
/// SHA-1.  A track SHA-1 must never be queried here or vice versa.
pub fn lookup_redump_disk_sha1<'a>(
    source: &'a ImportedRedumpSource,
    sha1: &str,
) -> &'a [DatDiskRef] {
    source.disk_index.lookup_disk_sha1(sha1)
}

/// Converts Redump member rows.  Actual audio/data tracks are fixed to
/// `DiscTrack`; a `.cue` control-file row is instead structural/layout
/// metadata and can never become an exact-track claim.
pub fn observations_from_redump_matches(
    source: &ImportedRedumpSource,
    algorithm: ChecksumAlgorithm,
    hash_value: &str,
) -> Vec<EvidenceObservation> {
    lookup_redump(&source.index, algorithm, hash_value)
        .iter()
        .flat_map(|rom| {
            if is_cue_sheet(rom) {
                return vec![cue_layout_observation(source, rom, hash_value)];
            }
            vec![
                EvidenceObservation {
                    provenance: provenance(source, Representation::DiscTrack),
                    claim: ClaimType::ExactTrackMatch,
                    claim_strength: track_strength(rom, algorithm),
                    identity_scope: IdentityScope::DumpIdentity,
                    hash_or_value: Some(hash_value.to_string()),
                    platform_candidate: source.system_name.clone(),
                    release_candidate: Some(rom.game_name.clone()),
                    notes: Some(format!("Redump track match in {}", source.artifact_name)),
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

/// Converts only a DAT `<disk sha1>` hit.  The SHA-1 is a CHD logical
/// identity and therefore emits `LogicalChd`/`ExactLogicalDiscMatch`, never
/// an ordinary ROM or track match.
pub fn observations_from_redump_disk_matches(
    source: &ImportedRedumpSource,
    sha1: &str,
) -> Vec<EvidenceObservation> {
    lookup_redump_disk_sha1(source, sha1)
        .iter()
        .flat_map(|disk| {
            [
                EvidenceObservation {
                    provenance: provenance(source, Representation::LogicalChd),
                    claim: ClaimType::ExactLogicalDiscMatch,
                    claim_strength: ClaimStrength::Strong,
                    identity_scope: IdentityScope::DumpIdentity,
                    hash_or_value: Some(sha1.to_string()),
                    platform_candidate: source.system_name.clone(),
                    release_candidate: Some(disk.game_name.clone()),
                    notes: Some(format!(
                        "Redump logical CHD match in {}",
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
