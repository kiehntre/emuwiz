use std::path::Path;

use tempfile::tempdir;

use super::{
    ImportedMameListxmlSource, import_mame_listxml, observations_from_mame_listxml_disk_matches,
    observations_from_mame_listxml_matches,
};
use crate::dat::classification::DatContentClass;
use crate::dat::identity::{DatPlatformIdentity, identify_dat_source};
use crate::dat::model::{ChecksumAlgorithm, DatEcosystem};
use crate::dat::parsers::parse_dat_file;
use crate::platform_evidence_fusion::evidence_lineage::{
    AgreementStatus, ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope,
    LineageRelation, Provenance, Representation, SourceArtifactIdentity, SourceFamily,
    independent_source_group_count, merge_evidence,
};

const SHA: &str = "111111111111111111111111111111111111111a";
const DISK: &str = "555555555555555555555555555555555555555e";

const XML: &str = r#"<mame build="test"><machine name="pacman" sourcefile="pacman/pacman.cpp" runnable="yes"><description>Pac-Man</description><year>1980</year><manufacturer>Namco</manufacturer><rom name="pac.bin" crc="AAAAAAAA" md5="22222222222222222222222222222222" sha1="111111111111111111111111111111111111111a"/><disk name="pac.chd" sha1="555555555555555555555555555555555555555e"/><softwarelist name="pacman" status="original"/><device_ref name="z80"/><driver status="good"/></machine><machine name="puckman" cloneof="pacman" romof="pacman" sampleof="pacman"><rom name="puck.bin" crc="BBBBBBBB"/></machine><machine name="bios" isbios="yes"><rom name="bios.bin" sha1="333333333333333333333333333333333333333c" status="baddump"/></machine><machine name="device" isdevice="yes"/><machine name="mech" ismechanical="yes"/><machine name="norun" runnable="no"/><machine><rom name="missing.bin" sha1="444444444444444444444444444444444444444d" status="nodump"/></machine></mame>"#;

fn write(dir: &Path, name: &str, xml: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, xml).unwrap();
    path
}

fn source() -> ImportedMameListxmlSource {
    let dir = tempdir().unwrap();
    import_mame_listxml(&write(dir.path(), "mame.xml", XML)).unwrap()
}

// ---------------------------------------------------------------------
// 1. Genuine MAME listxml is detected; runnable machine creates evidence.
// ---------------------------------------------------------------------

#[test]
fn detects_and_streams_multiple_machines() {
    let imported = source();
    assert_eq!(imported.dat.source.ecosystem, DatEcosystem::MAMEArcade);
    assert_eq!(imported.dat.games.len(), 6);
    assert_eq!(
        imported.dat.games[0].description.as_deref(),
        Some("Pac-Man")
    );
    assert_eq!(imported.dat.games[0].year.as_deref(), Some("1980"));
    assert_eq!(imported.dat.games[0].manufacturer.as_deref(), Some("Namco"));
}

#[test]
fn normal_runnable_machine_creates_evidence() {
    let imported = source();
    let observations =
        observations_from_mame_listxml_matches(&imported, ChecksumAlgorithm::Sha1, SHA);
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].release_candidate.as_deref(), Some("pacman"));
    assert_eq!(observations[0].claim, ClaimType::ExactBytesMatch);
}

// ---------------------------------------------------------------------
// 2. Generic XML is never falsely detected as MAME listxml.
// ---------------------------------------------------------------------

#[test]
fn listxml_is_not_softwarelist_or_generic() {
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "x.xml", XML);
    assert_eq!(
        parse_dat_file(&path, Default::default())
            .unwrap()
            .dat
            .source
            .ecosystem,
        DatEcosystem::MAMEArcade
    );

    let path = write(
        dir.path(),
        "s.xml",
        "<softwarelist name=\"x\"><software name=\"y\"/></softwarelist>",
    );
    assert_eq!(
        parse_dat_file(&path, Default::default())
            .unwrap()
            .dat
            .source
            .ecosystem,
        DatEcosystem::MAMESoftwareList
    );

    // A generic Logiqx `<datafile>` that merely mentions "mame" in ordinary
    // header text or in an XML comment must never be misdetected: the sniff
    // inspects the actual root element's tag name, never a raw substring
    // search over the file's bytes (a leading `<!-- <mame ... -->` comment
    // is exactly the shape a naive substring scan would misdetect).
    let path = write(
        dir.path(),
        "generic.xml",
        r#"<?xml version="1.0"?><!-- exported from <mame> for reference --><datafile><header><name>Not MAME At All</name></header><game name="test"><rom name="test.bin" size="1" crc="AAAAAAAA"/></game></datafile>"#,
    );
    assert_eq!(
        parse_dat_file(&path, Default::default())
            .unwrap()
            .dat
            .source
            .ecosystem,
        DatEcosystem::GenericLogiqx
    );
    assert_eq!(
        parse_dat_file(&path, Default::default())
            .unwrap()
            .dat
            .games
            .len(),
        1
    );

    // Nor a tag that merely starts with the same four letters.
    let path = write(
        dir.path(),
        "mameinfo.xml",
        r#"<mameinfo><game name="test"/></mameinfo>"#,
    );
    assert!(import_mame_listxml(&path).is_err());
}

// ---------------------------------------------------------------------
// 3./4. Runnable arcade machine vs BIOS/device/non-runnable classification.
// ---------------------------------------------------------------------

#[test]
fn relationships_and_machine_states_are_preserved() {
    let imported = source();
    assert_eq!(imported.dat.games[1].clone_of.as_deref(), Some("pacman"));
    assert_eq!(imported.dat.games[1].rom_of.as_deref(), Some("pacman"));
    assert_eq!(imported.dat.games[1].sample_of.as_deref(), Some("pacman"));
    // BIOS and device machines are classified NonGame - never a playable
    // title just because they carry a `<rom>`/empty element like any other
    // machine.
    assert_eq!(
        imported.dat.games[2].content_classification.class,
        DatContentClass::NonGame
    );
    assert_eq!(
        imported.dat.games[3].content_classification.class,
        DatContentClass::NonGame
    );
    assert_eq!(
        imported.dat.games[4].original_metadata.fields["ismechanical"],
        "yes"
    );
    // `runnable="no"` is preserved as provenance but is deliberately NOT
    // itself treated as the BIOS/device NonGame signal - only the explicit
    // `isbios`/`isdevice` declarations are.
    assert_eq!(imported.dat.games[5].runnable.as_deref(), Some("no"));
    assert_ne!(
        imported.dat.games[5].content_classification.class,
        DatContentClass::NonGame
    );
}

// ---------------------------------------------------------------------
// 5. Clone/parent relationship preserved (also covered above; kept as its
//    own focused assertion per the required test list).
// ---------------------------------------------------------------------

#[test]
fn clone_parent_relationship_is_preserved() {
    let imported = source();
    let clone = &imported.dat.games[1];
    assert_eq!(clone.name, "puckman");
    assert_eq!(clone.clone_of.as_deref(), Some("pacman"));
    assert_eq!(clone.rom_of.as_deref(), Some("pacman"));
}

// ---------------------------------------------------------------------
// 6. Filename/shortname similarity alone creates no identity.
// ---------------------------------------------------------------------

#[test]
fn machine_name_is_mame_namespace_not_platform_identity() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "name.xml",
        "<mame><machine name=\"neocd\"/></mame>",
    );
    let dat = parse_dat_file(&path, Default::default()).unwrap().dat;
    assert_eq!(dat.games[0].name, "neocd");
    assert!(matches!(
        identify_dat_source(&dat),
        DatPlatformIdentity::Unknown
    ));
}

#[test]
fn filename_similarity_creates_no_identity() {
    let dir = tempdir().unwrap();
    // A file named after a completely different platform's convention must
    // have zero bearing on the parsed machine identity or evidence.
    let path = write(
        dir.path(),
        "Sega - Mega Drive - Genesis.xml",
        "<mame><machine name=\"sf2\"><rom name=\"r.bin\" crc=\"AAAAAAAA\"/></machine></mame>",
    );
    let imported = import_mame_listxml(&path).unwrap();
    assert_eq!(imported.dat.games[0].name, "sf2");
    assert!(matches!(
        identify_dat_source(&imported.dat),
        DatPlatformIdentity::Unknown
    ));
}

// ---------------------------------------------------------------------
// 7. Strong conflicting evidence fails closed.
// ---------------------------------------------------------------------

#[test]
fn strong_conflicting_mame_evidence_fails_closed() {
    let imported = source();
    let mame = observations_from_mame_listxml_matches(&imported, ChecksumAlgorithm::Sha1, SHA)
        .pop()
        .unwrap();
    // A second, genuinely independent strong source (e.g. Redump) asserting
    // a *different* value for the same claim - real conflicting evidence,
    // never silently resolved in either source's favour.
    let other = EvidenceObservation {
        provenance: Provenance {
            channel: EvidenceChannel::LocalRedump,
            upstream_source: SourceFamily::Redump,
            upstream_version: None,
            source_artifact: None,
            imported_at_unix: None,
            retrieved_at_unix: None,
            generator_version: None,
            lineage: LineageRelation::Independent,
            representation: Representation::PhysicalFile,
        },
        claim: ClaimType::ExactBytesMatch,
        claim_strength: ClaimStrength::Strong,
        identity_scope: IdentityScope::DumpIdentity,
        hash_or_value: Some("999999999999999999999999999999999999999f".to_string()),
        platform_candidate: None,
        release_candidate: Some("pacman".to_string()),
        notes: None,
    };
    let summaries = merge_evidence(&[mame, other]);
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].status.is_conflict());
}

// ---------------------------------------------------------------------
// 9. MAME-derived Redump/CHD evidence still follows the existing derived-
//    lineage rules - this module itself only ever emits independent
//    SourceFamily::MAMEArcade evidence, never SourceFamily::MAMERedump.
// ---------------------------------------------------------------------

#[test]
fn disk_is_logical_chd_only() {
    let imported = source();
    let observations = observations_from_mame_listxml_disk_matches(&imported, DISK);
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].claim, ClaimType::ExactLogicalDiscMatch);
    assert_eq!(
        observations[0].provenance.representation,
        Representation::LogicalChd
    );
    assert_eq!(
        observations[0].provenance.upstream_source,
        SourceFamily::MAMEArcade
    );
    assert_ne!(
        observations[0].provenance.upstream_source,
        SourceFamily::MAMERedump
    );
}

#[test]
fn mame_arcade_evidence_never_becomes_derived_mameredump_evidence() {
    let imported = source();
    let arcade = observations_from_mame_listxml_disk_matches(&imported, DISK)
        .pop()
        .unwrap();
    assert_eq!(arcade.provenance.lineage, LineageRelation::Independent);

    // A hand-constructed MAMERedump-derived observation for the *same*
    // claim/value, as `mame_redump_bridge` would produce it - kept inline
    // here (rather than calling that module) so this test stays scoped to
    // proving the lineage-mixing property, not re-deriving CHD identity.
    let derived = EvidenceObservation {
        provenance: Provenance {
            channel: EvidenceChannel::LocalMame,
            upstream_source: SourceFamily::MAMERedump,
            upstream_version: None,
            source_artifact: Some(SourceArtifactIdentity {
                source_family: SourceFamily::MAMERedump,
                upstream_version: None,
                artifact_sha256: None,
                artifact_name: None,
            }),
            imported_at_unix: None,
            retrieved_at_unix: None,
            generator_version: None,
            lineage: LineageRelation::DerivedFrom,
            representation: Representation::LogicalChd,
        },
        claim: ClaimType::ExactLogicalDiscMatch,
        claim_strength: ClaimStrength::Strong,
        identity_scope: IdentityScope::DumpIdentity,
        hash_or_value: arcade.hash_or_value.clone(),
        platform_candidate: None,
        release_candidate: arcade.release_candidate.clone(),
        notes: None,
    };

    let summaries = merge_evidence(&[arcade, derived]);
    assert_eq!(summaries.len(), 1);
    // MAMEArcade and MAMERedump are genuinely distinct evidence lanes (MAME
    // listxml evidence is never itself Redump-derived), so both still count
    // as separate lanes here - but the MAMERedump side is explicitly a
    // derivative of Redump, not a second independent preservation vote, so
    // this must classify as derived agreement, never plain independent
    // agreement.
    assert_eq!(summaries[0].status, AgreementStatus::DerivedAgreement);
    assert_eq!(
        independent_source_group_count(&summaries[0].observations),
        2
    );
}

// ---------------------------------------------------------------------
// Hash strength, status handling, malformed input, and scale (unchanged
// from the original ingestion work; kept to guard the parser/convert
// boundary together).
// ---------------------------------------------------------------------

#[test]
fn hash_strength_status_and_lineage_are_truthful() {
    let imported = source();
    let observations =
        observations_from_mame_listxml_matches(&imported, ChecksumAlgorithm::Sha1, SHA);
    assert_eq!(observations[0].claim_strength, ClaimStrength::Strong);
    assert_eq!(
        observations[0].provenance.upstream_source,
        SourceFamily::MAMEArcade
    );
    assert_eq!(
        observations[0].provenance.lineage,
        LineageRelation::Independent
    );
    assert_eq!(
        observations[0].provenance.representation,
        Representation::PhysicalFile
    );

    assert_eq!(
        observations_from_mame_listxml_matches(
            &imported,
            ChecksumAlgorithm::Md5,
            "22222222222222222222222222222222"
        )[0]
        .claim_strength,
        ClaimStrength::Strong
    );
    assert_eq!(
        observations_from_mame_listxml_matches(&imported, ChecksumAlgorithm::Crc32, "aaaaaaaa")[0]
            .claim_strength,
        ClaimStrength::Corroborated
    );
    assert!(
        observations_from_mame_listxml_matches(
            &imported,
            ChecksumAlgorithm::Sha1,
            "444444444444444444444444444444444444444d"
        )
        .is_empty()
    );
    assert_eq!(
        observations_from_mame_listxml_matches(
            &imported,
            ChecksumAlgorithm::Sha1,
            "333333333333333333333333333333333333333c"
        )[0]
        .claim_strength,
        ClaimStrength::Corroborated
    );
}

#[test]
fn malformed_xml_fails_closed() {
    let dir = tempdir().unwrap();
    assert!(import_mame_listxml(&write(dir.path(), "bad.xml", "<mame><machine")).is_err());
}

#[test]
fn missing_machine_name_is_skipped_with_warning() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "missing.xml",
        "<mame><machine><rom name=\"x\" crc=\"AAAAAAAA\"/></machine></mame>",
    );
    let outcome = parse_dat_file(&path, Default::default()).unwrap();
    assert!(outcome.dat.games.is_empty());
    assert!(
        outcome
            .dat
            .source
            .parse_warnings
            .iter()
            .any(|warning| warning.contains("machine without name skipped"))
    );
}

#[test]
fn contradictory_nodump_and_hash_has_no_identity_evidence() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "nodump.xml",
        "<mame><machine name=\"contradiction\"><rom name=\"x.bin\" sha1=\"111111111111111111111111111111111111111a\" status=\"nodump\"/></machine></mame>",
    );
    let imported = import_mame_listxml(&path).unwrap();
    assert_eq!(imported.index.lookup_sha1(SHA).len(), 1);
    assert!(
        observations_from_mame_listxml_matches(&imported, ChecksumAlgorithm::Sha1, SHA).is_empty()
    );
}

#[test]
fn generated_many_machines_streams_without_dom_model() {
    let mut xml = String::from("<mame>");
    for index in 0..5_000 {
        xml.push_str(&format!(
            "<machine name=\"m{index}\"><rom name=\"r{index}\" crc=\"AAAAAAAA\"/></machine>"
        ));
    }
    xml.push_str("</mame>");
    let dir = tempdir().unwrap();
    let imported = import_mame_listxml(&write(dir.path(), "many.xml", &xml)).unwrap();
    assert_eq!(imported.dat.games.len(), 5_000);
}

// ---------------------------------------------------------------------
// 10. Existing MAME Software List behavior is unaffected by this module's
//     presence (a direct spot check; the full regression is the separate
//     `mame_software_list` test module run during validation).
// ---------------------------------------------------------------------

#[test]
fn software_list_ecosystem_and_evidence_are_unaffected_by_mame_arcade_existing() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "softlist.xml",
        r#"<softwarelist name="neogeo" description="SNK Neo Geo"><software name="pbobbl2n"><description>Puzzle Bobble 2 (NGM-2120)</description><rom name="pbobbl2n.rom" size="1" crc="aaaaaaaa"/></software></softwarelist>"#,
    );
    let dat = parse_dat_file(&path, Default::default()).unwrap().dat;
    assert_eq!(dat.source.ecosystem, DatEcosystem::MAMESoftwareList);
    // `<software name>` remains excluded from machine-shortname evidence,
    // exactly as before this module existed.
    assert!(matches!(
        identify_dat_source(&dat),
        DatPlatformIdentity::Unknown
    ));
}
