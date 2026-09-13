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
//! # Phase 1 boundary - read this before adding an execution path
//!
//! **No file is created, hardlinked, symlinked, copied, renamed, or
//! deleted anywhere in this module or its submodules.** No `es_systems.xml`
//! or other frontend configuration file is ever written. The only I/O this
//! module performs is an *optional, explicitly opted-in, read-only*
//! inspection of an existing destination path (see
//! [`destination_inspection`]) - never a write. See `tests.rs`'s
//! `zero_side_effects` module for the structural proof.
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
//! # Phase 2 execution boundary (not implemented here)
//!
//! A future Phase 2 would consume a [`model::PublisherPlan`] whose items
//! are all [`model::PublisherActionSafety::SafeToAct`] and turn each
//! [`model::PublisherPlannedAction`] into a real filesystem operation,
//! most likely by converting accepted items back into
//! [`crate::playing_library::LinkedLibraryOperation`]s and reusing the
//! existing, reviewed
//! [`crate::playing_library::apply_adapter::build_playing_library_transaction`]
//! journal engine - exactly the same reuse-not-reinvent seam
//! `playing_library`'s own module doc comment already describes for its
//! own apply path. No such conversion function exists yet; Phase 1 stops
//! at [`model::PublisherPlan`].

pub mod destination_inspection;
pub mod es_de;
pub mod model;
pub mod planner;
pub mod romm;

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
