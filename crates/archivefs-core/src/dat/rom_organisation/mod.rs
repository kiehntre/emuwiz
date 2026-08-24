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

pub mod model;
pub mod plan;
pub mod transaction;

#[cfg(test)]
mod linked_library_tests;
#[cfg(test)]
mod tests;

pub use model::{OrganisationMode, OrganisationPlan, OrganisationPlanEntry, OrganisationStatus};
pub use plan::{OrganisationCandidate, OrganisationPlanRequest, build_organisation_plan};
pub use transaction::{
    OrganisationRollbackOutcome, apply_organisation_transaction, build_organisation_transaction,
    revalidate_organisation_plan, rollback_organisation_transaction,
};
