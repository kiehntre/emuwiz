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
        spawn_sameboy(command).map_err(|error| format!("SameBoy could not start: {error:?}"))
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
        Ok(request) => super::show_standalone_launch_action(
            ui,
            state,
            StandaloneLaunchRequest::SameBoy(request),
        ),
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
