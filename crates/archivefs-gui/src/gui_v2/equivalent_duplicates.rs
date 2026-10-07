//! "Same game, different format": equivalent-content review (CUE/BIN ↔ CHD and
//! N64 byte-order variants). Equivalence is decided entirely by the core scan;
//! this file schedules it on the worker, presents the groups, and relays the
//! reviewed group to the journalled quarantine apply.
use super::{
    App, Route, Section,
    backend::{Command, DuplicateRepairRecord},
};
use crate::ui::theme;
use archivefs_core::repair::{
    N64EquivalentGroup, N64EquivalentScanReport, OpticalEquivalentGroup,
    OpticalEquivalentScanReport, apply_n64_equivalent_group, apply_optical_equivalent_group,
    scan_n64_equivalent_duplicates, scan_optical_equivalent_duplicates,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui::{self, RichText};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EquivalentKind {
    Optical,
    N64,
}

impl EquivalentKind {
    fn label(self) -> &'static str {
        match self {
            Self::Optical => "discs (CUE/BIN and CHD)",
            Self::N64 => "N64 games (z64 / v64 / n64)",
        }
    }
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Optical => &["cue", "chd"],
            Self::N64 => &["z64", "v64", "n64"],
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum EquivalentGroup {
    Optical(Box<OpticalEquivalentGroup>),
    N64(Box<N64EquivalentGroup>),
}

#[derive(Debug)]
pub(super) enum EquivalentScan {
    Optical(OpticalEquivalentScanReport),
    N64(N64EquivalentScanReport),
}

#[derive(Default)]
pub(super) struct EquivalentState {
    pub optical: Option<OpticalEquivalentScanReport>,
    pub n64: Option<N64EquivalentScanReport>,
    pub job: Option<u64>,
    pub selected: Option<(EquivalentKind, usize)>,
    pub confirm: bool,
    pub error: Option<String>,
    /// Plain result line and the index of the journalled record in `repair_history`.
    pub result: Option<(String, usize)>,
}

/// What one group looks like to a person; pure so it is testable without egui.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct GroupView {
    pub title: String,
    pub identity: String,
    pub preference_reason: &'static str,
    pub keep: Vec<String>,
    pub quarantine: Vec<String>,
    pub saves_bytes: u64,
    pub detail: Vec<String>,
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn stem(path: &Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

pub(super) fn optical_view(group: &OpticalEquivalentGroup) -> GroupView {
    let chd_bytes: u64 = group.chd.files.iter().map(|f| f.size_bytes).sum();
    let cue_bytes: u64 = group.cue_bin.files.iter().map(|f| f.size_bytes).sum();
    GroupView {
        title: stem(&group.preferred),
        identity: group.canonical_sha256.clone(),
        preference_reason: "The existing disc review prefers CHD after proving it matches the CUE/BIN contents.",
        keep: vec![format!("{} (recommended)", name(&group.preferred))],
        quarantine: group
            .quarantine_candidates
            .iter()
            .map(|p| name(p))
            .collect(),
        saves_bytes: group.projected_savings,
        detail: vec![
            format!(
                "CUE/BIN set: {} file(s), {}",
                group.cue_bin.files.len(),
                bytes(cue_bytes)
            ),
            format!(
                "CHD: {} file(s), {}",
                group.chd.files.len(),
                bytes(chd_bytes)
            ),
            format!("Matching disc fingerprint: {}", group.canonical_sha256),
        ],
    }
}

pub(super) fn n64_view(group: &N64EquivalentGroup) -> GroupView {
    GroupView {
        title: stem(&group.preferred),
        identity: group.canonical_sha256.clone(),
        preference_reason: "The existing N64 review prefers z64, then v64, then n64; equal formats use source-path order after content verification.",
        keep: vec![format!("{} (recommended)", name(&group.preferred))],
        quarantine: group
            .quarantine_candidates
            .iter()
            .map(|p| name(p))
            .collect(),
        saves_bytes: group.projected_savings,
        detail: group
            .members
            .iter()
            .map(|m| {
                format!(
                    "{} · {:?} byte order · {}",
                    m.path.display(),
                    m.byte_order,
                    bytes(m.size_bytes)
                )
            })
            .chain([format!(
                "Matching canonical content: {}",
                group.canonical_sha256
            )])
            .collect(),
    }
}

pub(super) fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[derive(Debug, PartialEq, Eq)]
enum GroupAction {
    Preview,
    Confirm,
    Cancel,
}

/// Rendering emits an intent only on a click; the existing controller handles it.
fn show_group(ui: &mut egui::Ui, view: &GroupView, selected: bool) -> Option<GroupAction> {
    let mut action = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(&view.title).strong());
                    ui.label("Verified matching contents. Review the preferred copy before any move; nothing is deleted automatically.");
                    for line in &view.keep { ui.colored_label(theme::SUCCESS, format!("Preferred copy: {line}")); }
                    ui.label(view.preference_reason);
                    ui.label(format!("Other copies: {}", view.quarantine.iter().take(super::library_reconciliation::PAGE_SIZE).cloned().collect::<Vec<_>>().join(", ")));
                    ui.collapsing("Details", |ui| {
                        for line in view.detail.iter().take(super::library_reconciliation::PAGE_SIZE) {
                            ui.small(line);
                        }
                    });
                    if selected {
                        ui.label("Preview reads only. Apply moves the reviewed redundant files into recoverable quarantine; the preferred copy stays unchanged. Undo available.");
                        for line in &view.keep {
                            ui.colored_label(theme::SUCCESS, format!("Keep: {line}"));
                        }
                        for line in view.quarantine.iter().take(super::library_reconciliation::PAGE_SIZE) {
                            ui.colored_label(theme::WARNING, format!("Move to quarantine: {line}"));
                        }
                        ui.label(format!("Redundant copies contain {}. Quarantine is recoverable; nothing is deleted and disk space is not freed.", bytes(view.saves_bytes)));
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add(egui::Button::new(RichText::new(format!("Move {} file(s) to quarantine…", view.quarantine.len())).strong()).fill(theme::PRIMARY_ACTION))
                                .clicked()
                            {
                                action = Some(GroupAction::Confirm);
                            }
                            if ui.button("Cancel").clicked() {
                                action = Some(GroupAction::Cancel);
                            }
                        });
                    } else if ui.button("Preview").clicked() {
                        action = Some(GroupAction::Preview);
                    }
                });
    action
}

// ---- worker side ---------------------------------------------------------

fn source_roots() -> Result<Vec<PathBuf>, String> {
    archivefs_core::Config::load_default()
        .map(|config| config.source_folders)
        .map_err(|error| error.to_string())
}

fn collect_files(root: &Path, extensions: &[&str], cancel: &AtomicBool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if entry.file_name() != archivefs_core::repair::QUARANTINE_DIRECTORY_NAME {
                    stack.push(path);
                }
            } else if kind.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

pub(super) fn scan(kind: EquivalentKind, cancel: &AtomicBool) -> Result<EquivalentScan, String> {
    scan_in(source_roots()?, kind, cancel)
}

fn scan_in(
    roots: Vec<PathBuf>,
    kind: EquivalentKind,
    cancel: &AtomicBool,
) -> Result<EquivalentScan, String> {
    if roots.is_empty() {
        return Err("No game folders are set up yet, so there is nothing to check.".into());
    }
    let candidates: Vec<PathBuf> = roots
        .iter()
        .flat_map(|root| collect_files(root, kind.extensions(), cancel))
        .collect();
    let trusted = TrustedRoots::from_paths(roots);
    Ok(match kind {
        EquivalentKind::Optical => EquivalentScan::Optical(scan_optical_equivalent_duplicates(
            &candidates,
            &trusted,
            Some(cancel),
        )),
        EquivalentKind::N64 => EquivalentScan::N64(scan_n64_equivalent_duplicates(
            &candidates,
            &trusted,
            Some(cancel),
        )),
    })
}

pub(super) fn apply(group: &EquivalentGroup) -> Result<DuplicateRepairRecord, String> {
    let journal_dir = archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
        .map_err(|error| error.to_string())?;
    apply_in(source_roots()?, journal_dir, group)
}

fn apply_in(
    roots: Vec<PathBuf>,
    journal_dir: PathBuf,
    group: &EquivalentGroup,
) -> Result<DuplicateRepairRecord, String> {
    let first = match group {
        EquivalentGroup::Optical(g) => g.quarantine_candidates.first(),
        EquivalentGroup::N64(g) => g.quarantine_candidates.first(),
    }
    .ok_or("This group has nothing to move.")?;
    let trusted_root = super::backend::trusted_root_for(first, &roots)?;
    std::fs::create_dir_all(&journal_dir)
        .map_err(|error| format!("Could not prepare repair history: {error}"))?;
    let trusted = TrustedRoots::from_paths(roots);
    let cancel = AtomicBool::new(false);
    let result = match group {
        EquivalentGroup::Optical(g) => {
            apply_optical_equivalent_group(g, &trusted_root, trusted, &journal_dir, &cancel)
        }
        EquivalentGroup::N64(g) => {
            apply_n64_equivalent_group(g, &trusted_root, trusted, &journal_dir, &cancel)
        }
    }
    .map_err(|error| format!("Nothing was moved: {error}"))?;
    Ok(DuplicateRepairRecord {
        transaction: result.transaction,
        trusted_root,
        journal_dir,
    })
}

// ---- UI side -------------------------------------------------------------

impl App {
    fn start_equivalent_scan(&mut self, kind: EquivalentKind) {
        if self.equiv.job.is_some() {
            return;
        }
        let id = self.activity.queue(
            match kind {
                EquivalentKind::Optical => "Checking for the same disc stored twice",
                EquivalentKind::N64 => "Checking for the same N64 game stored twice",
            },
            Route::Section(Section::Duplicates),
            true,
        );
        let cancel = self
            .activity
            .jobs
            .get(&id)
            .and_then(|job| job.cancel.clone())
            .unwrap_or_default();
        self.equiv.job = Some(id);
        self.equiv.error = None;
        self.equiv.result = None;
        self.equiv.selected = None;
        self.send(id, Command::EquivalentScan { kind, cancel });
    }

    fn start_equivalent_apply(&mut self, kind: EquivalentKind, index: usize) {
        if self.equiv.job.is_some() {
            return;
        }
        let group = match kind {
            EquivalentKind::Optical => self
                .equiv
                .optical
                .as_ref()
                .and_then(|r| r.groups.get(index))
                .map(|g| EquivalentGroup::Optical(Box::new(g.clone()))),
            EquivalentKind::N64 => self
                .equiv
                .n64
                .as_ref()
                .and_then(|r| r.groups.get(index))
                .map(|g| EquivalentGroup::N64(Box::new(g.clone()))),
        };
        let Some(group) = group else {
            self.equiv.error = Some("That group is no longer available. Check again.".into());
            return;
        };
        let id = self.activity.queue(
            "Moving the redundant copy to quarantine",
            Route::Section(Section::History),
            false,
        );
        self.equiv.job = Some(id);
        self.equiv.confirm = false;
        self.send(
            id,
            Command::EquivalentApply {
                group: Box::new(group),
            },
        );
    }

    pub(super) fn equivalent_scan_done(&mut self, scan: EquivalentScan) {
        self.equiv.job = None;
        self.equiv.selected = None;
        match scan {
            EquivalentScan::Optical(report) => self.equiv.optical = Some(report),
            EquivalentScan::N64(report) => self.equiv.n64 = Some(report),
        }
    }

    pub(super) fn equivalent_applied(&mut self, record: DuplicateRepairRecord) {
        self.equiv.job = None;
        let count = record.transaction.entries.len();
        self.repair_history.push(record);
        self.equiv.result = Some((
            format!("Moved {count} file(s) to EmuWiz quarantine. Nothing was deleted."),
            self.repair_history.len() - 1,
        ));
        // Both reports are now out of date; the user must check again.
        self.equiv.optical = None;
        self.equiv.n64 = None;
        self.equiv.selected = None;
        self.duplicate_report = None;
        self.problem_summary = None;
    }

    pub(super) fn equivalent_failed(&mut self, error: &str) {
        self.equiv.job = None;
        self.equiv.confirm = false;
        self.equiv.error = Some(
            if error.contains("StaleSource") || error.contains("changed") {
                format!(
                    "A file changed since the check, so nothing was moved. Check again. ({error})"
                )
            } else {
                error.to_string()
            },
        );
        self.equiv.optical = None;
        self.equiv.n64 = None;
        self.equiv.selected = None;
    }

    pub(super) fn equivalent_duplicates_section(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Same game, different format");
            ui.label("Finds the same game stored two ways, such as a CUE/BIN set and a CHD, or N64 files with different byte order. EmuWiz only offers a group when the contents are proven to match; nothing is deleted, and you can undo the move.");
            ui.horizontal_wrapped(|ui| {
                let idle = self.equiv.job.is_none();
                for kind in [EquivalentKind::Optical, EquivalentKind::N64] {
                    if ui
                        .add_enabled(idle, egui::Button::new(format!("Check {}", kind.label())))
                        .clicked()
                    {
                        self.start_equivalent_scan(kind);
                    }
                }
                if !idle {
                    ui.spinner();
                    ui.label("Checking… you can keep browsing.");
                }
            });
            if let Some(error) = self.equiv.error.clone() {
                ui.colored_label(theme::WARNING, "The duplicate check or move could not finish. Review History before retrying a move; run a fresh check if the source files changed.");
                crate::ui::components::technical_details(ui, "equivalent_error", |ui| { ui.label(error); });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Review source folders").clicked() { self.go(Route::Section(Section::Sources)); }
                    if ui.button("Review History & Undo").clicked() { self.go(Route::Section(Section::History)); }
                });
            }
            if let Some((message, index)) = self.equiv.result.clone() {
                ui.strong(message);
                ui.horizontal_wrapped(|ui| {
                    let applied = self.repair_history.get(index).is_some_and(|r| {
                        r.transaction.state
                            == archivefs_core::dat::rename_apply::model::TransactionState::Applied
                    });
                    if applied {
                        if ui.button("Undo — put the files back").clicked() {
                            self.undo_history_entry(index);
                        }
                    } else {
                        ui.label("Undone — the files are back where they were.");
                    }
                    if ui.button("Open History & Undo").clicked() {
                        self.go(Route::Section(Section::History));
                    }
                });
            }
            let optical = self.equiv.optical.as_ref().map(|r| {
                (
                    r.files_examined,
                    r.groups.iter().map(optical_view).collect::<Vec<_>>(),
                    r.excluded.iter().map(|e| (e.path.clone(), e.reason.clone())).collect::<Vec<_>>(),
                )
            });
            let n64 = self.equiv.n64.as_ref().map(|r| {
                (
                    r.files_examined,
                    r.groups.iter().map(n64_view).collect::<Vec<_>>(),
                    r.excluded.iter().map(|e| (e.path.clone(), e.reason.clone())).collect::<Vec<_>>(),
                )
            });
            if let Some((examined, views, excluded)) = optical {
                self.equivalent_results(ui, EquivalentKind::Optical, examined, &views, &excluded);
            }
            if let Some((examined, views, excluded)) = n64 {
                self.equivalent_results(ui, EquivalentKind::N64, examined, &views, &excluded);
            }
        });
        self.equivalent_confirm_window(ui);
    }

    fn equivalent_results(
        &mut self,
        ui: &mut egui::Ui,
        kind: EquivalentKind,
        examined: usize,
        views: &[GroupView],
        excluded: &[(PathBuf, String)],
    ) {
        ui.separator();
        ui.strong(format!("Results: {}", kind.label()));
        if views.is_empty() {
            ui.label(format!(
                "No equivalent copies found ({examined} file(s) examined)."
            ));
        } else {
            let total: u64 = views.iter().map(|v| v.saves_bytes).sum();
            ui.label(format!(
                "{examined} file(s) examined · {} group(s) · {} in redundant copies. Quarantine preserves these bytes; it does not free disk space.",
                views.len(),
                bytes(total)
            ));
        }
        for (index, view) in views
            .iter()
            .enumerate()
            .take(super::library_reconciliation::PAGE_SIZE)
        {
            let selected = self.equiv.selected == Some((kind, index));
            ui.push_id(
                ("equivalent-group", kind as u8, &view.identity),
                |ui| match show_group(ui, view, selected) {
                    Some(GroupAction::Preview) => {
                        self.equiv.selected = Some((kind, index));
                        self.equiv.confirm = false;
                    }
                    Some(GroupAction::Confirm) => self.equiv.confirm = true,
                    Some(GroupAction::Cancel) => self.equiv.selected = None,
                    None => {}
                },
            );
        }
        if views.len() > super::library_reconciliation::PAGE_SIZE {
            ui.label(format!(
                "Showing the first {} groups; totals include all groups.",
                super::library_reconciliation::PAGE_SIZE
            ));
        }
        if !excluded.is_empty() {
            ui.label(format!(
                "Needs review: {} candidates. No automatic choice or move is offered.",
                excluded.len()
            ));
            ui.collapsing(format!("Couldn't decide ({})", excluded.len()), |ui| {
                ui.label("EmuWiz could not prove these match, so no action is offered.");
                for (path, reason) in excluded
                    .iter()
                    .take(super::library_reconciliation::PAGE_SIZE)
                {
                    ui.small(format!("{} — {reason}", path.display()));
                }
            });
        }
    }

    fn equivalent_confirm_window(&mut self, ui: &mut egui::Ui) {
        if !self.equiv.confirm {
            return;
        }
        let Some((kind, index)) = self.equiv.selected else {
            self.equiv.confirm = false;
            return;
        };
        egui::Window::new("Move redundant copy to quarantine?")
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label("The redundant copy moves to EmuWiz's recoverable quarantine. The copy you keep is not changed, and you can undo this.");
                ui.label("Everything is re-checked immediately before anything moves. If a file changed since the check, nothing is moved.");
                if ui.button("Cancel").clicked() {
                    self.equiv.confirm = false;
                }
                if ui.button("Move to quarantine").clicked() {
                    self.start_equivalent_apply(kind, index);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A tiny valid-looking N64 image: Z64 magic then deterministic filler.
    fn z64() -> Vec<u8> {
        let mut data = vec![0x80, 0x37, 0x12, 0x40];
        data.extend((0..8192u32).map(|i| (i % 251) as u8));
        data
    }
    fn v64(z: &[u8]) -> Vec<u8> {
        z.chunks(2).flat_map(|p| [p[1], p[0]]).collect()
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("n64");
        fs::create_dir_all(&root).unwrap();
        let z = z64();
        fs::write(root.join("Game.z64"), &z).unwrap();
        fs::write(root.join("Game (swapped).v64"), v64(&z)).unwrap();
        fs::write(root.join("Other.z64"), [&z[..4], &[9u8; 4096]].concat()).unwrap();
        let journal = temp.path().join("journal");
        (temp, root, journal)
    }

    fn scan_n64(root: &Path) -> N64EquivalentScanReport {
        match scan_in(
            vec![root.to_path_buf()],
            EquivalentKind::N64,
            &AtomicBool::new(false),
        )
        .unwrap()
        {
            EquivalentScan::N64(report) => report,
            EquivalentScan::Optical(_) => unreachable!(),
        }
    }

    #[test]
    fn n64_variants_group_and_view_keeps_one_and_quarantines_one() {
        let (_t, root, _j) = fixture();
        let report = scan_n64(&root);
        assert_eq!(report.groups.len(), 1);
        let view = n64_view(&report.groups[0]);
        assert_eq!(view.keep.len(), 1);
        assert_eq!(view.quarantine.len(), 1);
        assert!(view.saves_bytes > 0);
        assert!(
            view.detail
                .iter()
                .any(|l| l.contains("Matching canonical content"))
        );
    }

    #[test]
    fn apply_quarantines_then_history_rollback_restores() {
        let (_t, root, journal) = fixture();
        let report = scan_n64(&root);
        let group = EquivalentGroup::N64(Box::new(report.groups[0].clone()));
        let before = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .count() as u64;
        let record = apply_in(vec![root.clone()], journal.clone(), &group).unwrap();
        let moved = report.groups[0].quarantine_candidates[0].clone();
        assert!(!moved.exists());
        assert!(report.groups[0].preferred.exists());
        assert!(root.join(".emuwiz-quarantine").exists());
        // The existing History/Undo path (rollback_quarantine_transaction) restores it.
        let mut tx = record.transaction;
        archivefs_core::repair::rollback_quarantine_transaction(
            &mut tx,
            &journal,
            &AtomicBool::new(false),
            &record.trusted_root,
        )
        .unwrap();
        assert!(moved.exists());
        // The core apply creates (and does not journal) its quarantine folder, so an
        // empty one can remain after undo; every game file is back.
        let games = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .count();
        assert_eq!(games as u64, before);
    }

    #[test]
    fn changed_file_after_scan_moves_nothing() {
        let (_t, root, journal) = fixture();
        let report = scan_n64(&root);
        let group = EquivalentGroup::N64(Box::new(report.groups[0].clone()));
        fs::write(
            &report.groups[0].quarantine_candidates[0],
            b"changed since the scan",
        )
        .unwrap();
        assert!(apply_in(vec![root.clone()], journal, &group).is_err());
        assert!(report.groups[0].quarantine_candidates[0].exists());
        assert!(!root.join(".emuwiz-quarantine").join("x").exists());
    }

    #[test]
    fn no_source_folders_is_an_explained_refusal() {
        let error = scan_in(vec![], EquivalentKind::Optical, &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("No game folders"));
    }

    #[test]
    fn quarantined_files_are_not_rescanned() {
        let (_t, root, journal) = fixture();
        let report = scan_n64(&root);
        let group = EquivalentGroup::N64(Box::new(report.groups[0].clone()));
        apply_in(vec![root.clone()], journal, &group).unwrap();
        assert!(scan_n64(&root).groups.is_empty());
    }

    #[test]
    fn byte_sizes_read_plainly() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
    }

    // ---- synthetic optical fixtures (no real media) ------------------------
    const RAW: usize = 2352;

    fn raw_sector(value: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; RAW];
        bytes[..12].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0]);
        bytes[15] = 1;
        bytes[16..16 + 2048].fill(value);
        bytes
    }

    /// Minimal MODE1_RAW CHD, same layout the core optical tests build.
    fn chd_for(sectors: &[Vec<u8>]) -> Vec<u8> {
        let unit = RAW as u32;
        let hunk = unit * sectors.len() as u32;
        let payload = format!(
            "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:{} PREGAP:0 PGTYPE:NONE PGSUB:NONE POSTGAP:0",
            sectors.len()
        );
        let meta_offset = 124u64;
        let map_offset = meta_offset + 16 + payload.len() as u64;
        let data_offset = (map_offset + 4).div_ceil(hunk as u64) * hunk as u64;
        let mut chd = vec![0u8; data_offset as usize];
        chd[..8].copy_from_slice(b"MComprHD");
        chd[8..12].copy_from_slice(&124u32.to_be_bytes());
        chd[12..16].copy_from_slice(&5u32.to_be_bytes());
        chd[32..40].copy_from_slice(&(hunk as u64).to_be_bytes());
        chd[40..48].copy_from_slice(&map_offset.to_be_bytes());
        chd[48..56].copy_from_slice(&meta_offset.to_be_bytes());
        chd[56..60].copy_from_slice(&hunk.to_be_bytes());
        chd[60..64].copy_from_slice(&unit.to_be_bytes());
        let p = meta_offset as usize;
        chd[p..p + 4].copy_from_slice(b"CHT2");
        chd[p + 5..p + 8].copy_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
        chd[p + 16..p + 16 + payload.len()].copy_from_slice(payload.as_bytes());
        chd[map_offset as usize..map_offset as usize + 4]
            .copy_from_slice(&(data_offset / hunk as u64).to_be_bytes()[4..]);
        for raw in sectors {
            chd.extend_from_slice(raw);
        }
        chd
    }

    fn write_disc(root: &Path, title: &str, bin_value: u8, chd_value: u8) {
        fs::write(root.join(format!("{title}.bin")), [bin_value; 2048 * 2]).unwrap();
        fs::write(
            root.join(format!("{title}.cue")),
            format!("FILE \"{title}.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n"),
        )
        .unwrap();
        fs::write(
            root.join(format!("{title}.chd")),
            chd_for(&[raw_sector(chd_value), raw_sector(chd_value)]),
        )
        .unwrap();
    }

    fn scan_optical(root: &Path) -> OpticalEquivalentScanReport {
        match scan_in(
            vec![root.to_path_buf()],
            EquivalentKind::Optical,
            &AtomicBool::new(false),
        )
        .unwrap()
        {
            EquivalentScan::Optical(report) => report,
            EquivalentScan::N64(_) => unreachable!(),
        }
    }

    #[test]
    fn matching_cue_bin_and_chd_apply_and_restore_while_a_mismatch_gets_no_group() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("psx");
        fs::create_dir_all(&root).unwrap();
        write_disc(&root, "Synthetic Match", 0x11, 0x11);
        write_disc(&root, "Synthetic Control", 0x22, 0x33); // different content
        let report = scan_optical(&root);
        assert_eq!(report.groups.len(), 1, "only the matching pair is a group");
        let group = &report.groups[0];
        let view = optical_view(group);
        assert_eq!(view.title, "Synthetic Match");
        assert_eq!(
            view.quarantine,
            ["Synthetic Match.cue", "Synthetic Match.bin"]
        );
        assert!(view.keep[0].starts_with("Synthetic Match.chd"));
        assert!(
            view.detail
                .iter()
                .any(|l| l.contains("CUE/BIN set: 2 file(s)"))
        );
        let journal = temp.path().join("journal");
        let record = apply_in(
            vec![root.clone()],
            journal.clone(),
            &EquivalentGroup::Optical(Box::new(group.clone())),
        )
        .unwrap();
        // CUE+BIN of the match moved together; keeper and the control untouched.
        for gone in ["Synthetic Match.cue", "Synthetic Match.bin"] {
            assert!(!root.join(gone).exists(), "{gone} should be quarantined");
        }
        for kept in [
            "Synthetic Match.chd",
            "Synthetic Control.cue",
            "Synthetic Control.bin",
            "Synthetic Control.chd",
        ] {
            assert!(root.join(kept).exists(), "{kept} must stay");
        }
        let mut tx = record.transaction;
        archivefs_core::repair::rollback_quarantine_transaction(
            &mut tx,
            &journal,
            &AtomicBool::new(false),
            &record.trusted_root,
        )
        .unwrap();
        for back in ["Synthetic Match.cue", "Synthetic Match.bin"] {
            assert!(root.join(back).exists());
        }
        assert_eq!(
            fs::read(root.join("Synthetic Match.bin")).unwrap(),
            [0x11u8; 4096]
        );
    }

    #[test]
    fn stale_optical_group_moves_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("psx");
        fs::create_dir_all(&root).unwrap();
        write_disc(&root, "Synthetic Match", 0x11, 0x11);
        let report = scan_optical(&root);
        let group = EquivalentGroup::Optical(Box::new(report.groups[0].clone()));
        fs::write(root.join("Synthetic Match.bin"), [0x99u8; 4096]).unwrap();
        assert!(apply_in(vec![root.clone()], temp.path().join("j"), &group).is_err());
        assert!(root.join("Synthetic Match.cue").exists());
        assert!(root.join("Synthetic Match.chd").exists());
    }
}

#[cfg(test)]
mod organisation_usability {
    use super::*;

    fn texts(shape: &egui::Shape, output: &mut String) {
        match shape {
            egui::Shape::Text(text) => {
                output.push_str(text.galley.text());
                output.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    texts(shape, output);
                }
            }
            _ => {}
        }
    }
    fn render_group(view: &GroupView, selected: bool) -> (String, Option<GroupAction>) {
        let ctx = egui::Context::default();
        let mut action = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.push_id(
                        (
                            "equivalent-group",
                            EquivalentKind::N64 as u8,
                            &view.identity,
                        ),
                        |ui| {
                            action = show_group(ui, view, selected);
                        },
                    );
                });
            },
        );
        let mut text = String::new();
        for shape in output.shapes {
            texts(&shape.shape, &mut text);
        }
        (text, action)
    }
    fn view() -> GroupView {
        GroupView {
            title: "Game".into(),
            identity: "canonical-fingerprint".into(),
            preference_reason: "Existing review prefers the verified z64 representation.",
            keep: vec!["Game.z64".into()],
            quarantine: vec!["Game.v64".into()],
            saves_bytes: 1234,
            detail: vec!["fingerprint evidence".into()],
        }
    }

    #[test]
    fn preferred_copy_and_existing_reason_visible_before_preview() {
        let (text, action) = render_group(&view(), false);
        assert!(text.contains("Preferred copy: Game.z64"));
        assert!(text.contains("prefers the verified z64"));
        assert!(text.contains("Other copies: Game.v64"));
        assert!(text.contains("Preview"));
        assert!(!text.contains("Move 1 file(s) to quarantine"));
        assert!(action.is_none());
    }
    #[test]
    fn quarantine_bytes_are_not_claimed_as_freed_disk_space() {
        let (text, _) = render_group(&view(), true);
        assert!(text.contains("disk space is not freed"));
        assert!(!text.contains("Frees about"));
        assert!(!text.contains("could be freed"));
    }
    #[test]
    fn painting_review_never_requests_a_destructive_action() {
        for selected in [false, true] {
            let (_, action) = render_group(&view(), selected);
            assert!(action.is_none());
        }
    }
    #[test]
    fn repeated_labels_use_content_identity_independent_of_position() {
        let view = view();
        let id = egui::Id::new((
            "equivalent-group",
            EquivalentKind::N64 as u8,
            &view.identity,
        ));
        let mut renamed = view;
        renamed.title = "Other display label".into();
        assert_eq!(
            id,
            egui::Id::new((
                "equivalent-group",
                EquivalentKind::N64 as u8,
                &renamed.identity
            ))
        );
        renamed.identity = "different-content".into();
        assert_ne!(
            id,
            egui::Id::new((
                "equivalent-group",
                EquivalentKind::N64 as u8,
                &renamed.identity
            ))
        );
    }
}
