//! Derived production-registry parity model.
//!
//! This module deliberately owns no extension list.  It projects the
//! platform, media, ingestion, Inspector, and loose-identity registries into
//! one machine-readable view so tests and audits can compare their different
//! contracts without making them identical.

use std::path::Path;

use crate::ArchiveKind;
use crate::game_identity::{IdentityPlatform, supported_loose_rom_format};
use crate::ingestion::content_registry::{ContentKind, content_kind_for_extension};
use crate::inspector::{InspectorEntryClassification, classify_entry};
use crate::media_registry::{MEDIA_FORMATS, MediaFormat, kind_for_extension};
use crate::platform::{PLATFORMS, platforms_with_strong_extension};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionDisposition {
    /// The extension has a persisted media row, ingestion content category,
    /// or bounded loose identity route.
    Registered,
    /// The platform advertises the extension, but no direct identity route is
    /// currently claimed.  This is an explicit audit finding, not proof that
    /// the format is supported structurally.
    DeferredOrUnsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionParity {
    pub extension: String,
    pub claiming_platforms: Vec<&'static str>,
    pub media_kind: Option<ArchiveKind>,
    pub content_kind: Option<ContentKind>,
    pub inspector_likely_content: bool,
    pub identity_dispatched: bool,
    pub disposition: ProductionDisposition,
}

const IDENTITY_PLATFORMS: &[IdentityPlatform] = &[
    IdentityPlatform::MegaDrive,
    IdentityPlatform::Snes,
    IdentityPlatform::Nes,
    IdentityPlatform::GameBoy,
    IdentityPlatform::GameBoyColor,
    IdentityPlatform::GameBoyAdvance,
    IdentityPlatform::N64,
    IdentityPlatform::Ngp,
    IdentityPlatform::Ngpc,
];

fn identity_dispatches(extension: &str) -> bool {
    let path = Path::new("fixture").with_extension(extension);
    IDENTITY_PLATFORMS
        .iter()
        .any(|platform| supported_loose_rom_format(&path, *platform).is_some())
}

fn row(extension: &str) -> ExtensionParity {
    let media_kind = kind_for_extension(extension);
    let content_kind = content_kind_for_extension(extension);
    let identity_dispatched = identity_dispatches(extension);
    let inspector_likely_content = classify_entry(&format!("fixture.{extension}"), false)
        == InspectorEntryClassification::LikelyContent;
    let disposition = if media_kind.is_some() || content_kind.is_some() || identity_dispatched {
        ProductionDisposition::Registered
    } else {
        ProductionDisposition::DeferredOrUnsupported
    };
    ExtensionParity {
        extension: extension.to_string(),
        claiming_platforms: platforms_with_strong_extension(extension)
            .into_iter()
            .map(|platform| platform.id)
            .collect(),
        media_kind,
        content_kind,
        inspector_likely_content,
        identity_dispatched,
        disposition,
    }
}

/// Every extension claimed by a platform as strong evidence, deduplicated in
/// registry order.
pub fn advertised_strong_extensions() -> Vec<ExtensionParity> {
    let mut extensions = Vec::new();
    for platform in PLATFORMS {
        for extension in platform.strong_extensions {
            if !extensions
                .iter()
                .any(|row: &ExtensionParity| row.extension == *extension)
            {
                extensions.push(row(extension));
            }
        }
    }
    extensions
}

/// Every media row, projected through the ingestion and Inspector contracts.
pub fn media_registry_parity() -> Vec<ExtensionParity> {
    MEDIA_FORMATS
        .iter()
        .map(|format: &MediaFormat| row(format.extension))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_advertised_strong_extension_has_an_explicit_disposition() {
        let rows = advertised_strong_extensions();
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| matches!(
            row.disposition,
            ProductionDisposition::Registered | ProductionDisposition::DeferredOrUnsupported
        )));
    }

    #[test]
    fn identity_dispatched_direct_formats_are_ingestion_reachable() {
        for row in advertised_strong_extensions()
            .into_iter()
            .filter(|row| row.identity_dispatched)
        {
            assert!(
                row.content_kind.is_some(),
                "identity-dispatched .{} has no ingestion content disposition",
                row.extension
            );
        }
    }

    #[test]
    fn media_rows_have_a_truthful_ingestion_or_specialist_disposition() {
        for row in media_registry_parity().into_iter().filter(|row| {
            row.media_kind != Some(ArchiveKind::Zip)
                && row.media_kind != Some(ArchiveKind::SevenZip)
                && row.media_kind != Some(ArchiveKind::Rar)
        }) {
            assert!(
                row.content_kind.is_some() || row.identity_dispatched,
                "media .{} has no content or identity disposition",
                row.extension
            );
        }
    }

    #[test]
    fn shared_extensions_never_become_single_platform_proof() {
        for extension in [
            "bin", "iso", "img", "rom", "dsk", "tap", "cas", "adf", "chd", "zip", "7z", "rar",
            "hdf",
        ] {
            assert!(
                platforms_with_strong_extension(extension).is_empty(),
                ".{extension} must not gain a single strong platform claim"
            );
        }
    }
}
