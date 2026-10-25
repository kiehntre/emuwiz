//! Direct, local MAME software-list ingestion.
//!
//! This adapter deliberately reuses the streaming Logiqx parser and the
//! collision-preserving DAT indexes. Software-list names and software
//! shortnames are retained as MAME namespace metadata; neither is treated as
//! a canonical platform ID or inferred from a filename.

pub mod convert;
pub mod import;

pub use convert::{
    lookup_mame_software_list, lookup_mame_software_list_disk_sha1,
    observations_from_mame_software_list_disk_matches,
    observations_from_mame_software_list_matches,
};
pub use import::{
    ImportedMameSoftwareListSource, MameSoftwareListImportError, import_mame_software_list,
};

#[cfg(test)]
mod tests;
