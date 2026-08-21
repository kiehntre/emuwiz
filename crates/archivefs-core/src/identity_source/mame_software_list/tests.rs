use std::path::Path;

use tempfile::tempdir;

use crate::dat::classification::{ContentSelectionPolicy, DatContentClass};
use crate::dat::identity::{DatPlatformIdentity, identify_dat_source};
use crate::dat::model::{ChecksumAlgorithm, DatEcosystem};
use crate::dat::parsers::parse_dat_file;
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, ClaimType, EvidenceChannel, LineageRelation, Representation, SourceFamily,
    known_derivation,
};

use super::convert::{
    lookup_mame_software_list, lookup_mame_software_list_disk_sha1,
    observations_from_mame_software_list_disk_matches,
    observations_from_mame_software_list_matches,
};
use super::import::{MameSoftwareListImportError, import_mame_software_list};

const SHA1: &str = "111111111111111111111111111111111111111a";
const MD5: &str = "22222222222222222222222222222222";
const CRC: &str = "aaaaaaaa";
const CRC_ONLY: &str = "bbbbbbbb";
const BADDUMP_SHA1: &str = "333333333333333333333333333333333333333c";
const NODUMP_SHA1: &str = "444444444444444444444444444444444444444d";
const DISK_SHA1: &str = "555555555555555555555555555555555555555e";

const SOFTWARE_LIST_XML: &str = r#"<?xml version="1.0"?>
<softwarelist name="testcart" description="Test Cartridge System">
  <software name="goodgame" supported="yes">
    <description>Good Game</description><year>1992</year><publisher>Test Publisher</publisher>
    <part name="cart" interface="cart"><dataarea name="rom">
      <rom name="good-game.bin" size="16" crc="AAAAAAAA" md5="22222222222222222222222222222222" sha1="111111111111111111111111111111111111111a"/>
    </dataarea></part>
  </software>
  <software name="partialgame" cloneof="goodgame" supported="partial">
    <description>Partial Game</description>
    <part name="cart" interface="cart"><dataarea name="rom"><rom name="partial.bin" size="8" crc="BBBBBBBB"/></dataarea></part>
  </software>
  <software name="unsupportedgame" supported="no">
    <description>Unsupported Game</description>
    <part name="cart" interface="cart"><dataarea name="rom"><rom name="bad.bin" sha1="333333333333333333333333333333333333333c" status="baddump"/></dataarea></part>
  </software>
  <software name="nodumpgame" supported="yes">
    <part name="cart" interface="cart"><dataarea name="rom"><rom name="missing.bin" sha1="444444444444444444444444444444444444444d" status="nodump"/></dataarea></part>
  </software>
  <software name="diskgame" supported="yes">
    <part name="cd" interface="cdrom"><diskarea name="cdrom"><disk name="diskgame" sha1="555555555555555555555555555555555555555e"/></diskarea></part>
  </software>
  <software name="collisionone" supported="yes"><part name="a"><dataarea name="r"><rom name="one.bin" crc="DEADBEEF"/></dataarea></part></software>
  <software name="collisiontwo" supported="yes"><part name="b"><dataarea name="r"><rom name="two.bin" crc="DEADBEEF"/></dataarea></part></software>
</softwarelist>"#;

fn write_dat(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn imported() -> super::import::ImportedMameSoftwareListSource {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "filename-is-not-authority.xml",
        SOFTWARE_LIST_XML,
    );
    import_mame_software_list(&path).unwrap()
}

fn exact(
    source: &super::import::ImportedMameSoftwareListSource,
    algorithm: ChecksumAlgorithm,
    hash: &str,
) -> crate::platform_evidence_fusion::evidence_lineage::EvidenceObservation {
    observations_from_mame_software_list_matches(source, algorithm, hash)
        .into_iter()
        .find(|observation| observation.claim == ClaimType::ExactBytesMatch)
        .unwrap()
}

#[test]
fn software_list_root_has_a_distinct_ecosystem() {
    assert_eq!(
        imported().dat.source.ecosystem,
        DatEcosystem::MAMESoftwareList
    );
}

#[test]
fn ordinary_logiqx_datafile_detection_is_unchanged() {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "ordinary.dat",
        "<datafile><header><name>Generic DAT</name></header><game name=\"x\"/></datafile>",
    );
    assert_eq!(
        parse_dat_file(&path, Default::default())
            .unwrap()
            .dat
            .source
            .ecosystem,
        DatEcosystem::GenericLogiqx
    );
}

#[test]
fn list_name_shortname_metadata_and_clone_are_preserved() {
    let source = imported();
    assert_eq!(source.software_list_name.as_deref(), Some("testcart"));
    assert_eq!(source.dat.games[0].name, "goodgame");
    assert_eq!(source.dat.games[1].clone_of.as_deref(), Some("goodgame"));
    assert_eq!(
        source.dat.games[0].description.as_deref(),
        Some("Good Game")
    );
    assert_eq!(source.dat.games[0].year.as_deref(), Some("1992"));
    assert_eq!(
        source.dat.games[0].manufacturer.as_deref(),
        Some("Test Publisher")
    );
}

#[test]
fn software_list_name_is_not_automatic_platform_identity() {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "Nintendo - Game Boy.xml",
        SOFTWARE_LIST_XML.replace("testcart", "gameboy").as_str(),
    );
    let source = import_mame_software_list(&path).unwrap();
    assert!(matches!(
        identify_dat_source(&source.dat),
        DatPlatformIdentity::Unknown
    ));
}

#[test]
fn software_shortname_is_not_automatic_platform_identity() {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "untrusted.xml",
        r#"<softwarelist name="unknown"><software name="neocd"><part name="cart"><dataarea name="rom"><rom name="x.bin" crc="AAAAAAAA"/></dataarea></part></software></softwarelist>"#,
    );
    let source = import_mame_software_list(&path).unwrap();
    assert!(matches!(
        identify_dat_source(&source.dat),
        DatPlatformIdentity::Unknown
    ));
}

#[test]
fn supported_states_are_preserved_without_turning_no_into_non_game() {
    let source = imported();
    assert_eq!(source.dat.games[0].supported.as_deref(), Some("yes"));
    assert_eq!(source.dat.games[1].supported.as_deref(), Some("partial"));
    let unsupported = &source.dat.games[2];
    assert_eq!(unsupported.supported.as_deref(), Some("no"));
    assert_eq!(
        unsupported.content_classification.class,
        DatContentClass::Unknown
    );
    assert_eq!(
        ContentSelectionPolicy::GamesOnly.eligibility(&unsupported.content_classification),
        crate::dat::classification::ContentEligibility::NeedsReview
    );
}

#[test]
fn parts_and_disk_areas_remain_distinct() {
    let source = imported();
    assert_eq!(source.rom_count, 6);
    assert_eq!(source.dat.games[0].parts.len(), 1);
    assert_eq!(source.dat.games[0].parts[0].data_areas[0].roms.len(), 1);
    assert_eq!(source.dat.games[4].parts[0].disk_areas[0].disks.len(), 1);
}

#[test]
fn strong_hashes_are_exact_software_member_evidence() {
    let source = imported();
    for (algorithm, hash) in [
        (ChecksumAlgorithm::Sha1, SHA1),
        (ChecksumAlgorithm::Md5, MD5),
    ] {
        let observation = exact(&source, algorithm, hash);
        assert_eq!(
            observation.provenance.representation,
            Representation::SoftwareListMember
        );
        assert_eq!(observation.claim_strength, ClaimStrength::Strong);
        assert_eq!(observation.provenance.channel, EvidenceChannel::LocalMame);
        assert_eq!(
            observation.provenance.upstream_source,
            SourceFamily::MAMESoftwareList
        );
        assert_eq!(observation.provenance.lineage, LineageRelation::Independent);
        assert_eq!(observation.platform_candidate, None);
    }
}

#[test]
fn crc_is_corroborated_even_when_row_also_has_sha1_and_md5() {
    assert_eq!(
        exact(&imported(), ChecksumAlgorithm::Crc32, CRC).claim_strength,
        ClaimStrength::Corroborated
    );
    assert_eq!(
        exact(&imported(), ChecksumAlgorithm::Crc32, CRC_ONLY).claim_strength,
        ClaimStrength::Corroborated
    );
}

#[test]
fn baddump_is_downgraded() {
    let observation = exact(&imported(), ChecksumAlgorithm::Sha1, BADDUMP_SHA1);
    assert_eq!(observation.claim_strength, ClaimStrength::Corroborated);
}

#[test]
fn nodump_never_emits_identity_evidence_even_with_a_hash() {
    let source = imported();
    assert_eq!(
        lookup_mame_software_list(&source.index, ChecksumAlgorithm::Sha1, NODUMP_SHA1).len(),
        1
    );
    assert!(
        observations_from_mame_software_list_matches(&source, ChecksumAlgorithm::Sha1, NODUMP_SHA1)
            .is_empty()
    );
    assert!(
        source
            .dat
            .source
            .parse_warnings
            .iter()
            .any(|warning| warning.contains("status=nodump together with a checksum"))
    );
}

#[test]
fn disk_sha1_uses_only_the_logical_chd_lane() {
    let source = imported();
    assert_eq!(
        lookup_mame_software_list_disk_sha1(&source, DISK_SHA1).len(),
        1
    );
    let observations = observations_from_mame_software_list_disk_matches(&source, DISK_SHA1);
    let exact = observations
        .iter()
        .find(|observation| observation.claim == ClaimType::ExactLogicalDiscMatch)
        .unwrap();
    assert_eq!(exact.provenance.representation, Representation::LogicalChd);
    assert_eq!(
        exact.provenance.upstream_source,
        SourceFamily::MAMESoftwareList
    );
    assert_eq!(exact.platform_candidate, None);
}

#[test]
fn collisions_are_preserved_not_selected() {
    let source = imported();
    let matches = lookup_mame_software_list(&source.index, ChecksumAlgorithm::Crc32, "deadbeef");
    assert_eq!(matches.len(), 2);
    let observations = observations_from_mame_software_list_matches(
        &imported(),
        ChecksumAlgorithm::Crc32,
        "deadbeef",
    );
    assert_eq!(
        observations
            .iter()
            .filter(|observation| observation.claim == ClaimType::ExactBytesMatch)
            .count(),
        2
    );
}

#[test]
fn filename_and_description_have_no_hash_authority() {
    let source = imported();
    assert!(
        lookup_mame_software_list(&source.index, ChecksumAlgorithm::Sha1, "good-game.bin")
            .is_empty()
    );
    assert!(
        observations_from_mame_software_list_matches(&source, ChecksumAlgorithm::Sha1, "Good Game")
            .is_empty()
    );
}

#[test]
fn malformed_and_non_software_list_inputs_fail_closed() {
    let dir = tempdir().unwrap();
    let malformed = write_dat(dir.path(), "broken.xml", "<softwarelist");
    assert!(matches!(
        import_mame_software_list(&malformed),
        Err(MameSoftwareListImportError::Parse(_))
    ));
    let datafile = write_dat(
        dir.path(),
        "not-mame.xml",
        "<datafile><header><name>Generic</name></header></datafile>",
    );
    assert!(matches!(
        import_mame_software_list(&datafile),
        Err(MameSoftwareListImportError::NotMameSoftwareList { .. })
    ));
}

#[test]
fn mame_redump_derivation_is_unchanged() {
    assert_eq!(
        known_derivation(SourceFamily::MAMERedump),
        Some(SourceFamily::Redump)
    );
    assert_eq!(known_derivation(SourceFamily::MAMESoftwareList), None);
}
