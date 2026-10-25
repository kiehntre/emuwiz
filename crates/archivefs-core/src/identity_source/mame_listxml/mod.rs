//! Direct, local MAME arcade machine (`mame -listxml`) evidence.
//!
//! This is independent of [`super::mame_software_list`] (a different DAT
//! ecosystem, [`crate::dat::model::DatEcosystem::MAMESoftwareList`], and a
//! different XML vocabulary): a MAME software-list DAT names emulated
//! *software media* for a fixed platform, while a `-listxml` dump names
//! MAME's own *arcade machines/devices* with no canonical platform decided
//! here. It is also independent of
//! [`crate::platform_evidence_fusion::mame_redump_bridge`], which produces
//! [`crate::platform_evidence_fusion::evidence_lineage::SourceFamily::MAMERedump`]
//! evidence *derived from* Redump - this module never asserts that lineage
//! itself; its own ROM/disk hash matches stay
//! [`crate::platform_evidence_fusion::evidence_lineage::SourceFamily::MAMEArcade`],
//! an independent lane.
pub mod convert;
pub mod import;

pub use convert::{
    observations_from_mame_listxml_disk_matches, observations_from_mame_listxml_matches,
};
pub use import::{ImportedMameListxmlSource, MameListxmlImportError, import_mame_listxml};

#[cfg(test)]
mod tests;
