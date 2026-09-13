//! Universal Library Publisher Profiles - Phase 1 (read-only planning).
//!
//! # What this is
//!
//! A generic, frontend-agnostic model ([`model`]) for describing how one
//! "library publisher" target (RomM, ES-DE, and later Pegasus, LaunchBox,
//! ROMNight, Steam, ...) wants an already-elected
//! [`crate::playing_library::PlayingLibraryPlan`] (the existing 1G1R
//! planner's output) projected into its own destination tree.
//!
//! Two profiles ship in Phase 1: [`romm`] and [`es_de`]. Each is a small
//! module that only *describes* its frontend's conventions (path layout,
//! platform-mapping lookup) by delegating to this crate's own existing,
//! reviewed mapping tables
//! ([`crate::platform_evidence_fusion::romm_platform_mapping`],
//! [`crate::launch::es_de_export`]) - neither profile module invents a
//! folder name, slug, or system id. [`planner::build_publisher_plan`] is
//! the single, frontend-agnostic engine that turns any profile plus a
//! `PlayingLibraryPlan` into a [`model::PublisherPlan`].
//!
//! # Planning and execution boundary - read this before using execution
//!
//! Planning and transaction building never creates, hardlinks, symlinks,
//! copies, renames, or deletes files. Phase 2A/2B adds an explicit core apply
//! helper that delegates to the existing journaled transaction engine; no GUI
//! Apply path calls it yet. No `es_systems.xml` or other frontend
//! configuration file is ever written by Publisher Profiles. The only I/O
//! during planning/building is an optional, explicitly opted-in, read-only
//! inspection of an existing destination path (see
//! [`destination_inspection`]).
//!
//! # Relationship with the existing 1G1R / Playing Library planner
//!
//! This module never re-scans, re-hashes, or re-elects anything. It
//! consumes an already-built [`crate::playing_library::PlayingLibraryPlan`]
//! exactly the way [`crate::playing_library::romm_projection`] and
//! [`crate::playing_library::retrodeck_projection`] already do - this
//! module is a *generalization* of that same established pattern, not a
//! replacement for it. Those two existing, apply-capable projection
//! modules are untouched by Phase 1: this crate now has one *additional*,
//! more strongly-typed, explicitly non-executing planning surface,
//! reusable by any future frontend without hard-coding RomM (or ES-DE)
//! assumptions into a shared planner.
//!
//! # Source library vs published library
//!
//! Every [`model::PublisherPlanItem::source_path`] and companion source
//! path is copied verbatim from the untouched source archive the 1G1R
//! election already resolved. A [`model::PublisherPlan`] describes a
//! *destination projection only* - it never mutates, reinterprets, or
//! contaminates the source library's own truth.
//!
//! # Phase 2 execution boundary
//!
//! Phase 2A/2B consumes only [`model::PublisherActionSafety::SafeToAct`]
//! items, converts them back into
//! [`crate::playing_library::LinkedLibraryOperation`]s, and reuses the
//! existing reviewed transaction builder, journal, executor, rollback, and
//! reconciliation seams. The explicit publisher apply helper supports only
//! hardlinks or caller-selected symlinks plus owned destination directories.
//! Copy publishing, frontend metadata/configuration, confirmation UI, and
//! GUI Apply remain outside this phase.

pub mod destination_inspection;
pub mod es_de;
pub mod execution;
pub mod model;
pub mod planner;
pub mod romm;

pub use execution::{
    PublisherDestinationRootIdentity, PublisherDirectory, PublisherDirectoryState,
    PublisherExecutionError, PublisherLinkMode, PublisherTransaction, apply_publisher_transaction,
    build_publisher_transaction, build_publisher_transaction_with_policy,
    rollback_publisher_transaction,
};
pub use model::{
    BiosPublishPolicy, DestinationState, PathSegment, PublisherActionKind, PublisherActionSafety,
    PublisherBiosRequirement, PublisherCompanionItem, PublisherConflict, PublisherFrontend,
    PublisherMediaRule, PublisherMetadataRule, PublisherNamingRule, PublisherPathRule,
    PublisherPlan, PublisherPlanItem, PublisherPlanSummary, PublisherPlannedAction,
    PublisherPlatformMapping, PublisherProfile, PublisherWarning,
};
pub use planner::{PublisherPlanRequest, build_publisher_plan};

#[cfg(test)]
mod tests;
