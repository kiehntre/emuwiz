//! The game recovery engine.
//!
//! When a game cannot start, this module works out why from the file itself
//! and the emulators installed on this machine, says what is fixable and what
//! is not, and, for transformations that have been validated end to end,
//! prepares a verified playable copy without changing the original.
//!
//! * [`model`]: the typed findings, evidence, confidence and proposed actions.
//! * [`capability`]: the reviewed matrix of formats, routes and limits.
//! * [`diagnose`]: the read-only diagnosis rules.
//! * [`disc_zip`] and [`cue_sheet`]: bounded inspection of a disc set in a ZIP.
//! * [`prepared_media`]: the create-new, verified, reversible store of
//!   prepared copies.
//!
//! Adding a format means adding a capability row, an inspector that produces
//! evidence, and (only once validated) a preparation in the store. The model,
//! the store's guarantees, the CLI and Blue's page do not change.
//!
//! See `docs/GAME_RECOVERY_ENGINE.md`.

pub mod bulk;
pub mod capability;
pub mod cue_sheet;
pub mod diagnose;
pub mod disc_zip;
pub mod disk_identity;
pub mod model;
pub mod moves;
pub mod prepared_media;
pub mod storage;

#[cfg(test)]
mod tests;

pub use bulk::{BulkCandidate, BulkItemPlan, BulkPlan, Queue, QueueState};
pub use capability::{
    CAPABILITY_MATRIX, CapabilityRow, CapabilityStatus, capability_matrix_markdown, capability_rows,
};
pub use diagnose::{
    HostFacts, RecoveryContext, RetroArchHost, diagnose, may_apply, preparation_may_apply,
};
pub use disc_zip::{DiscSetInspection, DiscSetLimits, DiscSetRefusal, inspect_disc_zip};
pub use model::{
    ActionEffect, ActionKind, ActionPerformer, Blocking, Confidence, Evidence, EvidenceSource,
    Finding, FindingCode, ProposedAction, RecoveryReport, RecoverySubject, RecoveryVerdict,
};
pub use prepared_media::{
    LaunchPathCheck, PreparationPlan, PrepareError, PrepareOutcome, PrepareProgress,
    PreparedMediaStore, PreparedSetSummary, RelinkOutcome, RemovalReport, RemoveError,
};
