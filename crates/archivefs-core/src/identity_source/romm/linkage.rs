//! Read-only explanations for why a local path is, or is not, linked to RomM.
//!
//! RomM linkage is deliberately path-based in this stage. This module does not
//! invent a title matcher: it reports the exact evidence available from the
//! published cache, configured path mappings and local path presence.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::identity_source::cache::IdentityCache;
use crate::identity_source::model::ExternalIdentityRecord;
use crate::identity_source::path_map::PathMappings;

/// The linkage state a caller can explain without hashing or contacting RomM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RommLinkageStatus {
    Linked,
    NoImportCache,
    NoPathMapping,
    ProviderPathUnmapped,
    TranslatedPathElsewhere,
    TranslatedPathMissing,
    LocalPathMovedOrStale,
    UnknownPlatform,
    Ambiguous,
}

impl RommLinkageStatus {
    pub fn code(self) -> &'static str {
        match self {
            Self::Linked => "linked",
            Self::NoImportCache => "no_import_cache",
            Self::NoPathMapping => "no_path_mapping",
            Self::ProviderPathUnmapped => "provider_path_unmapped",
            Self::TranslatedPathElsewhere => "translated_path_elsewhere",
            Self::TranslatedPathMissing => "translated_path_missing",
            Self::LocalPathMovedOrStale => "local_path_moved_or_stale",
            Self::UnknownPlatform => "unknown_platform",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// One bounded, display-ready explanation of a local path's RomM linkage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RommLinkageDiagnostic {
    pub status: RommLinkageStatus,
    pub reason_code: &'static str,
    pub explanation: String,
    pub local_path: PathBuf,
    pub provider_game_id: Option<String>,
    pub provider_platform_slug: Option<String>,
    pub provider_path: Option<String>,
    /// The path produced by the configured mapping, when translation succeeded.
    pub translated_local_path: Option<PathBuf>,
    pub canonical_platform: Option<String>,
}

impl RommLinkageDiagnostic {
    fn simple(
        local_path: &Path,
        status: RommLinkageStatus,
        explanation: impl Into<String>,
    ) -> Self {
        Self {
            status,
            reason_code: status.code(),
            explanation: explanation.into(),
            local_path: local_path.to_path_buf(),
            provider_game_id: None,
            provider_platform_slug: None,
            provider_path: None,
            translated_local_path: None,
            canonical_platform: None,
        }
    }

    fn for_record(
        local_path: &Path,
        status: RommLinkageStatus,
        explanation: impl Into<String>,
        record: &ExternalIdentityRecord,
        translated_local_path: Option<PathBuf>,
    ) -> Self {
        Self {
            status,
            reason_code: status.code(),
            explanation: explanation.into(),
            local_path: local_path.to_path_buf(),
            provider_game_id: Some(record.provider_game_id.clone()),
            provider_platform_slug: record.provider_platform_name.clone(),
            provider_path: Some(record.provider_path.clone()),
            translated_local_path,
            canonical_platform: record.platform_candidate.clone(),
        }
    }
}

/// Explains the best available RomM linkage evidence for one local path.
///
/// This function reads only the supplied cache, performs pure path translation,
/// and observes whether paths exist. It never computes hashes, performs title
/// matching, contacts RomM, or changes either cache or filesystem.
pub fn diagnose_local_path(
    cache: Option<&IdentityCache>,
    mappings: &PathMappings,
    local_path: &Path,
) -> RommLinkageDiagnostic {
    let Some(cache) = cache else {
        return RommLinkageDiagnostic::simple(
            local_path,
            RommLinkageStatus::NoImportCache,
            "No published RomM identity cache is available; import RomM identities first.",
        );
    };
    if cache.records.is_empty() {
        return RommLinkageDiagnostic::simple(
            local_path,
            RommLinkageStatus::NoImportCache,
            "The published RomM identity cache contains no usable game records.",
        );
    }

    let exact: Vec<&ExternalIdentityRecord> = cache
        .records
        .iter()
        .filter(|record| record.archivefs_path.as_deref() == Some(local_path))
        .collect();
    if exact.len() > 1 {
        let record = exact[0];
        return RommLinkageDiagnostic::for_record(
            local_path,
            RommLinkageStatus::Ambiguous,
            format!(
                "{} imported RomM records claim this exact local path, so linkage is ambiguous.",
                exact.len()
            ),
            record,
            Some(local_path.to_path_buf()),
        );
    }
    if let Some(record) = exact.first().copied() {
        if record.platform_candidate.is_none() && record.provider_platform_name.is_some() {
            return RommLinkageDiagnostic::for_record(
                local_path,
                RommLinkageStatus::UnknownPlatform,
                "A RomM record is linked to this path, but its platform slug is not known to EmuWiz.",
                record,
                Some(local_path.to_path_buf()),
            );
        }
        if !local_path.exists() {
            return RommLinkageDiagnostic::for_record(
                local_path,
                RommLinkageStatus::LocalPathMovedOrStale,
                "The cached RomM linkage points at this local path, but the path is now missing or stale.",
                record,
                Some(local_path.to_path_buf()),
            );
        }
        return RommLinkageDiagnostic::for_record(
            local_path,
            RommLinkageStatus::Linked,
            "An imported RomM record claims this exact local path. Artwork availability is a separate diagnostic.",
            record,
            Some(local_path.to_path_buf()),
        );
    }

    if mappings.is_empty() {
        return RommLinkageDiagnostic::simple(
            local_path,
            RommLinkageStatus::NoPathMapping,
            "RomM records exist, but no provider-to-local path mapping is configured.",
        );
    }

    let mut unmapped: Option<(&ExternalIdentityRecord, String)> = None;
    let mut elsewhere: Option<(&ExternalIdentityRecord, PathBuf)> = None;
    let mut missing: Option<(&ExternalIdentityRecord, PathBuf)> = None;
    let mut unknown: Option<&ExternalIdentityRecord> = None;

    for record in &cache.records {
        if record.platform_candidate.is_none() && record.provider_platform_name.is_some() {
            unknown.get_or_insert(record);
        }
        let translation = mappings.translate(&record.provider_path);
        let Some(translated) = translation.archivefs_path() else {
            unmapped.get_or_insert((record, translation.provider_path().to_string()));
            continue;
        };
        if translated == local_path {
            // This should have been caught above from the cached field. Keep the
            // branch defensive if a caller supplies a cache assembled manually.
            continue;
        }
        let translated = translated.to_path_buf();
        if translated.exists() {
            elsewhere.get_or_insert((record, translated));
        } else {
            missing.get_or_insert((record, translated));
        }
    }

    if let Some(record) = unknown {
        return RommLinkageDiagnostic::for_record(
            local_path,
            RommLinkageStatus::UnknownPlatform,
            "RomM contains a platform slug that EmuWiz cannot normalize; no platform guess was made.",
            record,
            None,
        );
    }
    if let Some((record, provider_path)) = unmapped {
        return RommLinkageDiagnostic::for_record(
            local_path,
            RommLinkageStatus::ProviderPathUnmapped,
            format!("The RomM path `{provider_path}` is not covered by the configured mappings."),
            record,
            None,
        );
    }
    if let Some((record, translated)) = missing {
        return RommLinkageDiagnostic::for_record(
            local_path,
            RommLinkageStatus::TranslatedPathMissing,
            "A RomM record translates through the configured mapping, but its expected local path is missing; this is safe stale/move evidence, not a title match.",
            record,
            Some(translated),
        );
    }
    if let Some((record, translated)) = elsewhere {
        return RommLinkageDiagnostic::for_record(
            local_path,
            RommLinkageStatus::TranslatedPathElsewhere,
            "A RomM record translates successfully, but to a different local path.",
            record,
            Some(translated),
        );
    }

    RommLinkageDiagnostic::simple(
        local_path,
        RommLinkageStatus::ProviderPathUnmapped,
        "RomM records exist, but none could be translated to a local path by the configured mappings.",
    )
}

/// Maximum local paths considered by one summary request.
pub const MAX_SUMMARY_PATHS: usize = 10_000;

/// Bounded counts suitable for a future GUI diagnostic card.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RommLinkageSummary {
    pub inspected: usize,
    pub truncated: bool,
    pub linked: usize,
    pub no_import_cache: usize,
    pub no_path_mapping: usize,
    pub translated_elsewhere: usize,
    pub stale_or_missing: usize,
    pub unknown_platform: usize,
    pub unresolved_other: usize,
}

/// Diagnoses up to [`MAX_SUMMARY_PATHS`] local paths without hashing or network access.
pub fn summarize_local_paths(
    cache: Option<&IdentityCache>,
    mappings: &PathMappings,
    local_paths: &[PathBuf],
) -> RommLinkageSummary {
    let mut summary = RommLinkageSummary {
        truncated: local_paths.len() > MAX_SUMMARY_PATHS,
        ..Default::default()
    };
    for path in local_paths.iter().take(MAX_SUMMARY_PATHS) {
        summary.inspected += 1;
        match diagnose_local_path(cache, mappings, path).status {
            RommLinkageStatus::Linked => summary.linked += 1,
            RommLinkageStatus::NoImportCache => summary.no_import_cache += 1,
            RommLinkageStatus::NoPathMapping => summary.no_path_mapping += 1,
            RommLinkageStatus::TranslatedPathElsewhere => summary.translated_elsewhere += 1,
            RommLinkageStatus::TranslatedPathMissing | RommLinkageStatus::LocalPathMovedOrStale => {
                summary.stale_or_missing += 1
            }
            RommLinkageStatus::UnknownPlatform => summary.unknown_platform += 1,
            RommLinkageStatus::ProviderPathUnmapped | RommLinkageStatus::Ambiguous => {
                summary.unresolved_other += 1
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::path_map::{PathMapping, ProviderPathKind};
    use crate::identity_source::romm::normalise::{
        NormalisationReport, canonical_platform_for_romm_slug, normalise_rom,
    };
    use serde_json::json;

    fn mappings(items: &[(&str, &str)]) -> PathMappings {
        let items = items
            .iter()
            .map(|(provider, local)| PathMapping {
                provider_prefix: (*provider).to_string(),
                archivefs_prefix: PathBuf::from(local),
            })
            .collect::<Vec<_>>();
        PathMappings::validate(&items, &[], ProviderPathKind::ProviderRelative).unwrap()
    }

    fn record(id: u64, slug: &str, path: &str) -> ExternalIdentityRecord {
        let mut report = NormalisationReport::default();
        normalise_rom(
            &json!({
                "id": id,
                "platform_slug": slug,
                "fs_path": "roms/gb",
                "fs_name": path.rsplit('/').next().unwrap(),
                "name": "Game Title"
            }),
            "server",
            &mappings(&[("roms", "/tmp/romm-linkage-tests")]),
            1,
            &mut report,
        )
        .unwrap()
    }

    fn cache(records: Vec<ExternalIdentityRecord>) -> IdentityCache {
        IdentityCache {
            format_version: crate::identity_source::cache::CACHE_FORMAT_VERSION,
            provider: crate::identity_source::model::IdentityProvider::Romm,
            server_id: "server".to_string(),
            server_version: None,
            source_fingerprint: "test".to_string(),
            imported_at_unix_seconds: 1,
            platforms: Vec::new(),
            records,
            rejected_hashes: Vec::new(),
            unknown_platforms: Vec::new(),
            server_reported_total: None,
        }
    }

    #[test]
    fn exact_path_is_linked_even_when_artwork_is_absent() {
        let mut item = record(1, "gb", "Game.gb");
        let path = PathBuf::from("/tmp/romm-linkage-tests/Game.gb");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"fixture").unwrap();
        item.archivefs_path = Some(path.clone());
        let result = diagnose_local_path(
            Some(&cache(vec![item])),
            &mappings(&[("roms", "/tmp/romm-linkage-tests")]),
            &path,
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(result.status, RommLinkageStatus::Linked);
    }

    #[test]
    fn an_exact_cached_path_that_is_gone_is_stale() {
        let mut item = record(6, "gb", "Gone.gb");
        let path = PathBuf::from("/tmp/romm-linkage-tests/Gone.gb");
        item.archivefs_path = Some(path.clone());
        assert_eq!(
            diagnose_local_path(
                Some(&cache(vec![item])),
                &mappings(&[("roms", "/tmp/romm-linkage-tests")]),
                &path
            )
            .status,
            RommLinkageStatus::LocalPathMovedOrStale
        );
    }

    #[test]
    fn absent_cache_is_distinct_from_an_empty_import() {
        let mappings = mappings(&[("roms", "/tmp/romm-linkage-tests")]);
        assert_eq!(
            diagnose_local_path(None, &mappings, Path::new("/x")).status,
            RommLinkageStatus::NoImportCache
        );
        assert_eq!(
            diagnose_local_path(Some(&cache(Vec::new())), &mappings, Path::new("/x")).status,
            RommLinkageStatus::NoImportCache
        );
    }

    #[test]
    fn missing_mapping_and_translated_elsewhere_are_distinct() {
        let item = record(2, "gb", "Game.gb");
        assert_eq!(
            diagnose_local_path(
                Some(&cache(vec![item.clone()])),
                &PathMappings::validate(&[], &[], ProviderPathKind::ProviderRelative).unwrap(),
                Path::new("/x")
            )
            .status,
            RommLinkageStatus::NoPathMapping
        );
        let result = diagnose_local_path(
            Some(&cache(vec![item])),
            &mappings(&[("roms", "/tmp/romm-linkage-tests")]),
            Path::new("/other/Game.gb"),
        );
        assert_eq!(result.status, RommLinkageStatus::TranslatedPathMissing);
    }

    #[test]
    fn unknown_platform_is_reported_without_guessing() {
        let item = record(3, "atari-st-ste", "Game.st");
        let result = diagnose_local_path(
            Some(&cache(vec![item])),
            &mappings(&[("roms", "/tmp/romm-linkage-tests")]),
            Path::new("/other/Game.st"),
        );
        assert_eq!(result.status, RommLinkageStatus::UnknownPlatform);
        assert_eq!(canonical_platform_for_romm_slug("atari-st-ste"), None);
    }

    #[test]
    fn game_boy_and_atari_st_aliases_remain_known() {
        assert_eq!(canonical_platform_for_romm_slug("gb"), Some("Game Boy"));
        assert_eq!(
            canonical_platform_for_romm_slug("atari-st"),
            Some("AtariST")
        );
    }

    #[test]
    fn summary_is_bounded_and_read_only() {
        let item = record(4, "gb", "Game.gb");
        let cache = cache(vec![item]);
        let before = cache.clone();
        let paths = vec![PathBuf::from("/x"); MAX_SUMMARY_PATHS + 1];
        let summary = summarize_local_paths(
            Some(&cache),
            &mappings(&[("roms", "/tmp/romm-linkage-tests")]),
            &paths,
        );
        assert_eq!(summary.inspected, MAX_SUMMARY_PATHS);
        assert!(summary.truncated);
        assert_eq!(cache, before);
    }

    #[test]
    fn diagnostic_does_not_match_by_title() {
        let item = record(5, "gb", "Different.gb");
        let result = diagnose_local_path(
            Some(&cache(vec![item])),
            &mappings(&[("roms", "/tmp/romm-linkage-tests")]),
            Path::new("/tmp/romm-linkage-tests/Game Title.gb"),
        );
        assert_ne!(result.status, RommLinkageStatus::Linked);
    }
}
