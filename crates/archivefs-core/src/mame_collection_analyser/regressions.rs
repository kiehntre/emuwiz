use super::*;
use crate::dat::model::{DatDeviceRefEntry, DatDiskEntry};
use crate::dat::parsers::mame_listxml::MAME_LISTXML_PARSER_SCHEMA_VERSION;

fn game(name: &str) -> DatGameEntry {
    DatGameEntry {
        name: name.into(),
        mame_listxml_schema_version: Some(MAME_LISTXML_PARSER_SCHEMA_VERSION),
        roms: vec![DatRomEntry {
            name: "main.bin".into(),
            size_bytes: Some(1),
            crc32: Some("12345678".into()),
            sha1: Some("a".repeat(40)),
            ..Default::default()
        }],
        ..Default::default()
    }
}
fn catalogue(games: Vec<DatGameEntry>) -> ParsedDat {
    super::tests::catalogue(games)
}
fn observed(game: &DatGameEntry) -> MameObservedSet {
    MameObservedSet {
        name: game.name.clone(),
        location: format!("/synthetic/{}.zip", game.name).into(),
        directory: false,
        members: game
            .roms
            .iter()
            .map(|r| MameObservedMember {
                name: r.name.clone(),
                size_bytes: r.size_bytes,
                crc32: r.crc32.clone(),
                md5: r.md5.clone(),
                sha1: r.sha1.clone(),
            })
            .collect(),
    }
}
fn inventory(sets: Vec<MameObservedSet>) -> MameCollectionInventory {
    MameCollectionInventory {
        sets,
        inspected_completely: true,
        ..Default::default()
    }
}
fn health(dat: &ParsedDat, inv: &MameCollectionInventory, name: &str) -> MameSetHealth {
    analyse_collection(dat, inv, None, None)
        .set_results
        .into_iter()
        .find(|r| r.name == name)
        .unwrap()
        .health
}
#[test]
fn regression_hash_contradiction_overrides_crc() {
    let g = game("g");
    let mut s = observed(&g);
    s.members[0].sha1 = Some("b".repeat(40));
    assert_eq!(
        health(&catalogue(vec![g]), &inventory(vec![s]), "g"),
        MameSetHealth::Bad
    );
}
#[test]
fn regression_name_size_and_crc_only_are_not_verified() {
    for weak in [false, true] {
        let mut g = game("g");
        g.roms[0].sha1 = None;
        if !weak {
            g.roms[0].crc32 = None;
        }
        let s = observed(&g);
        assert_eq!(
            health(&catalogue(vec![g]), &inventory(vec![s]), "g"),
            MameSetHealth::Unknown
        );
    }
}
#[test]
fn regression_incomplete_inventory_cannot_be_good() {
    let g = game("g");
    let mut inv = inventory(vec![observed(&g)]);
    inv.inspected_completely = false;
    assert_eq!(
        health(&catalogue(vec![g]), &inv, "g"),
        MameSetHealth::Unknown
    );
}
#[test]
fn regression_missing_chd_and_chd_only() {
    for roms in [true, false] {
        let mut g = game("g");
        if !roms {
            g.roms.clear();
        }
        g.disks.push(DatDiskEntry {
            name: Some("disk".into()),
            sha1: Some("b".repeat(40)),
            ..Default::default()
        });
        let inv = inventory(vec![observed(&g)]);
        assert_eq!(health(&catalogue(vec![g]), &inv, "g"), MameSetHealth::Bad);
    }
}
#[test]
fn regression_unresolved_relationships_never_good() {
    for kind in 0..3 {
        let mut g = game("g");
        match kind {
            0 => g.clone_of = Some("absent".into()),
            1 => g.rom_of = Some("absent".into()),
            _ => g.device_refs.push(DatDeviceRefEntry {
                name: Some("absent".into()),
            }),
        }
        let inv = inventory(vec![observed(&g)]);
        assert_ne!(health(&catalogue(vec![g]), &inv, "g"), MameSetHealth::Good);
    }
}
#[test]
fn regression_preservation_is_not_clean_storage() {
    for status in ["nodump", "baddump", "no_dump", "bad_dump"] {
        let mut g = game("g");
        g.roms[0].status = Some(status.into());
        let inv = inventory(vec![observed(&g)]);
        assert_ne!(health(&catalogue(vec![g]), &inv, "g"), MameSetHealth::Good);
    }
}
#[test]
fn regression_unknown_versions_are_unknown_despite_good_roms() {
    let g = game("g");
    let inv = inventory(vec![observed(&g)]);
    assert_eq!(
        analyse_collection(&catalogue(vec![g]), &inv, None, None).version_compatibility,
        MameVersionCompatibility::ExactSourceVersionUnknown
    );
}
#[test]
fn regression_merged_split_nonmerged_and_romof_not_cloneof() {
    let p = game("parent");
    let mut c = game("clone");
    c.clone_of = Some("lineage".into());
    c.rom_of = Some("parent".into());
    c.roms[0].merge = Some("main.bin".into());
    let mut lineage = game("lineage");
    lineage.roms[0].sha1 = Some("b".repeat(40));
    for split in [true, false] {
        let mut child = observed(&c);
        if split {
            child.members.clear();
        }
        let inv = inventory(vec![observed(&p), child]);
        assert_eq!(
            health(
                &catalogue(vec![p.clone(), c.clone(), lineage.clone()]),
                &inv,
                "clone"
            ),
            MameSetHealth::Good
        );
    }
    let mut c = game("clone");
    c.clone_of = Some("parent".into());
    c.roms[0].name = "unique.bin".into();
    c.roms[0].sha1 = Some("c".repeat(40));
    let mut merged = observed(&p);
    let mut member = observed(&c).members.remove(0);
    member.name = "clone/unique.bin".into();
    merged.members.push(member);
    assert_eq!(
        health(&catalogue(vec![p, c]), &inventory(vec![merged]), "clone"),
        MameSetHealth::Good
    );
}
#[test]
fn regression_bios_and_device_require_verified_support() {
    for device in [false, true] {
        let mut g = game("g");
        let mut support = game("support");
        support.roms[0].name = "bios.bin".into();
        if device {
            support.is_device = Some("yes".into());
            g.device_refs.push(DatDeviceRefEntry {
                name: Some("support".into()),
            });
        } else {
            support.is_bios = Some("yes".into());
            g.rom_of = Some("support".into());
        }
        let inv = inventory(vec![observed(&g)]);
        let dat = catalogue(vec![g.clone(), support.clone()]);
        assert_eq!(health(&dat, &inv, "g"), MameSetHealth::Bad);
        let inv = inventory(vec![observed(&g), observed(&support)]);
        assert_eq!(health(&dat, &inv, "g"), MameSetHealth::Good);
    }
}
#[test]
fn regression_cycles_duplicates_and_order() {
    let mut a = game("a");
    let mut b = game("b");
    a.clone_of = Some("b".into());
    b.clone_of = Some("a".into());
    let inv = inventory(vec![observed(&a), observed(&b)]);
    let first = analyse_collection(&catalogue(vec![a.clone(), b.clone()]), &inv, None, None);
    let second = analyse_collection(&catalogue(vec![b, a.clone()]), &inv, None, None);
    assert_eq!(first, second);
    assert!(
        first
            .set_results
            .iter()
            .all(|r| r.health == MameSetHealth::Unknown)
    );
    assert_eq!(
        health(&catalogue(vec![a.clone(), a]), &inv, "a"),
        MameSetHealth::Unknown
    );
    let g = game("g");
    let dat = catalogue(vec![g.clone()]);
    assert_eq!(
        health(&dat, &inventory(vec![observed(&g), observed(&g)]), "g"),
        MameSetHealth::Unknown
    );
    let mut set = observed(&g);
    set.members.push(set.members[0].clone());
    assert_eq!(
        health(&dat, &inventory(vec![set]), "g"),
        MameSetHealth::Unknown
    );
}
#[test]
fn regression_version_comparison_is_independent_of_storage() {
    let g = game("g");
    let mut dat = catalogue(vec![g]);
    dat.source.version = Some("0.270 (mame0270)".into());
    let inv = inventory(vec![]);
    assert_eq!(
        analyse_collection(&dat, &inv, Some("MAME v0.270".into()), None).version_compatibility,
        MameVersionCompatibility::CatalogueVersionMatch
    );
    assert_eq!(
        analyse_collection(&dat, &inv, Some("0.269".into()), None).version_compatibility,
        MameVersionCompatibility::CatalogueVersionMismatch
    );
    for v in [None, Some("unknown".into())] {
        assert_eq!(
            analyse_collection(&dat, &inv, v, None).version_compatibility,
            MameVersionCompatibility::ExactSourceVersionUnknown
        );
    }
}
#[test]
fn regression_chd_combined_digest_verification_is_explicit() {
    let mut g = game("g");
    g.roms.clear();
    g.disks.push(DatDiskEntry {
        name: Some("disk".into()),
        sha1: Some("b".repeat(40)),
        ..Default::default()
    });
    let mut set = observed(&g);
    set.members.push(MameObservedMember {
        name: "disk.chd".into(),
        size_bytes: None,
        crc32: None,
        md5: None,
        sha1: Some("b".repeat(40)),
    });
    let dat = catalogue(vec![g]);
    let inv = inventory(vec![set]);
    assert_eq!(health(&dat, &inv, "g"), MameSetHealth::Unknown);
    let chds = [MameVerifiedChd {
        set_name: "g".into(),
        name: "disk".into(),
        verified_overall_sha1: "b".repeat(40),
    }];
    let result = analyse_collection_with_chds(&dat, &inv, None, None, &chds);
    assert_eq!(result.set_results[0].health, MameSetHealth::Good);
    assert_eq!(result.chd_coverage.sets_with_chd, 1);
    assert_eq!(
        result.set_results[0].evidence.runnability,
        MameRunnability::NotAssessed
    );
    let bad = [MameVerifiedChd {
        verified_overall_sha1: "c".repeat(40),
        ..chds[0].clone()
    }];
    assert_eq!(
        analyse_collection_with_chds(&dat, &inv, None, None, &bad).set_results[0].health,
        MameSetHealth::Bad
    );
}
#[test]
fn regression_nodump_never_offers_repair_or_claims_unrunnable() {
    let mut g = game("g");
    g.roms[0].status = Some("nodump".into());
    let dat = catalogue(vec![g.clone()]);
    let mut set = observed(&g);
    set.members.clear();
    let inv = inventory(vec![set]);
    let r = analyse_collection(&dat, &inv, None, None);
    assert_eq!(r.set_results[0].health, MameSetHealth::Unknown);
    assert_eq!(
        r.set_results[0].evidence.runnability,
        MameRunnability::NotAssessed
    );
    assert!(r.set_results[0].missing_members.is_empty());
    assert!(!r.top_missing_dependencies[0].obtainable_or_expected);
    assert_eq!(r.top_missing_dependencies[0].potentially_repairs_sets, 0);
}
#[test]
fn regression_missing_stronger_hash_and_case_normalisation() {
    let g = game("g");
    let mut set = observed(&g);
    set.members[0].sha1 = None;
    let dat = catalogue(vec![g.clone()]);
    assert_eq!(
        health(&dat, &inventory(vec![set]), "g"),
        MameSetHealth::Unknown
    );
    let mut set = observed(&g);
    set.members[0].sha1 = Some("A".repeat(40));
    assert_eq!(
        health(&dat, &inventory(vec![set]), "g"),
        MameSetHealth::Good
    );
}
#[test]
fn regression_real_listxml_provenance_and_boolean_flags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mame.xml");
    std::fs::write(&path,format!(r#"<mame build="0.280"><machine name="g" isbios="no" isdevice="no" runnable="yes"><rom name="main.bin" size="1" crc="12345678" sha1="{}"/></machine></mame>"#,"a".repeat(40))).unwrap();
    let dat = crate::dat::parsers::mame_listxml::parse_mame_listxml(&path, Default::default())
        .unwrap()
        .dat;
    let inv = inventory(vec![observed(&dat.games[0])]);
    let result = analyse_collection(&dat, &inv, Some("0.280".into()), None);
    assert_eq!(result.set_results[0].health, MameSetHealth::Good);
    assert_eq!(
        result.version_compatibility,
        MameVersionCompatibility::CatalogueVersionMatch
    );
    let plan = crate::mame_playing_library::build_mame_playing_library_plan(
        &crate::mame_playing_library::MamePlayingLibraryRequest {
            catalogue: &dat,
            inventory: &inv,
            analysis: &result,
            preferences: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(plan.selected_sets.len(), 1);
    assert!(plan.selected_sets[0].complete);
    let mut partial = inv.clone();
    partial.inspected_completely = false;
    let result = analyse_collection(&dat, &partial, Some("0.280".into()), None);
    let plan = crate::mame_playing_library::build_mame_playing_library_plan(
        &crate::mame_playing_library::MamePlayingLibraryRequest {
            catalogue: &dat,
            inventory: &partial,
            analysis: &result,
            preferences: Default::default(),
        },
    )
    .unwrap();
    assert!(plan.selected_sets.is_empty());
}
#[test]
fn regression_ambiguous_merge_provider_and_stale_metadata() {
    let parent = game("parent");
    let mut clone = game("clone");
    clone.clone_of = Some("parent".into());
    clone.roms[0].merge = Some("main.bin".into());
    let inv = inventory(vec![observed(&parent), observed(&clone)]);
    let mut other = parent.clone();
    other.roms[0].sha1 = Some("b".repeat(40));
    let a = analyse_collection(
        &catalogue(vec![parent.clone(), other.clone(), clone.clone()]),
        &inv,
        None,
        None,
    );
    let b = analyse_collection(
        &catalogue(vec![clone, other, parent.clone()]),
        &inv,
        None,
        None,
    );
    assert_eq!(a, b);
    assert!(
        a.set_results
            .iter()
            .all(|r| r.health == MameSetHealth::Unknown)
    );
    let mut old = parent;
    old.mame_listxml_schema_version = None;
    assert_eq!(
        health(&catalogue(vec![old]), &inv, "parent"),
        MameSetHealth::Unknown
    );
}
#[test]
fn regression_preservation_mixtures_and_disk_nodump() {
    let mut g = game("g");
    let mut undumped = g.roms[0].clone();
    undumped.name = "undumped.bin".into();
    undumped.status = Some("nodump".into());
    g.roms.push(undumped);
    let mut set = observed(&g);
    set.members.pop();
    let dat = catalogue(vec![g.clone()]);
    assert_eq!(
        health(&dat, &inventory(vec![set.clone()]), "g"),
        MameSetHealth::Unknown
    );
    g.roms[0].status = Some("baddump".into());
    assert_eq!(
        health(
            &catalogue(vec![g.clone()]),
            &inventory(vec![set.clone()]),
            "g"
        ),
        MameSetHealth::Unknown
    );
    set.members.clear();
    assert_eq!(
        health(&catalogue(vec![g]), &inventory(vec![set]), "g"),
        MameSetHealth::Bad
    );
    let mut g = game("disk");
    g.roms.clear();
    g.disks.push(DatDiskEntry {
        name: Some("undumped".into()),
        status: Some("nodump".into()),
        ..Default::default()
    });
    let r = analyse_collection(
        &catalogue(vec![g.clone()]),
        &inventory(vec![observed(&g)]),
        None,
        None,
    );
    assert_eq!(r.set_results[0].health, MameSetHealth::Unknown);
    assert!(!r.top_missing_dependencies[0].obtainable_or_expected);
    assert_eq!(r.chd_coverage.sets_missing_chd, 0);
    assert_eq!(r.chd_coverage.sets_unverified_chd, 1);
}

#[test]
fn absent_catalogue_machine_is_not_an_acquisition_request() {
    let dat = catalogue(vec![game("not-owned")]);
    let inv = inventory(vec![]);
    let result = analyse_collection(&dat, &inv, None, None);
    assert_eq!(result.set_results[0].health, MameSetHealth::Unknown);
    assert_eq!(result.failing_sets, 0);
    assert!(result.top_missing_dependencies.is_empty());
}
#[test]
fn partial_digest_and_filename_evidence_are_distinct() {
    let mut g = game("g");
    let mut set = observed(&g);
    set.members[0].sha1 = None;
    let report = analyse_collection(
        &catalogue(vec![g.clone()]),
        &inventory(vec![set]),
        None,
        None,
    );
    assert_eq!(
        report.set_results[0].evidence.requirements[0].content,
        MameMemberEvidence::PartialHash
    );
    g.roms[0].sha1 = None;
    g.roms[0].crc32 = None;
    let set = observed(&g);
    let report = analyse_collection(&catalogue(vec![g]), &inventory(vec![set]), None, None);
    assert_eq!(
        report.set_results[0].evidence.requirements[0].content,
        MameMemberEvidence::NameOrSizeOnly
    );
}

#[test]
fn missing_borrowed_parent_storage_blocks_split_clone() {
    let parent = game("parent");
    let mut clone = game("clone");
    clone.clone_of = Some("parent".into());
    clone.roms[0].merge = Some("main.bin".into());
    let mut set = observed(&clone);
    set.members.clear();
    assert_eq!(
        health(
            &catalogue(vec![parent, clone]),
            &inventory(vec![set]),
            "clone"
        ),
        MameSetHealth::Bad
    );
}

#[test]
fn ambiguous_update_comparison_is_unknown_and_order_independent() {
    let old = catalogue(vec![game("g")]);
    for member_duplicate in [false, true] {
        let mut current = old.clone();
        if member_duplicate {
            let mut duplicate = current.games[0].roms[0].clone();
            duplicate.sha1 = Some("b".repeat(40));
            current.games[0].roms.push(duplicate);
        } else {
            let mut duplicate = current.games[0].clone();
            duplicate.roms[0].sha1 = Some("b".repeat(40));
            current.games.push(duplicate);
        }
        let inv = inventory(vec![]);
        let first = analyse_collection(&current, &inv, None, Some(&old));
        current.games.reverse();
        for game in &mut current.games {
            game.roms.reverse();
        }
        let second = analyse_collection(&current, &inv, None, Some(&old));
        assert_eq!(first, second);
        assert!(!first.update_readiness.unwrap().comparison_available);
        assert!(!compare_catalogues(&current, &old).comparison_available);
        assert_eq!(
            compare_catalogues(&current, &old),
            compare_catalogues(&old, &current)
        );
    }
}
