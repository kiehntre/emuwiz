//! Feature-owned, read-only firmware guidance. Selection remains Launch's.
use super::*;
use archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord;
use archivefs_core::identity_source::hashing::FileFingerprint;
use archivefs_core::{
    game_firmware_adapters::AdapterFirmwareContext,
    game_firmware_readiness::*,
    launch::{LaunchCandidate, LaunchTarget},
    patch_manager::*,
};
use std::time::{Duration, Instant};
#[path = "game_firmware_inspection.rs"]
mod inspection;
#[derive(Clone, Debug)]
pub(super) enum Source {
    Pcsx2(Pcsx2Profile, Vec<FirmwareIdentityRecord>),
    DuckStation(
        DuckStationProfile,
        Vec<FirmwareIdentityRecord>,
        Option<String>,
    ),
    Flycast(FlycastProfile),
    RetroArch(
        archivefs_core::emulator_environment::retroarch::CoreInfoFinding,
        Option<PathBuf>,
    ),
    Unavailable(String),
}
#[derive(Clone, Debug)]
pub(super) struct Request {
    context: AdapterFirmwareContext,
    candidate: LaunchCandidate,
    game_path: PathBuf,
    game_fingerprint: Option<FileFingerprint>,
    source: Source,
}
struct Observed {
    readiness: GameFirmwareReadiness,
    checked_at: Instant,
}
struct Job {
    key: String,
    receiver: Receiver<std::result::Result<Observed, String>>,
}
#[derive(Default)]
pub(super) struct State {
    key: String,
    job: Option<Job>,
    result: Option<GameFirmwareReadiness>,
    completed: Option<Instant>,
    error: Option<String>,
}
impl State {
    fn select(&mut self, key: String) {
        if self.key != key {
            self.key = key;
            self.result = None;
            self.completed = None;
            self.error = None;
        }
    }
    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let result=match job.receiver.try_recv(){Ok(r)=>r,Err(TryRecvError::Empty)=>return,Err(TryRecvError::Disconnected)=>Err("Firmware inspection stopped without a result. No readiness conclusion is available; recheck.".into())};
        let same = job.key == self.key;
        self.job = None;
        if !same {
            return;
        }
        match result {
            Ok(r) => {
                self.completed = Some(r.checked_at);
                self.result = Some(r.readiness);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }
    fn start(&mut self, ctx: egui::Context, request: Request) {
        if self.job.is_some() {
            return;
        }
        self.result = None;
        self.completed = None;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job {
            key: self.key.clone(),
            receiver: rx,
        });
        std::thread::spawn(move || {
            let result = inspection::inspect(request).map(|readiness| Observed {
                readiness,
                checked_at: Instant::now(),
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn displayed_state(&self) -> Option<GameFirmwareState> {
        self.result.as_ref().map(|r| {
            if self
                .completed
                .is_none_or(|at| at.elapsed() > Duration::from_secs(30))
            {
                GameFirmwareState::Stale
            } else {
                r.state.clone()
            }
        })
    }
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        request: Option<Request>,
        reason: &str,
        inventory: Option<&archivefs_core::bios_projection::BiosMasterInventory>,
    ) {
        let key = request
            .as_ref()
            .map(|r| format!("{r:?}"))
            .unwrap_or_else(|| reason.to_string());
        self.select(key);
        self.poll();
        ui.collapsing("BIOS / firmware evidence for this game",|ui|{
            ui.small("Read-only guidance for the selected emulator/profile. No downloads, settings changes or emulator execution. Firmware evidence does not prove that a game will run and does not change Launch decisions.");
            ui.small(inventory_note(inventory));
            let Some(request)=request else{ui.label(format!("Unknown: {reason}"));return;};
            ui.label(format!("Assessed target: {:?}",request.candidate.target));
            if let Source::Unavailable(reason)=&request.source{ui.label(format!("Unknown: {reason}"));return;}
            if ui.add_enabled(self.job.is_none(),egui::Button::new("Check BIOS / firmware evidence")).clicked(){self.start(ui.ctx().clone(),request.clone());}
            if self.job.is_some(){ui.label("Inspecting selected profile and firmware on a worker…");}
            if let Some(e)=&self.error{ui.label(format!("Unknown: {e}"));}
            if let Some(r)=&self.result {
                let expired=self.completed.is_none_or(|at|at.elapsed()>Duration::from_secs(30));
                ui.label(format!("Required firmware: {}",r.requirement.label));
                ui.strong(format!("Firmware assessment: {:?}",self.displayed_state().unwrap_or(GameFirmwareState::Unknown)));
                if expired{ui.label("This observed snapshot expired. Recheck before relying on its evidence.");}else{ui.small("Observed snapshot only. Recheck after changing files or settings; Launch performs its own checks.");ui.ctx().request_repaint_after(Duration::from_secs(1));}
                ui.label(format!("Evidence freshness: {:?}",if expired { FirmwareFreshnessState::Stale } else { r.freshness.state }));
                for item in r.evidence.iter().take(12){
                    if let Some(path)=&item.path{ui.label(path.display().to_string());}
                    ui.label(item.provenance.chars().take(1800).collect::<String>());
                    if item.region.is_some()||item.version.is_some(){ui.label(format!("Observed ROM region: {}; version: {} (not a game requirement)",item.region.as_deref().unwrap_or("unknown"),item.version.as_deref().unwrap_or("unknown")));}
                    if let Some(sha)=&item.sha256{ui.small(format!("Observed SHA-256: {sha}"));}
                }
            }
        });
    }
}
impl NativeWorkflows {
    pub(super) fn show_game_firmware_guidance(
        &mut self,
        ui: &mut egui::Ui,
        game_id: i64,
        path: &Path,
        input: &launch_readiness_page::LaunchReadinessInput,
    ) {
        let launch_readiness_page::LaunchReadinessInput::Plan {
            plan,
            pcsx2,
            duckstation,
            ..
        } = input
        else {
            return;
        };
        let Some(platform) = plan
            .platform_id
            .as_deref()
            .filter(|p| matches!(*p, "PS2" | "PSX" | "Dreamcast" | "PC Engine" | "AtariST"))
        else {
            return;
        };
        let Some(candidate) = launch_readiness_summary::selected_candidate(plan) else {
            self.firmware_guidance.show(ui,None,"No unambiguous emulator/profile is selected. Use the existing emulator selection controls; a global BIOS inventory is not selected-profile evidence.",self.app.emulator_readiness.bios_projection_page.inventory());
            return;
        };
        let context = AdapterFirmwareContext {
            game_id: Some(game_id.to_string()),
            platform: platform.into(),
            release_identity: plan.game_key.clone(),
            ..Default::default()
        };
        let game_fingerprint = match &self.app.selected_evidence_ui.selected_evidence {
            selected_evidence_page::SelectedEvidenceState::Ready { report, .. }
                if report.path == path =>
            {
                report.hashes.as_ref().map(|h| h.fingerprint.clone())
            }
            _ => None,
        };
        let source = match &candidate.target {
            LaunchTarget::Standalone {
                adapter_id,
                profile_id,
                profile_path,
            } => {
                match *adapter_id {
                    "pcsx2"=>pcsx2.as_ref().and_then(|c|unique_profile(&c.discovery.profiles,|p|&p.profile_id==profile_id && profile_path.as_ref()==Some(&p.configuration_path))).map(|p|Source::Pcsx2(p.clone(),pcsx2.as_ref().unwrap().firmware_evidence.clone())),
                    "duckstation"=>duckstation.as_ref().and_then(|c|unique_profile(&c.discovery.profiles,|p|&p.profile_id==profile_id && profile_path.as_ref()==Some(&p.configuration_path))).map(|p|Source::DuckStation(p.clone(),duckstation.as_ref().unwrap().firmware_evidence.clone(),duckstation.as_ref().unwrap().verified_ps1_serial.clone())),
                    "flycast"=>match &self.app.emulator_readiness.flycast_profiles{FlycastProfilesState::Ready(r)=>unique_profile(&r.discovery.profiles,|p|&p.profile_id==profile_id && profile_path.as_ref()==Some(&p.configuration_path)).map(|p|Source::Flycast(p.clone())),_=>None},
                    // Hatari is not currently contributed by Blue's Launch
                    // bridge. Never discover/select another profile to fill
                    // that gap or turn this guidance into a selection policy.
                    _=>None,
                }.unwrap_or_else(||Source::Unavailable(format!("No current firmware evidence is bound to this selected {adapter_id} profile. {}",required_label(platform))))
            }
            LaunchTarget::RetroArchCore { profile, core_stem, .. } => {
                match &self.app.emulator_readiness.retroarch_profiles {
                    RetroArchProfilesState::Ready(discovery) => {
                        unique_profile(&discovery.environment.profiles, |p| p.profile_kind==profile.profile_kind && p.scope==profile.scope)
                        .and_then(|p| unique_profile(&p.cores, |core| &core.core_stem==core_stem).map(|core| {
                            use archivefs_core::emulator_environment::retroarch::{PathPurpose,ResolutionState};
                            let system=unique_profile(&p.paths, |v|v.purpose==PathPurpose::System && v.resolution==ResolutionState::ConfiguredResolved)
                                .and_then(|v|v.resolved_path.as_ref()).filter(|v|!v.lossy).map(|v|PathBuf::from(&v.display));
                            Source::RetroArch(core.info.clone(),system)
                        }))
                        .unwrap_or_else(||Source::Unavailable("No unique selected core/profile metadata is available. Firmware policy is unknown.".into()))
                    },
                    _=>Source::Unavailable("Selected core/profile discovery has not completed. Firmware policy is unknown.".into())
                }
            }
        };
        self.firmware_guidance.show(
            ui,
            Some(Request {
                context,
                candidate: candidate.clone(),
                game_path: path.into(),
                game_fingerprint,
                source,
            }),
            "",
            self.app.emulator_readiness.bios_projection_page.inventory(),
        );
    }
}
fn inventory_note(
    inventory: Option<&archivefs_core::bios_projection::BiosMasterInventory>,
) -> String {
    match inventory{Some(i)=>format!("Existing global BIOS inventory: {} files, {} warnings. Pool presence is not proof that this profile is configured to use any of them.",i.entries.len(),i.warnings.len()),None=>"Global BIOS inventory has not been inspected. This guidance uses only the selected adapter’s existing verifier evidence.".into()}
}
fn unique_profile<T>(profiles: &[T], accept: impl Fn(&T) -> bool) -> Option<&T> {
    let mut it = profiles.iter().filter(|p| accept(p));
    let one = it.next()?;
    it.next().is_none().then_some(one)
}
fn required_label(platform: &str) -> &'static str {
    match platform {
        "PS2" => "PCSX2 requires a PS2 BIOS.",
        "PSX" => "DuckStation requires a PS1 BIOS for the resolved slot.",
        "Dreamcast" => "Dreamcast firmware requires both BIOS and flash.",
        "AtariST" => {
            "Hatari requires a TOS ROM; per-game machine/version compatibility is unknown."
        }
        "PC Engine" => {
            "For CD media, the System Card requirement depends on the selected emulator and title. Current policy is unknown; HuCard media must not be treated as CD."
        }
        _ => "Firmware requirement is unknown.",
    }
}
#[cfg(test)]
#[path = "game_firmware_guidance/tests.rs"]
mod tests;
