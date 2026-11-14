//! Deterministic fault injection for the transformation engine.
//!
//! A [`FaultPlan`] is passed explicitly to one apply, recovery or Undo call.
//! Production callers pass `None`; nothing reads an environment variable, so a
//! shipped binary cannot be told to misbehave.
//!
//! Faults are addressed by *points*: every journal record and every mutation
//! boundary inside a step has a stable [`Point`]. A test enumerates the points
//! one clean run passes through and then re-runs with a crash or an error at
//! each of them.
//!
//! A *crash* stops the engine immediately: no further journal record, no
//! cleanup, no destructor side effects beyond closing descriptors. In-process
//! it is an `io::Error` the engine propagates without handling; with
//! [`FaultPlan::abort_process`] it is `std::process::abort()`, used by tests
//! that run the engine in a child process and kill it for real.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Write};
use std::rc::Rc;

use super::model::Event;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Point {
    /// After the record with this sequence number is durable.
    AfterRecord(u64),
    /// Instead of writing this record: write only its first half (a torn
    /// write) and crash.
    TornRecord(u64),
    /// Inside step `step` (by plan index), at a named boundary.
    Step { step: usize, at: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Crash,
    /// Return this OS error (e.g. `libc::ENOSPC`, `libc::EACCES`, `libc::EIO`).
    Error(i32),
}

#[derive(Default)]
struct State {
    target: Option<(Point, Action)>,
    /// Every point passed, in order (for enumeration).
    trace: Vec<Point>,
    /// Arbitrary callbacks at a point (substitution/race tests).
    hooks: Vec<(Point, Box<dyn FnMut()>)>,
    fired: bool,
}

/// Shared, cheaply clonable fault configuration for one engine call.
#[derive(Clone, Default)]
pub struct FaultPlan {
    state: Rc<RefCell<State>>,
    pub abort_process: bool,
}

const CRASH_MARK: &str = "ouroboros-injected-crash";

pub fn is_crash(error: &io::Error) -> bool {
    error.to_string().contains(CRASH_MARK)
}

pub(crate) fn crash_error() -> io::Error {
    io::Error::other(CRASH_MARK)
}

impl FaultPlan {
    pub fn tracing() -> Self {
        Self::default()
    }

    pub fn at(point: Point, action: Action) -> Self {
        let plan = Self::default();
        plan.state.borrow_mut().target = Some((point, action));
        plan
    }

    pub fn with_hook(self, point: Point, hook: impl FnMut() + 'static) -> Self {
        self.state.borrow_mut().hooks.push((point, Box::new(hook)));
        self
    }

    pub fn trace(&self) -> Vec<Point> {
        self.state.borrow().trace.clone()
    }

    pub fn fired(&self) -> bool {
        self.state.borrow().fired
    }

    fn fire(&self, action: Action) -> io::Error {
        self.state.borrow_mut().fired = true;
        match action {
            Action::Crash => {
                if self.abort_process {
                    std::process::abort();
                }
                crash_error()
            }
            Action::Error(code) => io::Error::from_raw_os_error(code),
        }
    }

    /// Checks one point: runs its hooks, then fires an armed fault.
    pub(crate) fn check(&self, point: Point) -> io::Result<()> {
        let mut hooks = {
            let mut state = self.state.borrow_mut();
            state.trace.push(point.clone());
            let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut state.hooks)
                .into_iter()
                .partition(|(p, _)| *p == point);
            state.hooks = rest;
            mine
        };
        for (_, hook) in hooks.iter_mut() {
            hook();
        }
        let armed = {
            let state = self.state.borrow();
            match &state.target {
                Some((p, action)) if *p == point && !state.fired => Some(*action),
                _ => None,
            }
        };
        match armed {
            Some(action) => Err(self.fire(action)),
            None => Ok(()),
        }
    }

    pub(crate) fn before_journal_write(
        &self,
        seq: u64,
        file: &mut File,
        line: &[u8],
    ) -> io::Result<()> {
        let point = Point::TornRecord(seq);
        self.state.borrow_mut().trace.push(point.clone());
        let armed = {
            let state = self.state.borrow();
            matches!(&state.target, Some((p, _)) if *p == point) && !state.fired
        };
        if armed {
            file.write_all(&line[..line.len() / 2])?;
            file.sync_data()?;
            return Err(self.fire(Action::Crash));
        }
        Ok(())
    }

    pub(crate) fn after_journal_write(&self, seq: u64, _event: &Event) -> io::Result<()> {
        self.check(Point::AfterRecord(seq))
    }
}

/// Convenience used by the engine at every mutation boundary.
pub(crate) fn point(fault: &Option<FaultPlan>, step: usize, at: &'static str) -> io::Result<()> {
    match fault {
        Some(f) => f.check(Point::Step { step, at }),
        None => Ok(()),
    }
}
