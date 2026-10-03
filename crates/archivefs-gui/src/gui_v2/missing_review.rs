//! "Missing games" review: surfaces the backend's forget-confirmed-missing
//! preview/apply/undo. Eligibility is decided entirely by the core
//! classification; this file only presents it and relays the reviewed token.
use super::{App, Route, Section, backend::Command};
use crate::ui::theme;
use archivefs_core::Database;
use archivefs_core::catalogue_health::{
    ForgetMissingEntry, ForgetMissingPlan, ForgetMissingResult, MissingClassification,
};
use eframe::egui::{self, RichText};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct MissingReviewState {
    pub plan: Option<Box<ForgetMissingPlan>>,
    pub job: Option<u64>,
    pub confirm: bool,
    pub result: Option<String>,
    pub error: Option<String>,
    pub receipt: Option<PathBuf>,
}

/// Groups shown, in display order. Only the first may be acted on.
pub(super) const GROUPS: [MissingClassification; 5] = [
    MissingClassification::ConfirmedMissing,
    MissingClassification::PossiblyMoved,
    MissingClassification::SourceUnavailable,
    MissingClassification::ScanIncomplete,
    MissingClassification::ReviewRequired,
];

pub(super) fn group_copy(class: MissingClassification) -> (&'static str, &'static str) {
    match class {
        MissingClassification::ConfirmedMissing => (
            "Confirmed missing",
            "EmuWiz scanned this folder completely and the file is not there, and nothing looks like it was moved. Safe to remove from your catalogue.",
        ),
        MissingClassification::PossiblyMoved => (
            "Possibly moved",
            "A matching file exists elsewhere. Nothing is removed; rescan or update the game folder instead.",
        ),
        MissingClassification::SourceUnavailable => (
            "Folder or drive not reachable",
            "The folder these games live on can't be reached right now. Reconnect it; nothing is removed.",
        ),
        MissingClassification::ScanIncomplete => (
            "Last scan did not finish",
            "EmuWiz can't be sure these are gone. Run a full scan; nothing is removed.",
        ),
        _ => (
            "Needs review",
            "The evidence is not conclusive. Nothing is removed.",
        ),
    }
}

pub(super) fn entries_in(
    plan: &ForgetMissingPlan,
    class: MissingClassification,
) -> Vec<&ForgetMissingEntry> {
    plan.entries
        .iter()
        .filter(|e| e.classification == class)
        .collect()
}

pub(super) fn has_findings(plan: &ForgetMissingPlan) -> bool {
    plan.counts.total > 0
        && plan
            .entries
            .iter()
            .any(|e| e.classification != MissingClassification::NotMissing)
}

// ---- worker side (called from backend.rs) -------------------------------

fn roots() -> Vec<PathBuf> {
    archivefs_core::load_source_folder_configs_default()
        .map(|sources| sources.into_iter().map(|s| s.path).collect())
        .unwrap_or_default()
}

fn db_path() -> Result<PathBuf, String> {
    archivefs_core::default_database_path().map_err(|e| e.to_string())
}

pub(super) fn preview() -> Result<ForgetMissingPlan, String> {
    preview_at(&db_path()?, &roots())
}

fn preview_at(db: &Path, roots: &[PathBuf]) -> Result<ForgetMissingPlan, String> {
    Database::open_catalogue_health_read_only(db)
        .and_then(|database| database.preview_forget_confirmed_missing(roots))
        .map_err(|e| e.to_string())
}

/// Re-previews and applies only if the fresh plan is the one the user reviewed.
pub(super) fn apply(reviewed_token: &str) -> Result<ForgetMissingResult, String> {
    apply_at(&db_path()?, &roots(), reviewed_token)
}

fn apply_at(
    db: &Path,
    roots: &[PathBuf],
    reviewed_token: &str,
) -> Result<ForgetMissingResult, String> {
    let mut database = Database::open_or_create(db).map_err(|e| e.to_string())?;
    let plan = database
        .preview_forget_confirmed_missing(roots)
        .map_err(|e| e.to_string())?;
    if plan.token() != reviewed_token {
        return Err(format!(
            "{}: the library changed since you reviewed it. Nothing was removed. Review again.",
            archivefs_core::catalogue_health::FORGET_PLAN_STALE
        ));
    }
    database
        .apply_forget_confirmed_missing(&plan)
        .map_err(|e| e.to_string())
}

pub(super) fn undo(receipt: &Path) -> Result<usize, String> {
    let mut database = Database::open_or_create(&db_path()?).map_err(|e| e.to_string())?;
    database
        .undo_forget_missing(receipt)
        .map_err(|e| e.to_string())
}

// ---- UI side -------------------------------------------------------------

fn plural(n: usize) -> String {
    format!("{n} {}", if n == 1 { "entry" } else { "entries" })
}

impl App {
    pub(super) fn start_missing_preview(&mut self) {
        if self.missing.job.is_some() {
            return;
        }
        let id = self.activity.queue(
            "Checking which games are really missing",
            Route::Section(Section::Problems),
            false,
        );
        self.missing.job = Some(id);
        self.send(id, Command::MissingPreview);
    }

    fn start_missing_apply(&mut self, token: String) {
        if self.missing.job.is_some() {
            return;
        }
        let id = self.activity.queue(
            "Forgetting confirmed-missing catalogue entries",
            Route::Section(Section::Problems),
            false,
        );
        self.missing.job = Some(id);
        self.missing.confirm = false;
        self.send(id, Command::MissingApply { token });
    }

    fn start_missing_undo(&mut self, receipt: PathBuf) {
        if self.missing.job.is_some() {
            return;
        }
        let id = self.activity.queue(
            "Restoring forgotten catalogue entries",
            Route::Section(Section::Problems),
            false,
        );
        self.missing.job = Some(id);
        self.send(id, Command::MissingUndo { receipt });
    }

    /// Called from the event loop when a missing-review job fails.
    pub(super) fn missing_failed(&mut self, error: &str) {
        self.missing.job = None;
        self.missing.confirm = false;
        // A stale/failed apply must show a fresh preview, not the old one.
        self.missing.plan = None;
        self.missing.error = Some(error.to_string());
    }

    pub(super) fn missing_review_panel(&mut self, ui: &mut egui::Ui) {
        if self.missing.plan.is_none() && self.missing.job.is_none() && self.missing.error.is_none()
        {
            self.start_missing_preview();
        }
        if let Some(error) = self.missing.error.clone() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong("Missing games review");
                ui.colored_label(theme::WARNING, "The missing-game operation could not finish. Check the source drive and review Activity before trying again.");
                crate::ui::components::technical_details(ui, "missing_review_error", |ui| { ui.label(&error); });
                if ui.button("Review game folders").clicked() { self.go(Route::Section(Section::Sources)); }
                if ui.button("Open Activity details").clicked() { self.go(Route::Section(Section::Activity)); }
                if ui.add_enabled(self.missing.job.is_none(), egui::Button::new("Check again")).clicked() {
                    self.missing.error = None;
                    self.missing.plan = None;
                }
            });
        }
        if let Some(message) = self.missing.result.clone() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong("Missing games");
                ui.label(message);
                if let Some(receipt) = self.missing.receipt.clone() {
                    if ui
                        .add_enabled(
                            self.missing.job.is_none(),
                            egui::Button::new("Undo — restore these entries"),
                        )
                        .clicked()
                    {
                        self.start_missing_undo(receipt.clone());
                    }
                    ui.collapsing("Details", |ui| {
                        ui.monospace(format!("Undo receipt: {}", receipt.display()));
                    });
                }
            });
        }
        if self.missing.job.is_some()
            && (self.missing.receipt.is_some() || self.missing.plan.is_some())
        {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.small("Checking or updating the game list… No game files are deleted.");
            });
        }
        // The review appears once its evidence is available.
        let Some(plan) = self.missing.plan.clone() else {
            return;
        };
        if !has_findings(&plan) {
            return;
        }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Missing games");
            ui.label("EmuWiz checked every missing game against its folder's last scan. Only entries it can prove are gone can be removed. Your game files, artwork, saves and manuals are never touched.");
            for class in GROUPS {
                let entries = entries_in(&plan, class);
                if entries.is_empty() {
                    continue;
                }
                let (title, why) = group_copy(class);
                ui.add_space(6.0);
                egui::CollapsingHeader::new(RichText::new(format!("{title} ({})", entries.len())).strong())
                    .id_salt(("missing_group", title))
                    .default_open(class == MissingClassification::ConfirmedMissing)
                    .show(ui, |ui| {
                        ui.label(why);
                        for entry in &entries {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(&entry.display_name).strong());
                                if let Some(platform) = &entry.platform {
                                    ui.label(format!("· {platform}"));
                                }
                            });
                            ui.small(format!("Last known path: {}", entry.absolute_path.display()));
                            ui.collapsing(format!("Why — {}", entry.display_name), |ui| {
                                ui.label(&entry.reason);
                                if let Some(ev) = &entry.evidence {
                                    ui.monospace(format!(
                                        "scan #{} · source generation {} · recorded {}",
                                        ev.scan_run_id, ev.source_generation, ev.missing_recorded_at
                                    ));
                                }
                            });
                        }
                    });
            }
            let confirmed = plan.counts.confirmed_missing;
            if confirmed == 0 {
                ui.add_space(6.0);
                ui.label("Nothing can be removed safely right now.");
            } else {
                let removes = plan.confirmed().fold((0, 0, 0), |a, e| {
                    (
                        a.0 + e.removes.platform_assignments + e.removes.scan_observations,
                        a.1 + e.removes.dat_identities + e.removes.verified_identity_facts,
                        a.2 + e.removes.screenscraper_enrichments,
                    )
                });
                ui.add_space(6.0);
                ui.label(format!(
                    "Removing them also clears {} scan/platform record(s), {} identity record(s) and {} ScreenScraper record(s). You can undo this.",
                    removes.0, removes.1, removes.2
                ));
                ui.add_enabled_ui(self.missing.job.is_none(), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("Forget {confirmed} confirmed-missing games…")).strong(),
                            )
                            .fill(theme::PRIMARY_ACTION)
                            .min_size(egui::vec2(180.0, 44.0)),
                        )
                        .clicked()
                    {
                        self.missing.confirm = true;
                        self.missing.result = None;
                    }
                });
            }
        });
        if self.missing.confirm {
            let confirmed = plan.counts.confirmed_missing;
            egui::Window::new("Forget missing games?")
                .collapsible(false)
                .resizable(true)
                .default_width(420.0)
                .max_width((ui.ctx().content_rect().width() - 40.0).max(240.0))
                .vscroll(true)
                .show(ui.ctx(), |ui| {
                    ui.label(format!(
                        "Remove {} from EmuWiz's catalogue? No game files are deleted, and you can undo this.",
                        plural(confirmed)
                    ));
                    ui.label("The check is repeated just before anything changes. If the library changed since you reviewed it, nothing is removed.");
                    if ui.button("Cancel").clicked() {
                        self.missing.confirm = false;
                    }
                    if ui.button(format!("Forget {}", plural(confirmed))).clicked() {
                        let token = plan.token();
                        self.start_missing_apply(token);
                    }
                });
        }
    }

    pub(super) fn missing_apply_done(&mut self, result: ForgetMissingResult) {
        self.missing.job = None;
        self.missing.plan = None;
        self.missing.error = None;
        self.missing.result = Some(match &result.receipt_path {
            Some(_) => format!(
                "Forgot {}. No files were deleted.",
                plural(result.forgotten)
            ),
            None => "Nothing to forget.".into(),
        });
        self.missing.receipt = result.receipt_path;
        self.load(false);
    }

    pub(super) fn missing_undo_done(&mut self, restored: usize) {
        self.missing.job = None;
        self.missing.plan = None;
        self.missing.receipt = None;
        self.missing.result = Some(format!(
            "Restored {} to EmuWiz's game list. No game files were recreated or changed.",
            plural(restored)
        ));
        self.load(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::{Archive, Config, scan_and_persist};
    use std::fs;

    struct Fixture {
        temp: tempfile::TempDir,
        roots: Vec<PathBuf>,
        db: Database,
    }
    impl Fixture {
        fn db_path(&self) -> PathBuf {
            self.temp.path().join("library.sqlite3")
        }
        fn scan(&mut self) {
            scan_and_persist(
                &mut self.db,
                &Config {
                    source_folders: self.roots.clone(),
                    mount_root: self.temp.path().join("mounts"),
                    ratarmount_bin: "ratarmount".into(),
                    master_rom_root: None,
                },
                "test",
            )
            .unwrap();
        }
        fn count(&self) -> i64 {
            rusqlite::Connection::open(self.db_path())
                .unwrap()
                .query_row("SELECT count(*) FROM archives", [], |r| r.get(0))
                .unwrap()
        }
    }

    /// `gone.zip` was scanned then deleted (confirmed missing); `kept.zip` stays.
    fn fixture() -> (Fixture, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        fs::create_dir_all(&root).unwrap();
        let mut db = Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
        db.register_source_folders(std::slice::from_ref(&root))
            .unwrap();
        let sid = db.list_source_folders().unwrap()[0].id;
        let mut gone = PathBuf::new();
        for name in ["gone.zip", "kept.zip"] {
            let p = root.join(name);
            fs::write(&p, b"rom bytes").unwrap();
            let a = Archive::from_path_in_root(&p, &root).unwrap();
            db.upsert_archive(sid, &root, &a).unwrap();
            if name == "gone.zip" {
                gone = p;
            }
        }
        let mut f = Fixture {
            temp,
            roots: vec![root],
            db,
        };
        f.scan();
        fs::remove_file(&gone).unwrap();
        f.scan();
        (f, gone)
    }

    #[test]
    fn confirmed_group_holds_only_the_vanished_game() {
        let (f, _) = fixture();
        let plan = preview_at(&f.db_path(), &f.roots).unwrap();
        assert!(has_findings(&plan));
        let confirmed = entries_in(&plan, MissingClassification::ConfirmedMissing);
        assert_eq!(confirmed.len(), 1);
        assert!(confirmed[0].absolute_path.ends_with("gone.zip"));
        assert!(entries_in(&plan, MissingClassification::PossiblyMoved).is_empty());
    }

    #[test]
    fn unreachable_source_offers_nothing_to_forget() {
        let (f, _) = fixture();
        fs::rename(&f.roots[0], f.temp.path().join("offline")).unwrap();
        let plan = preview_at(&f.db_path(), &f.roots).unwrap();
        assert_eq!(plan.counts.confirmed_missing, 0);
        assert_eq!(
            entries_in(&plan, MissingClassification::SourceUnavailable).len(),
            2
        );
    }

    #[test]
    fn stale_token_removes_nothing() {
        let (mut f, gone) = fixture();
        let token = preview_at(&f.db_path(), &f.roots).unwrap().token();
        fs::write(&gone, b"rom bytes").unwrap(); // user restored the file
        f.scan();
        let error = apply_at(&f.db_path(), &f.roots, &token).unwrap_err();
        assert!(error.starts_with(archivefs_core::catalogue_health::FORGET_PLAN_STALE));
        assert_eq!(f.count(), 2);
    }

    #[test]
    fn apply_then_undo_round_trips_and_keeps_files() {
        let (f, _) = fixture();
        let token = preview_at(&f.db_path(), &f.roots).unwrap().token();
        let result = apply_at(&f.db_path(), &f.roots, &token).unwrap();
        assert_eq!(result.forgotten, 1);
        assert_eq!(f.count(), 1);
        assert!(f.roots[0].join("kept.zip").exists());
        let receipt = result.receipt_path.unwrap();
        // a second apply of the same token has nothing left to forget
        assert!(apply_at(&f.db_path(), &f.roots, &token).is_err());
        let mut db = Database::open_or_create(f.db_path()).unwrap();
        assert_eq!(db.undo_forget_missing(&receipt).unwrap(), 1);
        assert_eq!(f.count(), 2);
    }

    #[test]
    fn clean_library_has_no_findings() {
        let (mut f, gone) = fixture();
        fs::write(&gone, b"rom bytes").unwrap();
        f.scan();
        assert!(!has_findings(&preview_at(&f.db_path(), &f.roots).unwrap()));
    }
}
