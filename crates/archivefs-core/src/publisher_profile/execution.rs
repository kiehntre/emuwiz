//! Phase 2A/2B PublisherPlan-to-transaction adapter.
//!
//! This is a foundation only. It reuses the existing Playing Library
//! transaction builder and shared rename executor model. Building remains
//! inspection-only; the separate explicit apply helper delegates execution to
//! the existing journaled transaction pipeline. Hardlink and symlink are
//! distinct caller-selected modes, with no automatic fallback or copy mode.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::dat::rename_apply::{
    ApplyError, ApplyOutcome, RenameTransaction, TransactionOperation, capture_identity,
    destination_is_confined,
};
use crate::playing_library::{
    CandidateEvidenceSummary, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
    PlayingLibraryPlan, PlayingLibraryPolicy, build_playing_library_transaction,
};

use super::destination_inspection::inspect_destination;
use super::model::{DestinationState, PublisherActionKind, PublisherActionSafety, PublisherPlan};
use super::planner::publisher_plan_hash_matches;

/// Explicit link selection. There is no implicit hardlink-to-symlink
/// fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublisherLinkMode {
    Hardlink,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublisherDirectoryState {
    PreExisting,
    CreatedByTransaction,
    NotCreated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherDirectory {
    pub path: PathBuf,
    pub state: PublisherDirectoryState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherDestinationRootIdentity {
    pub modified: std::time::SystemTime,
    #[cfg(unix)]
    pub ino: u64,
    #[cfg(unix)]
    pub dev: u64,
}

/// Publisher-specific directory plan wrapped around the existing shared
/// transaction. The shared transaction's `created_directories` remains the
/// durable ownership record after apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherTransaction {
    pub transaction: RenameTransaction,
    pub directories: Vec<PublisherDirectory>,
    pub link_mode: PublisherLinkMode,
    pub destination_root_identity: PublisherDestinationRootIdentity,
}

/// A typed refusal from the publisher adapter. Every variant is fail-closed:
/// no transaction is returned and no filesystem mutation occurs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherExecutionError {
    StalePlan {
        detail: String,
    },
    NoEligibleItems,
    UnsupportedAction {
        title: String,
        action: PublisherActionKind,
    },
    HardlinkUnavailable {
        title: String,
        detail: String,
    },
    DestinationDirectoryMissing {
        path: PathBuf,
    },
    DestinationChanged {
        title: String,
        state: DestinationState,
    },
    Collision {
        detail: String,
    },
    SourceInvalid {
        title: String,
        detail: String,
    },
    TransactionBuild(String),
    DirectoryConflict {
        path: PathBuf,
        detail: String,
    },
}

/// Builds an unwritten, unapplied shared transaction from the safe subset of
/// a PublisherPlan. Already-present and non-safe items are excluded. The
/// result uses `TransactionOperation::CreateHardlink`, so the eventual shared
/// executor acts only on destination-side paths and leaves sources in place.
///
/// The adapter performs a fresh plan-hash, source, destination, case-fold, and
/// same-filesystem check immediately before constructing the transaction.
/// The compatibility hardlink-only entry point retains Phase 2A's existing
/// destination-directory requirement; the policy entry point explicitly plans
/// missing directories for Phase 2B's shared directory orchestrator.
pub fn build_publisher_transaction(
    plan: &PublisherPlan,
    generation: u64,
) -> Result<RenameTransaction, PublisherExecutionError> {
    if std::fs::symlink_metadata(&plan.destination_root).is_err() {
        return Err(PublisherExecutionError::DestinationDirectoryMissing {
            path: plan.destination_root.clone(),
        });
    }
    let publisher =
        build_publisher_transaction_with_policy(plan, generation, PublisherLinkMode::Hardlink)?;
    if let Some(directory) = publisher
        .directories
        .iter()
        .find(|directory| directory.state == PublisherDirectoryState::NotCreated)
    {
        return Err(PublisherExecutionError::DestinationDirectoryMissing {
            path: directory.path.clone(),
        });
    }
    Ok(publisher.transaction)
}

/// Builds a Publisher transaction with an explicit hardlink or symlink mode.
/// This function only inspects the filesystem and returns an unwritten
/// transaction; directory creation happens only in the explicit apply helper.
pub fn build_publisher_transaction_with_policy(
    plan: &PublisherPlan,
    generation: u64,
    link_mode: PublisherLinkMode,
) -> Result<PublisherTransaction, PublisherExecutionError> {
    if !publisher_plan_hash_matches(plan) {
        return Err(PublisherExecutionError::StalePlan {
            detail: "publisher preview fingerprint no longer matches its contents".to_string(),
        });
    }
    if !plan.destination_root.is_absolute() {
        return Err(PublisherExecutionError::StalePlan {
            detail: "publisher destination root is not absolute".to_string(),
        });
    }
    let destination_root_identity = capture_destination_root_identity(&plan.destination_root)
        .map_err(|error| PublisherExecutionError::DirectoryConflict {
            path: plan.destination_root.clone(),
            detail: error.to_string(),
        })?;

    let mut selected: Vec<_> = plan
        .items
        .iter()
        .filter(|item| {
            item.safety == PublisherActionSafety::SafeToAct
                && item.destination_state != DestinationState::AlreadyCorrect
        })
        .collect();
    if selected.is_empty() {
        return Err(PublisherExecutionError::NoEligibleItems);
    }
    selected.sort_by_key(|item| {
        (
            item.planned_destination.clone().unwrap_or_default(),
            item.source_path.clone(),
        )
    });

    let mut by_exact: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut by_casefold: BTreeMap<String, (PathBuf, String)> = BTreeMap::new();
    for item in &selected {
        if item.planned_action.kind != PublisherActionKind::Symlink {
            return Err(PublisherExecutionError::UnsupportedAction {
                title: item.dat_entry_name.clone(),
                action: item.planned_action.kind,
            });
        }
        let Some(destination) = item.planned_destination.as_ref() else {
            return Err(PublisherExecutionError::SourceInvalid {
                title: item.dat_entry_name.clone(),
                detail: "safe item has no destination path".to_string(),
            });
        };
        if !item.source_path.is_absolute() || !destination.is_absolute() {
            return Err(PublisherExecutionError::SourceInvalid {
                title: item.dat_entry_name.clone(),
                detail: "source and destination must be absolute".to_string(),
            });
        }
        let Some(parent) = destination.parent() else {
            return Err(PublisherExecutionError::DestinationDirectoryMissing {
                path: destination.clone(),
            });
        };
        if let Ok(parent_metadata) = std::fs::metadata(parent)
            && !parent_metadata.is_dir()
        {
            return Err(PublisherExecutionError::DirectoryConflict {
                path: parent.to_path_buf(),
                detail: "required destination parent is not a directory".to_string(),
            });
        }
        let source = capture_identity(&item.source_path).map_err(|error| {
            PublisherExecutionError::SourceInvalid {
                title: item.dat_entry_name.clone(),
                detail: error.to_string(),
            }
        })?;
        if source.kind != crate::dat::rename_apply::ObjectKind::RegularFile {
            return Err(PublisherExecutionError::SourceInvalid {
                title: item.dat_entry_name.clone(),
                detail: "source is not a regular file".to_string(),
            });
        }
        if link_mode == PublisherLinkMode::Hardlink
            && !same_filesystem(&item.source_path, parent, &plan.destination_root)
        {
            return Err(PublisherExecutionError::HardlinkUnavailable {
                title: item.dat_entry_name.clone(),
                detail: "source and destination are on different filesystems; choose explicit SYMLINK mode".to_string(),
            });
        }
        if !destination_is_confined(destination, &plan.destination_root) {
            return Err(PublisherExecutionError::SourceInvalid {
                title: item.dat_entry_name.clone(),
                detail: "destination is outside the publisher root".to_string(),
            });
        }

        let live_state = inspect_destination(destination, &item.source_path);
        if live_state != DestinationState::Missing {
            return Err(PublisherExecutionError::DestinationChanged {
                title: item.dat_entry_name.clone(),
                state: live_state,
            });
        }
        if parent.exists() && has_casefold_sibling(parent, destination) {
            return Err(PublisherExecutionError::Collision {
                detail: format!(
                    "live case-fold destination collision for {}",
                    item.dat_entry_name
                ),
            });
        }
        let exact_key = destination.clone();
        if let Some(previous) = by_exact.insert(exact_key.clone(), item.dat_entry_name.clone()) {
            return Err(PublisherExecutionError::Collision {
                detail: format!(
                    "exact destination collision between {previous} and {}",
                    item.dat_entry_name
                ),
            });
        }
        let case_key = destination.to_string_lossy().to_ascii_lowercase();
        if let Some((previous_path, previous_title)) = by_casefold.get(&case_key)
            && previous_path != destination
        {
            return Err(PublisherExecutionError::Collision {
                detail: format!(
                    "case-fold destination collision between {previous_title} and {}",
                    item.dat_entry_name
                ),
            });
        }
        by_casefold.insert(case_key, (destination.clone(), item.dat_entry_name.clone()));
    }

    let playing_plan = PlayingLibraryPlan {
        destination_root: plan.destination_root.clone(),
        policy: PlayingLibraryPolicy::default(),
        archives_examined: selected.len(),
        families_examined: selected.len(),
        elected_games: selected
            .iter()
            .map(|item| ElectedGame {
                dat_entry_name: item.dat_entry_name.clone(),
                family_root_name: item.dat_entry_name.clone(),
                explanation: ElectionExplanation {
                    steps: vec![
                        "publisher plan item accepted by the publisher safety filter".to_string(),
                    ],
                    rejected: Vec::new(),
                    winner_evidence: CandidateEvidenceSummary::unknown(),
                },
                launcher_operation: LinkedLibraryOperation {
                    source_path: item.source_path.clone(),
                    destination_path: item.planned_destination.clone().expect("validated above"),
                },
                companion_operations: item
                    .companions
                    .iter()
                    .map(|companion| LinkedLibraryOperation {
                        source_path: companion.source_path.clone(),
                        destination_path: companion.planned_destination.clone(),
                    })
                    .collect(),
            })
            .collect(),
        unresolved_groups: Vec::new(),
        exclusions: Vec::new(),
        singleton_families: selected.len(),
        conflicts: Vec::new(),
        operations: selected
            .iter()
            .flat_map(|item| {
                std::iter::once(LinkedLibraryOperation {
                    source_path: item.source_path.clone(),
                    destination_path: item.planned_destination.clone().expect("validated above"),
                })
                .chain(item.companions.iter().map(|companion| {
                    LinkedLibraryOperation {
                        source_path: companion.source_path.clone(),
                        destination_path: companion.planned_destination.clone(),
                    }
                }))
            })
            .collect(),
        rejected_launchers: Vec::new(),
    };

    let mut transaction = build_playing_library_transaction(&playing_plan, generation)
        .map_err(PublisherExecutionError::TransactionBuild)?;
    let expected_entries = selected
        .iter()
        .map(|item| 1 + item.companions.len())
        .sum::<usize>();
    if transaction.entries.len() != expected_entries {
        return Err(PublisherExecutionError::SourceInvalid {
            title: "publisher batch".to_string(),
            detail:
                "one or more source files changed or disappeared while building the transaction"
                    .to_string(),
        });
    }
    let mut transaction_exact = BTreeMap::new();
    let mut transaction_casefold = BTreeMap::new();
    for entry in &mut transaction.entries {
        let Some(parent) = entry.destination_path.parent() else {
            return Err(PublisherExecutionError::DestinationDirectoryMissing {
                path: entry.destination_path.clone(),
            });
        };
        if !destination_is_confined(&entry.destination_path, &plan.destination_root) {
            return Err(PublisherExecutionError::SourceInvalid {
                title: entry.original_basename.clone(),
                detail: "destination is outside the publisher root".to_string(),
            });
        }
        if link_mode == PublisherLinkMode::Hardlink
            && !same_filesystem(&entry.source_path, parent, &plan.destination_root)
        {
            return Err(PublisherExecutionError::HardlinkUnavailable {
                title: entry.original_basename.clone(),
                detail: "source and destination are on different filesystems; choose explicit SYMLINK mode".to_string(),
            });
        }
        if inspect_destination(&entry.destination_path, &entry.source_path)
            != DestinationState::Missing
            || (parent.exists() && has_casefold_sibling(parent, &entry.destination_path))
        {
            return Err(PublisherExecutionError::DestinationChanged {
                title: entry.original_basename.clone(),
                state: inspect_destination(&entry.destination_path, &entry.source_path),
            });
        }
        if transaction_exact
            .insert(
                entry.destination_path.clone(),
                entry.original_basename.clone(),
            )
            .is_some()
        {
            return Err(PublisherExecutionError::Collision {
                detail: "exact destination collision in transaction batch".to_string(),
            });
        }
        let case_key = entry
            .destination_path
            .to_string_lossy()
            .to_ascii_lowercase();
        if let Some((previous, _)) = transaction_casefold.get(&case_key)
            && previous != &entry.destination_path
        {
            return Err(PublisherExecutionError::Collision {
                detail: "case-fold destination collision in transaction batch".to_string(),
            });
        }
        transaction_casefold.insert(
            case_key,
            (
                entry.destination_path.clone(),
                entry.original_basename.clone(),
            ),
        );
        entry.operation = match link_mode {
            PublisherLinkMode::Hardlink => TransactionOperation::CreateHardlink {
                expected_source: entry.source_path.clone(),
                destination_root: plan.destination_root.clone(),
            },
            // The shared transaction convention uses absolute targets. A
            // relative-link policy would need separate portability evidence.
            PublisherLinkMode::Symlink => TransactionOperation::CreateSymlink {
                expected_target: entry.source_path.clone(),
                destination_root: plan.destination_root.clone(),
            },
        };
    }
    let directories = required_directories(&transaction, &plan.destination_root)?;
    Ok(PublisherTransaction {
        transaction,
        directories,
        link_mode,
        destination_root_identity,
    })
}

/// Applies using the existing journaled directory orchestration and shared
/// executor. No GUI caller is added in Phase 2B.
pub fn apply_publisher_transaction(
    publisher: &mut PublisherTransaction,
    current_generation: u64,
    trusted: crate::safe_read::TrustedRoots,
    journal_dir: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ApplyOutcome, ApplyError> {
    let root = PathBuf::from(&publisher.transaction.source_scan_root);
    let root_now = capture_destination_root_identity(&root).map_err(|error| {
        ApplyError::HardConflicts(vec![(
            root.clone(),
            vec![format!(
                "publisher destination root changed or disappeared: {error}"
            )],
        )])
    })?;
    if publisher.destination_root_identity != root_now {
        return Err(ApplyError::HardConflicts(vec![(
            root,
            vec!["publisher destination root changed since preview".to_string()],
        )]));
    }
    let outcome = crate::platform_evidence_fusion::plan_transaction::apply_plan_transaction(
        &mut publisher.transaction,
        current_generation,
        &root,
        trusted,
        journal_dir,
        cancel,
        false,
    )?;
    for directory in &mut publisher.directories {
        directory.state = if publisher
            .transaction
            .created_directories
            .contains(&directory.path)
        {
            PublisherDirectoryState::CreatedByTransaction
        } else {
            PublisherDirectoryState::PreExisting
        };
    }
    Ok(outcome)
}

fn capture_destination_root_identity(
    path: &Path,
) -> std::io::Result<PublisherDestinationRootIdentity> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "publisher destination root is not a directory",
        ));
    }
    Ok(PublisherDestinationRootIdentity {
        modified: metadata.modified()?,
        #[cfg(unix)]
        ino: std::os::unix::fs::MetadataExt::ino(&metadata),
        #[cfg(unix)]
        dev: std::os::unix::fs::MetadataExt::dev(&metadata),
    })
}

/// Rolls back through the existing shared rollback engine and owned-directory
/// cleanup. Only transaction-created empty directories can be removed.
pub fn rollback_publisher_transaction(
    publisher: &mut PublisherTransaction,
    journal_dir: &Path,
    cancel: &std::sync::atomic::AtomicBool,
    trusted: &crate::safe_read::TrustedRoots,
) -> Result<crate::platform_evidence_fusion::plan_transaction::PlanRollbackOutcome, String> {
    let outcome = crate::platform_evidence_fusion::plan_transaction::rollback_plan_transaction(
        &mut publisher.transaction,
        journal_dir,
        cancel,
        trusted,
    )?;
    for directory in &mut publisher.directories {
        directory.state = if outcome.directories_removed.contains(&directory.path) {
            PublisherDirectoryState::NotCreated
        } else {
            PublisherDirectoryState::PreExisting
        };
    }
    Ok(outcome)
}

fn required_directories(
    transaction: &RenameTransaction,
    root: &Path,
) -> Result<Vec<PublisherDirectory>, PublisherExecutionError> {
    let root_metadata = std::fs::symlink_metadata(root).map_err(|_| {
        PublisherExecutionError::DirectoryConflict {
            path: root.to_path_buf(),
            detail: "publisher destination root must already exist as a directory".to_string(),
        }
    })?;
    if !root_metadata.is_dir() {
        return Err(PublisherExecutionError::DirectoryConflict {
            path: root.to_path_buf(),
            detail: "publisher destination root is not a directory".to_string(),
        });
    }
    let mut paths = std::collections::BTreeSet::new();
    for entry in &transaction.entries {
        let Some(mut parent) = entry.destination_path.parent().map(Path::to_path_buf) else {
            continue;
        };
        let mut chain = Vec::new();
        while parent.starts_with(root) && parent != root {
            chain.push(parent.clone());
            parent = parent
                .parent()
                .ok_or_else(|| PublisherExecutionError::DirectoryConflict {
                    path: entry.destination_path.clone(),
                    detail: "destination directory chain escaped its root".to_string(),
                })?
                .to_path_buf();
        }
        if parent != root {
            return Err(PublisherExecutionError::DirectoryConflict {
                path: entry.destination_path.clone(),
                detail: "destination directory escaped its approved root".to_string(),
            });
        }
        chain.reverse();
        paths.extend(chain);
    }
    paths
        .into_iter()
        .map(|path| {
            let state = match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => PublisherDirectoryState::PreExisting,
                Ok(_) => {
                    return Err(PublisherExecutionError::DirectoryConflict {
                        path,
                        detail: "required destination path is not a directory".to_string(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    PublisherDirectoryState::NotCreated
                }
                Err(error) => {
                    return Err(PublisherExecutionError::DirectoryConflict {
                        path,
                        detail: error.to_string(),
                    });
                }
            };
            Ok(PublisherDirectory { path, state })
        })
        .collect()
}

fn has_casefold_sibling(parent: &Path, destination: &Path) -> bool {
    let Some(destination_name) = destination.file_name() else {
        return true;
    };
    let destination_name = destination_name.to_string_lossy().to_ascii_lowercase();
    std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name())
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .any(|name| name == destination_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_evidence_fusion::romm_platform_mapping::FrontendPlatformMapping;
    use crate::publisher_profile::romm::{resolve_romm_platform_mapping, romm_profile};
    use crate::publisher_profile::{PublisherPlanRequest, build_publisher_plan};

    fn playing_plan(sources: &[PathBuf]) -> PlayingLibraryPlan {
        let operations = sources
            .iter()
            .map(|source| LinkedLibraryOperation {
                source_path: source.clone(),
                destination_path: PathBuf::from("/playing")
                    .join(source.file_name().expect("source filename")),
            })
            .collect::<Vec<_>>();
        PlayingLibraryPlan {
            destination_root: PathBuf::from("/playing"),
            policy: PlayingLibraryPolicy::default(),
            archives_examined: sources.len(),
            families_examined: sources.len(),
            elected_games: sources
                .iter()
                .zip(operations.iter())
                .map(|(source, operation)| ElectedGame {
                    dat_entry_name: source.file_stem().unwrap().to_string_lossy().into_owned(),
                    family_root_name: source.file_stem().unwrap().to_string_lossy().into_owned(),
                    explanation: ElectionExplanation {
                        steps: Vec::new(),
                        rejected: Vec::new(),
                        winner_evidence: CandidateEvidenceSummary::unknown(),
                    },
                    launcher_operation: operation.clone(),
                    companion_operations: Vec::new(),
                })
                .collect(),
            unresolved_groups: Vec::new(),
            exclusions: Vec::new(),
            singleton_families: sources.len(),
            conflicts: Vec::new(),
            operations,
            rejected_launchers: Vec::new(),
        }
    }

    fn publisher_plan(
        sources: &[PathBuf],
        destination_root: &Path,
    ) -> crate::publisher_profile::PublisherPlan {
        let mapping =
            resolve_romm_platform_mapping("Amiga", &FrontendPlatformMapping::default(), None);
        build_publisher_plan(&PublisherPlanRequest {
            profile: &romm_profile(),
            playing_library_plan: &playing_plan(sources),
            platform_mapping: mapping,
            destination_root: destination_root.to_path_buf(),
            existing_destination_root: Some(destination_root),
        })
        .expect("publisher plan")
    }

    fn prepare_destination(root: &Path) {
        std::fs::create_dir_all(root.join("roms/amiga")).unwrap();
    }

    #[test]
    fn safe_items_convert_to_hardlinks_without_creating_anything() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        let transaction = build_publisher_transaction(&plan, 7).unwrap();

        assert_eq!(transaction.plan_generation, 7);
        assert_eq!(transaction.entries.len(), 1);
        assert!(matches!(
            transaction.entries[0].operation,
            TransactionOperation::CreateHardlink { .. }
        ));
        assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
        assert_eq!(
            std::fs::read_dir(destination.join("roms/amiga"))
                .unwrap()
                .count(),
            0
        );
        assert!(transaction.created_directories.is_empty());
    }

    #[test]
    fn shared_executor_applies_hardlink_and_keeps_source_untouched() {
        use crate::dat::rename_apply::executor::{
            ApplyExecution, HardConflictMode, apply_transaction,
        };
        use crate::dat::rename_apply::journal::write_journal;
        use crate::dat::rename_apply::preflight::DirectoryPolicy;
        use crate::safe_read::TrustedRoots;
        use std::sync::atomic::AtomicBool;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        let mut transaction = build_publisher_transaction(&plan, 7).unwrap();
        let journal = temp.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        write_journal(&journal, &transaction).unwrap();
        let cancel = AtomicBool::new(false);
        let outcome = apply_transaction(&mut ApplyExecution {
            transaction: &mut transaction,
            approved_paths: [source.to_string_lossy().into_owned()]
                .into_iter()
                .collect(),
            current_generation: 7,
            trusted: TrustedRoots::from_paths([temp.path()]),
            journal_dir: journal,
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &cancel,
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .unwrap();
        assert_eq!(outcome.summary.applied, 1);
        let destination_path = transaction.entries[0].destination_path.clone();
        assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
        assert_eq!(std::fs::read(&destination_path).unwrap(), b"source bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(&source).unwrap().ino(),
                std::fs::metadata(destination_path).unwrap().ino()
            );
        }
    }

    #[test]
    fn symlink_mode_creates_nested_directories_and_rolls_back_safely() {
        use crate::safe_read::TrustedRoots;
        use std::sync::atomic::AtomicBool;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        std::fs::create_dir(&destination).unwrap();
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        let mut publisher =
            build_publisher_transaction_with_policy(&plan, 11, PublisherLinkMode::Symlink).unwrap();
        assert_eq!(publisher.directories.len(), 2);
        assert!(
            publisher
                .directories
                .iter()
                .all(|directory| directory.state == PublisherDirectoryState::NotCreated)
        );

        let journal = temp.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        let cancel = AtomicBool::new(false);
        apply_publisher_transaction(
            &mut publisher,
            11,
            TrustedRoots::from_paths([temp.path()]),
            &journal,
            &cancel,
        )
        .unwrap();
        let destination_path = publisher.transaction.entries[0].destination_path.clone();
        assert!(destination_path.is_symlink());
        assert_eq!(std::fs::read_link(&destination_path).unwrap(), source);
        assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
        assert!(
            publisher
                .directories
                .iter()
                .all(|directory| directory.state == PublisherDirectoryState::CreatedByTransaction)
        );

        rollback_publisher_transaction(
            &mut publisher,
            &journal,
            &cancel,
            &TrustedRoots::from_paths([temp.path()]),
        )
        .unwrap();
        assert!(source.exists());
        assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
        assert!(!destination_path.exists());
        assert!(
            publisher
                .directories
                .iter()
                .all(|directory| directory.state == PublisherDirectoryState::NotCreated)
        );
        assert!(destination.exists());
        assert!(!destination.join("roms").exists());
    }

    #[test]
    fn symlink_mode_preserves_pre_existing_directories() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        std::fs::create_dir_all(destination.join("roms")).unwrap();
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        let publisher =
            build_publisher_transaction_with_policy(&plan, 1, PublisherLinkMode::Symlink).unwrap();
        assert_eq!(
            publisher.directories[0].state,
            PublisherDirectoryState::PreExisting
        );
        assert_eq!(
            publisher.directories[1].state,
            PublisherDirectoryState::NotCreated
        );
    }

    #[test]
    fn symlink_wrong_target_broken_link_and_file_are_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        let other = temp.path().join("Other.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        std::fs::write(&other, b"other bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);

        for (kind, setup) in [("wrong", 0u8), ("broken", 1u8), ("file", 2u8)] {
            let plan = publisher_plan(std::slice::from_ref(&source), &destination);
            let planned = plan.items[0].planned_destination.clone().unwrap();
            match setup {
                0 => std::os::unix::fs::symlink(&other, &planned).unwrap(),
                1 => std::os::unix::fs::symlink(temp.path().join("gone"), &planned).unwrap(),
                _ => std::fs::write(&planned, b"occupied").unwrap(),
            }
            let error =
                build_publisher_transaction_with_policy(&plan, 1, PublisherLinkMode::Symlink)
                    .expect_err(kind);
            assert!(matches!(
                error,
                PublisherExecutionError::DestinationChanged { .. }
            ));
            std::fs::remove_file(&planned).unwrap();
        }
    }

    #[test]
    fn symlink_mode_blocks_when_source_disappears_after_preview() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        std::fs::remove_file(&source).unwrap();
        assert!(matches!(
            build_publisher_transaction_with_policy(&plan, 1, PublisherLinkMode::Symlink),
            Err(PublisherExecutionError::SourceInvalid { .. })
        ));
    }

    #[test]
    fn symlink_mode_blocks_when_destination_root_changes_after_preview() {
        use crate::safe_read::TrustedRoots;
        use std::sync::atomic::AtomicBool;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        let mut publisher =
            build_publisher_transaction_with_policy(&plan, 1, PublisherLinkMode::Symlink).unwrap();
        std::fs::remove_dir_all(&destination).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let journal = temp.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        let error = apply_publisher_transaction(
            &mut publisher,
            1,
            TrustedRoots::from_paths([temp.path()]),
            &journal,
            &AtomicBool::new(false),
        )
        .expect_err("changed destination root must block apply");
        assert!(matches!(error, ApplyError::HardConflicts(_)));
        assert!(!destination.join("roms").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_mode_supports_a_cross_filesystem_destination_when_available() {
        use crate::safe_read::TrustedRoots;
        use std::sync::atomic::AtomicBool;

        let Ok(destination_temp) = tempfile::tempdir_in("/dev/shm") else {
            return;
        };
        let source_temp = tempfile::tempdir().unwrap();
        let source = source_temp.path().join("Game.rom");
        std::fs::write(&source, b"cross filesystem bytes").unwrap();
        let destination = destination_temp.path().join("published");
        std::fs::create_dir(&destination).unwrap();
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        assert!(matches!(
            build_publisher_transaction_with_policy(&plan, 1, PublisherLinkMode::Hardlink),
            Err(PublisherExecutionError::HardlinkUnavailable { .. })
        ));
        let mut publisher =
            build_publisher_transaction_with_policy(&plan, 1, PublisherLinkMode::Symlink).unwrap();
        let journal = source_temp.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        apply_publisher_transaction(
            &mut publisher,
            1,
            TrustedRoots::from_paths([source_temp.path(), &destination]),
            &journal,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            publisher.transaction.entries[0]
                .destination_path
                .is_symlink()
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"cross filesystem bytes");
    }

    #[test]
    fn review_or_unsupported_items_never_enter_a_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let mapping = crate::publisher_profile::es_de::resolve_es_de_platform_mapping(
            "not a reviewed platform",
        );
        let plan = build_publisher_plan(&PublisherPlanRequest {
            profile: &crate::publisher_profile::es_de::es_de_profile(),
            playing_library_plan: &playing_plan(std::slice::from_ref(&source)),
            platform_mapping: mapping,
            destination_root: destination.clone(),
            existing_destination_root: Some(&destination),
        })
        .unwrap();
        assert!(matches!(
            build_publisher_transaction(&plan, 1),
            Err(PublisherExecutionError::NoEligibleItems)
        ));
    }

    #[test]
    fn exact_destination_change_is_rejected_before_transaction_build() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        let planned = plan.items[0].planned_destination.clone().unwrap();
        std::fs::write(planned, b"unexpected").unwrap();
        assert!(matches!(
            build_publisher_transaction(&plan, 1),
            Err(PublisherExecutionError::DestinationChanged { .. })
        ));
    }

    #[test]
    fn casefold_destination_change_is_rejected_before_transaction_build() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        let planned = plan.items[0].planned_destination.clone().unwrap();
        let sibling = planned.parent().unwrap().join("GAME.ROM");
        if sibling != planned {
            std::fs::write(sibling, b"unexpected").unwrap();
            assert!(matches!(
                build_publisher_transaction(&plan, 1),
                Err(PublisherExecutionError::Collision { .. })
            ));
        }
    }

    #[test]
    #[cfg(unix)]
    fn stale_destination_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        let planned = plan.items[0].planned_destination.clone().unwrap();
        std::os::unix::fs::symlink(temp.path().join("gone"), planned).unwrap();
        assert!(matches!(
            build_publisher_transaction(&plan, 1),
            Err(PublisherExecutionError::DestinationChanged {
                state: DestinationState::Stale,
                ..
            })
        ));
    }

    #[test]
    fn tampered_preview_is_stale() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let mut plan = publisher_plan(std::slice::from_ref(&source), &destination);
        plan.items[0].dat_entry_name.push_str(" changed");
        assert!(matches!(
            build_publisher_transaction(&plan, 1),
            Err(PublisherExecutionError::StalePlan { .. })
        ));
    }

    #[test]
    fn transaction_entries_are_sorted_deterministically() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("B.rom");
        let second = temp.path().join("A.rom");
        std::fs::write(&first, b"b").unwrap();
        std::fs::write(&second, b"a").unwrap();
        let destination = temp.path().join("published");
        prepare_destination(&destination);
        let plan = publisher_plan(&[first, second], &destination);
        let transaction = build_publisher_transaction(&plan, 1).unwrap();
        let paths = transaction
            .entries
            .iter()
            .map(|entry| entry.destination_path.clone())
            .collect::<Vec<_>>();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
    }

    #[test]
    fn missing_destination_directory_is_explicitly_unsupported_without_mkdir() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.rom");
        std::fs::write(&source, b"source bytes").unwrap();
        let destination = temp.path().join("published");
        let plan = publisher_plan(std::slice::from_ref(&source), &destination);
        assert!(matches!(
            build_publisher_transaction(&plan, 1),
            Err(PublisherExecutionError::DestinationDirectoryMissing { .. })
        ));
        assert!(!destination.exists());
    }
}

fn same_filesystem(source: &Path, destination_parent: &Path, fallback_root: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(source_metadata) = std::fs::metadata(source) else {
            return false;
        };
        let destination_metadata =
            std::fs::metadata(destination_parent).or_else(|_| std::fs::metadata(fallback_root));
        let Ok(destination_metadata) = destination_metadata else {
            return false;
        };
        source_metadata.dev() == destination_metadata.dev()
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination_parent, fallback_root);
        false
    }
}
