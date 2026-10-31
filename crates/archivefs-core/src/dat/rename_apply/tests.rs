//! Integration tests for the gated rename transaction executor, journal,
//! crash recovery and rollback - including hostile filesystem changes between
//! review and apply, no-clobber proofs, and content-integrity proofs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::dat::classification::{
    ContentSelectionPolicy, DatContentClassification, DatOriginalMetadata,
};
use crate::dat::rename_plan::{
    ProposalState, RenamePlan, RenamePlanCounts, RenameProposal, SourceObjectKind,
};
use crate::safe_read::TrustedRoots;

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

fn cancelled() -> AtomicBool {
    AtomicBool::new(true)
}

fn write(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"fixture contents").unwrap();
    path
}

fn proposal(source: &str, current: &str, proposed: &str, state: ProposalState) -> RenameProposal {
    RenameProposal {
        source_path: PathBuf::from(source),
        current_basename: current.to_string(),
        proposed_basename: Some(proposed.to_string()),
        platform: None,
        platform_display: None,
        source_id: "src".to_string(),
        source_display_name: "Source".to_string(),
        game_name: Some("Game".to_string()),
        rom_name: Some(proposed.to_string()),
        verdict_label: "Exact".to_string(),
        match_confident: true,
        explanations: Vec::new(),
        content_policy: ContentSelectionPolicy::AllEntries,
        content_classification: DatContentClassification::unknown(),
        original_metadata: DatOriginalMetadata::default(),
        state,
        object_kind: SourceObjectKind::RegularFile,
        ambiguity_reason: None,
        collision: None,
        blockers: Vec::new(),
        extension_status: None,
        sanitisation_notes: Vec::new(),
        actionable: state == ProposalState::Suggested,
        audited_identity: None,
        is_outer_archive: false,
    }
}

fn plan(proposals: Vec<RenameProposal>, generation: u64, scan_root: &Path) -> RenamePlan {
    let counts = RenamePlanCounts::from_proposals(&proposals);
    RenamePlan {
        generation,
        source_id: "src".to_string(),
        source_display_name: "Source".to_string(),
        scan_root: scan_root.to_string_lossy().into_owned(),
        platform: None,
        platform_display: None,
        content_policy: ContentSelectionPolicy::AllEntries,
        classifier_version: crate::dat::classification::CLASSIFIER_VERSION.to_string(),
        proposals,
        counts,
        audited_total: counts.total,
        verified_total: counts.total,
        truncated: false,
    }
}

/// Builds and applies a transaction in one call (review identity captured
/// at the same moment as apply, so only non-hostile flows use this).
fn apply(
    plan: &RenamePlan,
    approved: BTreeSet<String>,
    trusted: TrustedRoots,
    journal_dir: &Path,
    mode: HardConflictMode,
    cancel: &AtomicBool,
) -> Result<ApplyOutcome, ApplyError> {
    let tx = build_transaction(plan, &approved, plan.generation)?;
    apply_exec(
        tx,
        approved,
        trusted,
        journal_dir,
        mode,
        cancel,
        plan.generation,
    )
}

/// Applies an already-built transaction (used when the test mutates files
/// between review-time build and apply).
fn apply_exec(
    mut tx: RenameTransaction,
    approved: BTreeSet<String>,
    trusted: TrustedRoots,
    journal_dir: &Path,
    mode: HardConflictMode,
    cancel: &AtomicBool,
    current_generation: u64,
) -> Result<ApplyOutcome, ApplyError> {
    apply_transaction(&mut ApplyExecution {
        transaction: &mut tx,
        approved_paths: approved,
        current_generation,
        trusted,
        journal_dir: journal_dir.to_path_buf(),
        hard_conflict_mode: mode,
        cancel,
        directory_policy: super::preflight::DirectoryPolicy::SameDirectory,
        allow_symlink_source: false,
    })
}

fn approved_of(paths: &[&Path]) -> BTreeSet<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// A recursive `(relative path, inode, size, mtime, contents)` snapshot.
fn snapshot(root: &Path) -> Vec<(PathBuf, u64, u64, u64, Vec<u8>)> {
    let mut out = Vec::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_dir() {
                queue.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                let content = std::fs::read(&path).unwrap_or_default();
                let inode = std::os::unix::fs::MetadataExt::ino(&meta);
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|e| e.as_secs())
                    .unwrap_or(0);
                out.push((relative, inode, meta.len(), modified, content));
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Happy path, gating, and no-clobber proofs
// ---------------------------------------------------------------------------

#[test]
fn one_approved_safe_rename_applies() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "goldenaxe.hdf");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let proposals = vec![proposal(
        source.to_str().unwrap(),
        "goldenaxe.hdf",
        "Golden Axe (Europe).hdf",
        ProposalState::Suggested,
    )];
    let plan = plan(proposals, 1, &roms);
    let approved = approved_of(&[&source]);

    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved.clone(),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();

    assert_eq!(outcome.transaction.state, TransactionState::Applied);
    assert_eq!(
        outcome.transaction.classifier_version.as_deref(),
        Some(crate::dat::classification::CLASSIFIER_VERSION)
    );
    assert_eq!(outcome.summary.applied, 1);
    assert_eq!(outcome.summary.failed, 0);
    assert_eq!(
        inspect_exact_resume(
            &outcome.transaction,
            plan.generation,
            &compute_plan_digest(&plan, &approved),
        ),
        ExactResumeInspection::AlreadyComplete
    );
    assert!(!source.exists());
    assert!(roms.join("Golden Axe (Europe).hdf").exists());
    // Content is identical through the rename.
    assert_eq!(
        std::fs::read(roms.join("Golden Axe (Europe).hdf")).unwrap(),
        b"fixture contents"
    );
    // The journal is present and says Applied.
    assert!(
        journal
            .join(format!("{}.json", outcome.transaction.transaction_id))
            .exists()
    );
}

#[test]
fn an_unapproved_proposal_cannot_apply() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    let error = apply(
        &plan,
        BTreeSet::new(), // nothing approved
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(error, ApplyError::NothingApproved);
    assert!(source.exists(), "nothing was touched");
}

#[test]
fn an_ambiguous_proposal_cannot_apply() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let mut p = proposal(
        source.to_str().unwrap(),
        "a.bin",
        "b.bin",
        ProposalState::Ambiguous,
    );
    p.actionable = false;
    let plan = plan(vec![p], 1, &roms);
    let cancel = no_cancel();
    let error = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(
        error,
        ApplyError::NothingApproved,
        "ambiguous proposals are not applicable"
    );
    assert!(source.exists());
}

#[test]
fn a_conflict_proposal_cannot_apply() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let mut p = proposal(
        source.to_str().unwrap(),
        "a.bin",
        "b.bin",
        ProposalState::Conflict,
    );
    p.actionable = false;
    let plan = plan(vec![p], 1, &roms);
    let cancel = no_cancel();
    let error = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(
        error,
        ApplyError::NothingApproved,
        "conflict proposals are not applicable"
    );
    assert!(source.exists());
}

#[test]
fn a_stale_generation_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        5,
        &roms,
    );
    let error = build_transaction(&plan, &approved_of(&[&source]), 6).unwrap_err();
    assert!(matches!(error, ApplyError::StalePlan { .. }));
    assert!(source.exists());
}

#[test]
fn a_changed_classifier_version_is_rejected_before_apply_without_filesystem_changes() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let mut transaction = build_transaction(&plan, &approved, 1).unwrap();
    transaction.classifier_version = Some("superseded-classifier".to_string());
    let before = snapshot(dir.path());

    let error = apply_exec(
        transaction,
        approved,
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &no_cancel(),
        1,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ApplyError::StaleClassifierVersion {
            plan: Some(ref version),
            ..
        } if version == "superseded-classifier"
    ));
    assert_eq!(
        snapshot(dir.path()),
        before,
        "no journal or rename was written"
    );
    assert!(source.exists());
    assert!(!roms.join("b.bin").exists());
}

#[test]
fn a_missing_classifier_version_is_rejected_before_apply_without_filesystem_changes() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let mut transaction = build_transaction(&plan, &approved, 1).unwrap();
    transaction.classifier_version = None;
    let before = snapshot(dir.path());

    let error = apply_exec(
        transaction,
        approved,
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &no_cancel(),
        1,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ApplyError::StaleClassifierVersion { plan: None, .. }
    ));
    assert_eq!(
        snapshot(dir.path()),
        before,
        "no journal or rename was written"
    );
    assert!(source.exists());
    assert!(!roms.join("b.bin").exists());
}

#[test]
fn an_existing_destination_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    write(&roms, "b.bin"); // destination exists
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    let error = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();

    // AbortAll: a hard conflict prevents the batch from starting at all.
    assert!(matches!(error, ApplyError::HardConflicts(_)));
    assert!(source.exists(), "the source must not move");
    assert_eq!(
        std::fs::read(roms.join("b.bin")).unwrap(),
        b"fixture contents"
    );
}

#[test]
fn an_existing_destination_in_skip_mode_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    write(&roms, "b.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
    )
    .unwrap();
    assert_eq!(outcome.summary.skipped, 1);
    assert_eq!(outcome.transaction.entries[0].state, EntryState::Skipped);
    assert!(source.exists());
}

#[test]
fn a_symlink_source_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let target = write(&roms, "target.bin");
    let link = roms.join("link.bin");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let mut p = proposal(
        link.to_str().unwrap(),
        "link.bin",
        "renamed.bin",
        ProposalState::Suggested,
    );
    p.object_kind = SourceObjectKind::Symlink;
    let plan = plan(vec![p], 1, &roms);
    let cancel = no_cancel();
    let error = apply(
        &plan,
        approved_of(&[&link]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(
        error,
        ApplyError::NothingApproved,
        "symlink sources are never applicable"
    );
    // The link still points at its target; neither was touched.
    assert_eq!(std::fs::read_link(&link).unwrap(), target);
    assert!(target.exists());
}

#[test]
fn outside_trusted_roots_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    // Trusted root is a DIFFERENT directory than the source's parent.
    let error = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&other]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert!(matches!(error, ApplyError::HardConflicts(_)), "{error:?}");
    assert!(source.exists());
}

#[test]
fn cancellation_before_first_rename_leaves_everything_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let before = snapshot(&roms);
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = cancelled();
    let error = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(error, ApplyError::Cancelled);
    assert_eq!(snapshot(&roms), before, "nothing changed");
}

#[test]
fn an_apply_failure_stops_subsequent_operations() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let a = write(&roms, "a.bin");
    let b = write(&roms, "b.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    // b.bin's destination already exists, so it fails preflight; a is fine.
    write(&roms, "B.bin");
    let proposals = vec![
        proposal(
            a.to_str().unwrap(),
            "a.bin",
            "A.bin",
            ProposalState::Suggested,
        ),
        proposal(
            b.to_str().unwrap(),
            "b.bin",
            "B.bin",
            ProposalState::Suggested,
        ),
    ];
    let plan = plan(proposals, 1, &roms);
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&a, &b]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
    )
    .unwrap();
    // b is skipped (preflight hard conflict); a applies.
    assert_eq!(outcome.summary.skipped, 1);
    assert_eq!(outcome.summary.applied, 1);
    assert!(roms.join("A.bin").exists());
    assert!(b.exists());
}

// ---------------------------------------------------------------------------
// Hostile filesystem changes between review and apply
// ---------------------------------------------------------------------------

#[test]
fn source_replaced_with_a_symlink_after_approval_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    // Hostile change: replace the source with a symlink after approval.
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(dir.path().join("elsewhere.bin"), &source).unwrap();
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, 0);
    assert_eq!(
        outcome.summary.skipped, 1,
        "the symlink substitution is a hard conflict"
    );
    let entry = &outcome.transaction.entries[0];
    assert!(
        entry
            .preflight_failures
            .iter()
            .any(|f| f.contains("symlink")),
        "{:?}",
        entry.preflight_failures
    );
    // The symlink is still there; the target was never touched.
    assert!(
        std::fs::symlink_metadata(&source)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn source_replaced_with_a_different_inode_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let tx = build_transaction(&plan, &approved, 1).unwrap();
    // Hostile change: delete and recreate the file (different inode) after review.
    std::fs::remove_file(&source).unwrap();
    std::fs::write(&source, b"different").unwrap();
    let cancel = no_cancel();
    let outcome = apply_exec(
        tx,
        approved,
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
        1,
    )
    .unwrap();
    assert_eq!(
        outcome.summary.applied, 0,
        "a different object must not be renamed"
    );
    assert_eq!(outcome.summary.skipped, 1);
}

#[test]
fn destination_created_after_approval_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    // Hostile change: destination appears after approval.
    write(&roms, "b.bin");
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        outcome.summary.applied, 0,
        "an appearing destination must never be overwritten"
    );
    assert_eq!(
        std::fs::read(roms.join("b.bin")).unwrap(),
        b"fixture contents"
    );
}

#[test]
fn source_renamed_externally_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let tx = build_transaction(&plan, &approved, 1).unwrap();
    // Hostile change: rename the source externally after review.
    let renamed = roms.join("a-moved.bin");
    std::fs::rename(&source, &renamed).unwrap();
    let cancel = no_cancel();
    let outcome = apply_exec(
        tx,
        approved,
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
        1,
    )
    .unwrap();
    assert_eq!(
        outcome.summary.applied, 0,
        "an externally renamed source must not be touched"
    );
    assert_eq!(outcome.summary.skipped, 1);
    assert!(renamed.exists());
}

#[test]
fn size_changed_after_approval_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let tx = build_transaction(&plan, &approved, 1).unwrap();
    // Hostile change: change the size after review.
    std::fs::write(&source, b"much longer content").unwrap();
    let cancel = no_cancel();
    let outcome = apply_exec(
        tx,
        approved,
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
        1,
    )
    .unwrap();
    assert_eq!(
        outcome.summary.applied, 0,
        "a resized file must not be renamed"
    );
    assert_eq!(outcome.summary.skipped, 1);
}

#[test]
fn destination_parent_changed_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let tx = build_transaction(&plan, &approved, 1).unwrap();
    // Hostile change: the source is moved into a different directory after review.
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let moved = elsewhere.join("a.bin");
    std::fs::rename(&source, &moved).unwrap();
    let cancel = no_cancel();
    let outcome = apply_exec(
        tx,
        approved,
        TrustedRoots::from_paths([&roms, &elsewhere]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
        1,
    )
    .unwrap();
    assert_eq!(
        outcome.summary.applied, 0,
        "a source moved elsewhere must not be renamed"
    );
    assert_eq!(outcome.summary.skipped, 1);
    assert!(moved.exists());
}

#[test]
fn a_case_fold_sibling_appearing_after_approval_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "game.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    // Proposal: game.bin -> Game.bin
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "game.bin",
            "Game.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let tx = build_transaction(&plan, &approved, 1).unwrap();
    // Hostile change: a second file appears with the same case-fold after review.
    write(&roms, "GAME.BIN");
    let cancel = no_cancel();
    let outcome = apply_exec(
        tx,
        approved,
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
        1,
    )
    .unwrap();
    assert_eq!(
        outcome.summary.applied, 0,
        "the case-fold collision must be detected at apply time"
    );
    assert_eq!(outcome.summary.skipped, 1);
}

#[test]
fn duplicate_batch_destinations_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let a = write(&roms, "a.bin");
    let b = write(&roms, "b.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    // Two proposals targeting the same destination.
    let proposals = vec![
        proposal(
            a.to_str().unwrap(),
            "a.bin",
            "Same.bin",
            ProposalState::Suggested,
        ),
        proposal(
            b.to_str().unwrap(),
            "b.bin",
            "Same.bin",
            ProposalState::Suggested,
        ),
    ];
    let plan = plan(proposals, 1, &roms);
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&a, &b]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        outcome.summary.applied, 0,
        "duplicate targets must not apply"
    );
}

// ---------------------------------------------------------------------------
// Journal ordering and crash recovery
// ---------------------------------------------------------------------------

#[test]
fn the_journal_is_written_before_any_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();
    // The journal exists and reflects the applied state.
    let path = journal_path(&journal, &outcome.transaction.transaction_id).unwrap();
    let persisted = read_journal(&path).unwrap();
    assert_eq!(persisted.state, TransactionState::Applied);
    assert_eq!(persisted.entries[0].state, EntryState::Applied);
}

#[test]
fn crash_after_journal_write_before_first_rename_is_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    // Simulate a crash: journal written, state Planned, nothing renamed.
    let tx = RenameTransaction {
        transaction_id: "crash1".to_string(),
        plan_generation: 1,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
        created_at_unix: 1,
        source_scan_root: "/tmp/roms".to_string(),
        state: TransactionState::Planned,
        entries: vec![TransactionEntry {
            source_path: PathBuf::from("/tmp/roms/a.bin"),
            destination_path: PathBuf::from("/tmp/roms/b.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "b.bin".to_string(),
            identity: ObjectIdentity {
                freshness: None,
                size_bytes: 1,
                modified_unix: 1,
                kind: ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            operation: Default::default(),
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    write_journal(&journal, &tx).unwrap();

    let (recovery, problems) = find_recovery_transactions(&journal);
    assert!(problems.is_empty());
    assert_eq!(recovery.len(), 1);
    assert_eq!(
        recovery[0].applied_count(),
        0,
        "nothing was renamed before the crash"
    );
}

#[test]
fn crash_after_first_of_n_renames_is_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let mut tx = RenameTransaction {
        transaction_id: "crash2".to_string(),
        plan_generation: 1,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
        created_at_unix: 1,
        source_scan_root: "/tmp/roms".to_string(),
        state: TransactionState::Applying,
        entries: Vec::new(),
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let mk = |source: &str, state: EntryState| TransactionEntry {
        source_path: PathBuf::from(source),
        destination_path: PathBuf::from(source.replace("a.bin", "A.bin").replace("b.bin", "B.bin")),
        original_basename: source.rsplit('/').next().unwrap().to_string(),
        proposed_basename: "x.bin".to_string(),
        identity: ObjectIdentity {
            freshness: None,
            size_bytes: 1,
            modified_unix: 1,
            kind: ObjectKind::RegularFile,
            #[cfg(unix)]
            ino: 1,
            #[cfg(unix)]
            dev: 1,
        },
        operation: Default::default(),
        preflight_passed: false,
        preflight_failures: Vec::new(),
        state,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    };
    tx.entries.push(mk("/tmp/roms/a.bin", EntryState::Applied));
    tx.entries.push(mk("/tmp/roms/b.bin", EntryState::Planned));
    write_journal(&journal, &tx).unwrap();

    let (recovery, _) = find_recovery_transactions(&journal);
    assert_eq!(recovery.len(), 1);
    assert_eq!(
        recovery[0].applied_count(),
        1,
        "one rename happened before the crash"
    );
    assert_eq!(recovery[0].state, TransactionState::Applying);
}

#[test]
fn recovery_never_auto_resumes() {
    // There is no resume function anywhere in the module: the only recovery
    // operations are reading journals and rolling back on explicit choice.
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let tx = RenameTransaction {
        transaction_id: "nore".to_string(),
        plan_generation: 1,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
        created_at_unix: 1,
        source_scan_root: "/tmp/roms".to_string(),
        state: TransactionState::Applying,
        entries: Vec::new(),
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    write_journal(&journal, &tx).unwrap();
    let (recovery, _) = find_recovery_transactions(&journal);
    assert_eq!(recovery.len(), 1);
    // A recovery journal is never acted on without an explicit rollback call.
    // (The transaction state remains untouched here.)
    assert_eq!(recovery[0].state, TransactionState::Applying);
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

fn apply_one(dir: &Path) -> (RenamePlan, RenameTransaction, BTreeSet<String>) {
    let roms = dir.join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();
    (plan, outcome.transaction, approved_of(&[&source]))
}

#[test]
fn a_successful_rollback_restores_the_original_path_and_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    let cancel = no_cancel();
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    assert_eq!(outcome.transaction.state, TransactionState::RolledBack);
    let roms = dir.path().join("roms");
    assert!(roms.join("a.bin").exists(), "the original path is restored");
    assert!(!roms.join("b.bin").exists());
    assert_eq!(
        std::fs::read(roms.join("a.bin")).unwrap(),
        b"fixture contents"
    );
}

#[test]
fn rollback_reverses_in_reverse_order() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let a = write(&roms, "a.bin");
    let b = write(&roms, "b.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let proposals = vec![
        proposal(
            a.to_str().unwrap(),
            "a.bin",
            "A.bin",
            ProposalState::Suggested,
        ),
        proposal(
            b.to_str().unwrap(),
            "b.bin",
            "B.bin",
            ProposalState::Suggested,
        ),
    ];
    let plan = plan(proposals, 1, &roms);
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&a, &b]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, 2);
    let mut tx = outcome.transaction;

    // The second-applied entry (index 1) is rolled back first. Block that by
    // making its rollback impossible? Instead we prove order via the journal's
    // rolled_back_at timestamps / the fact that rollback of the second must
    // succeed before the first is attempted: we break the FIRST-applied entry's
    // destination externally, so only the second can roll back.
    std::fs::remove_file(roms.join("A.bin")).unwrap(); // first-applied destination gone
    let rollback = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert!(
        matches!(rollback.result, RollbackResult::PartiallyRolledBack { .. }),
        "second rolls back, first cannot: {:?}",
        rollback.result
    );
    assert!(
        roms.join("b.bin").exists(),
        "the second entry was rolled back"
    );
    assert!(!roms.join("B.bin").exists());
    assert!(
        !roms.join("a.bin").exists(),
        "the first could not roll back (destination gone)"
    );
}

#[test]
fn rollback_refuses_when_the_destination_was_changed_externally() {
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    // Hostile change: replace the destination after apply.
    let roms = dir.path().join("roms");
    std::fs::write(roms.join("b.bin"), b"replaced by an attacker").unwrap();
    let cancel = no_cancel();
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert!(matches!(
        outcome.result,
        RollbackResult::RollbackFailed { .. }
    ));
    assert_eq!(outcome.transaction.state, TransactionState::RollbackFailed);
    assert!(!roms.join("a.bin").exists(), "nothing was moved back");
}

#[test]
fn rollback_refuses_when_the_original_name_is_occupied() {
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    let roms = dir.path().join("roms");
    // Hostile change: a new file occupies the original source path.
    write(&roms, "a.bin");
    let cancel = no_cancel();
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert!(matches!(
        outcome.result,
        RollbackResult::RollbackFailed { .. }
    ));
    // The occupied original is untouched; the destination still has the data.
    assert_eq!(
        std::fs::read(roms.join("a.bin")).unwrap(),
        b"fixture contents"
    );
    assert_eq!(
        std::fs::read(roms.join("b.bin")).unwrap(),
        b"fixture contents"
    );
}

#[test]
fn repeated_rollback_is_idempotent_and_safe() {
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    let cancel = no_cancel();
    let first = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert_eq!(first.result, RollbackResult::FullyRolledBack);
    let second = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert_eq!(
        second.result,
        RollbackResult::FullyRolledBack,
        "second rollback is a safe no-op"
    );
    let roms = dir.path().join("roms");
    assert!(roms.join("a.bin").exists());
    assert!(!roms.join("b.bin").exists());
}

/// Applies `names.len()` renames (`x.bin` -> `X.bin`) as one transaction.
fn apply_many(dir: &Path, names: &[&str]) -> (RenameTransaction, PathBuf) {
    let roms = dir.join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let journal = dir.join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let sources: Vec<PathBuf> = names.iter().map(|name| write(&roms, name)).collect();
    let proposals = sources
        .iter()
        .zip(names)
        .map(|(source, name)| {
            proposal(
                source.to_str().unwrap(),
                name,
                &name.to_uppercase().replace(".BIN", ".bin"),
                ProposalState::Suggested,
            )
        })
        .collect();
    let plan = plan(proposals, 1, &roms);
    let cancel = no_cancel();
    let approved: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();
    let outcome = apply(
        &plan,
        approved_of(&approved),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, names.len());
    (outcome.transaction, journal)
}

fn journal_bytes(journal: &Path, transaction_id: &str) -> Vec<u8> {
    std::fs::read(journal_path(journal, transaction_id).unwrap()).unwrap()
}

#[test]
fn a_repeated_rollback_after_a_refused_undo_is_never_a_full_rollback() {
    // CONFIRMED BUG: the first pass correctly ended RollbackFailed, but a
    // second request found no Applied entry to process, counted nothing as
    // incomplete and rewrote the journal as RolledBack / FullyRolledBack while
    // the changed destination was still on disk.
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    let roms = dir.path().join("roms");
    std::fs::write(roms.join("b.bin"), b"changed by the user").unwrap();
    let cancel = no_cancel();

    let first = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert!(matches!(
        first.result,
        RollbackResult::RollbackFailed { .. }
    ));
    let reason = tx.entries[0]
        .failure_reason
        .clone()
        .expect("the refusal is recorded");

    for attempt in 2..=3 {
        let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
        let RollbackResult::RollbackFailed { failed } = &outcome.result else {
            panic!("attempt {attempt} must stay failed: {:?}", outcome.result);
        };
        assert_eq!(failed.len(), 1);
        assert!(failed[0].1.contains(&reason), "{failed:?}");
        assert_eq!(outcome.transaction.state, TransactionState::RollbackFailed);
        assert!(!outcome.transaction.is_fully_rolled_back());
        // The durable journal keeps the failure and its original reason.
        let persisted = read_journal(&journal_path(&journal, &tx.transaction_id).unwrap()).unwrap();
        assert_eq!(persisted.state, TransactionState::RollbackFailed);
        assert_eq!(persisted.entries[0].state, EntryState::RollbackFailed);
        assert_eq!(
            persisted.entries[0].failure_reason.as_deref(),
            Some(reason.as_str())
        );
        assert!(persisted.state.needs_recovery());
        // The user's file is untouched and nothing was moved back.
        assert_eq!(
            std::fs::read(roms.join("b.bin")).unwrap(),
            b"changed by the user"
        );
        assert!(!roms.join("a.bin").exists());
    }
}

#[test]
fn a_persisted_rollback_failure_survives_a_restart_and_a_repeated_request() {
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    let roms = dir.path().join("roms");
    std::fs::write(roms.join("b.bin"), b"changed by the user").unwrap();
    let cancel = no_cancel();
    rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    let transaction_id = tx.transaction_id.clone();
    drop(tx);

    // "Restart": only the journal on disk is known.
    let (recovery, problems) = find_recovery_transactions(&journal);
    assert!(problems.is_empty());
    assert_eq!(recovery.len(), 1);
    let mut reloaded = recovery.into_iter().next().unwrap();
    assert_eq!(reloaded.transaction_id, transaction_id);
    let outcome = rollback_transaction(&mut reloaded, &journal, &cancel).unwrap();
    assert!(matches!(
        outcome.result,
        RollbackResult::RollbackFailed { .. }
    ));
    let (still_recovery, _) = find_recovery_transactions(&journal);
    assert_eq!(still_recovery.len(), 1, "the failure is still surfaced");
    assert_eq!(still_recovery[0].state, TransactionState::RollbackFailed);
    assert_eq!(
        std::fs::read(roms.join("b.bin")).unwrap(),
        b"changed by the user"
    );
}

#[test]
fn a_repeated_rollback_after_a_partial_undo_stays_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin"]);
    let roms = dir.path().join("roms");
    // Index 1 (b) is reversed first; index 0 (a) is then refused.
    std::fs::write(roms.join("A.bin"), b"changed by the user").unwrap();
    let cancel = no_cancel();
    let first = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert!(matches!(
        first.result,
        RollbackResult::PartiallyRolledBack { .. }
    ));

    let second = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    // Nothing further could be reversed, so this pass is a plain failure.
    let RollbackResult::RollbackFailed { failed } = &second.result else {
        panic!("a repeat must stay incomplete: {:?}", second.result);
    };
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, roms.join("a.bin"));
    let persisted = read_journal(&journal_path(&journal, &tx.transaction_id).unwrap()).unwrap();
    assert_eq!(persisted.state, TransactionState::RollbackFailed);
    assert_eq!(persisted.entries[0].state, EntryState::RollbackFailed);
    assert!(persisted.entries[0].failure_reason.is_some());
    assert_eq!(persisted.entries[1].state, EntryState::RolledBack);
    assert_eq!(
        std::fs::read(roms.join("A.bin")).unwrap(),
        b"changed by the user"
    );
    assert!(!roms.join("a.bin").exists());
    assert_eq!(
        std::fs::read(roms.join("b.bin")).unwrap(),
        b"fixture contents"
    );
}

#[test]
fn remaining_entries_continue_safely_around_a_blocked_entry() {
    // Replaces the earlier stop-at-first-refusal rule: batch preflight keeps
    // entries independent, so one refused plain move no longer strands the
    // entries reversed after it.
    let dir = tempfile::tempdir().unwrap();
    let (mut tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin", "c.bin"]);
    let roms = dir.path().join("roms");
    // Reverse order: c is reversed, b is refused, a is still reversed.
    std::fs::write(roms.join("B.bin"), b"changed by the user").unwrap();
    let cancel = no_cancel();
    let first = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    let RollbackResult::PartiallyRolledBack {
        rolled_back,
        failed,
    } = &first.result
    else {
        panic!("{:?}", first.result);
    };
    assert_eq!(rolled_back, &vec![roms.join("c.bin"), roms.join("a.bin")]);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, roms.join("b.bin"));
    let reason = tx.entries[1].failure_reason.clone().unwrap();

    // A repeat changes nothing while b's destination is still not the object
    // that was renamed, and never claims a full rollback.
    let second = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    let RollbackResult::RollbackFailed { failed } = &second.result else {
        panic!("{:?}", second.result);
    };
    assert_eq!(failed.len(), 1);
    assert!(failed[0].1.contains("identity changed"), "{failed:?}");
    assert!(failed[0].1.contains(&reason), "{failed:?}");
    let persisted = read_journal(&journal_path(&journal, &tx.transaction_id).unwrap()).unwrap();
    assert_eq!(persisted.state, TransactionState::RollbackFailed);
    assert_eq!(
        persisted
            .entries
            .iter()
            .map(|entry| entry.state)
            .collect::<Vec<_>>(),
        vec![
            EntryState::RolledBack,
            EntryState::RollbackFailed,
            EntryState::RolledBack
        ]
    );
    assert_eq!(
        persisted.entries[1].failure_reason.as_deref(),
        Some(reason.as_str())
    );
    assert_eq!(
        std::fs::read(roms.join("B.bin")).unwrap(),
        b"changed by the user"
    );
    assert!(!roms.join("b.bin").exists());
    assert_eq!(
        std::fs::read(roms.join("a.bin")).unwrap(),
        b"fixture contents"
    );
    assert_eq!(
        std::fs::read(roms.join("c.bin")).unwrap(),
        b"fixture contents"
    );
}

/// a.bin -> A.bin and b.bin -> B.bin applied; another file then takes the
/// original name a.bin, so a's undo is refused and recorded for review.
fn applied_with_a_blocked(dir: &Path) -> (RenameTransaction, PathBuf, PathBuf) {
    let (mut tx, journal) = apply_many(dir, &["a.bin", "b.bin"]);
    let roms = dir.join("roms");
    std::fs::write(roms.join("a.bin"), b"someone else's file").unwrap();
    let first = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
    assert!(matches!(
        first.result,
        RollbackResult::PartiallyRolledBack { .. }
    ));
    assert_eq!(tx.entries[0].state, EntryState::RollbackFailed);
    assert_eq!(tx.entries[1].state, EntryState::RolledBack);
    (tx, journal, roms)
}

#[test]
fn a_blocked_entry_remains_needs_review_and_its_retry_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tx, journal, roms) = applied_with_a_blocked(dir.path());
    let before = journal_bytes(&journal, &tx.transaction_id);
    let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
    let RollbackResult::RollbackFailed { failed } = &outcome.result else {
        panic!("{:?}", outcome.result);
    };
    assert_eq!(failed.len(), 1);
    assert!(
        failed[0]
            .1
            .contains("needs review: the original path is still occupied"),
        "{failed:?}"
    );
    assert!(failed[0].1.contains("nothing was changed"), "{failed:?}");
    assert_eq!(tx.entries[0].state, EntryState::RollbackFailed);
    assert_eq!(tx.state, TransactionState::RollbackFailed);
    assert!(!tx.is_fully_rolled_back());
    assert_eq!(
        std::fs::read(roms.join("a.bin")).unwrap(),
        b"someone else's file"
    );
    assert_eq!(
        std::fs::read(roms.join("A.bin")).unwrap(),
        b"fixture contents"
    );
    // The entry's recorded state and reason are untouched.
    let after = read_journal(&journal_path(&journal, &tx.transaction_id).unwrap()).unwrap();
    let earlier: RenameTransaction = serde_json::from_slice(&before).unwrap();
    assert_eq!(after.entries[0], earlier.entries[0]);
}

#[test]
fn once_the_obstruction_is_gone_the_guarded_retry_restores_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tx, journal, roms) = applied_with_a_blocked(dir.path());
    std::fs::remove_file(roms.join("a.bin")).unwrap();
    let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    assert!(tx.is_fully_rolled_back());
    assert_eq!(
        std::fs::read(roms.join("a.bin")).unwrap(),
        b"fixture contents"
    );
    assert!(!roms.join("A.bin").exists());
    let persisted = read_journal(&journal_path(&journal, &tx.transaction_id).unwrap()).unwrap();
    assert!(persisted.is_fully_rolled_back());
    assert!(persisted.entries[0].failure_reason.is_none());
}

#[test]
fn a_retry_is_refused_when_the_moved_files_identity_changed() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tx, journal, roms) = applied_with_a_blocked(dir.path());
    std::fs::remove_file(roms.join("a.bin")).unwrap();
    // Same path, different object: never restored over the original name.
    std::fs::remove_file(roms.join("A.bin")).unwrap();
    std::fs::write(roms.join("A.bin"), b"a different file").unwrap();
    let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
    let RollbackResult::RollbackFailed { failed } = &outcome.result else {
        panic!("{:?}", outcome.result);
    };
    assert!(failed[0].1.contains("identity changed"), "{failed:?}");
    assert_eq!(tx.entries[0].state, EntryState::RollbackFailed);
    assert!(!roms.join("a.bin").exists());
    assert_eq!(
        std::fs::read(roms.join("A.bin")).unwrap(),
        b"a different file"
    );
}

#[test]
fn a_retry_is_refused_when_the_moved_file_is_gone_or_replaced_by_a_directory() {
    for replacement in ["missing", "directory"] {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal, roms) = applied_with_a_blocked(dir.path());
        std::fs::remove_file(roms.join("a.bin")).unwrap();
        std::fs::remove_file(roms.join("A.bin")).unwrap();
        if replacement == "directory" {
            std::fs::create_dir(roms.join("A.bin")).unwrap();
        }
        let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        let RollbackResult::RollbackFailed { failed } = &outcome.result else {
            panic!("{replacement}: {:?}", outcome.result);
        };
        let expected = if replacement == "missing" {
            "no longer at its recorded location"
        } else {
            "restoring could merge"
        };
        assert!(failed[0].1.contains(expected), "{failed:?}");
        assert!(!roms.join("a.bin").exists());
        assert_eq!(tx.entries[0].state, EntryState::RollbackFailed);
    }
}

/// The first Undo after Apply is protected exactly like a retry: a journal
/// changed between Apply and Undo is never overwritten from memory.
#[test]
fn a_journal_tampered_before_the_first_undo_refuses_without_any_write() {
    for tamper in ["edited", "deleted", "older"] {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin"]);
        let roms = dir.path().join("roms");
        let path = journal_path(&journal, &tx.transaction_id).unwrap();
        let session = tx.clone();
        match tamper {
            "edited" => {
                let mut recorded = read_journal(&path).unwrap();
                recorded.entries[1].destination_path = roms.join("Elsewhere.bin");
                std::fs::write(&path, serde_json::to_string(&recorded).unwrap()).unwrap();
            }
            "deleted" => std::fs::remove_file(&path).unwrap(),
            _ => {
                // A valid-looking journal from before the batch finished.
                // Written outside EmuWiz: never a sealed generation.
                let mut older = tx.clone();
                older.state = TransactionState::Applying;
                older.entries[1].state = EntryState::Planned;
                std::fs::write(&path, serde_json::to_string(&older).unwrap()).unwrap();
            }
        }
        let on_disk = || std::fs::read(&path).ok();
        let tampered = on_disk();
        for attempt in 1..=2 {
            let refused = rollback_transaction(&mut tx, &journal, &no_cancel())
                .expect_err("a tampered journal must refuse the first Undo");
            assert!(
                is_journal_integrity_loss(&refused),
                "{tamper} attempt {attempt}: {refused}"
            );
            assert_eq!(on_disk(), tampered, "{tamper} attempt {attempt}");
            assert_eq!(tx, session, "{tamper}");
            assert!(!roms.join("a.bin").exists() && !roms.join("b.bin").exists());
            assert!(roms.join("A.bin").exists() && roms.join("B.bin").exists());
        }
    }
}

/// Tampering between two retries is caught on the next retry.
#[test]
fn a_journal_tampered_between_two_retries_refuses_the_next_without_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin", "c.bin"]);
    let roms = dir.path().join("roms");
    let path = journal_path(&journal, &tx.transaction_id).unwrap();
    std::fs::write(roms.join("a.bin"), b"blocker a").unwrap();
    std::fs::write(roms.join("b.bin"), b"blocker b").unwrap();
    let first = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
    assert!(matches!(
        first.result,
        RollbackResult::PartiallyRolledBack { .. }
    ));
    // First retry: b's blocker is gone, a's remains.
    std::fs::remove_file(roms.join("b.bin")).unwrap();
    let second = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
    assert!(matches!(
        second.result,
        RollbackResult::PartiallyRolledBack { .. }
    ));
    assert_eq!(tx.entries[1].state, EntryState::RolledBack);
    // Between retries the journal is edited and a's blocker removed.
    std::fs::remove_file(roms.join("a.bin")).unwrap();
    let mut recorded = read_journal(&path).unwrap();
    recorded.entries[0].failure_reason = Some("edited between retries".into());
    std::fs::write(&path, serde_json::to_string(&recorded).unwrap()).unwrap();
    let tampered = std::fs::read(&path).unwrap();
    let session = tx.clone();
    for _ in 0..2 {
        let refused = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap_err();
        assert!(is_journal_integrity_loss(&refused), "{refused}");
        assert_eq!(std::fs::read(&path).unwrap(), tampered);
        assert_eq!(tx, session);
        assert!(!roms.join("a.bin").exists());
        assert!(roms.join("A.bin").exists());
    }
}

/// A journal changed outside this Undo is never overwritten from memory and
/// never adopted: every repeat attempt in the same session is refused with no
/// file or journal mutation.
#[test]
fn a_tampered_journal_is_refused_on_every_attempt_without_any_write() {
    for tamper in ["edited", "symlink", "deleted", "older", "between"] {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin"]);
        let roms = dir.path().join("roms");
        let path = journal_path(&journal, &tx.transaction_id).unwrap();
        let older = std::fs::read(&path).unwrap();
        // A valid partial Undo: a's original name is taken.
        std::fs::write(roms.join("a.bin"), b"someone else's file").unwrap();
        let first = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        assert!(matches!(
            first.result,
            RollbackResult::PartiallyRolledBack { .. }
        ));
        std::fs::remove_file(roms.join("a.bin")).unwrap();
        let session = tx.clone();

        match tamper {
            "edited" => {
                let mut recorded = read_journal(&path).unwrap();
                recorded.entries[0].failure_reason = Some("edited elsewhere".into());
                std::fs::write(&path, serde_json::to_string(&recorded).unwrap()).unwrap();
            }
            "symlink" => {
                let elsewhere = dir.path().join("elsewhere.json");
                std::fs::rename(&path, &elsewhere).unwrap();
                std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
            }
            "deleted" => std::fs::remove_file(&path).unwrap(),
            // A valid-looking journal from before the partial Undo.
            "older" => std::fs::write(&path, &older).unwrap(),
            _ => {
                let target = path.clone();
                super::rollback::AFTER_JOURNAL_VALIDATION.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        std::fs::write(&target, b"{\"changed\": \"between checks\"}").unwrap();
                    }));
                });
            }
        }
        let on_disk = || {
            std::fs::symlink_metadata(&path)
                .ok()
                .map(|_| std::fs::read(&path).unwrap_or_default())
        };
        let tampered = if tamper == "between" {
            Some(b"{\"changed\": \"between checks\"}".to_vec())
        } else {
            on_disk()
        };

        for attempt in 1..=2 {
            let refused = rollback_transaction(&mut tx, &journal, &no_cancel())
                .expect_err("a tampered journal must refuse the Undo");
            assert!(
                is_journal_integrity_loss(&refused),
                "{tamper} attempt {attempt}: {refused}"
            );
            assert_eq!(on_disk(), tampered, "{tamper} attempt {attempt}");
            assert_eq!(tx, session, "{tamper}: the session record changed");
            assert!(!roms.join("a.bin").exists(), "{tamper}");
            assert_eq!(
                std::fs::read(roms.join("A.bin")).unwrap(),
                b"fixture contents",
                "{tamper}"
            );
            assert_eq!(tx.entries[0].state, EntryState::RollbackFailed);
        }
    }
}
/// The Undo fence re-reads the journal on its opened descriptor: a FIFO
/// swapped in after validation, and after the fence's own path check, is
/// refused as integrity loss without a blocking open, and is preserved.
#[cfg(unix)]
#[test]
fn a_journal_swapped_for_a_fifo_after_validation_refuses_undo_without_blocking() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;
    let dir = tempfile::tempdir().unwrap();
    let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
    let roms = dir.path().join("roms");
    let path = journal_path(&journal, &tx.transaction_id).unwrap();
    let staged = dir.path().join("staged-fifo");
    let cpath = std::ffi::CString::new(staged.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    let target = path.clone();
    super::rollback::AFTER_JOURNAL_VALIDATION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            // The fence's next read: its lstat sees the regular journal, then
            // the swap lands before its open.
            super::journal::BEFORE_REGULAR_OPEN.with(|open| {
                let mut staged = Some(staged);
                *open.borrow_mut() = Some(Box::new(move |opened: &Path| {
                    if opened == target
                        && let Some(staged) = staged.take()
                    {
                        std::fs::rename(&staged, &target).unwrap();
                    }
                }));
            });
        }));
    });
    let refused = super::journal::fifo_guard::bounded(&[&path], || {
        rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap_err()
    });
    super::journal::BEFORE_REGULAR_OPEN.with(|open| *open.borrow_mut() = None);
    assert!(is_journal_integrity_loss(&refused), "{refused}");
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_fifo()
    );
    assert!(!roms.join("a.bin").exists());
    assert_eq!(
        std::fs::read(roms.join("A.bin")).unwrap(),
        b"fixture contents"
    );
}

/// Applied by the real executor: m.bin -> M.bin (move), a symbolic link
/// sym/s.bin -> s.bin and a hard link hard/h.bin -> h.bin.
#[cfg(unix)]
struct Mixed {
    _dir: tempfile::TempDir,
    tx: RenameTransaction,
    journal: PathBuf,
    roms: PathBuf,
    sym: PathBuf,
    hard: PathBuf,
}

#[cfg(unix)]
fn applied_mixed() -> Mixed {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let (roms, sym, hard, journal) = (
        base.join("roms"),
        base.join("sym"),
        base.join("hard"),
        base.join("journal"),
    );
    for path in [&roms, &sym, &hard, &journal] {
        std::fs::create_dir_all(path).unwrap();
    }
    let entry = |source: PathBuf, destination: PathBuf, operation| TransactionEntry {
        original_basename: source.file_name().unwrap().to_string_lossy().into_owned(),
        proposed_basename: destination
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        identity: capture_identity(&source).unwrap(),
        source_path: source,
        destination_path: destination,
        operation,
        preflight_passed: false,
        preflight_failures: Vec::new(),
        state: EntryState::Planned,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    };
    let (m, s, h) = (
        write(&roms, "m.bin"),
        write(&roms, "s.bin"),
        write(&roms, "h.bin"),
    );
    let entries = vec![
        entry(
            m.clone(),
            roms.join("M.bin"),
            TransactionOperation::RenameMove,
        ),
        entry(
            s.clone(),
            sym.join("s.bin"),
            TransactionOperation::CreateSymlink {
                expected_target: s.clone(),
                destination_root: sym.clone(),
            },
        ),
        entry(
            h.clone(),
            hard.join("h.bin"),
            TransactionOperation::CreateHardlink {
                expected_source: h.clone(),
                destination_root: hard.clone(),
            },
        ),
    ];
    let approved = entries
        .iter()
        .map(|entry| entry.source_path.to_string_lossy().into_owned())
        .collect();
    let mut tx = RenameTransaction {
        transaction_id: super::journal::new_transaction_id(1),
        plan_generation: 1,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
        created_at_unix: 1,
        source_scan_root: base.to_string_lossy().into_owned(),
        state: TransactionState::Planned,
        entries,
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let cancel = no_cancel();
    let outcome = apply_transaction(&mut ApplyExecution {
        transaction: &mut tx,
        approved_paths: approved,
        current_generation: 1,
        trusted: TrustedRoots::from_paths([&base]),
        journal_dir: journal.clone(),
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel: &cancel,
        directory_policy: super::preflight::DirectoryPolicy::SameFilesystem,
        allow_symlink_source: false,
    })
    .unwrap();
    assert!(outcome.summary.fully_resolved(), "{:?}", outcome.summary);
    assert_eq!(std::fs::read_link(sym.join("s.bin")).unwrap(), s);
    Mixed {
        _dir: dir,
        tx,
        journal,
        roms,
        sym,
        hard,
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Whether directory permissions are enforced for this user (not root).
#[cfg(unix)]
fn permissions_enforced(dir: &Path) -> bool {
    let probe = dir.join(".probe");
    set_mode(dir, 0o555);
    let enforced = std::fs::write(&probe, b"x").is_err();
    set_mode(dir, 0o755);
    let _ = std::fs::remove_file(&probe);
    if !enforced {
        eprintln!("SKIPPED: directory permissions are not enforced for this user");
    }
    enforced
}

#[cfg(unix)]
#[test]
fn a_mixed_move_and_link_undo_continues_around_a_blocked_link_but_never_retries_it() {
    let mut m = applied_mixed();
    if !permissions_enforced(&m.sym) {
        return;
    }
    set_mode(&m.sym, 0o555);
    let first = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    set_mode(&m.sym, 0o755);
    let RollbackResult::PartiallyRolledBack {
        rolled_back,
        failed,
    } = &first.result
    else {
        panic!("{:?}", first.result);
    };
    // Reverse order: the hard link and the move are still undone around it.
    assert_eq!(
        rolled_back,
        &vec![m.roms.join("h.bin"), m.roms.join("m.bin")]
    );
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, m.roms.join("s.bin"));
    assert_eq!(m.tx.entries[1].state, EntryState::RollbackFailed);
    assert!(!m.hard.join("h.bin").exists());
    assert!(m.roms.join("m.bin").exists());
    assert!(std::fs::symlink_metadata(m.sym.join("s.bin")).is_ok());

    // The obstruction is gone, yet a link is never removed by a retry: it
    // stays needing review and the transaction never claims completion.
    let second = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    let RollbackResult::RollbackFailed { failed } = &second.result else {
        panic!("{:?}", second.result);
    };
    assert!(
        failed[0].1.contains("the link still matches its record"),
        "{failed:?}"
    );
    assert!(
        failed[0].1.contains("not retried automatically"),
        "{failed:?}"
    );
    assert_eq!(m.tx.entries[1].state, EntryState::RollbackFailed);
    assert!(!m.tx.is_fully_rolled_back());
    assert!(std::fs::symlink_metadata(m.sym.join("s.bin")).is_ok());
}

/// Refuses the symbolic link's first undo by occupying its path differently,
/// then returns the refusal of a second, guarded attempt.
#[cfg(unix)]
fn link_retry_refusal(m: &mut Mixed) -> String {
    let first = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert!(
        matches!(first.result, RollbackResult::PartiallyRolledBack { .. }),
        "{:?}",
        first.result
    );
    assert_eq!(m.tx.entries[1].state, EntryState::RollbackFailed);
    let second = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    let RollbackResult::RollbackFailed { failed } = &second.result else {
        panic!("{:?}", second.result);
    };
    assert_eq!(failed.len(), 1);
    assert!(failed[0].1.contains("needs review"), "{failed:?}");
    assert!(failed[0].1.contains("nothing was changed"), "{failed:?}");
    assert_eq!(m.tx.entries[1].state, EntryState::RollbackFailed);
    assert!(!m.tx.is_fully_rolled_back());
    failed[0].1.clone()
}

#[cfg(unix)]
#[test]
fn a_link_whose_target_changed_is_never_removed_by_a_retry() {
    let mut m = applied_mixed();
    let link = m.sym.join("s.bin");
    let elsewhere = m.roms.join("unrelated.bin");
    std::fs::write(&elsewhere, b"unrelated").unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    let refusal = link_retry_refusal(&mut m);
    assert!(refusal.contains("points somewhere else"), "{refusal}");
    assert_eq!(std::fs::read_link(&link).unwrap(), elsewhere);
    assert_eq!(std::fs::read(&elsewhere).unwrap(), b"unrelated");
}

#[cfg(unix)]
#[test]
fn a_link_replaced_by_a_normal_file_is_never_removed_by_a_retry() {
    let mut m = applied_mixed();
    let link = m.sym.join("s.bin");
    std::fs::remove_file(&link).unwrap();
    std::fs::write(&link, b"a person's own file").unwrap();
    let refusal = link_retry_refusal(&mut m);
    assert!(refusal.contains("a normal file now occupies"), "{refusal}");
    assert_eq!(std::fs::read(&link).unwrap(), b"a person's own file");
}

#[cfg(unix)]
#[test]
fn a_missing_link_stays_needs_review_and_is_never_claimed_undone() {
    let mut m = applied_mixed();
    std::fs::remove_file(m.sym.join("s.bin")).unwrap();
    let refusal = link_retry_refusal(&mut m);
    assert!(refusal.contains("recorded link is missing"), "{refusal}");
    assert!(m.roms.join("s.bin").exists());
}

#[cfg(unix)]
#[test]
fn even_the_exact_recorded_link_is_never_removed_by_a_retry() {
    let mut m = applied_mixed();
    let link = m.sym.join("s.bin");
    let target = std::fs::read_link(&link).unwrap();
    std::fs::remove_file(&link).unwrap();
    std::fs::write(&link, b"temporary obstruction").unwrap();
    link_retry_refusal(&mut m);
    // The person removes the obstruction and the exact recorded link is back.
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let outcome = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    let RollbackResult::RollbackFailed { failed } = &outcome.result else {
        panic!("{:?}", outcome.result);
    };
    assert!(
        failed[0].1.contains("not retried automatically"),
        "{failed:?}"
    );
    assert_eq!(std::fs::read_link(&link).unwrap(), target);
    assert_eq!(std::fs::read(&target).unwrap(), b"fixture contents");
}

#[cfg(unix)]
#[test]
fn a_hard_link_retry_requires_the_recorded_inode() {
    let mut m = applied_mixed();
    let link = m.hard.join("h.bin");
    std::fs::remove_file(&link).unwrap();
    std::fs::write(&link, b"fixture contents").unwrap();
    // Reverse order reaches the hard link first, then continues.
    let first = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert!(matches!(
        first.result,
        RollbackResult::PartiallyRolledBack { .. }
    ));
    assert_eq!(m.tx.entries[2].state, EntryState::RollbackFailed);
    let second = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    let RollbackResult::RollbackFailed { failed } = &second.result else {
        panic!("{:?}", second.result);
    };
    assert!(failed[0].1.contains("identity changed"), "{failed:?}");
    assert_eq!(std::fs::read(&link).unwrap(), b"fixture contents");
    assert!(m.roms.join("h.bin").exists());
}

#[test]
fn a_copy_is_never_retried_and_the_limitation_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let journal = base.join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let source = write(&base, "c.bin");
    let destination = base.join("copy.bin");
    std::fs::copy(&source, &destination).unwrap();
    let mut tx = RenameTransaction {
        transaction_id: super::journal::new_transaction_id(2),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: 1,
        source_scan_root: base.to_string_lossy().into_owned(),
        state: TransactionState::RollbackFailed,
        entries: vec![TransactionEntry {
            original_basename: "c.bin".into(),
            proposed_basename: "copy.bin".into(),
            identity: capture_identity(&source).unwrap(),
            source_path: source.clone(),
            destination_path: destination.clone(),
            operation: TransactionOperation::CreateCopy {
                expected_source: source.clone(),
                destination_root: base.clone(),
            },
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state: EntryState::RollbackFailed,
            failure_reason: Some("an earlier refusal".into()),
            applied_at_unix: Some(1),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    super::journal::write_journal(&journal, &tx).unwrap();
    let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
    let RollbackResult::RollbackFailed { failed } = &outcome.result else {
        panic!("{:?}", outcome.result);
    };
    assert!(
        failed[0].1.contains("not supported for copies"),
        "{failed:?}"
    );
    assert!(destination.exists());
    assert!(source.exists());
}

/// Which MAME mutation workflow a fixture journal belongs to.
#[derive(Clone, Copy)]
enum MameWorkflow {
    Rebuild,
    InternalRepair,
}

/// An applied three-member journal recorded as a MAME workflow, as its Apply
/// writes it: rebuild carries its staging root, internal repair its plan digest.
fn mame_fixture(dir: &Path, workflow: MameWorkflow) -> (RenameTransaction, PathBuf, PathBuf) {
    let (mut tx, journal) = apply_many(dir, &["a.bin", "b.bin", "c.bin"]);
    match workflow {
        MameWorkflow::Rebuild => {
            let staging = dir.join("staging");
            std::fs::create_dir_all(&staging).unwrap();
            tx.unknown.insert(
                "workflow".into(),
                serde_json::Value::String(
                    crate::dat::mame_merged_reconstruction::MAME_RECONSTRUCTION_WORKFLOW.into(),
                ),
            );
            tx.source_scan_root = staging.to_string_lossy().into_owned();
        }
        MameWorkflow::InternalRepair => {
            tx.unknown.insert(
                "mame_internal_repair_plan_digest".into(),
                serde_json::json!("synthetic-digest"),
            );
        }
    }
    // The workflow is recorded from the first sealed write, as its Apply does.
    tx.transaction_id = format!("{}-tagged", tx.transaction_id);
    super::journal::write_journal(&journal, &tx).unwrap();
    (tx, journal, dir.join("roms"))
}

fn entry_states(tx: &RenameTransaction) -> Vec<EntryState> {
    tx.entries.iter().map(|entry| entry.state).collect()
}

/// A blocked member stays needs-review, the pass continues past it, and the
/// partial progress is kept without History claiming a rollback.
#[test]
fn mame_undo_blocked_member_stays_needs_review_and_the_pass_continues() {
    for workflow in [MameWorkflow::Rebuild, MameWorkflow::InternalRepair] {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal, roms) = mame_fixture(dir.path(), workflow);
        std::fs::write(roms.join("b.bin"), b"someone else's file").unwrap();
        let first = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        let RollbackResult::PartiallyRolledBack {
            rolled_back,
            failed,
        } = &first.result
        else {
            panic!("{:?}", first.result);
        };
        assert_eq!(rolled_back.len(), 2, "{rolled_back:?}");
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert_eq!(failed[0].0, roms.join("b.bin"));
        assert_eq!(
            entry_states(&tx),
            vec![
                EntryState::RolledBack,
                EntryState::RollbackFailed,
                EntryState::RolledBack
            ]
        );
        assert_eq!(tx.state, TransactionState::RollbackFailed);
        assert!(!tx.is_fully_rolled_back());
        assert_eq!(
            std::fs::read(roms.join("b.bin")).unwrap(),
            b"someone else's file",
            "the unrelated file is never overwritten"
        );
    }
}

/// After the obstruction is cleared, a later Undo retries the needs-review
/// member and only then marks the whole transaction rolled back.
#[test]
fn mame_undo_retries_after_the_obstruction_is_cleared_and_completes() {
    for workflow in [MameWorkflow::Rebuild, MameWorkflow::InternalRepair] {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal, roms) = mame_fixture(dir.path(), workflow);
        std::fs::write(roms.join("b.bin"), b"someone else's file").unwrap();
        rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        assert!(!tx.is_fully_rolled_back());
        std::fs::remove_file(roms.join("b.bin")).unwrap();
        let second = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        assert_eq!(second.result, RollbackResult::FullyRolledBack);
        assert!(tx.is_fully_rolled_back());
        assert_eq!(tx.state, TransactionState::RolledBack);
        assert_eq!(
            std::fs::read(roms.join("b.bin")).unwrap(),
            b"fixture contents"
        );
        assert!(!roms.join("B.bin").exists());
    }
}

/// A rebuilt file that no longer matches its recorded identity is refused with
/// the exact blocker, left in place and kept needs-review.
#[test]
fn mame_undo_refuses_a_changed_rebuilt_file_even_after_the_obstruction_is_cleared() {
    for workflow in [MameWorkflow::Rebuild, MameWorkflow::InternalRepair] {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal, roms) = mame_fixture(dir.path(), workflow);
        std::fs::write(roms.join("b.bin"), b"someone else's file").unwrap();
        rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        std::fs::remove_file(roms.join("b.bin")).unwrap();
        std::fs::remove_file(roms.join("B.bin")).unwrap();
        std::fs::write(roms.join("B.bin"), b"a different file now").unwrap();
        let second = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        let RollbackResult::RollbackFailed { failed } = &second.result else {
            panic!("{:?}", second.result);
        };
        assert!(failed[0].1.contains("identity changed"), "{failed:?}");
        assert_eq!(tx.entries[1].state, EntryState::RollbackFailed);
        assert!(!tx.is_fully_rolled_back());
        assert!(!roms.join("b.bin").exists());
        assert_eq!(
            std::fs::read(roms.join("B.bin")).unwrap(),
            b"a different file now"
        );
    }
}

/// The journal fence is a universal precondition: a journal changed before its first Undo is
/// refused on every attempt without any write or move.
#[test]
fn mame_internal_repair_undo_refuses_a_tampered_journal_without_any_write() {
    for tamper in ["edited", "deleted", "older"] {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin"]);
        let roms = dir.path().join("roms");
        tx.unknown.insert(
            "mame_internal_repair_plan_digest".into(),
            serde_json::json!("synthetic-digest"),
        );
        // The workflow is recorded from the first sealed write, as its Apply does.
        tx.transaction_id = format!("{}-tagged", tx.transaction_id);
        write_journal(&journal, &tx).unwrap();
        let path = journal_path(&journal, &tx.transaction_id).unwrap();
        let session = tx.clone();
        match tamper {
            "edited" => {
                let mut recorded = read_journal(&path).unwrap();
                recorded.entries[0].destination_path = roms.join("Elsewhere.bin");
                std::fs::write(&path, serde_json::to_string(&recorded).unwrap()).unwrap();
            }
            "deleted" => std::fs::remove_file(&path).unwrap(),
            _ => {
                // Written outside EmuWiz: never a sealed generation.
                let mut older = tx.clone();
                older.state = TransactionState::Applying;
                std::fs::write(&path, serde_json::to_string(&older).unwrap()).unwrap();
            }
        }
        let tampered = std::fs::read(&path).ok();
        for attempt in 1..=3 {
            let refused = rollback_transaction(&mut tx, &journal, &no_cancel())
                .expect_err("a tampered journal must refuse internal-repair Undo");
            assert!(
                is_journal_integrity_loss(&refused),
                "{tamper} attempt {attempt}: {refused}"
            );
            assert_eq!(std::fs::read(&path).ok(), tampered, "{tamper} {attempt}");
            assert_eq!(tx, session, "{tamper}");
            assert!(roms.join("A.bin").exists() && roms.join("B.bin").exists());
            assert!(!roms.join("a.bin").exists() && !roms.join("b.bin").exists());
        }
    }
}

/// MAME rebuild keeps refusing a changed journal through its own staging lock.
#[test]
fn mame_rebuild_undo_keeps_refusing_a_changed_journal_through_its_staging_lock() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
    let roms = dir.path().join("roms");
    let staging = dir.path().join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    tx.unknown.insert(
        "workflow".into(),
        serde_json::Value::String(
            crate::dat::mame_merged_reconstruction::MAME_RECONSTRUCTION_WORKFLOW.into(),
        ),
    );
    tx.source_scan_root = staging.to_string_lossy().into_owned();
    // The workflow is recorded from the first sealed write, as its Apply does.
    tx.transaction_id = format!("{}-tagged", tx.transaction_id);
    write_journal(&journal, &tx).unwrap();
    let path = journal_path(&journal, &tx.transaction_id).unwrap();
    let mut recorded = read_journal(&path).unwrap();
    recorded.entries[0].failure_reason = Some("edited elsewhere".into());
    std::fs::write(&path, serde_json::to_string(&recorded).unwrap()).unwrap();
    let tampered = std::fs::read(&path).unwrap();
    for _ in 0..2 {
        let refused = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap_err();
        assert!(refused.contains("MAME Undo journal changed"), "{refused}");
        assert!(!is_journal_integrity_loss(&refused), "{refused}");
        assert_eq!(std::fs::read(&path).unwrap(), tampered);
        assert!(roms.join("A.bin").exists() && !roms.join("a.bin").exists());
    }
}

/// Ordinary rename workflows, by contrast, continue and retry the same case.
#[test]
fn an_ordinary_rename_undo_continues_and_retries_the_same_case() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin", "c.bin"]);
    let roms = dir.path().join("roms");
    std::fs::write(roms.join("b.bin"), b"someone else's file").unwrap();
    rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
    assert_eq!(
        tx.entries
            .iter()
            .map(|entry| entry.state)
            .collect::<Vec<_>>(),
        vec![
            EntryState::RolledBack,
            EntryState::RollbackFailed,
            EntryState::RolledBack
        ]
    );
    std::fs::remove_file(roms.join("b.bin")).unwrap();
    let second = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
    assert_eq!(second.result, RollbackResult::FullyRolledBack);
    assert_eq!(
        std::fs::read(roms.join("b.bin")).unwrap(),
        b"fixture contents"
    );
}

#[test]
fn a_rolled_back_journal_contradicted_by_its_entries_is_refused_and_left_unchanged() {
    for contradicting in [
        EntryState::Applied,
        EntryState::Applying,
        EntryState::RollingBack,
        EntryState::RollbackFailed,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (_plan, mut tx, _) = apply_one(dir.path());
        let journal = dir.path().join("journal");
        let roms = dir.path().join("roms");
        tx.state = TransactionState::RolledBack;
        tx.entries[0].state = contradicting;
        write_journal(&journal, &tx).unwrap();
        let before = journal_bytes(&journal, &tx.transaction_id);
        let cancel = no_cancel();

        let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
        assert!(
            matches!(outcome.result, RollbackResult::RollbackFailed { .. }),
            "{contradicting:?}: {:?}",
            outcome.result
        );
        assert!(!outcome.transaction.is_fully_rolled_back());
        assert_eq!(before, journal_bytes(&journal, &tx.transaction_id));
        assert!(roms.join("b.bin").exists(), "nothing was moved");
        assert!(!roms.join("a.bin").exists());
    }
}

#[test]
fn entries_that_were_never_applied_do_not_block_a_full_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    for state in [
        EntryState::Planned,
        EntryState::PreflightPassed,
        EntryState::ApplyFailed,
        EntryState::Skipped,
        EntryState::RolledBack,
    ] {
        tx.entries[0].state = state;
        assert!(!tx.entries[0].blocks_full_rollback(), "{state:?}");
    }
    tx.state = TransactionState::RolledBack;
    assert!(tx.is_fully_rolled_back());
    tx.state = TransactionState::RollbackFailed;
    assert!(!tx.is_fully_rolled_back());
}

#[test]
fn a_completed_transaction_cannot_be_applied_twice() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();
    // Applying the same plan again finds no source at the old path.
    let error = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(error, ApplyError::NothingApproved);
}

// ---------------------------------------------------------------------------
// Cancellation during rollback: never report a cancelled rollback as fully
// complete, and leave the remaining Applied entries retryable.
// ---------------------------------------------------------------------------

/// Applies three renames and then starts a rollback that is cancelled after the
/// first reverse rename (the highest-index entry, restored first), returning
/// the rollback outcome.
///
/// The rollback runs on a helper thread; this thread busy-waits for the first
/// reverse rename to land (the original path of the last-applied entry
/// reappears) and flips cancellation from the durable `RollingBack`/journal
/// checkpoint window, so exactly the first entry reverses.
fn apply_three_and_cancel_after_first_reverse_rename(
    dir: &Path,
) -> (RollbackOutcome, std::path::PathBuf) {
    apply_three_and_cancel_after_reversals(dir, 1)
}

/// Applies three renames, then rolls back with cancellation raised right after
/// `cancel_after` reverse renames (1..=2 leave the transaction partial).
fn apply_three_and_cancel_after_reversals(
    dir: &Path,
    cancel_after: usize,
) -> (RollbackOutcome, std::path::PathBuf) {
    let roms = dir.join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let a = write(&roms, "a.bin");
    let b = write(&roms, "b.bin");
    let c = write(&roms, "c.bin");
    let journal = dir.join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let proposals = vec![
        proposal(
            a.to_str().unwrap(),
            "a.bin",
            "A.bin",
            ProposalState::Suggested,
        ),
        proposal(
            b.to_str().unwrap(),
            "b.bin",
            "B.bin",
            ProposalState::Suggested,
        ),
        proposal(
            c.to_str().unwrap(),
            "c.bin",
            "C.bin",
            ProposalState::Suggested,
        ),
    ];
    let plan = plan(proposals, 1, &roms);
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&a, &b, &c]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, 3, "three renames applied");

    let mut tx = outcome.transaction;
    // Cancel at an exact, deterministic point: the hook fires on this thread
    // immediately after the `cancel_after`-th reversal, so the next loop
    // iteration sees the flag. This replaces a second thread polling the
    // filesystem and racing the worker, which on a loaded runner could observe
    // the first reversal late and let a second (or third) one through, making
    // "exactly one entry reversed" a scheduling guess rather than a fact.
    let cancel = Arc::new(AtomicBool::new(false));
    let hook_cancel = cancel.clone();
    super::rollback::AFTER_REVERSAL.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |reversed| {
            if reversed == cancel_after {
                hook_cancel.store(true, Ordering::Relaxed);
            }
        }));
    });
    let result = rollback_transaction(&mut tx, &journal, &cancel);
    super::rollback::AFTER_REVERSAL.with(|hook| *hook.borrow_mut() = None);
    (result.unwrap(), journal)
}

#[test]
fn cancelled_rollback_before_the_first_step_is_never_full() {
    // CONFIRMED BUG: cancellation stopped the rollback loop before any reverse
    // rename; the destination stayed applied, the original path stayed absent,
    // and yet the old code reported FullyRolledBack + TransactionState::RolledBack
    // because `failed` was empty.
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    let cancel = cancelled(); // cancellation is already true
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();

    assert_ne!(
        outcome.result,
        RollbackResult::FullyRolledBack,
        "a cancelled rollback is not a full rollback"
    );
    assert_ne!(
        outcome.transaction.state,
        TransactionState::RolledBack,
        "a cancelled rollback must stay honestly incomplete"
    );
    assert_eq!(
        outcome.transaction.entries[0].state,
        EntryState::Applied,
        "the applied entry is untouched and still eligible"
    );
    let roms = dir.path().join("roms");
    assert!(
        roms.join("b.bin").exists(),
        "the destination remains applied"
    );
    assert!(
        !roms.join("a.bin").exists(),
        "the original path is still absent"
    );
    // The incomplete rollback is recorded in the durable journal.
    let reloaded =
        read_journal(&journal_path(&journal, &outcome.transaction.transaction_id).unwrap())
            .unwrap();
    assert_eq!(reloaded.state, TransactionState::RollbackFailed);
    assert_eq!(reloaded.entries[0].state, EntryState::Applied);
    assert!(reloaded.entries[0].is_eligible_for_rollback());
}

#[test]
fn cancelled_mid_rollback_is_partial_with_remaining_applied() {
    let dir = tempfile::tempdir().unwrap();
    let (outcome, _journal) = apply_three_and_cancel_after_first_reverse_rename(dir.path());
    let roms = dir.path().join("roms");

    let rolled_back = outcome.result.rolled_back_paths();
    assert_eq!(
        rolled_back.len(),
        1,
        "exactly one entry reversed: {rolled_back:?}"
    );
    assert!(
        matches!(outcome.result, RollbackResult::PartiallyRolledBack { .. }),
        "a cancelled mid-batch rollback is partial: {:?}",
        outcome.result
    );
    assert_ne!(outcome.transaction.state, TransactionState::RolledBack);
    assert_eq!(
        outcome.transaction.state,
        TransactionState::RollbackFailed,
        "the transaction remains honestly incomplete"
    );

    let rolled_back: Vec<PathBuf> = outcome
        .transaction
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::RolledBack)
        .map(|entry| entry.source_path.clone())
        .collect();
    assert_eq!(rolled_back.len(), 1, "exactly one entry is RolledBack");
    let remaining: Vec<&str> = outcome
        .transaction
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .map(|entry| entry.source_path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(remaining, vec!["a.bin", "b.bin"], "the rest stay Applied");

    assert!(
        roms.join("c.bin").exists(),
        "the reversed entry is restored"
    );
    assert!(roms.join("B.bin").exists(), "still applied");
    assert!(roms.join("A.bin").exists(), "still applied");
    assert!(!roms.join("b.bin").exists());
    assert!(!roms.join("a.bin").exists());

    // The remaining Applied entries are still eligible for a later rollback.
    let eligible: Vec<&str> = outcome
        .transaction
        .entries
        .iter()
        .filter(|entry| entry.is_eligible_for_rollback())
        .map(|entry| entry.source_path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(eligible, vec!["a.bin", "b.bin"]);
}

#[test]
fn cancelled_after_two_reversals_is_partial_with_the_last_entry_still_applied() {
    // The outcome CI once produced by accident (two entries reversed before the
    // cancel landed) is a valid partial rollback and must be reported exactly.
    let dir = tempfile::tempdir().unwrap();
    let (outcome, _journal) = apply_three_and_cancel_after_reversals(dir.path(), 2);
    let roms = dir.path().join("roms");

    assert_eq!(outcome.result.rolled_back_paths().len(), 2);
    assert!(matches!(
        outcome.result,
        RollbackResult::PartiallyRolledBack { .. }
    ));
    assert_eq!(outcome.transaction.state, TransactionState::RollbackFailed);
    let applied: Vec<&str> = outcome
        .transaction
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .map(|entry| entry.source_path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(applied, vec!["a.bin"]);
    assert!(roms.join("c.bin").exists() && roms.join("b.bin").exists());
    assert!(roms.join("A.bin").exists() && !roms.join("a.bin").exists());
}

#[test]
fn rollback_without_cancellation_stays_fully_rolled_back() {
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    let cancel = no_cancel();
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    assert_eq!(outcome.transaction.state, TransactionState::RolledBack);
    assert_eq!(outcome.transaction.entries[0].state, EntryState::RolledBack);
}

#[test]
fn cancellation_after_completion_still_reports_fully_rolled_back() {
    // Cancellation that arrives only after every reverse rename already ran
    // must not downgrade a genuine full rollback.
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    let cancel = no_cancel();
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    assert_eq!(outcome.transaction.state, TransactionState::RolledBack);
    // Cancel afterwards: nothing remains Applied, so full rollback stays true.
    cancel.store(true, Ordering::Relaxed);
    let second = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert_eq!(second.result, RollbackResult::FullyRolledBack);
    assert_eq!(second.transaction.state, TransactionState::RolledBack);
}

#[test]
fn repeated_rollback_after_cancellation_finishes_the_remaining_entries() {
    let dir = tempfile::tempdir().unwrap();
    let (outcome, journal) = apply_three_and_cancel_after_first_reverse_rename(dir.path());
    let roms = dir.path().join("roms");

    // Already-RolledBack entries are not touched again; the remaining Applied
    // entries finish safely on a fresh (uncancelled) retry.
    let mut tx = outcome.transaction;
    let retry_cancel = no_cancel();
    let final_outcome = rollback_transaction(&mut tx, &journal, &retry_cancel).unwrap();
    assert_eq!(final_outcome.result, RollbackResult::FullyRolledBack);
    assert_eq!(
        final_outcome.transaction.state,
        TransactionState::RolledBack
    );

    for name in ["a.bin", "b.bin", "c.bin"] {
        assert_eq!(
            std::fs::read(roms.join(name)).unwrap(),
            b"fixture contents",
            "{name} restored exactly once"
        );
    }
    for name in ["A.bin", "B.bin", "C.bin"] {
        assert!(!roms.join(name).exists(), "{name} gone after rollback");
    }
}

#[test]
fn crash_reconciled_applied_entry_with_cancellation_is_never_full() {
    // A crash-reconciled Applying -> Applied entry must not be reported as
    // fully rolled back when cancellation stops the rollback before reversal.
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let mut tx = build_transaction(&plan, &approved, 1).unwrap();
    let destination = tx.entries[0].destination_path.clone();
    tx.entries[0].state = ApplyEntryState::Applying;
    write_journal(&journal, &tx).unwrap();
    // The rename happened but the journal was never updated: the crash.
    super::noclobber::rename_noreplace(&source, &destination).unwrap();

    let issues = reconcile_recovery(&mut tx, &journal).unwrap();
    assert_eq!(tx.entries[0].state, ApplyEntryState::Applied);
    assert_eq!(
        issues[0].kind,
        super::reconcile::RecoveryIssueKind::RenameConfirmed
    );

    // Cancellation before the reverse rename.
    let cancel = cancelled();
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert_ne!(outcome.result, RollbackResult::FullyRolledBack);
    assert_ne!(outcome.transaction.state, TransactionState::RolledBack);
    assert_eq!(
        outcome.transaction.entries[0].state,
        ApplyEntryState::Applied
    );
    assert!(destination.exists(), "still applied");
    assert!(!source.exists(), "original path still absent");
}

#[test]
fn cancelled_rollback_persists_durably_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (_plan, mut tx, _) = apply_one(dir.path());
    let journal = dir.path().join("journal");
    let cancel = cancelled();
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert_ne!(outcome.result, RollbackResult::FullyRolledBack);
    assert_eq!(outcome.transaction.state, TransactionState::RollbackFailed);

    // A restart reloads the journal: the incomplete rollback is durable and
    // the remaining Applied entry is still offered for rollback.
    let transaction_id = outcome.transaction.transaction_id.clone();
    let reloaded = read_journal(&journal_path(&journal, &transaction_id).unwrap()).unwrap();
    assert_eq!(reloaded.state, TransactionState::RollbackFailed);
    assert_eq!(reloaded.entries[0].state, EntryState::Applied);
    assert!(reloaded.entries[0].is_eligible_for_rollback());
    let (recovery, _) = find_recovery_transactions(&journal);
    assert_eq!(
        recovery.len(),
        1,
        "incomplete rollback is surfaced for recovery"
    );

    // Retrying from the reloaded journal completes the rollback.
    let mut retry = reloaded;
    let retry_cancel = no_cancel();
    let final_outcome = rollback_transaction(&mut retry, &journal, &retry_cancel).unwrap();
    assert_eq!(final_outcome.result, RollbackResult::FullyRolledBack);
    assert_eq!(
        final_outcome.transaction.state,
        TransactionState::RolledBack
    );
    let roms = dir.path().join("roms");
    assert!(roms.join("a.bin").exists());
    assert!(!roms.join("b.bin").exists());
}

// ---------------------------------------------------------------------------
// No-clobber and content-integrity proofs
// ---------------------------------------------------------------------------

#[test]
fn content_is_identical_through_apply_and_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let before = std::fs::read(&source).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();
    let after_apply = std::fs::read(roms.join("b.bin")).unwrap();
    assert_eq!(after_apply, before, "bytes unchanged through rename");

    let mut tx = outcome.transaction;
    rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    let after_rollback = std::fs::read(roms.join("a.bin")).unwrap();
    assert_eq!(after_rollback, before, "bytes unchanged through rollback");
}

#[test]
fn a_failed_preflight_leaves_all_files_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    write(&roms, "b.bin"); // destination exists -> hard conflict
    let before = snapshot(&roms);
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    let error = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert!(matches!(error, ApplyError::HardConflicts(_)));
    assert_eq!(
        snapshot(&roms),
        before,
        "a failed preflight changes nothing"
    );
}

#[test]
fn rename_cannot_escape_the_source_directory() {
    // Same-directory is enforced structurally: the destination is built from
    // the source's parent. A traversal-tainted proposed name is rejected by
    // the safe-basename check in preflight.
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    // A hostile proposed name with a path separator.
    let p = proposal(
        source.to_str().unwrap(),
        "a.bin",
        "../escape.bin",
        ProposalState::Suggested,
    );
    // destination_path would be parent.join("../escape.bin") - but preflight
    // rejects the unsafe basename before anything happens.
    let plan = plan(vec![p], 1, &roms);
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, 0, "traversal names never escape");
    assert!(source.exists());
    assert!(!dir.path().join("escape.bin").exists());
}

#[test]
fn broken_symlink_substitution_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    // Hostile change: replace the source with a broken symlink.
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(dir.path().join("nowhere.bin"), &source).unwrap();
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::SkipUnsafeSubset,
        &cancel,
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, 0);
}

#[test]
fn a_symlink_loop_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let a = roms.join("a.bin");
    let b = roms.join("b.bin");
    // a -> b, b -> a : a symlink loop.
    std::os::unix::fs::symlink(&b, &a).unwrap();
    std::os::unix::fs::symlink(&a, &b).unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let mut p = proposal(
        a.to_str().unwrap(),
        "a.bin",
        "renamed.bin",
        ProposalState::Suggested,
    );
    p.object_kind = SourceObjectKind::Symlink;
    let plan = plan(vec![p], 1, &roms);
    let cancel = no_cancel();
    let error = apply(
        &plan,
        approved_of(&[&a]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(
        error,
        ApplyError::NothingApproved,
        "symlink loops are never applicable"
    );
}

#[test]
fn a_path_traversal_proposed_name_is_blocked_by_preflight() {
    // Preflight's safe-basename check (plus derive-time blocking in the plan)
    // rejects any proposed name that is not a single safe component.
    assert!(super::preflight::is_safe_basename("Game (Europe).hdf"));
    assert!(!super::preflight::is_safe_basename("../escape.hdf"));
    assert!(!super::preflight::is_safe_basename("a/b.hdf"));
    assert!(!super::preflight::is_safe_basename(".."));
    assert!(!super::preflight::is_safe_basename(""));
}

// ---------------------------------------------------------------------------
// Repeated stress runs (destination race, cancellation, rollback, recovery)
// ---------------------------------------------------------------------------

#[test]
fn stress_destination_creation_race_never_overwrites() {
    // Run the "destination appears after review" hostile case repeatedly; the
    // destination must never be overwritten and the source must never move.
    for _ in 0..25 {
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        std::fs::create_dir_all(&roms).unwrap();
        let source = write(&roms, "a.bin");
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let plan = plan(
            vec![proposal(
                source.to_str().unwrap(),
                "a.bin",
                "b.bin",
                ProposalState::Suggested,
            )],
            1,
            &roms,
        );
        let approved = approved_of(&[&source]);
        let tx = build_transaction(&plan, &approved, 1).unwrap();
        write(&roms, "b.bin"); // destination appears after review
        let cancel = no_cancel();
        let outcome = apply_exec(
            tx,
            approved,
            TrustedRoots::from_paths([&roms]),
            &journal,
            HardConflictMode::SkipUnsafeSubset,
            &cancel,
            1,
        )
        .unwrap();
        assert_eq!(outcome.summary.applied, 0);
        assert!(source.exists(), "iteration: source must not move");
        assert_eq!(
            std::fs::read(roms.join("b.bin")).unwrap(),
            b"fixture contents"
        );
    }
}

#[test]
fn stress_cancellation_leaves_everything_untouched() {
    for _ in 0..25 {
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        std::fs::create_dir_all(&roms).unwrap();
        let source = write(&roms, "a.bin");
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let plan = plan(
            vec![proposal(
                source.to_str().unwrap(),
                "a.bin",
                "b.bin",
                ProposalState::Suggested,
            )],
            1,
            &roms,
        );
        let cancel = cancelled();
        let error = apply(
            &plan,
            approved_of(&[&source]),
            TrustedRoots::from_paths([&roms]),
            &journal,
            HardConflictMode::AbortAll,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error, ApplyError::Cancelled);
        assert!(source.exists());
        assert!(!roms.join("b.bin").exists());
    }
}

#[test]
fn stress_apply_and_rollback_round_trip_preserves_bytes() {
    for _ in 0..25 {
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        std::fs::create_dir_all(&roms).unwrap();
        let source = write(&roms, "a.bin");
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let before = std::fs::read(&source).unwrap();
        let plan = plan(
            vec![proposal(
                source.to_str().unwrap(),
                "a.bin",
                "b.bin",
                ProposalState::Suggested,
            )],
            1,
            &roms,
        );
        let cancel = no_cancel();
        let outcome = apply(
            &plan,
            approved_of(&[&source]),
            TrustedRoots::from_paths([&roms]),
            &journal,
            HardConflictMode::AbortAll,
            &cancel,
        )
        .unwrap();
        assert_eq!(std::fs::read(roms.join("b.bin")).unwrap(), before);
        let mut tx = outcome.transaction;
        rollback_transaction(&mut tx, &journal, &cancel).unwrap();
        assert_eq!(std::fs::read(roms.join("a.bin")).unwrap(), before);
        assert!(!roms.join("b.bin").exists());
    }
}

#[test]
fn stress_crash_recovery_fixtures_are_detected() {
    for _ in 0..25 {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let mut tx = RenameTransaction {
            transaction_id: "stress-recovery".to_string(),
            plan_generation: 1,
            classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
            created_at_unix: 1,
            source_scan_root: "/tmp/roms".to_string(),
            state: TransactionState::Applying,
            entries: Vec::new(),
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        };
        tx.entries.push(TransactionEntry {
            source_path: PathBuf::from("/tmp/roms/a.bin"),
            destination_path: PathBuf::from("/tmp/roms/b.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "b.bin".to_string(),
            identity: ObjectIdentity {
                freshness: None,
                size_bytes: 1,
                modified_unix: 1,
                kind: ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            operation: Default::default(),
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: EntryState::Applied,
            failure_reason: None,
            applied_at_unix: Some(2),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        });
        write_journal(&journal, &tx).unwrap();
        let (recovery, problems) = find_recovery_transactions(&journal);
        assert!(problems.is_empty());
        assert_eq!(recovery.len(), 1);
        assert_eq!(recovery[0].applied_count(), 1);
    }
}

// ---------------------------------------------------------------------------
// Crash-window regression tests
// ---------------------------------------------------------------------------

use crate::dat::rename_apply::{EntryState as ApplyEntryState, reconcile_recovery};

#[test]
fn crash_before_the_rename_syscall_is_reconciled_as_not_applied() {
    // The durable journal says Applying, but the process died before the
    // syscall: source intact, destination absent. Recovery must conclude the
    // rename did not happen and mutate nothing.
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let before = snapshot(&roms);
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let mut tx = build_transaction(&plan, &approved, 1).unwrap();
    tx.entries[0].state = ApplyEntryState::Applying;
    write_journal(&journal, &tx).unwrap();

    let issues = reconcile_recovery(&mut tx, &journal).unwrap();
    assert_eq!(tx.entries[0].state, ApplyEntryState::Skipped, "not applied");
    assert_eq!(tx.applied_count(), 0);
    assert_eq!(
        issues[0].kind,
        super::reconcile::RecoveryIssueKind::RenameDidNotHappen
    );
    assert_eq!(snapshot(&roms), before, "recovery mutates nothing");
}

#[test]
fn crash_after_the_rename_syscall_is_reconciled_as_applied_and_rollback_restores() {
    // CONFIRMED BUG: the durable journal says Applying, the real production
    // rename_noreplace already happened, and the process died before the
    // Applied state was persisted. Without reconciliation the journal would
    // report applied_count 0 and rollback would leave the file stranded.
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let contents = std::fs::read(&source).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let mut tx = build_transaction(&plan, &approved, 1).unwrap();
    let destination = tx.entries[0].destination_path.clone();
    tx.entries[0].state = ApplyEntryState::Applying;
    write_journal(&journal, &tx).unwrap();

    // Real production mutation (the syscall the executor would have made).
    super::noclobber::rename_noreplace(&source, &destination).unwrap();
    // No journal write after the syscall: this is the crash.

    // The raw journal is the bug: applied_count 0 while the file is renamed.
    assert_eq!(
        tx.applied_count(),
        0,
        "the raw journal is the confirmed-bug state"
    );

    let issues = reconcile_recovery(&mut tx, &journal).unwrap();
    assert_eq!(
        tx.applied_count(),
        1,
        "recovery recognises the confirmed rename"
    );
    assert_eq!(tx.entries[0].state, ApplyEntryState::Applied);
    assert_eq!(
        issues[0].kind,
        super::reconcile::RecoveryIssueKind::RenameConfirmed
    );
    assert!(!source.exists());
    assert!(destination.exists());

    // Rollback must now restore the original path and bytes.
    let cancel = no_cancel();
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    assert!(source.exists());
    assert!(!destination.exists());
    assert_eq!(std::fs::read(&source).unwrap(), contents);
}

#[test]
fn crash_after_syscall_with_wrong_destination_identity_is_not_classified_as_applied() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let mut tx = build_transaction(&plan, &approved, 1).unwrap();
    let destination = tx.entries[0].destination_path.clone();
    tx.entries[0].state = ApplyEntryState::Applying;
    write_journal(&journal, &tx).unwrap();

    // The rename happened, then the destination was replaced with a different
    // object before recovery ran.
    super::noclobber::rename_noreplace(&source, &destination).unwrap();
    std::fs::remove_file(&destination).unwrap();
    std::fs::write(&destination, b"replaced").unwrap();

    let issues = reconcile_recovery(&mut tx, &journal).unwrap();
    assert_eq!(
        tx.entries[0].state,
        ApplyEntryState::Applying,
        "left unresolved"
    );
    assert_eq!(tx.applied_count(), 0);
    assert_eq!(
        issues[0].kind,
        super::reconcile::RecoveryIssueKind::DestinationIdentityChanged
    );
    // Rollback refuses to guess.
    let cancel = no_cancel();
    let outcome = rollback_transaction(&mut tx, &journal, &cancel).unwrap();
    assert!(matches!(
        outcome.result,
        RollbackResult::RollbackFailed { .. }
    ));
    assert_eq!(std::fs::read(&destination).unwrap(), b"replaced");
}

#[test]
fn applying_with_both_present_is_refused_not_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let mut tx = build_transaction(&plan, &approved, 1).unwrap();
    let destination = tx.entries[0].destination_path.clone();
    tx.entries[0].state = ApplyEntryState::Applying;
    write_journal(&journal, &tx).unwrap();
    // The same object appears at both paths (a hard link) - indeterminate.
    std::fs::hard_link(&source, &destination).unwrap();

    let issues = reconcile_recovery(&mut tx, &journal).unwrap();
    assert_eq!(
        tx.entries[0].state,
        ApplyEntryState::Applying,
        "left unresolved"
    );
    assert_eq!(
        issues[0].kind,
        super::reconcile::RecoveryIssueKind::BothSourceAndDestination
    );
    assert!(source.exists());
    assert!(destination.exists());
}

#[test]
fn applying_with_both_absent_reports_manual_review() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let mut tx = build_transaction(&plan, &approved, 1).unwrap();
    tx.entries[0].state = ApplyEntryState::Applying;
    write_journal(&journal, &tx).unwrap();
    // Both paths vanished externally.
    std::fs::remove_file(&source).unwrap();

    let issues = reconcile_recovery(&mut tx, &journal).unwrap();
    assert_eq!(
        tx.entries[0].state,
        ApplyEntryState::Applying,
        "left unresolved"
    );
    assert_eq!(
        issues[0].kind,
        super::reconcile::RecoveryIssueKind::BothAbsent
    );
}

#[test]
fn journal_write_failure_before_the_syscall_prevents_any_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    // The journal "directory" is actually a file, so every write fails.
    let journal = dir.path().join("journal");
    std::fs::write(&journal, b"not a directory").unwrap();
    let before = snapshot(&roms);
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let cancel = no_cancel();
    let error = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();
    assert!(matches!(error, ApplyError::Journal(_)), "{error:?}");
    assert_eq!(
        snapshot(&roms),
        before,
        "a journal failure must mean zero mutation"
    );
}

#[test]
fn crash_during_rollback_is_reconciled_honestly() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "a.bin");
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let contents = std::fs::read(&source).unwrap();
    let plan = plan(
        vec![proposal(
            source.to_str().unwrap(),
            "a.bin",
            "b.bin",
            ProposalState::Suggested,
        )],
        1,
        &roms,
    );
    let approved = approved_of(&[&source]);
    let tx = build_transaction(&plan, &approved, 1).unwrap();
    let destination = tx.entries[0].destination_path.clone();
    // Apply for real.
    let cancel = no_cancel();
    let outcome = apply_exec(
        tx,
        approved,
        TrustedRoots::from_paths(std::slice::from_ref(&roms)),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
        1,
    )
    .unwrap();
    let mut tx = outcome.transaction;

    // Simulate a crash during rollback: the durable RollingBack checkpoint was
    // written, the reverse rename happened, but RolledBack was never persisted.
    tx.entries[0].state = ApplyEntryState::RollingBack;
    write_journal(&journal, &tx).unwrap();
    super::noclobber::rename_noreplace(&destination, &source).unwrap();
    // No journal write after the reverse rename: the crash.

    let issues = reconcile_recovery(&mut tx, &journal).unwrap();
    assert_eq!(tx.entries[0].state, ApplyEntryState::RolledBack);
    assert_eq!(
        issues[0].kind,
        super::reconcile::RecoveryIssueKind::RenameDidNotHappen
    );
    assert!(source.exists());
    assert!(!destination.exists());
    assert_eq!(std::fs::read(&source).unwrap(), contents);
}

// ---------------------------------------------------------------------------
// Outer archive rename: the executor is content-agnostic, so a proposal
// pointing at a real .zip file is applied and rolled back exactly like any
// other regular file - these tests prove that with a real archive rather
// than an arbitrary fixture, and prove its bytes and member list survive
// completely untouched.
// ---------------------------------------------------------------------------

/// Writes a real, valid, multi-member ZIP to `path` and returns its exact
/// bytes and its member name list, for later identity comparison.
fn write_real_zip(path: &Path) -> (Vec<u8>, Vec<String>) {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    let mut writer = ZipWriter::new(std::fs::File::create(path).unwrap());
    let members = ["game.cue", "game (Track 1).bin", "game (Track 2).bin"];
    for name in members {
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"member contents").unwrap();
    }
    writer.finish().unwrap();
    let bytes = std::fs::read(path).unwrap();
    (bytes, members.iter().map(|name| name.to_string()).collect())
}

/// The exact member-name list of a ZIP at `path`, in archive order.
fn zip_member_names(path: &Path) -> Vec<String> {
    let file = std::fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .unwrap()
                .name()
                .unwrap()
                .into_owned()
        })
        .collect()
}

fn outer_archive_proposal(source: &str, current: &str, proposed: &str) -> RenameProposal {
    let mut proposal = proposal(source, current, proposed, ProposalState::Suggested);
    proposal.rom_name = None;
    proposal.game_name = Some("Sonic the Hedgehog (USA, Europe)".to_string());
    proposal.verdict_label = "Set complete".to_string();
    proposal.audited_identity = capture_identity(Path::new(source)).ok();
    proposal.is_outer_archive = true;
    proposal
}

#[test]
fn outer_transaction_construction_refuses_an_object_replaced_after_planning() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = write(&roms, "old.zip");
    let proposal = outer_archive_proposal(source.to_str().unwrap(), "old.zip", "Game (World).zip");
    let plan = plan(vec![proposal], 1, &roms);

    std::fs::remove_file(&source).unwrap();
    std::fs::write(&source, b"replacement archive").unwrap();

    let error = build_transaction(&plan, &approved_of(&[&source]), 1).unwrap_err();
    assert_eq!(error, ApplyError::NothingApproved);
}

#[test]
fn an_outer_zip_rename_applies_with_archive_bytes_and_members_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = roms.join("bad_old_name.zip");
    let (original_bytes, original_members) = write_real_zip(&source);
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();

    let proposals = vec![outer_archive_proposal(
        source.to_str().unwrap(),
        "bad_old_name.zip",
        "Sonic the Hedgehog (USA, Europe).zip",
    )];
    let plan = plan(proposals, 1, &roms);
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();

    assert_eq!(outcome.transaction.state, TransactionState::Applied);
    assert!(!source.exists());
    let destination = roms.join("Sonic the Hedgehog (USA, Europe).zip");
    assert!(destination.exists());

    // Bytes are exactly identical - the executor never opened the archive,
    // only renamed the path.
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        original_bytes,
        "archive bytes must be byte-for-byte identical after an outer rename"
    );
    // The member list - names, order, count - is exactly identical.
    assert_eq!(
        zip_member_names(&destination),
        original_members,
        "no inner member name may change from an outer archive rename"
    );
}

#[test]
fn an_outer_archive_rollback_restores_the_original_path_and_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = roms.join("bad_old_name.7z");
    // A real ZIP written under a `.7z` name is fine here: rollback only
    // cares about path/identity, never archive-format internals.
    let (original_bytes, original_members) = write_real_zip(&source);
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();

    let proposals = vec![outer_archive_proposal(
        source.to_str().unwrap(),
        "bad_old_name.7z",
        "Golden Axe (Europe).7z",
    )];
    let plan = plan(proposals, 1, &roms);
    let cancel = no_cancel();
    let outcome = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap();
    let mut tx = outcome.transaction;
    assert!(roms.join("Golden Axe (Europe).7z").exists());

    let rollback = rollback_transaction(&mut tx, &journal, &cancel).unwrap();

    assert_eq!(rollback.result, RollbackResult::FullyRolledBack);
    assert_eq!(rollback.transaction.state, TransactionState::RolledBack);
    assert!(source.exists(), "the original path is restored");
    assert!(!roms.join("Golden Axe (Europe).7z").exists());
    assert_eq!(
        std::fs::read(&source).unwrap(),
        original_bytes,
        "archive bytes must be byte-for-byte identical after rollback"
    );
    assert_eq!(zip_member_names(&source), original_members);
}

#[test]
fn an_outer_archive_rename_refuses_safely_on_an_existing_destination() {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    let source = roms.join("bad_old_name.zip");
    let (original_bytes, _) = write_real_zip(&source);
    // The proposed canonical name already exists.
    let (existing_bytes, _) = write_real_zip(&roms.join("Sonic the Hedgehog (USA, Europe).zip"));
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();

    let proposals = vec![outer_archive_proposal(
        source.to_str().unwrap(),
        "bad_old_name.zip",
        "Sonic the Hedgehog (USA, Europe).zip",
    )];
    let plan = plan(proposals, 1, &roms);
    let cancel = no_cancel();
    let error = apply(
        &plan,
        approved_of(&[&source]),
        TrustedRoots::from_paths([&roms]),
        &journal,
        HardConflictMode::AbortAll,
        &cancel,
    )
    .unwrap_err();

    // AbortAll: a hard conflict prevents the batch from starting at all - no
    // partial mutation, no overwrite, source untouched.
    assert!(matches!(error, ApplyError::HardConflicts(_)));
    assert!(
        source.exists(),
        "the source must not be moved on a refused rename"
    );
    assert_eq!(std::fs::read(&source).unwrap(), original_bytes);
    assert_eq!(
        std::fs::read(roms.join("Sonic the Hedgehog (USA, Europe).zip")).unwrap(),
        existing_bytes,
        "the pre-existing destination archive must not be overwritten"
    );
}

#[path = "dirsync_tests.rs"]
mod dirsync_tests;

/// A link entry journalled as "not adopted" (its link was absent at the
/// checkpoint) must never adopt an exact link that appears before its own
/// syscall: Undo would later remove a link this transaction did not create.
#[cfg(unix)]
#[test]
fn a_non_adopted_link_entry_refuses_an_exact_link_that_appeared_after_its_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let source = write(dir.path(), "game.bin");
    let destination = dir.path().join("link.bin");
    let mut entry = TransactionEntry {
        source_path: source.clone(),
        destination_path: destination.clone(),
        original_basename: "game.bin".to_string(),
        proposed_basename: "link.bin".to_string(),
        identity: capture_identity(&source).unwrap(),
        operation: TransactionOperation::CreateSymlink {
            expected_target: source.clone(),
            destination_root: dir.path().to_path_buf(),
        },
        preflight_passed: true,
        preflight_failures: Vec::new(),
        state: EntryState::Applying,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    };
    entry.unknown.insert(
        super::executor::LINK_ADOPTED_KEY.to_string(),
        serde_json::Value::Bool(false),
    );
    std::os::unix::fs::symlink(&source, &destination).unwrap();

    let result =
        super::executor::apply_mutation_in(&entry, &TrustedRoots::from_paths([dir.path()]));
    assert!(
        matches!(&result, Err((EntryState::ApplyFailed, _))),
        "a late link was adopted: {result:?}"
    );
    assert_eq!(std::fs::read_link(&destination).unwrap(), source);

    // A legacy entry without the marker cannot use matching text as proof.
    entry.unknown.clear();
    assert!(matches!(
        super::executor::apply_mutation_in(&entry, &TrustedRoots::from_paths([dir.path()])),
        Err((EntryState::ApplyFailed, _))
    ));
    // The same refusal applies when a link arrives between batch preflight
    // and the fresh entry's checkpoint.
    entry.state = EntryState::Planned;
    assert!(super::executor::record_link_adoption(&mut entry).is_err());
    assert!(
        !entry
            .unknown
            .contains_key(super::executor::LINK_ADOPTED_KEY)
    );
    assert_eq!(std::fs::read_link(&destination).unwrap(), source);
}

/// Unreadable link evidence (here ELOOP from a looping parent) is not proof the
/// link is absent: no "not adopted" decision may be journalled, otherwise a crash
/// before the syscall lets recovery confirm a pre-existing link that Undo then
/// removes.
#[cfg(unix)]
#[test]
fn unreadable_link_evidence_records_no_adoption_decision() {
    let dir = tempfile::tempdir().unwrap();
    let source = write(dir.path(), "game.bin");
    let looping = dir.path().join("loop");
    std::os::unix::fs::symlink(&looping, &looping).unwrap();
    let destination = looping.join("link.bin");
    let mut entry = TransactionEntry {
        source_path: source.clone(),
        destination_path: destination,
        original_basename: "game.bin".to_string(),
        proposed_basename: "link.bin".to_string(),
        identity: capture_identity(&source).unwrap(),
        operation: TransactionOperation::CreateSymlink {
            expected_target: source.clone(),
            destination_root: dir.path().to_path_buf(),
        },
        preflight_passed: true,
        preflight_failures: Vec::new(),
        state: EntryState::Planned,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    };

    let outcome = super::executor::record_link_adoption(&mut entry);
    assert!(outcome.is_err(), "unreadable evidence must fail the entry");
    assert!(
        !entry
            .unknown
            .contains_key(super::executor::LINK_ADOPTED_KEY),
        "an adoption decision was journalled from unreadable evidence: {outcome:?}"
    );
}

/// Re-applying an Applied link entry whose link cannot be read (ELOOP) must not
/// downgrade it to ApplyFailed: that would drop it from Undo while the link this
/// transaction created may still be in place.
#[cfg(unix)]
#[test]
fn unreadable_link_evidence_keeps_an_applied_link_entry_undoable() {
    let dir = tempfile::tempdir().unwrap();
    let source = write(dir.path(), "game.bin");
    let looping = dir.path().join("loop");
    std::os::unix::fs::symlink(&looping, &looping).unwrap();
    let mut entry = TransactionEntry {
        source_path: source.clone(),
        destination_path: looping.join("link.bin"),
        original_basename: "game.bin".to_string(),
        proposed_basename: "link.bin".to_string(),
        identity: capture_identity(&source).unwrap(),
        operation: TransactionOperation::CreateSymlink {
            expected_target: source.clone(),
            destination_root: dir.path().to_path_buf(),
        },
        preflight_passed: true,
        preflight_failures: Vec::new(),
        state: EntryState::Applying,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    };
    entry.unknown.insert(
        super::executor::LINK_ADOPTED_KEY.to_string(),
        serde_json::Value::Bool(false),
    );

    let result = super::executor::reverify_recorded_link(&entry, EntryState::Applied);
    assert!(
        matches!(&result, Some(Err((EntryState::Applied, _)))),
        "unavailable evidence changed the entry's outcome: {result:?}"
    );
}

// --- Independent journal seals and the mixed-version gate ------------------

mod sealed_journals {
    use super::*;
    use crate::dat::rename_apply::seal::{self, SealCheck};

    /// "Restart": nothing survives but the journal and its seal.
    fn reopen(journal: &Path, id: &str) -> RenameTransaction {
        read_journal(&journal_path(journal, id).unwrap()).unwrap()
    }

    #[test]
    fn apply_then_restart_then_undo_is_verified_against_the_seal() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin"]);
        assert!(matches!(
            seal::verify(&journal, &tx.transaction_id),
            SealCheck::Sealed { .. }
        ));
        let id = tx.transaction_id.clone();
        drop(tx);
        let mut reopened = reopen(&journal, &id);
        let outcome = rollback_transaction(&mut reopened, &journal, &no_cancel()).unwrap();
        assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
        assert!(matches!(
            seal::verify(&journal, &id),
            SealCheck::Sealed { .. }
        ));
    }

    /// Edited, deleted, older-replaced or swapped journals are refused after a
    /// restart, on every attempt, with nothing written or moved.
    #[test]
    fn a_journal_changed_before_restart_is_refused_on_every_attempt() {
        for tamper in ["edited", "deleted", "older", "swapped"] {
            let dir = tempfile::tempdir().unwrap();
            let (tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin"]);
            let roms = dir.path().join("roms");
            let id = tx.transaction_id.clone();
            let path = journal_path(&journal, &id).unwrap();
            // A later sealed generation exists, so an earlier copy is "older".
            let older = std::fs::read(&path).unwrap();
            std::fs::write(roms.join("a.bin"), b"blocker").unwrap();
            let mut held = tx.clone();
            rollback_transaction(&mut held, &journal, &no_cancel()).unwrap();
            std::fs::remove_file(roms.join("a.bin")).unwrap();
            match tamper {
                "edited" => {
                    let mut recorded = reopen(&journal, &id);
                    recorded.entries[0].failure_reason = Some("edited".into());
                    std::fs::write(&path, serde_json::to_string(&recorded).unwrap()).unwrap();
                }
                "deleted" => std::fs::remove_file(&path).unwrap(),
                "older" => std::fs::write(&path, &older).unwrap(),
                _ => {
                    // Another valid, sealed journal copied over this one.
                    let other_dir = dir.path().join("other");
                    std::fs::create_dir_all(&other_dir).unwrap();
                    let (other, other_journal) = apply_many(&other_dir, &["z.bin"]);
                    let bytes =
                        std::fs::read(journal_path(&other_journal, &other.transaction_id).unwrap())
                            .unwrap();
                    std::fs::write(&path, bytes).unwrap();
                }
            }
            let tampered = std::fs::read(&path).ok();
            assert!(!seal::verify(&journal, &id).trusted(), "{tamper}");
            for attempt in 1..=2 {
                // After a restart only the disk is known; it must not be trusted.
                let mut reopened = match tamper {
                    "deleted" => held.clone(),
                    _ => serde_json::from_slice(tampered.as_ref().unwrap()).unwrap(),
                };
                let refused = rollback_transaction(&mut reopened, &journal, &no_cancel())
                    .expect_err("an unsealed change must refuse Undo");
                assert!(
                    is_journal_integrity_loss(&refused),
                    "{tamper} {attempt}: {refused}"
                );
                assert_eq!(std::fs::read(&path).ok(), tampered, "{tamper} {attempt}");
                assert!(roms.join("A.bin").exists() && !roms.join("a.bin").exists());
            }
        }
    }

    /// A crash after the journal write but before the seal commit is
    /// reconciled from the durable intent; a crash before the journal write
    /// keeps the committed generation. Anything else needs review.
    #[test]
    fn a_crash_between_journal_and_seal_is_reconciled_from_durable_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin"]);
        let roms = dir.path().join("roms");
        let id = tx.transaction_id.clone();
        std::fs::write(roms.join("a.bin"), b"blocker").unwrap();
        seal::hooks::BEFORE_COMMIT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|| Err("simulated crash".into())));
        });
        let crashed = rollback_transaction(&mut tx, &journal, &no_cancel());
        assert!(crashed.is_err(), "{crashed:?}");
        assert!(matches!(
            seal::verify(&journal, &id),
            SealCheck::PendingIntent { .. }
        ));
        // Restart: the journal matches the recorded intent, so Undo proceeds.
        std::fs::remove_file(roms.join("a.bin")).unwrap();
        let mut reopened = reopen(&journal, &id);
        let outcome = rollback_transaction(&mut reopened, &journal, &no_cancel()).unwrap();
        assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
        assert!(matches!(
            seal::verify(&journal, &id),
            SealCheck::Sealed { .. }
        ));

        // A pending intent whose journal was then changed is never adopted.
        let dir = tempfile::tempdir().unwrap();
        let (tx, journal) = apply_many(dir.path(), &["c.bin"]);
        let path = journal_path(&journal, &tx.transaction_id).unwrap();
        let mut next = tx.clone();
        next.entries[0].failure_reason = Some("intended".into());
        seal::hooks::BEFORE_COMMIT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|| Err("simulated crash".into())));
        });
        assert!(write_journal(&journal, &next).is_err());
        let mut other = tx.clone();
        other.entries[0].failure_reason = Some("neither".into());
        std::fs::write(&path, serde_json::to_string(&other).unwrap()).unwrap();
        assert!(matches!(
            seal::verify(&journal, &tx.transaction_id),
            SealCheck::Mismatch(_)
        ));
        let refused = rollback_transaction(&mut other, &journal, &no_cancel()).unwrap_err();
        assert!(is_journal_integrity_loss(&refused), "{refused}");
    }

    #[test]
    fn a_lost_seal_turns_the_journal_legacy_and_it_is_never_resealed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        let path = journal_path(&journal, &tx.transaction_id).unwrap();
        seal::forget(&journal, &tx.transaction_id).unwrap();
        assert_eq!(
            seal::verify(&journal, &tx.transaction_id),
            SealCheck::Legacy
        );
        let before = std::fs::read(&path).unwrap();
        let refused = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap_err();
        assert!(refused.contains("legacy unsealed journal"), "{refused}");
        assert!(write_journal(&journal, &tx).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            seal::verify(&journal, &tx.transaction_id),
            SealCheck::Legacy
        );
    }

    #[test]
    fn an_older_emuwiz_blocks_every_journal_write_and_undo_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        let roms = dir.path().join("roms");
        let path = journal_path(&journal, &tx.transaction_id).unwrap();
        let before = std::fs::read(&path).unwrap();
        let table = super::super::writer_gate::tests::fake_table("emuwiz", 4_000_010, "9");
        super::super::writer_gate::set_process_root_for_this_thread(Some(
            table.path().to_path_buf(),
        ));
        let undo = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap_err();
        let write = write_journal(&journal, &tx).unwrap_err().to_string();
        let fresh = dir.path().join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        let mut new_tx = tx.clone();
        new_tx.transaction_id = "never-written".into();
        let create = write_journal(&fresh, &new_tx).unwrap_err().to_string();
        super::super::writer_gate::set_process_root_for_this_thread(None);
        for refusal in [&undo, &write, &create] {
            assert!(
                super::super::writer_gate::is_older_writer_refusal(refusal),
                "{refusal}"
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!fresh.join("never-written.json").exists());
        assert_eq!(
            seal::verify(&fresh, "never-written"),
            SealCheck::Absent,
            "no seal is created while an older writer runs"
        );
        assert!(roms.join("A.bin").exists() && !roms.join("a.bin").exists());
        // Once it has gone, the same Undo runs.
        let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    }

    #[test]
    fn an_older_emuwiz_blocks_undo_record_removal_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
        let path = journal_path(&journal, &tx.transaction_id).unwrap();
        let before = std::fs::read(&path).unwrap();
        let sealed = seal::verify(&journal, &tx.transaction_id);
        assert_ne!(sealed, SealCheck::Absent);
        let table = super::super::writer_gate::tests::fake_table("emuwiz", 4_000_020, "9");
        super::super::writer_gate::set_process_root_for_this_thread(Some(
            table.path().to_path_buf(),
        ));
        let refused = remove_journal(&journal, &tx.transaction_id)
            .unwrap_err()
            .to_string();
        super::super::writer_gate::set_process_root_for_this_thread(None);
        assert!(
            super::super::writer_gate::is_older_writer_refusal(&refused),
            "{refused}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(seal::verify(&journal, &tx.transaction_id), sealed);
        // Once it has gone, the same removal runs and forgets the seal.
        remove_journal(&journal, &tx.transaction_id).unwrap();
        assert!(!path.exists());
        assert_eq!(
            seal::verify(&journal, &tx.transaction_id),
            SealCheck::Absent
        );
    }

    #[test]
    fn inventory_classifies_without_writing_or_creating_anything() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, journal) = apply_many(dir.path(), &["a.bin"]);
        let legacy = RenameTransaction {
            transaction_id: "legacy-one".into(),
            ..tx.clone()
        };
        let legacy_path = journal_path(&journal, "legacy-one").unwrap();
        std::fs::write(&legacy_path, serde_json::to_string(&legacy).unwrap()).unwrap();
        let snapshot = |dir: &Path| {
            let mut files: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    (path.clone(), std::fs::read(&path).unwrap())
                })
                .collect();
            files.sort();
            files
        };
        let before = snapshot(&journal);
        let items = seal::inventory(&journal);
        assert_eq!(snapshot(&journal), before);
        let class = |id: &str| {
            items
                .iter()
                .find(|item| item.transaction_id == id)
                .unwrap()
                .seal
                .clone()
        };
        assert_eq!(class("legacy-one"), SealCheck::Legacy);
        assert!(matches!(
            class(&tx.transaction_id),
            SealCheck::Sealed { .. }
        ));
    }
}

#[test]
fn every_recognised_legacy_folder_is_found_once_without_creating_any() {
    let home = tempfile::tempdir().unwrap();
    let archivefs = home
        .path()
        .join(".local/share/archivefs/rename-transactions");
    let emuwiz = home.path().join(".local/share/emuwiz/rename-transactions");
    use crate::dat::rename_apply::seal;
    assert!(seal::recognised_legacy_dirs_in(Some(home.path()), None).is_empty());
    assert!(
        !archivefs.exists() && !emuwiz.exists(),
        "discovery creates nothing"
    );
    std::fs::create_dir_all(&archivefs).unwrap();
    std::fs::create_dir_all(&emuwiz).unwrap();
    let data = home.path().join(".local/share/archivefs");
    let found = seal::recognised_legacy_dirs_in(Some(home.path()), Some(&data));
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.contains(&archivefs) && found.contains(&emuwiz));
}

/// The independently sealed journal is authoritative and the caller's copy is
/// only a cache: everyday failures never become "integrity lost".
mod sealed_journal_authority {
    use super::*;
    use crate::dat::rename_apply::seal::{self, JOURNAL_TEMPORARILY_UNAVAILABLE};

    #[test]
    fn a_temporarily_unavailable_journal_folder_keeps_undo_and_retries_once_restored() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        let roms = dir.path().join("roms");
        let away = dir.path().join("journal-away");
        for obstruction in ["missing", "file"] {
            std::fs::rename(&journal, &away).unwrap();
            if obstruction == "file" {
                std::fs::write(&journal, b"not a folder").unwrap();
            }
            let refused = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap_err();
            // Refused as unavailable (by the seal, or first by the ownership
            // check), never as an integrity loss.
            assert!(
                refused.contains(JOURNAL_TEMPORARILY_UNAVAILABLE)
                    || refused.contains("ownership namespace unavailable"),
                "{refused}"
            );
            assert!(!is_journal_integrity_loss(&refused), "{refused}");
            assert!(matches!(
                seal::verify(&journal, &tx.transaction_id),
                seal::SealCheck::Unavailable(_)
            ));
            assert!(roms.join("A.bin").exists() && !roms.join("a.bin").exists());
            if obstruction == "file" {
                std::fs::remove_file(&journal).unwrap();
            }
            std::fs::rename(&away, &journal).unwrap();
        }
        let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    }

    #[cfg(unix)]
    #[test]
    fn a_write_that_fails_before_replacing_the_journal_rebuilds_the_cache_from_disk() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        let roms = dir.path().join("roms");
        // Probe whether directory permissions are enforced for this user.
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o555)).unwrap();
        let enforced = std::fs::write(journal.join(".probe"), b"x").is_err();
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_file(journal.join(".probe"));
        if !enforced {
            eprintln!("SKIPPED: directory permissions are not enforced for this user");
            return;
        }
        let locked = journal.clone();
        seal::hooks::AFTER_INTENT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
            }));
        });
        let failed = rollback_transaction(&mut tx, &journal, &no_cancel());
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(failed.is_err(), "{failed:?}");
        // The journal is still the committed generation; the in-memory copy
        // moved ahead. That is not tampering.
        assert!(matches!(
            seal::verify(&journal, &tx.transaction_id),
            seal::SealCheck::Sealed { .. }
        ));
        let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
        assert!(roms.join("a.bin").exists());
    }

    #[test]
    fn a_journal_written_but_not_sealed_is_continued_from_its_recorded_intent() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        seal::hooks::BEFORE_COMMIT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|| Err("seal commit failed".into())));
        });
        assert!(rollback_transaction(&mut tx, &journal, &no_cancel()).is_err());
        assert!(matches!(
            seal::verify(&journal, &tx.transaction_id),
            seal::SealCheck::PendingIntent { .. }
        ));
        // Same running session, same handle.
        let outcome = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap();
        assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
        assert!(matches!(
            seal::verify(&journal, &tx.transaction_id),
            seal::SealCheck::Sealed { .. }
        ));
    }

    #[test]
    fn a_stale_copy_after_another_legitimate_generation_is_rebuilt_not_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, journal) = apply_many(dir.path(), &["a.bin", "b.bin"]);
        let roms = dir.path().join("roms");
        let mut stale = tx.clone();
        // Another Undo (another page, another session) advances the journal.
        std::fs::write(roms.join("a.bin"), b"blocker").unwrap();
        let mut other = tx.clone();
        rollback_transaction(&mut other, &journal, &no_cancel()).unwrap();
        std::fs::remove_file(roms.join("a.bin")).unwrap();
        let outcome = rollback_transaction(&mut stale, &journal, &no_cancel()).unwrap();
        assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
        assert!(roms.join("a.bin").exists() && roms.join("b.bin").exists());
    }

    #[test]
    fn genuine_disk_tampering_stays_refused_and_is_never_repaired_from_memory() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        let roms = dir.path().join("roms");
        let path = journal_path(&journal, &tx.transaction_id).unwrap();
        let mut edited = tx.clone();
        edited.entries[0].failure_reason = Some("edited outside EmuWiz".into());
        std::fs::write(&path, serde_json::to_string(&edited).unwrap()).unwrap();
        let tampered = std::fs::read(&path).unwrap();
        for _ in 0..3 {
            let refused = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap_err();
            assert!(is_journal_integrity_loss(&refused), "{refused}");
            assert_eq!(std::fs::read(&path).unwrap(), tampered);
            assert!(
                write_journal(&journal, &tx).is_err(),
                "never repaired from memory"
            );
            assert!(roms.join("A.bin").exists() && !roms.join("a.bin").exists());
        }
    }
}

/// Seals follow the journal folder's own identity, never its path or inode.
mod sealed_namespace_identity {
    use super::*;
    use crate::dat::rename_apply::seal::{self, NAMESPACE_FILE, SealCheck};

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }

    #[test]
    fn a_complete_recovery_set_moved_to_another_path_stays_verifiable() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        assert!(journal.join(NAMESPACE_FILE).exists());
        // Copied (new path, new inode) together with its identity; the seal
        // store travels with it.
        let moved = dir.path().join("restored-elsewhere").join("journal");
        copy_dir(&journal, &moved);
        std::fs::remove_dir_all(&journal).unwrap();
        assert!(matches!(
            seal::verify(&moved, &tx.transaction_id),
            SealCheck::Sealed { .. }
        ));
        let outcome = rollback_transaction(&mut tx, &moved, &no_cancel()).unwrap();
        assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    }

    #[test]
    fn journals_copied_without_their_seals_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        let roms = dir.path().join("roms");
        let name = format!("{}.json", tx.transaction_id);
        // Only the journal: no identity, so no seal can match it.
        let bare = dir.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::copy(journal.join(&name), bare.join(&name)).unwrap();
        assert_eq!(seal::verify(&bare, &tx.transaction_id), SealCheck::Legacy);
        let refused = rollback_transaction(&mut tx, &bare, &no_cancel()).unwrap_err();
        assert!(seal::is_seal_refusal(&refused), "{refused}");
        // Journal and identity, but no matching seals in the store.
        let unsealed = dir.path().join("unsealed");
        copy_dir(&journal, &unsealed);
        seal::forget(&unsealed, &tx.transaction_id).unwrap();
        assert_eq!(
            seal::verify(&unsealed, &tx.transaction_id),
            SealCheck::Legacy
        );
        let refused = rollback_transaction(&mut tx, &unsealed, &no_cancel()).unwrap_err();
        assert!(seal::is_seal_refusal(&refused), "{refused}");
        assert!(roms.join("A.bin").exists() && !roms.join("a.bin").exists());
    }

    #[test]
    fn an_unrelated_folder_substituted_at_the_old_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, journal) = apply_many(dir.path(), &["a.bin"]);
        let roms = dir.path().join("roms");
        let name = format!("{}.json", tx.transaction_id);
        let original = std::fs::read(journal.join(&name)).unwrap();
        std::fs::rename(&journal, dir.path().join("journal-away")).unwrap();
        for identity in [None, Some("0123456789abcdef0123456789abcdef")] {
            std::fs::create_dir_all(&journal).unwrap();
            std::fs::write(journal.join(&name), &original).unwrap();
            if let Some(identity) = identity {
                std::fs::write(journal.join(NAMESPACE_FILE), identity).unwrap();
            }
            assert!(!seal::verify(&journal, &tx.transaction_id).trusted());
            let refused = rollback_transaction(&mut tx, &journal, &no_cancel()).unwrap_err();
            assert!(seal::is_seal_refusal(&refused), "{identity:?}: {refused}");
            assert!(roms.join("A.bin").exists() && !roms.join("a.bin").exists());
            assert_eq!(std::fs::read(journal.join(&name)).unwrap(), original);
            std::fs::remove_dir_all(&journal).unwrap();
        }
    }
}

// ---------------------------------------------------------------------
// Retryable Undo after an environmental failure (#263)
// ---------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn error_classification_separates_an_outage_from_a_changed_object() {
    use super::rollback::{is_file_unavailable, is_unavailable};
    let os = std::io::Error::from_raw_os_error;
    for code in [
        libc::EIO,
        libc::ESTALE,
        libc::ENOTCONN,
        libc::ETIMEDOUT,
        libc::EHOSTDOWN,
        libc::EHOSTUNREACH,
        libc::ENETDOWN,
        libc::ENETUNREACH,
        libc::ENODEV,
        libc::ENXIO,
        libc::ENOENT,
    ] {
        assert!(is_unavailable(&os(code)), "errno {code} is an outage");
    }
    for code in [
        libc::EACCES,
        libc::EPERM,
        libc::ELOOP,
        libc::ENOTDIR,
        libc::EEXIST,
        libc::EISDIR,
        libc::EINVAL,
        libc::EROFS,
        libc::ENOSPC,
    ] {
        assert!(!is_unavailable(&os(code)), "errno {code} is not an outage");
    }
    assert!(!is_unavailable(&std::io::Error::other("changed")));
    assert!(!is_unavailable(&std::io::Error::from(
        std::io::ErrorKind::PermissionDenied
    )));

    let dir = tempfile::tempdir().unwrap();
    let missing_in_present = dir.path().join("gone.bin");
    let not_found = os(libc::ENOENT);
    assert!(
        !is_file_unavailable(&missing_in_present, &not_found),
        "a file missing from a folder that is present was changed"
    );
    let in_missing_folder = dir.path().join("nofolder/gone.bin");
    assert!(is_file_unavailable(&in_missing_folder, &not_found));
    assert!(is_file_unavailable(&missing_in_present, &os(libc::EIO)));
    assert!(!is_file_unavailable(&missing_in_present, &os(libc::EACCES)));
}

#[cfg(unix)]
#[test]
fn an_outage_link_stays_applied_and_finishes_after_the_folder_returns() {
    let mut m = applied_mixed();
    let away = m.hard.with_extension("away");
    std::fs::rename(&m.hard, &away).unwrap();
    let first = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert!(!matches!(first.result, RollbackResult::FullyRolledBack));
    assert_eq!(
        entry_states(&m.tx),
        vec![
            EntryState::RolledBack,
            EntryState::RolledBack,
            EntryState::Applied
        ]
    );
    assert!(m.tx.entries[2].failure_reason.is_some());
    assert_ne!(m.tx.state, TransactionState::RolledBack);
    std::fs::rename(&away, &m.hard).unwrap();
    let second = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(second.result, RollbackResult::FullyRolledBack);
    assert_eq!(m.tx.state, TransactionState::RolledBack);
    assert!(m.tx.entries[2].failure_reason.is_none());
}

#[cfg(unix)]
#[test]
fn cancellation_during_an_outage_leaves_every_entry_retryable() {
    // Cancelled before anything runs, with the hard link's folder away.
    let mut m = applied_mixed();
    let away = m.hard.with_extension("away");
    std::fs::rename(&m.hard, &away).unwrap();
    let cancelled = AtomicBool::new(true);
    let outcome = rollback_transaction(&mut m.tx, &m.journal, &cancelled).unwrap();
    assert!(!matches!(outcome.result, RollbackResult::FullyRolledBack));
    assert!(
        entry_states(&m.tx)
            .iter()
            .all(|state| *state == EntryState::Applied)
    );
    assert!(m.sym.join("s.bin").exists());

    // Cancelled after the first reversal; the outage entry was already held back.
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    super::rollback::AFTER_REVERSAL.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| flag.store(true, Ordering::Relaxed)));
    });
    let outcome = rollback_transaction(&mut m.tx, &m.journal, &cancel).unwrap();
    super::rollback::AFTER_REVERSAL.with(|hook| *hook.borrow_mut() = None);
    assert!(!matches!(outcome.result, RollbackResult::FullyRolledBack));
    assert_eq!(
        entry_states(&m.tx),
        vec![
            EntryState::Applied,
            EntryState::RolledBack,
            EntryState::Applied
        ]
    );

    // Folder back, no cancellation: the rest completes and nothing is double-undone.
    std::fs::rename(&away, &m.hard).unwrap();
    let done = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(done.result, RollbackResult::FullyRolledBack);
    assert_eq!(m.tx.state, TransactionState::RolledBack);
}

#[cfg(unix)]
#[test]
fn a_coordinated_rollback_still_records_an_unavailable_link_as_refused() {
    let mut m = applied_mixed();
    let away = m.hard.with_extension("away");
    std::fs::rename(&m.hard, &away).unwrap();
    let trusted = TrustedRoots::from_paths([m.roms.parent().unwrap()]);
    let outcome = super::rollback::rollback_coordinated_for_test(
        &mut m.tx,
        &m.journal,
        &no_cancel(),
        &trusted,
    )
    .unwrap();
    assert!(!matches!(outcome.result, RollbackResult::FullyRolledBack));
    assert_eq!(
        m.tx.entries[2].state,
        EntryState::RollbackFailed,
        "coordinated callers keep the old recorded-refusal behaviour"
    );
    assert!(
        !m.tx.entries[2]
            .failure_reason
            .as_deref()
            .unwrap_or_default()
            .contains('\u{1}'),
        "no internal marker leaks into the reason"
    );
}

/// Every directory entry under `root`, recursively.
#[cfg(unix)]
fn tree_names(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in read.flatten() {
            out.push(item.file_name().to_string_lossy().into_owned());
            if item.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push(item.path());
            }
        }
    }
    out
}

#[cfg(unix)]
fn hidden_leftovers(root: &Path) -> Vec<String> {
    tree_names(root)
        .into_iter()
        .filter(|name| {
            name.starts_with(".emuwiz-undo-q-") || name.starts_with(".emuwiz-undone-hl-")
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn a_hard_link_undo_leaves_no_hidden_file_and_frees_the_extra_name() {
    use std::os::unix::fs::MetadataExt;
    let mut m = applied_mixed();
    let source = m.roms.join("h.bin");
    let before = std::fs::read(&source).unwrap();
    assert_eq!(std::fs::metadata(&source).unwrap().nlink(), 2);
    let outcome = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    assert!(!m.hard.join("h.bin").exists());
    assert_eq!(std::fs::read(&source).unwrap(), before);
    // Storage accounting: the source is the only name for its data again.
    assert_eq!(std::fs::metadata(&source).unwrap().nlink(), 1);
    // No quarantine folder or retained link anywhere, and the folder is empty.
    let hard_dir_listing = tree_names(&m.hard);
    assert!(hard_dir_listing.is_empty(), "{hard_dir_listing:?}");
    assert!(hidden_leftovers(m.roms.parent().unwrap()).is_empty());
}

#[cfg(unix)]
#[test]
fn a_foreign_file_swapped_in_before_the_move_is_restored_and_leaves_nothing_behind() {
    let mut m = applied_mixed();
    let link = m.hard.join("h.bin");
    let held = m.hard.join("held-original");
    let held_for_hook = held.clone();
    super::dirsync::set_undo_move_hook(Some(Box::new(move |path| {
        std::fs::rename(path, &held_for_hook).unwrap();
        std::fs::write(path, b"unrelated user file").unwrap();
    })));
    let outcome = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    super::dirsync::set_undo_move_hook(None);
    assert!(!matches!(outcome.result, RollbackResult::FullyRolledBack));
    assert_eq!(std::fs::read(&link).unwrap(), b"unrelated user file");
    assert!(held.exists(), "the original link is untouched");
    assert_eq!(m.tx.entries[2].state, EntryState::RollbackFailed);
    let reason = m.tx.entries[2].failure_reason.clone().unwrap();
    assert!(reason.contains("foreign object restored"), "{reason}");
    assert!(hidden_leftovers(&m.hard).is_empty(), "quarantine removed");
}

#[cfg(unix)]
#[test]
fn a_foreign_file_whose_name_is_retaken_stays_in_quarantine_and_is_reported_incomplete() {
    let mut m = applied_mixed();
    let link = m.hard.join("h.bin");
    let held = m.hard.join("held-original");
    let (held_for_hook, link_for_hook) = (held.clone(), link.clone());
    super::dirsync::set_undo_move_hook(Some(Box::new(move |path| {
        std::fs::rename(path, &held_for_hook).unwrap();
        std::fs::write(path, b"foreign").unwrap();
    })));
    // After the foreign file is moved aside, someone recreates its old name.
    super::dirsync::set_moved_hook(Some(Box::new(move || {
        std::fs::write(&link_for_hook, b"newcomer").unwrap();
    })));
    let outcome = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    super::dirsync::set_undo_move_hook(None);
    super::dirsync::set_moved_hook(None);
    assert!(!matches!(outcome.result, RollbackResult::FullyRolledBack));
    assert_eq!(std::fs::read(&link).unwrap(), b"newcomer");
    assert_ne!(m.tx.entries[2].state, EntryState::RolledBack);
    // Nothing was deleted: the foreign file is still somewhere under the folder.
    let kept = tree_names(&m.hard);
    assert_eq!(hidden_leftovers(&m.hard).len(), 1, "{kept:?}");
    assert!(held.exists(), "the original link is untouched");
}

#[cfg(unix)]
#[test]
fn an_interruption_after_the_move_resumes_after_a_restart_and_removes_everything() {
    use std::os::unix::fs::MetadataExt;
    let mut m = applied_mixed();
    let source = m.roms.join("h.bin");
    // The process dies once the link is inside its quarantine.
    super::dirsync::set_unlink_hook(Some(Box::new(|_| {
        Err(std::io::Error::other("simulated crash"))
    })));
    let first = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    super::dirsync::set_unlink_hook(None);
    assert!(!matches!(first.result, RollbackResult::FullyRolledBack));
    assert_eq!(m.tx.entries[2].state, EntryState::RollingBack);
    assert!(!m.hard.join("h.bin").exists());
    assert_eq!(
        hidden_leftovers(&m.hard).len(),
        1,
        "the quarantine is retained"
    );
    assert_eq!(std::fs::metadata(&source).unwrap().nlink(), 2);
    assert_ne!(m.tx.state, TransactionState::RolledBack);

    // A fresh process knows only the sealed journal on disk.
    let id = m.tx.transaction_id.clone();
    let mut restarted =
        read_journal(&journal_path(&m.journal, &id).unwrap()).expect("journal reread");
    assert_eq!(restarted.entries[2].state, EntryState::RollingBack);
    assert!(
        restarted.entries[2]
            .unknown
            .contains_key("emuwiz_hl_quarantine_v1")
    );
    let done = rollback_transaction(&mut restarted, &m.journal, &no_cancel()).unwrap();
    assert_eq!(done.result, RollbackResult::FullyRolledBack);
    assert_eq!(restarted.state, TransactionState::RolledBack);
    assert!(hidden_leftovers(m.roms.parent().unwrap()).is_empty());
    assert_eq!(std::fs::metadata(&source).unwrap().nlink(), 1);
    assert!(tree_names(&m.hard).is_empty());
}

#[cfg(unix)]
#[test]
fn an_interruption_right_after_creating_the_quarantine_leaves_the_link_and_resumes() {
    let mut m = applied_mixed();
    super::dirsync::set_quarantine_hook(Some(Box::new(|| {
        Err(std::io::Error::other("simulated crash"))
    })));
    let first = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    super::dirsync::set_quarantine_hook(None);
    assert!(!matches!(first.result, RollbackResult::FullyRolledBack));
    assert!(m.hard.join("h.bin").exists(), "the link was never moved");
    assert_eq!(m.tx.entries[2].state, EntryState::RollingBack);
    let done = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(done.result, RollbackResult::FullyRolledBack);
    assert!(!m.hard.join("h.bin").exists());
    assert!(tree_names(&m.hard).is_empty());
}

#[cfg(unix)]
#[test]
fn an_unrecognised_object_in_the_quarantine_is_never_removed_and_undo_stays_incomplete() {
    let mut m = applied_mixed();
    super::dirsync::set_unlink_hook(Some(Box::new(|_| {
        Err(std::io::Error::other("simulated crash"))
    })));
    let _ = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    super::dirsync::set_unlink_hook(None);
    // While interrupted, the retained file is replaced by something else.
    let quarantine = tree_names(&m.hard);
    assert_eq!(quarantine.len(), 1, "{quarantine:?}");
    let record = m.tx.entries[2].unknown["emuwiz_hl_quarantine_v1"].clone();
    let folder = m.hard.join(record["dir"].as_str().unwrap());
    let file = folder.join(record["file"].as_str().unwrap());
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, b"someone else's file").unwrap();
    let again = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert!(!matches!(again.result, RollbackResult::FullyRolledBack));
    assert_eq!(std::fs::read(&file).unwrap(), b"someone else's file");
    assert_ne!(m.tx.entries[2].state, EntryState::RolledBack);
    assert_ne!(m.tx.state, TransactionState::RolledBack);
}

#[cfg(unix)]
#[test]
fn cancellation_between_hard_link_undos_never_leaves_a_quarantine() {
    let mut m = applied_mixed();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    super::rollback::AFTER_REVERSAL.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| flag.store(true, Ordering::Relaxed)));
    });
    // Reverse order: the hard link is first, so it completes before the stop.
    let outcome = rollback_transaction(&mut m.tx, &m.journal, &cancel).unwrap();
    super::rollback::AFTER_REVERSAL.with(|hook| *hook.borrow_mut() = None);
    assert!(!matches!(outcome.result, RollbackResult::FullyRolledBack));
    assert_eq!(m.tx.entries[2].state, EntryState::RolledBack);
    assert!(hidden_leftovers(m.roms.parent().unwrap()).is_empty());
    let done = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(done.result, RollbackResult::FullyRolledBack);
}

#[cfg(unix)]
#[test]
fn an_independent_file_beside_the_link_survives_hard_link_undo() {
    let mut m = applied_mixed();
    let independent = m.hard.join("mine.txt");
    std::fs::write(&independent, b"mine").unwrap();
    let outcome = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    assert_eq!(std::fs::read(&independent).unwrap(), b"mine");
    assert!(hidden_leftovers(&m.hard).is_empty());
}

#[cfg(unix)]
#[test]
fn a_missing_destination_root_is_an_outage_not_an_invalid_record() {
    let mut m = applied_mixed();
    let away = m.hard.with_extension("away");
    std::fs::rename(&m.hard, &away).unwrap();
    let _ = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(m.tx.entries[2].state, EntryState::Applied);
    std::fs::rename(&away, &m.hard).unwrap();
    let done = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(done.result, RollbackResult::FullyRolledBack);
}

#[cfg(unix)]
fn interrupted_inside_quarantine() -> Mixed {
    let mut m = applied_mixed();
    super::dirsync::set_unlink_hook(Some(Box::new(|_| {
        Err(std::io::Error::other("simulated crash"))
    })));
    let _ = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    super::dirsync::set_unlink_hook(None);
    assert_eq!(m.tx.entries[2].state, EntryState::RollingBack);
    m
}

#[cfg(unix)]
#[test]
fn an_offline_folder_never_turns_a_retained_quarantine_into_a_recorded_rollback() {
    let mut m = interrupted_inside_quarantine();
    let record = m.tx.entries[2].unknown["emuwiz_hl_quarantine_v1"].clone();
    let away = m.hard.with_extension("away");
    std::fs::rename(&m.hard, &away).unwrap();
    for _ in 0..2 {
        let outcome = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
        assert!(!matches!(outcome.result, RollbackResult::FullyRolledBack));
        assert_ne!(m.tx.entries[2].state, EntryState::RolledBack);
        assert_ne!(m.tx.state, TransactionState::RolledBack);
        assert_eq!(m.tx.entries[2].unknown["emuwiz_hl_quarantine_v1"], record);
    }
    std::fs::rename(&away, &m.hard).unwrap();
    let done = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(done.result, RollbackResult::FullyRolledBack);
    assert!(hidden_leftovers(m.roms.parent().unwrap()).is_empty());
}

#[cfg(unix)]
#[test]
fn an_empty_mount_point_in_the_folders_place_is_not_taken_for_a_finished_removal() {
    let mut m = interrupted_inside_quarantine();
    let record = m.tx.entries[2].unknown["emuwiz_hl_quarantine_v1"].clone();
    let away = m.hard.with_extension("away");
    std::fs::rename(&m.hard, &away).unwrap();
    std::fs::create_dir(&m.hard).unwrap(); // the unmounted mount point
    let outcome = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert!(!matches!(outcome.result, RollbackResult::FullyRolledBack));
    assert_ne!(m.tx.entries[2].state, EntryState::RolledBack);
    assert_ne!(m.tx.state, TransactionState::RolledBack);
    assert_eq!(m.tx.entries[2].unknown["emuwiz_hl_quarantine_v1"], record);
    assert!(
        tree_names(&m.hard).is_empty(),
        "the stand-in was not touched"
    );
    std::fs::remove_dir(&m.hard).unwrap();
    std::fs::rename(&away, &m.hard).unwrap();
    let done = rollback_transaction(&mut m.tx, &m.journal, &no_cancel()).unwrap();
    assert_eq!(done.result, RollbackResult::FullyRolledBack);
}

#[cfg(unix)]
#[test]
fn only_the_exact_quarantine_record_shape_is_accepted() {
    let m = interrupted_inside_quarantine();
    let mut entry = m.tx.entries[2].clone();
    assert!(super::dirsync::hardlink_quarantine_names(&entry).is_some());
    for (dir, file) in [
        ("hard", "0".repeat(32)),
        (".emuwiz-undo-q-short", "0".repeat(32)),
        (
            ".emuwiz-undo-q-00000000000000000000000000000000",
            "x".repeat(32),
        ),
        (
            ".emuwiz-undo-q-../../../etc00000000000000000",
            "0".repeat(32),
        ),
    ] {
        entry.unknown.insert(
            "emuwiz_hl_quarantine_v1".into(),
            serde_json::json!({"dir": dir, "file": file}),
        );
        assert!(
            super::dirsync::hardlink_quarantine_names(&entry).is_none(),
            "{dir}"
        );
    }
}
