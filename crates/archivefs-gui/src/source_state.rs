//! Configuration-authoritative source presentation.

use std::path::PathBuf;

use archivefs_core::{SourceAvailability, SourceFolderView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceStateView {
    pub sources: Vec<SourceFolderView>,
    pub catalogue_available: bool,
}

/// Merges configuration-owned source paths with optional catalogue data.
/// Missing catalogue data never fabricates archive counts or removes sources.
pub(crate) fn merge_configured_sources(
    configured: &[PathBuf],
    catalogue_sources: Option<&[SourceFolderView]>,
) -> SourceStateView {
    if let Some(catalogue_sources) = catalogue_sources {
        return SourceStateView {
            sources: catalogue_sources.to_vec(),
            catalogue_available: true,
        };
    }

    SourceStateView {
        sources: configured
            .iter()
            .cloned()
            .map(|path| SourceFolderView {
                path,
                enabled: true,
                created_at: None,
                id: None,
                availability: SourceAvailability::Available,
                last_scan_status: None,
                last_scan_error: None,
                last_scan_at: None,
                last_successful_scan_at: None,
                last_archive_count: None,
                assigned_platform: None,
                unknown_archive_count: 0,
            })
            .collect(),
        catalogue_available: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_source_survives_missing_catalogue() {
        let view = merge_configured_sources(&[PathBuf::from("/library")], None);

        assert_eq!(view.sources.len(), 1);
        assert_eq!(view.sources[0].path, PathBuf::from("/library"));
        assert!(!view.catalogue_available);
        assert_eq!(view.sources[0].last_archive_count, None);
    }

    #[test]
    fn no_configured_sources_remains_empty_without_catalogue() {
        let view = merge_configured_sources(&[], None);

        assert!(view.sources.is_empty());
        assert!(!view.catalogue_available);
    }

    #[test]
    fn catalogue_data_enriches_configured_sources() {
        let catalogue = SourceFolderView {
            path: PathBuf::from("/library"),
            enabled: true,
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            id: Some(7),
            availability: SourceAvailability::Available,
            last_scan_status: Some(archivefs_core::SourceScanStatus::Success),
            last_scan_error: None,
            last_scan_at: Some("2026-01-02T00:00:00Z".to_string()),
            last_successful_scan_at: Some("2026-01-02T00:00:00Z".to_string()),
            last_archive_count: Some(12),
            assigned_platform: None,
            unknown_archive_count: 0,
        };
        let view = merge_configured_sources(&[PathBuf::from("/library")], Some(&[catalogue]));

        assert!(view.catalogue_available);
        assert_eq!(view.sources[0].id, Some(7));
        assert_eq!(view.sources[0].last_archive_count, Some(12));
    }
}
