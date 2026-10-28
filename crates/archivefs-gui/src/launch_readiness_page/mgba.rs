//! Selected loose GB/GBC/GBA launch through Core's native mGBA preflight.
use super::*;
use archivefs_core::launch::mgba_execution::{capture_mgba_config, capture_mgba_executable};
use archivefs_core::launch::process_spawn::{CapturedFileIdentity, WatchedProcess};
use archivefs_core::launch::{MgbaLaunchRequest, preflight_mgba_launch, spawn_mgba};
use archivefs_core::patch_manager::{
    MgbaProfileDiscoveryRoots, discover_mgba_profiles, resolve_mgba_native_launch_binding,
};
use archivefs_core::platform_evidence_fusion::FusionOutcome;

#[derive(Clone)]
pub(crate) struct Request {
    plan: LaunchPlan,
    candidate: LaunchCandidate,
}

struct Reviewed {
    core: MgbaLaunchRequest,
    roots: MgbaProfileDiscoveryRoots,
}

impl Request {
    pub(crate) fn offer(plan: &LaunchPlan, candidate: &LaunchCandidate) -> Result<Self, String> {
        if !matches!(
            &candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "mgba",
                ..
            }
        ) {
            return Err("The selected candidate is not native mGBA.".into());
        }
        if candidate.readiness != LaunchReadiness::Ready
            || !candidate.blockers.is_empty()
            || !candidate.warnings.is_empty()
            || candidate.content.requires_mount
            || candidate.content.container != Some(LaunchContainerKind::PlainFile)
        {
            return Err("Only a ready, direct loose cartridge can launch in native mGBA. Resolve the listed readiness blockers first.".into());
        }
        let path = candidate
            .content
            .resolved_path
            .as_ref()
            .ok_or("No direct cartridge path is available.")?;
        let platform = plan
            .platform_id
            .as_deref()
            .ok_or("Game platform is unresolved.")?;
        let _ = plan
            .game_key
            .as_ref()
            .ok_or("Game identity is unresolved.")?;
        let extension = match platform {
            "Game Boy" => "gb",
            "Game Boy Color" => "gbc",
            "Game Boy Advance" => "gba",
            _ => return Err("Native mGBA supports only GB, GBC and GBA games.".into()),
        };
        if !path.is_absolute()
            || !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        {
            return Err("Native mGBA needs a direct .gb, .gbc or .gba file matching the selected platform; archives are unsupported.".into());
        }
        Ok(Self {
            plan: plan.clone(),
            candidate: candidate.clone(),
        })
    }
    pub(crate) fn key(&self) -> (PathBuf, String) {
        (
            self.candidate
                .content
                .resolved_path
                .clone()
                .unwrap_or_default(),
            "mgba".into(),
        )
    }
    pub(crate) fn execute(self) -> Result<WatchedProcess, String> {
        let roots = MgbaProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?;
        self.execute_with_roots(roots)
    }
    fn execute_with_roots(
        self,
        roots: MgbaProfileDiscoveryRoots,
    ) -> Result<WatchedProcess, String> {
        let reviewed = review(&self.plan, &self.candidate, roots)?;
        let identity = fresh_identity(
            &reviewed.core.selected_content_path,
            &reviewed.core.expected_platform_id,
        )?;
        let command =
            preflight_mgba_launch(&reviewed.core, &reviewed.roots, &identity).map_err(|e| {
                format!(
                    "mGBA was not started: {}. Recheck this game and emulator profile.",
                    e.detail
                )
            })?;
        spawn_mgba(&command).map_err(|e| format!("mGBA could not start: {e}"))
    }
}

pub(super) fn offer_for(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<Result<Request, String>> {
    matches!(
        &candidate.target,
        LaunchTarget::Standalone {
            adapter_id: "mgba",
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
            super::show_standalone_launch_action(ui, state, StandaloneLaunchRequest::Mgba(request))
        }
        Err(reason) => {
            ui.colored_label(ui.visuals().warn_fg_color, reason);
        }
    }
}

pub(super) fn show_running(ui: &mut egui::Ui, process: &WatchedProcess) {
    ui.small(format!("mGBA running · PID {}", process.pid));
    ui.small("Starting the process does not prove the game ran.");
}

pub(super) fn show_exited(ui: &mut egui::Ui, process: &mut WatchedProcess) {
    if let Some(report) = process.poll() {
        ui.label(super::exit_line("mGBA", report));
    }
}

pub(super) fn exit_failure(
    report: &archivefs_core::launch::process_spawn::ProcessExitReport,
) -> Option<String> {
    match &report.status {
        Ok(status) if status.success() => None,
        _ => Some(super::exit_line("mGBA", report)),
    }
}

fn capture(path: &Path) -> Result<CapturedFileIdentity, String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is not a regular, non-symlink file",
            path.display()
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity(path: &Path, platform: &str) -> Result<CanonicalIdentityStatus, String> {
    let report = crate::selected_evidence_page::gather_selected_evidence_fast(path, Some(platform))
        .map_err(|e| format!("mGBA identity could not be rechecked: {e}"))?;
    let content = &report.identity_result.content;
    if matches!(
        content.outcome,
        FusionOutcome::Conflict | FusionOutcome::Ambiguous
    ) || content
        .resolved_platform
        .as_deref()
        .is_some_and(|p| p != platform)
    {
        return Err("Content platform evidence conflicts with the selected game. Rescan it before launching.".into());
    }
    Ok(archivefs_core::launch::canonical_identity_from_game_report(&report.game_identity_report).0)
}

fn review(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    roots: MgbaProfileDiscoveryRoots,
) -> Result<Reviewed, String> {
    Request::offer(plan, candidate)?;
    let LaunchTarget::Standalone {
        adapter_id: "mgba",
        profile_id,
        ..
    } = &candidate.target
    else {
        return Err("The selected candidate is not native mGBA.".into());
    };
    let path = candidate
        .content
        .resolved_path
        .as_ref()
        .ok_or("No direct cartridge path is available.")?;
    let platform = plan
        .platform_id
        .as_deref()
        .ok_or("Game platform is unresolved.")?;
    let game_key = plan
        .game_key
        .as_ref()
        .ok_or("Game identity is unresolved.")?;
    let profile = discover_mgba_profiles(&roots)
        .profiles
        .into_iter()
        .find(|p| &p.profile_id == profile_id)
        .ok_or("The selected mGBA profile is unavailable. Recheck emulator setup.")?;
    let binding = resolve_mgba_native_launch_binding(&profile).map_err(|e| e.detail)?;
    let (executable_identity, executable_sha256) = capture_mgba_executable(&binding.executable)
        .map_err(|e| format!("mGBA executable could not be verified: {e}"))?;
    let config = profile
        .config_path
        .as_deref()
        .map(capture_mgba_config)
        .transpose()
        .map_err(|e| format!("mGBA configuration could not be verified: {e}"))?;
    let core = MgbaLaunchRequest {
        selected_content_path: path.clone(),
        expected_platform_id: platform.into(),
        expected_game_key: game_key.clone(),
        profile_id: profile_id.clone(),
        expected_executable: binding.executable.clone(),
        content_identity: capture(path)?,
        executable_identity,
        executable_sha256,
        expected_config_path: profile.config_path.clone(),
        config_identity: config.map(|(identity, _)| identity),
        config_sha256: config.map(|(_, digest)| digest),
    };
    Ok(Reviewed { core, roots })
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::{
        CandidatePreference, FirmwareReadiness, LaunchContentRef, LaunchReadiness,
    };
    use std::collections::BTreeMap;

    fn fixture(
        code: i32,
    ) -> (
        tempfile::TempDir,
        MgbaProfileDiscoveryRoots,
        LaunchPlan,
        LaunchCandidate,
    ) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game.gba");
        std::fs::write(&game, b"synthetic-gba-cartridge-bytes").unwrap();
        let executable = dir.path().join("mgba");
        std::fs::write(&executable, format!("#!/bin/sh\nexit {code}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = dir.path().join("config");
        std::fs::create_dir(&config).unwrap();
        let roots = MgbaProfileDiscoveryRoots {
            home: dir.path().into(),
            xdg_config_home: dir.path().join("unused"),
            explicit_configuration_roots: vec![config.clone()],
            portable_configuration_roots: vec![],
            explicit_executables: vec![executable],
            known_version_outputs: BTreeMap::new(),
        };
        let identity = fresh_identity(&game, "Game Boy Advance").unwrap();
        let CanonicalIdentityStatus::Resolved(identity) = identity else {
            panic!("fixture identity unresolved")
        };
        let candidate = LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "mgba",
                profile_id: format!("mgba:{}", config.display()),
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
    fn selected_cartridge_launches_and_nonzero_exit_is_visible() {
        for code in [0, 7] {
            let (_dir, roots, plan, candidate) = fixture(code);
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
        }
    }

    #[test]
    fn changed_game_and_unsupported_archive_refuse() {
        let (_dir, roots, plan, candidate) = fixture(0);
        let request = Request::offer(&plan, &candidate).unwrap();
        std::fs::write(
            candidate.content.resolved_path.as_ref().unwrap(),
            b"different bytes",
        )
        .unwrap();
        assert!(
            request
                .execute_with_roots(roots)
                .err()
                .unwrap()
                .contains("not started")
        );
        let (_dir, _roots, plan, mut candidate) = fixture(0);
        candidate.content.container = Some(LaunchContainerKind::Archive);
        assert!(Request::offer(&plan, &candidate).is_err());
    }

    #[test]
    fn selection_keys_do_not_reuse_an_old_games_result() {
        let (dir, _roots, plan, candidate) = fixture(0);
        let first = Request::offer(&plan, &candidate).unwrap();
        let mut second_candidate = candidate.clone();
        let next = dir.path().join("next.gba");
        std::fs::write(&next, b"another synthetic game").unwrap();
        second_candidate.content.resolved_path = Some(next);
        let second = Request::offer(&plan, &second_candidate).unwrap();
        assert_ne!(first.key(), second.key());
    }
}
