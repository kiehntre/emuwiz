//! The ES-DE [`PublisherProfile`] - task section 4.
//!
//! Reuses the existing, reviewed
//! [`crate::launch::es_de_export::ES_DE_SYSTEM_MAP`] table
//! (`es_de_system_for_platform`) for every platform decision; this module
//! never invents a system folder name. Nothing here writes
//! `es_systems.xml` or any other ES-DE configuration file - see task
//! section 4's explicit "planning only" instruction.

use super::model::{
    PathSegment, PublisherFrontend, PublisherMediaRule, PublisherMetadataRule, PublisherNamingRule,
    PublisherPathRule, PublisherPlatformMapping, PublisherProfile,
};
use crate::launch::es_de_export::es_de_system_for_platform;

/// ES-DE's reviewed destination layout: `<destination_root>/<system>/`,
/// mirroring [`crate::playing_library::retrodeck_projection`]'s own
/// existing ES-DE-compatible tree, and matching ES-DE's own
/// `%ROMPATH%/<system>/` convention (the same folder also holds
/// `gamelists`/`downloaded_media`, which Phase 1 does not touch).
pub fn es_de_profile() -> PublisherProfile {
    PublisherProfile {
        frontend: PublisherFrontend::EsDe,
        path_rule: PublisherPathRule {
            segments: vec![PathSegment::PlatformFolder],
        },
        naming_rule: PublisherNamingRule::PreserveSourceFileName,
        media_rule: PublisherMediaRule::PublishElectedFilesUnchanged,
        metadata_rule: PublisherMetadataRule::None,
        // ES-DE's own launchable-file conventions are broad and per-system;
        // Phase 1 does not claim a reviewed exhaustive list.
        accepted_extensions: Vec::new(),
        unsupported_features: vec![
            "gamelist_xml_write",
            "es_systems_xml_write",
            "playlist_generation",
            "bios_separation",
            "metadata_write",
        ],
    }
}

/// Resolves one canonical platform id to its ES-DE system folder, using the
/// exact same reviewed table + `equivalent_platform_ids` fallback
/// `es_de_system_for_platform` already exposes for export/publish.
pub fn resolve_es_de_platform_mapping(canonical_platform_id: &str) -> PublisherPlatformMapping {
    match es_de_system_for_platform(canonical_platform_id) {
        Some(mapping) => PublisherPlatformMapping::Mapped {
            canonical_platform_id: canonical_platform_id.to_string(),
            folder: mapping.es_de_system.to_string(),
        },
        // ES-DE's own reviewed table draws no distinction between
        // "genuinely unmapped" and "ambiguous" - a platform is either in
        // the vetted table (via itself or a recognized equivalence) or it
        // is not. Phase 1 reports the honest default rather than guessing
        // a finer status this table does not itself carry.
        None => PublisherPlatformMapping::Unmapped {
            canonical_platform_id: canonical_platform_id.to_string(),
        },
    }
}
