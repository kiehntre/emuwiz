//! "Check emulator compatibility" on an arcade game's details.
//!
//! One explicit button runs Core's `arcade_compat_check` on a worker thread
//! for this one game. Nothing runs on open, nothing is hashed in the
//! background, and the result is advice only: it never changes the user's
//! emulator choice, never launches anything, and is not launch readiness.
//! A result is kept only while its archive, catalogues, emulator files and
//! entered evidence are unchanged.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use archivefs_core::arcade_compat_check::{
    ArcadeCheckError, ArcadeCheckReport, ArcadeCheckRequest, FbNeoCheckInput, MameCheckInput,
    run_arcade_compat_check,
};
use archivefs_core::arcade_recommendation::{
    ArcadeEmulator, ArcadeRecommendationChoice, ArcadeRecommendationConfidence,
};
use archivefs_core::dat::catalogue_selection::{CatalogueAvailability, EvidenceValue};
use archivefs_core::dat::model::DatEcosystem;
use eframe::egui;

use super::mame_collection_health::HealthPage;

const FRESHNESS_INTERVAL: Duration = Duration::from_secs(2);

/// Everything a result depends on besides file stamps.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Inputs {
    game_id: i64,
    machine: String,
    archive: PathBuf,
    mame_catalogue: Option<PathBuf>,
    mame_version: String,
    mame_exe: String,
    fbneo_dat: Option<PathBuf>,
    fbneo_core: String,
    fbneo_version: String,
}

impl Inputs {
    fn request(&self) -> ArcadeCheckRequest {
        let path = |text: &str| {
            let text = text.trim();
            (!text.is_empty()).then(|| PathBuf::from(text))
        };
        ArcadeCheckRequest {
            set_name: self.machine.clone(),
            archive_path: self.archive.clone(),
            mame: self
                .mame_catalogue
                .clone()
                .map(|catalogue_path| MameCheckInput {
                    catalogue_path,
                    version_output: self.mame_version.clone(),
                    executable_path: path(&self.mame_exe),
                }),
            fbneo: self.fbneo_dat.clone().map(|dat_path| FbNeoCheckInput {
                dat_path,
                core_path: path(&self.fbneo_core),
                reported_version: {
                    let v = self.fbneo_version.trim();
                    (!v.is_empty()).then(|| v.to_string())
                },
            }),
            // Blue keeps no stored arcade-emulator preference; none is invented.
            user_preference: None,
        }
    }
}

struct Stored {
    inputs: Inputs,
    report: Arc<ArcadeCheckReport>,
}

struct Running {
    inputs: Inputs,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Result<ArcadeCheckReport, ArcadeCheckError>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(super) struct State {
    stored: Option<Stored>,
    running: Option<Running>,
    error: Option<String>,
    stale_reason: Option<&'static str>,
    next_freshness: Option<Instant>,
    fbneo_core: String,
    fbneo_version: String,
    mame_exe: String,
    fbneo_dat: Option<PathBuf>,
    seen_game: Option<i64>,
}

/// What the panel says. Pure so it can be tested without a window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum View {
    NoEvidenceYet,
    Checking,
    Stale(&'static str),
    Failed(String),
    Recommended {
        emulator: ArcadeEmulator,
        alternatives: Vec<ArcadeEmulator>,
        confidence: ArcadeRecommendationConfidence,
    },
    Insufficient,
    /// Both emulators were evaluated and neither is proven compatible.
    Conflicting,
}

pub(super) fn emulator_name(emulator: ArcadeEmulator) -> &'static str {
    match emulator {
        ArcadeEmulator::FinalBurnNeo => "FinalBurn Neo",
        ArcadeEmulator::Mame => "MAME",
    }
}

pub(super) fn view_of(report: &ArcadeCheckReport) -> View {
    let r = &report.recommendation;
    match r.preferred {
        ArcadeRecommendationChoice::Mame => recommended(ArcadeEmulator::Mame, r),
        ArcadeRecommendationChoice::FinalBurnNeo => recommended(ArcadeEmulator::FinalBurnNeo, r),
        ArcadeRecommendationChoice::Either => recommended(ArcadeEmulator::FinalBurnNeo, r),
        ArcadeRecommendationChoice::Neither => View::Conflicting,
        ArcadeRecommendationChoice::Unknown => View::Insufficient,
    }
}

fn recommended(
    emulator: ArcadeEmulator,
    r: &archivefs_core::arcade_recommendation::ArcadeEmulatorRecommendation,
) -> View {
    View::Recommended {
        emulator,
        alternatives: r.alternatives.clone(),
        confidence: r.confidence,
    }
}

impl State {
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        game_id: i64,
        machine: Option<String>,
        archive: PathBuf,
        health: &HealthPage,
    ) {
        if self.seen_game != Some(game_id) {
            self.seen_game = Some(game_id);
            self.stored = None;
            self.running = None;
            self.error = None;
            self.stale_reason = None;
        }
        self.poll();
        let mame_catalogue = health.selected.as_ref().and_then(|reference| {
            health
                .picker
                .summaries()
                .iter()
                .find(|s| &s.reference == reference)
                .and_then(|s| s.technical_path.clone())
        });
        let fbneo_dats: Vec<(String, PathBuf)> = health
            .picker
            .summaries()
            .iter()
            .filter(|s| {
                s.enabled
                    && matches!(s.availability, CatalogueAvailability::Ready)
                    && matches!(
                        &s.ecosystem,
                        EvidenceValue::Assigned(DatEcosystem::FBNeo)
                            | EvidenceValue::Confirmed(DatEcosystem::FBNeo)
                    )
            })
            .filter_map(|s| Some((s.display_name.clone(), s.technical_path.clone()?)))
            .collect();
        if self
            .fbneo_dat
            .as_ref()
            .is_some_and(|p| !fbneo_dats.iter().any(|(_, q)| q == p))
        {
            self.fbneo_dat = None;
        }
        let current = machine.as_ref().map(|machine| Inputs {
            game_id,
            machine: machine.clone(),
            archive,
            mame_catalogue,
            mame_version: health.version_output.clone(),
            mame_exe: self.mame_exe.clone(),
            fbneo_dat: self.fbneo_dat.clone(),
            fbneo_core: self.fbneo_core.clone(),
            fbneo_version: self.fbneo_version.clone(),
        });
        self.check_freshness(current.as_ref());

        ui.collapsing("Emulator compatibility (MAME / FinalBurn Neo)", |ui| {
            let Some(current) = current.as_ref() else {
                ui.label("A single verified machine identity is needed before this can be checked.");
                return;
            };
            ui.weak("Advice only. It does not change your emulator choice or start anything, and it is not launch readiness.");
            self.inputs_ui(ui, health, &fbneo_dats);
            let can_run = (current.mame_catalogue.is_some() || current.fbneo_dat.is_some())
                && self.running.is_none();
            ui.horizontal(|ui| {
                let button = ui.add_enabled(can_run, egui::Button::new("Check emulator compatibility"));
                if button.clicked() {
                    self.start(current.clone());
                }
                if self.running.is_some() {
                    ui.spinner();
                    if ui.button("Cancel").clicked() {
                        self.running = None;
                    }
                }
            });
            if current.mame_catalogue.is_none() && current.fbneo_dat.is_none() {
                ui.label("Choose a MAME catalogue (MAME Collection Health) or an FBNeo DAT to check against.");
            }
            self.result_ui(ui);
        });
        if self.running.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn inputs_ui(&mut self, ui: &mut egui::Ui, health: &HealthPage, dats: &[(String, PathBuf)]) {
        ui.collapsing("Evidence this check uses", |ui| {
            ui.label(if health.selected.is_some() {
                "MAME catalogue: the one chosen in MAME Collection Health."
            } else {
                "MAME catalogue: none chosen."
            });
            ui.label(if health.version_output.trim().is_empty() {
                "MAME version output: none captured (MAME evidence stays unknown)."
            } else {
                "MAME version output: as captured in MAME Collection Health (not authenticated)."
            });
            ui.horizontal(|ui| {
                ui.label("MAME executable (provenance only, never run):");
                ui.text_edit_singleline(&mut self.mame_exe);
            });
            egui::ComboBox::from_label("FBNeo DAT")
                .selected_text(
                    dats.iter()
                        .find(|(_, p)| Some(p) == self.fbneo_dat.as_ref())
                        .map_or("None", |(name, _)| name.as_str()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.fbneo_dat, None, "None");
                    for (name, path) in dats {
                        ui.selectable_value(&mut self.fbneo_dat, Some(path.clone()), name);
                    }
                });
            ui.horizontal(|ui| {
                ui.label("FBNeo core or executable:");
                ui.text_edit_singleline(&mut self.fbneo_core);
            });
            ui.horizontal(|ui| {
                ui.label("FBNeo version (optional):");
                ui.text_edit_singleline(&mut self.fbneo_version);
            });
            ui.weak("Only the game's own ZIP and the support ZIPs beside it are read, and only when you press Check. CHD disk images are not read.");
        });
    }

    fn result_ui(&self, ui: &mut egui::Ui) {
        let view = self.view();
        match &view {
            View::NoEvidenceYet => {
                ui.label("No evidence yet. Press the button to check this game.");
            }
            View::Checking => {
                ui.label("Checking this game's files against the catalogues…");
            }
            View::Stale(reason) => {
                ui.label(format!("Earlier result discarded: {reason}. Check again."));
            }
            View::Failed(error) => {
                ui.label(format!("The check did not complete: {error}"));
            }
            View::Recommended {
                emulator,
                alternatives,
                confidence,
            } => {
                ui.strong(format!(
                    "Recommended emulator: {}",
                    emulator_name(*emulator)
                ));
                if !alternatives.is_empty() {
                    let names: Vec<_> = alternatives.iter().map(|e| emulator_name(*e)).collect();
                    ui.label(format!("Also proven compatible: {}", names.join(", ")));
                }
                if *confidence == ArcadeRecommendationConfidence::ProvenWithWarnings {
                    ui.label("With warnings; see details.");
                }
            }
            View::Insufficient => {
                ui.strong("Insufficient evidence");
                ui.label("Neither emulator could be proven compatible or incompatible; missing evidence is not treated as a failure.");
            }
            View::Conflicting => {
                ui.strong("Conflicting evidence");
                ui.label("Both emulators were checked and neither is proven compatible with this game's files.");
            }
        }
        if let Some(stored) = &self.stored
            && matches!(
                view,
                View::Recommended { .. } | View::Insufficient | View::Conflicting
            )
        {
            let report = &stored.report;
            for reason in &report.recommendation.reasons {
                ui.label(reason);
            }
            for warning in &report.recommendation.warnings {
                ui.label(format!("Warning: {warning}"));
            }
            for limit in &report.limits {
                ui.weak(limit);
            }
            if !report.details.is_empty() {
                ui.collapsing("Details", |ui| {
                    for line in &report.details {
                        ui.label(line);
                    }
                });
            }
            let p = &report.provenance;
            ui.weak(format!(
                    "Checked {} archive(s){}. Valid while those files, the catalogues and the evidence above are unchanged.",
                    p.archives.len(),
                    p.mame_catalogue
                        .as_ref()
                        .map(|(_, sha)| format!("; MAME catalogue sha256 {}", &sha[..sha.len().min(12)]))
                        .unwrap_or_default(),
                ));
        }
    }

    pub(super) fn view(&self) -> View {
        if self.running.is_some() {
            return View::Checking;
        }
        if let Some(reason) = self.stale_reason {
            return View::Stale(reason);
        }
        if let Some(error) = &self.error {
            return View::Failed(error.clone());
        }
        self.stored
            .as_ref()
            .map_or(View::NoEvidenceYet, |s| view_of(&s.report))
    }

    fn start(&mut self, inputs: Inputs) {
        self.error = None;
        self.stale_reason = None;
        self.stored = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let request = inputs.request();
        let worker_cancel = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let _ = sender.send(run_arcade_compat_check(&request, &worker_cancel));
        });
        self.running = Some(Running {
            inputs,
            cancel,
            receiver,
        });
    }

    fn poll(&mut self) {
        let Some(running) = self.running.as_ref() else {
            return;
        };
        let outcome = match running.receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(ArcadeCheckError::Archive(
                "The check stopped before producing a result.".into(),
            )),
        };
        let Some(running) = self.running.take() else {
            return;
        };
        match outcome {
            Ok(report) => {
                self.stored = Some(Stored {
                    inputs: running.inputs.clone(),
                    report: Arc::new(report),
                });
            }
            Err(ArcadeCheckError::Cancelled) => {}
            Err(error) => self.error = Some(error.to_string()),
        }
        self.next_freshness = None;
    }

    /// Discard a result whose evidence moved. Stat-only, at most every 2 s.
    fn check_freshness(&mut self, current: Option<&Inputs>) {
        if current.is_none() {
            self.running = None;
        }
        let Some(stored) = &self.stored else { return };
        let reason = match current {
            Some(c) if *c != stored.inputs => Some("the catalogue or entered evidence changed"),
            None => Some("this game's identity is no longer verified"),
            _ if self.next_freshness.is_some_and(|t| Instant::now() < t) => None,
            _ if !stored.report.provenance.is_current() => {
                Some("the archive, a catalogue or an emulator file changed on disk")
            }
            _ => {
                self.next_freshness = Some(Instant::now() + FRESHNESS_INTERVAL);
                None
            }
        };
        if let Some(reason) = reason {
            self.stored = None;
            self.stale_reason = Some(reason);
        } else if current.is_some() {
            self.stale_reason = None;
        }
    }
}

#[cfg(test)]
#[path = "arcade_compat_panel_tests.rs"]
mod tests;
