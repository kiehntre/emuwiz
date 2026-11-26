//! Canonical ROM organisation into a user-configured master ROM root.
//!
//! The user configures a master ROM root (for example `/mnt/games/roms`).
//! EmuWiz can then *plan*, and after explicit approval *apply*, moving
//! identified games into neutral canonical EmuWiz platform directories
//! beneath that root.
//!
//! # Safety
//!
//! - Planning is read-only; nothing moves until the user approves an apply.
//! - Generic destination platform directories come from the canonical
//!   EmuWiz platform registry; RomM mappings are used only by explicit
//!   RomM-specific workflows.
//! - Mutations reuse the `rename_apply` engine: durable journal before any
//!   mutation, per-entry `Applying` checkpoint, `renameat2(RENAME_NOREPLACE)`
//!   no-clobber moves, identity re-check immediately before each move, shared
//!   crash reconciliation and rollback, honest cancellation.
//! - Only same-filesystem moves are supported. A cross-filesystem destination
//!   is refused with "Unsupported" - there is no copy+delete fallback.
//! - Symlink-only mode moves the link *object* and never dereferences or
//!   touches its target.
//! - Linked-library mode creates links beneath an approved library root and
//!   never renames, moves, deletes or rewrites any original source file.
//! - Only canonical platform directories created by the apply are ever
//!   removed on rollback, and only while empty; a pre-existing user directory
//!   is never removed.
//!
//! # Modes
//!
//! Four explicit, never-combined modes: rename in place, move the real file,
//! organise an existing symlink object only, or build a linked library from
//! untouched regular sources.

pub mod disc_sets;
pub mod game_rename;
pub mod link_target;
pub mod model;
pub mod plan;
pub mod protected_catalogue;
pub mod rename_guard;
mod split_disc_lock;
pub mod tombstones;
pub mod transaction;

#[cfg(test)]
mod disc_set_tests;
#[cfg(test)]
mod evidence_first_tests;
#[cfg(test)]
mod game_rename_tests;
#[cfg(test)]
mod linked_library_tests;
#[cfg(test)]
mod rename_guard_tests;
#[cfg(test)]
mod tests;

pub use disc_sets::{disc_set_of, hold_disc_sets_together, refuse_partial_disc_sets};
pub use model::{
    DeferredCapture, OrganisationMode, OrganisationPlan, OrganisationPlanEntry, OrganisationStatus,
};
pub use plan::{
    NEEDS_VERIFICATION_REASON, OrganisationCandidate, OrganisationPlanRequest,
    build_organisation_plan, build_organisation_plan_reporting, build_rename_plan_from_evidence,
    layout_folder_in,
};
pub use transaction::{
    BUILD_CANCELLED, OrganisationRollbackOutcome, PlatformCorrection,
    apply_organisation_transaction, bind_organisation_transaction, build_organisation_transaction,
    build_organisation_transaction_cancellable, is_organisation_transaction,
    open_history_transaction_for_undo, prepare_organisation_catalogue,
    prepare_organisation_catalogue_with_platforms, revalidate_organisation_plan,
    rollback_organisation_transaction, undo_blocker,
};
