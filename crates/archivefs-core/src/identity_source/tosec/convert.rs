//! Converts a reused [`DatIndex`] lookup into lineage-aware TOSEC evidence.

use crate::dat::index::{DatIndex, DatRomRef};
use crate::dat::model::ChecksumAlgorithm;
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope, LineageRelation,
    Provenance, Representation, SourceArtifactIdentity, SourceFamily,
};

use super::filename_metadata::{
    TosecDumpFlags, TosecFilenameMetadata, parse_tosec_filename_metadata,
};
use super::import::ImportedTosecSource;

/// The existing exact claim that truthfully corresponds to a caller-supplied
/// representation. The importer never guesses representation from a TOSEC
/// filename or DAT member name.
pub fn claim_for_representation(representation: Representation) -> ClaimType {
    match representation {
        Representation::PhysicalFile => ClaimType::ExactBytesMatch,
        Representation::NormalizedRom => ClaimType::ExactNormalizedMatch,
        Representation::DiscTrack => ClaimType::ExactTrackMatch,
        Representation::LogicalChd => ClaimType::ExactLogicalDiscMatch,
        Representation::WHDLoadSlave => ClaimType::ExactSlaveMatch,
        _ => ClaimType::PlatformCandidate,
    }
}

fn source_artifact(source: &ImportedTosecSource) -> SourceArtifactIdentity {
    SourceArtifactIdentity {
        source_family: SourceFamily::TOSEC,
        upstream_version: source.upstream_version.clone(),
        artifact_sha256: Some(source.artifact_sha256.clone()),
        artifact_name: Some(source.artifact_name.clone()),
    }
}

fn provenance(
    source: &ImportedTosecSource,
    lineage: LineageRelation,
    representation: Representation,
) -> Provenance {
    Provenance {
        channel: EvidenceChannel::LocalTosec,
        upstream_source: SourceFamily::TOSEC,
        upstream_version: source.upstream_version.clone(),
        source_artifact: Some(source_artifact(source)),
        imported_at_unix: None,
        retrieved_at_unix: None,
        generator_version: None,
        lineage,
        representation,
    }
}

fn strength_for(rom: &DatRomRef, matched_algorithm: ChecksumAlgorithm) -> ClaimStrength {
    match matched_algorithm {
        ChecksumAlgorithm::Sha1 | ChecksumAlgorithm::Sha256 | ChecksumAlgorithm::Md5 => {
            ClaimStrength::Strong
        }
        ChecksumAlgorithm::Crc32 => {
            if rom.checksums.iter().any(|checksum| {
                matches!(
                    checksum.algorithm,
                    ChecksumAlgorithm::Sha1 | ChecksumAlgorithm::Sha256 | ChecksumAlgorithm::Md5
                )
            }) {
                ClaimStrength::Strong
            } else {
                ClaimStrength::Corroborated
            }
        }
    }
}

fn matched_title(rom: &DatRomRef) -> TosecFilenameMetadata {
    let mut metadata = parse_tosec_filename_metadata(&rom.game_name);
    if metadata.title.is_empty() {
        metadata.title = rom.game_name.clone();
    }
    metadata
}

fn exact_observation(
    source: &ImportedTosecSource,
    rom: &DatRomRef,
    representation: Representation,
    matched_algorithm: ChecksumAlgorithm,
    hash_value: &str,
    metadata: &TosecFilenameMetadata,
) -> EvidenceObservation {
    EvidenceObservation {
        provenance: provenance(source, LineageRelation::Independent, representation),
        claim: claim_for_representation(representation),
        claim_strength: strength_for(rom, matched_algorithm),
        identity_scope: IdentityScope::DumpIdentity,
        hash_or_value: Some(hash_value.to_string()),
        platform_candidate: Some(source.system_name.clone()),
        // The unannotated title is a useful game-level label, while the hash
        // remains the dump identity. Dump annotations are emitted separately
        // below so clean/cracked/trainer variants stay distinguishable.
        release_candidate: Some(metadata.title.clone()),
        notes: Some(format!(
            "TOSEC exact match in {} ({}) for DAT entry {}",
            source.system_name, source.artifact_name, rom.game_name
        )),
    }
}

fn platform_observation(
    source: &ImportedTosecSource,
    representation: Representation,
) -> EvidenceObservation {
    EvidenceObservation {
        provenance: provenance(source, LineageRelation::Independent, representation),
        claim: ClaimType::PlatformCandidate,
        claim_strength: ClaimStrength::Corroborated,
        identity_scope: IdentityScope::PlatformIdentity,
        hash_or_value: None,
        platform_candidate: Some(source.system_name.clone()),
        release_candidate: None,
        notes: Some("TOSEC DAT header platform metadata after exact hash match".to_string()),
    }
}

fn metadata_observation(
    source: &ImportedTosecSource,
    claim: ClaimType,
    claim_strength: ClaimStrength,
    identity_scope: IdentityScope,
    value: String,
    notes: &str,
) -> EvidenceObservation {
    EvidenceObservation {
        provenance: provenance(
            source,
            LineageRelation::MetadataOnly,
            Representation::StructuralMetadata,
        ),
        claim,
        claim_strength,
        identity_scope,
        hash_or_value: Some(value),
        platform_candidate: None,
        release_candidate: None,
        notes: Some(format!("TOSEC name metadata after DAT hash match: {notes}")),
    }
}

fn metadata_observations(
    source: &ImportedTosecSource,
    metadata: &TosecFilenameMetadata,
) -> Vec<EvidenceObservation> {
    let mut out = vec![metadata_observation(
        source,
        ClaimType::ReleaseCandidate,
        ClaimStrength::DisplayOnly,
        IdentityScope::GameIdentity,
        metadata.title.clone(),
        "game title",
    )];

    if let Some(year) = &metadata.year {
        out.push(metadata_observation(
            source,
            ClaimType::DisplayMetadata,
            ClaimStrength::DisplayOnly,
            IdentityScope::ReleaseIdentity,
            year.clone(),
            "year",
        ));
    }
    if let Some(publisher) = &metadata.publisher {
        out.push(metadata_observation(
            source,
            ClaimType::DisplayMetadata,
            ClaimStrength::DisplayOnly,
            IdentityScope::ReleaseIdentity,
            publisher.clone(),
            "publisher",
        ));
    }
    for country in &metadata.countries {
        out.push(metadata_observation(
            source,
            ClaimType::RegionMetadata,
            ClaimStrength::DisplayOnly,
            IdentityScope::ReleaseIdentity,
            country.clone(),
            "TOSEC country token",
        ));
    }
    for language in &metadata.languages {
        out.push(metadata_observation(
            source,
            ClaimType::LanguageMetadata,
            ClaimStrength::DisplayOnly,
            IdentityScope::ReleaseIdentity,
            language.clone(),
            "TOSEC language token",
        ));
    }
    for revision in [&metadata.version, &metadata.revision]
        .into_iter()
        .flatten()
    {
        out.push(metadata_observation(
            source,
            ClaimType::RevisionCandidate,
            ClaimStrength::DisplayOnly,
            IdentityScope::ReleaseIdentity,
            revision.clone(),
            "TOSEC version/revision token",
        ));
    }
    append_variant_observations(source, metadata.flags, &mut out);
    out
}

fn append_variant_observations(
    source: &ImportedTosecSource,
    flags: TosecDumpFlags,
    out: &mut Vec<EvidenceObservation>,
) {
    for label in flags.labels() {
        let strength = if matches!(label, "bad dump" | "overdump" | "underdump" | "virus") {
            // The hash match still proves which published dump this is; this
            // separate weak status records TOSEC's quality warning without
            // promoting or replacing the hash/source claim.
            ClaimStrength::Weak
        } else if label == "verified good" {
            // A TOSEC [!] is useful corpus metadata, but remains separate
            // from exact hash strength and source lineage.
            ClaimStrength::Corroborated
        } else {
            ClaimStrength::DisplayOnly
        };
        out.push(metadata_observation(
            source,
            ClaimType::VariantStatus,
            strength,
            IdentityScope::DumpIdentity,
            label.to_string(),
            "TOSEC dump-status token",
        ));
    }
}

/// Looks up a hash in the collision-preserving shared [`DatIndex`]. An empty
/// slice is the neutral unknown/no-match result; no filename fallback exists.
pub fn lookup_tosec<'a>(
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

/// Converts every matching TOSEC member into exact and post-match metadata
/// observations. Collision/multiplicity is intentionally retained: no first
/// result is selected and filename metadata is produced only for those exact
/// hash-selected members.
pub fn observations_from_tosec_matches(
    source: &ImportedTosecSource,
    algorithm: ChecksumAlgorithm,
    hash_value: &str,
    representation: Representation,
) -> Vec<EvidenceObservation> {
    let matches = lookup_tosec(&source.index, algorithm, hash_value);
    let mut out = Vec::new();
    for rom in matches {
        let metadata = matched_title(rom);
        out.push(exact_observation(
            source,
            rom,
            representation,
            algorithm,
            hash_value,
            &metadata,
        ));
        out.push(platform_observation(source, representation));
        out.extend(metadata_observations(source, &metadata));
    }
    out
}
