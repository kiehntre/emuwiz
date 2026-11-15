//! Hard Job #3: Blue settles a cheat/mod install that EmuWiz crashed in
//! (only a pending record on disk) automatically at startup, on a
//! background worker, into a reviewable History entry with Undo available.
//! It never performs Undo itself, never touches a destination root a live
//! apply still holds, and never deletes or overwrites a changed file.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::cheats_mods::duckstation_environment::test_environment::{Guard, TestMachine, install};
use archivefs_core::patch_manager::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedApplyConfirmation, SharedApplyOptions,
    SharedApplyStatus, SharedPreviewRequest, build_shared_preview, build_shared_transaction_plan,
    discover_pending_operations, discover_shared_apply_history, execute_shared_apply,
    preview_shared_rollback, set_test_terminal_journal_fault,
};

struct Crashed {
    _dir: tempfile::TempDir,
    _env: Guard,
    history: PathBuf,
    backups: PathBuf,
    destination: PathBuf,
    installed: PathBuf,
}

fn crashed_install(label: &str) -> Crashed {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let source_root = root.join("source");
    let destination = root.join("destination");
    let history = root.join("history");
    let backups = root.join("backups");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&destination).unwrap();
    let source = source_root.join("game.cht");
    fs::write(&source, b"cheats = 1\ncheat0_desc = \"new\"\n").unwrap();
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::RetroArch,
        selected_archive: root.join("selected.zip"),
        platform: Some("NES".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::RetroArchCatalogueMatch,
            state: PreviewIdentityState::Verified,
            value: Some("archive-1".into()),
            archive_path: root.join("selected.zip"),
            revision: None,
        },
        destination_root: destination.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::RetroArch,
            source_path: source,
            expected_source_digest: None,
            destination_relative_paths: vec![PathBuf::from("Nintendo - NES/game.cht")],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .unwrap();
    let plan = build_shared_transaction_plan(
        &report,
        "retroarch-native",
        "trusted-catalogue",
        &source_root,
    )
    .unwrap();
    // The crash: every file is published and checkpointed, the receipt
    // never gets written.
    set_test_terminal_journal_fault(true);
    let result = execute_shared_apply(
        &plan,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: true,
                replacement_approved: false,
            }),
            operation_id: format!("crashed-{label}"),
            timestamp_unix_seconds: 1_700_000_000,
            current_context: plan.context.clone(),
            history_root: history.clone(),
            backup_root: backups.clone(),
        },
    );
    set_test_terminal_journal_fault(false);
    assert!(result.journal_path.is_none(), "the crash left no receipt");
    let installed = destination.join("Nintendo - NES/game.cht");
    assert!(installed.exists());
    assert_eq!(discover_pending_operations(&history).len(), 1);
    assert!(discover_shared_apply_history(&history).journals.is_empty());
    let env = install(TestMachine {
        duckstation: None,
        history_root: history.clone(),
        backup_root: backups.clone(),
    });
    Crashed {
        _dir: dir,
        _env: env,
        history,
        backups,
        destination,
        installed,
    }
}

/// "Restart": a fresh app, the startup worker, polled until it reports.
fn restarted_app() -> ArchiveFsApp {
    let ctx = egui::Context::default();
    let mut app = app_for_operation_tests();
    app.start_startup_shared_recovery(ctx);
    for _ in 0..400 {
        app.poll_startup_shared_recovery();
        if !matches!(
            app.startup_shared_recovery,
            StartupSharedRecoveryState::Running { .. }
        ) {
            return app;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("startup recovery never finished");
}

fn activity_messages(app: &ArchiveFsApp) -> Vec<String> {
    app.history
        .entries()
        .filter(|entry| entry.action == ActivityAction::CheatInstall)
        .map(|entry| entry.message.clone())
        .collect()
}

fn lock_root(root: &Path) -> fs::File {
    use std::os::fd::AsRawFd;
    let file = fs::File::open(root).unwrap();
    // SAFETY: a live descriptor; LOCK_EX|LOCK_NB either succeeds or fails.
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    file
}

#[test]
fn restart_after_a_crash_settles_the_install_for_undo_without_undoing_it() {
    let crashed = crashed_install("restart");
    let app = restarted_app();
    assert!(matches!(
        app.startup_shared_recovery,
        StartupSharedRecoveryState::Done(ref recovered) if recovered.len() == 1
    ));
    let messages = activity_messages(&app);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("interrupted before it finished") && m.contains("use Undo there")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.starts_with("Startup check: 1 interrupted install(s) recorded for Undo, 0 still running, 0 need(s) attention")),
        "{messages:?}"
    );
    // Nothing was undone automatically.
    assert_eq!(
        fs::read(&crashed.installed).unwrap(),
        b"cheats = 1\ncheat0_desc = \"new\"\n"
    );
    // The gate the emulator pages (mGBA, melonDS, Flycast, RPCS3) check
    // before a new install is clear without visiting Mods & Cheats.
    assert!(discover_pending_operations(&crashed.history).is_empty());
    // History lists it and Undo is available on it.
    let history = discover_shared_apply_history(&crashed.history);
    assert_eq!(history.journals.len(), 1);
    assert_eq!(
        history.journals[0].1.status,
        SharedApplyStatus::PartialFailure
    );
    let journal_path = history.journals[0].0.to_path_buf().unwrap();
    let preview = preview_shared_rollback(&journal_path, &crashed.destination, &crashed.backups);
    assert!(preview.available, "{:?}", preview.entries);
}

#[test]
fn startup_leaves_a_destination_held_by_a_live_apply_alone() {
    let crashed = crashed_install("live");
    let held = lock_root(&crashed.destination);
    let app = restarted_app();
    let messages = activity_messages(&app);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("still running") && m.contains("left alone")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("0 interrupted install(s) recorded for Undo, 1 still running")),
        "{messages:?}"
    );
    assert_eq!(discover_pending_operations(&crashed.history).len(), 1);
    assert!(
        discover_shared_apply_history(&crashed.history)
            .journals
            .is_empty()
    );
    drop(held);
    // The secondary route (History refresh) settles it once the lock is gone.
    let ctx = egui::Context::default();
    let mut app = app;
    app.refresh_shared_history(ctx);
    for _ in 0..400 {
        app.poll_shared_history();
        if matches!(app.shared_history, SharedHistoryState::Ready(_)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(discover_pending_operations(&crashed.history).is_empty());
    assert_eq!(
        discover_shared_apply_history(&crashed.history)
            .journals
            .len(),
        1
    );
}

#[test]
fn a_file_changed_after_the_crash_blocks_undo_and_is_never_touched() {
    let crashed = crashed_install("conflict");
    fs::write(&crashed.installed, b"edited in RetroArch\n").unwrap();
    let app = restarted_app();
    let messages = activity_messages(&app);
    assert!(
        messages.iter().any(|m| m.contains("Undo stays blocked")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("1 need(s) attention")),
        "{messages:?}"
    );
    assert_eq!(
        fs::read(&crashed.installed).unwrap(),
        b"edited in RetroArch\n"
    );
    let history = discover_shared_apply_history(&crashed.history);
    assert_eq!(history.journals.len(), 1);
    let journal_path = history.journals[0].0.to_path_buf().unwrap();
    let preview = preview_shared_rollback(&journal_path, &crashed.destination, &crashed.backups);
    assert!(!preview.available);
}

#[cfg(unix)]
#[test]
fn a_recovery_that_cannot_write_its_receipt_keeps_the_pending_record_and_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let crashed = crashed_install("readonly-history");
    fs::set_permissions(&crashed.history, fs::Permissions::from_mode(0o555)).unwrap();
    let app = restarted_app();
    fs::set_permissions(&crashed.history, fs::Permissions::from_mode(0o755)).unwrap();
    let messages = activity_messages(&app);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("could not be settled") && m.contains("pending record is kept")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("1 need(s) attention")),
        "{messages:?}"
    );
    assert_eq!(discover_pending_operations(&crashed.history).len(), 1);
    assert!(
        discover_shared_apply_history(&crashed.history)
            .journals
            .is_empty()
    );
    assert_eq!(
        fs::read(&crashed.installed).unwrap(),
        b"cheats = 1\ncheat0_desc = \"new\"\n"
    );
}

#[test]
fn startup_with_nothing_pending_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let _env = install(TestMachine {
        duckstation: None,
        history_root: dir.path().join("history"),
        backup_root: dir.path().join("backups"),
    });
    let app = restarted_app();
    assert!(matches!(
        app.startup_shared_recovery,
        StartupSharedRecoveryState::Done(ref recovered) if recovered.is_empty()
    ));
    assert!(activity_messages(&app).is_empty());
}
