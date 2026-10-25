//! Read-only restart interpretation for the operator workflow.
//!
//! After any controller or service termination, this answers "what state is
//! this transaction really in, and what may happen next?" from trusted
//! evidence only: the deployment configuration and registration, the
//! admission store (read under the participants' lock, never written), the
//! bound compact store (shared lock, never repaired), the FD records and the
//! current files. A journal claim is accepted only when the files agree with
//! it. Anything missing, stale, corrupt or contradictory is REVIEW_REQUIRED
//! with the reason; nothing here starts, resumes or repairs recovery, consumes
//! or renews a grant, or changes any authority record.
use super::*;
use approval::GrantVerdict;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartState {
    /// Provisioned; no enrolled transaction and no partial enrollment.
    NotEnrolled,
    /// Enrolled; Apply has not started (no checkpoint published).
    AwaitingApply,
    /// Apply started and did not finish. Apply is spent; only a fresh signed
    /// Undo may act, and it re-verifies trusted evidence per entry.
    ApplyIncomplete,
    /// Every entry applied and verified.
    Applied,
    /// Undo started and did not finish; resumable with a fresh signed Undo.
    UndoIncomplete,
    /// Verified settlement recorded, ownership not yet released.
    SettledAwaitingRelease,
    /// Verified settlement recorded and ownership released.
    Settled,
    /// Evidence is missing, stale, corrupt, contradictory or parked for review.
    ReviewRequired,
    /// A live participating process holds the operation; retry when idle.
    Busy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NextStep {
    ReviewApply,
    ReviewUndo,
    OperatorReview,
    RetryWhenIdle,
    None,
}
#[derive(Clone, Debug, Serialize)]
pub struct RetainedFile {
    pub path: PathBuf,
    pub identity: ObjectIdentity,
    pub original_anchor: PathBuf,
    pub signed_grant_nonce: String,
    pub sha256: String,
}
#[derive(Debug, Serialize)]
pub struct EntryStatus {
    pub index: usize,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub state: EntryState,
    pub action: Option<String>,
    pub phase: Option<String>,
    pub verdict: String,
    pub retained_foreign: Option<RetainedFile>,
}
#[derive(Debug, Serialize)]
pub struct RestartStatus {
    pub state: RestartState,
    pub next: NextStep,
    pub reasons: Vec<String>,
    pub transaction_id: Option<String>,
    pub checkpoint_pending: bool,
    pub ownership: String,
    pub entries: Vec<EntryStatus>,
}
#[derive(Debug, Serialize)]
pub struct GrantStatus {
    pub verdict: GrantVerdict,
    pub reason: String,
    pub status: RestartStatus,
}

fn blank(state: RestartState, next: NextStep, reason: String) -> RestartStatus {
    RestartStatus {
        state,
        next,
        reasons: vec![reason],
        transaction_id: None,
        checkpoint_pending: false,
        ownership: "unknown".into(),
        entries: vec![],
    }
}
fn review(reason: impl Into<String>) -> RestartStatus {
    blank(
        RestartState::ReviewRequired,
        NextStep::OperatorReview,
        reason.into(),
    )
}
fn busy(reason: impl std::fmt::Display) -> RestartStatus {
    blank(
        RestartState::Busy,
        NextStep::RetryWhenIdle,
        format!("{reason}; nothing was read as current, retry when the operation ends"),
    )
}
fn is_busy(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::WouldBlock
        || e.to_string().contains("compact writer active")
        || e.to_string().contains("already owned by another writer")
}
fn present(p: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}
fn matches(p: &Path, id: &ObjectIdentity) -> bool {
    capture_identity(p).is_ok_and(|v| crate::dat::rename_apply::identity::identity_matches(id, &v))
}

/// Read-only restart status. Never fails: errors become ReviewRequired/Busy.
pub fn status(state: &Path) -> RestartStatus {
    inspect(state, None).0
}

/// Read-only grant check. Never consumes, renews or recreates a grant.
pub fn inspect_grant(
    state: &Path,
    grant: &approval::Approval,
    action: RequestedAction,
) -> GrantStatus {
    let (status, verdict) = inspect(state, Some((grant, action)));
    let (verdict, reason) = verdict.unwrap_or_else(|| {
        (
            GrantVerdict::NotApplicable,
            format!(
                "transaction is {:?}; no grant can act until the status allows it",
                status.state
            ),
        )
    });
    GrantStatus {
        verdict,
        reason,
        status,
    }
}

type GrantCheck<'a> = Option<(&'a approval::Approval, RequestedAction)>;

fn inspect(state: &Path, grant: GrantCheck) -> (RestartStatus, Option<(GrantVerdict, String)>) {
    let c = match load(state) {
        Ok(c) => c,
        Err(e) => {
            return (
                review(format!(
                    "deployment configuration, root binding or issuer directory unavailable or changed: {e}"
                )),
                None,
            );
        }
    };
    let registration = match directory(state).and_then(|d| read(&d, "registration.json")) {
        Ok(bytes) => match serde_json::from_slice::<Registration>(&bytes) {
            Ok(r) => Some(r),
            Err(e) => return (review(format!("registration record corrupt: {e}")), None),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return (review(format!("registration record unreadable: {e}")), None),
    };
    let Some(r) = registration else {
        return (unregistered(&c), None);
    };
    let base = match Authority::decode_legacy(&r.original, 1024 * 1024) {
        Ok(b) => b,
        Err(e) => return (review(format!("registered plan corrupt: {e:?}")), None),
    };
    if let Err(e) = Bridge::supported(&base)
        .and_then(|_| Bridge::confined(&c.root, &base))
        .and_then(|_| {
            if shared_engine::selected(&base.transaction)
                && r.ticket.root == c.root
                && r.ticket.root_id == c.root_id
                && r.ticket.binding == base.binding()
            {
                Ok(())
            } else {
                Err(fail(
                    "registration does not bind this root and plan to a shared FD transaction",
                ))
            }
        })
    {
        return (review(format!("registered authority invalid: {e}")), None);
    }
    let (ownership, _admission) = match c.authority.inspect(Some(&base)) {
        Ok(v) => v,
        Err(e) if is_busy(&e) => return (busy("a participating operation holds admission"), None),
        Err(e) => return (review(format!("admission authority unreadable: {e}")), None),
    };
    let store = match Store::inspect(&r.ticket, &base, CAP) {
        Ok(s) => s,
        Err(e) if is_busy(&e) => return (busy("a compact writer holds the store"), None),
        Err(e) => {
            return (
                review(format!(
                    "compact recovery evidence missing or rebound to a different object: {e}"
                )),
                None,
            );
        }
    };
    if let Some(result) = super::root_reuse::closed_status(&c, &r, &base, &store, &ownership) {
        return match result {
            Ok(out) => {
                let verdict = grant.map(|_| {
                    (
                        GrantVerdict::NotApplicable,
                        "historical transaction settled and released; old grants cannot act".into(),
                    )
                });
                (out, verdict)
            }
            Err(e) => (
                review(format!(
                    "historical settlement evidence requires review: {e}"
                )),
                None,
            ),
        };
    }
    let mut out = classify(&base, &store, &ownership);
    if let Ok(scan) = store.scan(&base)
        && let Ok(tx) = scan.runtime.restore(&base, &scan.stream)
        && let Err(e) = approval::verify_retention_receipts(&c.verifier, &store, &base, &tx)
    {
        out.state = RestartState::ReviewRequired;
        out.next = NextStep::OperatorReview;
        out.reasons.push(format!(
            "retained-file approval evidence unavailable or inconsistent: {e}"
        ));
        return (out, None);
    }
    let verdict = grant.map(|(grant, action)| {
        let allowed = match out.state {
            RestartState::AwaitingApply => action!=RequestedAction::RetainForeign,
            RestartState::ApplyIncomplete|RestartState::Applied|RestartState::UndoIncomplete|RestartState::SettledAwaitingRelease => action==RequestedAction::Undo,
            RestartState::ReviewRequired if out.next==NextStep::ReviewUndo => action==RequestedAction::Undo || action==RequestedAction::RetainForeign,
            _=>false,
        } && store.scan(&base).and_then(|scan| {
            let tx=scan.runtime.restore(&base,&scan.stream)?;
            super::super::fd_contract_bridge::eligible_entries(&c.root,&tx,action,!scan.cursor.is_empty()||scan.pending.is_some())
        }).is_ok();
        let observations = match restart::observations_of(&store, &base)
            .and_then(|o| serde_json::to_vec(&o).map_err(fail))
        {
            Ok(o) => o,
            Err(e) => {
                out.reasons
                    .push(format!("current file observations unavailable: {e}"));
                return (
                    GrantVerdict::NotApplicable,
                    "file observations unavailable".into(),
                );
            }
        };
        let (verdict, reason) = grant.inspect(&c.verifier, action, &store, &base, &observations);
        if verdict == GrantVerdict::Usable && !allowed {
            (
                GrantVerdict::NotApplicable,
                format!(
                    "grant is genuine and current, but a {action:?} cannot act on a {:?} transaction",
                    out.state
                ),
            )
        } else {
            (verdict, reason)
        }
    });
    (out, verdict)
}

/// No registration: either nothing was enrolled, or enrollment died part-way.
fn unregistered(c: &Configuration) -> RestartStatus {
    if c.lineage.is_some() {
        return match c.authority.inspect(None) {
            Ok((inspection, _hold)) => {
                if !inspection.unreleased.is_empty() {
                    return review(
                        "successor enrollment interrupted; unreleased ownership has no registration; preserve evidence",
                    );
                }
                match super::root_reuse::verify_predecessors_read(c,&inspection) {
                    Ok(())=>blank(RestartState::NotEnrolled,NextStep::None,"Verified predecessor history retained; successor is not enrolled. Review a new plan and enroll explicitly.".into()),
                    Err(e)=>review(format!("successor history/evidence unavailable: {e}")),
                }
            }
            Err(e) if is_busy(&e) => busy(e),
            Err(e) => review(format!("successor admission unavailable: {e}")),
        };
    }
    let mut reasons = vec![];
    match c.authority.inspect(None) {
        Ok((i, _lock)) if !i.unreleased.is_empty() => reasons.push(format!(
            "admission holds unreleased ownership for {:?} but no registration was written",
            i.unreleased
        )),
        Ok(_) => (),
        Err(e) if is_busy(&e) => return busy("a participating operation holds admission"),
        Err(e) => reasons.push(format!("admission authority unreadable: {e}")),
    }
    match present(&c.root.join(NAMESPACE)) {
        Ok(true) => reasons.push("compact recovery evidence exists without a registration".into()),
        Ok(false) => (),
        Err(e) => reasons.push(format!("compact namespace unreadable: {e}")),
    }
    match std::fs::read_dir(c.root.join(".emuwiz-r2-holding")) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if name != BINDING && !name.starts_with("probe-") {
                            reasons.push(format!(
                                "private holding contains enrollment evidence {name:?} without a registration"
                            ));
                        }
                    }
                    Err(e) => reasons.push(format!("holding directory unreadable: {e}")),
                }
            }
        }
        Err(e) => reasons.push(format!("holding directory unreadable: {e}")),
    }
    if reasons.is_empty() {
        return blank(
            RestartState::NotEnrolled,
            NextStep::None,
            "provisioned; no transaction is enrolled".into(),
        );
    }
    let mut out =
        review("enrollment did not complete; its partial evidence is retained and never adopted");
    out.reasons.extend(reasons);
    out
}

fn classify(base: &Authority, store: &Store, ownership: &admission::Inspection) -> RestartStatus {
    let mut out = RestartStatus {
        state: RestartState::ReviewRequired,
        next: NextStep::OperatorReview,
        reasons: vec![],
        transaction_id: Some(base.transaction_id().into()),
        checkpoint_pending: false,
        ownership: "unknown".into(),
        entries: vec![],
    };
    let scan = match store.scan(base) {
        Ok(s) => s,
        Err(e) => {
            out.reasons
                .push(format!("compact evidence corrupt or inconsistent: {e}"));
            return out;
        }
    };
    if scan.pending.is_some() {
        match store.pending_transaction(base) {
            Ok(Some(_)) => {
                out.checkpoint_pending = true;
                out.reasons.push(
                    "a fully written but uncommitted checkpoint exists; a fresh signed grant verifies it against the files and commits it before any mutation".into(),
                );
            }
            Ok(None) => (),
            Err(e) => {
                out.reasons.push(format!("torn checkpoint retained: {e}"));
                return out;
            }
        }
    }
    let tx = match scan.runtime.restore(base, &scan.stream) {
        Ok(tx) => tx,
        Err(e) => {
            out.reasons
                .push(format!("checkpoint contradicts its semantic record: {e}"));
            return out;
        }
    };
    let sealed = materialize(base, &scan.stream, usize::MAX)
        .map(|o| o.rollback_recorded)
        .unwrap_or(false);
    let mut problems = vec![];
    let mut parked_reasons = Vec::new();
    let released = match &ownership.owner {
        None => {
            problems.push("no admission ownership record exists for this transaction (the authority store is missing it, possibly an older snapshot)".to_string());
            false
        }
        Some((binding, _)) if binding.as_slice() != base.binding().as_slice() => {
            problems.push("admission ownership belongs to a different plan".into());
            false
        }
        Some((_, true)) => {
            if tx.state != TransactionState::RolledBack || !sealed {
                problems.push(format!(
                    "ownership was released while the transaction is {:?} (contradiction: restored or replaced evidence)",
                    tx.state
                ));
            }
            true
        }
        Some((_, false)) => {
            if !ownership.claims_complete {
                problems.push("ownership claims for an object or name slot are missing".into());
            }
            false
        }
    };
    let foreign: Vec<_> = ownership
        .unreleased
        .iter()
        .filter(|id| id.as_str() != base.transaction_id())
        .collect();
    if !foreign.is_empty() {
        problems.push(format!(
            "admission also holds unreleased foreign transactions {foreign:?}"
        ));
    }
    out.ownership = if released {
        "released".into()
    } else if ownership.owner.is_some() {
        "held".into()
    } else {
        "missing".into()
    };
    if let Err(e) = contract::verify_records(&tx) {
        problems.push(format!("recorded state contradicts current files: {e}"));
    }
    for (index, e) in tx.entries.iter().enumerate() {
        let evidence = match contract::evidence(&tx, index) {
            Ok(v) => v,
            Err(err) => {
                problems.push(format!("entry {index}: {err}"));
                continue;
            }
        };
        use contract::{Action, Phase};
        let unchanged = |problems: &mut Vec<String>| {
            let source = matches(&e.source_path, &e.identity);
            let destination = present(&e.destination_path);
            if !source || !matches!(destination, Ok(false)) {
                problems.push(format!(
                    "entry {index}: the record says this file was never moved, but the original is {} at its source and the destination is {} (stale, rewound or externally changed evidence)",
                    if source { "present" } else { "NOT" },
                    match destination {
                        Ok(true) => "occupied".to_string(),
                        Ok(false) => "free".to_string(),
                        Err(err) => format!("unreadable ({err})"),
                    }
                ));
            }
        };
        let verdict = match (e.state, evidence.as_ref().map(|v| (v.action, v.phase))) {
            (EntryState::Planned | EntryState::PreflightPassed, None)
            | (EntryState::ApplyFailed | EntryState::Skipped, None) => {
                unchanged(&mut problems);
                "not mutated".to_string()
            }
            (EntryState::Skipped, Some((Action::Undo, Phase::Unmutated))) => {
                unchanged(&mut problems);
                "verified unmutated by Undo".to_string()
            }
            (EntryState::Applied, Some((Action::Apply, Phase::Complete))) => {
                "applied and verified".to_string()
            }
            (EntryState::RolledBack, Some((Action::Undo, Phase::Complete))) => {
                "restored and verified".to_string()
            }
            (EntryState::RolledBack,Some((Action::Undo,Phase::RetainedForeign))) => {
                match contract::paths(&store.ticket.root,&tx,index) {
                    Ok((held,_))=>format!("restored and verified; foreign object permanently retained at {}; original continuity anchor retained; signed receipt {}",held.display(),evidence.as_ref().and_then(|v|v.retention.as_ref()).map(|r|r.nonce.as_str()).unwrap_or("MISSING")),
                    Err(err)=>{problems.push(err.to_string());"REVIEW_REQUIRED: retained location unavailable".into()},
                }
            }
            (
                EntryState::Applying | EntryState::ApplyFailed,
                Some((Action::Apply, Phase::CapturePending | Phase::Captured | Phase::CleanupPending)),
            ) => "interrupted Apply: a fresh signed Undo re-verifies trusted evidence and either restores it or parks it for review".to_string(),
            (
                EntryState::RollingBack,
                Some((Action::Undo, Phase::CapturePending | Phase::Captured | Phase::CleanupPending)),
            ) => "interrupted Undo: resumable with a fresh signed Undo".to_string(),
            (_, Some((action, phase))) => {
                parked_reasons.push(format!(
                    "entry {index} is parked for operator review ({action:?}/{phase:?}): {}",
                    if action==Action::Undo && phase==Phase::PossiblyUnrecoverable && matches(&e.source_path,&e.identity) {
                        "enrolled identity observed at original path; recorded cleanup remains unresolved"
                    } else {e.failure_reason.as_deref().unwrap_or("no detail recorded")}
                ));
                if action==Action::Undo && phase==Phase::PossiblyUnrecoverable && matches(&e.source_path,&e.identity) {
                    format!("REVIEW_REQUIRED: enrolled identity observed at original path {}; historical cleanup uncertainty remains, not a completed Undo",e.source_path.display())
                } else {format!("REVIEW_REQUIRED: {action:?}/{phase:?}")}
            }
            (state, None) => {
                problems.push(format!(
                    "entry {index} is {} without FD evidence",
                    state.label()
                ));
                "REVIEW_REQUIRED: missing evidence".to_string()
            }
        };
        out.entries.push(EntryStatus {
            index,
            source: e.source_path.clone(),
            destination: e.destination_path.clone(),
            state: e.state,
            action: evidence.as_ref().map(|v| format!("{:?}", v.action)),
            phase: evidence.as_ref().map(|v| format!("{:?}", v.phase)),
            verdict,
            retained_foreign: evidence
                .as_ref()
                .filter(|v| v.phase == Phase::RetainedForeign)
                .and_then(|v| {
                    Some(RetainedFile {
                        path: contract::paths(&store.ticket.root, &tx, index).ok()?.0,
                        identity: v.held_identity.clone()?,
                        original_anchor: contract::anchor::path(&store.ticket.root, &tx, index)
                            .ok()?,
                        signed_grant_nonce: v.retention.as_ref()?.nonce.clone(),
                        sha256: hex(&v.held_identity.as_ref()?.freshness.as_ref()?.sha256),
                    })
                }),
        });
    }
    if !problems.is_empty() {
        out.reasons.extend(problems);
        out.reasons.extend(parked_reasons);
        return out;
    }
    if !parked_reasons.is_empty() {
        out.reasons.extend(parked_reasons);
        let undo = super::super::fd_contract_bridge::eligible_entries(
            &store.ticket.root,
            &tx,
            RequestedAction::Undo,
            true,
        )
        .is_ok();
        let retain = super::super::fd_contract_bridge::eligible_entries(
            &store.ticket.root,
            &tx,
            RequestedAction::RetainForeign,
            true,
        )
        .is_ok();
        if undo || retain {
            out.next = NextStep::ReviewUndo;
            out.reasons.push("Fresh reviewed ordinary Undo may recover eligible other entries only. A distinct retain-foreign review/approval is required for eligible foreign holds; ownership stays held until all entries and retained receipts verify.".into());
        }
        return out;
    }
    let spent = !scan.cursor.is_empty() || out.checkpoint_pending;
    (out.state, out.next) = match tx.state {
        TransactionState::Planned if !spent => (RestartState::AwaitingApply, NextStep::ReviewApply),
        TransactionState::Planned | TransactionState::Applying | TransactionState::ApplyFailed => {
            (RestartState::ApplyIncomplete, NextStep::ReviewUndo)
        }
        TransactionState::Applied => (RestartState::Applied, NextStep::ReviewUndo),
        TransactionState::RollingBack => (RestartState::UndoIncomplete, NextStep::ReviewUndo),
        TransactionState::RolledBack if !sealed => {
            out.reasons
                .push("terminal state recorded without its seal".into());
            return out;
        }
        TransactionState::RolledBack if released => (RestartState::Settled, NextStep::None),
        TransactionState::RolledBack => {
            out.reasons.push("verified settlement is recorded but ownership is still held; a fresh signed Undo performs only the verified release".into());
            (RestartState::SettledAwaitingRelease, NextStep::ReviewUndo)
        }
        TransactionState::RollbackFailed => {
            out.reasons
                .push("rollback recorded as failed; operator review required".into());
            return out;
        }
        TransactionState::Finalising | TransactionState::Finalised => {
            out.reasons.push(
                "a finalise state is not valid for an enrolled transaction; operator review required"
                    .into(),
            );
            return out;
        }
    };
    if out.reasons.is_empty() {
        out.reasons.push(match out.state {
            RestartState::AwaitingApply => "enrolled; Apply not started; requires a fresh reviewed Apply grant".into(),
            RestartState::ApplyIncomplete => "Apply was interrupted or stopped; Apply cannot resume; only a fresh reviewed Undo grant may act".into(),
            RestartState::Applied => "applied and verified; Undo requires a fresh reviewed grant".into(),
            RestartState::UndoIncomplete => "Undo was interrupted; resumable only with a fresh reviewed Undo grant".into(),
            _ => "settled and released; no further action".into(),
        });
    }
    out
}
