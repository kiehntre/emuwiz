//! Multi-platform RomM library plan: combines several already-projected,
//! per-platform RomM projections ([`super::romm_projection`]) into one flat,
//! deterministic, GUI-ready report spanning a whole collection.
//!
//! # Why this exists
//!
//! [`super::romm_projection::build_romm_projection`] already turns one
//! elected [`PlayingLibraryPlan`] plus one verified [`DatPlatformIdentity`]
//! into a RomM-ready symlink plan for that one platform - reused here
//! completely unchanged, including its slug mapping, its multi-file
//! (companion) handling, and its within-platform duplicate-destination
//! refusal. What did not exist is anything that plans a whole *collection*:
//! a novice pointing EmuWiz at several platform folders at once had no way
//! to get one combined report, and nothing checked a platform's planned
//! destinations against what is actually already on disk, or against
//! another platform's plan, before Apply.
//!
//! This module adds exactly that thin layer, and nothing else:
//!
//! - Loops over several `(label, plan, identity)` inputs, calling the
//!   existing single-platform projection for each. A platform whose
//!   projection fails (unsupported/ambiguous platform, unresolved Playing
//!   Library conflicts, ...) is recorded as [`RommLibraryBlockedPlatform`]
//!   and the rest of the collection is still planned - one bad platform
//!   never aborts the whole report.
//! - Flattens every successfully projected platform's games into one
//!   [`RommLibraryPlanEntry`] per file (launcher and companions alike),
//!   already shaped for a future GUI: source, destination, platform,
//!   elected game, operation kind, and an optional block reason.
//! - Adds two read-only filesystem checks neither the single-platform
//!   projection nor Playing Library performs at plan time:
//!   [`RommLibraryBlockReason::MissingSource`] (the source file is no
//!   longer there) and [`RommLibraryBlockReason::DestinationOccupied`] (the
//!   destination path already names something on disk that this plan did
//!   not put there). Both are `fs::symlink_metadata` reads only - nothing
//!   here creates, moves, or deletes anything, matching
//!   [`crate::library_views`]'s own "planning never mutates" convention.
//! - Adds [`RommLibraryBlockReason::UnsafeSource`], the same symlink-safety
//!   contract [`crate::safe_read`] already enforces authoritatively at
//!   apply time, applied here only as an early, advisory read (metadata,
//!   not an open) so a preview can warn about it before Apply refuses it.
//! - Adds [`RommLibraryBlockReason::DuplicateDestination`], which
//!   [`super::romm_projection`] cannot itself catch because it plans one
//!   platform at a time: two different source libraries both mistakenly
//!   mapped to the same platform (and therefore the same RomM slug) would
//!   otherwise silently race for one destination path.
//!
//! None of these checks change what a *successful* single-platform
//! projection looks like - the PS3/PSP/Xbox nested, multi-file layouts
//! [`super::romm_projection`] already produces are passed through byte for
//! byte (see this module's own tests).
//!
//! # Apply
//!
//! There is no new filesystem-mutation engine. [`build_romm_library_apply_transactions`]
//! simply calls the existing [`super::romm_projection::build_romm_projection`]
//! and [`super::romm_projection::build_romm_projection_transaction`] once per
//! platform that was not already blocked, and returns the resulting
//! [`crate::dat::rename_apply::model::RenameTransaction`] values - the exact
//! same durable-journal, no-clobber, rollback-safe engine every other
//! linked-library apply in this codebase already uses. Nothing here calls
//! `apply_transaction` itself; that remains the caller's job, exactly as it
//! is for a single-platform RomM projection today.
//!
//! The per-entry advisory blocks above are deliberately not threaded into
//! what gets applied: the authoritative no-clobber/missing-source refusal
//! already lives in [`crate::dat::rename_apply::preflight`] and runs again,
//! unchanged, when a transaction is actually applied. This module's checks
//! exist so a preview can *say so first*, not to replace that enforcement.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::dat::identity::DatPlatformIdentity;
use crate::dat::rename_apply::model::RenameTransaction;
use crate::safe_read::TrustedRoots;

use super::PlayingLibraryPlan;
use super::romm_projection::{
    RommLibraryProjectionPlan, RommVisibility, build_romm_projection,
    build_romm_projection_transaction, build_romm_projection_with_visibility,
};

/// One already-elected platform to fold into a combined RomM library plan.
///
/// Producing `plan` and `identity` is entirely the caller's job - scanning,
/// hash-matching against a DAT, and 1G1R election are unchanged and this
/// module never repeats or re-decides any of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommLibraryPlatformInput {
    /// A human-readable label for this platform/source in reports - the
    /// configured source folder's display name, for example. Never used to
    /// decide a platform or a destination; that is entirely `identity`'s
    /// job.
    pub label: String,
    pub plan: PlayingLibraryPlan,
    pub identity: DatPlatformIdentity,
}

/// How one planned entry would be materialised.
///
/// Playing Library's apply engine only ever creates a symlink - see
/// [`super::LinkedLibraryOperation`]'s own doc comment - so this has one
/// variant today. It stays an explicit enum (rather than being implied)
/// because a future GUI should never hard-code that assumption itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RommLibraryOperationKind {
    Symlink,
}

/// Why one planned entry is blocked from being safely applied. Never
/// resolved automatically - a blocked entry is reported, not silently
/// skipped, moved aside, or overwritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RommLibraryBlockReason {
    /// The source file this entry would link to no longer exists.
    MissingSource,
    /// The source is a symlink whose target could not be proven safe under
    /// the caller's [`TrustedRoots`] - broken, looping, outside every
    /// trusted root, or not a regular file.
    UnsafeSource { detail: String },
    /// Something already exists at the planned destination that this plan
    /// did not put there. Never overwritten - see this module's doc.
    DestinationOccupied,
    /// Another entry in this same combined plan - on a different platform
    /// input - already claims this exact destination path.
    DuplicateDestination { other_dat_entry_name: String },
}

impl RommLibraryBlockReason {
    pub fn label(&self) -> &'static str {
        match self {
            Self::MissingSource => "Missing source",
            Self::UnsafeSource { .. } => "Unsafe source",
            Self::DestinationOccupied => "Destination already occupied",
            Self::DuplicateDestination { .. } => "Duplicate planned destination",
        }
    }
}

/// One flattened, GUI-ready row: one file (a launcher or one companion) for
/// one elected game on one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommLibraryPlanEntry {
    pub platform_label: String,
    pub canonical_platform_id: String,
    pub romm_platform_slug: String,
    /// The elected DAT entry name this file belongs to - the game
    /// [`PlayingLibraryPlan`] chose via 1G1R election, reused unchanged.
    pub dat_entry_name: String,
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    pub operation: RommLibraryOperationKind,
    /// Whether this file is the launcher (the one a frontend should
    /// launch) or a companion file the release also requires.
    pub is_launcher: bool,
    /// `None` means this entry is ready to apply. `Some` names exactly why
    /// it is not, for a future GUI to display without re-deriving it.
    pub blocked: Option<RommLibraryBlockReason>,
}

/// One platform that could not be projected at all - not one blocked file,
/// but nothing about the platform could even be planned (unsupported or
/// ambiguous platform identity, an unresolved Playing Library conflict,
/// ...). Kept distinct from a per-entry block because there are no entries
/// to show for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommLibraryBlockedPlatform {
    pub label: String,
    pub reason: String,
}

/// The complete, read-only result of one combined planning run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommLibraryPlan {
    pub destination_root: PathBuf,
    /// One row per file across every successfully projected platform, in
    /// input order and then destination order - deterministic for the same
    /// inputs regardless of filesystem iteration order, since nothing here
    /// reads a directory listing.
    pub entries: Vec<RommLibraryPlanEntry>,
    pub blocked_platforms: Vec<RommLibraryBlockedPlatform>,
}

impl RommLibraryPlan {
    /// Entries with nothing blocking them.
    pub fn ready_entries(&self) -> impl Iterator<Item = &RommLibraryPlanEntry> {
        self.entries.iter().filter(|entry| entry.blocked.is_none())
    }

    /// Entries with a reported block.
    pub fn blocked_entries(&self) -> impl Iterator<Item = &RommLibraryPlanEntry> {
        self.entries.iter().filter(|entry| entry.blocked.is_some())
    }

    pub fn ready_count(&self) -> usize {
        self.ready_entries().count()
    }

    pub fn blocked_count(&self) -> usize {
        self.blocked_entries().count() + self.blocked_platforms.len()
    }
}

/// Builds a combined, multi-platform RomM plan from several already-elected
/// platform inputs.
///
/// Performs no filesystem mutation. The only I/O is
/// `fs::symlink_metadata`/`fs::canonicalize` reads used to classify what
/// already exists at each source and destination - the same read-only
/// contract [`crate::library_views::plan_library_view`] already documents
/// for the same reason.
///
/// `trusted` governs the advisory symlink-source check exactly the way it
/// governs every other read in this codebase - see [`TrustedRoots::none`]
/// for the fail-closed default when no roots are configured.
pub fn build_romm_library_plan(
    inputs: &[RommLibraryPlatformInput],
    destination_root: &Path,
    trusted: &TrustedRoots,
) -> Result<RommLibraryPlan, String> {
    if !destination_root.is_absolute() {
        return Err("the RomM destination root must be an absolute path".to_string());
    }

    let mut entries = Vec::new();
    let mut blocked_platforms = Vec::new();
    // Destination -> the first entry that claimed it, across every
    // platform, in the order platforms were supplied. Deliberately a
    // `BTreeMap` keyed by the destination path so lookups are independent
    // of hashing, keeping the whole report deterministic for identical
    // inputs.
    let mut claimed_destinations: BTreeMap<PathBuf, String> = BTreeMap::new();

    for input in inputs {
        let projection = match build_romm_projection(
            &input.plan,
            &input.identity,
            destination_root.to_path_buf(),
        ) {
            Ok(projection) => projection,
            Err(reason) => {
                blocked_platforms.push(RommLibraryBlockedPlatform {
                    label: input.label.clone(),
                    reason,
                });
                continue;
            }
        };

        for row in flatten_platform(
            &input.label,
            &projection,
            trusted,
            &mut claimed_destinations,
        ) {
            entries.push(row);
        }
    }

    Ok(RommLibraryPlan {
        destination_root: destination_root.to_path_buf(),
        entries,
        blocked_platforms,
    })
}

fn flatten_platform(
    label: &str,
    projection: &RommLibraryProjectionPlan,
    trusted: &TrustedRoots,
    claimed_destinations: &mut BTreeMap<PathBuf, String>,
) -> Vec<RommLibraryPlanEntry> {
    let mut rows = Vec::new();
    for game in &projection.games {
        let mut push_row = |operation: &super::LinkedLibraryOperation, is_launcher: bool| {
            let blocked = classify_entry(
                &operation.source_path,
                &operation.destination_path,
                &game.dat_entry_name,
                trusted,
                claimed_destinations,
            );
            rows.push(RommLibraryPlanEntry {
                platform_label: label.to_string(),
                canonical_platform_id: projection.canonical_platform_id.clone(),
                romm_platform_slug: projection.romm_platform_slug.clone(),
                dat_entry_name: game.dat_entry_name.clone(),
                source_path: operation.source_path.clone(),
                destination_path: operation.destination_path.clone(),
                operation: RommLibraryOperationKind::Symlink,
                is_launcher,
                blocked,
            });
        };
        push_row(&game.launcher, true);
        for companion in &game.companions {
            push_row(companion, false);
        }
    }
    rows
}

/// Classifies one planned entry against the filesystem and against every
/// destination already claimed earlier in this same combined plan. Read-only:
/// only `fs::symlink_metadata`/`fs::canonicalize`.
fn classify_entry(
    source: &Path,
    destination: &Path,
    dat_entry_name: &str,
    trusted: &TrustedRoots,
    claimed_destinations: &mut BTreeMap<PathBuf, String>,
) -> Option<RommLibraryBlockReason> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if let Err(detail) = check_symlink_source_safety(source, trusted) {
                return Some(RommLibraryBlockReason::UnsafeSource { detail });
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(RommLibraryBlockReason::MissingSource);
        }
        Err(_) => {
            return Some(RommLibraryBlockReason::MissingSource);
        }
    }

    if fs::symlink_metadata(destination).is_ok() {
        return Some(RommLibraryBlockReason::DestinationOccupied);
    }

    if let Some(other) = claimed_destinations.get(destination) {
        return Some(RommLibraryBlockReason::DuplicateDestination {
            other_dat_entry_name: other.clone(),
        });
    }
    claimed_destinations.insert(destination.to_path_buf(), dat_entry_name.to_string());
    None
}

/// A conservative, read-only proxy for [`crate::safe_read`]'s symlink
/// policy: the symlink must resolve (no break, no loop) and its canonical
/// target must lie inside a configured trusted root. This is advisory only
/// - the authoritative check runs again, unchanged, inside
/// [`crate::safe_read::open_bounded_read`] and the apply engine's own
/// preflight when the plan is actually applied.
fn check_symlink_source_safety(source: &Path, trusted: &TrustedRoots) -> Result<(), String> {
    if trusted.is_empty() {
        return Err(
            "no trusted roots are configured, so a symlinked source cannot be verified safe"
                .to_string(),
        );
    }
    let canonical = fs::canonicalize(source)
        .map_err(|error| format!("symlink target could not be resolved: {error}"))?;
    if !trusted.contains_canonical(&canonical) {
        return Err("symlink target resolves outside every trusted root".to_string());
    }
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|error| format!("symlink target could not be read: {error}"))?;
    if !metadata.is_file() {
        return Err("symlink target is not a regular file".to_string());
    }
    Ok(())
}

/// Builds one apply-ready [`RenameTransaction`] per platform that was not
/// already blocked in `plan`, reusing
/// [`super::romm_projection::build_romm_projection`] and
/// [`super::romm_projection::build_romm_projection_transaction`] completely
/// unchanged - no new filesystem-mutation engine, no new journal format.
///
/// A platform listed in `plan.blocked_platforms` is skipped entirely: it
/// was already refused at plan time, and nothing here retries it. This does
/// not re-run the per-entry advisory checks `plan` already reports (missing
/// source, unsafe source, occupied/duplicate destination) - those remain
/// advisory, and the authoritative refusal for any of them still happens
/// inside the reused apply engine's own preflight when the returned
/// transaction is applied.
pub fn build_romm_library_apply_transactions(
    plan: &RommLibraryPlan,
    inputs: &[RommLibraryPlatformInput],
    visibility: &RommVisibility,
    generation: u64,
) -> Vec<(String, Result<RenameTransaction, String>)> {
    let blocked_labels: std::collections::BTreeSet<&str> = plan
        .blocked_platforms
        .iter()
        .map(|blocked| blocked.label.as_str())
        .collect();

    inputs
        .iter()
        .filter(|input| !blocked_labels.contains(input.label.as_str()))
        .map(|input| {
            let result = build_romm_projection_with_visibility(
                &input.plan,
                &input.identity,
                plan.destination_root.clone(),
                visibility.clone(),
            )
            .and_then(|projection| build_romm_projection_transaction(&projection, generation));
            (input.label.clone(), result)
        })
        .collect()
}

#[cfg(test)]
mod tests;
