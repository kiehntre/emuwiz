//! Blue page: PlayStation memory cards (PS1 and PS2).
//!
//! Finds cards in the emulators' save folders, shows what is on them, backs
//! them up, exports single saves and builds a copy of a card with a save
//! added. It never writes to a card it shows: every result is a new file in
//! an EmuWiz-owned folder. All reading and writing happens on a worker
//! thread; this module owns only the page state and its drawing.

use crate::ui::components::{
    ActionStyle, StatusTone, action_button, banner, card, section_header, status_badge,
};
use archivefs_core::memory_card_manager::{
    self as manager, CardFormat, CardHealth, CardReport, SaveExport, SaveImport,
    backup::{self, BackupCheck, CardBackup},
    discovery::{self, CardSearch, FoundCard},
    external_tools::{self, ExternalTool},
};
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

const INTRO: &str = "Look inside PS1 and PS2 memory cards, back them up, and copy single saves \
in and out. EmuWiz never changes a card shown here: backups, exported saves and new cards are \
always separate files.";

/// Where this page writes. All three are EmuWiz-owned folders.
#[derive(Clone, Debug)]
pub(super) struct Folders {
    pub backups: PathBuf,
    pub exports: PathBuf,
    pub copies: PathBuf,
}

impl Folders {
    fn live() -> Result<Self, String> {
        let backups = backup::default_backup_root().map_err(|error| error.to_string())?;
        let data = backups
            .parent()
            .ok_or("EmuWiz's data folder is unavailable")?
            .to_path_buf();
        Ok(Self {
            backups,
            exports: data.join("memory-card-exports"),
            copies: data.join("memory-card-copies"),
        })
    }
}

enum Outcome {
    Found {
        search: CardSearch,
        tools: Vec<ExternalTool>,
        backups: Vec<CardBackup>,
    },
    Card(PathBuf, Result<CardReport, String>),
    Backup(Result<CardBackup, String>, Vec<CardBackup>),
    Export(Result<SaveExport, String>),
    Import(Result<SaveImport, String>),
    Check(PathBuf, Result<BackupCheck, String>),
    Started(Result<String, String>),
}

#[derive(Default)]
pub(super) struct State {
    /// `None` until first use: then the folders under EmuWiz's data folder.
    folders: Option<Folders>,
    /// Look for cards only in this folder (tests); otherwise in the
    /// emulators' configured save folders.
    only_folder: Option<PathBuf>,
    started: bool,
    job: Option<Receiver<Outcome>>,
    busy: Option<&'static str>,
    cards: Vec<FoundCard>,
    unavailable: Vec<String>,
    tools: Vec<ExternalTool>,
    backups: Vec<CardBackup>,
    checks: HashMap<PathBuf, String>,
    selected: Option<PathBuf>,
    report: Option<Result<CardReport, String>>,
    message: Option<(bool, String)>,
    import_file: String,
}

impl State {
    #[cfg(test)]
    pub(super) fn for_folder(cards: PathBuf, folders: Folders) -> Self {
        Self {
            folders: Some(folders),
            only_folder: Some(cards),
            ..Self::default()
        }
    }

    fn folders(&mut self) -> Result<Folders, String> {
        if self.folders.is_none() {
            self.folders = Some(Folders::live()?);
        }
        Ok(self.folders.clone().expect("set above"))
    }

    fn start(
        &mut self,
        ctx: &egui::Context,
        busy: &'static str,
        work: impl FnOnce() -> Outcome + Send + 'static,
    ) {
        if self.job.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.job = Some(receiver);
        self.busy = Some(busy);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(work());
            ctx.request_repaint();
        });
    }

    pub(super) fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let outcome = match job.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.busy = None;
                self.message = Some((false, "The memory-card worker stopped. Try again.".into()));
                return;
            }
        };
        self.job = None;
        self.busy = None;
        match outcome {
            Outcome::Found {
                search,
                tools,
                backups,
            } => {
                self.cards = search.cards;
                self.unavailable = search.unavailable;
                self.tools = tools;
                self.backups = backups;
            }
            Outcome::Card(path, report) => {
                if self.selected.as_ref() == Some(&path) {
                    self.report = Some(report);
                }
            }
            Outcome::Backup(result, backups) => {
                self.backups = backups;
                self.message = Some(match result {
                    Ok(backup) => (
                        true,
                        format!(
                            "Backup saved and verified: {} ({} bytes, SHA-256 {}…). The card \
                             was only read.",
                            backup.backup_file.display(),
                            backup.size_bytes,
                            &backup.sha256[..12]
                        ),
                    ),
                    Err(error) => (false, format!("No backup was made: {error}")),
                });
            }
            Outcome::Export(result) => {
                self.message = Some(match result {
                    Ok(export) => (
                        true,
                        format!(
                            "Exported {} to {} (.{}, {} bytes). The card was only read.",
                            export.save_name,
                            export.destination.display(),
                            export.format,
                            export.bytes
                        ),
                    ),
                    Err(error) => (false, format!("Nothing was exported: {error}")),
                });
            }
            Outcome::Import(result) => {
                self.message = Some(match result {
                    Ok(import) => (
                        true,
                        format!(
                            "New card with {} added: {}. Checked: {}. The card you opened is \
                             unchanged.",
                            import.save_name,
                            import.new_card.display(),
                            import.checks.join("; ")
                        ),
                    ),
                    Err(error) => (false, format!("No new card was made: {error}")),
                });
            }
            Outcome::Check(record, result) => {
                let text = match result {
                    Ok(check) => check.detail,
                    Err(error) => format!("Could not check this backup: {error}"),
                };
                self.checks.insert(record, text);
            }
            Outcome::Started(result) => {
                self.message = Some(match result {
                    Ok(text) => (true, text),
                    Err(error) => (false, error),
                });
            }
        }
    }

    pub(super) fn refresh(&mut self, ctx: &egui::Context) {
        let only = self.only_folder.clone();
        let backups = self.folders().map(|folders| folders.backups).ok();
        self.start(ctx, "Looking for memory cards…", move || {
            let (search, tools) = match only {
                Some(folder) => (
                    CardSearch {
                        cards: discovery::find_cards_in(&folder, None),
                        unavailable: Vec::new(),
                    },
                    Vec::new(),
                ),
                None => (
                    discovery::find_configured_cards(),
                    external_tools::find_external_tools(),
                ),
            };
            Outcome::Found {
                search,
                tools,
                backups: backups
                    .map(|root| backup::list_backups(&root))
                    .unwrap_or_default(),
            }
        });
    }

    pub(super) fn open(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.selected = Some(path.clone());
        self.report = None;
        self.message = None;
        self.start(ctx, "Reading the card…", move || {
            let report = manager::read_card(&path).map_err(|error| error.to_string());
            Outcome::Card(path, report)
        });
    }

    pub(super) fn back_up(&mut self, ctx: &egui::Context) {
        let (Some(card), Ok(folders)) = (self.selected.clone(), self.folders()) else {
            return;
        };
        self.start(ctx, "Backing up and verifying…", move || {
            let result =
                backup::backup_card(&card, &folders.backups).map_err(|error| error.to_string());
            Outcome::Backup(result, backup::list_backups(&folders.backups))
        });
    }

    pub(super) fn export(&mut self, ctx: &egui::Context, save_name: String, extension: &str) {
        let (Some(card), Ok(folders)) = (self.selected.clone(), self.folders()) else {
            return;
        };
        let extension = extension.to_string();
        self.start(ctx, "Exporting the save…", move || {
            let result = std::fs::create_dir_all(&folders.exports)
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    let destination = unused_name(&folders.exports, &save_name, &extension);
                    manager::export_save(&card, &save_name, &destination)
                        .map_err(|error| error.to_string())
                });
            Outcome::Export(result)
        });
    }

    pub(super) fn import(&mut self, ctx: &egui::Context) {
        let (Some(card), Ok(folders)) = (self.selected.clone(), self.folders()) else {
            return;
        };
        let save_file = PathBuf::from(self.import_file.trim());
        let ps1 = matches!(&self.report, Some(Ok(report)) if report.format.platform()
            == Some(manager::CardPlatform::PlayStation));
        self.start(ctx, "Building and checking the new card…", move || {
            let result = std::fs::create_dir_all(&folders.copies)
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    let stem = format!("{}-with-{}", file_stem(&card), file_stem(&save_file));
                    // A PS1 result is always a plain 128 KB image.
                    let extension = if ps1 {
                        "mcd".to_string()
                    } else {
                        card.extension()
                            .map(|extension| extension.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "ps2".into())
                    };
                    let new_card = unused_name(&folders.copies, &stem, &extension);
                    manager::import_save_into_copy(&card, &save_file, &new_card)
                        .map_err(|error| error.to_string())
                });
            Outcome::Import(result)
        });
    }

    fn check(&mut self, ctx: &egui::Context, record: PathBuf) {
        self.start(ctx, "Checking the backup…", move || {
            let result = backup::verify_backup(&record).map_err(|error| error.to_string());
            Outcome::Check(record, result)
        });
    }

    fn start_tool(&mut self, ctx: &egui::Context, tool: ExternalTool) {
        self.start(ctx, "Starting the editor…", move || {
            Outcome::Started(
                external_tools::launch(&tool)
                    .map(|_| {
                        format!(
                            "{} was started as its own program. Open a backup copy in it, not \
                             a card an emulator is using.",
                            tool.name
                        )
                    })
                    .map_err(|error| error.to_string()),
            )
        });
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "card".into())
}

/// `<folder>/<stem>.<extension>`, or with `-2`, `-3`… when that exists.
/// Characters outside letters, digits, `-`, `_` and `.` become `_`.
fn unused_name(folder: &Path, stem: &str, extension: &str) -> PathBuf {
    let safe: String = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || "-_.".contains(character) {
                character
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "save".to_string()
    } else {
        safe
    };
    let mut candidate = folder.join(format!("{safe}.{extension}"));
    let mut number = 2;
    while std::fs::symlink_metadata(&candidate).is_ok() {
        candidate = folder.join(format!("{safe}-{number}.{extension}"));
        number += 1;
    }
    candidate
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll();
    if !state.started {
        state.started = true;
        state.refresh(ui.ctx());
    }
    let idle = state.job.is_none();
    section_header(ui, "PlayStation memory cards", Some(INTRO));
    ui.horizontal(|ui| {
        if action_button(ui, "Look again", ActionStyle::Secondary, idle).clicked() {
            state.refresh(ui.ctx());
        }
        if action_button(ui, "Open a card file…", ActionStyle::Secondary, idle).clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Choose a PS1 or PS2 memory card")
                .pick_file()
        {
            state.open(ui.ctx(), path);
        }
        if let Some(busy) = state.busy {
            ui.spinner();
            ui.label(busy);
        }
    });
    if let Some((ok, text)) = &state.message {
        let (title, tone) = if *ok {
            ("Done", StatusTone::Success)
        } else {
            ("Not done", StatusTone::Blocked)
        };
        banner(ui, title, text, tone);
    }

    let mut open = None;
    card(ui, |ui| {
        ui.strong(format!("Cards found ({})", state.cards.len()));
        if state.cards.is_empty() && idle {
            ui.label(
                "No PS1 or PS2 memory cards were found in the save folders of PCSX2, \
                 DuckStation, RetroArch or RPCS3. Use “Open a card file…” for a card kept \
                 elsewhere.",
            );
        }
        for found in &state.cards {
            let name = found
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Two installs of one emulator often hold cards with the same
            // file name, so the folder is part of the line.
            let folder = found
                .path
                .parent()
                .map(|parent| {
                    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
                    match home.and_then(|home| parent.strip_prefix(home).ok()) {
                        Some(rest) => format!("~/{}", rest.display()),
                        None => parent.display().to_string(),
                    }
                })
                .unwrap_or_default();
            let line = format!(
                "{name}  ·  {}  ·  {}  ·  {folder}",
                found.emulator.as_deref().unwrap_or("Folder"),
                found.format.label()
            );
            let selected = state.selected.as_ref() == Some(&found.path);
            if ui
                .selectable_label(selected, line)
                .on_hover_text(found.path.display().to_string())
                .clicked()
                && idle
            {
                open = Some(found.path.clone());
            }
        }
        for line in &state.unavailable {
            ui.colored_label(ui.visuals().warn_fg_color, format!("Not readable: {line}"));
        }
    });
    if let Some(path) = open {
        state.open(ui.ctx(), path);
    }

    let mut export = None;
    let mut back_up = false;
    let mut import = false;
    if let (Some(path), Some(report)) = (&state.selected, &state.report) {
        card(ui, |ui| match report {
            Err(error) => {
                banner(
                    ui,
                    "This file could not be read",
                    error,
                    StatusTone::Blocked,
                );
            }
            Ok(report) => {
                ui.heading(
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                ui.label(egui::RichText::new(path.display().to_string()).small());
                ui.horizontal(|ui| {
                    let (label, tone) = match report.health {
                        CardHealth::Healthy => ("Readable: two readers agree", StatusTone::Success),
                        CardHealth::Warnings => ("Read with warnings", StatusTone::Warning),
                        CardHealth::Unreadable => ("Not readable", StatusTone::Blocked),
                    };
                    status_badge(ui, label, tone);
                    ui.label(report.format.label());
                    ui.label(size_text(report.size_bytes));
                    if let Some(free) = &report.free_space {
                        ui.label(free);
                    }
                });
                for note in &report.notes {
                    ui.colored_label(ui.visuals().warn_fg_color, note);
                }
                let can_back_up = !matches!(
                    report.format,
                    CardFormat::NotACard | CardFormat::Pcsx2Folder
                );
                if action_button(
                    ui,
                    "Back up this card",
                    ActionStyle::Primary,
                    idle && can_back_up,
                )
                .on_hover_text(
                    "Copies the card to EmuWiz's backup folder and checks the copy. The \
                         card is only read.",
                )
                .clicked()
                {
                    back_up = true;
                }
                ui.add_space(6.0);
                ui.strong(format!("Saves ({})", report.saves.len()));
                egui::Grid::new("memory_card_saves")
                    .striped(true)
                    .num_columns(6)
                    .show(ui, |ui| {
                        for heading in ["Title", "Game code", "Region", "Size", "Name on card", ""]
                        {
                            ui.label(egui::RichText::new(heading).small().strong());
                        }
                        ui.end_row();
                        for save in &report.saves {
                            ui.label(save.title.as_deref().unwrap_or("(no title in the save)"));
                            ui.label(save.product_code.as_deref().unwrap_or("-"));
                            ui.label(save.region.unwrap_or("-"));
                            ui.label(match (save.blocks, save.file_count) {
                                (Some(blocks), _) => format!("{blocks} block(s)"),
                                (None, Some(files)) => {
                                    format!("{} · {files} file(s)", size_text(save.size_bytes))
                                }
                                (None, None) => size_text(save.size_bytes),
                            });
                            ui.label(egui::RichText::new(&save.name).monospace().small())
                                .on_hover_text(save.modified.as_deref().map_or_else(
                                    || "The card stores no date for this save.".to_string(),
                                    |time| format!("Last written {time} (card clock)"),
                                ));
                            match report.export_format {
                                Some(format) => {
                                    if action_button(
                                        ui,
                                        format!("Export .{format}"),
                                        ActionStyle::Secondary,
                                        idle,
                                    )
                                    .clicked()
                                    {
                                        export = Some((save.name.clone(), format));
                                    }
                                }
                                None => {
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
                if report.saves.is_empty() && report.format != CardFormat::NotACard {
                    ui.label("This card has no saves.");
                }
                if !report.import_formats.is_empty() {
                    ui.add_space(6.0);
                    ui.strong("Add a save to a copy of this card");
                    ui.label(format!(
                        "Accepts {}. EmuWiz writes a new card file with the save added and \
                         checks it; the card above is not changed.",
                        report
                            .import_formats
                            .iter()
                            .map(|format| format!(".{format}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.import_file)
                                .hint_text("Save file to add")
                                .desired_width(360.0),
                        );
                        if action_button(ui, "Choose…", ActionStyle::Quiet, idle).clicked()
                            && let Some(file) = rfd::FileDialog::new()
                                .set_title("Choose a save file")
                                .pick_file()
                        {
                            state.import_file = file.display().to_string();
                        }
                        let ready = idle && !state.import_file.trim().is_empty();
                        if action_button(ui, "Make the new card", ActionStyle::Secondary, ready)
                            .clicked()
                        {
                            import = true;
                        }
                    });
                }
                if !report.unsupported.is_empty() {
                    ui.collapsing("What EmuWiz cannot do with this card", |ui| {
                        for line in &report.unsupported {
                            ui.label(line);
                        }
                    });
                }
            }
        });
    }
    if back_up {
        state.back_up(ui.ctx());
    }
    if let Some((save_name, format)) = export {
        state.export(ui.ctx(), save_name, format);
    }
    if import {
        state.import(ui.ctx());
    }

    let mut check = None;
    if !state.backups.is_empty() {
        card(ui, |ui| {
            ui.strong(format!("Backups ({})", state.backups.len()));
            for backup in &state.backups {
                let record = backup.record_file();
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{}  ·  {}  ·  {}  ·  {} save(s)",
                        backup
                            .source
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        super::save_snapshots::format_unix_seconds(backup.created_unix_seconds),
                        backup.card_kind,
                        backup.save_names.len()
                    ))
                    .on_hover_text(backup.backup_file.display().to_string());
                    if action_button(ui, "Check", ActionStyle::Quiet, idle).clicked() {
                        check = Some(record.clone());
                    }
                });
                if let Some(detail) = state.checks.get(&record) {
                    ui.label(egui::RichText::new(detail).small());
                }
            }
        });
    }
    if let Some(record) = check {
        state.check(ui.ctx(), record);
    }

    let mut start_tool = None;
    card(ui, |ui| {
        ui.strong("Other card editors");
        if state.tools.is_empty() {
            ui.label(
                "Optional: if you install myMCpp (PS2) or PSMemoryManager (PS1 and PS2) \
                 yourself, EmuWiz can start it from here. They are separate GPL programs; \
                 EmuWiz does not include or download them.",
            );
        }
        for tool in &state.tools {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} ({}, {})",
                    tool.name, tool.handles, tool.licence
                ))
                .on_hover_text(tool.executable.display().to_string());
                if action_button(ui, format!("Start {}", tool.name), ActionStyle::Quiet, idle)
                    .on_hover_text(
                        "Starts it as its own program, without a card. Open a backup copy in \
                         it, not a card an emulator is using.",
                    )
                    .clicked()
                {
                    start_tool = Some(tool.clone());
                }
            });
        }
    });
    if let Some(tool) = start_tool {
        state.start_tool(ui.ctx(), tool);
    }
}

#[cfg(test)]
#[path = "memory_cards_tests.rs"]
mod tests;
