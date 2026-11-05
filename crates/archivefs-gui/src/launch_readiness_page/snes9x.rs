//! Selected SNES launch through Core's native Snes9x preflight.
use super::*;
use archivefs_core::launch::process_spawn::{CapturedFileIdentity, WatchedProcess};
use archivefs_core::launch::{Snes9xLaunchRequest, preflight_snes9x_launch, spawn_snes9x};
use archivefs_core::patch_manager::{
    Snes9xProfileDiscoveryRoots, discover_snes9x_profiles, resolve_snes9x_native_launch_binding,
};
use sha2::{Digest, Sha256};
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
                adapter_id: "snes9x",
                ..
            }
        ) {
            return Err("The selected candidate is not native Snes9x.".into());
        }
        if candidate.readiness != LaunchReadiness::Ready
            || !candidate.blockers.is_empty()
            || !candidate.warnings.is_empty()
            || candidate.content.requires_mount
            || candidate.content.container != Some(LaunchContainerKind::PlainFile)
        {
            return Err("Only a ready, direct SNES cartridge can launch in Snes9x. Resolve the listed blockers first.".into());
        }
        if plan.platform_id.as_deref() != Some("SNES") || plan.game_key.is_none() {
            return Err("A verified SNES identity is required. Recheck this game.".into());
        }
        let path = candidate
            .content
            .resolved_path
            .as_deref()
            .ok_or("No direct SNES cartridge path is available.")?;
        if !path.is_absolute()
            || !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("sfc") || e.eq_ignore_ascii_case("smc"))
        {
            return Err(
                "Snes9x accepts only a direct .sfc or .smc file; archives are unsupported.".into(),
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
            "snes9x".into(),
        )
    }

    pub(crate) fn execute(self) -> Result<WatchedProcess, String> {
        self.execute_with_roots(Snes9xProfileDiscoveryRoots::from_environment())
    }

    fn execute_with_roots(
        self,
        roots: Snes9xProfileDiscoveryRoots,
    ) -> Result<WatchedProcess, String> {
        let LaunchTarget::Standalone { profile_id, .. } = &self.candidate.target else {
            return Err("Snes9x profile binding is no longer available.".into());
        };
        let path = self
            .candidate
            .content
            .resolved_path
            .as_ref()
            .ok_or("SNES cartridge path is unavailable.")?;
        let profile = discover_snes9x_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| &profile.profile_id == profile_id)
            .ok_or("The selected Snes9x profile was not rediscovered. Recheck emulator setup.")?;
        let binding =
            resolve_snes9x_native_launch_binding(&profile).map_err(|error| error.detail)?;
        let request = Snes9xLaunchRequest {
            selected_content_path: path.clone(),
            expected_platform_id: "SNES".into(),
            expected_game_key: self.plan.game_key.ok_or("Game identity is unresolved.")?,
            profile_id: profile_id.clone(),
            expected_executable: binding.executable.clone(),
            content_identity: self.content_identity,
            executable_identity: capture(&binding.executable)?,
        };
        let identity = fresh_identity(path, &request.expected_game_key, &request.content_identity)?;
        let command = preflight_snes9x_launch(&request, &roots, &identity).map_err(|error| {
            format!(
                "Snes9x was not started: {}. Recheck this game and emulator profile.",
                error.detail
            )
        })?;
        spawn_snes9x(&command).map_err(|error| format!("Snes9x could not start: {error}"))
    }
}

fn capture(path: &Path) -> Result<CapturedFileIdentity, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Snes9x content or executable is not a regular non-symlink file.".into());
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity(
    path: &Path,
    expected_key: &str,
    authorized_file: &CapturedFileIdentity,
) -> Result<CanonicalIdentityStatus, String> {
    // A path-based evidence read may block if this ROM is replaced by a FIFO.
    // Read and hash one validated, nonblocking regular-file handle instead.
    let file = archivefs_core::launch::process_spawn::open_regular_file_nonblocking(path)
        .map_err(|error| format!("SNES cartridge could not be opened safely: {error}"))?;
    let opened =
        CapturedFileIdentity::capture(&file.metadata().map_err(|error| error.to_string())?);
    if opened != *authorized_file || capture(path)? != opened {
        return Err("SNES cartridge changed since launch was selected.".into());
    }
    let mut bytes = Vec::new();
    file.take(archivefs_core::game_identity::MAX_LOOSE_ROM_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("SNES cartridge could not be read: {error}"))?;
    if bytes.len() as u64 > archivefs_core::game_identity::MAX_LOOSE_ROM_BYTES {
        return Err("SNES cartridge exceeds the supported identity size.".into());
    }
    if capture(path)? != opened {
        return Err("SNES cartridge changed during identity inspection.".into());
    }
    let actual_key = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual_key != expected_key {
        return Err("Fresh SNES cartridge identity no longer matches the selected game.".into());
    }
    Ok(CanonicalIdentityStatus::Resolved(
        archivefs_core::launch::ResolvedIdentity {
            platform_id: "SNES".into(),
            game_key: actual_key,
        },
    ))
}

/// Shown wherever Snes9x Play is offered: the launch identity is the SHA-256 of
/// the cartridge file, which is not a DAT check.
pub(crate) const LIMITATION: &str = "Local launch identity (this cartridge file's own SHA-256), not a DAT check. Snes9x is only started; EmuWiz cannot confirm the game runs.";

pub(super) fn offer_for(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<Result<Request, String>> {
    matches!(
        candidate.target,
        LaunchTarget::Standalone {
            adapter_id: "snes9x",
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
            ui.small(LIMITATION);
            super::show_standalone_launch_action(
                ui,
                state,
                StandaloneLaunchRequest::Snes9x(request),
            )
        }
        Err(reason) => {
            ui.colored_label(ui.visuals().warn_fg_color, reason);
        }
    }
}

pub(super) fn show_running(ui: &mut egui::Ui, process: &WatchedProcess) {
    ui.small(format!("Snes9x running · PID {}", process.pid));
    ui.small("Starting the process does not prove the game ran.");
}

pub(super) fn show_exited(ui: &mut egui::Ui, process: &mut WatchedProcess) {
    if let Some(report) = process.poll() {
        ui.label(super::exit_line("Snes9x", report));
    }
}

pub(super) fn exit_failure(
    report: &archivefs_core::launch::process_spawn::ProcessExitReport,
) -> Option<String> {
    match &report.status {
        Ok(status) if status.success() => None,
        _ => Some(super::exit_line("Snes9x", report)),
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
        Snes9xProfileDiscoveryRoots,
        LaunchPlan,
        LaunchCandidate,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("homebrew.sfc");
        let homebrew = vec![0x5a_u8; 4096];
        std::fs::write(&game, homebrew).unwrap();
        let executable = dir.path().join("snes9x");
        std::fs::write(&executable, format!("#!/bin/sh\nexit {code}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let roots = Snes9xProfileDiscoveryRoots {
            explicit_executables: vec![executable.clone()],
            path_override: None,
            known_version_outputs: BTreeMap::new(),
        };
        let report =
            crate::selected_evidence_page::gather_selected_evidence_fast(&game, Some("SNES"))
                .unwrap();
        let CanonicalIdentityStatus::Resolved(identity) =
            archivefs_core::launch::canonical_identity_from_game_report(
                &report.game_identity_report,
            )
            .0
        else {
            panic!("disposable SNES fixture has unresolved identity")
        };
        let profile = discover_snes9x_profiles(&roots)
            .profiles
            .into_iter()
            .next()
            .unwrap();
        let candidate = LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "snes9x",
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
    fn selected_snes_starts_and_reports_exit_without_touching_the_game() {
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
    fn fifo_replacing_selected_cartridge_is_refused_without_blocking() {
        let (_dir, roots, plan, candidate) = fixture(0);
        let request = Request::offer(&plan, &candidate).unwrap();
        let path = candidate.content.resolved_path.as_ref().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
        assert!(request.execute_with_roots(roots).is_err());
    }

    #[test]
    fn selected_snes_game_reaches_blue_planning() {
        let (_dir, _roots, plan, candidate) = fixture(0);
        let game = candidate.content.resolved_path.as_ref().unwrap();
        let report =
            crate::selected_evidence_page::gather_selected_evidence_fast(game, Some("SNES"))
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

    fn blue_input(plan: &LaunchPlan) -> LaunchReadinessInput {
        LaunchReadinessInput::Plan {
            plan: plan.clone(),
            retroarch: None,
            retroarch_scanned: true,
            standalone_scans_complete: true,
            dolphin: None,
            pcsx2: None,
            duckstation: None,
            ppsspp: None,
            rpcs3: None,
            xemu: None,
            xenia: None,
            melonds: None,
            cemu: None,
            flycast: None,
            xroar: None,
            azahar: None,
            vice: None,
            amiga_whdload: None,
            #[cfg(target_os = "linux")]
            shadps4: None,
        }
    }

    /// Blue Play for `plan`; waits out any one-time asynchronous check.
    fn blue_play(plan: &LaunchPlan) -> GamerPlayAction {
        for _ in 0..200 {
            let action = gamer_play_action(&blue_input(plan));
            let checking = matches!(
                &action,
                GamerPlayAction::BlockedTyped(blocker)
                    if blocker.kind == GamerBlockerKind::CheckingGame
            );
            if !checking {
                return action;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the check never completed")
    }

    fn blue_request(plan: &LaunchPlan) -> Request {
        let GamerPlayAction::Launch(request) = blue_play(plan) else {
            panic!("Blue Play must offer the Snes9x launch")
        };
        let TypedLaunchRequest::Standalone(StandaloneLaunchRequest::Snes9x(request)) = *request
        else {
            panic!("not a Snes9x request")
        };
        request
    }

    #[test]
    fn a_ready_cartridge_reaches_blue_play_with_the_local_identity_caveat() {
        let (_dir, _roots, plan, _candidate) = fixture(0);
        let GamerPlayAction::Launch(request) = blue_play(&plan) else {
            panic!("Blue Play must offer the Snes9x launch")
        };
        assert!(matches!(
            *request,
            TypedLaunchRequest::Standalone(StandaloneLaunchRequest::Snes9x(_))
        ));
        assert_eq!(request.adapter_name(), "Snes9x");
        assert_eq!(
            request.unverified_status_label(),
            "Ready, local launch identity"
        );
        assert!(
            request
                .unverified_warnings()
                .iter()
                .any(|line| line.contains("not a DAT check"))
        );
    }

    #[test]
    fn blue_play_refuses_archives_unready_candidates_and_missing_or_wrong_identity() {
        let (_dir, _roots, plan, _candidate) = fixture(0);
        let mut archive = plan.clone();
        archive.candidates[0].content.container = Some(LaunchContainerKind::Archive);
        let mut blocked = plan.clone();
        blocked.candidates[0].readiness = LaunchReadiness::Blocked;
        let mut warned = plan.clone();
        warned.candidates[0].warnings = vec![archivefs_core::launch::LaunchWarning::new(
            archivefs_core::launch::LaunchWarningKind::FirmwarePresentUnverified,
            "test warning",
        )];
        let mut keyless = plan.clone();
        keyless.game_key = None;
        let mut wrong_platform = plan.clone();
        wrong_platform.platform_id = Some("N64".into());
        let mut wrong_adapter = plan.clone();
        wrong_adapter.candidates[0].target = LaunchTarget::Standalone {
            adapter_id: "desmume",
            profile_id: "x".into(),
            profile_path: None,
        };
        for refused in [
            archive,
            blocked,
            warned,
            keyless,
            wrong_platform,
            wrong_adapter,
        ] {
            let action = blue_play(&refused);
            assert!(
                !matches!(
                    &action,
                    GamerPlayAction::Launch(request)
                        if matches!(**request, TypedLaunchRequest::Standalone(StandaloneLaunchRequest::Snes9x(_)))
                ),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn a_blue_built_request_runs_exits_relaunches_and_refuses_changed_files() {
        for code in [0, 7] {
            let (_dir, roots, plan, candidate) = fixture(code);
            let request = blue_request(&plan);
            let path = candidate.content.resolved_path.clone().unwrap();
            let before = std::fs::read(&path).unwrap();
            // Play, then relaunch from a clone of the same request: each start
            // rediscovers the emulator and rechecks the file.
            for _ in 0..2 {
                let mut process = request.clone().execute_with_roots(roots.clone()).unwrap();
                let report = loop {
                    if let Some(report) = process.poll() {
                        break report;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                };
                assert_eq!(report.status.as_ref().unwrap().code(), Some(code));
                assert_eq!(exit_failure(report).is_some(), code != 0);
            }
            assert_eq!(std::fs::read(&path).unwrap(), before, "game untouched");
            // A change after Play was offered is refused at click time.
            std::fs::write(&path, b"different bytes").unwrap();
            assert!(request.execute_with_roots(roots).is_err());
        }
    }

    #[test]
    fn a_missing_emulator_is_refused_at_click_time() {
        let (_dir, mut roots, plan, _candidate) = fixture(0);
        let request = blue_request(&plan);
        roots.explicit_executables.clear();
        assert!(request.execute_with_roots(roots).is_err());
    }

    #[test]
    fn only_sfc_and_smc_files_are_offered() {
        let (_dir, _roots, plan, mut candidate) = fixture(0);
        let sfc = candidate.content.resolved_path.clone().unwrap();
        for (name, offered) in [
            ("g.SMC", true),
            ("g.smc", true),
            ("g.bin", false),
            ("g.zip", false),
            ("g.fig", false),
        ] {
            let path = sfc.with_file_name(name);
            std::fs::copy(&sfc, &path).unwrap();
            candidate.content.resolved_path = Some(path);
            assert_eq!(Request::offer(&plan, &candidate).is_ok(), offered, "{name}");
        }
    }
}
