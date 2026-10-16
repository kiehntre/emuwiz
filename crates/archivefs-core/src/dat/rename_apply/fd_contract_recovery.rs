//! Signed Undo of an interrupted or failed Apply under Contract 1-I.
//!
//! Before any entry reaches this module the admitted bridge has verified the
//! signed grant, the trusted committed checkpoint prefix and the current
//! observations. Decisions here rest only on trusted private evidence: the
//! enrollment anchor and the holding entry, each pinned through an open
//! descriptor, plus exact identity checks. A library name alone proves
//! nothing, and no journal state is taken as proof that a step happened.
//! Anything that cannot be proven is recorded as a review state or refused
//! with every byte of evidence retained. Forward completion of a whole
//! interrupted Apply remains unsupported; a single entry is rolled forward
//! only when its publication is already proven, and only so the ordinary
//! trusted-holding Undo can reverse it.
use super::*;

/// What the caller does after an interrupted Apply entry has been resolved.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// The entry is Applied/Complete or adopted as RollingBack/Captured: run
    /// the ordinary Undo for it.
    Undo,
    /// The entry is settled (Skipped) or parked for operator review.
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Absent,
    Expected,
    Other,
}
impl std::fmt::Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Absent => "absent",
            Self::Expected => "the enrolled object",
            Self::Other => "a different object",
        })
    }
}
fn observe(path: &Path, expected: &ObjectIdentity) -> io::Result<Slot> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Slot::Absent),
        Err(e) => Err(e),
        Ok(_) => Ok(if expected_at(path, expected) {
            Slot::Expected
        } else {
            Slot::Other
        }),
    }
}
fn review(text: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!(
        "{text}; evidence retained; operator review required"
    ))
}

/// One entry of an approved Undo whose transaction may hold interrupted or
/// failed Apply entries. Dispatches on the entry's exact runtime evidence and
/// never on the transaction-level state alone.
pub(crate) fn undo_entry_recovering(
    root: &Path,
    tx: &mut RenameTransaction,
    index: usize,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
    hook: &mut impl FnMut(Edge) -> io::Result<()>,
) -> io::Result<()> {
    let state = tx.entries[index].state;
    if !enrolled(&tx.entries[index]) {
        return Err(io::Error::other("explicit FD enrollment required"));
    }
    let runtime = evidence(tx, index)?;
    match (state, runtime) {
        (_, Some(v)) if v.action == Action::Undo && v.phase == Phase::Unmutated => {
            retire_unmutated(root, tx, index, persist, hook, true)
        }
        (_, Some(v)) if v.action == Action::Undo => {
            undo_entry(root, tx, index, persist, hook, Fault::None)
        }
        (EntryState::Applied, Some(v))
            if v.action == Action::Apply && v.phase == Phase::Complete =>
        {
            undo_entry(root, tx, index, persist, hook, Fault::None)
        }
        (EntryState::Applying | EntryState::ApplyFailed, Some(v))
            if v.action == Action::Apply
                && matches!(
                    v.phase,
                    Phase::CapturePending | Phase::Captured | Phase::CleanupPending
                ) =>
        {
            match resolve_interrupted_apply(root, tx, index, persist, hook, &v)? {
                Next::Undo => undo_entry(root, tx, index, persist, hook, Fault::None),
                Next::Settled => Ok(()),
            }
        }
        (_, Some(_)) => Err(io::Error::other(
            "different interrupted FD action requires operator review",
        )),
        // An enrolled entry applied by the legacy executor (no shared engine).
        (EntryState::Applied, None) => undo_entry(root, tx, index, persist, hook, Fault::None),
        (
            EntryState::Planned
            | EntryState::PreflightPassed
            | EntryState::ApplyFailed
            | EntryState::Skipped,
            None,
        ) => retire_unmutated(root, tx, index, persist, hook, false),
        (state, None) => Err(io::Error::other(format!(
            "entry {index} is {} without FD runtime evidence; operator review required",
            state.label()
        ))),
    }
}

/// An entry this transaction never mutated: prove it, record it, and retire
/// the enrollment anchor. The record is written BEFORE the anchor unlink so a
/// crash in between is recovered by re-verifying and finishing the unlink.
fn retire_unmutated(
    root: &Path,
    tx: &mut RenameTransaction,
    index: usize,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
    hook: &mut impl FnMut(Edge) -> io::Result<()>,
    recorded: bool,
) -> io::Result<()> {
    let e = tx.entries[index].clone();
    let (holding, salvage) = paths(root, tx, index)?;
    let anchor_path = anchor::path(root, tx, index)?;
    let enrolled_anchor = e.unknown.get(anchor::ENROLLED) == Some(&serde_json::json!(true));
    if enrolled_anchor && !recorded && missing(&anchor_path) {
        return Err(io::Error::other(
            "enrollment anchor missing; operator review required",
        ));
    }
    // Pin the anchor first: opening it proves the private anchor still names
    // the enrolled inode, so a matching source identity below is the same
    // object and not a reused inode.
    let anchor_pin = if enrolled_anchor && !missing(&anchor_path) {
        Some(Publisher::open(
            root,
            &anchor_path,
            &e.source_path,
            &e.identity,
        )?)
    } else {
        None
    };
    let observed = [
        (
            "the original is intact at its source name",
            expected_at(&e.source_path, &e.identity),
        ),
        (
            "the destination name is absent",
            missing(&e.destination_path),
        ),
        ("the holding slot is empty", missing(&holding)),
        ("the salvage slot is empty", missing(&salvage)),
    ];
    if let Some((claim, _)) = observed.iter().find(|(_, ok)| !ok) {
        return Err(review(format!(
            "entry {index} was never mutated by this transaction, but the current names contradict its enrolled identity (not true: {claim}); the anchor is retained"
        )));
    }
    if !recorded {
        let detail = match e.failure_reason.as_deref() {
            Some(reason) if e.state == EntryState::ApplyFailed => format!(
                "Undo verified no mutation (Apply refused before any file change: {reason}); enrollment anchor retired"
            ),
            _ => "Undo verified no mutation (Apply had not started for this entry); enrollment anchor retired".into(),
        };
        let entry = &mut tx.entries[index];
        entry.state = EntryState::Skipped;
        entry.failure_reason = Some(detail.clone());
        entry.unknown.insert(
            KEY.into(),
            serde_json::to_value(Evidence {
                action: Action::Undo,
                phase: Phase::Unmutated,
                held_identity: None,
                salvage_identity: None,
                detail,
            })
            .map_err(io::Error::other)?,
        );
        tx.state = if tx
            .entries
            .iter()
            .all(|e| matches!(e.state, EntryState::RolledBack | EntryState::Skipped))
        {
            TransactionState::RolledBack
        } else {
            TransactionState::RollingBack
        };
        persist(tx)?;
        hook(Edge::UnmutatedRecorded)?;
    }
    if let Some(pin) = anchor_pin.as_ref() {
        hook(Edge::BeforeAnchorUnlink)?;
        unlink_private(pin)?;
        hook(Edge::AfterAnchorUnlink)?;
    }
    if !expected_at(&e.source_path, &e.identity) || !missing(&anchor_path) {
        return Err(review(format!(
            "entry {index}: verification after anchor retirement failed"
        )));
    }
    Ok(())
}

/// Record that this entry's Apply failed and is parked for review. Mirrors
/// the executor's outcome for a refused Apply step; the transaction stays
/// RollingBack so no projection may call it resumable or complete.
fn settle_apply_review(
    tx: &mut RenameTransaction,
    index: usize,
    phase: Phase,
    held: Option<ObjectIdentity>,
    detail: &str,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
) -> io::Result<Next> {
    let entry = &mut tx.entries[index];
    entry.state = EntryState::ApplyFailed;
    entry.failure_reason = Some(detail.into());
    entry.unknown.insert(
        KEY.into(),
        serde_json::to_value(Evidence {
            action: Action::Apply,
            phase,
            held_identity: held,
            salvage_identity: None,
            detail: detail.into(),
        })
        .map_err(io::Error::other)?,
    );
    tx.state = TransactionState::RollingBack;
    persist(tx)?;
    Ok(Next::Settled)
}

/// The interrupted Apply had already moved the original into trusted holding
/// and nothing was published: Undo adopts the held original and continues the
/// ordinary trusted-holding Undo from its Captured phase.
fn adopt_held_original(
    tx: &mut RenameTransaction,
    index: usize,
    captured: ObjectIdentity,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
    hook: &mut impl FnMut(Edge) -> io::Result<()>,
) -> io::Result<Next> {
    checkpoint(
        tx,
        index,
        value(
            Phase::Captured,
            Some(captured),
            "Interrupted Apply had captured the original into trusted holding; Undo adopted the held original / Undo incomplete",
        ),
        Action::Undo,
        persist,
    )?;
    hook(Edge::AdoptedRecorded)?;
    Ok(Next::Undo)
}

/// Finish the Apply of one entry through the existing resumable Apply path
/// (holding cleanup and completion checks), then hand it to the ordinary Undo.
fn roll_forward(
    root: &Path,
    tx: &mut RenameTransaction,
    index: usize,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
    hook: &mut impl FnMut(Edge) -> io::Result<()>,
) -> io::Result<Next> {
    apply_entry(root, tx, index, persist, hook)?;
    if tx.entries[index].state == EntryState::Applied
        && evidence(tx, index)?.is_some_and(|v| v.phase == Phase::Complete)
    {
        return Ok(Next::Undo);
    }
    // The Apply path recorded a review phase. Mirror the executor: this
    // entry's Apply failed; nothing further is attempted without review.
    let detail = tx.entries[index]
        .failure_reason
        .clone()
        .unwrap_or_else(|| "Apply incomplete".into());
    let current = evidence(tx, index)?.ok_or_else(|| {
        io::Error::other("Apply review phase lost its runtime evidence; operator review required")
    })?;
    settle_apply_review(
        tx,
        index,
        current.phase,
        current.held_identity,
        &detail,
        persist,
    )
}

/// Classify an interrupted Apply entry from trusted evidence and resolve it to
/// a state the ordinary Undo can act on, or to a recorded review state.
fn resolve_interrupted_apply(
    root: &Path,
    tx: &mut RenameTransaction,
    index: usize,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
    hook: &mut impl FnMut(Edge) -> io::Result<()>,
    previous: &Evidence,
) -> io::Result<Next> {
    let e = tx.entries[index].clone();
    let expected = e.identity.clone();
    let (holding, salvage) = paths(root, tx, index)?;
    let anchor_path = anchor::path(root, tx, index)?;
    // Apply retains the anchor through completion. Without it, inode
    // continuity cannot be proven and nothing below may be inferred.
    let _anchor_pin = if e.unknown.get(anchor::ENROLLED) == Some(&serde_json::json!(true)) {
        if missing(&anchor_path) {
            return Err(io::Error::other(
                "enrollment anchor missing; operator review required",
            ));
        }
        Some(Publisher::open(
            root,
            &anchor_path,
            &e.destination_path,
            &expected,
        )?)
    } else {
        None
    };
    if !missing(&salvage) {
        return Err(review(format!(
            "entry {index}: salvage slot occupied while the Apply phase is {:?}; contradictory evidence",
            previous.phase
        )));
    }
    let source = observe(&e.source_path, &expected)?;
    let destination = observe(&e.destination_path, &expected)?;
    let held = if missing(&holding) {
        None
    } else {
        Some(capture_identity(&holding)?)
    };
    let held_matches = held
        .as_ref()
        .is_some_and(|captured| identity_matches(&expected, captured));
    match previous.phase {
        Phase::CapturePending => match held {
            None if source == Slot::Expected && destination == Slot::Absent => {
                retire_unmutated(root, tx, index, persist, hook, false)?;
                Ok(Next::Settled)
            }
            None => Err(review(format!(
                "entry {index}: interrupted Apply capture is unresolved (source is {source}, destination is {destination}, holding is empty); the enrollment anchor retains the original"
            ))),
            Some(captured) if !held_matches => settle_apply_review(
                tx,
                index,
                Phase::ForeignHeld,
                Some(captured),
                "Held for review / Apply incomplete: a foreign object was captured from the original location during the interrupted Apply; automatic return refused under G3",
                persist,
            ),
            Some(captured) if destination != Slot::Absent => settle_apply_review(
                tx,
                index,
                Phase::DestinationOccupied,
                Some(captured),
                "Destination occupied before publication was recorded; original retained in trusted holding / Apply incomplete",
                persist,
            ),
            Some(captured) => adopt_held_original(tx, index, captured, persist, hook),
        },
        Phase::Captured => match held {
            None => Err(review(format!(
                "entry {index}: trusted holding entry missing after its capture was recorded; the enrollment anchor retains the original"
            ))),
            Some(captured) if !held_matches => settle_apply_review(
                tx,
                index,
                Phase::ForeignHeld,
                Some(captured),
                "Held for review / Apply incomplete: the trusted holding entry is no longer the enrolled object",
                persist,
            ),
            Some(captured) => match destination {
                Slot::Absent => adopt_held_original(tx, index, captured, persist, hook),
                Slot::Expected => {
                    // Publication is proven, not inferred: the destination
                    // names the inode pinned by both the holding and anchor
                    // descriptors, so it cannot be a reused inode or a
                    // different file. Record it and finish the Apply.
                    checkpoint(
                        tx,
                        index,
                        value(
                            Phase::CleanupPending,
                            Some(captured),
                            "Destination publication verified during recovery through the pinned holding descriptor; holding cleanup pending / Undo incomplete",
                        ),
                        Action::Apply,
                        persist,
                    )?;
                    hook(Edge::RollForwardRecorded)?;
                    roll_forward(root, tx, index, persist, hook)
                }
                Slot::Other => settle_apply_review(
                    tx,
                    index,
                    Phase::DestinationOccupied,
                    Some(captured),
                    "Destination occupied by a different object; original retained in trusted holding / Apply incomplete",
                    persist,
                ),
            },
        },
        Phase::CleanupPending => match (destination, held) {
            (Slot::Expected, Some(_)) if held_matches => {
                roll_forward(root, tx, index, persist, hook)
            }
            (Slot::Expected, Some(captured)) => settle_apply_review(
                tx,
                index,
                Phase::Incomplete,
                Some(captured),
                "Trusted holding entry is no longer the enrolled object after publication / Apply incomplete",
                persist,
            ),
            (Slot::Expected, None) if source != Slot::Absent => settle_apply_review(
                tx,
                index,
                Phase::Incomplete,
                None,
                "Links were observed after cleanup, but the original name is occupied again; current names are unresolved / Apply incomplete",
                persist,
            ),
            (Slot::Expected, None) => {
                // Completion is proven by the pinned anchor: the destination
                // names the enrolled inode, the holding link is gone and the
                // original name is free. Record the completion the crash
                // prevented, then let the ordinary Undo reverse it.
                checkpoint(
                    tx,
                    index,
                    value(Phase::Complete, None, "Apply completed"),
                    Action::Apply,
                    persist,
                )?;
                hook(Edge::RollForwardRecorded)?;
                verify_records(tx)?;
                Ok(Next::Undo)
            }
            (_, Some(captured)) if held_matches => settle_apply_review(
                tx,
                index,
                Phase::Incomplete,
                Some(captured),
                "Destination no longer verified after its publication was recorded; original retained in trusted holding / Apply incomplete",
                persist,
            ),
            (_, Some(captured)) => settle_apply_review(
                tx,
                index,
                Phase::Incomplete,
                Some(captured),
                "Destination no longer verified and the trusted holding entry is not the enrolled object / Apply incomplete",
                persist,
            ),
            (_, None) => settle_apply_review(
                tx,
                index,
                Phase::Incomplete,
                None,
                "Destination no longer verified after its publication was recorded and the holding link is gone; the enrollment anchor retains the original / Apply incomplete",
                persist,
            ),
        },
        _ => Err(io::Error::other(
            "different interrupted FD action requires operator review",
        )),
    }
}
