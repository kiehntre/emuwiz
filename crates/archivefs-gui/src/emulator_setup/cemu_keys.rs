//! Explicit, read-only diagnostics. Results never enter Cemu launch requests.
use archivefs_core::{
    launch::cemu_readiness::CemuReadinessInspection,
    wiiu_content_readiness::key_diagnostics::{
        self as diagnostic, ProfileDiagnostics, Requirement, StoreDiagnostic, StoreState,
    },
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

const MAX_PROFILES: usize = archivefs_core::patch_manager::CEMU_MAX_PROFILES;

#[derive(Default)]
struct Report {
    profiles: Vec<ProfileDiagnostics>,
    selected: Option<StoreDiagnostic>,
    truncated: bool,
}

#[derive(Default)]
pub(super) struct State {
    receiver: Option<Receiver<Report>>,
    report: Option<Report>,
    error: bool,
}

fn requirement_text(requirement: Requirement) -> &'static str {
    match requirement {
        Requirement::NotRequired => {
            "Additional disc keys are not required for this inspected decrypted, extracted content. This is structural evidence, not complete game verification."
        }
        Requirement::DiscKeyRequired => {
            "This inspected encrypted disc format requires a matching disc key. Store presence or valid syntax cannot prove a key applies to this game. Encrypted-container Launch remains unsupported."
        }
        Requirement::EncryptedTitleUnsupported => {
            "This encrypted title package requires title/ticket key material. A Cemu disc keys.txt check cannot establish readiness for this package. EmuWiz does not extract keys, decrypt or install packages; manually prepare a supported decrypted, extracted game before refreshing readiness."
        }
        Requirement::Unknown => {
            "Key requirements are unknown because this content's structure or encryption cannot be established by the supported inspection. Refresh after manually preparing supported content; a filename is not proof."
        }
    }
}

fn store_text(state: &StoreState) -> String {
    match state {
        StoreState::Missing => "Missing at this checked location; other Cemu user-data locations were not searched.".into(),
        StoreState::Empty => "No entries found. Comments and blank lines do not provide keys.".into(),
        StoreState::Malformed { line } => format!("Structurally invalid for this diagnostic at line {line}. Check the local file manually; no values are displayed."),
        StoreState::LineTooLong { line } => format!("Unknown: line {line} exceeds the 4 KiB diagnostic limit. Check it manually; a long comment is not proof of invalid syntax."),
        StoreState::WellFormedUnverified { entries } => format!("{entries} syntactically accepted entries. Applicability UNKNOWN: no key or game content was verified, and Cemu's active store binding is not confirmed."),
        StoreState::TooLarge => "Unknown: store exceeds the 64 KiB diagnostic limit; check it manually.".into(),
        StoreState::UnsafePath => "Unknown: this diagnostic refuses links, special files or unsafe paths. Choose a regular keys.txt in a real directory.".into(),
        StoreState::Unreadable => "Unknown: store could not be read safely. Check the path and permissions manually.".into(),
        StoreState::Changed => "Unknown: store changed during inspection. Re-check after local changes finish.".into(),
    }
}

impl State {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    fn begin(
        &mut self,
        inspection: &CemuReadinessInspection,
        selected: Option<PathBuf>,
        ctx: egui::Context,
    ) {
        if diagnostic::requirement(&inspection.content) != Requirement::DiscKeyRequired {
            return;
        }
        self.report = None;
        self.error = false;
        // Use the current inspection's exact profile/executable paths. Do not
        // rediscover profiles, persist paths or change runtime profile binding.
        let truncated = selected.is_none() && inspection.profiles.len() > MAX_PROFILES;
        let candidates: Vec<_> = if selected.is_none() {
            inspection
                .profiles
                .iter()
                .take(MAX_PROFILES)
                .map(|p| {
                    (
                        p.profile_id.clone(),
                        p.profile_path.clone(),
                        p.executable.clone(),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let report = Report {
                profiles: candidates
                    .into_iter()
                    .map(|(id, path, exe)| {
                        diagnostic::inspect_candidates(&id, &path, exe.as_deref())
                    })
                    .collect(),
                selected: selected.map(|path| diagnostic::inspect_store(&path)),
                truncated,
            };
            let _ = sender.send(report);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(report) => {
                    self.report = Some(report);
                    self.receiver = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.error = true;
                    self.receiver = None;
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
            }
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, inspection: &CemuReadinessInspection) {
        self.poll(ui.ctx());
        crate::ui::components::card(ui, |ui| {
            ui.strong("Wii U key diagnostics");
            let requirement = diagnostic::requirement(&inspection.content);
            ui.label(requirement_text(requirement));
            if requirement != Requirement::DiscKeyRequired {
                return;
            }
            ui.label("Checks are read-only and optional. Only bounded syntax evidence is retained; no values are displayed. Results describe the file at check time; re-check after changes.");
            ui.label("Cemu uses its user-data folder for keys.txt. Profile and executable folders below are candidates only. In Cemu use File > Open Cemu folder to locate the active folder, configure your existing local keys manually, then choose that keys.txt here if needed. No downloads, decryption or installation are provided.");
            if inspection.profiles.is_empty() {
                ui.label("No configured Cemu profile candidates are available. Configure Cemu in Emulator Setup, or choose your existing keys.txt for a standalone syntax check.");
            }
            let busy = self.receiver.is_some();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !busy && !inspection.profiles.is_empty(),
                        egui::Button::new("Check configured key-store candidates"),
                    )
                    .clicked()
                {
                    self.begin(inspection, None, ui.ctx().clone());
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Choose existing keys.txt"))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title("Choose existing Cemu keys.txt for read-only syntax check")
                        .add_filter("Cemu key store", &["txt"])
                        .pick_file()
                {
                    self.begin(inspection, Some(path), ui.ctx().clone());
                }
            });
            if busy {
                ui.label("Checking bounded key-store syntax...");
            }
            if self.error {
                ui.label(
                    "Diagnostic worker stopped. No readiness was established; run the check again.",
                );
            }
            if let Some(report) = &self.report {
                for profile in &report.profiles {
                    ui.label(format!("Candidate profile: {}", profile.profile_id));
                    for store in &profile.locations {
                        show_store(ui, store);
                    }
                }
                if let Some(store) = &report.selected {
                    ui.label("Explicitly selected store (not a runtime profile binding):");
                    show_store(ui, store);
                }
                if report.truncated {
                    ui.label("Only the first 16 profile candidates were checked. Other locations remain unknown.");
                }
            }
        });
    }
}

fn show_store(ui: &mut egui::Ui, store: &StoreDiagnostic) {
    ui.label(store.path.display().to_string());
    ui.label(store_text(&store.state));
}

#[cfg(test)]
#[path = "cemu_keys/tests.rs"]
mod tests;
