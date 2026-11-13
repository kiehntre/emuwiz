//! Multi-select for the Blue Library (Browse & Play) list.
//!
//! Plain click keeps today's single-game behaviour: the caller opens the game
//! exactly as before and the bulk set is emptied. Ctrl+click toggles one game,
//! Shift+click selects the range from the anchor in the order the list is
//! shown, Ctrl+Shift+click adds that range. Ctrl+A selects every shown game
//! and Escape clears the set, both only while no text field has the keyboard.
//!
//! The set is keyed by catalogue id, never by row, so sorting and filtering
//! cannot move it onto other games. A reload re-resolves every id (an older
//! row's id becomes its current row's id) and drops games that are gone, and
//! says so. Games a filter hides stay selected and are counted as hidden; the
//! bar offers "Keep only shown" to drop them.
//!
//! Bulk actions are read-only and reuse existing operations: copy titles or
//! file paths, open containing folders through the existing Open Folder
//! command, and a tally of the saved launch-check answers. Nothing here
//! deletes, moves, renames, applies, launches or starts a check.
//! Adding games to a collection is deferred to the favourites/collections
//! work (PR #278); it is deliberately not offered.

// Integration pending: Library sort (Batch 1 D) and Ready filter (Batch 2
// Job 6) are still changing the list code, so the row click and bar are wired
// in after they merge. Remove this once wired.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use super::{
    App,
    library::{CheckedReadiness, Game, Library, ReadinessCache},
    routes::Route,
};
use crate::ui::theme;
use eframe::egui::{self, RichText};

/// Most games one selection may hold. Keeps clipboard text and per-frame work
/// bounded on a very large library; bigger asks are refused, never truncated.
pub(super) const MAX_SELECTED: usize = 5_000;
/// Most distinct folders "Show in file manager" opens at once.
pub(super) const MAX_FOLDERS: usize = 5;

/// What a click on a row should do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ClickOutcome {
    /// A plain click: open this game exactly as a click always has.
    Open(i64),
    /// The bulk set changed; do not navigate.
    Selection,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ClickModifiers {
    /// Ctrl (Cmd on macOS): toggle one game.
    pub toggle: bool,
    /// Shift: range from the anchor.
    pub range: bool,
}

impl ClickModifiers {
    pub fn from_egui(modifiers: egui::Modifiers) -> Self {
        Self {
            toggle: modifiers.command,
            range: modifiers.shift,
        }
    }
}

/// How many games are selected and how many of them the current filter hides.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SelectionCount {
    pub selected: usize,
    pub hidden: usize,
}

impl SelectionCount {
    pub fn label(self) -> String {
        let games = if self.selected == 1 { "game" } else { "games" };
        if self.hidden == 0 {
            format!("{} {games} selected", self.selected)
        } else {
            format!(
                "{} {games} selected ({} hidden by filter)",
                self.selected, self.hidden
            )
        }
    }
}

/// The ids drawn by the list, held with the list they came from so a new
/// list (filter, sort, reload) is always noticed: the `Arc` is kept, so its
/// address cannot be reused by another list while cached.
#[derive(Default)]
struct VisibleCache {
    list: Option<Arc<Vec<usize>>>,
    library: Weak<Library>,
    ids: HashSet<i64>,
}

#[derive(Default)]
pub(super) struct LibrarySelection {
    ids: BTreeSet<i64>,
    /// Where a Shift+click range starts: the last game clicked.
    anchor: Option<i64>,
    /// The library the ids were last reconciled against.
    reconciled: Weak<Library>,
    visible: VisibleCache,
    /// One plain sentence about the last refusal or prune.
    pub(super) notice: Option<String>,
}

impl LibrarySelection {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn contains(&self, id: i64) -> bool {
        self.ids.contains(&id)
    }
    pub fn anchor(&self) -> Option<i64> {
        self.anchor
    }
    pub fn ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.ids.iter().copied()
    }
    pub fn clear(&mut self) {
        self.ids.clear();
        self.notice = None;
    }

    /// Apply one row click. `current` is the game the route already shows
    /// (it joins the set on the first Ctrl/Shift click, as in a file manager);
    /// `order` is the shown list's ids, top to bottom.
    pub fn click(
        &mut self,
        id: i64,
        modifiers: ClickModifiers,
        current: Option<i64>,
        order: &[i64],
    ) -> ClickOutcome {
        self.notice = None;
        if !modifiers.toggle && !modifiers.range {
            self.ids.clear();
            self.anchor = Some(id);
            return ClickOutcome::Open(id);
        }
        let anchor = self.anchor.or(current);
        if modifiers.range
            && let Some(range) = anchor.and_then(|anchor| range_between(order, anchor, id))
        {
            let mut next = if modifiers.toggle {
                self.ids.clone()
            } else {
                BTreeSet::new()
            };
            next.extend(range);
            if next.len() > MAX_SELECTED {
                self.notice = Some(too_many());
                return ClickOutcome::Selection;
            }
            self.ids = next;
            // The anchor stays, so the next Shift+click re-draws from it.
            self.anchor = anchor;
            return ClickOutcome::Selection;
        }
        // Toggle, or a range whose start is not in the shown list.
        if self.ids.is_empty()
            && let Some(current) = current.filter(|current| *current != id)
        {
            self.ids.insert(current);
        }
        if !self.ids.remove(&id) {
            if self.ids.len() >= MAX_SELECTED {
                self.notice = Some(too_many());
                return ClickOutcome::Selection;
            }
            self.ids.insert(id);
        }
        self.anchor = Some(id);
        ClickOutcome::Selection
    }

    /// Ctrl+A: every shown game. Refused, not truncated, past the limit.
    pub fn select_all(&mut self, order: &[i64]) {
        if order.len() > MAX_SELECTED {
            self.notice = Some(format!(
                "There are {} games shown; Select all works on up to {}. Narrow the list with search or a system first.",
                order.len(),
                MAX_SELECTED
            ));
            return;
        }
        self.notice = None;
        self.ids = order.iter().copied().collect();
        self.anchor = order.first().copied();
    }

    /// Drop the games the current list does not show.
    pub fn keep_only(&mut self, shown: &HashSet<i64>) {
        self.ids.retain(|id| shown.contains(id));
        if self.anchor.is_some_and(|anchor| !shown.contains(&anchor)) {
            self.anchor = None;
        }
        self.notice = None;
    }

    /// Re-key the set against a reloaded library: `resolve` maps an id to the
    /// current game id, or `None` when the game is gone. Returns how many
    /// selected games were dropped.
    pub fn reconcile(&mut self, resolve: impl Fn(i64) -> Option<i64>) -> usize {
        let before = self.ids.len();
        self.ids = self.ids.iter().filter_map(|id| resolve(*id)).collect();
        self.anchor = self.anchor.and_then(&resolve);
        let dropped = before - self.ids.len();
        if dropped > 0 {
            self.notice = Some(if dropped == 1 {
                "1 selected game is no longer in the library, so it was removed from the selection."
                    .into()
            } else {
                format!(
                    "{dropped} selected games are no longer in the library, so they were removed from the selection."
                )
            });
        }
        dropped
    }

    pub fn count(&self, shown: &HashSet<i64>) -> SelectionCount {
        SelectionCount {
            selected: self.ids.len(),
            hidden: self.ids.iter().filter(|id| !shown.contains(id)).count(),
        }
    }

    /// Selected ids in the order shown, then the hidden ones by id.
    pub fn ordered(&self, order: &[i64]) -> Vec<i64> {
        let mut out: Vec<i64> = order
            .iter()
            .copied()
            .filter(|id| self.ids.contains(id))
            .collect();
        let shown: HashSet<i64> = out.iter().copied().collect();
        out.extend(self.ids.iter().copied().filter(|id| !shown.contains(id)));
        out
    }

    /// Reconcile against `library` once per loaded library.
    fn reconcile_with(&mut self, library: &Arc<Library>) {
        let current = Arc::downgrade(library);
        if Weak::ptr_eq(&self.reconciled, &current) {
            return;
        }
        self.reconciled = current;
        self.reconcile(|id| {
            let id = library.resolve(id);
            library.by_id.contains_key(&id).then_some(id)
        });
    }

    /// The shown ids, rebuilt only when the list or library changes.
    fn shown(&mut self, library: &Arc<Library>, list: &Arc<Vec<usize>>) -> &HashSet<i64> {
        let cache = &mut self.visible;
        let same = cache
            .list
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, list))
            && Weak::ptr_eq(&cache.library, &Arc::downgrade(library));
        if !same {
            cache.ids = ids_in_order(library, list).into_iter().collect();
            cache.list = Some(list.clone());
            cache.library = Arc::downgrade(library);
        }
        &cache.ids
    }
}

fn too_many() -> String {
    format!("A selection holds at most {MAX_SELECTED} games.")
}

/// Ids from `from` to `to` inclusive, in list order; `None` when either is
/// not in the list.
fn range_between(order: &[i64], from: i64, to: i64) -> Option<Vec<i64>> {
    let a = order.iter().position(|id| *id == from)?;
    let b = order.iter().position(|id| *id == to)?;
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    Some(order[start..=end].to_vec())
}

/// The shown list's ids, top to bottom.
pub(super) fn ids_in_order(library: &Library, list: &[usize]) -> Vec<i64> {
    list.iter()
        .filter_map(|index| library.games.get(*index))
        .map(|game| game.archive.id)
        .collect()
}

/// One title per line.
pub(super) fn titles_text(games: &[&Game]) -> String {
    games
        .iter()
        .map(|game| game.title.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// One absolute file path per line.
pub(super) fn paths_text(games: &[&Game]) -> String {
    games
        .iter()
        .map(|game| game.archive.absolute_path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// One game path per distinct containing folder, in selection order, for the
/// existing Open Folder command (which opens the path's parent). Refused when
/// that would open more than [`MAX_FOLDERS`] windows.
pub(super) fn folders_to_open(games: &[&Game]) -> Result<Vec<PathBuf>, String> {
    let mut seen: HashSet<&Path> = HashSet::new();
    let mut out = Vec::new();
    for game in games {
        let path = game.archive.absolute_path.as_path();
        let Some(folder) = path.parent() else {
            continue;
        };
        if seen.insert(folder) {
            out.push(path.to_path_buf());
        }
    }
    if out.is_empty() {
        return Err("None of the selected games has a containing folder.".into());
    }
    if out.len() > MAX_FOLDERS {
        return Err(format!(
            "The selected games are in {} folders; Show in file manager opens at most {MAX_FOLDERS}. Select fewer games.",
            out.len()
        ));
    }
    Ok(out)
}

/// The saved launch-check answers for the selection. Read-only: nothing is
/// checked here, and a game without a current answer is Not checked.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ReadinessTally {
    pub ready: usize,
    pub not_ready: usize,
    pub not_checked: usize,
}

impl ReadinessTally {
    pub fn of(games: &[&Game], cache: &ReadinessCache) -> Self {
        let mut tally = Self::default();
        for game in games {
            match cache.get(game) {
                Some(CheckedReadiness::Ready) => tally.ready += 1,
                Some(CheckedReadiness::NotReady(_)) => tally.not_ready += 1,
                None => tally.not_checked += 1,
            }
        }
        tally
    }
    pub fn line(self) -> String {
        format!(
            "Launch check: {} ready · {} not ready · {} not checked",
            self.ready, self.not_ready, self.not_checked
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BarAction {
    Clear,
    KeepOnlyShown,
    CopyTitles,
    CopyPaths,
    ShowInFileManager,
}

pub(super) struct BarView<'a> {
    pub count: SelectionCount,
    pub readiness: ReadinessTally,
    pub notice: Option<&'a str>,
}

/// The "N selected" bar. Drawn only while something is selected.
pub(super) fn show_bar(ui: &mut egui::Ui, view: &BarView<'_>) -> Option<BarAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(view.count.label()).strong());
        if ui.small_button("Clear").clicked() {
            action = Some(BarAction::Clear);
        }
        if view.count.hidden > 0
            && ui
                .small_button("Keep only shown")
                .on_hover_text("Remove the games the current search or filter hides.")
                .clicked()
        {
            action = Some(BarAction::KeepOnlyShown);
        }
        ui.separator();
        for (label, hint, choice) in [
            (
                "Copy titles",
                "Copy the selected games' titles, one per line.",
                BarAction::CopyTitles,
            ),
            (
                "Copy file paths",
                "Copy the selected games' file paths, one per line.",
                BarAction::CopyPaths,
            ),
            (
                "Show in file manager",
                "Open the folders holding the selected games.",
                BarAction::ShowInFileManager,
            ),
        ] {
            if ui.button(label).on_hover_text(hint).clicked() {
                action = Some(choice);
            }
        }
    });
    let mut detail = view.readiness.line();
    if view.count.hidden > 0 {
        detail.push_str(" — actions include the hidden games.");
    }
    ui.label(
        RichText::new(detail)
            .size(theme::METADATA_SIZE)
            .color(theme::SECONDARY_TEXT),
    );
    if let Some(notice) = view.notice {
        ui.label(
            RichText::new(notice)
                .size(theme::METADATA_SIZE)
                .color(theme::SECONDARY_TEXT),
        );
    }
    action
}

/// Ctrl+A or Escape for the Library list, never while a text field has the
/// keyboard. Escape is taken only while something is selected, so it still
/// goes Back otherwise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Shortcut {
    SelectAll,
    Clear,
}

pub(super) fn take_shortcut(context: &egui::Context, has_selection: bool) -> Option<Shortcut> {
    if context.wants_keyboard_input() {
        return None;
    }
    context.input_mut(|input| {
        if input.consume_key(egui::Modifiers::COMMAND, egui::Key::A) {
            Some(Shortcut::SelectAll)
        } else if has_selection && input.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
            Some(Shortcut::Clear)
        } else {
            None
        }
    })
}

impl App {
    /// A click on a Library row. Returns `true` when the click changed the
    /// bulk set; otherwise the caller opens the game exactly as before.
    pub(super) fn library_selection_click(
        &mut self,
        id: i64,
        modifiers: egui::Modifiers,
        list: &[usize],
    ) -> bool {
        let modifiers = ClickModifiers::from_egui(modifiers);
        if !modifiers.toggle && !modifiers.range {
            // Nothing to compute: keep the plain click path cheap.
            self.library_selection.click(id, modifiers, None, &[]);
            return false;
        }
        let order = ids_in_order(&self.library, list);
        let current = self.router.current.game();
        self.library_selection.click(id, modifiers, current, &order) == ClickOutcome::Selection
    }

    /// Ctrl+A / Escape for the Library list.
    pub(super) fn library_selection_shortcuts(
        &mut self,
        context: &egui::Context,
        list: &Arc<Vec<usize>>,
    ) {
        match take_shortcut(context, !self.library_selection.is_empty()) {
            Some(Shortcut::SelectAll) => {
                let order = ids_in_order(&self.library, list);
                self.library_selection.select_all(&order);
            }
            Some(Shortcut::Clear) => self.library_selection.clear(),
            None => {}
        }
    }

    /// The selection bar above the list, and its read-only bulk actions.
    pub(super) fn library_selection_bar(&mut self, ui: &mut egui::Ui, list: &Arc<Vec<usize>>) {
        let library = self.library.clone();
        self.library_selection.reconcile_with(&library);
        if self.library_selection.is_empty() {
            if let Some(notice) = &self.library_selection.notice {
                ui.label(
                    RichText::new(notice)
                        .size(theme::METADATA_SIZE)
                        .color(theme::SECONDARY_TEXT),
                );
            }
            return;
        }
        let count = {
            let selection = &mut self.library_selection;
            let shown = selection.shown(&library, list).clone();
            selection.count(&shown)
        };
        let games: Vec<&Game> = self
            .library_selection
            .ids()
            .filter_map(|id| library.game(id))
            .collect();
        let readiness = ReadinessTally::of(&games, &self.browse_play.readiness);
        let view = BarView {
            count,
            readiness,
            notice: self.library_selection.notice.as_deref(),
        };
        let Some(action) = show_bar(ui, &view) else {
            return;
        };
        let ordered = || {
            let order = ids_in_order(&library, list);
            self.library_selection
                .ordered(&order)
                .into_iter()
                .filter_map(|id| library.game(id))
                .collect::<Vec<&Game>>()
        };
        match action {
            BarAction::Clear => self.library_selection.clear(),
            BarAction::KeepOnlyShown => {
                let shown = self.library_selection.shown(&library, list).clone();
                self.library_selection.keep_only(&shown);
            }
            BarAction::CopyTitles => {
                let games = ordered();
                ui.ctx().copy_text(titles_text(&games));
                self.library_selection.notice = Some(copied(games.len(), "title"));
            }
            BarAction::CopyPaths => {
                let games = ordered();
                ui.ctx().copy_text(paths_text(&games));
                self.library_selection.notice = Some(copied(games.len(), "file path"));
            }
            BarAction::ShowInFileManager => {
                let games = ordered();
                match folders_to_open(&games) {
                    Ok(paths) => {
                        self.library_selection.notice = None;
                        for path in paths {
                            let job = self.activity.queue(
                                "Opening the game folder",
                                Route::BrowsePlay,
                                false,
                            );
                            self.send(job, super::backend::Command::OpenFolder(path));
                        }
                    }
                    Err(refusal) => self.library_selection.notice = Some(refusal),
                }
            }
        }
    }
}

fn copied(count: usize, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("Copied {count} {noun}{plural}.")
}

#[cfg(test)]
#[path = "library_selection/tests.rs"]
mod tests;
