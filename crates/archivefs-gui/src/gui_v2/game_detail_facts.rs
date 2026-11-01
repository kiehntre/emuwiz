//! Game Details "At a glance": saved facts about one catalogue entry, plus
//! links between the entries of one disc set.
//!
//! Presentation only. Each value comes from existing saved or checked
//! evidence and carries its limits; anything else reads "Not recorded". A file
//! name is never read as a region, and no row here says a game can be played:
//! that answer belongs to the readiness card alone.

use super::{
    launch_readiness_summary::{
        GameReadinessSummary, ReadinessAction, ReadinessFreshness, ReadinessPresentationState,
    },
    library::{Detail, Game, Library, media_kind_label},
};
use crate::ui::theme;
use archivefs_core::{ArchiveKind, MANUAL_PLATFORM_SOURCE};
use eframe::egui::{self, RichText};

const NOT_RECORDED: &str = "Not recorded";
const CHECKING: &str = "Checking…";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Tone {
    Plain,
    Good,
    Warning,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Fact {
    pub label: &'static str,
    pub value: String,
    pub note: Option<String>,
    pub tone: Tone,
}

/// The evidence available right now. `None` means "not known yet".
#[derive(Clone, Copy, Default)]
pub(super) struct Sources<'a> {
    pub detail: Option<&'a Detail>,
    pub detail_failed: bool,
    pub readiness: Option<&'a GameReadinessSummary>,
    /// The selected value and its source, from the shared metadata resolver.
    pub region: Option<(&'a str, &'a str)>,
    pub region_checked: bool,
    pub discs: Option<&'a str>,
}

/// One disc set as seen from one game page.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct DiscSummary {
    pub game: i64,
    pub library: usize,
    pub headline: String,
    /// Label and game of each other disc that has its own library entry.
    pub links: Vec<(String, i64)>,
}

/// Remembers the page's disc set so the catalogue is walked once per game and
/// the facts can name it without inspecting the disc files a second time.
#[derive(Default)]
pub(super) struct State {
    pub discs: Option<DiscSummary>,
}

impl State {
    /// True when this game's set was not the one remembered.
    pub(super) fn note_discs(
        &mut self,
        game: i64,
        library: &std::sync::Arc<Library>,
        guidance: &crate::multidisc_launch_view::DiscGuidance,
    ) -> bool {
        let address = std::sync::Arc::as_ptr(library) as usize;
        if let Some(discs) = self
            .discs
            .as_mut()
            .filter(|discs| discs.game == game && discs.library == address)
        {
            discs.headline.clone_from(&guidance.headline);
            return false;
        }
        self.discs = Some(DiscSummary {
            game,
            library: address,
            headline: guidance.headline.clone(),
            links: disc_links(library, game, guidance),
        });
        true
    }

    pub(super) fn discs_for(
        &self,
        game: i64,
        library: &std::sync::Arc<Library>,
    ) -> Option<&DiscSummary> {
        let address = std::sync::Arc::as_ptr(library) as usize;
        self.discs
            .as_ref()
            .filter(|discs| discs.game == game && discs.library == address)
    }
}

fn disc_links(
    library: &Library,
    game: i64,
    guidance: &crate::multidisc_launch_view::DiscGuidance,
) -> Vec<(String, i64)> {
    guidance
        .discs
        .iter()
        .filter(|disc| !disc.this_entry)
        .filter_map(|disc| {
            let path = disc.path.as_deref()?;
            let found = library
                .games
                .iter()
                .find(|other| other.archive.absolute_path == path)?;
            (found.archive.id != game).then(|| (disc.label.clone(), found.archive.id))
        })
        .collect()
}

pub(super) fn facts(game: &Game, sources: Sources<'_>) -> Vec<Fact> {
    let mut rows = vec![system(game), region(sources), format(game), size(game)];
    rows.push(availability(game, sources));
    if !game.is_preservation_only() {
        rows.push(emulator(sources));
    }
    if let Some(discs) = sources.discs {
        rows.push(plain("Discs", discs.to_string(), None));
    }
    rows
}

fn plain(label: &'static str, value: String, note: Option<String>) -> Fact {
    Fact {
        label,
        value,
        note,
        tone: Tone::Plain,
    }
}

fn system(game: &Game) -> Fact {
    let note = game.archive.platform_source.as_deref().map(|source| {
        if source == MANUAL_PLATFORM_SOURCE {
            "Set by you".to_string()
        } else {
            "Worked out from the file's location".to_string()
        }
    });
    plain("System", game.platform.clone(), note)
}

fn region(sources: Sources<'_>) -> Fact {
    match sources.region {
        Some((value, source)) => plain("Region", value.to_string(), Some(format!("From {source}"))),
        None if sources.region_checked => plain("Region", NOT_RECORDED.into(), None),
        None => plain("Region", CHECKING.into(), None),
    }
}

fn is_folder(game: &Game) -> bool {
    matches!(
        ArchiveKind::from_storage(&game.archive.archive_kind),
        Some(ArchiveKind::ArcadeSetDirectory | ArchiveKind::ScummVmDirectory)
    )
}

/// A printable file extension, or none at all for a folder or an odd name.
fn extension(game: &Game) -> Option<String> {
    if is_folder(game) {
        return None;
    }
    let extension = game.archive.absolute_path.extension()?.to_str()?;
    (!extension.is_empty()
        && extension.len() <= 8
        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    .then(|| extension.to_ascii_uppercase())
}

/// An index file names the disc's other files without holding their data.
fn is_index_file(game: &Game) -> bool {
    extension(game).is_some_and(|extension| matches!(extension.as_str(), "CUE" | "GDI" | "M3U"))
}

fn format(game: &Game) -> Fact {
    let kind = media_kind_label(&game.archive.archive_kind);
    let value = match (extension(game), kind) {
        (Some(extension), "ZIP" | "7z" | "RAR" | "LHA" | "Media") => extension,
        (Some(extension), kind) => format!("{extension} · {kind}"),
        (None, "Media") => NOT_RECORDED.to_string(),
        (None, kind) => kind.to_string(),
    };
    plain("File format", value, None)
}

fn size(game: &Game) -> Fact {
    let Some(bytes) = game.archive.size_bytes else {
        return plain("Size", NOT_RECORDED.into(), None);
    };
    let note = if is_index_file(game) {
        "This index file only, not the disc data it lists"
    } else {
        "At the last scan"
    };
    plain(
        "Size",
        super::library_reconciliation::human_bytes(bytes),
        Some(note.into()),
    )
}

fn availability(game: &Game, sources: Sources<'_>) -> Fact {
    let scope = is_index_file(game)
        .then(|| "This index file only. Can I play it? checks the files it lists.".to_string());
    let warning = |value: &str, note: Option<String>| Fact {
        label: "File",
        value: value.to_string(),
        note,
        tone: Tone::Warning,
    };
    let Some(detail) = sources.detail else {
        return if sources.detail_failed {
            warning("Could not be checked", None)
        } else {
            plain("File", CHECKING.into(), None)
        };
    };
    if !detail.file_present {
        return warning(
            "Not found",
            Some("The file was moved or removed, or its drive is disconnected.".into()),
        );
    }
    if game.attention {
        return warning(
            "Found, needs attention",
            Some(format!(
                "Saved status: {}. The file exists, but a scan flagged a problem.",
                game.archive.last_known_health
            )),
        );
    }
    if is_folder(game) {
        // A folder's own size and date say nothing about the files inside.
        return plain("File", "Folder found".into(), None);
    }
    if !detail.unchanged {
        return warning("Found, changed since the last scan", scope);
    }
    Fact {
        label: "File",
        value: "Found, unchanged since the last scan".to_string(),
        note: scope,
        tone: Tone::Good,
    }
}

fn emulator(sources: Sources<'_>) -> Fact {
    let Some(summary) = sources.readiness else {
        return plain("Emulator", CHECKING.into(), None);
    };
    if summary.freshness == ReadinessFreshness::Stale {
        return plain("Emulator", "Needs a recheck".into(), None);
    }
    if let Some(emulator) = &summary.emulator {
        return plain(
            "Emulator",
            emulator.name.clone(),
            Some(format!("{} · chosen by the launch check", emulator.profile)),
        );
    }
    if summary.status == ReadinessPresentationState::Checking {
        return plain("Emulator", CHECKING.into(), None);
    }
    if summary.primary_action == Some(ReadinessAction::ChooseEmulator) {
        return plain("Emulator", "Choose one in Can I play it?".into(), None);
    }
    match sources.detail.map(|detail| detail.installed.as_slice()) {
        Some([]) => plain("Emulator", "None found".into(), None),
        Some(installed) => plain(
            "Emulator",
            "None chosen for this game".into(),
            Some(format!("On this computer: {}", installed.join(", "))),
        ),
        None => plain("Emulator", CHECKING.into(), None),
    }
}

/// The facts as a two-column list, then the one read-only quick action.
pub(super) fn show(ui: &mut egui::Ui, game: &Game, rows: &[Fact]) {
    ui.label(RichText::new("At a glance").color(theme::muted(ui)));
    let width = ui.available_width();
    egui::Grid::new(("v2_detail_facts", game.archive.id))
        .num_columns(2)
        .spacing(egui::vec2(16.0, 4.0))
        .min_col_width(88.0)
        .max_col_width((width - 104.0).max(88.0))
        .show(ui, |ui| {
            for row in rows {
                ui.label(RichText::new(row.label).color(theme::muted(ui)));
                ui.vertical(|ui| {
                    let color = match row.tone {
                        Tone::Plain => ui.visuals().text_color(),
                        Tone::Good => theme::success(ui),
                        Tone::Warning => theme::warning(ui),
                    };
                    ui.add(egui::Label::new(RichText::new(&row.value).color(color)).wrap());
                    if let Some(note) = &row.note {
                        ui.add(
                            egui::Label::new(RichText::new(note).small().color(theme::muted(ui)))
                                .wrap(),
                        );
                    }
                });
                ui.end_row();
            }
        });
    if let Some(path) = game.archive.absolute_path.to_str()
        && ui
            .button("Copy file path")
            .on_hover_text("Copies the path as text. Your files are not touched.")
            .clicked()
    {
        ui.ctx().copy_text(path.to_string());
    }
}

/// Buttons to the set's other discs. Going there only opens that game's page.
pub(super) fn show_disc_links(ui: &mut egui::Ui, links: &[(String, i64)]) -> Option<i64> {
    let mut chosen = None;
    if !links.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for (label, id) in links {
                if ui.button(format!("Go to {label}")).clicked() {
                    chosen = Some(*id);
                }
            }
        });
    }
    chosen
}

#[cfg(test)]
#[path = "game_detail_facts/tests.rs"]
mod tests;
