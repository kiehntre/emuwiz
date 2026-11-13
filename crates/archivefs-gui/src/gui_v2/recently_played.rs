//! Blue "Continue playing": the games RetroArch itself recorded playing.
//!
//! EmuWiz keeps no play history of its own. RetroArch does: its History
//! playlist (`content_history_path` in `retroarch.cfg`, most recent first).
//! This module reads that file read-only, through the core reader
//! ([`read_native_content_history`]), and matches each entry to a Library game
//! by **exact path only** - after resolving `~` and lexical normalisation
//! (repeated `/`, `.` components). Nothing else is evidence: RetroArch's label,
//! CRC and database name are hints, never identity, so an entry whose path is
//! not a Library game's path is shown as unmatched, never guessed.
//!
//! - Order is the file's order. The file has no times or play counts and
//!   none are shown or invented.
//! - EmuWiz's zipped-cartridge launches run from a fresh temporary
//!   `emuwiz-zip-rom-*` folder; RetroArch records that temporary path, and no
//!   saved map leads back to the ZIP, so those entries stay unmatched and say
//!   so. Amiga HDF working copies (named by the original's digest) likewise.
//! - Opening a matched game goes to the existing Game Details page; this
//!   module adds no launch path. Nothing is written anywhere.
//!
//! The file read, the Library path index and the existence checks for
//! unmatched entries run on a worker thread, never during paint.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::emulator_environment::retroarch::content_history::{
    ContentHistoryState, RetroArchContentHistory, read_native_content_history,
};
use archivefs_core::emulator_environment::retroarch::{
    ContentPathKind, DiscoveryEnvironment, RetroArchPlaylistEntry,
};
use archivefs_core::launch::amiga_hdf_working_copy;
use eframe::egui::{self, RichText};

use super::library::{Library, SharedLibrary};
use super::media_sources::Kind;
use super::simple_shell::palette;
use crate::ui::theme;

/// History entries examined (and existence-checked when unmatched).
pub(super) const MAX_ROWS: usize = 200;
/// How stale the list may get while Home stays open.
const REFRESH_AFTER: Duration = Duration::from_secs(30);
/// Prefix of the private folder `zip_cartridge_stage` creates per launch.
const ZIP_STAGE_PREFIX: &str = "emuwiz-zip-rom-";
const TILE: egui::Vec2 = egui::vec2(132.0, 176.0);
const TILE_GAP: f32 = 10.0;
const MAX_COVERS: usize = 8;

/// What one history entry is, as far as exact evidence goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Played {
    /// Its path is exactly this current Library game's path.
    Game(i64),
    /// Its path is the path of more than one Library entry.
    Ambiguous,
    /// An EmuWiz temporary copy extracted from a ZIP for one launch.
    TemporaryCopy,
    /// An EmuWiz private Amiga hard-disk working copy.
    AmigaWorkingCopy,
    /// Not a Library path, and the file is gone.
    FileMissing,
    /// Not a Library path (the file may or may not still exist).
    NotInLibrary,
    /// RetroArch recorded no usable absolute path.
    NoUsablePath,
}

impl Played {
    pub(super) fn words(&self) -> &'static str {
        match self {
            Self::Game(_) => "In your Library",
            Self::Ambiguous => "Matches more than one Library entry",
            Self::TemporaryCopy => "EmuWiz temporary copy from a ZIP; the ZIP is not recorded",
            Self::AmigaWorkingCopy => {
                "EmuWiz Amiga working copy; the original file is not recorded"
            }
            Self::FileMissing => "File missing",
            Self::NotInLibrary => "Played in RetroArch, not in your Library",
            Self::NoUsablePath => "RetroArch recorded no file path",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlayedRow {
    /// RetroArch's label, else the file name it recorded. Display only.
    pub name: String,
    pub core: Option<String>,
    pub path: Option<String>,
    pub played: Played,
}

#[derive(Clone, Debug)]
pub(super) struct Snapshot {
    /// `None`: no `HOME`, so no RetroArch profile to read.
    pub state: Option<ContentHistoryState>,
    pub history_enabled: Option<bool>,
    pub history_file: Option<PathBuf>,
    /// One row per distinct recorded path, in the file's order.
    pub rows: Vec<PlayedRow>,
    pub truncated: bool,
}

impl Snapshot {
    /// Matched Library games, most recent first, each once.
    pub(super) fn games(&self) -> Vec<i64> {
        let mut seen = HashSet::new();
        self.rows
            .iter()
            .filter_map(|row| match row.played {
                Played::Game(id) => seen.insert(id).then_some(id),
                _ => None,
            })
            .collect()
    }

    pub(super) fn unmatched(&self) -> impl Iterator<Item = &PlayedRow> {
        self.rows
            .iter()
            .filter(|row| !matches!(row.played, Played::Game(_)))
    }

    /// One plain sentence when there is nothing to list, or a caveat.
    pub(super) fn notice(&self) -> Option<String> {
        let state = self.state.as_ref()?;
        let unreadable = |why: &str| {
            Some(format!(
                "RetroArch's play history could not be read: {why}. Nothing was changed."
            ))
        };
        match state {
            ContentHistoryState::ConfigMissing => None,
            ContentHistoryState::ConfigUnreadable => unreadable("retroarch.cfg is not readable"),
            ContentHistoryState::PathNotConfigured => {
                unreadable("retroarch.cfg does not set a history file")
            }
            ContentHistoryState::PathUnresolved { raw } => {
                unreadable(&format!("the history file \"{raw}\" is not a full path"))
            }
            ContentHistoryState::FileMissing => {
                Some("RetroArch has not recorded any played games yet.".into())
            }
            ContentHistoryState::FileUnreadable => {
                unreadable("the history file is not a regular file")
            }
            ContentHistoryState::FileTooLarge => unreadable("the history file is too large"),
            ContentHistoryState::FileMalformed => {
                unreadable("the history file is not in RetroArch's playlist format")
            }
            ContentHistoryState::Read if self.rows.is_empty() => {
                Some("RetroArch's play history is empty.".into())
            }
            ContentHistoryState::Read if self.history_enabled == Some(false) => {
                Some("History is turned off in RetroArch, so this list may be out of date.".into())
            }
            ContentHistoryState::Read => None,
        }
    }

    /// Hidden entirely when there is no RetroArch configuration to read.
    pub(super) fn hidden(&self) -> bool {
        matches!(self.state, None | Some(ContentHistoryState::ConfigMissing))
    }
}

/// Exact paths of Library rows. A superseded (moved) row's path leads to the
/// current row that confidently replaces it; retired rows are not listed and
/// do not match. A path held by two different games matches neither.
pub(super) struct PathIndex(HashMap<PathBuf, Option<i64>>);

impl PathIndex {
    pub(super) fn new(library: &Library) -> Self {
        let mut index = HashMap::with_capacity(library.games.len());
        let rows = library
            .games
            .iter()
            .map(|game| (game, game.archive.id))
            .chain(
                library
                    .historical
                    .iter()
                    .map(|game| (game, library.resolve(game.archive.id))),
            );
        for (game, id) in rows {
            let key = lexical(&game.archive.absolute_path);
            index
                .entry(key)
                .and_modify(|seen: &mut Option<i64>| {
                    if *seen != Some(id) {
                        *seen = None;
                    }
                })
                .or_insert(Some(id));
        }
        Self(index)
    }

    fn get(&self, path: &Path) -> Option<Option<i64>> {
        self.0.get(path).copied()
    }
}

/// Lexical normalisation only: never follows symlinks or resolves `..`.
fn lexical(path: &Path) -> PathBuf {
    path.components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect()
}

/// A recorded path as an absolute path: `~` resolved against `home`; any
/// other relative value is unusable.
pub(super) fn normalise(raw: &str, home: Option<&Path>) -> Option<PathBuf> {
    let path = if raw == "~" {
        home?.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home?.join(rest)
    } else {
        PathBuf::from(raw)
    };
    path.is_absolute().then(|| lexical(&path))
}

/// Where EmuWiz's own launch copies live, for honest labels.
#[derive(Clone, Debug, Default)]
pub(super) struct Context {
    pub home: Option<PathBuf>,
    pub temp_dir: PathBuf,
    pub amiga_roots: Vec<PathBuf>,
}

impl Context {
    fn emuwiz_copy(&self, path: &Path) -> Option<Played> {
        let parent = path.parent()?;
        if parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(ZIP_STAGE_PREFIX))
            && parent.parent() == Some(self.temp_dir.as_path())
        {
            return Some(Played::TemporaryCopy);
        }
        let in_root = self.amiga_roots.iter().any(|root| path.starts_with(root));
        let named = path
            .components()
            .any(|c| c.as_os_str() == amiga_hdf_working_copy::WORKING_COPY_DIR);
        (in_root || named).then_some(Played::AmigaWorkingCopy)
    }
}

/// Classifies one entry. `exists` is consulted only for unmatched paths.
fn classify(
    entry: &RetroArchPlaylistEntry,
    index: &PathIndex,
    context: &Context,
    exists: &dyn Fn(&Path) -> Option<bool>,
) -> Played {
    let content = &entry.content_path;
    let raw = match content.kind {
        ContentPathKind::Filesystem | ContentPathKind::Relative => content.raw.as_deref(),
        // Played from inside an archive: the container is the file on disk.
        ContentPathKind::ArchiveMember => content.archive_path.as_deref(),
        _ => None,
    };
    let Some(path) = raw.and_then(|raw| normalise(raw, context.home.as_deref())) else {
        return Played::NoUsablePath;
    };
    match index.get(&path) {
        Some(Some(id)) => Played::Game(id),
        Some(None) => Played::Ambiguous,
        None => context
            .emuwiz_copy(&path)
            .unwrap_or_else(|| match exists(&path) {
                Some(false) => Played::FileMissing,
                _ => Played::NotInLibrary,
            }),
    }
}

fn display_name(entry: &RetroArchPlaylistEntry) -> String {
    if let Some(label) = &entry.label {
        return label.clone();
    }
    let content = &entry.content_path;
    content
        .archive_member_path
        .as_deref()
        .or(content.raw.as_deref())
        .and_then(|raw| Path::new(raw).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unnamed entry".into())
}

/// Pure: the rows for a history, against one Library index.
pub(super) fn build_snapshot(
    history: Option<RetroArchContentHistory>,
    index: &PathIndex,
    context: &Context,
    exists: &dyn Fn(&Path) -> Option<bool>,
) -> Snapshot {
    let Some(history) = history else {
        return Snapshot {
            state: None,
            history_enabled: None,
            history_file: None,
            rows: Vec::new(),
            truncated: false,
        };
    };
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for entry in history.entries.iter().take(MAX_ROWS) {
        // RetroArch keeps one entry per path and core; one row per path.
        if let Some(raw) = &entry.content_path.raw
            && !seen.insert(raw.clone())
        {
            continue;
        }
        rows.push(PlayedRow {
            name: display_name(entry),
            core: entry.core_name.clone(),
            path: entry.content_path.raw.clone(),
            played: classify(entry, index, context, exists),
        });
    }
    Snapshot {
        state: Some(history.state),
        history_enabled: history.history_enabled,
        history_file: history.history_file,
        rows,
        truncated: history.truncated || history.entries.len() > MAX_ROWS,
    }
}

/// Worker body: reads the file, indexes the Library, checks unmatched paths.
fn load(library: Arc<Library>) -> Snapshot {
    let environment = DiscoveryEnvironment::from_process_environment();
    let history = read_native_content_history(&HostReadOnlyFilesystem, &environment);
    let amiga_roots = amiga_hdf_working_copy::effective_storage()
        .map(|storage| {
            std::iter::once(storage.root)
                .chain(storage.earlier_roots)
                .chain(storage.used_roots)
                .collect()
        })
        .unwrap_or_default();
    let context = Context {
        home: environment
            .home
            .filter(|home| !home.is_empty())
            .map(PathBuf::from),
        temp_dir: lexical(&std::env::temp_dir()),
        amiga_roots,
    };
    let index = PathIndex::new(&library);
    build_snapshot(history, &index, &context, &|path| path.try_exists().ok())
}

#[derive(Default)]
pub(super) struct State {
    snapshot: Option<Arc<Snapshot>>,
    /// The Library the snapshot was matched against.
    library: Weak<Library>,
    loaded_at: Option<Instant>,
    job: Option<(Weak<Library>, Receiver<Snapshot>)>,
    pub(super) show_unmatched: bool,
}

impl State {
    fn poll(&mut self) {
        let Some((library, receiver)) = &self.job else {
            return;
        };
        match receiver.try_recv() {
            Ok(snapshot) => {
                self.library = library.clone();
                self.snapshot = Some(Arc::new(snapshot));
                self.loaded_at = Some(Instant::now());
                self.job = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.loaded_at = Some(Instant::now());
                self.job = None;
            }
        }
    }

    /// Starts a read when none ran yet, the Library changed, or the last one
    /// is stale. Never more than one at a time.
    fn refresh_if_due(&mut self, ctx: &egui::Context, library: &SharedLibrary) {
        if self.job.is_some() {
            return;
        }
        let same_library = self.library.as_ptr() == Arc::as_ptr(library);
        let fresh = self
            .loaded_at
            .is_some_and(|at| at.elapsed() < REFRESH_AFTER);
        if same_library && fresh {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let library = Arc::clone(library);
        let weak = Arc::downgrade(&library);
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("emuwiz-recently-played".into())
            .spawn(move || {
                let _ = sender.send(load(library));
                ctx.request_repaint();
            });
        if spawned.is_ok() {
            self.job = Some((weak, receiver));
        } else {
            self.loaded_at = Some(Instant::now());
        }
    }

    /// The most recently played Library game, if any.
    pub(super) fn first_game(&self) -> Option<i64> {
        self.snapshot.as_ref()?.games().first().copied()
    }
}

impl super::App {
    /// Home's "Continue playing" row: covers of matched games, then an honest
    /// count of the entries that are not Library games.
    pub(super) fn home_recently_played(&mut self, ui: &mut egui::Ui) {
        self.recently_played.poll();
        self.recently_played.refresh_if_due(ui.ctx(), &self.library);
        let Some(snapshot) = self.recently_played.snapshot.clone() else {
            return;
        };
        if snapshot.hidden() {
            return;
        }
        let library = self.library.clone();
        let fit = (((ui.available_width() + TILE_GAP) / (TILE.x + TILE_GAP)).floor() as usize)
            .clamp(1, MAX_COVERS);
        let games: Vec<_> = snapshot
            .games()
            .into_iter()
            .filter_map(|id| library.game(id))
            .take(fit)
            .collect();
        ui.add_space(18.0);
        ui.label(theme::section_title("Continue playing"));
        ui.label(
            RichText::new("Games RetroArch recorded playing, most recent first.")
                .color(palette::SECONDARY_TEXT),
        );
        ui.add_space(6.0);
        if !games.is_empty() {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = TILE_GAP;
                for game in games {
                    ui.push_id(("v2_recently_played", game.archive.id), |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(TILE.x, TILE.y + 48.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(TILE.x);
                                self.picture(ui, game, Kind::Cover, TILE);
                                self.game_link(ui, game);
                            },
                        );
                    });
                }
            });
        } else if snapshot.notice().is_none() {
            ui.label(
                RichText::new("None of the games RetroArch played recently are in your Library.")
                    .color(palette::SECONDARY_TEXT),
            );
        }
        if let Some(notice) = snapshot.notice() {
            ui.label(RichText::new(notice).color(palette::SECONDARY_TEXT));
        }
        if snapshot.truncated {
            ui.label(
                RichText::new(format!(
                    "Only the {MAX_ROWS} most recent history entries are shown."
                ))
                .color(palette::DIM_TEXT),
            );
        }
        self.recently_played_unmatched(ui, &snapshot);
    }

    fn recently_played_unmatched(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        let unmatched: Vec<_> = snapshot.unmatched().collect();
        if unmatched.is_empty() {
            return;
        }
        let open = self.recently_played.show_unmatched;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} other RetroArch {} not in your Library.",
                    unmatched.len(),
                    if unmatched.len() == 1 {
                        "entry is"
                    } else {
                        "entries are"
                    }
                ))
                .color(palette::SECONDARY_TEXT),
            );
            if ui.link(if open { "Hide" } else { "Show" }).clicked() {
                self.recently_played.show_unmatched = !open;
            }
        });
        if !open {
            return;
        }
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
            .corner_radius(10)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("v2_recently_played_unmatched")
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for (index, row) in unmatched.iter().enumerate() {
                            if index > 0 {
                                ui.separator();
                            }
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(&row.name).strong());
                                if let Some(core) = &row.core {
                                    ui.label(RichText::new(core).color(palette::DIM_TEXT));
                                }
                            });
                            let words = ui.label(
                                RichText::new(row.played.words()).color(palette::SECONDARY_TEXT),
                            );
                            if let Some(path) = &row.path {
                                words.on_hover_text(path);
                            }
                        }
                    });
            });
        if let Some(file) = &snapshot.history_file {
            ui.label(
                RichText::new(format!("Read from {}", file.display()))
                    .size(12.0)
                    .color(palette::DIM_TEXT),
            );
        }
    }
}

#[cfg(test)]
#[path = "recently_played/tests.rs"]
mod tests;
