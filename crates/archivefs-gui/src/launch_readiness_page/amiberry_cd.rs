//! Blue configured-profile CD32/CDTV launch; no profile rewriting.
use super::{
    LaunchPlan, StandaloneLaunchRequest, StandaloneLaunchState, preview_identity,
    show_standalone_launch_action,
};
use archivefs_core::launch::amiberry_cd_profile::{
    ReviewedAmiberryCdProfile, review_amiberry_cd_profile,
};
use archivefs_core::launch::planning::CanonicalIdentityStatus;
use archivefs_core::patch_manager::{
    AmigaEmulatorKind, AmigaInstallationType, AmigaProfileDiscoveryRoots, discover_amiga_profiles,
};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
#[derive(Debug, Clone)]
pub(super) struct Request {
    reviewed: ReviewedAmiberryCdProfile,
}
impl Request {
    pub(super) fn key(&self) -> (PathBuf, String) {
        (
            self.reviewed.request.selected_content.path.clone(),
            "amiberry_cd".into(),
        )
    }
    pub(super) fn execute(
        self,
    ) -> Result<archivefs_core::launch::LaunchedAmiberryCdProcess, String> {
        let request = &self.reviewed.request;
        let identity = super::native_cd_identity::verify(&request.selected_content.path)?;
        self.reviewed.launch(&identity)
    }
}
type Results = Vec<(String, Result<Request, String>)>;
#[derive(Default)]
pub(super) struct Panel {
    focused: Option<PathBuf>,
    key: Option<(Option<PathBuf>, CanonicalIdentityStatus)>,
    pending: Option<Receiver<Results>>,
    results: Results,
    chosen: Option<String>,
}
impl Panel {
    pub(super) fn focus(&mut self, path: Option<&Path>) {
        self.focused = path.map(Path::to_path_buf);
    }
}
fn inspect(
    selected: Option<PathBuf>,
    identity: CanonicalIdentityStatus,
    roots: Option<AmigaProfileDiscoveryRoots>,
) -> Results {
    let Some(roots) = roots else {
        return vec![("Amiberry".into(),Err("No user configuration root is available. Configure your native Amiberry installation, then check again.".into()))];
    };
    let verified = match selected
        .as_deref()
        .ok_or("Select a direct CD ISO first.".into())
        .and_then(super::native_cd_identity::verify)
    {
        Ok(verified)
            if matches!(identity, CanonicalIdentityStatus::Unknown) || identity == verified =>
        {
            verified
        }
        Ok(_) => {
            return vec![(
                "CD identity".into(),
                Err("Catalogue identity changed. Refresh readiness.".into()),
            )];
        }
        Err(reason) => return vec![("CD identity".into(), Err(reason))],
    };
    if !matches!(&verified,CanonicalIdentityStatus::Resolved(i) if matches!(i.platform_id.as_str(),"AmigaCD32"|"Commodore CDTV"))
    {
        return vec![(
            "CD identity".into(),
            Err("The catalogue does not identify this ISO as CD32 or CDTV.".into()),
        )];
    }
    let profiles = discover_amiga_profiles(&roots).profiles;
    let mut results = Vec::new();
    for profile in profiles
        .into_iter()
        .filter(|p| p.emulator == AmigaEmulatorKind::Amiberry)
    {
        if !profile.eligible || profile.installation_type == AmigaInstallationType::FlatpakUser {
            results.push((profile.profile_id,Err("This Amiberry installation is unavailable or needs a Flatpak launch contract. Blue's CD route supports native configured profiles.".into())));
            continue;
        }
        let mut configs = profile.profile_paths.clone();
        configs.extend(profile.global_config_path.clone());
        configs.sort();
        configs.dedup();
        if configs.is_empty() {
            results.push((profile.profile_id.clone(),Err("No existing Amiberry configuration was found. Supply a configured CD32/CDTV UAE profile yourself, then check again.".into())));
        }
        if profile.executable_candidates.is_empty() {
            results.push((profile.profile_id.clone(),Err("No native Amiberry executable was found. Configure your existing installation and check again; EmuWiz does not install or download it.".into())));
        }
        for exe in &profile.executable_candidates {
            if exe.installation_type == AmigaInstallationType::FlatpakUser {
                continue;
            }
            for config in &configs {
                let label = format!("{} · {}", exe.path.display(), config.display());
                let result = selected
                    .as_deref()
                    .ok_or("Select a CD32/CDTV disc before checking profiles.".into())
                    .and_then(|selected| {
                        review_amiberry_cd_profile(&exe.path, config, selected, &verified)
                    })
                    .map(|reviewed| Request { reviewed });
                results.push((label, result));
            }
        }
    }
    if results.is_empty() {
        results.push(("Amiberry".into(),Err("No configured native Amiberry profile is available. Supply Amiberry and an existing CD32/CDTV UAE profile in its configuration directory, then check again. Required Kickstart and extended ROM files must be supplied by you.".into())));
    }
    results
}
pub(super) fn show_unresolved(ui: &mut egui::Ui, state: &mut StandaloneLaunchState) {
    let plan = LaunchPlan {
        platform_id: None,
        game_key: None,
        candidates: Vec::new(),
        summary: Default::default(),
        media_topology: None,
    };
    show(ui, &plan, state);
}
pub(super) fn show(ui: &mut egui::Ui, plan: &LaunchPlan, state: &mut StandaloneLaunchState) {
    if !matches!(
        plan.platform_id.as_deref(),
        Some("AmigaCD32" | "Commodore CDTV")
    ) && !(plan.platform_id.is_none()
        && state
            .amiberry_cd
            .focused
            .as_ref()
            .is_some_and(|p| p.extension().is_some_and(|s| s.eq_ignore_ascii_case("iso"))))
    {
        return;
    }
    let selected = state.amiberry_cd.focused.clone().or_else(|| {
        plan.candidates
            .iter()
            .find_map(|c| c.content.resolved_path.clone())
    });
    let feedback_path = selected.clone();
    let identity = preview_identity(plan);
    let key = (selected.clone(), identity.clone());
    if state.amiberry_cd.key.as_ref() != Some(&key) {
        state.amiberry_cd = Panel {
            focused: selected.clone(),
            key: Some(key),
            ..Default::default()
        };
    }
    let active = state.is_active();
    let mut launch = None;
    ui.group(|ui| {
        ui.strong(format!("Amiberry · {}", plan.platform_id.as_deref().unwrap_or("CD")));
        ui.small("Uses your existing native profile without changing it. Blue requires a direct ISO with a matching registered DAT. The exact machine, disc and firmware are reviewed. Firmware presence is not authenticity; the emulator runs with your account permissions.");
        if ui.add_enabled(!active && state.amiberry_cd.pending.is_none(), egui::Button::new("Check configured Amiberry CD profiles")).clicked() {
            let (tx, rx) = mpsc::channel();
            state.amiberry_cd.results.clear();
            state.amiberry_cd.chosen = None;
            state.amiberry_cd.pending = Some(rx);
            std::thread::spawn(move || {
                let _ = tx.send(inspect(selected, identity, AmigaProfileDiscoveryRoots::from_environment()));
            });
        }
        if let Some(rx) = &state.amiberry_cd.pending {
            match rx.try_recv() {
                Ok(results) => {
                    state.amiberry_cd.results = results;
                    state.amiberry_cd.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    state.amiberry_cd.results = vec![("Amiberry".into(), Err("Profile check ended unexpectedly. Check again.".into()))];
                    state.amiberry_cd.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.label("Checking…");
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        egui::ScrollArea::vertical().id_salt("amiberry-cd-profile-results").max_height(180.).show(ui, |ui| {
            for (label, result) in &state.amiberry_cd.results {
                ui.small(label);
                match result {
                    Err(reason) => { ui.label(reason); }
                    Ok(request) => {
                        ui.small(format!("Reviewed {} · ISO: {}", request.reviewed.request.machine_model, request.reviewed.request.selected_content.path.display()));
                        if ui.add_enabled(!active, egui::Button::new("Use this configured CD profile")).clicked() {
                            state.amiberry_cd.chosen = Some(label.clone());
                        }
                        if state.amiberry_cd.chosen.as_ref() == Some(label) {
                            launch = Some(request.clone());
                        }
                    }
                }
            }
        });
    });
    if let Some(request) = launch {
        if state.is_active()
            && !state.tracked.as_ref().is_some_and(|(p, a, _)| {
                p == &request.reviewed.request.selected_content.path && a == "amiberry_cd"
            })
        {
            ui.label("Another standalone launch is active; wait before starting Amiberry.");
            return;
        }
        show_standalone_launch_action(ui, state, StandaloneLaunchRequest::AmiberryCd(request));
    } else {
        super::native_cd_feedback::show(
            ui,
            state,
            feedback_path.as_deref(),
            "amiberry_cd",
            "Amiberry CD",
            |process| {
                if let super::StandaloneProcess::AmiberryCd(p) = process {
                    let pid = p.pid;
                    Some((
                        pid,
                        p.poll()
                            .map(|report| super::exit_line("Amiberry CD", report)),
                    ))
                } else {
                    None
                }
            },
        );
    }
}
#[cfg(test)]
#[path = "amiberry_cd_tests.rs"]
mod tests;
