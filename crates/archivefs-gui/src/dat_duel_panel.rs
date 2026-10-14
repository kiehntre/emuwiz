//! DAT Duel: a read-only comparison of the actual contents of two local DAT
//! files, drawn under DAT Sources.
//!
//! This is not the "Refresh impact" view, which compares inventories already
//! imported into the library database. Nothing here imports a DAT, picks an
//! authority, touches a ROM or writes a file: both DATs are parsed with the
//! existing parser and `DatLimits` by `archivefs_core::dat::duel`, off the GUI
//! thread, one comparison at a time.
//!
//! A running parse cannot be cancelled. Changing a selection therefore
//! *supersedes* the comparison: its result is discarded when it arrives and
//! a new comparison waits until the old worker has finished.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::SystemTime;

use archivefs_core::dat::duel::{
    Absence, AmbiguousGroup, Compatibility, Confidence, DuelOptions, DuelRefusal, DuelReport,
    NameRelation, OnlyEntry, Pairing, RomEvidence, compare, fingerprint_file,
};
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::parsers::parse_dat_file;
use eframe::egui;

use crate::ui::components::technical_details;

/// Rows drawn at once. Result lists can hold thousands of entries.
pub(crate) const PAGE_SIZE: usize = 25;
/// Upper bound on the copied troubleshooting report.
const COPY_BYTE_CAP: usize = 256 * 1024;
const COPY_ITEMS_START: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Renamed,
    SameNameConflict,
    CrossNameConflict,
    SameNameUnverified,
    Ambiguous,
    OnlyInA,
    OnlyInB,
}

impl Section {
    const ALL: [Section; 7] = [
        Section::Renamed,
        Section::SameNameConflict,
        Section::CrossNameConflict,
        Section::SameNameUnverified,
        Section::Ambiguous,
        Section::OnlyInA,
        Section::OnlyInB,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::Renamed => "Same content, different name",
            Section::SameNameConflict => "Same name, different checksums",
            Section::CrossNameConflict => "Shared checksum, contradicting data",
            Section::SameNameUnverified => "Same name, cannot be verified",
            Section::Ambiguous => "Ambiguous (several candidates)",
            Section::OnlyInA => "Only in DAT A",
            Section::OnlyInB => "Only in DAT B",
        }
    }

    /// The `truncated_sections` tag the engine uses for this list.
    fn engine_name(self) -> &'static str {
        match self {
            Section::Renamed => "renamed",
            Section::SameNameConflict => "same_name_conflicts",
            Section::CrossNameConflict => "cross_name_conflicts",
            Section::SameNameUnverified => "same_name_unverified",
            Section::Ambiguous => "ambiguous",
            Section::OnlyInA => "only_in_a",
            Section::OnlyInB => "only_in_b",
        }
    }
}

/// How much to trust a row. Ambiguous and conflicting rows are never
/// `Verified`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trust {
    Verified,
    Probable,
    Conflict,
    Ambiguous,
    Unverifiable,
    /// One-sided with a usable checksum and no counterpart.
    Unmatched,
}

impl Trust {
    fn label(self) -> &'static str {
        match self {
            Trust::Verified => "Verified (strong checksum match)",
            Trust::Probable => "Probable (CRC32 + size only)",
            Trust::Conflict => "Checksums contradict",
            Trust::Ambiguous => "Ambiguous - not verified",
            Trust::Unverifiable => "Unverifiable",
            Trust::Unmatched => "No counterpart",
        }
    }

    fn colour(self) -> egui::Color32 {
        match self {
            Trust::Verified => egui::Color32::from_rgb(60, 160, 90),
            Trust::Probable => egui::Color32::from_rgb(200, 150, 40),
            Trust::Conflict => egui::Color32::from_rgb(210, 80, 70),
            Trust::Ambiguous => egui::Color32::from_rgb(200, 110, 40),
            Trust::Unverifiable => egui::Color32::GRAY,
            Trust::Unmatched => egui::Color32::from_rgb(90, 140, 200),
        }
    }
}

/// One listed result, indexing into the report so the report is not cloned.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub(crate) section: Section,
    pub(crate) index: usize,
    pub(crate) trust: Trust,
    pub(crate) title: String,
    search: String,
}

/// Every field the expanded row displays, so search finds what the user sees.
fn search_text(e: &RomEvidence) -> String {
    let mut text = [
        Some(e.game.as_str()),
        Some(e.rom.as_str()),
        e.sha256.as_deref(),
        e.sha1.as_deref(),
        e.md5.as_deref(),
        e.crc32.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    if let Some(n) = e.size_bytes {
        text.push_str(&format!(" {n}"));
    }
    text
}

fn pairing_trust(p: &Pairing, section: Section) -> Trust {
    match section {
        Section::SameNameUnverified => Trust::Unverifiable,
        Section::SameNameConflict | Section::CrossNameConflict => Trust::Conflict,
        _ => match p.matched_by.as_ref().map(|m| m.confidence) {
            Some(Confidence::Verified) => Trust::Verified,
            Some(Confidence::Probable) => Trust::Probable,
            None => Trust::Unverifiable,
        },
    }
}

fn pair_title(p: &Pairing) -> String {
    if p.a.rom == p.b.rom && p.a.game == p.b.game {
        format!("{} / {}", p.a.game, p.a.rom)
    } else {
        format!("{} / {}  ↔  {} / {}", p.a.game, p.a.rom, p.b.game, p.b.rom)
    }
}

fn only_trust(e: &OnlyEntry) -> Trust {
    match e.absence {
        Absence::NoCounterpart => Trust::Unmatched,
        Absence::Unverifiable => Trust::Unverifiable,
    }
}

fn build_rows(report: &DuelReport) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut pairs = |section: Section, items: &[Pairing]| {
        for (index, p) in items.iter().enumerate() {
            let title = pair_title(p);
            let search =
                format!("{} {} {}", title, search_text(&p.a), search_text(&p.b)).to_lowercase();
            rows.push(Row {
                section,
                index,
                trust: pairing_trust(p, section),
                title,
                search,
            });
        }
    };
    pairs(Section::Renamed, &report.renamed);
    pairs(Section::SameNameConflict, &report.same_name_conflicts);
    pairs(Section::CrossNameConflict, &report.cross_name_conflicts);
    pairs(Section::SameNameUnverified, &report.same_name_unverified);
    for (index, g) in report.ambiguous.iter().enumerate() {
        let title = format!("{} entries in A  ↔  {} entries in B", g.a.len(), g.b.len());
        let names: String =
            g.a.iter()
                .chain(&g.b)
                .map(search_text)
                .collect::<Vec<_>>()
                .join(" ");
        rows.push(Row {
            section: Section::Ambiguous,
            index,
            trust: Trust::Ambiguous,
            search: format!("{title} {names}").to_lowercase(),
            title,
        });
    }
    for (section, items) in [
        (Section::OnlyInA, &report.only_in_a),
        (Section::OnlyInB, &report.only_in_b),
    ] {
        for (index, e) in items.iter().enumerate() {
            let title = format!("{} / {}", e.record.game, e.record.rom);
            rows.push(Row {
                section,
                index,
                trust: only_trust(e),
                search: format!("{title} {}", search_text(&e.record)).to_lowercase(),
                title,
            });
        }
    }
    rows
}

/// The size and modification time seen when a comparison started. A result is
/// only shown while both inputs still look exactly like this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    dev: u64,
    ino: u64,
    ctime: (i64, i64),
}

fn stamp(path: &Path) -> Result<FileStamp, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    Ok(FileStamp {
        len: meta.len(),
        modified: meta.modified().ok(),
        dev: meta.dev(),
        ino: meta.ino(),
        ctime: (meta.ctime(), meta.ctime_nsec()),
    })
}

pub(crate) enum WorkerResult {
    Report(Box<DuelReport>),
    Refused(DuelRefusal),
    Failed(String),
}

/// Parses and compares. The only filesystem access is reading the two DAT
/// files, exactly as `dat duel` does on the command line.
pub(crate) fn run_duel(a: &Path, b: &Path, allow_incompatible: bool) -> WorkerResult {
    let load = |label: &str, path: &Path| {
        let outcome = parse_dat_file(path, DatLimits::default())
            .map_err(|e| format!("DAT {label} ({}): {e}", path.display()))?;
        let fp =
            fingerprint_file(path).map_err(|e| format!("DAT {label} ({}): {e}", path.display()))?;
        Ok::<_, String>((outcome.dat, fp))
    };
    let (dat_a, fp_a) = match load("A", a) {
        Ok(v) => v,
        Err(e) => return WorkerResult::Failed(e),
    };
    let (dat_b, fp_b) = match load("B", b) {
        Ok(v) => v,
        Err(e) => return WorkerResult::Failed(e),
    };
    match compare(
        &dat_a,
        &dat_b,
        [Some(fp_a), Some(fp_b)],
        DuelOptions {
            allow_incompatible,
            ..DuelOptions::default()
        },
    ) {
        Ok(report) => WorkerResult::Report(Box::new(report)),
        Err(refusal) => WorkerResult::Refused(refusal),
    }
}

struct Job {
    token: u64,
    stamps: [FileStamp; 2],
    paths: [PathBuf; 2],
    allowed_incompatible: bool,
    rx: Receiver<Finished>,
}

/// A worker's answer plus the files' stamps taken right after it finished,
/// so completion never stats the filesystem on the GUI thread.
struct Finished {
    result: WorkerResult,
    after: Option<[FileStamp; 2]>,
}

pub(crate) struct Shown {
    pub(crate) report: DuelReport,
    pub(crate) rows: Vec<Row>,
    pub(crate) allowed_incompatible: bool,
}

pub(crate) enum Outcome {
    Report(Box<Shown>),
    Refused(DuelRefusal),
    Failed(String),
    /// A result that no longer describes what is on disk or selected.
    Stale(String),
}

#[derive(Default)]
pub(crate) struct DatDuelPanel {
    paths: [Option<PathBuf>; 2],
    /// Explicit acknowledgement required before `allow_incompatible` is sent.
    confirm_incompatible: bool,
    token: u64,
    job: Option<Job>,
    pub(crate) outcome: Option<Outcome>,
    section: Option<Section>,
    trust: Option<Trust>,
    filter: String,
    page: usize,
    copy_status: Option<String>,
    validation: Option<String>,
}

impl DatDuelPanel {
    pub(crate) fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    pub(crate) fn path(&self, side: usize) -> Option<&Path> {
        self.paths[side].as_deref()
    }

    /// Choosing a different file supersedes any comparison in flight and
    /// clears the shown result, the override acknowledgement and filters.
    pub(crate) fn select(&mut self, side: usize, path: PathBuf) {
        if self.paths[side].as_ref() == Some(&path) {
            return;
        }
        self.paths[side] = Some(path);
        self.supersede();
    }

    fn supersede(&mut self) {
        self.token = self.token.wrapping_add(1);
        self.outcome = None;
        self.confirm_incompatible = false;
        self.section = None;
        self.trust = None;
        self.filter.clear();
        self.page = 0;
        self.copy_status = None;
        self.validation = None;
    }

    /// Starts a comparison with the real engine on a worker thread.
    pub(crate) fn start(&mut self, ctx: &egui::Context) {
        let (Some(a), Some(b)) = (self.paths[0].clone(), self.paths[1].clone()) else {
            return;
        };
        let allow = self.confirm_incompatible;
        let ctx = ctx.clone();
        self.start_with(move || {
            let result = run_duel(&a, &b, allow);
            ctx.request_repaint();
            result
        });
    }

    /// `start` with an injectable worker, so tests control completion.
    pub(crate) fn start_with(&mut self, work: impl FnOnce() -> WorkerResult + Send + 'static) {
        if self.job.is_some() {
            return;
        }
        let (Some(a), Some(b)) = (self.paths[0].clone(), self.paths[1].clone()) else {
            return;
        };
        let same_file = a == b
            || matches!((stamp(&a), stamp(&b)), (Ok(x), Ok(y)) if x.dev == y.dev && x.ino == y.ino);
        if same_file {
            self.validation = Some(
                "Choose two different DAT files; a file compared with itself shows nothing.".into(),
            );
            return;
        }
        let stamps = match (stamp(&a), stamp(&b)) {
            (Ok(x), Ok(y)) => [x, y],
            (Err(e), _) | (_, Err(e)) => {
                self.validation = Some(format!("Cannot read DAT: {e}"));
                return;
            }
        };
        self.validation = None;
        self.outcome = None;
        let (tx, rx) = mpsc::channel();
        let watched = [a.clone(), b.clone()];
        std::thread::spawn(move || {
            let result = work();
            let after = match (stamp(&watched[0]), stamp(&watched[1])) {
                (Ok(x), Ok(y)) => Some([x, y]),
                _ => None,
            };
            let _ = tx.send(Finished { result, after });
        });
        self.job = Some(Job {
            token: self.token,
            stamps,
            paths: [a, b],
            allowed_incompatible: self.confirm_incompatible,
            rx,
        });
    }

    /// Drains a finished worker. Returns true when state changed.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(job) = &self.job else {
            return false;
        };
        let received = match job.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => None,
        };
        let job = self.job.take().expect("job checked above");
        if job.token != self.token {
            // Superseded: the selection changed while it ran. Nothing shown.
            return true;
        }
        self.outcome = Some(match received {
            None => Outcome::Failed("The comparison stopped unexpectedly. Try again.".into()),
            Some(Finished {
                result: WorkerResult::Failed(e),
                ..
            }) => Outcome::Failed(e),
            Some(Finished {
                result: WorkerResult::Refused(r),
                ..
            }) => Outcome::Refused(r),
            Some(Finished {
                result: WorkerResult::Report(report),
                after,
            }) => self.accept(job, *report, after),
        });
        self.page = 0;
        true
    }

    fn accept(&self, job: Job, report: DuelReport, after: Option<[FileStamp; 2]>) -> Outcome {
        for side in 0..2 {
            let still = self.paths[side].as_deref() == Some(job.paths[side].as_path())
                && after.as_ref().map(|a| &a[side]) == Some(&job.stamps[side]);
            let fingerprint_matches = report.sources[side]
                .file
                .as_ref()
                .is_none_or(|f| f.size_bytes == job.stamps[side].len);
            if !still || !fingerprint_matches {
                return Outcome::Stale(format!(
                    "DAT {} changed while it was being compared, so the result was discarded. Compare again.",
                    if side == 0 { "A" } else { "B" }
                ));
            }
        }
        Outcome::Report(Box::new(Shown {
            rows: build_rows(&report),
            report,
            allowed_incompatible: job.allowed_incompatible,
        }))
    }

    /// Rows passing the section, trust and text filters, in report order.
    pub(crate) fn visible_rows<'a>(&self, shown: &'a Shown) -> Vec<&'a Row> {
        let needle = self.filter.trim().to_lowercase();
        shown
            .rows
            .iter()
            .filter(|r| self.section.is_none_or(|s| s == r.section))
            .filter(|r| self.trust.is_none_or(|t| t == r.trust))
            .filter(|r| needle.is_empty() || r.search.contains(&needle))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn set_filters(
        &mut self,
        section: Option<Section>,
        trust: Option<Trust>,
        text: &str,
    ) {
        self.section = section;
        self.trust = trust;
        self.filter = text.to_string();
        self.page = 0;
    }

    #[cfg(test)]
    pub(crate) fn set_confirm_incompatible(&mut self, value: bool) {
        self.confirm_incompatible = value;
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        if self.poll() {
            ui.ctx().request_repaint();
        }
        ui.label(
            "Read-only. Compares what is inside two DAT catalogue files. Your ROMs, your library and \
             your DAT settings are never changed, and no DAT is chosen for you.",
        );
        ui.label(
            "This is different from \"Refresh impact\" above, which compares inventories already \
             imported into the library.",
        );
        let mut chosen = None;
        for (side, name) in ["DAT A", "DAT B"].into_iter().enumerate() {
            ui.horizontal(|ui| {
                let label = format!("Choose {name}…");
                if ui
                    .add_enabled(!self.is_busy(), egui::Button::new(label))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title(format!("Choose {name} to compare"))
                        .add_filter("DAT catalogues", &["dat", "xml"])
                        .pick_file()
                {
                    chosen = Some((side, path));
                }
                match self.path(side) {
                    Some(p) => ui.label(p.display().to_string()),
                    None => ui.weak("not chosen"),
                };
            });
        }
        if let Some((side, path)) = chosen {
            self.select(side, path);
        }
        let ready = self.paths[0].is_some() && self.paths[1].is_some();
        ui.horizontal(|ui| {
            let label = if self.confirm_incompatible {
                "Compare (hash evidence only)"
            } else {
                "Compare"
            };
            if ui
                .add_enabled(ready && !self.is_busy(), egui::Button::new(label))
                .clicked()
            {
                self.start(ui.ctx());
            }
            if self.is_busy() {
                ui.spinner();
                ui.label(
                    "Comparing… large catalogues can take a while. You can keep using EmuWiz.",
                );
            }
        });
        if let Some(message) = &self.validation {
            ui.colored_label(egui::Color32::from_rgb(210, 80, 70), message);
        }
        if self.is_busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }

        let refusal = match &self.outcome {
            Some(Outcome::Refused(r)) => Some(r.clone()),
            _ => None,
        };
        match &self.outcome {
            None => {}
            Some(Outcome::Failed(e)) => {
                ui.colored_label(
                    egui::Color32::from_rgb(210, 80, 70),
                    format!("Comparison failed: {e}"),
                );
            }
            Some(Outcome::Stale(e)) => {
                ui.colored_label(egui::Color32::from_rgb(200, 150, 40), e);
            }
            Some(Outcome::Refused(_)) => {
                let detail = describe_refusal(&refusal.expect("refusal cloned above"));
                ui.colored_label(
                    egui::Color32::from_rgb(200, 150, 40),
                    "These catalogues are not directly comparable",
                );
                ui.label(detail);
                ui.label(
                    "Different systems use different identities, so name differences would be \
                     meaningless. You can still look at checksum evidence only; nothing is changed either way.",
                );
                let mut confirm = self.confirm_incompatible;
                if ui
                    .checkbox(
                        &mut confirm,
                        "I understand these may be different catalogues; show hash evidence only",
                    )
                    .changed()
                {
                    self.confirm_incompatible = confirm;
                }
                ui.label("Tick the box, then press \"Compare (hash evidence only)\".");
            }
            Some(Outcome::Report(_)) => self.show_report(ui),
        }
    }

    fn show_report(&mut self, ui: &mut egui::Ui) {
        // Taken out so `self` can be mutated by the filter widgets.
        let Some(Outcome::Report(shown)) = self.outcome.take() else {
            return;
        };
        ui.separator();
        show_headline(ui, &shown);
        show_sources(ui, &shown.report);
        show_summary(ui, &shown.report);

        ui.horizontal_wrapped(|ui| {
            ui.label("Show:");
            if ui.selectable_label(self.section.is_none(), "All").clicked() {
                self.section = None;
                self.page = 0;
            }
            for s in Section::ALL {
                let n = shown.rows.iter().filter(|r| r.section == s).count();
                if ui
                    .selectable_label(self.section == Some(s), format!("{} ({n})", s.label()))
                    .clicked()
                {
                    self.section = Some(s);
                    self.page = 0;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Confidence:");
            for (label, value) in [
                ("Any", None),
                ("Verified", Some(Trust::Verified)),
                ("Probable", Some(Trust::Probable)),
                ("Unverifiable", Some(Trust::Unverifiable)),
                ("Conflict", Some(Trust::Conflict)),
                ("Ambiguous", Some(Trust::Ambiguous)),
                ("No counterpart", Some(Trust::Unmatched)),
            ] {
                if ui.selectable_label(self.trust == value, label).clicked() {
                    self.trust = value;
                    self.page = 0;
                }
            }
            ui.label("Search:");
            if ui.text_edit_singleline(&mut self.filter).changed() {
                self.page = 0;
            }
        });

        let rows = self.visible_rows(&shown);
        let pages = rows.len().div_ceil(PAGE_SIZE).max(1);
        self.page = self.page.min(pages - 1);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.page > 0, egui::Button::new("Previous"))
                .clicked()
            {
                self.page -= 1;
            }
            ui.label(format!(
                "Page {} of {pages} • {} matching entries",
                self.page + 1,
                rows.len()
            ));
            if ui
                .add_enabled(self.page + 1 < pages, egui::Button::new("Next"))
                .clicked()
            {
                self.page += 1;
            }
        });
        if rows.is_empty() {
            ui.label(if shown.rows.is_empty() {
                "No differences to list."
            } else {
                "No entries match the current filters."
            });
        }
        let page_rows: Vec<Row> = rows
            .into_iter()
            .skip(self.page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .cloned()
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("dat-duel-results")
            .max_height(360.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for row in &page_rows {
                    show_row(ui, &shown.report, row);
                }
            });

        ui.horizontal(|ui| {
            if ui.button("Copy JSON report").clicked() {
                match bounded_json(&shown.report) {
                    Ok((text, note)) => {
                        ui.ctx().copy_text(text);
                        self.copy_status = Some(note);
                    }
                    Err(e) => self.copy_status = Some(format!("Could not build the report: {e}")),
                }
            }
            if let Some(s) = &self.copy_status {
                ui.label(s);
            }
        });
        technical_details(ui, "dat-duel-hashes-note", |ui| {
            ui.label("Verified = SHA-256, SHA-1 or MD5 (a strong checksum) agrees. Probable = only CRC32 and size agree, which can collide. \
                      Unverifiable = a side has no checksum to compare, so a difference is not proven.");
        });
        self.outcome = Some(Outcome::Report(shown));
    }
}

fn describe_refusal(r: &DuelRefusal) -> String {
    let why = match r.compatibility {
        Compatibility::IncompatibleEcosystem => "The two DATs come from different DAT families.",
        Compatibility::IncompatibleCatalogue => "The two DATs describe different platforms.",
        Compatibility::UnverifiedNamespace => {
            "At least one DAT has no catalogue name, so EmuWiz cannot tell what it describes."
        }
        _ => "The catalogues are not marked as compatible.",
    };
    format!("{why} ({})", r.detail)
}

fn show_headline(ui: &mut egui::Ui, shown: &Shown) {
    let r = &shown.report;
    let (text, colour) = match r.compatibility {
        Compatibility::Compatible => (
            "Same catalogue name - names can be compared.",
            egui::Color32::from_rgb(60, 160, 90),
        ),
        Compatibility::VariantDiffers => (
            "Same platform, different variants (for example headered vs headerless). Checksum differences are expected.",
            egui::Color32::from_rgb(200, 150, 40),
        ),
        _ => (
            "Catalogue identity is missing or incompatible. Showing hash evidence only; name differences are not meaningful.",
            egui::Color32::from_rgb(210, 80, 70),
        ),
    };
    ui.colored_label(colour, text);
    ui.label(
        "This is a snapshot of the two files as they were when the comparison ran (file SHA-256 \
         under each DAT's technical details). It is not live verification: if either file changes, \
         compare again.",
    );
    if shown.allowed_incompatible
        && !matches!(
            r.compatibility,
            Compatibility::Compatible | Compatibility::VariantDiffers
        )
    {
        ui.colored_label(
            egui::Color32::from_rgb(210, 80, 70),
            "Override confirmed by you: these catalogues were not confirmed as comparable.",
        );
    }
    if !r.naming_comparable {
        ui.label("Name comparisons are not shown as differences because the namespaces differ.");
    }
    if !r.truncated_sections.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(200, 150, 40),
            format!(
                "Incomplete listing: {} cut at {} entries each. Totals in the summary are complete.",
                r.truncated_sections.join(", "),
                archivefs_core::dat::duel::DEFAULT_MAX_ITEMS_PER_SECTION
            ),
        );
    }
    if r.summary.candidate_buckets_truncated > 0 {
        ui.colored_label(
            egui::Color32::from_rgb(200, 150, 40),
            format!(
                "{} very common checksums were only partly examined; some matches may be missing.",
                r.summary.candidate_buckets_truncated
            ),
        );
    }
    if r.sources.iter().all(|s| s.records_compared == 0) {
        ui.label("Both catalogues are empty: there is nothing to compare.");
    } else if shown.rows.is_empty()
        && r.truncated_sections.is_empty()
        && r.summary.candidate_buckets_truncated == 0
    {
        ui.label("No differences were found among the entries that could be compared.");
    }
}

fn show_sources(ui: &mut egui::Ui, r: &DuelReport) {
    for (side, name) in ["DAT A", "DAT B"].into_iter().enumerate() {
        let s = &r.sources[side];
        let title = s.name.as_deref().unwrap_or("(no catalogue name in header)");
        ui.label(format!(
            "{name}: {title} • {} • version {} • {} entries compared",
            s.ecosystem_label,
            s.version.as_deref().unwrap_or("not recorded"),
            s.records_compared
        ));
        technical_details(ui, ("dat-duel-source", side), |ui| {
            ui.label(format!("File: {}", s.file_path));
            if let Some(f) = &s.file {
                ui.label(format!(
                    "File SHA-256: {} ({} bytes)",
                    f.sha256, f.size_bytes
                ));
            }
            ui.label(format!(
                "Parse warnings: {} • invalid checksum fields ignored: {}",
                s.parse_warning_count, s.invalid_checksum_fields
            ));
            if let Some(a) = &s.author {
                ui.label(format!("Author: {a}"));
            }
        });
    }
}

fn show_summary(ui: &mut egui::Ui, r: &DuelReport) {
    let s = &r.summary;
    ui.label(format!(
        "Identical entries: {} verified, {} probable (same name and content - counted, not listed)",
        s.verified_same_name, s.probable_same_name
    ));
    ui.label(format!(
        "Same content, different name: {} verified, {} probable • Ambiguous: {} • Conflicts: {} same-name, {} cross-name",
        s.renamed_verified, s.renamed_probable, s.ambiguous_groups, s.same_name_conflicts, s.cross_name_conflicts
    ));
    ui.label(format!(
        "Only in A: {} confirmed, {} unverifiable • Only in B: {} confirmed, {} unverifiable",
        s.only_in_a_no_counterpart,
        s.only_in_a_unverifiable,
        s.only_in_b_no_counterpart,
        s.only_in_b_unverifiable
    ));
}

fn evidence_lines(ui: &mut egui::Ui, side: &str, e: &RomEvidence) {
    ui.label(format!(
        "{side}: {} / {} ({})",
        e.game,
        e.rom,
        e.size_bytes
            .map(|n| format!("{n} bytes"))
            .unwrap_or_else(|| "size not recorded".into())
    ));
    let mut hashes = Vec::new();
    for (k, v) in [
        ("SHA-256", &e.sha256),
        ("SHA-1", &e.sha1),
        ("MD5", &e.md5),
        ("CRC32", &e.crc32),
    ] {
        if let Some(v) = v {
            hashes.push(format!("{k} {v}"));
        }
    }
    ui.label(if hashes.is_empty() {
        "no checksums recorded".to_string()
    } else {
        hashes.join("  •  ")
    });
    if e.bios {
        ui.label("Marked as BIOS");
    }
}

fn group_lines(ui: &mut egui::Ui, side: &str, items: &[RomEvidence]) {
    for e in items {
        evidence_lines(ui, side, e);
    }
}

fn relation_text(r: NameRelation) -> &'static str {
    match r {
        NameRelation::Identical => "same names",
        NameRelation::CaseOnly => "names differ only by letter case",
        NameRelation::SameGameDifferentRom => "same game, different ROM file name",
        NameRelation::DifferentGameSameRom => "different game name, same ROM file name",
        NameRelation::Different => "different names",
    }
}

fn show_row(ui: &mut egui::Ui, report: &DuelReport, row: &Row) {
    let header = format!("[{}] {}", row.trust.label(), row.title);
    egui::CollapsingHeader::new(egui::RichText::new(header).color(row.trust.colour()))
        .id_salt(("dat-duel-row", row.section.engine_name(), row.index))
        .show(ui, |ui| {
            let pair = |ui: &mut egui::Ui, p: &Pairing| {
                ui.label(format!("Names: {}", relation_text(p.name_relation)));
                if let Some(m) = &p.matched_by {
                    ui.label(format!(
                        "Matched by {} ({})",
                        m.basis,
                        match m.confidence {
                            Confidence::Verified => "verified",
                            Confidence::Probable => "probable only",
                        }
                    ));
                }
                for c in &p.contradictions {
                    ui.label(format!("Contradiction in {}: A {} vs B {}", c.field, c.a, c.b));
                }
                if p.bios_differs {
                    ui.label("BIOS flag differs between the catalogues.");
                }
                evidence_lines(ui, "A", &p.a);
                evidence_lines(ui, "B", &p.b);
            };
            match row.section {
                Section::Renamed => pair(ui, &report.renamed[row.index]),
                Section::SameNameConflict => pair(ui, &report.same_name_conflicts[row.index]),
                Section::CrossNameConflict => pair(ui, &report.cross_name_conflicts[row.index]),
                Section::SameNameUnverified => pair(ui, &report.same_name_unverified[row.index]),
                Section::Ambiguous => {
                    let g: &AmbiguousGroup = &report.ambiguous[row.index];
                    ui.label("Several entries could match each other, so no single pairing is claimed.");
                    if g.members_truncated {
                        ui.label("Group members were capped; more entries are involved.");
                    }
                    group_lines(ui, "A", &g.a);
                    group_lines(ui, "B", &g.b);
                }
                Section::OnlyInA | Section::OnlyInB => {
                    let (e, side) = if row.section == Section::OnlyInA {
                        (&report.only_in_a[row.index], "A")
                    } else {
                        (&report.only_in_b[row.index], "B")
                    };
                    ui.label(match e.absence {
                        Absence::NoCounterpart => "The other catalogue has no entry with this checksum or name.",
                        Absence::Unverifiable => {
                            "The other catalogue has no checksum of this kind, so absence is not proven."
                        }
                    });
                    evidence_lines(ui, side, &e.record);
                }
            }
        });
}

/// Serialises the report for troubleshooting, bounded in item count and
/// bytes. Returns the text and a one-line description of what was cut.
pub(crate) fn bounded_json(report: &DuelReport) -> Result<(String, String), String> {
    let value = serde_json::to_value(report).map_err(|e| e.to_string())?;
    let engine_cut = if report.truncated_sections.is_empty() {
        String::new()
    } else {
        format!(
            " The comparison itself was already incomplete ({} cut).",
            report.truncated_sections.join(", ")
        )
    };
    let mut limit = COPY_ITEMS_START;
    loop {
        let mut v = value.clone();
        let mut cut = Vec::new();
        if let Some(obj) = v.as_object_mut() {
            for (key, val) in obj.iter_mut() {
                if let Some(arr) = val.as_array_mut()
                    && arr.len() > limit
                {
                    cut.push(format!("{key}: {} of {}", limit, arr.len()));
                    arr.truncate(limit);
                }
            }
            obj.insert(
                "copy_bounds".into(),
                serde_json::json!({ "max_items_per_section": limit, "sections_cut": cut }),
            );
        }
        let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        if text.len() <= COPY_BYTE_CAP {
            let note = if cut.is_empty() {
                format!("Copied every entry the comparison produced.{engine_cut}")
            } else {
                format!("Copied a bounded report ({}).{engine_cut}", cut.join(", "))
            };
            return Ok((text, note));
        }
        if limit <= 1 {
            // Nested lists can still be too large: fall back to counts only.
            let minimal = serde_json::json!({
                "schema_version": report.schema_version,
                "read_only": report.read_only,
                "compatibility": value["compatibility"],
                "sources": value["sources"],
                "summary": value["summary"],
                "truncated_sections": value["truncated_sections"],
                "copy_bounds": { "summary_only": true },
            });
            let text = serde_json::to_string_pretty(&minimal).map_err(|e| e.to_string())?;
            return Ok((
                text,
                format!("Copied a summary only; the entry lists were too large.{engine_cut}"),
            ));
        }
        limit = (limit / 2).max(1);
    }
}

/// The disclosure title. Kept near the top of DAT Sources, collapsed, so it is
/// discoverable without scrolling past the whole catalogue page.
pub(crate) const SECTION_TITLE: &str = "Compare two DAT files (DAT Duel)";

impl DatDuelPanel {
    pub(crate) fn show_section(&mut self, ui: &mut egui::Ui) {
        crate::ui::components::collapsible_section(
            ui,
            "dat-duel-section",
            SECTION_TITLE,
            false,
            |ui| {
                self.show(ui);
            },
        );
    }
}

impl crate::app::ArchiveFsApp {
    /// The DAT Duel disclosure followed by the DAT Sources page.
    pub(crate) fn show_dat_sources_with_duel(&mut self, ui: &mut egui::Ui) {
        self.sources_ui.dat_duel.show_section(ui);
        self.show_dat_sources_page(ui);
    }
}

#[cfg(test)]
mod tests;
