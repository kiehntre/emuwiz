//! Game Details "At a glance": saved facts about one catalogue entry, plus
//! links between the entries of one disc set.
//!
//! Presentation only. Each value comes from existing saved or checked
//! evidence and carries its limits; anything else reads "Not recorded". The
//! one inference is a region hint from the file name, labelled as a hint on
//! screen and never stored. No row here says a game can be played: that answer
//! belongs to the readiness card alone.

use super::{
    launch_readiness_summary::{
        GameReadinessSummary, ReadinessAction, ReadinessFreshness, ReadinessPresentationState,
    },
    library::{Detail, Game, SharedLibrary, media_kind_label},
};
use crate::{
    media_sets_page::record_from_catalogue,
    multidisc_launch_view::{DiscGuidance, MultiDiscLaunchContext},
    ui::theme,
};
use archivefs_core::{
    ArchiveKind, MANUAL_PLATFORM_SOURCE,
    media_set::{MediaRecord, index_media, resolve_index},
};
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
    /// The region and catalogue of a saved exact DAT match of this file.
    pub dat_region: Option<(&'a str, &'a str)>,
    /// That match still holds against the current catalogue.
    pub dat_current: bool,
    pub discs: Option<&'a str>,
}

/// The disc set of one entry, worked out away from the UI thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DiscLookup {
    pub game: i64,
    pub library: usize,
    pub guidance: DiscGuidance,
    /// Label and game of each other disc that has its own library entry.
    pub links: Vec<(String, i64)>,
}

/// The current game's lookup, and which game and library it was asked for.
#[derive(Default)]
pub(super) struct State {
    asked: Option<(i64, usize)>,
    found: Option<DiscLookup>,
}

impl State {
    /// True once per game and library, when a lookup is worth starting.
    pub(super) fn should_look_up(&mut self, game: &Game, library: &SharedLibrary) -> bool {
        let key = (game.archive.id, library_key(library));
        if self.asked == Some(key) {
            return false;
        }
        self.asked = Some(key);
        self.found = None;
        names_a_disc(game)
    }

    /// A late answer for a game or library no longer on screen is dropped.
    pub(super) fn finish(&mut self, found: DiscLookup) {
        if self.asked == Some((found.game, found.library)) {
            self.found = Some(found);
        }
    }

    pub(super) fn discs_for(&self, game: i64, library: &SharedLibrary) -> Option<&DiscLookup> {
        self.found
            .as_ref()
            .filter(|found| found.game == game && found.library == library_key(library))
    }
}

fn library_key(library: &SharedLibrary) -> usize {
    std::sync::Arc::as_ptr(library) as usize
}

/// Filename evidence only, and only a hint: most games stop here, so opening
/// them does no catalogue work at all.
fn names_a_disc(game: &Game) -> bool {
    record_from_catalogue(&game.archive).is_some_and(|record| carries_disc_number(&record))
}

fn carries_disc_number(record: &MediaRecord) -> bool {
    record
        .evidence
        .iter()
        .any(|evidence| evidence.ordinal.is_some() || evidence.expected_count.is_some())
}

/// The set around one entry, resolved by the core engine from that entry's
/// folder alone. Runs on the background worker: it checks disc files, which
/// can stall on a slow or disconnected drive.
pub(super) fn look_up(library: &SharedLibrary, id: i64) -> Option<DiscLookup> {
    let started = std::time::Instant::now();
    let game = library.game(id)?;
    let path = &game.archive.absolute_path;
    let folder = path.parent()?;
    let neighbours: Vec<&Game> = library
        .games
        .iter()
        .filter(|other| {
            other.archive.last_verified_missing_at.is_none()
                && other.archive.absolute_path.parent() == Some(folder)
        })
        .collect();
    let records = neighbours
        .iter()
        .filter_map(|other| record_from_catalogue(&other.archive))
        .filter(carries_disc_number)
        .collect();
    let sets = resolve_index(index_media(records)).sets;
    let context = MultiDiscLaunchContext::for_content(&sets, path, &[]);
    let found = context
        .as_ref()
        .and_then(|context| context.sets.first())
        .map(|set| {
            let guidance = crate::multidisc_launch_view::guidance(set, path, None);
            let links = disc_links(&neighbours, id, &guidance);
            DiscLookup {
                game: id,
                library: library_key(library),
                guidance,
                links,
            }
        });
    log::debug!(
        "gui_v2 disc lookup: {} of {} entries in the folder, {} ms",
        neighbours.len(),
        library.games.len(),
        started.elapsed().as_millis()
    );
    found
}

fn disc_links(neighbours: &[&Game], game: i64, guidance: &DiscGuidance) -> Vec<(String, i64)> {
    guidance
        .discs
        .iter()
        .filter(|disc| !disc.this_entry)
        .filter_map(|disc| {
            let path = disc.path.as_deref()?;
            let found = neighbours
                .iter()
                .find(|other| other.archive.absolute_path == path)?;
            (found.archive.id != game).then(|| (disc.label.clone(), found.archive.id))
        })
        .collect()
}

pub(super) fn facts(game: &Game, sources: Sources<'_>) -> Vec<Fact> {
    let mut rows = vec![
        system(game),
        region(game, sources),
        format(game),
        size(game),
    ];
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

/// Saved metadata first, then a saved DAT match, then a labelled hint.
fn region(game: &Game, sources: Sources<'_>) -> Fact {
    if let Some((value, source)) = sources.region {
        return plain("Region", value.to_string(), Some(format!("From {source}")));
    }
    if let Some((value, catalogue)) = sources.dat_region {
        let note = if sources.dat_current {
            format!("From the verified match in {catalogue}")
        } else {
            // Not "needs a recheck": while it is still being re-read the
            // Identity line says "Checking verification".
            format!("From a saved match in {catalogue}. Not confirmed as current.")
        };
        return plain("Region", value.to_string(), Some(note));
    }
    if !sources.region_checked || (sources.detail.is_none() && !sources.detail_failed) {
        return plain("Region", CHECKING.into(), None);
    }
    match (!is_folder(game))
        .then(|| filename_region_hint(&game.archive.absolute_path))
        .flatten()
    {
        Some(hint) => plain(
            "Region",
            format!("{hint} (filename hint)"),
            Some("Read from the file name. Not verified.".into()),
        ),
        None => plain("Region", NOT_RECORDED.into(), None),
    }
}

/// Whole tags only. Abbreviations such as `U`, `E`, `JP` or `UK` stay out:
/// the same letters mean other things in other naming schemes.
const FILENAME_REGION_TAGS: &[&str] = &[
    "Argentina",
    "Asia",
    "Australia",
    "Austria",
    "Belgium",
    "Brazil",
    "Canada",
    "Chile",
    "China",
    "Croatia",
    "Denmark",
    "Europe",
    "Finland",
    "France",
    "Germany",
    "Greece",
    "Hong Kong",
    "Hungary",
    "India",
    "Ireland",
    "Italy",
    "Japan",
    "Korea",
    "Latin America",
    "Mexico",
    "Netherlands",
    "New Zealand",
    "Norway",
    "Poland",
    "Portugal",
    "Russia",
    "Scandinavia",
    "Spain",
    "Sweden",
    "Switzerland",
    "Taiwan",
    "Turkey",
    "United Kingdom",
    "USA",
    "World",
];

/// A display hint from the file's own name, never its folders, and never
/// stored. It needs one bracketed tag, after a title, made only of whole
/// region names: `Game (USA, Europe)`. Two different tags are ambiguous.
pub(super) fn filename_region_hint(path: &std::path::Path) -> Option<String> {
    let name = path.file_stem()?.to_str()?;
    let mut tags = Vec::new();
    let mut rest = name;
    let mut title_seen = false;
    while let Some(open) = rest.find('(') {
        title_seen |= !rest[..open].trim().is_empty();
        let after = &rest[open + 1..];
        let close = after.find(')')?;
        let group = &after[..close];
        let parts: Vec<&str> = group.split(',').map(str::trim).collect();
        if parts.iter().all(|part| FILENAME_REGION_TAGS.contains(part)) {
            if !title_seen {
                return None;
            }
            tags.push(parts.join(", "));
        }
        rest = &after[close + 1..];
    }
    tags.dedup();
    match tags.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
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
        .spacing(egui::vec2(16.0, 8.0))
        .min_col_width(88.0)
        .max_col_width((width - 104.0).max(88.0))
        .show(ui, |ui| {
            for row in rows {
                // Both cells start at the top, or a two-line value would
                // leave its label beside the row above.
                ui.vertical(|ui| {
                    ui.label(RichText::new(row.label).color(theme::muted(ui)));
                });
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
