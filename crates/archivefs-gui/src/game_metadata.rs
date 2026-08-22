//! Gamer View's game-metadata enrichment lookup: synopsis, genre, players,
//! rating, release year, read from RomM's already-cached identity records
//! (see `archivefs_core::identity_source::romm::enrichment`).
//!
//! Deliberately mirrors `gamer_artwork.rs`'s cover-worker pattern rather
//! than reusing it directly: opening/indexing the identity cache for
//! covers already happens on `CoverWorker`'s own thread, and text
//! enrichment has none of that system's image-decode/texture/priority-
//! queue complexity, so extending `CoverWorker`'s message protocol to also
//! carry text would have added real risk to an already-tuned, already-
//! tested system for no benefit. This worker only ever reads the same
//! on-disk cache covers already read - no second network path, no second
//! cover cache, no artwork of any kind.
//!
//! # Cache-only; one lookup per selection, not per row
//!
//! Unlike covers (needed for every visible row every frame), enrichment is
//! only ever needed for the one currently selected/featured game, so a
//! linear-scan cache lookup per selection change - not per frame, not per
//! row - is the entire cost. There is no priority queue, no fairness
//! scheduling, and no "unchanged" fast path, because none of the problems
//! those solve exist at this scale.
//!
//! # Update reloads the cache; it does not sync
//!
//! "Update game information" (see [`GameMetadataWorker::reload`]) re-opens
//! the identity cache file from disk - useful right after a RomM sync run
//! from the Sources page picks up newly-cached enrichment without
//! restarting the app. It never itself contacts RomM's server: fetching
//! fresh data from RomM is the Sources page's existing sync action, which
//! this deliberately does not duplicate.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use archivefs_core::ArchiveMetadata;
use archivefs_core::identity_source::cache::IdentityCache;
use eframe::egui;

/// What the worker found for one lookup.
#[derive(Debug, Clone)]
pub(crate) enum GameMetadataResult {
    /// The cache has a record for this path with at least one enrichment
    /// field set.
    Found(Box<ArchiveMetadata>),
    /// The cache opened fine, but has no record for this path (never
    /// synced, or RomM never matched this file) - distinct from
    /// [`Self::Unavailable`] so the UI can say "not matched" rather than
    /// "no metadata source configured".
    NotFound,
    /// The identity cache itself could not be opened - RomM is not
    /// configured, has never been synced, or its published cache is
    /// unreadable. The game remains completely usable either way; this
    /// only changes what the enrichment panel says.
    Unavailable,
}

/// One answer, bound to the path that asked for it - a reply for a path
/// that is no longer selected by the time it arrives is simply not drawn.
pub(crate) struct GameMetadataReply {
    pub(crate) local_path: PathBuf,
    pub(crate) result: GameMetadataResult,
}

enum WorkerMessage {
    Lookup(PathBuf),
    /// Re-opens the cache from disk. See the module doc comment: this is
    /// "pick up what a sync already wrote", never a sync itself.
    Reload,
}

/// The background worker. Started lazily, the same way `CoverWorker` is -
/// a session that never opens Gamer View never opens the identity cache.
pub(crate) struct GameMetadataWorker {
    requests: Sender<WorkerMessage>,
    replies: Receiver<GameMetadataReply>,
    /// The path a `Lookup` was last sent for and no reply has arrived for
    /// yet - see [`should_send_lookup`]. Cleared once [`Self::poll`] drains
    /// that path's reply, so exactly one request is ever in flight per
    /// path, however many frames the answer takes to arrive.
    in_flight: Option<PathBuf>,
}

impl GameMetadataWorker {
    /// Starts the worker. Opening the cache happens on the thread, so a
    /// large cache (this project has seen one over 50 MB) never delays a
    /// frame.
    pub(crate) fn start(context: egui::Context) -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<WorkerMessage>();
        let (reply_tx, reply_rx) = std::sync::mpsc::channel::<GameMetadataReply>();
        std::thread::spawn(move || {
            let mut cache: Option<IdentityCache> = None;
            let mut tried_open = false;
            while let Ok(message) = request_rx.recv() {
                match message {
                    WorkerMessage::Reload => {
                        tried_open = true;
                        cache = open_cache();
                        context.request_repaint();
                    }
                    WorkerMessage::Lookup(local_path) => {
                        if !tried_open {
                            tried_open = true;
                            cache = open_cache();
                        }
                        let result = match &cache {
                            Some(cache) => lookup(cache, &local_path),
                            None => GameMetadataResult::Unavailable,
                        };
                        let _ = reply_tx.send(GameMetadataReply { local_path, result });
                        context.request_repaint();
                    }
                }
            }
        });
        Self {
            requests: request_tx,
            replies: reply_rx,
            in_flight: None,
        }
    }

    /// Asks for one path's enrichment. Fire-and-forget: the answer arrives
    /// through [`Self::poll`] on a later frame, matched by path.
    ///
    /// A no-op when a request for this exact path is already awaiting a
    /// reply - the caller re-derives "does the focused game have an
    /// answer yet" every frame, so without this guard the same `Lookup`
    /// would be sent again on every frame between selection and reply.
    pub(crate) fn request(&mut self, local_path: &Path) {
        if !should_send_lookup(&self.in_flight, local_path) {
            return;
        }
        self.in_flight = Some(local_path.to_path_buf());
        let _ = self
            .requests
            .send(WorkerMessage::Lookup(local_path.to_path_buf()));
    }

    /// "Update game information": re-opens the identity cache from disk.
    pub(crate) fn reload(&self) {
        let _ = self.requests.send(WorkerMessage::Reload);
    }

    /// Drains whatever replies have arrived since the last poll, clearing
    /// the in-flight guard for any path that just answered.
    pub(crate) fn poll(&mut self) -> Vec<GameMetadataReply> {
        let mut replies = Vec::new();
        loop {
            match self.replies.try_recv() {
                Ok(reply) => {
                    if self.in_flight.as_deref() == Some(reply.local_path.as_path()) {
                        self.in_flight = None;
                    }
                    replies.push(reply);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        replies
    }
}

/// Whether a new lookup for `path` should actually be enqueued, given what
/// is already in flight. Pure, so the one-request-per-path rule is tested
/// without a real worker thread.
fn should_send_lookup(in_flight: &Option<PathBuf>, path: &Path) -> bool {
    in_flight.as_deref() != Some(path)
}

fn lookup(cache: &IdentityCache, local_path: &Path) -> GameMetadataResult {
    use archivefs_core::identity_source::romm::enrichment::enrichment_metadata;

    match cache.record_for_path(local_path) {
        Some(record) if record.has_game_information() => {
            GameMetadataResult::Found(Box::new(enrichment_metadata(record)))
        }
        Some(_) | None => GameMetadataResult::NotFound,
    }
}

/// Opens the RomM identity cache exactly the way `gamer_artwork`'s cover
/// source does: read-only, no network. `None` covers every way this can
/// fail - not configured, never synced, unreadable - without
/// distinguishing them further; all of them mean the same thing to this
/// worker ([`GameMetadataResult::Unavailable`]).
fn open_cache() -> Option<IdentityCache> {
    use archivefs_core::identity_source::model::IdentityProvider;
    use archivefs_core::identity_source::settings::{SettingsLocation, default_identity_root};
    use archivefs_core::identity_source::status::IdentitySourceApi;

    let identity_root = default_identity_root().ok()?;
    // Confirms a provider is actually configured before touching the cache
    // file at all - an unconfigured provider and a missing cache file
    // should both quietly resolve to "no enrichment available", not two
    // different code paths that happen to produce the same result.
    SettingsLocation::new(&identity_root, IdentityProvider::Romm)
        .load()
        .ok()?;
    let api = IdentitySourceApi::new(&identity_root, IdentityProvider::Romm);
    api.open_cache(None).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- should_send_lookup: one in-flight request per path -------------

    #[test]
    fn a_fresh_path_with_nothing_in_flight_should_be_sent() {
        let in_flight = None;
        assert!(should_send_lookup(&in_flight, Path::new("/library/a.zip")));
    }

    #[test]
    fn the_same_path_already_in_flight_is_not_sent_again() {
        // The exact bug found in live validation: without this guard, every
        // frame between selection and reply re-enqueues the same Lookup.
        let in_flight = Some(PathBuf::from("/library/a.zip"));
        assert!(!should_send_lookup(&in_flight, Path::new("/library/a.zip")));
    }

    #[test]
    fn a_different_path_is_sent_even_while_another_is_in_flight() {
        // Selecting a new game while the previous lookup hasn't answered
        // yet must still ask about the new one - the guard is per-path,
        // not a blanket "one request at a time".
        let in_flight = Some(PathBuf::from("/library/a.zip"));
        assert!(should_send_lookup(&in_flight, Path::new("/library/b.zip")));
    }

    #[test]
    fn requesting_the_same_path_twice_only_enqueues_one_lookup() {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<WorkerMessage>();
        let (_reply_tx, reply_rx) = std::sync::mpsc::channel::<GameMetadataReply>();
        let mut worker = GameMetadataWorker {
            requests: request_tx,
            replies: reply_rx,
            in_flight: None,
        };
        let path = PathBuf::from("/library/a.zip");
        worker.request(&path);
        worker.request(&path);
        worker.request(&path);
        let mut received = 0;
        while request_rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(
            received, 1,
            "repeated requests for the same still-pending path must not \
             enqueue more than one Lookup"
        );
    }

    #[test]
    fn a_reply_clears_the_guard_so_the_same_path_can_be_requested_again() {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<WorkerMessage>();
        let (reply_tx, reply_rx) = std::sync::mpsc::channel::<GameMetadataReply>();
        let mut worker = GameMetadataWorker {
            requests: request_tx,
            replies: reply_rx,
            in_flight: None,
        };
        let path = PathBuf::from("/library/a.zip");
        worker.request(&path);
        reply_tx
            .send(GameMetadataReply {
                local_path: path.clone(),
                result: GameMetadataResult::NotFound,
            })
            .expect("reply channel is open");
        worker.poll();
        worker.request(&path);
        let mut received = 0;
        while request_rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(
            received, 2,
            "once a reply arrives, a later request for the same path is a \
             fresh request, not a duplicate"
        );
    }
}
