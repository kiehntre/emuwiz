//! User-visible recovery of the existing ADF trainer planner. This surface has
//! no installer, launch mutation, emulator command or runtime injection path.
//! WHDLoad CUSTOM switches remain exclusively in whdload_trainer_flow.
use archivefs_core::{amiga_adf_trainer::*, patch_manager::CheatReviewChoice};
use eframe::egui;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PreviewEmulator {
    #[default]
    FsUae,
    WinUae,
    Amiberry,
    Other,
}
impl PreviewEmulator {
    fn label(self) -> &'static str {
        match self {
            Self::FsUae => "FS-UAE",
            Self::WinUae => "WinUAE",
            Self::Amiberry => "Amiberry",
            Self::Other => "Other / RetroArch",
        }
    }
    pub(crate) fn blocker(self) -> &'static str {
        match self {
            Self::FsUae => {
                "Preview only. EmuWiz has no verified FS-UAE trainer-file loader or runtime execution link. Lua memory APIs in some builds are not an established trainer loading interface; builds without Lua cannot execute them."
            }
            Self::WinUae => {
                "Preview only. WinUAE's interactive debugger is not an EmuWiz trainer execution interface. No file loader or guarded runtime connection is implemented."
            }
            Self::Amiberry => {
                "Preview only. No supported Amiberry ADF trainer execution mechanism is established. WHDLoad CUSTOM options do not apply to floppy images."
            }
            Self::Other => {
                "Preview only. No ADF trainer format or runtime mechanism is verified for this emulator/core."
            }
        }
    }
}

pub(crate) fn is_candidate(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("adf"))
}

#[derive(Clone, Debug)]
pub(crate) struct Loaded {
    pub(crate) media: AdfMedia,
    pub(crate) import: AmigaTrainerImport,
    pub(crate) plan: AmigaTrainerPlan,
}

/// Read only the selected disk and explicitly chosen trainer source. The image
/// must validate structurally; .adf is merely a navigation hint, never identity.
pub(crate) fn load(game: &Path, catalogue_name: &str, source: &Path) -> Result<Loaded, String> {
    if !is_candidate(game) {
        return Err(
            "Choose a loose ADF image; WHDLoad, IPF, DMS and archives use different mechanisms."
                .into(),
        );
    }
    let media = inspect_adf_media(game, Some(catalogue_name)).map_err(|e| e.to_string())?;
    let import = import_amiga_trainers_from_file(source).map_err(|e| e.to_string())?;
    let plan = plan_amiga_adf_trainers(
        &import,
        &AmigaTrainerContext::for_media(&media),
        &BTreeMap::new(),
    );
    Ok(Loaded {
        media,
        import,
        plan,
    })
}

pub(crate) fn replan(loaded: &mut Loaded, choices: &BTreeMap<usize, CheatReviewChoice>) {
    loaded.plan = plan_amiga_adf_trainers(
        &loaded.import,
        &AmigaTrainerContext::for_media(&loaded.media),
        choices,
    );
}

/// Re-read both inputs after explicit selection. A stale source/image cannot
/// produce a supposedly verified preview. There is deliberately no enable API.
pub(crate) fn preview(
    loaded: &Loaded,
    catalogue_name: &str,
    source: &Path,
    choices: &BTreeMap<usize, CheatReviewChoice>,
    selected: &BTreeSet<usize>,
    emulator: PreviewEmulator,
) -> Result<String, String> {
    if selected.is_empty() {
        return Err("Select at least one compatible trainer for preview.".into());
    }
    let mut fresh = load(&loaded.media.path, catalogue_name, source)?;
    if fresh.media != loaded.media || fresh.import != loaded.import {
        return Err(
            "The disk or trainer source changed. Read both files again and review a new selection."
                .into(),
        );
    }
    replan(&mut fresh, choices);
    if selected.iter().any(|i| !fresh.plan.selected.contains(i)) {
        return Err("A selected trainer is incompatible, duplicated or has an unresolved conflict. Review the selection.".into());
    }
    fresh.plan.selected.retain(|i| selected.contains(i));
    for a in &mut fresh.plan.assessments {
        if a.selection == AmigaTrainerSelection::Selected && !selected.contains(&a.index) {
            a.selection = AmigaTrainerSelection::NotChosen;
        }
    }
    let trainers: Vec<_> = fresh
        .import
        .trainers
        .iter()
        .filter(|t| selected.contains(&t.index))
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "mode": "preview_only", "execution_enabled": false,
        "emulator": emulator.label(), "runtime_blocker": emulator.blocker(),
        "source_file": fresh.import.source_name, "source_sha256": fresh.import.source_sha256,
        "source_verification": "Exact local source bytes; author and reference are declarations, not independently authenticated",
        "import_summary": {"accepted": fresh.import.trainers.len(), "rejected": fresh.import.rejected, "dropped_over_limit": fresh.import.dropped_over_limit},
        "trainers": trainers, "plan": fresh.plan,
    })).map_err(|e| e.to_string())
}

enum WorkResult {
    Loaded(Loaded),
    Preview(String),
}
#[derive(Default)]
pub(crate) struct AdfTrainerState {
    source: String,
    emulator: PreviewEmulator,
    loaded: Option<Loaded>,
    choices: BTreeMap<usize, CheatReviewChoice>,
    selected: BTreeSet<usize>,
    preview: Option<String>,
    error: Option<String>,
    worker: Option<Receiver<Result<WorkResult, String>>>,
}
impl AdfTrainerState {
    fn invalidate(&mut self) {
        self.preview = None;
        self.error = None;
    }
    fn clear_source(&mut self) {
        self.invalidate();
        self.loaded = None;
        self.selected.clear();
        self.choices.clear();
    }
    fn poll(&mut self) {
        let Some(receiver) = &self.worker else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(v) => v,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The read-only trainer worker stopped. Try reading the files again.".into())
            }
        };
        self.worker = None;
        match result {
            Ok(WorkResult::Loaded(loaded)) => self.loaded = Some(loaded),
            Ok(WorkResult::Preview(text)) => self.preview = Some(text),
            Err(error) => {
                self.preview = None;
                self.error = Some(error);
            }
        }
    }
    fn start_load(&mut self, game: PathBuf, catalogue_name: String, ctx: egui::Context) {
        if self.worker.is_some() {
            return;
        }
        self.clear_source();
        let source = PathBuf::from(&self.source);
        let (sender, receiver) = mpsc::channel();
        self.worker = Some(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(load(&game, &catalogue_name, &source).map(WorkResult::Loaded));
            ctx.request_repaint();
        });
    }
    fn start_preview(&mut self, catalogue_name: String, ctx: egui::Context) {
        if self.worker.is_some() {
            return;
        }
        self.invalidate();
        let Some(loaded) = self.loaded.clone() else {
            return;
        };
        let (choices, selected, emulator, source) = (
            self.choices.clone(),
            self.selected.clone(),
            self.emulator,
            PathBuf::from(&self.source),
        );
        let (sender, receiver) = mpsc::channel();
        self.worker = Some(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(
                preview(
                    &loaded,
                    &catalogue_name,
                    &source,
                    &choices,
                    &selected,
                    emulator,
                )
                .map(WorkResult::Preview),
            );
            ctx.request_repaint();
        });
    }
}

fn blocker_label(b: &AmigaTrainerBlocker) -> String {
    use AmigaTrainerBlocker as B;
    match b {
        B::Mechanism(m) => m
            .unsupported_reason()
            .unwrap_or("Mechanism is not supported")
            .into(),
        B::MediaNotTargeted | B::IdentityMismatch => "Does not target this exact disk image".into(),
        B::AlternateMedia(m) => format!("Alternate media: {}", m.label()),
        B::WrongDisk { wanted, found } => format!("Needs disk {wanted}; this is disk {found}"),
        B::ConflictNeedsChoice => "Review the conflicting trainer choices below".into(),
        B::NotChosen => "Not chosen in conflict review".into(),
        B::SetRequired
        | B::SetIncomplete
        | B::SetUnverified
        | B::SetOrderUnverified
        | B::SetAmbiguous => {
            "Needs a complete, verified disk set; this panel inspects one disk only".into()
        }
        B::DiskUnconfirmed => "Trainer does not establish this disk's ordinal".into(),
        B::ReleaseMismatch | B::RegionMismatch | B::RevisionMismatch => {
            "Release, revision or region does not match".into()
        }
        B::ReleaseEvidenceUnknown(field) => format!("Verified {field} evidence is missing"),
        B::PlatformMismatch { claimed } => format!("Incompatible platform: {claimed}"),
        B::IdentityNotVerified | B::IdentityEmpty | B::IdentityConflicting => {
            "Exact-image identity is missing or conflicting".into()
        }
        B::TooManyForGame => "Per-game trainer limit reached".into(),
        B::CanonicalRefusal(_) => {
            "The existing cheat applicability checks refused this trainer".into()
        }
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    game: &Path,
    catalogue_name: &str,
    state: &mut AdfTrainerState,
) {
    state.poll();
    ui.heading("Amiga ADF trainers");
    ui.label("Import and review trainers for this exact floppy image. WHDLoad CUSTOM switches are separate and do not apply here.");
    ui.strong("Preview only — trainers cannot be enabled in-game from this panel.");
    ui.label("Your disk, game identity evidence and saved launch settings stay unchanged.");
    ui.label(format!("Selected disk: {}", game.display()));
    let busy = state.worker.is_some();
    ui.add_enabled_ui(!busy, |ui| {
        let before = state.emulator;
        egui::ComboBox::from_label("Emulator for this preview")
            .selected_text(state.emulator.label())
            .show_ui(ui, |ui| {
                for e in [
                    PreviewEmulator::FsUae,
                    PreviewEmulator::WinUae,
                    PreviewEmulator::Amiberry,
                    PreviewEmulator::Other,
                ] {
                    ui.selectable_value(&mut state.emulator, e, e.label());
                }
            });
        if before != state.emulator {
            state.invalidate();
        }
        ui.label("This preview choice does not change your launch emulator.");
        ui.label(state.emulator.blocker());
        ui.label("Local trainer file (emuwiz-amiga-adf-trainer/1 JSON):");
        if ui.text_edit_singleline(&mut state.source).changed() {
            state.clear_source();
        }
        ui.horizontal(|ui| {
            if ui.button("Choose trainer file…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Amiga ADF trainer JSON", &["json"])
                    .pick_file()
                {
                    state.source = path.to_string_lossy().into_owned();
                    state.clear_source();
                }
            }
            if ui
                .add_enabled(
                    !state.source.trim().is_empty(),
                    egui::Button::new("Read disk and trainers"),
                )
                .clicked()
            {
                state.start_load(game.to_path_buf(), catalogue_name.into(), ui.ctx().clone());
            }
        });
    });
    if busy || state.worker.is_some() {
        ui.spinner();
        ui.label("Reading selected files; no changes are made…");
    }
    if let Some(error) = &state.error {
        ui.colored_label(crate::ui::theme::warning(ui), error);
    }
    let Some(loaded) = state.loaded.as_ref() else {
        return;
    };
    ui.label(format!(
        "Validated {} AmigaDOS image · SHA-256 {}",
        loaded.media.dos_family, loaded.media.sha256
    ));
    ui.label(format!(
        "Source: {} · SHA-256 {}",
        loaded.import.source_name, loaded.import.source_sha256
    ));
    ui.label("Source bytes are verified locally. Author names and references below are supplied by the file, not authenticated endorsements.");
    if loaded.import.trainers.is_empty() {
        ui.label("No valid trainer records were imported.");
    }
    if loaded.import.dropped_over_limit > 0 {
        ui.label(format!(
            "{} additional entries skipped because the per-file limit was reached",
            loaded.import.dropped_over_limit
        ));
    }
    for d in &loaded.import.diagnostics {
        ui.label(format!("Rejected entry {}: {}", d.index + 1, d.reason));
    }
    if loaded.import.rejected > loaded.import.diagnostics.len() {
        ui.label(format!(
            "{} entries rejected in total (diagnostics bounded)",
            loaded.import.rejected
        ));
    }
    let mut changed = false;
    ui.add_enabled_ui(!busy, |ui| {
        for t in &loaded.import.trainers {
            let Some(a) = loaded.plan.assessments.iter().find(|a| a.index == t.index) else {
                continue;
            };
            ui.group(|ui| {
                let eligible = loaded.plan.selected.contains(&t.index);
                let mut selected = state.selected.contains(&t.index);
                if ui
                    .add_enabled(
                        eligible,
                        egui::Checkbox::new(
                            &mut selected,
                            format!("{} — {}", t.title, t.mechanism.label()),
                        ),
                    )
                    .changed()
                {
                    if selected {
                        state.selected.insert(t.index);
                    } else {
                        state.selected.remove(&t.index);
                    }
                    changed = true;
                }
                ui.label(if eligible {
                    "Exact image compatible · preview only"
                } else {
                    a.status.label()
                });
                ui.label(format!("Declared source: {}", t.source.name));
                if let Some(reference) = &t.source.reference {
                    ui.label(format!("Reference: {reference}"));
                }
                if let Some(description) = &t.description {
                    ui.label(description);
                }
                if let AmigaTrainerSelection::DuplicateOf(index) = a.selection {
                    ui.label(format!(
                        "Duplicate of entry {}; preview that entry instead",
                        index + 1
                    ));
                }
                for b in &a.blockers {
                    ui.label(blocker_label(b));
                }
                for w in &t.writes {
                    ui.monospace(format!(
                        "0x{:08X}: {:?} = 0x{:X} · original {:?} · {:?}",
                        w.address, w.width, w.value, w.original, w.timing
                    ));
                }
            });
        }
        for group in &loaded.plan.conflict_groups {
            ui.group(|ui| {
                ui.strong("Conflicting trainers — choose before selecting");
                for (position, index) in group.trainers.iter().enumerate() {
                    let title = loaded
                        .import
                        .trainers
                        .iter()
                        .find(|t| t.index == *index)
                        .map(|t| t.title.as_str())
                        .unwrap_or("Unavailable");
                    ui.label(format!("{}: {title} (entry {})", position + 1, index + 1));
                }
                let mut choice = state.choices.get(&group.group_index).copied();
                egui::ComboBox::from_id_salt(("adf_conflict", group.group_index))
                    .selected_text(match choice {
                        Some(CheatReviewChoice::KeepA) => "Keep first",
                        Some(CheatReviewChoice::KeepB) => "Keep second",
                        Some(_) => "Skip group",
                        None => "Choose…",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut choice, None, "Unresolved");
                        if group.trainers.len() == 2 {
                            ui.selectable_value(
                                &mut choice,
                                Some(CheatReviewChoice::KeepA),
                                "Keep first",
                            );
                            ui.selectable_value(
                                &mut choice,
                                Some(CheatReviewChoice::KeepB),
                                "Keep second",
                            );
                        }
                        ui.selectable_value(
                            &mut choice,
                            Some(CheatReviewChoice::Skip),
                            "Skip group",
                        );
                    });
                if choice != state.choices.get(&group.group_index).copied() {
                    match choice {
                        Some(c) => {
                            state.choices.insert(group.group_index, c);
                        }
                        None => {
                            state.choices.remove(&group.group_index);
                        }
                    }
                    changed = true;
                }
            });
        }
    });
    if changed {
        state.invalidate();
        if let Some(loaded) = &mut state.loaded {
            replan(loaded, &state.choices);
            state.selected.retain(|i| loaded.plan.selected.contains(i));
        }
    }
    if ui
        .add_enabled(
            !busy && !state.selected.is_empty(),
            egui::Button::new("Preview selected options"),
        )
        .clicked()
    {
        state.start_preview(catalogue_name.into(), ui.ctx().clone());
    }
    ui.add_enabled(false, egui::Button::new("Enable in emulator — unavailable"));
    if let Some(text) = &state.preview {
        ui.strong("Verified read-only preview — no trainer has been enabled");
        if ui.button("Copy preview report").clicked() {
            ui.ctx().copy_text(text.clone());
        }
        egui::ScrollArea::vertical()
            .id_salt("adf_preview")
            .max_height(280.0)
            .show(ui, |ui| {
                ui.monospace(text);
            });
    }
}

#[cfg(test)]
mod tests;
