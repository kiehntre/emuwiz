//! Three bounded lanes: two local decoders and one remote/cache writer. UI work
//! is map lookups, queueing and bounded texture uploads, never image/file I/O.
use super::{
    library::SharedLibrary,
    media_sources::{Kind, MediaIndex, Source},
    thumbnail::{self, Pixels, Timings},
};
use archivefs_core::identity_source::romm::connectivity::RommConnectivity;
use eframe::egui;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

pub(super) const LOCAL_WORKERS: usize = 2;
pub(super) const REMOTE_WORKERS: usize = 1;
const MAX_QUEUE: usize = 96;
/// A failed picture (for example an unreachable provider) is asked for again
/// after this long, instead of staying failed until the library is reloaded.
const FAILURE_RETRY: Duration = Duration::from_secs(120);
const MAX_TEXTURES: usize = 192;
/// After a provider proves unreachable, further remote pictures fail fast for
/// this long without touching the network; then exactly one probe is allowed.
/// Shorter than [`FAILURE_RETRY`] so a picture's own retry finds a fresh probe.
pub(super) const PROVIDER_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Key {
    pub game: i64,
    pub kind: Kind,
    pub generation: u64,
}

/// Why a picture could not be produced. `connectivity` is set only when the
/// cause is the provider's state (unreachable, unauthorised, switched off), so
/// the interface never mistakes an offline server for a missing picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolveFailure {
    pub message: String,
    pub connectivity: Option<RommConnectivity>,
}
impl ResolveFailure {
    fn provider(state: RommConnectivity) -> Self {
        Self {
            message: state.plain_message().into(),
            connectivity: Some(state),
        }
    }
}
impl From<String> for ResolveFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            connectivity: None,
        }
    }
}
impl From<&str> for ResolveFailure {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

/// What the last remote attempts showed about the RomM provider. Shared by the
/// picture workers so one unreachable provider costs one attempt per backoff
/// window, not one per visible cover, and never holds up other providers.
pub(super) struct ProviderHealth {
    backoff: Duration,
    last: Mutex<Option<(RommConnectivity, Instant)>>,
}
impl ProviderHealth {
    pub(super) fn new(backoff: Duration) -> Self {
        Self {
            backoff,
            last: Mutex::new(None),
        }
    }
    /// `Some(state)` when a request should fail fast without any network use.
    /// When the window has passed, the caller becomes the single probe: the
    /// window restarts so everything else keeps failing fast until it reports.
    fn gate(&self) -> Option<RommConnectivity> {
        let mut last = self.last.lock().ok()?;
        let (state, since) = (*last)?;
        let waits =
            state.is_recoverable_by_waiting() || state == RommConnectivity::AuthenticationFailed;
        if !waits {
            return None;
        }
        if since.elapsed() < self.backoff {
            return Some(state);
        }
        *last = Some((state, Instant::now()));
        None
    }
    fn record(&self, state: RommConnectivity) {
        if let Ok(mut last) = self.last.lock() {
            *last = Some((state, Instant::now()));
        }
    }
    /// A person asked to retry: the next request is allowed through.
    pub(super) fn reset(&self) {
        if let Ok(mut last) = self.last.lock() {
            *last = None;
        }
    }
    pub(super) fn snapshot(&self) -> Option<(RommConnectivity, Duration)> {
        self.last
            .lock()
            .ok()
            .and_then(|last| last.map(|(state, at)| (state, at.elapsed())))
    }
}

struct Request {
    key: Key,
    index: Arc<MediaIndex>,
    remote: bool,
    cancel: Arc<AtomicBool>,
    cache: Option<std::path::PathBuf>,
}
/// A provider-state failure is `Unavailable` (the picture may well exist); any
/// other failure is `Failed`. Neither is ever `Missing`.
pub(super) fn picture_for_failure(failure: ResolveFailure) -> Picture {
    match failure.connectivity {
        Some(issue) if issue != RommConnectivity::Reachable => Picture::Unavailable {
            issue,
            message: failure.message,
        },
        _ => Picture::Failed(failure.message),
    }
}

/// What to say in a picture's place. Pure, so every state is unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PictureLabel {
    pub headline: &'static str,
    pub detail: Option<String>,
}
impl PictureLabel {
    fn new(headline: &'static str) -> Self {
        Self {
            headline,
            detail: None,
        }
    }
}
/// A: index not ready · B: ready (no label) · C: loading · D: provider
/// unavailable · E: no picture · F: failed, will retry.
pub(super) fn picture_label(
    preparing: bool,
    paused: bool,
    picture: Option<&Picture>,
) -> PictureLabel {
    match picture {
        Some(Picture::Ready { .. }) => PictureLabel::new(""),
        _ if preparing => PictureLabel::new("Preparing artwork…"),
        Some(Picture::Missing) => PictureLabel::new("No picture yet"),
        Some(Picture::Unavailable { issue, .. }) => PictureLabel {
            headline: "Picture unavailable",
            detail: Some(issue.plain_message().to_string()),
        },
        Some(Picture::Failed(_)) => PictureLabel {
            headline: "Picture temporarily unavailable",
            detail: Some("Will try again soon.".into()),
        },
        _ if paused => PictureLabel::new("Pictures paused"),
        _ => PictureLabel::new("Loading picture…"),
    }
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Request>,
    stopped: bool,
}
type SharedQueue = Arc<(Mutex<Queue>, Condvar)>;
pub(super) type Indexer = Arc<dyn Fn(&SharedLibrary) -> MediaIndex + Send + Sync>;

enum Reply {
    Index {
        generation: u64,
        index: Arc<MediaIndex>,
    },
    Picture {
        key: Key,
        result: Result<Pixels, ResolveFailure>,
        completed: Instant,
    },
}

pub(super) enum Picture {
    Loading,
    Missing,
    /// The picture is known to exist (or may exist) but the provider cannot
    /// serve it right now. Not the same as `Missing`.
    Unavailable {
        issue: RommConnectivity,
        message: String,
    },
    Failed(String),
    Ready {
        texture: egui::TextureHandle,
        timings: Timings,
    },
}

pub(super) struct Artwork {
    queue: SharedQueue,
    index_requests: Option<Sender<(u64, SharedLibrary)>>,
    index_replies: Receiver<Reply>,
    replies: Receiver<Reply>,
    pub index: Option<Arc<MediaIndex>>,
    pub generation: u64,
    pub pictures: HashMap<Key, Picture>,
    failed_at: HashMap<Key, Instant>,
    failure_retry: Duration,
    pending: HashMap<Key, Arc<AtomicBool>>,
    wanted: HashSet<Key>,
    last_seen: HashMap<Key, u64>,
    frame: u64,
    pub requested: u64,
    pub completed: u64,
    pub failures: u64,
    pub cancelled: u64,
    pub index_loading: bool,
    pub paused: bool,
    cache: Option<std::path::PathBuf>,
    pub health: Arc<ProviderHealth>,
}

impl Artwork {
    pub fn start(context: egui::Context) -> Self {
        Self::with_cache(context, None)
    }
    pub fn with_cache(context: egui::Context, cache: Option<std::path::PathBuf>) -> Self {
        Self::with_indexer(
            context,
            cache,
            Arc::new(|library: &SharedLibrary| MediaIndex::discover(library)),
        )
    }
    /// The index builder is injected so tests never read a developer's real
    /// provider data; production always uses [`MediaIndex::discover`].
    pub fn with_indexer(
        context: egui::Context,
        cache: Option<std::path::PathBuf>,
        indexer: Indexer,
    ) -> Self {
        let queue: SharedQueue = Arc::default();
        let health = Arc::new(ProviderHealth::new(PROVIDER_BACKOFF));
        let (answers, replies) = mpsc::sync_channel(32);
        let (index_answers, index_replies) = mpsc::sync_channel(1);
        let (index_requests, index_rx) = mpsc::channel::<(u64, SharedLibrary)>();
        {
            let index_answers = index_answers.clone();
            let context = context.clone();
            std::thread::spawn(move || {
                while let Ok(mut request) = index_rx.recv() {
                    // A rescan can supersede a queued index. Only build the newest.
                    while let Ok(newer) = index_rx.try_recv() {
                        request = newer;
                    }
                    let index = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        indexer(&request.1)
                    }))
                    .unwrap_or_else(|_| MediaIndex {
                        warnings: vec![
                            "Artwork discovery stopped unexpectedly. Reload the library to retry."
                                .into(),
                        ],
                        ..MediaIndex::default()
                    });
                    if let Ok(root) = thumbnail::cache_root() {
                        thumbnail::trim_cache(&root);
                    }
                    if index_answers
                        .send(Reply::Index {
                            generation: request.0,
                            index: Arc::new(index),
                        })
                        .is_err()
                    {
                        break;
                    }
                    log::debug!("gui_v2 artwork indexing finished: generation {}", request.0);
                    context.request_repaint();
                }
            });
        }
        for worker in 0..LOCAL_WORKERS + REMOTE_WORKERS {
            let remote = worker >= LOCAL_WORKERS;
            let queue = queue.clone();
            let answers = answers.clone();
            let context = context.clone();
            let health = health.clone();
            std::thread::spawn(move || {
                let transport = TimedTransport::default();
                loop {
                    let request = {
                        let (lock, ready) = &*queue;
                        let Ok(mut state) = lock.lock() else {
                            break;
                        };
                        loop {
                            if state.stopped {
                                return;
                            }
                            if let Some(position) =
                                state.jobs.iter().position(|job| job.remote == remote)
                            {
                                break state.jobs.remove(position);
                            }
                            state = match ready.wait(state) {
                                Ok(state) => state,
                                Err(_) => return,
                            };
                        }
                    };
                    let Some(mut request) = request else {
                        continue;
                    };
                    let result = if request.cancel.load(Ordering::Relaxed) {
                        Err("Cancelled".into())
                    } else {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| resolve(&request, &transport, &health)))
                            .unwrap_or_else(|_| Err("The picture could not be decoded safely. Retry or choose another picture.".into()))
                    };
                    let result = match result {
                        Ok(Some(pixels)) => Ok(pixels),
                        Ok(None) => {
                            // Probe disk caches on a local lane first. A slow
                            // online request cannot queue ahead of cached covers.
                            request.remote = true;
                            if let Ok(mut state) = queue.0.lock() {
                                state.jobs.push_back(request);
                                queue.1.notify_all();
                            }
                            continue;
                        }
                        Err(error) => Err(error),
                    };
                    if answers
                        .send(Reply::Picture {
                            key: request.key,
                            result,
                            completed: Instant::now(),
                        })
                        .is_err()
                    {
                        break;
                    }
                    context.request_repaint();
                }
            });
        }
        Self {
            queue,
            index_requests: Some(index_requests),
            index_replies,
            replies,
            index: None,
            generation: 0,
            cache,
            pictures: HashMap::new(),
            failed_at: HashMap::new(),
            failure_retry: FAILURE_RETRY,
            pending: HashMap::new(),
            wanted: HashSet::new(),
            last_seen: HashMap::new(),
            frame: 0,
            requested: 0,
            completed: 0,
            failures: 0,
            cancelled: 0,
            index_loading: false,
            paused: false,
            health,
        }
    }

    pub fn reload(&mut self, library: SharedLibrary) {
        self.cancel();
        self.paused = false;
        self.generation += 1;
        self.pending.clear();
        self.index = None;
        self.index_loading = true;
        self.pictures.clear();
        self.last_seen.clear();
        self.requested = 0;
        self.completed = 0;
        self.failures = 0;
        self.cancelled = 0;
        log::debug!(
            "gui_v2 artwork indexing queued: generation {}, {} library games",
            self.generation,
            library.games.len()
        );
        if self
            .index_requests
            .as_ref()
            .is_none_or(|sender| sender.send((self.generation, library)).is_err())
        {
            self.index_loading = false;
        }
    }
    pub fn begin_frame(&mut self, context: &egui::Context) {
        self.frame += 1;
        self.wanted.clear();
        if let Ok(Reply::Index { generation, index }) = self.index_replies.try_recv()
            && generation == self.generation
        {
            self.index = Some(index);
            self.index_loading = false;
            context.request_repaint();
        }
        // Bound uploads per frame; the bounded reply channel supplies backpressure.
        for _ in 0..4 {
            let Ok(reply) = self.replies.try_recv() else {
                break;
            };
            match reply {
                Reply::Index { generation, index } if generation == self.generation => {
                    self.index = Some(index);
                    self.index_loading = false;
                }
                Reply::Picture {
                    key,
                    result,
                    completed,
                } if key.generation == self.generation => {
                    self.completed += 1;
                    let cancelled = self
                        .pending
                        .remove(&key)
                        .is_none_or(|cancel| cancel.load(Ordering::Relaxed));
                    if !cancelled {
                        let picture = match result {
                            Ok(mut pixels) => {
                                pixels.timings.delivery = completed.elapsed();
                                log::debug!("gui_v2 artwork {:?}: {:?}", key, pixels.timings);
                                let texture = context.load_texture(
                                    format!("v2-{}-{:?}-{}", key.game, key.kind, key.generation),
                                    pixels.image,
                                    egui::TextureOptions::LINEAR,
                                );
                                Picture::Ready {
                                    texture,
                                    timings: pixels.timings,
                                }
                            }
                            Err(failure) => {
                                self.failures += 1;
                                self.failed_at.insert(key, Instant::now());
                                picture_for_failure(failure)
                            }
                        };
                        self.pictures.insert(key, picture);
                    } else {
                        self.cancelled += 1;
                        self.pictures.remove(&key);
                    }
                }
                _ => {}
            }
            context.request_repaint();
        }
    }
    pub fn key(&self, game: i64, kind: Kind) -> Key {
        Key {
            game,
            kind,
            generation: self.generation,
        }
    }
    pub fn request(&mut self, game: i64, kind: Kind) -> Key {
        let key = self.key(game, kind);
        self.wanted.insert(key);
        self.last_seen.insert(key, self.frame);
        // A failure is not permanent: once it is old enough, forget it so the
        // picture is requested again.
        if matches!(
            self.pictures.get(&key),
            Some(Picture::Failed(_) | Picture::Unavailable { .. })
        ) && self
            .failed_at
            .get(&key)
            .is_some_and(|at| at.elapsed() >= self.failure_retry)
        {
            self.pictures.remove(&key);
            self.failed_at.remove(&key);
        }
        if self.paused || self.pictures.contains_key(&key) || self.pending.contains_key(&key) {
            return key;
        }
        let Some(index) = self.index.as_ref() else {
            return key;
        };
        let Some(source) = index.source(game, kind) else {
            self.pictures.insert(key, Picture::Missing);
            return key;
        };
        let _ = source;
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut state) = self.queue.0.lock()
            && state.jobs.len() < MAX_QUEUE
        {
            state.jobs.push_back(Request {
                key,
                index: index.clone(),
                remote: false,
                cancel: cancel.clone(),
                cache: self.cache.clone(),
            });
            self.pending.insert(key, cancel);
            self.pictures.insert(key, Picture::Loading);
            self.requested += 1;
            self.queue.1.notify_all();
        }
        key
    }
    pub fn end_frame(&mut self) {
        for (key, cancel) in &self.pending {
            if !self.wanted.contains(key) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        if self.pictures.len() > MAX_TEXTURES {
            let mut old: Vec<_> = self
                .last_seen
                .iter()
                .filter(|(key, _)| !self.wanted.contains(key) && !self.pending.contains_key(key))
                .map(|(key, frame)| (*key, *frame))
                .collect();
            old.sort_by_key(|(_, frame)| *frame);
            for (key, _) in old.into_iter().take(self.pictures.len() - MAX_TEXTURES) {
                self.pictures.remove(&key);
                self.last_seen.remove(&key);
            }
        }
    }
    pub fn retry(&mut self, key: Key) {
        // A person's explicit retry is allowed to probe the provider again.
        self.health.reset();
        if !self.pending.contains_key(&key) {
            self.pictures.remove(&key);
            self.failed_at.remove(&key);
        }
        self.paused = false;
    }
    pub fn cancel(&mut self) {
        self.paused = true;
        for cancel in self.pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    /// The picture list is not ready to answer yet: covers cannot be looked up
    /// until the artwork index exists.
    pub fn preparing(&self) -> bool {
        self.index.is_none()
    }
    /// How long until a failed picture is asked for again (only while it stays
    /// on screen). `None` when the picture is not in a retryable state.
    pub fn retry_in(&self, key: Key) -> Option<Duration> {
        matches!(
            self.pictures.get(&key),
            Some(Picture::Failed(_) | Picture::Unavailable { .. })
        )
        .then(|| {
            self.failed_at.get(&key).map_or(Duration::ZERO, |at| {
                self.failure_retry.saturating_sub(at.elapsed())
            })
        })
    }
    pub fn active(&self) -> usize {
        self.pending.len()
    }
}

impl Drop for Artwork {
    fn drop(&mut self) {
        self.cancel();
        self.index_requests.take();
        if let Ok(mut state) = self.queue.0.lock() {
            state.stopped = true;
            state.jobs.clear();
            self.queue.1.notify_all();
        }
    }
}

fn resolve(
    request: &Request,
    transport: &TimedTransport,
    health: &ProviderHealth,
) -> Result<Option<Pixels>, ResolveFailure> {
    resolve_with(
        request,
        transport,
        &archivefs_core::identity_source::net_policy::SystemResolver,
        health,
    )
}

/// The resolver with its network seams injected: name resolution and the HTTP
/// transport. Production passes the system resolver and `ureq`; tests pass
/// fakes, so no test depends on a developer's DNS or on a real RomM.
fn resolve_with<R, T>(
    request: &Request,
    transport: &TimedTransport<T>,
    resolver: &R,
    health: &ProviderHealth,
) -> Result<Option<Pixels>, ResolveFailure>
where
    R: archivefs_core::identity_source::net_policy::HostResolver,
    T: archivefs_core::identity_source::romm::client::RommTransport,
{
    let start = Instant::now();
    let source = request
        .index
        .source(request.key.game, request.key.kind)
        .ok_or("No picture is available.")?;
    let metadata_time = start.elapsed();
    let mut pixels = match source {
        Source::Local(path) => thumbnail::load_local(
            path,
            &request
                .cache
                .clone()
                .map(Ok)
                .unwrap_or_else(thumbnail::cache_root)?,
        )?,
        Source::Remote { record, kind } => {
            use archivefs_core::identity_source::{
                artwork::{ArtworkCache, ArtworkRequest},
                model::IdentityProvider,
                romm::config::{ConfigRefusal, ValidatedRommSource},
                settings::load_token_file,
            };
            let request_art = match kind {
                Kind::Cover => ArtworkRequest::from_record(record),
                Kind::Screenshot(index) => ArtworkRequest::from_media(
                    &record.provider_game_id,
                    record
                        .artwork
                        .as_ref()
                        .and_then(|art| art.screenshots.get(*index))
                        .ok_or("This screenshot is no longer available.")?,
                ),
            };
            let cache = ArtworkCache::new(&request.index.identity_root, IdentityProvider::Romm);
            let mut timings = Timings::default();
            let start = Instant::now();
            // Cache first: a cached picture never needs the provider, so it is
            // shown whether or not RomM can be reached.
            let cached = cache.lookup(&request.index.server, &request_art);
            timings.lookup = start.elapsed();
            timings.cache_hit = cached.is_some();
            let thumbnail = if let Some(cached) = cached {
                cached
            } else {
                if !request.remote {
                    return Ok(None);
                }
                let settings = &request.index.settings.source;
                if !settings.enabled {
                    return Err(ResolveFailure {
                        message:
                            "Online artwork is switched off. Existing pictures remain available."
                                .into(),
                        connectivity: Some(RommConnectivity::Disabled),
                    });
                }
                if settings.url.trim().is_empty() {
                    return Err(ResolveFailure::provider(RommConnectivity::NotConfigured));
                }
                // A provider that just proved unreachable is not asked again for
                // every visible cover: fail fast, no DNS, no socket.
                if let Some(state) = health.gate() {
                    return Err(ResolveFailure::provider(state));
                }
                let start = Instant::now();
                let token = load_token_file(settings.token_path.as_deref()).map_err(|_| ResolveFailure {
                    message: "Artwork access needs attention. Open Sources & Providers to check the connection.".into(),
                    connectivity: Some(RommConnectivity::AuthenticationFailed),
                })?;
                let source = ValidatedRommSource::validate(
                    settings,
                    &token,
                    &request.index.trusted_roots,
                    resolver,
                )
                .map_err(|refusal| {
                    let state = match &refusal {
                        ConfigRefusal::Endpoint(endpoint) => {
                            Some(RommConnectivity::from_endpoint_refusal(endpoint))
                        }
                        _ => None,
                    };
                    if let Some(state) = state.filter(|state| state.is_unreachable()) {
                        health.record(state);
                    }
                    match state {
                        Some(state) => ResolveFailure::provider(state),
                        None => ResolveFailure::from(
                            "The artwork connection could not be approved. Check its setup in Sources & Providers.",
                        ),
                    }
                })?;
                transport.reset();
                let now = std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let answer = cache
                    .fetch(&source, transport, &request_art, now, Some(&request.cancel))
                    .map_err(|refusal| {
                        let state = RommConnectivity::from_artwork_refusal(&refusal);
                        if let Some(state) = state.filter(|state| {
                            state.is_unreachable()
                                || state.is_recoverable_by_waiting()
                                || *state == RommConnectivity::AuthenticationFailed
                        }) {
                            health.record(state);
                        }
                        ResolveFailure {
                            message: match state {
                                Some(state) => state.plain_message().to_string(),
                                None => refusal.detail(),
                            },
                            connectivity: state,
                        }
                    })?;
                timings.network = transport.elapsed();
                if transport.contacted() {
                    health.record(RommConnectivity::Reachable);
                }
                timings.provider_processing = start.elapsed().saturating_sub(timings.network);
                answer
            };
            // Core already writes bounded PNGs. Decode/resize the grid-sized copy
            // here, never upload or retain the provider's full-size source.
            let image = thumbnail::decode(&thumbnail.path, false, &mut timings)?;
            Pixels { image, timings }
        }
    };
    pixels.timings.metadata = metadata_time;
    Ok(Some(pixels))
}

struct TimedTransport<T = archivefs_core::identity_source::romm::client::UreqTransport> {
    inner: T,
    time: Mutex<Duration>,
    calls: std::sync::atomic::AtomicUsize,
}
impl Default for TimedTransport {
    fn default() -> Self {
        Self::new(archivefs_core::identity_source::romm::client::UreqTransport::default())
    }
}
impl<T> TimedTransport<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            time: Mutex::new(Duration::ZERO),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
    fn reset(&self) {
        self.calls.store(0, Ordering::Relaxed);
        if let Ok(mut time) = self.time.lock() {
            *time = Duration::ZERO;
        }
    }
    /// The provider was actually contacted (a cached or locally-mapped picture
    /// proves nothing about the server).
    fn contacted(&self) -> bool {
        self.calls.load(Ordering::Relaxed) > 0
    }
    fn elapsed(&self) -> Duration {
        self.time.lock().map(|time| *time).unwrap_or_default()
    }
}
impl<T: archivefs_core::identity_source::romm::client::RommTransport>
    archivefs_core::identity_source::romm::client::RommTransport for TimedTransport<T>
{
    fn get(
        &self,
        url: &str,
        authorization: Option<&str>,
        max_bytes: usize,
        timeout: Duration,
    ) -> Result<
        archivefs_core::identity_source::romm::client::RommHttpResponse,
        archivefs_core::identity_source::romm::client::RommRequestError,
    > {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let start = Instant::now();
        let result = self.inner.get(url, authorization, max_bytes, timeout);
        if let Ok(mut time) = self.time.lock() {
            *time += start.elapsed();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_v2_local_cache_probe_defers_remote_work_to_the_network_lane() {
        let directory = tempfile::tempdir().unwrap();
        let record = serde_json::from_value(serde_json::json!({
            "provider": "romm", "server_id": "fixture", "provider_game_id": "game", "provider_path": "/game",
            "regions": [], "hashes": [], "metadata_provider_ids": [], "related_files": [], "sibling_game_ids": [],
            "imported_at_unix_seconds": 0, "verification": "strong_external", "conflicts": [], "evidence": [],
            "artwork": { "reference": "/cover.png", "small_reference": "/cover.png" }
        })).unwrap();
        let mut index = MediaIndex {
            identity_root: directory.path().to_path_buf(),
            server: "fixture".into(),
            ..MediaIndex::default()
        };
        index.covers.insert(
            1,
            Source::Remote {
                record: Arc::new(record),
                kind: Kind::Cover,
            },
        );
        let mut request = Request {
            key: Key {
                game: 1,
                kind: Kind::Cover,
                generation: 1,
            },
            index: Arc::new(index),
            remote: false,
            cancel: Arc::new(AtomicBool::new(false)),
            cache: None,
        };
        let transport = TimedTransport::default();
        assert!(matches!(resolve(&request, &transport, &health()), Ok(None)));
        assert_eq!(transport.elapsed(), Duration::ZERO);
        request.remote = true;
        assert!(resolve(&request, &transport, &health()).is_err()); // disabled provider, never a request
        assert_eq!(transport.elapsed(), Duration::ZERO);
    }

    #[test]
    fn a_failed_picture_is_requested_again_after_the_retry_delay_not_never() {
        let context = egui::Context::default();
        let mut artwork = Artwork::start(context);
        let mut index = MediaIndex::default();
        index.covers.insert(
            1,
            Source::Local(std::path::PathBuf::from("/definitely/not/here.png")),
        );
        artwork.index = Some(Arc::new(index));
        artwork.paused = false;
        let key = artwork.key(1, Kind::Cover);
        artwork
            .pictures
            .insert(key, Picture::Failed("provider unreachable".into()));
        artwork.failed_at.insert(key, Instant::now());
        // Fresh failure: not re-requested yet.
        artwork.request(1, Kind::Cover);
        assert!(matches!(
            artwork.pictures.get(&key),
            Some(Picture::Failed(_))
        ));
        // Old failure: forgotten and queued again.
        artwork.failed_at.insert(
            key,
            Instant::now() - artwork.failure_retry - Duration::from_secs(1),
        );
        artwork.request(1, Kind::Cover);
        assert!(matches!(artwork.pictures.get(&key), Some(Picture::Loading)));
        assert_eq!(artwork.active(), 1);
    }

    fn write_png(path: &std::path::Path) {
        image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 30, 30, 255]))
            .save(path)
            .unwrap();
    }

    fn wait_for(artwork: &mut Artwork, context: &egui::Context, key: Key) -> bool {
        for _ in 0..400 {
            artwork.begin_frame(context);
            if matches!(artwork.pictures.get(&key), Some(Picture::Ready { .. })) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn a_resolved_local_cover_reaches_the_gui_as_a_ready_texture() {
        let directory = tempfile::tempdir().unwrap();
        let cover = directory.path().join("cover.png");
        write_png(&cover);
        let context = egui::Context::default();
        let mut artwork =
            Artwork::with_cache(context.clone(), Some(directory.path().join("cache")));
        let mut index = MediaIndex::default();
        index.covers.insert(7, Source::Local(cover));
        artwork.index = Some(Arc::new(index));
        let key = artwork.request(7, Kind::Cover);
        assert!(matches!(artwork.pictures.get(&key), Some(Picture::Loading)));
        assert!(
            wait_for(&mut artwork, &context, key),
            "cover never became ready"
        );
        // A game with no resolved source is Missing, not a wrong picture.
        let other = artwork.request(8, Kind::Cover);
        assert!(matches!(
            artwork.pictures.get(&other),
            Some(Picture::Missing)
        ));
    }

    #[test]
    fn a_historical_no_cover_recovers_when_a_refreshed_index_has_the_cover() {
        let directory = tempfile::tempdir().unwrap();
        let cover = directory.path().join("cover.png");
        write_png(&cover);
        let context = egui::Context::default();
        let mut artwork =
            Artwork::with_cache(context.clone(), Some(directory.path().join("cache")));
        artwork.index = Some(Arc::new(MediaIndex::default()));
        let first = artwork.request(3, Kind::Cover);
        assert!(matches!(
            artwork.pictures.get(&first),
            Some(Picture::Missing)
        ));
        // What `reload` does when providers/assets become available: a new
        // generation, dropped pictures, and the new index.
        artwork.generation += 1;
        artwork.pictures.clear();
        let mut refreshed = MediaIndex::default();
        refreshed.covers.insert(3, Source::Local(cover));
        artwork.index = Some(Arc::new(refreshed));
        let second = artwork.request(3, Kind::Cover);
        assert_ne!(first.generation, second.generation);
        assert!(
            wait_for(&mut artwork, &context, second),
            "recovered cover never became ready"
        );
    }

    #[test]
    fn a_retry_that_fails_again_waits_a_full_delay_and_a_later_success_replaces_failure() {
        let directory = tempfile::tempdir().unwrap();
        let cover = directory.path().join("late.png");
        let context = egui::Context::default();
        let mut artwork =
            Artwork::with_cache(context.clone(), Some(directory.path().join("cache")));
        let mut index = MediaIndex::default();
        index.covers.insert(5, Source::Local(cover.clone()));
        artwork.index = Some(Arc::new(index));
        let key = artwork.key(5, Kind::Cover);
        artwork
            .pictures
            .insert(key, Picture::Failed("earlier".into()));
        let old = Instant::now() - artwork.failure_retry - Duration::from_secs(1);
        artwork.failed_at.insert(key, old);
        // Retry #1: the file is still absent, so it fails again...
        artwork.request(5, Kind::Cover);
        for _ in 0..400 {
            artwork.begin_frame(&context);
            if matches!(artwork.pictures.get(&key), Some(Picture::Failed(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            artwork.pictures.get(&key),
            Some(Picture::Failed(_))
        ));
        // ...and is stamped anew, so repeated requests do not busy-loop.
        assert!(artwork.failed_at[&key] > old);
        artwork.request(5, Kind::Cover);
        artwork.request(5, Kind::Cover);
        assert_eq!(artwork.active(), 0);
        assert!(matches!(
            artwork.pictures.get(&key),
            Some(Picture::Failed(_))
        ));
        // The cover appears later; once the delay passes the retry succeeds and
        // replaces the failed state.
        write_png(&cover);
        artwork.failed_at.insert(
            key,
            Instant::now() - artwork.failure_retry - Duration::from_secs(1),
        );
        artwork.request(5, Kind::Cover);
        assert!(wait_for(&mut artwork, &context, key));
        assert!(!artwork.failed_at.contains_key(&key));
    }

    // ---- RomM connectivity: deterministic, no DNS, no sockets ----

    use archivefs_core::identity_source::{
        net_policy::HostResolver,
        romm::client::{RommHttpResponse, RommRequestError, RommTransport},
        settings::ProviderSettings,
    };
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::AtomicUsize;

    struct FakeResolver {
        answer: Result<Vec<IpAddr>, String>,
        calls: AtomicUsize,
    }
    impl FakeResolver {
        fn resolving() -> Self {
            Self {
                answer: Ok(vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50))]),
                calls: AtomicUsize::new(0),
            }
        }
        fn failing() -> Self {
            Self {
                answer: Err("failed to lookup address information".into()),
                calls: AtomicUsize::new(0),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }
    impl HostResolver for FakeResolver {
        fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.answer.clone()
        }
    }

    struct FakeServer {
        script: Mutex<Vec<Result<RommHttpResponse, RommRequestError>>>,
        calls: AtomicUsize,
    }
    impl FakeServer {
        fn new(script: Vec<Result<RommHttpResponse, RommRequestError>>) -> Self {
            Self {
                script: Mutex::new(script),
                calls: AtomicUsize::new(0),
            }
        }
        fn serving_png() -> Self {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 200, 30, 255]))
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            let body = bytes.into_inner();
            Self::new(
                (0..8)
                    .map(|_| {
                        Ok(RommHttpResponse {
                            status: 200,
                            body: body.clone(),
                            location: None,
                        })
                    })
                    .collect(),
            )
        }
        fn failing(error: RommRequestError) -> Self {
            Self::new((0..8).map(|_| Err(error.clone())).collect())
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }
    impl RommTransport for FakeServer {
        fn get(
            &self,
            _url: &str,
            _authorization: Option<&str>,
            _max_bytes: usize,
            _timeout: Duration,
        ) -> Result<RommHttpResponse, RommRequestError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut script = self.script.lock().unwrap();
            if script.is_empty() {
                return Err(RommRequestError::Timeout);
            }
            script.remove(0)
        }
    }

    /// A remote-cover request against a configured (or not) RomM endpoint.
    fn remote_request(directory: &std::path::Path, url: &str, enabled: bool) -> Request {
        use std::os::unix::fs::PermissionsExt;
        let token = directory.join("token");
        std::fs::write(&token, "rk_test_abcdef0123456789\n").unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let record = serde_json::from_value(serde_json::json!({
            "provider": "romm", "server_id": "fixture", "provider_game_id": "game", "provider_path": "/game",
            "regions": [], "hashes": [], "metadata_provider_ids": [], "related_files": [], "sibling_game_ids": [],
            "imported_at_unix_seconds": 0, "verification": "strong_external", "conflicts": [], "evidence": [],
            "artwork": { "reference": "/cover.png", "small_reference": "/cover.png" }
        })).unwrap();
        let mut settings = ProviderSettings::default();
        settings.source.enabled = enabled;
        settings.source.url = url.into();
        settings.source.token_path = Some(token);
        let mut index = MediaIndex {
            identity_root: directory.join("identity"),
            server: url.trim_end_matches('/').to_string(),
            settings,
            ..MediaIndex::default()
        };
        index.covers.insert(
            1,
            Source::Remote {
                record: Arc::new(record),
                kind: Kind::Cover,
            },
        );
        Request {
            key: Key {
                game: 1,
                kind: Kind::Cover,
                generation: 1,
            },
            index: Arc::new(index),
            remote: true,
            cancel: Arc::new(AtomicBool::new(false)),
            cache: Some(directory.join("thumbs")),
        }
    }
    fn health() -> ProviderHealth {
        ProviderHealth::new(Duration::from_secs(60))
    }
    fn state_of(result: &Result<Option<Pixels>, ResolveFailure>) -> Option<RommConnectivity> {
        result
            .as_ref()
            .err()
            .and_then(|failure| failure.connectivity)
    }

    #[test]
    fn any_configured_endpoint_form_works_and_none_names_a_machine() {
        // Host name, IP + port, and HTTPS: whatever the person configured is
        // used as-is; the resolver is injected, so no real DNS is involved.
        for url in [
            "http://romm.local:8080",
            "http://192.168.1.50:8080",
            "https://romm.example.com",
            "http://my-nas",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let request = remote_request(directory.path(), url, true);
            let transport = TimedTransport::new(FakeServer::serving_png());
            let resolver = FakeResolver::resolving();
            let health = health();
            let result = resolve_with(&request, &transport, &resolver, &health);
            assert!(
                matches!(result, Ok(Some(_))),
                "{url}: {:?}",
                state_of(&result)
            );
            assert_eq!(resolver.calls(), 1, "{url}");
            assert_eq!(health.snapshot().unwrap().0, RommConnectivity::Reachable);
        }
    }

    #[test]
    fn a_base_path_is_refused_by_the_endpoint_policy_not_called_a_dns_failure() {
        let directory = tempfile::tempdir().unwrap();
        let request = remote_request(directory.path(), "http://192.168.1.50:8080/romm", true);
        let transport = TimedTransport::new(FakeServer::serving_png());
        let result = resolve_with(&request, &transport, &FakeResolver::resolving(), &health());
        assert_eq!(state_of(&result), Some(RommConnectivity::EndpointRefused));
        assert_eq!(transport.inner.calls(), 0);
    }

    #[test]
    fn no_romm_configuration_is_not_configured_and_makes_no_request() {
        let directory = tempfile::tempdir().unwrap();
        let request = remote_request(directory.path(), "", true);
        let transport = TimedTransport::new(FakeServer::serving_png());
        let resolver = FakeResolver::resolving();
        let result = resolve_with(&request, &transport, &resolver, &health());
        assert_eq!(state_of(&result), Some(RommConnectivity::NotConfigured));
        assert_eq!((resolver.calls(), transport.inner.calls()), (0, 0));
        let request = remote_request(directory.path(), "http://192.168.1.50:8080", false);
        let result = resolve_with(&request, &transport, &resolver, &health());
        assert_eq!(state_of(&result), Some(RommConnectivity::Disabled));
        assert_eq!((resolver.calls(), transport.inner.calls()), (0, 0));
    }

    #[test]
    fn a_dns_failure_is_reported_once_and_then_fails_fast_without_more_lookups() {
        let directory = tempfile::tempdir().unwrap();
        let request = remote_request(directory.path(), "http://romm.example.invalid:8080", true);
        let transport = TimedTransport::new(FakeServer::serving_png());
        let resolver = FakeResolver::failing();
        let health = health();
        let first = resolve_with(&request, &transport, &resolver, &health);
        assert_eq!(state_of(&first), Some(RommConnectivity::DnsFailure));
        assert_eq!(resolver.calls(), 1);
        // Many more covers ask: no request storm, no lookups, same honest state.
        for _ in 0..50 {
            let again = resolve_with(&request, &transport, &resolver, &health);
            assert_eq!(state_of(&again), Some(RommConnectivity::DnsFailure));
        }
        assert_eq!(resolver.calls(), 1);
        assert_eq!(transport.inner.calls(), 0);
    }

    #[test]
    fn refused_timeout_http_and_auth_failures_are_told_apart() {
        for (error, expected) in [
            (
                RommRequestError::Transport {
                    detail: "an I/O error occurred (connection refused)".into(),
                },
                RommConnectivity::ConnectionRefused,
            ),
            (RommRequestError::Timeout, RommConnectivity::Timeout),
            (
                RommRequestError::HttpStatus { status: 503 },
                RommConnectivity::HttpError(503),
            ),
            (
                RommRequestError::Unauthorised { status: 401 },
                RommConnectivity::AuthenticationFailed,
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let request = remote_request(directory.path(), "http://192.168.1.50:8080", true);
            let transport = TimedTransport::new(FakeServer::failing(error));
            let result = resolve_with(&request, &transport, &FakeResolver::resolving(), &health());
            assert_eq!(state_of(&result), Some(expected));
            assert!(transport.inner.calls() >= 1);
        }
    }

    #[test]
    fn a_cached_cover_is_shown_while_romm_is_offline_and_touches_no_network() {
        let directory = tempfile::tempdir().unwrap();
        let request = remote_request(directory.path(), "http://192.168.1.50:8080", true);
        // Online once: the picture is fetched and cached.
        let online = TimedTransport::new(FakeServer::serving_png());
        let first = resolve_with(&request, &online, &FakeResolver::resolving(), &health());
        assert!(matches!(first, Ok(Some(_))));
        // Now RomM is unreachable, DNS is broken and the provider is marked bad.
        let offline = TimedTransport::new(FakeServer::failing(RommRequestError::Timeout));
        let resolver = FakeResolver::failing();
        let health = health();
        health.record(RommConnectivity::DnsFailure);
        let second = resolve_with(&request, &offline, &resolver, &health);
        match second {
            Ok(Some(pixels)) => assert!(pixels.timings.cache_hit),
            other => panic!("cached cover must still show: {:?}", state_of(&other)),
        }
        assert_eq!((resolver.calls(), offline.inner.calls()), (0, 0));
    }

    #[test]
    fn an_uncached_known_cover_while_offline_is_unavailable_not_missing() {
        let failure = ResolveFailure::provider(RommConnectivity::DnsFailure);
        let picture = picture_for_failure(failure);
        assert!(matches!(picture, Picture::Unavailable { .. }));
        let label = picture_label(false, false, Some(&picture));
        assert_eq!(label.headline, "Picture unavailable");
        assert_eq!(
            label.detail.as_deref(),
            Some("RomM cannot currently be reached.")
        );
        assert_ne!(label.headline, "No picture yet");
    }

    #[test]
    fn a_game_with_no_romm_record_is_still_a_truthful_no_picture_when_romm_is_offline() {
        let context = egui::Context::default();
        let mut artwork = Artwork::start(context);
        artwork.health.record(RommConnectivity::DnsFailure);
        artwork.index = Some(Arc::new(MediaIndex::default()));
        let key = artwork.request(99, Kind::Cover);
        assert!(matches!(artwork.pictures.get(&key), Some(Picture::Missing)));
        assert_eq!(
            picture_label(false, false, artwork.pictures.get(&key)).headline,
            "No picture yet"
        );
    }

    #[test]
    fn the_provider_recovers_after_the_backoff_and_a_success_replaces_the_failure() {
        let directory = tempfile::tempdir().unwrap();
        let request = remote_request(directory.path(), "http://192.168.1.50:8080", true);
        let health = ProviderHealth::new(Duration::from_millis(30));
        let transport = TimedTransport::new(FakeServer::serving_png());
        let down = FakeResolver::failing();
        let first = resolve_with(&request, &transport, &down, &health);
        assert_eq!(state_of(&first), Some(RommConnectivity::DnsFailure));
        // Inside the window: still failing fast.
        assert_eq!(
            state_of(&resolve_with(&request, &transport, &down, &health)),
            Some(RommConnectivity::DnsFailure)
        );
        assert_eq!(down.calls(), 1);
        // After the window one probe goes out; DNS is back, the cover loads.
        std::thread::sleep(Duration::from_millis(40));
        let up = FakeResolver::resolving();
        let recovered = resolve_with(&request, &transport, &up, &health);
        assert!(matches!(recovered, Ok(Some(_))));
        assert_eq!(health.snapshot().unwrap().0, RommConnectivity::Reachable);
    }

    #[test]
    fn a_person_pressing_retry_is_allowed_to_probe_again_immediately() {
        let context = egui::Context::default();
        let mut artwork = Artwork::start(context);
        artwork.health.record(RommConnectivity::DnsFailure);
        assert!(artwork.health.gate().is_some());
        let key = artwork.key(1, Kind::Cover);
        artwork.retry(key);
        assert!(artwork.health.gate().is_none());
    }

    #[test]
    fn one_offline_provider_does_not_stop_local_pictures() {
        let directory = tempfile::tempdir().unwrap();
        let cover = directory.path().join("local.png");
        write_png(&cover);
        let context = egui::Context::default();
        let mut artwork =
            Artwork::with_cache(context.clone(), Some(directory.path().join("cache")));
        artwork.health.record(RommConnectivity::DnsFailure);
        let mut index = MediaIndex::default();
        index.covers.insert(4, Source::Local(cover));
        artwork.index = Some(Arc::new(index));
        let key = artwork.request(4, Kind::Cover);
        assert!(wait_for(&mut artwork, &context, key));
    }

    #[test]
    fn every_normal_artwork_state_has_its_own_honest_words() {
        let unavailable = Picture::Unavailable {
            issue: RommConnectivity::Timeout,
            message: String::new(),
        };
        let failed = Picture::Failed("x".into());
        // A: index not ready
        assert_eq!(
            picture_label(true, false, None).headline,
            "Preparing artwork…"
        );
        // C: loading
        assert_eq!(
            picture_label(false, false, Some(&Picture::Loading)).headline,
            "Loading picture…"
        );
        assert_eq!(
            picture_label(false, false, None).headline,
            "Loading picture…"
        );
        // D: provider unavailable
        let d = picture_label(false, false, Some(&unavailable));
        assert_eq!(d.headline, "Picture unavailable");
        assert_eq!(
            d.detail.as_deref(),
            Some("RomM cannot currently be reached.")
        );
        // E: nothing anywhere
        assert_eq!(
            picture_label(false, false, Some(&Picture::Missing)).headline,
            "No picture yet"
        );
        // F: failed, will retry
        let f = picture_label(false, false, Some(&failed));
        assert_eq!(f.headline, "Picture temporarily unavailable");
        assert!(f.detail.is_some());
        // Paused is only shown when nothing more specific is known.
        assert_eq!(picture_label(false, true, None).headline, "Pictures paused");
    }

    #[test]
    fn a_stale_result_for_another_game_cannot_land_on_the_current_game() {
        // Pictures are keyed by game and generation, so a late answer for game 1
        // is stored under game 1 and never shown for game 2.
        let context = egui::Context::default();
        let mut artwork = Artwork::start(context);
        artwork.index = Some(Arc::new(MediaIndex::default()));
        let first = artwork.key(1, Kind::Cover);
        let second = artwork.key(2, Kind::Cover);
        assert_ne!(first, second);
        artwork
            .pictures
            .insert(first, Picture::Failed("late answer".into()));
        artwork.request(2, Kind::Cover);
        assert!(matches!(
            artwork.pictures.get(&second),
            Some(Picture::Missing)
        ));
        // A reload (new generation) drops every old-generation picture's key.
        artwork.generation += 1;
        assert_ne!(artwork.key(1, Kind::Cover), first);
    }
}
