//! Direct local classic TOSEC DAT ingestion.
//!
//! This is a local-only adapter around the existing bounded DAT parser and
//! collision-preserving [`crate::dat::index::DatIndex`]. It accepts only DATs
//! whose internal parsed metadata identifies the TOSEC ecosystem; DAT paths
//! and member filenames are never identity authority. The supported v1 scope
//! is classic computer/media catalogues (floppy, tape, cartridge, and similar
//! ROM-style members). TOSEC-ISO/PIX and new optical-disc authority are
//! intentionally outside this adapter.

pub mod convert;
pub mod filename_metadata;
pub mod import;

pub use convert::{claim_for_representation, lookup_tosec, observations_from_tosec_matches};
pub use filename_metadata::{TosecDumpFlags, TosecFilenameMetadata, parse_tosec_filename_metadata};
pub use import::{ImportedTosecSource, TosecImportError, import_tosec_dat};

#[cfg(test)]
mod tests;
