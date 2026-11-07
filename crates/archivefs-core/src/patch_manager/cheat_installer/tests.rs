use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::canonical_platform_for_alias;
use crate::emulator_environment::EncodedPath;
use crate::patch_manager::cheat_catalogue::{
    CheatCatalogueFormat, CheatGameMatch, CheatGameRecord, CheatInstalledState,
    CheatMatchConfidence, CheatStagingAction, CheatStagingPlan,
};
use crate::patch_manager::parse_cheat_install_run;

// ---------------------------------------------------------------------
// Fixture helpers - every test uses its own temporary directories under
// `std::env::temp_dir()`; nothing here ever touches a real `$HOME`, a
// real RetroArch cheat directory, or the live EmuWiz database.
// ---------------------------------------------------------------------

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "archivefs-cheat-installer-test-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn dummy_game_match(confidence: CheatMatchConfidence) -> CheatGameMatch {
    CheatGameMatch {
        confidence,
        evidence: Vec::new(),
        candidates: Vec::new(),
    }
}

/// Builds one `CheatAvailabilityEntry` whose `source_file_path` points at a
/// real file - the installer performs real bounded reads against it, so
/// every test that exercises apply/dry-run logic needs a genuine file on
/// disk, not a synthetic path.
#[allow(clippy::too_many_arguments)]
fn build_entry(
    game_name: &str,
    platform: &str,
    source_path: EncodedPath,
    source_hash: String,
    confidence: CheatMatchConfidence,
    action: CheatStagingAction,
    reason: &'static str,
    destination_root: &Path,
    existing_destination_hash: Option<String>,
) -> CheatAvailabilityEntry {
    let canonical = canonical_platform_for_alias(platform).unwrap_or(platform);
    let proposed_destination_path = if matches!(action, CheatStagingAction::NotEligible) {
        None
    } else {
        Some(EncodedPath::from_path(
            &destination_root
                .join(canonical)
                .join(format!("{game_name}.cht")),
        ))
    };
    let game = CheatGameRecord {
        source_game_name: game_name.to_string(),
        source_platform: Some(platform.to_string()),
        source_region: None,
        source_revision: None,
        source_identifier: None,
        source_content_hash: None,
        target_emulator: Some("retroarch".to_string()),
        cheat_count: 1,
        cheats: Vec::new(),
        enabled_by_default_count: 0,
        source_file_path: source_path.clone(),
        source_file_hash: Some(source_hash.clone()),
        format: CheatCatalogueFormat::RetroarchChtDirectory,
        parsing_complete: true,
        parsing_diagnostics: Vec::new(),
    };
    let staging_plan = CheatStagingPlan {
        source_cheat_path: source_path,
        proposed_destination_path,
        source_file_hash: Some(source_hash),
        existing_destination_hash,
        planned_action: action,
        reason,
    };
    CheatAvailabilityEntry {
        game,
        game_match: dummy_game_match(confidence),
        installed_state: CheatInstalledState::Unknown,
        installed_state_detail: Vec::new(),
        staging_candidate: false,
        destructive_if_applied: false,
        staging_plan,
    }
}

fn write_source(root: &Path, name: &str, content: &[u8]) -> (EncodedPath, String) {
    let path = root.join(name);
    fs::write(&path, content).unwrap();
    (EncodedPath::from_path(&path), hex_sha256(content))
}

fn install_new_entry(
    catalogue_root: &Path,
    destination_root: &Path,
    game_name: &str,
    content: &[u8],
) -> CheatAvailabilityEntry {
    let (source_path, hash) = write_source(catalogue_root, &format!("{game_name}.cht"), content);
    build_entry(
        game_name,
        "Atari - 2600",
        source_path,
        hash,
        CheatMatchConfidence::Strong,
        CheatStagingAction::InstallNew,
        "destination_missing",
        destination_root,
        None,
    )
}

fn options(
    destination_root: &Path,
    journal_directory: &Path,
    backup_directory: &Path,
    dry_run: bool,
    confirmed: bool,
    allow_replace_different: bool,
    run_id: &str,
) -> CheatInstallOptions {
    CheatInstallOptions {
        destination_root: destination_root.to_path_buf(),
        allow_replace_different,
        dry_run,
        confirmed,
        journal_directory: journal_directory.to_path_buf(),
        backup_directory: backup_directory.to_path_buf(),
        run_id: run_id.to_string(),
        started_at_unix_seconds: 1_700_000_000,
        catalogue_source: "Fixture Catalogue".to_string(),
    }
}

fn dir_entries(dir: &Path) -> Vec<String> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

struct FaultGuard;

impl FaultGuard {
    fn new(point: FaultPoint) -> Self {
        inject_fault_for_test(Some(point));
        Self
    }
}

impl Drop for FaultGuard {
    fn drop(&mut self) {
        inject_fault_for_test(None);
    }
}

// ---------------------------------------------------------------------
// Dry-run / confirmation gate
// ---------------------------------------------------------------------

#[test]
fn dry_run_creates_nothing() {
    let catalogue_root = temp_root("dry-run-catalogue");
    let destination_root = temp_root("dry-run-dest");
    let journal_dir = temp_root("dry-run-journal");
    let backup_dir = temp_root("dry-run-backup");
    fs::remove_dir_all(&journal_dir).unwrap();
    fs::remove_dir_all(&backup_dir).unwrap();

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        true,
        true,
        false,
        "run-1",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    assert!(outcome.run.dry_run);
    assert_eq!(outcome.run.status, CheatInstallRunStatus::DryRun);
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::InstalledNew
    );
    assert!(!outcome.run.entries[0].applied);
    assert_eq!(outcome.run.summary.writes_attempted, 0);
    assert_eq!(outcome.run.summary.writes_succeeded, 0);
    assert!(!destination_root.join("Atari2600").exists());
    assert!(!journal_dir.exists());
    assert!(outcome.journal_path.is_none());
}

#[test]
fn apply_without_yes_writes_nothing() {
    let catalogue_root = temp_root("no-yes-catalogue");
    let destination_root = temp_root("no-yes-dest");
    let journal_dir = temp_root("no-yes-journal");
    let backup_dir = temp_root("no-yes-backup");
    fs::remove_dir_all(&journal_dir).unwrap();
    fs::remove_dir_all(&backup_dir).unwrap();

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    // dry_run: false, confirmed: false - must still behave as a dry run.
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        false,
        false,
        "run-2",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    assert!(outcome.run.dry_run);
    assert!(!destination_root.join("Atari2600").exists());
    assert!(!journal_dir.exists());
}

// ---------------------------------------------------------------------
// install_new
// ---------------------------------------------------------------------

#[test]
fn install_new_creates_expected_file_with_matching_hash() {
    let catalogue_root = temp_root("install-new-catalogue");
    let destination_root = temp_root("install-new-dest");
    let journal_dir = temp_root("install-new-journal");
    let backup_dir = temp_root("install-new-backup");

    let content = b"cheats = 1\ncheat0_desc = \"Infinite Lives\"\ncheat0_enable = false\n";
    let entry = install_new_entry(&catalogue_root, &destination_root, "Frogger", content);
    let source_hash = entry.staging_plan.source_file_hash.clone().unwrap();
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-3",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    let result = &outcome.run.entries[0];
    assert_eq!(result.outcome, CheatInstallOutcome::InstalledNew);
    assert!(result.applied);

    let installed_path = destination_root.join("Atari2600").join("Frogger.cht");
    let installed_bytes = fs::read(&installed_path).unwrap();
    assert_eq!(installed_bytes, content);
    assert_eq!(hex_sha256(&installed_bytes), source_hash);
    assert_eq!(
        result.resulting_destination_hash.as_deref(),
        Some(source_hash.as_str())
    );

    // No stray temporary files left behind.
    let leftovers: Vec<String> = dir_entries(&destination_root.join("Atari2600"))
        .into_iter()
        .filter(|name| name.starts_with(".archivefs-"))
        .collect();
    assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
}

// ---------------------------------------------------------------------
// already_installed
// ---------------------------------------------------------------------

#[test]
fn already_installed_performs_no_write() {
    let catalogue_root = temp_root("already-installed-catalogue");
    let destination_root = temp_root("already-installed-dest");
    let journal_dir = temp_root("already-installed-journal");
    let backup_dir = temp_root("already-installed-backup");

    let content = b"cheats = 0\n";
    let (source_path, hash) = write_source(&catalogue_root, "Frogger.cht", content);
    let platform_dir = destination_root.join("Atari2600");
    fs::create_dir_all(&platform_dir).unwrap();
    let destination_path = platform_dir.join("Frogger.cht");
    fs::write(&destination_path, content).unwrap();
    let before_metadata = fs::metadata(&destination_path).unwrap();

    let entry = build_entry(
        "Frogger",
        "Atari - 2600",
        source_path,
        hash.clone(),
        CheatMatchConfidence::Strong,
        CheatStagingAction::AlreadyInstalled,
        "hash_match",
        &destination_root,
        Some(hash),
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-4",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    let result = &outcome.run.entries[0];
    assert_eq!(result.outcome, CheatInstallOutcome::AlreadyInstalled);
    assert!(!result.applied);
    assert!(!result.write_required);

    let after_metadata = fs::metadata(&destination_path).unwrap();
    assert_eq!(before_metadata.len(), after_metadata.len());
    assert_eq!(fs::read(&destination_path).unwrap(), content);
    assert!(outcome.run.entries[0].backup_path.is_none());
}

// ---------------------------------------------------------------------
// replace_different
// ---------------------------------------------------------------------

fn replace_different_entry(
    catalogue_root: &Path,
    destination_root: &Path,
    new_content: &[u8],
    old_content: &[u8],
) -> CheatAvailabilityEntry {
    let (source_path, source_hash) = write_source(catalogue_root, "Frogger.cht", new_content);
    let platform_dir = destination_root.join("Atari2600");
    fs::create_dir_all(&platform_dir).unwrap();
    fs::write(platform_dir.join("Frogger.cht"), old_content).unwrap();
    let old_hash = hex_sha256(old_content);
    build_entry(
        "Frogger",
        "Atari - 2600",
        source_path,
        source_hash,
        CheatMatchConfidence::Strong,
        CheatStagingAction::ReplaceDifferent,
        "hash_mismatch",
        destination_root,
        Some(old_hash),
    )
}

#[test]
fn replace_different_blocked_without_permission() {
    let catalogue_root = temp_root("replace-blocked-catalogue");
    let destination_root = temp_root("replace-blocked-dest");
    let journal_dir = temp_root("replace-blocked-journal");
    let backup_dir = temp_root("replace-blocked-backup");

    let entry = replace_different_entry(&catalogue_root, &destination_root, b"new\n", b"old\n");
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-5",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    let result = &outcome.run.entries[0];
    assert_eq!(
        result.outcome,
        CheatInstallOutcome::SkippedReplaceNotAllowed
    );
    assert!(!result.applied);
    assert!(result.backup_path.is_none());
    let destination_path = destination_root.join("Atari2600").join("Frogger.cht");
    assert_eq!(fs::read(&destination_path).unwrap(), b"old\n");
    assert!(backup_dir_is_empty(&backup_dir));
}

fn backup_dir_is_empty(dir: &Path) -> bool {
    !dir.exists() || fs::read_dir(dir).unwrap().next().is_none()
}

#[test]
fn replacement_with_permission_creates_verified_backup_and_installs_new_content() {
    let catalogue_root = temp_root("replace-allowed-catalogue");
    let destination_root = temp_root("replace-allowed-dest");
    let journal_dir = temp_root("replace-allowed-journal");
    let backup_dir = temp_root("replace-allowed-backup");

    let entry = replace_different_entry(
        &catalogue_root,
        &destination_root,
        b"new content\n",
        b"old content\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        true,
        "run-6",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    let result = &outcome.run.entries[0];
    assert_eq!(result.outcome, CheatInstallOutcome::ReplacedWithBackup);
    assert!(result.applied);

    let backup_path = result.backup_path.as_ref().expect("backup path recorded");
    let backup_bytes = fs::read(&backup_path.display).unwrap();
    assert_eq!(backup_bytes, b"old content\n");
    assert_eq!(hex_sha256(&backup_bytes), hex_sha256(b"old content\n"));

    let destination_path = destination_root.join("Atari2600").join("Frogger.cht");
    assert_eq!(fs::read(&destination_path).unwrap(), b"new content\n");
    assert_eq!(
        result.resulting_destination_hash.as_deref(),
        Some(hex_sha256(b"new content\n")).as_deref()
    );
}

#[test]
fn failed_replacement_preserves_original_or_verified_backup() {
    let catalogue_root = temp_root("replace-failed-catalogue");
    let destination_root = temp_root("replace-failed-dest");
    let journal_dir = temp_root("replace-failed-journal");
    let backup_dir = temp_root("replace-failed-backup");

    let entry = replace_different_entry(
        &catalogue_root,
        &destination_root,
        b"new content\n",
        b"old content\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        true,
        "run-7",
    );

    let _guard = FaultGuard::new(FaultPoint::Rename);
    let outcome = execute_cheat_install_run(&[entry], &opts);
    drop(_guard);

    let result = &outcome.run.entries[0];
    assert_eq!(result.outcome, CheatInstallOutcome::FailedWrite);
    // The backup was already durably created before the injected rename
    // failure and must be preserved.
    let backup_path = result.backup_path.as_ref().expect("backup preserved");
    assert_eq!(fs::read(&backup_path.display).unwrap(), b"old content\n");
    // The original destination was never opened for writing and remains
    // exactly as it was.
    let destination_path = destination_root.join("Atari2600").join("Frogger.cht");
    assert_eq!(fs::read(&destination_path).unwrap(), b"old content\n");
}

// ---------------------------------------------------------------------
// Source/destination revalidation
// ---------------------------------------------------------------------

#[test]
fn changed_source_after_preview_is_rejected() {
    let catalogue_root = temp_root("source-changed-catalogue");
    let destination_root = temp_root("source-changed-dest");
    let journal_dir = temp_root("source-changed-journal");
    let backup_dir = temp_root("source-changed-backup");

    let (source_path, _original_hash) =
        write_source(&catalogue_root, "Frogger.cht", b"cheats = 0\n");
    // Simulate drift: the record claims a hash that no longer matches the
    // file's real, current content.
    let stale_hash = hex_sha256(b"a completely different original content");
    let entry = build_entry(
        "Frogger",
        "Atari - 2600",
        source_path,
        stale_hash,
        CheatMatchConfidence::Strong,
        CheatStagingAction::InstallNew,
        "destination_missing",
        &destination_root,
        None,
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-8",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    let result = &outcome.run.entries[0];
    assert_eq!(result.outcome, CheatInstallOutcome::SkippedSourceChanged);
    assert!(!destination_root.join("Atari2600").exists());
}

#[test]
fn changed_destination_after_preview_is_rejected() {
    let catalogue_root = temp_root("dest-changed-catalogue");
    let destination_root = temp_root("dest-changed-dest");
    let journal_dir = temp_root("dest-changed-journal");
    let backup_dir = temp_root("dest-changed-backup");

    let (source_path, source_hash) = write_source(&catalogue_root, "Frogger.cht", b"new content\n");
    let platform_dir = destination_root.join("Atari2600");
    fs::create_dir_all(&platform_dir).unwrap();
    fs::write(
        platform_dir.join("Frogger.cht"),
        b"actual current content\n",
    )
    .unwrap();

    // The plan claims a *different* previous hash than what is really
    // there now (simulating something else having changed it since
    // preview).
    let stale_previous_hash = hex_sha256(b"content the preview saw, which is now gone");
    let entry = build_entry(
        "Frogger",
        "Atari - 2600",
        source_path,
        source_hash,
        CheatMatchConfidence::Strong,
        CheatStagingAction::ReplaceDifferent,
        "hash_mismatch",
        &destination_root,
        Some(stale_previous_hash),
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        true,
        "run-9",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    let result = &outcome.run.entries[0];
    assert_eq!(
        result.outcome,
        CheatInstallOutcome::SkippedDestinationChanged
    );
    assert!(result.backup_path.is_none());
    assert_eq!(
        fs::read(platform_dir.join("Frogger.cht")).unwrap(),
        b"actual current content\n"
    );
}

// ---------------------------------------------------------------------
// Eligibility gates
// ---------------------------------------------------------------------

#[test]
fn weak_match_writes_nothing() {
    let catalogue_root = temp_root("weak-catalogue");
    let destination_root = temp_root("weak-dest");
    let journal_dir = temp_root("weak-journal");
    let backup_dir = temp_root("weak-backup");

    let (source_path, hash) = write_source(&catalogue_root, "Frogger.cht", b"cheats = 0\n");
    let entry = build_entry(
        "Frogger",
        "Atari - 2600",
        source_path,
        hash,
        CheatMatchConfidence::Weak,
        CheatStagingAction::NotEligible,
        "weak_match_not_eligible",
        &destination_root,
        None,
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-10",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::SkippedNotEligible
    );
    assert!(!destination_root.exists() || dir_entries(&destination_root).is_empty());
}

#[test]
fn ambiguous_match_writes_nothing() {
    let catalogue_root = temp_root("ambiguous-catalogue");
    let destination_root = temp_root("ambiguous-dest");
    let journal_dir = temp_root("ambiguous-journal");
    let backup_dir = temp_root("ambiguous-backup");

    let (source_path, hash) = write_source(&catalogue_root, "Frogger.cht", b"cheats = 0\n");
    let entry = build_entry(
        "Frogger",
        "Atari - 2600",
        source_path,
        hash,
        CheatMatchConfidence::Ambiguous,
        CheatStagingAction::NotEligible,
        "ambiguous_match_not_eligible",
        &destination_root,
        None,
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-11",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::SkippedNotEligible
    );
    assert!(!destination_root.exists() || dir_entries(&destination_root).is_empty());
}

#[test]
fn duplicate_destination_writes_nothing() {
    let catalogue_root = temp_root("duplicate-catalogue");
    let destination_root = temp_root("duplicate-dest");
    let journal_dir = temp_root("duplicate-journal");
    let backup_dir = temp_root("duplicate-backup");

    // Already flagged as a conflict upstream (by the staging preview).
    let (source_path_a, hash_a) = write_source(&catalogue_root, "a.cht", b"cheats = 0\n");
    let entry_a = build_entry(
        "Frogger",
        "Atari - 2600",
        source_path_a,
        hash_a,
        CheatMatchConfidence::Exact,
        CheatStagingAction::Conflict,
        "duplicate_destination",
        &destination_root,
        None,
    );
    let (source_path_b, hash_b) = write_source(&catalogue_root, "b.cht", b"cheats = 1\n");
    let entry_b = build_entry(
        "Frogger",
        "Atari - 2600",
        source_path_b,
        hash_b,
        CheatMatchConfidence::Exact,
        CheatStagingAction::Conflict,
        "duplicate_destination",
        &destination_root,
        None,
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-12",
    );

    let outcome = execute_cheat_install_run(&[entry_a, entry_b], &opts);
    assert!(
        outcome
            .run
            .entries
            .iter()
            .all(|entry| entry.outcome == CheatInstallOutcome::SkippedConflict)
    );
    assert!(!destination_root.exists() || dir_entries(&destination_root).is_empty());
}

#[test]
fn installer_own_duplicate_defense_blocks_a_second_entry_at_apply_time() {
    // Two entries that (hypothetically) both resolved to `install_new` for
    // the exact same destination - the installer's own batch-level
    // duplicate tracking must still catch this even if upstream somehow
    // did not.
    let catalogue_root = temp_root("apply-duplicate-catalogue");
    let destination_root = temp_root("apply-duplicate-dest");
    let journal_dir = temp_root("apply-duplicate-journal");
    let backup_dir = temp_root("apply-duplicate-backup");

    let entry_a = install_new_entry(&catalogue_root, &destination_root, "Frogger", b"one\n");
    let (source_path_b, hash_b) = write_source(&catalogue_root, "Frogger-alt.cht", b"two\n");
    let entry_b = build_entry(
        "Frogger",
        "Atari - 2600",
        source_path_b,
        hash_b,
        CheatMatchConfidence::Strong,
        CheatStagingAction::InstallNew,
        "destination_missing",
        &destination_root,
        None,
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-13",
    );

    let outcome = execute_cheat_install_run(&[entry_a, entry_b], &opts);
    let outcomes: Vec<CheatInstallOutcome> = outcome
        .run
        .entries
        .iter()
        .map(|entry| entry.outcome)
        .collect();
    assert_eq!(outcomes[0], CheatInstallOutcome::InstalledNew);
    assert_eq!(outcomes[1], CheatInstallOutcome::SkippedConflict);
    assert_eq!(
        outcome.run.entries[1].reason_code,
        "duplicate_destination_at_apply_time"
    );
}

// ---------------------------------------------------------------------
// Destination/path/symlink safety
// ---------------------------------------------------------------------

#[test]
fn traversal_rejected() {
    let catalogue_root = temp_root("traversal-catalogue");
    let destination_root = temp_root("traversal-dest");
    let journal_dir = temp_root("traversal-journal");
    let backup_dir = temp_root("traversal-backup");

    let (source_path, hash) = write_source(&catalogue_root, "evil.cht", b"cheats = 0\n");
    let entry = build_entry(
        "../../../etc/passwd",
        "Atari - 2600",
        source_path,
        hash,
        CheatMatchConfidence::Exact,
        CheatStagingAction::InstallNew,
        "destination_missing",
        &destination_root,
        None,
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-14",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::FailedUnsafePath
    );
    assert!(!Path::new("/etc/passwd.cht").exists());
}

#[cfg(unix)]
#[test]
fn parent_symlink_rejected() {
    let catalogue_root = temp_root("parent-symlink-catalogue");
    let destination_root = temp_root("parent-symlink-dest");
    let journal_dir = temp_root("parent-symlink-journal");
    let backup_dir = temp_root("parent-symlink-backup");
    let outside = temp_root("parent-symlink-outside");

    fs::create_dir_all(&destination_root).unwrap();
    std::os::unix::fs::symlink(&outside, destination_root.join("Atari2600")).unwrap();

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-15",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::FailedUnsafePath
    );
    assert!(dir_entries(&outside).is_empty());
}

#[cfg(unix)]
#[test]
fn final_symlink_rejected() {
    let catalogue_root = temp_root("final-symlink-catalogue");
    let destination_root = temp_root("final-symlink-dest");
    let journal_dir = temp_root("final-symlink-journal");
    let backup_dir = temp_root("final-symlink-backup");
    let outside = temp_root("final-symlink-outside");
    fs::write(outside.join("secret.cht"), b"do not touch\n").unwrap();

    let platform_dir = destination_root.join("Atari2600");
    fs::create_dir_all(&platform_dir).unwrap();
    std::os::unix::fs::symlink(outside.join("secret.cht"), platform_dir.join("Frogger.cht"))
        .unwrap();

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-16",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::FailedUnsafePath
    );
    assert_eq!(
        fs::read(outside.join("secret.cht")).unwrap(),
        b"do not touch\n"
    );
}

#[cfg(unix)]
#[test]
fn root_symlink_rejected() {
    let catalogue_root = temp_root("root-symlink-catalogue");
    let real_root = temp_root("root-symlink-real");
    let outside = temp_root("root-symlink-outside");
    let journal_dir = temp_root("root-symlink-journal");
    let backup_dir = temp_root("root-symlink-backup");
    fs::remove_dir_all(&real_root).unwrap();
    std::os::unix::fs::symlink(&outside, &real_root).unwrap();

    let entry = install_new_entry(&catalogue_root, &real_root, "Frogger", b"cheats = 0\n");
    let opts = options(
        &real_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-17",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::FailedUnsafePath
    );
    assert!(dir_entries(&outside).is_empty());
}

#[test]
fn absent_root_created_only_during_approved_apply() {
    let catalogue_root = temp_root("absent-root-catalogue");
    let destination_root = temp_root("absent-root-dest");
    fs::remove_dir_all(&destination_root).unwrap();
    let journal_dir = temp_root("absent-root-journal");
    let backup_dir = temp_root("absent-root-backup");

    // Dry run first: root must not be created.
    let dry_entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let dry_opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        true,
        false,
        false,
        "run-18a",
    );
    let dry_outcome = execute_cheat_install_run(&[dry_entry], &dry_opts);
    assert_eq!(
        dry_outcome.run.entries[0].outcome,
        CheatInstallOutcome::InstalledNew
    );
    assert!(!destination_root.exists());

    // Real, confirmed apply: root and platform directory are created.
    let apply_entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let apply_opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-18b",
    );
    let apply_outcome = execute_cheat_install_run(&[apply_entry], &apply_opts);
    assert_eq!(
        apply_outcome.run.entries[0].outcome,
        CheatInstallOutcome::InstalledNew
    );
    assert!(
        destination_root
            .join("Atari2600")
            .join("Frogger.cht")
            .exists()
    );
}

// ---------------------------------------------------------------------
// Failure injection
// ---------------------------------------------------------------------

#[test]
fn no_partial_final_file_after_injected_write_failure() {
    let catalogue_root = temp_root("partial-write-catalogue");
    let destination_root = temp_root("partial-write-dest");
    let journal_dir = temp_root("partial-write-journal");
    let backup_dir = temp_root("partial-write-backup");

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-19",
    );

    let _guard = FaultGuard::new(FaultPoint::TempDestinationWrite);
    let outcome = execute_cheat_install_run(&[entry], &opts);
    drop(_guard);

    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::FailedWrite
    );
    assert!(
        !destination_root
            .join("Atari2600")
            .join("Frogger.cht")
            .exists()
    );
}

#[test]
fn temporary_file_cleaned_after_failure() {
    let catalogue_root = temp_root("temp-cleanup-catalogue");
    let destination_root = temp_root("temp-cleanup-dest");
    let journal_dir = temp_root("temp-cleanup-journal");
    let backup_dir = temp_root("temp-cleanup-backup");

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-20",
    );

    let _guard = FaultGuard::new(FaultPoint::FinalVerification);
    let outcome = execute_cheat_install_run(&[entry], &opts);
    drop(_guard);

    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::FailedVerification
    );
    assert!(
        !destination_root
            .join("Atari2600")
            .join("Frogger.cht")
            .exists()
    );
    let leftovers = dir_entries(&destination_root.join("Atari2600"));
    assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
}

#[test]
fn failed_verification_reported() {
    let catalogue_root = temp_root("failed-verify-catalogue");
    let destination_root = temp_root("failed-verify-dest");
    let journal_dir = temp_root("failed-verify-journal");
    let backup_dir = temp_root("failed-verify-backup");

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-21",
    );

    let _guard = FaultGuard::new(FaultPoint::FinalVerification);
    let outcome = execute_cheat_install_run(&[entry], &opts);
    drop(_guard);

    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::FailedVerification
    );
    assert_eq!(outcome.run.summary.failed, 1);
}

#[test]
fn backup_retained_after_verification_failure() {
    let catalogue_root = temp_root("backup-retained-catalogue");
    let destination_root = temp_root("backup-retained-dest");
    let journal_dir = temp_root("backup-retained-journal");
    let backup_dir = temp_root("backup-retained-backup");

    let entry = replace_different_entry(
        &catalogue_root,
        &destination_root,
        b"new content\n",
        b"old content\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        true,
        "run-22",
    );

    let _guard = FaultGuard::new(FaultPoint::FinalVerification);
    let outcome = execute_cheat_install_run(&[entry], &opts);
    drop(_guard);

    let result = &outcome.run.entries[0];
    assert_eq!(result.outcome, CheatInstallOutcome::FailedVerification);
    let backup_path = result
        .backup_path
        .as_ref()
        .expect("backup preserved despite verification failure");
    assert_eq!(fs::read(&backup_path.display).unwrap(), b"old content\n");
    let destination_path = destination_root.join("Atari2600").join("Frogger.cht");
    assert_eq!(fs::read(&destination_path).unwrap(), b"old content\n");
}

#[test]
fn backup_write_failure_is_reported_and_original_untouched() {
    let catalogue_root = temp_root("backup-write-fail-catalogue");
    let destination_root = temp_root("backup-write-fail-dest");
    let journal_dir = temp_root("backup-write-fail-journal");
    let backup_dir = temp_root("backup-write-fail-backup");

    let entry = replace_different_entry(
        &catalogue_root,
        &destination_root,
        b"new content\n",
        b"old content\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        true,
        "run-23",
    );

    let _guard = FaultGuard::new(FaultPoint::BackupWrite);
    let outcome = execute_cheat_install_run(&[entry], &opts);
    drop(_guard);

    let result = &outcome.run.entries[0];
    assert_eq!(result.outcome, CheatInstallOutcome::FailedBackup);
    assert!(result.backup_path.is_none());
    let destination_path = destination_root.join("Atari2600").join("Frogger.cht");
    assert_eq!(fs::read(&destination_path).unwrap(), b"old content\n");
}

// ---------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------

#[test]
fn real_apply_writes_one_journal_that_parses() {
    let catalogue_root = temp_root("journal-catalogue");
    let destination_root = temp_root("journal-dest");
    let journal_dir = temp_root("journal-journal");
    let backup_dir = temp_root("journal-backup");

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-24",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    let journal_path = outcome
        .journal_path
        .expect("journal written for a real apply run");
    assert!(outcome.journal_error.is_none());

    let entries_in_journal_dir = dir_entries(&journal_dir);
    assert_eq!(
        entries_in_journal_dir.len(),
        1,
        "exactly one journal file: {entries_in_journal_dir:?}"
    );

    let json = fs::read_to_string(&journal_path).unwrap();
    let parsed =
        parse_cheat_install_run(&json).expect("journal parses through parse_cheat_install_run");
    assert_eq!(parsed.run_id, outcome.run.run_id);
    assert_eq!(parsed.entries.len(), outcome.run.entries.len());
    assert_eq!(parsed.status, outcome.run.status);
}

#[test]
fn dry_run_writes_no_journal() {
    let catalogue_root = temp_root("dry-journal-catalogue");
    let destination_root = temp_root("dry-journal-dest");
    let journal_dir = temp_root("dry-journal-journal");
    let backup_dir = temp_root("dry-journal-backup");
    fs::remove_dir_all(&journal_dir).unwrap();

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        true,
        true,
        false,
        "run-25",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    assert!(outcome.journal_path.is_none());
    assert!(outcome.journal_error.is_none());
    assert!(!journal_dir.exists());
}

#[test]
fn journal_finalize_failure_leaves_a_truthful_pending_journal() {
    let catalogue_root = temp_root("journal-fail-catalogue");
    let destination_root = temp_root("journal-fail-dest");
    let journal_dir = temp_root("journal-fail-journal");
    let backup_dir = temp_root("journal-fail-backup");

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-26",
    );

    let _guard = FaultGuard::new(FaultPoint::JournalFinalize);
    let outcome = execute_cheat_install_run(&[entry], &opts);
    drop(_guard);

    // The pending journal is still on disk, so its path is reported (L5).
    assert_eq!(outcome.journal_path, Some(journal_dir.join("run-26.json")));
    assert!(outcome.journal_error.is_some());
    // The file itself was genuinely installed - the journal failure must
    // not be papered over as if nothing happened, but it also must not
    // retroactively mark the successful file write as failed.
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatInstallOutcome::InstalledNew
    );
    assert!(outcome.run.entries[0].applied);
    assert_eq!(outcome.run.status, CheatInstallRunStatus::Success);
    assert!(
        destination_root
            .join("Atari2600")
            .join("Frogger.cht")
            .exists()
    );
    // The on-disk record is the last per-entry update: pending, but it
    // already lists the applied entry, so Undo stays possible.
    let on_disk =
        parse_cheat_install_run(&fs::read_to_string(journal_dir.join("run-26.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk.status, CheatInstallRunStatus::InProgress);
    assert_eq!(on_disk.completed_at_unix_seconds, None);
    assert_eq!(on_disk.settled_entries, 1);
    assert!(on_disk.entries[0].applied);
    assert!(on_disk.is_interrupted());
}

#[test]
fn journal_never_overwrites_an_existing_journal() {
    let catalogue_root = temp_root("journal-no-overwrite-catalogue");
    let destination_root = temp_root("journal-no-overwrite-dest");
    let journal_dir = temp_root("journal-no-overwrite-journal");
    let backup_dir = temp_root("journal-no-overwrite-backup");

    let entry_one = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "same-run-id",
    );
    let first = execute_cheat_install_run(&[entry_one], &opts);
    assert!(first.journal_path.is_some());

    // A second run reusing the exact same run_id/journal_directory must
    // never clobber the first journal.
    let entry_two = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let second = execute_cheat_install_run(&[entry_two], &opts);
    assert!(second.journal_path.is_none());
    assert!(second.journal_error.is_some());
    assert_eq!(dir_entries(&journal_dir).len(), 1);
}

// ---------------------------------------------------------------------
// Lossless paths
// ---------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn non_utf8_destination_root_remains_lossless() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let catalogue_root = temp_root("non-utf8-root-catalogue");
    let mut raw_root = std::env::temp_dir().into_os_string().into_vec();
    raw_root.push(b'/');
    raw_root.extend_from_slice(
        format!("archivefs-cheat-installer-nonutf8-{}-", std::process::id()).as_bytes(),
    );
    raw_root.extend_from_slice(b"bad-\xFF-root");
    let destination_root = PathBuf::from(OsString::from_vec(raw_root));
    let _ = fs::remove_dir_all(&destination_root);
    fs::create_dir_all(&destination_root).unwrap();
    let journal_dir = temp_root("non-utf8-root-journal");
    let backup_dir = temp_root("non-utf8-root-backup");

    let entry = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Frogger",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-27",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    let result = &outcome.run.entries[0];
    assert_eq!(result.outcome, CheatInstallOutcome::InstalledNew);
    assert!(result.applied);

    let destination = result.destination_path.as_ref().unwrap();
    assert!(destination.lossy);
    assert!(destination.display.contains('\u{FFFD}'));

    // The real installed file exists at the real (non-UTF-8) path, even
    // though its JSON-safe representation is necessarily lossy.
    let real_installed_path = destination_root.join("Atari2600").join("Frogger.cht");
    assert_eq!(fs::read(&real_installed_path).unwrap(), b"cheats = 0\n");

    // A round-trip through the run's own JSON must not panic and must
    // still mark the path lossy - never silently "fix" it into something
    // that looks like a real, reversible path.
    let json = serde_json::to_string(&outcome.run).unwrap();
    assert!(json.contains("\u{FFFD}") || json.contains("\\ufffd"));
    let round_tripped: super::super::cheat_install_result::CheatInstallRun =
        serde_json::from_str(&json).unwrap();
    assert!(
        round_tripped.entries[0]
            .destination_path
            .as_ref()
            .unwrap()
            .lossy
    );
}

#[test]
fn lossy_source_path_is_rejected_rather_than_misread() {
    let catalogue_root = temp_root("lossy-source-catalogue");
    let destination_root = temp_root("lossy-source-dest");
    let journal_dir = temp_root("lossy-source-journal");
    let backup_dir = temp_root("lossy-source-backup");

    // A real file exists, but the record's own `source_file_path` claims
    // to be lossy (as if it had been round-tripped through a lossy
    // representation upstream) - the installer must refuse to guess at
    // the real path rather than silently opening whatever the lossy
    // display string happens to point at.
    let (mut source_path, hash) = write_source(&catalogue_root, "Frogger.cht", b"cheats = 0\n");
    source_path.lossy = true;

    let entry = build_entry(
        "Frogger",
        "Atari - 2600",
        source_path,
        hash,
        CheatMatchConfidence::Strong,
        CheatStagingAction::InstallNew,
        "destination_missing",
        &destination_root,
        None,
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        false,
        "run-28",
    );

    let outcome = execute_cheat_install_run(&[entry], &opts);
    let result = &outcome.run.entries[0];
    assert_eq!(result.outcome, CheatInstallOutcome::SkippedSourceChanged);
    assert_eq!(result.reason_code, "source_path_lossy_cannot_revalidate");
    assert!(!destination_root.join("Atari2600").exists());
}

// ---------------------------------------------------------------------
// Architectural guardrail: no live HOME/database/RetroArch access
// ---------------------------------------------------------------------

#[test]
fn module_source_never_touches_live_home_or_database() {
    let source = include_str!("../cheat_installer.rs");
    for forbidden in [
        "env::var(\"HOME\")",
        "env::var(\"XDG_",
        "std::env::home_dir",
        "Database::open",
        "Database::create",
        "default_database_path",
        "ureq::",
        "reqwest",
        "TcpStream",
    ] {
        assert!(
            !source.contains(forbidden),
            "cheat_installer.rs must never reference `{forbidden}` - every path/root/journal \
             location must be supplied explicitly via `CheatInstallOptions`"
        );
    }
}

// ---------------------------------------------------------------------
// Crash-proof Undo: the journal must exist before the first mutation
// (Night Watch S04-01 / D2-01).
// ---------------------------------------------------------------------

#[test]
fn d2_01_no_destination_is_mutated_without_a_prior_durable_journal() {
    let catalogue_root = temp_root("d2-01-catalogue");
    let destination_root = temp_root("d2-01-dest");
    let backup_dir = temp_root("d2-01-backup");
    let parent = temp_root("d2-01-journal-parent");
    let journal_dir = parent.join("journal");
    fs::write(&journal_dir, b"not a directory").unwrap(); // journal can never be created

    // One replace (user-edited existing file) + one new install, like the CLI repro.
    let replace = replace_different_entry(
        &catalogue_root,
        &destination_root,
        b"catalogue content\n",
        b"USER EDITED\n",
    );
    let new = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Pitfall",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        true,
        "d2-01",
    );

    let outcome = execute_cheat_install_run(&[replace, new], &opts);

    let platform_dir = destination_root.join("Atari2600");
    assert_eq!(
        fs::read(platform_dir.join("Frogger.cht")).unwrap(),
        b"USER EDITED\n",
        "the user's file must not be replaced when no Undo record can exist"
    );
    assert!(
        !platform_dir.join("Pitfall.cht").exists(),
        "no new file without a journal"
    );
    assert!(
        dir_entries(&platform_dir)
            .iter()
            .all(|n| !n.starts_with(".archivefs-cheat-install-")),
        "no temporary install file may be left in the emulator's cheat folder"
    );
    assert!(
        backup_dir_is_empty(&backup_dir),
        "no backup may be created when no Undo record can exist"
    );
    assert_ne!(outcome.run.status, CheatInstallRunStatus::Success);
    assert!(outcome.journal_error.is_some());
}

#[test]
fn journal_fault_before_first_mutation_fails_closed_with_nothing_written() {
    let catalogue_root = temp_root("journal-first-catalogue");
    let destination_root = temp_root("journal-first-dest");
    let journal_dir = temp_root("journal-first-journal");
    let backup_dir = temp_root("journal-first-backup");

    let replace = replace_different_entry(
        &catalogue_root,
        &destination_root,
        b"catalogue content\n",
        b"USER EDITED\n",
    );
    let new = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Pitfall",
        b"cheats = 0\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        true,
        "journal-first",
    );

    let _guard = FaultGuard::new(FaultPoint::JournalWrite);
    let outcome = execute_cheat_install_run(&[replace, new], &opts);
    drop(_guard);

    let platform_dir = destination_root.join("Atari2600");
    assert_eq!(dir_entries(&platform_dir), vec!["Frogger.cht".to_string()]);
    assert_eq!(
        fs::read(platform_dir.join("Frogger.cht")).unwrap(),
        b"USER EDITED\n"
    );
    assert!(backup_dir_is_empty(&backup_dir));
    assert!(
        dir_entries(&journal_dir).is_empty(),
        "no partial journal may be left behind: {:?}",
        dir_entries(&journal_dir)
    );
    assert_eq!(outcome.run.status, CheatInstallRunStatus::Failed);
    assert!(outcome.journal_path.is_none());
    assert!(outcome.journal_error.is_some());
    for entry in &outcome.run.entries {
        assert!(!entry.applied);
        assert_eq!(entry.reason_code, "journal_unavailable");
    }
}

// ---------------------------------------------------------------------
// Crash recovery: pending journal, per-entry updates, rollback of an
// interrupted run (Night Watch S04-01).
// ---------------------------------------------------------------------

use crate::patch_manager::{
    CheatHistoryOptions, CheatInstallEntrySettlement, CheatRollbackAvailability,
    CheatRollbackOptions, CheatRollbackOutcome, CheatRollbackRunOutcome, CheatRollbackRunStatus,
    execute_cheat_rollback_run, inspect_cheat_install_journal,
};

struct CrashFixture {
    catalogue_root: PathBuf,
    destination_root: PathBuf,
    journal_dir: PathBuf,
    backup_dir: PathBuf,
    rollback_dir: PathBuf,
    entries: Vec<CheatAvailabilityEntry>,
    opts: CheatInstallOptions,
}

/// Three entries in install order: a replacement of the user's edited
/// `Frogger.cht` (backup required), then two new installs.
fn crash_fixture(label: &str, run_id: &str) -> CrashFixture {
    let catalogue_root = temp_root(&format!("{label}-catalogue"));
    let destination_root = temp_root(&format!("{label}-dest"));
    let journal_dir = temp_root(&format!("{label}-journal"));
    let backup_dir = temp_root(&format!("{label}-backup"));
    let rollback_dir = temp_root(&format!("{label}-rollback"));
    let replace = replace_different_entry(
        &catalogue_root,
        &destination_root,
        b"catalogue content\n",
        b"USER EDITED\n",
    );
    let pitfall = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Pitfall",
        b"cheats = 0\n",
    );
    let zaxxon = install_new_entry(
        &catalogue_root,
        &destination_root,
        "Zaxxon",
        b"cheats = 2\n",
    );
    let opts = options(
        &destination_root,
        &journal_dir,
        &backup_dir,
        false,
        true,
        true,
        run_id,
    );
    CrashFixture {
        catalogue_root,
        destination_root,
        journal_dir,
        backup_dir,
        rollback_dir,
        entries: vec![replace, pitfall, zaxxon],
        opts,
    }
}

impl CrashFixture {
    fn platform_dir(&self) -> PathBuf {
        self.destination_root.join("Atari2600")
    }

    fn journal_path(&self) -> PathBuf {
        self.journal_dir.join(format!("{}.json", self.opts.run_id))
    }

    fn on_disk_journal(&self) -> CheatInstallRun {
        parse_cheat_install_run(&fs::read_to_string(self.journal_path()).unwrap()).unwrap()
    }

    /// Runs the installer under `fault` and asserts it died by the
    /// injected crash (a panic the test catches).
    fn run_until_crash(&self, fault: FaultPoint) {
        let guard = FaultGuard::new(fault);
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute_cheat_install_run(&self.entries, &self.opts)
        }));
        drop(guard);
        assert!(crashed.is_err(), "the injected crash must abort the run");
    }

    fn rollback(&self, confirmed: bool) -> CheatRollbackRunOutcome {
        execute_cheat_rollback_run(
            &self.journal_path(),
            &CheatRollbackOptions {
                journal_path: self.journal_path(),
                destination_root: self.destination_root.clone(),
                backup_directory: self.backup_dir.clone(),
                rollback_journal_directory: self.rollback_dir.clone(),
                dry_run: false,
                confirmed,
                run_id: format!("{}-rollback", self.opts.run_id),
                started_at_unix_seconds: 1_700_000_100,
            },
        )
    }

    fn history_options(&self) -> CheatHistoryOptions {
        CheatHistoryOptions {
            journal_root: self.journal_dir.clone(),
            backup_root: self.backup_dir.clone(),
            rollback_journal_root: self.rollback_dir.clone(),
        }
    }

    fn assert_destination_is_pre_run_state(&self) {
        assert_eq!(
            dir_entries(&self.platform_dir()),
            vec!["Frogger.cht".to_string()],
            "only the user's original file may remain"
        );
        assert_eq!(
            fs::read(self.platform_dir().join("Frogger.cht")).unwrap(),
            b"USER EDITED\n"
        );
    }
}

#[test]
fn pending_journal_is_written_before_the_first_mutation_in_planned_state() {
    let fixture = crash_fixture("pending-journal", "run-pending");
    // Crash right after entry 0 was applied, before its journal update: the
    // on-disk journal is exactly the pending journal written up front.
    fixture.run_until_crash(FaultPoint::CrashBeforeJournalUpdate(0));

    let journal = fixture.on_disk_journal();
    assert_eq!(journal.status, CheatInstallRunStatus::InProgress);
    assert!(journal.is_interrupted());
    assert_eq!(journal.completed_at_unix_seconds, None);
    assert_eq!(journal.settled_entries, 0);
    assert_eq!(journal.run_id, "run-pending");
    assert_eq!(journal.catalogue_source, "Fixture Catalogue");
    assert!(journal.allow_replace_different);
    assert_eq!(
        journal.destination_root,
        Some(CheatInstallPath::from_path(&fixture.destination_root))
    );
    assert_eq!(
        journal.temp_file_prefix.as_deref(),
        Some(format!(".archivefs-cheat-install-{}-", std::process::id()).as_str())
    );
    assert_eq!(journal.entries.len(), 3);
    for entry in &journal.entries {
        assert!(!entry.applied, "a pending journal never claims an apply");
        assert!(entry.resulting_destination_hash.is_none());
        assert!(entry.expected_source_hash.is_some());
        assert!(entry.destination_path.is_some());
    }
    let replace = &journal.entries[0];
    assert_eq!(replace.outcome, CheatInstallOutcome::ReplacedWithBackup);
    assert_eq!(
        replace.previous_destination_hash.as_deref(),
        Some(hex_sha256(b"USER EDITED\n").as_str())
    );
    let expected_backup = fixture.backup_dir.join(format!(
        "Frogger.cht.run-pending.{}.bak",
        &hex_sha256(b"USER EDITED\n")[..12]
    ));
    assert_eq!(
        replace.planned_backup_path,
        Some(CheatInstallPath::from_path(&expected_backup))
    );
    assert!(replace.backup_path.is_none());
    assert_eq!(
        journal.entries[1].outcome,
        CheatInstallOutcome::InstalledNew
    );
    assert_eq!(
        journal.entries[2].outcome,
        CheatInstallOutcome::InstalledNew
    );
    assert_eq!(
        journal.entry_settlement(0),
        CheatInstallEntrySettlement::InFlight
    );
    assert_eq!(
        journal.entry_settlement(1),
        CheatInstallEntrySettlement::NotStarted
    );

    // The crash happened after the replacement was published: the
    // destination holds catalogue content and the backup is exactly where
    // the pending journal said it would be.
    assert_eq!(
        fs::read(fixture.platform_dir().join("Frogger.cht")).unwrap(),
        b"catalogue content\n"
    );
    assert_eq!(fs::read(&expected_backup).unwrap(), b"USER EDITED\n");

    // History sees an interrupted run with an in-flight replacement that is
    // rollback-available via the planned backup; never "success".
    let inspection =
        inspect_cheat_install_journal(&fixture.journal_path(), &fixture.history_options()).unwrap();
    assert!(inspection.interrupted);
    assert_eq!(inspection.settled_entries, 0);
    assert_eq!(inspection.install_status, CheatInstallRunStatus::InProgress);
    assert!(
        inspection
            .warnings
            .iter()
            .any(|w| w.contains("interrupted"))
    );
    assert_eq!(
        inspection.entries[0].settlement,
        CheatInstallEntrySettlement::InFlight
    );
    assert_eq!(
        inspection.entries[0].rollback_availability,
        CheatRollbackAvailability::Available
    );
    assert_eq!(
        inspection.entries[1].settlement,
        CheatInstallEntrySettlement::NotStarted
    );
    assert_eq!(
        inspection.entries[1].rollback_availability,
        CheatRollbackAvailability::Unnecessary
    );

    // Rollback restores the user's file from the planned backup.
    let outcome = fixture.rollback(true);
    assert_eq!(outcome.run.status, CheatRollbackRunStatus::Success);
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatRollbackOutcome::RestoredBackup
    );
    assert_eq!(
        outcome.run.entries[1].outcome,
        CheatRollbackOutcome::NoChangeRequired
    );
    fixture.assert_destination_is_pre_run_state();
    let _ = &fixture.catalogue_root;
}

#[test]
fn crash_after_entry_k_journal_update_rolls_back_exactly_the_applied_entries() {
    let fixture = crash_fixture("crash-after-k", "run-crash-k");
    fixture.run_until_crash(FaultPoint::CrashAfterJournalUpdate(1));

    let journal = fixture.on_disk_journal();
    assert_eq!(journal.status, CheatInstallRunStatus::InProgress);
    assert_eq!(journal.settled_entries, 2);
    assert!(journal.entries[0].applied);
    assert_eq!(
        journal.entries[0].outcome,
        CheatInstallOutcome::ReplacedWithBackup
    );
    assert!(journal.entries[0].backup_path.is_some());
    assert!(journal.entries[1].applied);
    assert_eq!(
        journal.entries[1].outcome,
        CheatInstallOutcome::InstalledNew
    );
    assert!(!journal.entries[2].applied, "entry 3 is still planned");
    assert!(journal.entries[2].resulting_destination_hash.is_none());
    assert_eq!(
        journal.entry_settlement(2),
        CheatInstallEntrySettlement::InFlight
    );
    // Entry 3 was never started on disk.
    assert!(!fixture.platform_dir().join("Zaxxon.cht").exists());
    assert_eq!(journal.summary.writes_succeeded, 2);

    let inspection =
        inspect_cheat_install_journal(&fixture.journal_path(), &fixture.history_options()).unwrap();
    assert!(inspection.interrupted);
    assert_eq!(inspection.settled_entries, 2);
    assert_eq!(
        inspection.entries[0].settlement,
        CheatInstallEntrySettlement::Settled
    );
    assert_eq!(
        inspection.entries[0].rollback_availability,
        CheatRollbackAvailability::Available
    );
    assert_eq!(
        inspection.entries[1].rollback_availability,
        CheatRollbackAvailability::Available
    );
    assert_eq!(
        inspection.entries[2].settlement,
        CheatInstallEntrySettlement::InFlight
    );
    assert_eq!(
        inspection.entries[2].rollback_availability,
        CheatRollbackAvailability::Unnecessary,
        "an in-flight new install whose destination is absent was never published"
    );

    // Dry-run rollback previews, writes nothing.
    let preview = fixture.rollback(false);
    assert_eq!(preview.run.status, CheatRollbackRunStatus::DryRun);
    assert!(fixture.platform_dir().join("Pitfall.cht").exists());

    let outcome = fixture.rollback(true);
    assert_eq!(outcome.run.status, CheatRollbackRunStatus::Success);
    assert_eq!(
        outcome.run.entries[0].outcome,
        CheatRollbackOutcome::RestoredBackup
    );
    assert_eq!(
        outcome.run.entries[1].outcome,
        CheatRollbackOutcome::RemovedInstalledFile
    );
    assert_eq!(
        outcome.run.entries[2].outcome,
        CheatRollbackOutcome::NoChangeRequired
    );
    fixture.assert_destination_is_pre_run_state();
    // The backup recorded in the journal was the one used.
    let backup = PathBuf::from(&journal.entries[0].backup_path.as_ref().unwrap().display);
    assert_eq!(fs::read(&backup).unwrap(), b"USER EDITED\n");

    // History now binds the rollback journal and reports completion.
    let after =
        inspect_cheat_install_journal(&fixture.journal_path(), &fixture.history_options()).unwrap();
    assert!(after.rollback.exists);
    assert_eq!(after.rollback.completed_successfully, Some(true));
    assert!(after.entries.iter().all(|entry| entry.rollback_availability
        == CheatRollbackAvailability::AlreadyCompleted));
}

#[test]
fn entry_published_but_unrecorded_is_resolved_by_destination_hash_at_rollback() {
    let fixture = crash_fixture("crash-before-update", "run-in-flight");
    // Entry 0 settled and recorded; entry 1 published but the process died
    // before the journal update - the classic kill-between-rename-and-record.
    fixture.run_until_crash(FaultPoint::CrashBeforeJournalUpdate(1));

    let journal = fixture.on_disk_journal();
    assert_eq!(journal.settled_entries, 1);
    assert!(journal.entries[0].applied);
    assert!(
        !journal.entries[1].applied,
        "journal still shows the planned state"
    );
    assert_eq!(
        journal.entry_settlement(1),
        CheatInstallEntrySettlement::InFlight
    );
    assert!(
        fixture.platform_dir().join("Pitfall.cht").exists(),
        "the file was published before the crash"
    );

    // Leftovers this run could have left (same pid prefix) are swept by the
    // rollback; another process's temp file is not ours to touch.
    let own_temp = fixture.platform_dir().join(format!(
        ".archivefs-cheat-install-{}-deadbeef-7",
        std::process::id()
    ));
    fs::write(&own_temp, b"partial").unwrap();
    let foreign_temp = fixture
        .platform_dir()
        .join(".archivefs-cheat-install-999999999-cafe-1");
    fs::write(&foreign_temp, b"someone else's").unwrap();

    let inspection =
        inspect_cheat_install_journal(&fixture.journal_path(), &fixture.history_options()).unwrap();
    assert_eq!(
        inspection.entries[1].settlement,
        CheatInstallEntrySettlement::InFlight
    );
    assert_eq!(
        inspection.entries[1].rollback_availability,
        CheatRollbackAvailability::Available,
        "destination hash equals the source hash, so it was applied"
    );

    let outcome = fixture.rollback(true);
    assert_eq!(outcome.run.status, CheatRollbackRunStatus::Success);
    assert_eq!(
        outcome.run.entries[1].outcome,
        CheatRollbackOutcome::RemovedInstalledFile
    );
    assert!(outcome.run.entries[1].message.contains("interrupted"));
    assert!(!own_temp.exists(), "this run's leftover temp file is swept");
    assert!(
        foreign_temp.exists(),
        "another process's temp file is untouched"
    );
    fs::remove_file(&foreign_temp).unwrap();
    fixture.assert_destination_is_pre_run_state();

    // An in-flight entry whose destination holds neither the catalogue nor
    // the pre-install content is never touched.
    let fixture = crash_fixture("crash-before-update-edited", "run-in-flight-edited");
    fixture.run_until_crash(FaultPoint::CrashBeforeJournalUpdate(1));
    fs::write(
        fixture.platform_dir().join("Pitfall.cht"),
        b"edited in RetroArch after the crash\n",
    )
    .unwrap();
    let outcome = fixture.rollback(true);
    assert_eq!(
        outcome.run.entries[1].outcome,
        CheatRollbackOutcome::FailedDestinationChanged
    );
    assert_eq!(
        fs::read(fixture.platform_dir().join("Pitfall.cht")).unwrap(),
        b"edited in RetroArch after the crash\n"
    );
    assert_ne!(outcome.run.status, CheatRollbackRunStatus::Success);
}

#[test]
fn mid_run_journal_update_failure_stops_further_entries_truthfully() {
    let fixture = crash_fixture("update-fault", "run-update-fault");
    let guard = FaultGuard::new(FaultPoint::JournalUpdate(0));
    let outcome = execute_cheat_install_run(&fixture.entries, &fixture.opts);
    drop(guard);

    assert!(outcome.run.entries[0].applied);
    for entry in &outcome.run.entries[1..] {
        assert!(!entry.applied);
        assert_eq!(entry.outcome, CheatInstallOutcome::FailedWrite);
        assert_eq!(entry.reason_code, JOURNAL_UPDATE_FAILED_REASON);
    }
    assert!(!fixture.platform_dir().join("Pitfall.cht").exists());
    assert_eq!(outcome.run.status, CheatInstallRunStatus::PartialFailure);
    assert!(
        outcome.journal_error.is_some(),
        "the update failure is reported"
    );
    // The final write succeeded, so the on-disk record is complete and
    // claims exactly what was done.
    assert!(outcome.journal_path.is_some());
    let journal = fixture.on_disk_journal();
    assert_eq!(journal.status, CheatInstallRunStatus::PartialFailure);
    assert_eq!(journal.settled_entries, 3);
    assert!(journal.entries[0].applied);
    assert!(!journal.entries[1].applied);
}

#[test]
fn finished_journal_marks_every_entry_settled_and_old_journals_still_parse() {
    let fixture = crash_fixture("finished", "run-finished");
    let outcome = execute_cheat_install_run(&fixture.entries, &fixture.opts);
    assert_eq!(outcome.run.status, CheatInstallRunStatus::Success);
    let journal = fixture.on_disk_journal();
    assert_eq!(journal.settled_entries, 3);
    assert!(journal.completed_at_unix_seconds.is_some());
    assert!((0..3).all(|i| journal.entry_settlement(i) == CheatInstallEntrySettlement::Settled));

    // A journal written before `settled_entries`/`in_progress` existed has
    // neither field and is read as fully settled.
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fixture.journal_path()).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("settled_entries");
    value.as_object_mut().unwrap().remove("temp_file_prefix");
    for entry in value["entries"].as_array_mut().unwrap() {
        entry.as_object_mut().unwrap().remove("planned_backup_path");
    }
    let legacy = parse_cheat_install_run(&value.to_string()).unwrap();
    assert!(!legacy.is_interrupted());
    assert_eq!(legacy.settled_entries, 0);
    assert!((0..3).all(|i| legacy.entry_settlement(i) == CheatInstallEntrySettlement::Settled));
}

// ---------------------------------------------------------------------
// Rollback: no clobbering after a stale hash check, and a pending rollback
// record before the first mutation (Night Watch S04-02 / D2-02).
// ---------------------------------------------------------------------

use crate::patch_manager::cheat_rollback::{
    FaultPoint as RollbackFaultPoint, inject_rollback_fault_for_test,
    set_before_destination_mutation_hook_for_test,
};

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn d2_02_rollback_never_destroys_a_cheat_rewritten_after_its_hash_check() {
    let fixture = crash_fixture("d2-02", "d2-02");
    let installed = execute_cheat_install_run(&fixture.entries, &fixture.opts);
    assert_eq!(installed.run.status, CheatInstallRunStatus::Success);

    let racing = b"cheats = 1\ncheat0_desc = \"EDITED IN RETROARCH\"\n".to_vec();
    let racing_for_hook = racing.clone();
    set_before_destination_mutation_hook_for_test(Some(Box::new(move |path: &Path| {
        fs::write(path, &racing_for_hook).unwrap()
    })));
    let outcome = fixture.rollback(true);
    set_before_destination_mutation_hook_for_test(None);

    let survivors = |root: &Path| -> usize {
        walk(root)
            .into_iter()
            .filter(|p| fs::read(p).ok().as_deref() == Some(&racing[..]))
            .count()
    };
    // All three racing edits (Frogger restore target, Pitfall and Zaxxon
    // removal targets) must survive, in place or at a reported path.
    assert_eq!(
        survivors(&fixture.destination_root)
            + survivors(&fixture.rollback_dir)
            + survivors(&fixture.backup_dir),
        3
    );
    assert_eq!(
        fs::read(fixture.platform_dir().join("Frogger.cht")).unwrap(),
        racing
    );
    assert_eq!(
        fs::read(fixture.platform_dir().join("Pitfall.cht")).unwrap(),
        racing
    );
    assert_ne!(outcome.run.status, CheatRollbackRunStatus::Success);
    for entry in &outcome.run.entries {
        assert_eq!(
            entry.outcome,
            CheatRollbackOutcome::FailedDestinationChanged,
            "{}",
            entry.message
        );
        assert!(!entry.wrote);
    }
    // No temp or parked file is left next to the user's cheats.
    assert_eq!(
        dir_entries(&fixture.platform_dir()),
        vec!["Frogger.cht", "Pitfall.cht", "Zaxxon.cht"]
    );
    // The truthful rollback record exists and binds to the install.
    let inspection =
        inspect_cheat_install_journal(&fixture.journal_path(), &fixture.history_options()).unwrap();
    assert!(inspection.rollback.exists);
    assert_eq!(inspection.rollback.completed_successfully, Some(false));
}

#[test]
fn rollback_fails_closed_when_its_record_cannot_be_created() {
    let fixture = crash_fixture("rb-journal-first", "rb-journal-first");
    execute_cheat_install_run(&fixture.entries, &fixture.opts);
    let before = dir_entries(&fixture.platform_dir());

    inject_rollback_fault_for_test(Some(RollbackFaultPoint::JournalWrite));
    let outcome = fixture.rollback(true);
    inject_rollback_fault_for_test(None);

    assert_eq!(dir_entries(&fixture.platform_dir()), before);
    assert_eq!(
        fs::read(fixture.platform_dir().join("Frogger.cht")).unwrap(),
        b"catalogue content\n",
        "nothing may be restored without a rollback record"
    );
    assert_eq!(outcome.run.status, CheatRollbackRunStatus::Failed);
    assert!(outcome.journal_path.is_none());
    assert!(outcome.journal_error.is_some());
    assert!(outcome.run.entries.iter().all(|entry| {
        !entry.wrote && entry.error_code.as_deref() == Some("journal_unavailable")
    }));
    assert!(dir_entries(&fixture.rollback_dir).is_empty());
}

#[test]
fn rollback_killed_mid_way_leaves_a_pending_record_and_can_be_resumed() {
    let fixture = crash_fixture("rb-crash", "rb-crash");
    execute_cheat_install_run(&fixture.entries, &fixture.opts);

    inject_rollback_fault_for_test(Some(RollbackFaultPoint::CrashAfterEntry(0)));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fixture.rollback(true)));
    inject_rollback_fault_for_test(None);
    assert!(crashed.is_err());

    // Entry 0 was restored; the record says exactly that and no more.
    assert_eq!(
        fs::read(fixture.platform_dir().join("Frogger.cht")).unwrap(),
        b"USER EDITED\n"
    );
    assert!(fixture.platform_dir().join("Pitfall.cht").exists());
    let journals = dir_entries(&fixture.rollback_dir);
    assert_eq!(journals, vec!["rb-crash-rollback.json".to_string()]);
    let pending: crate::patch_manager::CheatRollbackRun =
        crate::patch_manager::parse_cheat_rollback_run(
            &fs::read_to_string(fixture.rollback_dir.join(&journals[0])).unwrap(),
        )
        .unwrap();
    assert_eq!(pending.status, CheatRollbackRunStatus::InProgress);
    assert_eq!(pending.completed_at_unix_seconds, None);
    assert_eq!(
        pending.entries[0].outcome,
        CheatRollbackOutcome::RestoredBackup
    );
    assert!(pending.entries[0].wrote);
    assert_eq!(pending.entries[1].outcome, CheatRollbackOutcome::Pending);
    assert_eq!(pending.entries[2].outcome, CheatRollbackOutcome::Pending);

    // History binds the pending record and refuses to call it completed.
    let inspection =
        inspect_cheat_install_journal(&fixture.journal_path(), &fixture.history_options()).unwrap();
    assert!(inspection.rollback.exists);
    assert_eq!(inspection.rollback.completed_successfully, Some(false));

    // A second rollback run finishes the job.
    let resumed = execute_cheat_rollback_run(
        &fixture.journal_path(),
        &CheatRollbackOptions {
            journal_path: fixture.journal_path(),
            destination_root: fixture.destination_root.clone(),
            backup_directory: fixture.backup_dir.clone(),
            rollback_journal_directory: fixture.rollback_dir.clone(),
            dry_run: false,
            confirmed: true,
            run_id: "rb-crash-rollback-2".into(),
            started_at_unix_seconds: 1_700_000_200,
        },
    );
    assert_eq!(resumed.run.status, CheatRollbackRunStatus::Success);
    assert_eq!(
        resumed.run.entries[0].outcome,
        CheatRollbackOutcome::AlreadyRestored
    );
    assert_eq!(
        resumed.run.entries[1].outcome,
        CheatRollbackOutcome::RemovedInstalledFile
    );
    fixture.assert_destination_is_pre_run_state();

    // History prefers the completed resumed run over the interrupted one it
    // superseded: not ambiguous, rollback complete (M2).
    let after =
        inspect_cheat_install_journal(&fixture.journal_path(), &fixture.history_options()).unwrap();
    assert!(after.rollback.exists);
    assert!(!after.rollback.ambiguous);
    assert_eq!(after.rollback.completed_successfully, Some(true));
    assert_eq!(
        after.rollback.run_id.as_deref(),
        Some("rb-crash-rollback-2")
    );
    assert!(after.entries.iter().all(|entry| entry.rollback_availability
        == CheatRollbackAvailability::AlreadyCompleted));
}

// ---------------------------------------------------------------------
// Installer lock: rollback refuses a still-live install (review M1).
// ---------------------------------------------------------------------

fn hold_installer_lock(journal_path: &Path) -> fs::File {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(installer_lock_path(journal_path))
        .unwrap();
    assert!(try_lock_exclusive(&file).unwrap(), "test must get the lock");
    file
}

#[test]
fn rollback_refuses_an_in_progress_journal_whose_installer_is_still_running() {
    let fixture = crash_fixture("live-install", "live-install");
    fixture.run_until_crash(FaultPoint::CrashAfterJournalUpdate(0));
    assert!(fixture.on_disk_journal().is_interrupted());
    let journal_before = fs::read(fixture.journal_path()).unwrap();
    let dest_before = dir_entries(&fixture.platform_dir());
    let frogger_before = fs::read(fixture.platform_dir().join("Frogger.cht")).unwrap();

    // A live installer holds the journal's lock.
    let held = hold_installer_lock(&fixture.journal_path());
    for confirmed in [true, false] {
        let refused = fixture.rollback(confirmed);
        assert_ne!(refused.run.status, CheatRollbackRunStatus::Success);
        assert!(refused.journal_path.is_none());
        assert!(refused.run.entries.iter().all(|entry| {
            !entry.wrote
                && entry.error_code.as_deref() == Some("install_still_running")
                && entry.message.contains("install still running")
        }));
    }
    assert_eq!(dir_entries(&fixture.platform_dir()), dest_before);
    assert_eq!(
        fs::read(fixture.platform_dir().join("Frogger.cht")).unwrap(),
        frogger_before
    );
    assert_eq!(fs::read(fixture.journal_path()).unwrap(), journal_before);
    assert!(dir_entries(&fixture.rollback_dir).is_empty());

    // The installer ends (a crash releases the lock the same way).
    drop(held);
    let outcome = fixture.rollback(true);
    assert_eq!(outcome.run.status, CheatRollbackRunStatus::Success);
    fixture.assert_destination_is_pre_run_state();
}

#[test]
fn installer_holds_its_lock_for_the_whole_run_and_removes_it_when_done() {
    let fixture = crash_fixture("installer-lock", "installer-lock");
    // Someone else holding this run's lock makes the installer fail closed.
    let held = hold_installer_lock(&fixture.journal_path());
    let blocked = execute_cheat_install_run(&fixture.entries, &fixture.opts);
    assert_eq!(blocked.run.status, CheatInstallRunStatus::Failed);
    assert!(blocked.journal_path.is_none());
    assert!(!fixture.journal_path().exists());
    assert!(!fixture.platform_dir().join("Pitfall.cht").exists());
    drop(held);
    fs::remove_file(installer_lock_path(&fixture.journal_path())).unwrap();

    let done = execute_cheat_install_run(&fixture.entries, &fixture.opts);
    assert_eq!(done.run.status, CheatInstallRunStatus::Success);
    assert_eq!(
        dir_entries(&fixture.journal_dir),
        vec!["installer-lock.json".to_string()],
        "the lock file is removed when the run ends"
    );
}

// ---------------------------------------------------------------------
// Filesystems without RENAME_NOREPLACE (review L1).
// ---------------------------------------------------------------------

#[test]
fn rollback_removes_installed_files_where_noreplace_is_unsupported() {
    let fixture = crash_fixture("noreplace-unsupported", "noreplace-unsupported");
    let installed = execute_cheat_install_run(&fixture.entries, &fixture.opts);
    assert_eq!(installed.run.status, CheatInstallRunStatus::Success);

    inject_rollback_fault_for_test(Some(RollbackFaultPoint::NoReplaceUnsupported));
    let outcome = fixture.rollback(true);
    inject_rollback_fault_for_test(None);

    assert_eq!(outcome.run.status, CheatRollbackRunStatus::Success);
    for entry in &outcome.run.entries[1..] {
        assert_eq!(
            entry.outcome,
            CheatRollbackOutcome::RemovedInstalledFile,
            "{}",
            entry.message
        );
        assert!(entry.wrote);
    }
    fixture.assert_destination_is_pre_run_state();
}

#[test]
fn noreplace_fallback_never_removes_a_cheat_rewritten_after_its_hash_check() {
    let fixture = crash_fixture("noreplace-race", "noreplace-race");
    execute_cheat_install_run(&fixture.entries, &fixture.opts);

    let racing = b"cheats = 1\ncheat0_desc = \"EDITED\"\n".to_vec();
    let racing_for_hook = racing.clone();
    set_before_destination_mutation_hook_for_test(Some(Box::new(move |path: &Path| {
        fs::write(path, &racing_for_hook).unwrap()
    })));
    inject_rollback_fault_for_test(Some(RollbackFaultPoint::NoReplaceUnsupported));
    let outcome = fixture.rollback(true);
    inject_rollback_fault_for_test(None);
    set_before_destination_mutation_hook_for_test(None);

    for (entry, name) in outcome.run.entries[1..]
        .iter()
        .zip(["Pitfall.cht", "Zaxxon.cht"])
    {
        assert_eq!(
            entry.outcome,
            CheatRollbackOutcome::FailedDestinationChanged,
            "{}",
            entry.message
        );
        assert!(!entry.wrote);
        assert_eq!(fs::read(fixture.platform_dir().join(name)).unwrap(), racing);
    }
}
