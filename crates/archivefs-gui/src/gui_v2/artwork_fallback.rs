//! The tile painted where a game has no cover (yet): a platform-coloured
//! plate with the platform's name, its bundled hardware picture and the
//! game's title, inside the Blue dashed "this is not a cover" outline.
//!
//! Everything is drawn from what is already in the executable: the platform
//! registry's display names, the bundled hardware art (`Imagery`) and a small
//! fixed palette. No new texture, file or network request exists for a
//! placeholder. The per-platform style is built once and kept for the
//! session, keyed by the platform string (about a hundred), never by game, so
//! a 136,000-game shelf costs one hash lookup per visible tile.

use std::{collections::HashMap, sync::Arc};

use eframe::egui::{self, Color32};

use super::{artwork::PictureLabel, imagery::Imagery, library::UNKNOWN_PLATFORM};
use crate::ui::{
    platform_artwork::{
        canonical_platform_for_artwork, platform_asset_id, platform_fallback_asset_id,
    },
    theme,
};

/// Muted plates that all keep white text at WCAG AA contrast (tested).
const PALETTE: [Color32; 12] = [
    Color32::from_rgb(0x8e, 0x3b, 0x46),
    Color32::from_rgb(0xa0, 0x52, 0x2d),
    Color32::from_rgb(0x85, 0x67, 0x1c),
    Color32::from_rgb(0x4f, 0x7a, 0x28),
    Color32::from_rgb(0x2e, 0x7d, 0x5b),
    Color32::from_rgb(0x1f, 0x76, 0x7a),
    Color32::from_rgb(0x2b, 0x6c, 0xa3),
    Color32::from_rgb(0x46, 0x52, 0xa8),
    Color32::from_rgb(0x6a, 0x4c, 0x9c),
    Color32::from_rgb(0x8e, 0x3e, 0x7e),
    Color32::from_rgb(0x7a, 0x52, 0x30),
    Color32::from_rgb(0x3d, 0x6e, 0x8c),
];
/// A game whose platform is not known gets a neutral plate, not a "platform" colour.
const UNKNOWN_PLATE: Color32 = Color32::from_rgb(0x4a, 0x55, 0x68);
const BAND_HEIGHT: f32 = 22.0;
const BAND_FONT: f32 = 12.0;
const TITLE_FONT: f32 = 14.0;
const STATUS_FONT: f32 = 11.0;
const TEXT: Color32 = theme::PRIMARY_TEXT;
const MUTED: Color32 = Color32::from_rgb(0xd5, 0xdd, 0xea);

/// How one platform's missing-artwork tile looks. Built once per platform.
#[derive(Debug, PartialEq)]
pub(super) struct Style {
    pub band: Color32,
    pub body: Color32,
    /// The registry's display name, or the stored platform text verbatim.
    pub name: String,
    /// A few letters for tiles too narrow for the name.
    pub short: String,
    asset_id: String,
    fallback: &'static str,
}

impl Style {
    pub(super) fn for_platform(platform: &str) -> Self {
        let known = canonical_platform_for_artwork(platform);
        let unknown = platform.trim().is_empty() || platform == UNKNOWN_PLATFORM;
        let band = if unknown {
            UNKNOWN_PLATE
        } else {
            // Stable across builds and runs: the canonical id, or the stored
            // text for a platform this build does not know.
            PALETTE
                [(fnv1a(known.map_or(platform, |known| known.id)) % PALETTE.len() as u64) as usize]
        };
        let name = known.map_or(platform, |known| known.display_name);
        let short = known
            .and_then(|known| short_label(known.id))
            .map_or_else(|| name.to_owned(), str::to_owned);
        Self {
            band,
            body: scale(band, 0.55),
            name: name.to_owned(),
            short,
            asset_id: platform_asset_id(platform, unknown),
            fallback: platform_fallback_asset_id(platform, unknown),
        }
    }
}

/// One style per platform string for the session.
#[derive(Default)]
pub(super) struct Styles(HashMap<String, Arc<Style>>);

impl Styles {
    pub(super) fn get(&mut self, platform: &str) -> Arc<Style> {
        if let Some(style) = self.0.get(platform) {
            return Arc::clone(style);
        }
        let style = Arc::new(Style::for_platform(platform));
        self.0.insert(platform.to_owned(), Arc::clone(&style));
        style
    }
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

/// Paint the placeholder for `title` on `platform` into `rect`. `label` is
/// the artwork pipeline's current state ("No picture yet", "Loading…").
/// `detail_on_tile`: a large single picture (Game Details) rather than a
/// shelf tile; it shows the label's detail and leaves the title to the page.
pub(super) fn paint(
    ui: &egui::Ui,
    imagery: &mut Imagery,
    rect: egui::Rect,
    title: &str,
    platform: &str,
    label: &PictureLabel,
    detail_on_tile: bool,
) {
    let style = imagery.placeholder_style(platform);
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    painter.rect_filled(rect, 8.0, style.body);
    let banded = rect.height() >= 48.0;
    let mut top = rect.top();
    if banded {
        let band = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), BAND_HEIGHT));
        painter.rect_filled(
            band,
            egui::CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            },
            style.band,
        );
        let wrap = (rect.width() - 10.0).max(1.0);
        let font = egui::FontId::proportional(BAND_FONT);
        // The full name when it fits, otherwise the short label.
        let mut galley = painter.layout_job(one_line(&style.name, font.clone(), wrap, TEXT));
        if galley.elided {
            galley = painter.layout_job(one_line(&style.short, font, wrap, TEXT));
        }
        painter.galley(band.center() - galley.size() / 2.0, galley, TEXT);
        top = band.bottom();
    }
    // Words only where they fit; a small tile's tooltip carries them.
    let labelled = rect.width() > 90.0 && rect.height() > 90.0;
    let mut bottom = rect.bottom() - 6.0;
    if labelled {
        let wrap = (rect.width() - 14.0).max(40.0);
        let status = std::iter::once(label.headline)
            .chain(label.detail.as_deref().filter(|_| detail_on_tile));
        let mut galleys: Vec<_> = status
            .map(|text| {
                let mut job = one_line(text, egui::FontId::proportional(STATUS_FONT), wrap, MUTED);
                job.wrap.max_rows = 2;
                painter.layout_job(job)
            })
            .collect();
        // A shelf tile carries the title like a cover would. Larger pictures
        // (Game Details) sit beside the page's own heading, so they do not
        // repeat it.
        if !detail_on_tile {
            let mut job = one_line(title, egui::FontId::proportional(TITLE_FONT), wrap, TEXT);
            job.wrap.max_rows = 2;
            job.wrap.break_anywhere = false;
            galleys.insert(0, painter.layout_job(job));
        }
        for galley in galleys.into_iter().rev() {
            bottom -= galley.size().y;
            painter.galley(
                egui::pos2(rect.center().x - galley.size().x / 2.0, bottom),
                galley,
                TEXT,
            );
            bottom -= 2.0;
        }
    }
    let room = egui::Rect::from_min_max(
        egui::pos2(rect.left(), top + 4.0),
        egui::pos2(rect.right(), bottom - 2.0),
    );
    let side = room.width().min(room.height()) * if labelled { 0.8 } else { 0.85 };
    if side >= 16.0 {
        imagery.paint_platform_asset(
            ui,
            egui::Rect::from_center_size(room.center(), egui::vec2(side, side)),
            &style.asset_id,
            style.fallback,
            Color32::from_white_alpha(170),
        );
    }
    // Blue: the same dashed outline as before, so it never reads as a cover.
    if theme::blue_profile(ui) {
        crate::ui::components::paint_dashed_outline(&painter, rect, theme::blue::BORDER_STRONG);
    } else {
        painter.rect_stroke(rect, 8.0, theme::border(ui), egui::StrokeKind::Inside);
    }
}

/// An elided text job (one row unless the caller raises `max_rows`). Left
/// aligned so the galley's size is its painted extent; callers centre it.
fn one_line(text: &str, font: egui::FontId, wrap: f32, color: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, wrap);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job
}

fn scale(color: Color32, factor: f32) -> Color32 {
    let channel = |value: u8| (f32::from(value) * factor).round() as u8;
    Color32::from_rgb(channel(color.r()), channel(color.g()), channel(color.b()))
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Common short names for the registry's canonical ids. Plain abbreviations
/// only: no logos or trademarks' artwork. Unlisted platforms use their name.
fn short_label(id: &str) -> Option<&'static str> {
    Some(match id {
        "3DO" => "3DO",
        "Acorn Archimedes" => "Archimedes",
        "Acorn Electron" => "Electron",
        "AmigaCD32" => "CD32",
        "Amstrad CPC" => "CPC",
        "Apple II" => "Apple II",
        "Macintosh" => "Mac",
        "Arcade" => "Arcade",
        "Atari2600" => "2600",
        "Atari5200" => "5200",
        "Atari7800" => "7800",
        "Atari 8-bit" => "Atari 8-bit",
        "Atari Jaguar" => "Jaguar",
        "Atari Lynx" => "Lynx",
        "AtariST" => "Atari ST",
        "WonderSwan" => "WS",
        "WonderSwan Color" => "WSC",
        "BBC Micro" => "BBC",
        "ColecoVision" => "Coleco",
        "Commodore 128" => "C128",
        "Commodore 64" => "C64",
        "Amiga" => "Amiga",
        "Commodore CDTV" => "CDTV",
        "VIC-20" => "VIC-20",
        "Dragon / Tandy CoCo" => "CoCo",
        "Enterprise" => "Enterprise",
        "FM Towns" => "FM Towns",
        "Vectrex" => "Vectrex",
        "Intellivision" => "Intv",
        "Xbox" => "Xbox",
        "Xbox360" => "X360",
        "DOS" => "DOS",
        "NEC PC-8801" => "PC-88",
        "PC-98" | "NEC PC-9801" => "PC-98",
        "NeoGeo" => "Neo Geo",
        "NeoGeo64" => "NG64",
        "Neo Geo CD" => "NGCD",
        "Neo Geo Pocket" => "NGP",
        "Neo Geo Pocket Color" => "NGPC",
        "Nintendo 3DS" => "3DS",
        "N64" => "N64",
        "Nintendo DS" => "DS",
        "NES" => "NES",
        "Game Boy" => "GB",
        "Game Boy Advance" => "GBA",
        "Game Boy Color" => "GBC",
        "GameCube" => "GC",
        "Pokemon Mini" => "Mini",
        "Switch" => "Switch",
        "Virtual Boy" => "VB",
        "Wii" => "Wii",
        "WiiU" => "Wii U",
        "NGage" => "N-Gage",
        "PC Engine" => "PCE",
        "PC Engine CD" => "PCE-CD",
        "Philips CD-i" => "CD-i",
        "Sega 32X" => "32X",
        "Dreamcast" => "DC",
        "GameGear" => "GG",
        "MasterSystem" => "SMS",
        "MegaDrive" => "MD",
        "Sega CD" => "MCD",
        "Sega Pico" => "Pico",
        "Saturn" => "Saturn",
        "Sharp X68000" => "X68000",
        "PSX" => "PS1",
        "PS2" => "PS2",
        "PS3" => "PS3",
        "PS4" => "PS4",
        "PSP" => "PSP",
        "PlayStation Vita" => "Vita",
        "SNES" => "SNES",
        "Thomson MO" => "MO",
        "Thomson TO" => "TO",
        "TurboGrafx-16" => "TG-16",
        "Watara Supervision" => "SV",
        "ZX Spectrum" => "ZX",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color32) -> f32 {
        let linear = |value: u8| {
            let value = f32::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    }

    #[test]
    fn every_plate_keeps_the_label_readable() {
        let text = luminance(TEXT);
        for plate in PALETTE.iter().chain([&UNKNOWN_PLATE]) {
            let contrast = (text + 0.05) / (luminance(*plate) + 0.05);
            assert!(contrast >= 4.5, "{plate:?}: {contrast}");
        }
    }

    #[test]
    fn known_platforms_get_their_registry_name_short_label_and_hardware_art() {
        let snes = Style::for_platform("SNES");
        assert_eq!(snes.name, "Super Nintendo Entertainment System");
        assert_eq!(snes.short, "SNES");
        assert_eq!(snes.asset_id, "snes");
        assert!(PALETTE.contains(&snes.band));
        // Display names and aliases resolve to the same platform and plate.
        assert_eq!(
            Style::for_platform("Super Nintendo Entertainment System"),
            snes
        );
        assert_eq!(Style::for_platform("Game Boy Advance").short, "GBA");
        assert_eq!(Style::for_platform("PSX").short, "PS1");
        // Neighbouring systems are told apart by colour, not only by text.
        let distinct: std::collections::HashSet<_> = [
            "SNES",
            "NES",
            "MegaDrive",
            "PSX",
            "Game Boy Advance",
            "Amiga",
        ]
        .into_iter()
        .map(|platform| Style::for_platform(platform).band)
        .collect();
        assert!(distinct.len() >= 5, "{distinct:?}");
        // Every registry short label is short enough for the list tile.
        for platform in archivefs_core::platform::PLATFORMS {
            if let Some(short) = short_label(platform.id) {
                assert!(short.chars().count() <= 11, "{}: {short}", platform.id);
            }
        }
    }

    #[test]
    fn unknown_and_unrecognised_platforms_fall_back_honestly() {
        let unknown = Style::for_platform(UNKNOWN_PLATFORM);
        assert_eq!(unknown.band, UNKNOWN_PLATE);
        assert_eq!(unknown.name, UNKNOWN_PLATFORM);
        assert_eq!(unknown.fallback, "unknown");
        assert_eq!(Style::for_platform("").band, UNKNOWN_PLATE);
        // A platform this build does not know keeps its stored text verbatim,
        // a stable colour and the generic glyph; it is never renamed.
        let stored = Style::for_platform("Future Console 9");
        assert_eq!(stored.name, "Future Console 9");
        assert_eq!(stored.short, "Future Console 9");
        assert_eq!(stored.fallback, "unknown");
        assert!(PALETTE.contains(&stored.band));
        assert_eq!(Style::for_platform("Future Console 9"), stored);
    }

    #[test]
    fn styles_are_cached_per_platform_not_per_game() {
        let mut styles = Styles::default();
        let first = styles.get("SNES");
        for _ in 0..10_000 {
            assert!(Arc::ptr_eq(&first, &styles.get("SNES")));
            styles.get("MegaDrive");
            styles.get(UNKNOWN_PLATFORM);
        }
        assert_eq!(styles.len(), 3);
    }
}
