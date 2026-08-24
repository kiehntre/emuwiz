//! Focused tests for the BuildLinkedLibrary organisation mode: planning,
//! CreateSymlink transaction conversion, apply, destination states, directory
//! ownership, rollback semantics and source-untouched guarantees.
//!
//! Every mutation test uses temporary directories only. The source tree and
//! the linked-library root are deliberately separate directories.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::model::{OrganisationMode, OrganisationStatus};
use super::*;
use crate::dat::rename_apply::model::{ObjectKind, RenameTransaction, TransactionOperation};
use crate::platform::identity::{
    PlatformIdentityConfidence, PlatformIdentityEvidence, PlatformIdentityResolution,
    PlatformIdentitySource,
};
use crate::safe_read::TrustedRoots;

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

fn resolved(platform: &str) -> PlatformIdentityResolution {
    PlatformIdentityResolution::Resolved {
        generation: 1,
        platform: platform.to_string(),
        display_name: crate::platform::display_name_for(platform).to_string(),
        confidence: PlatformIdentityConfidence::High,
        evidence: vec![PlatformIdentityEvidence {
            platform: platform.to_string(),
            source: PlatformIdentitySource::VerifiedDat,
            confidence: PlatformIdentityConfidence::High,
            generation: 1,
            detail: "test evidence".to_string(),
        }],
    }
}

fn write_source(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn candidate(source_path: PathBuf, canonical_name: Option<String>) -> OrganisationCandidate {
    OrganisationCandidate {
        source_path,
        resolution: resolved("Atari2600"),
        canonical_name,
        content_classification: None,
        original_metadata: Default::default(),
    }
}

fn plan_for(library_root: &Path, candidates: &[OrganisationCandidate]) -> OrganisationPlan {
    build_organisation_plan(&OrganisationPlanRequest {
        master_root: library_root,
        mode: OrganisationMode::BuildLinkedLibrary,
        content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
        candidates,
        generation: 1,
    })
}

fn build_tx(plan: &OrganisationPlan, approved: &BTreeSet<String>) -> RenameTransaction {
    build_organisation_transaction(plan, approved, plan.generation).expect("build transaction")
}

fn apply_plan(
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
    journal_dir: &Path,
) -> crate::dat::rename_apply::executor::ApplyOutcome {
    std::fs::create_dir_all(&plan.master_root).unwrap();
    let mut tx = build_tx(plan, approved);
    let mut trusted_roots =
        vec![std::fs::canonicalize(&plan.master_root).unwrap_or_else(|_| plan.master_root.clone())];
    for entry in plan.suggested() {
        if let Some(parent) = entry.source_path.parent()
            && let Ok(canonical) = std::fs::canonicalize(parent)
        {
            trusted_roots.push(canonical);
        }
    }
    apply_organisation_transaction(
        &mut tx,
        approved,
        plan.generation,
        TrustedRoots::from_paths(trusted_roots),
        journal_dir,
        &no_cancel(),
        plan.mode,
        &plan.master_root,
    )
    .expect("apply")
}

fn approved_of(sources: &[&Path]) -> BTreeSet<String> {
    sources
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

struct Fixture {
    _root: tempfile::TempDir,
    source_tree: PathBuf,
    library_root: PathBuf,
    journal_dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join(name);
        std::fs::create_dir_all(&base).unwrap();
        let fixture = Self {
            _root: root,
            source_tree: base.join("sources"),
            library_root: base.join("emuwiz-library"),
            journal_dir: base.join("journals"),
        };
        std::fs::create_dir_all(&fixture.journal_dir).unwrap();
        fixture
    }
}

// ---------------------------------------------------------------------------
// Mode identity and transaction conversion
// ---------------------------------------------------------------------------

#[test]
fn build_linked_library_is_distinct_from_the_other_modes() {
    assert_ne!(
        OrganisationMode::BuildLinkedLibrary,
        OrganisationMode::RenameInPlace
    );
    assert_ne!(
        OrganisationMode::BuildLinkedLibrary,
        OrganisationMode::MoveRealFile
    );
    assert_ne!(
        OrganisationMode::BuildLinkedLibrary,
        OrganisationMode::OrganiseSymlinkOnly
    );
    let json = serde_json::to_string(&OrganisationMode::BuildLinkedLibrary).unwrap();
    assert_eq!(json, "\"build_linked_library\"");
    let round: OrganisationMode = serde_json::from_str(&json).unwrap();
    assert_eq!(round, OrganisationMode::BuildLinkedLibrary);
}

#[test]
fn linked_library_declares_sources_untouched_and_the_others_do_not() {
    assert!(OrganisationMode::BuildLinkedLibrary.leaves_sources_untouched());
    assert!(!OrganisationMode::RenameInPlace.leaves_sources_untouched());
    assert!(!OrganisationMode::MoveRealFile.leaves_sources_untouched());
}

#[test]
fn a_regular_source_plans_a_canonical_destination_under_the_library_root() {
    let fx = Fixture::new("plan");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    let plan = plan_for(
        &fx.library_root,
        &[candidate(source, Some("Combat (USA).bin".to_string()))],
    );

    let entry = &plan.entries[0];
    assert_eq!(entry.status, OrganisationStatus::Suggested);
    assert_eq!(entry.mode, OrganisationMode::BuildLinkedLibrary);
    assert_eq!(
        entry.destination_path,
        fx.library_root.join("Atari 2600").join("Combat (USA).bin")
    );
}

#[test]
fn a_symlink_source_is_blocked_in_linked_library_mode() {
    let fx = Fixture::new("symlink-source");
    let real = write_source(&fx.source_tree, "real.bin", b"data");
    let link = fx.source_tree.join("linked.bin");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let plan = plan_for(&fx.library_root, &[candidate(link, None)]);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
}

#[test]
fn a_directory_source_is_blocked_in_linked_library_mode() {
    let fx = Fixture::new("dir-source");
    let dir = fx.source_tree.join("game-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let plan = plan_for(&fx.library_root, &[candidate(dir, None)]);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
}

#[test]
fn a_regular_source_becomes_a_create_symlink_transaction_with_exact_target_and_root() {
    let fx = Fixture::new("tx");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    let approved = approved_of(&[&source]);
    let tx = build_tx(&plan, &approved);

    assert_eq!(tx.entries.len(), 1);
    let entry = &tx.entries[0];
    match &entry.operation {
        TransactionOperation::CreateSymlink {
            expected_target,
            destination_root,
        } => {
            assert_eq!(
                expected_target.as_path(),
                source.as_path(),
                "target must be the exact source"
            );
            assert!(expected_target.is_absolute());
            assert!(
                destination_root.is_absolute(),
                "the approved root must be absolute"
            );
            assert_eq!(destination_root.as_path(), fx.library_root.as_path());
        }
        other => panic!("expected CreateSymlink, got {other:?}"),
    }
    assert_eq!(entry.identity.kind, ObjectKind::RegularFile);
}

#[test]
fn existing_move_mode_still_emits_rename_move_operations() {
    let fx = Fixture::new("move-mode");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    let candidate = candidate(source.clone(), Some("Combat (USA).bin".to_string()));
    let plan = build_organisation_plan(&OrganisationPlanRequest {
        master_root: &fx.library_root,
        mode: OrganisationMode::MoveRealFile,
        content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
        candidates: std::slice::from_ref(&candidate),
        generation: 1,
    });
    let approved = approved_of(&[&source]);
    let tx = build_tx(&plan, &approved);
    assert!(matches!(
        tx.entries[0].operation,
        TransactionOperation::RenameMove
    ));
}

// ---------------------------------------------------------------------------
// Apply, destination states and source-untouched guarantees
// ---------------------------------------------------------------------------

#[test]
fn apply_creates_the_link_and_leaves_the_source_untouched() {
    let fx = Fixture::new("apply");
    let contents = b"original atari rom bytes".to_vec();
    let source = write_source(&fx.source_tree, "Combat.bin", &contents);
    let before = std::fs::symlink_metadata(&source).unwrap();
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    let approved = approved_of(&[&source]);
    let outcome = apply_plan(&plan, &approved, &fx.journal_dir);

    let destination = fx.library_root.join("Atari 2600").join("Combat (USA).bin");
    let metadata = std::fs::symlink_metadata(&destination).unwrap();
    assert!(metadata.file_type().is_symlink());
    assert_eq!(std::fs::read_link(&destination).unwrap(), source);

    // The source is byte-identical, still a regular file, still not a link.
    assert_eq!(std::fs::read(&source).unwrap(), contents);
    let after = std::fs::symlink_metadata(&source).unwrap();
    assert!(after.is_file() && !after.file_type().is_symlink());
    assert_eq!(before.len(), after.len());

    assert_eq!(outcome.transaction.applied_count(), 1);
}

#[test]
fn an_exact_existing_link_is_already_present_and_excluded_from_the_transaction() {
    let fx = Fixture::new("noop");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    // Pre-create the exact link the mode would create.
    std::fs::create_dir_all(fx.library_root.join("Atari 2600")).unwrap();
    let destination = fx.library_root.join("Atari 2600").join("Combat (USA).bin");
    std::os::unix::fs::symlink(&source, &destination).unwrap();

    let plan = plan_for(
        &fx.library_root,
        &[candidate(source, Some("Combat (USA).bin".to_string()))],
    );
    assert_eq!(plan.entries[0].status, OrganisationStatus::AlreadyOrganised);

    // AlreadyOrganised entries are never Suggested, so the transaction has
    // nothing to do: a true no-op.
    assert!(plan.suggested().next().is_none());
    // The pre-existing link was not touched.
    assert_eq!(
        std::fs::read_link(fx.library_root.join("Atari 2600").join("Combat (USA).bin")).unwrap(),
        fx.source_tree.join("Combat.bin")
    );
}

#[test]
fn a_wrong_target_link_regular_file_or_directory_at_the_destination_conflicts() {
    let fx = Fixture::new("conflict");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    write_source(&fx.source_tree, "other.bin", b"other");
    std::fs::create_dir_all(fx.library_root.join("Atari 2600")).unwrap();
    let destination = fx.library_root.join("Atari 2600").join("Combat (USA).bin");

    // Wrong-target link.
    std::os::unix::fs::symlink(fx.source_tree.join("other.bin"), &destination).unwrap();
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    assert_eq!(plan.entries[0].status, OrganisationStatus::Conflict);
    std::fs::remove_file(&destination).unwrap();

    // Regular file occupying the name.
    std::fs::write(&destination, b"occupied").unwrap();
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    assert_eq!(plan.entries[0].status, OrganisationStatus::Conflict);
    std::fs::remove_file(&destination).unwrap();

    // Directory occupying the name.
    std::fs::create_dir_all(&destination).unwrap();
    let plan = plan_for(
        &fx.library_root,
        &[candidate(source, Some("Combat (USA).bin".to_string()))],
    );
    assert_eq!(
        plan.entries[0].status,
        OrganisationStatus::Conflict,
        "nothing is ever auto-replaced"
    );
}

#[test]
fn a_changed_source_after_preview_blocks_apply() {
    let fx = Fixture::new("changed-source");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    let approved = approved_of(&[&source]);
    let mut tx = build_tx(&plan, &approved);
    // The file changes between preview/build and apply.
    std::fs::write(&source, b"different bytes now").unwrap();

    std::fs::create_dir_all(&plan.master_root).unwrap();
    let trusted = TrustedRoots::from_paths([
        std::fs::canonicalize(&plan.master_root).unwrap(),
        std::fs::canonicalize(source.parent().unwrap()).unwrap(),
    ]);
    let error = apply_organisation_transaction(
        &mut tx,
        &approved,
        plan.generation,
        trusted,
        &fx.journal_dir,
        &no_cancel(),
        plan.mode,
        &plan.master_root,
    )
    .expect_err("a changed source must block the apply");
    assert!(matches!(
        error,
        crate::dat::rename_apply::executor::ApplyError::HardConflicts(_)
    ));
    // Nothing was created.
    assert!(
        !fx.library_root
            .join("Atari 2600")
            .join("Combat (USA).bin")
            .exists()
    );
}

#[test]
fn a_destination_outside_the_approved_root_blocks_apply() {
    let fx = Fixture::new("outside-root");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    let approved = approved_of(&[&source]);
    let mut tx = build_tx(&plan, &approved);
    // Simulate corrupted intent: the destination escapes the recorded
    // approved root while still claiming it as its authority.
    tx.entries[0].destination_path = fx.library_root.parent().unwrap().join("escaped.bin");

    std::fs::create_dir_all(&plan.master_root).unwrap();
    let trusted = TrustedRoots::from_paths([
        std::fs::canonicalize(&plan.master_root).unwrap(),
        std::fs::canonicalize(source.parent().unwrap()).unwrap(),
    ]);
    let error = apply_organisation_transaction(
        &mut tx,
        &approved,
        plan.generation,
        trusted,
        &fx.journal_dir,
        &no_cancel(),
        plan.mode,
        &plan.master_root,
    )
    .expect_err("an escaping destination must be refused");
    assert!(matches!(
        error,
        crate::dat::rename_apply::executor::ApplyError::HardConflicts(_)
    ));
}

#[test]
fn an_unsafe_destination_ancestry_blocks_apply() {
    let fx = Fixture::new("unsafe-ancestry");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    let approved = approved_of(&[&source]);
    let mut tx = build_tx(&plan, &approved);
    std::fs::create_dir_all(&plan.master_root).unwrap();
    // Replace the would-be platform directory with a symlink to elsewhere.
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), fx.library_root.join("Atari 2600")).unwrap();

    let trusted = TrustedRoots::from_paths([
        std::fs::canonicalize(&plan.master_root).unwrap(),
        std::fs::canonicalize(source.parent().unwrap()).unwrap(),
    ]);
    let error = apply_organisation_transaction(
        &mut tx,
        &approved,
        plan.generation,
        trusted,
        &fx.journal_dir,
        &no_cancel(),
        plan.mode,
        &plan.master_root,
    )
    .expect_err("a symlinked ancestor must refuse link creation");
    assert!(matches!(
        error,
        crate::dat::rename_apply::executor::ApplyError::HardConflicts(_)
    ));
}

// ---------------------------------------------------------------------------
// Directory ownership and rollback
// ---------------------------------------------------------------------------

#[test]
fn safe_platform_directories_are_created_and_owned() {
    let fx = Fixture::new("dirs");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    let approved = approved_of(&[&source]);
    let outcome = apply_plan(&plan, &approved, &fx.journal_dir);

    // The platform directory was created by this transaction and recorded as
    // owned.
    assert!(fx.library_root.join("Atari 2600").is_dir());
    assert_eq!(outcome.transaction.created_directories.len(), 1);
}

#[test]
fn rollback_removes_the_created_link_but_never_touches_the_source() {
    let fx = Fixture::new("rollback");
    let contents = b"atari combat rom".to_vec();
    let source = write_source(&fx.source_tree, "Combat.bin", &contents);
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    let approved = approved_of(&[&source]);
    apply_plan(&plan, &approved, &fx.journal_dir);
    let destination = fx.library_root.join("Atari 2600").join("Combat (USA).bin");
    assert!(std::fs::symlink_metadata(&destination).is_ok());

    // Recover the journalled transaction (as History & Rollback would) and
    // roll it back.
    let (mut transactions, problems) =
        crate::dat::rename_apply::journal::list_journals(&fx.journal_dir);
    assert!(problems.is_empty(), "{problems:?}");
    let mut transaction = transactions.remove(0);
    let result = rollback_organisation_transaction(
        &mut transaction,
        &fx.journal_dir,
        &no_cancel(),
        &plan.master_root,
    )
    .unwrap();

    // The created link is gone; the source is byte-identical and regular.
    assert!(std::fs::symlink_metadata(&destination).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), contents);
    let metadata = std::fs::symlink_metadata(&source).unwrap();
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert!(matches!(
        result.rollback.result,
        crate::dat::rename_apply::model::RollbackResult::FullyRolledBack
    ));
}

#[test]
fn rollback_preserves_pre_existing_directories_and_exact_links() {
    let fx = Fixture::new("preserve");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    // A PRE-EXISTING platform directory and a PRE-EXISTING exact link for a
    // second game that this apply never touches.
    std::fs::create_dir_all(fx.library_root.join("Atari 2600")).unwrap();
    let other = write_source(&fx.source_tree, "Pitfall.bin", b"pitfall");
    let other_link = fx.library_root.join("Atari 2600").join("Pitfall.bin");
    std::os::unix::fs::symlink(&other, &other_link).unwrap();

    let candidates = vec![
        candidate(source.clone(), Some("Combat (USA).bin".to_string())),
        candidate(other.clone(), Some("Pitfall.bin".to_string())),
    ];
    let plan = plan_for(&fx.library_root, &candidates);
    // The second entry is AlreadyOrganised (exact link exists): no-op.
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    assert_eq!(plan.entries[1].status, OrganisationStatus::AlreadyOrganised);
    let approved = approved_of(&[&source]);
    apply_plan(&plan, &approved, &fx.journal_dir);

    let created_link = fx.library_root.join("Atari 2600").join("Combat (USA).bin");
    assert!(std::fs::symlink_metadata(&created_link).is_ok());

    let (mut transactions, _) = crate::dat::rename_apply::journal::list_journals(&fx.journal_dir);
    let mut transaction = transactions.remove(0);
    let result = rollback_organisation_transaction(
        &mut transaction,
        &fx.journal_dir,
        &no_cancel(),
        &plan.master_root,
    )
    .unwrap();

    // The transaction-created link is removed...
    assert!(std::fs::symlink_metadata(&created_link).is_err());
    // ...the pre-existing exact link survives untouched...
    assert_eq!(std::fs::read_link(&other_link).unwrap(), other);
    // ...and the pre-existing directory is never removed.
    assert!(fx.library_root.join("Atari 2600").is_dir());
    assert_eq!(std::fs::read(&source).unwrap(), b"combat");
    assert_eq!(std::fs::read(&other).unwrap(), b"pitfall");
    let _ = result;
}

#[test]
fn rollback_refuses_a_changed_target_link() {
    let fx = Fixture::new("changed-link");
    let source = write_source(&fx.source_tree, "Combat.bin", b"combat");
    write_source(&fx.source_tree, "other.bin", b"other");
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    let approved = approved_of(&[&source]);
    apply_plan(&plan, &approved, &fx.journal_dir);
    let destination = fx.library_root.join("Atari 2600").join("Combat (USA).bin");

    // Someone repoints the link after the transaction created it.
    std::fs::remove_file(&destination).unwrap();
    std::os::unix::fs::symlink(fx.source_tree.join("other.bin"), &destination).unwrap();

    let (mut transactions, _) = crate::dat::rename_apply::journal::list_journals(&fx.journal_dir);
    let mut transaction = transactions.remove(0);
    let result = rollback_organisation_transaction(
        &mut transaction,
        &fx.journal_dir,
        &no_cancel(),
        &plan.master_root,
    )
    .unwrap();

    // The changed link is refused, not removed and not followed.
    assert!(std::fs::symlink_metadata(&destination).is_ok());
    assert_eq!(
        std::fs::read_link(&destination).unwrap(),
        fx.source_tree.join("other.bin")
    );
    assert!(matches!(
        result.rollback.result,
        crate::dat::rename_apply::model::RollbackResult::RollbackFailed { .. }
            | crate::dat::rename_apply::model::RollbackResult::PartiallyRolledBack { .. }
    ));
}

// ---------------------------------------------------------------------------
// End-to-end: real source tree -> separate linked-library root
// ---------------------------------------------------------------------------

#[test]
fn atari_2600_source_is_linked_into_a_separate_library_root_end_to_end() {
    // SOURCE: a regular Atari 2600 game file in one source tree.
    let fx = Fixture::new("atari-e2e");
    let contents = b"combat for the atari 2600 - original bytes".to_vec();
    let source = write_source(&fx.source_tree.join("Atari 2600"), "Combat.bin", &contents);
    let source_metadata_before = std::fs::symlink_metadata(&source).unwrap();

    // DESTINATION ROOT: a completely separate linked-library root directory.
    std::fs::create_dir_all(&fx.library_root).unwrap();

    // PLAN: the same canonical organisation planning rules as every mode.
    let plan = plan_for(
        &fx.library_root,
        &[candidate(
            source.clone(),
            Some("Combat (USA).bin".to_string()),
        )],
    );
    let entry = plan.suggested().next().expect("a Suggested entry");
    let destination = entry.destination_path.clone();

    // APPLY: through the journalled CreateSymlink transaction layer.
    let approved = approved_of(&[&source]);
    let outcome = apply_plan(&plan, &approved, &fx.journal_dir);
    assert_eq!(outcome.transaction.applied_count(), 1);

    // VERIFY: destination is a symlink to the exact absolute source.
    let link_metadata = std::fs::symlink_metadata(&destination).unwrap();
    assert!(link_metadata.file_type().is_symlink());
    assert_eq!(std::fs::read_link(&destination).unwrap(), source);
    // Reading through the link yields the original bytes.
    assert_eq!(std::fs::read(&destination).unwrap(), contents);

    // VERIFY: the source still exists, is byte-identical, regular, not a link.
    assert_eq!(std::fs::read(&source).unwrap(), contents);
    let source_metadata_after = std::fs::symlink_metadata(&source).unwrap();
    assert!(source_metadata_after.is_file());
    assert!(!source_metadata_after.file_type().is_symlink());
    assert_eq!(source_metadata_before.len(), source_metadata_after.len());

    // ROLLBACK: removes only the exact unchanged transaction-created link.
    let (mut transactions, _) = crate::dat::rename_apply::journal::list_journals(&fx.journal_dir);
    let mut transaction = transactions.remove(0);
    let result = rollback_organisation_transaction(
        &mut transaction,
        &fx.journal_dir,
        &no_cancel(),
        &plan.master_root,
    )
    .unwrap();
    assert!(matches!(
        result.rollback.result,
        crate::dat::rename_apply::model::RollbackResult::FullyRolledBack
    ));
    assert!(std::fs::symlink_metadata(&destination).is_err());

    // The source is STILL unchanged after rollback.
    assert_eq!(std::fs::read(&source).unwrap(), contents);
    let final_metadata = std::fs::symlink_metadata(&source).unwrap();
    assert!(final_metadata.is_file() && !final_metadata.file_type().is_symlink());
}
