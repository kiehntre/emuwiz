//! Cover artwork for the Gamer View game list.
//!
//! # Why this exists separately from the Details panel's cover
//!
//! The Details panel loads exactly one cover, on an explicit button press, through
//! [`RommOperation`](crate::RommOperation). That queue holds one operation at a
//! time and *drops* anything asked for while something else runs
//! (`start_romm_operation` returns `false`), because it also carries imports and
//! hash verifications, which must not overlap. A scrolling list of games needs many
//! covers at once and must never block a mount behind a thumbnail, so it cannot use
//! that queue - which is precisely why Gamer View drew platform icons and never a
//! RomM cover.
//!
//! Nothing here is a second downloader or a second image cache. Fetching,
//! validation, thumbnailing, bounding and on-disk storage all remain
//! [`archivefs_core::identity_source::artwork::ArtworkCache`]'s job, and decoding
//! remains [`crate::romm_game::decode_thumbnail`]'s. What this module adds is the
//! scheduling a list needs: which records to ask about, in what order, how many at
//! once, and how to hold the answers so scrolling back does not ask again.
//!
//! # Approved artwork references
//!
//! The core artwork cache remains the authority for fetch policy. The GUI only
//! schedules records whose artwork is either hosted by RomM or carries the exact
//! LaunchBox reference shape approved by the core. Other public scraper URLs stay
//! provenance-only and are never handed to the worker.
//!
//! # Answers belong to records, not to rows
//!
//! `egui`'s `show_rows` reuses row positions as the list scrolls, so anything
//! stored per row index would eventually be drawn beside a different game. Every
//! answer here is keyed by the record's own local path and carries the
//! `provider_game_id` it was resolved for; a row draws a cover only when both match
//! the record it is drawing. A reply for a library generation that has since been
//! replaced is discarded rather than stored.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(debug_assertions)]
use std::time::Instant;

#[cfg(debug_assertions)]
mod cover_timing {
    use super::{GamerArtworkKind, HashMap, Instant, Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    static ENQUEUED: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

    fn enabled() -> bool {
        std::env::var_os("EMUWIZ_COVER_TIMING").is_some()
    }

    fn key(generation: u64, path: &Path, kind: GamerArtworkKind) -> String {
        format!("{generation}:{}:{kind:?}", path.display())
    }

    fn name(path: &Path) -> &str {
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("<unnamed>")
    }

    pub(super) fn enqueued(generation: u64, path: &Path, kind: GamerArtworkKind) {
        if !enabled() {
            return;
        }
        let pending = ENQUEUED.get_or_init(|| Mutex::new(HashMap::new()));
        let Ok(mut pending) = pending.lock() else {
            return;
        };
        if pending.len() >= 1024 {
            pending.clear();
        }
        pending.insert(key(generation, path, kind), Instant::now());
    }

    pub(super) fn worker_started(
        generation: u64,
        path: &Path,
        kind: GamerArtworkKind,
    ) -> Option<Instant> {
        if !enabled() {
            return None;
        }
        ENQUEUED
            .get()
            .and_then(|pending| pending.lock().ok())
            .and_then(|mut pending| pending.remove(&key(generation, path, kind)))
    }

    pub(super) fn stage(path: &Path, kind: GamerArtworkKind, label: &str, started: Instant) {
        if enabled() {
            eprintln!(
                "[cover-timing] file={} kind={kind:?} stage={label} ms={:.1}",
                name(path),
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    pub(super) fn completed(
        generation: u64,
        path: &PathBuf,
        kind: GamerArtworkKind,
        queued: Option<Instant>,
        started: Instant,
        answer: &str,
    ) {
        if !enabled() {
            return;
        }
        let queue_ms = queued
            .map(|time| started.duration_since(time).as_secs_f64() * 1000.0)
            .unwrap_or_default();
        eprintln!(
            "[cover-timing] generation={generation} file={} kind={kind:?} answer={answer} enqueue_to_worker_ms={queue_ms:.1} worker_ms={:.1}",
            name(path),
            started.elapsed().as_secs_f64() * 1000.0
        );
    }

    pub(super) fn upload(path: &Path, kind: GamerArtworkKind, started: Instant) {
        stage(path, kind, "texture_upload", started);
    }
}

use eframe::egui;

use crate::ui::theme;

/// How many rows beyond the visible window are asked for, on each side.
///
/// Enough that a steady scroll usually finds the next cover already decoded, small
/// enough that a flick through a 13,891-record library asks about tens of records
/// rather than thousands.
pub(crate) const LOOK_AHEAD_ROWS: usize = 6;

/// The most new records one frame may ask about.
///
/// A jump to the far end of the list would otherwise queue the whole newly visible
/// window at once. The remainder is asked for on the following frames, which is
/// still faster than a person can read them.
pub(crate) const MAX_REQUESTS_PER_FRAME: usize = 6;

/// How many answers are held in memory at once.
///
/// Bounds texture memory, and is the reason a large library cannot accumulate work
/// or pixels for every record. Well above any window a person can see, so scrolling
/// away and back within a screenful or two never costs a second request.
pub(crate) const MAX_TRACKED_COVERS: usize = 256;

/// The box a row's cover is drawn inside, matching the existing artwork slot so
/// adding covers changes no row's height.
pub(crate) const COVER_BOX: f32 = 56.0;

/// The most selected-game requests one frame may emit.
///
/// One, because there is only ever one selected game - and stating it as a ceiling
/// is what makes the fairness argument checkable: a burst of selection changes can
/// take at most one of the frame's [`MAX_REQUESTS_PER_FRAME`] slots, so visible
/// rows always keep the rest.
pub(crate) const MAX_SELECTED_REQUESTS_PER_FRAME: usize = 1;

/// How many higher-priority jobs the worker may serve before it must take a lower
/// one.
///
/// Priority alone is not fairness: a selection the user keeps changing would sit at
/// the head of the queue forever and the visible rows behind it would never be
/// read. After this many, the oldest lower-priority job goes next regardless.
pub(crate) const FAIRNESS_RUN: u32 = 4;

/// The most jobs the worker will hold. Beyond it the lowest-priority, oldest work
/// is dropped - it describes rows that are no longer on screen, and the UI re-asks
/// for anything it still wants on the next frame it draws.
pub(crate) const MAX_QUEUED_JOBS: usize = 256;

/// What a cover request is for, in the order it deserves to be served.
///
/// The selected game is what a person is looking at, so it goes first even when a
/// long look-ahead backlog is already queued - which FIFO alone would not give.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CoverPriority {
    Selected,
    Visible,
    LookAhead,
}

/// The approved RomM media kind requested by the shared artwork worker.
/// Screenshots use the same origin validation, response limits, decode checks,
/// and on-disk cache as covers; the index is the screenshot's provider order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GamerArtworkKind {
    Cover,
    Screenshot(usize),
}

/// Why a row has no cover to draw.
///
/// Every variant draws the same placeholder; the distinction exists so the reason
/// can be stated in a tooltip rather than leaving a person guessing, and so tests
/// can tell "RomM has no artwork for this" apart from "this failed to load".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoCover {
    /// The file is not in the imported RomM catalogue at all.
    NoRommIdentity,
    /// RomM has the record but recorded no artwork for it.
    NoArtwork,
    /// RomM recorded only a scraper URL on a public host. Never fetched.
    PublicOnly,
    /// Nothing cached, and RomM could not be reached or is not configured.
    Unavailable,
    /// A request or a decode was refused or failed.
    Failed,
}

impl NoCover {
    /// What a person is told when they hover the placeholder. Never carries a URL,
    /// a path or a token, matching the core's own wording rules.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::NoRommIdentity => "No RomM identity for this file, so no cover is available.",
            Self::NoArtwork => "RomM recorded no artwork for this game.",
            Self::PublicOnly => {
                "Public artwork reference recorded, but EmuWiz does not fetch from public hosts."
            }
            Self::Unavailable => "No cached cover, and RomM was not reachable.",
            Self::Failed => "The cover could not be loaded.",
        }
    }
}

/// What resolving one record produced.
#[derive(Clone, Debug)]
pub(crate) enum CoverAnswer {
    /// Decoded pixels, ready for the UI thread to upload. Decoding happens on the
    /// worker: the upload is cheap, the decode is not.
    Ready(Box<crate::romm_game::CoverImage>),
    /// The record still resolves to the cover the caller already holds decoded, so
    /// nothing was read and nothing was decoded.
    ///
    /// This is what makes an identity refresh cheap. The key is a digest of the
    /// server, the `provider_game_id` and RomM's own artwork identity, so a key that
    /// still matches *is* proof that both the record's identity and its artwork are
    /// unchanged - there is no way to answer `Unchanged` for a record whose provider
    /// id moved.
    Unchanged {
        key: String,
    },
    None(NoCover),
}

/// One record to resolve, and what the caller already has for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoverJob {
    pub(crate) local_path: PathBuf,
    pub(crate) priority: CoverPriority,
    pub(crate) kind: GamerArtworkKind,
    /// The cover key already decoded for this record, when one is being
    /// revalidated after an identity refresh. Lets the worker answer
    /// [`CoverAnswer::Unchanged`] instead of reading and decoding the thumbnail
    /// again.
    pub(crate) held_key: Option<String>,
}

/// One answer, bound to what asked for it.
///
/// `generation` and `provider_game_id` are what make a late answer safe: an answer
/// from a replaced library is dropped, and an answer is only ever drawn beside the
/// record whose id it names.
#[derive(Clone, Debug)]
pub(crate) struct CoverReply {
    pub(crate) generation: u64,
    pub(crate) local_path: PathBuf,
    /// The RomM record this answers for, or `None` when the path has no RomM
    /// identity at all.
    pub(crate) provider_game_id: Option<String>,
    pub(crate) kind: GamerArtworkKind,
    /// The exact screenshot count from the matched external record. It is
    /// present for screenshot replies so the Details view can stop probing.
    pub(crate) screenshot_count: Option<usize>,
    pub(crate) answer: CoverAnswer,
}

/// Delivery state projected by the core resolver for a selected record. This is
/// separate from decoded artwork so the UI can represent provider-level pending
/// work before a concrete image answer arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MediaDeliveryUpdate {
    pub(crate) generation: u64,
    pub(crate) local_path: PathBuf,
    pub(crate) cover: archivefs_core::identity_source::media_resolver::MediaDelivery,
    pub(crate) screenshots: archivefs_core::identity_source::media_resolver::MediaDelivery,
}

/// What one row's cover area is showing.
#[derive(Clone)]
pub(crate) enum CoverSlot {
    /// Asked for, not yet answered. The placeholder is drawn meanwhile, so the row
    /// never changes height when the answer arrives.
    Loading,
    Ready {
        texture: egui::TextureHandle,
        provider_game_id: String,
        /// The artwork cache key these pixels came from, offered back to the worker
        /// on revalidation so an unchanged record costs no decode.
        key: String,
    },
    /// Decoded before an identity refresh, not yet confirmed against the new
    /// catalogue.
    ///
    /// The texture is kept - that is what makes an unchanged record free - but the
    /// *placeholder* is drawn until the refreshed catalogue confirms this path still
    /// resolves to the same record. Drawing the held cover meanwhile is exactly the
    /// thing that would show one game's art beside another after a re-import.
    Revalidating {
        texture: egui::TextureHandle,
        provider_game_id: String,
        key: String,
        /// Whether the worker has already been asked to confirm this. Without it a
        /// revalidating row would be re-asked on every frame until its reply
        /// arrived, which is a request per frame per visible row.
        requested: bool,
    },
    None(NoCover),
}

/// Gamer View's scheduling and holding of covers.
///
/// Deliberately free of threads and of any network or disk access: it decides what
/// to ask for and what to keep, and is driven entirely through [`Self::visible`]
/// and [`Self::absorb`]. That is what lets every rule it enforces be tested without
/// a RomM instance.
#[derive(Default)]
pub(crate) struct GamerCoverCache {
    /// Bumped when the loaded library is replaced. Answers naming an older
    /// generation are discarded: the same path may now be a different file.
    generation: u64,
    slots: HashMap<PathBuf, CoverSlot>,
    delivery: HashMap<PathBuf, archivefs_core::identity_source::media_resolver::MediaDelivery>,
    /// The frame each slot was last drawn or asked for, for eviction order.
    last_used: HashMap<PathBuf, u64>,
    frame: u64,
}

impl GamerCoverCache {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Points the cache at a replaced library, discarding everything about the old
    /// one.
    ///
    /// Paths are the identity a slot is keyed by, and a reloaded library may map the
    /// same path to a different archive, so every slot and every texture goes. This
    /// is the only thing that clears the cache: a search or a platform change
    /// narrows which records are *visible* without changing what any of them is, so
    /// throwing their covers away would only cause the same covers to be fetched
    /// again.
    pub(crate) fn library_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.slots.clear();
        self.delivery.clear();
        self.last_used.clear();
    }

    /// What is drawn for one record right now, or `None` when nothing has been asked
    /// for it yet.
    ///
    /// A `Ready` slot is returned only when it names the record being drawn. That
    /// check is what makes a reused row position unable to inherit the previous
    /// occupant's cover.
    pub(crate) fn slot_for(
        &self,
        local_path: &Path,
        provider_game_id: Option<&str>,
    ) -> Option<&CoverSlot> {
        match self.slots.get(local_path)? {
            CoverSlot::Ready {
                provider_game_id: held,
                ..
            } if provider_game_id.is_some_and(|wanted| wanted != held) => None,
            slot => Some(slot),
        }
    }

    /// Points the cache at a refreshed RomM catalogue.
    ///
    /// Called when an import or a cache replacement succeeded, so what any path
    /// resolves to may have changed. Unlike [`Self::library_changed`] this keeps the
    /// decoded textures: an import usually leaves most records exactly as they were,
    /// and re-decoding thousands of thumbnails to discover that would be waste.
    ///
    /// What it does not keep is the *binding*. Every ready slot becomes
    /// [`CoverSlot::Revalidating`], which draws the placeholder until the new
    /// catalogue confirms the record, so a path whose provider id moved cannot show
    /// the previous record's cover even for a frame. Everything else is dropped so
    /// it is asked again - which is what lets a game that has just gained a RomM
    /// identity acquire artwork without a restart.
    ///
    /// The generation bump discards replies already in flight against the old
    /// catalogue.
    pub(crate) fn identity_refreshed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.delivery.clear();
        let mut retained = HashMap::with_capacity(self.slots.len());
        for (path, slot) in self.slots.drain() {
            match slot {
                CoverSlot::Ready {
                    texture,
                    provider_game_id,
                    key,
                }
                | CoverSlot::Revalidating {
                    texture,
                    provider_game_id,
                    key,
                    ..
                } => {
                    retained.insert(
                        path,
                        CoverSlot::Revalidating {
                            texture,
                            provider_game_id,
                            key,
                            // A second import supersedes the first, so anything
                            // already asked is asked again against the newer
                            // catalogue.
                            requested: false,
                        },
                    );
                }
                // A pending request and a "no cover" answer both described the old
                // catalogue. Dropping them is what makes a newly matched record
                // eligible on the very next frame it is visible.
                CoverSlot::Loading | CoverSlot::None(_) => {}
            }
        }
        self.slots = retained;
    }

    pub(crate) fn absorb_delivery(&mut self, update: &MediaDeliveryUpdate) -> bool {
        if update.generation != self.generation {
            return false;
        }
        self.delivery
            .insert(update.local_path.clone(), update.cover);
        true
    }

    pub(crate) fn delivery(
        &self,
        path: &Path,
    ) -> Option<archivefs_core::identity_source::media_resolver::MediaDelivery> {
        self.delivery.get(path).copied()
    }

    /// Declares the window of records on screen and returns the ones to ask about.
    ///
    /// `visible` is the rows the list is actually drawing; `look_ahead` extends it on
    /// both sides. Anything already asked for or already answered is not asked again,
    /// which is what makes scrolling away and back free. At most
    /// [`MAX_REQUESTS_PER_FRAME`] records are returned, so no single frame can queue
    /// a whole library.
    ///
    /// The returned paths are marked [`CoverSlot::Loading`] before they are handed
    /// out, so the same record cannot be asked for twice while an answer is in
    /// flight.
    pub(crate) fn visible(
        &mut self,
        selected: Option<&Path>,
        window: &[PathBuf],
        look_ahead: &[PathBuf],
    ) -> Vec<CoverJob> {
        self.frame = self.frame.wrapping_add(1);
        for path in selected
            .into_iter()
            .chain(window.iter().map(PathBuf::as_path))
        {
            self.last_used.insert(path.to_path_buf(), self.frame);
        }
        for path in look_ahead {
            self.last_used.insert(path.clone(), self.frame);
        }

        let mut wanted: Vec<CoverJob> = Vec::new();
        // Three passes in the order the results are wanted on screen. The selected
        // game is what a person is actually looking at, so it is asked for first
        // even when it is also an ordinary visible row; the look-ahead is asked for
        // last, and only with whatever is left of the frame's budget.
        let mut consider = |cache: &mut Self, path: &Path, priority: CoverPriority| {
            if wanted.len() >= MAX_REQUESTS_PER_FRAME {
                return;
            }
            if priority == CoverPriority::Selected
                && wanted
                    .iter()
                    .filter(|job| job.priority == CoverPriority::Selected)
                    .count()
                    >= MAX_SELECTED_REQUESTS_PER_FRAME
            {
                return;
            }
            if wanted.iter().any(|job| job.local_path == path) {
                return;
            }
            let held_key = match cache.slots.get(path) {
                // Already asked, or already answered. Nothing to do.
                Some(CoverSlot::Loading | CoverSlot::Ready { .. } | CoverSlot::None(_)) => return,
                // Already asked to confirm; waiting on the answer.
                Some(CoverSlot::Revalidating {
                    requested: true, ..
                }) => return,
                // Waiting on the refreshed catalogue. Offer the key it already
                // holds, so an unchanged record is confirmed without a decode.
                Some(CoverSlot::Revalidating { key, .. }) => Some(key.clone()),
                None => None,
            };
            wanted.push(CoverJob {
                local_path: path.to_path_buf(),
                priority,
                kind: GamerArtworkKind::Cover,
                held_key,
            });
        };

        if let Some(path) = selected {
            consider(self, path, CoverPriority::Selected);
        }
        for path in window {
            consider(self, path, CoverPriority::Visible);
        }
        for path in look_ahead {
            consider(self, path, CoverPriority::LookAhead);
        }

        for job in &wanted {
            match self.slots.get_mut(&job.local_path) {
                // Marked as asked, keeping the texture that makes an unchanged
                // record free to confirm. Replacing it with `Loading` would throw
                // those pixels away.
                Some(CoverSlot::Revalidating { requested, .. }) => *requested = true,
                Some(_) => {}
                None => {
                    self.slots
                        .insert(job.local_path.clone(), CoverSlot::Loading);
                }
            }
        }
        self.evict();
        wanted
    }

    /// Takes one answer, uploading its pixels if it is still wanted.
    ///
    /// Returns whether the answer was kept. A stale generation is dropped here, and
    /// so is an answer for a record no longer tracked - both would otherwise hold a
    /// texture nothing will ever draw.
    pub(crate) fn absorb(&mut self, context: &egui::Context, reply: CoverReply) -> bool {
        if !matches!(reply.kind, GamerArtworkKind::Cover) {
            return false;
        }
        if reply.generation != self.generation {
            return false;
        }
        if !self.slots.contains_key(&reply.local_path) {
            // Evicted while in flight. Keeping it would reintroduce an entry the
            // bound has already decided not to hold.
            return false;
        }
        let slot = match reply.answer {
            CoverAnswer::None(reason) => CoverSlot::None(reason),
            CoverAnswer::Unchanged { key } => {
                // The refreshed catalogue resolves this path to the same cover key,
                // which by construction means the same record and the same artwork.
                // The texture already held is promoted back to visible without a
                // read, a decode or an upload.
                let Some(CoverSlot::Revalidating {
                    texture,
                    provider_game_id,
                    key: held,
                    ..
                }) = self.slots.get(&reply.local_path)
                else {
                    // Nothing left to confirm - evicted, or already superseded.
                    return false;
                };
                if held != &key {
                    // Should not happen: the worker only answers `Unchanged` for the
                    // key it was offered. Refusing is the safe reading either way.
                    return false;
                }
                CoverSlot::Ready {
                    texture: texture.clone(),
                    provider_game_id: provider_game_id.clone(),
                    key,
                }
            }
            CoverAnswer::Ready(image) => {
                let Some(provider_game_id) = reply.provider_game_id else {
                    // A cover with no record to attach it to cannot be drawn safely.
                    return false;
                };
                #[cfg(debug_assertions)]
                cover_timing::upload(&reply.local_path, reply.kind, Instant::now());
                CoverSlot::Ready {
                    texture: context.load_texture(
                        format!("archivefs-gamer-cover-{}", image.key),
                        image.image.clone(),
                        egui::TextureOptions::LINEAR,
                    ),
                    provider_game_id,
                    key: image.key.clone(),
                }
            }
        };
        self.slots.insert(reply.local_path, slot);
        true
    }

    /// Drops the least recently seen slots once the bound is exceeded.
    fn evict(&mut self) {
        if self.slots.len() <= MAX_TRACKED_COVERS {
            return;
        }
        let mut order: Vec<(u64, PathBuf)> = self
            .slots
            .keys()
            .map(|path| (self.last_used.get(path).copied().unwrap_or(0), path.clone()))
            .collect();
        order.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        let excess = self.slots.len() - MAX_TRACKED_COVERS;
        for (_, path) in order.into_iter().take(excess) {
            self.slots.remove(&path);
            self.last_used.remove(&path);
        }
    }

    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.slots.len()
    }
}

/// Selected-Details screenshot state. It is intentionally separate from the
/// library cover slots so a screenshot can never occupy or evict a cover slot.
/// The worker and core ArtworkCache remain shared with covers.
#[derive(Default)]
pub(crate) struct GamerScreenshotCache {
    generation: u64,
    screenshot_count: HashMap<PathBuf, usize>,
    slots: HashMap<(PathBuf, usize), CoverSlot>,
    delivery: HashMap<PathBuf, archivefs_core::identity_source::media_resolver::MediaDelivery>,
}

pub(crate) const MAX_DETAILS_SCREENSHOTS: usize = 5;
const MAX_TRACKED_SCREENSHOTS: usize = 32;

impl GamerScreenshotCache {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn library_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.screenshot_count.clear();
        self.slots.clear();
        self.delivery.clear();
    }

    /// Refresh provider bindings without blanking an already-visible gallery.
    /// Ready slots remain available; obsolete in-flight slots are released so
    /// the current generation can request them again from the refreshed
    /// provider snapshot.
    pub(crate) fn identity_refreshed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.delivery.clear();
        self.slots
            .retain(|_, slot| matches!(slot, CoverSlot::Ready { .. }));
    }

    pub(crate) fn absorb_delivery(&mut self, update: &MediaDeliveryUpdate) -> bool {
        if update.generation != self.generation {
            return false;
        }
        self.delivery
            .insert(update.local_path.clone(), update.screenshots);
        true
    }

    pub(crate) fn delivery(
        &self,
        path: &Path,
    ) -> Option<archivefs_core::identity_source::media_resolver::MediaDelivery> {
        self.delivery.get(path).copied()
    }

    pub(crate) fn screenshot_count(&self, path: &Path) -> Option<usize> {
        self.screenshot_count.get(path).copied()
    }

    /// Returns the bounded number of screenshot cells the Details view should
    /// reserve, including the initial probe while RomM's screenshot count is
    /// still being resolved. A confirmed zero is retained as an intentional
    /// empty state so a completed cover request cannot make the section vanish.
    pub(crate) fn section_count(&self, path: &Path) -> Option<usize> {
        let count = self
            .screenshot_count(path)
            .map_or(1, |count| count.min(MAX_DETAILS_SCREENSHOTS));
        let pending = self.delivery(path)
            == Some(archivefs_core::identity_source::media_resolver::MediaDelivery::RemotePending);
        if count == 0 && !pending {
            return Some(0);
        }
        let count = if pending { count.max(1) } else { count };
        if pending
            && !(0..count).any(|index| {
                matches!(
                    self.slot_for(path, index),
                    Some(
                        CoverSlot::Loading
                            | CoverSlot::Ready { .. }
                            | CoverSlot::Revalidating { .. }
                    )
                )
            })
        {
            return Some(count);
        }
        (0..count)
            .any(|index| {
                matches!(
                    self.slot_for(path, index),
                    Some(
                        CoverSlot::Loading
                            | CoverSlot::Ready { .. }
                            | CoverSlot::Revalidating { .. }
                    )
                )
            })
            .then_some(count)
    }

    pub(crate) fn has_loading(&self, path: &Path) -> bool {
        self.slots.iter().any(|((slot_path, _), slot)| {
            slot_path == path && matches!(slot, CoverSlot::Loading | CoverSlot::Revalidating { .. })
        })
    }

    pub(crate) fn ready_count(&self, path: &Path) -> usize {
        self.slots
            .iter()
            .filter(|((slot_path, _), slot)| {
                slot_path == path && matches!(slot, CoverSlot::Ready { .. })
            })
            .count()
    }

    /// Requests only the selected game's first screenshot until the imported
    /// record tells us how many real references exist, then requests the bounded
    /// visible subset in provider order.
    pub(crate) fn visible(&mut self, path: &Path) -> Vec<CoverJob> {
        let count = self
            .screenshot_count(path)
            .map_or(1, |count| count.min(MAX_DETAILS_SCREENSHOTS));
        (0..count)
            .filter_map(|index| {
                let key = (path.to_path_buf(), index);
                if self.slots.contains_key(&key) {
                    return None;
                }
                self.slots.insert(key, CoverSlot::Loading);
                self.evict();
                Some(CoverJob {
                    local_path: path.to_path_buf(),
                    priority: CoverPriority::Selected,
                    kind: GamerArtworkKind::Screenshot(index),
                    held_key: None,
                })
            })
            .collect()
    }

    pub(crate) fn slot_for(&self, path: &Path, index: usize) -> Option<&CoverSlot> {
        self.slots.get(&(path.to_path_buf(), index))
    }

    pub(crate) fn absorb(&mut self, context: &egui::Context, reply: CoverReply) -> bool {
        let GamerArtworkKind::Screenshot(index) = reply.kind else {
            return false;
        };
        if reply.generation != self.generation {
            return false;
        }
        let key = (reply.local_path.clone(), index);
        if !self.slots.contains_key(&key) {
            return false;
        }
        if let Some(count) = reply.screenshot_count {
            self.screenshot_count
                .insert(reply.local_path.clone(), count);
        }
        let slot = match reply.answer {
            CoverAnswer::Ready(image) => {
                let texture = context.load_texture(
                    format!("gamer-screenshot-{}-{index}", image.key),
                    image.image,
                    egui::TextureOptions::LINEAR,
                );
                CoverSlot::Ready {
                    texture,
                    provider_game_id: reply.provider_game_id.unwrap_or_default(),
                    key: image.key.clone(),
                }
            }
            CoverAnswer::Unchanged { .. } => CoverSlot::None(NoCover::Failed),
            CoverAnswer::None(reason) => CoverSlot::None(reason),
        };
        self.slots.insert(key, slot);
        self.evict();
        context.request_repaint();
        true
    }

    fn evict(&mut self) {
        while self.slots.len() > MAX_TRACKED_SCREENSHOTS {
            let Some(key) = self.slots.keys().next().cloned() else {
                break;
            };
            self.slots.remove(&key);
        }
    }

    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.slots.len()
    }
}

// --- The featured cover -------------------------------------------------

/// How wide the featured panel's content column is allowed to get.
///
/// The panel itself is around 730px at 1920x1080, and a title, a status line and a
/// Mount button stretched across all of it read as a form rather than a feature.
/// Constraining the column keeps the block cohesive and keeps Mount emphatic
/// without becoming a banner.
pub(crate) const GAMER_FEATURED_CONTENT_MAX_WIDTH: f32 = 560.0;

/// The tallest a *real* cover (an actual box-art image, not a fallback) is
/// drawn, so a 1440p+ panel does not turn one loaded cover into a wall
/// poster. Real covers get a taller ceiling than the fallback plate - see
/// [`FEATURED_COVER_MAX_HEIGHT_FALLBACK`] - since a genuine cover is the
/// presentation the hero exists to show off, and is worth letting grow
/// further on a roomy window.
pub(crate) const FEATURED_COVER_MAX_HEIGHT: f32 = 460.0;

/// The tallest the *fallback* platform-art plate (drawn while a cover is
/// still loading, has none, or none exists for this platform) is drawn.
/// Deliberately kept below [`FEATURED_COVER_MAX_HEIGHT`]: hardware
/// iconography was framed at a specific, modest scale, and blowing it up to
/// match a real cover's ceiling would make placeholder art read as more
/// important than it is.
pub(crate) const FEATURED_COVER_MAX_HEIGHT_FALLBACK: f32 = 300.0;

/// Below this there is not enough of an image left to be worth the space, and the
/// artwork is dropped rather than the actions.
pub(crate) const FEATURED_COVER_MIN_HEIGHT: f32 = 72.0;

/// The first-frame estimate of what the title, status and actions need beneath the
/// artwork.
///
/// Only an estimate, and only until the panel has drawn once: from then on the
/// caller measures the block and reserves its real height. That is what makes
/// "reduce the artwork before hiding the actions" true rather than hoped for - the
/// cover only ever gets what is genuinely left over, however the title wrapped.
pub(crate) const FEATURED_RESERVED_BELOW: f32 = 300.0;

/// A *real* cover's portrait shape - width:height. Slightly wider than the
/// fallback's (3:4 rather than 2:3) so a loaded cover reads as filling its
/// column instead of a narrow strip in a wide gutter, while still reading as
/// an unmistakably portrait box-art shape.
pub(crate) const FEATURED_COVER_ASPECT: f32 = 3.0 / 4.0;

/// The *fallback* platform-art plate's portrait shape - narrower than the
/// real-cover ratio, preserving the tighter framing hardware/platform glyphs
/// were designed around.
pub(crate) const FEATURED_COVER_ASPECT_FALLBACK: f32 = 2.0 / 3.0;

/// The box reserved for the featured cover, or `None` when there is not enough
/// height to give it any.
///
/// `budget` is the height left for the artwork *after* the caller has set aside
/// what the title, the status and the actions need - measured, not estimated. The
/// subtraction belongs there rather than here: doing it in both places takes it
/// twice, which shrinks the cover on a large window and hides it on a small one.
///
/// `is_real_cover` selects between the taller, wider real-cover budget and the
/// more restrained fallback one (see [`FEATURED_COVER_ASPECT`],
/// [`FEATURED_COVER_ASPECT_FALLBACK`], [`FEATURED_COVER_MAX_HEIGHT`] and
/// [`FEATURED_COVER_MAX_HEIGHT_FALLBACK`]) - a loaded cover and the
/// placeholder plate are never the same size on a window with room to spare.
///
/// The box is the same size across every frame a *given* cover state persists,
/// which is what stops the actions beneath it moving from frame to frame.
pub(crate) fn featured_cover_box(
    panel_width: f32,
    budget: f32,
    is_real_cover: bool,
) -> Option<egui::Vec2> {
    let (aspect, max_height) = if is_real_cover {
        (FEATURED_COVER_ASPECT, FEATURED_COVER_MAX_HEIGHT)
    } else {
        (
            FEATURED_COVER_ASPECT_FALLBACK,
            FEATURED_COVER_MAX_HEIGHT_FALLBACK,
        )
    };
    // Capped so a 1440p panel does not turn one thumbnail into a poster, and
    // clamped to fit across the panel at its portrait ratio.
    let by_width = (panel_width - 2.0 * theme::PAGE_GUTTER).max(0.0) / aspect;
    let height = budget.min(by_width).min(max_height);
    if height < FEATURED_COVER_MIN_HEIGHT {
        return None;
    }
    Some(egui::vec2(height * aspect, height))
}

/// Fits an image inside the reserved box, preserving its aspect ratio.
///
/// A portrait cover fills the box's height; a landscape or unusually shaped one
/// fills its width and is letterboxed top and bottom. Nothing is ever stretched,
/// and nothing is cropped away.
pub(crate) fn fit_within(box_size: egui::Vec2, image: egui::Vec2) -> egui::Vec2 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return egui::Vec2::ZERO;
    }
    let scale = (box_size.x / image.x).min(box_size.y / image.y);
    image * scale
}

/// Extends a visible row range with look-ahead, clamped to the list.
///
/// Kept separate from the list rendering so the bound can be asserted directly:
/// this is what stops a 13,891-record library from being asked about.
pub(crate) fn look_ahead_range(
    visible: std::ops::Range<usize>,
    total: usize,
) -> std::ops::Range<usize> {
    let start = visible.start.saturating_sub(LOOK_AHEAD_ROWS);
    let end = visible.end.saturating_add(LOOK_AHEAD_ROWS).min(total);
    start..end.max(start)
}

// --- Resolving one record ------------------------------------------------

/// What may be done for one record, decided before anything is asked for.
///
/// Split out of [`RommCoverSource::resolve`] so the rule that matters most can be
/// asserted without a RomM instance: a record whose only artwork is a public
/// scraper URL has no plan that reaches a fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoverPlan {
    /// Look in the shared cache, then request an artwork reference that the core
    /// resolver has a policy for (RomM-hosted or approved LaunchBox).
    UseRommHostedCover,
    /// Draw the placeholder. No request is made.
    Placeholder(NoCover),
}

/// Decides what one record allows.
///
/// Delegates the artwork question to [`crate::romm_game::availability_of`], the
/// same classification the Details panel and the record browser use, so the three
/// cannot drift apart about what counts as fetchable.
pub(crate) fn plan_for(
    record: &archivefs_core::identity_source::model::ExternalIdentityRecord,
) -> CoverPlan {
    match crate::romm_game::availability_of(record) {
        crate::romm_game::ArtworkAvailability::Fetchable => CoverPlan::UseRommHostedCover,
        crate::romm_game::ArtworkAvailability::None => CoverPlan::Placeholder(NoCover::NoArtwork),
        crate::romm_game::ArtworkAvailability::PublicOnly
            if record
                .artwork
                .as_ref()
                .is_some_and(|artwork| is_approved_launchbox_reference(&artwork.reference)) =>
        {
            CoverPlan::UseRommHostedCover
        }
        crate::romm_game::ArtworkAvailability::PublicOnly => {
            CoverPlan::Placeholder(NoCover::PublicOnly)
        }
    }
}

/// The core cache performs the complete URL validation before any request. This
/// narrow scheduling check keeps unrelated public scraper URLs out of the worker
/// while allowing the one approved external media source through the same cache.
fn is_approved_launchbox_reference(reference: &str) -> bool {
    reference.starts_with("https://images.launchbox-app.com/")
}

/// Projects the already-imported RomM artwork record into the core resolver's
/// provider snapshots. This is intentionally a pure projection: it does not
/// refresh RomM, touch ES-DE, or perform any filesystem/network work.
fn resolver_input_for_record(
    record: &archivefs_core::identity_source::model::ExternalIdentityRecord,
    selection_generation: u64,
    esde: Option<&archivefs_core::emulator_environment::es_de_metadata::EsDeResolvedEntry>,
    provider_generation: u64,
    launchbox_index: Option<
        &archivefs_core::identity_source::launchbox_local::LaunchBoxLocalProviderIndex,
    >,
) -> archivefs_core::identity_source::media_resolver::MediaResolverInput {
    use archivefs_core::identity_source::media_resolver::{
        MediaDelivery, MediaProvider, MediaResolverInput, ProviderMediaSnapshot,
    };
    use archivefs_core::identity_source::model::MediaReference;

    let mut romm = ProviderMediaSnapshot::new(MediaProvider::RommDirect);
    let mut launchbox = ProviderMediaSnapshot::new(MediaProvider::LaunchBoxViaRomm);
    romm.delivery = MediaDelivery::RemoteReady;
    launchbox.delivery = MediaDelivery::RemoteReady;
    if let Some(artwork) = record.artwork.as_ref() {
        let cover = MediaReference {
            hosted_reference: artwork
                .small_reference
                .clone()
                .or_else(|| artwork.large_reference.clone()),
            public_reference: Some(artwork.reference.clone()),
        };
        if is_approved_launchbox_reference(&artwork.reference) {
            launchbox.cover = Some(cover);
        } else if cover.hosted_reference.is_some() {
            romm.cover = Some(cover);
        }
        for screenshot in &artwork.screenshots {
            if screenshot.hosted_reference.is_some() {
                romm.screenshots.push(screenshot.clone());
            } else if screenshot
                .public_reference
                .as_deref()
                .is_some_and(is_approved_launchbox_reference)
            {
                launchbox.screenshots.push(screenshot.clone());
            }
        }
    }
    if let Some(esde) = esde {
        let mut local = ProviderMediaSnapshot::new(MediaProvider::EsDe);
        local.delivery = MediaDelivery::LocalReady;
        if esde
            .media
            .cover
            .is_some_and(|media| media.exists && media.readable)
        {
            local.cover = esde.entry.media.cover.as_ref().map(|path| MediaReference {
                hosted_reference: Some(path.to_string_lossy().into_owned()),
                public_reference: None,
            });
        }
        if esde
            .media
            .screenshot
            .is_some_and(|media| media.exists && media.readable)
        {
            if let Some(path) = esde.entry.media.screenshot.as_ref() {
                local.screenshots.push(MediaReference {
                    hosted_reference: Some(path.to_string_lossy().into_owned()),
                    public_reference: None,
                });
            }
        }
        if esde
            .media
            .video
            .is_some_and(|media| media.exists && media.readable)
        {
            local.video = esde.entry.media.video.as_ref().map(|path| MediaReference {
                hosted_reference: Some(path.to_string_lossy().into_owned()),
                public_reference: None,
            });
        }
        let local_launchbox = launchbox_snapshot(record, launchbox_index);
        return MediaResolverInput {
            providers: [Some(romm), Some(launchbox), Some(local), local_launchbox]
                .into_iter()
                .flatten()
                .collect(),
            pending_providers: Vec::new(),
            failed_providers: Vec::new(),
            selection_generation,
            provider_generation,
        };
    }
    let local_launchbox = launchbox_snapshot(record, launchbox_index);
    MediaResolverInput {
        providers: [Some(romm), Some(launchbox), local_launchbox]
            .into_iter()
            .flatten()
            .collect(),
        pending_providers: Vec::new(),
        failed_providers: Vec::new(),
        selection_generation,
        provider_generation,
    }
}

fn launchbox_snapshot<'a>(
    record: &archivefs_core::identity_source::model::ExternalIdentityRecord,
    index: Option<
        &'a archivefs_core::identity_source::launchbox_local::LaunchBoxLocalProviderIndex,
    >,
) -> Option<archivefs_core::identity_source::media_resolver::ProviderMediaSnapshot> {
    let index = index?;
    let launchbox_id = record
        .metadata_provider_ids
        .iter()
        .find(|id| id.provider.eq_ignore_ascii_case("launchbox"))
        .map(|id| id.id.as_str());
    let lookup = index.lookup(
        launchbox_id,
        record.archivefs_path.as_deref(),
        record
            .platform_candidate
            .as_deref()
            .or(record.provider_platform_name.as_deref()),
        None,
    )?;
    Some(index.media_snapshot(&lookup))
}

/// Turns a local path into a cover, using the core's cache for everything.
///
/// Held by the worker thread and built once, because opening the identity cache and
/// indexing 13,891 records by path is work that must happen once per library rather
/// than once per row - `IdentityCache::record_for_path` is a linear scan, and one
/// per visible row per frame is a scan storm.
pub(crate) struct RommCoverSource {
    settings: archivefs_core::identity_source::settings::ProviderSettings,
    server_id: String,
    /// Path to the record that claims it. Built once, so a lookup is a hash probe.
    by_path: HashMap<PathBuf, archivefs_core::identity_source::model::ExternalIdentityRecord>,
    artwork: archivefs_core::identity_source::artwork::ArtworkCache,
    /// Validated lazily and at most once. A failure here degrades the list to
    /// cache-only rather than disabling it: covers already on disk still draw.
    source: Option<Result<archivefs_core::identity_source::romm::config::ValidatedRommSource, ()>>,
    transport: archivefs_core::identity_source::romm::client::UreqTransport,
    trusted_roots: Option<Vec<PathBuf>>,
    esde: Option<archivefs_core::emulator_environment::es_de_metadata::EsDeProviderCollection>,
    launchbox:
        Option<archivefs_core::identity_source::launchbox_local::LaunchBoxLocalProviderIndex>,
}

/// The page size the catalogue is walked in.
///
/// [`IdentityCache::page`] deliberately clamps its limit, so there is no "give me
/// everything" call: asking for one enormous page silently returns only the first.
const CATALOGUE_PAGE: usize = 1_000;

/// Indexes every record in the catalogue by the local path it claims.
///
/// `IdentityCache::record_for_path` is a linear scan, and one per visible row per
/// frame over 36,259 records is a scan storm; this pays for it once per library.
///
/// Walked page by page on purpose. `page(0, usize::MAX)` looks like it reads the
/// whole catalogue and does not - it clamps, returns the first thousand records,
/// and leaves every game past them looking as though RomM had never heard of it.
pub(crate) fn index_by_path(
    cache: &archivefs_core::identity_source::cache::IdentityCache,
) -> HashMap<PathBuf, archivefs_core::identity_source::model::ExternalIdentityRecord> {
    let mut by_path = HashMap::new();
    let mut offset = 0;
    loop {
        let page = cache.page(offset, CATALOGUE_PAGE);
        if page.is_empty() {
            break;
        }
        for record in page {
            if let Some(path) = record.archivefs_path.as_deref() {
                by_path.insert(normalized_selected_path(path), record.clone());
            }
        }
        offset += page.len();
    }
    by_path
}

/// Canonicalizes only lexical path syntax for provider association. It does not
/// resolve symlinks or touch the filesystem: the selected catalogue path and
/// the provider's mapped path must remain the same source item, not merely two
/// paths that happen to resolve to the same inode.
fn normalized_selected_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

impl RommCoverSource {
    fn delivery(&self, generation: u64, local_path: &Path) -> Option<MediaDeliveryUpdate> {
        let record = self.by_path.get(&normalized_selected_path(local_path))?;
        let platform = record
            .platform_candidate
            .as_deref()
            .or(record.provider_platform_name.as_deref());
        let esde = platform.and_then(|platform| {
            self.esde
                .as_ref()
                .and_then(|collection| collection.lookup_path(platform, local_path))
        });
        let provider_generation = self
            .esde
            .as_ref()
            .map_or(0, |snapshot| snapshot.generation)
            .max(
                self.launchbox
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.generation),
            );
        let resolved = archivefs_core::identity_source::media_resolver::resolve_media(
            &resolver_input_for_record(
                record,
                generation,
                esde.as_ref(),
                provider_generation,
                self.launchbox.as_ref(),
            ),
        );
        Some(MediaDeliveryUpdate {
            generation,
            local_path: local_path.to_path_buf(),
            cover: resolved.cover_delivery,
            screenshots: resolved.screenshots_delivery,
        })
    }

    /// Opens the published cache and indexes it. Touches no network.
    pub(crate) fn open(
        trusted_roots: Option<Vec<PathBuf>>,
        esde: Option<archivefs_core::emulator_environment::es_de_metadata::EsDeProviderCollection>,
        launchbox: Option<
            archivefs_core::identity_source::launchbox_local::LaunchBoxLocalProviderIndex,
        >,
    ) -> Result<Self, String> {
        use archivefs_core::identity_source::artwork::ArtworkCache;
        use archivefs_core::identity_source::hashing::LocalHashCache;
        use archivefs_core::identity_source::model::IdentityProvider;
        use archivefs_core::identity_source::settings::{SettingsLocation, default_identity_root};
        use archivefs_core::identity_source::status::IdentitySourceApi;

        let identity_root = default_identity_root()?;
        let settings = SettingsLocation::new(&identity_root, IdentityProvider::Romm)
            .load()
            .map_err(|error| error.detail())?;
        let api = IdentitySourceApi::new(&identity_root, IdentityProvider::Romm);
        let cache = api.open_cache(None).map_err(|refusal| refusal.detail())?;
        let status = api.status(&settings.source, &LocalHashCache::new(), false);
        let server_id = status
            .server_id
            .clone()
            .unwrap_or_else(|| settings.source.url.clone());

        Ok(Self {
            settings,
            server_id,
            by_path: index_by_path(&cache),
            artwork: ArtworkCache::new(&identity_root, IdentityProvider::Romm),
            source: None,
            transport: archivefs_core::identity_source::romm::client::UreqTransport::new(),
            trusted_roots,
            esde,
            launchbox,
        })
    }

    /// Resolves one record, reading the cache first and requesting only when it
    /// must.
    ///
    /// `held_key` is the cover key the caller already has decoded. When the record
    /// still resolves to it, the answer is [`CoverAnswer::Unchanged`] and no
    /// thumbnail is read or decoded at all.
    pub(crate) fn resolve(
        &mut self,
        generation: u64,
        local_path: &Path,
        kind: GamerArtworkKind,
        held_key: Option<&str>,
    ) -> CoverReply {
        use archivefs_core::identity_source::artwork::{ArtworkCache, ArtworkRequest};

        #[cfg(debug_assertions)]
        let resolve_started = Instant::now();

        let reply = |provider_game_id: Option<String>,
                     screenshot_count: Option<usize>,
                     answer: CoverAnswer| CoverReply {
            generation,
            local_path: local_path.to_path_buf(),
            provider_game_id,
            kind,
            screenshot_count,
            answer,
        };

        let Some(record) = self
            .by_path
            .get(&normalized_selected_path(local_path))
            .cloned()
        else {
            return reply(None, None, CoverAnswer::None(NoCover::NoRommIdentity));
        };
        let game_id = record.provider_game_id.clone();
        let platform = record
            .platform_candidate
            .as_deref()
            .or(record.provider_platform_name.as_deref());
        let esde = platform.and_then(|platform| {
            self.esde
                .as_ref()
                .and_then(|collection| collection.lookup_path(platform, local_path))
        });
        let resolved_media = archivefs_core::identity_source::media_resolver::resolve_media(
            &resolver_input_for_record(
                &record,
                generation,
                esde.as_ref(),
                self.esde
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.generation)
                    .max(
                        self.launchbox
                            .as_ref()
                            .map_or(0, |snapshot| snapshot.generation),
                    ),
                self.launchbox.as_ref(),
            ),
        );
        #[cfg(debug_assertions)]
        cover_timing::stage(local_path, kind, "resolver", resolve_started);
        let screenshot_count = Some(resolved_media.screenshots.len());
        let selected_media = match kind {
            GamerArtworkKind::Cover => resolved_media.cover.as_ref(),
            GamerArtworkKind::Screenshot(index) => resolved_media.screenshots.get(index),
        };
        if let Some(item) = selected_media.filter(|item| {
            matches!(
                item.provider,
                archivefs_core::identity_source::media_resolver::MediaProvider::EsDe
                    | archivefs_core::identity_source::media_resolver::MediaProvider::LaunchBoxLocal
            )
        }) {
            let path = Path::new(
                item.reference
                    .hosted_reference
                    .as_deref()
                    .unwrap_or_default(),
            );
            #[cfg(debug_assertions)]
            let local_started = Instant::now();
            let thumbnail = fs::metadata(path).ok().map(|metadata| {
                archivefs_core::identity_source::artwork::CachedThumbnail {
                    key: format!("local:{}", path.display()),
                    path: path.to_path_buf(),
                    width: 0,
                    height: 0,
                    bytes: metadata.len(),
                }
            });
            return match thumbnail
                .as_ref()
                .and_then(|thumbnail| crate::romm_game::decode_thumbnail(thumbnail, false).ok())
            {
                Some(image) => {
                    #[cfg(debug_assertions)]
                    cover_timing::stage(local_path, kind, "local_read_decode", local_started);
                    reply(
                        Some(game_id),
                        screenshot_count,
                        CoverAnswer::Ready(Box::new(image)),
                    )
                }
                None => {
                    #[cfg(debug_assertions)]
                    cover_timing::stage(
                        local_path,
                        kind,
                        "local_read_decode_failed",
                        local_started,
                    );
                    reply(
                        Some(game_id),
                        screenshot_count,
                        CoverAnswer::None(NoCover::Failed),
                    )
                }
            };
        }
        let request = match kind {
            GamerArtworkKind::Cover => {
                if resolved_media.cover.is_none() {
                    // Preserve the existing human-facing distinction for a
                    // missing cover versus a public, non-approved reference.
                    let reason = match plan_for(&record) {
                        CoverPlan::Placeholder(reason) => reason,
                        CoverPlan::UseRommHostedCover => NoCover::NoArtwork,
                    };
                    return reply(Some(game_id), screenshot_count, CoverAnswer::None(reason));
                }
                ArtworkRequest::from_record(&record)
            }
            GamerArtworkKind::Screenshot(index) => {
                let Some(media) = resolved_media
                    .screenshots
                    .get(index)
                    .map(|item| &item.reference)
                else {
                    return reply(
                        Some(game_id),
                        screenshot_count,
                        CoverAnswer::None(NoCover::NoArtwork),
                    );
                };
                ArtworkRequest::from_media(&record.provider_game_id, media)
            }
        };
        let key = ArtworkCache::key_for(&self.server_id, &request);
        if held_key == Some(key.as_str()) {
            // The key is a digest of the server, the provider game id and RomM's own
            // artwork identity, so a match is proof that neither the record nor its
            // cover moved. The caller's pixels are still the right pixels.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_secs() as i64)
                .unwrap_or_default();
            // Kept warm in the eviction order even though nothing was read.
            self.artwork.touch(&self.server_id, &key, now);
            return reply(
                Some(game_id),
                screenshot_count,
                CoverAnswer::Unchanged { key },
            );
        }
        #[cfg(debug_assertions)]
        let lookup_started = Instant::now();
        if let Some(thumbnail) = self.artwork.lookup(&self.server_id, &request) {
            #[cfg(debug_assertions)]
            cover_timing::stage(local_path, kind, "cache_lookup_hit", lookup_started);
            #[cfg(debug_assertions)]
            let decode_started = Instant::now();
            return match crate::romm_game::decode_thumbnail(&thumbnail, true) {
                Ok(image) => {
                    #[cfg(debug_assertions)]
                    cover_timing::stage(local_path, kind, "cache_decode", decode_started);
                    reply(
                        Some(game_id),
                        screenshot_count,
                        CoverAnswer::Ready(Box::new(image)),
                    )
                }
                Err(_) => {
                    #[cfg(debug_assertions)]
                    cover_timing::stage(local_path, kind, "cache_decode_failed", decode_started);
                    reply(
                        Some(game_id),
                        screenshot_count,
                        CoverAnswer::None(NoCover::Failed),
                    )
                }
            };
        }
        #[cfg(debug_assertions)]
        cover_timing::stage(local_path, kind, "cache_lookup_miss", lookup_started);

        if self.validated_source().is_none() {
            return reply(
                Some(game_id),
                screenshot_count,
                CoverAnswer::None(NoCover::Unavailable),
            );
        }
        // Re-borrowed immutably now that validation is settled, so the fetch can
        // read the source alongside the cache and the transport.
        let Some(Ok(source)) = self.source.as_ref() else {
            return reply(
                Some(game_id),
                screenshot_count,
                CoverAnswer::None(NoCover::Unavailable),
            );
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs() as i64)
            .unwrap_or_default();
        #[cfg(debug_assertions)]
        let fetch_started = Instant::now();
        match self
            .artwork
            .fetch(source, &self.transport, &request, now, None)
        {
            Ok(thumbnail) => {
                #[cfg(debug_assertions)]
                cover_timing::stage(local_path, kind, "fetch", fetch_started);
                #[cfg(debug_assertions)]
                let decode_started = Instant::now();
                match crate::romm_game::decode_thumbnail(&thumbnail, false) {
                    Ok(image) => {
                        #[cfg(debug_assertions)]
                        cover_timing::stage(local_path, kind, "remote_decode", decode_started);
                        reply(
                            Some(game_id),
                            screenshot_count,
                            CoverAnswer::Ready(Box::new(image)),
                        )
                    }
                    Err(_) => {
                        #[cfg(debug_assertions)]
                        cover_timing::stage(
                            local_path,
                            kind,
                            "remote_decode_failed",
                            decode_started,
                        );
                        reply(
                            Some(game_id),
                            screenshot_count,
                            CoverAnswer::None(NoCover::Failed),
                        )
                    }
                }
            }
            Err(refusal) => {
                #[cfg(debug_assertions)]
                cover_timing::stage(local_path, kind, "fetch_refused", fetch_started);
                use archivefs_core::identity_source::artwork::ArtworkRefusal;
                let reason = match refusal {
                    ArtworkRefusal::Request(_) | ArtworkRefusal::Cancelled => NoCover::Unavailable,
                    _ => NoCover::Failed,
                };
                reply(Some(game_id), screenshot_count, CoverAnswer::None(reason))
            }
        }
    }

    /// Rebuilds the path index from the published catalogue as it is now.
    ///
    /// Returns whether it was replaced. A catalogue that cannot be reopened - a
    /// failed import that left no new cache, or an unreadable one - leaves the
    /// existing index in place: a refresh that cannot find anything better must not
    /// throw away something that works.
    pub(crate) fn reindex(&mut self) -> bool {
        use archivefs_core::identity_source::model::IdentityProvider;
        use archivefs_core::identity_source::settings::{SettingsLocation, default_identity_root};
        use archivefs_core::identity_source::status::IdentitySourceApi;

        let Ok(identity_root) = default_identity_root() else {
            return false;
        };
        let api = IdentitySourceApi::new(&identity_root, IdentityProvider::Romm);
        let Ok(cache) = api.open_cache(None) else {
            return false;
        };
        self.by_path = index_by_path(&cache);
        // Settings can have moved with the import - a changed mapping is exactly the
        // kind of thing a re-import follows - so the server identity is re-read too.
        // A failure here leaves the previous one, which is still the one the cached
        // thumbnails are keyed by.
        if let Ok(settings) = SettingsLocation::new(&identity_root, IdentityProvider::Romm).load() {
            let status = api.status(
                &settings.source,
                &archivefs_core::identity_source::hashing::LocalHashCache::new(),
                false,
            );
            self.server_id = status
                .server_id
                .clone()
                .unwrap_or_else(|| settings.source.url.clone());
            self.settings = settings;
            // The validated source is derived from those settings, so it is dropped
            // and re-established lazily rather than left describing the old ones.
            self.source = None;
        }
        true
    }

    /// Validates the source once. `None` means cache-only from here on.
    fn validated_source(
        &mut self,
    ) -> Option<&archivefs_core::identity_source::romm::config::ValidatedRommSource> {
        if self.source.is_none() {
            self.source = Some(self.validate());
        }
        self.source.as_ref().and_then(|result| result.as_ref().ok())
    }

    fn validate(
        &self,
    ) -> Result<archivefs_core::identity_source::romm::config::ValidatedRommSource, ()> {
        use archivefs_core::identity_source::settings::load_token_file;

        if !self.settings.source.enabled {
            return Err(());
        }
        let token = load_token_file(self.settings.source.token_path.as_deref()).map_err(|_| ())?;
        let roots = self.trusted_roots.as_deref().ok_or(())?;
        archivefs_core::identity_source::romm::config::ValidatedRommSource::validate(
            &self.settings.source,
            &token,
            roots,
            &archivefs_core::identity_source::net_policy::SystemResolver,
        )
        .map_err(|_| ())
    }
}

// --- The worker ----------------------------------------------------------

/// The thread covers are resolved on, and the channels to it.
///
/// One thread, deliberately. Covers are small and the artwork index is a single
/// shared file, so more threads would contend on it rather than finish sooner; one
/// thread is also a hard ceiling on how much work a fast scroll can cause. It is
/// entirely separate from `RommOperation`'s single slot, so a cover can never delay
/// a mount and a running import can never stop covers from drawing.
pub(crate) struct CoverWorker {
    requests: std::sync::mpsc::Sender<WorkerMessage>,
    replies: std::sync::mpsc::Receiver<CoverReply>,
    delivery: std::sync::mpsc::Receiver<MediaDeliveryUpdate>,
}

/// One job waiting on the worker, with what it needs to be ordered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueuedJob {
    pub(crate) generation: u64,
    pub(crate) job: CoverJob,
    /// Arrival order, so ties break oldest-first and the choice is deterministic.
    pub(crate) sequence: u64,
}

/// Chooses the next job to resolve, with bounded fairness.
///
/// Normally the highest-priority job wins, oldest first among equals - that is what
/// puts a freshly selected game ahead of a look-ahead backlog already queued, which
/// plain FIFO delivery cannot do.
///
/// `served_high` counts how many jobs have been taken ahead of something lower.
/// Once it reaches [`FAIRNESS_RUN`], the oldest *lower*-priority job is taken
/// instead and the count resets. Without that, a person holding a key down to move
/// the selection would keep a selected-priority job at the head of the queue
/// forever and the visible rows behind it would never be read.
pub(crate) fn next_job(queue: &mut Vec<QueuedJob>, served_high: &mut u32) -> Option<QueuedJob> {
    if queue.is_empty() {
        return None;
    }
    let best = queue
        .iter()
        .map(|queued| queued.job.priority)
        .min()
        .expect("the queue is not empty");
    // The fairness release: something lower is waiting and the run has gone on long
    // enough, so it goes next whatever is at the head.
    let wanted =
        if *served_high >= FAIRNESS_RUN && queue.iter().any(|queued| queued.job.priority > best) {
            *served_high = 0;
            queue
                .iter()
                .map(|queued| queued.job.priority)
                .filter(|priority| *priority > best)
                .min()
                .expect("a lower priority was just observed")
        } else {
            if queue.iter().any(|queued| queued.job.priority > best) {
                *served_high += 1;
            } else {
                // Nothing is being held back, so nothing is being unfair to.
                *served_high = 0;
            }
            best
        };
    let index = queue
        .iter()
        .enumerate()
        .filter(|(_, queued)| queued.job.priority == wanted)
        .min_by_key(|(_, queued)| queued.sequence)
        .map(|(index, _)| index)
        .expect("a job with the chosen priority");
    Some(queue.remove(index))
}

/// Drops the least valuable work once the queue is over its bound.
///
/// Lowest priority and oldest first: those describe rows that have most likely
/// scrolled away, and the UI re-asks for anything it still wants on the next frame
/// it draws.
pub(crate) fn trim_queue(queue: &mut Vec<QueuedJob>) {
    if queue.len() <= MAX_QUEUED_JOBS {
        return;
    }
    queue.sort_by(|left, right| {
        left.job
            .priority
            .cmp(&right.job.priority)
            .then_with(|| right.sequence.cmp(&left.sequence))
    });
    queue.truncate(MAX_QUEUED_JOBS);
}

/// What the UI thread asks the worker to do.
enum WorkerMessage {
    Resolve {
        generation: u64,
        job: CoverJob,
    },
    /// Reopen the catalogue and rebuild the path index.
    ///
    /// Sent when a RomM import or cache replacement succeeded, and only then - the
    /// index is not rebuilt on a timer or per frame, because on this library it is
    /// 36,259 records and rebuilding it speculatively would be the reload storm this
    /// message exists to avoid.
    Reindex,
    UpdateEsDe(
        Option<archivefs_core::emulator_environment::es_de_metadata::EsDeProviderCollection>,
    ),
    UpdateLaunchBox(
        Option<archivefs_core::identity_source::launchbox_local::LaunchBoxLocalProviderIndex>,
    ),
}

impl CoverWorker {
    /// Starts the worker. Opening the catalogue happens on the thread, so a large
    /// library never delays the first frame.
    pub(crate) fn start(
        context: egui::Context,
        trusted_roots: Option<Vec<PathBuf>>,
        esde: Option<archivefs_core::emulator_environment::es_de_metadata::EsDeProviderCollection>,
        launchbox: Option<
            archivefs_core::identity_source::launchbox_local::LaunchBoxLocalProviderIndex,
        >,
    ) -> Self {
        let (request_sender, request_receiver) = std::sync::mpsc::channel::<WorkerMessage>();
        let (reply_sender, reply_receiver) = std::sync::mpsc::channel::<CoverReply>();
        let (delivery_sender, delivery_receiver) =
            std::sync::mpsc::channel::<MediaDeliveryUpdate>();
        std::thread::spawn(move || {
            let mut source: Option<RommCoverSource> = None;
            let mut esde_snapshot = esde;
            let mut launchbox_snapshot = launchbox;
            let mut opened = false;
            let mut queue: Vec<QueuedJob> = Vec::new();
            let mut served_high = 0_u32;
            let mut sequence = 0_u64;
            loop {
                // Block for work, then take everything else that has arrived. The
                // drain is what makes prioritising possible at all: with one job
                // read at a time the order is whatever the channel delivered, and a
                // freshly selected game would sit behind a look-ahead backlog.
                let mut messages = Vec::new();
                if queue.is_empty() {
                    match request_receiver.recv() {
                        Ok(message) => messages.push(message),
                        Err(_) => return,
                    }
                }
                messages.extend(request_receiver.try_iter());

                for message in messages {
                    match message {
                        WorkerMessage::Reindex => {
                            match source.as_mut() {
                                // Reindexed in place, keeping the current index if
                                // the new catalogue cannot be read.
                                Some(source) => {
                                    source.reindex();
                                }
                                // Never opened, or the first open failed. An import
                                // may have created what was missing, so try again.
                                None => {
                                    opened = true;
                                    source = RommCoverSource::open(
                                        trusted_roots.clone(),
                                        esde_snapshot.clone(),
                                        launchbox_snapshot.clone(),
                                    )
                                    .ok();
                                }
                            }
                            // Everything queued was resolved against the previous
                            // catalogue's generation and has been superseded. The UI
                            // has already moved its slots to `Revalidating` and
                            // re-asks for what is on screen.
                            queue.clear();
                            context.request_repaint();
                        }
                        WorkerMessage::UpdateEsDe(snapshot) => {
                            esde_snapshot = snapshot.clone();
                            if let Some(source) = source.as_mut() {
                                source.esde = snapshot;
                            }
                            context.request_repaint();
                        }
                        WorkerMessage::UpdateLaunchBox(snapshot) => {
                            launchbox_snapshot = snapshot.clone();
                            if let Some(source) = source.as_mut() {
                                source.launchbox = snapshot;
                            }
                            context.request_repaint();
                        }
                        WorkerMessage::Resolve { generation, job } => {
                            sequence = sequence.wrapping_add(1);
                            queue.push(QueuedJob {
                                generation,
                                job,
                                sequence,
                            });
                        }
                    }
                }
                trim_queue(&mut queue);

                let Some(queued) = next_job(&mut queue, &mut served_high) else {
                    continue;
                };
                #[cfg(debug_assertions)]
                let queued_at = cover_timing::worker_started(
                    queued.generation,
                    &queued.job.local_path,
                    queued.job.kind,
                );
                #[cfg(debug_assertions)]
                let worker_started = Instant::now();
                if !opened {
                    opened = true;
                    source = RommCoverSource::open(
                        trusted_roots.clone(),
                        esde_snapshot.clone(),
                        launchbox_snapshot.clone(),
                    )
                    .ok();
                }
                let reply = match source.as_mut() {
                    Some(source) => {
                        if let Some(update) =
                            source.delivery(queued.generation, &queued.job.local_path)
                        {
                            let _ = delivery_sender.send(update);
                        }
                        source.resolve(
                            queued.generation,
                            &queued.job.local_path,
                            queued.job.kind,
                            queued.job.held_key.as_deref(),
                        )
                    }
                    // The catalogue itself could not be opened - RomM has never been
                    // imported, or the published cache is unreadable. That is not the
                    // same as a record having no identity, and saying so would send
                    // someone looking for the wrong problem.
                    None => CoverReply {
                        generation: queued.generation,
                        local_path: queued.job.local_path,
                        provider_game_id: None,
                        kind: queued.job.kind,
                        screenshot_count: None,
                        answer: CoverAnswer::None(NoCover::Unavailable),
                    },
                };
                #[cfg(debug_assertions)]
                {
                    let answer = match &reply.answer {
                        CoverAnswer::Ready(image) if image.from_cache => "ready-cache-or-local",
                        CoverAnswer::Ready(_) => "ready",
                        CoverAnswer::Unchanged { .. } => "unchanged",
                        CoverAnswer::None(_) => "none",
                    };
                    cover_timing::completed(
                        reply.generation,
                        &reply.local_path,
                        reply.kind,
                        queued_at,
                        worker_started,
                        answer,
                    );
                }
                if reply_sender.send(reply).is_err() {
                    return;
                }
                context.request_repaint();
            }
        });
        Self {
            requests: request_sender,
            replies: reply_receiver,
            delivery: delivery_receiver,
        }
    }

    /// Asks about one record. Dropped silently if the worker has gone.
    pub(crate) fn request(&self, generation: u64, job: CoverJob) {
        #[cfg(debug_assertions)]
        cover_timing::enqueued(generation, &job.local_path, job.kind);
        let _ = self
            .requests
            .send(WorkerMessage::Resolve { generation, job });
    }

    /// Asks the worker to reopen the catalogue after a successful import.
    pub(crate) fn reindex(&self) {
        let _ = self.requests.send(WorkerMessage::Reindex);
    }

    pub(crate) fn update_esde(
        &self,
        snapshot: Option<
            archivefs_core::emulator_environment::es_de_metadata::EsDeProviderCollection,
        >,
    ) {
        let _ = self.requests.send(WorkerMessage::UpdateEsDe(snapshot));
    }

    pub(crate) fn update_launchbox(
        &self,
        snapshot: Option<
            archivefs_core::identity_source::launchbox_local::LaunchBoxLocalProviderIndex,
        >,
    ) {
        let _ = self.requests.send(WorkerMessage::UpdateLaunchBox(snapshot));
    }

    /// Every answer that has arrived. Never blocks, so the UI thread never waits on
    /// a decode or a request.
    pub(crate) fn drain(&self) -> Vec<CoverReply> {
        self.replies.try_iter().collect()
    }

    pub(crate) fn drain_delivery(&self) -> Vec<MediaDeliveryUpdate> {
        self.delivery.try_iter().collect()
    }
}

#[cfg(test)]
#[path = "gamer_artwork/tests.rs"]
mod tests;
