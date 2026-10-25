//! Direct, local Redump DAT ingestion.
//!
//! This is a deliberately narrow adapter over the existing Logiqx parser,
//! collision-preserving [`crate::dat::index::DatIndex`], and separate CHD
//! disk index.  It performs no network work and never assigns identity from
//! a filename.  Track rows produce `DiscTrack` observations; DAT `<disk>`
//! SHA-1 rows use the distinct `LogicalChd` lane.

pub mod convert;
pub mod import;

pub use convert::{
    claim_for_representation, lookup_redump, lookup_redump_disk_sha1,
    observations_from_redump_disk_matches, observations_from_redump_matches,
};
pub use import::{ImportedRedumpSource, RedumpImportError, import_redump_dat};

#[cfg(test)]
mod tests;
