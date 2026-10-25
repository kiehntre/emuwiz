//! Read-only NES/FDS readiness. Native dispatch deliberately remains blocked
//! until executable generation and Mesen's typed settings contract are bound.
use archivefs_core::{
    launch,
    patch_manager::{self, MesenProfileDiscoveryRoots},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};
pub(crate) const NATIVE_CONTRACT_BLOCKER: &str = "Native Mesen launch is unavailable: automatic discovery does not establish a supported Mesen 2 executable or validate its complete typed settings. Single-instance forwarding could use a different BIOS directory. No emulator was launched. No settings or media were changed.";
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub path: PathBuf,
    pub identity: launch::CanonicalIdentityStatus,
}
#[derive(Clone)]
struct Profile {
    input: launch::StandaloneProfileInput,
    detail: String,
}
#[derive(Clone, Default)]
pub(crate) struct Snapshot {
    profiles: Vec<Profile>,
}
impl Snapshot {
    fn unavailable(detail: &str) -> Self {
        Self {
            profiles: vec![Profile {
                input: launch::StandaloneProfileInput {
                    adapter_id: "mesen",
                    profile_id: "mesen:unavailable".into(),
                    profile_path: None,
                    eligible: false,
                    firmware: launch::FirmwareReadiness::Unknown,
                },
                detail: detail.into(),
            }],
        }
    }
    pub(crate) fn inputs(&self) -> impl Iterator<Item = launch::StandaloneProfileInput> + '_ {
        self.profiles.iter().map(|p| p.input.clone())
    }
    pub(crate) fn apply(&self, plan: &mut launch::LaunchPlan) {
        for candidate in &mut plan.candidates {
            let launch::LaunchTarget::Standalone {
                adapter_id: "mesen",
                profile_id,
                ..
            } = &candidate.target
            else {
                continue;
            };
            if let Some(p) = self
                .profiles
                .iter()
                .find(|p| &p.input.profile_id == profile_id)
            {
                candidate.blockers.insert(
                    0,
                    launch::LaunchBlocker::new(
                        launch::LaunchBlockerKind::MesenBindingUnavailable,
                        format!("{} {}", p.detail, NATIVE_CONTRACT_BLOCKER),
                    ),
                );
                candidate.readiness = launch::LaunchReadiness::Blocked;
            }
        }
    }
}
fn gather(selection: &Selection, roots: MesenProfileDiscoveryRoots) -> Snapshot {
    let discovery = patch_manager::discover_mesen_profiles(&roots);
    if discovery.profiles.is_empty() {
        return Snapshot::unavailable(
            "No native Mesen candidate was discovered on PATH with an existing default or portable profile.",
        );
    }
    let profiles = discovery
        .profiles
        .into_iter()
        .map(|profile| {
            let media = launch::mesen_nes::validate_mesen_nes_media(&selection.path);
            let firmware = launch::mesen_nes::inspect_mesen_nes_firmware(&profile, &selection.path);
            let detail = match &media {
                Ok(()) => firmware.detail,
                Err(detail) => format!("Media is unsupported: {detail}"),
            };
            Profile {
                input: launch::StandaloneProfileInput {
                    adapter_id: "mesen",
                    profile_id: profile.profile_id,
                    profile_path: Some(profile.configuration_path),
                    eligible: false,
                    firmware: if media.is_ok() {
                        firmware.firmware
                    } else {
                        launch::FirmwareReadiness::Unknown
                    },
                },
                detail,
            }
        })
        .collect();
    Snapshot { profiles }
}
#[derive(Default)]
pub(crate) struct Controller {
    selection: Option<Selection>,
    pending: Option<(Selection, Receiver<Snapshot>)>,
    current: Snapshot,
    checked: Option<Instant>,
    repaint: Option<eframe::egui::Context>,
}
impl Controller {
    pub(crate) fn set_repaint_context(&mut self, context: &eframe::egui::Context) {
        self.repaint = Some(context.clone());
    }
    pub(crate) fn snapshot(state: &Arc<Mutex<Self>>, selection: Selection) -> Snapshot {
        let Ok(mut state) = state.lock() else {
            return Snapshot::unavailable("Mesen readiness worker is unavailable.");
        };
        if state.selection.as_ref() != Some(&selection) {
            state.selection = Some(selection.clone());
            state.current = Snapshot::default();
            state.checked = None;
        }
        if let Some((job, receiver)) = state.pending.take() {
            match receiver.try_recv() {
                Ok(snapshot) if job == selection => {
                    state.current = snapshot;
                    state.checked = Some(Instant::now());
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => state.pending = Some((job, receiver)),
                Err(TryRecvError::Disconnected) => {
                    state.current = Snapshot::unavailable("Mesen readiness inspection failed.");
                    state.checked = Some(Instant::now());
                }
            }
        }
        if state.pending.is_none()
            && state
                .checked
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(5))
        {
            let (tx, rx) = mpsc::channel();
            state.pending = Some((selection.clone(), rx));
            let repaint = state.repaint.clone();
            std::thread::spawn(move || {
                let snapshot = MesenProfileDiscoveryRoots::from_environment()
                    .map(|roots| gather(&selection, roots))
                    .unwrap_or_else(|e| Snapshot::unavailable(&e.to_string()));
                let _ = tx.send(snapshot);
                if let Some(context) = repaint {
                    context.request_repaint();
                }
            });
        }
        if let Some(context) = &state.repaint {
            context.request_repaint_after(Duration::from_secs(5));
        }
        // Mark pending explicitly; absence is not success or missing firmware.
        if state.current.profiles.is_empty() {
            Snapshot::unavailable(
                "Checking native Mesen NES/FDS readiness; firmware evidence is not loaded yet.",
            )
        } else {
            state.current.clone()
        }
    }
}
#[cfg(test)]
mod tests;
