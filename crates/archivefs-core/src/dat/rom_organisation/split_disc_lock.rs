//! Movement locks for disc sets an earlier operation left not whole.
//!
//! [`crate::dat::rename_apply::split_disc_set_places`] finds, per History
//! operation, the disc sets whose files are not all at their new place or
//! all at their old place. The set's main file (its `.cue`, `.m3u` or `.gdi`)
//! may be at neither place only because "Rename this game's file" renamed it
//! afterwards, once or several times, in its own folder. This module follows
//! exactly those renames through History and releases the lock when the set
//! is then whole. Nothing else counts: a later History move of any other
//! file of the set, or of the main file to another folder, leaves the set
//! locked, however valid its record. Apply and Undo keep their own strict
//! checks (`split_disc_set_reports`, identity checks); nothing here relaxes
//! them, and diagnosis only reads file metadata (never contents) and never
//! moves, restores or replaces anything.
//!
//! A rename counts only on full evidence, every step in History: a later
//! RenameMove, applied and not undone, from exactly the place the file was
//! last recorded at, into the same folder, of the same file (device and
//! inode) unchanged (kind, size and modification time), no earlier than the
//! step before it (the first step no earlier than the moment the earlier
//! operation put the file there); and the file is at the last destination
//! now, still the same and unchanged. More than one candidate record, a
//! record of this place that does not match the file, or a record that says
//! a move was undone while the file is not back, keeps the set locked.
//!
//! When a set stays locked, the message names the History operation that
//! left it split and offers only steps checked to work against the files as
//! they are now; otherwise it says why none can be offered.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::dat::rename_apply::model::{
    EntryState, ObjectIdentity, RenameTransaction, TransactionEntry, TransactionOperation,
    TransactionState,
};

/// Why a file is locked: its set, the History operation that left the set
/// not whole, and what the person can do that will work.
#[derive(Debug, Clone)]
pub(crate) struct SplitSetLock {
    pub lead: String,
    pub operation: String,
    pub recovery: String,
}

impl SplitSetLock {
    /// The plain reason shown on a blocked row or a refused Apply.
    pub(crate) fn reason(&self) -> String {
        format!(
            "this file belongs to the disc set of {}, which History operation {} left with its files in two places, so Folder platform check, Rename verified games and Quick Rename will not move it until that set is whole again. {}",
            self.lead, self.operation, self.recovery
        )
    }
}

/// Every recorded path (old and new) of every disc set left not whole in
/// `journals`, after following later renames of its main file, plus the
/// current path of any of its files History moved on.
pub(crate) fn split_set_locks(journals: &[RenameTransaction]) -> BTreeMap<PathBuf, SplitSetLock> {
    let moves = LaterMoves::new(journals);
    let mut locks = BTreeMap::new();
    for journal in journals {
        for (lead, places) in crate::dat::rename_apply::split_disc_set_places(journal) {
            let fates: Vec<Fate> = places
                .iter()
                .map(|(entry, place)| match place {
                    Some(at_new) => Fate::Here { at_new: *at_new },
                    None => moves.follow(journal, entry, &lead),
                })
                .collect();
            if is_whole(&places, &fates) {
                continue;
            }
            let lock = SplitSetLock {
                lead,
                operation: journal.transaction_id.clone(),
                recovery: recovery(journal, &places, &fates),
            };
            for ((entry, _), fate) in places.iter().zip(&fates) {
                for path in [&entry.source_path, &entry.destination_path] {
                    locks.insert(path.clone(), lock.clone());
                }
                if let Fate::Trail { now, .. } = fate {
                    locks.insert(now.clone(), lock.clone());
                }
            }
        }
    }
    locks
}

/// One later History move of a file.
#[derive(Debug, Clone, Copy)]
struct Step<'a> {
    tx: &'a RenameTransaction,
    entry: &'a TransactionEntry,
}

/// Where one file of the set is, in the set's own terms.
#[derive(Debug)]
enum Fate<'a> {
    /// At its recorded new (`true`) or old (`false`) place, by device and
    /// inode.
    Here { at_new: bool },
    /// Moved on from its recorded new or old place by these later History
    /// moves (oldest first), and that file, unchanged, is at `now`.
    /// `renamed`: only same-folder renames of the set's main file, from the
    /// place History says it was at, so the set is still together.
    Trail {
        from_new: bool,
        steps: Vec<Step<'a>>,
        now: PathBuf,
        renamed: bool,
    },
    /// Nothing at the place History last put it, and no later record.
    Missing { expected: PathBuf },
    /// A different file at the place History last put it.
    Replaced { expected: PathBuf },
    /// History does not show where it went.
    Unclear { why: String },
}

/// Which recorded place the entry's state says the file is at (`true` new),
/// and since when; `None` when an Apply or Undo of it did not finish.
fn recorded_side(entry: &TransactionEntry) -> Option<(bool, Option<u64>)> {
    match entry.state {
        EntryState::Applied => Some((true, entry.applied_at_unix)),
        EntryState::RolledBack => Some((false, entry.rolled_back_at_unix)),
        EntryState::Planned
        | EntryState::PreflightPassed
        | EntryState::Skipped
        | EntryState::ApplyFailed => Some((false, None)),
        EntryState::Applying | EntryState::RollingBack | EntryState::RollbackFailed => None,
    }
}

/// What History says about a step of this file that did not finish
/// (`None` for every finished state). Undo settles or retries such a file
/// by its own checks when it runs, so no steps are worked out here.
fn unfinished(entry: &TransactionEntry, file: &str) -> Option<String> {
    match entry.state {
        EntryState::RollbackFailed => Some(format!(
            "an earlier Undo of {file} did not finish, so whether Undo would finish it now cannot be worked out reliably here (Undo checks it again when it runs)"
        )),
        EntryState::RollingBack => Some(format!(
            "an Undo of {file} was interrupted, so whether Undo would finish it now cannot be worked out reliably here (Undo checks it again when it runs)"
        )),
        EntryState::Applying => Some(format!(
            "the move of {file} was interrupted while it was being applied, so where History leaves it cannot be worked out reliably here"
        )),
        _ => None,
    }
}

/// How History records a later move that does not lead to the file.
fn record_state(step: &Step) -> &'static str {
    if step.tx.state == TransactionState::RolledBack || step.entry.state == EntryState::RolledBack {
        "says its move of it from {at} was undone, but it is not back there"
    } else {
        match step.entry.state {
            EntryState::RollbackFailed => {
                "records a failed Undo of its move of it from {at}, and it is not there"
            }
            EntryState::RollingBack => {
                "records an interrupted Undo of its move of it from {at}, and it is not there"
            }
            EntryState::Applying => {
                "records an interrupted move of it from {at}, and it is not there"
            }
            _ => "records a move of it from {at} that did not complete, and it is not there",
        }
    }
}

enum Trail<'a> {
    None,
    Found { steps: Vec<Step<'a>>, now: PathBuf },
    Unclear(String),
}

/// Every later RenameMove History records, indexed by the place it moved a
/// file from.
struct LaterMoves<'a> {
    from: BTreeMap<&'a Path, Vec<Step<'a>>>,
}

impl<'a> LaterMoves<'a> {
    fn new(journals: &'a [RenameTransaction]) -> Self {
        let mut from: BTreeMap<&Path, Vec<Step>> = BTreeMap::new();
        for tx in journals {
            for entry in &tx.entries {
                if matches!(entry.operation, TransactionOperation::RenameMove)
                    && entry.applied_at_unix.is_some()
                {
                    from.entry(entry.source_path.as_path())
                        .or_default()
                        .push(Step { tx, entry });
                }
            }
        }
        Self { from }
    }

    /// Where `entry`'s file (of `journal`, in the set of `lead`) went.
    fn follow(
        &self,
        journal: &'a RenameTransaction,
        entry: &'a TransactionEntry,
        lead: &str,
    ) -> Fate<'a> {
        let Some((at_new, since)) = recorded_side(entry) else {
            return Fate::Unclear {
                why: unfinished(entry, "it").unwrap_or_default(),
            };
        };
        let place = |new: bool| {
            if new {
                &entry.destination_path
            } else {
                &entry.source_path
            }
        };
        let not_before = since.unwrap_or(0).max(journal.created_at_unix);
        match self.trail(journal, entry, place(at_new), not_before) {
            Trail::Found { steps, now } => {
                let renamed = entry.source_path.to_string_lossy() == lead
                    && steps.iter().all(|step| {
                        step.entry.source_path.parent() == step.entry.destination_path.parent()
                    });
                Fate::Trail {
                    from_new: at_new,
                    steps,
                    now,
                    renamed,
                }
            }
            Trail::Unclear(why) => Fate::Unclear { why },
            // Not where its own record says: it may have been moved by hand
            // to the other place and on from there. That is never a rename
            // that keeps the set together, only described.
            Trail::None => {
                match self.trail(journal, entry, place(!at_new), journal.created_at_unix) {
                    Trail::Found { steps, now } => Fate::Trail {
                        from_new: !at_new,
                        steps,
                        now,
                        renamed: false,
                    },
                    Trail::Unclear(why) => Fate::Unclear { why },
                    Trail::None => {
                        let expected = place(at_new).clone();
                        if occupied(&expected) {
                            Fate::Replaced { expected }
                        } else {
                            Fate::Missing { expected }
                        }
                    }
                }
            }
        }
    }

    /// The later moves of the file from `start`, and where it is now.
    fn trail(
        &self,
        journal: &RenameTransaction,
        entry: &TransactionEntry,
        start: &Path,
        not_before: u64,
    ) -> Trail<'a> {
        let mut at = start.to_path_buf();
        let mut identity = &entry.identity;
        let mut not_before = not_before;
        let mut steps: Vec<Step<'a>> = Vec::new();
        let mut seen = BTreeSet::from([journal.transaction_id.clone()]);
        loop {
            let records: Vec<Step<'a>> = self
                .from
                .get(at.as_path())
                .into_iter()
                .flatten()
                .copied()
                .filter(|step| {
                    !seen.contains(&step.tx.transaction_id)
                        && step.tx.created_at_unix >= journal.created_at_unix
                        && step.entry.applied_at_unix.is_some_and(|t| t >= not_before)
                })
                .collect();
            let happened = |step: &Step| {
                step.tx.state != TransactionState::RolledBack
                    && step.entry.state == EntryState::Applied
            };
            let matching: Vec<Step<'a>> = records
                .iter()
                .copied()
                .filter(|step| {
                    happened(step) && same_unchanged_file(identity, &step.entry.identity)
                })
                .collect();
            match matching.as_slice() {
                [] => {
                    if is_there_unchanged(&at, identity) {
                        return if steps.is_empty() {
                            Trail::None
                        } else {
                            Trail::Found { steps, now: at }
                        };
                    }
                    // Records of this place that do not lead anywhere: a
                    // move of this file said to be undone, or a move of a
                    // file that is not this one as History recorded it.
                    if let Some(step) = records.first() {
                        let why = if same_unchanged_file(identity, &step.entry.identity) {
                            format!(
                                "History operation {} {}",
                                step.tx.transaction_id,
                                record_state(step).replace("{at}", &at.display().to_string())
                            )
                        } else {
                            format!(
                                "History operation {} moved a file on from {}, but not this file as History recorded it",
                                step.tx.transaction_id,
                                at.display()
                            )
                        };
                        return Trail::Unclear(why);
                    }
                    return if steps.is_empty() {
                        Trail::None
                    } else {
                        Trail::Unclear(format!(
                            "History says it was moved to {}, but it is not there unchanged",
                            at.display()
                        ))
                    };
                }
                [step] => {
                    seen.insert(step.tx.transaction_id.clone());
                    steps.push(*step);
                    at = step.entry.destination_path.clone();
                    identity = &step.entry.identity;
                    not_before = step.entry.applied_at_unix.unwrap_or(not_before);
                }
                several => {
                    let ids: Vec<&str> = several
                        .iter()
                        .map(|step| step.tx.transaction_id.as_str())
                        .collect();
                    return Trail::Unclear(format!(
                        "more than one History operation ({}) moved it on from {}",
                        ids.join(", "),
                        at.display()
                    ));
                }
            }
        }
    }
}

/// The same file (device and inode), not changed (kind, size, modification
/// time): metadata only, the content is never read.
fn same_unchanged_file(expected: &ObjectIdentity, recorded: &ObjectIdentity) -> bool {
    #[cfg(unix)]
    {
        expected.dev == recorded.dev
            && expected.ino == recorded.ino
            && expected.kind == recorded.kind
            && expected.size_bytes == recorded.size_bytes
            && expected.modified_unix == recorded.modified_unix
    }
    #[cfg(not(unix))]
    {
        let _ = (expected, recorded);
        false
    }
}

/// Whether the file recorded as `identity` is at `path` now, unchanged.
fn is_there_unchanged(path: &Path, identity: &ObjectIdentity) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(path).is_ok_and(|now| {
            now.dev() == identity.dev
                && now.ino() == identity.ino
                && now.len() == identity.size_bytes
                && now.mtime() == identity.modified_unix
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (path, identity);
        false
    }
}

fn occupied(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn free(path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

fn place(entry: &TransactionEntry, new: bool) -> &Path {
    if new {
        &entry.destination_path
    } else {
        &entry.source_path
    }
}

/// Whole: every file on one side (by device and inode, the main file
/// possibly renamed in its folder), or whole by place on one side (as
/// [`crate::dat::rename_apply::split_disc_set_paths`] judges it, the main
/// file's rename followed).
fn is_whole(places: &[(&TransactionEntry, Option<bool>)], fates: &[Fate]) -> bool {
    let together = |side: bool| {
        fates.iter().all(|fate| match fate {
            Fate::Here { at_new } => *at_new == side,
            Fate::Trail {
                from_new, renamed, ..
            } => *renamed && *from_new == side,
            _ => false,
        })
    };
    together(true)
        || together(false)
        || whole_by_place(places, fates, true, &BTreeSet::new())
        || whole_by_place(places, fates, false, &BTreeSet::new())
}

/// Every file has something at its place on `side` (or is the main file
/// renamed from there), and nothing is at any place on the other side;
/// `assume` are places counted as filled (a copy put back).
fn whole_by_place(
    places: &[(&TransactionEntry, Option<bool>)],
    fates: &[Fate],
    side: bool,
    assume: &BTreeSet<PathBuf>,
) -> bool {
    places.iter().zip(fates).all(|((entry, _), fate)| {
        let here = place(entry, side);
        let there = place(entry, !side);
        let filled = matches!(fate, Fate::Here { at_new } if *at_new == side)
            || matches!(fate, Fate::Trail { from_new, renamed: true, .. } if *from_new == side)
            || occupied(here)
            || assume.contains(here);
        filled && !occupied(there) && !assume.contains(there)
    })
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Something that stops a recovery step from working.
enum Blocker {
    /// A different file is where a step would put one.
    Occupied(PathBuf),
    /// Anything else, in plain words.
    Other(String),
}

/// What the person can do that will work, or why nothing can be offered.
fn recovery(
    journal: &RenameTransaction,
    places: &[(&TransactionEntry, Option<bool>)],
    fates: &[Fate],
) -> String {
    let id = &journal.transaction_id;
    let mut problems = Vec::new();
    let mut copies = BTreeSet::new();
    let mut copy_side = BTreeSet::new();
    for ((entry, _), fate) in places.iter().zip(fates) {
        let file = name(&entry.source_path);
        match fate {
            Fate::Missing { expected } => {
                problems.push(format!(
                    "{file} is missing: nothing is at {}, where History last put it",
                    expected.display()
                ));
                copies.insert(expected.clone());
                copy_side.insert(expected == &entry.destination_path);
            }
            Fate::Replaced { expected } => problems.push(format!(
                "{file} has been replaced: {} holds a different file than the one History recorded",
                expected.display()
            )),
            Fate::Unclear { why } => {
                problems.push(format!("History does not show where {file} went: {why}"))
            }
            Fate::Here { .. } | Fate::Trail { .. } => {
                if let Some(why) = unfinished(entry, &file) {
                    problems.push(why);
                }
            }
        }
    }
    if !problems.is_empty() {
        let only_missing = copies.len() == problems.len() && copy_side.len() == 1;
        if only_missing
            && copy_side
                .first()
                .is_some_and(|side| whole_by_place(places, fates, *side, &copies))
        {
            let at: Vec<String> = copies
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            return format!(
                "{}. Undo of History operation {id} cannot put back a file that is missing. If you have a copy (for example from a backup), put it at {}: the set is then whole by place and this lock lifts. A copy is a different file to History: it does not restore the original file's identity, and Undo of {id} will not put this set back afterwards.",
                problems.join("; "),
                at.join(" and ")
            );
        }
        return format!(
            "{}. No steps that would work can be worked out from History, so no Undo is suggested; check the set's files by hand.",
            problems.join("; ")
        );
    }
    let all_applied = places
        .iter()
        .all(|(entry, _)| entry.state == EntryState::Applied);
    // Whole at the new place is the shorter way when History moved every
    // file; otherwise Undo puts the set back at its old place.
    let plans = if all_applied {
        vec![
            plan(journal, places, fates, true),
            plan(journal, places, fates, false),
        ]
    } else {
        vec![plan(journal, places, fates, false)]
    };
    let mut first_blocker = None;
    for planned in plans {
        match planned {
            Ok((steps, to_new)) => {
                let numbered: Vec<String> = steps
                    .iter()
                    .enumerate()
                    .map(|(index, step)| format!("Step {}: {step}", index + 1))
                    .collect();
                return format!(
                    "To make it whole, in this order: {} The set is then whole at its {} place and this lock lifts. These steps were checked against where the files are and their size and modification time now, not their contents, so they are not guaranteed to succeed: every Undo still asks for confirmation, makes its normal content and identity checks, and changes nothing if a file has changed.",
                    numbered.join(" "),
                    if to_new { "new" } else { "old" }
                );
            }
            Err(blocker) => {
                first_blocker.get_or_insert(blocker);
            }
        }
    }
    match first_blocker {
        Some(Blocker::Occupied(path)) => format!(
            "{} is occupied by a different file, and nothing will overwrite it, so no steps that would work can be offered while it is there. If that file does not belong there, move it away yourself; this message then shows the steps that finish the set.",
            path.display()
        ),
        Some(Blocker::Other(why)) => format!(
            "{why}. No steps that would work can be worked out from History, so no Undo is suggested; check the set's files by hand."
        ),
        None => "No steps that would work can be worked out from History, so no Undo is suggested; check the set's files by hand.".into(),
    }
}

/// The steps that make the set whole on `to_new`'s side, each checked
/// against the files as they are now.
fn plan(
    journal: &RenameTransaction,
    places: &[(&TransactionEntry, Option<bool>)],
    fates: &[Fate],
    to_new: bool,
) -> Result<(Vec<String>, bool), Blocker> {
    let id = &journal.transaction_id;
    let mut trails: Vec<&[Step]> = Vec::new();
    let mut manual = Vec::new();
    let mut undo_first = false;
    for ((entry, _), fate) in places.iter().zip(fates) {
        // Where the file is once later moves of it are undone.
        let at_new = match fate {
            Fate::Here { at_new } => *at_new,
            Fate::Trail {
                from_new,
                steps,
                renamed,
                ..
            } => {
                // A renamed main file can stay renamed when the set ends at
                // the side it was renamed on; otherwise its renames go first.
                if !(*renamed && *from_new == to_new) {
                    trails.push(steps);
                }
                *from_new
            }
            _ => return Err(Blocker::Other("a file of the set cannot be found".into())),
        };
        let applied = entry.state == EntryState::Applied;
        let renamed_stays =
            matches!(fate, Fate::Trail { renamed: true, from_new, .. } if *from_new == to_new);
        if to_new {
            if at_new {
                if !renamed_stays
                    && let Fate::Here { .. } = fate
                    && !is_there_unchanged(&entry.destination_path, &entry.identity)
                {
                    return Err(Blocker::Other(format!(
                        "{} has changed since History moved it",
                        entry.destination_path.display()
                    )));
                }
                continue;
            }
            if !applied {
                return Err(Blocker::Other(format!(
                    "History never moved {}",
                    name(&entry.source_path)
                )));
            }
            manual.push((entry.source_path.clone(), entry.destination_path.clone()));
        } else if at_new {
            if applied {
                // Undo of the operation puts it back.
                if !free(&entry.source_path) {
                    return Err(Blocker::Occupied(entry.source_path.clone()));
                }
                if let Fate::Here { .. } = fate
                    && !is_there_unchanged(&entry.destination_path, &entry.identity)
                {
                    return Err(Blocker::Other(format!(
                        "{} has changed since History moved it, so Undo of History operation {id} would keep this set where it is",
                        entry.destination_path.display()
                    )));
                }
                undo_first = true;
            } else {
                manual.push((entry.destination_path.clone(), entry.source_path.clone()));
            }
        } else if applied {
            // Undo only puts back a file that is at its new place: it goes
            // there first, then Undo returns the set together.
            manual.push((entry.source_path.clone(), entry.destination_path.clone()));
            undo_first = true;
        }
    }
    for (_, to) in &manual {
        if !free(to) {
            return Err(Blocker::Occupied(to.clone()));
        }
    }
    let order = undo_order(&trails)?;
    // A step of a followed trail ends where the next step starts: once the
    // newer steps are undone, the file is back at its destination (the last
    // destination was checked when the trail was followed).
    let chained: BTreeSet<*const TransactionEntry> = trails
        .iter()
        .flat_map(|trail| trail.iter().map(|step| std::ptr::from_ref(step.entry)))
        .collect();
    // Every place a file of this set is recorded at or is now: an Undo of a
    // later operation that would also move a file at one of them (other
    // than along a followed trail) could split the set again.
    let set_places: BTreeSet<&Path> = places
        .iter()
        .flat_map(|(entry, _)| {
            [
                entry.source_path.as_path(),
                entry.destination_path.as_path(),
            ]
        })
        .chain(fates.iter().flat_map(|fate| {
            match fate {
                Fate::Trail { steps, .. } => steps
                    .iter()
                    .flat_map(|step| {
                        [
                            step.entry.source_path.as_path(),
                            step.entry.destination_path.as_path(),
                        ]
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            }
        }))
        .collect();
    let mut impacts = Vec::new();
    for later in &order {
        impacts.push(undo_would_work(later, &chained, &set_places)?);
    }
    if undo_first
        && !matches!(
            journal.state,
            TransactionState::Applied
                | TransactionState::ApplyFailed
                | TransactionState::RollbackFailed
        )
    {
        return Err(Blocker::Other(format!(
            "History operation {id} cannot be undone in its current state"
        )));
    }
    let mut steps = Vec::new();
    for (later, impact) in order.iter().zip(impacts) {
        steps.push(undo_step(later, "a later operation", impact));
    }
    for (from, to) in &manual {
        steps.push(format!(
            "Move {} yourself from {} to {}.",
            name(from),
            from.display(),
            to.display()
        ));
    }
    if undo_first {
        steps.push(undo_step(
            journal,
            "the operation that left the set split",
            own_impact(journal, places),
        ));
    }
    Ok((steps, to_new))
}

/// The later operations to undo, newest first: a later move of a file is
/// undone before an earlier move of the same file; otherwise newest first by
/// when it was applied, then created, then its id, so the order is the same
/// every time even when History records the same second.
fn undo_order<'a>(trails: &[&[Step<'a>]]) -> Result<Vec<&'a RenameTransaction>, Blocker> {
    let mut nodes: BTreeMap<&str, &RenameTransaction> = BTreeMap::new();
    let mut applied: BTreeMap<&str, u64> = BTreeMap::new();
    // after[a] = operations that must be undone before a.
    let mut after: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for trail in trails {
        for (index, step) in trail.iter().enumerate() {
            let id = step.tx.transaction_id.as_str();
            nodes.insert(id, step.tx);
            let when = step.entry.applied_at_unix.unwrap_or(0);
            applied
                .entry(id)
                .and_modify(|t| *t = (*t).max(when))
                .or_insert(when);
            let entry = after.entry(id).or_default();
            if let Some(next) = trail.get(index + 1) {
                entry.insert(next.tx.transaction_id.as_str());
            }
        }
    }
    let mut order = Vec::new();
    let mut done: BTreeSet<&str> = BTreeSet::new();
    while done.len() < nodes.len() {
        let ready = nodes
            .iter()
            .filter(|(id, _)| {
                !done.contains(*id)
                    && after
                        .get(*id)
                        .is_none_or(|before| before.iter().all(|b| done.contains(b)))
            })
            .max_by_key(|(id, tx)| (applied.get(*id).copied(), tx.created_at_unix, **id));
        let Some((id, tx)) = ready else {
            return Err(Blocker::Other(
                "History records later moves of its files in an order that cannot be undone safely"
                    .into(),
            ));
        };
        done.insert(id);
        order.push(*tx);
    }
    Ok(order)
}

/// Whether Undo of a later operation would put back everything it moved:
/// every moved file still at its new place unchanged, every old place free.
fn undo_would_work(
    tx: &RenameTransaction,
    chained: &BTreeSet<*const TransactionEntry>,
    set_places: &BTreeSet<&Path>,
) -> Result<Impact, Blocker> {
    if !matches!(
        tx.state,
        TransactionState::Applied
            | TransactionState::ApplyFailed
            | TransactionState::RollbackFailed
    ) {
        return Err(Blocker::Other(format!(
            "History operation {} cannot be undone in its current state",
            tx.transaction_id
        )));
    }
    for entry in &tx.entries {
        match entry.state {
            EntryState::Planned
            | EntryState::PreflightPassed
            | EntryState::Skipped
            | EntryState::ApplyFailed
            | EntryState::RolledBack => {}
            EntryState::Applied if matches!(entry.operation, TransactionOperation::RenameMove) => {
                let on_trail = chained.contains(&std::ptr::from_ref(entry));
                if !on_trail
                    && (set_places.contains(entry.source_path.as_path())
                        || set_places.contains(entry.destination_path.as_path()))
                {
                    return Err(Blocker::Other(format!(
                        "Undo of History operation {} would also move other files of this disc set ({}), which could split it again",
                        tx.transaction_id,
                        name(&entry.destination_path)
                    )));
                }
                if !on_trail && !is_there_unchanged(&entry.destination_path, &entry.identity) {
                    return Err(Blocker::Other(format!(
                        "{} has changed or gone since History operation {} moved it, so its Undo would not finish",
                        entry.destination_path.display(),
                        tx.transaction_id
                    )));
                }
                if !free(&entry.source_path) {
                    return Err(Blocker::Occupied(entry.source_path.clone()));
                }
            }
            EntryState::Applied => {
                return Err(Blocker::Other(format!(
                    "History operation {} did more than rename or move files, so whether its Undo would finish cannot be checked here",
                    tx.transaction_id
                )));
            }
            EntryState::RollbackFailed | EntryState::RollingBack | EntryState::Applying => {
                let file = name(&entry.source_path);
                return Err(Blocker::Other(format!(
                    "in History operation {}, {}",
                    tx.transaction_id,
                    unfinished(entry, &file).unwrap_or_default()
                )));
            }
        }
    }
    let total = tx
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .count();
    let own = tx
        .entries
        .iter()
        .filter(|entry| chained.contains(&std::ptr::from_ref(*entry)))
        .count();
    Ok(Impact::Known {
        total,
        unrelated: total - own,
    })
}

/// What an Undo of a whole operation would change, as far as History shows.
enum Impact {
    Known { total: usize, unrelated: usize },
    Unknown,
}

/// The impact of undoing the operation that left the set split: every
/// applied entry, those not of this set counted apart. Not reliable when it
/// recorded anything but renames and moves, or a step that did not finish.
fn own_impact(journal: &RenameTransaction, places: &[(&TransactionEntry, Option<bool>)]) -> Impact {
    let reliable = journal.entries.iter().all(|entry| {
        matches!(entry.operation, TransactionOperation::RenameMove)
            && unfinished(entry, "").is_none()
    });
    if !reliable {
        return Impact::Unknown;
    }
    let set: BTreeSet<*const TransactionEntry> = places
        .iter()
        .map(|(entry, _)| std::ptr::from_ref(*entry))
        .collect();
    let applied: Vec<&TransactionEntry> = journal
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .collect();
    let others: Vec<&&TransactionEntry> = applied
        .iter()
        .filter(|entry| !set.contains(&std::ptr::from_ref(**entry)))
        .collect();
    // The other files are counted only when each is where the operation put
    // it, unchanged, with its old place free, as Undo needs; otherwise how
    // much of the operation Undo would put back cannot be told.
    if !others.iter().all(|entry| {
        is_there_unchanged(&entry.destination_path, &entry.identity) && free(&entry.source_path)
    }) {
        return Impact::Unknown;
    }
    let unrelated = others.len();
    Impact::Known {
        total: applied.len(),
        unrelated,
    }
}

fn files(count: usize) -> String {
    if count == 1 {
        "1 file".into()
    } else {
        format!("{count} files")
    }
}

/// History's own summary of an operation, from its entries (as the History
/// list shows it), when that can be told reliably.
fn listed_as(tx: &RenameTransaction) -> Option<String> {
    let moves = tx
        .entries
        .iter()
        .all(|entry| matches!(entry.operation, TransactionOperation::RenameMove));
    // History lists other workflows under their own names.
    let other_workflow = tx.source_scan_root == "verified-ps1-pbp-export-v1"
        || tx.unknown.contains_key("mame_internal_repair_plan_digest")
        || tx.unknown.contains_key("workflow")
        || tx.entries.iter().any(|entry| {
            entry
                .destination_path
                .components()
                .any(|component| component.as_os_str() == crate::repair::QUARANTINE_DIRECTORY_NAME)
        });
    if !moves || other_workflow || tx.entries.is_empty() {
        return None;
    }
    let count = files(tx.entries.len());
    if tx
        .entries
        .iter()
        .all(|entry| entry.source_path.parent() == entry.destination_path.parent())
    {
        Some(format!("Renamed {count}"))
    } else {
        Some(format!("Moved {count}"))
    }
}

/// One Undo step, saying plainly that it reverses the whole operation.
fn undo_step(tx: &RenameTransaction, role: &str, impact: Impact) -> String {
    let id = &tx.transaction_id;
    let recorded = tx.entries.len();
    let kind = match listed_as(tx) {
        Some(summary) => match impact {
            Impact::Known { total, .. } if total != recorded => format!(
                "listed in History as \"{summary}\", which counts every file it recorded, including any already put back"
            ),
            _ => format!("listed in History as \"{summary}\""),
        },
        None => "its type cannot be told reliably from History".into(),
    };
    let scope = match impact {
        Impact::Known { total, unrelated: 0 } => format!(
            "it puts back all {} that operation changed, all of them files of this disc set",
            files(total)
        ),
        Impact::Known { total, unrelated } => format!(
            "it puts back all {} that operation changed, {} of them not part of this disc set",
            files(total),
            unrelated
        ),
        Impact::Unknown => "how many files it would change, and how many of them are not part of this disc set, cannot be worked out reliably from History".into(),
    };
    format!(
        "Undo History operation {id} in History ({role}, {kind}). This Undo reverses that whole operation, not just this game: {scope}."
    )
}
