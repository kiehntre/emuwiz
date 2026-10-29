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

/// Creates the selected output directory without following a substituted
/// symlink in any component. The shared executor still checks confinement
/// immediately before each link operation.
#[cfg(unix)]
pub fn prepare_destination_root(path: &std::path::Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    if !path.is_absolute() {
        return Err("destination must be absolute".into());
    }
    let mut current = std::fs::File::open("/").map_err(|error| error.to_string())?;
    let mut walked = std::path::PathBuf::from("/");
    for component in path.components() {
        let Component::Normal(name) = component else {
            if component == Component::RootDir {
                continue;
            }
            return Err("destination contains an unsafe path component".into());
        };
        let name = CString::new(name.as_bytes()).map_err(|_| "destination contains NUL")?;
        // SAFETY: the directory descriptor and NUL-terminated component are
        // live for each call. The returned descriptor is immediately owned.
        let fd = unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("destination directory is unsafe: {error}"));
            }
            // SAFETY: same live descriptor and component; EEXIST is handled
            // by the following O_NOFOLLOW open, which verifies the object.
            if unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o755) } < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(format!("could not create destination directory: {error}"));
                }
            }
        }
        let fd = unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(format!(
                "destination directory is unsafe: {}",
                std::io::Error::last_os_error()
            ));
        }
        current = unsafe { std::fs::File::from_raw_fd(fd) };
        walked.push(std::ffi::OsStr::from_bytes(name.as_bytes()));
        let by_path = std::fs::symlink_metadata(&walked).map_err(|error| error.to_string())?;
        let by_fd = current.metadata().map_err(|error| error.to_string())?;
        if !by_path.is_dir()
            || by_path.file_type().is_symlink()
            || (by_path.dev(), by_path.ino()) != (by_fd.dev(), by_fd.ino())
        {
            return Err("destination directory changed while preparing it".into());
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn prepare_destination_root(_path: &std::path::Path) -> Result<(), String> {
    Err("linked Playing Library creation requires safe directory handles on this platform".into())
}

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
    use crate::dat::rename_apply::executor::{
        ApplyError, ApplyExecution, HardConflictMode, apply_transaction,
    };
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
                    winner_evidence: crate::playing_library::CandidateEvidenceSummary::unknown(),
                },
                launcher_operation: operation.clone(),
                companion_operations: Vec::new(),
            }],
            unresolved_groups: Vec::new(),
            exclusions: Vec::new(),
            singleton_families: 1,
            conflicts: Vec::new(),
            operations: vec![operation],
            rejected_launchers: Vec::new(),
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

    /// A plan with one CUE-based election: the launcher plus two companion
    /// BIN tracks.
    fn plan_with_cue_and_two_tracks(
        cue: PathBuf,
        track1: PathBuf,
        track2: PathBuf,
        destination_root: PathBuf,
    ) -> PlayingLibraryPlan {
        let launcher_operation = LinkedLibraryOperation {
            source_path: cue,
            destination_path: destination_root.join("Game (Europe).cue"),
        };
        let companion_operations = vec![
            LinkedLibraryOperation {
                source_path: track1,
                destination_path: destination_root.join("track1.bin"),
            },
            LinkedLibraryOperation {
                source_path: track2,
                destination_path: destination_root.join("track2.bin"),
            },
        ];
        let mut operations = vec![launcher_operation.clone()];
        operations.extend(companion_operations.iter().cloned());
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
                    winner_evidence: crate::playing_library::CandidateEvidenceSummary::unknown(),
                },
                launcher_operation,
                companion_operations,
            }],
            unresolved_groups: Vec::new(),
            exclusions: Vec::new(),
            singleton_families: 1,
            conflicts: Vec::new(),
            operations,
            rejected_launchers: Vec::new(),
        }
    }

    #[test]
    fn the_transaction_links_every_file_of_a_multi_file_release() {
        let temp = tempfile::tempdir().expect("temp dir");
        let cue = temp.path().join("Game (Europe).cue");
        let track1 = temp.path().join("track1.bin");
        let track2 = temp.path().join("track2.bin");
        std::fs::write(&cue, b"FILE \"track1.bin\" BINARY\n").unwrap();
        std::fs::write(&track1, b"one").unwrap();
        std::fs::write(&track2, b"two").unwrap();
        let destination_root = temp.path().join("playing");

        let plan = plan_with_cue_and_two_tracks(
            cue.clone(),
            track1.clone(),
            track2.clone(),
            destination_root.clone(),
        );
        let transaction = build_playing_library_transaction(&plan, 1).expect("transaction");

        assert_eq!(transaction.entries.len(), 3, "{:?}", transaction.entries);
        let sources: Vec<&std::path::Path> = transaction
            .entries
            .iter()
            .map(|entry| entry.source_path.as_path())
            .collect();
        assert!(sources.contains(&cue.as_path()));
        assert!(sources.contains(&track1.as_path()));
        assert!(sources.contains(&track2.as_path()));
    }

    #[test]
    fn applying_a_multi_file_release_links_the_complete_set() {
        let temp = tempfile::tempdir().expect("temp dir");
        let cue = temp.path().join("Game (Europe).cue");
        let track1 = temp.path().join("track1.bin");
        let track2 = temp.path().join("track2.bin");
        std::fs::write(&cue, b"FILE \"track1.bin\" BINARY\n").unwrap();
        std::fs::write(&track1, b"one").unwrap();
        std::fs::write(&track2, b"two").unwrap();
        let destination_root = temp.path().join("playing");
        std::fs::create_dir_all(&destination_root).unwrap();
        let journal_dir = temp.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();

        let plan = plan_with_cue_and_two_tracks(
            cue.clone(),
            track1.clone(),
            track2.clone(),
            destination_root.clone(),
        );
        let mut transaction = build_playing_library_transaction(&plan, 1).expect("transaction");
        write_journal(&journal_dir, &transaction).unwrap();

        let approved_paths = [&cue, &track1, &track2]
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let outcome = apply_transaction(&mut ApplyExecution {
            transaction: &mut transaction,
            approved_paths,
            current_generation: 1,
            trusted: TrustedRoots::from_paths([temp.path()]),
            journal_dir: journal_dir.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .expect("apply succeeds");

        assert_eq!(outcome.summary.applied, 3);
        for (name, original) in [
            ("Game (Europe).cue", &cue),
            ("track1.bin", &track1),
            ("track2.bin", &track2),
        ] {
            let link = destination_root.join(name);
            assert!(link.is_symlink(), "{name} must be linked");
            assert_eq!(&std::fs::read_link(&link).unwrap(), original);
        }
    }

    #[test]
    fn an_induced_failure_in_a_multi_file_release_leaves_no_partial_release() {
        // A destination one of the release's own files would need to
        // occupy is pre-created as a real directory (not a symlink) - the
        // shared preflight's no-clobber check refuses to replace it.
        // `HardConflictMode::AbortAll` (what this apply path always uses)
        // preflights the *entire* batch before mutating anything, so this
        // proves requirement 11 directly: the failure leaves nothing
        // applied at all, for any file in the release - not just the one
        // that actually conflicts.
        let temp = tempfile::tempdir().expect("temp dir");
        let cue = temp.path().join("Game (Europe).cue");
        let track1 = temp.path().join("track1.bin");
        let track2 = temp.path().join("track2.bin");
        std::fs::write(&cue, b"FILE \"track1.bin\" BINARY\n").unwrap();
        std::fs::write(&track1, b"one").unwrap();
        std::fs::write(&track2, b"two").unwrap();
        let destination_root = temp.path().join("playing");
        std::fs::create_dir_all(&destination_root).unwrap();
        // Block one companion's destination with a real directory.
        std::fs::create_dir_all(destination_root.join("track2.bin")).unwrap();
        let journal_dir = temp.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();

        let plan = plan_with_cue_and_two_tracks(
            cue.clone(),
            track1.clone(),
            track2.clone(),
            destination_root.clone(),
        );
        let mut transaction = build_playing_library_transaction(&plan, 1).expect("transaction");
        write_journal(&journal_dir, &transaction).unwrap();

        let approved_paths = [&cue, &track1, &track2]
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let error = apply_transaction(&mut ApplyExecution {
            transaction: &mut transaction,
            approved_paths,
            current_generation: 1,
            trusted: TrustedRoots::from_paths([temp.path()]),
            journal_dir: journal_dir.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .expect_err("a hard conflict on any one file must abort the whole batch");
        assert!(matches!(error, ApplyError::HardConflicts(_)));

        // Nothing was applied for this release - not the launcher, not
        // the unblocked companion either.
        assert!(!destination_root.join("Game (Europe).cue").exists());
        assert!(!destination_root.join("track1.bin").exists());
        // The pre-existing blocking directory itself is untouched.
        assert!(destination_root.join("track2.bin").is_dir());
        // The originals are always untouched.
        assert_eq!(std::fs::read(&track1).unwrap(), b"one");
        assert_eq!(std::fs::read(&track2).unwrap(), b"two");
    }

    #[test]
    fn rollback_of_a_multi_file_release_retains_proved_link_tombstones() {
        let temp = tempfile::tempdir().expect("temp dir");
        let cue = temp.path().join("Game (Europe).cue");
        let track1 = temp.path().join("track1.bin");
        let track2 = temp.path().join("track2.bin");
        std::fs::write(&cue, b"FILE \"track1.bin\" BINARY\n").unwrap();
        std::fs::write(&track1, b"one").unwrap();
        std::fs::write(&track2, b"two").unwrap();
        let destination_root = temp.path().join("playing");
        std::fs::create_dir_all(&destination_root).unwrap();
        let journal_dir = temp.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();

        let plan = plan_with_cue_and_two_tracks(
            cue.clone(),
            track1.clone(),
            track2.clone(),
            destination_root.clone(),
        );
        let mut transaction = build_playing_library_transaction(&plan, 1).expect("transaction");
        write_journal(&journal_dir, &transaction).unwrap();
        let approved_paths = [&cue, &track1, &track2]
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        apply_transaction(&mut ApplyExecution {
            transaction: &mut transaction,
            approved_paths,
            current_generation: 1,
            trusted: TrustedRoots::from_paths([temp.path()]),
            journal_dir: journal_dir.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .expect("apply succeeds");
        assert!(destination_root.join("Game (Europe).cue").is_symlink());
        assert!(destination_root.join("track1.bin").is_symlink());
        assert!(destination_root.join("track2.bin").is_symlink());

        crate::dat::rename_apply::rollback::rollback_transaction(
            &mut transaction,
            &journal_dir,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .expect("rollback succeeds");

        assert!(!destination_root.join("Game (Europe).cue").exists());
        assert!(!destination_root.join("track1.bin").exists());
        assert!(!destination_root.join("track2.bin").exists());
        for entry in &transaction.entries {
            let proof = entry.created_link_identity().unwrap();
            let tombstone = crate::dat::rename_apply::dirsync::link_undo_tombstone_path(
                &entry.destination_path,
                &proof,
            );
            assert!(tombstone.is_symlink());
            assert_eq!(std::fs::read_link(&tombstone).unwrap(), entry.source_path);
            assert_eq!(
                entry
                    .unknown
                    .get("emuwiz_link_undo_tombstone_v1")
                    .and_then(serde_json::Value::as_str),
                Some(tombstone.to_str().unwrap())
            );
        }
        // Only the generated links moved out of the public library paths;
        // every master file remains exactly as it was.
        assert!(cue.is_file() && !cue.is_symlink());
        assert_eq!(
            std::fs::read(&cue).unwrap(),
            b"FILE \"track1.bin\" BINARY\n"
        );
        assert_eq!(std::fs::read(&track1).unwrap(), b"one");
        assert_eq!(std::fs::read(&track2).unwrap(), b"two");
    }

    #[test]
    fn reapplying_the_same_multi_file_plan_refuses_existing_links() {
        let temp = tempfile::tempdir().expect("temp dir");
        let cue = temp.path().join("Game (Europe).cue");
        let track1 = temp.path().join("track1.bin");
        let track2 = temp.path().join("track2.bin");
        std::fs::write(&cue, b"FILE \"track1.bin\" BINARY\n").unwrap();
        std::fs::write(&track1, b"one").unwrap();
        std::fs::write(&track2, b"two").unwrap();
        let destination_root = temp.path().join("playing");
        std::fs::create_dir_all(&destination_root).unwrap();
        let journal_dir = temp.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();
        let trusted = TrustedRoots::from_paths([temp.path()]);
        let approved_paths: std::collections::BTreeSet<String> = [&cue, &track1, &track2]
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();

        let plan = plan_with_cue_and_two_tracks(
            cue.clone(),
            track1.clone(),
            track2.clone(),
            destination_root.clone(),
        );

        // First apply.
        let mut first = build_playing_library_transaction(&plan, 1).expect("transaction");
        write_journal(&journal_dir, &first).unwrap();
        let outcome = apply_transaction(&mut ApplyExecution {
            transaction: &mut first,
            approved_paths: approved_paths.clone(),
            current_generation: 1,
            trusted: trusted.clone(),
            journal_dir: journal_dir.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .expect("first apply succeeds");
        assert_eq!(outcome.summary.applied, 3);

        // A fresh operation cannot claim links that an earlier operation made.
        let mut second = build_playing_library_transaction(&plan, 2).expect("transaction");
        write_journal(&journal_dir, &second).unwrap();
        let result = apply_transaction(&mut ApplyExecution {
            transaction: &mut second,
            approved_paths,
            current_generation: 2,
            trusted,
            journal_dir: journal_dir.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        });
        assert!(matches!(result, Err(ApplyError::HardConflicts(_))));
        assert_eq!(second.applied_count(), 0);

        for name in ["Game (Europe).cue", "track1.bin", "track2.bin"] {
            assert!(destination_root.join(name).is_symlink());
        }
    }

    #[cfg(unix)]
    #[test]
    fn undo_refuses_an_external_link_with_the_same_target_text() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("game.rom");
        std::fs::write(&source, b"verified").unwrap();
        let destination = temp.path().join("playing");
        std::fs::create_dir_all(&destination).unwrap();
        let journal = temp.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let plan = plan_with_one_operation(source.clone(), destination.clone());
        let mut tx = build_playing_library_transaction(&plan, 1).unwrap();
        let approved_paths = std::iter::once(source.to_string_lossy().into_owned()).collect();
        apply_transaction(&mut ApplyExecution {
            transaction: &mut tx,
            approved_paths,
            current_generation: 1,
            trusted: TrustedRoots::from_paths([temp.path()]),
            journal_dir: journal.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .unwrap();
        let link = destination.join("Game (Europe).zip");
        let owned = crate::dat::rename_apply::identity::capture_identity(&link).unwrap();
        let held_original = destination.join("held-original-link");
        std::fs::rename(&link, &held_original).unwrap();
        std::os::unix::fs::symlink(&source, &link).unwrap();
        let replacement = crate::dat::rename_apply::identity::capture_identity(&link).unwrap();
        assert!(!crate::dat::rename_apply::identity::identity_matches(
            &owned,
            &replacement
        ));
        let result = crate::dat::rename_apply::rollback::rollback_transaction(
            &mut tx,
            &journal,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert!(matches!(
            result.result,
            crate::dat::rename_apply::model::RollbackResult::RollbackFailed { .. }
        ));
        assert!(link.is_symlink());
        assert!(held_original.is_symlink());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn undo_race_restores_a_replacement_without_deleting_it() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("game.rom");
        std::fs::write(&source, b"verified").unwrap();
        let destination = temp.path().join("playing");
        std::fs::create_dir_all(&destination).unwrap();
        let journal = temp.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let plan = plan_with_one_operation(source.clone(), destination.clone());
        let mut tx = build_playing_library_transaction(&plan, 1).unwrap();
        apply_transaction(&mut ApplyExecution {
            transaction: &mut tx,
            approved_paths: std::iter::once(source.to_string_lossy().into_owned()).collect(),
            current_generation: 1,
            trusted: TrustedRoots::from_paths([temp.path()]),
            journal_dir: journal.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .unwrap();
        let link = destination.join("Game (Europe).zip");
        let held_original = destination.join("held-original-link");
        let source_for_hook = source.clone();
        let held_for_hook = held_original.clone();
        let mut fired = false;
        crate::dat::rename_apply::dirsync::set_undo_move_hook(Some(Box::new(move |path| {
            if fired {
                return;
            }
            fired = true;
            std::fs::rename(path, &held_for_hook).unwrap();
            std::os::unix::fs::symlink(&source_for_hook, path).unwrap();
        })));
        let result = crate::dat::rename_apply::rollback::rollback_transaction(
            &mut tx,
            &journal,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        crate::dat::rename_apply::dirsync::set_undo_move_hook(None);
        assert!(matches!(
            result.result,
            crate::dat::rename_apply::model::RollbackResult::RollbackFailed { .. }
        ));
        assert!(link.is_symlink());
        assert!(held_original.is_symlink());
        assert_eq!(std::fs::read(source).unwrap(), b"verified");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn post_create_failure_retains_unproved_link_for_restart_review() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("game.rom");
        std::fs::write(&source, b"verified").unwrap();
        let destination = temp.path().join("playing");
        std::fs::create_dir_all(&destination).unwrap();
        let journal = temp.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let plan = plan_with_one_operation(source.clone(), destination.clone());
        let mut tx = build_playing_library_transaction(&plan, 1).unwrap();
        crate::dat::rename_apply::dirsync::set_after_link_hook(Some(Box::new(|_| {
            Err(std::io::Error::other("injected post-create failure"))
        })));
        let outcome = apply_transaction(&mut ApplyExecution {
            transaction: &mut tx,
            approved_paths: std::iter::once(source.to_string_lossy().into_owned()).collect(),
            current_generation: 1,
            trusted: TrustedRoots::from_paths([temp.path()]),
            journal_dir: journal.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .unwrap();
        crate::dat::rename_apply::dirsync::set_after_link_hook(None);
        assert_eq!(outcome.transaction.state, TransactionState::ApplyFailed);
        assert_eq!(tx.entries[0].state, EntryState::Applying);
        assert!(tx.entries[0].created_link_identity().is_none());
        assert!(destination.join("Game (Europe).zip").is_symlink());
        let issues =
            crate::dat::rename_apply::reconcile::reconcile_recovery(&mut tx, &journal).unwrap();
        assert_eq!(
            issues[0].kind,
            crate::dat::rename_apply::reconcile::RecoveryIssueKind::DestinationIdentityChanged
        );
        assert_eq!(tx.entries[0].state, EntryState::Applying);
        assert_eq!(std::fs::read(source).unwrap(), b"verified");
    }
}
