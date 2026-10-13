//! Narrow regressions for independent review findings F1/F2.
use super::*;
use crate::dat::model::DatEcosystem;
use crate::diagnostics::arcade_dat_version::MameVersion;

fn catalogue(roms: &str, build: &str) -> ParsedDat {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.xml");
    std::fs::write(
        &path,
        format!(r#"<mame build="{build}"><machine name="g">{roms}</machine></mame>"#),
    )
    .unwrap();
    crate::dat::parsers::mame_listxml::parse_mame_listxml(&path, Default::default())
        .unwrap()
        .dat
}
fn rom(status: &str, hash: char) -> String {
    let sha = if hash == '-' {
        String::new()
    } else {
        format!(r#"sha1="{}""#, hash.to_string().repeat(40))
    };
    format!(r#"<rom name="main.bin" size="1" status="{status}" {sha}/>"#)
}
fn inventory() -> MameCollectionInventory {
    MameCollectionInventory {
        inspected_completely: true,
        sets: vec![MameObservedSet {
            name: "g".into(),
            location: "/synthetic/g.zip".into(),
            directory: false,
            members: vec![MameObservedMember {
                name: "main.bin".into(),
                size_bytes: Some(1),
                crc32: None,
                md5: None,
                sha1: Some("a".repeat(40)),
            }],
        }],
        ..Default::default()
    }
}
fn reports_both_orders(first: &str, second: &str) -> MameCollectionAnalysis {
    let dat = catalogue(&format!("{first}{second}"), "0.280");
    let mut reversed = dat.clone();
    reversed.games[0].roms.reverse();
    let inv = inventory();
    let a = analyse_collection(&dat, &inv, None, None);
    let b = analyse_collection(&reversed, &inv, None, None);
    assert_eq!(a, b);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
    a
}
#[test]
fn duplicate_preservation_retains_nodump_and_baddump_without_acquisition() {
    for (status, hash, preservation) in [
        ("nodump", '-', MamePreservationStatus::NoVerifiedDumpExists),
        ("baddump", 'a', MamePreservationStatus::NeedsRedump),
    ] {
        let report = reports_both_orders(&rom("good", 'a'), &rom(status, hash));
        let result = &report.set_results[0];
        assert_eq!(result.health, MameSetHealth::Unknown);
        assert!(!result.evidence.unresolved_dependencies.is_empty());
        assert!(
            result
                .evidence
                .requirements
                .iter()
                .any(|r| r.preservation == preservation)
        );
        assert!(!report.top_missing_dependencies.is_empty());
        assert!(
            report
                .top_missing_dependencies
                .iter()
                .all(|r| !r.obtainable_or_expected && r.potentially_repairs_sets == 0)
        );
        assert_eq!(
            report.top_missing_dependencies[0].preservation_status,
            preservation
        );
    }
}
#[test]
fn duplicate_conflicting_identity_or_unsupported_status_is_unresolved() {
    for second in [
        rom("good", 'b'),
        rom("unexpected", 'a'),
        rom("", 'a'),
        rom("good", 'a').replace("size=\"1\"", "size=\"2\""),
    ] {
        let report = reports_both_orders(&rom("good", 'a'), &second);
        assert_eq!(report.set_results[0].health, MameSetHealth::Unknown);
        assert!(
            !report.set_results[0]
                .evidence
                .unresolved_dependencies
                .is_empty()
        );
        assert!(
            report
                .top_missing_dependencies
                .iter()
                .all(|r| !r.obtainable_or_expected && r.potentially_repairs_sets == 0)
        );
    }
}
#[test]
fn unsupported_or_unresolved_metadata_never_advertises_acquisition() {
    let mut inv = inventory();
    inv.sets[0].members.clear();
    for value in [
        rom("", 'a'),
        rom("unexpected", 'a'),
        format!("{}{}", rom("good", 'a'), rom("nodump", '-')),
    ] {
        let dat = catalogue(&value, "0.280");
        let report = analyse_collection(&dat, &inv, None, None);
        assert_ne!(report.set_results[0].health, MameSetHealth::Good);
        assert!(
            report
                .top_missing_dependencies
                .iter()
                .all(|r| !r.obtainable_or_expected && r.potentially_repairs_sets == 0)
        );
    }
    let mut dat = catalogue(&rom("good", 'a'), "0.280");
    dat.games[0].rom_of = Some("unknown-provider".into());
    let report = analyse_collection(&dat, &inv, None, None);
    assert!(
        report
            .top_missing_dependencies
            .iter()
            .all(|r| !r.obtainable_or_expected && r.potentially_repairs_sets == 0)
    );
}

#[test]
fn identical_duplicate_declarations_remain_truthful() {
    for (status, hash, health) in [
        ("good", 'a', MameSetHealth::Good),
        ("nodump", '-', MameSetHealth::Unknown),
        ("baddump", 'a', MameSetHealth::Unknown),
    ] {
        let value = rom(status, hash);
        let report = reports_both_orders(&value, &value);
        assert_eq!(report.set_results[0].health, health);
        assert!(
            report.set_results[0]
                .evidence
                .unresolved_dependencies
                .is_empty()
        );
        assert_eq!(report.set_results[0].expected_members, 1);
        assert!(
            report
                .top_missing_dependencies
                .iter()
                .all(|r| !r.obtainable_or_expected)
        );
    }
    let default_status = rom("", 'a').replace(" status=\"\"", "");
    let report = reports_both_orders(&rom("good", 'a'), &default_status);
    assert_eq!(report.set_results[0].health, MameSetHealth::Good);
}
#[test]
fn conflicting_preservation_statuses_stay_visible_in_canonical_receipts() {
    let report = reports_both_orders(&rom("nodump", '-'), &rom("baddump", 'a'));
    let statuses: Vec<_> = report.set_results[0]
        .evidence
        .requirements
        .iter()
        .map(|r| r.preservation)
        .collect();
    assert!(statuses.contains(&MamePreservationStatus::NoVerifiedDumpExists));
    assert!(statuses.contains(&MamePreservationStatus::NeedsRedump));
    assert!(
        report
            .top_missing_dependencies
            .iter()
            .all(|r| !r.obtainable_or_expected && r.potentially_repairs_sets == 0)
    );
}
#[test]
fn arbitrary_unicode_version_evidence_never_panics() {
    let dat = catalogue(&rom("good", 'a'), "0.280");
    let inv = inventory();
    for ch in [
        'é',
        '🦀',
        '中',
        '\u{0301}',
        '\u{2028}',
        '\u{0085}',
        '\0',
        '\u{10ffff}',
    ] {
        for text in [
            format!("0.280{ch}"),
            format!("MAME {ch}0.280"),
            format!("0.280u{ch}"),
            format!("{ch}0.280"),
        ] {
            // Also exercise the shared parser directly, not only the new boundary.
            let _ = MameVersion::parse(&text);
            let a = analyse_collection(&dat, &inv, Some(text.clone()), None);
            assert_eq!(
                a.version_compatibility,
                MameVersionCompatibility::ExactSourceVersionUnknown,
                "{text:?}"
            );
            let mut bad = dat.clone();
            bad.source.version = Some(text.clone());
            assert_eq!(
                analyse_collection(&bad, &inv, Some("0.280".into()), None).version_compatibility,
                MameVersionCompatibility::ExactSourceVersionUnknown,
                "{text:?}"
            );
        }
    }
    let xml = catalogue(&rom("good", 'a'), "0.280é");
    assert_eq!(
        analyse_collection(&xml, &inv, Some("0.280".into()), None).version_compatibility,
        MameVersionCompatibility::ExactSourceVersionUnknown
    );
}
#[test]
fn unrelated_and_ambiguous_versions_on_either_side_are_unknown() {
    let dat = catalogue(&rom("good", 'a'), "0.280");
    let inv = inventory();
    for text in [
        "DuckStation 0.280",
        "FBNeo 0.280",
        "MAME 0.280 0.279",
        "MAME 0.279 0.280",
        "MAME 0.280 0.280",
        "0.280 (mame0279)",
        "0.280 (DuckStation)",
        "MAME 0.280, DuckStation 0.280",
        "0.280 (mame0280-0.279)",
    ] {
        assert_eq!(
            analyse_collection(&dat, &inv, Some(text.into()), None).version_compatibility,
            MameVersionCompatibility::ExactSourceVersionUnknown,
            "{text}"
        );
        let mut bad = dat.clone();
        bad.source.version = Some(text.into());
        assert_eq!(
            analyse_collection(&bad, &inv, Some("0.280".into()), None).version_compatibility,
            MameVersionCompatibility::ExactSourceVersionUnknown,
            "{text}"
        );
    }
}
#[test]
fn catalogue_identity_requires_mame_provenance() {
    let dat = catalogue(&rom("good", 'a'), "0.280");
    let inv = inventory();
    for ecosystem in [
        DatEcosystem::FBNeo,
        DatEcosystem::NoIntro,
        DatEcosystem::GenericLogiqx,
    ] {
        let mut other = dat.clone();
        other.source.ecosystem = ecosystem;
        assert_eq!(
            analyse_collection(&other, &inv, Some("0.280".into()), None).version_compatibility,
            MameVersionCompatibility::ExactSourceVersionUnknown
        );
    }
    let mut stale = dat;
    stale.games[0].mame_listxml_schema_version = None;
    assert_eq!(
        analyse_collection(&stale, &inv, Some("0.280".into()), None).version_compatibility,
        MameVersionCompatibility::ExactSourceVersionUnknown
    );
}
#[test]
fn legitimate_release_beta_update_and_build_evidence_compare() {
    let inv = inventory();
    for (catalogue_version, installed) in [
        ("0.280", "0.280"),
        ("0.280 (mame0280)", "MAME v0.280"),
        (
            "0.270 (mame0270-154-gabddfb0404c-dirty)",
            "MAME version 0.270",
        ),
        ("0.106u2", "MAME 0.106u2"),
        ("0.37b16", "v0.37b16"),
    ] {
        let dat = catalogue(&rom("good", 'a'), catalogue_version);
        assert_eq!(
            analyse_collection(&dat, &inv, Some(installed.into()), None).version_compatibility,
            MameVersionCompatibility::CatalogueVersionMatch
        );
        assert_eq!(
            analyse_collection(&dat, &inv, Some("0.281".into()), None).version_compatibility,
            MameVersionCompatibility::CatalogueVersionMismatch
        );
    }
}
