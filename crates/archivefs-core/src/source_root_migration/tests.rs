use super::*;
use std::fs;

fn ref_(id: &str, path: &str, semantics: PathSemantics) -> MigrationReference {
    MigrationReference {
        id: id.into(),
        subsystem: "test".into(),
        path: path.into(),
        semantics,
        migratable: true,
        requires_existence: true,
        historical: false,
        live: true,
        claimed_destination: None,
    }
}

#[test]
fn exact_suffix_is_rebased_without_fuzzy_matching() {
    let root = std::env::temp_dir().join("archivefs-migration-contract");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(new.join("nes/subdir")).unwrap();
    fs::write(new.join("nes/subdir/game.rom"), b"x").unwrap();
    let mut r = ref_(
        "rom",
        &old.join("nes/subdir/game.rom").to_string_lossy(),
        PathSemantics::SourceRootAbsolute,
    );
    let p = plan_rebase(&old, &new, &r);
    assert_eq!(p.classification, MigrationClassification::SafeRebase);
    assert_eq!(p.candidate_path, Some(new.join("nes/subdir/game.rom")));
    r.path = old.join("nes/subdir/Game Different.rom");
    assert_eq!(
        plan_rebase(&old, &new, &r).classification,
        MigrationClassification::TargetMissing
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn classifications_fail_closed_and_history_is_preserved() {
    let old = PathBuf::from("/old");
    let new = PathBuf::from("/new");
    let mut outside = ref_(
        "outside",
        "/elsewhere/game",
        PathSemantics::SourceRootAbsolute,
    );
    outside.requires_existence = false;
    assert_eq!(
        plan_rebase(&old, &new, &outside).classification,
        MigrationClassification::OutsideNewRoot
    );
    let mut history = ref_("history", "/old/game", PathSemantics::TransactionHistorical);
    history.historical = true;
    assert_eq!(
        plan_rebase(&old, &new, &history).classification,
        MigrationClassification::HistoricalOnly
    );
    let mut emulator = ref_("emu", "/bin/emulator", PathSemantics::UserSelectedExternal);
    emulator.migratable = false;
    assert_eq!(
        plan_rebase(&old, &new, &emulator).classification,
        MigrationClassification::NotApplicable
    );
    let mut cache = ref_("cache", "/cache/derived", PathSemantics::CacheInternal);
    cache.migratable = false;
    assert_eq!(
        plan_rebase(&old, &new, &cache).classification,
        MigrationClassification::RegenerateInstead
    );
}

#[cfg(unix)]
#[test]
fn source_symlink_requires_review_and_destination_symlink_is_rejected() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join("archivefs-migration-symlink-contract");
    let old = root.join("old");
    let new = root.join("new");
    let outside = root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();
    fs::write(outside.join("game.rom"), b"x").unwrap();
    symlink(outside.join("game.rom"), old.join("game.rom")).unwrap();
    let source = ref_(
        "source-link",
        &old.join("game.rom").to_string_lossy(),
        PathSemantics::SourceRootAbsolute,
    );
    assert_eq!(
        plan_rebase(&old, &new, &source).classification,
        MigrationClassification::ManualReview
    );
    fs::write(old.join("other.rom"), b"x").unwrap();
    symlink(outside.join("game.rom"), new.join("other.rom")).unwrap();
    let destination = ref_(
        "destination-link",
        &old.join("other.rom").to_string_lossy(),
        PathSemantics::SourceRootAbsolute,
    );
    assert_eq!(
        plan_rebase(&old, &new, &destination).classification,
        MigrationClassification::TargetMissing
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsupported_and_destination_owned_paths_are_not_source_rebased() {
    let old = PathBuf::from("/old");
    let new = PathBuf::from("/new");
    for (semantics, expected) in [
        (
            PathSemantics::DestinationRootAbsolute,
            MigrationClassification::NotApplicable,
        ),
        (
            PathSemantics::ExternalProviderPath,
            MigrationClassification::NotApplicable,
        ),
        (
            PathSemantics::Unknown,
            MigrationClassification::ManualReview,
        ),
    ] {
        let mut reference = ref_("unsupported", "/external/value", semantics);
        reference.migratable = false;
        assert_eq!(plan_rebase(&old, &new, &reference).classification, expected);
    }
}

#[test]
fn idempotence_identical_and_already_current() {
    let old = PathBuf::from("/old");
    let new = PathBuf::from("/new");
    let mut current = ref_("rom", "/new/game", PathSemantics::SourceRootAbsolute);
    current.requires_existence = false;
    assert_eq!(
        plan_rebase(&old, &new, &current).classification,
        MigrationClassification::AlreadyCurrent
    );
    assert_eq!(
        plan_rebase(&new, &new, &current).classification,
        MigrationClassification::AlreadyCurrent
    );
}

#[test]
fn traversal_missing_destination_conflict_and_aggregate_are_deterministic() {
    let old = PathBuf::from("/old");
    let new = PathBuf::from("/new");
    let mut traversal = ref_(
        "escape",
        "/old/../elsewhere/x",
        PathSemantics::SourceRootAbsolute,
    );
    traversal.requires_existence = false;
    assert_eq!(
        plan_rebase(&old, &new, &traversal).classification,
        MigrationClassification::OutsideNewRoot
    );
    let mut conflict = ref_("conflict", "/old/x", PathSemantics::SourceRootAbsolute);
    conflict.requires_existence = false;
    conflict.claimed_destination = Some(PathBuf::from("/new/other"));
    assert_eq!(
        plan_rebase(&old, &new, &conflict).classification,
        MigrationClassification::Ambiguous
    );
    let snapshots = vec![SubsystemSnapshot {
        subsystem: "test".into(),
        references: vec![conflict],
    }];
    let a = plan_source_root_migration(&old, &new, &snapshots);
    let b = plan_source_root_migration(&old, &new, &snapshots);
    assert_eq!(a, b);
    assert_eq!(a.totals.conflicts, 1);
}
