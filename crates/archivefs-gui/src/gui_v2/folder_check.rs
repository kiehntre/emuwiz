//! "Organise my games" in Blue: identify, preview, approve, move, verify.
//!
//! Checking compares each game's bytes with every installed catalogue, with
//! validated headers and with calibrated content checks, so a game is judged
//! on its content and never on its folder, name or extension. Checking only
//! reads. Confirmed games (a catalogue hash, or a calibrated validator that
//! establishes the system without identifying the game) can then be moved to
//! their system's folder after one approval of a readable preview; games with
//! no identifiable system can go to `Unnamed/Unknown/` when the person asks.
//! Probable and Ambiguous results never move: they are shown with their
//! evidence and the person can Choose system, Recheck or Leave here.

use super::{App, library::Game};
use archivefs_core::folder_platform_check::{
    self as fpc, Candidate, Catalogue, Finding, RunOutput, Tier, UnknownResult,
};
use archivefs_core::platform::display_name_for;
use eframe::egui;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[cfg(test)]
mod tests;

pub(super) const READ_ONLY_NOTE: &str =
    "Checking only reads. Nothing moves until you review the preview and approve it.";
pub(super) const KNOWN_LIMITS: &str = "Games EmuWiz moves keep their records, favourites and history, and Undo puts them back. A system you choose yourself is remembered by the game's content, so a renamed or moved file, a rescan or a rebuilt library keeps it. The Probable group is a heuristic based on file structure, not a catalogue match, and never moves on its own. Only single cartridge files and single-file ZIPs are read for identification; disc and folder games keep the system the scan gave them unless you choose one. Disc sets (.cue, .gdi, .m3u) move with every track they name, and game folders move whole, by rename on the same drive only; a set that names files in a sub-folder, a folder holding other games, links or another drive is left where it is with the reason.";

/// Why changing the system is not the same as making a game playable.
pub(super) fn playable_note(detected: &str, has_unh: bool) -> String {
    let mut text = String::from(
        "Changing the system only records the platform for this game. It does not necessarily make the ROM playable: Blue's identity and launch checks for the new system still apply.",
    );
    // Explanatory wording only: shown when the chosen system is NES and the
    // game's file or ZIP member really has a .unh extension.
    if detected == "NES" && has_unh {
        text.push_str(" NES \".unh\" unheadered images are staged for the emulator as \".nes\" without changing their bytes; the emulator supplies the mapper from its own game database, so a game it does not know (some homebrew) may still not start.");
    }
    text
}

/// Everything the Choose system window adds when a selection is made. Pure so
/// it can be tested; the window only renders these lines.
pub(super) fn system_change_preview(
    current: &str,
    current_source_note: &str,
    current_source_code: Option<&str>,
    selected: &str,
    finding: Option<&Finding>,
) -> Vec<String> {
    let mut lines = vec![
        format!(
            "Current system: {} ({}; source: {}).",
            display_name_for(current),
            current_source_note.trim_end_matches('.'),
            current_source_code.unwrap_or("not recorded"),
        ),
        format!("You are choosing: {}.", display_name_for(selected)),
    ];
    match finding {
        Some(f) => {
            lines.push(format!(
                "Folder check suggestion: {} ({}).",
                display_name_for(&f.detected),
                f.tier.label()
            ));
            lines.extend(f.lines.iter().map(|l| format!("Evidence: {l}")));
            if !core_same(&f.detected, selected) {
                lines.push("Your choice differs from the folder check suggestion. That is allowed; nothing is checked against it.".into());
            }
        }
        None => lines.push("No folder check result for this game.".into()),
    }
    lines.push(playable_note(selected, finding.is_some_and(|f| f.has_unh)));
    lines.push("Your choice is remembered by the game's content, so a rename, a move, a rescan or a rebuilt library keeps it. Organise my games then offers to move it to that system's folder.".into());
    lines.push("Your choice outranks the folder name for this one game and is saved only when you confirm. No game file is changed.".into());
    lines
}

fn core_same(a: &str, b: &str) -> bool {
    archivefs_core::dat::catalogue_recognition::same_platform(a, b)
}

/// Stable fingerprint of what a card is about, so a dismissal applies to
/// exactly that evidence and a changed result shows the card again.
pub(super) fn fingerprint(findings: &[&Finding]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut keys: Vec<(i64, Tier, &str)> = findings
        .iter()
        .map(|f| (f.archive_id, f.tier, f.detected.as_str()))
        .collect();
    keys.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    keys.hash(&mut hasher);
    hasher.finish()
}

/// One folder's card content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FolderCard {
    pub folder: PathBuf,
    pub folder_label: String,
    pub detected: String,
    pub count: usize,
    pub all_confirmed: bool,
    pub fingerprint: u64,
}

impl FolderCard {
    pub(super) fn headline(&self) -> String {
        let system = display_name_for(&self.detected);
        let games = if self.count == 1 { "game" } else { "games" };
        let how = if self.all_confirmed {
            format!("match {system} catalogue entries")
        } else {
            format!("match or look like {system} games")
        };
        format!(
            "Your folder is named {}, but {} {games} inside {how}. Review them?",
            self.folder_label, self.count
        )
    }
}

/// Folder name as a person reads it.
pub(super) fn folder_label(folder: &std::path::Path) -> String {
    folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.display().to_string())
}

/// Cards for folders with Confirmed or Probable findings (Ambiguous alone
/// never raises a card). One card per folder and detected system, largest
/// first; dismissed ones are left out.
pub(super) fn cards(findings: &[Finding], dismissed: &HashSet<(PathBuf, u64)>) -> Vec<FolderCard> {
    let mut groups: BTreeMap<(PathBuf, String), Vec<&Finding>> = BTreeMap::new();
    for f in findings.iter().filter(|f| f.tier != Tier::Ambiguous) {
        groups
            .entry((f.folder.clone(), f.detected.clone()))
            .or_default()
            .push(f);
    }
    let mut out: Vec<FolderCard> = groups
        .into_iter()
        .map(|((folder, detected), fs)| FolderCard {
            folder_label: folder_label(&folder),
            all_confirmed: fs.iter().all(|f| f.tier == Tier::Confirmed),
            count: fs.len(),
            fingerprint: fingerprint(&fs),
            folder,
            detected,
        })
        .filter(|c| !dismissed.contains(&(c.folder.clone(), c.fingerprint)))
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then(a.folder.cmp(&b.folder)));
    out
}

/// The "Checked N of M" line, with a stopped-early notice and each kind of
/// game that was not examined counted separately.
pub(super) fn progress_text(
    done: usize,
    total: usize,
    stopped_early: bool,
    missing: usize,
    unreadable: usize,
    too_large: usize,
    skipped: usize,
) -> String {
    let mut text = if stopped_early {
        format!("Stopped early - {done} of {total} checked.")
    } else {
        format!("Checked {done} of {total} games.")
    };
    if missing > 0 {
        text.push_str(&format!(
            " {missing} no longer at their recorded location (not read)."
        ));
    }
    if unreadable > 0 {
        text.push_str(&format!(" {unreadable} could not be read."));
    }
    if too_large > 0 {
        text.push_str(&format!(" {too_large} too large to check."));
    }
    if skipped > 0 {
        text.push_str(&format!(
            " {skipped} skipped (a ZIP with several files, an unsafe or inconsistent ZIP, or a file that changed)."
        ));
    }
    text
}

/// Review-list grouping: detected system, then tier.
pub(super) fn grouped(findings: &[Finding]) -> BTreeMap<String, BTreeMap<Tier, Vec<&Finding>>> {
    let mut out: BTreeMap<String, BTreeMap<Tier, Vec<&Finding>>> = BTreeMap::new();
    for f in findings {
        out.entry(f.detected.clone())
            .or_default()
            .entry(f.tier)
            .or_default()
            .push(f);
    }
    out
}

/// Cartridge games (judged against every system) and games with no system
/// at all (so they can be identified, or offered as Unknown). A system the
/// person chose is never second-guessed here.
pub(super) fn candidates_of(games: &[Game]) -> Vec<Candidate> {
    let cartridge = fpc::cartridge_platforms();
    let mut out: Vec<Candidate> = games
        .iter()
        .filter(|g| g.archive.platform_source.as_deref() != Some("manual"))
        .filter_map(|g| {
            let assigned = archivefs_core::canonical_platform_for_alias(&g.platform);
            let assigned = match assigned {
                Some(platform) if cartridge.contains(&platform) => platform.to_string(),
                Some(_) => return None,
                None => String::new(),
            };
            Some(Candidate {
                archive_id: g.archive.id,
                title: g.title.clone(),
                path: g.archive.absolute_path.clone(),
                assigned,
                assigned_source: g.archive.platform_source.clone(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

enum Message {
    Done(Result<(Arc<Vec<Catalogue>>, bool, RunOutput), String>),
}

#[derive(Default)]
pub(super) struct State {
    pub candidates: Vec<Candidate>,
    pub next: usize,
    pub findings: Vec<Finding>,
    pub checked_per_folder: BTreeMap<PathBuf, usize>,
    pub checked: usize,
    pub missing: usize,
    pub unreadable: usize,
    pub too_large: usize,
    pub skipped: usize,
    pub stopped_early: bool,
    pub error: Option<String>,
    pub ran: bool,
    pub dismissed: HashSet<(PathBuf, u64)>,
    /// Checked games with no recorded system and no platform evidence.
    pub unknown: Vec<UnknownResult>,
    /// Games the person chose to leave where they are (this session).
    pub left_here: HashSet<i64>,
    pub organise: super::organise::State,
    pub preferences: super::organise::Preferences,
    /// Game folders the person picked and gave a system (this session).
    pub picked_folders: Vec<fpc::organise::FolderCandidate>,
    /// Disc sets or files the person picked and gave a system (this session).
    pub picked_files: Vec<fpc::organise::ChosenGame>,
    /// A folder being picked: its path and the system chosen so far.
    pub pick: Option<(PathBuf, String)>,
    /// Typed path, for when no file chooser is available.
    pub typed_path: String,
    /// Filter for the system list when picking.
    pub system_filter: String,
    /// Run automatic organising when the current check finishes.
    auto_after_run: bool,
    /// The running pass is a recheck of a few games, not the next batch.
    rechecking: bool,
    pub preferences_loaded: bool,
    catalogues: Option<Arc<Vec<Catalogue>>>,
    progress: Arc<AtomicUsize>,
    cancel: Option<Arc<AtomicBool>>,
    receiver: Option<Receiver<Message>>,
}

impl State {
    pub(super) fn busy(&self) -> bool {
        self.receiver.is_some()
    }

    pub(super) fn total(&self) -> usize {
        self.candidates.len()
    }

    pub(super) fn has_more(&self) -> bool {
        self.ran && self.next < self.candidates.len()
    }

    pub(super) fn finding_for(&self, archive_id: i64) -> Option<&Finding> {
        self.findings.iter().find(|f| f.archive_id == archive_id)
    }

    pub(super) fn dismiss(&mut self, card: &FolderCard) {
        self.dismissed
            .insert((card.folder.clone(), card.fingerprint));
    }

    pub(super) fn cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Checks every remaining game (Cancel stops between games). Read only.
    pub(super) fn start(&mut self, games: &[Game]) {
        if self.busy() {
            return;
        }
        if !self.ran {
            *self = Self {
                dismissed: std::mem::take(&mut self.dismissed),
                left_here: std::mem::take(&mut self.left_here),
                organise: std::mem::take(&mut self.organise),
                preferences: std::mem::take(&mut self.preferences),
                picked_folders: std::mem::take(&mut self.picked_folders),
                picked_files: std::mem::take(&mut self.picked_files),
                candidates: candidates_of(games),
                ran: true,
                ..Default::default()
            };
        }
        self.auto_after_run = self.preferences.auto_organise_confirmed;
        self.rechecking = false;
        let candidates = self.candidates.clone();
        let start = self.next;
        self.spawn_check(candidates, start);
    }

    /// Rechecks the given games with the current catalogues and detectors,
    /// replacing their earlier results. Read only.
    pub(super) fn recheck(&mut self, ids: &HashSet<i64>) {
        if self.busy() {
            return;
        }
        let subset: Vec<Candidate> = self
            .candidates
            .iter()
            .filter(|c| ids.contains(&c.archive_id))
            .cloned()
            .collect();
        if subset.is_empty() {
            return;
        }
        self.rechecking = true;
        // A recheck after new catalogues must not reuse the cached list.
        self.catalogues = None;
        self.spawn_check(subset, 0);
    }

    fn spawn_check(&mut self, candidates: Vec<Candidate>, start: usize) {
        self.stopped_early = false;
        let known = self.catalogues.clone();
        let progress = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        self.progress = progress.clone();
        self.cancel = Some(cancel.clone());
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = (|| {
                let config = archivefs_core::Config::load_default().map_err(|e| e.to_string())?;
                let trusted = archivefs_core::safe_read::TrustedRoots::from_paths(
                    config.source_folders.clone(),
                );
                let (catalogues, complete) = match known {
                    Some(c) => (c, true),
                    None => {
                        let configured = archivefs_core::dat::sources::default_dat_sources_config_path()
                            .map(|p| {
                                archivefs_core::dat::configured_catalogues::load_configured_catalogues(&p)
                            })
                            .unwrap_or_default();
                        let (list, complete) = fpc::load_cartridge_catalogues(&configured, &cancel);
                        (Arc::new(list), complete)
                    }
                };
                let output = fpc::check_candidates(
                    &candidates,
                    start,
                    candidates.len().saturating_sub(start),
                    &catalogues,
                    &trusted,
                    &cancel,
                    &mut |n| progress.store(n, Ordering::Relaxed),
                );
                Ok((catalogues, complete, output))
            })();
            let _ = sender.send(Message::Done(result));
        });
    }

    pub(super) fn poll(&mut self) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Message::Done(result)) => {
                self.receiver = None;
                self.cancel = None;
                match result {
                    Ok((catalogues, complete, out)) => {
                        // A list cut short by Cancel must never be reused.
                        if complete && !out.cancelled {
                            self.catalogues = Some(catalogues);
                        }
                        self.stopped_early = out.cancelled;
                        if !std::mem::take(&mut self.rechecking) {
                            self.too_large += out.too_large;
                            self.skipped += out.skipped;
                            self.next += out.checked;
                            self.checked += out.checked;
                            self.missing += out.missing;
                            self.unreadable += out.unreadable;
                            for (folder, n) in out.checked_per_folder.clone() {
                                *self.checked_per_folder.entry(folder).or_default() += n;
                            }
                        }
                        // A recheck replaces the earlier result for that game.
                        let rechecked: HashSet<i64> = out
                            .findings
                            .iter()
                            .map(|f| f.archive_id)
                            .chain(out.unknown.iter().map(|u| u.archive_id))
                            .collect();
                        self.findings.retain(|f| !rechecked.contains(&f.archive_id));
                        self.unknown.retain(|u| !rechecked.contains(&u.archive_id));
                        self.findings.extend(out.findings);
                        self.unknown.extend(out.unknown);
                        if std::mem::take(&mut self.auto_after_run) && !out.cancelled {
                            let confirmed: Vec<Finding> = self
                                .findings
                                .iter()
                                .filter(|f| {
                                    f.tier.is_movable() && !self.left_here.contains(&f.archive_id)
                                })
                                .cloned()
                                .collect();
                            if !confirmed.is_empty() {
                                self.organise.plan(
                                    confirmed,
                                    Vec::new(),
                                    Vec::new(),
                                    Vec::new(),
                                    true,
                                );
                            }
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.receiver = None;
                self.cancel = None;
                self.error = Some("The check stopped unexpectedly. Nothing was changed.".into());
            }
        }
    }
}

impl App {
    /// The dismissible card, shown only once a check found Confirmed or
    /// Probable mismatches. Never starts a check by itself.
    pub(super) fn folder_check_cards(&mut self, ui: &mut egui::Ui) {
        let cards = cards(&self.folder_check.findings, &self.folder_check.dismissed);
        let mut review = false;
        let mut dismiss: Option<FolderCard> = None;
        for card in cards.iter().take(3) {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong(card.headline());
                ui.small(READ_ONLY_NOTE);
                ui.horizontal(|ui| {
                    if ui.button("Review detected mismatches").clicked() {
                        review = true;
                    }
                    if ui.button("Not now").clicked() {
                        dismiss = Some(card.clone());
                    }
                });
            });
            ui.add_space(8.0);
        }
        if let Some(card) = dismiss {
            self.folder_check.dismiss(&card);
        }
        if review {
            self.go(super::routes::Route::Tool(
                super::routes::ToolPage::FolderCheck,
            ));
        }
    }

    pub(in crate::gui_v2) fn folder_check_page(&mut self, ui: &mut egui::Ui) {
        self.folder_check.poll();
        self.folder_check.organise.poll();
        if !self.folder_check.preferences_loaded {
            self.folder_check.preferences_loaded = true;
            self.folder_check.preferences =
                super::organise::load_preferences(super::organise::preferences_path().as_deref());
        }
        let busy = self.folder_check.busy();
        let organising = self.folder_check.organise.busy();
        if busy || organising {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        ui.label("Select a folder by adding it as a source in Setup; EmuWiz then identifies every game by its content, shows where each one belongs, and moves only what you approve.");
        ui.small(READ_ONLY_NOTE);
        ui.small("Results: Confirmed = the game's SHA-1 is in a catalogue for that system. Platform confirmed = a calibrated content check establishes the system, though not the exact game. Probable = only the file structure suggests a system. Ambiguous = the evidence disagrees. Unknown = nothing matched any system. A file name, extension or folder alone is never evidence, and no catalogue match never means \"unknown system\".");
        ui.small(KNOWN_LIMITS);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let label = if !self.folder_check.ran {
                "1. Identify my games"
            } else if self.folder_check.has_more() {
                "Continue identifying"
            } else {
                "Identify again"
            };
            if ui
                .add_enabled(!busy && !organising, egui::Button::new(label))
                .clicked()
            {
                if self.folder_check.ran && !self.folder_check.has_more() {
                    self.folder_check.ran = false;
                }
                let games = self.library.games.clone();
                self.folder_check.start(&games);
            }
            if busy && ui.button("Cancel").clicked() {
                self.folder_check.cancel();
            }
        });
        let mut auto = self.folder_check.preferences.auto_organise_confirmed;
        if ui
            .checkbox(&mut auto, "Automatically organise newly confirmed games after each check (off unless you switch it on; Probable, Ambiguous and Unknown games are never moved automatically)")
            .changed()
        {
            self.folder_check.preferences.auto_organise_confirmed = auto;
            if let Some(path) = super::organise::preferences_path()
                && let Err(error) =
                    super::organise::save_preferences(&path, &self.folder_check.preferences)
            {
                self.folder_check.error = Some(format!("The setting could not be saved: {error}"));
            }
        }
        if self.folder_check.ran {
            let done = self.folder_check.next
                + if busy {
                    self.folder_check.progress.load(Ordering::Relaxed)
                } else {
                    0
                };
            ui.label(progress_text(
                done,
                self.folder_check.total(),
                self.folder_check.stopped_early && !busy,
                self.folder_check.missing,
                self.folder_check.unreadable,
                self.folder_check.too_large,
                self.folder_check.skipped,
            ));
        } else {
            ui.label("Nothing has been read yet.");
        }
        if let Some(error) = &self.folder_check.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        self.organise_panel(ui, busy);
        let left_here = self.folder_check.left_here.clone();
        let mut open: Option<i64> = None;
        let mut move_one: Option<i64> = None;
        let mut leave: Option<i64> = None;
        let mut recheck: Option<i64> = None;
        let mut unknown_to_folder: Option<i64> = None;
        egui::ScrollArea::vertical()
            .id_salt("v2_folder_check_results")
            .show(ui, |ui| {
                for (detected, tiers) in grouped(&self.folder_check.findings) {
                    ui.heading(format!("Belong in {}", display_name_for(&detected)));
                    for (tier, items) in tiers {
                        let items: Vec<&&Finding> = items.iter().filter(|f| !left_here.contains(&f.archive_id)).collect();
                        if items.is_empty() {
                            continue;
                        }
                        egui::CollapsingHeader::new(format!("{} ({})", tier.label(), items.len()))
                            .id_salt((detected.clone(), tier))
                            .default_open(tier.is_movable())
                            .show(ui, |ui| {
                                for f in items.iter().take(100) {
                                    ui.group(|ui| {
                                        ui.strong(&f.title);
                                        ui.label(format!(
                                            "Now in {} (recorded as {}, source: {}). Content says: {}.",
                                            folder_label(&f.folder),
                                            display_name_for(&f.assigned),
                                            f.assigned_source.as_deref().unwrap_or("not recorded"),
                                            display_name_for(&f.detected),
                                        ));
                                        for line in &f.lines {
                                            ui.small(line);
                                        }
                                        ui.horizontal(|ui| {
                                            if tier.is_movable() && ui.button("Move to correct folder").clicked() {
                                                move_one = Some(f.archive_id);
                                            }
                                            if ui.button("Review").clicked() {
                                                open = Some(f.archive_id);
                                            }
                                            if ui.button("Choose system").clicked() {
                                                open = Some(f.archive_id);
                                            }
                                            if ui.button("Leave here").clicked() {
                                                leave = Some(f.archive_id);
                                            }
                                        });
                                    });
                                }
                                if items.len() > 100 {
                                    ui.small(format!("Showing 100 of {}. Organise my games covers all of them.", items.len()));
                                }
                            });
                    }
                }
                let unknown: Vec<&UnknownResult> = self
                    .folder_check
                    .unknown
                    .iter()
                    .filter(|u| !left_here.contains(&u.archive_id))
                    .collect();
                if !unknown.is_empty() {
                    ui.heading(format!("Unknown games ({})", unknown.len()));
                    ui.small("Nothing identified these yet. Recheck after adding catalogues, choose the system yourself, move them to Unnamed/Unknown, or leave them here.");
                    for u in unknown.iter().take(100) {
                        ui.group(|ui| {
                            ui.strong(&u.title);
                            ui.small(u.path.display().to_string());
                            ui.small(&u.reason);
                            ui.horizontal(|ui| {
                                if ui.button("Recheck").clicked() {
                                    recheck = Some(u.archive_id);
                                }
                                if ui.button("Choose system").clicked() {
                                    open = Some(u.archive_id);
                                }
                                if ui.button("Move to Unnamed/Unknown").clicked() {
                                    unknown_to_folder = Some(u.archive_id);
                                }
                                if ui.button("Leave here").clicked() {
                                    leave = Some(u.archive_id);
                                }
                            });
                        });
                    }
                }
            });
        if let Some(id) = leave {
            self.folder_check.left_here.insert(id);
        }
        if let Some(id) = recheck {
            self.folder_check.recheck(&HashSet::from([id]));
        }
        if let Some(id) = move_one {
            let one: Vec<Finding> = self
                .folder_check
                .findings
                .iter()
                .filter(|f| f.archive_id == id)
                .cloned()
                .collect();
            self.folder_check
                .organise
                .plan(one, Vec::new(), Vec::new(), Vec::new(), false);
        }
        if let Some(id) = unknown_to_folder {
            let one: Vec<fpc::organise::UnknownGame> = self
                .folder_check
                .unknown
                .iter()
                .filter(|u| u.archive_id == id)
                .map(|u| fpc::organise::UnknownGame {
                    archive_id: u.archive_id,
                    path: u.path.clone(),
                })
                .collect();
            self.folder_check
                .organise
                .plan(Vec::new(), Vec::new(), one, Vec::new(), false);
        }
        if let Some(id) = open {
            self.go(super::routes::Route::ReviewIdentity(id));
        }
    }

    /// Organise my games: the batch button, the preview with one approval,
    /// the result and Undo.
    fn organise_panel(&mut self, ui: &mut egui::Ui, checking: bool) {
        use super::organise::Phase;
        let left_here = &self.folder_check.left_here;
        let movable: Vec<Finding> = self
            .folder_check
            .findings
            .iter()
            .filter(|f| f.tier.is_movable() && !left_here.contains(&f.archive_id))
            .cloned()
            .collect();
        let mut chosen: Vec<fpc::organise::ChosenGame> =
            super::organise::chosen_games(&self.library.games)
                .into_iter()
                .filter(|g| !left_here.contains(&g.archive_id))
                .collect();
        chosen.extend(self.folder_check.picked_files.iter().cloned());
        let unknown: Vec<fpc::organise::UnknownGame> = self
            .folder_check
            .unknown
            .iter()
            .filter(|u| !left_here.contains(&u.archive_id))
            .map(|u| fpc::organise::UnknownGame {
                archive_id: u.archive_id,
                path: u.path.clone(),
            })
            .collect();
        let picked = self.folder_check.picked_folders.clone();
        self.game_folder_picker(ui, checking);
        let organise = &mut self.folder_check.organise;
        ui.add_space(6.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Organise my games");
            match &organise.phase {
                Phase::Idle | Phase::Failed(_) | Phase::Done(_) => {
                    ui.label(format!(
                        "{} confirmed game(s) are outside their system's folder; {} game(s) you assigned yourself are outside theirs; {} unknown.",
                        movable.len(),
                        chosen.len(),
                        unknown.len()
                    ));
                    ui.checkbox(&mut organise.include_unknown, "Also move unknown games to Unnamed/Unknown (their original location is journalled, and Recheck can still find their system later)");
                    ui.small("Game folders (such as PS3 disc folders found by their PARAM.SFO layout) and disc sets (a .cue, .gdi or .m3u with its tracks) are included and always move complete.");
                    if ui
                        .add_enabled(!checking && !organise.busy(), egui::Button::new("2. Review and move all confirmed games"))
                        .clicked()
                    {
                        let unknown = if organise.include_unknown { unknown.clone() } else { Vec::new() };
                        organise.plan(movable.clone(), chosen.clone(), unknown, picked.clone(), false);
                    }
                }
                Phase::Working(text) => {
                    ui.label(*text);
                }
                Phase::Preview(..) => {}
            }
            let mut approve = false;
            let mut discard = false;
            if let Phase::Preview(plans, folders, summary) = &organise.phase {
                let (folder_moves, folders_in_place, folders_refused) =
                    super::organise::folder_counts(folders);
                ui.label(format!(
                    "Preview: {} game(s) will move to their system folder and {} to Unnamed/Unknown. {} are already in the right place, {} would collide with an existing file and stay where they are, and {} are left for you to review.",
                    summary.to_move,
                    summary.to_unknown,
                    summary.already_correct,
                    summary.collisions,
                    summary.left_alone + summary.blocked
                ));
                if folder_moves + folders_in_place + folders_refused > 0 {
                    ui.label(format!(
                        "Game folders: {folder_moves} will move whole, {folders_in_place} already in place, {folders_refused} stay where they are."
                    ));
                }
                egui::ScrollArea::vertical().id_salt("v2_organise_preview").min_scrolled_height(220.0).max_height(320.0).show(ui, |ui| {
                    for (candidate, preview) in folders {
                        use archivefs_core::folder_platform_check::organise::folder_move::FolderPreview;
                        let why = if candidate.chosen { "your choice" } else { candidate.evidence.as_str() };
                        match preview {
                            FolderPreview::Move(plan) => {
                                ui.label(format!(
                                    "Whole folder {} -> {} ({} files, {}){}. System: {} ({why}).",
                                    candidate.path.display(),
                                    plan.destination.display(),
                                    plan.layout.entries.iter().filter(|e| !e.is_dir).count(),
                                    human_bytes(plan.layout.total_bytes),
                                    if plan.creates_folder { ", new folder created on approval" } else { "" },
                                    display_name_for(&plan.platform),
                                ));
                            }
                            FolderPreview::AlreadyInPlace => {
                                ui.small(format!("Already in place: {}", candidate.path.display()));
                            }
                            FolderPreview::Refused(reason) => {
                                ui.small(format!("Stays: {} - {reason}", candidate.path.display()));
                            }
                        }
                    }
                    for plan in plans {
                        for (lead, members) in plan.sets.iter().take(50) {
                            ui.small(format!(
                                "Set: {} moves with {} track/disc file{}.",
                                lead.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                members.len(),
                                if members.len() == 1 { "" } else { "s" }
                            ));
                        }
                        let mut by_folder: BTreeMap<PathBuf, usize> = BTreeMap::new();
                        for e in plan.plan.suggested() {
                            if let Some(parent) = e.destination_path.parent() {
                                *by_folder.entry(parent.to_path_buf()).or_default() += 1;
                            }
                        }
                        for (folder, n) in &by_folder {
                            let new = if folder.exists() { "" } else { " (new folder, created on approval)" };
                            ui.label(format!("{n} game(s) -> {}{new}", folder.display()));
                        }
                        for e in plan.plan.entries.iter().filter(|e| e.status != archivefs_core::dat::rom_organisation::OrganisationStatus::Suggested && e.status != archivefs_core::dat::rom_organisation::OrganisationStatus::AlreadyOrganised).take(50) {
                            ui.small(format!(
                                "Stays: {} - {}",
                                e.source_path.display(),
                                e.reason.as_deref().unwrap_or(e.status.label())
                            ));
                        }
                        for (path, why) in plan.left_alone.iter().take(50) {
                            ui.small(format!("Stays: {} - {why}", path.display()));
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let total = summary.to_move + summary.to_unknown + folder_moves;
                    let label = format!("3. Organise {total} game(s)");
                    if ui.add_enabled(total > 0, egui::Button::new(label)).clicked() {
                        approve = true;
                    }
                    if ui.button("Not now").clicked() {
                        discard = true;
                    }
                });
                ui.small("Nothing is overwritten, renamed, extracted or recompressed. Each move is checked against the file you previewed and journalled; Undo puts every game and its system back.");
            }
            if approve {
                organise.approve();
            }
            if discard {
                organise.phase = Phase::Idle;
            }
            if let Phase::Done(report) = &organise.phase {
                for line in report.lines() {
                    ui.label(line);
                }
            }
            if let Phase::Failed(error) = &organise.phase {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            if (!organise.applied.is_empty() || !organise.applied_folders.is_empty())
                && !organise.busy()
                && ui.button("Undo this organisation").clicked()
            {
                organise.undo();
            }
            if let Some(message) = &organise.undo_message {
                ui.label(message);
            }
        });
    }

    /// Pick a game folder and give it a system; it joins the next preview.
    fn game_folder_picker(&mut self, ui: &mut egui::Ui, checking: bool) {
        let state = &mut self.folder_check;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!checking && !state.organise.busy(), egui::Button::new("Add a game folder..."))
                .on_hover_text("For a game stored as a folder (for example a PS3 disc folder): choose its system and EmuWiz moves the whole folder into that system's folder after the preview.")
                .clicked()
                && let Some(path) = rfd::FileDialog::new().set_title("Choose one game's folder").pick_folder()
            {
                state.pick = Some((path, String::new()));
            }
            if ui
                .add_enabled(!checking && !state.organise.busy(), egui::Button::new("Add a disc set or file..."))
                .on_hover_text("For a .cue, .gdi or .m3u (its tracks and discs move with it) or any single game file the library has not identified: choose its system and it joins the preview.")
                .clicked()
                && let Some(path) = rfd::FileDialog::new().set_title("Choose a disc set (.cue, .gdi, .m3u) or a game file").pick_file()
            {
                state.pick = Some((path, String::new()));
            }
            ui.add(
                egui::TextEdit::singleline(&mut state.typed_path)
                    .hint_text("or type a game folder or file path")
                    .desired_width(260.0),
            );
            let typed = PathBuf::from(state.typed_path.trim());
            if ui
                .add_enabled(typed.is_absolute() && typed.exists(), egui::Button::new("Use this path"))
                .clicked()
            {
                state.pick = Some((typed, String::new()));
                state.typed_path.clear();
            }
            for (i, file) in state.picked_files.clone().iter().enumerate() {
                ui.small(format!("{} -> {}", file.path.display(), display_name_for(&file.platform)));
                if ui.small_button("Remove").clicked() {
                    state.picked_files.remove(i);
                }
            }
            for (i, folder) in state.picked_folders.clone().iter().enumerate() {
                ui.small(format!(
                    "{} -> {}",
                    folder.path.display(),
                    display_name_for(&folder.platform)
                ));
                if ui.small_button("Remove").clicked() {
                    state.picked_folders.remove(i);
                }
            }
        });
        let mut add = None;
        let mut cancel = false;
        if let Some((path, platform)) = &mut state.pick {
            ui.group(|ui| {
                let is_folder = path.is_dir();
                ui.label(format!(
                    "{}: {}",
                    if is_folder {
                        "Game folder"
                    } else {
                        "Game file"
                    },
                    path.display()
                ));
                if let Some(title) = archivefs_core::game_package::ps3_game_folder(path) {
                    ui.small(format!(
                        "Its layout is a PS3 disc folder (title ID {title})."
                    ));
                }
                let filter = &mut state.system_filter;
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(filter)
                            .hint_text("Filter systems")
                            .desired_width(140.0),
                    );
                    let needle = filter.trim().to_ascii_lowercase();
                    let mut systems: Vec<_> = archivefs_core::platform::PLATFORMS
                        .iter()
                        .filter(|p| {
                            needle.is_empty()
                                || p.display_name.to_ascii_lowercase().contains(&needle)
                                || p.id.to_ascii_lowercase().contains(&needle)
                        })
                        .collect();
                    systems.sort_by_key(|p| p.display_name);
                    egui::ComboBox::from_id_salt("v2_pick_folder_system")
                        .selected_text(if platform.is_empty() {
                            "Choose system".to_string()
                        } else {
                            display_name_for(platform).to_string()
                        })
                        .height(360.0)
                        .show_ui(ui, |ui| {
                            for p in systems {
                                ui.selectable_value(platform, p.id.to_string(), p.display_name);
                            }
                        });
                    if ui
                        .add_enabled(
                            !platform.is_empty(),
                            egui::Button::new("Add to the preview"),
                        )
                        .clicked()
                    {
                        add = Some(fpc::organise::FolderCandidate {
                            path: path.clone(),
                            platform: platform.clone(),
                            evidence: "your system choice".into(),
                            chosen: true,
                        });
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        }
        if let Some(candidate) = add {
            if candidate.path.is_dir() {
                state.picked_folders.retain(|f| f.path != candidate.path);
                state.picked_folders.push(candidate);
            } else {
                state.picked_files.retain(|f| f.path != candidate.path);
                state.picked_files.push(fpc::organise::ChosenGame {
                    archive_id: 0,
                    path: candidate.path,
                    platform: candidate.platform,
                });
            }
            state.pick = None;
        }
        if cancel {
            state.pick = None;
        }
    }
}

fn human_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else {
        format!("{bytes} bytes")
    }
}
