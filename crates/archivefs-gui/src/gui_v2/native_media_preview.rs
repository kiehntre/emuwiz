//! Blue presentation of the existing scanner's bounded read-only projection.
//! Confirmation hands the sealed allowlist to the existing source-action worker.
use archivefs_core::ingestion::preview::{
    MediaDisposition, MediaPreview, load_spectrum_catalogues, preview_media,
};
use archivefs_core::{Database, SourceFolderView};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};

#[derive(Default)]
pub(super) struct State {
    root: String,
    job: Option<Receiver<Result<MediaPreview, String>>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<(usize, usize)>>,
    report: Option<MediaPreview>,
    error: Option<String>,
    generation: u64,
    database: PathBuf,
    sources: Vec<SourceFolderView>,
    reviewed_root: String,
    page: usize,
    filter: usize,
    result_page: usize,
    confirm_apply: bool,
    backup_acknowledged: bool,
    requested_apply: Option<archivefs_core::ingestion::bounded_import::BoundedImportPlan>,
    apply_cancel: Option<Arc<AtomicBool>>,
}

/// A nested folder may be inspected without changing source configuration.
/// The most specific configured source controls role/enablement. Refuse roots
/// containing configured child roots rather than crossing their ownership.
fn validate_root(root: &Path, sources: &[SourceFolderView]) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("Choose an absolute folder inside an enabled game source.".into());
    }
    let canonical = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    if canonical != root {
        return Err("Choose the canonical folder path; symlink/alias roots are not inspected by this preview.".into());
    }
    let source = sources
        .iter()
        .filter(|s| root.starts_with(&s.path))
        .max_by_key(|s| s.path.components().count())
        .ok_or("The folder is outside configured game sources.")?;
    if !source.enabled || !source.role.may_enter_game_scan() {
        return Err(
            "The selected source is disabled or excluded from game scanning by its saved role."
                .into(),
        );
    }
    if sources
        .iter()
        .any(|s| s.path != root && s.path.starts_with(root))
    {
        return Err("This folder contains configured child sources. Choose a narrower folder so their exclusions remain intact.".into());
    }
    Ok(())
}

fn run(
    root: PathBuf,
    database: PathBuf,
    sources: Vec<SourceFolderView>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<MediaPreview, String> {
    let config = archivefs_core::default_config_path().map_err(|e| e.to_string())?;
    run_at(root, database, sources, &config, cancel, progress)
}

fn run_at(
    root: PathBuf,
    database: PathBuf,
    sources: Vec<SourceFolderView>,
    config_path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<MediaPreview, String> {
    validate_root(&root, &sources)?;
    let review = super::ScanReview {
        action: crate::SourceAction::ScanOne(root.clone()),
        generation: 0,
        database_path: database.clone(),
        database_existed: true,
        sources: sources.clone(),
        known_systems: Default::default(),
    };
    review.validate_saved_at(config_path, &database)?;
    let db = Database::open_read_only(&database).map_err(|e| e.to_string())?;
    let config = archivefs_core::Config {
        source_folders: vec![root],
        mount_root: PathBuf::new(),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    };
    let import_context =
        archivefs_core::ingestion::bounded_import::begin_bounded_preview(&db, config_path)?;
    let known = db.load_archives().map_err(|e| e.to_string())?;
    // Missing catalogue configuration does not prevent structural inspection,
    // but comparison uncertainty must remain visible.
    let (catalogues, problem) = match load_spectrum_catalogues() {
        Ok(c) => (c, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    let mut report = preview_media(&config, &[], &known, &catalogues, cancel, progress)?;
    if let Some(problem) = problem {
        report
            .problems
            .push(format!("Catalogue comparison unavailable: {problem}"));
    }
    review.validate_saved_at(config_path, &database)?;
    match archivefs_core::ingestion::bounded_import::prepare_bounded_import(&report, import_context)
    {
        Ok(plan) => report.bounded_import = Some(plan),
        Err(reason) => report
            .problems
            .push(format!("Bounded Apply unavailable: {reason}")),
    }
    review.validate_saved_at(config_path, &database)?;
    Ok(report)
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(cancel) = &self.apply_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl State {
    pub(super) fn take_apply_request(
        &mut self,
    ) -> Option<(
        archivefs_core::ingestion::bounded_import::BoundedImportPlan,
        Arc<AtomicBool>,
    )> {
        let plan = self.requested_apply.take()?;
        let cancel = Arc::new(AtomicBool::new(false));
        self.apply_cancel = Some(cancel.clone());
        Some((plan, cancel))
    }
    pub(super) fn show_apply_result(&mut self, ui: &mut egui::Ui, details: &str) {
        ui.collapsing("Bounded import exclusions and skip reasons", |ui| {
            show_apply_details(ui, details, &mut self.result_page);
        });
    }
    fn current(&self, database: &Path, generation: u64, sources: &[SourceFolderView]) -> bool {
        self.generation == generation
            && self.database == database
            && self.sources == sources
            && self.reviewed_root == self.root
    }
    fn start(
        &mut self,
        ctx: egui::Context,
        database: &Path,
        generation: u64,
        sources: &[SourceFolderView],
    ) {
        self.generation = generation;
        self.database = database.to_path_buf();
        self.sources = sources.to_vec();
        self.reviewed_root = self.root.clone();
        self.report = None;
        self.confirm_apply = false;
        self.backup_acknowledged = false;
        self.error = None;
        self.page = 0;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.progress = Arc::new(Mutex::new((0, 0)));
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        let root = PathBuf::from(self.root.trim());
        let database = self.database.clone();
        let sources = self.sources.clone();
        let cancel = self.cancel.clone();
        let progress = self.progress.clone();
        std::thread::spawn(move || {
            let result = run(root, database, sources, &cancel, &mut |n, total| {
                if let Ok(mut p) = progress.lock() {
                    *p = (n, total);
                }
                ctx.request_repaint();
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let result = match job.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("Preview worker stopped. No library changes were made.".into())
            }
        };
        self.job = None;
        match result {
            Ok(report) if !report.cancelled && !self.cancel.load(Ordering::Relaxed) => {
                self.report = Some(report)
            }
            Ok(_) => {
                self.error = Some(
                    "Preview cancelled; partial results were discarded. Nothing was imported."
                        .into(),
                )
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        database: &Path,
        generation: u64,
        sources: &[SourceFolderView],
        library_busy: bool,
    ) {
        self.poll();
        if library_busy || !self.current(database, generation, sources) {
            self.report = None;
            self.confirm_apply = false;
            self.backup_acknowledged = false;
            if self.job.is_some() {
                self.cancel.store(true, Ordering::Relaxed);
            }
        }
        if !library_busy {
            self.apply_cancel = None;
        }
        if library_busy && let Some(cancel) = &self.apply_cancel {
            ui.label("Bounded Apply is running on the source worker. No root scan is running.");
            if ui.button("Cancel bounded Apply").clicked() {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        egui::CollapsingHeader::new("Read-only media preview").id_salt("blue_media_preview").show(ui, |ui| {
            ui.label("Mr Wiz · Choose one folder to inspect. Prepare is read-only and uses the existing scanner. Import requires the separate reviewed allowlist confirmation below. No automatic merge, DAT audit or ROM changes.");
            if self.root.is_empty() {
                self.root = sources.iter().find(|s| s.enabled && s.role.may_enter_game_scan())
                    .map(|s| s.path.display().to_string()).unwrap_or_default();
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Selected folder:");
                if ui.add_enabled(self.job.is_none(), egui::TextEdit::singleline(&mut self.root).desired_width(450.0)).changed() {
                    self.report = None; self.confirm_apply = false; self.backup_acknowledged = false;
                }
            });
            ui.label(format!("Library: {} | System comparison: ZX Spectrum. Other media retain their existing scanner classification.", database.display()));
            ui.small("Comparison input budget: 4 GiB plus 256 MiB decoded archive members. Scanner/parser reads are additional; charged inputs are estimates, not measured disk I/O. File relationships do not prove emulator usability. Different releases remain separate candidates.");
            if ui.add_enabled(!library_busy && self.job.is_none(), egui::Button::new("Prepare read-only media preview")).clicked() {
                self.start(ui.ctx().clone(), database, generation, sources);
            }
            if self.job.is_some() {
                let (n, total) = self.progress.lock().map(|p| *p).unwrap_or_default();
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    ui.label(if total == 0 { "Reading selected scope using the existing scanner…".into() }
                        else { format!("Comparing media {n}/{total}. Archive checks follow the loose-file pass.") });
                    if ui.button("Cancel preview").clicked() { self.cancel.store(true, Ordering::Relaxed); }
                });
                ui.small("Cancellation is checked during content comparison. The existing discovery walk and catalogue preparation must return before cancellation can finish.");
            }
            if let Some(error) = &self.error { ui.label(error); }
            if let Some(report) = &self.report {
                if let Some(plan) = &report.bounded_import {
                    ui.separator();
                    ui.label(format!("Bounded import scope: {} | ZX Spectrum | {} reviewed media candidates, at most {} insertions | {} explicit exclusions.", plan.root().display(), plan.candidate_count(), plan.projected_insertions(), plan.exclusions().len()));
                    ui.collapsing("Exact approval fingerprints",|ui| {
                        ui.label(format!("Allowlist SHA-256: {}",plan.approval_sha256()));
                        ui.label(format!("Exclusions SHA-256: {}",plan.exclusions_sha256()));
                    });
                    ui.small("Different releases, formats and parts remain separate records. No automatic grouping, identity merge, missing reconciliation or DAT history updates. New/unreviewed paths are never visited by Apply. Stale scope refuses the batch; changed or ambiguous files are skipped with reasons.");
                    if ui.add_enabled(!library_busy && self.job.is_none() && plan.projected_insertions()>0,egui::Button::new("Review bounded import…")).clicked() {
                        self.confirm_apply = true;
                        self.backup_acknowledged = false;
                    }

                }
                show_report(ui, report, &mut self.filter, &mut self.page);
            }
        });
        if self.confirm_apply
            && let Some(plan) = self.report.as_ref().and_then(|r| r.bounded_import.as_ref())
        {
            egui::Modal::new(egui::Id::new("blue_confirm_bounded_import")).show(ui.ctx(), |ui| {
                ui.set_max_width(680.0);
                ui.heading("Confirm database changes for this allowlist only");
                ui.label(format!("Root: {} | ZX Spectrum | at most {} new media rows | {} exclusions",plan.root().display(),plan.projected_insertions(),plan.exclusions().len()));
                ui.label(format!("Database: {}",plan.database().display()));
                ui.small(format!("Allowlist SHA-256: {}",plan.approval_sha256()));
                ui.small(format!("Exclusions SHA-256: {}",plan.exclusions_sha256()));
                ui.label("Releases, formats and parts stay separate. Apply revalidates this exact allowlist; it never scans the root. Review expires after 30 minutes.");
                ui.checkbox(&mut self.backup_acknowledged,"I have a consistent database backup and understand that committed imports have no automatic Undo.");
                ui.small("Cancellation/errors roll back this batch. After commit, recovery requires the pre-import database backup; restoring it also loses later database changes.");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Cancel import review").clicked() { self.confirm_apply = false; self.backup_acknowledged = false; }
                    if ui.add_enabled(self.backup_acknowledged && !library_busy, egui::Button::new("Confirm and apply reviewed allowlist")).clicked() { self.requested_apply = Some(plan.clone()); }
                });
            });
        }
        if self.requested_apply.is_some() {
            self.report = None;
            self.confirm_apply = false;
            self.backup_acknowledged = false;
        }
    }
}

fn show_apply_details(ui: &mut egui::Ui, details: &str, page: &mut usize) {
    let count = details.lines().count();
    *page = (*page).min(count.saturating_sub(1) / 50);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(*page > 0, egui::Button::new("Previous reasons"))
            .clicked()
        {
            *page -= 1;
        }
        if ui
            .add_enabled((*page + 1) * 50 < count, egui::Button::new("Next reasons"))
            .clicked()
        {
            *page += 1;
        }
        ui.label(format!(
            "{count} exclusion/skip details | page {}",
            *page + 1
        ));
    });
    egui::ScrollArea::vertical()
        .id_salt("blue_bounded_import_reasons")
        .max_height(240.0)
        .show(ui, |ui| {
            for reason in details.lines().skip(*page * 50).take(50) {
                ui.label(reason);
            }
        });
}

fn show_report(ui: &mut egui::Ui, report: &MediaPreview, filter: &mut usize, page: &mut usize) {
    if report.partial_coverage {
        ui.strong("Partial discovery: counts cover observed files only. Some paths could not be inspected; this is not proof that other media are absent.");
    }
    for root in &report.roots {
        ui.strong(format!("Preview scope: {}", root.display()));
    }
    ui.label(format!("{} eligible media candidates | {} exact archive representations | {} existing library paths | {} excluded ({} further exclusion details omitted)",report.count(MediaDisposition::Eligible),report.count(MediaDisposition::RepresentedArchive),report.count(MediaDisposition::Existing),report.count(MediaDisposition::Excluded)+report.omitted_exclusion_details,report.omitted_exclusion_details));
    ui.small("Candidate counts are not unique-game counts or a database diff. Bounded Apply, when available, consumes only the sealed candidate allowlist after separate confirmation. The full-source Scan is a different action and does not consume these exclusions.");
    ui.collapsing("Catalogue comparison sources", |ui| {
        for source in &report.catalogue_basis {
            ui.label(source);
        }
    });
    ui.collapsing(
        format!("Comparison problems ({})", report.problems.len()),
        |ui| {
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .show(ui, |ui| {
                    for problem in &report.problems {
                        ui.label(problem);
                    }
                });
        },
    );
    ui.label(format!("Charged comparison inputs: {} bytes. Archive/catalogue checks incomplete for {} new candidates.",report.charged_input_bytes,report.items.iter().filter(|i|i.disposition==MediaDisposition::Eligible&&!i.relationship_checks_complete).count()));
    let labels = [
        "All",
        "Eligible new media",
        "Already represented archive content",
        "Alternate releases / candidates",
        "Multipart media",
        "Unsupported / ambiguous",
    ];
    ui.horizontal_wrapped(|ui| {
        for (i, label) in labels.iter().enumerate() {
            if ui.selectable_label(*filter == i, *label).clicked() {
                *filter = i;
                *page = 0;
            }
        }
    });
    let rows: Vec<_> = report
        .items
        .iter()
        .filter(|i| match *filter {
            1 => i.disposition == MediaDisposition::Eligible,
            2 => i.disposition == MediaDisposition::RepresentedArchive,
            3 => i.alternate.is_some(),
            4 => i.multipart.is_some(),
            5 => {
                i.disposition == MediaDisposition::Excluded
                    || i.alternate
                        .as_deref()
                        .is_some_and(|s| s.starts_with("Ambiguous"))
            }
            _ => true,
        })
        .collect();
    *page = (*page).min(rows.len().saturating_sub(1) / 50);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(*page > 0, egui::Button::new("Previous"))
            .clicked()
        {
            *page -= 1;
        }
        if ui
            .add_enabled((*page + 1) * 50 < rows.len(), egui::Button::new("Next"))
            .clicked()
        {
            *page += 1;
        }
        ui.label(format!("{} matches | page {}", rows.len(), *page + 1));
    });
    egui::ScrollArea::vertical()
        .id_salt("blue_media_preview_rows")
        .max_height(260.0)
        .show(ui, |ui| {
            for row in rows.iter().skip(*page * 50).take(50) {
                ui.separator();
                ui.label(row.path.display().to_string());
                ui.label(&row.explanation);
                if let Some(alternate) = &row.alternate {
                    ui.label(alternate);
                }
                if let Some(part) = &row.multipart {
                    ui.label(format!(
                "Multipart media: {part}. Parts remain separate; no automatic grouping or merge."
            ));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_never_creates_a_missing_database() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("absent.sqlite3");
        let config = temp.path().join("config.toml");
        archivefs_core::save_source_folder_configs_to(
            &config,
            &[archivefs_core::SourceFolderConfig {
                path: temp.path().into(),
                enabled: true,
                created_at: None,
            }],
            &temp.path().join("mount"),
            "",
            None,
        )
        .unwrap();
        assert!(
            run_at(
                temp.path().into(),
                db.clone(),
                sources(temp.path()),
                &config,
                &AtomicBool::new(false),
                &mut |_, _| {}
            )
            .is_err()
        );
        assert!(!db.exists());
    }

    fn sources(root: &Path) -> Vec<SourceFolderView> {
        archivefs_core::build_source_folder_views(
            &[archivefs_core::SourceFolderConfig {
                path: root.to_path_buf(),
                enabled: true,
                created_at: None,
            }],
            &[],
        )
    }
    fn rendered(ctx: &egui::Context, show: impl FnMut(&egui::Context)) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 600.0),
                )),
                ..Default::default()
            },
            show,
        )
    }
    fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
        fn find(shape: &egui::Shape, needle: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(t) if t.galley.text().contains(needle) => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, needle)),
                _ => None,
            }
        }
        output.shapes.iter().find_map(|s| find(&s.shape, needle))
    }
    #[test]
    fn read_only_preview_scope_preserves_child_roles_and_disabled_sources() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("zxs");
        std::fs::create_dir(&child).unwrap();
        let mut views = sources(temp.path());
        assert!(validate_root(&child, &views).is_ok());
        views.extend(sources(&child));
        assert!(validate_root(temp.path(), &views).is_err());
        views[1].enabled = false;
        assert!(validate_root(&child, &views).is_err());
        assert!(validate_root(Path::new("relative"), &views).is_err());
    }
    #[test]
    fn read_only_preview_stale_generation_database_roots_and_cancelled_worker_are_discarded() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("db");
        let views = sources(temp.path());
        let mut state = State::default();
        state.database = db.clone();
        state.generation = 7;
        state.sources = views.clone();
        state.root = temp.path().display().to_string();
        state.reviewed_root = state.root.clone();
        assert!(state.current(&db, 7, &views));
        assert!(!state.current(&db, 8, &views));
        assert!(!state.current(&db.with_extension("other"), 7, &views));
        let (tx, rx) = mpsc::channel();
        state.job = Some(rx);
        state.cancel.store(true, Ordering::Relaxed);
        tx.send(Ok(MediaPreview::default())).unwrap();
        state.poll();
        assert!(state.report.is_none());
        assert!(state.error.as_ref().unwrap().contains("cancelled"));
        state.report = Some(MediaPreview::default());
        let ctx = egui::Context::default();
        let _ = rendered(&ctx, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui, &db, 8, &views, false));
        });
        assert!(state.report.is_none());
        assert!(!db.exists());
    }
    #[test]
    fn read_only_preview_controls_and_paging_fit_small_display_and_have_no_apply() {
        use archivefs_core::ingestion::preview::MediaPreviewItem;
        let mut report = MediaPreview::default();
        report.roots.push("/synthetic/zxs".into());
        report.problems = vec!["Comparison unavailable".into(); 100];
        for n in 0..125 {
            report.items.push(MediaPreviewItem {
                path: format!("/synthetic/zxs/unique{n:03}.z80").into(),
                disposition: MediaDisposition::Eligible,
                explanation:
                    "Structurally accepted; game identity and emulator usability are separate."
                        .into(),
                alternate: None,
                multipart: None,
                relationship_checks_complete: false,
            });
        }
        let ctx = egui::Context::default();
        let mut filter = 0;
        let mut page = 0;
        let output = rendered(&ctx, |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| show_report(ui, &report, &mut filter, &mut page));
        });
        assert!(text_rect(&output, "125 eligible").is_some());
        let next = text_rect(&output, "Next").unwrap();
        assert!(next.max.x < 1024.0 && next.max.y < 600.0);
        assert!(text_rect(&output, "Confirm scan").is_none());
        assert!(text_rect(&output, "unique050").is_none());
        page = 1;
        let output = rendered(&ctx, |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| show_report(ui, &report, &mut filter, &mut page));
        });
        assert!(text_rect(&output, "unique050").is_some());
        assert!(text_rect(&output, "unique000").is_none());
    }
    #[test]
    fn read_only_preview_filters_keep_relationships_separate_from_eligibility() {
        use archivefs_core::ingestion::preview::MediaPreviewItem;
        let mut report = MediaPreview::default();
        report.items = vec![
            MediaPreviewItem {
                path: "/fixture/alternate.sna".into(),
                disposition: MediaDisposition::Eligible,
                explanation: "Different bytes, retained separately".into(),
                alternate: Some("Same catalogue game; not interchangeable".into()),
                multipart: None,
                relationship_checks_complete: true,
            },
            MediaPreviewItem {
                path: "/fixture/side.z80".into(),
                disposition: MediaDisposition::Eligible,
                explanation: "Part of game".into(),
                alternate: None,
                multipart: Some("Part 1 of 2".into()),
                relationship_checks_complete: true,
            },
            MediaPreviewItem {
                path: "/fixture/bad.trd".into(),
                disposition: MediaDisposition::Excluded,
                explanation: "Invalid TR-DOS descriptor".into(),
                alternate: None,
                multipart: None,
                relationship_checks_complete: false,
            },
        ];
        let ctx = egui::Context::default();
        for (mut filter, expected, absent) in [
            (3, "alternate.sna", "side.z80"),
            (4, "side.z80", "alternate.sna"),
            (5, "bad.trd", "side.z80"),
        ] {
            let mut page = 0;
            let output = rendered(&ctx, |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| show_report(ui, &report, &mut filter, &mut page));
            });
            assert!(text_rect(&output, expected).is_some());
            assert!(text_rect(&output, absent).is_none());
        }
    }
    #[test]
    fn read_only_preview_worker_uses_disposable_database_without_writes() {
        assert!(
            archivefs_core::test_environment::isolated_root().is_some(),
            "default catalogue reads require private test HOME/XDG"
        );
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("zxs");
        std::fs::create_dir(&root).unwrap();
        let mut bytes = vec![0; 49182];
        bytes[7] = 0x80;
        bytes[9] = 0xc0;
        bytes[27] = 255;
        bytes[28] = 255;
        bytes[29] = 1;
        std::fs::write(root.join("fixture.z80"), &bytes).unwrap();
        let config = temp.path().join("config.toml");
        let database = temp.path().join("disposable.sqlite3");
        archivefs_core::save_source_folder_configs_to(
            &config,
            &[archivefs_core::SourceFolderConfig {
                path: root.clone(),
                enabled: true,
                created_at: None,
            }],
            &temp.path().join("mount"),
            "",
            None,
        )
        .unwrap();
        archivefs_core::Database::open_or_create(&database)
            .unwrap()
            .close()
            .unwrap();
        let views = archivefs_core::list_source_folder_views_at(&config, &database).unwrap();
        let before = std::fs::read(&database).unwrap();
        let result = run_at(
            root.clone(),
            database.clone(),
            views,
            &config,
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(result.count(MediaDisposition::Eligible), 1);
        assert_eq!(std::fs::read(&database).unwrap(), before);
        assert_eq!(std::fs::read(root.join("fixture.z80")).unwrap(), bytes);
    }
}

#[cfg(test)]
#[path = "native_media_apply_tests.rs"]
mod apply_tests;
