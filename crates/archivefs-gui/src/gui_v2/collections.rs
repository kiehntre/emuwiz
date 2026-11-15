//! Favourites and collections in Browse & Play and Game Details.
//!
//! The list is Browse & Play's own, narrowed to a collection's members, so
//! search, system and readiness filters work unchanged. A scope lives here and
//! not in [`super::library::Filter`]: the Classic Library and the filter worker
//! share that filter, and walking a collection's members stays cheap however
//! large the library is.
//!
//! Every database touch happens on one worker thread, in order. A click updates
//! the screen at once; the worker's answer then replaces it, and a failed write
//! puts back what the database really holds and says so. The generic backend
//! worker is not used because a scan can hold it for hours.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use archivefs_core::{
    CollectionItemKey, CollectionReanchor, Database, GameCollection, GameCollectionKind,
    MAX_COLLECTION_NAME_CHARS,
};
use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};

use super::{
    App,
    browse_play::thousands,
    library::{Filter, Library},
};
use crate::ui::theme;

const STAR: &str = "★";
const STAR_ON: Color32 = Color32::from_rgb(245, 197, 66);
const STAR_OFF: Color32 = Color32::from_rgb(150, 156, 170);
const GONE_ROW_HEIGHT: f32 = 44.0;
/// A gone row that also shows a proposed match and its confirm button.
const GONE_PROPOSAL_ROW_HEIGHT: f32 = 92.0;

/// How a collection is listed. Library-wide browsing stays in title order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum CollectionSort {
    #[default]
    TitleAscending,
    TitleDescending,
    System,
    RecentlyAdded,
}

impl CollectionSort {
    pub(super) fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub const ALL: [Self; 4] = [
        Self::TitleAscending,
        Self::TitleDescending,
        Self::System,
        Self::RecentlyAdded,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::TitleAscending => "Title A–Z",
            Self::TitleDescending => "Title Z–A",
            Self::System => "System",
            Self::RecentlyAdded => "Recently added",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Member {
    /// Saved catalogue ids that resolve to this game; older rows of one game
    /// can leave several, and removing it must remove them all.
    stored: Vec<CollectionItemKey>,
    added_at: String,
}

/// A saved game that no catalogue row answers for now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GoneGame {
    stored: CollectionItemKey,
    title: String,
    platform: String,
    file_name: String,
    /// A listed game at the item's own source, path and size: shown as a
    /// possible match, saved only when the user confirms it.
    proposed: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct View {
    pub id: i64,
    pub favourites: bool,
    pub name: String,
    pub members: HashMap<i64, Member>,
    pub gone: Vec<GoneGame>,
    /// Members per system, largest first.
    pub platforms: Vec<(String, usize)>,
}

/// Saved collections as this library shows them. Read-only. A game is a member
/// only through its own saved row, or the row EmuWiz's own rename record moved
/// it to. Every other link (same name and size, a path match after the row was
/// forgotten) is a guess: it is only proposed, on the item's gone entry.
pub(super) fn build_views(collections: &[GameCollection], library: &Library) -> Vec<View> {
    use archivefs_core::catalogue_supersession::SupersessionEvidence;
    let listed = |id: i64| Some(library.resolve(id)).filter(|id| library.by_id.contains_key(id));
    // (member, proposal) for an item whose own row still exists.
    let own_row = |own: i64| -> (Option<i64>, Option<i64>) {
        if library.by_id.contains_key(&own) {
            return (Some(own), None);
        }
        match library.historical_links.get(&own) {
            Some(link) if !library.by_id.contains_key(&link.current_id) => (None, None),
            Some(link) if link.evidence == SupersessionEvidence::JournalRename => {
                (Some(link.current_id), None)
            }
            Some(link) => (None, Some(link.current_id)),
            None => (None, None),
        }
    };
    collections
        .iter()
        .map(|collection| {
            let mut members: HashMap<i64, Member> = HashMap::new();
            let mut gone = Vec::new();
            for item in &collection.items {
                let key = CollectionItemKey {
                    archive_id: item.stored_archive_id,
                    created_at: item.stored_created_at.clone(),
                };
                let (member, link_proposal) =
                    item.resolved_archive_id.map_or((None, None), own_row);
                match member {
                    Some(id) => {
                        let member = members.entry(id).or_insert_with(|| Member {
                            stored: Vec::new(),
                            added_at: item.added_at.clone(),
                        });
                        member.stored.push(key);
                        if item.added_at > member.added_at {
                            member.added_at = item.added_at.clone();
                        }
                    }
                    None => gone.push(GoneGame {
                        stored: key,
                        title: item.title.clone(),
                        platform: item
                            .platform
                            .as_deref()
                            .map_or(super::library::UNKNOWN_PLATFORM.to_string(), |platform| {
                                super::library::canonical_platform_name(Some(platform))
                            }),
                        file_name: item
                            .absolute_path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        proposed: link_proposal.or(item.proposed_archive_id.and_then(listed)),
                    }),
                }
            }
            let platforms = platform_counts(&members, library);
            View {
                id: collection.id,
                favourites: collection.kind == GameCollectionKind::Favourites,
                name: collection.name.clone(),
                members,
                gone,
                platforms,
            }
        })
        .collect()
}

fn platform_counts(members: &HashMap<i64, Member>, library: &Library) -> Vec<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for id in members.keys() {
        if let Some(game) = library.game(*id) {
            *counts.entry(game.platform.as_str()).or_default() += 1;
        }
    }
    let mut ordered: Vec<_> = counts
        .into_iter()
        .map(|(name, count)| (name.to_string(), count))
        .collect();
    ordered.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ordered
}

/// The members matching `filter`, in `sort` order. Costs the size of the
/// collection, not of the library.
pub(super) fn scoped_indices(
    library: &Library,
    view: &View,
    filter: &Filter,
    sort: CollectionSort,
) -> Vec<usize> {
    let ids: HashSet<i64> = view.members.keys().copied().collect();
    let mut indices = library.filter_members(filter, &ids);
    match sort {
        // Games are held in title order already.
        CollectionSort::TitleAscending => {}
        CollectionSort::TitleDescending => indices.reverse(),
        CollectionSort::System => indices.sort_by(|a, b| {
            library.games[*a]
                .platform
                .cmp(&library.games[*b].platform)
                .then(a.cmp(b))
        }),
        CollectionSort::RecentlyAdded => {
            let added = |index: &usize| {
                view.members
                    .get(&library.games[*index].archive.id)
                    .map(|member| member.added_at.as_str())
            };
            indices.sort_by(|a, b| added(b).cmp(&added(a)).then(a.cmp(b)));
        }
    }
    indices
}

pub(super) enum Change {
    Add {
        collection: i64,
        game: i64,
    },
    Remove {
        collection: i64,
        /// The game on screen, when there is one. Removed by its current row
        /// too, so a move or an add still in the queue cannot bring it back.
        game: Option<i64>,
        /// Rows a queued re-anchor is moving this item to.
        moved_to: Vec<i64>,
        stored: Vec<CollectionItemKey>,
    },
    Create {
        name: String,
        game: Option<i64>,
    },
    Rename {
        collection: i64,
        seen: String,
        name: String,
    },
    Delete {
        collection: i64,
        seen: String,
    },
    /// The user confirmed a proposed match: save the item against that row.
    Confirm(CollectionReanchor),
}

enum Job {
    Reload,
    Change(Change),
}

struct Answer {
    /// `None`: the saved collections could not be read at all.
    saved: Option<Vec<GameCollection>>,
    error: Option<String>,
    created: Option<i64>,
    /// After a load: whether automatic updates for renamed games were paused
    /// because a rename record could not be trusted. `None`: not checked.
    journal_paused: Option<bool>,
}

fn apply(path: &Path, change: Change) -> Result<Option<i64>, String> {
    let mut database = Database::open_existing(path).map_err(|error| error.to_string())?;
    let mut created = None;
    let result = match change {
        Change::Add { collection, game } => database.add_game_to_collection(collection, game),
        Change::Remove {
            collection,
            game,
            moved_to,
            stored,
        } => database
            .remove_items_from_collection(collection, &stored)
            .and_then(|()| {
                game.into_iter().chain(moved_to).try_for_each(|game| {
                    database.remove_current_game_from_collection(collection, game)
                })
            }),
        Change::Create { name, game } => database.create_game_collection(&name).and_then(|id| {
            created = Some(id);
            game.map_or(Ok(()), |game| database.add_game_to_collection(id, game))
        }),
        Change::Rename {
            collection,
            seen,
            name,
        } => database.rename_game_collection(collection, &seen, &name),
        Change::Delete { collection, seen } => database.delete_game_collection(collection, &seen),
        Change::Confirm(step) => database.reanchor_collection_items(std::slice::from_ref(&step)),
    };
    result.map(|()| created).map_err(|error| error.to_string())
}

/// The one save that loading may make: items EmuWiz's own completed rename
/// record proves moved (see `save_journalled_collection_moves`). Guesses by
/// name, path or size are never saved here. It is retried at the next load if
/// it fails, and items still show through the rename record meanwhile.
/// Returns whether automatic updates were paused by an untrusted record
/// (`Some(false)` when there is no record to check); `None` when the library
/// could not be opened, which changes nothing on screen.
fn save_journalled_moves(path: &Path) -> Option<bool> {
    let Some(journals) = path
        .parent()
        .map(|dir| dir.join(archivefs_core::dat::rename_apply::RENAME_TRANSACTIONS_DIRECTORY))
        .filter(|dir| dir.is_dir())
    else {
        return Some(false);
    };
    match Database::open_existing(path)
        .and_then(|mut database| database.save_journalled_collection_moves(&journals))
    {
        Ok(result) => {
            if let Some(reason) = &result.paused {
                log::warn!("collections: automatic updates for renamed games paused: {reason}");
            }
            Some(result.paused.is_some())
        }
        Err(error) => {
            log::warn!("collections: renamed games were not saved to their new entry: {error}");
            None
        }
    }
}

fn answer(path: &Path, job: Job) -> Answer {
    let mut journal_paused = None;
    let (error, created) = match job {
        Job::Reload => {
            journal_paused = save_journalled_moves(path);
            (None, None)
        }
        Job::Change(change) => match apply(path, change) {
            Ok(created) => (None, created),
            Err(error) => (Some(error), None),
        },
    };
    let saved = if path.exists() {
        Database::open_read_only(path)
            .and_then(|database| database.load_game_collections())
            .ok()
    } else {
        Some(Vec::new())
    };
    Answer {
        saved,
        error,
        created,
        journal_paused,
    }
}

fn start_worker(
    path: PathBuf,
    context: egui::Context,
) -> (mpsc::Sender<Job>, mpsc::Receiver<Answer>) {
    let (jobs, requests) = mpsc::channel();
    let (answers, results) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(job) = requests.recv() {
            if answers.send(answer(&path, job)).is_err() {
                break;
            }
            context.request_repaint();
        }
    });
    (jobs, results)
}

const UNAVAILABLE: &str = "Collections are unavailable. Reopen EmuWiz.";
const READ_FAILED: &str = "Saved collections could not be read just now.";
const JOURNAL_PAUSED: &str = "Automatic updates for renamed games are paused: a rename record could not be read or is unfinished. Your favourites and collections are kept as they are, and you can still change them yourself.";

#[derive(Default)]
enum Dialog {
    #[default]
    None,
    Create {
        name: String,
        game: Option<i64>,
    },
    Rename {
        collection: i64,
        /// The name when the dialog opened, which is what the user was shown.
        seen: String,
        name: String,
    },
    Delete {
        collection: i64,
    },
    Gone {
        collection: i64,
    },
}

#[derive(Default)]
pub(super) struct State {
    worker: Option<(mpsc::Sender<Job>, mpsc::Receiver<Answer>)>,
    /// Jobs sent and not answered. Answers are only shown once this is zero,
    /// so a slower answer cannot undo a newer click on screen.
    pending: usize,
    saved: Vec<GameCollection>,
    views: Vec<View>,
    /// Bumped whenever a list on screen may change.
    version: u64,
    /// Confirmed moves sent and not yet read back; a removal must follow them.
    reanchoring: Vec<CollectionReanchor>,
    message: Option<String>,
    /// Automatic updates for renamed games are paused: a rename record could
    /// not be trusted. Shown as a note; nothing else changes.
    journal_paused: bool,
    dialog: Dialog,
    open_after_create: bool,
    pub(super) scope: Option<i64>,
    pub(super) sort: CollectionSort,
}

impl State {
    pub(super) fn with_database(path: Option<PathBuf>, context: egui::Context) -> Self {
        Self {
            worker: path.map(|path| start_worker(path, context)),
            ..Self::default()
        }
    }

    fn send(&mut self, job: Job) {
        if let Some((jobs, _)) = &self.worker {
            if jobs.send(job).is_ok() {
                self.pending += 1;
            } else {
                self.message = Some(UNAVAILABLE.to_string());
            }
        }
    }

    pub(super) fn version(&self) -> u64 {
        self.version
    }

    /// Whether the saved collections have loaded; until then there is nothing
    /// a star could safely write to.
    pub(super) fn ready(&self) -> bool {
        !self.views.is_empty()
    }

    fn favourites(&self) -> Option<&View> {
        self.views.iter().find(|view| view.favourites)
    }

    pub(super) fn is_favourite(&self, game: i64) -> bool {
        self.favourites()
            .is_some_and(|view| view.members.contains_key(&game))
    }

    pub(super) fn scoped(&self) -> Option<&View> {
        let scope = self.scope?;
        self.views.iter().find(|view| view.id == scope)
    }

    /// The library changed: show what we hold against it, then re-read.
    pub(super) fn library_changed(&mut self, library: &Library) {
        self.rebuild(library);
        self.send(Job::Reload);
    }

    fn rebuild(&mut self, library: &Library) {
        self.views = build_views(&self.saved, library);
        if self.scoped().is_none() {
            self.scope = None;
        }
        self.version += 1;
    }

    /// Collect finished work. Called every frame; does nothing when idle.
    pub(super) fn poll(&mut self, library: &Arc<Library>) {
        let mut newest = None;
        while let Some((_, answers)) = &self.worker {
            let answer = match answers.try_recv() {
                Ok(answer) => answer,
                Err(mpsc::TryRecvError::Empty) => break,
                // Nothing further will be answered, so nothing may wait for it.
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.pending > 0 {
                        self.pending = 0;
                        self.message = Some(UNAVAILABLE.to_string());
                    }
                    break;
                }
            };
            self.pending = self.pending.saturating_sub(1);
            if let Some(error) = &answer.error {
                self.message = Some(format!("That change was not saved: {error}"));
            }
            if let Some(paused) = answer.journal_paused {
                self.journal_paused = paused;
            }
            if self.open_after_create && answer.created.is_some() {
                self.scope = answer.created;
                self.open_after_create = false;
            }
            newest = Some(answer);
        }
        let Some(answer) = newest else {
            return;
        };
        if let Some(saved) = answer.saved {
            self.saved = saved;
            if self.message.as_deref() == Some(READ_FAILED) {
                self.message = None;
            }
        } else {
            // A write's own failure matters more and stays on screen.
            self.message.get_or_insert_with(|| READ_FAILED.to_string());
        }
        if self.pending > 0 {
            return;
        }
        self.reanchoring.clear();
        // Loading or reloading only reads: nothing is moved or saved here.
        self.rebuild(library);
    }

    fn change(&mut self, change: Change) {
        self.message = None;
        self.send(Job::Change(change));
    }

    /// Add or remove one game, on screen at once.
    fn toggle(&mut self, collection: i64, game: i64, library: &Library) {
        let Some(view) = self.views.iter_mut().find(|view| view.id == collection) else {
            return;
        };
        let change = match view.members.remove(&game) {
            Some(member) => Change::Remove {
                collection,
                game: Some(game),
                moved_to: self
                    .reanchoring
                    .iter()
                    .filter(|step| {
                        member.stored.iter().any(|key| {
                            key.archive_id == step.stored_archive_id
                                && key.created_at == step.stored_created_at
                        })
                    })
                    .map(|step| step.current_archive_id)
                    .collect(),
                stored: member.stored,
            },
            None => {
                view.members.insert(
                    game,
                    Member {
                        // Known once the saved form is read back; a removal
                        // meanwhile goes by the game's current row.
                        stored: Vec::new(),
                        added_at: archivefs_core::format_unix_timestamp_utc(
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |time| time.as_secs() as i64),
                        ),
                    },
                );
                Change::Add { collection, game }
            }
        };
        view.platforms = platform_counts(&view.members, library);
        self.version += 1;
        self.change(change);
    }
}

fn scope_button(ui: &mut egui::Ui, text: String, selected: bool) -> bool {
    ui.add(egui::Button::new(text).selected(selected)).clicked()
}

impl App {
    /// The star on a card, row or panel: one click, no dialog. Returns its
    /// response so a card can treat a focused star as focus on the card.
    pub(super) fn favourite_star(
        &mut self,
        ui: &mut egui::Ui,
        game: i64,
        title: &str,
    ) -> Option<egui::Response> {
        let view = self.collections.favourites()?;
        let favourites = view.id;
        let on = view.members.contains_key(&game);
        let hint = if on {
            format!("Remove {title} from Favourites")
        } else {
            format!("Add {title} to Favourites")
        };
        let star = RichText::new(STAR)
            .size(18.0)
            .color(if on { STAR_ON } else { STAR_OFF });
        let response = ui.add(egui::Button::new(star).frame(false));
        // Name it by what it does, not by its glyph.
        let name = hint.clone();
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name.clone())
        });
        let response = response.on_hover_text(hint);
        if response.clicked() {
            let library = self.library.clone();
            self.collections.toggle(favourites, game, &library);
        }
        Some(response)
    }

    /// Star, plain words, and the collections menu, for one selected game.
    pub(super) fn game_collection_actions(&mut self, ui: &mut egui::Ui, game: i64, title: &str) {
        // Owner decision 2026-11-15: selecting for artwork never offers a
        // Favourites or Collections change, anywhere.
        if !self.collections.ready() || self.batch_selecting {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            self.favourite_star(ui, game, title);
            ui.label(if self.collections.is_favourite(game) {
                "In Favourites"
            } else {
                "Favourite"
            });
            let library = self.library.clone();
            ui.menu_button("Collections", |ui| {
                let mut toggled = None;
                for view in self
                    .collections
                    .views
                    .iter()
                    .filter(|view| !view.favourites)
                {
                    let mut member = view.members.contains_key(&game);
                    if ui.checkbox(&mut member, &view.name).changed() {
                        toggled = Some(view.id);
                    }
                }
                if let Some(collection) = toggled {
                    self.collections.toggle(collection, game, &library);
                }
                if ui.button("New collection…").clicked() {
                    self.collections.dialog = Dialog::Create {
                        name: String::new(),
                        game: Some(game),
                    };
                    ui.close();
                }
            });
        });
    }

    /// Why a change did not stick. Drawn wherever collections can be changed,
    /// including when nothing else can be drawn because the first read failed.
    pub(super) fn collection_message(&mut self, ui: &mut egui::Ui) {
        let Some(message) = &self.collections.message else {
            return;
        };
        ui.colored_label(ui.visuals().error_fg_color, message);
        if !self.collections.ready() && ui.small_button("Try again").clicked() {
            self.collections.message = None;
            self.collections.send(Job::Reload);
        }
    }

    /// A quiet note while automatic updates for renamed games are paused.
    /// Browsing and every manual control work as usual.
    pub(super) fn collection_journal_note(&self, ui: &mut egui::Ui) {
        if self.collections.journal_paused {
            ui.label(
                RichText::new(JOURNAL_PAUSED)
                    .small()
                    .color(ui.visuals().warn_fg_color),
            );
        }
    }

    /// Which games the page lists, and the tools for that collection.
    pub(super) fn browse_play_scope_bar(&mut self, ui: &mut egui::Ui) {
        self.collection_message(ui);
        self.collection_journal_note(ui);
        if !self.collections.ready() {
            return;
        }
        let mut chosen = None;
        let mut dialog = None;
        let scope = self.collections.scope;
        // Selecting for artwork shows the lists but none of their editing.
        let editing = !self.batch_selecting;
        ui.horizontal_wrapped(|ui| {
            if scope_button(ui, "All games".into(), scope.is_none()) {
                chosen = Some(None);
            }
            if let Some(view) = self.collections.favourites()
                && scope_button(
                    ui,
                    format!("{STAR} Favourites  {}", thousands(view.members.len())),
                    scope == Some(view.id),
                )
            {
                chosen = Some(Some(view.id));
            }
            let current = self
                .collections
                .scoped()
                .filter(|view| !view.favourites)
                .map_or("Collections".to_string(), |view| view.name.clone());
            egui::ComboBox::from_id_salt("v2_browse_play_collection")
                .selected_text(current)
                .width(180.0)
                .height(320.0)
                .show_ui(ui, |ui| {
                    for view in self
                        .collections
                        .views
                        .iter()
                        .filter(|view| !view.favourites)
                    {
                        let text = format!("{}  ·  {}", view.name, thousands(view.members.len()));
                        if ui.selectable_label(scope == Some(view.id), text).clicked() {
                            chosen = Some(Some(view.id));
                        }
                    }
                    if editing && ui.button("New collection…").clicked() {
                        dialog = Some(Dialog::Create {
                            name: String::new(),
                            game: None,
                        });
                    }
                });
            let mut sort = self.collections.sort;
            let scoped = self
                .collections
                .scoped()
                .map(|view| (view.id, view.favourites, view.name.clone(), view.gone.len()));
            if let Some((id, favourites, name, gone)) = scoped {
                egui::ComboBox::from_id_salt("v2_browse_play_collection_sort")
                    .selected_text(sort.label())
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for choice in CollectionSort::ALL {
                            ui.selectable_value(&mut sort, choice, choice.label());
                        }
                    });
                if editing && !favourites {
                    // Until the last change is answered the list may already be out of date.
                    let settled = self.collections.pending == 0;
                    if ui
                        .add_enabled(settled, egui::Button::new("Rename").small())
                        .clicked()
                    {
                        dialog = Some(Dialog::Rename {
                            collection: id,
                            seen: name.clone(),
                            name,
                        });
                    }
                    if ui
                        .add_enabled(settled, egui::Button::new("Delete").small())
                        .clicked()
                    {
                        dialog = Some(Dialog::Delete { collection: id });
                    }
                }
                if editing
                    && gone > 0
                    && ui
                        .small_button(format!("{} no longer in the library", thousands(gone)))
                        .clicked()
                {
                    dialog = Some(Dialog::Gone { collection: id });
                }
            }
            if sort != self.collections.sort {
                self.collections.sort = sort;
                self.preferences_dirty = Some(std::time::Instant::now());
            }
        });
        if let Some(scope) = chosen {
            self.choose_scope(scope);
        }
        if let Some(dialog) = dialog {
            self.collections.dialog = dialog;
        }
    }

    pub(super) fn choose_scope(&mut self, scope: Option<i64>) {
        self.collections.scope = scope;
        // A system with no members here would open on an empty page.
        if let Some(view) = self.collections.scoped()
            && !self.filter.platform.is_empty()
            && !view
                .platforms
                .iter()
                .any(|(name, _)| *name == self.filter.platform)
        {
            self.filter.platform.clear();
            self.change_filter();
        }
    }

    /// Shown in place of the shelf when a collection has nothing to list.
    pub(super) fn collection_empty(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(view) = self.collections.scoped() else {
            return false;
        };
        if !view.members.is_empty() {
            return false;
        }
        let heading = if view.favourites {
            "No favourites yet"
        } else {
            "This collection is empty"
        };
        let hint = if view.favourites {
            "Select the star on any game to keep it here."
        } else {
            "Open a game and use Collections to add it here."
        };
        ui.label(theme::section_title(heading));
        ui.label(hint);
        if ui.button("Browse all games").clicked() {
            self.choose_scope(None);
        }
        true
    }

    pub(super) fn collection_dialogs(&mut self, context: &egui::Context) {
        let mut close = false;
        let mut change = None;
        let mut open_new = false;
        // Drawn every frame on these pages, so nothing is copied when idle.
        // Selecting for artwork keeps any open dialog for afterwards.
        if matches!(self.collections.dialog, Dialog::None) || self.batch_selecting {
            return;
        }
        // The dialog's own target, not whatever is listed now: the page behind
        // it stays usable.
        let target = match self.collections.dialog {
            Dialog::Rename { collection, .. }
            | Dialog::Delete { collection }
            | Dialog::Gone { collection } => Some(collection),
            _ => None,
        };
        let selected = target.and_then(|id| {
            let view = self.collections.views.iter().find(|view| view.id == id)?;
            let count = view.members.len() + view.gone.len();
            Some((view.name.clone(), count, view.gone.clone()))
        });
        if target.is_some() && selected.is_none() {
            self.collections.dialog = Dialog::None;
            return;
        }
        let settled = self.collections.pending == 0;
        let library = self.library.clone();
        match &mut self.collections.dialog {
            Dialog::None => {}
            Dialog::Create { name, game } => {
                egui::Window::new("New collection")
                    .collapsible(false)
                    .resizable(false)
                    .show(context, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(name)
                                .char_limit(MAX_COLLECTION_NAME_CHARS)
                                .hint_text("Collection name"),
                        );
                        ui.horizontal(|ui| {
                            let valid = archivefs_core::validate_collection_name(name).is_ok();
                            if ui.add_enabled(valid, egui::Button::new("Create")).clicked() {
                                open_new = game.is_none();
                                change = Some(Change::Create {
                                    name: name.clone(),
                                    game: *game,
                                });
                                close = true;
                            }
                            close |= ui.button("Cancel").clicked();
                        });
                    });
            }
            Dialog::Rename {
                collection,
                seen,
                name,
            } => {
                egui::Window::new("Rename collection")
                    .collapsible(false)
                    .resizable(false)
                    .show(context, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(name).char_limit(MAX_COLLECTION_NAME_CHARS),
                        );
                        ui.horizontal(|ui| {
                            let valid =
                                settled && archivefs_core::validate_collection_name(name).is_ok();
                            if ui.add_enabled(valid, egui::Button::new("Rename")).clicked() {
                                change = Some(Change::Rename {
                                    collection: *collection,
                                    seen: seen.clone(),
                                    name: name.clone(),
                                });
                                close = true;
                            }
                            close |= ui.button("Cancel").clicked();
                        });
                    });
            }
            Dialog::Delete { collection } => {
                let (name, count, _) = selected.unwrap_or_default();
                egui::Window::new("Delete collection")
                    .collapsible(false)
                    .resizable(false)
                    .show(context, |ui| {
                        ui.label(format!(
                            "Delete {name}? Its list of {} {} cannot be brought back. The games stay in your library and no files change.",
                            thousands(count),
                            if count == 1 { "game" } else { "games" }
                        ));
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(settled, egui::Button::new("Delete collection"))
                                .clicked()
                            {
                                change = Some(Change::Delete {
                                    collection: *collection,
                                    seen: name.clone(),
                                });
                                close = true;
                            }
                            close |= ui.button("Cancel").clicked();
                        });
                    });
            }
            Dialog::Gone { collection } => {
                let collection = *collection;
                let (name, _, rows) = selected.unwrap_or_default();
                let mut open = true;
                egui::Window::new("No longer in the library")
                    .open(&mut open)
                    .default_width(460.0)
                    .show(context, |ui| {
                        ui.label(format!("Saved in {name}, but with no catalogue entry now. A game comes back on its own if a scan finds the same file in the same place. A game that moved stays here until you remove it. Nothing is removed unless you remove it."));
                        let height = if rows.iter().any(|game| game.proposed.is_some()) {
                            GONE_PROPOSAL_ROW_HEIGHT
                        } else {
                            GONE_ROW_HEIGHT
                        };
                        egui::ScrollArea::vertical().max_height(320.0).show_rows(
                            ui,
                            height,
                            rows.len(),
                            |ui, range| {
                                for game in &rows[range] {
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.label(RichText::new(&game.title).strong());
                                            ui.small(format!(
                                                "{} · {}",
                                                game.platform, game.file_name
                                            ));
                                            if let Some(proposed) = game.proposed {
                                                let title = library
                                                    .game(proposed)
                                                    .map_or_else(String::new, |found| found.title.clone());
                                                ui.small(format!("Possible match: {title}. It looks the same by name and size, but that does not prove it is the same game. Nothing is saved unless you confirm, and confirming applies to this saved game in every collection that holds it."));
                                                if ui
                                                    .add_enabled(settled, egui::Button::new("Keep as this game").small())
                                                    .clicked()
                                                {
                                                    change = Some(Change::Confirm(CollectionReanchor {
                                                        stored_archive_id: game.stored.archive_id,
                                                        stored_created_at: game.stored.created_at.clone(),
                                                        current_archive_id: proposed,
                                                    }));
                                                }
                                            }
                                        });
                                        if ui.small_button("Remove").clicked() {
                                            change = Some(Change::Remove {
                                                collection,
                                                game: None,
                                                moved_to: Vec::new(),
                                                stored: vec![game.stored.clone()],
                                            });
                                        }
                                    });
                                }
                            },
                        );
                    });
                close = !open || rows.is_empty();
            }
        }
        if close {
            self.collections.dialog = Dialog::None;
        }
        if let Some(change) = change {
            if let Change::Remove {
                collection, stored, ..
            } = &change
                && let Some(view) = self
                    .collections
                    .views
                    .iter_mut()
                    .find(|view| view.id == *collection)
            {
                view.gone.retain(|game| !stored.contains(&game.stored));
            }
            if let Change::Confirm(step) = &change {
                // The row is already shown; a removal before the read-back
                // must also reach the item's new row.
                self.collections.reanchoring.push(step.clone());
                for view in &mut self.collections.views {
                    view.gone.retain(|game| {
                        game.stored.archive_id != step.stored_archive_id
                            || game.stored.created_at != step.stored_created_at
                    });
                }
            }
            self.collections.open_after_create = open_new;
            self.collections.change(change);
        }
    }
}

#[cfg(test)]
#[path = "collections_tests.rs"]
mod tests;
