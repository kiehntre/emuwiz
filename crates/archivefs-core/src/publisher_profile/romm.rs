//! The RomM [`PublisherProfile`] - task section 3.
//!
//! Reuses the existing, reviewed
//! [`crate::platform_evidence_fusion::romm_platform_mapping`] table
//! (`production_romm_slug`/`production_romm_status`) for every platform
//! decision; this module never invents a slug.

use super::model::{
    BiosPublishPolicy, PathSegment, PublisherFrontend, PublisherMediaRule, PublisherMetadataRule,
    PublisherNamingRule, PublisherPathRule, PublisherPlatformMapping, PublisherProfile,
};
use crate::identity_source::cache::IdentityCache;
use crate::platform_evidence_fusion::romm_platform_mapping::{
    FrontendPlatformMapping, RommMappingSupportStatus, production_romm_slug, production_romm_status,
};

/// RomM's reviewed destination layout: `<destination_root>/roms/<slug>/`.
/// Matches [`crate::playing_library::romm_projection::build_romm_projection`]'s
/// own existing `romm_root = destination_root.join("roms").join(slug)`.
pub fn romm_profile() -> PublisherProfile {
    PublisherProfile {
        frontend: PublisherFrontend::RomM,
        path_rule: PublisherPathRule {
            segments: vec![PathSegment::Literal("roms"), PathSegment::PlatformFolder],
        },
        naming_rule: PublisherNamingRule::PreserveSourceFileName,
        media_rule: PublisherMediaRule::PublishElectedFilesUnchanged,
        metadata_rule: PublisherMetadataRule::None,
        // RomM's own scanner is extension-driven and broad; Phase 1 does not
        // claim a reviewed exhaustive list, so this stays empty ("not
        // reviewed yet") rather than a guessed list - accepted_extensions
        // gating is skipped for RomM until a real reviewed list exists.
        accepted_extensions: Vec::new(),
        unsupported_features: vec![
            "playlist_generation",
            "bios_separation",
            "metadata_write",
            "config_write",
        ],
    }
}

/// Resolves one canonical platform id to its RomM folder, using the exact
/// same tiered resolution `build_romm_projection` already trusts (explicit
/// override, then live-cache tier, then the vetted static table).
pub fn resolve_romm_platform_mapping(
    canonical_platform_id: &str,
    overrides: &FrontendPlatformMapping,
    identity_cache: Option<&IdentityCache>,
) -> PublisherPlatformMapping {
    match production_romm_status(canonical_platform_id, overrides, identity_cache) {
        RommMappingSupportStatus::Mapped => {
            let slug = production_romm_slug(canonical_platform_id, overrides, identity_cache)
                .expect("Mapped status guarantees a slug is resolvable");
            PublisherPlatformMapping::Mapped {
                canonical_platform_id: canonical_platform_id.to_string(),
                folder: slug,
            }
        }
        RommMappingSupportStatus::Unmapped => PublisherPlatformMapping::Unmapped {
            canonical_platform_id: canonical_platform_id.to_string(),
        },
        RommMappingSupportStatus::Ambiguous => PublisherPlatformMapping::Ambiguous {
            canonical_platform_id: canonical_platform_id.to_string(),
        },
        RommMappingSupportStatus::Unsupported => PublisherPlatformMapping::Unsupported {
            canonical_platform_id: canonical_platform_id.to_string(),
        },
    }
}

/// RomM's Phase 1 BIOS policy: not reviewed. RomM's own BIOS/firmware
/// handling (a dedicated `bios` folder per platform, per its docs) has not
/// been independently verified against this codebase's BIOS model, so
/// every BIOS requirement surfaces as [`BiosPublishPolicy::NotReviewed`]
/// rather than a guessed destination - task section 12's "unknown policy".
pub fn romm_bios_policy() -> BiosPublishPolicy {
    BiosPublishPolicy::NotReviewed
}
