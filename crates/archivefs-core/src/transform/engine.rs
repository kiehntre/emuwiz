//! The executor: drives a plan's steps through the journal, resumes or
//! compensates interrupted transactions, and derives their status.
//!
//! # Protocol
//!
//! For each step the engine appends a durable *Intent*, performs the step, and
//! appends a durable *Done* carrying the evidence the step produced. Every
//! step is decided by probing the world, never by trusting the journal's last
//! line, so a crash anywhere (including between a syscall and its Done) is
//! resolved by re-running the step, which finds its effect present or absent.
//!
//! Direction:
//!
//! - With no failure and no cancellation, apply and recovery move *forward*.
//! - A step that refuses (made no change it cannot reverse) before the plan's
//!   point of no return switches the transaction to *compensation*: every
//!   attempted step is reversed in reverse order, each by probing.
//! - Once the point-of-no-return step is Done the engine only moves forward.
//!   A later problem is recorded as *Blocked* with the facts known to hold;
//!   it is never written as Complete and never "rolled back" by guessing.
//! - A step that cannot classify the world (neither location holds the
//!   expected object, the catalogue diverged) is Blocked in either direction.
//!
//! Complete is appended only after every forward step is Done, including an
//! independent final verification (I7). Compensated is appended only after
//! every attempted step is reversed or explicitly retained as residue.
//!
//! # Execution authority
//!
//! Execution requires a test-only `ScratchAuthority` (see
//! [`super::authority`]); a production build cannot construct one, so
//! production relocation, and therefore retirement of production originals,
//! is unavailable. No file in a game folder grants anything. Planning,
//! preview, status and Undo planning are read-only and work everywhere.

use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use crate::catalogue_health::SourceRootBinding;
use crate::dat::rename_apply::dirsync::RenameDirs;
use crate::dat::rename_apply::identity::{
    capture_identity, content_identity_matches, identity_matches,
};
use crate::dat::rename_apply::model::{ObjectIdentity, TransactionEntry};
use crate::dat::rename_apply::noclobber::NoClobberError;
use crate::database::organisation_paths;
use crate::safe_read::TrustedRoots;

use super::authority::ScratchAuthority;
use super::fault::{self, FaultPlan};
use super::fsops;
use super::journal::{self, JournalDamage, JournalWriter, LoadedJournal};
use super::lock;
use super::model::*;

/// Per-call configuration. Production callers pass no fault plan and have
/// no execution authority.
pub struct Context<'a> {
    pub journal_root: PathBuf,
    pub fault: Option<FaultPlan>,
    pub cancel: Option<&'a AtomicBool>,
    pub(crate) authority: Option<&'a ScratchAuthority>,
}

impl<'a> Context<'a> {
    pub fn new(journal_root: impl Into<PathBuf>) -> Self {
        Self {
            journal_root: journal_root.into(),
            fault: None,
            cancel: None,
            authority: None,
        }
    }

    /// This crate's unit tests only (`cfg(test)`, never a feature): execute
    /// under `authority`.
    #[cfg(test)]
    pub(crate) fn with_authority(mut self, authority: &'a ScratchAuthority) -> Self {
        self.authority = Some(authority);
        self
    }
}

/// What a transaction's journal proves about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxnState {
    /// Every step Done and independently verified.
    Complete,
    /// Every attempted step reversed; `residue` lists what was retained.
    Compensated { residue: Vec<PathBuf> },
    /// Stopped short of a terminal outcome; needs review, then recovery.
    Blocked {
        step: usize,
        reason: String,
        safe_facts: Vec<String>,
    },
    /// Another process holds the transaction right now.
    Running,
    /// Interrupted without an outcome; recovery is required.
    Interrupted { last: Option<(usize, Direction)> },
    /// The journal cannot be trusted; nothing will act on it.
    Corrupt { reason: String },
}

impl TxnState {
    /// Only Complete is success. Everything else must never be shown green.
    pub fn is_success(&self) -> bool {
        matches!(self, TxnState::Complete)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, TxnState::Complete | TxnState::Compensated { .. })
    }

    pub fn label(&self) -> &'static str {
        match self {
            TxnState::Complete => "Complete (verified)",
            TxnState::Compensated { residue } if residue.is_empty() => {
                "Rolled back - nothing changed"
            }
            TxnState::Compensated { .. } => "Rolled back - items retained for review",
            TxnState::Blocked { .. } => "Needs attention - recovery incomplete",
            TxnState::Running => "In progress in another process",
            TxnState::Interrupted { .. } => "Interrupted - recovery required",
            TxnState::Corrupt { .. } => "Journal damaged - manual review required",
        }
    }
}

#[derive(Debug)]
pub enum EngineError {
    /// Refused before anything was journalled or changed.
    Refused(String),
    /// The transaction started but could not continue (for example its
    /// journal could not be written). It is not terminal: recovery is
    /// required, and status reports it as Interrupted.
    Interrupted(String),
    /// An injected crash (tests only). The journal is as a dead process left it.
    Crashed(io::Error),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Refused(m) => write!(f, "{m}"),
            EngineError::Interrupted(m) => write!(f, "interrupted: {m}; recovery is required"),
            EngineError::Crashed(e) => write!(f, "crashed: {e}"),
        }
    }
}

enum StepError {
    Crash(io::Error),
    /// No change that compensation cannot find by probing.
    Refused(String),
    /// The world cannot be classified; stop and report.
    Unsafe {
        reason: String,
        facts: Vec<String>,
    },
}

impl From<io::Error> for StepError {
    fn from(e: io::Error) -> Self {
        if fault::is_crash(&e) {
            StepError::Crash(e)
        } else {
            StepError::Refused(e.to_string())
        }
    }
}

fn refused<T>(m: impl Into<String>) -> Result<T, StepError> {
    Err(StepError::Refused(m.into()))
}

fn unsafe_<T>(reason: impl Into<String>, facts: Vec<String>) -> Result<T, StepError> {
    Err(StepError::Unsafe {
        reason: reason.into(),
        facts,
    })
}

/// Any error after an irreversible mutation must stop, not compensate.
fn after_irreversible(e: io::Error) -> StepError {
    if fault::is_crash(&e) {
        StepError::Crash(e)
    } else {
        StepError::Unsafe {
            reason: format!("error after an irreversible step: {e}"),
            facts: vec![],
        }
    }
}

/// The canonical serialisation whose SHA-256 is the plan digest.
pub fn plan_bytes(plan: &Plan) -> Vec<u8> {
    serde_json::to_vec_pretty(plan).expect("plan serialises")
}

pub fn plan_digest(plan: &Plan) -> String {
    journal::hex(&Sha256::digest(plan_bytes(plan)))
}

fn crash_or(e: io::Error) -> EngineError {
    if fault::is_crash(&e) {
        EngineError::Crashed(e)
    } else {
        EngineError::Refused(e.to_string())
    }
}

/// The authorisation gate in front of every execution: a test-only
/// `ScratchAuthority` that created both roots, under the scratch policy.
/// There is no other way to execute in this release.
pub(crate) fn authorise(ctx: &Context<'_>, plan: &Plan) -> Result<(), String> {
    let Some(authority) = ctx.authority else {
        return Err(super::authority::PRODUCTION_UNAVAILABLE.into());
    };
    // Production plans are never executable; a test plan only by an
    // authority of its own policy.
    if plan.filesystem_policy == super::fscompat::FsPolicy::Production
        || plan.filesystem_policy != authority.policy()
    {
        return Err(super::authority::PRODUCTION_UNAVAILABLE.into());
    }
    for root in [&plan.source_root.path, &plan.destination_root.path] {
        authority.admits(root)?;
    }
    // Only the step sequences the planner produces, entirely inside the
    // admitted roots, are ever executed.
    super::shape::validate(plan)?;
    // The catalogue whose drive the plan checked is the database it writes.
    if organisation_paths::bound_database(&plan.catalogue_binding)? != plan.catalogue {
        return Err(
            "the plan's catalogue is not the database its binding writes; nothing was changed"
                .into(),
        );
    }
    // An Undo only ever reverses its own relocation.
    super::undo::undo_matches_relocation(ctx, plan)
}

/// Paths a plan may touch, for resource locking.
pub fn resources(plan: &Plan) -> Vec<PathBuf> {
    let mut out = vec![plan.source_path.clone(), plan.destination_path.clone()];
    if let Some(r) = &plan.retired_path {
        out.push(r.clone());
    }
    for step in &plan.steps {
        match step {
            Step::Rename { from, to, .. } => {
                out.push(from.clone());
                out.push(to.clone());
            }
            Step::CreatePrivateDir { dir, .. } | Step::RemovePrivateDir { dir, .. } => {
                out.push(dir.clone())
            }
            Step::CopyToStaging { staged } | Step::VerifyStaged { staged } => {
                out.push(staged.clone())
            }
            Step::DiscardDuplicate { path, keeper, .. } => {
                out.push(path.clone());
                out.push(keeper.clone());
            }
            _ => {}
        }
    }
    out
}

/// History folded out of a journal's records.
#[derive(Default)]
struct History {
    fwd_attempted: Vec<bool>,
    fwd_done: Vec<Option<StepEvidence>>,
    comp_done: Vec<Option<StepEvidence>>,
    compensating: bool,
    cancelled: bool,
    failed_before_pnr: bool,
    outcome: Option<(Outcome, String)>,
    last: Option<(usize, Direction)>,
    blocked: Option<(usize, String, Vec<String>)>,
    recoveries: u32,
    undone_by: Vec<String>,
}

impl History {
    fn new(n: usize) -> Self {
        Self {
            fwd_attempted: vec![false; n],
            fwd_done: vec![None; n],
            comp_done: vec![None; n],
            ..Default::default()
        }
    }

    fn fold(plan: &Plan, journal: &LoadedJournal) -> Result<Self, String> {
        let n = plan.steps.len();
        let pnr = plan.point_of_no_return;
        let mut h = Self::new(n);
        for record in &journal.records {
            let in_range = |s: usize| {
                if s < n {
                    Ok(s)
                } else {
                    Err(format!("record {} names step {s} of {n}", record.seq))
                }
            };
            match &record.event {
                Event::Begun { .. } => {}
                Event::Intent { step, direction } => {
                    let s = in_range(*step)?;
                    h.last = Some((s, *direction));
                    h.blocked = None;
                    match direction {
                        Direction::Forward => h.fwd_attempted[s] = true,
                        Direction::Compensate => h.compensating = true,
                    }
                }
                Event::Done {
                    step,
                    direction,
                    evidence,
                } => {
                    let s = in_range(*step)?;
                    match direction {
                        Direction::Forward => h.fwd_done[s] = Some(evidence.clone()),
                        Direction::Compensate => h.comp_done[s] = Some(evidence.clone()),
                    }
                }
                Event::Failed {
                    step, direction, ..
                } => {
                    let s = in_range(*step)?;
                    if *direction == Direction::Forward && h.fwd_done[pnr].is_none() && s <= pnr {
                        h.failed_before_pnr = true;
                    }
                }
                Event::RecoveryStarted { .. } => {
                    h.recoveries += 1;
                    h.blocked = None;
                }
                Event::CancelRequested => h.cancelled = true,
                Event::Blocked {
                    step,
                    reason,
                    safe_facts,
                } => h.blocked = Some((*step, reason.clone(), safe_facts.clone())),
                Event::Outcome { outcome } => {
                    if h.outcome.is_some() {
                        return Err("journal records two outcomes".into());
                    }
                    // Complete is only valid when every forward step is Done.
                    if *outcome == Outcome::Complete && h.fwd_done.iter().any(Option::is_none) {
                        return Err("journal claims Complete with unfinished steps".into());
                    }
                    h.outcome = Some((outcome.clone(), record.sum.clone()));
                }
                Event::UndoneBy { undo_txn } => h.undone_by.push(undo_txn.clone()),
            }
        }
        Ok(h)
    }

    fn state(&self) -> TxnState {
        match &self.outcome {
            Some((Outcome::Complete, _)) => TxnState::Complete,
            Some((Outcome::Compensated { residue }, _)) => TxnState::Compensated {
                residue: residue.clone(),
            },
            None => match &self.blocked {
                Some((step, reason, safe_facts)) => TxnState::Blocked {
                    step: *step,
                    reason: reason.clone(),
                    safe_facts: safe_facts.clone(),
                },
                None => TxnState::Interrupted { last: self.last },
            },
        }
    }
}

pub fn parse_plan(bytes: &[u8]) -> Result<Plan, String> {
    let plan: Plan = serde_json::from_slice(bytes).map_err(|e| format!("plan unreadable: {e}"))?;
    if plan.format != PLAN_FORMAT || plan.version != PLAN_VERSION {
        return Err(format!(
            "plan format {} v{} is not supported by this build ({PLAN_FORMAT} v{PLAN_VERSION})",
            plan.format, plan.version
        ));
    }
    if plan.steps.is_empty() || plan.point_of_no_return >= plan.steps.len() {
        return Err("plan has an invalid step list".into());
    }
    if !matches!(plan.steps.last(), Some(Step::FinalVerify)) {
        return Err("plan does not end with final verification".into());
    }
    Ok(plan)
}

/// A loaded, verified journal with its plan and folded history.
pub struct Loaded {
    pub journal: LoadedJournal,
    pub plan: Plan,
    history: History,
}

pub fn load(ctx: &Context<'_>, txn_id: &str) -> Result<Loaded, String> {
    let journal = journal::load(&ctx.journal_root, txn_id).map_err(|d| match d {
        JournalDamage::Corrupt(m) => m,
        JournalDamage::TornTail { .. } => "torn".into(),
    })?;
    let plan = parse_plan(&journal.plan_bytes)?;
    if plan.txn_id != txn_id {
        return Err(format!(
            "journal {txn_id} holds the plan of {}",
            plan.txn_id
        ));
    }
    if plan_bytes(&plan) != journal.plan_bytes {
        return Err("plan bytes are not in canonical form; the plan may have been edited".into());
    }
    let history = History::fold(&plan, &journal)?;
    Ok(Loaded {
        journal,
        plan,
        history,
    })
}

/// Read-only status of one transaction.
pub fn status(ctx: &Context<'_>, txn_id: &str) -> (TxnState, Option<Plan>) {
    match load(ctx, txn_id) {
        Err(reason) => (TxnState::Corrupt { reason }, None),
        Ok(loaded) => {
            let state = loaded.history.state();
            let state = if !state.is_terminal() && lock::transaction_busy(&loaded.journal.dir) {
                TxnState::Running
            } else {
                state
            };
            (state, Some(loaded.plan))
        }
    }
}

/// Every transaction under the journal root, with its status. Entries that
/// are not transactions are reported as Corrupt, never ignored.
pub fn list(ctx: &Context<'_>) -> Vec<(String, TxnState, Option<Plan>)> {
    let mut out = Vec::new();
    match journal::list(&ctx.journal_root) {
        Ok((ids, foreign)) => {
            for id in ids {
                let (state, plan) = status(ctx, &id);
                out.push((id, state, plan));
            }
            for path in foreign {
                if path.file_name().is_some_and(|n| n == lock::LOCK_DIR) {
                    continue;
                }
                out.push((
                    path.display().to_string(),
                    TxnState::Corrupt {
                        reason: "unrecognised entry in the journal directory".into(),
                    },
                    None,
                ));
            }
        }
        Err(e) => out.push((
            ctx.journal_root.display().to_string(),
            TxnState::Corrupt {
                reason: format!("journal directory unreadable: {e}"),
            },
            None,
        )),
    }
    out
}

/// Applies an approved plan. `approved_digest` must equal the digest shown
/// in preview: a plan that differs in any byte is refused (I4).
pub fn apply(
    ctx: &Context<'_>,
    plan: &Plan,
    approved_digest: &str,
) -> Result<TxnState, EngineError> {
    // First, before reading anything the caller supplied: without the
    // test-only authority there is no execution at all.
    if ctx.authority.is_none() {
        return Err(EngineError::Refused(
            super::authority::PRODUCTION_UNAVAILABLE.into(),
        ));
    }
    let bytes = plan_bytes(plan);
    let digest = journal::hex(&Sha256::digest(&bytes));
    if digest != approved_digest {
        return Err(EngineError::Refused(
            "the plan differs from the one approved in preview; nothing was changed".into(),
        ));
    }
    parse_plan(&bytes).map_err(EngineError::Refused)?;
    authorise(ctx, plan).map_err(EngineError::Refused)?;
    let _locks = lock::acquire(&ctx.journal_root, &resources(plan))
        .map_err(|e| EngineError::Refused(e.to_string()))?;
    let writer = JournalWriter::create(&ctx.journal_root, &plan.txn_id, &bytes, ctx.fault.clone())
        .map_err(crash_or)?;
    let _txn_lock =
        lock::lock_transaction(writer.dir()).map_err(|e| EngineError::Refused(e.to_string()))?;
    let mut run = Run {
        plan,
        ctx,
        writer,
        h: History::new(plan.steps.len()),
    };
    run.append(Event::Begun {
        pid: std::process::id(),
        engine: ENGINE.into(),
    })?;
    let state = run.drive()?;
    finished(ctx, plan, &state);
    Ok(state)
}

/// After a Complete Undo, records it on the relocation (audit only).
fn finished(ctx: &Context<'_>, plan: &Plan, state: &TxnState) {
    if *state == TxnState::Complete && matches!(plan.kind, PlanKind::Undo { .. }) {
        let _ = super::undo::mark_undone(ctx, plan);
    }
}

/// Recovers (resumes or compensates) one transaction. Terminal transactions
/// are returned as they are, with nothing appended (I3).
pub fn recover(ctx: &Context<'_>, txn_id: &str) -> Result<TxnState, EngineError> {
    // As in `apply`: no authority, no recovery, before any journal is read
    // or locked. Status reading uses `status`, not this.
    if ctx.authority.is_none() {
        return Err(EngineError::Refused(
            super::authority::PRODUCTION_UNAVAILABLE.into(),
        ));
    }
    let journal = match journal::load(&ctx.journal_root, txn_id) {
        Ok(j) => j,
        Err(JournalDamage::Corrupt(reason)) => return Ok(TxnState::Corrupt { reason }),
        Err(JournalDamage::TornTail { .. }) => unreachable!("load reports torn tails in-band"),
    };
    let plan = match parse_plan(&journal.plan_bytes) {
        Ok(p) if p.txn_id == txn_id && plan_bytes(&p) == journal.plan_bytes => p,
        Ok(_) => {
            return Ok(TxnState::Corrupt {
                reason: "plan does not match its journal".into(),
            });
        }
        Err(reason) => return Ok(TxnState::Corrupt { reason }),
    };
    let history = match History::fold(&plan, &journal) {
        Ok(h) => h,
        Err(reason) => return Ok(TxnState::Corrupt { reason }),
    };
    if history.state().is_terminal() {
        return Ok(history.state());
    }
    authorise(ctx, &plan).map_err(EngineError::Refused)?;
    let _locks = lock::acquire(&ctx.journal_root, &resources(&plan))
        .map_err(|e| EngineError::Refused(e.to_string()))?;
    let _txn_lock = match lock::lock_transaction(&journal.dir) {
        Ok(l) => l,
        Err(lock::LockError::Busy(_)) => return Ok(TxnState::Running),
        Err(e) => return Err(EngineError::Refused(e.to_string())),
    };
    // Re-load under the lock: another recovery may have finished meanwhile.
    let journal = match journal::load(&ctx.journal_root, txn_id) {
        Ok(j) => j,
        Err(JournalDamage::Corrupt(reason)) => return Ok(TxnState::Corrupt { reason }),
        Err(JournalDamage::TornTail { .. }) => unreachable!(),
    };
    let history = match History::fold(&plan, &journal) {
        Ok(h) => h,
        Err(reason) => return Ok(TxnState::Corrupt { reason }),
    };
    if history.state().is_terminal() {
        return Ok(history.state());
    }
    let torn = journal.torn_tail.as_ref().map(|(_, b)| journal::hex(b));
    let writer = JournalWriter::reopen(&journal, ctx.fault.clone()).map_err(crash_or)?;
    let attempt = history.recoveries + 1;
    let mut run = Run {
        plan: &plan,
        ctx,
        writer,
        h: history,
    };
    run.append(Event::RecoveryStarted {
        attempt,
        pid: std::process::id(),
        torn_tail_bytes: torn,
    })?;
    let state = run.drive()?;
    finished(ctx, &plan, &state);
    Ok(state)
}

/// Recovers every non-terminal transaction, oldest id first.
pub fn recover_all(ctx: &Context<'_>) -> Vec<(String, Result<TxnState, EngineError>)> {
    let ids = journal::list(&ctx.journal_root)
        .map(|(ids, _)| ids)
        .unwrap_or_default();
    ids.into_iter()
        .map(|id| {
            let r = recover(ctx, &id);
            (id, r)
        })
        .collect()
}

struct Run<'p, 'c, 'x> {
    plan: &'p Plan,
    ctx: &'c Context<'x>,
    writer: JournalWriter,
    h: History,
}

impl Run<'_, '_, '_> {
    fn append(&mut self, event: Event) -> Result<(), EngineError> {
        self.writer.append(event).map(|_| ()).map_err(|e| {
            if fault::is_crash(&e) {
                EngineError::Crashed(e)
            } else {
                EngineError::Interrupted(format!("the journal could not be written ({e})"))
            }
        })
    }

    fn facts(&self) -> Vec<String> {
        let mut facts = Vec::new();
        for (i, step) in self.plan.steps.iter().enumerate() {
            if self.h.fwd_done[i].is_some() {
                facts.push(format!("step {i} ({}) done", step.label()));
            }
            if self.h.comp_done[i].is_some() {
                facts.push(format!("step {i} ({}) reversed", step.label()));
            }
        }
        facts.push(format!(
            "the original is retained (never deleted) at {}",
            self.plan
                .retired_path
                .as_ref()
                .unwrap_or(&self.plan.source_path)
                .display()
        ));
        facts
    }

    fn block(
        &mut self,
        step: usize,
        reason: String,
        mut facts: Vec<String>,
    ) -> Result<TxnState, EngineError> {
        facts.extend(self.facts());
        self.append(Event::Blocked {
            step,
            reason: reason.clone(),
            safe_facts: facts.clone(),
        })?;
        Ok(TxnState::Blocked {
            step,
            reason,
            safe_facts: facts,
        })
    }

    fn drive(&mut self) -> Result<TxnState, EngineError> {
        let n = self.plan.steps.len();
        let pnr = self.plan.point_of_no_return;
        let mut compensate = self.h.fwd_done[pnr].is_none()
            && (self.h.compensating || self.h.cancelled || self.h.failed_before_pnr);
        if !compensate {
            for i in 0..n {
                if self.h.fwd_done[i].is_some() {
                    continue;
                }
                let past = self.h.fwd_done[pnr].is_some();
                if !past && self.ctx.cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                    self.append(Event::CancelRequested)?;
                    compensate = true;
                    break;
                }
                self.append(Event::Intent {
                    step: i,
                    direction: Direction::Forward,
                })?;
                self.h.fwd_attempted[i] = true;
                match self.forward(i) {
                    Ok(evidence) => {
                        self.append(Event::Done {
                            step: i,
                            direction: Direction::Forward,
                            evidence: evidence.clone(),
                        })?;
                        self.h.fwd_done[i] = Some(evidence);
                    }
                    Err(StepError::Crash(e)) => return Err(EngineError::Crashed(e)),
                    Err(StepError::Refused(error)) => {
                        self.append(Event::Failed {
                            step: i,
                            direction: Direction::Forward,
                            error: error.clone(),
                        })?;
                        if self.h.fwd_done[pnr].is_none() {
                            compensate = true;
                            break;
                        }
                        return self.block(i, error, vec![]);
                    }
                    Err(StepError::Unsafe { reason, facts }) => {
                        return self.block(i, reason, facts);
                    }
                }
            }
            if !compensate {
                self.append(Event::Outcome {
                    outcome: Outcome::Complete,
                })?;
                return Ok(TxnState::Complete);
            }
        }
        for i in (0..n).rev() {
            if !self.h.fwd_attempted[i] || self.h.comp_done[i].is_some() {
                continue;
            }
            self.append(Event::Intent {
                step: i,
                direction: Direction::Compensate,
            })?;
            match self.compensate(i) {
                Ok(evidence) => {
                    self.append(Event::Done {
                        step: i,
                        direction: Direction::Compensate,
                        evidence: evidence.clone(),
                    })?;
                    self.h.comp_done[i] = Some(evidence);
                }
                Err(StepError::Crash(e)) => return Err(EngineError::Crashed(e)),
                Err(StepError::Refused(reason)) => return self.block(i, reason, vec![]),
                Err(StepError::Unsafe { reason, facts }) => return self.block(i, reason, facts),
            }
        }
        let residue = self
            .h
            .comp_done
            .iter()
            .flatten()
            .filter_map(|e| match e {
                StepEvidence::Retained { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        self.append(Event::Outcome {
            outcome: Outcome::Compensated {
                residue: residue.clone(),
            },
        })?;
        Ok(TxnState::Compensated { residue })
    }

    fn point(&self, step: usize, at: &'static str) -> io::Result<()> {
        fault::point(&self.ctx.fault, step, at)
    }

    // ---- evidence ------------------------------------------------------

    fn staged_identity(&self) -> Option<ObjectIdentity> {
        self.plan
            .steps
            .iter()
            .enumerate()
            .find_map(|(i, s)| match (s, &self.h.fwd_done[i]) {
                (Step::CopyToStaging { .. }, Some(StepEvidence::Identity { identity })) => {
                    Some(identity.clone())
                }
                _ => None,
            })
    }

    fn dir_id(&self, dir: &Path) -> Option<DirId> {
        self.plan
            .steps
            .iter()
            .enumerate()
            .find_map(|(i, s)| match (s, &self.h.fwd_done[i]) {
                (Step::CreatePrivateDir { dir: d, .. }, Some(StepEvidence::Directory { id }))
                    if d == dir =>
                {
                    Some(*id)
                }
                _ => None,
            })
    }

    fn expected(&self, expect: Expect) -> Result<ObjectIdentity, StepError> {
        match expect {
            Expect::Original => Ok(self.plan.source_identity.clone()),
            Expect::Staged => self.staged_identity().map_or_else(
                || unsafe_("the staged copy's identity was never recorded", vec![]),
                Ok,
            ),
            Expect::Published => self
                .plan
                .published_identity
                .clone()
                .map_or_else(|| unsafe_("the plan has no published identity", vec![]), Ok),
        }
    }

    /// The identity the destination must carry once published.
    fn destination_identity(&self) -> Result<ObjectIdentity, StepError> {
        match (&self.plan.kind, self.plan.strategy) {
            (PlanKind::Undo { .. }, _) => self.expected(Expect::Published),
            (PlanKind::Relocate, Strategy::AtomicRename) => self.expected(Expect::Original),
            (PlanKind::Relocate, Strategy::CopyVerifyRetire) => self.expected(Expect::Staged),
        }
    }

    fn catalogue_entry(&self) -> Result<TransactionEntry, StepError> {
        let p = self.plan;
        let name = |path: &Path| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut value = serde_json::json!({
            "source_path": p.source_path,
            "destination_path": p.destination_path,
            "original_basename": name(&p.source_path),
            "proposed_basename": name(&p.destination_path),
            "identity": p.source_identity,
            "operation": { "kind": "rename_move" },
        });
        value[organisation_paths::CATALOGUE_BINDING_KEY] = p.catalogue_binding.clone();
        serde_json::from_value(value).map_or_else(|e| refused(format!("catalogue entry: {e}")), Ok)
    }

    /// Opens the shared catalogue move. `towards_destination` is the row's
    /// direction (forward relocation = true).
    fn catalogue(
        &self,
        towards_destination: bool,
    ) -> Result<organisation_paths::CatalogueMove, StepError> {
        let entry = self.catalogue_entry()?;
        match organisation_paths::begin(&entry, !towards_destination, true) {
            Ok(Some(mv)) => Ok(mv),
            Ok(None) => refused("the plan has no catalogue binding"),
            Err(e) => refused(e),
        }
    }

    fn roots_current(&self) -> Result<(), StepError> {
        let p = self.plan;
        let parent = |path: &Path| path.parent().map(Path::to_path_buf).unwrap_or_default();
        for (root, folder) in [
            (&p.source_root, parent(&p.source_path)),
            (&p.destination_root, parent(&p.destination_path)),
        ] {
            if SourceRootBinding::inspect(&root.path).as_ref() != Some(&root.binding) {
                return refused(format!(
                    "library root {} is unavailable or is no longer the same filesystem",
                    root.path.display()
                ));
            }
            // Fail closed: the filesystem must still be a supported one with
            // the options it was planned with, and the game's folder must
            // not have become a mount nested in the root.
            // Re-authorised before every step: the root must still be the
            // very directory the authority created (not replaced, remounted
            // or substituted).
            authorise(self.ctx, p).map_err(StepError::Refused)?;
            let policy = p.filesystem_policy;
            let (facts, _) = super::fscompat::check_root(&root.path, &[&folder], policy)
                .map_err(StepError::Refused)?;
            if !p.filesystems.contains(&facts) {
                return refused(format!(
                    "{} changed filesystem type or mount options since planning ({})",
                    root.path.display(),
                    facts.summary()
                ));
            }
        }
        // The catalogue's drive (bound at planning) and the journal's drive
        // carry the commit and the records retirement depends on.
        let policy = p.filesystem_policy;
        let catalogue_dir = p.catalogue.parent().unwrap_or(Path::new("/"));
        let (facts, _) = super::fscompat::check_path(catalogue_dir, policy)
            .map_err(|e| StepError::Refused(format!("the catalogue's drive: {e}")))?;
        if !p.filesystems.contains(&facts) {
            return refused(format!(
                "the catalogue's drive changed filesystem type or mount options since planning ({})",
                facts.summary()
            ));
        }
        super::fscompat::check_path(&self.ctx.journal_root, policy)
            .map_err(|e| StepError::Refused(format!("the transaction journal's drive: {e}")))?;
        Ok(())
    }

    // ---- forward steps -------------------------------------------------

    fn forward(&self, i: usize) -> Result<StepEvidence, StepError> {
        self.point(i, "begin")?;
        // Every step, not only the first: storage can change mid-operation
        // (remounted read-only, replaced, options changed), above all before
        // the original is retired.
        self.roots_current()?;
        match &self.plan.steps[i] {
            Step::CheckPreconditions => self.check_preconditions(),
            Step::CreatePrivateDir { dir, root } => self.create_private_dir(i, dir, root),
            Step::CopyToStaging { staged } => self.copy_to_staging(i, staged),
            Step::VerifyStaged { staged } => self.verify_staged(staged),
            Step::Rename {
                from,
                to,
                roots,
                expect,
            } => self.rename(i, from, to, roots, *expect, false),
            Step::CommitCatalogue { reverse } => self.commit(i, !*reverse, false),
            Step::DiscardDuplicate { path, keeper, root } => self.discard(i, path, keeper, root),
            Step::RemovePrivateDir { dir, root } => self.remove_private_dir(i, dir, root, false),
            Step::FinalVerify => self.final_verify(),
        }
    }

    fn check_preconditions(&self) -> Result<StepEvidence, StepError> {
        let p = self.plan;
        self.roots_current()?;
        match &p.kind {
            PlanKind::Relocate => {
                let current = capture_identity(&p.source_path)?;
                if !identity_matches(&p.source_identity, &current) {
                    return refused(
                        "the source changed since planning (content, inode or time); regenerate the plan",
                    );
                }
                if !fsops::not_found(&p.destination_path)? {
                    return refused("the destination already exists; nothing was overwritten");
                }
                let verdict = super::graph::relocation_verdict(&p.source_path);
                if !verdict.blockers.is_empty() || verdict.digest != p.dependency_digest {
                    return refused(
                        "the game's dependencies changed since planning; regenerate the plan",
                    );
                }
                // DAT-declared set dependencies, re-read now (a DAT audit
                // may have run since planning).
                let db = crate::Database::open_read_only(&p.catalogue).map_err(|e| {
                    StepError::Refused(format!("the catalogue cannot be read: {e}"))
                })?;
                let declared = crate::database::organisation_paths::audited_set_dependencies(
                    &db,
                    &p.source_path,
                )
                .map_err(StepError::Refused)?;
                drop(db);
                if !declared.is_empty() {
                    return refused(
                        "the DAT audit now records set dependencies for this file; it is not relocated by this slice",
                    );
                }
                // The row must still name the original (not already the
                // destination: that would be a catalogue this plan did not
                // see).
                if self.catalogue(true)?.already() {
                    return refused(
                        "the catalogue already names the destination; regenerate the plan",
                    );
                }
            }
            PlanKind::Undo {
                of_txn,
                of_complete_sum,
                ..
            } => {
                super::undo::forward_still_undoable(self.ctx, of_txn, of_complete_sum, &p.txn_id)
                    .map_err(StepError::Refused)?;
                let published = self.expected(Expect::Published)?;
                let current = capture_identity(&p.destination_path)?;
                if !identity_matches(&published, &current) {
                    return refused(
                        "the relocated file changed since it was relocated; Undo would discard that change",
                    );
                }
                if !fsops::not_found(&p.source_path)? {
                    return refused(
                        "something now exists at the original location; nothing was overwritten",
                    );
                }
                if let Some(retired) = &p.retired_path {
                    let r = capture_identity(retired)?;
                    if !identity_matches(&p.source_identity, &r) {
                        return refused(
                            "the retained original no longer matches its recorded identity",
                        );
                    }
                }
                // Undo starts only while the catalogue still names the
                // relocated location.
                if self.catalogue(false)?.already() {
                    return refused(
                        "the catalogue no longer names the relocated location; Undo would not match it",
                    );
                }
            }
        }
        Ok(StepEvidence::Verified {
            summary: "preconditions re-proved".into(),
        })
    }

    fn create_private_dir(
        &self,
        i: usize,
        dir: &Path,
        root: &Path,
    ) -> Result<StepEvidence, StepError> {
        let (parent, name) = fsops::open_parent(dir, root)?;
        match parent.stat(name)? {
            // The name carries this transaction's random id: an existing
            // private directory is the one an interrupted attempt created.
            Some(st) if fsops::is_private_dir(&st) => {
                // Found after a crash: it may be visible but not yet durable.
                parent.sync()?;
                return Ok(StepEvidence::Directory {
                    id: (st.st_dev, st.st_ino),
                });
            }
            Some(_) => return refused(format!("{} is occupied by something else", dir.display())),
            None => {}
        }
        parent.mkdir_private(name)?;
        self.point(i, "after_mkdir")?;
        parent.sync()?;
        let st = parent
            .stat(name)?
            .ok_or_else(|| StepError::Refused("created directory vanished".into()))?;
        Ok(StepEvidence::Directory {
            id: (st.st_dev, st.st_ino),
        })
    }

    fn copy_to_staging(&self, i: usize, staged: &Path) -> Result<StepEvidence, StepError> {
        let p = self.plan;
        let Some(proof) = p.source_identity.freshness.clone() else {
            return refused("the plan has no content proof for the source");
        };
        let (sdir, sname) = fsops::open_parent(&p.source_path, &p.source_root.path)?;
        let (ddir, dname) = fsops::open_parent(staged, &p.destination_root.path)?;
        if let Some(parent) = staged.parent()
            && let Some(id) = self.dir_id(parent)
            && id != ddir.id
        {
            return unsafe_(
                "the staging directory is not the one this plan created",
                vec![],
            );
        }
        if ddir.stat(dname)?.is_some() {
            // A previous attempt left a (possibly partial) copy. Remove it
            // only while the original is proven intact.
            let current = capture_identity(&sdir.entry(sname))?;
            if !identity_matches(&p.source_identity, &current) {
                return refused("the source changed; an earlier partial copy is retained");
            }
            ddir.unlink(dname)?;
            ddir.sync()?;
        }
        let mut source = sdir.open_read(sname)?;
        let meta = source.metadata()?;
        let id = &p.source_identity;
        if !meta.is_file()
            || meta.dev() != id.dev
            || meta.ino() != id.ino
            || meta.len() != id.size_bytes
            || meta.modified()? != proof.modified
        {
            return refused("the source changed since planning; nothing was copied");
        }
        let mut out = ddir.create_new(dname)?;
        self.point(i, "created")?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        let limit = id.size_bytes.saturating_add(1);
        let mut total = 0u64;
        let mut first = true;
        loop {
            let want = (limit - total).min(buffer.len() as u64) as usize;
            if want == 0 {
                break;
            }
            let n = source.read(&mut buffer[..want])?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            out.write_all(&buffer[..n])?;
            total += n as u64;
            if first {
                first = false;
                self.point(i, "partial")?;
            }
        }
        self.point(i, "written")?;
        let read_hash: [u8; 32] = hasher.finalize().into();
        if total != id.size_bytes || read_hash != proof.sha256 {
            return refused(
                "the source changed while it was being copied; the partial copy will be compensated",
            );
        }
        // The published copy keeps the original's permission bits (never
        // set-id or sticky bits), so other users and media services keep
        // the access they had. It is created 0600 and only widened here,
        // once its content is complete.
        {
            use std::os::unix::fs::PermissionsExt;
            out.set_permissions(std::fs::Permissions::from_mode(meta.mode() & 0o777))?;
        }
        out.set_times(std::fs::FileTimes::new().set_modified(proof.modified))?;
        out.sync_all()?;
        self.point(i, "synced")?;
        fsops::drop_cache(&out);
        drop(out);
        ddir.sync()?;
        self.point(i, "dir_synced")?;
        let identity = capture_identity(&ddir.entry(dname))?;
        Ok(StepEvidence::Identity { identity })
    }

    fn verify_staged(&self, staged: &Path) -> Result<StepEvidence, StepError> {
        let p = self.plan;
        let recorded = self.expected(Expect::Staged)?;
        let (ddir, dname) = fsops::open_parent(staged, &p.destination_root.path)?;
        if let Ok(f) = ddir.open_read(dname) {
            fsops::drop_cache(&f);
        }
        let current = capture_identity(&ddir.entry(dname))?;
        if !identity_matches(&recorded, &current)
            || !content_identity_matches(&p.source_identity, &current)
        {
            return refused("the staged copy does not match the source content");
        }
        let (sdir, sname) = fsops::open_parent(&p.source_path, &p.source_root.path)?;
        let source = capture_identity(&sdir.entry(sname))?;
        if !identity_matches(&p.source_identity, &source) {
            return refused("the source changed after it was copied");
        }
        Ok(StepEvidence::Identity { identity: current })
    }

    fn rename(
        &self,
        i: usize,
        from: &Path,
        to: &Path,
        roots: &[PathBuf],
        expect: Expect,
        compensate: bool,
    ) -> Result<StepEvidence, StepError> {
        let expected = self.expected(expect)?;
        let trusted = TrustedRoots::from_paths(roots);
        if trusted.roots().len() != {
            let mut r = roots.to_vec();
            r.sort();
            r.dedup();
            r.len()
        } {
            return refused("a library root is unavailable");
        }
        let dirs = RenameDirs::open_in(from, to, &trusted)?;
        let probe = |path: PathBuf| -> io::Result<Option<ObjectIdentity>> {
            match capture_identity(&path) {
                Ok(id) => Ok(Some(id)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        };
        let at_from = probe(dirs.source_entry())?;
        let at_to = probe(dirs.destination_entry())?;
        let ours = |x: &Option<ObjectIdentity>| {
            x.as_ref().is_some_and(|id| identity_matches(&expected, id))
        };
        let describe = || {
            vec![
                format!(
                    "{}: {}",
                    from.display(),
                    if at_from.is_some() {
                        "present"
                    } else {
                        "absent"
                    }
                ),
                format!(
                    "{}: {}",
                    to.display(),
                    if at_to.is_some() { "present" } else { "absent" }
                ),
            ]
        };
        let (reverse, landing) = if compensate {
            (true, dirs.source_entry())
        } else {
            (false, dirs.destination_entry())
        };
        let (here, there) = if compensate {
            (&at_to, &at_from)
        } else {
            (&at_from, &at_to)
        };
        if ours(here) && there.is_none() {
            self.point(
                i,
                if compensate {
                    "comp_before_rename"
                } else {
                    "before_rename"
                },
            )?;
            match dirs.rename_noreplace(reverse) {
                Ok(()) => {}
                Err(NoClobberError::DestinationExists) => {
                    return refused(
                        "the target appeared just before the rename; nothing was overwritten",
                    );
                }
                Err(e) => return refused(format!("{e:?}")),
            }
            self.point(
                i,
                if compensate {
                    "comp_after_rename"
                } else {
                    "after_rename"
                },
            )?;
            dirs.sync_and_verify(reverse)?;
            self.point(i, if compensate { "comp_synced" } else { "synced" })?;
            let landed = capture_identity(&landing)?;
            if !identity_matches(&expected, &landed) {
                return unsafe_(
                    "the moved object changed identity during the rename",
                    describe(),
                );
            }
            return Ok(StepEvidence::Identity { identity: landed });
        }
        if here.is_none() && ours(there) {
            dirs.sync_and_verify(reverse)?;
            return Ok(StepEvidence::AlreadyDone {
                finding: "the object is already in place".into(),
            });
        }
        if compensate {
            // Forward never moved it: the object is not at the destination,
            // or the destination holds something this plan did not create
            // while the original is still where it was.
            // This view may come from a reverse rename that a crash left
            // visible but not durable: make it durable before recording it.
            if at_to.is_none() && at_from.is_some() {
                dirs.sync_and_verify(reverse)?;
                return Ok(StepEvidence::AlreadyDone {
                    finding: "the object was never moved".into(),
                });
            }
            if ours(&at_from) && at_to.is_some() {
                dirs.sync_and_verify(reverse)?;
                return Ok(StepEvidence::AlreadyDone {
                    finding: "the object was never moved; the destination belongs to someone else"
                        .into(),
                });
            }
            return unsafe_(
                "the moved object cannot be located with its recorded identity",
                describe(),
            );
        }
        if ours(&at_from) && at_to.is_some() {
            return refused(
                "the target is occupied by an object this plan did not create; nothing was overwritten",
            );
        }
        if at_from.is_some() && at_to.is_none() {
            return refused(format!(
                "{} changed since it was verified (possibly a user modification); it was not moved",
                from.display()
            ));
        }
        unsafe_("neither location holds the expected object", describe())
    }

    /// Moves the catalogue row towards the destination (`towards = true`) or
    /// back towards the source. The content the row will name is re-proved
    /// first.
    fn commit(
        &self,
        i: usize,
        towards_destination: bool,
        compensating: bool,
    ) -> Result<StepEvidence, StepError> {
        let p = self.plan;
        if compensating {
            // Reversing: if the row never moved (or is already back), there is
            // nothing to do, whatever other rows a concurrent scan added at
            // the other path; those are separate entries and are left alone.
            let entry = self.catalogue_entry()?;
            match organisation_paths::bound_row_at(&entry, !towards_destination) {
                Ok(true) => {
                    return Ok(StepEvidence::AlreadyDone {
                        finding: "the catalogue row names the original location".into(),
                    });
                }
                Ok(false) => {}
                Err(e) => {
                    return unsafe_(format!("the catalogue row cannot be located: {e}"), vec![]);
                }
            }
        }
        let mv = self.catalogue(towards_destination)?;
        if mv.already() {
            return Ok(StepEvidence::AlreadyDone {
                finding: "the catalogue already names the target".into(),
            });
        }
        // Re-prove the content the row is about to name, and that the
        // original is unmodified (I6), immediately before the swap.
        let source = capture_identity(&p.source_path);
        let source_ok = source
            .as_ref()
            .is_ok_and(|s| identity_matches(&p.source_identity, s));
        let forward_relocation = matches!(p.kind, PlanKind::Relocate) == towards_destination;
        if towards_destination {
            let dest = capture_identity(&p.destination_path)?;
            if !identity_matches(&self.destination_identity()?, &dest) {
                return refused(
                    "the destination does not hold the verified copy; the catalogue was not changed",
                );
            }
            if forward_relocation && p.strategy == Strategy::CopyVerifyRetire && !source_ok {
                return refused(
                    "the source changed after it was copied; the catalogue was not changed",
                );
            }
        } else if !source_ok && !(matches!(p.kind, PlanKind::Relocate)) {
            return refused("the restored original does not match its recorded identity");
        }
        self.point(i, "before_commit")?;
        mv.commit().map_err(StepError::Refused)?;
        self.point(i, "after_commit")?;
        Ok(StepEvidence::Catalogue { rows_changed: 1 })
    }

    fn discard(
        &self,
        i: usize,
        path: &Path,
        keeper: &Path,
        root: &Path,
    ) -> Result<StepEvidence, StepError> {
        let p = self.plan;
        let (dir, name) = fsops::open_parent(path, root)?;
        if dir.stat(name)?.is_none() {
            // The unlink may be visible but not yet durable.
            dir.sync()?;
            return Ok(StepEvidence::AlreadyDone {
                finding: "the duplicate is already removed".into(),
            });
        }
        let published = self.expected(Expect::Published)?;
        let current = capture_identity(&dir.entry(name))?;
        if !identity_matches(&published, &current)
            || !content_identity_matches(&p.source_identity, &current)
        {
            return unsafe_(
                "the private directory does not hold the recorded duplicate; it is retained",
                vec![],
            );
        }
        let kept = capture_identity(keeper)?;
        if !identity_matches(&p.source_identity, &kept) {
            return refused("the original is not verified intact; the duplicate is retained");
        }
        self.point(i, "before_unlink")?;
        dir.unlink(name)?;
        self.point(i, "after_unlink").map_err(after_irreversible)?;
        dir.sync().map_err(after_irreversible)?;
        Ok(StepEvidence::Verified {
            summary: "removed a byte-identical duplicate while the verified original is in place"
                .into(),
        })
    }

    fn remove_private_dir(
        &self,
        i: usize,
        dir: &Path,
        root: &Path,
        compensate: bool,
    ) -> Result<StepEvidence, StepError> {
        let (parent, name) = fsops::open_parent(dir, root)?;
        match parent.stat(name)? {
            None => {
                // The removal may be visible but not yet durable.
                parent.sync()?;
                return Ok(StepEvidence::AlreadyDone {
                    finding: "the directory is already gone".into(),
                });
            }
            Some(st) if !fsops::is_private_dir(&st) => {
                return unsafe_(
                    format!("{} is not this plan's private directory", dir.display()),
                    vec![],
                );
            }
            Some(_) => {}
        }
        match parent.rmdir(name) {
            Ok(()) => {}
            Err(e)
                if e.raw_os_error() == Some(libc::ENOTEMPTY)
                    || e.raw_os_error() == Some(libc::EEXIST) =>
            {
                if compensate {
                    return Ok(StepEvidence::Retained {
                        path: dir.to_path_buf(),
                        reason: "the directory is not empty; its contents were kept".into(),
                    });
                }
                return unsafe_(
                    format!("{} is not empty; it is retained for review", dir.display()),
                    vec![],
                );
            }
            Err(e) => return Err(e.into()),
        }
        self.point(i, "after_rmdir")?;
        parent.sync()?;
        Ok(StepEvidence::None)
    }

    fn final_verify(&self) -> Result<StepEvidence, StepError> {
        let p = self.plan;
        let fail = |what: String| unsafe_(format!("final verification failed: {what}"), vec![]);
        self.roots_current()?;
        match p.kind {
            PlanKind::Relocate => {
                let dest = capture_identity(&p.destination_path)?;
                if !identity_matches(&self.destination_identity()?, &dest)
                    || !content_identity_matches(&p.source_identity, &dest)
                {
                    return fail("the destination does not hold the verified content".into());
                }
                if !fsops::not_found(&p.source_path)? {
                    return fail("the original location is still occupied".into());
                }
                if let Some(retired) = &p.retired_path {
                    let r = capture_identity(retired)?;
                    if !identity_matches(&p.source_identity, &r) {
                        return fail(
                            "the retained original does not match its recorded identity".into(),
                        );
                    }
                }
                let mv = self.catalogue(true)?;
                if !mv.already() {
                    return fail("the catalogue does not name the destination".into());
                }
            }
            PlanKind::Undo { .. } => {
                let source = capture_identity(&p.source_path)?;
                if !identity_matches(&p.source_identity, &source) {
                    return fail(
                        "the restored original does not match its recorded identity".into(),
                    );
                }
                if !fsops::not_found(&p.destination_path)? {
                    return fail("the relocated location is still occupied".into());
                }
                let mv = self.catalogue(false)?;
                if !mv.already() {
                    return fail("the catalogue does not name the original location".into());
                }
            }
        }
        for step in &p.steps {
            if let Step::RemovePrivateDir { dir, .. } = step
                && !fsops::not_found(dir)?
            {
                return fail(format!("{} still exists", dir.display()));
            }
        }
        Ok(StepEvidence::Verified {
            summary: "end state independently re-proved".into(),
        })
    }

    // ---- compensation --------------------------------------------------

    fn compensate(&self, i: usize) -> Result<StepEvidence, StepError> {
        self.point(i, "comp_begin")?;
        self.roots_current()?;
        let p = self.plan;
        let nothing = || {
            Ok(StepEvidence::AlreadyDone {
                finding: "nothing to reverse".into(),
            })
        };
        match &p.steps[i] {
            Step::CheckPreconditions | Step::VerifyStaged { .. } | Step::FinalVerify => nothing(),
            // Only reached when these failed before mutating: a Done discard
            // or removal is past the point of no return.
            Step::DiscardDuplicate { .. } | Step::RemovePrivateDir { .. } => nothing(),
            Step::CreatePrivateDir { dir, root } => self.remove_private_dir(i, dir, root, true),
            Step::CopyToStaging { staged } => {
                let (ddir, dname) = fsops::open_parent(staged, &p.destination_root.path)?;
                let Some(st) = ddir.stat(dname)? else {
                    // An earlier removal may be visible but not yet durable.
                    ddir.sync()?;
                    return nothing();
                };
                let source = capture_identity(&p.source_path);
                let intact = source
                    .as_ref()
                    .is_ok_and(|s| identity_matches(&p.source_identity, s));
                if !fsops::is_regular(&st) || !intact {
                    // Make the retained copy durable so the residue the
                    // outcome lists still exists after a power loss.
                    ddir.sync()?;
                    return Ok(StepEvidence::Retained {
                        path: staged.to_path_buf(),
                        reason: "the original is not verified intact, so this copy was kept".into(),
                    });
                }
                self.point(i, "comp_before_unlink")?;
                ddir.unlink(dname)?;
                ddir.sync()?;
                Ok(StepEvidence::Verified {
                    summary: "removed the staged copy; the verified original is in place".into(),
                })
            }
            Step::Rename {
                from,
                to,
                roots,
                expect,
            } => self.rename(i, from, to, roots, *expect, true),
            Step::CommitCatalogue { reverse } => self.commit(i, *reverse, true),
        }
    }
}
