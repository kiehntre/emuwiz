//! Converts a [`PlayingLibraryPlan`]'s elected operations into a
//! [`RenameTransaction`] the *existing* shared apply engine can run, journal,
//! and roll back.
//!
//! There is no second filesystem engine here: every entry becomes a
//! `TransactionOperation::CreateSymlink`, the exact operation
//! `crate::dat::rom_organisation`'s `BuildLinkedLibrary` mode already
//! produces, applied through the same `crate::dat::rename_apply::executor`,
//! journaled and rolled back through the same `crate::dat::rename_apply`
//! machinery every other apply path in this crate uses.

use crate::dat::classification::CLASSIFIER_VERSION;
use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::rename_apply::model::{
    EntryState, ObjectKind, RenameTransaction, TransactionEntry, TransactionOperation,
    TransactionState,
};

use super::PlayingLibraryPlan;

/// Builds an unwritten, unapplied [`RenameTransaction`] from `plan`'s
/// conflict-free elected operations. Read-only: captures each source's
/// current identity but writes no journal and mutates nothing. The caller
/// still owns writing the journal and calling
/// `crate::dat::rename_apply::executor::apply_transaction` - exactly the
/// same two-step shape every other apply path in this crate follows.
///
/// `generation` becomes the transaction's `plan_generation`, so a later
/// apply attempt against a plan the caller has since regenerated is refused
/// by the shared executor's own staleness check, the same protection every
/// other plan/apply pair in this crate already has.
///
/// Refuses outright if `plan` still has unresolved destination conflicts -
/// applying a subset while some elections literally could not get a safe
/// destination name would be misleading, not merely partial. A source that
/// vanished since planning, or is no longer a regular file, is silently
/// dropped from the transaction (the same defence in depth
/// `crate::dat::rom_organisation::transaction::build_organisation_transaction`
/// already applies) rather than failing the whole batch over one stale file.
pub fn build_playing_library_transaction(
    plan: &PlayingLibraryPlan,
    generation: u64,
) -> Result<RenameTransaction, String> {
    if !plan.conflicts.is_empty() {
        return Err(format!(
            "{} destination name conflict(s) must be resolved before applying",
            plan.conflicts.len()
        ));
    }
    if !plan.destination_root.is_absolute() {
        return Err("the destination folder must be an absolute path".to_string());
    }

    let mut entries = Vec::new();
    for operation in &plan.operations {
        if !operation.source_path.is_absolute() {
            continue;
        }
        let Ok(identity) = capture_identity(&operation.source_path) else {
            continue;
        };
        if identity.kind != ObjectKind::RegularFile {
            continue;
        }
        let original_basename = operation
            .source_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(proposed_basename) = operation.destination_path.file_name() else {
            continue;
        };
        entries.push(TransactionEntry {
            source_path: operation.source_path.clone(),
            destination_path: operation.destination_path.clone(),
            original_basename,
            proposed_basename: proposed_basename.to_string_lossy().into_owned(),
            identity,
            operation: TransactionOperation::CreateSymlink {
                expected_target: operation.source_path.clone(),
                destination_root: plan.destination_root.clone(),
            },
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        });
    }
    if entries.is_empty() {
        return Err("no election produced an applicable link operation".to_string());
    }

    Ok(RenameTransaction {
        transaction_id: crate::dat::rename_apply::journal::new_transaction_id(
            crate::dat::sources::now_unix(),
        ),
        plan_generation: generation,
        classifier_version: Some(CLASSIFIER_VERSION.to_string()),
        created_at_unix: crate::dat::sources::now_unix(),
        source_scan_root: plan.destination_root.to_string_lossy().into_owned(),
        state: TransactionState::Planned,
        entries,
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::dat::rename_apply::executor::{ApplyExecution, HardConflictMode, apply_transaction};
    use crate::dat::rename_apply::journal::write_journal;
    use crate::dat::rename_apply::preflight::DirectoryPolicy;
    use crate::playing_library::model::{
        DestinationConflict, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
        PlayingLibraryPolicy,
    };
    use crate::safe_read::TrustedRoots;

    fn plan_with_one_operation(source: PathBuf, destination_root: PathBuf) -> PlayingLibraryPlan {
        let operation = LinkedLibraryOperation {
            source_path: source,
            destination_path: destination_root.join("Game (Europe).zip"),
        };
        PlayingLibraryPlan {
            destination_root,
            policy: PlayingLibraryPolicy::default(),
            archives_examined: 1,
            families_examined: 1,
            elected_games: vec![ElectedGame {
                dat_entry_name: "Game (Europe)".to_string(),
                family_root_name: "Game (Europe)".to_string(),
                explanation: ElectionExplanation {
                    steps: Vec::new(),
                    rejected: Vec::new(),
                },
                operation: operation.clone(),
            }],
            unresolved_groups: Vec::new(),
            exclusions: Vec::new(),
            singleton_families: 1,
            conflicts: Vec::new(),
            operations: vec![operation],
        }
    }

    #[test]
    fn builds_a_create_symlink_transaction_from_one_election() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source = temp.path().join("Game (Europe).zip");
        std::fs::write(&source, b"contents").unwrap();
        let destination_root = temp.path().join("playing");

        let plan = plan_with_one_operation(source.clone(), destination_root.clone());
        let transaction = build_playing_library_transaction(&plan, 1).expect("transaction");

        assert_eq!(transaction.entries.len(), 1);
        assert_eq!(transaction.entries[0].source_path, source);
        assert_eq!(
            transaction.entries[0].destination_path,
            destination_root.join("Game (Europe).zip")
        );
        match &transaction.entries[0].operation {
            TransactionOperation::CreateSymlink {
                expected_target,
                destination_root: root,
            } => {
                assert_eq!(expected_target, &source);
                assert_eq!(root, &destination_root);
            }
            other => panic!("expected CreateSymlink, got {other:?}"),
        }
        // The original file is untouched: building a transaction never
        // mutates anything.
        assert_eq!(std::fs::read(&source).unwrap(), b"contents");
        assert!(!destination_root.exists());
    }

    #[test]
    fn refuses_when_conflicts_remain() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source = temp.path().join("Game (Europe).zip");
        std::fs::write(&source, b"contents").unwrap();
        let destination_root = temp.path().join("playing");
        let mut plan = plan_with_one_operation(source, destination_root);
        plan.conflicts.push(DestinationConflict {
            destination_basename: "game.zip".to_string(),
            contenders: vec!["A".to_string(), "B".to_string()],
            destinations: Vec::new(),
        });

        let error = build_playing_library_transaction(&plan, 1)
            .expect_err("must refuse while conflicts remain");
        assert!(error.contains("conflict"));
    }

    #[test]
    fn the_built_transaction_applies_through_the_existing_shared_executor_and_creates_a_symlink() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source = temp.path().join("Game (Europe).zip");
        std::fs::write(&source, b"contents").unwrap();
        let destination_root = temp.path().join("playing");
        std::fs::create_dir_all(&destination_root).unwrap();
        let journal_dir = temp.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();

        let plan = plan_with_one_operation(source.clone(), destination_root.clone());
        let mut transaction = build_playing_library_transaction(&plan, 1).expect("transaction");
        write_journal(&journal_dir, &transaction).unwrap();

        let outcome = apply_transaction(&mut ApplyExecution {
            transaction: &mut transaction,
            approved_paths: [source.to_string_lossy().into_owned()]
                .into_iter()
                .collect(),
            current_generation: 1,
            trusted: TrustedRoots::from_paths([temp.path()]),
            journal_dir: journal_dir.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .expect("apply succeeds");

        assert_eq!(outcome.summary.applied, 1);
        let link = destination_root.join("Game (Europe).zip");
        assert!(link.is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), source);
        // The original file is untouched.
        assert_eq!(std::fs::read(&source).unwrap(), b"contents");

        let on_disk = crate::dat::rename_apply::read_journal(
            &crate::dat::rename_apply::journal_path(&journal_dir, &transaction.transaction_id)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(on_disk.state, TransactionState::Applied);
    }
}
