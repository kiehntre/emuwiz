//! Native GUI-v2 browsing of the read-only RomM identity snapshot.

use crate::romm_source::RommImportSummary;
use archivefs_core::identity_source::{
    cache::IdentityCache,
    matching::LocalPresence,
    model::{ExternalIdentityRecord, ExternalVerification},
};
use std::path::Path;

pub(crate) const MAX_VISIBLE: usize = 200;

#[derive(Clone, Debug, Default)]
pub(crate) struct RommBrowserSnapshot {
    pub cache: Option<IdentityCache>,
    pub status: String,
}
impl RommBrowserSnapshot {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            cache: None,
            status: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PresenceFilter {
    #[default]
    Any,
    Present,
    Missing,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RommBrowserState {
    pub snapshot: Option<RommBrowserSnapshot>,
    pub search: String,
    pub platform: Option<String>,
    pub presence: PresenceFilter,
    pub selected: Option<String>,
    pub page: usize,
    pub loading: bool,
    pub operation: Option<(u64, RommLibraryOperation)>,
    pub operation_error: Option<String>,
    pub last_import: Option<RommImportSummary>,
    pub last_preview: Option<RommImportSummary>,
    pub last_delta: Option<RommCacheDelta>,
    pub refresh_baseline: Option<IdentityCache>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RommLibraryOperation {
    Refresh,
    PreviewImport,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RommCacheDelta {
    pub added_games: usize,
    pub removed_games: usize,
    pub unchanged_games: usize,
    pub added_platforms: usize,
    pub removed_platforms: usize,
}

impl RommCacheDelta {
    pub(crate) fn between(before: Option<&IdentityCache>, after: &IdentityCache) -> Self {
        let before_games: std::collections::BTreeSet<_> = before
            .into_iter()
            .flat_map(|cache| {
                cache
                    .records
                    .iter()
                    .map(|record| record.provider_game_id.as_str())
            })
            .collect();
        let after_games: std::collections::BTreeSet<_> = after
            .records
            .iter()
            .map(|record| record.provider_game_id.as_str())
            .collect();
        let before_platforms: std::collections::BTreeSet<_> = before
            .into_iter()
            .flat_map(|cache| {
                cache
                    .platforms
                    .iter()
                    .map(|platform| platform.provider_slug.as_str())
            })
            .collect();
        let after_platforms: std::collections::BTreeSet<_> = after
            .platforms
            .iter()
            .map(|platform| platform.provider_slug.as_str())
            .collect();
        Self {
            added_games: after_games.difference(&before_games).count(),
            removed_games: before_games.difference(&after_games).count(),
            unchanged_games: after_games.intersection(&before_games).count(),
            added_platforms: after_platforms.difference(&before_platforms).count(),
            removed_platforms: before_platforms.difference(&after_platforms).count(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlatformRow {
    pub id: Option<String>,
    pub slug: String,
    pub name: Option<String>,
    pub canonical: Option<String>,
    pub games: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GameRow {
    pub id: String,
    pub title: String,
    pub platform: String,
    pub platform_slug: String,
    pub local_path: Option<String>,
    pub presence: Option<LocalPresence>,
    pub verification: ExternalVerification,
    pub file_size: Option<u64>,
    pub files: usize,
    pub artwork: bool,
    pub provenance: String,
}

fn presence(path: Option<&Path>) -> Option<LocalPresence> {
    path.map(LocalPresence::observe)
}
fn missing(value: Option<LocalPresence>) -> bool {
    matches!(
        value,
        Some(LocalPresence::Absent | LocalPresence::ParentAbsent | LocalPresence::DanglingSymlink)
    )
}
fn title(record: &ExternalIdentityRecord) -> String {
    record.title.clone().unwrap_or_else(|| "(untitled)".into())
}

impl RommBrowserState {
    pub fn platforms(&self) -> Vec<PlatformRow> {
        let Some(cache) = self.snapshot.as_ref().and_then(|s| s.cache.as_ref()) else {
            return Vec::new();
        };
        let mut rows: Vec<_> = cache
            .platforms
            .iter()
            .map(|p| PlatformRow {
                id: p.provider_platform_id.clone(),
                slug: p.provider_slug.clone(),
                name: p.provider_name.clone(),
                canonical: p.canonical.clone(),
                games: p.rom_count,
            })
            .collect();
        rows.sort_by(|a, b| a.slug.cmp(&b.slug).then_with(|| a.id.cmp(&b.id)));
        rows
    }
    pub fn filtered_games(&self) -> Vec<GameRow> {
        let Some(cache) = self.snapshot.as_ref().and_then(|s| s.cache.as_ref()) else {
            return Vec::new();
        };
        let needle = self.search.trim().to_lowercase();
        let mut rows: Vec<_> = cache
            .records
            .iter()
            .filter_map(|r| {
                let local_presence = presence(r.archivefs_path.as_deref());
                let provider_platform = r.provider_platform_name.clone().unwrap_or_default();
                let platform_ok = self.platform.as_ref().is_none_or(|wanted| {
                    r.platform_candidate.as_ref() == Some(wanted) || &provider_platform == wanted
                });
                let search_ok = needle.is_empty()
                    || title(r).to_lowercase().contains(&needle)
                    || r.provider_path.to_lowercase().contains(&needle);
                let presence_ok = match self.presence {
                    PresenceFilter::Any => true,
                    PresenceFilter::Present => local_presence == Some(LocalPresence::File),
                    PresenceFilter::Missing => missing(local_presence),
                };
                (platform_ok && search_ok && presence_ok).then(|| GameRow {
                    id: r.provider_game_id.clone(),
                    title: title(r),
                    platform: r
                        .platform_candidate
                        .clone()
                        .or(r.provider_platform_name.clone())
                        .unwrap_or_else(|| "Unknown".into()),
                    platform_slug: provider_platform,
                    local_path: r.archivefs_path.as_ref().map(|p| p.display().to_string()),
                    presence: local_presence,
                    verification: r.verification,
                    file_size: r.file_size_bytes,
                    files: r.related_files.len().max(1),
                    artwork: r.artwork.is_some(),
                    provenance: r.server_id.clone(),
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        rows.truncate(MAX_VISIBLE);
        rows
    }
    pub fn selected_record(&self) -> Option<&ExternalIdentityRecord> {
        let id = self.selected.as_deref()?;
        self.snapshot
            .as_ref()?
            .cache
            .as_ref()?
            .records
            .iter()
            .find(|r| r.provider_game_id == id)
    }
}

pub(crate) fn load_snapshot() -> Result<RommBrowserSnapshot, String> {
    let root = archivefs_core::identity_source::settings::default_identity_root()
        .map_err(|e| e.to_string())?;
    let api = archivefs_core::identity_source::status::IdentitySourceApi::new(
        &root,
        archivefs_core::identity_source::model::IdentityProvider::Romm,
    );
    match api.open_cache(None) {
        Ok(cache) => Ok(RommBrowserSnapshot {
            status: format!("Cached from {}", cache.server_id),
            cache: Some(cache),
        }),
        Err(error) => Ok(RommBrowserSnapshot::unavailable(error.detail())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(ids: &[&str], platforms: &[&str]) -> IdentityCache {
        IdentityCache {
            format_version: archivefs_core::identity_source::cache::CACHE_FORMAT_VERSION,
            provider: archivefs_core::identity_source::model::IdentityProvider::Romm,
            server_id: "https://romm.example".into(),
            server_version: None,
            source_fingerprint: "fixture".into(),
            imported_at_unix_seconds: 1,
            platforms: platforms
                .iter()
                .map(
                    |slug| archivefs_core::identity_source::romm::normalise::NormalisedPlatform {
                        provider_platform_id: None,
                        provider_slug: (*slug).into(),
                        provider_name: Some((*slug).into()),
                        canonical: None,
                        rom_count: None,
                    },
                )
                .collect(),
            records: ids
                .iter()
                .map(
                    |id| archivefs_core::identity_source::model::ExternalIdentityRecord {
                        provider: archivefs_core::identity_source::model::IdentityProvider::Romm,
                        server_id: "https://romm.example".into(),
                        provider_platform_id: None,
                        provider_game_id: (*id).into(),
                        provider_file_id: None,
                        provider_path: format!("gb/{id}.gb"),
                        archivefs_path: None,
                        title: Some((*id).into()),
                        platform_candidate: None,
                        provider_platform_name: None,
                        regions: Vec::new(),
                        revision: None,
                        hashes: Vec::new(),
                        file_size_bytes: None,
                        metadata_provider_ids: Vec::new(),
                        artwork: None,
                        related_files: Vec::new(),
                        sibling_game_ids: Vec::new(),
                        imported_at_unix_seconds: 1,
                        provider_updated_at: None,
                        verification: ExternalVerification::Unmatched,
                        conflicts: Vec::new(),
                        evidence: Vec::new(),
                        synopsis: None,
                        genres: Vec::new(),
                        players: None,
                        rating: None,
                        release_year: None,
                        howlongtobeat: None,
                    },
                )
                .collect(),
            rejected_hashes: Vec::new(),
            unknown_platforms: Vec::new(),
            server_reported_total: None,
        }
    }

    #[test]
    fn empty_snapshot_is_safe() {
        let state = RommBrowserState {
            snapshot: Some(RommBrowserSnapshot::unavailable("offline")),
            ..Default::default()
        };
        assert!(state.platforms().is_empty());
        assert!(state.filtered_games().is_empty());
    }

    #[test]
    fn refresh_delta_reports_added_removed_and_unchanged_records() {
        let before = cache(&["same", "removed"], &["gb", "old"]);
        let after = cache(&["same", "added"], &["gb", "new"]);
        assert_eq!(
            RommCacheDelta::between(Some(&before), &after),
            RommCacheDelta {
                added_games: 1,
                removed_games: 1,
                unchanged_games: 1,
                added_platforms: 1,
                removed_platforms: 1,
            }
        );
    }

    #[test]
    fn first_refresh_is_all_new_and_never_requires_a_previous_cache() {
        let after = cache(&["one", "two"], &["gb"]);
        assert_eq!(
            RommCacheDelta::between(None, &after),
            RommCacheDelta {
                added_games: 2,
                removed_games: 0,
                unchanged_games: 0,
                added_platforms: 1,
                removed_platforms: 0,
            }
        );
    }
}
