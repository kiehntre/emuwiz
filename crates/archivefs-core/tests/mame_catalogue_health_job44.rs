//! Deterministic catalogue and collection evidence; no real collection IO.
use archivefs_core::dat::model::ParsedDat;
use archivefs_core::dat::parsers::mame_listxml::parse_mame_listxml;
use archivefs_core::mame_collection_analyser::*;

fn catalogue(machines: &str) -> ParsedDat {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalogue.xml");
    std::fs::write(&path, format!("<mame build=\"0.280\">{machines}</mame>")).unwrap();
    parse_mame_listxml(&path, Default::default()).unwrap().dat
}
const ROM: &str = r#"<rom name="main" size="1" sha1="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"/>"#;
const DISK: &str = r#"<disk name="hdd" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"/>"#;

fn inventory(disk_set: Option<&str>) -> MameCollectionInventory {
    let mut sets = vec![MameObservedSet {
        name: "child".into(),
        location: "/synthetic/child.zip".into(),
        directory: false,
        members: vec![MameObservedMember {
            name: "main".into(),
            size_bytes: Some(1),
            sha1: Some("a".repeat(40)),
            crc32: None,
            md5: None,
        }],
    }];
    if let Some(set) = disk_set {
        sets.push(MameObservedSet {
            name: set.into(),
            location: format!("/synthetic/{set}").into(),
            directory: true,
            members: vec![MameObservedMember {
                name: "hdd.chd".into(),
                // Even a generic file hash equal to the declared CHD hash is
                // not verified CHD content or a verified parent chain.
                sha1: Some("b".repeat(40)),
                size_bytes: None,
                crc32: None,
                md5: None,
            }],
        });
    }
    MameCollectionInventory {
        sets,
        inspected_completely: true,
        ..Default::default()
    }
}
fn child(report: &MameCollectionAnalysis) -> &MameSetResult {
    report
        .set_results
        .iter()
        .find(|r| r.name == "child")
        .unwrap()
}

#[test]
fn lineage_does_not_import_the_parents_unrelated_dependency_failures() {
    for unrelated in [
        r#"<device_ref name="absent"/>"#,
        r#"<device_ref name="parent"/>"#,
    ] {
        let dat = catalogue(&format!(
            r#"<machine name="child" cloneof="parent">{ROM}</machine><machine name="parent" romof="absent">{unrelated}</machine>"#
        ));
        let report = analyse_collection(&dat, &inventory(None), None, None);
        assert_eq!(child(&report).health, MameSetHealth::Good, "{report:?}");
        assert!(child(&report).evidence.unresolved_dependencies.is_empty());
    }
}

#[test]
fn a_parent_also_used_as_rom_source_keeps_its_dependency_failures() {
    let dat = catalogue(&format!(
        r#"<machine name="child" cloneof="parent" romof="parent">{ROM}</machine><machine name="parent" romof="absent"/>"#
    ));
    let report = analyse_collection(&dat, &inventory(None), None, None);
    assert_eq!(child(&report).health, MameSetHealth::Unknown);
    assert!(!child(&report).evidence.unresolved_dependencies.is_empty());
}

#[test]
fn parent_stored_chd_is_unverified_rather_than_missing() {
    for relationship in [r#"cloneof="parent""#, r#"romof="parent""#] {
        let dat = catalogue(&format!(
            r#"<machine name="child" {relationship}>{ROM}{DISK}</machine><machine name="parent">{DISK}</machine>"#
        ));
        let report = analyse_collection(&dat, &inventory(Some("parent")), None, None);
        let result = child(&report);
        assert_eq!(result.health, MameSetHealth::Unknown, "{report:?}");
        assert!(result.missing_members.is_empty());
        assert_eq!(
            result
                .evidence
                .requirements
                .iter()
                .find(|r| r.disk)
                .unwrap()
                .content,
            MameMemberEvidence::Uninspectable
        );
        assert!(!result.evidence.verified_complete());
    }
}

#[test]
fn unrelated_chd_does_not_satisfy_a_missing_disk() {
    let dat = catalogue(&format!(r#"<machine name="child">{ROM}{DISK}</machine>"#));
    let report = analyse_collection(&dat, &inventory(Some("unrelated")), None, None);
    assert_eq!(child(&report).health, MameSetHealth::Bad);
    assert_eq!(child(&report).missing_members, ["child/hdd"]);
}

#[test]
fn parent_chd_requires_explicit_chd_aware_evidence_and_retains_nodump_limits() {
    for (status, expected) in [
        ("good", MameSetHealth::Good),
        ("nodump", MameSetHealth::Unknown),
    ] {
        let disk = DISK.replace("/>", &format!(r#" status="{status}"/>"#));
        let dat = catalogue(&format!(
            r#"<machine name="child" cloneof="parent">{ROM}{disk}</machine><machine name="parent"/>"#
        ));
        let report = analyse_collection_with_chds(
            &dat,
            &inventory(Some("parent")),
            None,
            None,
            &[MameVerifiedChd {
                set_name: "parent".into(),
                name: "hdd".into(),
                verified_overall_sha1: "b".repeat(40),
            }],
        );
        assert_eq!(child(&report).health, expected);
        assert_eq!(
            child(&report).evidence.runnability,
            MameRunnability::NotAssessed
        );
    }
}

#[test]
fn unresolved_merged_disk_storage_is_not_confirmed_absence() {
    let disk = DISK.replace("/>", r#" merge="different-name"/>"#);
    let dat = catalogue(&format!(
        r#"<machine name="child" cloneof="parent" romof="parent">{ROM}{disk}</machine><machine name="parent"/>"#
    ));
    let report = analyse_collection(&dat, &inventory(None), None, None);
    assert_eq!(child(&report).health, MameSetHealth::Unknown);
    assert!(!child(&report).evidence.unresolved_dependencies.is_empty());
    assert!(child(&report).missing_members.is_empty());
    assert_eq!(report.chd_coverage.sets_missing_chd, 0);
}

#[test]
fn wrong_parent_chd_does_not_disprove_an_unverified_child_copy() {
    let dat = catalogue(&format!(
        r#"<machine name="child" cloneof="parent">{ROM}{DISK}</machine><machine name="parent"/>"#
    ));
    let mut inv = inventory(Some("parent"));
    let candidate = inv.sets[1].members[0].clone();
    inv.sets[0].members.push(candidate);
    let report = analyse_collection_with_chds(
        &dat,
        &inv,
        None,
        None,
        &[MameVerifiedChd {
            set_name: "parent".into(),
            name: "hdd".into(),
            verified_overall_sha1: "c".repeat(40),
        }],
    );
    assert_eq!(child(&report).health, MameSetHealth::Unknown);
    assert!(child(&report).missing_members.is_empty());
}
