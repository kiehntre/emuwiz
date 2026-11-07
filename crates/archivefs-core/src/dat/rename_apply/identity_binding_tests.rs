//! Hard Job #6 / Operation Identity Lock: a rename proposal is bound to the
//! file the audit hashed, and apply refuses - before any journal write or
//! mutation - when the file at the source path is no longer that file.
//!
//! Owner decision 2026-11-06: one changed file refuses the whole approved
//! batch (never a silently different batch), each refusal names the file
//! and the reason, unchanged files keep their audit evidence, and every
//! source is re-validated by preflight immediately before its own rename.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::*;
use crate::dat::classification::{
    ContentSelectionPolicy, DatContentClassification, DatOriginalMetadata,
};
use crate::dat::rename_plan::{
    ProposalState, RenamePlan, RenamePlanCounts, RenameProposal, SourceObjectKind,
};
use crate::identity_source::hashing::FileFingerprint;
use crate::safe_read::TrustedRoots;

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn bound_proposal(source: &Path, proposed: &str) -> RenameProposal {
    let current = source.file_name().unwrap().to_string_lossy().into_owned();
    RenameProposal {
        source_path: source.to_path_buf(),
        current_basename: current,
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
        state: ProposalState::Suggested,
        object_kind: SourceObjectKind::RegularFile,
        ambiguity_reason: None,
        collision: None,
        blockers: Vec::new(),
        extension_status: None,
        sanitisation_notes: Vec::new(),
        actionable: true,
        audited_identity: None,
        // The fingerprint the audit would have recorded around its hashing read.
        audited_fingerprint: Some(FileFingerprint::observe(source).expect("fixture exists")),
        is_outer_archive: false,
    }
}

fn plan_of(proposals: Vec<RenameProposal>, scan_root: &Path) -> RenamePlan {
    let counts = RenamePlanCounts::from_proposals(&proposals);
    RenamePlan {
        generation: 1,
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

fn approved(paths: &[&Path]) -> BTreeSet<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| (entry.path(), std::fs::read(entry.path()).unwrap()))
        .collect();
    files.sort();
    files
}

fn execution<'a>(
    tx: &'a mut RenameTransaction,
    paths: BTreeSet<String>,
    root: &Path,
    cancel: &'a AtomicBool,
) -> ApplyExecution<'a> {
    ApplyExecution {
        transaction: tx,
        approved_paths: paths,
        current_generation: 1,
        trusted: TrustedRoots::from_paths([root.to_path_buf()]),
        journal_dir: root.join("journal"),
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel,
        directory_policy: Default::default(),
        allow_symlink_source: false,
    }
}

fn refusal(error: ApplyError) -> Vec<ChangedSource> {
    match error {
        ApplyError::SourceChangedSinceAudit(changed) => changed,
        other => panic!("expected a changed-source refusal, got {other:?}"),
    }
}

/// Rewrites `path` in place with same-size content and restores its mtime,
/// the S03-02 shape: path, size and mtime all look unchanged.
fn rewrite_same_size_restoring_mtime(path: &Path, bytes: &[u8]) {
    let before = std::fs::metadata(path).unwrap();
    assert_eq!(
        before.len() as usize,
        bytes.len(),
        "fixture must be same-size"
    );
    let modified = before.modified().unwrap();
    // Let the clock tick so the change time differs from the original write.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    std::io::Write::write_all(&mut &file, bytes).unwrap();
    file.set_modified(modified).unwrap();
    drop(file);
    let after = std::fs::metadata(path).unwrap();
    assert_eq!(after.modified().unwrap(), modified);
    assert_eq!(after.len(), before.len());
}

#[test]
fn an_unchanged_bound_source_builds_and_applies() {
    let tree = tempfile::tempdir().unwrap();
    let source = write(tree.path(), "old.bin", b"original bytes");
    let plan = plan_of(
        vec![bound_proposal(&source, "Game (Europe).bin")],
        tree.path(),
    );
    let paths = approved(&[&source]);
    let mut tx = build_transaction(&plan, &paths, 1).unwrap();
    assert_eq!(tx.entries.len(), 1);
    let cancel = AtomicBool::new(false);
    let outcome = apply_transaction(&mut execution(&mut tx, paths, tree.path(), &cancel)).unwrap();
    assert_eq!(outcome.transaction.entries[0].state, EntryState::Applied);
    assert!(tree.path().join("Game (Europe).bin").is_file());
    assert!(!source.exists());
}

/// Changed file after preview: same name, same size, same mtime, different
/// bytes. The reviewed verdict described the old bytes; the batch is refused
/// with the reason and nothing on disk changes.
#[cfg(unix)]
#[test]
fn a_same_size_rewrite_with_restored_mtime_is_refused_before_any_mutation() {
    let tree = tempfile::tempdir().unwrap();
    let source = write(tree.path(), "old.bin", b"original bytes");
    let plan = plan_of(
        vec![bound_proposal(&source, "Game (Europe).bin")],
        tree.path(),
    );
    rewrite_same_size_restoring_mtime(&source, b"replaced bytes");
    let before = snapshot(tree.path());

    let error = build_transaction(&plan, &approved(&[&source]), 1).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("changed since the audit"), "{text}");
    assert!(text.contains("rewritten in place"), "{text}");
    assert!(
        text.contains("unchanged files keep their cached hashes"),
        "{text}"
    );
    let changed = refusal(error);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].path, source);
    assert_eq!(changed[0].reason, SourceChangeReason::RewrittenInPlace);
    assert_eq!(snapshot(tree.path()), before);
    assert_eq!(std::fs::read(&source).unwrap(), b"replaced bytes");
}

/// Path change between planning and application: the source moved. Nothing
/// at the planned path is renamed, and no other file is touched.
#[test]
fn a_source_moved_after_the_audit_is_refused() {
    let tree = tempfile::tempdir().unwrap();
    let source = write(tree.path(), "old.bin", b"original bytes");
    let plan = plan_of(
        vec![bound_proposal(&source, "Game (Europe).bin")],
        tree.path(),
    );
    std::fs::rename(&source, tree.path().join("elsewhere.bin")).unwrap();
    let before = snapshot(tree.path());
    let changed = refusal(build_transaction(&plan, &approved(&[&source]), 1).unwrap_err());
    assert_eq!(changed[0].path, source);
    assert_eq!(changed[0].reason, SourceChangeReason::Missing);
    assert_eq!(snapshot(tree.path()), before);
}

/// A different file placed at the planned path (same name, different bytes,
/// a different inode) is not the audited file either.
#[test]
fn a_source_replaced_by_another_file_is_refused() {
    let tree = tempfile::tempdir().unwrap();
    let source = write(tree.path(), "old.bin", b"original bytes");
    let plan = plan_of(
        vec![bound_proposal(&source, "Game (Europe).bin")],
        tree.path(),
    );
    // Outside one kernel timestamp tick: a recycled inode written in the same
    // tick with the same size is the documented residual blind spot of any
    // metadata fingerprint, and never the audit-to-apply gap in practice.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::remove_file(&source).unwrap();
    write(tree.path(), "old.bin", b"other bytes!!!");
    let before = snapshot(tree.path());
    let changed = refusal(build_transaction(&plan, &approved(&[&source]), 1).unwrap_err());
    assert_eq!(changed[0].path, source);
    // A recycled inode number would show as a timestamp change instead.
    assert!(
        matches!(
            changed[0].reason,
            SourceChangeReason::Replaced | SourceChangeReason::ModifiedTimeChanged
        ),
        "{:?}",
        changed[0].reason
    );
    assert_eq!(snapshot(tree.path()), before);
}

/// One changed source refuses the whole approved batch rather than quietly
/// renaming the rest: the user approved a plan, not a subset of it. Only the
/// changed file is named; the steady file's evidence is untouched.
#[test]
fn one_changed_source_refuses_the_whole_batch_and_names_only_it() {
    let tree = tempfile::tempdir().unwrap();
    let steady = write(tree.path(), "steady.bin", b"steady bytes!!");
    let changed = write(tree.path(), "changed.bin", b"original bytes");
    let steady_fingerprint = FileFingerprint::observe(&steady).unwrap();
    let plan = plan_of(
        vec![
            bound_proposal(&steady, "Steady (Europe).bin"),
            bound_proposal(&changed, "Changed (Europe).bin"),
        ],
        tree.path(),
    );
    std::fs::write(&changed, b"these are new bytes, longer").unwrap();
    let before = snapshot(tree.path());
    let changes =
        refusal(build_transaction(&plan, &approved(&[&steady, &changed]), 1).unwrap_err());
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, changed);
    assert_eq!(changes[0].reason, SourceChangeReason::SizeChanged);
    assert_eq!(snapshot(tree.path()), before);
    // The steady file still observes to its audited fingerprint, so a fresh
    // audit's ctime-bound cache serves its hashes without re-reading it.
    assert_eq!(
        FileFingerprint::observe(&steady).unwrap(),
        steady_fingerprint
    );
    assert_eq!(
        plan.proposals[0].audited_fingerprint.as_ref(),
        Some(&steady_fingerprint)
    );
}

/// Between review (transaction built) and apply the source is rewritten.
/// The per-entry preflight that runs immediately before each rename syscall
/// refuses it, and in AbortAll mode nothing in the batch is renamed.
#[test]
fn a_source_changed_between_review_and_apply_is_refused_by_preflight() {
    let tree = tempfile::tempdir().unwrap();
    let steady = write(tree.path(), "steady.bin", b"steady bytes!!");
    let changed = write(tree.path(), "changed.bin", b"original bytes");
    let plan = plan_of(
        vec![
            bound_proposal(&steady, "Steady (Europe).bin"),
            bound_proposal(&changed, "Changed (Europe).bin"),
        ],
        tree.path(),
    );
    let paths = approved(&[&steady, &changed]);
    let mut tx = build_transaction(&plan, &paths, 1).unwrap();
    assert_eq!(tx.entries.len(), 2);
    std::fs::write(&changed, b"edited after the review").unwrap();
    let before = snapshot(tree.path());
    let cancel = AtomicBool::new(false);
    let error =
        apply_transaction(&mut execution(&mut tx, paths, tree.path(), &cancel)).unwrap_err();
    match error {
        ApplyError::HardConflicts(conflicts) => {
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].0, changed);
        }
        other => panic!("expected preflight hard conflicts, got {other:?}"),
    }
    assert_eq!(snapshot(tree.path()), before);
}

/// The refusal names the files and their reasons, and counts the rest.
#[test]
fn the_refusal_names_changed_files_with_reasons_and_counts_the_rest() {
    let changed: Vec<ChangedSource> = (0..5)
        .map(|i| ChangedSource {
            path: PathBuf::from(format!("/lib/f{i}.bin")),
            reason: SourceChangeReason::SizeChanged,
        })
        .collect();
    let text = ApplyError::SourceChangedSinceAudit(changed).to_string();
    assert!(
        text.starts_with("5 approved file(s) changed since the audit"),
        "{text}"
    );
    assert!(
        text.contains(
            "f0.bin: size changed; f1.bin: size changed; f2.bin: size changed and 2 more"
        ),
        "{text}"
    );
    assert!(text.contains("nothing was renamed"), "{text}");
}

/// A loose proposal with neither a fingerprint nor an archive identity is not
/// tied to any audited bytes, so it is refused (named, before any mutation)
/// rather than blessed with whatever is on disk at review time.
#[test]
fn a_proposal_without_a_fingerprint_is_refused_not_rebound() {
    let tree = tempfile::tempdir().unwrap();
    let source = write(tree.path(), "old.bin", b"original bytes");
    let mut proposal = bound_proposal(&source, "Game (Europe).bin");
    proposal.audited_fingerprint = None;
    let plan = plan_of(vec![proposal], tree.path());
    match build_transaction(&plan, &approved(&[&source]), 1) {
        Err(ApplyError::SourceChangedSinceAudit(changed)) => {
            assert_eq!(changed.len(), 1);
            assert_eq!(
                changed[0].reason,
                SourceChangeReason::FingerprintIncomparable
            );
        }
        other => panic!("expected a named refusal, got {other:?}"),
    }
    assert!(source.exists(), "nothing may be touched");
}
