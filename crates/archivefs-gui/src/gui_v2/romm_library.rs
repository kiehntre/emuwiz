//! Native GUI-v2 browsing of the read-only RomM identity snapshot.

use archivefs_core::identity_source::{
    cache::IdentityCache,
    matching::LocalPresence,
    model::{ExternalIdentityRecord, ExternalVerification},
};
use std::path::Path;

pub(crate) const MAX_VISIBLE: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RommBrowserState {
    pub snapshot: Option<RommBrowserSnapshot>,
    pub search: String,
    pub platform: Option<String>,
    pub presence: PresenceFilter,
    pub selected: Option<String>,
    pub page: usize,
    pub loading: bool,
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
    #[test]
    fn empty_snapshot_is_safe() {
        let state = RommBrowserState {
            snapshot: Some(RommBrowserSnapshot::unavailable("offline")),
            ..Default::default()
        };
        assert!(state.platforms().is_empty());
        assert!(state.filtered_games().is_empty());
    }
}
