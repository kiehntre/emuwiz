use std::path::Path;

use tempfile::tempdir;

use super::{
    FBNeoImportError, import_fbneo_dat, lookup_fbneo, lookup_fbneo_disk_sha1,
    observations_from_fbneo_disk_matches, observations_from_fbneo_matches,
};
use crate::dat::{
    classification::DatContentClass,
    identity::{DatPlatformIdentity, identify_dat_source},
    model::{ChecksumAlgorithm, DatEcosystem},
    parsers::parse_dat_file,
};
use crate::platform_evidence_fusion::evidence_lineage::{
    AgreementStatus, ClaimStrength, ClaimType, EvidenceChannel, LineageRelation, Representation,
    SourceFamily, hasheous_observation, known_derivation, merge_evidence,
};

const SHA: &str = "111111111111111111111111111111111111111a";
const SHARED_SHA: &str = "222222222222222222222222222222222222222b";
const MD5: &str = "33333333333333333333333333333333";
const BAD_SHA: &str = "444444444444444444444444444444444444444d";
const NODUMP_SHA: &str = "555555555555555555555555555555555555555e";
const UNKNOWN_SHA: &str = "666666666666666666666666666666666666666f";
const DISK: &str = "777777777777777777777777777777777777777a";

const FBNEO_XML: &str = r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>FBNeo - Neo Geo</name>
    <description>FinalBurn Neo arcade preservation catalogue</description>
    <version>test-1</version>
    <author>FBNeo</author>
  </header>
  <game name="parent">
    <description>Parent Title</description><year>1996</year><manufacturer>Example</manufacturer>
    <rom name="parent.bin" size="1" crc="AAAAAAAA" sha1="111111111111111111111111111111111111111a"/>
    <rom name="md5.bin" size="2" md5="33333333333333333333333333333333"/>
    <rom name="crc-only.bin" size="3" crc="BBBBBBBB"/>
    <rom name="shared.bin" size="4" crc="CCCCCCCC" sha1="222222222222222222222222222222222222222b" merge="shared-parent.bin"/>
    <disk name="logical.chd" sha1="777777777777777777777777777777777777777a"/>
  </game>
  <game name="clone" cloneof="parent" romof="parent" sampleof="parent">
    <rom name="shared.bin" size="4" crc="CCCCCCCC" sha1="222222222222222222222222222222222222222b" merge="shared-parent.bin"/>
  </game>
  <game name="bad"><rom name="bad.bin" sha1="444444444444444444444444444444444444444d" crc="DDDDDDDD" status="baddump"/></game>
  <game name="nodump"><rom name="nodump.bin" sha1="555555555555555555555555555555555555555e" crc="EEEEEEEE" status="nodump"/></game>
  <game name="unknown-status"><metadata arbitrary="retained-or-ignored"/><rom name="unknown.bin" sha1="666666666666666666666666666666666666666f" status="future-status"/></game>
  <game name="bios-support" isbios="yes"><rom name="bios.bin" crc="FFFFFFFF"/></game>
</datafile>"#;

fn write(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn source() -> super::ImportedFBNeoSource {
    let dir = tempdir().unwrap();
    import_fbneo_dat(&write(dir.path(), "catalogue.dat", FBNEO_XML)).unwrap()
}

#[test]
fn detects_fbneo_and_preserves_catalogue_context() {
    let source = source();
    assert_eq!(source.dat.source.ecosystem, DatEcosystem::FBNeo);
    assert_eq!(source.source_name.as_deref(), Some("FBNeo - Neo Geo"));
    assert_eq!(source.upstream_version.as_deref(), Some("test-1"));
    assert_eq!(source.dat.games.len(), 6);
    assert_eq!(
        source.dat.games[0].description.as_deref(),
        Some("Parent Title")
    );
    assert_eq!(source.dat.games[0].year.as_deref(), Some("1996"));
    assert_eq!(source.dat.games[0].manufacturer.as_deref(), Some("Example"));
    assert_eq!(source.entry_count, 6);
    assert_eq!(source.rom_count, 9);
    assert_eq!(source.disk_count, 1);
    assert_eq!(source.artifact_name, "catalogue.dat");
    assert_eq!(source.artifact_sha256.len(), 64);
}

#[test]
fn fbneo_detection_is_distinct_from_other_supported_xml_catalogues() {
    let dir = tempdir().unwrap();
    let cases = [
        ("fbneo.dat", FBNEO_XML, DatEcosystem::FBNeo),
        (
            "generic.dat",
            "<datafile><header><name>Custom ROM Archive</name></header></datafile>",
            DatEcosystem::GenericLogiqx,
        ),
        (
            "nointro.dat",
            "<datafile><header><name>No-Intro Test</name></header></datafile>",
            DatEcosystem::NoIntro,
        ),
        (
            "redump.dat",
            "<datafile><header><name>Redump Test</name></header></datafile>",
            DatEcosystem::Redump,
        ),
        (
            "software.xml",
            "<softwarelist name=\"test\" description=\"Test\"></softwarelist>",
            DatEcosystem::MAMESoftwareList,
        ),
        (
            "tosec.dat",
            "<datafile><header><name>TOSEC Test</name></header></datafile>",
            DatEcosystem::Tosec,
        ),
    ];

    for (name, contents, expected) in cases {
        let path = write(dir.path(), name, contents);
        assert_eq!(
            parse_dat_file(&path, Default::default())
                .unwrap()
                .dat
                .source
                .ecosystem,
            expected,
            "{name}"
        );
    }
}

#[test]
fn checksum_statuses_are_conservative_and_filename_has_no_lookup_authority() {
    let source = source();
    assert_eq!(
        observations_from_fbneo_matches(&source, ChecksumAlgorithm::Sha1, SHA)[0].claim_strength,
        ClaimStrength::Strong
    );
    assert_eq!(
        observations_from_fbneo_matches(&source, ChecksumAlgorithm::Md5, MD5)[0].claim_strength,
        ClaimStrength::Strong
    );
    assert_eq!(
        observations_from_fbneo_matches(&source, ChecksumAlgorithm::Crc32, "bbbbbbbb")[0]
            .claim_strength,
        ClaimStrength::Corroborated
    );
    assert!(
        observations_from_fbneo_matches(&source, ChecksumAlgorithm::Sha1, NODUMP_SHA).is_empty()
    );
    assert_eq!(
        observations_from_fbneo_matches(&source, ChecksumAlgorithm::Sha1, BAD_SHA)[0]
            .claim_strength,
        ClaimStrength::Corroborated
    );
    assert!(
        observations_from_fbneo_matches(&source, ChecksumAlgorithm::Sha1, UNKNOWN_SHA).is_empty()
    );
    assert!(lookup_fbneo(&source.index, ChecksumAlgorithm::Sha1, "parent.bin").is_empty());
    assert_eq!(source.index.lookup_sha1(NODUMP_SHA).len(), 1);
    assert_eq!(
        source.index.lookup_sha1(NODUMP_SHA)[0].status.as_deref(),
        Some("nodump")
    );
}

#[test]
fn shortnames_and_header_text_are_not_canonical_platform_authority() {
    let source = source();
    assert!(matches!(
        identify_dat_source(&source.dat),
        DatPlatformIdentity::Unknown
    ));
    assert_eq!(source.dat.games[0].name, "parent");
}

#[test]
fn clone_shared_rom_and_bios_metadata_are_preserved_without_flattening() {
    let source = source();
    let clone = &source.dat.games[1];
    assert_eq!(clone.clone_of.as_deref(), Some("parent"));
    assert_eq!(clone.rom_of.as_deref(), Some("parent"));
    assert_eq!(clone.sample_of.as_deref(), Some("parent"));
    let shared = lookup_fbneo(&source.index, ChecksumAlgorithm::Sha1, SHARED_SHA);
    assert_eq!(shared.len(), 2);
    assert!(
        shared
            .iter()
            .all(|row| row.merge.as_deref() == Some("shared-parent.bin"))
    );
    assert_eq!(shared[0].game_name, "parent");
    assert_eq!(shared[1].game_name, "clone");
    assert_eq!(source.dat.games[5].is_bios.as_deref(), Some("yes"));
    assert_eq!(
        source.dat.games[5].content_classification.class,
        DatContentClass::Unknown
    );
}

#[test]
fn collisions_remain_multiple_fbneo_candidates_without_a_first_winner() {
    let source = source();
    let observations =
        observations_from_fbneo_matches(&source, ChecksumAlgorithm::Sha1, SHARED_SHA);
    assert_eq!(observations.len(), 2);
    let releases: Vec<_> = observations
        .iter()
        .map(|observation| observation.release_candidate.as_deref())
        .collect();
    assert_eq!(releases, vec![Some("parent"), Some("clone")]);
}

#[test]
fn fbneo_evidence_is_an_independent_non_mame_non_redump_lane() {
    let observation = &observations_from_fbneo_matches(&source(), ChecksumAlgorithm::Sha1, SHA)[0];
    assert_eq!(observation.provenance.channel, EvidenceChannel::LocalFBNeo);
    assert_eq!(observation.provenance.upstream_source, SourceFamily::FBNeo);
    assert_eq!(observation.provenance.lineage, LineageRelation::Independent);
    assert_ne!(
        observation.provenance.upstream_source,
        SourceFamily::MAMEArcade
    );
    assert_ne!(observation.provenance.upstream_source, SourceFamily::Redump);
    assert_ne!(
        observation.provenance.upstream_source,
        SourceFamily::MAMERedump
    );
    assert_eq!(known_derivation(SourceFamily::FBNeo), None);
}

#[test]
fn disk_sha1_stays_in_the_logical_chd_lane() {
    let source = source();
    assert!(lookup_fbneo(&source.index, ChecksumAlgorithm::Sha1, DISK).is_empty());
    assert_eq!(lookup_fbneo_disk_sha1(&source, DISK).len(), 1);
    let observation = &observations_from_fbneo_disk_matches(&source, DISK)[0];
    assert_eq!(observation.claim, ClaimType::ExactLogicalDiscMatch);
    assert_eq!(
        observation.provenance.representation,
        Representation::LogicalChd
    );
    assert_eq!(observation.claim_strength, ClaimStrength::Strong);
}

#[test]
fn malformed_and_non_fbneo_catalogues_fail_closed() {
    let dir = tempdir().unwrap();
    let malformed = write(dir.path(), "bad.dat", "<datafile><game");
    assert!(import_fbneo_dat(&malformed).is_err());

    let generic = write(
        dir.path(),
        "not-fbneo.dat",
        "<datafile><header><name>Custom</name></header></datafile>",
    );
    assert!(matches!(
        import_fbneo_dat(&generic),
        Err(FBNeoImportError::NotFBNeo { .. })
    ));
}

#[test]
fn conflicting_strong_evidence_against_fbneo_fails_closed() {
    let source = source();
    let fbneo = observations_from_fbneo_matches(&source, ChecksumAlgorithm::Sha1, SHA)
        .into_iter()
        .next()
        .unwrap();
    let conflicting_no_intro = hasheous_observation(
        "NoIntro",
        Representation::PhysicalFile,
        ClaimType::ExactBytesMatch,
        Some(SHARED_SHA.to_string()),
        None,
    );
    assert_eq!(
        merge_evidence(&[fbneo, conflicting_no_intro])[0].status,
        AgreementStatus::IndependentSourceConflict
    );
}
