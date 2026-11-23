//! Project Ouroboros: transactional library transformation and recovery.
//!
//! A transformation is an immutable, digest-approved [`Plan`] compiled to
//! idempotent [`Step`]s and executed through a durable, hash-chained journal
//! ([`journal`]). The engine never pretends that the filesystem, SQLite and
//! external applications share one atomic transaction: each resource change
//! is a separate durable step, decided by probing, with explicit
//! compensation before a point of no return and explicit Blocked states
//! after it.
//!
//! Supported transformation in this build: relocation of one loose game file
//! between two enabled `games` library roots, same filesystem (atomic
//! rename) or different filesystems (copy, verify, publish, commit, retain
//! original). Planning and preview work on any library; apply, recovery and
//! Undo run only under a crate-private, `cfg(test)`-only scratch authority
//! (see [`authority`]) on scratch roots it created; no build of the CLI or
//! Blue, with any profile or feature set, can execute at all.
//! Nothing here deletes an original: retired originals are retained under
//! [`RetentionPolicy::RetainUntilReleased`] and no release operation exists.
//!
//! Invariants enforced (see `docs/design/ouroboros-transactions.md`):
//! I1 original retired only after a verified, durable successor; I2 Done
//! records carry evidence; I3 recovery is idempotent; I4 stale plans refused;
//! I5 dependency changes refuse; I6 user modifications never overwritten;
//! I7 Complete only after every step and final verification; I8 Undo only
//! while its preconditions hold; I9 ambiguous ownership never authorises
//! removal; I10 uncertainty is reported as Blocked or Corrupt, never success.

#![cfg(target_os = "linux")]

pub mod authority;
pub mod engine;
pub mod fault;
pub mod fscompat;
mod fsops;
pub mod graph;
pub mod journal;
pub mod lock;
pub mod model;
pub mod plan;
mod shape;
pub mod undo;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod model_check;
#[cfg(test)]
mod tests;

pub use authority::PRODUCTION_UNAVAILABLE;
pub use engine::{
    Context, EngineError, TxnState, apply, list, plan_digest, recover, recover_all, status,
};
pub use model::{Plan, PlanKind, RetentionPolicy, Step, Strategy};
#[cfg(test)]
pub(crate) use plan::plan_relocation_scratch;
pub use plan::{Preview, plan_relocation};
pub use undo::{apply_undo, plan_undo, undo_blocker, undone_by};

/// Journal directory name under the EmuWiz data directory.
pub const TRANSFORM_JOURNAL_DIRECTORY: &str = "transform-journals-v1";

/// `<data dir>/transform-journals-v1`.
pub fn default_journal_root() -> crate::Result<std::path::PathBuf> {
    Ok(crate::app_dirs::data_dir()?.join(TRANSFORM_JOURNAL_DIRECTORY))
}
