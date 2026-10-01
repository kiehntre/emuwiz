use super::*;
use crate::game_identity::{IdentityConfidence, IdentityProvenance};
use crate::patch_manager::CheatReconciliationOutcome;
use std::fs;
use tempfile::{TempDir, tempdir};

fn fact(kind: IdentityKind, value: &str) -> IdentityEvidence {
    IdentityEvidence {
        kind,
        status: IdentityStatus::Verified,
        value: Some(value.into()),
        confidence: IdentityConfidence::StructuredMetadata,
        provenance: IdentityProvenance {
            archive_path: PathBuf::from("/unopened/game.rom"),
            member_path: None,
            member_index: None,
            method: "fixture evidence".into(),
        },
        diagnostic: String::new(),
    }
}
fn game(id: &str, title: &str) -> CheatPackCatalogueGame {
    CheatPackCatalogueGame {
        game: UserCheatLibraryGame {
            game_id: id.into(),
            title: title.into(),
            platform: Some("NES".into()),
            ..Default::default()
        },
        facts: vec![fact(IdentityKind::LooseRomSha256, &"a".repeat(64))],
        revision: None,
    }
}
fn association() -> CheatPackAssociation {
    CheatPackAssociation {
        title: Some("Example".into()),
        platform: Some("NES".into()),
        identities: vec![CheatPackIdentityRequirement {
            kind: IdentityKind::LooseRomSha256,
            value: "a".repeat(64),
        }],
        ..Default::default()
    }
}
fn write(root: &Path, path: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}
fn cht(code: &str) -> Vec<u8> {
    format!("cheats = 1\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"{code}\"\ncheat0_enable = true\n").into_bytes()
}
fn run(
    root: &Path,
    association: Option<CheatPackAssociation>,
    catalogue: &[CheatPackCatalogueGame],
) -> CheatPackPreview {
    let associations = fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().unwrap().is_file())
        .filter_map(|e| {
            association
                .clone()
                .map(|a| (PathBuf::from(e.file_name()), a))
        })
        .collect();
    preview_cheat_pack(
        root,
        catalogue,
        &associations,
        &BTreeSet::new(),
        &CheatPackLimits::default(),
    )
    .unwrap()
}
fn ready() -> (TempDir, CheatPackPreview) {
    let root = tempdir().unwrap();
    write(root.path(), "example.cht", &cht("AAAA"));
    let p = run(root.path(), Some(association()), &[game("g", "Example")]);
    (root, p)
}
fn replan(p: CheatPackPreview) -> CheatPackPreview {
    plan_cheat_pack_preview(p, &BTreeSet::new()).unwrap()
}

#[test]
fn empty_root_is_complete_and_has_no_actions() {
    let r = tempdir().unwrap();
    let p = run(r.path(), None, &[]);
    assert_eq!(p.totals, CheatPackTotals::default());
    assert!(p.complete);
    assert!(p.actions_reconcile());
    assert!(!p.can_apply());
}
#[test]
fn one_valid_file_retains_chain_and_source_enable_is_informational() {
    let (_r, p) = ready();
    assert_eq!(p.totals.files_accepted, 1);
    assert_eq!(p.totals.would_add, 1);
    assert_eq!(p.totals.usable_cheats, 1);
    assert_eq!(p.files[0].observation_indices, vec![0]);
    assert_eq!(p.logical_cheats[0].observation_indices, vec![0]);
    assert_eq!(p.games[0].logical_cheat_indices, vec![0]);
    assert!(p.observations[0].source_enabled_by_default);
    assert!(!p.can_apply());
}
#[test]
fn several_valid_files_and_bad_file_are_isolated() {
    let r = tempdir().unwrap();
    for i in 0..3 {
        write(r.path(), &format!("{i}.cht"), &cht(&format!("CODE{i}")));
    }
    write(r.path(), "bad.cht", b"not a cheat");
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(p.totals.files_discovered, 4);
    assert_eq!(p.totals.files_readable, 4);
    assert_eq!(p.totals.files_accepted, 3);
    assert_eq!(p.totals.files_malformed, 1);
    assert_eq!(p.totals.observations, 3);
    assert_eq!(p.totals.would_review, 3);
    assert!(
        p.files
            .iter()
            .find(|f| f.path == Path::new("bad.cht"))
            .unwrap()
            .diagnostics
            .len()
            > 0
    );
}
#[test]
fn verified_hash_is_exact_and_does_not_use_transient_id_as_identity() {
    let (r, p) = ready();
    assert_eq!(p.files[0].match_strength, CheatPackMatchStrength::Exact);
    let other = run(
        r.path(),
        Some(association()),
        &[game("different-db-id", "Example")],
    );
    assert_eq!(p.logical_cheats[0].key, other.logical_cheats[0].key);
}
#[test]
fn existing_identifier_parser_matches_verified_product_id_strongly() {
    let r = tempdir().unwrap();
    write(
        r.path(),
        "GAFE01.ini",
        b"[Gecko]\n$Lives\n04123456 00000001\n",
    );
    let mut g = game("gc", "Example");
    g.game.platform = Some("GameCube".into());
    g.facts = vec![fact(IdentityKind::DolphinGameId, "GAFE01")];
    let p = run(r.path(), None, &[g]);
    assert_eq!(p.files[0].match_strength, CheatPackMatchStrength::Strong);
    assert_eq!(p.totals.would_add, 1);
}

#[test]
fn metadata_string_without_verified_fact_is_never_exact() {
    let r = tempdir().unwrap();
    write(
        r.path(),
        "SLUS-12345.pnach",
        b"gametitle=Example\npatch=1,EE,00123456,word,1\n",
    );
    let mut g = game("ps2", "Example");
    g.game.platform = Some("PlayStation 2".into());
    g.game.serial = Some("SLUS-12345".into());
    g.facts.clear();
    let p = run(r.path(), None, &[g]);
    assert_eq!(p.files[0].match_strength, CheatPackMatchStrength::Possible);
    assert_eq!(p.totals.would_add, 0);
}
#[test]
fn title_only_is_weak_even_with_platform() {
    let r = tempdir().unwrap();
    write(r.path(), "irrelevant.cht", &cht("A"));
    let a = CheatPackAssociation {
        title: Some("Example".into()),
        platform: Some("NES".into()),
        ..Default::default()
    };
    let p = run(r.path(), Some(a), &[game("g", "Example")]);
    assert_eq!(p.files[0].match_strength, CheatPackMatchStrength::Possible);
    assert_eq!(p.totals.would_review, 1);
}
#[test]
fn filename_only_association_is_weak() {
    let r = tempdir().unwrap();
    write(r.path(), "Example.cht", &cht("A"));
    let p = run(r.path(), None, &[game("g", "Example")]);
    assert_eq!(p.files[0].match_strength, CheatPackMatchStrength::Unmatched);
    assert!(p.files[0].association.title.is_none());
    assert_eq!(p.files[0].association.filename.as_deref(), Some("Example"));
}
#[test]
fn platform_only_is_not_a_wildcard() {
    let r = tempdir().unwrap();
    write(r.path(), "irrelevant.cht", &cht("A"));
    let a = CheatPackAssociation {
        platform: Some("NES".into()),
        ..Default::default()
    };
    let p = run(r.path(), Some(a), &[game("g", "Example")]);
    assert_eq!(p.totals.unmatched_games, 1);
}
#[test]
fn ambiguous_identity_retains_all_candidates_and_no_winner() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    let p = run(
        r.path(),
        Some(association()),
        &[game("a", "Example"), game("b", "Example")],
    );
    assert_eq!(p.files[0].match_strength, CheatPackMatchStrength::Ambiguous);
    assert_eq!(p.files[0].matches.len(), 2);
    assert_eq!(p.totals.would_remain_ambiguous, 1);
    assert_eq!(p.totals.would_add, 0);
}
#[test]
fn unmatched_is_explicit() {
    let r = tempdir().unwrap();
    write(r.path(), "Unknown.cht", &cht("A"));
    let p = run(r.path(), None, &[game("g", "Example")]);
    assert_eq!(p.totals.would_remain_unmatched, 1);
    assert_eq!(p.totals.unmatched_games, 1);
}
#[test]
fn conflicting_verified_requirements_do_not_fall_back_to_title() {
    let r = tempdir().unwrap();
    write(r.path(), "Example.cht", &cht("A"));
    let mut a = association();
    a.identities.push(CheatPackIdentityRequirement {
        kind: IdentityKind::Ps2Serial,
        value: "SLUS-12345".into(),
    });
    let p = run(r.path(), Some(a), &[game("g", "Example")]);
    assert_eq!(p.files[0].match_strength, CheatPackMatchStrength::Exact);
    assert_eq!(p.totals.would_add, 0);
    assert!(
        p.observations[0]
            .assessment
            .as_ref()
            .unwrap()
            .blockers
            .contains(&CheatApplicabilityIssue::RequiredIdentityUnknown)
    );
}
#[test]
fn contradictory_catalogue_facts_cannot_supply_exact_match() {
    let r = tempdir().unwrap();
    write(r.path(), "Example.cht", &cht("A"));
    let mut g = game("g", "Example");
    g.facts
        .push(fact(IdentityKind::LooseRomSha256, &"b".repeat(64)));
    let p = run(r.path(), Some(association()), &[g]);
    assert_eq!(p.files[0].match_strength, CheatPackMatchStrength::Ambiguous);
}
#[test]
fn exact_duplicate_is_one_logical_cheat_and_known_copy_not_independent_source() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    write(r.path(), "b.cht", &cht("A"));
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(p.totals.observations, 2);
    assert_eq!(p.totals.logical_cheats, 1);
    assert_eq!(p.totals.exact_duplicates, 1);
    assert_eq!(p.totals.usable_cheats, 1);
    assert_eq!(p.totals.would_add, 1);
    assert_eq!(p.totals.would_retain_existing, 1);
    assert_eq!(p.logical_cheats[0].known_copies, 1);
    assert_eq!(p.logical_cheats[0].independent_source_groups, 0);
    assert_eq!(p.totals.corroborating_sources, 0);
}
#[test]
fn understood_equivalent_code_reuses_main_semantic_key() {
    let r = tempdir().unwrap();
    write(r.path(), "a.pnach", b"patch=1,EE,00123456,word,00000001\n");
    write(r.path(), "b.pnach", b"patch=1,EE,00123456,word,1\n");
    let mut a = association();
    a.platform = Some("PlayStation 2".into());
    let mut g = game("g", "Example");
    g.game.platform = a.platform.clone();
    let p = run(r.path(), Some(a), &[g]);
    assert_eq!(p.totals.logical_cheats, 1);
    assert_eq!(p.totals.exact_duplicates, 1);
    assert_eq!(p.totals.would_corroborate, 1);
}
#[test]
fn cross_source_observation_corroborates_without_fake_independence() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    let mut other = b"# source B\n".to_vec();
    other.extend(cht("A"));
    write(r.path(), "b.cht", &other);
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(p.totals.would_corroborate, 1);
    assert_eq!(p.logical_cheats[0].distinct_source_contents, 2);
    assert_eq!(p.logical_cheats[0].independent_source_groups, 0);
}
#[test]
fn duplicate_subset_does_not_hide_same_name_code_conflict() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    write(r.path(), "b.cht", &cht("A"));
    write(r.path(), "c.cht", &cht("B"));
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(p.totals.logical_cheats, 1);
    assert_eq!(p.totals.conflicts, 1);
    assert_eq!(p.totals.would_review, 3);
    assert_eq!(p.totals.would_add, 0);
    assert_eq!(p.totals.usable_cheats, 0);
    assert!(
        p.logical_cheats[0]
            .reconciliation
            .as_ref()
            .unwrap()
            .auto_winner
            .is_none()
    );
}
#[test]
fn conflicting_duplicate_index_field_remains_visible_and_review_only() {
    let r = tempdir().unwrap();
    let mut bytes = cht("A");
    bytes.extend(b"cheat0_code = \"B\"\n");
    write(r.path(), "a.cht", &bytes);
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert!(p.observations[0].source_index_conflict);
    assert!(
        p.logical_cheats[0]
            .relationships
            .contains(&CheatDuplicateKind::SourceIndexConflict)
    );
    assert_eq!(p.observations[0].raw_code, "A");
    assert_eq!(p.totals.would_reject, 1);
}
#[test]
fn region_variant_is_not_collapsed() {
    let (_r, mut p) = ready();
    let mut o = p.observations[0].clone();
    p.observations[0].association.region = Some("US".into());
    o.association.region = Some("EU".into());
    p.observations.push(o);
    let p = replan(p);
    assert!(
        p.logical_cheats[0]
            .relationships
            .contains(&CheatDuplicateKind::RegionVariant)
    );
    assert_eq!(p.totals.would_review, 2);
}
#[test]
fn revision_variant_is_not_collapsed() {
    let (_r, mut p) = ready();
    let mut o = p.observations[0].clone();
    p.observations[0].association.revision = Some("1".into());
    o.association.revision = Some("2".into());
    p.observations.push(o);
    let p = replan(p);
    assert!(
        p.logical_cheats[0]
            .relationships
            .contains(&CheatDuplicateKind::VersionVariant)
    );
    assert_eq!(p.totals.would_review, 2);
}
#[test]
fn wrong_region_is_based_on_verified_release_fact() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    let mut g = game("g", "Example");
    g.facts.push(fact(IdentityKind::DolphinRegion, "US"));
    let mut a = association();
    a.region = Some("EU".into());
    let p = run(r.path(), Some(a), &[g]);
    assert_eq!(p.totals.region_mismatches, 1);
    assert_eq!(p.totals.would_review, 1);
}
#[test]
fn wrong_revision_is_based_on_verified_release_fact() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    let mut g = game("g", "Example");
    g.facts.push(fact(IdentityKind::DolphinRevision, "1"));
    let mut a = association();
    a.revision = Some("2".into());
    let p = run(r.path(), Some(a), &[g]);
    assert_eq!(p.totals.revision_mismatches, 1);
}
#[test]
fn missing_release_evidence_is_review_not_a_wildcard() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    let mut a = association();
    a.region = Some("US".into());
    let p = run(r.path(), Some(a), &[game("g", "Example")]);
    assert_eq!(p.totals.would_review, 1);
    assert_eq!(p.totals.would_add, 0);
}
#[test]
fn unsupported_format_does_not_hide_known_files() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    write(r.path(), "pack.zip", b"not extracted");
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(p.totals.unsupported_formats, 1);
    assert_eq!(p.totals.files_rejected, 1);
    assert_eq!(p.totals.would_add, 1);
}
#[test]
fn malformed_entry_between_valid_entries_preserves_neighbours() {
    let r = tempdir().unwrap();
    write(r.path(),"a.cht",b"cheats=3\ncheat0_desc=Lives\ncheat0_code=A\ncheat1_desc=Broken\ncheat1_code=\ncheat2_desc=Level\ncheat2_code=C\n");
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(p.totals.observations, 3);
    assert_eq!(p.totals.would_add, 2);
    assert_eq!(p.totals.would_reject, 1);
    assert_eq!(p.totals.malformed_cheats, 1);
    assert!(p.actions_reconcile());
}
#[test]
fn source_rom_saves_config_catalogue_and_existing_snapshot_are_unmodified() {
    let r = tempdir().unwrap();
    let external = tempdir().unwrap();
    let source = write(r.path(), "a.cht", &cht("A"));
    let rom = write(external.path(), "game.rom", b"ROM");
    let save = write(external.path(), "save.srm", b"SAVE");
    let config = write(external.path(), "retroarch.cfg", b"CONFIG");
    let database = write(external.path(), "cheats.sqlite", b"DB");
    let before: Vec<_> = [&source, &rom, &save, &config, &database]
        .into_iter()
        .map(|p| fs::read(p).unwrap())
        .collect();
    let catalogue = vec![game("g", "Example")];
    let initial = catalogue.clone();
    let existing = BTreeSet::from([String::from("existing-key")]);
    let associations = BTreeMap::from([(PathBuf::from("a.cht"), association())]);
    let p = preview_cheat_pack(
        r.path(),
        &catalogue,
        &associations,
        &existing,
        &CheatPackLimits::default(),
    )
    .unwrap();
    assert!(!p.can_apply());
    assert_eq!(catalogue, initial);
    assert_eq!(existing, BTreeSet::from([String::from("existing-key")]));
    for (p, b) in [&source, &rom, &save, &config, &database]
        .into_iter()
        .zip(before)
    {
        assert_eq!(fs::read(p).unwrap(), b);
    }
    assert_eq!(fs::read_dir(r.path()).unwrap().count(), 1);
    assert_eq!(fs::read_dir(external.path()).unwrap().count(), 4);
}
#[test]
fn repeated_previews_are_identical_without_clock_or_cache_state() {
    let (r, p) = ready();
    let next = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(p, next);
    assert_eq!(
        serde_json::to_vec(&p).unwrap(),
        serde_json::to_vec(&next).unwrap()
    );
}
#[test]
fn catalogue_order_does_not_change_ambiguity_or_output() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    let games = [game("a", "Example"), game("b", "Example")];
    let p = run(r.path(), Some(association()), &games);
    let q = run(
        r.path(),
        Some(association()),
        &[games[1].clone(), games[0].clone()],
    );
    assert_eq!(p, q);
}
#[test]
fn prepared_enumeration_order_has_identical_plan() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    write(r.path(), "b.cht", &cht("B"));
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    let mut q = p.clone();
    q.files.reverse();
    for o in &mut q.observations {
        o.file_index = 1 - o.file_index;
    }
    q.observations.reverse();
    assert_eq!(replan(q), replan(p));
}
#[test]
fn file_creation_order_has_no_effect_on_sorted_detail() {
    let a = tempdir().unwrap();
    let b = tempdir().unwrap();
    for name in ["z.cht", "a.cht", "m.cht"] {
        write(a.path(), name, &cht(name));
    }
    for name in ["m.cht", "a.cht", "z.cht"] {
        write(b.path(), name, &cht(name));
    }
    let p = run(a.path(), Some(association()), &[game("g", "Example")]);
    let q = run(b.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(
        p.files.iter().map(|f| &f.path).collect::<Vec<_>>(),
        q.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(p.totals, q.totals);
    assert_eq!(
        p.observations
            .iter()
            .map(|o| (&o.logical_key, o.action))
            .collect::<Vec<_>>(),
        q.observations
            .iter()
            .map(|o| (&o.logical_key, o.action))
            .collect::<Vec<_>>()
    );
}
#[test]
fn source_path_changes_preserve_logical_keys_with_same_evidence() {
    let (r, p) = ready();
    let other = tempdir().unwrap();
    write(
        other.path(),
        "different-name.cht",
        &fs::read(r.path().join("example.cht")).unwrap(),
    );
    let q = run(other.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(p.observations[0].logical_key, q.observations[0].logical_key);
    assert_ne!(
        p.observations[0].provenance.original_path,
        q.observations[0].provenance.original_path
    );
}
#[test]
fn code_region_revision_and_engine_changes_change_identity() {
    let (_r, p) = ready();
    let key = p.observations[0].logical_key.clone();
    for variant in 0..4 {
        let mut q = p.clone();
        let o = &mut q.observations[0];
        match variant {
            0 => o.raw_code = "B".into(),
            1 => o.association.region = Some("EU".into()),
            2 => o.association.revision = Some("2".into()),
            _ => o.engine = Some("different".into()),
        };
        assert_ne!(replan(q).observations[0].logical_key, key);
    }
}
#[test]
fn opaque_code_whitespace_is_not_over_normalized() {
    let (_r, p) = ready();
    let mut q = p.clone();
    q.observations[0].raw_code = "AA AA".into();
    assert_ne!(
        p.observations[0].logical_key,
        replan(q).observations[0].logical_key
    );
}
#[test]
fn provenance_groups_and_mirror_evidence_are_separate() {
    let (_r, mut p) = ready();
    p.observations[0].source_group = Some("A".into());
    let mut second = p.observations[0].clone();
    second.source_group = Some("B".into());
    second.provenance.source_sha256 = "source-b".into();
    let mut mirror = second.clone();
    mirror.mirror_of = Some("B".into());
    mirror.source_group = Some("mirror".into());
    mirror.provenance.source_sha256 = "mirror".into();
    p.observations.extend([second, mirror]);
    let p = replan(p);
    assert_eq!(p.logical_cheats[0].observation_indices.len(), 3);
    assert_eq!(p.logical_cheats[0].independent_source_groups, 2);
    assert_eq!(p.logical_cheats[0].known_mirrors, 1);
    assert_eq!(
        p.observations[2].action,
        CheatPackAction::WouldRetainExisting
    );
}
#[test]
fn aggregate_partitions_reconcile_with_detail() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    write(r.path(), "b.cht", &cht("A"));
    write(r.path(), "broken.cht", b"cheat0_desc=Broken\n");
    write(r.path(), "unknown.txt", b"unsupported");
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert!(p.actions_reconcile());
    assert_eq!(p.totals.observations, p.observations.len());
    assert_eq!(p.totals.logical_cheats, p.logical_cheats.len());
    assert_eq!(
        p.totals.files_discovered,
        p.totals.files_accepted + p.totals.files_malformed + p.totals.files_rejected
    );
    assert_eq!(
        p.files
            .iter()
            .map(|f| f.observation_indices.len())
            .sum::<usize>(),
        p.observations.len()
    );
}
#[test]
fn large_synthetic_pack_uses_one_group_without_pairwise_expansion() {
    let r = tempdir().unwrap();
    for i in 0..2048 {
        let mut bytes = format!("# independent bytes {i}\n").into_bytes();
        bytes.extend(cht("A"));
        write(r.path(), &format!("{i:04}.cht"), &bytes);
    }
    let start = std::time::Instant::now();
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    eprintln!("2048-file pack preview: {:?}", start.elapsed());
    assert_eq!(p.totals.observations, 2048);
    assert_eq!(p.totals.logical_cheats, 1);
    assert_eq!(p.totals.would_add, 1);
    assert_eq!(p.totals.would_corroborate, 2047);
    assert_eq!(
        p.logical_cheats[0]
            .reconciliation
            .as_ref()
            .unwrap()
            .groups
            .len(),
        1
    );
    assert!(p.complete);
    assert!(p.actions_reconcile());
}
#[test]
fn overflowing_directory_is_rejected_deterministically_not_arbitrary_prefix() {
    let r = tempdir().unwrap();
    for i in 0..4 {
        write(r.path(), &format!("{i}.cht"), &cht("A"));
    }
    let mut l = CheatPackLimits::default();
    l.source.max_files_visited = 3;
    let p = preview_cheat_pack(r.path(), &[], &BTreeMap::new(), &BTreeSet::new(), &l).unwrap();
    assert!(!p.complete);
    assert!(p.files.is_empty());
    assert!(p.diagnostics.iter().any(|d|matches!(d,CheatPackDiagnostic::Source(s) if s.kind==super::super::user_cheat_import::UserCheatDiagnosticKind::FileLimitReached)));
}
#[test]
fn nested_enumeration_shares_budget_and_preserves_collected_root_neighbour() {
    let r = tempdir().unwrap();
    for i in 0..4 {
        write(r.path(), &format!("a/{i}.cht"), &cht("A"));
    }
    write(r.path(), "b/hidden.cht", &cht("B"));
    let root_bytes = cht("ROOT");
    let root_file = write(r.path(), "z.cht", &root_bytes);
    let mut limits = CheatPackLimits::default();
    // Three root entries leave three entries for the entire descendant tree.
    // The four-entry child is refused as a whole and exhausts that remainder.
    limits.source.max_files_visited = 6;
    let preview =
        || preview_cheat_pack(r.path(), &[], &BTreeMap::new(), &BTreeSet::new(), &limits).unwrap();
    let p = preview();
    assert!(!p.complete);
    assert_eq!(p.files.len(), 1);
    assert_eq!(p.files[0].path, PathBuf::from("z.cht"));
    assert_eq!(p.totals.observations, 1);
    assert!(p.actions_reconcile());
    let diagnostic_paths: Vec<_> = p
        .diagnostics
        .iter()
        .filter_map(|d| {
            match d {
            CheatPackDiagnostic::Source(s)
                if s.kind
                    == super::super::user_cheat_import::UserCheatDiagnosticKind::FileLimitReached =>
            {
                Some(s.path.strip_prefix(r.path()).unwrap().to_path_buf())
            }
            _ => None,
        }
        })
        .collect();
    assert_eq!(
        diagnostic_paths,
        vec![PathBuf::from("a"), PathBuf::from("b")]
    );
    assert_eq!(p, preview());
    assert_eq!(fs::read(root_file).unwrap(), root_bytes);
    assert!(!p.can_apply());
}
#[test]
fn file_line_code_and_observation_bounds_are_explicit() {
    let r = tempdir().unwrap();
    write(r.path(), "oversize.cht", &vec![b'A'; 9000]);
    write(
        r.path(),
        "entries.cht",
        b"cheat0_desc=One\ncheat0_code=A\ncheat1_desc=Two\ncheat1_code=B\n",
    );
    let mut l = CheatPackLimits::default();
    l.source.max_cheats_per_file = 1;
    let p = preview_cheat_pack(r.path(), &[], &BTreeMap::new(), &BTreeSet::new(), &l).unwrap();
    assert!(!p.complete);
    assert_eq!(p.totals.observations, 1);
    assert_eq!(p.totals.files_rejected, 1);
}
#[test]
fn impossible_limit_settings_are_refused_before_scan() {
    let r = tempdir().unwrap();
    let mut l = CheatPackLimits::default();
    l.source.max_file_bytes = u64::MAX;
    l.source.max_total_bytes = u64::MAX;
    assert!(preview_cheat_pack(r.path(), &[], &BTreeMap::new(), &BTreeSet::new(), &l).is_err());
}
#[test]
fn existing_keys_retain_without_mutation() {
    let (_r, p) = ready();
    let existing = BTreeSet::from([p.observations[0].logical_key.clone()]);
    let p = plan_cheat_pack_preview(p, &existing).unwrap();
    assert_eq!(p.totals.would_retain_existing, 1);
    assert_eq!(p.totals.would_add, 0);
    assert_eq!(existing.len(), 1);
}
#[test]
fn malformed_prepared_reference_returns_error_without_panic() {
    let (_r, mut p) = ready();
    p.observations[0].file_index = usize::MAX;
    assert!(plan_cheat_pack_preview(p, &BTreeSet::new()).is_err());
}
#[test]
fn code_conflict_between_interpretations_is_not_an_equivalent_duplicate() {
    let (_r, mut p) = ready();
    let mut o = p.observations[0].clone();
    o.document.operations = vec![cheat_ir::CheatOperation::Write8 {
        address: 1,
        value: 1,
    }];
    p.observations.push(o);
    let p = replan(p);
    assert!(
        p.logical_cheats[0]
            .relationships
            .contains(&CheatDuplicateKind::AmbiguousPossibleDuplicate)
    );
    assert_eq!(p.totals.would_review, 2);
}
#[test]
fn syntax_engine_variant_requires_review() {
    let (_r, mut p) = ready();
    let mut o = p.observations[0].clone();
    p.observations[0].engine = Some("0".into());
    o.engine = Some("1".into());
    p.observations.push(o);
    let p = replan(p);
    assert!(
        p.logical_cheats[0]
            .relationships
            .contains(&CheatDuplicateKind::SyntaxVariant)
    );
    assert_eq!(p.totals.would_review, 2);
}
#[test]
fn unsupported_target_seam_rejects_without_apply() {
    let (_r, mut p) = ready();
    use crate::patch_manager::{CheatApplySupport, CheatRoute, CheatRouteBasis, CheatRouteTarget};
    p.observations[0].assessment.as_mut().unwrap().support.route = Some(CheatRoute {
        platform_id: "NES".into(),
        target: CheatRouteTarget::standalone("unsupported"),
        basis: CheatRouteBasis::ExplicitSelection,
        apply_support: CheatApplySupport::Unsupported,
        native_format: "unsupported",
        alternatives: vec![],
    });
    let p = replan(p);
    assert_eq!(p.totals.unsupported_targets, 1);
    assert_eq!(p.totals.would_reject, 1);
    assert!(!p.can_apply());
}
#[test]
fn different_verified_games_never_share_source_index_bucket() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    write(r.path(), "b.cht", &cht("A"));
    let mut other = game("b", "Other");
    other.facts = vec![fact(IdentityKind::LooseRomSha256, &"b".repeat(64))];
    let mut b = association();
    b.identities[0].value = "b".repeat(64);
    let a = BTreeMap::from([
        (PathBuf::from("a.cht"), association()),
        (PathBuf::from("b.cht"), b),
    ]);
    let p = preview_cheat_pack(
        r.path(),
        &[game("a", "Example"), other],
        &a,
        &BTreeSet::new(),
        &CheatPackLimits::default(),
    )
    .unwrap();
    assert_eq!(p.totals.games_represented, 2);
    assert_eq!(p.totals.logical_cheats, 2);
    assert_eq!(p.totals.would_add, 2);
}
#[test]
fn existing_dolphin_and_xenia_local_formats_have_observation_detail() {
    let r = tempdir().unwrap();
    write(
        r.path(),
        "GAFE01.ini",
        b"[Gecko]\n$Lives\n04123456 00000001\n",
    );
    write(r.path(),"example.patch.toml",b"title_name=\"Example\"\ntitle_id=\"12345678\"\nhash=\"0123456789ABCDEF\"\n[[patch]]\nname=\"Lives\"\ndesc=\"test\"\nauthor=\"fixture\"\nis_enabled=false\n[[patch.be32]]\naddress=0x123456\nvalue=1\n");
    let p = run(r.path(), None, &[]);
    assert_eq!(p.totals.files_accepted, 2);
    assert_eq!(p.totals.observations, 2);
}
#[cfg(unix)]
#[test]
fn symlinks_and_symlinked_ancestors_cannot_escape_root() {
    use std::os::unix::fs::symlink;
    let r = tempdir().unwrap();
    let outside = tempdir().unwrap();
    write(outside.path(), "outside.cht", &cht("A"));
    symlink(
        outside.path().join("outside.cht"),
        r.path().join("escape.cht"),
    )
    .unwrap();
    symlink(outside.path(), r.path().join("escape-dir")).unwrap();
    write(r.path(), "safe.cht", &cht("B"));
    let p = run(r.path(), None, &[]);
    assert_eq!(p.files.len(), 1);
    assert_eq!(p.files[0].path, PathBuf::from("safe.cht"));
    assert_eq!(p.diagnostics.len(), 2);
    assert!(
        preview_cheat_pack(
            &r.path().join("escape-dir"),
            &[],
            &BTreeMap::new(),
            &BTreeSet::new(),
            &CheatPackLimits::default()
        )
        .is_err()
    );
    let alias = r.path().join("alias");
    symlink(outside.path(), &alias).unwrap();
    assert!(
        preview_cheat_pack(
            &alias.join("outside.cht"),
            &[],
            &BTreeMap::new(),
            &BTreeSet::new(),
            &CheatPackLimits::default()
        )
        .unwrap()
        .files[0]
            .state
            == CheatPackFileState::Unreadable
    );
}
#[test]
fn bounded_malformed_fixture_sweep_never_panics() {
    let r = tempdir().unwrap();
    let mut seed = 42u32;
    for i in 0..64 {
        let bytes: Vec<_> = (0..256)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        write(r.path(), &format!("{i}.cht"), &bytes);
    }
    let p = run(r.path(), None, &[]);
    assert_eq!(p.totals.files_discovered, 64);
    assert_eq!(p.totals.files_malformed, 64);
    assert!(p.actions_reconcile());
}

#[test]
fn recognized_pnach_crc_requires_verified_fact_and_is_exact() {
    let r = tempdir().unwrap();
    write(
        r.path(),
        "SLUS-12345_A1B2C3D4.pnach",
        b"gametitle=Example\npatch=1,EE,00123456,word,1\n",
    );
    let mut g = game("ps2", "Example");
    g.game.platform = Some("PlayStation 2".into());
    g.facts = vec![
        fact(IdentityKind::Ps2Serial, "SLUS-12345"),
        fact(IdentityKind::Pcsx2ExecutableCrc, "A1B2C3D4"),
    ];
    let p = run(r.path(), None, &[g]);
    assert_eq!(p.files[0].match_strength, CheatPackMatchStrength::Strong);
}
#[test]
fn pnach_execution_mode_is_part_of_logical_identity() {
    let r = tempdir().unwrap();
    write(r.path(), "a.pnach", b"patch=1,EE,00123456,word,1\n");
    write(r.path(), "b.pnach", b"patch=0,EE,00123456,word,1\n");
    let mut a = association();
    a.platform = Some("PlayStation 2".into());
    let mut g = game("g", "Example");
    g.game.platform = a.platform.clone();
    let p = run(r.path(), Some(a), &[g]);
    assert_ne!(p.observations[0].logical_key, p.observations[1].logical_key);
    assert_eq!(p.totals.would_review, 2);
}
#[test]
fn diagnostics_have_one_pack_wide_retention_budget() {
    let r = tempdir().unwrap();
    for i in 0..8 {
        write(r.path(), &format!("{i}.cht"), b"cheat0_code=A\n");
    }
    let mut l = CheatPackLimits::default();
    l.source.max_warnings = 3;
    let p = preview_cheat_pack(r.path(), &[], &BTreeMap::new(), &BTreeSet::new(), &l).unwrap();
    let retained = p.diagnostics.len()
        + p.files.iter().map(|f| f.diagnostics.len()).sum::<usize>()
        + p.observations
            .iter()
            .map(|o| o.diagnostics.len())
            .sum::<usize>();
    assert!(retained <= 3);
    assert!(!p.complete);
    assert!(p.observations.iter().any(|o| o.diagnostics_truncated));
    assert!(p.actions_reconcile());
}
#[test]
fn source_comments_globals_and_entry_extra_fields_are_retained() {
    let r = tempdir().unwrap();
    let mut bytes = b"# provenance comment\ncheat_delay=5\n".to_vec();
    bytes.extend(cht("A"));
    bytes.extend(b"cheat0_handler=0\n");
    write(r.path(), "a.cht", &bytes);
    let p = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert_eq!(p.files[0].source_comments, vec!["provenance comment"]);
    assert_eq!(p.files[0].source_metadata["cheat_delay"], "5");
    assert_eq!(p.observations[0].execution_fields["entry:handler"], "0");
}
#[test]
fn root_file_and_directory_preview_share_same_logical_identity() {
    let (r, p) = ready();
    let associations = BTreeMap::from([(PathBuf::from("example.cht"), association())]);
    let q = preview_cheat_pack(
        &r.path().join("example.cht"),
        &[game("g", "Example")],
        &associations,
        &BTreeSet::new(),
        &CheatPackLimits::default(),
    )
    .unwrap();
    assert_eq!(p.observations[0].logical_key, q.observations[0].logical_key);
}
#[test]
fn source_changes_are_observed_without_cache() {
    let (r, p) = ready();
    write(r.path(), "example.cht", &cht("CHANGED"));
    let q = run(r.path(), Some(association()), &[game("g", "Example")]);
    assert_ne!(p.observations[0].logical_key, q.observations[0].logical_key);
    assert_ne!(
        p.observations[0].provenance.source_sha256,
        q.observations[0].provenance.source_sha256
    );
}
#[test]
fn depth_and_total_byte_bounds_leave_independent_evidence() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    write(r.path(), "deep/nested/b.cht", &cht("B"));
    let mut l = CheatPackLimits::default();
    l.source.max_depth = 1;
    let p = preview_cheat_pack(r.path(), &[], &BTreeMap::new(), &BTreeSet::new(), &l).unwrap();
    assert!(!p.complete);
    assert_eq!(p.files.len(), 1);
    l.source.max_file_bytes = cht("A").len() as u64;
    l.source.max_total_bytes = l.source.max_file_bytes;
    l.source.max_depth = 16;
    let p = preview_cheat_pack(r.path(), &[], &BTreeMap::new(), &BTreeSet::new(), &l).unwrap();
    assert!(!p.complete);
    assert_eq!(p.totals.observations, 1);
    assert_eq!(p.totals.files_rejected, 1);
}

#[test]
fn candidate_overflow_is_bounded_ambiguous_and_catalogue_order_independent() {
    let r = tempdir().unwrap();
    write(r.path(), "Example.cht", &cht("A"));
    let mut games: Vec<_> = (0..64)
        .map(|i| game(&format!("{i:03}"), "Example"))
        .collect();
    let mut l = CheatPackLimits::default();
    l.max_matches_per_file = 3;
    let p = preview_cheat_pack(r.path(), &games, &BTreeMap::new(), &BTreeSet::new(), &l).unwrap();
    assert_eq!(p.files[0].matches.len(), 3);
    assert!(p.files[0].matches_truncated);
    assert!(!p.complete);
    assert_eq!(p.totals.would_remain_ambiguous, 1);
    games.reverse();
    let q = preview_cheat_pack(r.path(), &games, &BTreeMap::new(), &BTreeSet::new(), &l).unwrap();
    assert_eq!(p, q);
}
#[test]
fn oversized_native_code_retains_explicit_sample_and_digest_only() {
    let r = tempdir().unwrap();
    let bytes = (0..200)
        .map(|_| "patch=1,EE,00123456,word,1\n")
        .collect::<String>();
    write(r.path(), "a.pnach", bytes.as_bytes());
    let p = run(r.path(), None, &[]);
    assert_eq!(p.observations.len(), 1);
    assert!(p.observations[0].code_truncated);
    assert!(p.observations[0].raw_code.len() <= 4096);
    assert!(p.observations[0].document.operations.is_empty());
    assert!(p.observations[0].full_code_digest.is_some());
    assert_eq!(p.totals.would_reject, 1);
}

#[test]
fn missing_variant_constraint_is_an_ambiguous_possible_duplicate() {
    let (_r, mut p) = ready();
    let mut o = p.observations[0].clone();
    o.source_index = None;
    o.engine = Some("known".into());
    p.observations.push(o);
    let p = replan(p);
    assert!(
        p.logical_cheats[0]
            .relationships
            .contains(&CheatDuplicateKind::AmbiguousPossibleDuplicate)
    );
    assert_eq!(p.totals.would_review, 2);
    assert_eq!(p.totals.would_add, 0);
}

#[test]
fn refused_growing_read_still_counts_bytes_against_pack_budget() {
    let r = tempdir().unwrap();
    let path = write(r.path(), "a.cht", b"123456789");
    let mut consumed = 0;
    assert!(
        super::super::user_cheat_import::read_bounded_with_counter(&path, 1, 4, &mut consumed)
            .is_err()
    );
    assert_eq!(consumed, 5); // four payload bytes plus the required growth probe
}

#[test]
fn unverified_game_association_does_not_prove_logical_duplicates() {
    let r = tempdir().unwrap();
    write(r.path(), "a.cht", &cht("A"));
    write(r.path(), "b.cht", &cht("A"));
    let a = CheatPackAssociation {
        title: Some("Example".into()),
        ..Default::default()
    };
    let p = run(r.path(), Some(a), &[game("g", "Example")]);
    assert_eq!(p.totals.exact_duplicates, 0);
    assert!(
        p.logical_cheats[0]
            .relationships
            .contains(&CheatDuplicateKind::AmbiguousPossibleDuplicate)
    );
    assert_eq!(p.totals.would_review, 2);
}

#[test]
fn pack_applicability_is_a_projection_of_the_canonical_assessor() {
    for case in [
        "exact",
        "ambiguous",
        "conflicting",
        "candidate",
        "region",
        "revision",
        "duplicate",
        "variant",
    ] {
        let temp = tempdir().unwrap();
        write(temp.path(), "a.cht", &cht("A"));
        if matches!(case, "duplicate" | "variant") {
            write(
                temp.path(),
                "b.cht",
                &cht(if case == "variant" { "B" } else { "A" }),
            );
        }
        let mut game = game("g", "Example");
        let mut association = association();
        match case {
            "ambiguous" | "conflicting" => {
                let mut other = fact(IdentityKind::LooseRomSha256, &"b".repeat(64));
                if case == "ambiguous" {
                    other.status = IdentityStatus::Ambiguous;
                }
                game.facts.push(other);
            }
            "candidate" => game.facts[0].status = IdentityStatus::Candidate,
            "region" => {
                association.region = Some("Europe".into());
                game.facts.push(fact(IdentityKind::DolphinRegion, "USA"));
            }
            "revision" => {
                association.revision = Some("1".into());
                game.facts.push(fact(IdentityKind::DolphinRevision, "2"));
            }
            _ => {}
        }
        let preview = run(temp.path(), Some(association.clone()), &[game.clone()]);
        assert!(!preview.can_apply());
        for o in &preview.observations {
            let group = preview
                .logical_cheats
                .iter()
                .find(|g| {
                    g.observation_indices
                        .iter()
                        .any(|&i| std::ptr::eq(&preview.observations[i], o))
                })
                .unwrap();
            let canonical = assess_cheat_applicability(&CheatApplicabilityInput {
                game: selected_game(&game),
                association: association.clone(),
                document: o.document.clone(),
                parsing: CheatParseEvidence::Valid,
                native_cht: o.native_cht.clone(),
                route: None,
                reconciliation: group.reconciliation.clone(),
            });
            let actual = o.assessment.as_ref().unwrap();
            assert_eq!(o.applicability, canonical.state, "{case}");
            assert_eq!(actual.identity_match, canonical.identity_match, "{case}");
            assert_eq!(actual.blockers, canonical.blockers, "{case}");
            if matches!(case, "ambiguous" | "conflicting" | "region" | "revision") {
                assert!(
                    actual.blockers.iter().any(|b| b.is_hard_refusal()),
                    "{case}"
                );
                assert!(!matches!(
                    o.action,
                    CheatPackAction::WouldAdd | CheatPackAction::WouldCorroborate
                ));
            }
            if matches!(case, "duplicate" | "variant") {
                let report = group.reconciliation.as_ref().unwrap();
                let CheatReconciliationOutcome::Ready(expected) =
                    cheat_ir::reconcile_cheats_for_game(report.entries.clone())
                else {
                    panic!("{case}")
                };
                assert_eq!(
                    group.relationships,
                    expected.groups[0].classifications.iter().copied().collect()
                );
            }
        }
    }
}
