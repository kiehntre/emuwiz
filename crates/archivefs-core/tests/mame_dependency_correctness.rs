//! Synthetic declarations and checksum observations only; no ROM files are used.
use archivefs_core::arcade_mame_compatibility::*;
use archivefs_core::dat::dependency::{DependencyKind, DependencyOutcome, DependencyTarget};
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::model::ParsedDat;
use archivefs_core::dat::parsers::mame_listxml::parse_mame_listxml;
use archivefs_core::ready_to_play::ReadyToPlayState;

fn metadata(machines: &str) -> ParsedDat {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("synthetic.xml");
    std::fs::write(
        &path,
        format!("<mame build=\"synthetic\">{machines}</mame>"),
    )
    .unwrap();
    parse_mame_listxml(&path, DatLimits::default()).unwrap().dat
}

fn rom(set: &str, name: &str, crc: &str) -> MameObservedRom {
    MameObservedRom {
        set_name: set.into(),
        name: name.into(),
        size_bytes: Some(4),
        crc32: Some(crc.into()),
        sha1: None,
    }
}

fn evidence(names: &[&str], roms: Vec<MameObservedRom>) -> ObservedArcadeSetEvidence {
    let mut evidence =
        ObservedArcadeSetEvidence::new("child", ObservedEvidenceCompleteness::Complete);
    evidence.observed_set_names = names.iter().map(|name| (*name).into()).collect();
    evidence.observed_roms = roms;
    evidence
}

fn audit(dat: &ParsedDat, evidence: &ObservedArcadeSetEvidence) -> MameCompletenessEvaluation {
    let mame = InstalledMameEvidence::new(
        "/synthetic/mame",
        "synthetic",
        "/synthetic/listxml",
        [],
        Some("fixture".into()),
        None,
    );
    let alternatives = MameExpectationIndex::new(dat)
        .alternatives_for("child")
        .unwrap();
    let result = evidence.audit_alternatives(&mame, &alternatives);
    println!(
        "fixture={} state={:?} diagnostics={:?}",
        std::thread::current().name().unwrap_or("fixture"),
        result.state,
        result
            .mismatches
            .iter()
            .map(|m| (m.reason, &m.set_name, &m.member_name))
            .collect::<Vec<_>>()
    );
    result
}

const FAMILY: &str = r#"
    <machine name="child" cloneof="parent" romof="parent">
      <rom name="child.bin" size="4" crc="aaaaaaaa"/>
      <rom name="shared-alias.bin" merge="shared.bin" size="4" crc="bbbbbbbb"/>
    </machine>
    <machine name="parent">
      <rom name="shared.bin" size="4" crc="bbbbbbbb"/>
      <rom name="parent-only.bin" size="4" crc="cccccccc"/>
    </machine>"#;

#[test]
fn clone_parent_different_from_rom_source() {
    let dat = metadata(
        r#"<machine name="child" cloneof="lineage" romof="source"><rom name="alias.bin" merge="source.bin" size="4" crc="aaaaaaaa"/></machine>
      <machine name="lineage"><rom name="unneeded.bin" size="4" crc="bbbbbbbb"/></machine>
      <machine name="source"><rom name="source.bin" size="4" crc="aaaaaaaa"/></machine>"#,
    );
    let result = audit(
        &dat,
        &evidence(&["source"], vec![rom("source", "source.bin", "aaaaaaaa")]),
    );
    assert_eq!(result.state, MameSetCompatibilityState::Compatible);
    let expectation = MameExpectationIndex::new(&dat)
        .expectation_for("child")
        .unwrap();
    assert_eq!(expectation.parent.as_deref(), Some("lineage"));
    assert_eq!(expectation.rom_of.as_deref(), Some("source"));
    assert_eq!(
        expectation.dependencies.closure_set_names,
        ["child", "lineage", "source"]
    );
    assert!(!expectation.roms.iter().any(|r| r.name == "unneeded.bin"));
}

#[test]
fn parent_extra_roms_are_not_clone_requirements() {
    let dat = metadata(FAMILY);
    let result = audit(
        &dat,
        &evidence(
            &["child", "parent"],
            vec![
                rom("child", "child.bin", "aaaaaaaa"),
                rom("parent", "shared.bin", "bbbbbbbb"),
            ],
        ),
    );
    assert_eq!(result.state, MameSetCompatibilityState::Compatible);
    assert!(
        !MameExpectationIndex::new(&dat)
            .expectation_for("child")
            .unwrap()
            .roms
            .iter()
            .any(|r| r.name == "parent-only.bin")
    );
}

#[test]
fn missing_bios_is_diagnosed_as_bios() {
    let dat = metadata(
        r#"<machine name="child" romof="firmware"><rom name="game.bin" size="4" crc="aaaaaaaa"/></machine>
      <machine name="firmware" isbios="yes"><rom name="boot.bin" size="4" crc="bbbbbbbb"/></machine>"#,
    );
    let result = audit(
        &dat,
        &evidence(&["child"], vec![rom("child", "game.bin", "aaaaaaaa")]),
    );
    assert_eq!(result.state, MameSetCompatibilityState::Incompatible);
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::BiosMissing
                && m.set_name == "firmware"
                && m.member_name.as_deref() == Some("boot.bin"))
    );
}

#[test]
fn missing_device_rom_is_diagnosed_as_device() {
    let dat = metadata(
        r#"<machine name="child"><rom name="game.bin" size="4" crc="aaaaaaaa"/><device_ref name="sound"/></machine>
      <machine name="sound" isdevice="yes"><rom name="sound.bin" size="4" crc="bbbbbbbb"/></machine>"#,
    );
    let result = audit(
        &dat,
        &evidence(
            &["child", "sound"],
            vec![rom("child", "game.bin", "aaaaaaaa")],
        ),
    );
    assert_eq!(result.state, MameSetCompatibilityState::Incompatible);
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::DeviceDependencyMissing
                && m.set_name == "sound"
                && m.member_name.as_deref() == Some("sound.bin"))
    );
}

#[test]
fn explicit_merge_checksum_mismatch_is_unknown_even_with_local_copy() {
    let dat = metadata(
        r#"<machine name="child" romof="source"><rom name="alias.bin" merge="source.bin" size="4" crc="aaaaaaaa"/></machine>
      <machine name="source"><rom name="source.bin" size="4" crc="bbbbbbbb"/></machine>"#,
    );
    let result = audit(
        &dat,
        &evidence(
            &["child", "source"],
            vec![
                rom("child", "alias.bin", "aaaaaaaa"),
                rom("source", "source.bin", "bbbbbbbb"),
            ],
        ),
    );
    assert_eq!(result.state, MameSetCompatibilityState::Unknown);
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::EvidenceUnavailable
                && m.member_name.as_deref() == Some("alias.bin"))
    );
    assert!(
        result
            .alternatives
            .iter()
            .all(|a| a.ready_to_play_state == Some(ReadyToPlayState::Unknown))
    );
}

#[test]
fn clone_dependency_cycle_is_unknown_and_deterministic() {
    let mut dat = metadata(
        r#"<machine name="child" cloneof="parent"><rom name="game.bin" size="4" crc="aaaaaaaa"/></machine><machine name="parent" cloneof="child"/>"#,
    );
    let observed = evidence(
        &["child", "parent"],
        vec![rom("child", "game.bin", "aaaaaaaa")],
    );
    let first = audit(&dat, &observed);
    assert_eq!(first.state, MameSetCompatibilityState::Unknown);
    dat.games.reverse();
    assert_eq!(first, audit(&dat, &observed));
}

#[test]
fn missing_referenced_parent_is_metadata_unknown() {
    let dat = metadata(
        r#"<machine name="child" cloneof="absent"><rom name="game.bin" size="4" crc="aaaaaaaa"/></machine>"#,
    );
    let result = audit(
        &dat,
        &evidence(&["child"], vec![rom("child", "game.bin", "aaaaaaaa")]),
    );
    assert_eq!(result.state, MameSetCompatibilityState::Unknown);
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::EvidenceUnavailable && m.set_name == "absent")
    );
}

#[test]
fn correct_split_collection() {
    let dat = metadata(FAMILY);
    assert_eq!(
        audit(
            &dat,
            &evidence(
                &["child", "parent"],
                vec![
                    rom("child", "child.bin", "aaaaaaaa"),
                    rom("parent", "shared.bin", "bbbbbbbb")
                ]
            )
        )
        .state,
        MameSetCompatibilityState::Compatible
    );
}

#[test]
fn correct_merged_collection() {
    let dat = metadata(FAMILY);
    assert_eq!(
        audit(
            &dat,
            &evidence(
                &["parent"],
                vec![
                    rom("parent", "child/child.bin", "aaaaaaaa"),
                    rom("parent", "shared.bin", "bbbbbbbb")
                ]
            )
        )
        .state,
        MameSetCompatibilityState::Compatible
    );
}

#[test]
fn correct_non_merged_collection() {
    let dat = metadata(FAMILY);
    assert_eq!(
        audit(
            &dat,
            &evidence(
                &["child"],
                vec![
                    rom("child", "child.bin", "aaaaaaaa"),
                    rom("child", "shared-alias.bin", "bbbbbbbb")
                ]
            )
        )
        .state,
        MameSetCompatibilityState::Compatible
    );
}

#[test]
fn default_bios_selection_does_not_require_other_variants() {
    let dat = metadata(
        r#"<machine name="child"><biosset name="standard" description="Standard" default="yes"/><biosset name="alternate" description="Alternate"/>
      <rom name="boot.bin" bios="standard" size="4" crc="aaaaaaaa"/><rom name="other.bin" bios="alternate" size="4" crc="bbbbbbbb"/></machine>"#,
    );
    let result = audit(
        &dat,
        &evidence(&["child"], vec![rom("child", "boot.bin", "aaaaaaaa")]),
    );
    assert_eq!(result.state, MameSetCompatibilityState::Compatible);
    assert_eq!(dat.games[0].bios_sets.len(), 2);
}

#[test]
fn full_non_merged_support_is_valid_without_support_archives() {
    let dat = metadata(
        r#"<machine name="child" romof="firmware"><rom name="game.bin" size="4" crc="aaaaaaaa"/><device_ref name="sound"/></machine>
      <machine name="firmware" isbios="yes"><rom name="boot.bin" size="4" crc="bbbbbbbb"/></machine>
      <machine name="sound" isdevice="yes"><rom name="sound.bin" size="4" crc="cccccccc"/></machine>"#,
    );
    assert_eq!(
        audit(
            &dat,
            &evidence(
                &["child"],
                vec![
                    rom("child", "game.bin", "aaaaaaaa"),
                    rom("child", "boot.bin", "bbbbbbbb"),
                    rom("child", "sound.bin", "cccccccc")
                ]
            )
        )
        .state,
        MameSetCompatibilityState::Compatible
    );
}

#[test]
fn provenance_preserves_declaring_machine_owner_and_observed_location() {
    let dat = metadata(FAMILY);
    let index = MameExpectationIndex::new(&dat);
    let expectation = index.expectation_for("child").unwrap();
    let requirement = expectation
        .dependencies
        .resolution
        .roms
        .iter()
        .find(|r| r.member_name == "shared-alias.bin")
        .unwrap();
    assert_eq!(requirement.declaring_set, "child");
    assert_eq!(
        requirement.owner,
        Some(DependencyTarget::SetMember {
            set_name: "parent".into(),
            member_name: "shared.bin".into()
        })
    );
    let kinds: Vec<_> = expectation
        .dependencies
        .resolution
        .references
        .iter()
        .map(|r| r.requirement.kind)
        .collect();
    assert!(kinds.contains(&DependencyKind::ParentSet));
    assert!(kinds.contains(&DependencyKind::RomSource));
    assert!(kinds.contains(&DependencyKind::MergedRom));
    assert!(
        expectation
            .dependencies
            .resolution
            .references
            .iter()
            .all(|r| r.requirement.outcome == DependencyOutcome::Satisfied)
    );
    let result = audit(
        &dat,
        &evidence(
            &["parent"],
            vec![
                rom("parent", "child/child.bin", "aaaaaaaa"),
                rom("parent", "shared.bin", "bbbbbbbb"),
            ],
        ),
    );
    let compatible = result
        .alternatives
        .iter()
        .find(|a| a.state == MameSetCompatibilityState::Compatible)
        .unwrap();
    let verified = compatible
        .verified_roms
        .iter()
        .find(|r| r.member_name == "child.bin")
        .unwrap();
    assert_eq!(verified.declaring_set, "child");
    assert_eq!(verified.observed.set_name, "parent");
    assert_eq!(verified.observed.name, "child/child.bin");
    assert_eq!(verified.observed.crc32.as_deref(), Some("aaaaaaaa"));
}

#[test]
fn clone_lineage_alone_never_changes_declared_rom_ownership() {
    let dat = metadata(
        r#"<machine name="child" cloneof="parent"><rom name="game.bin" size="4" crc="aaaaaaaa"/></machine><machine name="parent"><rom name="unneeded.bin" size="4" crc="bbbbbbbb"/></machine>"#,
    );
    let expectation = MameExpectationIndex::new(&dat)
        .expectation_for("child")
        .unwrap();
    assert_eq!(expectation.roms.len(), 1);
    assert_eq!(
        expectation.dependencies.resolution.roms[0].owner,
        Some(DependencyTarget::SetMember {
            set_name: "child".into(),
            member_name: "game.bin".into()
        })
    );
    assert_eq!(
        audit(
            &dat,
            &evidence(&["child"], vec![rom("child", "game.bin", "aaaaaaaa")])
        )
        .state,
        MameSetCompatibilityState::Compatible
    );
}

#[test]
fn selected_bios_variant_applies_to_the_corresponding_provider() {
    let dat = metadata(
        r#"<machine name="child" romof="firmware">
      <biosset name="standard" default="yes"/><biosset name="alternate"/>
      <rom name="boot.bin" bios="standard" merge="boot.bin" size="4" crc="aaaaaaaa"/>
      <rom name="other.bin" bios="alternate" merge="other.bin" size="4" crc="bbbbbbbb"/>
    </machine><machine name="firmware" isbios="yes">
      <biosset name="standard" default="yes"/><biosset name="alternate"/>
      <rom name="boot.bin" bios="standard" size="4" crc="aaaaaaaa"/>
      <rom name="other.bin" bios="alternate" size="4" crc="bbbbbbbb"/>
    </machine>"#,
    );
    let index = MameExpectationIndex::new(&dat);
    let expectation = index
        .expectation_for_with_bios(
            "child",
            &std::collections::BTreeMap::from([("child".into(), "alternate".into())]),
        )
        .unwrap();
    let mame = InstalledMameEvidence::new(
        "/synthetic/mame",
        "synthetic",
        "/synthetic/listxml",
        [],
        None,
        None,
    );
    let result = evidence(
        &["firmware"],
        vec![rom("firmware", "other.bin", "bbbbbbbb")],
    )
    .audit(&mame, expectation);
    println!(
        "fixture=selected_bios_variant state={:?} diagnostics={:?}",
        result.state, result.mismatches
    );
    assert_eq!(result.state, MameSetCompatibilityState::Compatible);
    assert!(
        result
            .expectation
            .roms
            .iter()
            .all(|r| r.name == "other.bin")
    );
    assert_eq!(
        result
            .expectation
            .dependencies
            .resolution
            .bios_selections
            .get("firmware")
            .map(String::as_str),
        Some("alternate")
    );
}

#[test]
fn bios_selection_without_default_or_explicit_choice_is_unknown() {
    let dat = metadata(
        r#"<machine name="child"><biosset name="one"/><biosset name="two"/><rom name="one.bin" bios="one" size="4" crc="aaaaaaaa"/><rom name="two.bin" bios="two" size="4" crc="bbbbbbbb"/></machine>"#,
    );
    assert_eq!(
        audit(
            &dat,
            &evidence(
                &["child"],
                vec![
                    rom("child", "one.bin", "aaaaaaaa"),
                    rom("child", "two.bin", "bbbbbbbb")
                ]
            )
        )
        .state,
        MameSetCompatibilityState::Unknown
    );
}

#[test]
fn shared_device_diamond_is_valid_and_nested_missing_rom_has_device_provenance() {
    let dat = metadata(
        r#"<machine name="child"><rom name="game.bin" size="4" crc="aaaaaaaa"/><device_ref name="left"/><device_ref name="right"/></machine>
    <machine name="left" isdevice="yes"><device_ref name="shared"/></machine><machine name="right" isdevice="yes"><device_ref name="shared"/></machine>
    <machine name="shared" isdevice="yes"><rom name="shared.bin" size="4" crc="bbbbbbbb"/></machine>"#,
    );
    assert_eq!(
        audit(
            &dat,
            &evidence(
                &["child", "shared"],
                vec![
                    rom("child", "game.bin", "aaaaaaaa"),
                    rom("shared", "shared.bin", "bbbbbbbb")
                ]
            )
        )
        .state,
        MameSetCompatibilityState::Compatible
    );
    let missing = audit(
        &dat,
        &evidence(&["child"], vec![rom("child", "game.bin", "aaaaaaaa")]),
    );
    assert_eq!(missing.state, MameSetCompatibilityState::Incompatible);
    assert!(
        missing
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::DeviceDependencyMissing
                && m.set_name == "shared"
                && m.member_name.as_deref() == Some("shared.bin"))
    );
}

#[test]
fn malformed_missing_duplicate_and_cyclic_references_cannot_be_playable() {
    for machines in [
        r#"<machine name="child"><device_ref/></machine>"#,
        r#"<machine name="child" cloneof=""/>"#,
        r#"<machine name="child" romof=""/>"#,
        r#"<machine name="child"><biosset name="one" default=""/></machine>"#,
        r#"<machine name="child" romof="missing"/>"#,
        r#"<machine name="child" romof="source"/><machine name="source"/><machine name="source"/>"#,
        r#"<machine name="child" romof="source"/><machine name="source" romof="child"/>"#,
        r#"<machine name="child"><device_ref name="sound"/></machine><machine name="sound" isdevice="yes"><device_ref name="sound"/></machine>"#,
        r#"<machine name="child"/><machine name="child"/>"#,
    ] {
        assert_eq!(
            audit(
                &metadata(machines),
                &evidence(&["child", "source", "sound"], vec![])
            )
            .state,
            MameSetCompatibilityState::Unknown,
            "{machines}"
        );
    }
}

#[test]
fn invalid_explicit_merge_targets_remain_unknown_in_every_layout() {
    for target in [
        r#"<rom name="other.bin" size="4" crc="aaaaaaaa"/>"#,
        r#"<rom name="source.bin" size="4" crc="aaaaaaaa"/><rom name="source.bin" size="4" crc="aaaaaaaa"/>"#,
        r#"<rom name="source.bin" size="5" crc="aaaaaaaa"/>"#,
        r#"<rom name="source.bin" size="4" sha1="1111111111111111111111111111111111111111"/>"#,
    ] {
        let dat = metadata(&format!(
            r#"<machine name="child" romof="source"><rom name="alias.bin" merge="source.bin" size="4" crc="aaaaaaaa"/></machine><machine name="source">{target}</machine>"#
        ));
        assert_eq!(
            audit(
                &dat,
                &evidence(
                    &["child", "source"],
                    vec![rom("child", "alias.bin", "aaaaaaaa")]
                )
            )
            .state,
            MameSetCompatibilityState::Unknown
        );
    }
}

#[test]
fn unverified_identity_and_unrelated_archive_are_not_compatibility_proof() {
    let dat = metadata(r#"<machine name="child"><rom name="game.bin" size="4"/></machine>"#);
    assert_eq!(
        audit(
            &dat,
            &evidence(&["child"], vec![rom("child", "game.bin", "aaaaaaaa")])
        )
        .state,
        MameSetCompatibilityState::Unknown
    );
    let dat = metadata(
        r#"<machine name="child"><rom name="game.bin" size="4" crc="aaaaaaaa"/></machine>"#,
    );
    assert_eq!(
        audit(
            &dat,
            &evidence(
                &["unrelated"],
                vec![rom("unrelated", "game.bin", "aaaaaaaa")]
            )
        )
        .state,
        MameSetCompatibilityState::Incompatible
    );
}

#[test]
fn unused_layout_missing_files_are_not_reported_as_selected_set_failures() {
    let dat = metadata(FAMILY);
    let result = audit(
        &dat,
        &evidence(
            &["child", "parent"],
            vec![
                rom("child", "child.bin", "aaaaaaaa"),
                rom("parent", "shared.bin", "bbbbbbbb"),
            ],
        ),
    );
    assert_eq!(result.state, MameSetCompatibilityState::Compatible);
    assert!(result.mismatches.is_empty(), "{:?}", result.mismatches);
    assert!(
        result
            .alternatives
            .iter()
            .any(|a| a.state == MameSetCompatibilityState::Incompatible)
    );
}

#[test]
fn non_merged_game_can_use_a_separate_device_rom_source() {
    let dat = metadata(&format!(
        r#"{FAMILY}<machine name="sound" isdevice="yes" romof="soundbase"><rom name="alias.bin" merge="sound.bin" size="4" crc="dddddddd"/></machine>
      <machine name="soundbase" isdevice="yes"><rom name="sound.bin" size="4" crc="dddddddd"/></machine>"#
    ));
    let mut dat = dat;
    dat.games
        .iter_mut()
        .find(|g| g.name == "child")
        .unwrap()
        .device_refs
        .push(archivefs_core::dat::model::DatDeviceRefEntry {
            name: Some("sound".into()),
        });
    assert_eq!(
        audit(
            &dat,
            &evidence(
                &["child", "soundbase"],
                vec![
                    rom("child", "child.bin", "aaaaaaaa"),
                    rom("child", "shared-alias.bin", "bbbbbbbb"),
                    rom("soundbase", "sound.bin", "dddddddd")
                ]
            )
        )
        .state,
        MameSetCompatibilityState::Compatible
    );
}

#[test]
fn rom_source_is_a_search_location_without_automatic_ownership() {
    let dat = metadata(
        r#"<machine name="child" romof="source"><rom name="game.bin" size="4" crc="aaaaaaaa"/></machine><machine name="source"><rom name="source.bin" size="4" crc="aaaaaaaa"/><rom name="unneeded.bin" size="4" crc="bbbbbbbb"/></machine>"#,
    );
    let index = MameExpectationIndex::new(&dat);
    let expectation = index.expectation_for("child").unwrap();
    assert_eq!(expectation.roms.len(), 1);
    assert_eq!(
        expectation.dependencies.resolution.roms[0].owner,
        Some(DependencyTarget::SetMember {
            set_name: "child".into(),
            member_name: "game.bin".into()
        })
    );
    assert_eq!(
        audit(
            &dat,
            &evidence(&["source"], vec![rom("source", "source.bin", "aaaaaaaa")])
        )
        .state,
        MameSetCompatibilityState::Compatible
    );
}

#[test]
fn optional_marker_cannot_hide_bad_metadata_or_nodump_policy() {
    let dat = metadata(
        r#"<machine name="child"><rom name="bad.bin" merge="" optional="yes" size="4" crc="aaaaaaaa"/></machine>"#,
    );
    assert_eq!(
        audit(&dat, &evidence(&["child"], vec![])).state,
        MameSetCompatibilityState::Unknown
    );
    let dat = metadata(
        r#"<machine name="child"><rom name="unknown.bin" optional="yes" size="4" status="nodump"/></machine>"#,
    );
    assert_eq!(
        audit(&dat, &evidence(&["child"], vec![])).state,
        MameSetCompatibilityState::Unsupported
    );
}

#[test]
fn conflicting_copies_at_the_same_location_require_resolution_evidence() {
    let dat = metadata(
        r#"<machine name="child"><rom name="game.bin" size="4" crc="aaaaaaaa"/></machine>"#,
    );
    let good = rom("child", "game.bin", "aaaaaaaa");
    let bad = rom("child", "game.bin", "bbbbbbbb");
    let first = audit(&dat, &evidence(&["child"], vec![good.clone(), bad.clone()]));
    assert_eq!(first.state, MameSetCompatibilityState::Unknown);
    assert_eq!(
        first,
        audit(&dat, &evidence(&["child"], vec![bad, good.clone()]))
    );
    assert_eq!(
        audit(&dat, &evidence(&["child"], vec![good.clone(), good])).state,
        MameSetCompatibilityState::Compatible
    );
}

#[test]
fn merge_chain_cannot_hide_checksum_conflicts_behind_sparse_intermediate_metadata() {
    let dat = metadata(
        r#"<machine name="child" romof="middle"><rom name="alias.bin" merge="middle.bin" size="4" crc="aaaaaaaa" sha1="1111111111111111111111111111111111111111"/></machine>
      <machine name="middle" romof="source"><rom name="middle.bin" merge="source.bin" size="4" crc="aaaaaaaa"/></machine>
      <machine name="source"><rom name="source.bin" size="4" crc="aaaaaaaa" sha1="2222222222222222222222222222222222222222"/></machine>"#,
    );
    let mut observed = rom("source", "source.bin", "aaaaaaaa");
    observed.sha1 = Some("1111111111111111111111111111111111111111".into());
    assert_eq!(
        audit(&dat, &evidence(&["source"], vec![observed])).state,
        MameSetCompatibilityState::Unknown
    );
}

#[test]
fn borrowed_rom_checks_every_checksum_fact_along_the_owner_chain() {
    let dat = metadata(
        r#"<machine name="child" romof="middle"><rom name="alias.bin" merge="middle.bin" size="4" sha1="1111111111111111111111111111111111111111"/></machine>
      <machine name="middle" romof="source"><rom name="middle.bin" merge="source.bin" size="4" crc="aaaaaaaa" sha1="1111111111111111111111111111111111111111"/></machine>
      <machine name="source"><rom name="source.bin" size="4" crc="aaaaaaaa" sha1="1111111111111111111111111111111111111111"/></machine>"#,
    );
    let expectation = MameExpectationIndex::new(&dat)
        .expectation_for("child")
        .unwrap();
    assert_eq!(expectation.roms.len(), 1);
    assert_eq!(expectation.roms[0].checksums.len(), 2);
    let declarations = &expectation.dependencies.resolution.roms[0].declarations;
    assert_eq!(
        declarations
            .iter()
            .map(|rom| rom.set_name.as_str())
            .collect::<Vec<_>>(),
        ["child", "middle", "source"]
    );
    assert_eq!(declarations[0].checksums.len(), 1);
    let mut observed = rom("source", "source.bin", "bbbbbbbb");
    observed.sha1 = Some("1111111111111111111111111111111111111111".into());
    assert_eq!(
        audit(&dat, &evidence(&["source"], vec![observed.clone()])).state,
        MameSetCompatibilityState::Incompatible
    );
    observed.crc32 = Some("aaaaaaaa".into());
    assert_eq!(
        audit(&dat, &evidence(&["source"], vec![observed])).state,
        MameSetCompatibilityState::Compatible
    );
}

/// The shared owner resolver never accepts a CRC-only hop when the borrow
/// publishes SHA-1. Before consolidation the evaluator accepted this chain
/// (the source publishes only a CRC) and could report it Compatible.
#[test]
fn a_crc_only_owner_never_satisfies_a_sha1_borrow() {
    let dat = metadata(
        r#"<machine name="child" romof="middle"><rom name="alias.bin" merge="middle.bin" size="4" sha1="1111111111111111111111111111111111111111"/></machine>
      <machine name="middle" romof="source"><rom name="middle.bin" merge="source.bin" size="4" crc="aaaaaaaa" sha1="1111111111111111111111111111111111111111"/></machine>
      <machine name="source"><rom name="source.bin" size="4" crc="aaaaaaaa"/></machine>"#,
    );
    let expectation = MameExpectationIndex::new(&dat)
        .expectation_for("child")
        .unwrap();
    let requirement = &expectation.dependencies.resolution.roms[0];
    assert_eq!(requirement.owner, None, "{requirement:#?}");
    let mut observed = rom("source", "source.bin", "aaaaaaaa");
    observed.sha1 = Some("1111111111111111111111111111111111111111".into());
    assert_ne!(
        audit(&dat, &evidence(&["source"], vec![observed])).state,
        MameSetCompatibilityState::Compatible
    );
}

/// A set naming itself as its own ROM source is contradictory, and a later
/// declaration must agree with every earlier borrow in the chain, not only
/// its neighbour.
#[test]
fn self_sources_and_non_adjacent_checksum_conflicts_are_refused() {
    let own = metadata(
        r#"<machine name="child" romof="child"><rom name="a.bin" merge="a.bin" size="4" crc="aaaaaaaa" sha1="1111111111111111111111111111111111111111"/></machine>"#,
    );
    let expectation = MameExpectationIndex::new(&own)
        .expectation_for("child")
        .unwrap();
    assert_eq!(expectation.dependencies.resolution.roms[0].owner, None);

    // child agrees with middle on SHA-1; middle has no CRC; source's CRC
    // contradicts child's CRC two hops back.
    let chain = metadata(
        r#"<machine name="child" romof="middle"><rom name="a.bin" merge="m.bin" size="4" crc="aaaaaaaa" sha1="1111111111111111111111111111111111111111"/></machine>
      <machine name="middle" romof="source"><rom name="m.bin" merge="s.bin" size="4" sha1="1111111111111111111111111111111111111111"/></machine>
      <machine name="source"><rom name="s.bin" size="4" crc="bbbbbbbb" sha1="1111111111111111111111111111111111111111"/></machine>"#,
    );
    let expectation = MameExpectationIndex::new(&chain)
        .expectation_for("child")
        .unwrap();
    assert_eq!(expectation.dependencies.resolution.roms[0].owner, None);
}
