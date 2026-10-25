//! Direct, local FinalBurn Neo / FBNeo DAT ingestion.
//!
//! FBNeo DATs use the ordinary Logiqx `<datafile>` shape, so this adapter
//! reuses the shared parser and collision-preserving indexes. Its only local
//! authority is an explicit FBNeo brand in the DAT's own header metadata;
//! filenames and arcade shortnames are never used to classify the source or
//! assign a canonical platform.

pub mod convert;
pub mod import;

pub use convert::{
    lookup_fbneo, lookup_fbneo_disk_sha1, observations_from_fbneo_disk_matches,
    observations_from_fbneo_matches,
};
pub use import::{FBNeoImportError, ImportedFBNeoSource, import_fbneo_dat};

#[cfg(test)]
mod tests;
