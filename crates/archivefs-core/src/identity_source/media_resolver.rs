//! Deterministic, read-only media resolution over provider snapshots.
//!
//! This module deliberately does not fetch, parse XML, query Redis, or walk a
//! media tree. Providers build [`ProviderMediaSnapshot`] values during their
//! refresh/import phase; selected-item lookups are then pure in-memory work.
//! Identity and DAT/hash verification remain entirely outside this resolver.

use super::model::{ArtworkReference, MediaReference};

/// Providers already represented by EmuWiz's media model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MediaProvider {
    LocalCache,
    RommDirect,
    LaunchBoxViaRomm,
    EsDe,
    LaunchBoxLocal,
}

/// Delivery state exposed to a future UI without conflating "not loaded yet"
/// with "no media exists".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDelivery {
    LocalReady,
    RemotePending,
    RemoteReady,
    Unavailable,
}

/// One provider's already-indexed media for a selected game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMediaSnapshot {
    pub provider: MediaProvider,
    pub cover: Option<MediaReference>,
    pub screenshots: Vec<MediaReference>,
    pub video: Option<MediaReference>,
    pub delivery: MediaDelivery,
}

impl ProviderMediaSnapshot {
    pub fn new(provider: MediaProvider) -> Self {
        Self {
            provider,
            cover: None,
            screenshots: Vec::new(),
            video: None,
            delivery: MediaDelivery::Unavailable,
        }
    }

    /// Adapt an already-imported artwork projection. This is deliberately a
    /// pure adapter: it does not infer provider identity or perform I/O.
    pub fn from_artwork(
        provider: MediaProvider,
        artwork: &ArtworkReference,
        delivery: MediaDelivery,
    ) -> Self {
        Self {
            provider,
            cover: Some(MediaReference {
                hosted_reference: artwork
                    .small_reference
                    .clone()
                    .or_else(|| Some(artwork.reference.clone())),
                public_reference: Some(artwork.reference.clone()),
            }),
            screenshots: artwork.screenshots.clone(),
            video: None,
            delivery,
        }
    }
}

/// Input captured from current provider snapshots. Generation values bind an
/// async result to the selected game and provider snapshot that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaResolverInput {
    pub providers: Vec<ProviderMediaSnapshot>,
    pub pending_providers: Vec<MediaProvider>,
    pub failed_providers: Vec<MediaProvider>,
    pub selection_generation: u64,
    pub provider_generation: u64,
}

/// A resolved reference with explicit provenance and delivery state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMediaItem {
    pub reference: MediaReference,
    pub provider: MediaProvider,
    pub delivery: MediaDelivery,
}

/// The single result consumed by future GUI/application projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMediaSet {
    pub cover: Option<ResolvedMediaItem>,
    pub cover_delivery: MediaDelivery,
    pub screenshots: Vec<ResolvedMediaItem>,
    pub screenshots_delivery: MediaDelivery,
    pub video: Option<ResolvedMediaItem>,
    pub video_delivery: MediaDelivery,
    pub pending_providers: Vec<MediaProvider>,
    pub failed_providers: Vec<MediaProvider>,
    pub selection_generation: u64,
    pub provider_generation: u64,
}

impl ResolvedMediaSet {
    /// Whether this result may still be applied to the current selection.
    pub fn is_current(&self, selection_generation: u64, provider_generation: u64) -> bool {
        self.selection_generation == selection_generation
            && self.provider_generation == provider_generation
    }
}

const PROVIDER_PRIORITY: [MediaProvider; 5] = [
    MediaProvider::LocalCache,
    MediaProvider::RommDirect,
    MediaProvider::LaunchBoxViaRomm,
    MediaProvider::EsDe,
    MediaProvider::LaunchBoxLocal,
];

/// Bounded gallery size shared by all providers.
pub const MAX_RESOLVED_SCREENSHOTS: usize = 32;

/// Resolve one selected game's media without any I/O.
pub fn resolve_media(input: &MediaResolverInput) -> ResolvedMediaSet {
    let ordered = ordered_providers(&input.providers);
    let cover = ordered.iter().find_map(|snapshot| {
        snapshot.cover.as_ref().map(|reference| ResolvedMediaItem {
            reference: reference.clone(),
            provider: snapshot.provider,
            delivery: snapshot.delivery,
        })
    });
    let screenshots = dedupe_screenshots(&ordered);
    let video = ordered.iter().find_map(|snapshot| {
        snapshot.video.as_ref().map(|reference| ResolvedMediaItem {
            reference: reference.clone(),
            provider: snapshot.provider,
            delivery: snapshot.delivery,
        })
    });

    ResolvedMediaSet {
        cover_delivery: delivery_for(cover.as_ref(), &input.pending_providers),
        screenshots_delivery: delivery_for(screenshots.first(), &input.pending_providers),
        video_delivery: delivery_for(video.as_ref(), &input.pending_providers),
        cover,
        screenshots,
        video,
        pending_providers: sorted_unique(input.pending_providers.clone()),
        failed_providers: sorted_unique(input.failed_providers.clone()),
        selection_generation: input.selection_generation,
        provider_generation: input.provider_generation,
    }
}

fn ordered_providers<'a>(providers: &'a [ProviderMediaSnapshot]) -> Vec<&'a ProviderMediaSnapshot> {
    PROVIDER_PRIORITY
        .iter()
        .filter_map(|provider| {
            providers
                .iter()
                .find(|snapshot| snapshot.provider == *provider)
        })
        .collect()
}

fn dedupe_screenshots(providers: &[&ProviderMediaSnapshot]) -> Vec<ResolvedMediaItem> {
    let mut result = Vec::new();
    let mut keys = Vec::new();
    for snapshot in providers {
        for reference in &snapshot.screenshots {
            let key = reference_key(reference);
            if key.is_empty() || keys.iter().any(|existing| existing == &key) {
                continue;
            }
            keys.push(key);
            result.push(ResolvedMediaItem {
                reference: reference.clone(),
                provider: snapshot.provider,
                delivery: snapshot.delivery,
            });
            if result.len() == MAX_RESOLVED_SCREENSHOTS {
                return result;
            }
        }
    }
    result
}

fn reference_key(reference: &MediaReference) -> String {
    reference
        .hosted_reference
        .as_deref()
        .or(reference.public_reference.as_deref())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn delivery_for(item: Option<&ResolvedMediaItem>, pending: &[MediaProvider]) -> MediaDelivery {
    match item {
        Some(item) if item.delivery == MediaDelivery::LocalReady => MediaDelivery::LocalReady,
        Some(_) => MediaDelivery::RemoteReady,
        None if !pending.is_empty() => MediaDelivery::RemotePending,
        None => MediaDelivery::Unavailable,
    }
}

fn sorted_unique(mut providers: Vec<MediaProvider>) -> Vec<MediaProvider> {
    providers.sort();
    providers.dedup();
    providers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(value: &str) -> MediaReference {
        MediaReference {
            hosted_reference: Some(value.to_string()),
            public_reference: None,
        }
    }

    fn provider(provider: MediaProvider, delivery: MediaDelivery) -> ProviderMediaSnapshot {
        ProviderMediaSnapshot {
            provider,
            cover: None,
            screenshots: Vec::new(),
            video: None,
            delivery,
        }
    }

    fn input(providers: Vec<ProviderMediaSnapshot>) -> MediaResolverInput {
        MediaResolverInput {
            providers,
            pending_providers: Vec::new(),
            failed_providers: Vec::new(),
            selection_generation: 3,
            provider_generation: 7,
        }
    }

    #[test]
    fn direct_romm_cover_beats_launchbox_and_esde() {
        let mut direct = provider(MediaProvider::RommDirect, MediaDelivery::RemoteReady);
        direct.cover = Some(media("romm-cover"));
        let mut launchbox = provider(MediaProvider::LaunchBoxViaRomm, MediaDelivery::RemoteReady);
        launchbox.cover = Some(media("launchbox-cover"));
        let mut esde = provider(MediaProvider::EsDe, MediaDelivery::LocalReady);
        esde.cover = Some(media("esde-cover"));
        let resolved = resolve_media(&input(vec![esde, launchbox, direct]));
        assert_eq!(resolved.cover.unwrap().provider, MediaProvider::RommDirect);
    }

    #[test]
    fn local_cache_is_immediate_while_remote_provider_is_pending() {
        let mut cache = provider(MediaProvider::LocalCache, MediaDelivery::LocalReady);
        cache.cover = Some(media("cached-cover"));
        let mut request = input(vec![cache]);
        request.pending_providers = vec![MediaProvider::RommDirect];
        let resolved = resolve_media(&request);
        assert_eq!(resolved.cover_delivery, MediaDelivery::LocalReady);
        assert_eq!(resolved.pending_providers, vec![MediaProvider::RommDirect]);
    }

    #[test]
    fn screenshots_are_independent_bounded_and_deduplicated() {
        let mut direct = provider(MediaProvider::RommDirect, MediaDelivery::RemoteReady);
        direct.cover = Some(media("cover"));
        direct.screenshots = vec![media("shot-a"), media("shot-b")];
        let mut launchbox = provider(MediaProvider::LaunchBoxViaRomm, MediaDelivery::RemoteReady);
        launchbox.screenshots = vec![media("shot-b"), media("shot-c")];
        let resolved = resolve_media(&input(vec![direct, launchbox]));
        assert_eq!(resolved.screenshots.len(), 3);
        assert_eq!(resolved.screenshots[0].provider, MediaProvider::RommDirect);
        assert_eq!(
            resolved.screenshots[2].provider,
            MediaProvider::LaunchBoxViaRomm
        );
        assert_eq!(resolved.cover.unwrap().provider, MediaProvider::RommDirect);
    }

    #[test]
    fn video_is_per_game_and_keeps_provider_provenance() {
        let mut esde = provider(MediaProvider::EsDe, MediaDelivery::LocalReady);
        esde.video = Some(media("local-video"));
        let resolved = resolve_media(&input(vec![esde]));
        assert_eq!(resolved.video.unwrap().provider, MediaProvider::EsDe);
        assert_eq!(resolved.video_delivery, MediaDelivery::LocalReady);
    }

    #[test]
    fn failed_provider_does_not_hide_sibling_screenshots() {
        let mut launchbox = provider(MediaProvider::LaunchBoxViaRomm, MediaDelivery::RemoteReady);
        launchbox.screenshots = vec![media("shot")];
        let mut request = input(vec![launchbox]);
        request.failed_providers = vec![MediaProvider::RommDirect];
        let resolved = resolve_media(&request);
        assert_eq!(resolved.screenshots.len(), 1);
        assert_eq!(resolved.failed_providers, vec![MediaProvider::RommDirect]);
    }

    #[test]
    fn stale_generation_is_rejected() {
        let resolved = resolve_media(&input(Vec::new()));
        assert!(resolved.is_current(3, 7));
        assert!(!resolved.is_current(4, 7));
        assert!(!resolved.is_current(3, 8));
    }

    #[test]
    fn pending_without_media_is_not_reported_as_permanently_unavailable() {
        let mut request = input(Vec::new());
        request.pending_providers = vec![MediaProvider::LaunchBoxViaRomm];
        let resolved = resolve_media(&request);
        assert_eq!(resolved.cover_delivery, MediaDelivery::RemotePending);
        assert_eq!(resolved.screenshots_delivery, MediaDelivery::RemotePending);
    }
}
