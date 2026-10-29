//! Selected N64 launch through Core's native RMG preflight.
use super::*;
use archivefs_core::launch::process_spawn::{CapturedFileIdentity, WatchedProcess};
use archivefs_core::launch::{RmgLaunchRequest, preflight_rmg_launch, spawn_rmg};
use archivefs_core::patch_manager::{
    RmgProfileDiscoveryRoots, discover_rmg_profiles, resolve_rmg_native_launch_binding,
};
use archivefs_core::platform_evidence_fusion::FusionOutcome;

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
                adapter_id: "rmg",
                ..
            }
        ) {
            return Err("The selected candidate is not native RMG.".into());
        }
        if candidate.readiness != LaunchReadiness::Ready
            || !candidate.blockers.is_empty()
            || !candidate.warnings.is_empty()
            || candidate.content.requires_mount
            || candidate.content.container != Some(LaunchContainerKind::PlainFile)
        {
            return Err("Only a ready, direct N64 cartridge can launch in RMG. Resolve the listed blockers first.".into());
        }
        if plan.platform_id.as_deref() != Some("N64") || plan.game_key.is_none() {
            return Err("A verified N64 identity is required. Recheck this game.".into());
        }
        let path = candidate
            .content
            .resolved_path
            .as_deref()
            .ok_or("No direct N64 cartridge path is available.")?;
        if !path.is_absolute()
            || !path.extension().is_some_and(|e| {
                ["z64", "n64", "v64"]
                    .iter()
                    .any(|ext| e.eq_ignore_ascii_case(ext))
            })
        {
            return Err(
                "RMG accepts only a direct .z64, .n64 or .v64 file; archives are unsupported."
                    .into(),
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
            "rmg".into(),
        )
    }

    pub(crate) fn execute(self) -> Result<WatchedProcess, String> {
        self.execute_with_roots(RmgProfileDiscoveryRoots::from_environment())
    }

    fn execute_with_roots(self, roots: RmgProfileDiscoveryRoots) -> Result<WatchedProcess, String> {
        let LaunchTarget::Standalone { profile_id, .. } = &self.candidate.target else {
            return Err("RMG profile binding is no longer available.".into());
        };
        let path = self
            .candidate
            .content
            .resolved_path
            .as_ref()
            .ok_or("N64 cartridge path is unavailable.")?;
        let profile = discover_rmg_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| &profile.profile_id == profile_id)
            .ok_or("The selected RMG profile was not rediscovered. Recheck emulator setup.")?;
        let binding = resolve_rmg_native_launch_binding(&profile).map_err(|error| error.detail)?;
        let request = RmgLaunchRequest {
            selected_content_path: path.clone(),
            expected_platform_id: "N64".into(),
            expected_game_key: self.plan.game_key.ok_or("Game identity is unresolved.")?,
            profile_id: profile_id.clone(),
            expected_executable: binding.executable.clone(),
            content_identity: self.content_identity,
            executable_identity: capture(&binding.executable)?,
        };
        let identity = fresh_identity(path, &request.expected_game_key, &request.content_identity)?;
        let command = preflight_rmg_launch(&request, &roots, &identity).map_err(|error| {
            format!(
                "RMG was not started: {}. Recheck this game and emulator profile.",
                error.detail
            )
        })?;
        spawn_rmg(&command).map_err(|error| format!("RMG could not start: {error}"))
    }
}

fn capture(path: &Path) -> Result<CapturedFileIdentity, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("RMG content or executable is not a regular non-symlink file.".into());
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity(
    path: &Path,
    expected_key: &str,
    authorized_file: &CapturedFileIdentity,
) -> Result<CanonicalIdentityStatus, String> {
    let report = crate::selected_evidence_page::gather_selected_evidence_fast(path, Some("N64"))
        .map_err(|error| format!("N64 identity could not be rechecked: {error}"))?;
    let content = &report.identity_result.content;
    if matches!(
        content.outcome,
        FusionOutcome::Conflict | FusionOutcome::Ambiguous
    ) || content
        .resolved_platform
        .is_some_and(|platform| platform != "N64")
    {
        return Err(
            "Content platform evidence conflicts with N64. Rescan before launching.".into(),
        );
    }
    if capture(path)? != *authorized_file {
        return Err("N64 cartridge changed since launch was selected.".into());
    }
    let status =
        archivefs_core::launch::canonical_identity_from_game_report(&report.game_identity_report).0;
    if !matches!(&status, CanonicalIdentityStatus::Resolved(identity)
        if identity.platform_id == "N64" && identity.game_key == expected_key)
    {
        return Err("Fresh N64 cartridge identity no longer matches the selected game.".into());
    }
    Ok(status)
}

pub(super) fn offer_for(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<Result<Request, String>> {
    matches!(
        candidate.target,
        LaunchTarget::Standalone {
            adapter_id: "rmg",
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
        Ok(request) => {
            super::show_standalone_launch_action(ui, state, StandaloneLaunchRequest::Rmg(request))
        }
        Err(reason) => {
            ui.colored_label(ui.visuals().warn_fg_color, reason);
        }
    }
}

pub(super) fn show_running(ui: &mut egui::Ui, process: &WatchedProcess) {
    ui.small(format!("RMG running · PID {}", process.pid));
    ui.small("Starting the process does not prove the game ran.");
}

pub(super) fn show_exited(ui: &mut egui::Ui, process: &mut WatchedProcess) {
    if let Some(report) = process.poll() {
        ui.label(super::exit_line("RMG", report));
    }
}

pub(super) fn exit_failure(
    report: &archivefs_core::launch::process_spawn::ProcessExitReport,
) -> Option<String> {
    match &report.status {
        Ok(status) if status.success() => None,
        _ => Some(super::exit_line("RMG", report)),
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
        RmgProfileDiscoveryRoots,
        LaunchPlan,
        LaunchCandidate,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("homebrew.z64");
        let mut homebrew = vec![0_u8; 0x2000];
        homebrew[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        std::fs::write(&game, homebrew).unwrap();
        let executable = dir.path().join("RMG");
        std::fs::write(&executable, format!("#!/bin/sh\nexit {code}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let roots = RmgProfileDiscoveryRoots {
            explicit_executables: vec![executable.clone()],
            path_env: None,
            known_version_outputs: BTreeMap::new(),
        };
        let report =
            crate::selected_evidence_page::gather_selected_evidence_fast(&game, Some("N64"))
                .unwrap();
        let CanonicalIdentityStatus::Resolved(identity) =
            archivefs_core::launch::canonical_identity_from_game_report(
                &report.game_identity_report,
            )
            .0
        else {
            panic!("disposable N64 fixture has unresolved identity")
        };
        let profile = discover_rmg_profiles(&roots)
            .profiles
            .into_iter()
            .next()
            .unwrap();
        let candidate = LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "rmg",
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
            platform_id: Some(identity.platform_id),
            game_key: Some(identity.game_key),
            candidates: vec![candidate.clone()],
            summary: Default::default(),
            media_topology: None,
        };
        (dir, roots, plan, candidate)
    }

    #[test]
    fn selected_n64_starts_and_reports_exit_without_touching_the_game() {
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
        changed_code[4] = 0xff;
        std::fs::write(path, changed_code).unwrap();
        assert!(request.execute_with_roots(roots).is_err());
        let (_dir, _roots, plan, mut candidate) = fixture(0);
        candidate.content.container = Some(LaunchContainerKind::Archive);
        assert!(Request::offer(&plan, &candidate).is_err());
    }

    #[test]
    fn selected_n64_game_reaches_blue_planning() {
        let (_dir, _roots, plan, candidate) = fixture(0);
        let game = candidate.content.resolved_path.as_ref().unwrap();
        let report =
            crate::selected_evidence_page::gather_selected_evidence_fast(game, Some("N64"))
                .unwrap();
        let mut app = crate::tests::app_for_operation_tests();
        app.archive_context.focused = Some(game.clone());
        app.selected_evidence_ui.selected_evidence =
            crate::selected_evidence_page::SelectedEvidenceState::Ready {
                generation: 0,
                hasheous: crate::selected_evidence_page::HasheousState::Idle,
                report: Box::new(report),
            };
        let input = app.build_launch_readiness_input(None);
        assert!(matches!(
            input,
            crate::launch_readiness_page::LaunchReadinessInput::Plan { plan: actual, .. }
                if actual.platform_id == plan.platform_id && actual.game_key == plan.game_key
        ));
    }
}
