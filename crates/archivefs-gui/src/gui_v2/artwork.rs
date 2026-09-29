//! Three bounded lanes: two local decoders and one remote/cache writer. UI work
//! is map lookups, queueing and bounded texture uploads, never image/file I/O.
use super::{
    library::SharedLibrary,
    media_sources::{Kind, MediaIndex, Source},
    thumbnail::{self, Pixels, Timings},
};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Key {
    pub game: i64,
    pub kind: Kind,
    pub generation: u64,
}

struct Request {
    key: Key,
    index: Arc<MediaIndex>,
    remote: bool,
    cancel: Arc<AtomicBool>,
    cache: Option<std::path::PathBuf>,
}
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Request>,
    stopped: bool,
}
type SharedQueue = Arc<(Mutex<Queue>, Condvar)>;

enum Reply {
    Index {
        generation: u64,
        index: Arc<MediaIndex>,
    },
    Picture {
        key: Key,
        result: Result<Pixels, String>,
        completed: Instant,
    },
}

pub(super) enum Picture {
    Loading,
    Missing,
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
}

impl Artwork {
    pub fn start(context: egui::Context) -> Self {
        Self::with_cache(context, None)
    }
    pub fn with_cache(context: egui::Context, cache: Option<std::path::PathBuf>) -> Self {
        let queue: SharedQueue = Arc::default();
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
                        MediaIndex::discover(&request.1)
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
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| resolve(&request, &transport)))
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
                            Err(error) => {
                                self.failures += 1;
                                self.failed_at.insert(key, Instant::now());
                                Picture::Failed(error)
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
        if matches!(self.pictures.get(&key), Some(Picture::Failed(_)))
            && self
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

fn resolve(request: &Request, transport: &TimedTransport) -> Result<Option<Pixels>, String> {
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
                net_policy::SystemResolver,
                romm::config::ValidatedRommSource,
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
                    return Err(
                        "Online artwork is switched off. Existing pictures remain available."
                            .into(),
                    );
                }
                let start = Instant::now();
                let token = load_token_file(settings.token_path.as_deref()).map_err(|_| "Artwork access needs attention. Open Sources & Providers to check the connection.")?;
                let source = ValidatedRommSource::validate(settings, &token, &request.index.trusted_roots, &SystemResolver).map_err(|_| "The artwork connection could not be approved. Check its setup in Sources & Providers.")?;
                transport.reset();
                let now = std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let answer = cache
                    .fetch(&source, transport, &request_art, now, Some(&request.cancel))
                    .map_err(|refusal| refusal.detail())?;
                timings.network = transport.elapsed();
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

#[derive(Default)]
struct TimedTransport {
    inner: archivefs_core::identity_source::romm::client::UreqTransport,
    time: Mutex<Duration>,
}
impl TimedTransport {
    fn reset(&self) {
        if let Ok(mut time) = self.time.lock() {
            *time = Duration::ZERO;
        }
    }
    fn elapsed(&self) -> Duration {
        self.time.lock().map(|time| *time).unwrap_or_default()
    }
}
impl archivefs_core::identity_source::romm::client::RommTransport for TimedTransport {
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
        assert!(matches!(resolve(&request, &transport), Ok(None)));
        assert_eq!(transport.elapsed(), Duration::ZERO);
        request.remote = true;
        assert!(resolve(&request, &transport).is_err()); // disabled provider, never a request
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
}
