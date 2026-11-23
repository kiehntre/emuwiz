//! Executable specification and exhaustive model checker of the relocation
//! protocol.
//!
//! The model mirrors the engine's decision rules (direction, point of no
//! return, probe-based steps, compensation order) over an abstract world:
//! four file locations, two private directories and the catalogue row. It
//! distinguishes the *live* view (what a running process sees, including the
//! page cache) from the *durable* view (what survives power loss): a mutation
//! changes the live view and stays pending until its fsync; a power loss
//! reverts any subset of pending mutations. Journal appends are durable when
//! they return (the engine `fdatasync`s each record); a torn append is the
//! same as a crash before it.
//!
//! Durability assumptions (deliberately weaker than ext4's whole-journal
//! ordering; see docs/design/ouroboros-transactions.md section 4):
//!
//! - D1 explicit directory durability: a directory-entry change (create,
//!   unlink, rename half, mkdir, rmdir) is durable only after an fsync of
//!   the directory that holds it. A file fsync makes no directory change
//!   durable, and fsyncing one directory says nothing about another.
//! - D2 atomic rename: after a crash, a rename has happened entirely or not
//!   at all (never both names, never neither).
//! - D3 per-name linear history: after a crash each name holds one of the
//!   values it was given, and a durable later change to a name implies its
//!   earlier changes are durable. Changes to *different* names are
//!   unordered: any subset of them may be lost.
//! - D4 file data: a file's bytes are durable after its own fsync
//!   (`sync_all`); a user's edit is a data write to the original inode.
//! - D5 SQLite commits are atomic. In rollback-journal mode a commit is
//!   visible as soon as SQLite unlinks the journal, and durable only once
//!   the catalogue's folder is fsynced. `synchronous=EXTRA` does that inside
//!   the commit, so a process death can still fall between the two. A step
//!   that finds a commit already visible (recovery, final verification)
//!   persists it before recording anything (`CatalogueMove::open`).
//!
//! A rename is `RENAME_NOREPLACE` (it fails, changing nothing, when the
//! target exists at the moment of the call) and the engine re-checks the
//! moved object's identity afterwards.
//!
//! Explored transitions from every state: the engine's next micro-action, a
//! process crash (live view kept), a power loss (each pending-mutation subset
//! reverted), and bounded adversarial user actions (modify the source,
//! create a file at the destination). After a crash the engine restarts in
//! recovery. Invariants are checked on every reachable state.
//!
//! What this proves: the protocol design keeps the invariants under every
//! interleaving of those events at this granularity. What it does not prove:
//! that the Rust implementation matches the model (the fault-injection tests
//! check that on real filesystems), or that a filesystem honours fsync.

use std::collections::{HashSet, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Obj {
    Absent,
    Orig,
    Modified,
    Partial,
    Copy,
    Foreign,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Loc {
    Source,
    Staged,
    Dest,
    Retired,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum DirL {
    Staging,
    RetiredDir,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Fs {
    files: [Obj; 4],
    dirs: [bool; 2],
    db_at_dest: bool,
}

impl Fs {
    fn get(&self, l: Loc) -> Obj {
        self.files[l as usize]
    }
    fn set(&mut self, l: Loc, o: Obj) {
        self.files[l as usize] = o;
    }
    fn dir(&self, d: DirL) -> bool {
        self.dirs[d as usize]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Step {
    Check,
    MkDir(DirL),
    Copy,
    Verify,
    Rename { from: Loc, to: Loc, expect: Obj },
    Commit,
    RmDir(DirL),
    Final,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Rec {
    Intent(u8, bool),
    Done(u8, bool),
    Failed(u8),
    Blocked,
    Recovery,
    Cancel,
    Complete,
    Compensated,
}

/// A pending (un-synced) mutation: the durable values it would revert to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Pending {
    File(Loc, Obj),
    Pair(Loc, Obj, Loc, Obj),
    Dir(DirL, bool),
    /// A user's own create at a name (not the engine's), not yet durable.
    User(Loc, Obj),
    /// A catalogue commit (or its rollback) that is visible but not yet
    /// durable: the row's previous position. In rollback-journal mode the
    /// commit point is the journal's unlink, a directory change in the
    /// catalogue's folder (D5).
    Db(bool),
}

/// Directories that can be fsynced, as bits: the source folder (holds the
/// source and the retirement directory), the destination folder (holds the
/// destination and the staging directory), and the two private directories.
const SRC: u8 = 1;
const DST: u8 = 2;
const STG: u8 = 4;
const RET: u8 = 8;
/// The catalogue's folder (its rollback journal's directory entry).
const CAT: u8 = 16;

fn loc_dir(l: Loc) -> u8 {
    match l {
        Loc::Source => SRC,
        Loc::Dest => DST,
        Loc::Staged => STG,
        Loc::Retired => RET,
    }
}

fn parent_dir(d: DirL) -> u8 {
    match d {
        DirL::Staging => DST,
        DirL::RetiredDir => SRC,
    }
}

impl Pending {
    /// The directories that must all be synced for this mutation to be
    /// durable.
    fn needs(&self) -> u8 {
        match *self {
            Pending::File(l, _) => loc_dir(l),
            Pending::Pair(a, _, b, _) => loc_dir(a) | loc_dir(b),
            Pending::Dir(d, _) => parent_dir(d),
            Pending::User(l, _) => loc_dir(l),
            Pending::Db(_) => CAT,
        }
    }

    /// The names (file locations and private-directory entries) it changes.
    fn names(&self) -> u8 {
        let loc = |l: Loc| 1u8 << (l as u8);
        match *self {
            Pending::File(l, _) | Pending::User(l, _) => loc(l),
            Pending::Pair(a, _, b, _) => loc(a) | loc(b),
            Pending::Dir(d, _) => 1u8 << (4 + d as u8),
            Pending::Db(_) => 1u8 << 6,
        }
    }
}

/// Marks durable every pending change `durable` selects, then (D2, D3) every
/// earlier change to a name a durable change also touches.
fn make_durable(pending: &mut Vec<Pending>, durable: impl Fn(&Pending) -> bool) {
    let mut keep: Vec<bool> = pending.iter().map(|p| !durable(p)).collect();
    loop {
        let mut changed = false;
        for i in 0..pending.len() {
            if keep[i]
                && (i + 1..pending.len())
                    .any(|j| !keep[j] && pending[i].names() & pending[j].names() != 0)
            {
                keep[i] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut it = keep.into_iter();
    pending.retain(|_| it.next().unwrap());
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Op {
    Set(Loc, Obj),
    Move(Loc, Loc),
    Mkdir(DirL),
    Rmdir(DirL),
    Db(bool),
    /// fsync of the directories in the mask.
    Sync(u8),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Phase {
    /// Deciding the next step.
    Idle,
    /// Inside step `i` (`comp` = compensation): remaining ops, then the
    /// record to append.
    InStep {
        i: u8,
        comp: bool,
        ops: Vec<Op>,
        then: Result<(), StepEnd>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum StepEnd {
    Refused,
    Unsafe,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct State {
    live: Fs,
    pending: Vec<Pending>,
    journal: Vec<Rec>,
    phase: Phase,
    crashes: u8,
    modified_source: bool,
    foreign_dest: bool,
    /// The single bounded user action has happened.
    interfered: bool,
    stopped: bool,
    cancel: bool,
}

struct Protocol {
    steps: Vec<Step>,
    pnr: u8,
    atomic: bool,
    /// Whether a step that finds its effect already present fsyncs it
    /// (the engine does; the negative control turns it off).
    sync_on_probe: bool,
    /// Whether a commit step (or final verification) that finds the
    /// catalogue already at its target makes it durable before recording
    /// it (the engine does, through `CatalogueMove::open`; the negative
    /// control turns it off).
    db_sync_on_probe: bool,
    /// Whether renames refuse an existing target (the engine always uses
    /// `RENAME_NOREPLACE`; the negative control turns it off).
    noreplace: bool,
}

fn cvr() -> Protocol {
    use DirL::*;
    use Loc::*;
    Protocol {
        steps: vec![
            Step::Check,
            Step::MkDir(Staging),
            Step::MkDir(RetiredDir),
            Step::Copy,
            Step::Verify,
            Step::Rename {
                from: Staged,
                to: Dest,
                expect: Obj::Copy,
            },
            Step::Commit,
            Step::Rename {
                from: Source,
                to: Retired,
                expect: Obj::Orig,
            },
            Step::RmDir(Staging),
            Step::Final,
        ],
        pnr: 7,
        atomic: false,
        sync_on_probe: true,
        db_sync_on_probe: true,
        noreplace: true,
    }
}

fn atomic() -> Protocol {
    use Loc::*;
    Protocol {
        steps: vec![
            Step::Check,
            Step::Rename {
                from: Source,
                to: Dest,
                expect: Obj::Orig,
            },
            Step::Commit,
            Step::Final,
        ],
        pnr: 2,
        atomic: true,
        sync_on_probe: true,
        db_sync_on_probe: true,
        noreplace: true,
    }
}

struct Folded {
    attempted: Vec<bool>,
    done: Vec<bool>,
    comp_done: Vec<bool>,
    compensating: bool,
    failed_before_pnr: bool,
    cancelled: bool,
    terminal: Option<Rec>,
    blocked: bool,
}

fn fold(p: &Protocol, j: &[Rec]) -> Folded {
    let n = p.steps.len();
    let mut f = Folded {
        attempted: vec![false; n],
        done: vec![false; n],
        comp_done: vec![false; n],
        compensating: false,
        failed_before_pnr: false,
        cancelled: false,
        terminal: None,
        blocked: false,
    };
    for r in j {
        match *r {
            Rec::Intent(i, c) => {
                f.blocked = false;
                if c {
                    f.compensating = true
                } else {
                    f.attempted[i as usize] = true
                }
            }
            Rec::Done(i, c) => {
                if c {
                    f.comp_done[i as usize] = true
                } else {
                    f.done[i as usize] = true
                }
            }
            Rec::Failed(i) => {
                if !f.done[p.pnr as usize] && i <= p.pnr {
                    f.failed_before_pnr = true
                }
            }
            Rec::Cancel => f.cancelled = true,
            Rec::Blocked => f.blocked = true,
            Rec::Recovery => f.blocked = false,
            Rec::Complete | Rec::Compensated => f.terminal = Some(*r),
        }
    }
    f
}

/// The step body: probes the live world and returns the ops it would issue
/// and how it ends. Mirrors `engine::Run::{forward, compensate}`.
fn body(p: &Protocol, s: &State, i: usize, comp: bool) -> (Vec<Op>, Result<(), StepEnd>) {
    use Obj::*;
    use Op::*;
    let w = &s.live;
    let ok = |ops: Vec<Op>| (ops, Ok(()));
    let refuse = (vec![], Err(StepEnd::Refused));
    let unsafe_ = (vec![], Err(StepEnd::Unsafe));
    let dest_expect = if p.atomic { Orig } else { Copy };
    // A probe that finds the effect present: durable only if it fsyncs.
    let found = |mask: u8| {
        (
            if p.sync_on_probe {
                vec![Sync(mask)]
            } else {
                vec![]
            },
            Ok(()),
        )
    };
    if !comp {
        match p.steps[i] {
            Step::Check => {
                if w.get(Loc::Source) == Orig && w.get(Loc::Dest) == Absent {
                    ok(vec![])
                } else {
                    refuse
                }
            }
            Step::MkDir(d) => {
                if w.dir(d) {
                    found(parent_dir(d))
                } else {
                    ok(vec![Mkdir(d), Sync(parent_dir(d))])
                }
            }
            Step::Copy => {
                if !w.dir(DirL::Staging) {
                    return refuse;
                }
                let mut ops = vec![];
                if w.get(Loc::Staged) != Absent {
                    if w.get(Loc::Source) != Orig {
                        return refuse;
                    }
                    ops.extend([Set(Loc::Staged, Absent), Sync(STG)]);
                }
                if w.get(Loc::Source) != Orig {
                    return refuse;
                }
                ops.extend([Set(Loc::Staged, Partial), Set(Loc::Staged, Copy), Sync(STG)]);
                ok(ops)
            }
            Step::Verify => {
                if w.get(Loc::Staged) == Copy && w.get(Loc::Source) == Orig {
                    ok(vec![])
                } else {
                    refuse
                }
            }
            Step::Rename { from, to, expect } => {
                let (f, t) = (w.get(from), w.get(to));
                let both = loc_dir(from) | loc_dir(to);
                if (to == Loc::Retired && !w.dir(DirL::RetiredDir))
                    || (from == Loc::Staged && !w.dir(DirL::Staging))
                {
                    return refuse;
                }
                if f == expect && t == Absent {
                    ok(vec![Move(from, to), Sync(both)])
                } else if f == Absent && t == expect {
                    found(both)
                } else if f == expect || (f != Absent && t == Absent) {
                    refuse
                } else {
                    unsafe_
                }
            }
            Step::Commit => {
                if w.db_at_dest {
                    // Visible is not durable: persist before recording.
                    return ok(if p.db_sync_on_probe {
                        vec![Sync(CAT)]
                    } else {
                        vec![]
                    });
                }
                if w.get(Loc::Dest) != dest_expect || (!p.atomic && w.get(Loc::Source) != Orig) {
                    return refuse;
                }
                // synchronous=EXTRA: the commit fsyncs the journal's folder.
                ok(vec![Db(true), Sync(CAT)])
            }
            Step::RmDir(d) => {
                if !w.dir(d) {
                    found(parent_dir(d))
                } else if w.get(Loc::Staged) != Absent {
                    unsafe_
                } else {
                    ok(vec![Rmdir(d), Sync(parent_dir(d))])
                }
            }
            Step::Final => {
                let good = w.get(Loc::Dest) == dest_expect
                    && w.get(Loc::Source) == Absent
                    && (p.atomic || w.get(Loc::Retired) == Orig)
                    && w.db_at_dest
                    && !w.dir(DirL::Staging);
                if !good {
                    unsafe_
                } else if p.db_sync_on_probe {
                    ok(vec![Sync(CAT)])
                } else {
                    ok(vec![])
                }
            }
        }
    } else {
        match p.steps[i] {
            Step::Check | Step::Verify | Step::Final | Step::RmDir(_) => ok(vec![]),
            Step::MkDir(d) => {
                let occupied = match d {
                    DirL::Staging => w.get(Loc::Staged) != Absent,
                    DirL::RetiredDir => w.get(Loc::Retired) != Absent,
                };
                if !w.dir(d) {
                    found(parent_dir(d))
                } else if occupied {
                    ok(vec![]) // retained
                } else {
                    ok(vec![Rmdir(d), Sync(parent_dir(d))])
                }
            }
            Step::Copy => {
                // Nothing staged, or the staged copy is retained as residue
                // (the source is no longer the original): made durable.
                if w.get(Loc::Staged) == Absent || w.get(Loc::Source) != Orig {
                    found(STG)
                } else {
                    ok(vec![Set(Loc::Staged, Absent), Sync(STG)])
                }
            }
            Step::Rename { from, to, expect } => {
                let (f, t) = (w.get(from), w.get(to));
                let both = loc_dir(from) | loc_dir(to);
                if f == Absent && t == expect {
                    ok(vec![Move(to, from), Sync(both)])
                } else if t == Absent && f != Absent || f == expect && t != Absent {
                    found(both)
                } else {
                    unsafe_
                }
            }
            Step::Commit => {
                if !w.db_at_dest {
                    ok(if p.db_sync_on_probe {
                        vec![Sync(CAT)]
                    } else {
                        vec![]
                    })
                } else {
                    ok(vec![Db(false), Sync(CAT)])
                }
            }
        }
    }
}

fn apply_op(s: &mut State, op: Op) {
    match op {
        Op::Set(l, o) => {
            let before = s.live.get(l);
            s.pending.push(Pending::File(l, before));
            s.live.set(l, o);
        }
        Op::Move(a, b) => {
            let (va, vb) = (s.live.get(a), s.live.get(b));
            s.pending.push(Pending::Pair(a, va, b, vb));
            s.live.set(b, va);
            s.live.set(a, Obj::Absent);
        }
        Op::Mkdir(d) => {
            s.pending.push(Pending::Dir(d, s.live.dirs[d as usize]));
            s.live.dirs[d as usize] = true;
        }
        Op::Rmdir(d) => {
            s.pending.push(Pending::Dir(d, s.live.dirs[d as usize]));
            s.live.dirs[d as usize] = false;
        }
        Op::Db(v) => {
            // SQLite commit: atomic (D5), visible at once, durable only
            // once the catalogue's folder is synced.
            s.pending.push(Pending::Db(s.live.db_at_dest));
            s.live.db_at_dest = v;
        }
        Op::Sync(mask) => make_durable(&mut s.pending, |p| p.needs() & !mask == 0),
    }
}

/// The engine's next transition from `s`, or None when it has stopped.
fn engine_step(p: &Protocol, s: &State) -> Option<State> {
    if s.stopped {
        return None;
    }
    let mut n = s.clone();
    match &s.phase {
        Phase::InStep { i, comp, ops, then } => {
            if let Some((first, rest)) = ops.split_first() {
                let mut then = *then;
                let mut rest = rest.to_vec();
                if let Op::Move(a, b) = *first {
                    if p.noreplace && s.live.get(b) != Obj::Absent {
                        // RENAME_NOREPLACE: the target appeared after the
                        // probe. Nothing moves; the step refuses.
                        then = Err(StepEnd::Refused);
                        rest.clear();
                    } else {
                        let moved = s.live.get(a);
                        apply_op(&mut n, *first);
                        // The engine re-checks the moved object's identity.
                        if let Step::Rename { expect, .. } = p.steps[*i as usize]
                            && moved != expect
                        {
                            then = Err(StepEnd::Unsafe);
                        }
                    }
                } else {
                    apply_op(&mut n, *first);
                }
                n.phase = Phase::InStep {
                    i: *i,
                    comp: *comp,
                    ops: rest,
                    then,
                };
                return Some(n);
            }
            n.phase = Phase::Idle;
            match then {
                Ok(()) => n.journal.push(Rec::Done(*i, *comp)),
                Err(StepEnd::Refused) if !comp => n.journal.push(Rec::Failed(*i)),
                Err(_) => {
                    n.journal.push(Rec::Blocked);
                    n.stopped = true;
                }
            }
            Some(n)
        }
        Phase::Idle => {
            let f = fold(p, &s.journal);
            if f.terminal.is_some() || f.blocked {
                return None;
            }
            let steps = p.steps.len();
            let pnr = p.pnr as usize;
            let compensate = !f.done[pnr] && (f.compensating || f.cancelled || f.failed_before_pnr);
            if !compensate {
                if let Some(i) = (0..steps).find(|&i| !f.done[i]) {
                    if !f.done[pnr] && s.cancel && !f.cancelled {
                        n.journal.push(Rec::Cancel);
                        return Some(n);
                    }
                    n.journal.push(Rec::Intent(i as u8, false));
                    let (ops, then) = body(p, &n, i, false);
                    n.phase = Phase::InStep {
                        i: i as u8,
                        comp: false,
                        ops,
                        then,
                    };
                    return Some(n);
                }
                n.journal.push(Rec::Complete);
                n.stopped = true;
                return Some(n);
            }
            if let Some(i) = (0..steps)
                .rev()
                .find(|&i| f.attempted[i] && !f.comp_done[i])
            {
                n.journal.push(Rec::Intent(i as u8, true));
                let (ops, then) = body(p, &n, i, true);
                n.phase = Phase::InStep {
                    i: i as u8,
                    comp: true,
                    ops,
                    then,
                };
                return Some(n);
            }
            n.journal.push(Rec::Compensated);
            n.stopped = true;
            Some(n)
        }
    }
}

fn restart(mut s: State) -> State {
    s.phase = Phase::Idle;
    s.crashes += 1;
    let f_terminal = s
        .journal
        .iter()
        .any(|r| matches!(r, Rec::Complete | Rec::Compensated));
    if !f_terminal {
        s.journal.push(Rec::Recovery);
        s.stopped = false;
    }
    s
}

fn power_loss_variants(s: &State) -> Vec<State> {
    power_loss_variants_with(s, true)
}

/// Every state a power loss can leave. With `linear` (D3) a change can be
/// lost only together with every later change to the same name; without it
/// (the model before 2026-11-13, kept for the regression tests) any subset
/// can be lost, which can "restore" a stale value over a later change.
fn power_loss_variants_with(s: &State, linear: bool) -> Vec<State> {
    let k = s.pending.len();
    let mut out = Vec::new();
    for mask in 0u32..(1 << k) {
        let lost = |i: usize| mask & (1 << i) != 0;
        if linear
            && (0..k).any(|i| {
                lost(i)
                    && (i + 1..k)
                        .any(|j| !lost(j) && s.pending[i].names() & s.pending[j].names() != 0)
            })
        {
            continue;
        }
        let mut n = s.clone();
        // Revert the selected pending mutations, newest first.
        for (bit, pending) in s.pending.iter().enumerate().rev() {
            if mask & (1 << bit) == 0 {
                continue;
            }
            match *pending {
                Pending::File(l, o) => n.live.set(l, o),
                Pending::Pair(a, va, b, vb) => {
                    n.live.set(a, va);
                    n.live.set(b, vb);
                }
                Pending::Dir(d, v) => n.live.dirs[d as usize] = v,
                Pending::Db(v) => n.live.db_at_dest = v,
                Pending::User(l, o) => {
                    // The user's create never became durable: nothing of
                    // theirs exists to protect.
                    n.live.set(l, o);
                    n.foreign_dest = false;
                }
            }
        }
        n.pending.clear();
        out.push(restart(n));
    }
    out
}

/// Every change the engine made is durable. (The user's own unsynced
/// changes are theirs to sync.)
fn engine_durable(s: &State) -> bool {
    s.pending.iter().all(|p| matches!(p, Pending::User(..)))
}

fn check(p: &Protocol, s: &State) -> Result<(), String> {
    use Obj::*;
    let w = &s.live;
    let f = fold(p, &s.journal);
    let holds = |o: Obj| {
        [Loc::Source, Loc::Retired, Loc::Dest, Loc::Staged]
            .iter()
            .any(|&l| w.get(l) == o)
    };
    // Original content is never lost (I1). With a user edit, the edited
    // content is never lost (I6). A user's destination file is never lost.
    if !s.modified_source && !holds(Orig) {
        return Err("original content lost".into());
    }
    if s.modified_source && !holds(Modified) {
        return Err("user-modified content lost".into());
    }
    // A user edit racing the retirement rename itself can land in the
    // retention folder (it is kept, never lost); it must never be reported
    // as a completed relocation.
    if s.modified_source && w.get(Loc::Retired) == Modified && f.terminal == Some(Rec::Complete) {
        return Err("a user-modified original was retired and reported complete".into());
    }
    if s.foreign_dest && !holds(Foreign) {
        return Err("a user's destination file was overwritten".into());
    }
    // I1: the original leaves the library only with a verified successor
    // named by the catalogue.
    if !p.atomic && w.get(Loc::Retired) == Orig && !(w.get(Loc::Dest) == Copy && w.db_at_dest) {
        return Err("original retired without a verified successor in the catalogue".into());
    }
    // The catalogue never names a location the engine left empty.
    let dest_expect = if p.atomic { Orig } else { Copy };
    if w.db_at_dest && w.get(Loc::Dest) != dest_expect {
        return Err("catalogue names a destination that does not hold the content".into());
    }
    // I7 and I10.
    match f.terminal {
        Some(Rec::Complete) => {
            let good = w.get(Loc::Dest) == dest_expect
                && w.get(Loc::Source) == Absent
                && (p.atomic || w.get(Loc::Retired) == Orig)
                && w.db_at_dest
                && !w.dir(DirL::Staging)
                && engine_durable(s);
            if !good {
                return Err(format!("Complete recorded but world is {w:?}"));
            }
        }
        Some(Rec::Compensated) => {
            let src = w.get(Loc::Source);
            let good = (src == Orig || src == Modified)
                && !w.db_at_dest
                && matches!(w.get(Loc::Dest), Absent | Foreign)
                && w.get(Loc::Retired) == Absent
                && !w.dir(DirL::RetiredDir)
                && (src == Modified || (w.get(Loc::Staged) == Absent && !w.dir(DirL::Staging)))
                && engine_durable(s);
            if !good {
                return Err(format!("Compensated recorded but world is {w:?}"));
            }
        }
        _ => {}
    }
    Ok(())
}

/// The user edits the source: a data write to the original inode, made
/// durable by the user's own file fsync (D4). `Orig` names that inode, so
/// with `inode_semantics` the edit changes it wherever it is, including in
/// the values a power loss would restore; it changes no name. Without it
/// (the model before 2026-11-13) a pending rename can "restore" the stale
/// pre-edit value.
fn user_edits_source(s: &State, inode_semantics: bool) -> State {
    let mut m = s.clone();
    m.live.set(Loc::Source, Obj::Modified);
    if inode_semantics {
        let edit = |o: &mut Obj| {
            if *o == Obj::Orig {
                *o = Obj::Modified;
            }
        };
        for p in &mut m.pending {
            match p {
                Pending::File(_, o) | Pending::User(_, o) => edit(o),
                Pending::Pair(_, a, _, b) => {
                    edit(a);
                    edit(b);
                }
                Pending::Dir(..) | Pending::Db(_) => {}
            }
        }
    }
    m.modified_source = true;
    m.interfered = true;
    m
}

/// The user creates a file at the destination name: a directory change,
/// durable only after a directory fsync (D1). Explored both without that
/// fsync (a power loss may then lose the user's file, which is not the
/// engine's doing) and with the user fsyncing the destination folder.
fn user_creates_destination(s: &State, user_syncs_folder: bool) -> State {
    let mut m = s.clone();
    m.live.set(Loc::Dest, Obj::Foreign);
    m.pending.push(Pending::User(Loc::Dest, Obj::Absent));
    if user_syncs_folder {
        make_durable(&mut m.pending, |p| p.needs() & !DST == 0);
    }
    m.foreign_dest = true;
    m.interfered = true;
    m
}

struct Report {
    states: usize,
    terminal_complete: usize,
    terminal_compensated: usize,
    blocked: usize,
    /// States where a user edit raced the retirement rename into retention.
    modified_retained: usize,
}

fn explore(p: &Protocol, max_crashes: u8, adversary: bool, cancel: bool) -> Result<Report, String> {
    use Obj::*;
    let mut files = [Absent; 4];
    files[Loc::Source as usize] = Orig;
    let init = State {
        live: Fs {
            files,
            dirs: [false; 2],
            db_at_dest: false,
        },
        pending: vec![],
        journal: vec![],
        phase: Phase::Idle,
        crashes: 0,
        modified_source: false,
        foreign_dest: false,
        interfered: false,
        stopped: false,
        cancel: false,
    };
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([init]);
    let mut report = Report {
        states: 0,
        terminal_complete: 0,
        terminal_compensated: 0,
        blocked: 0,
        modified_retained: 0,
    };
    while let Some(s) = queue.pop_front() {
        if !seen.insert(s.clone()) {
            continue;
        }
        report.states += 1;
        check(p, &s).map_err(|e| format!("{e}\nstate: {s:?}"))?;
        if s.live.get(Loc::Retired) == Modified {
            report.modified_retained += 1;
        }
        let mut next = Vec::new();
        match engine_step(p, &s) {
            Some(n) => next.push(n),
            None => {
                let f = fold(p, &s.journal);
                match f.terminal {
                    Some(Rec::Complete) => report.terminal_complete += 1,
                    Some(Rec::Compensated) => report.terminal_compensated += 1,
                    _ => {
                        report.blocked += 1;
                        // Blocked is acceptable only after interference.
                        if !s.interfered {
                            return Err(format!(
                                "engine stopped without an outcome and without interference: {s:?}"
                            ));
                        }
                    }
                }
                // I3: recovering a stopped transaction changes nothing
                // when it is terminal.
                if f.terminal.is_some() {
                    let r = restart(s.clone());
                    if engine_step(p, &r).is_some() || r.live != s.live {
                        return Err("recovery of a terminal transaction acted".into());
                    }
                }
            }
        }
        if s.crashes < max_crashes {
            let mut crashed = s.clone();
            crashed.stopped = false;
            next.push(restart(crashed));
            next.extend(power_loss_variants(&s));
        }
        if adversary && !s.interfered {
            if s.live.get(Loc::Source) == Orig {
                next.push(user_edits_source(&s, true));
            }
            if s.live.get(Loc::Dest) == Absent {
                next.push(user_creates_destination(&s, false));
                next.push(user_creates_destination(&s, true));
            }
        }
        if cancel && !s.cancel {
            let mut m = s.clone();
            m.cancel = true;
            next.push(m);
        }
        queue.extend(next);
    }
    Ok(report)
}

#[test]
fn cross_filesystem_protocol_survives_crashes_and_power_loss() {
    let r = explore(&cvr(), 3, false, false).unwrap_or_else(|e| panic!("{e}"));
    eprintln!(
        "CVR crash/power-loss: {} states, {} complete, {} compensated, {} blocked",
        r.states, r.terminal_complete, r.terminal_compensated, r.blocked
    );
    assert!(r.terminal_complete > 0);
    assert_eq!(
        r.blocked, 0,
        "no interference: every schedule must reach an outcome"
    );
}

#[test]
fn cross_filesystem_protocol_with_user_interference_and_cancel() {
    let r = explore(&cvr(), 2, true, true).unwrap_or_else(|e| panic!("{e}"));
    eprintln!(
        "CVR adversarial: {} states, {} complete, {} compensated, {} blocked",
        r.states, r.terminal_complete, r.terminal_compensated, r.blocked
    );
    assert!(r.terminal_compensated > 0);
}

#[test]
fn same_filesystem_protocol_survives_crashes_power_loss_and_interference() {
    let r = explore(&atomic(), 3, true, true).unwrap_or_else(|e| panic!("{e}"));
    eprintln!(
        "atomic: {} states, {} complete, {} compensated, {} blocked",
        r.states, r.terminal_complete, r.terminal_compensated, r.blocked
    );
    assert!(r.terminal_complete > 0 && r.terminal_compensated > 0);
}

/// Negative control: the checker must catch a protocol that retires the
/// original before the catalogue commit (an I1 violation).
#[test]
fn checker_rejects_retire_before_commit() {
    let mut p = cvr();
    p.steps.swap(6, 7); // retire, then commit
    p.pnr = 6;
    let err = explore(&p, 1, false, false)
        .err()
        .expect("violation must be found");
    assert!(err.contains("successor"), "{err}");
}

/// Negative control: the power-loss model must make an unsynced publish
/// visibly unsafe, or the main tests would prove nothing about fsync.
#[test]
fn unsynced_publish_is_caught() {
    let p = cvr();
    let mut found = false;
    let mut s = State {
        live: Fs {
            files: [Obj::Orig, Obj::Copy, Obj::Absent, Obj::Absent],
            dirs: [true, true],
            db_at_dest: false,
        },
        pending: vec![],
        journal: vec![],
        phase: Phase::Idle,
        crashes: 0,
        modified_source: false,
        foreign_dest: false,
        interfered: false,
        stopped: false,
        cancel: false,
    };
    apply_op(&mut s, Op::Move(Loc::Staged, Loc::Dest)); // no fsync
    apply_op(&mut s, Op::Db(true));
    for v in power_loss_variants(&s) {
        if check(&p, &v).is_err() {
            found = true;
        }
    }
    assert!(found, "a publish without fsync must be detectably unsafe");
}

/// Negative control: a step that finds its effect already present after a
/// crash and records Done without fsyncing it can be reverted by a later
/// power loss after the outcome was recorded. The engine fsyncs on every
/// such probe path; the checker must catch the version that does not.
#[test]
fn already_present_without_fsync_is_caught() {
    let mut p = cvr();
    p.sync_on_probe = false;
    let err = explore(&p, 3, false, true)
        .err()
        .expect("violation must be found");
    assert!(
        err.contains("recorded but world") || err.contains("catalogue names"),
        "{err}"
    );
}

/// Negative control for the 2026-11-14 review finding: recovery that finds
/// the catalogue commit already visible (the process died after SQLite's
/// journal unlink, before the directory fsync) and records it without
/// making it durable. A later power loss rolls the row back after the
/// journal says Done. The engine persists first; the checker must catch
/// the version that does not.
#[test]
fn already_committed_catalogue_without_persisting_is_caught() {
    let mut p = cvr();
    p.db_sync_on_probe = false;
    let err = explore(&p, 3, false, true)
        .err()
        .expect("violation must be found");
    assert!(
        err.contains("recorded but world") || err.contains("catalogue"),
        "{err}"
    );
}

// ---- regressions for the counterexamples found on 2026-11-13 ---------------
//
// Each test rebuilds the state of one counterexample the checker reported
// and shows what changed: either the engine (and the model's mirror of it)
// was fixed, or the old model allowed a physically impossible state. Both
// kinds are kept so a future change cannot silently reintroduce them.

fn blank(files: [Obj; 4], dirs: [bool; 2]) -> State {
    State {
        live: Fs {
            files,
            dirs,
            db_at_dest: false,
        },
        pending: vec![],
        journal: vec![],
        phase: Phase::Idle,
        crashes: 0,
        modified_source: false,
        foreign_dest: false,
        interfered: false,
        stopped: false,
        cancel: false,
    }
}

/// CE-1 (engine fix): recovery found the published copy in place after a
/// crash before its fsync, recorded Done and committed the catalogue; a
/// power loss then reverted the publish. Fixed by fsyncing on every probe
/// path; `already_present_without_fsync_is_caught` is the whole-protocol
/// control, this is the exact state.
#[test]
fn regression_ce1_publish_found_in_place_is_made_durable() {
    use Loc::*;
    use Obj::*;
    let p = cvr();
    let mut s = blank([Orig, Copy, Absent, Absent], [true, true]);
    apply_op(&mut s, Op::Move(Staged, Dest)); // the crashed attempt
    let (ops, _) = body(&p, &s, 5, false); // recovery's probe of the publish
    assert!(ops.contains(&Op::Sync(STG | DST)), "{ops:?}");
    for op in ops {
        apply_op(&mut s, op);
    }
    assert!(
        s.pending.is_empty(),
        "the publish must be durable before Done"
    );
}

/// CE-2 (engine fix): the same, for compensation removing the staging
/// directory after cancellation; Compensated was recorded with the
/// directory still able to reappear.
#[test]
fn regression_ce2_removed_directory_found_gone_is_made_durable() {
    use Obj::*;
    let p = cvr();
    let mut s = blank([Orig, Absent, Absent, Absent], [true, false]);
    apply_op(&mut s, Op::Rmdir(DirL::Staging)); // the crashed attempt
    let (ops, _) = body(&p, &s, 1, true);
    for op in ops {
        apply_op(&mut s, op);
    }
    assert!(
        s.pending.is_empty(),
        "the removal must be durable before Done"
    );
}

/// CE-3 (model fix, mirrors the engine): a destination created between the
/// probe and the rename was overwritten because the old model's rename
/// clobbered. The engine uses RENAME_NOREPLACE; without it the checker
/// must find the overwrite.
#[test]
fn regression_ce3_rename_never_clobbers() {
    let mut p = atomic();
    p.noreplace = false;
    let err = explore(&p, 1, true, false)
        .err()
        .expect("a clobbering rename must be caught");
    assert!(err.contains("overwritten"), "{err}");
}

/// CE-4 (model fix): the forward rename and the compensating rename back
/// were both unsynced when the user edited the restored original. The old
/// model could revert only the first rename and "restore" the stale
/// pre-edit value over the edit: impossible, because the edit is a data
/// write to the same inode (D4) and the second rename is a later change to
/// the same names (D3).
#[test]
fn regression_ce4_power_loss_cannot_restore_a_stale_value_over_a_user_edit() {
    use Loc::*;
    use Obj::*;
    let p = atomic();
    let mut s = blank([Orig, Absent, Absent, Absent], [false, false]);
    apply_op(&mut s, Op::Move(Source, Dest));
    apply_op(&mut s, Op::Move(Dest, Source));
    // The old model reproduces the counterexample...
    let old = user_edits_source(&s, false);
    assert!(
        power_loss_variants_with(&old, false)
            .iter()
            .any(|v| check(&p, v).is_err_and(|e| e.contains("modified"))),
        "the old model's counterexample must still reproduce"
    );
    // ...the current one cannot, and the edit survives every power loss.
    let new = user_edits_source(&s, true);
    for v in power_loss_variants(&new) {
        check(&p, &v).unwrap();
        assert!(
            [Source, Dest].iter().any(|&l| v.live.get(l) == Modified),
            "{v:?}"
        );
    }
}

/// CE-5 (model fix): the publish and the compensating withdrawal were both
/// unsynced when the user created a file at the destination. The old model
/// could revert only the publish, deleting the user's file. Under D3 the
/// publish can be lost only together with the later changes to the
/// destination name, including the user's own create; if the user fsynced
/// the folder, all of them are durable. In no case does the engine remove
/// a durable user file. D1 is kept: the user's create is *not* assumed
/// durable without a directory fsync.
#[test]
fn regression_ce5_power_loss_cannot_remove_a_later_user_file() {
    use Loc::*;
    use Obj::*;
    let p = cvr();
    let mut s = blank([Orig, Copy, Absent, Absent], [true, true]);
    apply_op(&mut s, Op::Move(Staged, Dest));
    apply_op(&mut s, Op::Move(Dest, Staged));
    let unsynced = user_creates_destination(&s, false);
    assert!(
        power_loss_variants_with(&unsynced, false)
            .iter()
            .any(|v| check(&p, v).is_err_and(|e| e.contains("overwritten"))),
        "the old model's counterexample must still reproduce"
    );
    for v in power_loss_variants(&unsynced) {
        check(&p, &v).unwrap();
        // Either the user's create survived, or it never became durable.
        assert!(v.live.get(Dest) == Foreign || !v.foreign_dest, "{v:?}");
    }
    let synced = user_creates_destination(&s, true);
    assert!(
        synced.pending.is_empty(),
        "the user's folder fsync makes the earlier renames there durable (D2, D3)"
    );
    for v in power_loss_variants(&synced) {
        assert_eq!(v.live.get(Dest), Foreign);
    }
}

/// CE-6 (engine fix): compensation retained an unsynced partial copy as
/// residue; Compensated listed a path a power loss could remove.
#[test]
fn regression_ce6_retained_residue_is_made_durable() {
    use Obj::*;
    let p = cvr();
    let mut s = blank([Modified, Absent, Absent, Absent], [true, true]);
    apply_op(&mut s, Op::Set(Loc::Staged, Partial));
    let (ops, _) = body(&p, &s, 3, true);
    for op in ops {
        apply_op(&mut s, op);
    }
    assert!(s.pending.is_empty(), "{:?}", s.pending);
}

/// The relaxed invariant (a user edit racing the retirement rename can land
/// in retention) is reachable, and never ends Complete.
#[test]
fn regression_edit_racing_retirement_is_retained_and_never_complete() {
    let r = explore(&cvr(), 1, true, false).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        r.modified_retained > 0,
        "the race must be explored, not assumed away"
    );
}

/// D1 is not weakened: a directory change is durable only after an fsync
/// of its own directory; a file fsync or another directory's fsync does
/// not make it durable.
#[test]
fn directory_changes_need_their_own_directory_fsync() {
    use Loc::*;
    use Obj::*;
    let mut s = blank([Orig, Copy, Absent, Absent], [true, true]);
    apply_op(&mut s, Op::Move(Staged, Dest));
    apply_op(&mut s, Op::Sync(STG)); // only the staging side
    assert_eq!(
        s.pending.len(),
        1,
        "one directory's fsync is not enough for a rename"
    );
    apply_op(&mut s, Op::Sync(SRC | RET));
    assert_eq!(
        s.pending.len(),
        1,
        "unrelated directories make nothing durable"
    );
    apply_op(&mut s, Op::Sync(DST | STG));
    assert!(s.pending.is_empty());
}
