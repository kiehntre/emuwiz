//! Phase 2A PublisherPlan-to-transaction adapter.
//!
//! This is a foundation only. It reuses the existing Playing Library
//! transaction builder and shared rename executor model; it does not write a
//! journal or execute anything. Publisher items are deliberately converted to
//! hardlink operations only when the destination is already available and
//! same-filesystem evidence is present. There is no copy or symlink fallback.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::dat::rename_apply::{
    RenameTransaction, TransactionOperation, capture_identity, destination_is_confined,
};
use crate::playing_library::{
    CandidateEvidenceSummary, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
    PlayingLibraryPlan, PlayingLibraryPolicy, build_playing_library_transaction,
};

use super::destination_inspection::inspect_destination;
use super::model::{DestinationState, PublisherActionKind, PublisherActionSafety, PublisherPlan};
use super::planner::publisher_plan_hash_matches;

/// A typed refusal from the Phase 2A adapter. Every variant is fail-closed:
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
}

/// Builds an unwritten, unapplied shared transaction from the safe subset of
/// a PublisherPlan. Already-present and non-safe items are excluded. The
/// result uses `TransactionOperation::CreateHardlink`, so the eventual shared
/// executor acts only on destination-side paths and leaves sources in place.
///
/// The adapter performs a fresh plan-hash, source, destination, case-fold, and
/// same-filesystem check immediately before constructing the transaction.
/// Missing destination directories are unsupported in Phase 2A: the adapter
/// never performs implicit `mkdir` and never populates `created_directories`.
pub fn build_publisher_transaction(
    plan: &PublisherPlan,
    generation: u64,
) -> Result<RenameTransaction, PublisherExecutionError> {
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
        let Ok(parent_metadata) = std::fs::metadata(parent) else {
            return Err(PublisherExecutionError::DestinationDirectoryMissing {
                path: parent.to_path_buf(),
            });
        };
        if !parent_metadata.is_dir() {
            return Err(PublisherExecutionError::DestinationDirectoryMissing {
                path: parent.to_path_buf(),
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
        if !same_filesystem(&item.source_path, parent) {
            return Err(PublisherExecutionError::UnsupportedAction {
                title: item.dat_entry_name.clone(),
                action: PublisherActionKind::Hardlink,
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
        if has_casefold_sibling(parent, destination) {
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
                        "publisher plan item accepted by the Phase 2A safety filter".to_string(),
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
        if !destination_is_confined(&entry.destination_path, &plan.destination_root)
            || !same_filesystem(&entry.source_path, parent)
        {
            return Err(PublisherExecutionError::UnsupportedAction {
                title: entry.original_basename.clone(),
                action: PublisherActionKind::Hardlink,
            });
        }
        if inspect_destination(&entry.destination_path, &entry.source_path)
            != DestinationState::Missing
            || has_casefold_sibling(parent, &entry.destination_path)
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
        entry.operation = TransactionOperation::CreateHardlink {
            expected_source: entry.source_path.clone(),
            destination_root: plan.destination_root.clone(),
        };
    }
    Ok(transaction)
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

fn same_filesystem(source: &Path, destination_parent: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(source_metadata) = std::fs::metadata(source) else {
            return false;
        };
        let Ok(destination_metadata) = std::fs::metadata(destination_parent) else {
            return false;
        };
        source_metadata.dev() == destination_metadata.dev()
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination_parent);
        false
    }
}
