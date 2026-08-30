//! Universal source discovery: understands a mixed collection (archives,
//! loose ROMs, disc images, Amiga images, WHDLoad folders, extracted game
//! folders) as different *containers* that may hold playable *content*,
//! rather than assuming everything is an archive.
//!
//! # Container vs content
//!
//! [`container::ContainerKind`] answers "how is this stored" (zip, tar,
//! folder, direct file). [`content_registry::ContentKind`] answers "what
//! does it represent" (a ROM cartridge, a disc image, an Amiga image, a
//! WHDLoad install, a game folder). Neither ever assigns a *platform* -
//! that stays [`crate::platform`]'s job, reused unmodified through
//! [`crate::ArchiveIdentity::from_path`], so a loose `.v64` and a `.zip`
//! containing the same `.v64` reach identity through the same path (see
//! [`discovery::discover_archive`]/[`discovery::discover_direct_file`]).
//!
//! The result of scanning one source folder is a [`discovery::SourceDiscoveryReport`]:
//! every item found, each with an always-populated, human-readable
//! explanation - accepted items say what they are, skipped items say why
//! and what to do about it (see [`discovery::SkipReason`]). Nothing here
//! is user-facing UI; it is designed so a future GUI layer can render it
//! directly without re-deriving explanations of its own.
//!
//! # Read-only
//!
//! Every function in this module only ever reads: `read_dir`,
//! `symlink_metadata`, and opening files for reading. No file is renamed,
//! moved, deleted, extracted, or otherwise written by anything reachable
//! from [`discovery::discover_source`].
//!
//! # Relationship to the existing archive scanner
//!
//! This module is additive: `crate::ArchiveScanner`/`crate::ArchiveKind`
//! are untouched, and existing scanning behaviour is unchanged by this
//! module's addition. [`discovery::discover_source`] is a new, independent
//! entry point future callers (CLI/GUI) can adopt; it does not replace
//! `ArchiveScanner::scan_source` in this change.

pub mod container;
pub mod content_registry;
pub mod cue_bin;
pub mod discovery;
pub mod gdi;

#[cfg(test)]
mod tests;

pub use container::{ArchiveFormat, ContainerKind, FolderRole};
pub use content_registry::ContentKind;
pub use discovery::{
    DiscoveredStructuralEvidence, DiscoveryError, DiscoveryStats, GameDiscovery, IdentitySummary,
    SkipReason, SkipReasonCounts, SourceDiscoveryReport, ValidationState, discover_source,
    is_known_non_game_extension,
};
