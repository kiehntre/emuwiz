use std::path::Path;

use sha2::{Digest, Sha256};
use tempfile::tempdir;

use crate::dat::classification::multidisc_group_key;
use crate::dat::model::ChecksumAlgorithm;
use crate::platform_evidence_fusion::evidence_lineage::{
    AgreementStatus, ClaimStrength, ClaimType, EvidenceChannel, LineageRelation, Representation,
    SourceFamily, hasheous_observation, merge_evidence,
};

use super::convert::{
    claim_for_representation, lookup_redump, lookup_redump_disk_sha1,
    observations_from_redump_disk_matches, observations_from_redump_matches,
};
use super::import::{RedumpImportError, import_redump_dat};

const CUE_SHA1: &str = "111111111111111111111111111111111111111a";
const TRACK_SHA1: &str = "444444444444444444444444444444444444444d";
const TRACK_MD5: &str = "66666666666666666666666666666666";
const DISK_SHA1: &str = "333333333333333333333333333333333333333c";

const REDUMP_XML: &str = r#"<?xml version="1.0"?>
<datafile><header>
<name>Redump - Sony PlayStation</name><description>Redump DAT for Sony PlayStation</description>
<version>2026-08-01</version><author>Redump.org</author>
</header>
<game name="Example Game (USA) (Disc 1)">
<rom name="Example Game (USA) (Disc 1).cue" size="123" crc="AAAAAAAA" md5="22222222222222222222222222222222" sha1="111111111111111111111111111111111111111a"/>
<rom name="Example Game (USA) (Disc 1) (Track 02).bin" size="456" crc="BBBBBBBB" md5="66666666666666666666666666666666" sha1="444444444444444444444444444444444444444d"/>
<disk name="example.chd" sha1="333333333333333333333333333333333333333c"/>
</game>
<game name="Example Game (USA) (Disc 2)"><rom name="disc2.bin" size="789" crc="CCCCCCCC" sha1="555555555555555555555555555555555555555e"/></game>
</datafile>"#;

fn write_dat(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn imported() -> super::import::ImportedRedumpSource {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "anything-not-authoritative.dat", REDUMP_XML);
    import_redump_dat(&path).unwrap()
}

fn track(
    source: &super::import::ImportedRedumpSource,
    algorithm: ChecksumAlgorithm,
    hash: &str,
) -> crate::platform_evidence_fusion::evidence_lineage::EvidenceObservation {
    observations_from_redump_matches(source, algorithm, hash)
        .into_iter()
        .find(|observation| observation.claim == ClaimType::ExactTrackMatch)
        .unwrap()
}

#[test]
fn identifies_redump_from_header_name() {
    assert_eq!(
        imported().system_name.as_deref(),
        Some("Redump - Sony PlayStation")
    );
}

#[test]
fn identifies_redump_from_header_description() {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "not-redump.dat",
        REDUMP_XML
            .replacen("Redump - Sony PlayStation", "Sony PlayStation", 1)
            .as_str(),
    );
    assert!(import_redump_dat(&path).is_ok());
}

#[test]
fn identifies_redump_from_header_author() {
    let dir = tempdir().unwrap();
    let xml = REDUMP_XML
        .replace("Redump - Sony PlayStation", "Sony PlayStation")
        .replace("Redump DAT for Sony PlayStation", "Catalogue");
    let path = write_dat(dir.path(), "not-redump.dat", &xml);
    assert!(import_redump_dat(&path).is_ok());
}

#[test]
fn identifies_redump_from_header_version() {
    let dir = tempdir().unwrap();
    let xml = REDUMP_XML
        .replace("Redump - Sony PlayStation", "Sony PlayStation")
        .replace("Redump DAT for Sony PlayStation", "Catalogue")
        .replace("Redump.org", "Archive Team")
        .replace("2026-08-01", "Redump build 2026-08-01");
    let path = write_dat(dir.path(), "not-redump.dat", &xml);
    assert!(import_redump_dat(&path).is_ok());
}

#[test]
fn rejects_non_redump_dat() {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "redump-in-name.dat",
        "<datafile><header><name>Generic</name><author>Someone</author></header></datafile>",
    );
    assert!(matches!(
        import_redump_dat(&path),
        Err(RedumpImportError::NotRedump { .. })
    ));
}

#[test]
fn malformed_dat_is_clean_error() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "broken.dat", "<datafile><header><name>Redump");
    assert!(import_redump_dat(&path).is_err());
}

#[test]
fn bom_dat_imports() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "bom.dat", &format!("\u{feff}{REDUMP_XML}"));
    assert_eq!(import_redump_dat(&path).unwrap().entry_count, 2);
}

#[test]
fn version_comes_only_from_dat_metadata() {
    assert_eq!(imported().upstream_version.as_deref(), Some("2026-08-01"));
}

#[test]
fn version_is_not_fabricated_from_filename() {
    let dir = tempdir().unwrap();
    let xml = REDUMP_XML.replace("<version>2026-08-01</version>", "");
    let path = write_dat(dir.path(), "Redump 2099.dat", &xml);
    assert_eq!(import_redump_dat(&path).unwrap().upstream_version, None);
}

#[test]
fn source_system_is_not_fabricated_from_filename() {
    let dir = tempdir().unwrap();
    let xml = REDUMP_XML.replace("<name>Redump - Sony PlayStation</name>", "");
    let path = write_dat(dir.path(), "Nintendo - GameCube.dat", &xml);
    assert_eq!(import_redump_dat(&path).unwrap().system_name, None);
}

#[test]
fn artifact_hash_is_deterministic() {
    let source = imported();
    assert_eq!(source.artifact_sha256.len(), 64);
    assert_eq!(source.artifact_sha256, imported().artifact_sha256);
}

#[test]
fn artifact_hash_matches_streamed_sha256() {
    let dir = tempdir().unwrap();
    let contents = format!("{REDUMP_XML}{}", " ".repeat(128 * 1024));
    let path = write_dat(dir.path(), "large.dat", &contents);
    let source = import_redump_dat(&path).unwrap();
    assert_eq!(
        source.artifact_sha256,
        Sha256::digest(contents.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
}

#[test]
fn artifact_name_is_recorded() {
    assert_eq!(imported().artifact_name, "anything-not-authoritative.dat");
}

#[test]
fn source_counts_are_recorded() {
    let source = imported();
    assert_eq!(
        (source.entry_count, source.rom_count, source.disk_count),
        (2, 3, 1)
    );
}

#[test]
fn manifest_carries_system_version_artifact_and_counts() {
    let line = imported().manifest_line();
    for part in [
        "Sony PlayStation",
        "2026-08-01",
        "entries: 2",
        "roms: 3",
        "disks: 1",
    ] {
        assert!(line.contains(part));
    }
}

#[test]
fn sha1_track_lookup_is_exact() {
    assert_eq!(
        lookup_redump(&imported().index, ChecksumAlgorithm::Sha1, TRACK_SHA1).len(),
        1
    );
}

#[test]
fn md5_track_lookup_is_exact() {
    assert_eq!(
        lookup_redump(&imported().index, ChecksumAlgorithm::Md5, TRACK_MD5).len(),
        1
    );
}

#[test]
fn crc_track_lookup_is_exact() {
    assert_eq!(
        lookup_redump(&imported().index, ChecksumAlgorithm::Crc32, "bbbbbbbb").len(),
        1
    );
}

#[test]
fn unknown_hash_is_a_neutral_empty_match() {
    assert!(
        lookup_redump(
            &imported().index,
            ChecksumAlgorithm::Sha1,
            "0000000000000000000000000000000000000000"
        )
        .is_empty()
    );
}

#[test]
fn crc_collision_preserves_all_candidates() {
    let dir = tempdir().unwrap();
    let xml = REDUMP_XML.replace("</datafile>", "<game name=\"Collision\"><rom name=\"same.bin\" size=\"1\" crc=\"BBBBBBBB\"/></game></datafile>");
    let source = import_redump_dat(&write_dat(dir.path(), "x.dat", &xml)).unwrap();
    assert_eq!(
        lookup_redump(&source.index, ChecksumAlgorithm::Crc32, "bbbbbbbb").len(),
        2
    );
}

#[test]
fn sha1_collision_preserves_all_candidates() {
    let dir = tempdir().unwrap();
    let xml = REDUMP_XML.replace("</datafile>", &format!("<game name=\"Collision\"><rom name=\"same.bin\" size=\"1\" sha1=\"{TRACK_SHA1}\"/></game></datafile>"));
    let source = import_redump_dat(&write_dat(dir.path(), "x.dat", &xml)).unwrap();
    assert_eq!(
        lookup_redump(&source.index, ChecksumAlgorithm::Sha1, TRACK_SHA1).len(),
        2
    );
}

#[test]
fn lookup_order_is_deterministic() {
    let source = imported();
    assert_eq!(
        lookup_redump(&source.index, ChecksumAlgorithm::Sha1, TRACK_SHA1),
        lookup_redump(&source.index, ChecksumAlgorithm::Sha1, TRACK_SHA1)
    );
}

#[test]
fn multiple_tracks_stay_separate_rows() {
    let source = imported();
    assert_eq!(
        lookup_redump(
            &source.index,
            ChecksumAlgorithm::Sha1,
            "444444444444444444444444444444444444444d"
        )[0]
        .rom_name,
        "Example Game (USA) (Disc 1) (Track 02).bin"
    );
}

#[test]
fn track_match_is_disc_track() {
    assert_eq!(
        track(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1)
            .provenance
            .representation,
        Representation::DiscTrack
    );
}

#[test]
fn track_match_claim_is_exact_track() {
    assert_eq!(
        track(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1).claim,
        ClaimType::ExactTrackMatch
    );
}

#[test]
fn sha1_track_match_is_strong() {
    assert_eq!(
        track(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1).claim_strength,
        ClaimStrength::Strong
    );
}

#[test]
fn md5_track_match_is_strong() {
    assert_eq!(
        track(&imported(), ChecksumAlgorithm::Md5, TRACK_MD5).claim_strength,
        ClaimStrength::Strong
    );
}

#[test]
fn crc_only_track_is_corroborated() {
    let dir = tempdir().unwrap();
    let xml = REDUMP_XML.replace("<rom name=\"disc2.bin\" size=\"789\" crc=\"CCCCCCCC\" sha1=\"555555555555555555555555555555555555555e\"/>", "<rom name=\"disc2.bin\" size=\"789\" crc=\"CCCCCCCC\"/>");
    let source = import_redump_dat(&write_dat(dir.path(), "x.dat", &xml)).unwrap();
    assert_eq!(
        track(&source, ChecksumAlgorithm::Crc32, "cccccccc").claim_strength,
        ClaimStrength::Corroborated
    );
}

#[test]
fn cue_row_is_not_filename_identity() {
    let source = imported();
    assert!(
        lookup_redump(
            &source.index,
            ChecksumAlgorithm::Sha1,
            "111111111111111111111111111111111111111b"
        )
        .is_empty()
    );
}

#[test]
fn hashed_cue_row_is_structural_metadata_not_exact_track() {
    let observations =
        observations_from_redump_matches(&imported(), ChecksumAlgorithm::Sha1, CUE_SHA1);
    assert!(
        observations
            .iter()
            .all(|observation| observation.claim != ClaimType::ExactTrackMatch)
    );
    assert!(observations.iter().any(|observation| {
        observation.provenance.representation == Representation::StructuralMetadata
            && observation.claim == ClaimType::DisplayMetadata
            && observation.hash_or_value.as_deref() == Some(CUE_SHA1)
    }));
}

#[test]
fn display_metadata_is_separate_from_track_identity() {
    assert!(
        observations_from_redump_matches(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1)
            .iter()
            .any(|o| o.claim == ClaimType::DisplayMetadata
                && o.provenance.representation == Representation::Unknown)
    );
}

#[test]
fn logical_chd_uses_separate_disk_sha1_index() {
    assert_eq!(lookup_redump_disk_sha1(&imported(), DISK_SHA1).len(), 1);
}

#[test]
fn track_sha1_does_not_match_chd_lane() {
    assert!(lookup_redump_disk_sha1(&imported(), TRACK_SHA1).is_empty());
}

#[test]
fn logical_chd_match_has_logical_representation() {
    assert_eq!(
        observations_from_redump_disk_matches(&imported(), DISK_SHA1)[0]
            .provenance
            .representation,
        Representation::LogicalChd
    );
}

#[test]
fn logical_chd_match_has_logical_claim() {
    assert_eq!(
        observations_from_redump_disk_matches(&imported(), DISK_SHA1)[0].claim,
        ClaimType::ExactLogicalDiscMatch
    );
}

#[test]
fn disk_sha1_does_not_match_track_lane() {
    assert!(lookup_redump(&imported().index, ChecksumAlgorithm::Sha1, DISK_SHA1).is_empty());
}

#[test]
fn raw_disc_is_never_implicitly_emitted() {
    assert!(
        !observations_from_redump_matches(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1)
            .iter()
            .any(|o| o.provenance.representation == Representation::RawDisc)
    );
}

#[test]
fn unsupported_representation_never_gets_an_exact_claim() {
    assert_eq!(
        claim_for_representation(Representation::RawDisc),
        ClaimType::PlatformCandidate
    );
}

#[test]
fn disc_one_and_two_names_are_preserved() {
    let source = imported();
    assert_eq!(
        source
            .dat
            .games
            .iter()
            .map(|game| game.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Example Game (USA) (Disc 1)", "Example Game (USA) (Disc 2)"]
    );
}

#[test]
fn bare_disc_number_is_not_identity_or_invented_grouping_authority() {
    assert!(multidisc_group_key("Example Game (USA) (Disc 1)").is_none());
}

#[test]
fn explicit_multidisc_token_remains_a_grouping_candidate_only() {
    let key = multidisc_group_key("Example Game (USA) (Disc 1 of 2)").unwrap();
    assert_eq!(
        (key.base_title, key.part, key.total),
        ("Example Game (USA)".to_string(), 1, 2)
    );
}

#[test]
fn local_redump_channel_and_independent_lineage() {
    let observation = track(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1);
    assert_eq!(
        (
            observation.provenance.channel,
            observation.provenance.upstream_source,
            observation.provenance.lineage
        ),
        (
            EvidenceChannel::LocalRedump,
            SourceFamily::Redump,
            LineageRelation::Independent
        )
    );
}

#[test]
fn local_and_hasheous_redump_are_same_source() {
    let local = track(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1);
    let relay = hasheous_observation(
        "Redump",
        Representation::DiscTrack,
        ClaimType::ExactTrackMatch,
        Some(TRACK_SHA1.to_string()),
        None,
    );
    assert_eq!(
        merge_evidence(&[local, relay])[0].status,
        AgreementStatus::SameSourceAgreement
    );
}

#[test]
fn local_and_mameredump_agree_as_derived() {
    let local = track(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1);
    let mut derived = hasheous_observation(
        "MAMERedump",
        Representation::DiscTrack,
        ClaimType::ExactTrackMatch,
        Some(TRACK_SHA1.to_string()),
        None,
    );
    derived.provenance.lineage = LineageRelation::DerivedFrom;
    assert_eq!(
        merge_evidence(&[local, derived])[0].status,
        AgreementStatus::DerivedAgreement
    );
}

#[test]
fn local_and_mameredump_disagreement_is_derived_conflict() {
    let local = track(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1);
    let mut derived = hasheous_observation(
        "MAMERedump",
        Representation::DiscTrack,
        ClaimType::ExactTrackMatch,
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
        None,
    );
    derived.provenance.lineage = LineageRelation::DerivedFrom;
    assert_eq!(
        merge_evidence(&[local, derived])[0].status,
        AgreementStatus::DerivedSourceConflict
    );
}

#[test]
fn representation_mismatch_is_not_folded() {
    let track = track(&imported(), ChecksumAlgorithm::Sha1, TRACK_SHA1);
    let logical = observations_from_redump_disk_matches(&imported(), DISK_SHA1)[0].clone();
    assert_ne!(
        (track.claim, track.provenance.representation),
        (logical.claim, logical.provenance.representation)
    );
}

#[test]
fn no_serial_is_fabricated() {
    let source = imported();
    assert!(source.dat.games.iter().all(|game| {
        !game
            .original_metadata
            .fields
            .keys()
            .any(|key| key.eq_ignore_ascii_case("serial"))
    }));
    assert!(
        !format!(
            "{:?}",
            observations_from_redump_matches(&source, ChecksumAlgorithm::Sha1, TRACK_SHA1)
        )
        .to_ascii_lowercase()
        .contains("serial")
    );
}

#[test]
fn missing_path_is_io_error() {
    assert!(matches!(
        import_redump_dat(Path::new("/definitely-not-a-redump-dat.xml")),
        Err(RedumpImportError::Io { .. })
    ));
}
