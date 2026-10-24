//! Read-only Atari ST media and exact-profile readiness. No launch authority.
use archivefs_core::{
    disk_format::{self, DiskFormat, DiskFormatContext},
    game_firmware_adapters::{self, AdapterFirmwareContext},
    identity_source::hashing::FileFingerprint,
    launch::{self, CanonicalIdentityStatus, LaunchContentRef, LaunchPlan, LaunchTarget},
    patch_manager::{self, HatariGameInspection, HatariProfile, HatariProfileDiscoveryRoots},
    safe_read::{TrustedRoots, open_bounded_read},
};
use eframe::egui;
use std::{
    io::Read,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};
pub(crate) const RUNTIME_BLOCKER: &str = "Native Hatari launch is not authorised: --configfile adds to previously loaded settings. The current backend does not isolate inherited storage, automatic drive-B insertion or media writeback. No emulator was launched and no disk was modified.";
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub path: PathBuf,
    pub source_root: PathBuf,
    pub platform: Option<String>,
}
#[derive(Clone)]
struct Profile {
    profile: HatariProfile,
    inspection: HatariGameInspection,
    command: Option<launch::HatariCommand>,
    refusal: Option<String>,
}
#[derive(Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) selection: Option<Selection>,
    key: Option<String>,
    media: String,
    profiles: Vec<Profile>,
    error: Option<String>,
    checked: Option<Instant>,
}
impl Snapshot {
    fn pending(selection: Selection) -> Self {
        Self {
            selection: Some(selection),
            error: Some("Checking Atari ST media and existing Hatari profiles…".into()),
            ..Default::default()
        }
    }
    pub(crate) fn reason(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if self.profiles.is_empty() {
            return "No existing Hatari profile found. Configure Hatari and TOS in Emulator Setup, then re-check. Native launch remains unavailable.".into();
        }
        if self.profiles.len() == 1
            && let Some(refusal) = &self.profiles[0].refusal
        {
            return format!("Hatari preflight refused: {refusal}. {RUNTIME_BLOCKER}");
        }
        RUNTIME_BLOCKER.into()
    }
    pub(crate) fn plan(&self) -> LaunchPlan {
        let identity = self
            .key
            .as_ref()
            .map(|key| {
                CanonicalIdentityStatus::Resolved(launch::ResolvedIdentity {
                    platform_id: "AtariST".into(),
                    game_key: key.clone(),
                })
            })
            .unwrap_or(CanonicalIdentityStatus::Unknown);
        let content=LaunchContentRef {kind:Some(launch::LaunchContentKind::Unknown),container:Some(launch::LaunchContainerKind::PlainFile),resolved_path:self.selection.as_ref().map(|s|s.path.clone()),requires_mount:false,provenance:"Read-only structural Atari ST readiness, not verified release identity or launch authority".into()};
        let inputs: Vec<_> = self
            .profiles
            .iter()
            .map(|p| launch::StandaloneProfileInput {
                adapter_id: "hatari",
                profile_id: p.profile.profile_id.clone(),
                profile_path: Some(p.profile.config_path.clone()),
                eligible: p.profile.eligible,
                firmware: launch::hatari_firmware_readiness(p.inspection.health.tos.health),
            })
            .collect();
        let env = archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
            format_version: 2,
            profiles: vec![],
            diagnostics: vec![],
        };
        let mut plan = launch::build_launch_plan(&identity, &content, &inputs, &env, &[]);
        for candidate in &mut plan.candidates {
            candidate.blockers.insert(
                0,
                launch::LaunchBlocker::new(
                    launch::LaunchBlockerKind::HatariProfileUnavailable,
                    self.reason(),
                ),
            );
            candidate.readiness = launch::LaunchReadiness::Blocked;
        }
        plan.summary = launch::LaunchPlanSummary {
            candidates: plan.candidates.len(),
            blocked: plan.candidates.len(),
            ..Default::default()
        };
        plan
    }
    pub(crate) fn details(&self) -> Vec<String> {
        let mut details = vec![self.media.clone()];
        for p in &self.profiles {
            details.push(format!("Profile {}: machine {:?}; TOS {:?}. Filename/presence is not verified firmware or game compatibility.", p.profile.config_path.display(), p.inspection.config.machine.model, p.inspection.health.tos.health));
            if let Some(reason) = &p.refusal {
                details.push(format!("Preflight refused: {reason}"));
            }
        }
        if self.profiles.len() > 1 {
            details.push("Multiple profiles found; none selected for execution.".into());
        }
        details
    }
    pub(crate) fn show(&self, ui: &mut egui::Ui) {
        crate::ui::components::card(ui, |ui| {
            ui.strong("Atari ST / Hatari readiness");
            ui.label(self.reason());
            ui.label(&self.media);
            ui.label("Structural platform/representation evidence and a local content hash do not verify the game's catalogue identity or prove that it will run. No disk set is inferred or loaded from filenames.");
            if self
                .checked
                .is_some_and(|t| t.elapsed() > Duration::from_secs(30))
            {
                ui.label("Readiness snapshot expired; reopen/re-check Game Details. This card never authorises Launch.");
            }
            if self.profiles.is_empty() && self.checked.is_some() {
                ui.label("No existing Hatari profile found. Configure Hatari/TOS in Emulator Setup, then re-check.");
            }
            if self.profiles.len() > 1 {
                ui.label("Multiple Hatari profiles: none has been selected for execution. These are separate setup observations; no profile is substituted or chosen automatically.");
            }
            let plan = self.plan();
            for p in &self.profiles {
                ui.separator();
                ui.label(format!("Profile: {}", p.profile.config_path.display()));
                ui.label(format!(
                    "Machine: {:?} · TOS: {:?}",
                    p.inspection.config.machine.model, p.inspection.health.tos.health
                ));
                if let Some(path) = &p.inspection.health.tos.path {
                    ui.label(format!("Configured TOS: {}", path.display()));
                }
                crate::launch_readiness_page::controller_readiness::hatari_report(
                    &p.inspection.config,
                )
                .show(ui);
                if let Some(candidate)=plan.candidates.iter().find(|c|matches!(&c.target,LaunchTarget::Standalone {adapter_id:"hatari",profile_id,..} if profile_id==&p.profile.profile_id)) {
                    let guidance=game_firmware_adapters::hatari(&AdapterFirmwareContext {platform:"AtariST".into(),..Default::default()},candidate,&p.profile,&p.inspection.health);
                    ui.label(format!("Existing firmware guidance: {:?}. TOS filename/presence does not establish its identity or this game's machine/version compatibility.",guidance.state));
                }
                if let Some(reason) = &p.refusal {
                    ui.label(format!("Existing preflight refused: {reason}"));
                }
                if let Some(command) = &p.command {
                    ui.label("Existing backend command preview ONLY — NOT authorised for execution. Other inherited settings are not represented by these arguments.");
                    ui.monospace(format!("{:?} {:?}", command.executable, command.arguments));
                }
            }
        });
    }
}
fn inspect_media(selection: &Selection) -> Result<(String, String), String> {
    use sha2::{Digest, Sha256};
    if !selection.path.is_absolute()
        || !matches!(
            selection
                .path
                .extension()
                .and_then(|x| x.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("st" | "stx")
        )
    {
        return Err("This readiness slice validates ST and STX only. MSA, IPF/CAPS and other representations have no new decoding or launch support.".into());
    }
    if selection
        .platform
        .as_deref()
        .and_then(archivefs_core::canonical_platform_for_alias)
        .is_some_and(|p| p != "AtariST")
    {
        return Err(
            "Existing platform context conflicts with Atari ST; no identity was replaced.".into(),
        );
    }
    let before = FileFingerprint::observe(&selection.path).ok_or("Media is unavailable")?;
    if before.size_bytes > disk_format::MAX_PASTI_BYTES {
        return Err("Selected media exceeds the 32 MiB readiness bound".into());
    }
    let evidence = disk_format::inspect_disk_format(
        &selection.path,
        &TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    if !matches!(
        evidence.format,
        Some(DiskFormat::AtariStRawFloppy | DiskFormat::AtariStPasti)
    ) {
        return Err(format!(
            "ST/STX structure did not validate: {:?}",
            evidence.refusal
        ));
    }
    let mut request =
        archivefs_core::platform::DetectionRequest::new(&selection.path, &selection.source_root);
    request.read_signatures = true;
    let platform = archivefs_core::platform::detect_platform_report(&request);
    if platform.platform != Some("AtariST")
        || platform.confidence != archivefs_core::platform::DetectionConfidence::Confirmed
    {
        return Err("Raw ST/FAT12 structure is shared with DOS. Atari ST source corroboration is required; a filename or checksum alone cannot settle the platform.".into());
    }
    let file = open_bounded_read(&selection.path, &TrustedRoots::none())
        .map_err(|e| e.detail())?
        .into_file();
    let path_identity = std::fs::metadata(&selection.path).map_err(|e| e.to_string())?;
    let capture = launch::process_spawn::CapturedFileIdentity::capture;
    if capture(&file.metadata().map_err(|e| e.to_string())?) != capture(&path_identity) {
        return Err("Media changed while opening; re-check".into());
    }
    let mut file = file.take(disk_format::MAX_PASTI_BYTES + 1);
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > disk_format::MAX_PASTI_BYTES {
            return Err("Media grew beyond the read bound".into());
        }
        hash.update(&chunk[..n]);
    }
    if FileFingerprint::observe(&selection.path).as_ref() != Some(&before)
        || total != before.size_bytes
    {
        return Err("Media changed while inspected; re-check".into());
    }
    let mut detail = format!(
        "{}: {} bytes structurally inspected. Payload was not decoded or reconstructed.",
        evidence.format.unwrap().label(),
        evidence.bytes_inspected
    );
    let name = selection
        .path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let naming = archivefs_core::media_set::filename_evidence(
        &name,
        Some(archivefs_core::media_set::MediaFamily::Floppy),
    );
    if naming.ordinal.is_some()
        || naming.expected_count.is_some()
        || naming.side.is_some()
        || naming.role.is_some()
    {
        detail.push_str(" Filename suggests a disk/side/role relationship, but membership, order and completeness are unverified. The preview names only this selected disk in drive A; it is not a multi-disk plan.");
    }
    Ok((
        format!(
            "media-sha256:{}",
            hash.finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ),
        detail,
    ))
}
pub(crate) fn gather(selection: Selection, roots: HatariProfileDiscoveryRoots) -> Snapshot {
    let mut result = Snapshot {
        selection: Some(selection.clone()),
        checked: Some(Instant::now()),
        ..Default::default()
    };
    let (key, media) = match inspect_media(&selection) {
        Ok(v) => v,
        Err(e) => {
            result.error = Some(e);
            return result;
        }
    };
    result.key = Some(key.clone());
    result.media = media;
    let discovery = patch_manager::discover_hatari_profiles(&roots);
    if discovery.profiles.len() > 8 {
        result.error=Some("Hatari profile count exceeds the eight-profile readiness bound; no complete setup conclusion.".into());
        return result;
    }
    for profile in discovery.profiles {
        let config_before = FileFingerprint::observe(&profile.config_path);
        let inspection = patch_manager::inspect_hatari_game(
            &profile,
            &patch_manager::HatariSelectedGameRequest {
                canonical_platform: Some("AtariST".into()),
                identity_state: patch_manager::HatariIdentityState::Unresolved,
                verified_title: None,
            },
            &[],
        );
        // This opaque key identifies the read bytes for a proposed command;
        // no VerifiedIdentityFact, release title or persisted identity is minted.
        let command = profile
            .executable_candidates
            .as_slice()
            .first()
            .ok_or_else(|| "No Hatari executable discovered".into())
            .and_then(|exe| {
                launch::preflight_hatari_launch(
                    &launch::HatariLaunchRequest {
                        selected_content_path: selection.path.clone(),
                        expected_platform_id: "AtariST".into(),
                        profile_id: profile.profile_id.clone(),
                        expected_executable: exe.path.clone(),
                        expected_config_path: profile.config_path.clone(),
                        expected_machine_model: inspection.config.machine.model,
                        disk_drive: 'A',
                        ipf_backend_available: false,
                        expected_content_identity: std::fs::symlink_metadata(&selection.path)
                            .ok()
                            .map(|m| launch::process_spawn::CapturedFileIdentity::capture(&m)),
                        expected_config_identity: std::fs::symlink_metadata(&profile.config_path)
                            .ok()
                            .map(|m| launch::process_spawn::CapturedFileIdentity::capture(&m)),
                        tos_references: vec![],
                    },
                    &roots,
                    &CanonicalIdentityStatus::Resolved(launch::ResolvedIdentity {
                        platform_id: "AtariST".into(),
                        game_key: key.clone(),
                    }),
                )
                .map_err(|e| format!("{:?}: {}", e.kind, e.detail))
                .and_then(|command| {
                    if config_before.is_none()
                        || FileFingerprint::observe(&profile.config_path) != config_before
                        || Some(&command.selection.tos_path) != inspection.health.tos.path.as_ref()
                        || command.selection.machine_model != inspection.config.machine.model
                    {
                        Err("Profile changed during inspection; re-check".into())
                    } else {
                        Ok(command)
                    }
                })
            });
        let (command, refusal) = match command {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(e)),
        };
        result.profiles.push(Profile {
            profile,
            inspection,
            command,
            refusal,
        });
    }
    result
}
#[derive(Default)]
pub(crate) struct Controller {
    selection: Option<Selection>,
    pending: Option<(Selection, Receiver<Snapshot>)>,
    current: Snapshot,
    checked: Option<Instant>,
    repaint: Option<egui::Context>,
}
impl Controller {
    pub(crate) fn set_repaint_context(&mut self, ctx: &egui::Context) {
        self.repaint = Some(ctx.clone());
    }
    pub(crate) fn snapshot(state: &Arc<Mutex<Self>>, selection: Selection) -> Snapshot {
        let Ok(mut state) = state.lock() else {
            return Snapshot {
                error: Some("Hatari readiness worker is unavailable".into()),
                ..Default::default()
            };
        };
        if state.selection.as_ref() != Some(&selection) {
            state.selection = Some(selection.clone());
            state.current = Snapshot::pending(selection.clone());
            state.checked = None;
        }
        if let Some((job, rx)) = state.pending.take() {
            match rx.try_recv() {
                Ok(value) if job == selection => {
                    state.current = value;
                    state.checked = Some(Instant::now());
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => state.pending = Some((job, rx)),
                Err(TryRecvError::Disconnected) => {
                    state.current.error = Some("Hatari inspection stopped; re-check".into());
                    state.checked = Some(Instant::now());
                }
            }
        }
        if state.pending.is_none()
            && state
                .checked
                .is_none_or(|at| at.elapsed() > Duration::from_secs(5))
        {
            let (tx, rx) = mpsc::channel();
            state.pending = Some((selection.clone(), rx));
            let repaint = state.repaint.clone();
            std::thread::spawn(move || {
                let snapshot = patch_manager::HatariProfileDiscoveryRoots::from_environment()
                    .map(|roots| gather(selection, roots))
                    .unwrap_or_else(|e| Snapshot {
                        error: Some(e.to_string()),
                        ..Default::default()
                    });
                let _ = tx.send(snapshot);
                if let Some(ctx) = repaint {
                    ctx.request_repaint();
                }
            });
        }
        state.current.clone()
    }
}
#[cfg(test)]
mod tests;
