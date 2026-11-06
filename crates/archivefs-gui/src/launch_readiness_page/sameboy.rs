//! Selected Game Boy and Game Boy Color launch through Core's SameBoy preflight.
use super::*;
use archivefs_core::launch::process_spawn::{CapturedFileIdentity, ProcessExitReport};
use archivefs_core::launch::{
    LaunchedSameBoyProcess, SameBoyPreflightRequest, preflight_sameboy_launch, spawn_sameboy,
};
use archivefs_core::patch_manager::{
    SameBoyProfileDiscoveryRoots, discover_sameboy_profiles, resolve_sameboy_native_launch_binding,
};
use sha2::{Digest, Sha256};
use std::io::Read;

/// Shown wherever SameBoy Play is offered: the launch identity is this exact
/// file's content hash plus its cartridge header, which is not a DAT proof.
pub(crate) const LIMITATION: &str = "Local cartridge identity (this file's own content hash and header), not a DAT verification. SameBoy is only started; EmuWiz cannot confirm the game runs.";

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
                adapter_id: "sameboy",
                ..
            }
        ) {
            return Err("The selected candidate is not native SameBoy.".into());
        }
        if candidate.readiness != LaunchReadiness::Ready
            || !candidate.blockers.is_empty()
            || !candidate.warnings.is_empty()
            || candidate.content.requires_mount
            || candidate.content.container != Some(LaunchContainerKind::PlainFile)
        {
            return Err("Only a ready, direct Game Boy cartridge can launch in SameBoy. Resolve the listed blockers first.".into());
        }
        let extension = match plan.platform_id.as_deref() {
            Some("Game Boy") => "gb",
            Some("Game Boy Color") => "gbc",
            _ => {
                return Err(
                    "SameBoy requires a verified Game Boy or Game Boy Color identity.".into(),
                );
            }
        };
        if plan.game_key.is_none() {
            return Err("Game identity is unresolved. Recheck this game.".into());
        }
        let path = candidate
            .content
            .resolved_path
            .as_deref()
            .ok_or("No direct cartridge path is available.")?;
        if !path.is_absolute()
            || !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        {
            return Err("SameBoy needs a direct .gb or .gbc file matching the selected platform; archives are unsupported.".into());
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
            "sameboy".into(),
        )
    }

    pub(crate) fn execute(self) -> Result<LaunchedSameBoyProcess, String> {
        let roots = SameBoyProfileDiscoveryRoots::from_environment()
            .map_err(|error| format!("SameBoy profile roots are unavailable: {error:?}"))?;
        self.execute_with_roots(roots)
    }

    fn execute_with_roots(
        self,
        roots: SameBoyProfileDiscoveryRoots,
    ) -> Result<LaunchedSameBoyProcess, String> {
        let LaunchTarget::Standalone { profile_id, .. } = &self.candidate.target else {
            return Err("SameBoy profile binding is no longer available.".into());
        };
        let path = self
            .candidate
            .content
            .resolved_path
            .as_ref()
            .ok_or("Cartridge path is unavailable.")?;
        let expected_key = self
            .plan
            .game_key
            .as_deref()
            .ok_or("Game identity is unresolved.")?;
        fresh_identity(path, expected_key, &self.content_identity)?;
        let profile = discover_sameboy_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| &profile.profile_id == profile_id)
            .ok_or("The selected SameBoy profile was not rediscovered. Recheck emulator setup.")?;
        let binding =
            resolve_sameboy_native_launch_binding(&profile).map_err(|error| error.detail)?;
        let request = SameBoyPreflightRequest {
            selected_content_path: path.clone(),
            expected_platform_id: self
                .plan
                .platform_id
                .ok_or("Game platform is unresolved.")?,
            profile_id: profile_id.clone(),
            expected_executable: binding.executable,
            expected_content_identity: Some(self.content_identity),
        };
        let command = preflight_sameboy_launch(&request, &roots).map_err(|error| {
            format!(
                "SameBoy was not started: {}. Recheck this game and emulator profile.",
                error.detail
            )
        })?;
        spawn_sameboy(command).map_err(|error| {
            format!(
                "SameBoy could not start: {}",
                match error {
                    archivefs_core::launch::SameBoyLaunchSpawnError::Spawn(io) => io,
                }
            )
        })
    }
}

fn capture(path: &Path) -> Result<CapturedFileIdentity, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("SameBoy cartridge is not a regular non-symlink file.".into());
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity(
    path: &Path,
    expected_key: &str,
    authorized_file: &CapturedFileIdentity,
) -> Result<(), String> {
    let file = archivefs_core::launch::process_spawn::open_regular_file_nonblocking(path)
        .map_err(|error| format!("SameBoy cartridge could not be opened safely: {error}"))?;
    let opened =
        CapturedFileIdentity::capture(&file.metadata().map_err(|error| error.to_string())?);
    if opened != *authorized_file || capture(path)? != opened {
        return Err("SameBoy cartridge changed since launch was selected.".into());
    }
    let mut bytes = Vec::new();
    file.take(archivefs_core::game_identity::MAX_LOOSE_ROM_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("SameBoy cartridge could not be read: {error}"))?;
    if bytes.len() as u64 > archivefs_core::game_identity::MAX_LOOSE_ROM_BYTES
        || capture(path)? != opened
    {
        return Err("SameBoy cartridge is too large or changed during identity inspection.".into());
    }
    let actual_key = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual_key != expected_key {
        return Err("Fresh cartridge identity no longer matches the selected game.".into());
    }
    Ok(())
}

pub(super) fn offer_for(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<Result<Request, String>> {
    matches!(
        candidate.target,
        LaunchTarget::Standalone {
            adapter_id: "sameboy",
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
                StandaloneLaunchRequest::SameBoy(request),
            )
        }
        Err(reason) => {
            ui.colored_label(ui.visuals().warn_fg_color, reason);
        }
    }
}

pub(super) fn show_running(ui: &mut egui::Ui, process: &LaunchedSameBoyProcess) {
    ui.small(format!("SameBoy running · PID {}", process.pid));
    ui.small("Starting the process does not prove the game ran.");
}

pub(super) fn show_exited(ui: &mut egui::Ui, process: &mut LaunchedSameBoyProcess) {
    if let Some(report) = process.poll() {
        ui.label(super::exit_line("SameBoy", report));
    }
}

pub(super) fn exit_failure(report: &ProcessExitReport) -> Option<String> {
    match &report.status {
        Ok(status) if status.success() => None,
        _ => Some(super::exit_line("SameBoy", report)),
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
        SameBoyProfileDiscoveryRoots,
        LaunchPlan,
        LaunchCandidate,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("homebrew.gb");
        let mut rom = vec![0u8; archivefs_core::gb_header_evidence::GB_MIN_ROM_BYTES as usize];
        const LOGO: [u8; 48] = [
            0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C,
            0x00, 0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6,
            0xDD, 0xDD, 0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC,
            0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
        ];
        rom[0x104..0x134].copy_from_slice(&LOGO);
        let mut checksum = 0u8;
        for &byte in &rom[0x134..=0x14c] {
            checksum = checksum.wrapping_sub(byte).wrapping_sub(1);
        }
        rom[0x14d] = checksum;
        std::fs::write(&game, rom).unwrap();
        let executable = dir.path().join("sameboy");
        std::fs::write(&executable, format!("#!/bin/sh\nexit {code}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = dir.path().join("profile");
        std::fs::create_dir(&config).unwrap();
        let roots = SameBoyProfileDiscoveryRoots {
            home: dir.path().to_path_buf(),
            xdg_data_home: dir.path().join("no-xdg"),
            explicit_configuration_roots: vec![config],
            portable_configuration_roots: vec![],
            explicit_executables: vec![executable],
            known_version_outputs: BTreeMap::new(),
        };
        let report =
            crate::selected_evidence_page::gather_selected_evidence_fast(&game, Some("Game Boy"))
                .unwrap();
        let CanonicalIdentityStatus::Resolved(identity) =
            archivefs_core::launch::canonical_identity_from_game_report(
                &report.game_identity_report,
            )
            .0
        else {
            panic!("disposable Game Boy fixture has unresolved identity")
        };
        let profile = discover_sameboy_profiles(&roots)
            .profiles
            .into_iter()
            .find(|profile| profile.eligible)
            .unwrap();
        let candidate = LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "sameboy",
                profile_id: profile.profile_id,
                profile_path: Some(profile.configuration_path),
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
    fn selected_cartridge_starts_and_reports_exit() {
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
        let (_dir, _roots, plan, mut candidate) = fixture(0);
        candidate.content.container = Some(LaunchContainerKind::Archive);
        assert!(Request::offer(&plan, &candidate).is_err());
    }

    fn blue_play_action(plan: &LaunchPlan) -> GamerPlayAction {
        gamer_play_action(&blue_input(plan))
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
            vita3k: None,
            vice: None,
            amiga_whdload: None,
            #[cfg(target_os = "linux")]
            shadps4: None,
        }
    }

    /// Blue's status and the real request builder must agree, with and
    /// without a warning (the builder refuses warned candidates).
    #[test]
    fn blue_status_agrees_with_whether_the_request_builder_accepts() {
        use crate::gui_v2::launch_readiness_summary::{
            ReadinessFreshness, ReadinessPresentationState as State, project, route_truth,
        };
        let (_dir, _roots, plan, _candidate) = fixture(0);
        let clean = blue_input(&plan);
        let mut summary = project(&clean, ReadinessFreshness::Current);
        route_truth::apply(&mut summary, &route_truth::assess(&clean));
        assert_eq!(summary.status, State::Ready);
        assert!(matches!(
            blue_play_action(&plan),
            GamerPlayAction::Launch(_)
        ));

        let mut warned = plan.clone();
        warned.candidates[0].warnings = vec![archivefs_core::launch::LaunchWarning::new(
            archivefs_core::launch::LaunchWarningKind::FirmwarePresentUnverified,
            "test warning",
        )];
        let input = blue_input(&warned);
        let mut summary = project(&input, ReadinessFreshness::Current);
        route_truth::apply(&mut summary, &route_truth::assess(&input));
        assert_eq!(summary.status, State::Blocked);
        assert!(summary.explanation.contains("test warning"));
        assert!(matches!(
            blue_play_action(&warned),
            GamerPlayAction::BlockedTyped(_)
        ));
    }

    #[test]
    fn a_ready_cartridge_reaches_the_blue_play_action_as_local_identity() {
        let (_dir, _roots, plan, _candidate) = fixture(0);
        let GamerPlayAction::Launch(request) = blue_play_action(&plan) else {
            panic!("Blue Play must offer the SameBoy launch");
        };
        assert!(matches!(
            *request,
            TypedLaunchRequest::Standalone(StandaloneLaunchRequest::SameBoy(_))
        ));
        assert_eq!(request.adapter_name(), "SameBoy");
        assert_eq!(
            request.unverified_status_label(),
            "Ready, identity not DAT-verified"
        );
        assert!(
            request
                .unverified_warnings()
                .iter()
                .any(|line| line.contains("not a DAT verification"))
        );
    }

    #[test]
    fn blue_play_refuses_archives_unready_candidates_and_missing_identity() {
        let (_dir, _roots, plan, _candidate) = fixture(0);
        let mut archive = plan.clone();
        archive.candidates[0].content.container = Some(LaunchContainerKind::Archive);
        let mut blocked = plan.clone();
        blocked.candidates[0].readiness = LaunchReadiness::Blocked;
        let mut keyless = plan.clone();
        keyless.game_key = None;
        let mut wrong_platform = plan.clone();
        wrong_platform.platform_id = Some("Game Boy Advance".into());
        let mut wrong_extension = plan.clone();
        wrong_extension.platform_id = Some("Game Boy Color".into());
        for refused in [archive, blocked, keyless, wrong_platform, wrong_extension] {
            assert!(
                !matches!(blue_play_action(&refused), GamerPlayAction::Launch(_)),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn same_size_replacement_after_play_became_ready_is_refused_and_relaunch_works() {
        let (_dir, roots, plan, candidate) = fixture(0);
        let path = candidate.content.resolved_path.clone().unwrap();
        let request = Request::offer(&plan, &candidate).unwrap();
        // Relaunch: a second launch from the same offer starts again.
        for _ in 0..2 {
            let mut process = request.clone().execute_with_roots(roots.clone()).unwrap();
            while process.poll().is_none() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let mut replaced = std::fs::read(&path).unwrap();
        replaced[0x200] ^= 0xff;
        std::fs::write(&path, replaced).unwrap();
        let error = request.execute_with_roots(roots).err().unwrap();
        assert!(
            error.contains("changed") || error.contains("no longer matches"),
            "{error}"
        );
    }

    #[test]
    fn selected_game_reaches_blue_planning() {
        let (_dir, _roots, plan, candidate) = fixture(0);
        let game = candidate.content.resolved_path.as_ref().unwrap();
        let report =
            crate::selected_evidence_page::gather_selected_evidence_fast(game, Some("Game Boy"))
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
