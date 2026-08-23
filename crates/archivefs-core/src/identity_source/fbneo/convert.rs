//! FBNeo index hits as lineage-aware evidence.

use crate::dat::index::{DatDiskRef, DatIndex, DatRomRef};
use crate::dat::model::ChecksumAlgorithm;
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope, LineageRelation,
    Provenance, Representation, SourceArtifactIdentity, SourceFamily,
};

use super::import::ImportedFBNeoSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DumpStatus {
    Normal,
    BadDump,
    NoDump,
    Unknown,
}

fn dump_status(status: Option<&str>) -> DumpStatus {
    match status.map(str::trim).filter(|value| !value.is_empty()) {
        None => DumpStatus::Normal,
        Some(value) if value.eq_ignore_ascii_case("good") => DumpStatus::Normal,
        Some(value)
            if value.eq_ignore_ascii_case("baddump") || value.eq_ignore_ascii_case("bad") =>
        {
            DumpStatus::BadDump
        }
        Some(value) if value.eq_ignore_ascii_case("nodump") => DumpStatus::NoDump,
        Some(_) => DumpStatus::Unknown,
    }
}

fn source_artifact(source: &ImportedFBNeoSource) -> SourceArtifactIdentity {
    SourceArtifactIdentity {
        source_family: SourceFamily::FBNeo,
        upstream_version: source.upstream_version.clone(),
        artifact_sha256: Some(source.artifact_sha256.clone()),
        artifact_name: Some(source.artifact_name.clone()),
    }
}

fn provenance(source: &ImportedFBNeoSource, representation: Representation) -> Provenance {
    Provenance {
        channel: EvidenceChannel::LocalFBNeo,
        upstream_source: SourceFamily::FBNeo,
        upstream_version: source.upstream_version.clone(),
        source_artifact: Some(source_artifact(source)),
        imported_at_unix: None,
        retrieved_at_unix: None,
        generator_version: None,
        lineage: LineageRelation::Independent,
        representation,
    }
}

fn rom_strength(rom: &DatRomRef, algorithm: ChecksumAlgorithm) -> Option<ClaimStrength> {
    match dump_status(rom.status.as_deref()) {
        DumpStatus::NoDump | DumpStatus::Unknown => None,
        DumpStatus::BadDump => Some(ClaimStrength::Corroborated),
        DumpStatus::Normal => Some(match algorithm {
            ChecksumAlgorithm::Sha1 | ChecksumAlgorithm::Md5 | ChecksumAlgorithm::Sha256 => {
                ClaimStrength::Strong
            }
            ChecksumAlgorithm::Crc32 => ClaimStrength::Corroborated,
        }),
    }
}

fn disk_strength(disk: &DatDiskRef) -> Option<ClaimStrength> {
    match dump_status(disk.status.as_deref()) {
        DumpStatus::Normal => Some(ClaimStrength::Strong),
        DumpStatus::BadDump => Some(ClaimStrength::Corroborated),
        DumpStatus::NoDump | DumpStatus::Unknown => None,
    }
}

/// Collision-preserving ROM lookup. Names remain display metadata; no lookup
/// path uses a filename or an FBNeo machine shortname.
pub fn lookup_fbneo<'a>(
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

pub fn observations_from_fbneo_matches(
    source: &ImportedFBNeoSource,
    algorithm: ChecksumAlgorithm,
    hash_value: &str,
) -> Vec<EvidenceObservation> {
    lookup_fbneo(&source.index, algorithm, hash_value)
        .iter()
        .filter_map(|rom| {
            let claim_strength = rom_strength(rom, algorithm)?;
            Some(EvidenceObservation {
                provenance: provenance(source, Representation::PhysicalFile),
                claim: ClaimType::ExactBytesMatch,
                claim_strength,
                identity_scope: IdentityScope::DumpIdentity,
                hash_or_value: Some(hash_value.to_string()),
                platform_candidate: None,
                release_candidate: Some(rom.game_name.clone()),
                notes: Some(format!("FBNeo exact match in {}", source.artifact_name)),
            })
        })
        .collect()
}

/// Separate logical-CHD combined-SHA1 lookup lane. It cannot accept a ROM
/// SHA-1, CHD raw SHA-1, parent SHA-1, or physical-file hash.
pub fn lookup_fbneo_disk_sha1<'a>(source: &'a ImportedFBNeoSource, sha1: &str) -> &'a [DatDiskRef] {
    source.disk_index.lookup_disk_sha1(sha1)
}

pub fn observations_from_fbneo_disk_matches(
    source: &ImportedFBNeoSource,
    sha1: &str,
) -> Vec<EvidenceObservation> {
    lookup_fbneo_disk_sha1(source, sha1)
        .iter()
        .filter_map(|disk| {
            let claim_strength = disk_strength(disk)?;
            Some(EvidenceObservation {
                provenance: provenance(source, Representation::LogicalChd),
                claim: ClaimType::ExactLogicalDiscMatch,
                claim_strength,
                identity_scope: IdentityScope::DumpIdentity,
                hash_or_value: Some(sha1.to_string()),
                platform_candidate: None,
                release_candidate: Some(disk.game_name.clone()),
                notes: Some(format!(
                    "FBNeo logical CHD match in {}",
                    source.artifact_name
                )),
            })
        })
        .collect()
}
