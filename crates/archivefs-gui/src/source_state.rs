//! Configuration-authoritative source presentation.

use std::path::PathBuf;

use archivefs_core::{PersistedArchive, SourceAvailability, SourceFolderView};

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
                role: Default::default(),
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
            role: Default::default(),
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

/// A source's actual platform state, derived purely from the archives the
/// snapshot already has catalogued for it (`PersistedArchive::platform`,
/// matched by `PersistedArchive::source_folder_id == SourceFolderView::id`),
/// never a new query, rescan, persisted field, or schema change. This is
/// ground truth from the same data the Library page already shows, not a
/// guess: if every catalogued archive under a source agrees on one
/// platform, that is the source's platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourcePlatformState {
    /// This source has no catalogued archives yet (never scanned, or
    /// scanned and found nothing).
    NotYetKnown,
    /// Archives exist, but none resolved a platform.
    Unknown,
    /// Every catalogued archive agrees on this one platform.
    Single(String),
    /// Every catalogued archive resolved a platform, but not to the same
    /// one - `usize` is the number of distinct platforms found.
    Mixed(usize),
    /// Some archives resolved a platform and some did not.
    Partial { known: i64, unknown: i64 },
}

/// Computes [`SourcePlatformState`] for one source from the snapshot's full
/// archive list - an `O(archives)` scan per source, over data already
/// loaded in memory (never a filesystem rescan; see the type's own doc
/// comment for why no new persistence is needed).
pub(crate) fn source_platform_state(
    view: &SourceFolderView,
    archives: &[PersistedArchive],
) -> SourcePlatformState {
    let Some(source_id) = view.id else {
        return SourcePlatformState::NotYetKnown;
    };
    let mut resolved: Vec<&str> = Vec::new();
    let mut unresolved: i64 = 0;
    for archive in archives {
        if archive.source_folder_id != source_id {
            continue;
        }
        match archive.platform.as_deref() {
            Some(platform) => resolved.push(platform),
            None => unresolved += 1,
        }
    }
    if resolved.is_empty() && unresolved == 0 {
        return SourcePlatformState::NotYetKnown;
    }
    if resolved.is_empty() {
        return SourcePlatformState::Unknown;
    }
    if unresolved > 0 {
        return SourcePlatformState::Partial {
            known: resolved.len() as i64,
            unknown: unresolved,
        };
    }
    let mut distinct = resolved;
    distinct.sort_unstable();
    distinct.dedup();
    match distinct.as_slice() {
        [single] => SourcePlatformState::Single((*single).to_string()),
        many => SourcePlatformState::Mixed(many.len()),
    }
}

/// Simple, human-facing wording for [`SourcePlatformState`] - deliberately
/// avoids "unclassified", "heuristic", and "detected automatically"; a real
/// platform name is shown whenever the catalogued archives actually agree
/// on one. Deliberately just the *value*, with no "Platform:" prefix of
/// its own - the caller (the source card's facts grid) already supplies
/// that as the row's own label column, so prefixing it here as well would
/// render as the literal duplicate "Platform: Platform: X".
pub(crate) fn source_platform_value_label(state: &SourcePlatformState) -> String {
    match state {
        SourcePlatformState::NotYetKnown => "not yet known".to_string(),
        SourcePlatformState::Unknown => "Unknown".to_string(),
        SourcePlatformState::Single(platform) => platform.clone(),
        SourcePlatformState::Mixed(count) => format!("Mixed ({count} platforms)"),
        SourcePlatformState::Partial { known, unknown } => {
            format!("Partial ({known} known, {unknown} unknown)")
        }
    }
}
