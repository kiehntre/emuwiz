//! Game Details artwork showcase: the cover with a plain caption saying where
//! it came from, and a screenshot viewer with a large preview.
//!
//! Display only. Every picture comes from a reference the media index already
//! stored, through the existing bounded loaders (`Artwork::request`), so this
//! never downloads, contacts ScreenScraper or writes anything. The caption
//! never turns a stale or unreadable reference into a good-looking one: a
//! stale RomM record keeps its warning, and a stored file that is gone or
//! cannot be decoded says so instead of passing for "no artwork".

use eframe::egui::{self, RichText};

use crate::gui_v2::{
    App,
    artwork::Picture,
    library::Game,
    media_sources::{Kind, MediaIndex, Source},
};
use crate::ui::theme;
use archivefs_core::identity_source::model::ExternalVerification;

/// Where a stored picture reference points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::gui_v2) enum Origin {
    /// An image file found on this computer (ES-DE, LaunchBox or a media folder).
    LocalFile,
    /// The artwork reference in a saved RomM record.
    SavedRomm,
}

/// What is known about one picture right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::gui_v2) enum ArtworkState {
    /// The media index is still being built.
    Checking,
    /// The index has no stored reference for this picture.
    NoneStored,
    Paused,
    Loading {
        origin: Origin,
        stale: bool,
    },
    Shown {
        origin: Origin,
        stale: bool,
    },
    /// A reference exists, but the picture could not be produced: the file is
    /// gone, unsafe, too large or not a valid image.
    Unreadable {
        origin: Origin,
        stale: bool,
        reason: String,
    },
    /// RomM cannot serve the picture right now. Not the same as missing.
    ProviderUnavailable {
        stale: bool,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::gui_v2) enum Tone {
    Plain,
    Warning,
}

/// The words shown under a picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::gui_v2) struct Caption {
    pub headline: String,
    pub detail: Option<String>,
    pub tone: Tone,
}

/// Pure: the state of `kind` for `game`, from the index and loader results.
pub(in crate::gui_v2) fn state(
    index: Option<&MediaIndex>,
    game: i64,
    kind: Kind,
    picture: Option<&Picture>,
    paused: bool,
) -> ArtworkState {
    let Some(index) = index else {
        return ArtworkState::Checking;
    };
    let Some(source) = index.source(game, kind) else {
        return ArtworkState::NoneStored;
    };
    let (origin, stale) = match source {
        Source::Local(_) => (Origin::LocalFile, false),
        Source::Remote { record, .. } => (
            Origin::SavedRomm,
            record.verification == ExternalVerification::Stale,
        ),
    };
    match picture {
        Some(Picture::Ready { .. }) => ArtworkState::Shown { origin, stale },
        Some(Picture::Failed(reason)) => ArtworkState::Unreadable {
            origin,
            stale,
            reason: reason.clone(),
        },
        Some(Picture::Unavailable { message, .. }) => ArtworkState::ProviderUnavailable {
            stale,
            reason: message.clone(),
        },
        // The loader marks a vanished reference `Missing` too.
        Some(Picture::Missing) => ArtworkState::NoneStored,
        Some(Picture::Loading) | None if paused => ArtworkState::Paused,
        Some(Picture::Loading) | None => ArtworkState::Loading { origin, stale },
    }
}

const STALE_DETAIL: &str = "RomM's saved record is out of date because the game file changed or disappeared. The picture is display evidence only. Refresh the RomM index to recheck it.";

/// Pure: the caption for a picture named `what` ("cover", "screenshot").
/// `None` while the tile itself already says what is happening.
pub(in crate::gui_v2) fn caption(what: &str, state: &ArtworkState) -> Option<Caption> {
    let plain = |headline: String, detail: Option<String>| Caption {
        headline,
        detail,
        tone: Tone::Plain,
    };
    let warning = |headline: String, detail: Option<String>| Caption {
        headline,
        detail,
        tone: Tone::Warning,
    };
    match state {
        ArtworkState::Checking | ArtworkState::Paused | ArtworkState::Loading { stale: false, .. } => {
            None
        }
        ArtworkState::NoneStored => Some(plain(
            format!("No {what} saved for this game"),
            Some("EmuWiz shows only artwork your providers already saved. Artwork, Manuals & Extras shows where it looked.".into()),
        )),
        ArtworkState::Loading { stale: true, .. } | ArtworkState::Shown { stale: true, .. } => {
            Some(warning("Saved RomM artwork · record is stale".into(), Some(STALE_DETAIL.into())))
        }
        ArtworkState::Shown {
            origin: Origin::LocalFile,
            ..
        } => Some(plain("From a saved image on this computer".into(), None)),
        ArtworkState::Shown {
            origin: Origin::SavedRomm,
            ..
        } => Some(plain("From the saved RomM record".into(), None)),
        ArtworkState::Unreadable {
            origin,
            stale,
            reason,
        } => {
            let headline = match origin {
                Origin::LocalFile => format!("The saved {what} file can't be shown"),
                Origin::SavedRomm => format!("The saved RomM {what} can't be shown"),
            };
            let mut detail = reason.clone();
            if *stale {
                detail.push_str(" RomM's saved record is also stale.");
            }
            Some(warning(headline, Some(detail)))
        }
        ArtworkState::ProviderUnavailable { stale, reason } => {
            let mut detail = reason.clone();
            if *stale {
                detail.push_str(" RomM's saved record is also stale.");
            }
            Some(warning(
                format!("RomM can't provide this {what} right now"),
                Some(detail),
            ))
        }
    }
}

fn show_caption(ui: &mut egui::Ui, caption: &Caption) {
    let color = match caption.tone {
        Tone::Plain => theme::muted(ui),
        Tone::Warning => theme::warning(ui),
    };
    ui.add(egui::Label::new(RichText::new(&caption.headline).color(color)).wrap());
    if let Some(detail) = &caption.detail {
        ui.add(egui::Label::new(RichText::new(detail).small().color(theme::muted(ui))).wrap());
    }
}

/// Picture size for the large screenshot preview: 4:3, at most 640 wide.
pub(in crate::gui_v2) fn preview_size(available_width: f32) -> egui::Vec2 {
    let width = available_width.clamp(200.0, 640.0);
    egui::vec2(width, width * 0.75)
}

const THUMBNAIL: egui::Vec2 = egui::vec2(128.0, 96.0);

fn selection_id(game: i64) -> egui::Id {
    egui::Id::new(("v2_detail_screenshot_selected", game))
}

impl App {
    fn artwork_state(&self, game: i64, kind: Kind) -> ArtworkState {
        let key = self.artwork.key(game, kind);
        state(
            self.artwork.index.as_deref(),
            game,
            kind,
            self.artwork.pictures.get(&key),
            self.artwork.paused,
        )
    }

    /// The Game Details cover, with a caption saying where it came from and
    /// anything wrong with it.
    pub(in crate::gui_v2) fn detail_cover(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        size: egui::Vec2,
    ) {
        ui.vertical(|ui| {
            ui.set_width(size.x);
            self.picture(ui, game, Kind::Cover, size);
            if let Some(caption) =
                caption("cover", &self.artwork_state(game.archive.id, Kind::Cover))
            {
                ui.add_space(6.0);
                show_caption(ui, &caption);
            }
        });
    }

    /// Screenshots: one large preview, with numbered thumbnails once the
    /// player asks for them. Before that only the first screenshot loads, so
    /// opening a game still costs at most two pictures.
    pub(in crate::gui_v2) fn detail_screenshots(&mut self, ui: &mut egui::Ui, game: &Game) {
        let id = game.archive.id;
        let count = self
            .artwork
            .index
            .as_ref()
            .and_then(|index| index.screenshots.get(&id))
            .map_or(0, Vec::len);
        ui.add_space(theme::SPACE_MD);
        ui.label(theme::section_title("Screenshots"));
        if self.artwork.index.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Looking for screenshots…");
            });
            return;
        }
        if count == 0 {
            ui.weak("No screenshots found for this game.");
            return;
        }
        let stored: usize = ui.data(|data| data.get_temp(selection_id(id))).unwrap_or(0);
        let selected = if self.screenshots {
            stored.min(count - 1)
        } else {
            0
        };
        let size = preview_size(ui.available_width());
        self.picture(ui, game, Kind::Screenshot(selected), size);
        ui.add_space(6.0);
        ui.label(RichText::new(format!("Screenshot {} of {count}", selected + 1)).strong());
        if let Some(caption) = caption(
            "screenshot",
            &self.artwork_state(id, Kind::Screenshot(selected)),
        ) {
            show_caption(ui, &caption);
        }
        let key = self.artwork.key(id, Kind::Screenshot(selected));
        if matches!(
            self.artwork.pictures.get(&key),
            Some(Picture::Failed(_) | Picture::Unavailable { .. })
        ) && ui
            .button(format!("Retry screenshot {}", selected + 1))
            .clicked()
        {
            self.artwork.retry(key);
        }
        if count == 1 {
            return;
        }
        if !self.screenshots {
            if ui
                .button(format!("Show all screenshots ({count})"))
                .clicked()
            {
                self.screenshots = true;
            }
            return;
        }
        let mut chosen = selected;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    selected > 0,
                    egui::Button::new("Previous screenshot").min_size(egui::vec2(0.0, 36.0)),
                )
                .clicked()
            {
                chosen = selected - 1;
            }
            if ui
                .add_enabled(
                    selected + 1 < count,
                    egui::Button::new("Next screenshot").min_size(egui::vec2(0.0, 36.0)),
                )
                .clicked()
            {
                chosen = selected + 1;
            }
        });
        ui.add_space(theme::SPACE_SM);
        let gap = ui.spacing().item_spacing.x;
        let columns = ((ui.available_width() + gap) / (THUMBNAIL.x + gap))
            .floor()
            .max(1.0) as usize;
        egui::Grid::new(("v2_detail_screenshot_thumbnails", id))
            .num_columns(columns)
            .spacing(egui::vec2(gap, theme::SPACE_SM))
            .show(ui, |ui| {
                for ordinal in 0..count {
                    ui.vertical(|ui| {
                        ui.set_width(THUMBNAIL.x);
                        self.picture_tile(ui, game, Kind::Screenshot(ordinal), THUMBNAIL);
                        let label = format!("Screenshot {}", ordinal + 1);
                        if ui
                            .add(
                                egui::Button::selectable(ordinal == selected, label)
                                    .min_size(egui::vec2(THUMBNAIL.x, 32.0)),
                            )
                            .clicked()
                        {
                            chosen = ordinal;
                        }
                    });
                    if (ordinal + 1) % columns == 0 {
                        ui.end_row();
                    }
                }
            });
        if chosen != stored {
            ui.data_mut(|data| data.insert_temp(selection_id(id), chosen));
            if chosen != selected {
                ui.ctx().request_repaint();
            }
        }
    }
}

#[cfg(test)]
mod tests;
