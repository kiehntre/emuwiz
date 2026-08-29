//! Exact Duplicate Review and Quarantine: a plain-language GUI over the
//! existing, DAT-independent `archivefs_core::repair::exact_duplicate`
//! engine.
//!
//! This page never re-implements grouping, hashing, canonical-copy
//! selection, or multi-file protection - it only collects a source folder
//! (and, optionally, a trusted root and an already-organized "elected
//! library" folder), hands them to
//! [`archivefs_core::repair::scan_exact_duplicates`], and shows the result.
//! Applying builds proposals via
//! [`archivefs_core::repair::build_exact_duplicate_group_proposals`] and
//! moves them through the exact same
//! [`archivefs_core::repair::quarantine`] transaction/journal/rollback
//! engine every other quarantine flow in this app already uses - there is
//! no second filesystem-mutation path anywhere in this file.
//!
//! # What "exact duplicate" means here
//!
//! Only byte-identical files: matching size and a full-physical-file
//! SHA-256, computed over the complete outer file (never an archive
//! member). A CUE/GDI/M3U launcher and its own companions, a ZIP and its
//! matching loose member, an N64 ROM and its byte-swapped twin, or two
//! releases that merely match the same DAT entry are never called exact
//! duplicates - see the engine's own module doc comment for why that is
//! structurally guaranteed, not a rule bolted on here.
//!
//! # Manual choice, never an invented winner
//!
//! When the engine cannot determine a canonical copy from trusted-root or
//! elected-library evidence (`CanonicalRecommendation::RequiresUserChoice`),
//! this page lets a person pick one member to keep via
//! [`archivefs_core::repair::apply_user_choice`] - never an alphabetical or
//! first-found fallback. A group blocked by multi-file protection offers no
//! selection control at all: the reason is shown, and nothing about that
//! group can be quarantined until the underlying release relationship is
//! resolved elsewhere.
//!
//! # Deliberately not wired into the main sidebar yet
//!
//! [`show_exact_duplicate_review_page`] is a complete, independently
//! testable page function, but no `MainView` variant or sidebar entry has
//! been added for it in this change. `main.rs`/`navigation.rs` are large,
//! frequently-touched files a concurrent RetroDECK navigation change could
//! easily collide with; wiring reachability is a small, separate follow-up
//! once that risk is gone, not a reason to withhold the page itself.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir;
use archivefs_core::dat::rename_apply::model::RenameTransaction;
use archivefs_core::repair::quarantine::{
    apply_quarantine_transaction, build_quarantine_transaction,
};
use archivefs_core::repair::{
    CanonicalRecommendation, DuplicateHashCache, ExactDuplicateGroup, ExactDuplicateScanReport,
    GroupQuarantineReadiness, MultiFileProtection, RepairRecoveryReport, apply_user_choice,
    build_exact_duplicate_group_proposals, classify_persisted_transactions,
    rollback_quarantine_transaction, scan_exact_duplicates,
};
use archivefs_core::repair::{
    N64EquivalentScanReport, apply_n64_equivalent_group, rollback_n64_equivalent_group,
    scan_n64_equivalent_duplicates,
};
use archivefs_core::repair::{
    OpticalEquivalentScanReport, apply_optical_equivalent_group, rollback_optical_equivalent_group,
    scan_optical_equivalent_duplicates,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;

use crate::ui::{components as widgets, theme};

/// How the current scan is progressing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScanStatus {
    Scanning,
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DuplicateReviewMode {
    #[default]
    Exact,
    EquivalentN64,
    EquivalentOptical,
}

/// The running background scan job, mirroring the same one-shot
/// background-thread-plus-channel shape `repair_review_page` already uses
/// for its own whole-library scan.
struct ExactDuplicateScanJob {
    messages: Receiver<ExactDuplicateScanReport>,
}

/// A frozen "Move N copies to quarantine" confirmation for one group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplyConfirmation {
    pub(crate) group_index: usize,
    pub(crate) retained_path: PathBuf,
    pub(crate) redundant_count: usize,
    pub(crate) reclaimable_bytes: u64,
}

/// The page's authoritative state.
pub(crate) struct ExactDuplicateReviewPageState {
    pub(crate) source_root_draft: String,
    /// Optional: a folder a person trusts as the "keep from here" source.
    /// Every scanned file under it counts as trusted-root evidence.
    pub(crate) trusted_root_draft: String,
    /// Optional: a folder that already holds an organized/published
    /// library. Every regular file found under it counts as
    /// elected-library evidence for any scanned candidate at the same
    /// path.
    pub(crate) elected_library_draft: String,
    error: Option<String>,

    scan_job: Option<ExactDuplicateScanJob>,
    scan_cancel: Option<Arc<AtomicBool>>,
    scan_status: Option<ScanStatus>,

    report: Option<ExactDuplicateScanReport>,
    equivalent_report: Option<N64EquivalentScanReport>,
    optical_report: Option<OpticalEquivalentScanReport>,
    mode: DuplicateReviewMode,
    /// group index -> the path a person picked to keep, for a group whose
    /// own recommendation is `RequiresUserChoice`. Cleared on every new
    /// scan.
    manual_choice: BTreeMap<usize, PathBuf>,
    /// Which group's detail panel is expanded ("Why are these identical?").
    expanded_group: Option<usize>,

    apply_confirm: Option<ApplyConfirmation>,
    apply_error: Option<String>,
    /// The most recently applied quarantine transaction, kept so "You can
    /// undo this" has something concrete to roll back.
    applied: Option<RenameTransaction>,
    applied_equivalent: bool,
    rollback_error: Option<String>,

    recovery: Option<RepairRecoveryReport>,
    recovery_error: Option<String>,

    journal_dir: PathBuf,
    cache: DuplicateHashCache,
    /// Every directory root the most recent scan trusted (source root,
    /// trusted root, elected-library root). Reused unchanged for apply and
    /// rollback so a file scanned under any of them is never refused as
    /// "outside the trusted roots" purely because apply narrowed the scope
    /// to individual member paths instead of the directories that contain
    /// them.
    trust_scope_dirs: Vec<PathBuf>,
}

impl Default for ExactDuplicateReviewPageState {
    fn default() -> Self {
        Self {
            source_root_draft: String::new(),
            trusted_root_draft: String::new(),
            elected_library_draft: String::new(),
            error: None,
            scan_job: None,
            scan_cancel: None,
            scan_status: None,
            report: None,
            equivalent_report: None,
            optical_report: None,
            mode: DuplicateReviewMode::Exact,
            manual_choice: BTreeMap::new(),
            expanded_group: None,
            apply_confirm: None,
            apply_error: None,
            applied: None,
            applied_equivalent: false,
            rollback_error: None,
            recovery: None,
            recovery_error: None,
            journal_dir: default_rename_transaction_dir()
                .unwrap_or_else(|_| PathBuf::from("rename-transactions")),
            cache: DuplicateHashCache::new(),
            trust_scope_dirs: Vec::new(),
        }
    }
}

/// Every regular file under `root`, recursively, symlinks never followed.
/// This page's own minimal walker - deliberately not shared with any other
/// page's source-collection helper, so this file's dependencies stay
/// entirely self-contained.
fn collect_regular_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

impl ExactDuplicateReviewPageState {
    #[cfg(test)]
    pub(crate) fn with_journal_dir(journal_dir: PathBuf) -> Self {
        Self {
            journal_dir,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn apply_error(&self) -> Option<&str> {
        self.apply_error.as_deref()
    }

    pub(crate) fn rollback_error(&self) -> Option<&str> {
        self.rollback_error.as_deref()
    }

    pub(crate) fn report(&self) -> Option<&ExactDuplicateScanReport> {
        self.report.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn equivalent_report(&self) -> Option<&N64EquivalentScanReport> {
        self.equivalent_report.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn mode(&self) -> DuplicateReviewMode {
        self.mode
    }

    #[cfg(test)]
    pub(crate) fn select_equivalent_mode(&mut self) {
        self.mode = DuplicateReviewMode::EquivalentN64;
    }

    #[cfg(test)]
    pub(crate) fn select_optical_mode(&mut self) {
        self.mode = DuplicateReviewMode::EquivalentOptical;
    }

    pub(crate) fn scan_status(&self) -> Option<&ScanStatus> {
        self.scan_status.as_ref()
    }

    pub(crate) fn is_scan_running(&self) -> bool {
        matches!(self.scan_status, Some(ScanStatus::Scanning))
    }

    pub(crate) fn applied(&self) -> Option<&RenameTransaction> {
        self.applied.as_ref()
    }

    pub(crate) fn recovery(&self) -> Option<&RepairRecoveryReport> {
        self.recovery.as_ref()
    }

    /// Starts a background scan. Never touches the filesystem itself
    /// beyond reading directory listings and file bytes through the same
    /// trusted, bounded hashing the core engine already enforces.
    pub(crate) fn scan(&mut self) {
        if self.mode == DuplicateReviewMode::EquivalentN64 {
            self.scan_equivalent();
            return;
        }
        if self.mode == DuplicateReviewMode::EquivalentOptical {
            self.scan_optical();
            return;
        }
        self.error = None;
        self.apply_error = None;
        self.rollback_error = None;
        self.report = None;
        self.equivalent_report = None;
        self.manual_choice.clear();
        self.expanded_group = None;
        self.applied = None;
        self.apply_confirm = None;

        let source_trimmed = self.source_root_draft.trim().to_string();
        if source_trimmed.is_empty() {
            self.error = Some("choose a source folder first".to_string());
            return;
        }
        let source_root = PathBuf::from(&source_trimmed);
        if !source_root.is_dir() {
            self.error = Some(format!("'{source_trimmed}' is not a folder"));
            return;
        }

        let trusted_root_trimmed = self.trusted_root_draft.trim().to_string();
        let trusted_roots: Vec<PathBuf> = if trusted_root_trimmed.is_empty() {
            Vec::new()
        } else {
            vec![PathBuf::from(&trusted_root_trimmed)]
        };

        let elected_trimmed = self.elected_library_draft.trim().to_string();
        let elected_root = if elected_trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(&elected_trimmed))
        };

        std::fs::create_dir_all(&self.journal_dir).ok();
        self.recovery = Some(classify_persisted_transactions(&self.journal_dir));
        self.recovery_error = None;

        let mut candidates = collect_regular_files(&source_root);
        for extra_root in trusted_roots.iter().chain(elected_root.iter()) {
            if extra_root.is_dir() {
                for path in collect_regular_files(extra_root) {
                    if !candidates.contains(&path) {
                        candidates.push(path);
                    }
                }
            }
        }

        let elected_paths: BTreeSet<PathBuf> = elected_root
            .as_ref()
            .filter(|root| root.is_dir())
            .map(|root| collect_regular_files(root).into_iter().collect())
            .unwrap_or_default();

        let mut trust_scope: Vec<PathBuf> = vec![source_root.clone()];
        trust_scope.extend(trusted_roots.iter().cloned());
        if let Some(root) = &elected_root {
            trust_scope.push(root.clone());
        }
        self.trust_scope_dirs = trust_scope.clone();
        let trusted = TrustedRoots::from_paths(trust_scope);

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_thread = cancel.clone();
        let (sender, messages) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let report = scan_exact_duplicates(
                &candidates,
                &trusted,
                &trusted_roots,
                &elected_paths,
                Some(&cancel_for_thread),
            );
            let _ = sender.send(report);
        });

        self.scan_job = Some(ExactDuplicateScanJob { messages });
        self.scan_cancel = Some(cancel);
        self.scan_status = Some(ScanStatus::Scanning);
    }

    fn scan_equivalent(&mut self) {
        self.error = None;
        self.apply_error = None;
        self.rollback_error = None;
        self.report = None;
        self.equivalent_report = None;
        self.applied = None;
        self.applied_equivalent = false;
        let source = PathBuf::from(self.source_root_draft.trim());
        if !source.is_dir() {
            self.error = Some("choose a source folder first".to_string());
            return;
        }
        let candidates = collect_regular_files(&source);
        self.trust_scope_dirs = vec![source.clone()];
        let trusted = TrustedRoots::from_paths(vec![source]);
        self.equivalent_report = Some(scan_n64_equivalent_duplicates(&candidates, &trusted, None));
        self.scan_status = Some(ScanStatus::Completed);
    }

    fn scan_optical(&mut self) {
        self.error = None;
        self.apply_error = None;
        self.rollback_error = None;
        self.report = None;
        self.equivalent_report = None;
        self.optical_report = None;
        self.applied = None;
        self.applied_equivalent = false;
        let source = PathBuf::from(self.source_root_draft.trim());
        if !source.is_dir() {
            self.error = Some("choose a source folder first".to_string());
            return;
        }
        let candidates = collect_regular_files(&source);
        self.trust_scope_dirs = vec![source.clone()];
        let trusted = TrustedRoots::from_paths(vec![source]);
        self.optical_report = Some(scan_optical_equivalent_duplicates(
            &candidates,
            &trusted,
            None,
        ));
        self.scan_status = Some(ScanStatus::Completed);
    }

    /// Requests cancellation of a running scan. The scan itself keeps
    /// running cooperatively (every hash checks the flag between chunks)
    /// so this never leaves a half-written file behind - nothing here
    /// mutates anything at all, scanning is read-only.
    pub(crate) fn cancel_scan(&mut self) {
        if let Some(cancel) = &self.scan_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Drains the background scan job's channel, if one is running.
    /// Returns whether anything changed (so a caller can request a
    /// repaint).
    pub(crate) fn poll_scan(&mut self) -> bool {
        let Some(job) = self.scan_job.as_mut() else {
            return false;
        };
        match job.messages.try_recv() {
            Ok(report) => {
                let cancelled = self
                    .scan_cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::Relaxed));
                self.scan_status = Some(if cancelled {
                    ScanStatus::Cancelled
                } else {
                    ScanStatus::Completed
                });
                self.report = Some(report);
                self.scan_job = None;
                self.scan_cancel = None;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.scan_status = Some(ScanStatus::Failed(
                    "the scan worker disconnected unexpectedly".to_string(),
                ));
                self.scan_job = None;
                self.scan_cancel = None;
                true
            }
        }
    }

    /// The effective, evidence-derived group after any manual choice has
    /// been layered on top - never mutates `self.report`.
    pub(crate) fn effective_group(&self, group_index: usize) -> Option<ExactDuplicateGroup> {
        let group = self.report.as_ref()?.groups.get(group_index)?;
        if group.recommendation != CanonicalRecommendation::RequiresUserChoice {
            return Some(group.clone());
        }
        match self.manual_choice.get(&group_index) {
            Some(chosen) => apply_user_choice(group, chosen).ok(),
            // No choice made yet: still show the group as-is (needing a
            // choice), never hide it entirely.
            None => Some(group.clone()),
        }
    }

    /// Records a person's choice of which copy to keep for a group that
    /// needs one. Refused (silently, the choice simply is not recorded)
    /// for anything that is not `RequiresUserChoice` or is `Blocked` -
    /// this page never offers the control in that state, so reaching this
    /// with a stale index/click is treated as a no-op, not a crash.
    pub(crate) fn choose_retained(&mut self, group_index: usize, path: PathBuf) {
        let Some(report) = &self.report else { return };
        let Some(group) = report.groups.get(group_index) else {
            return;
        };
        if group.recommendation != CanonicalRecommendation::RequiresUserChoice {
            return;
        }
        if matches!(group.multi_file, MultiFileProtection::Blocked(_)) {
            return;
        }
        self.manual_choice.insert(group_index, path);
    }

    pub(crate) fn toggle_expanded(&mut self, group_index: usize) {
        self.expanded_group = if self.expanded_group == Some(group_index) {
            None
        } else {
            Some(group_index)
        };
    }

    pub(crate) fn is_expanded(&self, group_index: usize) -> bool {
        self.expanded_group == Some(group_index)
    }

    /// Opens the "Move N copies to quarantine" confirmation for one group,
    /// freezing exactly what will be quarantined - a no-op if the group is
    /// not actually ready.
    pub(crate) fn open_apply_confirmation(&mut self, group_index: usize) {
        if self.mode == DuplicateReviewMode::EquivalentN64 {
            let Some(group) = self
                .equivalent_report
                .as_ref()
                .and_then(|report| report.groups.get(group_index))
            else {
                return;
            };
            if group.quarantine_candidates.is_empty() || self.applied_equivalent {
                return;
            }
            self.apply_error = None;
            self.apply_confirm = Some(ApplyConfirmation {
                group_index,
                retained_path: group.preferred.clone(),
                redundant_count: group.quarantine_candidates.len(),
                reclaimable_bytes: group.projected_savings,
            });
            return;
        }
        if self.mode == DuplicateReviewMode::EquivalentOptical {
            let Some(group) = self
                .optical_report
                .as_ref()
                .and_then(|report| report.groups.get(group_index))
            else {
                return;
            };
            if group.quarantine_candidates.is_empty() || self.applied_equivalent {
                return;
            }
            self.apply_error = None;
            self.apply_confirm = Some(ApplyConfirmation {
                group_index,
                retained_path: group.preferred.clone(),
                redundant_count: group.quarantine_candidates.len(),
                reclaimable_bytes: group.projected_savings,
            });
            return;
        }
        let Some(group) = self.effective_group(group_index) else {
            return;
        };
        if group.readiness != GroupQuarantineReadiness::Safe {
            return;
        }
        let Some(retained_path) = group.recommendation.retained_path() else {
            return;
        };
        self.apply_error = None;
        self.apply_confirm = Some(ApplyConfirmation {
            group_index,
            retained_path: retained_path.to_path_buf(),
            redundant_count: group.redundant_paths.len(),
            reclaimable_bytes: group.reclaimable_bytes,
        });
    }

    pub(crate) fn cancel_apply_confirmation(&mut self) {
        self.apply_confirm = None;
    }

    pub(crate) fn apply_confirm(&self) -> Option<&ApplyConfirmation> {
        self.apply_confirm.as_ref()
    }

    /// Applies the confirmed group's quarantine, through the existing
    /// preview -> build -> apply quarantine transaction engine, unchanged.
    /// A group whose scan evidence has since gone stale (a source file
    /// changed since preview, or the group is no longer `Safe`) is refused
    /// with a plain reason rather than moved on stale authority.
    pub(crate) fn confirm_apply(&mut self, trusted_root: &Path) {
        let Some(confirmation) = self.apply_confirm.take() else {
            return;
        };
        if self.mode == DuplicateReviewMode::EquivalentN64 {
            let Some(group) = self
                .equivalent_report
                .as_ref()
                .and_then(|report| report.groups.get(confirmation.group_index))
                .cloned()
            else {
                self.apply_error =
                    Some("this equivalent-content group is no longer available".to_string());
                return;
            };
            let trusted = TrustedRoots::from_paths(self.trust_scope_dirs.clone());
            let cancel = AtomicBool::new(false);
            match apply_n64_equivalent_group(
                &group,
                trusted_root,
                trusted,
                &self.journal_dir,
                &cancel,
            ) {
                Ok(result) => {
                    self.applied = Some(result.transaction);
                    self.applied_equivalent = true;
                    self.apply_error = None;
                }
                Err(error) => self.apply_error = Some(format!("Nothing was moved: {error}")),
            }
            return;
        }
        if self.mode == DuplicateReviewMode::EquivalentOptical {
            let Some(group) = self
                .optical_report
                .as_ref()
                .and_then(|report| report.groups.get(confirmation.group_index))
                .cloned()
            else {
                self.apply_error = Some("this optical group is no longer available".to_string());
                return;
            };
            let trusted = TrustedRoots::from_paths(self.trust_scope_dirs.clone());
            let cancel = AtomicBool::new(false);
            match apply_optical_equivalent_group(
                &group,
                trusted_root,
                trusted,
                &self.journal_dir,
                &cancel,
            ) {
                Ok(result) => {
                    self.applied = Some(result.transaction);
                    self.applied_equivalent = true;
                    self.apply_error = None;
                }
                Err(error) => self.apply_error = Some(format!("Nothing was moved: {error}")),
            }
            return;
        }
        let Some(group) = self.effective_group(confirmation.group_index) else {
            self.apply_error = Some("this group is no longer part of the current scan".to_string());
            return;
        };
        if group.readiness != GroupQuarantineReadiness::Safe {
            self.apply_error = Some(
                "this group is no longer ready to quarantine; rescan before trying again"
                    .to_string(),
            );
            return;
        }

        let mut trust_scope = self.trust_scope_dirs.clone();
        trust_scope.push(trusted_root.to_path_buf());
        let trusted = TrustedRoots::from_paths(trust_scope);
        let cancel = AtomicBool::new(false);

        let proposals = match build_exact_duplicate_group_proposals(
            &group,
            trusted_root,
            &mut self.cache,
            &trusted,
            Some(&cancel),
        ) {
            Ok(proposals) => proposals,
            Err(error) => {
                self.apply_error = Some(plain_apply_error(&error));
                return;
            }
        };

        let Some(retained_path) = group.recommendation.retained_path() else {
            self.apply_error = Some("this group has no retained copy chosen".to_string());
            return;
        };

        let mut transaction = match build_quarantine_transaction(
            &proposals,
            retained_path,
            trusted_root,
            0,
            &mut self.cache,
            &trusted,
            Some(&cancel),
        ) {
            Ok(transaction) => transaction,
            Err(error) => {
                self.apply_error = Some(plain_apply_error(&error));
                return;
            }
        };

        if let Err(error) = std::fs::create_dir_all(&self.journal_dir) {
            self.apply_error = Some(format!(
                "could not create the quarantine journal folder: {error}"
            ));
            return;
        }

        match apply_quarantine_transaction(
            &mut transaction,
            retained_path,
            trusted_root,
            0,
            trusted,
            &self.journal_dir,
            &cancel,
            &mut self.cache,
        ) {
            Ok(_) => {
                self.applied = Some(transaction);
                self.apply_error = None;
                // The group just applied is stale now - remove it from the
                // live report so the UI never offers to apply it again
                // without a rescan.
                if let Some(report) = &mut self.report {
                    if confirmation.group_index < report.groups.len() {
                        report.groups[confirmation.group_index].readiness =
                            GroupQuarantineReadiness::NeedsReview(
                                "already quarantined in this session".to_string(),
                            );
                    }
                }
            }
            Err(error) => {
                self.apply_error = Some(plain_apply_error(&error.to_string()));
            }
        }
    }

    /// Rolls back the most recently applied transaction, restoring every
    /// moved file to its exact original path.
    pub(crate) fn rollback_last(&mut self, trusted_root: &Path) {
        let Some(mut transaction) = self.applied.take() else {
            self.rollback_error = Some("nothing to undo".to_string());
            return;
        };
        let cancel = AtomicBool::new(false);
        if self.applied_equivalent {
            let result = if self.mode == DuplicateReviewMode::EquivalentOptical {
                rollback_optical_equivalent_group(&mut transaction, &self.journal_dir, &cancel)
            } else {
                rollback_n64_equivalent_group(&mut transaction, &self.journal_dir, &cancel)
            };
            match result {
                Ok(_) => {
                    self.rollback_error = None;
                    self.applied_equivalent = false;
                }
                Err(error) => {
                    self.rollback_error = Some(error);
                    self.applied = Some(transaction);
                }
            }
            return;
        }
        match rollback_quarantine_transaction(
            &mut transaction,
            &self.journal_dir,
            &cancel,
            trusted_root,
        ) {
            Ok(_) => {
                self.rollback_error = None;
            }
            Err(error) => {
                self.rollback_error = Some(error);
                self.applied = Some(transaction);
            }
        }
    }

    /// Rolls back one interrupted transaction found by recovery.
    pub(crate) fn rollback_recovered(&mut self, index: usize, trusted_root: &Path) {
        let Some(recovery) = &mut self.recovery else {
            return;
        };
        if index >= recovery.recoverable.len() {
            return;
        }
        let mut transaction = recovery.recoverable.remove(index);
        let cancel = AtomicBool::new(false);
        match rollback_quarantine_transaction(
            &mut transaction,
            &self.journal_dir,
            &cancel,
            trusted_root,
        ) {
            Ok(_) => {
                self.rollback_error = None;
            }
            Err(error) => {
                self.rollback_error = Some(error);
                recovery.recoverable.insert(index, transaction);
            }
        }
    }
}

/// Turns a technical refusal string into one plain sentence up front,
/// keeping the exact original text available for "Technical details".
fn plain_apply_error(raw: &str) -> String {
    if raw.contains("changed between preview and apply")
        || raw.contains("no longer matches the SHA-256/size evidence")
    {
        format!(
            "One of these files changed since the scan, so nothing was moved. Scan again to \
             continue. (Technical detail: {raw})"
        )
    } else if raw.contains("collision") || raw.contains("already exists") {
        format!(
            "Something already exists at the quarantine destination, so nothing was moved. \
             (Technical detail: {raw})"
        )
    } else {
        format!("Nothing was moved: {raw}")
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Renders the whole page.
pub(crate) fn show_exact_duplicate_review_page(
    ui: &mut egui::Ui,
    state: &mut ExactDuplicateReviewPageState,
) {
    if state.poll_scan() {
        ui.ctx().request_repaint();
    }

    show_recovery_banner(ui, state);
    show_setup_card(ui, state);

    if let Some(status) = state.scan_status.clone() {
        show_scan_status(ui, &status);
    }

    if let Some(error) = state.error.clone() {
        ui.colored_label(theme::WARNING, error);
    }

    if state.mode == DuplicateReviewMode::EquivalentN64 {
        let group_count = state
            .equivalent_report
            .as_ref()
            .map_or(0, |r| r.groups.len());
        if group_count == 0 && state.scan_status == Some(ScanStatus::Completed) {
            ui.label("No equivalent N64 representations found.");
        }
        for index in 0..group_count {
            show_equivalent_group(ui, state, index);
        }
    } else if state.mode == DuplicateReviewMode::EquivalentOptical {
        let group_count = state
            .optical_report
            .as_ref()
            .map_or(0, |report| report.groups.len());
        if group_count == 0 && state.scan_status == Some(ScanStatus::Completed) {
            ui.label("No equivalent supported CUE/BIN and CHD discs found.");
        }
        for index in 0..group_count {
            show_optical_group(ui, state, index);
        }
    } else {
        let group_count = state.report.as_ref().map_or(0, |r| r.groups.len());
        for index in 0..group_count {
            show_group(ui, state, index);
        }
    }

    show_apply_confirmation_dialog(ui, state);

    if let Some(error) = state.apply_error.clone() {
        ui.colored_label(theme::WARNING, error);
    }

    if let Some(transaction) = state.applied.clone() {
        widgets::card(ui, |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Moved {} file(s) to quarantine. You can undo this.",
                    transaction
                        .entries
                        .iter()
                        .filter(|e| e.state
                            == archivefs_core::dat::rename_apply::model::EntryState::Applied)
                        .count()
                ))
                .color(theme::SUCCESS),
            );
            if widgets::action_button(
                ui,
                "Undo (roll back)",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                let trusted_root = PathBuf::from(state.trusted_root_draft.trim());
                let trusted_root = if trusted_root.as_os_str().is_empty() {
                    PathBuf::from(state.source_root_draft.trim())
                } else {
                    trusted_root
                };
                state.rollback_last(&trusted_root);
            }
        });
    }

    if let Some(error) = state.rollback_error.clone() {
        ui.colored_label(theme::WARNING, error);
    }
}

fn show_recovery_banner(ui: &mut egui::Ui, state: &mut ExactDuplicateReviewPageState) {
    let Some(recovery) = &state.recovery else {
        return;
    };
    if recovery.recoverable.is_empty() {
        return;
    }
    let count = recovery.recoverable.len();
    widgets::card(ui, |ui| {
        ui.colored_label(
            theme::WARNING,
            format!(
                "{count} quarantine move(s) were interrupted before this program closed last \
                 time. You can undo them."
            ),
        );
        for index in 0..count {
            if widgets::action_button(
                ui,
                format!("Undo interrupted quarantine #{}", index + 1),
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                let trusted_root = PathBuf::from(state.trusted_root_draft.trim());
                let trusted_root = if trusted_root.as_os_str().is_empty() {
                    PathBuf::from(state.source_root_draft.trim())
                } else {
                    trusted_root
                };
                state.rollback_recovered(index, &trusted_root);
                break;
            }
        }
    });
}

fn show_setup_card(ui: &mut egui::Ui, state: &mut ExactDuplicateReviewPageState) {
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Duplicate review").strong());
            if ui
                .selectable_label(state.mode == DuplicateReviewMode::Exact, "Exact duplicates")
                .clicked()
            {
                state.mode = DuplicateReviewMode::Exact;
                state.report = None;
                state.equivalent_report = None;
            }
            if ui
                .selectable_label(
                    state.mode == DuplicateReviewMode::EquivalentN64,
                    "Equivalent content (N64)",
                )
                .clicked()
            {
                state.mode = DuplicateReviewMode::EquivalentN64;
                state.report = None;
                state.equivalent_report = None;
                state.optical_report = None;
            }
            if ui
                .selectable_label(
                    state.mode == DuplicateReviewMode::EquivalentOptical,
                    "Equivalent content (optical disc)",
                )
                .clicked()
            {
                state.mode = DuplicateReviewMode::EquivalentOptical;
                state.report = None;
                state.equivalent_report = None;
                state.optical_report = None;
            }
        });
        if state.mode == DuplicateReviewMode::EquivalentN64 {
            ui.label("Find .z64, .v64 and .n64 files with different bytes but the same canonical N64 content.");
        } else if state.mode == DuplicateReviewMode::EquivalentOptical {
            ui.label("Find supported CUE/BIN and CHD discs whose optical fingerprints match. Only one MODE1/2048 data track is supported.");
        } else {
            ui.label("Choose a folder to scan for files that are byte-for-byte identical.");
        }
        ui.horizontal(|ui| {
            ui.label("Source folder:");
            ui.add(
                egui::TextEdit::singleline(&mut state.source_root_draft)
                    .id(egui::Id::new("exact_duplicate_source_root")),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Trusted folder (optional):");
            ui.add(
                egui::TextEdit::singleline(&mut state.trusted_root_draft)
                    .id(egui::Id::new("exact_duplicate_trusted_root")),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Already-organized library folder (optional):");
            ui.add(
                egui::TextEdit::singleline(&mut state.elected_library_draft)
                    .id(egui::Id::new("exact_duplicate_elected_root")),
            );
        });
        ui.horizontal(|ui| {
            let scanning = matches!(state.scan_status, Some(ScanStatus::Scanning));
            if widgets::action_button(ui, "Scan", widgets::ActionStyle::Primary, !scanning)
                .clicked()
            {
                state.scan();
            }
            if scanning
                && widgets::action_button(ui, "Cancel scan", widgets::ActionStyle::Secondary, true)
                    .clicked()
            {
                state.cancel_scan();
            }
        });
    });
}

fn show_scan_status(ui: &mut egui::Ui, status: &ScanStatus) {
    match status {
        ScanStatus::Scanning => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Scanning for exact copies...");
            });
        }
        ScanStatus::Completed => {}
        ScanStatus::Cancelled => {
            ui.colored_label(
                theme::muted(ui),
                "Scan cancelled. Nothing was changed. Results below are incomplete.",
            );
        }
        ScanStatus::Failed(error) => {
            ui.colored_label(theme::WARNING, format!("Scan failed: {error}"));
        }
    }
}

fn show_group(ui: &mut egui::Ui, state: &mut ExactDuplicateReviewPageState, index: usize) {
    let Some(group) = state.effective_group(index) else {
        return;
    };
    let raw_group = state
        .report
        .as_ref()
        .and_then(|r| r.groups.get(index))
        .cloned();
    let Some(raw_group) = raw_group else {
        return;
    };

    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("Exact copies").strong());
        ui.label(format!(
            "{} identical copies, {} each - {} reclaimable",
            group.members.len(),
            format_bytes(group.size_bytes),
            format_bytes(group.reclaimable_bytes)
        ));

        match &group.readiness {
            GroupQuarantineReadiness::Safe => {
                if let Some(retained) = group.recommendation.retained_path() {
                    ui.label(format!("Keep this copy: {}", retained.display()));
                    ui.label(group.recommendation.reason());
                }
                let count = group.redundant_paths.len();
                let noun = if count == 1 { "copy" } else { "copies" };
                if widgets::action_button(
                    ui,
                    format!("Move {count} {noun} to quarantine"),
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
                {
                    state.open_apply_confirmation(index);
                }
            }
            GroupQuarantineReadiness::Blocked(reason) => {
                ui.colored_label(
                    theme::WARNING,
                    plain_blocked_reason(&group.multi_file, reason),
                );
            }
            GroupQuarantineReadiness::NeedsReview(reason) => {
                if raw_group.recommendation == CanonicalRecommendation::RequiresUserChoice
                    && !matches!(raw_group.multi_file, MultiFileProtection::Blocked(_))
                {
                    ui.colored_label(
                        theme::WARNING,
                        "Needs your choice: which copy should be kept?",
                    );
                    for member in &raw_group.members {
                        if widgets::action_button(
                            ui,
                            format!("Keep this copy: {}", member.path.display()),
                            widgets::ActionStyle::Secondary,
                            true,
                        )
                        .clicked()
                        {
                            state.choose_retained(index, member.path.clone());
                        }
                    }
                } else {
                    ui.colored_label(theme::muted(ui), reason.clone());
                }
            }
        }

        if matches!(group.multi_file, MultiFileProtection::WholeReleaseDuplicate) {
            ui.label(
                "Protected because this belongs to a CUE/GDI/M3U game - the whole release will \
                 move together.",
            );
        }

        let expanded = state.is_expanded(index);
        if widgets::action_button(
            ui,
            if expanded {
                "Hide why"
            } else {
                "Why are these identical?"
            },
            widgets::ActionStyle::Quiet,
            true,
        )
        .clicked()
        {
            state.toggle_expanded(index);
        }
        if expanded {
            ui.label(format!(
                "Every copy has the same size ({}) and the same SHA-256 fingerprint, checked \
                 over the complete file.",
                format_bytes(group.size_bytes)
            ));
            for member in &group.members {
                ui.label(format!(
                    "- {} (trusted root: {}, in organized library: {})",
                    member.path.display(),
                    if member.in_trusted_root { "yes" } else { "no" },
                    if member.elected_in_library {
                        "yes"
                    } else {
                        "no"
                    }
                ));
            }
        }

        widgets::technical_details(ui, ("exact_duplicate_group", index), |ui| {
            ui.label(format!("SHA-256: {}", group.sha256));
            ui.label(format!("Size: {} bytes", group.size_bytes));
            ui.label(format!(
                "Legacy CRC32/MD5/SHA-1 (narrowing only): {} / {} / {}",
                group.legacy_crc32, group.legacy_md5, group.legacy_sha1
            ));
            ui.label(format!("Recommendation: {:?}", group.recommendation));
            ui.label(format!("Multi-file protection: {:?}", group.multi_file));
            ui.label(format!("Readiness: {:?}", group.readiness));
        });
    });
}

fn show_equivalent_group(
    ui: &mut egui::Ui,
    state: &mut ExactDuplicateReviewPageState,
    index: usize,
) {
    let Some(report) = state.equivalent_report.as_ref() else {
        return;
    };
    let Some(group) = report.groups.get(index) else {
        return;
    };
    let group = group.clone();
    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("Equivalent N64 content").strong());
        ui.label(format!(
            "{} byte-order representations · {} reclaimable",
            group.members.len(),
            format_bytes(group.projected_savings)
        ));
        ui.label(format!(
            "Preferred representation: {}",
            group.preferred.display()
        ));
        ui.label(format!("Canonical SHA-256: {}", group.canonical_sha256));
        for member in &group.members {
            ui.label(format!(
                "{} · {} · {} · physical SHA-256 {}",
                member.path.display(),
                member.byte_order.label(),
                format_bytes(member.size_bytes),
                member.physical_sha256
            ));
        }
        ui.label(format!(
            "Proposed quarantine: {}",
            group
                .quarantine_candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if !state.applied_equivalent
            && widgets::action_button(
                ui,
                "Move redundant representations to quarantine",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
        {
            state.open_apply_confirmation(index);
        }
        widgets::technical_details(ui, ("n64_equivalent_group", index), |ui| {
            ui.label("Physical hashes differ; canonical hashes match after the existing N64 byte-order normalization.");
            ui.label("Supported only for N64 .z64/.v64/.n64 representations in this review.");
        });
    });
}

fn show_optical_group(ui: &mut egui::Ui, state: &mut ExactDuplicateReviewPageState, index: usize) {
    let Some(report) = state.optical_report.as_ref() else {
        return;
    };
    let Some(group) = report.groups.get(index) else {
        return;
    };
    let group = group.clone();
    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("Equivalent optical content").strong());
        ui.label(format!(
            "CUE/BIN ↔ CHD · {} track · {} sectors · {} reclaimable",
            group.structure.track_count,
            group.structure.logical_sector_count,
            format_bytes(group.projected_savings)
        ));
        ui.label(format!("Canonical SHA-256: {}", group.canonical_sha256));
        ui.label(format!(
            "Preferred representation: {}",
            group.preferred.display()
        ));
        ui.label("The CUE and referenced BIN are one logical representation and are quarantined together.");
        for file in group.cue_bin.files.iter().chain(group.chd.files.iter()) {
            ui.label(format!(
                "{} · {} bytes · physical SHA-256 {}",
                file.path.display(),
                file.size_bytes,
                file.physical_sha256
            ));
        }
        if !state.applied_equivalent
            && widgets::action_button(
                ui,
                "Move redundant optical representation to quarantine",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
        {
            state.open_apply_confirmation(index);
        }
        widgets::technical_details(ui, ("optical_equivalent_group", index), |ui| {
            ui.label("Equivalence requires matching optical structure and canonical cooked-sector SHA-256.");
            ui.label("Supported only for one-track MODE1/2048 CUE/BIN and one-track zero-pregap MODE1_RAW CHD.");
        });
    });
}

fn plain_blocked_reason(multi_file: &MultiFileProtection, reason: &str) -> String {
    if let MultiFileProtection::Blocked(_) = multi_file {
        format!("Protected: {reason}")
    } else {
        format!("Blocked: {reason}")
    }
}

fn show_apply_confirmation_dialog(ui: &mut egui::Ui, state: &mut ExactDuplicateReviewPageState) {
    let Some(confirmation) = state.apply_confirm.clone() else {
        return;
    };
    let mut open = true;
    widgets::centered_window("Move copies to quarantine?")
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.label(format!(
                "Move {} {} to quarantine?",
                confirmation.redundant_count,
                if confirmation.redundant_count == 1 {
                    "copy"
                } else {
                    "copies"
                }
            ));
            ui.label(format!("Keeping: {}", confirmation.retained_path.display()));
            ui.label(format!(
                "This will free up {}.",
                format_bytes(confirmation.reclaimable_bytes)
            ));
            ui.label("Nothing is deleted. You can undo this afterward.");
            ui.horizontal(|ui| {
                if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    state.cancel_apply_confirmation();
                }
                if widgets::action_button(
                    ui,
                    "Move to quarantine",
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
                {
                    let trusted_root = PathBuf::from(state.trusted_root_draft.trim());
                    let trusted_root = if trusted_root.as_os_str().is_empty() {
                        PathBuf::from(state.source_root_draft.trim())
                    } else {
                        trusted_root
                    };
                    state.confirm_apply(&trusted_root);
                }
            });
        });
    if !open {
        state.cancel_apply_confirmation();
    }
}

#[cfg(test)]
mod tests;
