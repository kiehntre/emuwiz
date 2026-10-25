use std::collections::HashMap;

use super::*;
use crate::chd_identity::observe_chd_identity;
use crate::dat::index::{DatDiskKey, DatDiskRef, DiskLocation};
use crate::platform_evidence_fusion::evidence_lineage::{
    AgreementStatus, SourceFamily, independent_source_group_count, merge_evidence,
};

const TRACK_SHA1: &str = "1111111111111111111111111111111111111111";
const RAW_SHA1: &str = "2222222222222222222222222222222222222222";
const COMBINED_SHA1: &str = "3333333333333333333333333333333333333333";
const PARENT_SHA1: &str = "4444444444444444444444444444444444444444";
const PHYSICAL_CHD_SHA256: &str =
    "5555555555555555555555555555555555555555555555555555555555555555";

fn hex_bytes(value: &str) -> [u8; 20] {
    let mut out = [0; 20];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap();
    }
    out
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn chd_identity(raw: &str, combined: &str, parent: &str) -> ChdIdentityObservation {
    let mut bytes = vec![0_u8; 124];
    bytes[..8].copy_from_slice(b"MComprHD");
    put_u32(&mut bytes, 8, 124);
    put_u32(&mut bytes, 12, 5);
    put_u32(&mut bytes, 56, 0x20_000);
    put_u32(&mut bytes, 60, 0x800);
    bytes[64..84].copy_from_slice(&hex_bytes(raw));
    bytes[84..104].copy_from_slice(&hex_bytes(combined));
    bytes[104..124].copy_from_slice(&hex_bytes(parent));
    observe_chd_identity(&bytes).unwrap()
}

fn index_for(sha1: &str) -> DatDiskIndex {
    let disk = DatDiskRef {
        game_index: 0,
        game_name: "Example disc".to_string(),
        disk_key: DatDiskKey {
            game_index: 0,
            location: DiskLocation::TopLevel { disk_index: 0 },
        },
        disk_name: "display-only-name.chd".to_string(),
        sha1: sha1.to_string(),
        status: None,
        merge: None,
        optional: None,
    };
    DatDiskIndex {
        by_disk_sha1: HashMap::from([(sha1.to_string(), vec![disk])]),
    }
}

fn source_with_artifact_name(artifact_name: &str) -> MameRedumpSource {
    MameRedumpSource::from_explicit_classification(SourceArtifactIdentity {
        source_family: SourceFamily::MAMERedump,
        upstream_version: Some("0.280".to_string()),
        artifact_sha256: Some("a".repeat(64)),
        artifact_name: Some(artifact_name.to_string()),
    })
    .unwrap()
}

fn source() -> MameRedumpSource {
    source_with_artifact_name("arbitrary-display-name.xml")
}

fn logical_observation(value: &str) -> EvidenceObservation {
    observations_from_mame_redump_chd_identity(
        &source(),
        &index_for(value),
        &chd_identity(RAW_SHA1, value, "0000000000000000000000000000000000000000"),
    )
    .pop()
    .unwrap()
}

#[test]
fn only_combined_sha1_matches_the_mame_disk_index() {
    let identity = chd_identity(RAW_SHA1, COMBINED_SHA1, PARENT_SHA1);
    let observations =
        observations_from_mame_redump_chd_identity(&source(), &index_for(COMBINED_SHA1), &identity);
    assert_eq!(observations.len(), 1);
    assert_eq!(
        observations[0].hash_or_value.as_deref(),
        Some(COMBINED_SHA1)
    );
    assert!(
        observations_from_mame_redump_chd_identity(&source(), &index_for(RAW_SHA1), &identity)
            .is_empty()
    );
    assert!(
        observations_from_mame_redump_chd_identity(&source(), &index_for(PARENT_SHA1), &identity)
            .is_empty()
    );
}

#[test]
fn track_sha1_is_never_equated_with_chd_combined_sha1() {
    assert!(
        correlate_redump_track_set_to_chd_combined_sha1(&[TRACK_SHA1.to_string()], COMBINED_SHA1,)
            .is_none()
    );
    assert_ne!(TRACK_SHA1, COMBINED_SHA1);
}

#[test]
fn mame_redump_logical_chd_evidence_is_derived_from_redump() {
    let observation = logical_observation(COMBINED_SHA1);
    assert_eq!(
        observation.provenance.representation,
        Representation::LogicalChd
    );
    assert_eq!(observation.claim, ClaimType::ExactLogicalDiscMatch);
    assert_eq!(
        observation.provenance.upstream_source,
        SourceFamily::MAMERedump
    );
    assert_eq!(observation.provenance.lineage, LineageRelation::DerivedFrom);
    assert_eq!(
        classify_mame_redump_lineage(SourceFamily::MAMERedump),
        Some(LineageRelation::DerivedFrom)
    );
}

#[test]
fn unrelated_mame_families_remain_independent() {
    assert_eq!(classify_mame_redump_lineage(SourceFamily::MAMEArcade), None);
    assert_eq!(
        classify_mame_redump_lineage(SourceFamily::MAMESoftwareList),
        None
    );
}

#[test]
fn redump_and_mame_redump_agreement_are_one_upstream_lane() {
    let mut direct = logical_observation(COMBINED_SHA1);
    direct.provenance.channel = EvidenceChannel::LocalRedump;
    direct.provenance.upstream_source = SourceFamily::Redump;
    direct.provenance.lineage = LineageRelation::Independent;
    let derived = logical_observation(COMBINED_SHA1);
    let summaries = merge_evidence(&[direct, derived]);
    assert_eq!(summaries[0].status, AgreementStatus::DerivedAgreement);
    assert_eq!(
        independent_source_group_count(&summaries[0].observations),
        1
    );
}

#[test]
fn derived_source_conflict_is_preserved_and_fail_closed() {
    let mut direct = logical_observation(COMBINED_SHA1);
    direct.provenance.channel = EvidenceChannel::LocalRedump;
    direct.provenance.upstream_source = SourceFamily::Redump;
    direct.provenance.lineage = LineageRelation::Independent;
    direct.hash_or_value = Some(RAW_SHA1.to_string());
    let derived = logical_observation(COMBINED_SHA1);
    let summaries = merge_evidence(&[direct, derived]);
    assert_eq!(summaries[0].status, AgreementStatus::DerivedSourceConflict);
    assert!(summaries[0].status.is_conflict());
}

#[test]
fn raw_combined_parent_and_physical_hashes_remain_distinct() {
    let identity = chd_identity(RAW_SHA1, COMBINED_SHA1, PARENT_SHA1);
    assert_eq!(identity.raw_sha1_hex(), RAW_SHA1);
    assert_eq!(identity.combined_sha1_hex(), COMBINED_SHA1);
    assert_eq!(identity.parent_sha1_hex(), PARENT_SHA1);
    assert_ne!(PHYSICAL_CHD_SHA256, RAW_SHA1);
    assert_ne!(PHYSICAL_CHD_SHA256, COMBINED_SHA1);
    assert_ne!(PHYSICAL_CHD_SHA256, PARENT_SHA1);
}

#[test]
fn filename_and_cue_filename_have_zero_crosswalk_authority() {
    let track_hashes = vec![TRACK_SHA1.to_string()];
    let chd_filename = "Example Game (USA).chd";
    let cue_filename = "Example Game (USA).cue";
    // Neither display name appears in the bridge input. Even an intentionally
    // similar pair therefore cannot manufacture the absent mapping.
    assert!(
        correlate_redump_track_set_to_chd_combined_sha1(&track_hashes, COMBINED_SHA1).is_none()
    );
    assert_ne!(chd_filename, cue_filename);

    let differently_named_source = source_with_artifact_name("misleading-redump-name.xml");
    let identity = chd_identity(RAW_SHA1, COMBINED_SHA1, PARENT_SHA1);
    let observation = observations_from_mame_redump_chd_identity(
        &differently_named_source,
        &index_for(COMBINED_SHA1),
        &identity,
    )
    .pop()
    .unwrap();
    assert_eq!(observation.hash_or_value.as_deref(), Some(COMBINED_SHA1));
}

#[test]
fn missing_track_set_mapping_fails_closed() {
    assert!(
        correlate_redump_track_set_to_chd_combined_sha1(&[TRACK_SHA1.to_string()], COMBINED_SHA1,)
            .is_none()
    );
}

#[test]
fn child_chd_parent_reference_does_not_create_independent_identity() {
    let child = chd_identity(RAW_SHA1, COMBINED_SHA1, PARENT_SHA1);
    let observations =
        observations_from_mame_redump_chd_identity(&source(), &index_for(COMBINED_SHA1), &child);
    assert!(child.parent_required);
    assert_eq!(observations.len(), 1);
    assert_eq!(
        observations[0].provenance.representation,
        Representation::LogicalChd
    );
    assert_ne!(
        observations[0].provenance.representation,
        Representation::PhysicalFile
    );
    assert!(
        observations_from_mame_redump_chd_identity(&source(), &index_for(PARENT_SHA1), &child)
            .is_empty()
    );
}

#[test]
fn source_family_must_be_explicitly_mame_redump() {
    let error = MameRedumpSource::from_explicit_classification(SourceArtifactIdentity {
        source_family: SourceFamily::MAMEArcade,
        upstream_version: None,
        artifact_sha256: None,
        artifact_name: Some("mame-redump-looking-name.xml".to_string()),
    })
    .unwrap_err();
    assert_eq!(
        error,
        MameRedumpSourceError::NotMameRedump {
            found: SourceFamily::MAMEArcade
        }
    );
}
