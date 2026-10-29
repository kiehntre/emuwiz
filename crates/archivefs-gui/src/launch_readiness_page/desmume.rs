//! Selected Nintendo DS launch through Core's native DeSmuME preflight.
use super::*;
use archivefs_core::launch::process_spawn::{CapturedFileIdentity, WatchedProcess};
use archivefs_core::launch::{
    DesmumeLaunchRequest, DesmumeProfileDiscoveryRoots, discover_desmume_profiles,
    preflight_desmume_launch, resolve_desmume_native_launch_binding, spawn_desmume,
};
use archivefs_core::platform_evidence_fusion::FusionOutcome;
use std::io::Read;

#[derive(Clone)]
pub(crate) struct Request {
    plan: LaunchPlan,
    candidate: LaunchCandidate,
    content_identity: CapturedFileIdentity,
}

impl Request {
    fn offer(plan: &LaunchPlan, candidate: &LaunchCandidate) -> Result<Self, String> {
        if !matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "desmume",
                ..
            }
        ) {
            return Err("The selected candidate is not native DeSmuME.".into());
        }
        if candidate.readiness != LaunchReadiness::Ready
            || !candidate.blockers.is_empty()
            || !candidate.warnings.is_empty()
            || candidate.content.requires_mount
            || candidate.content.container != Some(LaunchContainerKind::PlainFile)
        {
            return Err("Only a ready, direct Nintendo DS cartridge can launch in DeSmuME. Resolve the listed blockers first.".into());
        }
        if plan.platform_id.as_deref() != Some("Nintendo DS") || plan.game_key.is_none() {
            return Err("A verified Nintendo DS identity is required. Recheck this game.".into());
        }
        let path = candidate
            .content
            .resolved_path
            .as_deref()
            .ok_or("No direct Nintendo DS cartridge path is available.")?;
        if !path.is_absolute()
            || !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("nds"))
        {
            return Err(
                "DeSmuME accepts only a direct .nds file; archives are unsupported.".into(),
            );
        }
        Ok(Self {
            plan: plan.clone(),
            candidate: candidate.clone(),
            content_identity: capture(path)?,
        })
    }

    pub(crate) fn key(&self) -> (PathBuf, String) {
        (
            self.candidate
                .content
                .resolved_path
                .clone()
                .unwrap_or_default(),
            "desmume".into(),
        )
    }

    pub(crate) fn execute(self) -> Result<WatchedProcess, String> {
        self.execute_with_roots(DesmumeProfileDiscoveryRoots::from_environment())
    }

    fn execute_with_roots(
        self,
        roots: DesmumeProfileDiscoveryRoots,
    ) -> Result<WatchedProcess, String> {
        let LaunchTarget::Standalone { profile_id, .. } = &self.candidate.target else {
            return Err("DeSmuME profile binding is no longer available.".into());
        };
        let path = self
            .candidate
            .content
            .resolved_path
            .as_ref()
            .ok_or("Nintendo DS cartridge path is unavailable.")?;
        let profile = discover_desmume_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| &profile.profile_id == profile_id)
            .ok_or("The selected DeSmuME profile was not rediscovered. Recheck emulator setup.")?;
        let binding =
            resolve_desmume_native_launch_binding(&profile).map_err(|error| error.detail)?;
        let request = DesmumeLaunchRequest {
            selected_content_path: path.clone(),
            expected_platform_id: "Nintendo DS".into(),
            expected_game_key: self.plan.game_key.ok_or("Game identity is unresolved.")?,
            profile_id: profile_id.clone(),
            expected_executable: binding.executable.clone(),
            content_identity: self.content_identity,
            executable_identity: capture(&binding.executable)?,
        };
        let identity = fresh_identity(path, &request.expected_game_key, &request.content_identity)?;
        let command = preflight_desmume_launch(&request, &roots, &identity).map_err(|error| {
            format!(
                "DeSmuME was not started: {}. Recheck this game and emulator profile.",
                error.detail
            )
        })?;
        spawn_desmume(&command).map_err(|error| format!("DeSmuME could not start: {error}"))
    }
}

fn capture(path: &Path) -> Result<CapturedFileIdentity, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("DeSmuME content or executable is not a regular non-symlink file.".into());
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity(
    path: &Path,
    expected_key: &str,
    authorized_file: &CapturedFileIdentity,
) -> Result<CanonicalIdentityStatus, String> {
    let report =
        crate::selected_evidence_page::gather_selected_evidence_fast(path, Some("Nintendo DS"))
            .map_err(|error| format!("Nintendo DS identity could not be rechecked: {error}"))?;
    let content = &report.identity_result.content;
    if matches!(
        content.outcome,
        FusionOutcome::Conflict | FusionOutcome::Ambiguous
    ) || content
        .resolved_platform
        .is_some_and(|platform| platform != "Nintendo DS")
    {
        return Err(
            "Content platform evidence conflicts with Nintendo DS. Rescan before launching.".into(),
        );
    }
    // The general game identity inspector does not yet parse Nintendo DS.
    // Its verified launch key is the NTR-prefixed cartridge game code, which
    // we re-read from the exact authorized file before Core's preflight.
    let expected_code = expected_key
        .strip_prefix("NTR-")
        .filter(|code| code.len() == 4 && code.as_bytes().iter().all(u8::is_ascii_graphic))
        .ok_or("A verified NTR cartridge game key is required to launch DeSmuME.")?;
    let mut file = archivefs_core::launch::process_spawn::open_regular_file_nonblocking(path)
        .map_err(|error| format!("Nintendo DS cartridge could not be opened: {error}"))?;
    if CapturedFileIdentity::capture(&file.metadata().map_err(|error| error.to_string())?)
        != *authorized_file
    {
        return Err("Nintendo DS cartridge changed since launch was selected.".into());
    }
    let mut header = [0_u8; 0x10];
    file.read_exact(&mut header)
        .map_err(|_| "Nintendo DS cartridge header is incomplete.")?;
    if &header[0x0c..0x10] != expected_code.as_bytes() {
        return Err("Nintendo DS game code no longer matches the selected game.".into());
    }
    Ok(CanonicalIdentityStatus::Resolved(
        archivefs_core::launch::ResolvedIdentity {
            platform_id: "Nintendo DS".into(),
            game_key: expected_key.into(),
        },
    ))
}

pub(super) fn offer_for(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<Result<Request, String>> {
    matches!(
        candidate.target,
        LaunchTarget::Standalone {
            adapter_id: "desmume",
            ..
        }
    )
    .then(|| Request::offer(plan, candidate))
}

pub(super) fn show_offer(
    ui: &mut egui::Ui,
    state: &mut StandaloneLaunchState,
    offer: Result<Request, String>,
) {
    ui.add_space(6.0);
    match offer {
        Ok(request) => super::show_standalone_launch_action(
            ui,
            state,
            StandaloneLaunchRequest::Desmume(request),
        ),
        Err(reason) => {
            ui.colored_label(ui.visuals().warn_fg_color, reason);
        }
    }
}

pub(super) fn show_running(ui: &mut egui::Ui, process: &WatchedProcess) {
    ui.small(format!("DeSmuME running · PID {}", process.pid));
    ui.small("Starting the process does not prove the game ran.");
}

pub(super) fn show_exited(ui: &mut egui::Ui, process: &mut WatchedProcess) {
    if let Some(report) = process.poll() {
        ui.label(super::exit_line("DeSmuME", report));
    }
}

pub(super) fn exit_failure(
    report: &archivefs_core::launch::process_spawn::ProcessExitReport,
) -> Option<String> {
    match &report.status {
        Ok(status) if status.success() => None,
        _ => Some(super::exit_line("DeSmuME", report)),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use archivefs_core::launch::{CandidatePreference, FirmwareReadiness, LaunchContentRef};
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(
        code: i32,
    ) -> (
        tempfile::TempDir,
        DesmumeProfileDiscoveryRoots,
        LaunchPlan,
        LaunchCandidate,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("homebrew.nds");
        let mut homebrew = vec![0_u8; 0x200];
        homebrew[0x0c..0x10].copy_from_slice(b"EWZP");
        std::fs::write(&game, homebrew).unwrap();
        let executable = dir.path().join("desmume");
        std::fs::write(&executable, format!("#!/bin/sh\nexit {code}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let roots = DesmumeProfileDiscoveryRoots {
            explicit_executables: vec![executable.clone()],
            path_env: None,
            known_version_outputs: BTreeMap::new(),
        };
        let profile = discover_desmume_profiles(&roots)
            .profiles
            .into_iter()
            .next()
            .unwrap();
        let candidate = LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "desmume",
                profile_id: profile.profile_id,
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: None,
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(game),
                requires_mount: false,
                provenance: "test".into(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: vec![],
            warnings: vec![],
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::SoleEligible,
        };
        let plan = LaunchPlan {
            platform_id: Some("Nintendo DS".into()),
            game_key: Some("NTR-EWZP".into()),
            candidates: vec![candidate.clone()],
            summary: Default::default(),
            media_topology: None,
        };
        (dir, roots, plan, candidate)
    }

    #[test]
    fn selected_nds_starts_and_reports_exit_without_touching_the_game() {
        for code in [0, 7] {
            let (_dir, roots, plan, candidate) = fixture(code);
            let game = candidate.content.resolved_path.as_ref().unwrap();
            let before = std::fs::read(game).unwrap();
            let mut process = Request::offer(&plan, &candidate)
                .unwrap()
                .execute_with_roots(roots)
                .unwrap();
            let report = loop {
                if let Some(report) = process.poll() {
                    break report;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            assert_eq!(report.status.as_ref().unwrap().code(), Some(code));
            assert_eq!(exit_failure(report).is_some(), code != 0);
            assert_eq!(std::fs::read(game).unwrap(), before);
        }
    }

    #[test]
    fn changed_content_and_archive_are_refused() {
        let (_dir, roots, plan, candidate) = fixture(0);
        let request = Request::offer(&plan, &candidate).unwrap();
        std::fs::write(
            candidate.content.resolved_path.as_ref().unwrap(),
            b"changed",
        )
        .unwrap();
        assert!(request.execute_with_roots(roots).is_err());
        let (_dir, roots, plan, candidate) = fixture(0);
        let request = Request::offer(&plan, &candidate).unwrap();
        let path = candidate.content.resolved_path.as_ref().unwrap();
        let mut changed_code = std::fs::read(path).unwrap();
        changed_code[0x0c..0x10].copy_from_slice(b"NOPE");
        std::fs::write(path, changed_code).unwrap();
        assert!(request.execute_with_roots(roots).is_err());
        let (_dir, _roots, plan, mut candidate) = fixture(0);
        candidate.content.container = Some(LaunchContainerKind::Archive);
        assert!(Request::offer(&plan, &candidate).is_err());
    }
}
