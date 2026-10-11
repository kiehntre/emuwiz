//! Independent-review reproductions using private XML and checksum observations.
use archivefs_core::arcade_mame_compatibility::*;
use archivefs_core::dat::dependency::{DependencyKind, DependencyTarget};
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::model::ParsedDat;
use archivefs_core::dat::parsers::mame_listxml::parse_mame_listxml;
use archivefs_core::identity_source::mame_listxml::import_mame_listxml;

fn metadata(xml: &str) -> ParsedDat {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("listxml.xml");
    std::fs::write(&path, format!("<mame build=\"review\">{xml}</mame>")).unwrap();
    parse_mame_listxml(&path, DatLimits::default()).unwrap().dat
}
fn pin() -> InstalledMameEvidence {
    InstalledMameEvidence::new(
        "/private/mame",
        "review",
        "/private/listxml",
        [],
        None,
        None,
    )
}
fn rom(set: &str, name: &str, crc: &str) -> MameObservedRom {
    MameObservedRom {
        set_name: set.into(),
        name: name.into(),
        size_bytes: Some(13),
        crc32: Some(crc.into()),
        sha1: None,
    }
}
fn evidence(
    roms: Vec<MameObservedRom>,
    completeness: ObservedEvidenceCompleteness,
) -> ObservedArcadeSetEvidence {
    let mut e = ObservedArcadeSetEvidence::new("variant", completeness);
    e.observed_set_names = roms.iter().map(|r| r.set_name.clone()).collect();
    e.observed_roms = roms;
    e
}
fn layouts() -> [MameCollectionLayout; 3] {
    [
        MameCollectionLayout::Split,
        MameCollectionLayout::Merged,
        MameCollectionLayout::NonMerged,
    ]
}
fn assert_layouts(
    dat: &ParsedDat,
    e: &ObservedArcadeSetEvidence,
    state: MameSetCompatibilityState,
) {
    let index = MameExpectationIndex::new(dat);
    for layout in layouts() {
        let result = e.audit(
            &pin(),
            index.expectation_for_layout("variant", layout).unwrap(),
        );
        assert_eq!(
            result.state, state,
            "layout={layout:?}; {:?}",
            result.mismatches
        );
    }
    let alternatives = e.audit_alternatives(&pin(), &index.alternatives_for("variant").unwrap());
    assert_eq!(alternatives.state, state, "{:?}", alternatives.mismatches);
}

#[test]
fn definite_crc_mismatch_survives_unresolved_dependencies_and_partial_observations() {
    for relationship in ["cloneof", "romof"] {
        let dat = metadata(&format!(
            r#"<machine name="variant" {relationship}="absent"><rom name="main" size="13" crc="1a2b3c4d"/></machine>"#
        ));
        for completeness in [
            ObservedEvidenceCompleteness::Complete,
            ObservedEvidenceCompleteness::Partial,
        ] {
            let e = evidence(vec![rom("variant", "main", "ffffffff")], completeness);
            assert_layouts(&dat, &e, MameSetCompatibilityState::Incompatible);
        }
    }
}

#[test]
fn uncertain_dependencies_and_unobserved_files_are_not_definite_incompatibility() {
    let dat = metadata(
        r#"<machine name="variant" romof="absent"><rom name="main" size="13" crc="1a2b3c4d"/></machine>"#,
    );
    for roms in [vec![], vec![rom("variant", "main", "1a2b3c4d")]] {
        assert_layouts(
            &dat,
            &evidence(roms, ObservedEvidenceCompleteness::Partial),
            MameSetCompatibilityState::Unknown,
        );
    }
}

#[test]
fn unknown_scan_completeness_does_not_hide_observed_wrong_bytes() {
    let dat = metadata(
        r#"<machine name="variant"><rom name="main" size="13" crc="1a2b3c4d"/></machine>"#,
    );
    for completeness in [
        ObservedEvidenceCompleteness::Unknown,
        ObservedEvidenceCompleteness::NotGathered,
    ] {
        assert_layouts(
            &dat,
            &evidence(vec![rom("variant", "main", "ffffffff")], completeness),
            MameSetCompatibilityState::Incompatible,
        );
        assert_layouts(
            &dat,
            &evidence(vec![], completeness),
            MameSetCompatibilityState::Unknown,
        );
    }
}

const DEVICE: &str = r#"<machine name="variant" romof="firmware"><rom name="main" size="13" crc="1a2b3c4d"/><device_ref name="controller"/></machine>
<machine name="firmware" isbios="yes"><rom name="boot" size="13" crc="3c4d5e6f"/></machine>
<machine name="controller" isdevice="yes"><rom name="ucode" size="13" crc="2b3c4d5e"/></machine>"#;
fn game_and_bios() -> Vec<MameObservedRom> {
    vec![
        rom("variant", "main", "1a2b3c4d"),
        rom("firmware", "boot", "3c4d5e6f"),
    ]
}

#[test]
fn device_rom_can_be_proven_in_system_bios_without_changing_owner() {
    let dat = metadata(DEVICE);
    let mut roms = game_and_bios();
    roms.push(rom("firmware", "ucode", "2b3c4d5e"));
    let e = evidence(roms, ObservedEvidenceCompleteness::Complete);
    assert_layouts(&dat, &e, MameSetCompatibilityState::Compatible);
    let expectation = MameExpectationIndex::new(&dat)
        .expectation_for("variant")
        .unwrap();
    let device = expectation
        .dependencies
        .resolution
        .roms
        .iter()
        .find(|r| r.member_name == "ucode")
        .unwrap();
    assert_eq!(device.supporting_kind, Some(DependencyKind::Device));
    assert_eq!(
        device.owner,
        Some(DependencyTarget::SetMember {
            set_name: "controller".into(),
            member_name: "ucode".into()
        })
    );
    let result = e.audit(&pin(), expectation);
    assert!(
        result
            .verified_roms
            .iter()
            .any(|r| r.declaring_set == "controller" && r.observed.set_name == "firmware")
    );
}

#[test]
fn device_missing_wrong_digest_and_unrelated_archive_remain_incompatible() {
    let dat = metadata(DEVICE);
    for extra in [
        None,
        Some(rom("firmware", "ucode", "ffffffff")),
        Some(rom("unrelated", "ucode", "2b3c4d5e")),
    ] {
        let mut roms = game_and_bios();
        roms.extend(extra);
        assert_layouts(
            &dat,
            &evidence(roms, ObservedEvidenceCompleteness::Complete),
            MameSetCompatibilityState::Incompatible,
        );
    }
}

#[test]
fn bad_dump_does_not_validate_an_empty_or_whitespace_merge() {
    for merge in ["", "   "] {
        let dat = metadata(&format!(
            r#"<machine name="variant"><rom name="main" status="baddump" merge="{merge}" size="13" crc="1a2b3c4d"/></machine>"#
        ));
        assert_layouts(
            &dat,
            &evidence(
                vec![rom("variant", "main", "1a2b3c4d")],
                ObservedEvidenceCompleteness::Complete,
            ),
            MameSetCompatibilityState::Unknown,
        );
    }
}

#[test]
fn optional_bad_dump_cannot_hide_malformed_merge() {
    let dat = metadata(
        r#"<machine name="variant"><rom name="main" status="baddump" optional="yes" merge="" size="13" crc="1a2b3c4d"/></machine>"#,
    );
    assert_layouts(
        &dat,
        &evidence(vec![], ObservedEvidenceCompleteness::Complete),
        MameSetCompatibilityState::Unknown,
    );
}

#[test]
fn absent_valid_and_contradictory_merge_are_distinct_from_dump_status() {
    let absent = metadata(
        r#"<machine name="variant"><rom name="main" status="baddump" size="13" crc="1a2b3c4d"/></machine>"#,
    );
    assert_layouts(
        &absent,
        &evidence(
            vec![rom("variant", "main", "1a2b3c4d")],
            ObservedEvidenceCompleteness::Complete,
        ),
        MameSetCompatibilityState::CompatibleWithWarnings,
    );
    for (owner_crc, expected) in [
        (
            "1a2b3c4d",
            MameSetCompatibilityState::CompatibleWithWarnings,
        ),
        ("ffffffff", MameSetCompatibilityState::Unknown),
    ] {
        let dat = metadata(&format!(
            r#"<machine name="variant" romof="bank"><rom name="main" merge="shared" status="baddump" size="13" crc="1a2b3c4d"/></machine><machine name="bank"><rom name="shared" size="13" crc="{owner_crc}"/></machine>"#
        ));
        assert_layouts(
            &dat,
            &evidence(
                vec![rom("variant", "main", "1a2b3c4d")],
                ObservedEvidenceCompleteness::Complete,
            ),
            expected,
        );
    }
}

fn erase_old_parser_facts(dat: &ParsedDat) -> ParsedDat {
    let mut json = serde_json::to_value(dat).unwrap();
    for game in json["games"].as_array_mut().unwrap() {
        game.as_object_mut()
            .unwrap()
            .remove("mame_listxml_schema_version");
        if game["clone_of"] == "" {
            game["clone_of"] = serde_json::Value::Null;
        }
        for rom in game["roms"].as_array_mut().unwrap() {
            if rom["merge"] == "" {
                rom["merge"] = serde_json::Value::Null;
            }
        }
        game["bios_sets"] = serde_json::json!([]);
    }
    serde_json::from_value(json).unwrap()
}

#[test]
fn old_parsed_cache_is_stale_even_when_evidence_claims_schema_two() {
    for xml in [
        r#"<machine name="variant" cloneof=""><rom name="main" size="13" crc="1a2b3c4d"/></machine>"#,
        r#"<machine name="variant"><rom name="main" merge="" size="13" crc="1a2b3c4d"/></machine>"#,
    ] {
        let old = erase_old_parser_facts(&metadata(xml));
        for schema in [1, 2] {
            let mut mame = pin();
            mame.parser_schema_version = schema;
            let result = evidence(
                vec![rom("variant", "main", "1a2b3c4d")],
                ObservedEvidenceCompleteness::Complete,
            )
            .audit(
                &mame,
                MameExpectationIndex::new(&old)
                    .expectation_for("variant")
                    .unwrap(),
            );
            assert_eq!(result.state, MameSetCompatibilityState::Unknown);
            assert!(
                result
                    .mismatches
                    .iter()
                    .any(|m| m.detail.to_lowercase().contains("stale")
                        && m.detail.to_lowercase().contains("reimport")),
                "{:?}",
                result.mismatches
            );
            assert_ne!(
                result.mame.parser_schema_version, 2,
                "stale input must not be relabelled current"
            );
        }
    }
}

#[test]
fn existing_importer_reparses_retained_xml_to_recover_bios_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retained.xml");
    let xml = r#"<mame build="review"><machine name="variant"><biosset name="stock" default="yes"/><biosset name="alternate"/><rom name="main" bios="stock" size="13" crc="1a2b3c4d"/><rom name="alternate" bios="alternate" size="13" crc="2b3c4d5e"/></machine></mame>"#;
    std::fs::write(&path, xml).unwrap();
    let fresh = import_mame_listxml(&path).unwrap();
    let stale = erase_old_parser_facts(&fresh.dat);
    let e = evidence(
        vec![rom("variant", "main", "1a2b3c4d")],
        ObservedEvidenceCompleteness::Complete,
    );
    let old_result = e.audit(
        &pin(),
        MameExpectationIndex::new(&stale)
            .expectation_for("variant")
            .unwrap(),
    );
    assert_eq!(old_result.state, MameSetCompatibilityState::Unknown);
    assert!(
        old_result
            .mismatches
            .iter()
            .any(|m| m.detail.to_lowercase().contains("stale"))
    );
    let pin =
        InstalledMameEvidence::from_imported("/private/mame", "review", &path, [], &fresh, None);
    let new_result = e.audit(
        &pin,
        MameExpectationIndex::new(&fresh.dat)
            .expectation_for("variant")
            .unwrap(),
    );
    assert_eq!(new_result.state, MameSetCompatibilityState::Compatible);
    assert_eq!(new_result.mame.parser_schema_version, 2);
}

#[test]
fn stale_support_metadata_does_not_hide_current_subjects_wrong_bytes() {
    let mut dat = metadata(
        r#"<machine name="variant" cloneof="family"><rom name="main" size="13" crc="1a2b3c4d"/></machine><machine name="family"/>"#,
    );
    let mut value = serde_json::to_value(&dat).unwrap();
    value["games"][1]
        .as_object_mut()
        .unwrap()
        .remove("mame_listxml_schema_version");
    dat = serde_json::from_value(value).unwrap();
    assert_layouts(
        &dat,
        &evidence(
            vec![rom("variant", "main", "ffffffff")],
            ObservedEvidenceCompleteness::Complete,
        ),
        MameSetCompatibilityState::Incompatible,
    );
    assert_layouts(
        &dat,
        &evidence(
            vec![rom("variant", "main", "1a2b3c4d")],
            ObservedEvidenceCompleteness::Complete,
        ),
        MameSetCompatibilityState::Unknown,
    );
}

#[test]
fn duplicate_current_declarations_are_ambiguous_rather_than_stale() {
    let dat = metadata(r#"<machine name="variant"/><machine name="variant"/>"#);
    let result = evidence(vec![], ObservedEvidenceCompleteness::Complete).audit(
        &pin(),
        MameExpectationIndex::new(&dat)
            .expectation_for("variant")
            .unwrap(),
    );
    assert_eq!(result.state, MameSetCompatibilityState::Unknown);
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.detail.contains("Ambiguous"))
    );
    assert!(
        !result
            .mismatches
            .iter()
            .any(|m| m.detail.to_lowercase().contains("stale"))
    );
}

#[test]
fn fresh_cache_round_trip_preserves_provenance_without_relabelling_old_pins() {
    let fresh = metadata(
        r#"<machine name="variant"><rom name="main" size="13" crc="1a2b3c4d"/></machine>"#,
    );
    let dat: ParsedDat = serde_json::from_slice(&serde_json::to_vec(&fresh).unwrap()).unwrap();
    assert_eq!(dat.games[0].mame_listxml_schema_version, Some(2));
    let e = evidence(
        vec![rom("variant", "main", "1a2b3c4d")],
        ObservedEvidenceCompleteness::Complete,
    );
    for schema in [0, 1, 2, 3] {
        let mut pin = pin();
        pin.parser_schema_version = schema;
        let result = e.audit(
            &pin,
            MameExpectationIndex::new(&dat)
                .expectation_for("variant")
                .unwrap(),
        );
        if schema == 0 || schema == 2 {
            assert_eq!(result.state, MameSetCompatibilityState::Compatible);
            assert_eq!(result.mame.parser_schema_version, 2);
        } else {
            assert_eq!(result.state, MameSetCompatibilityState::Unknown);
            assert_eq!(result.mame.parser_schema_version, schema);
            assert!(
                result
                    .mismatches
                    .iter()
                    .any(|m| m.reason == MameMismatchReason::StaleMetadata)
            );
        }
    }
}

#[test]
fn incomplete_disk_identity_is_unknown_but_observed_wrong_identity_is_incompatible() {
    let dat = metadata(
        r#"<machine name="variant"><disk name="disk" sha1="1111111111111111111111111111111111111111"/></machine>"#,
    );
    for completeness in [
        ObservedEvidenceCompleteness::Unknown,
        ObservedEvidenceCompleteness::Partial,
        ObservedEvidenceCompleteness::Complete,
    ] {
        for (sha1, expected) in [
            (None, MameSetCompatibilityState::Unknown),
            (
                Some("ffffffffffffffffffffffffffffffffffffffff"),
                MameSetCompatibilityState::Incompatible,
            ),
        ] {
            let mut e = evidence(vec![], completeness);
            e.observed_disks.push(MameObservedDisk {
                set_name: "variant".into(),
                name: Some("disk".into()),
                sha1: sha1.map(str::to_owned),
            });
            assert_layouts(&dat, &e, expected);
        }
    }
}
