//! Smart ROM Folder Check in Blue (phases 1-2, read only).
//!
//! Compares each cartridge game's bytes with the catalogues and with a few
//! content structures, and lists games whose evidence points at a different
//! system than the folder's. It only looks: nothing is assigned, moved or
//! saved. Results live in memory; dismissal lasts for this session only (a
//! persisted dismissal would need a new preferences field, which this change
//! deliberately avoids). Changing a system still goes through the existing
//! Review identity -> Choose system -> manual assignment path.

use super::{App, library::Game};
use archivefs_core::folder_platform_check::{
    self as fpc, Candidate, Catalogue, DEFAULT_RUN_CAP, Finding, RunOutput, Tier,
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
    "This only looks for mismatches. Nothing is changed or moved.";
pub(super) const KNOWN_LIMITS: &str = "Known limits: a renamed or moved file gets a new catalogue id and does not keep a system you chose; removing a source folder deletes the systems recorded for it. The Probable group is a heuristic based on file structure, not a catalogue match.";

/// Why changing the system is not the same as making a game playable.
pub(super) fn playable_note(detected: &str) -> String {
    let mut text = String::from(
        "Changing the system only records the platform for this game. It does not necessarily make the ROM playable: Blue's identity and launch checks for the new system still apply.",
    );
    if detected == "NES" {
        text.push_str(" For example, NES \".unh\" unheadered images are not accepted by Blue's NES identity or launch yet, so such a game stays unlaunchable after the change.");
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
    lines.push(playable_note(selected));
    lines.push("A renamed or moved file gets a new catalogue id and does not keep this choice; removing a source folder deletes the systems recorded for it.".into());
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

/// The share of checked games in a folder that disagree, as text.
pub(super) fn disagreement_text(disagree: usize, checked: usize) -> String {
    if checked == 0 {
        return "No games were read in this folder.".into();
    }
    format!(
        "{disagree} of {checked} checked games in this folder disagree with its system. The folder may hold several systems; each game is judged on its own."
    )
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

pub(super) fn candidates_of(games: &[Game]) -> Vec<Candidate> {
    let cartridge = fpc::cartridge_platforms();
    let mut out: Vec<Candidate> = games
        .iter()
        .filter_map(|g| {
            let assigned = archivefs_core::canonical_platform_for_alias(&g.platform)?;
            cartridge.contains(&assigned).then(|| Candidate {
                archive_id: g.archive.id,
                title: g.title.clone(),
                path: g.archive.absolute_path.clone(),
                assigned: assigned.to_string(),
                assigned_source: g.archive.platform_source.clone(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

enum Message {
    Done(Result<(Arc<Vec<Catalogue>>, RunOutput), String>),
}

#[derive(Default)]
pub(super) struct State {
    pub candidates: Vec<Candidate>,
    pub next: usize,
    pub findings: Vec<Finding>,
    pub checked_per_folder: BTreeMap<PathBuf, usize>,
    pub checked: usize,
    pub unreadable: usize,
    pub error: Option<String>,
    pub ran: bool,
    pub dismissed: HashSet<(PathBuf, u64)>,
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

    /// Starts one capped pass over the next games. Read only.
    pub(super) fn start(&mut self, games: &[Game]) {
        if self.busy() {
            return;
        }
        if !self.ran {
            *self = Self {
                dismissed: std::mem::take(&mut self.dismissed),
                candidates: candidates_of(games),
                ran: true,
                ..Default::default()
            };
        }
        let candidates = self.candidates.clone();
        let start = self.next;
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
                let catalogues = match known {
                    Some(c) => c,
                    None => {
                        let configured = archivefs_core::dat::sources::default_dat_sources_config_path()
                            .map(|p| {
                                archivefs_core::dat::configured_catalogues::load_configured_catalogues(&p)
                            })
                            .unwrap_or_default();
                        Arc::new(fpc::load_cartridge_catalogues(&configured, &cancel))
                    }
                };
                let output = fpc::check_candidates(
                    &candidates,
                    start,
                    DEFAULT_RUN_CAP,
                    &catalogues,
                    &trusted,
                    &cancel,
                    &mut |n| progress.store(n, Ordering::Relaxed),
                );
                Ok((catalogues, output))
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
                    Ok((catalogues, out)) => {
                        self.catalogues = Some(catalogues);
                        self.next += out.checked;
                        self.checked += out.checked;
                        self.unreadable += out.unreadable;
                        for (folder, n) in out.checked_per_folder {
                            *self.checked_per_folder.entry(folder).or_default() += n;
                        }
                        self.findings.extend(out.findings);
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
        if self.folder_check.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        ui.label(READ_ONLY_NOTE);
        ui.small("Evidence tiers: Confirmed = the file's SHA-1 is in a catalogue for another system and nothing contradicts it. Probable = the file's structure points at another system but no catalogue hash matched. Ambiguous = signals conflict. A file name or extension alone is never evidence.");
        ui.small(KNOWN_LIMITS);
        ui.add_space(6.0);
        let busy = self.folder_check.busy();
        ui.horizontal(|ui| {
            let label = if self.folder_check.ran {
                "Check the next games"
            } else {
                "Run folder check"
            };
            let can = !busy && (!self.folder_check.ran || self.folder_check.has_more());
            if ui.add_enabled(can, egui::Button::new(label)).clicked() {
                let games = self.library.games.clone();
                self.folder_check.start(&games);
            }
            if busy && ui.button("Cancel").clicked() {
                self.folder_check.cancel();
            }
        });
        if self.folder_check.ran {
            let done = self.folder_check.next
                + if busy {
                    self.folder_check.progress.load(Ordering::Relaxed)
                } else {
                    0
                };
            ui.label(format!(
                "Checked {} of {} cartridge games (up to {} per run).{}",
                done,
                self.folder_check.total(),
                DEFAULT_RUN_CAP,
                if self.folder_check.unreadable > 0 {
                    format!(
                        " {} could not be read and were skipped.",
                        self.folder_check.unreadable
                    )
                } else {
                    String::new()
                },
            ));
        } else {
            ui.label("Nothing has been read yet. The check reads one capped batch of cartridge games when you start it.");
        }
        if let Some(error) = &self.folder_check.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        let mut open: Option<i64> = None;
        for (folder, checked) in &self.folder_check.checked_per_folder {
            let disagree = self
                .folder_check
                .findings
                .iter()
                .filter(|f| &f.folder == folder)
                .count();
            if disagree > 0 {
                ui.small(format!(
                    "{}: {}",
                    folder_label(folder),
                    disagreement_text(disagree, *checked)
                ));
            }
        }
        egui::ScrollArea::vertical()
            .id_salt("v2_folder_check_results")
            .show(ui, |ui| {
                for (detected, tiers) in grouped(&self.folder_check.findings) {
                    ui.heading(format!("Look like {}", display_name_for(&detected)));
                    for (tier, items) in tiers {
                        egui::CollapsingHeader::new(format!("{} ({})", tier.label(), items.len()))
                            .id_salt((detected.clone(), tier))
                            .default_open(tier == Tier::Confirmed)
                            .show(ui, |ui| {
                                for f in items.iter().take(100) {
                                    ui.group(|ui| {
                                        ui.strong(&f.title);
                                        ui.label(format!(
                                            "Folder system: {} (source: {}). Detected: {}.",
                                            display_name_for(&f.assigned),
                                            f.assigned_source.as_deref().unwrap_or("not recorded"),
                                            display_name_for(&f.detected),
                                        ));
                                        for line in &f.lines {
                                            ui.small(line);
                                        }
                                        if ui.button("Review identity").clicked() {
                                            open = Some(f.archive_id);
                                        }
                                    });
                                }
                                if items.len() > 100 {
                                    ui.small(format!("Showing 100 of {}.", items.len()));
                                }
                            });
                    }
                }
            });
        if let Some(id) = open {
            self.go(super::routes::Route::ReviewIdentity(id));
        }
    }
}
