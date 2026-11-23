//! "Organise my games": preview, one approval, move, verify, Undo.
//!
//! Planning runs on a worker and only reads. Nothing moves until the person
//! presses the approval button on the preview (or has explicitly switched on
//! automatic organising of confirmed games). Moves go through the shared
//! journalled organiser, so every batch appears in History with Undo.

use archivefs_core::dat::rename_apply::EntryState;
use archivefs_core::folder_platform_check::Finding;
use archivefs_core::folder_platform_check::organise::folder_move::{
    self, FolderMoveRecord, FolderPreview,
};
use archivefs_core::folder_platform_check::organise::{
    self as org, ChosenGame, FolderCandidate, RootPlan, Summary, UnknownGame,
};
use archivefs_core::retroarch_playlist_relink::{self as relink, RelinkReport};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Preferences kept beside the library database. Automatic organising is
/// off unless the person switches it on.
pub(super) const PREFERENCES_FILE: &str = "organise-preferences.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Preferences {
    #[serde(default)]
    pub auto_organise_confirmed: bool,
}

pub(super) fn preferences_path() -> Option<PathBuf> {
    archivefs_core::default_database_path()
        .ok()
        .map(|db| db.with_file_name(PREFERENCES_FILE))
}

pub(super) fn load_preferences(path: Option<&Path>) -> Preferences {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(super) fn save_preferences(path: &Path, preferences: &Preferences) -> Result<(), String> {
    let text = serde_json::to_string_pretty(preferences).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// What one approved run did, in the person's terms.
#[derive(Debug, Clone, Default)]
pub(super) struct Report {
    pub organised: usize,
    pub to_unknown: usize,
    pub already_correct: usize,
    pub collisions: usize,
    pub left_alone: usize,
    pub failed: Vec<String>,
    /// Destination folders that the run created.
    pub created_folders: Vec<PathBuf>,
    pub retroarch: RelinkReport,
    /// Systems RomM should rescan (old and new folders).
    pub romm_platforms: BTreeSet<String>,
    pub transactions: Vec<String>,
    /// Whole game folders moved.
    pub folders_moved: usize,
    /// Set members (tracks, discs) that moved with their set.
    pub set_members_moved: usize,
    /// Interrupted earlier folder moves settled before this run.
    pub recovered: Vec<String>,
}

impl Report {
    pub(super) fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "Organised {} game{} into their system folders; {} moved to Unnamed/Unknown.",
            self.organised,
            if self.organised == 1 { "" } else { "s" },
            self.to_unknown
        )];
        lines.push(format!(
            "Already correct: {}. Collisions left untouched: {}. Left for you to review: {}. Failed: {}.",
            self.already_correct,
            self.collisions,
            self.left_alone,
            self.failed.len()
        ));
        if self.folders_moved > 0 {
            lines.push(format!(
                "{} of these {} whole game folder{}, moved as one unit with every file inside unchanged.",
                self.folders_moved,
                if self.folders_moved == 1 { "is a" } else { "are" },
                if self.folders_moved == 1 { "" } else { "s" }
            ));
        }
        if self.set_members_moved > 0 {
            lines.push(format!(
                "{} track or disc file{} moved together with {} set.",
                self.set_members_moved,
                if self.set_members_moved == 1 { "" } else { "s" },
                if self.set_members_moved == 1 {
                    "its"
                } else {
                    "their"
                }
            ));
        }
        for line in &self.recovered {
            lines.push(line.clone());
        }
        for folder in &self.created_folders {
            lines.push(format!("Created folder {}.", folder.display()));
        }
        for failure in self.failed.iter().take(20) {
            lines.push(format!("Not moved: {failure}"));
        }
        if self.retroarch.retroarch_running {
            lines.push("RetroArch is open, so its playlists were not updated. Close RetroArch and run Organise again to re-link them; nothing else is affected.".into());
        } else if !self.retroarch.rewritten.is_empty() {
            lines.push(format!(
                "RetroArch: re-linked {} playlist entr{} in {} file{} (the previous version of each is kept beside it).",
                self.retroarch.items,
                if self.retroarch.items == 1 { "y" } else { "ies" },
                self.retroarch.rewritten.len(),
                if self.retroarch.rewritten.len() == 1 { "" } else { "s" },
            ));
        }
        for (file, why) in &self.retroarch.skipped {
            lines.push(format!(
                "RetroArch playlist {} was not changed: {why}.",
                file.display()
            ));
        }
        if !self.romm_platforms.is_empty() && self.organised + self.to_unknown > 0 {
            lines.push(format!(
                "RomM: run a Quick scan of {} so it sees the new locations. EmuWiz never edits RomM's database.",
                self.romm_platforms.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if !self.transactions.is_empty() {
            lines.push(
                "Every move is journalled. To put everything back, use Undo here or in History."
                    .into(),
            );
        }
        lines
    }
}

/// Game folders in a preview, with what will happen to each.
pub(super) type FolderPreviews = Vec<(FolderCandidate, FolderPreview)>;

/// A finished preview: plans, folder moves, and games no plan covers.
type Planned = (Vec<RootPlan>, FolderPreviews, Vec<(PathBuf, String)>);

enum Message {
    Planned(Result<Planned, String>, bool),
    Applied(
        Box<Report>,
        Vec<archivefs_core::dat::rename_apply::RenameTransaction>,
        Vec<FolderMoveRecord>,
    ),
    /// Only interrupted folder moves were settled; nothing new was moved.
    Settled(Box<Report>),
}

/// Folder-move records left for this release to finish or a person to
/// review: (interrupted moves or Undos, unreadable records).
pub(super) fn pending_folder_moves() -> (usize, usize) {
    match archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir() {
        Ok(journal) => (
            folder_move::interrupted(&journal).len(),
            folder_move::unreadable(&journal).len(),
        ),
        Err(_) => (0, 0),
    }
}

/// Reports unreadable folder-move records (left untouched) and settles
/// every interrupted folder move or Undo from the files and the library.
fn settle_folder_moves(journal: &Path, report: &mut Report) {
    // A folder-move record this release cannot read (damaged, or written by
    // a newer EmuWiz) is reported and left untouched, never skipped silently.
    for (path, why) in folder_move::unreadable(journal) {
        report.failed.push(format!(
            "A folder-move record cannot be read and was left untouched: {} ({why}). Review it before moving that folder again.",
            path.display()
        ));
    }
    for mut record in folder_move::interrupted(journal) {
        match folder_move::recover(&mut record, journal) {
            Ok(state) => report.recovered.push(format!(
                "An interrupted earlier move of {} was settled: {state:?}.",
                record.source.display()
            )),
            Err(why) => report.failed.push(format!(
                "An interrupted earlier move of {} needs review: {why}",
                record.source.display()
            )),
        }
    }
}

#[derive(Default)]
pub(super) enum Phase {
    #[default]
    Idle,
    Working(&'static str),
    /// Plans, folder moves, counts, and picked games no plan covers (outside
    /// every source folder, or nothing to plan), each with its reason.
    Preview(
        Vec<RootPlan>,
        FolderPreviews,
        Summary,
        Vec<(PathBuf, String)>,
    ),
    Done(Report),
    Failed(String),
}

/// Preview lines for picked games that no plan covers, so a preview of only
/// such games explains itself instead of showing zero. Lists at most 50.
pub(super) fn excluded_lines(excluded: &[(PathBuf, String)]) -> Vec<String> {
    if excluded.is_empty() {
        return Vec::new();
    }
    let n = excluded.len();
    let mut lines = vec![format!(
        "{n} picked game{} not included in this preview and will not be moved:",
        if n == 1 { " is" } else { "s are" }
    )];
    for (path, why) in excluded.iter().take(50) {
        lines.push(format!("Not moved: {} - {why}", path.display()));
    }
    if n > 50 {
        lines.push(format!("...and {} more.", n - 50));
    }
    lines
}

/// Folder moves in a preview: (to move, already in place, refused).
pub(super) fn folder_counts(folders: &FolderPreviews) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for (_, preview) in folders {
        match preview {
            FolderPreview::Move(_) => counts.0 += 1,
            FolderPreview::AlreadyInPlace => counts.1 += 1,
            FolderPreview::Refused(_) => counts.2 += 1,
        }
    }
    counts
}

#[derive(Default)]
pub(super) struct State {
    pub phase: Phase,
    /// Also move checked games with no identifiable system to Unnamed/Unknown.
    pub include_unknown: bool,
    /// Set after a run changed the library, so Blue reloads it.
    pub library_changed: bool,
    /// The last applied transactions, for Undo.
    pub applied: Vec<archivefs_core::dat::rename_apply::RenameTransaction>,
    /// The last run's folder moves, for Undo.
    pub applied_folders: Vec<FolderMoveRecord>,
    pub undo_message: Option<String>,
    /// Interrupted and unreadable folder-move records, as of the last
    /// preview or run (read once when the page first shows, if unknown).
    pub pending_folder_moves: Option<(usize, usize)>,
    receiver: Option<Receiver<Message>>,
}

impl State {
    pub(super) fn busy(&self) -> bool {
        self.receiver.is_some()
    }

    /// Builds the preview on a worker. With `then_apply`, applies it straight
    /// away (only used when the person enabled automatic organising, and then
    /// only for confirmed games).
    pub(super) fn plan(
        &mut self,
        findings: Vec<Finding>,
        chosen: Vec<ChosenGame>,
        unknown: Vec<UnknownGame>,
        picked_folders: Vec<FolderCandidate>,
        then_apply: bool,
    ) {
        if self.busy() {
            return;
        }
        self.phase = Phase::Working("Preparing the preview. Nothing is moved yet.");
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = (|| {
                let config = archivefs_core::Config::load_default().map_err(|e| e.to_string())?;
                let roots = config.source_folders.clone();
                let refs: Vec<&Finding> = findings.iter().filter(|f| f.tier.is_movable()).collect();
                // Games that are folders move as whole folders.
                let (chosen_folders, chosen_files): (Vec<ChosenGame>, Vec<ChosenGame>) = chosen
                    .into_iter()
                    .partition(|g| std::fs::symlink_metadata(&g.path).is_ok_and(|m| m.is_dir()));
                let mut folders: Vec<FolderCandidate> = picked_folders;
                folders.extend(chosen_folders.into_iter().map(|g| FolderCandidate {
                    path: g.path,
                    platform: g.platform,
                    evidence: "your system choice".into(),
                    chosen: true,
                }));
                // Automatic runs only ever move confirmed single games.
                if !then_apply {
                    for found in org::found_game_folders(&roots) {
                        if !folders.iter().any(|f| f.path == found.path) {
                            folders.push(found);
                        }
                    }
                }
                let (mut plans, mut excluded) = org::plan_organisation_with_exclusions(
                    &refs,
                    &chosen_files,
                    &unknown,
                    &roots,
                    1,
                );
                for f in findings.iter().filter(|f| !f.tier.is_movable()) {
                    let note = (
                        f.path.clone(),
                        format!("{}: not moved ({}).", f.title, f.tier.label()),
                    );
                    match plans.first_mut() {
                        Some(first) => first.left_alone.push(note),
                        None => excluded.push(note),
                    }
                }
                let database = archivefs_core::default_database_path()
                    .and_then(archivefs_core::Database::open_read_only)
                    .map_err(|e| e.to_string())?;
                for plan in &mut plans {
                    if plan.plan.suggested().next().is_some() {
                        org::prepare(plan, &database).map_err(|reason| {
                            format!("The preview for {} could not be sealed: {reason}. Nothing was moved.", plan.root.display())
                        })?;
                    }
                }
                let folder_previews = org::preview_folders(&database, &roots, &folders);
                Ok((plans, folder_previews, excluded))
            })();
            let _ = sender.send(Message::Planned(result, then_apply));
        });
    }

    /// The person's one approval for every Suggested entry in the preview.
    pub(super) fn approve(&mut self) {
        let Phase::Preview(plans, folders, _, _) = std::mem::take(&mut self.phase) else {
            return;
        };
        self.start_apply(plans, folders);
    }

    fn start_apply(&mut self, plans: Vec<RootPlan>, folders: FolderPreviews) {
        self.phase = Phase::Working("Moving the approved games. Each move is journalled.");
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let (report, transactions, records) = apply_plans(&plans, &folders);
            let _ = sender.send(Message::Applied(Box::new(report), transactions, records));
        });
    }

    /// Settles interrupted folder moves and Undos without moving anything
    /// new, for when no preview has anything to organise.
    pub(super) fn finish_interrupted(&mut self) {
        if self.busy() {
            return;
        }
        self.phase = Phase::Working("Finishing interrupted folder moves. Nothing new is moved.");
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let mut report = Report::default();
            match archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir() {
                Ok(journal) => settle_folder_moves(&journal, &mut report),
                Err(error) => report
                    .failed
                    .push(format!("the journal folder is not available: {error}")),
            }
            let _ = sender.send(Message::Settled(Box::new(report)));
        });
    }

    pub(super) fn poll(&mut self) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Message::Planned(result, then_apply)) => {
                self.receiver = None;
                match result {
                    Ok((plans, folders, _)) if then_apply => self.start_apply(plans, folders),
                    Ok((plans, folders, excluded)) => {
                        self.pending_folder_moves = Some(pending_folder_moves());
                        let summary = org::summarise(&plans);
                        self.phase = Phase::Preview(plans, folders, summary, excluded);
                    }
                    Err(error) => self.phase = Phase::Failed(error),
                }
            }
            Ok(Message::Applied(report, transactions, records)) => {
                self.receiver = None;
                if report.organised + report.to_unknown > 0 {
                    self.library_changed = true;
                }
                self.applied = transactions;
                self.applied_folders = records;
                self.pending_folder_moves = Some(pending_folder_moves());
                self.phase = Phase::Done(*report);
            }
            Ok(Message::Settled(report)) => {
                self.receiver = None;
                if !report.recovered.is_empty() {
                    self.library_changed = true;
                }
                self.pending_folder_moves = Some(pending_folder_moves());
                self.phase = Phase::Done(*report);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.receiver = None;
                self.phase = Phase::Failed(
                    "Organising stopped unexpectedly. Any move already made is journalled; open History to review or Undo it.".into(),
                );
            }
        }
    }

    pub(super) fn take_library_changed(&mut self) -> bool {
        std::mem::take(&mut self.library_changed)
    }

    /// Undo of the last run, newest transaction first. Also reverses the
    /// platform corrections sealed into each move.
    pub(super) fn undo(&mut self) {
        let Ok(journal) =
            archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
        else {
            self.undo_message = Some("The journal folder is not available; use History.".into());
            return;
        };
        let mut restored = 0;
        let mut problems = Vec::new();
        // Folder moves ran last, so they are undone first.
        for mut record in std::mem::take(&mut self.applied_folders).into_iter().rev() {
            match folder_move::undo(&mut record, &journal) {
                Ok(()) => restored += 1,
                Err(why) => problems.push(format!("{}: {why}", record.destination.display())),
            }
        }
        for mut transaction in std::mem::take(&mut self.applied).into_iter().rev() {
            let root = PathBuf::from(&transaction.source_scan_root);
            let moved = transaction
                .entries
                .iter()
                .filter(|e| e.state == EntryState::Applied)
                .count();
            use archivefs_core::dat::rename_apply::RollbackResult;
            match archivefs_core::dat::rom_organisation::rollback_organisation_transaction(
                &mut transaction,
                &journal,
                &AtomicBool::new(false),
                &root,
            ) {
                Ok(outcome) => match outcome.rollback.result {
                    RollbackResult::FullyRolledBack => restored += moved,
                    RollbackResult::PartiallyRolledBack {
                        rolled_back,
                        failed,
                    } => {
                        restored += rolled_back.len();
                        problems.extend(
                            failed
                                .into_iter()
                                .map(|(p, why)| format!("{}: {why}", p.display())),
                        );
                    }
                    RollbackResult::RollbackFailed { failed } => {
                        problems.extend(
                            failed
                                .into_iter()
                                .map(|(p, why)| format!("{}: {why}", p.display())),
                        );
                    }
                },
                Err(error) => problems.push(error),
            }
        }
        self.library_changed = true;
        // An Undo that failed part-way leaves a record open: read it again.
        self.pending_folder_moves = None;
        self.undo_message = Some(if problems.is_empty() {
            format!(
                "Undo reversed {restored} move(s): every game, set and game folder is back where it was, with its previous system."
            )
        } else {
            format!(
                "Undo reversed {restored} move(s); some need History: {}",
                problems.join("; ")
            )
        });
    }
}

/// Applies every root's approved entries, then re-links RetroArch playlists
/// for what actually moved.
fn apply_plans(
    plans: &[RootPlan],
    folders: &FolderPreviews,
) -> (
    Report,
    Vec<archivefs_core::dat::rename_apply::RenameTransaction>,
    Vec<FolderMoveRecord>,
) {
    let mut report = Report::default();
    let mut transactions = Vec::new();
    let mut records = Vec::new();
    let summary = org::summarise(plans);
    report.already_correct = summary.already_correct;
    report.collisions = summary.collisions;
    report.left_alone = summary.left_alone + summary.blocked;
    let database = match archivefs_core::default_database_path()
        .and_then(archivefs_core::Database::open_read_only)
    {
        Ok(db) => db,
        Err(error) => {
            report
                .failed
                .push(format!("the library database could not be opened: {error}"));
            return (report, transactions, records);
        }
    };
    let journal = match archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
    {
        Ok(dir) => dir,
        Err(error) => {
            report
                .failed
                .push(format!("the journal folder is not available: {error}"));
            return (report, transactions, records);
        }
    };
    // Settle any folder move or Undo an earlier run left interrupted.
    settle_folder_moves(&journal, &mut report);
    let mut moves: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for plan in plans {
        let approved: BTreeSet<String> = plan
            .plan
            .suggested()
            .map(|e| e.source_path.to_string_lossy().into_owned())
            .collect();
        if approved.is_empty() {
            continue;
        }
        let platforms: BTreeMap<&Path, (&Option<String>, &str)> = plan
            .plan
            .entries
            .iter()
            .map(|e| {
                (
                    e.source_path.as_path(),
                    (&e.platform, e.platform_display_name.as_str()),
                )
            })
            .collect();
        let transaction = match org::apply(
            plan,
            &approved,
            &database,
            &journal,
            &AtomicBool::new(false),
        ) {
            Ok(outcome) => outcome.transaction,
            Err((error, transaction)) => {
                report
                    .failed
                    .push(format!("{}: {error}", plan.root.display()));
                match transaction {
                    Some(t) => *t,
                    None => continue,
                }
            }
        };
        report
            .created_folders
            .extend(transaction.created_directories.iter().cloned());
        for entry in &transaction.entries {
            if entry.state == EntryState::Applied
                && entry.unknown.contains_key("organisation_companion_v1")
            {
                report.set_members_moved += 1;
                moves.insert(entry.source_path.clone(), entry.destination_path.clone());
            } else if entry.state == EntryState::Applied {
                match platforms.get(entry.source_path.as_path()) {
                    Some((Some(_), name)) => {
                        report.organised += 1;
                        report.romm_platforms.insert((*name).to_string());
                    }
                    _ => report.to_unknown += 1,
                }
                moves.insert(entry.source_path.clone(), entry.destination_path.clone());
            } else if let Some(reason) = &entry.failure_reason {
                report
                    .failed
                    .push(format!("{}: {reason}", entry.source_path.display()));
            }
        }
        // Only a transaction that moved something has anything to undo.
        if transaction.applied_count() > 0 {
            report.transactions.push(transaction.transaction_id.clone());
            transactions.push(transaction);
        }
    }
    let database_path = database.path().to_path_buf();
    drop(database);
    for (candidate, preview) in folders {
        let FolderPreview::Move(plan) = preview else {
            continue;
        };
        match folder_move::apply(plan, &database_path, &journal) {
            Ok(record) => {
                report.organised += 1;
                report.folders_moved += 1;
                report.romm_platforms.insert(
                    archivefs_core::platform::display_name_for(&candidate.platform).to_string(),
                );
                report
                    .created_folders
                    .extend(record.created_dirs.iter().cloned());
                moves.insert(record.source.clone(), record.destination.clone());
                records.push(record);
            }
            Err(why) => report
                .failed
                .push(format!("{}: {why}", candidate.path.display())),
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        report.retroarch = relink::relink_playlists(&relink::playlist_dirs(&home), &moves);
    }
    (report, transactions, records)
}

/// Games the person assigned to a system that sit outside that system's
/// folder, so "Organise my games" can offer to move them too.
pub(super) fn chosen_games(games: &[super::library::Game]) -> Vec<ChosenGame> {
    games
        .iter()
        .filter(|g| g.archive.platform_source.as_deref() == Some("manual"))
        .filter_map(|g| {
            let platform = archivefs_core::canonical_platform_for_alias(&g.platform)?;
            let folder = g
                .archive
                .absolute_path
                .parent()?
                .file_name()?
                .to_string_lossy()
                .into_owned();
            let in_place = archivefs_core::platform::detect::platform_for_folder_name(&folder)
                .is_some_and(|p| p.id == platform);
            (!in_place).then(|| ChosenGame {
                archive_id: g.archive.id,
                path: g.archive.absolute_path.clone(),
                platform: platform.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_organising_is_off_until_switched_on() {
        assert!(!load_preferences(None).auto_organise_confirmed);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PREFERENCES_FILE);
        assert!(!load_preferences(Some(&path)).auto_organise_confirmed);
        save_preferences(
            &path,
            &Preferences {
                auto_organise_confirmed: true,
            },
        )
        .unwrap();
        assert!(load_preferences(Some(&path)).auto_organise_confirmed);
    }

    #[test]
    fn report_states_every_outcome_and_the_romm_next_step() {
        let report = Report {
            organised: 3,
            to_unknown: 1,
            already_correct: 2,
            collisions: 1,
            left_alone: 4,
            failed: vec!["x: busy".into()],
            romm_platforms: BTreeSet::from(["Nintendo Entertainment System".to_string()]),
            transactions: vec!["t".into()],
            ..Default::default()
        };
        let text = report.lines().join("\n");
        assert!(text.contains("Organised 3 games"));
        assert!(text.contains("1 moved to Unnamed/Unknown"));
        assert!(text.contains("Already correct: 2. Collisions left untouched: 1. Left for you to review: 4. Failed: 1."));
        assert!(text.contains("RomM: run a Quick scan of Nintendo Entertainment System"));
        assert!(text.contains("Undo"));
    }

    #[test]
    fn excluded_games_are_named_with_their_reason() {
        assert!(excluded_lines(&[]).is_empty());
        let one = excluded_lines(&[(
            PathBuf::from("/tmp/Game.zip"),
            "Outside every library source folder.".into(),
        )]);
        assert_eq!(
            one[0],
            "1 picked game is not included in this preview and will not be moved:"
        );
        assert_eq!(
            one[1],
            "Not moved: /tmp/Game.zip - Outside every library source folder."
        );
        let many: Vec<(PathBuf, String)> = (0..52)
            .map(|i| (PathBuf::from(format!("/x/{i}.zip")), "why".into()))
            .collect();
        let lines = excluded_lines(&many);
        assert!(lines[0].starts_with("52 picked games are"));
        assert_eq!(lines.len(), 1 + 50 + 1);
        assert_eq!(lines.last().unwrap(), "...and 2 more.");
    }
}
