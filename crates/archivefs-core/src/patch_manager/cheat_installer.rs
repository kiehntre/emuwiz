//! Write-capable RetroArch cheat installer - the first milestone in this
//! codebase that actually creates a directory, writes a file, creates a
//! backup, or writes a journal for a RetroArch cheat.
//!
//! This module reuses, and never duplicates:
//!
//! - [`super::cheat_catalogue`]'s staging preview
//!   ([`CheatAvailabilityEntry`]/its `staging_plan`) for eligibility and
//!   the originally proposed destination - `retroarch-cheat-catalogue`
//!   itself remains strictly read-only; nothing there changed to add this
//!   module.
//! - [`super::destination_safety`]'s read-only, symlink-aware path
//!   validation ([`assess_destination`]) for every revalidation this
//!   module performs immediately before a write - that module's own doc
//!   comment says a future write-capable caller must revalidate
//!   immediately before writing; this is that caller. This module never
//!   reimplements symlink/traversal detection itself.
//! - [`super::cheat_install_result`]'s [`CheatInstallEntryResult`]/
//!   [`CheatInstallRun`]/[`CheatInstallSummary`]/[`CheatInstallRunStatus`]
//!   data model, and its pure [`plan_cheat_install_entry`] bridge, as the
//!   starting point for every result this module produces - this module
//!   only ever *upgrades* a planned result to a real one, or downgrades it
//!   to a `skipped_*`/`failed_*` outcome on revalidation failure. It never
//!   defines a second result or journal shape.
//! - [`crate::canonical_platform_for_alias`] (the exact alias table
//!   matching already uses) to reconstruct the destination's platform
//!   directory fresh from the catalogue record, rather than trusting the
//!   preview's own path string.
//! - [`HostReadOnlyFilesystem`] for every bounded read this module
//!   performs (source revalidation, destination re-hash, backup
//!   verification).
//!
//! ## Revalidation, not trust
//!
//! Every write in this module is preceded by: a fresh bounded read and
//! hash of the source file (rejecting `skipped_source_changed` on any
//! mismatch or read failure); a fresh [`assess_destination`] call
//! (rejecting `failed_unsafe_path` on any symlink/traversal/wrong-type
//! problem); and a fresh read/hash of any existing destination content
//! (rejecting `skipped_destination_changed` on any mismatch from what the
//! staging preview captured). The staging preview's own proposed path and
//! hashes are never trusted as still-current.
//!
//! ## Accepted residual TOCTOU gap
//!
//! Per [`super::destination_safety`]'s own documented limitation, no
//! amount of revalidation before an operation eliminates every race
//! between that check and the operation itself. This module narrows that
//! window as far as practical: every write lands first in a uniquely
//! named temporary file in the *same* directory as the final destination
//! (never a predictable shared path), is flushed and `fsync`ed, and is
//! hash-verified *before* ever being made visible at its final name via
//! [`rename_no_replace`] (a Linux `renameat2(..., RENAME_NOREPLACE)` call
//! that atomically refuses to clobber an unexpected concurrent creation,
//! falling back to an existence check plus `rename` elsewhere). The
//! original destination content for a replacement is never read into a
//! write path at all - only ever backed up (verified) and then atomically
//! replaced - so there is no scenario in which this module's own code
//! path can leave the original destination partially modified without
//! either a verified backup or a verified new file in its place.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::canonical_platform_for_alias;
use crate::emulator_environment::{
    BoundedReadResult, HostReadOnlyFilesystem, ReadOnlyHostFilesystem,
};

use super::anchored_fs::{DirectoryHandle, DirectoryIdentity};
use super::cheat_catalogue::{
    CheatAvailabilityEntry, CheatCatalogueFormat, MAX_CATALOGUE_FILE_BYTES,
    NO_INSTALLABLE_CHEATS_REASON, cheat_destination_file_name, render_installable_cheat_document,
};
use super::cheat_install_result::{
    CHEAT_INSTALL_RUN_SCHEMA_VERSION, CheatInstallEntryResult, CheatInstallOutcome,
    CheatInstallPath, CheatInstallRun, CheatInstallRunStatus, CheatInstallSummary,
    PreviousDestinationState, plan_cheat_install_entry,
};
use super::destination_safety::{
    DestinationSafetyFailureReason, DestinationState, assess_destination,
};

/// Directory name (beneath the EmuWiz XDG data directory) that holds one
/// journal file per real apply run.
pub const CHEAT_INSTALL_RUNS_DIRECTORY_NAME: &str = "cheat-install-runs";
/// Directory name (beneath the EmuWiz XDG data directory) that holds
/// pre-replacement backups.
pub const CHEAT_INSTALL_BACKUPS_DIRECTORY_NAME: &str = "cheat-install-backups";

// ---------------------------------------------------------------------
// Test-only fault injection - compiled only under `cfg(test)`; a release
// build contains no injection state and `should_inject` is a trivial
// always-`false` function the optimizer removes entirely.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaultPoint {
    BackupWrite,
    TempDestinationWrite,
    Rename,
    FinalVerification,
    /// The pending journal cannot be created before the first mutation.
    JournalWrite,
    /// The per-entry journal rewrite after entry `n` (0-based) fails.
    JournalUpdate(usize),
    /// The final journal rewrite (status + completion time) fails.
    JournalFinalize,
    /// The process dies after entry `n` was applied but before its journal
    /// update - modelled as a panic the test catches with `catch_unwind`.
    CrashBeforeJournalUpdate(usize),
    /// The process dies right after entry `n`'s journal update.
    CrashAfterJournalUpdate(usize),
}

#[cfg(test)]
type AnchorHook = Box<dyn Fn(&Path)>;

#[cfg(test)]
thread_local! {
    static INJECTED_FAULT: std::cell::Cell<Option<FaultPoint>> = const { std::cell::Cell::new(None) };
    /// Runs with the destination root right before the installer opens its
    /// directory handle for a write - the window in which a parent
    /// directory could be swapped for a symlink (Night Watch D7-03).
    static BEFORE_ANCHOR_HOOK: std::cell::RefCell<Option<AnchorHook>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_before_anchor_hook_for_test(hook: Option<AnchorHook>) {
    BEFORE_ANCHOR_HOOK.with(|cell| *cell.borrow_mut() = hook);
}

fn run_before_anchor_hook(_root: &Path) {
    #[cfg(test)]
    BEFORE_ANCHOR_HOOK.with(|cell| {
        if let Some(hook) = cell.borrow().as_ref() {
            hook(_root);
        }
    });
}

#[cfg(test)]
fn inject_fault_for_test(point: Option<FaultPoint>) {
    INJECTED_FAULT.with(|cell| cell.set(point));
}

#[cfg(test)]
fn should_inject(point: FaultPoint) -> bool {
    INJECTED_FAULT.with(|cell| cell.get() == Some(point))
}

#[cfg(not(test))]
fn should_inject(_point: FaultPoint) -> bool {
    false
}

// ---------------------------------------------------------------------
// Options and top-level entry point
// ---------------------------------------------------------------------

/// Every input [`execute_cheat_install_run`] needs, gathered in one place
/// so the pure/impure boundary is explicit: everything here is supplied by
/// the caller (the CLI in production; a test fixture in tests) - this
/// module never resolves a default path, reads `$HOME`, or opens the live
/// EmuWiz database itself.
pub struct CheatInstallOptions {
    pub destination_root: PathBuf,
    pub allow_replace_different: bool,
    pub dry_run: bool,
    /// Whether the caller passed `--yes`. When `false`, the run is always
    /// treated as a dry run regardless of `dry_run` - see
    /// [`execute_cheat_install_run`].
    pub confirmed: bool,
    pub journal_directory: PathBuf,
    pub backup_directory: PathBuf,
    pub run_id: String,
    pub started_at_unix_seconds: u64,
    pub catalogue_source: String,
}

pub struct CheatInstallRunOutcome {
    pub run: CheatInstallRun,
    /// Where the journal was written, for a real (non-dry-run) run whose
    /// journal exists on disk - including a run whose final write failed
    /// (the journal then still reads `in_progress`; see `journal_error`).
    /// `None` for a dry run and for a run that failed closed.
    pub journal_path: Option<PathBuf>,
    /// Set when a real run's journal could not be created or kept up to
    /// date. The run's own entries and summary are still fully valid and
    /// reflect exactly what was (or was not) written to the filesystem:
    /// when the *pending* journal could not be created, nothing was written
    /// at all (every eligible entry carries reason code
    /// `journal_unavailable`); when a mid-run update failed, no further
    /// entry was attempted (reason code `journal_update_failed`).
    pub journal_error: Option<String>,
}

/// Executes one full installation run: plans every entry (reusing
/// [`plan_cheat_install_entry`]), revalidates and, if eligible and
/// permitted, applies each independently, and keeps one journal file for
/// a real (non-dry-run, confirmed) run.
///
/// `options.confirmed` is the `--yes` gate: whenever it is `false`, this
/// function behaves *exactly* as a dry run (identical planned outcomes,
/// zero filesystem writes, `dry_run: true` on the returned run) regardless
/// of `options.dry_run` - the two are combined, not layered, so a caller
/// can never accidentally write by only checking one of them.
///
/// ## Crash-proof Undo (journal before the first mutation)
///
/// A real run never touches the destination before an Undo record exists:
///
/// 1. A *pending* journal (`status: in_progress`, every entry in its
///    planned state, `settled_entries: 0`) is written at the final journal
///    path before the first filesystem mutation. If that write fails the
///    run **fails closed**: nothing is written anywhere, every eligible
///    entry is reported `failed_write` with reason `journal_unavailable`,
///    and the run's status is `failed`.
/// 2. After each entry settles the journal is atomically rewritten
///    (temp file, fsync, rename over the pending file) with
///    `settled_entries` advanced. If that rewrite fails, no further entry
///    is attempted (each is reported with reason `journal_update_failed`)
///    so the on-disk journal never claims less than one entry behind the
///    truth, and never more than was done.
/// 3. The journal is finalised once with the derived status and
///    completion time.
///
/// A journal still reading `in_progress` therefore means the process died
/// (or the final write failed): `entries[..settled_entries]` are final,
/// `entries[settled_entries]` may or may not have been applied (only the
/// destination's current hash can tell), and later entries were never
/// attempted. `cheat_rollback` and `cheat_history` read it exactly that
/// way.
///
/// One entry failing never stops another from being processed (except for
/// the journal-update failure above): every entry in `entries` is
/// attempted independently, in the given (already-deterministic) order.
pub fn execute_cheat_install_run(
    entries: &[CheatAvailabilityEntry],
    options: &CheatInstallOptions,
) -> CheatInstallRunOutcome {
    let effective_dry_run = options.dry_run || !options.confirmed;
    let mut claimed_destinations: BTreeSet<PathBuf> = BTreeSet::new();

    if effective_dry_run {
        let results: Vec<CheatInstallEntryResult> = entries
            .iter()
            .map(|entry| install_one_entry(entry, options, true, &mut claimed_destinations))
            .collect();
        let run = finished_run(options, results, true);
        return CheatInstallRunOutcome {
            run,
            journal_path: None,
            journal_error: None,
        };
    }

    // --- 1. Plan every entry and write the pending journal first. ---
    let planned: Vec<CheatInstallEntryResult> = entries
        .iter()
        .map(|entry| {
            let mut result = plan_cheat_install_entry(entry, options.allow_replace_different);
            if result.outcome == CheatInstallOutcome::ReplacedWithBackup
                && let (Some(destination), Some(previous_hash)) = (
                    result.destination_path.as_ref(),
                    result.previous_destination_hash.as_deref(),
                )
            {
                result.planned_backup_path =
                    Some(CheatInstallPath::from_path(&planned_backup_path(
                        Path::new(&destination.display),
                        previous_hash,
                        &options.backup_directory,
                        &options.run_id,
                    )));
            }
            result
        })
        .collect();
    let summary = CheatInstallSummary::from_entries(&planned, false);
    let mut run = CheatInstallRun {
        schema_version: CHEAT_INSTALL_RUN_SCHEMA_VERSION,
        run_id: options.run_id.clone(),
        started_at_unix_seconds: options.started_at_unix_seconds,
        completed_at_unix_seconds: None,
        dry_run: false,
        allow_replace_different: options.allow_replace_different,
        destination_root: Some(CheatInstallPath::from_path(&options.destination_root)),
        catalogue_source: options.catalogue_source.clone(),
        entries: planned,
        summary,
        status: CheatInstallRunStatus::InProgress,
        settled_entries: 0,
        temp_file_prefix: Some(own_temp_file_prefix()),
    };

    // The installer lock is taken before the journal becomes visible and
    // held for the whole run, so a rollback can tell a live run from a
    // crashed one (the kernel releases the lock when the process dies).
    let lock_and_journal = InstallerLock::acquire(
        &options.journal_directory,
        &options
            .journal_directory
            .join(journal_file_name(&run.run_id)),
    )
    .and_then(|lock| {
        create_pending_journal(&run, &options.journal_directory).map(|path| (lock, path))
    });
    let (_installer_lock, journal_path) = match lock_and_journal {
        Ok(pair) => pair,
        Err(error) => {
            // Fail closed: no Undo record, no mutation.
            for entry in &mut run.entries {
                if is_actionable_plan(entry.outcome) {
                    entry.outcome = CheatInstallOutcome::FailedWrite;
                    entry.reason_code = JOURNAL_UNAVAILABLE_REASON.to_string();
                    entry.detail.push(
                        "not attempted: the install record (journal) could not be created \
                         before the first change, so nothing was written"
                            .to_string(),
                    );
                }
            }
            let run = finished_run(options, run.entries, false);
            return CheatInstallRunOutcome {
                run,
                journal_path: None,
                journal_error: Some(format!(
                    "nothing was written: the install record could not be created: {error}"
                )),
            };
        }
    };

    // --- 2. Apply entries in order, recording each as it settles. ---
    let mut journal_error: Option<String> = None;
    for (index, entry) in entries.iter().enumerate() {
        if let Some(error) = &journal_error {
            let planned = &mut run.entries[index];
            if is_actionable_plan(planned.outcome) {
                planned.outcome = CheatInstallOutcome::FailedWrite;
                planned.reason_code = JOURNAL_UPDATE_FAILED_REASON.to_string();
                planned.detail.push(format!(
                    "not attempted: an earlier install-record update failed ({error})"
                ));
            }
            continue;
        }
        let planned_backup_path = run.entries[index].planned_backup_path.take();
        let mut result = install_one_entry(entry, options, false, &mut claimed_destinations);
        result.planned_backup_path = planned_backup_path;
        run.entries[index] = result;
        if should_inject(FaultPoint::CrashBeforeJournalUpdate(index)) {
            panic!("injected crash after entry {index} was applied, before its journal update");
        }
        run.settled_entries = index + 1;
        run.summary = CheatInstallSummary::from_entries(&run.entries, false);
        if should_inject(FaultPoint::JournalUpdate(index)) {
            journal_error = Some(format!(
                "injected journal update failure after entry {index}"
            ));
        } else if let Err(error) = rewrite_journal(&run, &journal_path, &options.journal_directory)
        {
            journal_error = Some(format!(
                "install record could not be updated after entry {index}: {error}"
            ));
        }
        if should_inject(FaultPoint::CrashAfterJournalUpdate(index)) {
            panic!("injected crash right after entry {index}'s journal update");
        }
    }

    // --- 3. Finalise once. ---
    let run = finished_run(options, run.entries, false);
    let finalize = if should_inject(FaultPoint::JournalFinalize) {
        Err("injected journal finalize failure".to_string())
    } else {
        rewrite_journal(&run, &journal_path, &options.journal_directory)
    };
    match finalize {
        Ok(()) => CheatInstallRunOutcome {
            run,
            journal_path: Some(journal_path),
            journal_error,
        },
        // The last per-entry (`in_progress`) journal is still on disk and
        // lists every applied entry, so its path is still reported: Undo
        // stays possible from it.
        Err(error) => CheatInstallRunOutcome {
            run,
            journal_path: Some(journal_path),
            journal_error: Some(match journal_error {
                Some(earlier) => {
                    format!("{earlier}; final install-record write also failed: {error}")
                }
                None => format!(
                    "the changes were applied but the install record could not be finalised \
                     (it still reads in_progress): {error}"
                ),
            }),
        },
    }
}

/// Reason code on every eligible entry of a run that failed closed because
/// its pending journal could not be created - nothing was written.
pub const JOURNAL_UNAVAILABLE_REASON: &str = "journal_unavailable";
/// Reason code on every entry that was not attempted because an earlier
/// per-entry journal update failed.
pub const JOURNAL_UPDATE_FAILED_REASON: &str = "journal_update_failed";

fn is_actionable_plan(outcome: CheatInstallOutcome) -> bool {
    matches!(
        outcome,
        CheatInstallOutcome::InstalledNew
            | CheatInstallOutcome::AlreadyInstalled
            | CheatInstallOutcome::ReplacedWithBackup
    )
}

fn finished_run(
    options: &CheatInstallOptions,
    entries: Vec<CheatInstallEntryResult>,
    dry_run: bool,
) -> CheatInstallRun {
    let summary = CheatInstallSummary::from_entries(&entries, dry_run);
    let status = CheatInstallRunStatus::derive(&summary, dry_run);
    let completed_at_unix_seconds = current_unix_seconds().max(options.started_at_unix_seconds);
    let settled_entries = entries.len();
    CheatInstallRun {
        schema_version: CHEAT_INSTALL_RUN_SCHEMA_VERSION,
        run_id: options.run_id.clone(),
        started_at_unix_seconds: options.started_at_unix_seconds,
        completed_at_unix_seconds: Some(completed_at_unix_seconds),
        dry_run,
        allow_replace_different: options.allow_replace_different,
        destination_root: Some(CheatInstallPath::from_path(&options.destination_root)),
        catalogue_source: options.catalogue_source.clone(),
        entries,
        summary,
        status,
        settled_entries,
        temp_file_prefix: if dry_run {
            None
        } else {
            Some(own_temp_file_prefix())
        },
    }
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------
// Per-entry install
// ---------------------------------------------------------------------

fn install_one_entry(
    entry: &CheatAvailabilityEntry,
    options: &CheatInstallOptions,
    effective_dry_run: bool,
    claimed_destinations: &mut BTreeSet<PathBuf>,
) -> CheatInstallEntryResult {
    let mut result = plan_cheat_install_entry(entry, options.allow_replace_different);

    // `skipped_not_eligible` / `skipped_conflict` / `skipped_replace_not_allowed`
    // are already fully and correctly decided by the pure bridge above -
    // no filesystem work is ever needed (or performed) for them.
    if !matches!(
        result.outcome,
        CheatInstallOutcome::InstalledNew
            | CheatInstallOutcome::AlreadyInstalled
            | CheatInstallOutcome::ReplacedWithBackup
    ) {
        return result;
    }

    let record = &entry.game;
    let filesystem = HostReadOnlyFilesystem;

    // --- 1. Source revalidation: never trust the preview's captured hash. ---
    // `EncodedPath.display` is a lossy, non-reversible rendering whenever
    // `lossy` is true - reconstructing a `Path` from it would silently read
    // the wrong file (or nothing at all). Every source that reaches
    // `cheat_catalogue`'s own pipeline already has a non-lossy path (a
    // non-UTF-8 catalogue filename is skipped before a `CheatGameRecord` is
    // ever created), but this module never assumes that invariant holds
    // without checking.
    if record.source_file_path.lossy {
        return reject(
            result,
            CheatInstallOutcome::SkippedSourceChanged,
            "source_path_lossy_cannot_revalidate",
            "source path is only available in lossy form; cannot safely re-open it",
        );
    }
    let source_path = Path::new(&record.source_file_path.display);
    let source_bytes = match filesystem.read_bounded(source_path, MAX_CATALOGUE_FILE_BYTES) {
        BoundedReadResult::Ok(bytes) => bytes,
        _ => {
            return reject(
                result,
                CheatInstallOutcome::SkippedSourceChanged,
                "source_missing_or_unreadable",
                "source file could not be re-read immediately before install",
            );
        }
    };
    let observed_file_hash = hex_sha256(&source_bytes);
    // --- 1b. What will actually be written (Night Watch D7-01). ---
    // A `.cht` source is installed byte for byte, so the file's own hash
    // must equal the preview's. A JSON manifest describes many games in one
    // file: the manifest must be unchanged since the preview (its own hash
    // against the record), and the per-game `.cht` document rendered from
    // it must hash to exactly what the preview promised. The manifest's
    // bytes themselves are never written anywhere.
    let (install_bytes, install_hash) = match record.format {
        CheatCatalogueFormat::RetroarchChtDirectory => {
            if Some(observed_file_hash.as_str()) != result.expected_source_hash.as_deref() {
                result.observed_source_hash = Some(observed_file_hash);
                return reject(
                    result,
                    CheatInstallOutcome::SkippedSourceChanged,
                    "source_hash_changed_since_preview",
                    "source content changed since the catalogue was previewed",
                );
            }
            (source_bytes, observed_file_hash)
        }
        CheatCatalogueFormat::JsonManifest => {
            if Some(observed_file_hash.as_str()) != record.source_file_hash.as_deref() {
                result.observed_source_hash = Some(observed_file_hash);
                return reject(
                    result,
                    CheatInstallOutcome::SkippedSourceChanged,
                    "source_hash_changed_since_preview",
                    "the catalogue manifest changed since it was previewed",
                );
            }
            let Some(rendered) = render_installable_cheat_document(record) else {
                return reject(
                    result,
                    CheatInstallOutcome::SkippedSourceChanged,
                    NO_INSTALLABLE_CHEATS_REASON,
                    "this manifest game lists no cheat with a code, so there is nothing RetroArch could load; nothing was written",
                );
            };
            let rendered_hash = hex_sha256(&rendered);
            if Some(rendered_hash.as_str()) != result.expected_source_hash.as_deref() {
                result.observed_source_hash = Some(rendered_hash);
                return reject(
                    result,
                    CheatInstallOutcome::SkippedSourceChanged,
                    "rendered_document_differs_from_preview",
                    "the per-game cheat file rendered from the manifest differs from the one the preview showed",
                );
            }
            result.detail.push(format!(
                "rendered a per-game RetroArch .cht from the catalogue manifest (manifest sha256 {observed_file_hash}); the manifest itself is never installed"
            ));
            (rendered, rendered_hash)
        }
    };
    result.observed_source_hash = Some(install_hash.clone());
    let source_bytes = install_bytes;
    let observed_source_hash = install_hash;

    // --- 2. Reconstruct and revalidate the destination fresh, via destination_safety. ---
    let Some(platform_hint) = record.source_platform.as_deref() else {
        // Unreachable for a genuinely eligible entry (the staging preview
        // never reaches install_new/already_installed/replace_different
        // without a resolvable platform), but this module never assumes
        // that invariant holds without checking.
        return reject(
            result,
            CheatInstallOutcome::FailedUnsafePath,
            "source_platform_unresolved",
            "no platform hint available to reconstruct the destination",
        );
    };
    let Some(canonical_platform) = canonical_platform_for_alias(platform_hint) else {
        return reject(
            result,
            CheatInstallOutcome::FailedUnsafePath,
            "source_platform_unresolved",
            "platform hint does not resolve to a known canonical platform",
        );
    };
    // Night Watch D7-02: the written name is derived by the same helper
    // the preview used, so the two can never disagree; a name the helper
    // refuses (control characters, separators) is never written.
    let Some(file_name) = cheat_destination_file_name(record) else {
        return reject(
            result,
            CheatInstallOutcome::FailedUnsafePath,
            "destination_name_unsafe",
            "the game name is not a safe file name",
        );
    };

    let assessment = match assess_destination(
        &options.destination_root,
        OsStr::new(canonical_platform),
        OsStr::new(&file_name),
    ) {
        Ok(assessment) => assessment,
        Err(error) => {
            return reject(
                result,
                CheatInstallOutcome::FailedUnsafePath,
                &destination_safety_reason_code(error.reason),
                &format!("destination safety check failed: {error}"),
            );
        }
    };

    let resolved_destination_path = assessment.proposed_destination.path().to_path_buf();
    let resolved_destination = CheatInstallPath::from_path(&resolved_destination_path);
    if result.destination_path.as_ref() != Some(&resolved_destination) {
        // The approved plan is the previewed path; nothing else is ever
        // written (Night Watch D7-02).
        result.detail.push(format!(
            "destination re-resolved at apply time ({}) differs from the previewed path",
            resolved_destination.display
        ));
        return reject(
            result,
            CheatInstallOutcome::SkippedDestinationChanged,
            "destination_differs_from_preview",
            "the destination path no longer matches the approved preview; nothing was written",
        );
    }
    result.destination_path = Some(resolved_destination);
    let anchor = WriteAnchor {
        root: options.destination_root.clone(),
        platform: canonical_platform.to_string(),
        file_name: file_name.clone(),
    };

    // --- 3. Batch-level duplicate-destination defense in depth. ---
    if !claimed_destinations.insert(resolved_destination_path.clone()) {
        return reject(
            result,
            CheatInstallOutcome::SkippedConflict,
            "duplicate_destination_at_apply_time",
            "another entry in this run already claimed this destination",
        );
    }

    // --- 4. Confirm the destination's real state still matches what was planned. ---
    let matches_expected_state = matches!(
        (result.outcome, assessment.destination_state),
        (CheatInstallOutcome::InstalledNew, DestinationState::Absent)
            | (
                CheatInstallOutcome::AlreadyInstalled,
                DestinationState::RegularFile
            )
            | (
                CheatInstallOutcome::ReplacedWithBackup,
                DestinationState::RegularFile
            )
    );
    if !matches_expected_state {
        return reject(
            result,
            CheatInstallOutcome::SkippedDestinationChanged,
            "destination_state_changed_since_preview",
            &format!("destination is now {:?}", assessment.destination_state),
        );
    }

    let mut fresh_destination_hash = None;
    if assessment.destination_state == DestinationState::RegularFile {
        match filesystem.read_bounded(&resolved_destination_path, MAX_CATALOGUE_FILE_BYTES) {
            BoundedReadResult::Ok(bytes) => fresh_destination_hash = Some(hex_sha256(&bytes)),
            _ => {
                return reject(
                    result,
                    CheatInstallOutcome::SkippedDestinationChanged,
                    "destination_unreadable_at_apply_time",
                    "existing destination could not be re-read immediately before install",
                );
            }
        }
    }

    match result.outcome {
        CheatInstallOutcome::AlreadyInstalled => {
            if fresh_destination_hash.as_deref() != Some(observed_source_hash.as_str()) {
                return reject(
                    result,
                    CheatInstallOutcome::SkippedDestinationChanged,
                    "destination_hash_changed_since_preview",
                    "existing destination content no longer matches the source",
                );
            }
            result.previous_destination_hash = fresh_destination_hash;
            return result;
        }
        CheatInstallOutcome::ReplacedWithBackup
            if fresh_destination_hash.as_deref() != result.previous_destination_hash.as_deref() =>
        {
            return reject(
                result,
                CheatInstallOutcome::SkippedDestinationChanged,
                "destination_hash_changed_since_preview",
                "existing destination content changed since the catalogue was previewed",
            );
        }
        _ => {}
    }

    if effective_dry_run {
        // Every field above already reflects exactly what a real run would
        // attempt; `applied` stays `false` and nothing further happens.
        return result;
    }

    match result.outcome {
        CheatInstallOutcome::InstalledNew => {
            install_new_file(&mut result, &anchor, &source_bytes, &observed_source_hash);
        }
        CheatInstallOutcome::ReplacedWithBackup => {
            replace_with_backup(
                &mut result,
                &anchor,
                &resolved_destination_path,
                &source_bytes,
                &observed_source_hash,
                &options.backup_directory,
                &options.run_id,
            );
        }
        _ => unreachable!("already_installed already returned above"),
    }
    result
}

/// Where a write lands, as the three names the write is anchored to: the
/// assessed destination root, the canonical platform directory beneath it,
/// and the file name. Every write opens the root once (no-follow) and does
/// everything else relative to open directory descriptors, so a directory
/// swapped for a symlink after the assessment redirects nothing (Night
/// Watch D7-03).
struct WriteAnchor {
    root: PathBuf,
    platform: String,
    file_name: String,
}

/// Opens the platform directory for writing, creating it when `create` is
/// allowed. The root must already be the plain directory the assessment
/// saw (compared by device and inode after a no-follow open); the platform
/// directory is opened relative to that root and must be a plain directory
/// too.
fn open_platform_directory(
    anchor: &WriteAnchor,
    create: bool,
) -> Result<DirectoryHandle, (CheatInstallOutcome, String, String)> {
    run_before_anchor_hook(&anchor.root);
    let root_metadata = fs::symlink_metadata(&anchor.root).map_err(|error| {
        (
            CheatInstallOutcome::FailedUnsafePath,
            "destination_root_missing".to_string(),
            format!("{}: {error}", anchor.root.display()),
        )
    })?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err((
            CheatInstallOutcome::FailedUnsafePath,
            "destination_root_not_a_plain_directory".to_string(),
            format!("{} is not a plain directory", anchor.root.display()),
        ));
    }
    let root = DirectoryHandle::open_verified(
        &anchor.root,
        DirectoryIdentity::of_metadata(&root_metadata),
    )
    .map_err(|error| {
        (
            CheatInstallOutcome::FailedUnsafePath,
            "destination_root_replaced_before_write".to_string(),
            format!("{}: {error}", anchor.root.display()),
        )
    })?;
    let platform = OsStr::new(&anchor.platform);
    let opened = if create {
        root.open_or_create_child_dir(platform).map(|(dir, _)| dir)
    } else {
        root.open_child_dir(platform)
    };
    opened.map_err(|error| {
        let kind = error.kind();
        if kind == io::ErrorKind::NotFound || kind == io::ErrorKind::AlreadyExists {
            (
                CheatInstallOutcome::FailedWrite,
                "directory_creation_failed".to_string(),
                format!("{}/{}: {error}", anchor.root.display(), anchor.platform),
            )
        } else {
            (
                CheatInstallOutcome::FailedUnsafePath,
                "parent_symlink_rejected".to_string(),
                format!(
                    "{}/{} is not a plain directory (replaced since the check?): {error}",
                    anchor.root.display(),
                    anchor.platform
                ),
            )
        }
    })
}

/// Writes `bytes` to a brand-new temporary file inside `dir`, flushed and
/// hash-verified through the directory handle, with the same permission
/// bits as before. Returns the temporary name; on any failure the
/// temporary file is removed and nothing is left behind.
fn stage_in_directory(
    dir: &DirectoryHandle,
    bytes: &[u8],
    expected_hash: &str,
) -> Result<String, (CheatInstallOutcome, String, String)> {
    let temp_name = unique_temp_component("cheat-install");
    let temp = OsStr::new(&temp_name);
    if should_inject(FaultPoint::TempDestinationWrite) {
        return Err((
            CheatInstallOutcome::FailedWrite,
            "injected_temp_write_failure".to_string(),
            String::new(),
        ));
    }
    let written = (|| -> io::Result<()> {
        let mut file = dir.create_new_file(temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o644))?;
        }
        Ok(())
    })();
    if let Err(error) = written {
        let _ = dir.unlink(temp);
        return Err((
            CheatInstallOutcome::FailedWrite,
            "temp_file_write_failed".to_string(),
            error.to_string(),
        ));
    }
    if let Err(error) = verify_temp_file_hash_in(dir, temp, expected_hash) {
        let _ = dir.unlink(temp);
        return Err((
            CheatInstallOutcome::FailedVerification,
            "temp_file_hash_mismatch".to_string(),
            error,
        ));
    }
    Ok(temp_name)
}

/// Hashes `name` inside `dir` through the directory handle (never by path).
fn hash_in_directory(dir: &DirectoryHandle, name: &OsStr) -> Result<String, String> {
    use std::io::Read;
    let mut file = dir
        .open_file(name)
        .map_err(|error| format!("could not open for verification: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    let mut total = 0_usize;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not read for verification: {error}"))?;
        if read == 0 {
            break;
        }
        total += read;
        if total > MAX_CATALOGUE_FILE_BYTES {
            return Err("file grew past the catalogue size limit during verification".into());
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn verify_temp_file_hash_in(
    dir: &DirectoryHandle,
    name: &OsStr,
    expected_hash: &str,
) -> Result<(), String> {
    if should_inject(FaultPoint::FinalVerification) {
        return Err("injected verification failure".to_string());
    }
    let actual = hash_in_directory(dir, name)?;
    if actual == expected_hash {
        Ok(())
    } else {
        Err(format!(
            "verification hash mismatch: expected {expected_hash}, got {actual}"
        ))
    }
}

fn reject(
    mut result: CheatInstallEntryResult,
    outcome: CheatInstallOutcome,
    reason_code: &str,
    detail: &str,
) -> CheatInstallEntryResult {
    result.outcome = outcome;
    result.reason_code = reason_code.to_string();
    result.detail.push(detail.to_string());
    result
}

fn destination_safety_reason_code(reason: DestinationSafetyFailureReason) -> String {
    serde_json::to_string(&reason)
        .map(|json| json.trim_matches('"').to_string())
        .unwrap_or_else(|_| "unsafe_destination".to_string())
}

// ---------------------------------------------------------------------
// Real writes: install_new
// ---------------------------------------------------------------------

fn install_new_file(
    result: &mut CheatInstallEntryResult,
    anchor: &WriteAnchor,
    source_bytes: &[u8],
    source_hash: &str,
) {
    // The destination root itself may be absent for a first-ever install
    // (the assessment approved creating it); every level beneath it is
    // created relative to the opened root, never by path.
    if fs::symlink_metadata(&anchor.root).is_err()
        && let Err((outcome, reason, detail)) = create_directory_chain(&anchor.root)
    {
        result.outcome = outcome;
        result.reason_code = reason;
        result.detail.push(detail);
        return;
    }
    let dir = match open_platform_directory(anchor, true) {
        Ok(dir) => dir,
        Err((outcome, reason, detail)) => {
            result.outcome = outcome;
            result.reason_code = reason;
            result.detail.push(detail);
            return;
        }
    };
    let temp_name = match stage_in_directory(&dir, source_bytes, source_hash) {
        Ok(name) => name,
        Err((outcome, reason, detail)) => {
            result.outcome = outcome;
            result.reason_code = reason;
            if !detail.is_empty() {
                result.detail.push(detail);
            }
            return;
        }
    };
    let temp = OsStr::new(&temp_name);
    let final_name = OsStr::new(&anchor.file_name);

    // Revalidate the exact final name immediately before rename, through
    // the same handle: it must still be genuinely absent.
    match dir.lstat(final_name) {
        Ok(None) => {}
        _ => {
            let _ = dir.unlink(temp);
            result.outcome = CheatInstallOutcome::SkippedDestinationChanged;
            result.reason_code = "destination_appeared_before_rename".to_string();
            result
                .detail
                .push("something occupied the destination immediately before rename".to_string());
            return;
        }
    }

    if should_inject(FaultPoint::Rename) {
        let _ = dir.unlink(temp);
        result.outcome = CheatInstallOutcome::FailedWrite;
        result.reason_code = "injected_rename_failure".to_string();
        return;
    }
    if let Err(error) = dir.rename_no_replace(temp, final_name) {
        let _ = dir.unlink(temp);
        result.outcome = CheatInstallOutcome::FailedWrite;
        result.reason_code = "rename_failed".to_string();
        result.detail.push(error.to_string());
        return;
    }
    if let Err(error) = dir.sync() {
        result
            .detail
            .push(format!("directory fsync failed (non-fatal): {error}"));
    }

    result.resulting_destination_hash = Some(source_hash.to_string());
    result.previous_destination_state = PreviousDestinationState::Absent;
    result.applied = true;
}

// ---------------------------------------------------------------------
// Real writes: replace_different (backup, then replace)
// ---------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn replace_with_backup(
    result: &mut CheatInstallEntryResult,
    anchor: &WriteAnchor,
    destination: &Path,
    source_bytes: &[u8],
    source_hash: &str,
    backup_directory: &Path,
    run_id: &str,
) {
    let filesystem = HostReadOnlyFilesystem;
    let previous_hash = result.previous_destination_hash.clone().unwrap_or_default();

    if let Err(error) = fs::create_dir_all(backup_directory) {
        result.outcome = CheatInstallOutcome::FailedBackup;
        result.reason_code = "backup_directory_creation_failed".to_string();
        result.detail.push(error.to_string());
        return;
    }

    if should_inject(FaultPoint::BackupWrite) {
        result.outcome = CheatInstallOutcome::FailedBackup;
        result.reason_code = "injected_backup_write_failure".to_string();
        return;
    }

    let current_destination_bytes =
        match filesystem.read_bounded(destination, MAX_CATALOGUE_FILE_BYTES) {
            BoundedReadResult::Ok(bytes) => bytes,
            _ => {
                result.outcome = CheatInstallOutcome::SkippedDestinationChanged;
                result.reason_code = "destination_unreadable_before_backup".to_string();
                return;
            }
        };
    if hex_sha256(&current_destination_bytes) != previous_hash {
        result.outcome = CheatInstallOutcome::SkippedDestinationChanged;
        result.reason_code = "destination_hash_changed_before_backup".to_string();
        return;
    }

    let backup_final_path =
        planned_backup_path(destination, &previous_hash, backup_directory, run_id);
    let backup_temp_path = backup_directory.join(unique_temp_component("cheat-backup"));

    if let Err(error) = write_new_temp_file(&backup_temp_path, &current_destination_bytes) {
        result.outcome = CheatInstallOutcome::FailedBackup;
        result.reason_code = "backup_temp_write_failed".to_string();
        result.detail.push(error.to_string());
        return;
    }
    if let Err(error) = verify_backup_file_hash(&backup_temp_path, &previous_hash) {
        let _ = fs::remove_file(&backup_temp_path);
        result.outcome = CheatInstallOutcome::FailedBackup;
        result.reason_code = "backup_hash_mismatch".to_string();
        result.detail.push(error);
        return;
    }
    if backup_final_path.exists() {
        let _ = fs::remove_file(&backup_temp_path);
        result.outcome = CheatInstallOutcome::FailedBackup;
        result.reason_code = "backup_already_exists".to_string();
        return;
    }
    if let Err(error) = rename_no_replace(&backup_temp_path, &backup_final_path) {
        let _ = fs::remove_file(&backup_temp_path);
        result.outcome = CheatInstallOutcome::FailedBackup;
        result.reason_code = "backup_finalize_failed".to_string();
        result.detail.push(error.to_string());
        return;
    }
    fsync_directory_best_effort(backup_directory, &mut result.detail);
    result.backup_path = Some(CheatInstallPath::from_path(&backup_final_path));

    // A verified backup now durably exists. Only now install the
    // replacement, using the exact same temp-write/verify/revalidate
    // sequence as a new install - the original destination content is
    // never opened for writing at any point; it is only ever read (above,
    // to create the backup) or atomically replaced (below).
    // A verified backup now durably exists. Only now install the
    // replacement, through the same anchored sequence as a new install.
    // Publication is an atomic exchange: the displaced file is re-hashed
    // under the temporary name (where nothing writing by path can reach
    // it) and the exchange is reversed if it is not the exact file that
    // was backed up - the original content is never opened for writing
    // and never overwritten unverified.
    let backup_note = format!(
        "original preserved; backup at {}",
        backup_final_path.display()
    );
    let dir = match open_platform_directory(anchor, false) {
        Ok(dir) => dir,
        Err((outcome, reason, detail)) => {
            result.outcome = outcome;
            result.reason_code = reason;
            result.detail.push(detail);
            result.detail.push(backup_note);
            return;
        }
    };
    let temp_name = match stage_in_directory(&dir, source_bytes, source_hash) {
        Ok(name) => name,
        Err((outcome, reason, detail)) => {
            result.outcome = outcome;
            result.reason_code = reason;
            if !detail.is_empty() {
                result.detail.push(detail);
            }
            result.detail.push(backup_note);
            return;
        }
    };
    let temp = OsStr::new(&temp_name);
    let final_name = OsStr::new(&anchor.file_name);

    match hash_in_directory(&dir, final_name) {
        Ok(hash) if hash == previous_hash => {}
        _ => {
            let _ = dir.unlink(temp);
            result.outcome = CheatInstallOutcome::SkippedDestinationChanged;
            result.reason_code = "destination_changed_before_replace".to_string();
            result.detail.push(format!(
                "backup preserved at {}",
                backup_final_path.display()
            ));
            return;
        }
    }

    if should_inject(FaultPoint::Rename) {
        let _ = dir.unlink(temp);
        result.outcome = CheatInstallOutcome::FailedWrite;
        result.reason_code = "injected_rename_failure".to_string();
        result.detail.push(backup_note);
        return;
    }
    if let Err(error) = dir.exchange(temp, final_name) {
        let _ = dir.unlink(temp);
        result.outcome = CheatInstallOutcome::FailedWrite;
        result.reason_code = "rename_failed".to_string();
        result.detail.push(error.to_string());
        result.detail.push(backup_note);
        return;
    }
    // `temp` now names the displaced file. Only the exact backed-up bytes
    // may be discarded.
    match hash_in_directory(&dir, temp) {
        Ok(hash) if hash == previous_hash => {
            let _ = dir.unlink(temp);
        }
        _ => match dir.exchange(temp, final_name) {
            Ok(()) => {
                let _ = dir.unlink(temp);
                result.outcome = CheatInstallOutcome::SkippedDestinationChanged;
                result.reason_code = "destination_changed_before_replace".to_string();
                result.detail.push(
                    "the file changed between the check and the replacement; it was put back untouched"
                        .to_string(),
                );
                result.detail.push(backup_note);
                return;
            }
            Err(error) => {
                result.outcome = CheatInstallOutcome::FailedWrite;
                result.reason_code = "displaced_file_kept".to_string();
                result.detail.push(format!(
                    "the file changed between the check and the replacement and could not be swapped back ({error}); the changed file is kept at {}",
                    destination.with_file_name(&temp_name).display()
                ));
                result.detail.push(backup_note);
                return;
            }
        },
    }
    if let Err(error) = dir.sync() {
        result
            .detail
            .push(format!("directory fsync failed (non-fatal): {error}"));
    }
    result.resulting_destination_hash = Some(source_hash.to_string());
    result.applied = true;
}

/// The deterministic backup path a `replaced_with_backup` entry uses:
/// `{file_name}.{safe_run_id}.{hash_prefix}.bak` beneath `backup_directory`.
/// Recorded in the pending journal (as `planned_backup_path`) before any
/// mutation and used verbatim by [`replace_with_backup`], so a crash-time
/// reader and the writer can never disagree about where a backup went.
fn planned_backup_path(
    destination: &Path,
    previous_hash: &str,
    backup_directory: &Path,
    run_id: &str,
) -> PathBuf {
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cheat".to_string());
    let hash_prefix: String = previous_hash.chars().take(12).collect();
    let safe_run_id = sanitize_for_filename(run_id);
    backup_directory.join(format!("{file_name}.{safe_run_id}.{hash_prefix}.bak"))
}

/// The prefix shared by every destination-side temporary file this process
/// creates (see [`unique_temp_component`]); recorded in the journal so a
/// rollback of an interrupted run can sweep exactly this run's leftovers.
fn own_temp_file_prefix() -> String {
    format!(".archivefs-cheat-install-{}-", std::process::id())
}

/// The advisory lock file an installer holds for its whole run, beside
/// its journal: `<journal dir>/.<journal file name>.lock`.
pub(crate) fn installer_lock_path(journal_path: &Path) -> PathBuf {
    let name = journal_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    journal_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(".{name}.lock"))
}

/// Result of probing an install journal's lock from another run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallerLockState {
    /// A live installer holds the lock: its run is still in progress.
    HeldByLiveInstaller,
    /// No lock file (finished run, or a journal older than the lock) or the
    /// lock was free (the installer died - the kernel released it).
    NotHeld,
}

/// Exclusive `flock` held by an installer for the whole run. Dropping it
/// unlinks the lock file, then releases the lock.
pub(crate) struct InstallerLock {
    path: PathBuf,
    _file: File,
}

impl InstallerLock {
    fn acquire(journal_directory: &Path, journal_path: &Path) -> Result<Self, String> {
        fs::create_dir_all(journal_directory)
            .map_err(|error| format!("could not create journal directory: {error}"))?;
        let path = installer_lock_path(journal_path);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("could not create the installer lock: {error}"))?;
        match try_lock_exclusive(&file) {
            Ok(true) => Ok(Self { path, _file: file }),
            Ok(false) => Err(format!(
                "another run already holds the installer lock {}",
                path.display()
            )),
            Err(error) => Err(format!("could not take the installer lock: {error}")),
        }
    }

    /// Probes (non-blocking) whether a live installer holds the lock for
    /// `journal_path`. On `NotHeld` with an existing lock file, the
    /// returned guard keeps the lock for the caller's lifetime.
    pub(crate) fn probe(journal_path: &Path) -> io::Result<(InstallerLockState, Option<File>)> {
        let path = installer_lock_path(journal_path);
        let file = match OpenOptions::new().read(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((InstallerLockState::NotHeld, None));
            }
            Err(error) => return Err(error),
        };
        if try_lock_exclusive(&file)? {
            Ok((InstallerLockState::NotHeld, Some(file)))
        } else {
            Ok((InstallerLockState::HeldByLiveInstaller, None))
        }
    }
}

impl Drop for InstallerLock {
    fn drop(&mut self) {
        // Unlink while still holding the lock: anyone who opens the path
        // afterwards finds nothing, anyone who opened it before gets the
        // lock only once this run is done.
        let _ = fs::remove_file(&self.path);
    }
}

/// `flock(LOCK_EX | LOCK_NB)`: `Ok(false)` when another open file holds it.
#[cfg(unix)]
pub(crate) fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: `file` owns a valid descriptor for the duration of the call.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(not(unix))]
pub(crate) fn try_lock_exclusive(_file: &File) -> io::Result<bool> {
    Ok(true)
}

fn journal_file_name(run_id: &str) -> String {
    format!("{}.json", sanitize_for_filename(run_id))
}

/// Creates the pending journal at its final path, refusing to clobber an
/// existing journal with the same run id. Durable before it returns.
fn create_pending_journal(
    run: &CheatInstallRun,
    journal_directory: &Path,
) -> Result<PathBuf, String> {
    if should_inject(FaultPoint::JournalWrite) {
        return Err("injected journal write failure".to_string());
    }
    fs::create_dir_all(journal_directory)
        .map_err(|error| format!("could not create journal directory: {error}"))?;
    let final_path = journal_directory.join(journal_file_name(&run.run_id));
    if fs::symlink_metadata(&final_path).is_ok() {
        return Err(format!(
            "a journal already exists at {}",
            final_path.display()
        ));
    }
    let temp_path = write_journal_temp(run, journal_directory)?;
    if let Err(error) = rename_no_replace(&temp_path, &final_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("could not finalize journal: {error}"));
    }
    let mut detail = Vec::new();
    fsync_directory_best_effort(journal_directory, &mut detail);
    Ok(final_path)
}

/// Atomically replaces this run's own journal with a newer state: temp
/// file, fsync, rename over the existing file. A clobbering rename is
/// correct here - the file being replaced is the one this run created
/// moments ago - and `rewrite` never produces a journal that claims more
/// than the run has actually done.
fn rewrite_journal(
    run: &CheatInstallRun,
    journal_path: &Path,
    journal_directory: &Path,
) -> Result<(), String> {
    let temp_path = write_journal_temp(run, journal_directory)?;
    if let Err(error) = fs::rename(&temp_path, journal_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("could not replace journal: {error}"));
    }
    let mut detail = Vec::new();
    fsync_directory_best_effort(journal_directory, &mut detail);
    Ok(())
}

fn write_journal_temp(run: &CheatInstallRun, journal_directory: &Path) -> Result<PathBuf, String> {
    let json = serde_json::to_string_pretty(run)
        .map_err(|error| format!("could not serialize journal: {error}"))?;
    let temp_path = journal_directory.join(unique_temp_component("cheat-journal"));
    write_new_temp_file(&temp_path, json.as_bytes())
        .map_err(|error| format!("could not write journal temp file: {error}"))?;
    Ok(temp_path)
}

// ---------------------------------------------------------------------
// Small filesystem/utility helpers
// ---------------------------------------------------------------------

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

static TEMP_NAME_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique, non-predictable temporary filename component - never a shared
/// path, never reused between calls even within the same process/second.
fn unique_temp_component(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = TEMP_NAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        ".archivefs-{prefix}-{}-{nanos:x}-{sequence:x}",
        std::process::id()
    )
}

fn sanitize_for_filename(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "run".to_string()
    } else {
        sanitized
    }
}

fn write_new_temp_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Verifies a backup temp file - deliberately not gated by the "final
/// verification" fault point (that name refers to the replacement
/// content's own verification, not the backup's); a backup-specific
/// failure is instead covered by [`FaultPoint::BackupWrite`], checked
/// earlier in the backup sequence.
fn verify_backup_file_hash(path: &Path, expected_hash: &str) -> Result<(), String> {
    verify_file_hash_unconditionally(path, expected_hash)
}

fn verify_file_hash_unconditionally(path: &Path, expected_hash: &str) -> Result<(), String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("could not re-read temp file for verification: {error}"))?;
    let actual = hex_sha256(&bytes);
    if actual == expected_hash {
        Ok(())
    } else {
        Err(format!(
            "verification hash mismatch: expected {expected_hash}, got {actual}"
        ))
    }
}

fn fsync_directory_best_effort(directory: &Path, detail: &mut Vec<String>) {
    match File::open(directory) {
        Ok(file) => {
            if let Err(error) = file.sync_all() {
                detail.push(format!("directory fsync failed (non-fatal): {error}"));
            }
        }
        Err(error) => detail.push(format!(
            "could not open directory for fsync (non-fatal): {error}"
        )),
    }
}

/// Creates every missing directory level from the filesystem root down
/// through `target_dir`, one level at a time, revalidating each with
/// `symlink_metadata` immediately after creating it. `fs::create_dir`
/// (never `create_dir_all`) is used for each level: on Unix it fails with
/// `AlreadyExists` if anything - including a symlink - already occupies
/// the path, so a symlink placed there by a concurrent process between our
/// check and our create is never silently traversed.
fn create_directory_chain(target_dir: &Path) -> Result<(), (CheatInstallOutcome, String, String)> {
    let mut current = PathBuf::new();
    for component in target_dir.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => continue,
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err((
                    CheatInstallOutcome::FailedUnsafePath,
                    "parent_symlink_rejected".to_string(),
                    format!("{} is a symlink, not a plain directory", current.display()),
                ));
            }
            Ok(_) => {
                return Err((
                    CheatInstallOutcome::FailedUnsafePath,
                    "parent_not_a_directory".to_string(),
                    format!("{} exists but is not a plain directory", current.display()),
                ));
            }
            Err(_) => create_one_directory_level(&current)?,
        }
    }
    Ok(())
}

fn create_one_directory_level(path: &Path) -> Result<(), (CheatInstallOutcome, String, String)> {
    if let Err(error) = fs::create_dir(path)
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        return Err((
            CheatInstallOutcome::FailedWrite,
            "directory_creation_failed".to_string(),
            format!("{}: {error}", path.display()),
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        _ => Err((
            CheatInstallOutcome::FailedUnsafePath,
            "directory_revalidation_failed".to_string(),
            format!(
                "{} is not a plain directory immediately after creation",
                path.display()
            ),
        )),
    }
}

/// Atomically renames `from` to `to`, refusing to clobber an unexpected
/// concurrent creation at `to`. Uses Linux's `renameat2(...,
/// RENAME_NOREPLACE)` where available; elsewhere, falls back to an
/// existence check immediately before a plain `rename` (a small, accepted
/// TOCTOU gap - see the module doc comment).
#[cfg(target_os = "linux")]
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from_c = std::ffi::CString::new(from.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let to_c = std::ffi::CString::new(to.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `from_c`/`to_c` are valid, NUL-terminated C strings owned for
    // the duration of this call; `AT_FDCWD` with absolute paths is a
    // documented, safe `renameat2` usage.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return Ok(());
    }
    // As in `rename_exchange`: a filesystem without RENAME_NOREPLACE is
    // reported as `Unsupported` so callers can choose a safe fallback.
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::ENOTSUP) => {
            Err(io::Error::new(io::ErrorKind::Unsupported, error))
        }
        _ => Err(error),
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists));
    }
    fs::rename(from, to)
}

#[cfg(test)]
mod tests;
