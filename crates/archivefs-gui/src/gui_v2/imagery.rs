//! Platform hardware artwork, the EmuWiz mascot and the Home showcase for GUI v2.
//!
//! Every picture here is already on this computer: the hardware PNGs compiled
//! into the executable (`ui::platform_artwork::BUNDLED_PLATFORM_ARTWORK`), an
//! optional EmuWiz-managed custom PNG in the platform-artwork folder, and the
//! bundled mascot badge. Nothing is fetched. The 1024px sources are decoded
//! once, on one worker thread, and reduced to a small texture; the UI thread
//! only looks textures up and paints them, falling back to the existing
//! native category glyph while a picture is loading or when none exists.
use super::{
    library::Library,
    media_sources::{MediaIndex, Source},
};
use crate::ui::{
    components::EMUWIZ_MASCOT_BADGE_PNG,
    platform_artwork::{
        bundled_platform_artwork, custom_platform_artwork_fingerprint,
        decode_bundled_platform_artwork, decode_custom_platform_artwork, fitted_artwork_rect,
        paint_platform_glyph_at, platform_asset_id, platform_fallback_asset_id,
    },
    theme,
};
use eframe::egui;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Weak,
        mpsc::{self, Receiver, Sender},
    },
};

/// Largest side of an uploaded platform/mascot texture. Cards paint these at
/// 28-150 logical pixels, so 256 stays sharp at 1.5x scaling while holding
/// 1/16th of the decoded source in memory.
pub(super) const THUMBNAIL_SIDE: u32 = 256;
/// Bounded texture uploads per frame keep a first visit to Platforms smooth.
const UPLOADS_PER_FRAME: usize = 6;
/// Home shows at most this many "newest with pictures" games.
pub(super) const SHOWCASE_LIMIT: usize = 12;
const MASCOT: &str = "emuwiz-mascot";

enum Art {
    Pending,
    Ready(egui::TextureHandle),
    /// No usable PNG: paint the native category glyph instead.
    Unavailable,
}

struct Job {
    asset_id: String,
    fallback_id: &'static str,
}

struct Worker {
    requests: Sender<Job>,
    replies: Receiver<(String, Option<egui::ColorImage>)>,
}

/// Newest games with a local cover, selected once per artwork index.
struct Showcase {
    library: Weak<Library>,
    index: Weak<MediaIndex>,
    games: Vec<i64>,
}

#[derive(Default)]
pub(super) struct Imagery {
    worker: Option<Worker>,
    art: HashMap<String, Art>,
    showcase: Option<Showcase>,
    /// Games opened this session, most recent first (never persisted).
    recent: Vec<i64>,
    pub decoded: u64,
}

/// What a platform paints as: its artwork texture, or a category glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlatformPaint {
    Artwork,
    Glyph,
}

impl Imagery {
    fn worker(&mut self, context: &egui::Context) -> &Worker {
        self.worker.get_or_insert_with(|| {
            let (requests, jobs) = mpsc::channel::<Job>();
            let (answers, replies) = mpsc::channel();
            let context = context.clone();
            let directory = archivefs_core::platform_artwork::default_platform_artwork_root().ok();
            std::thread::Builder::new()
                .name("emuwiz-v2-platform-art".into())
                .spawn(move || {
                    while let Ok(job) = jobs.recv() {
                        let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            if job.asset_id == MASCOT {
                                decode_mascot()
                            } else {
                                decode_platform(directory.as_ref(), &job.asset_id, job.fallback_id)
                            }
                        }))
                        .ok()
                        .flatten();
                        if answers.send((job.asset_id, image)).is_err() {
                            break;
                        }
                        context.request_repaint();
                    }
                })
                .ok();
            Worker { requests, replies }
        })
    }

    /// Upload a bounded number of finished decodes. Called once per frame.
    pub fn begin_frame(&mut self, context: &egui::Context) {
        let Some(worker) = &self.worker else {
            return;
        };
        for _ in 0..UPLOADS_PER_FRAME {
            let Ok((asset_id, image)) = worker.replies.try_recv() else {
                break;
            };
            let art = match image {
                Some(image) => {
                    self.decoded += 1;
                    Art::Ready(context.load_texture(
                        format!("v2-imagery-{asset_id}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    ))
                }
                None => Art::Unavailable,
            };
            self.art.insert(asset_id, art);
        }
    }

    /// Pictures still being decoded; used by tests and timing checks.
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.art
            .values()
            .filter(|art| matches!(art, Art::Pending))
            .count()
    }

    fn texture(
        &mut self,
        context: &egui::Context,
        asset_id: &str,
        fallback_id: &'static str,
    ) -> Option<&egui::TextureHandle> {
        if !self.art.contains_key(asset_id) {
            let sent = self
                .worker(context)
                .requests
                .send(Job {
                    asset_id: asset_id.to_owned(),
                    fallback_id,
                })
                .is_ok();
            self.art.insert(
                asset_id.to_owned(),
                if sent { Art::Pending } else { Art::Unavailable },
            );
        }
        match self.art.get(asset_id) {
            Some(Art::Ready(texture)) => Some(texture),
            _ => None,
        }
    }

    /// Paint a platform's hardware artwork aspect-fitted inside `rect`, or its
    /// category glyph when no artwork exists (or while it is still decoding).
    pub fn paint_platform(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        platform: &str,
        tint: egui::Color32,
    ) -> PlatformPaint {
        if !ui.is_rect_visible(rect) {
            return PlatformPaint::Glyph;
        }
        let asset_id = platform_asset_id(platform, false);
        let fallback = platform_fallback_asset_id(platform, false);
        let side = rect.width().min(rect.height());
        if let Some(texture) = self.texture(ui.ctx(), &asset_id, fallback) {
            ui.painter().image(
                texture.id(),
                fitted_artwork_rect(rect.center(), side, texture.size_vec2()),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                tint,
            );
            return PlatformPaint::Artwork;
        }
        paint_platform_glyph_at(
            ui.painter(),
            rect.center(),
            side * 0.8,
            theme::SECONDARY_TEXT.gamma_multiply(tint.a() as f32 / 255.0),
            fallback,
        );
        PlatformPaint::Glyph
    }

    /// Allocate a square of `side` points and paint the platform into it.
    pub fn platform_icon(
        &mut self,
        ui: &mut egui::Ui,
        platform: &str,
        side: f32,
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
        self.paint_platform(ui, rect, platform, egui::Color32::WHITE);
        response
    }

    /// The EmuWiz mascot badge, decoded once off the UI thread.
    pub fn mascot(&mut self, context: &egui::Context) -> Option<&egui::TextureHandle> {
        self.texture(context, MASCOT, "unknown")
    }

    /// Remember that a game's details were opened (cheap when repeated).
    pub fn note_opened(&mut self, game: i64) {
        if self.recent.first() == Some(&game) {
            return;
        }
        self.recent.retain(|id| *id != game);
        self.recent.insert(0, game);
        self.recent.truncate(SHOWCASE_LIMIT);
    }

    /// Games opened this session, most recent first. Their covers were
    /// already loaded when opened, so Home reuses those pictures.
    pub fn recently_opened(&self) -> &[i64] {
        &self.recent
    }

    /// Up to [`SHOWCASE_LIMIT`] games whose cover is a local file, newest file
    /// first. Only local covers qualify, so opening Home never asks an online
    /// provider for a picture. Computed once per artwork index.
    pub fn showcase(&mut self, library: &Arc<Library>, index: Option<&Arc<MediaIndex>>) -> &[i64] {
        let Some(index) = index else {
            return &[];
        };
        let library_weak = Arc::downgrade(library);
        let index_weak = Arc::downgrade(index);
        if self.showcase.as_ref().is_none_or(|showcase| {
            !showcase.index.ptr_eq(&index_weak) || !showcase.library.ptr_eq(&library_weak)
        }) {
            self.showcase = Some(Showcase {
                library: library_weak,
                index: index_weak,
                games: select_showcase(library, index),
            });
        }
        self.showcase
            .as_ref()
            .map_or(&[], |showcase| showcase.games.as_slice())
    }
}

pub(super) fn select_showcase(library: &Library, index: &MediaIndex) -> Vec<i64> {
    let mut candidates: Vec<_> = index
        .covers
        .iter()
        .filter(|(_, source)| matches!(source, Source::Local(_)))
        .filter_map(|(id, _)| library.game(*id))
        .filter(|game| !game.attention)
        .map(|game| {
            (
                std::cmp::Reverse(game.archive.modified_time_unix_seconds.unwrap_or(0)),
                game.title.to_lowercase(),
                game.archive.id,
            )
        })
        .collect();
    candidates.sort_unstable();
    candidates
        .into_iter()
        .take(SHOWCASE_LIMIT)
        .map(|(_, _, id)| id)
        .collect()
}

fn shrink(image: egui::ColorImage, side: u32) -> Option<egui::ColorImage> {
    let pixels: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_srgba_unmultiplied())
        .collect();
    let buffer = image::RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, pixels)?;
    let small = image::DynamicImage::ImageRgba8(buffer)
        .thumbnail(side, side)
        .into_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [small.width() as usize, small.height() as usize],
        small.as_raw(),
    ))
}

/// The documented resolution order (docs/PLATFORM_ARTWORK.md): the managed
/// custom PNG for the exact platform, the bundled hardware PNG, then a managed
/// custom category PNG. `None` means "paint the native glyph".
pub(super) fn decode_platform(
    directory: Option<&PathBuf>,
    asset_id: &str,
    fallback_id: &str,
) -> Option<egui::ColorImage> {
    let custom = |id: &str| {
        let fingerprint =
            custom_platform_artwork_fingerprint(directory.map(PathBuf::as_path), id).ok()?;
        decode_custom_platform_artwork(&fingerprint.path).ok()
    };
    let image = custom(asset_id)
        .or_else(|| {
            bundled_platform_artwork(asset_id)
                .and_then(|bundled| decode_bundled_platform_artwork(bundled.png).ok())
        })
        .or_else(|| {
            (fallback_id != asset_id)
                .then(|| custom(fallback_id))
                .flatten()
        })?;
    shrink(image, THUMBNAIL_SIDE)
}

fn decode_mascot() -> Option<egui::ColorImage> {
    let decoded =
        image::load_from_memory_with_format(EMUWIZ_MASCOT_BADGE_PNG, image::ImageFormat::Png)
            .ok()?
            .thumbnail(THUMBNAIL_SIDE, THUMBNAIL_SIDE)
            .into_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [decoded.width() as usize, decoded.height() as usize],
        decoded.as_raw(),
    ))
}

/// What an empty state shows beside its explanation.
#[derive(Clone, Copy)]
pub(super) enum EmptyArt<'a> {
    /// The EmuWiz mascot: "nothing here yet" states.
    Mascot,
    /// A platform's hardware (or its glyph).
    Platform(&'a str),
    /// A native category glyph such as "optical-disc" or "cartridge".
    Glyph(&'static str),
}

/// A restrained empty state: one small picture and a plain-English
/// explanation. Returns whether the optional primary action was chosen.
pub(super) fn empty_state(
    ui: &mut egui::Ui,
    imagery: &mut Imagery,
    art: EmptyArt<'_>,
    title: &str,
    detail: &str,
    action: Option<&str>,
) -> bool {
    let mut clicked = false;
    egui::Frame::new()
        .fill(theme::CARD_SURFACE)
        .stroke(theme::border(ui))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(theme::SPACE_LG as i8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().min(760.0));
            ui.horizontal(|ui| {
                let side = 76.0;
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
                ui.painter().rect_filled(rect, 10.0, theme::DEEP_BACKGROUND);
                let inner = rect.shrink(6.0);
                match art {
                    EmptyArt::Mascot => {
                        if let Some(texture) = imagery.mascot(ui.ctx()) {
                            ui.painter().image(
                                texture.id(),
                                fitted_artwork_rect(
                                    inner.center(),
                                    inner.width(),
                                    texture.size_vec2(),
                                ),
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        } else {
                            paint_platform_glyph_at(
                                ui.painter(),
                                inner.center(),
                                inner.width() * 0.7,
                                theme::SECONDARY_TEXT,
                                "console",
                            );
                        }
                    }
                    EmptyArt::Platform(platform) => {
                        imagery.paint_platform(
                            ui,
                            inner,
                            platform,
                            egui::Color32::from_white_alpha(170),
                        );
                    }
                    EmptyArt::Glyph(glyph) => {
                        paint_platform_glyph_at(
                            ui.painter(),
                            inner.center(),
                            inner.width() * 0.7,
                            theme::SECONDARY_TEXT,
                            glyph,
                        );
                    }
                }
                ui.add_space(theme::SPACE_MD);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .size(theme::SECTION_TITLE_SIZE)
                            .strong(),
                    );
                    ui.label(egui::RichText::new(detail).color(theme::muted(ui)));
                    if let Some(action) = action {
                        ui.add_space(theme::SPACE_XS);
                        clicked = ui
                            .add(
                                egui::Button::new(egui::RichText::new(action).strong())
                                    .fill(theme::PRIMARY_ACTION)
                                    .min_size(egui::vec2(180.0, 40.0)),
                            )
                            .clicked();
                    }
                });
            });
        });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_v2::library::Library;
    use archivefs_core::PersistedArchive;

    fn archive(id: i64, title: &str, modified: i64) -> PersistedArchive {
        PersistedArchive {
            id,
            source_folder_id: 1,
            relative_path: format!("{title}.iso").into(),
            absolute_path: format!("/fixture/{title}.iso").into(),
            archive_kind: "iso".into(),
            display_name: title.into(),
            normalized_name: title.into(),
            size_bytes: Some(4),
            modified_time_unix_seconds: Some(modified),
            platform: Some("PSX".into()),
            platform_source: Some("manual".into()),
            last_known_health: "pending".into(),
            last_seen_at: "2026-09-19".into(),
            last_verified_missing_at: None,
            identity_report: None,
        }
    }

    #[test]
    fn bundled_platform_art_is_reduced_to_a_bounded_thumbnail() {
        let image = decode_platform(None, "psx", "console").expect("bundled PSX artwork");
        assert!(image.size[0] as u32 <= THUMBNAIL_SIDE && image.size[1] as u32 <= THUMBNAIL_SIDE);
        assert!(image.size[0] as u32 == THUMBNAIL_SIDE || image.size[1] as u32 == THUMBNAIL_SIDE);
    }

    #[test]
    fn platform_without_any_artwork_falls_back_to_the_glyph() {
        assert!(decode_platform(None, "unknown", "unknown").is_none());
        assert!(decode_platform(None, "no-such-platform", "console").is_none());
    }

    #[test]
    fn managed_custom_platform_art_takes_priority_and_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(600, 300, image::Rgba([200, 10, 10, 255]))
            .save(directory.path().join("psx.png"))
            .unwrap();
        let path = directory.path().to_path_buf();
        let image = decode_platform(Some(&path), "psx", "console").unwrap();
        assert_eq!(image.size, [256, 128]);
        assert_eq!(image.pixels[0].to_srgba_unmultiplied(), [200, 10, 10, 255]);
        // A malformed custom file never masks the bundled artwork.
        std::fs::write(directory.path().join("psx.png"), b"not a png").unwrap();
        let image = decode_platform(Some(&path), "psx", "console").unwrap();
        assert_ne!(image.pixels[0].to_srgba_unmultiplied(), [200, 10, 10, 255]);
    }

    #[test]
    fn recently_opened_is_most_recent_first_deduplicated_and_bounded() {
        let mut imagery = Imagery::default();
        for id in 0..20 {
            imagery.note_opened(id);
        }
        imagery.note_opened(15);
        imagery.note_opened(15);
        let recent = imagery.recently_opened();
        assert_eq!(recent.len(), SHOWCASE_LIMIT);
        assert_eq!(&recent[..3], &[15, 19, 18]);
        assert_eq!(recent.iter().filter(|id| **id == 15).count(), 1);
    }

    #[test]
    fn mascot_decodes_to_a_bounded_texture() {
        let image = decode_mascot().unwrap();
        assert!(image.size.iter().all(|side| *side as u32 <= THUMBNAIL_SIDE));
    }

    #[test]
    fn showcase_uses_only_local_covers_newest_first_and_is_bounded() {
        let mut games: Vec<_> = (0..40)
            .map(|id| archive(id, &format!("Game {id}"), id))
            .collect();
        games.push(archive(99, "Remote only", 1_000));
        let library = Arc::new(Library::new(games));
        let mut index = MediaIndex::default();
        for id in 0..40 {
            index
                .covers
                .insert(id, Source::Local(format!("/covers/{id}.png").into()));
        }
        let record = serde_json::from_value(serde_json::json!({
            "provider": "romm", "server_id": "fixture", "provider_game_id": "game", "provider_path": "/game",
            "regions": [], "hashes": [], "metadata_provider_ids": [], "related_files": [], "sibling_game_ids": [],
            "imported_at_unix_seconds": 0, "verification": "strong_external", "conflicts": [], "evidence": [],
        }))
        .unwrap();
        index.covers.insert(
            99,
            Source::Remote {
                record: Arc::new(record),
                kind: super::super::media_sources::Kind::Cover,
            },
        );
        let selected = select_showcase(&library, &index);
        assert_eq!(selected.len(), SHOWCASE_LIMIT);
        assert_eq!(selected[0], 39);
        assert!(
            !selected.contains(&99),
            "remote covers must not be fetched from Home"
        );
        let mut imagery = Imagery::default();
        let index = Arc::new(index);
        let first = imagery.showcase(&library, Some(&index)).to_vec();
        assert_eq!(first, selected);
        assert!(imagery.showcase(&library, None).is_empty());
    }

    #[test]
    fn platform_art_decodes_once_off_the_ui_thread_and_is_reused() {
        let context = egui::Context::default();
        let mut imagery = Imagery::default();
        let mut painted = PlatformPaint::Glyph;
        let start = std::time::Instant::now();
        while painted == PlatformPaint::Glyph
            && start.elapsed() < std::time::Duration::from_secs(10)
        {
            let _ = context.run(egui::RawInput::default(), |context| {
                imagery.begin_frame(context);
                egui::CentralPanel::default().show(context, |ui| {
                    painted = imagery.paint_platform(
                        ui,
                        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(96.0, 96.0)),
                        "PSX",
                        egui::Color32::WHITE,
                    );
                });
            });
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(painted, PlatformPaint::Artwork);
        assert_eq!(imagery.decoded, 1);
        assert_eq!(imagery.pending(), 0);
        for _ in 0..3 {
            let _ = context.run(egui::RawInput::default(), |context| {
                imagery.begin_frame(context);
                egui::CentralPanel::default().show(context, |ui| {
                    imagery.paint_platform(
                        ui,
                        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(96.0, 96.0)),
                        "PSX",
                        egui::Color32::WHITE,
                    );
                });
            });
        }
        assert_eq!(
            imagery.decoded, 1,
            "a cached platform texture is never decoded again"
        );
    }
}
