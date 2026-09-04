use super::*;
use crate::database::{PersistedArchive, SourceFolderRecord};
use std::fs;
use std::path::PathBuf;

fn folder(id: i64, path: PathBuf) -> SourceFolderRecord {
    SourceFolderRecord {
        id,
        path,
        first_seen_at: "now".into(),
        last_scan_status: None,
        last_scan_error: None,
        last_scan_at: None,
        last_successful_scan_at: None,
        last_archive_count: None,
        assigned_platform: None,
        unknown_archive_count: 0,
    }
}

fn archive(id: i64, source_id: i64, relative: &str, absolute: PathBuf) -> PersistedArchive {
    PersistedArchive {
        id,
        source_folder_id: source_id,
        relative_path: relative.into(),
        absolute_path: absolute,
        archive_kind: "file".into(),
        display_name: "game".into(),
        normalized_name: "game".into(),
        size_bytes: None,
        modified_time_unix_seconds: None,
        platform: None,
        platform_source: None,
        last_known_health: "present".into(),
        last_seen_at: "now".into(),
        last_verified_missing_at: None,
        identity_report: None,
    }
}

fn input(
    old: &PathBuf,
    new: &PathBuf,
    folders: Vec<SourceFolderRecord>,
    archives: Vec<PersistedArchive>,
) -> SourceFolderMigrationInput {
    SourceFolderMigrationInput {
        old_root: old.clone(),
        new_root: new.clone(),
        source_folders: folders,
        archives,
        historical_paths: vec![old.join("historical.rom")],
        target_source_id: None,
    }
}

#[test]
fn source_rows_rebase_distinctly_and_archives_recompute() {
    let root = std::env::temp_dir().join("archivefs-source-folder-adapter");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(new.join("a/nested")).unwrap();
    fs::create_dir_all(new.join("b")).unwrap();
    let folders = vec![folder(2, old.join("a/nested")), folder(1, old.join("b"))];
    let report = plan_source_folder_migration(&input(
        &old,
        &new,
        folders,
        vec![archive(9, 2, "game.rom", old.join("a/nested/game.rom"))],
    ));
    assert_eq!(report.totals.source_folder_records, 2);
    assert_eq!(report.totals.source_roots_rebase, 2);
    assert_eq!(report.totals.archive_observations_to_recompute, 1);
    assert_eq!(
        report.archive_observations[0].classification,
        MigrationClassification::RegenerateInstead
    );
    assert_eq!(report.historical.len(), 1);
    assert_eq!(
        report.historical[0].classification,
        MigrationClassification::HistoricalOnly
    );
    assert_eq!(
        report.source_roots.migration.proposals[0].reference_id,
        "source-folder:1"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ownership_conflicts_overlap_and_missing_targets_fail_closed() {
    let root = std::env::temp_dir().join("archivefs-source-folder-adapter-safety");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(&new).unwrap();
    let report = plan_source_folder_migration(&input(
        &old,
        &new,
        vec![folder(1, old.clone()), folder(2, old.clone())],
        vec![],
    ));
    assert_eq!(report.totals.conflicts, 2);
    assert!(
        report
            .source_roots
            .migration
            .proposals
            .iter()
            .all(|p| p.classification == MigrationClassification::Ambiguous)
    );
    let overlap = plan_source_folder_migration(&input(
        &old,
        &old.join("nested"),
        vec![folder(3, old.clone())],
        vec![],
    ));
    assert_eq!(
        overlap.source_roots.migration.proposals[0].classification,
        MigrationClassification::Ambiguous
    );
    let missing = plan_source_folder_migration(&input(
        &old,
        &root.join("missing"),
        vec![folder(4, old.clone())],
        vec![],
    ));
    assert_eq!(
        missing.source_roots.migration.proposals[0].classification,
        MigrationClassification::TargetMissing
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wrong_owner_unknown_semantics_and_idempotence_are_explicit() {
    let old = PathBuf::from("/old");
    let new = PathBuf::from("/new");
    let mut value = input(
        &old,
        &new,
        vec![folder(7, old.clone()), folder(8, old.join("other"))],
        vec![],
    );
    value.target_source_id = Some(7);
    let report = plan_source_folder_migration(&value);
    assert_eq!(report.totals.migratable_source_roots, 1);
    assert_eq!(
        report.source_roots.migration.proposals[1].classification,
        MigrationClassification::ManualReview
    );
    let second = plan_source_folder_migration(&SourceFolderMigrationInput {
        source_folders: vec![folder(7, new.clone())],
        ..value
    });
    assert_eq!(
        second.source_roots.migration.proposals[0].classification,
        MigrationClassification::AlreadyCurrent
    );
    let mut unknown = MigrationReference {
        id: "unknown".into(),
        subsystem: "source-folder-database".into(),
        path: "/unknown/path".into(),
        semantics: PathSemantics::Unknown,
        migratable: false,
        requires_existence: true,
        historical: false,
        live: true,
        claimed_destination: None,
    };
    unknown.migratable = false;
    assert_eq!(
        crate::source_root_migration::plan_rebase(&old, &new, &unknown).classification,
        MigrationClassification::ManualReview
    );
}

#[cfg(unix)]
#[test]
fn symlinked_source_or_destination_is_not_safe() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join("archivefs-source-folder-adapter-links");
    let old = root.join("old");
    let new = root.join("new");
    let actual = root.join("actual");
    fs::create_dir_all(&actual).unwrap();
    symlink(&actual, &old).unwrap();
    let source =
        plan_source_folder_migration(&input(&old, &new, vec![folder(1, old.clone())], vec![]));
    assert_eq!(
        source.source_roots.migration.proposals[0].classification,
        MigrationClassification::ManualReview
    );
    fs::remove_file(&old).unwrap();
    fs::create_dir_all(&old).unwrap();
    symlink(&actual, &new).unwrap();
    let destination =
        plan_source_folder_migration(&input(&old, &new, vec![folder(2, old.clone())], vec![]));
    assert_eq!(
        destination.source_roots.migration.proposals[0].classification,
        MigrationClassification::TargetMissing
    );
    fs::remove_dir_all(root).unwrap();
}
