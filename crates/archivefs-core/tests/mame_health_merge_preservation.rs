//! Tiny parsed listxml catalogues and supplied checksum facts only; no ROM IO.
use archivefs_core::arcade_mame_compatibility::*;
use archivefs_core::dat::model::ParsedDat;
use archivefs_core::dat::parsers::mame_listxml::parse_mame_listxml;
use archivefs_core::mame_collection_analyser::*;

fn catalogue(machines: &str) -> ParsedDat {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalogue.xml");
    std::fs::write(&path, format!("<mame build=\"0.280\">{machines}</mame>")).unwrap();
    parse_mame_listxml(&path, Default::default()).unwrap().dat
}
fn member(name: &str) -> MameObservedMember {
    MameObservedMember {
        name: name.into(),
        size_bytes: Some(4),
        crc32: Some("aaaaaaaa".into()),
        md5: None,
        sha1: Some("a".repeat(40)),
    }
}
fn inventory(set: &str, name: &str) -> MameCollectionInventory {
    MameCollectionInventory {
        inspected_completely: true,
        sets: vec![MameObservedSet {
            name: set.into(),
            location: format!("/synthetic/{set}.zip").into(),
            directory: false,
            members: vec![member(name)],
        }],
        ..Default::default()
    }
}
fn borrowed(status: &str, intermediate_bad: bool) -> ParsedDat {
    let provider = if intermediate_bad { "middle" } else { "parent" };
    let middle = if intermediate_bad {
        r#"<machine name="middle" romof="parent"><rom name="alias.bin" merge="base.bin" size="4" crc="aaaaaaaa" status="baddump"/></machine>"#
    } else {
        ""
    };
    let target = if intermediate_bad {
        "alias.bin"
    } else {
        "base.bin"
    };
    catalogue(&format!(
        r#"<machine name="child" cloneof="parent" romof="{provider}"><rom name="child.bin" merge="{target}" size="4" crc="aaaaaaaa" sha1="{}"/></machine><machine name="parent"><rom name="base.bin" size="4" crc="aaaaaaaa" status="{status}"/></machine>{middle}"#,
        "a".repeat(40)
    ))
}
fn child_result(dat: &ParsedDat, inv: &MameCollectionInventory) -> MameSetResult {
    analyse_collection(dat, inv, None, None)
        .set_results
        .into_iter()
        .find(|r| r.name == "child")
        .unwrap()
}
#[test]
fn borrowed_bad_dump_is_not_clean_health_in_any_supported_layout() {
    for intermediate in [false, true] {
        let dat = borrowed(if intermediate { "good" } else { "baddump" }, intermediate);
        for (set, name) in [
            ("parent", "base.bin"),
            ("parent", "child/child.bin"),
            ("child", "child.bin"),
        ] {
            let result = child_result(&dat, &inventory(set, name));
            assert_eq!(
                result.health,
                MameSetHealth::Unknown,
                "{intermediate}/{set}/{name}: {result:?}"
            );
            assert!(!result.evidence.verified_complete());
            assert_eq!(result.evidence.runnability, MameRunnability::NotAssessed);
            assert_eq!(
                result.evidence.requirements[0].preservation,
                MamePreservationStatus::NeedsRedump
            );
            assert!(result.evidence.unresolved_dependencies.is_empty());
        }
    }
}
#[test]
fn resolver_retains_bad_dump_warning_and_original_declarations() {
    let dat = borrowed("baddump", false);
    let index = MameExpectationIndex::new(&dat);
    for layout in [
        MameCollectionLayout::Split,
        MameCollectionLayout::Merged,
        MameCollectionLayout::NonMerged,
    ] {
        let e = index.expectation_for_layout("child", layout).unwrap();
        assert_eq!(e.roms[0].status.as_deref(), Some("baddump"));
        let declarations = &e.dependencies.resolution.roms[0].declarations;
        assert_eq!(declarations[0].status, None);
        assert_eq!(declarations[1].status.as_deref(), Some("baddump"));
    }
    let mut facts = ObservedArcadeSetEvidence::new("child", ObservedEvidenceCompleteness::Complete);
    facts.observed_set_names = vec!["parent".into()];
    facts.observed_roms = vec![MameObservedRom {
        set_name: "parent".into(),
        name: "base.bin".into(),
        size_bytes: Some(4),
        crc32: Some("aaaaaaaa".into()),
        sha1: Some("a".repeat(40)),
    }];
    let installed = InstalledMameEvidence::new(
        "/synthetic/mame",
        "0.280",
        "/synthetic/listxml",
        [],
        Some("fixture".into()),
        None,
    );
    let result = facts.audit_alternatives(&installed, &index.alternatives_for("child").unwrap());
    assert_eq!(
        result.state,
        MameSetCompatibilityState::CompatibleWithWarnings
    );
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::BadDump)
    );
}
#[test]
fn clean_explicit_merge_remains_verified_in_each_layout() {
    let dat = borrowed("good", false);
    for (set, name) in [
        ("parent", "base.bin"),
        ("parent", "child/child.bin"),
        ("child", "child.bin"),
    ] {
        let result = child_result(&dat, &inventory(set, name));
        assert_eq!(
            result.health,
            MameSetHealth::Good,
            "{set}/{name}: {result:?}"
        );
        assert!(result.evidence.verified_complete());
        assert_eq!(result.evidence.runnability, MameRunnability::NotAssessed);
    }
}
#[test]
fn unrelated_parent_bad_dump_is_not_inherited_without_member_merge() {
    let mut dat = borrowed("baddump", false);
    dat.games
        .iter_mut()
        .find(|g| g.name == "child")
        .unwrap()
        .roms[0]
        .merge = None;
    let result = child_result(&dat, &inventory("child", "child.bin"));
    assert_eq!(result.health, MameSetHealth::Good);
    assert_eq!(
        result.evidence.requirements[0].preservation,
        MamePreservationStatus::VerifiedDumpExpected
    );
}
#[test]
fn absent_and_wrong_borrowed_bytes_are_not_clean_or_acquisition_opportunities() {
    let dat = borrowed("baddump", false);
    for missing in [true, false] {
        let mut inv = inventory("child", "child.bin");
        if missing {
            inv.sets[0].members.clear();
        } else {
            inv.sets[0].members[0].sha1 = Some("b".repeat(40));
        }
        let report = analyse_collection(&dat, &inv, None, None);
        let result = report
            .set_results
            .iter()
            .find(|r| r.name == "child")
            .unwrap();
        assert_eq!(result.health, MameSetHealth::Bad);
        assert!(!report.top_missing_dependencies.is_empty());
        assert!(
            report
                .top_missing_dependencies
                .iter()
                .filter(|r| r.affected_sets.contains(&"child".into()))
                .all(|r| !r.obtainable_or_expected
                    && r.preservation_status == MamePreservationStatus::NeedsRedump)
        );
    }
}

#[test]
fn old_clean_health_reports_require_reanalysis_before_playing_library_selection() {
    use archivefs_core::mame_playing_library::*;
    let dat = borrowed("good", false);
    let inv = inventory("child", "child.bin");
    let mut report = analyse_collection(&dat, &inv, None, None);
    let preferences = MamePlayingLibraryPreferences {
        include_imperfect: true,
        ..Default::default()
    };
    let plan = |report: &MameCollectionAnalysis| {
        build_mame_playing_library_plan(&MamePlayingLibraryRequest {
            catalogue: &dat,
            inventory: &inv,
            analysis: report,
            preferences: preferences.clone(),
        })
        .unwrap()
    };
    assert_eq!(plan(&report).selected_sets.len(), 1);
    // Schema 2 could certify the merge-provider BAD_DUMP defect as clean.
    // Serialized evidence cannot reconstruct the omitted provider status.
    report.schema_version = 2;
    assert!(plan(&report).selected_sets.is_empty());
}
