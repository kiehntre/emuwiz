//! Blue action for loose `.3dsx` homebrew over core's launch-only identity and
//! the existing Azahar preflight/spawn.
//!
//! The identity is a structurally validated 3DSX header bound to the file's
//! SHA-256 and filesystem identity. It is a *supported launch target*, never a
//! verified game: it is not a catalogue match and is never persisted.
use super::*;
use archivefs_core::{
    launch::{
        AZAHAR_HOMEBREW_KEY_PREFIX, AzaharHomebrewLaunchError, AzaharHomebrewLaunchRequest,
        LaunchContentRef, ThreeDsxEvidence, ThreeDsxRefusal, preflight_and_launch_azahar_homebrew,
        verify_three_dsx,
    },
    patch_manager::AzaharDiscoveryRoots,
};

pub(crate) fn is_three_dsx(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("3dsx"))
}

/// A catalogued platform other than Nintendo 3DS means other evidence places
/// the file elsewhere; the launch-only identity must not override it.
pub(crate) fn platform_allows(catalogued: Option<&str>) -> bool {
    catalogued.is_none_or(|platform| {
        platform.is_empty()
            || archivefs_core::canonical_platform_for_alias(platform) == Some("Nintendo 3DS")
    })
}

/// Launch-only file-hash keys (3DSX homebrew, local Amiga media) are never stored as a per-game default.
pub(crate) fn game_default_allowed(game_key: &str) -> bool {
    !game_key.starts_with(AZAHAR_HOMEBREW_KEY_PREFIX)
        && !game_key.starts_with(archivefs_core::launch::LOCAL_ADF_KEY_PREFIX)
}

#[derive(Clone)]
pub(crate) struct Context {
    pub selected: PathBuf,
    pub result: Result<ThreeDsxEvidence, ThreeDsxRefusal>,
}

#[derive(Clone)]
pub(crate) struct Request(AzaharHomebrewLaunchRequest);

pub(crate) fn roots_from_environment() -> AzaharDiscoveryRoots {
    AzaharDiscoveryRoots {
        explicit_executable: None,
        path: std::env::var_os("PATH"),
        config_root: std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/azahar")),
    }
}

impl Context {
    /// Worker-only: reads and hashes the file once.
    pub(crate) fn inspect(selected: PathBuf) -> Self {
        let result = verify_three_dsx(&selected);
        Self { selected, result }
    }

    /// Supplies a launch identity only when nothing else resolved one.
    pub(crate) fn inputs(
        &self,
        prior: &CanonicalIdentityStatus,
    ) -> Option<(CanonicalIdentityStatus, LaunchContentRef)> {
        if !matches!(prior, CanonicalIdentityStatus::Unknown) {
            return None;
        }
        let evidence = self.result.as_ref().ok()?;
        (evidence.path == self.selected)
            .then(|| (evidence.identity_status(), evidence.content_ref()))
    }

    pub(crate) fn request(
        &self,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
    ) -> Option<Request> {
        let evidence = self.result.as_ref().ok()?;
        let LaunchTarget::Standalone {
            adapter_id: "azahar",
            profile_id,
            ..
        } = &candidate.target
        else {
            return None;
        };
        let executable = PathBuf::from(profile_id.strip_prefix("azahar:")?);
        let metadata = std::fs::symlink_metadata(&executable).ok()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return None;
        }
        let ready = matches!(
            candidate.readiness,
            LaunchReadiness::Ready | LaunchReadiness::ReadyWithWarnings
        );
        (ready
            && candidate.blockers.is_empty()
            && !candidate.content.requires_mount
            && candidate.content.resolved_path.as_deref() == Some(self.selected.as_path())
            && evidence.path == self.selected
            && plan.platform_id.as_deref() == Some("Nintendo 3DS")
            && plan.game_key.as_deref()
                == Some(format!("{AZAHAR_HOMEBREW_KEY_PREFIX}{}", evidence.sha256).as_str()))
        .then(|| {
            Request(AzaharHomebrewLaunchRequest {
                evidence: evidence.clone(),
                expected_executable: executable,
                executable_identity:
                    archivefs_core::launch::process_spawn::CapturedFileIdentity::capture(&metadata),
            })
        })
    }

    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
        state: &mut StandaloneLaunchState,
    ) {
        if state.is_active()
            && !state
                .tracked
                .as_ref()
                .is_some_and(|(path, adapter, _)| path == &self.selected && adapter == "azahar")
        {
            ui.label("Another standalone launch is active; wait before starting Azahar.");
            return;
        }
        if let Some(request) = self.request(plan, candidate) {
            ui.small(
                "Nintendo 3DS homebrew (.3dsx): a launch target, not a catalogue-verified game. \
                 Azahar is started with this file; EmuWiz cannot confirm it runs.",
            );
            show_standalone_launch_action(ui, state, StandaloneLaunchRequest::Azahar(request));
        }
    }
}

impl Request {
    pub(crate) fn key(&self) -> (PathBuf, String) {
        (self.0.evidence.path.clone(), "azahar".into())
    }

    /// Core re-verifies the bytes, rediscovers Azahar and runs its preflight.
    pub(crate) fn execute(self) -> Result<archivefs_core::launch::LaunchedAzaharProcess, String> {
        self.execute_with(&roots_from_environment())
    }

    fn execute_with(
        self,
        roots: &AzaharDiscoveryRoots,
    ) -> Result<archivefs_core::launch::LaunchedAzaharProcess, String> {
        preflight_and_launch_azahar_homebrew(&self.0, roots).map_err(refusal_text)
    }
}

pub(crate) fn refusal_text(error: AzaharHomebrewLaunchError) -> String {
    match error {
        AzaharHomebrewLaunchError::Content(refusal) => format!(
            "Azahar was not started: the file is no longer valid .3dsx homebrew ({}).",
            refusal.detail
        ),
        AzaharHomebrewLaunchError::ContentChanged(detail) => {
            format!("Azahar was not started: {detail}. Recheck readiness before playing.")
        }
        AzaharHomebrewLaunchError::Executable(detail) => {
            format!("Azahar was not started: {detail}. Check the Azahar installation and retry.")
        }
        AzaharHomebrewLaunchError::Preflight(error) => {
            format!(
                "Azahar was not started: {}.",
                error.detail.trim_end_matches('.')
            )
        }
        AzaharHomebrewLaunchError::Spawn(error) => format!("Azahar could not be started: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::{FirmwareReadiness, StandaloneProfileInput, build_launch_plan};
    use std::os::unix::fs::PermissionsExt;

    fn sample_3dsx() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"3DSX");
        bytes.extend_from_slice(&0x2Cu16.to_le_bytes());
        bytes.extend_from_slice(&0x0Cu16.to_le_bytes());
        for value in [0u32, 0, 16, 8, 8, 4, 0, 0, 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.resize(0x2C + 36 + 28, 0);
        bytes
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        file: PathBuf,
        executable: PathBuf,
        context: Context,
        plan: LaunchPlan,
    }

    fn plan_for(context: &Context, executable: &Path, adapter: &'static str) -> LaunchPlan {
        let (identity, content) = context
            .inputs(&CanonicalIdentityStatus::Unknown)
            .unwrap_or((
                CanonicalIdentityStatus::Unknown,
                LaunchContentRef {
                    kind: None,
                    container: None,
                    resolved_path: None,
                    requires_mount: false,
                    provenance: String::new(),
                },
            ));
        let profiles = [StandaloneProfileInput {
            adapter_id: adapter,
            profile_id: format!("azahar:{}", executable.display()),
            profile_path: None,
            eligible: true,
            firmware: FirmwareReadiness::NotRequired,
        }];
        let env = archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![],
            diagnostics: vec![],
        };
        build_launch_plan(&identity, &content, &profiles, &env, &[])
    }

    fn fixture_with(name: &str, bytes: &[u8]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(name);
        std::fs::write(&file, bytes).unwrap();
        let executable = dir.path().join("azahar");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let context = Context::inspect(file.clone());
        let plan = plan_for(&context, &executable, "azahar");
        Fixture {
            _dir: dir,
            file,
            executable,
            context,
            plan,
        }
    }

    fn play_action(fixture: &Fixture) -> GamerPlayAction {
        gamer_play_action(&LaunchReadinessInput::Plan {
            plan: fixture.plan.clone(),
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
            azahar: Some(fixture.context.clone()),
            vita3k: None,
            vice: None,
            amiga_whdload: None,
            #[cfg(target_os = "linux")]
            shadps4: None,
        })
    }

    fn is_azahar_play(action: &GamerPlayAction) -> bool {
        matches!(
            action,
            GamerPlayAction::Launch(request)
                if matches!(**request, TypedLaunchRequest::Standalone(StandaloneLaunchRequest::Azahar(_)))
        )
    }

    #[test]
    fn a_non_3ds_catalogued_platform_blocks_the_launch_only_identity() {
        assert!(platform_allows(None));
        assert!(platform_allows(Some("")));
        assert!(platform_allows(Some("Nintendo 3DS")));
        assert!(!platform_allows(Some("Switch")));
        assert!(!platform_allows(Some("PSP")));
    }

    #[test]
    fn launch_only_keys_are_never_a_per_game_default() {
        assert!(!game_default_allowed("3dsx-sha256:abcd"));
        assert!(!game_default_allowed("local-adf-sha256:abcd"));
        assert!(game_default_allowed("some-catalogue-key"));
    }

    #[test]
    fn valid_homebrew_with_azahar_reaches_the_blue_play_action() {
        let fixture = fixture_with("game.3dsx", &sample_3dsx());
        assert!(is_azahar_play(&play_action(&fixture)));
        assert_eq!(fixture.plan.platform_id.as_deref(), Some("Nintendo 3DS"));
        assert!(
            fixture
                .plan
                .game_key
                .as_deref()
                .is_some_and(|key| key.starts_with(AZAHAR_HOMEBREW_KEY_PREFIX))
        );
    }

    #[test]
    fn a_fake_3dsx_never_gets_a_play_action() {
        let fixture = fixture_with("fake.3dsx", b"just text with a 3dsx extension padding..");
        assert!(fixture.context.result.is_err());
        assert!(!is_azahar_play(&play_action(&fixture)));
    }

    #[test]
    fn retail_formats_get_no_launch_identity() {
        for name in ["retail.3ds", "retail.cia", "retail.cci", "retail.bin"] {
            let fixture = fixture_with(name, &sample_3dsx());
            assert!(!is_azahar_play(&play_action(&fixture)), "{name}");
        }
    }

    #[test]
    fn stronger_identity_is_never_overridden() {
        let fixture = fixture_with("game.3dsx", &sample_3dsx());
        for prior in [
            CanonicalIdentityStatus::Conflicting,
            CanonicalIdentityStatus::Resolved(archivefs_core::launch::ResolvedIdentity {
                platform_id: "Nintendo 3DS".into(),
                game_key: "catalogue-key".into(),
            }),
        ] {
            assert!(fixture.context.inputs(&prior).is_none());
        }
    }

    #[test]
    fn request_requires_the_matching_plan_key_and_azahar_candidate() {
        let fixture = fixture_with("game.3dsx", &sample_3dsx());
        let candidate = &fixture.plan.candidates[0];
        assert!(fixture.context.request(&fixture.plan, candidate).is_some());
        let mut wrong_key = fixture.plan.clone();
        wrong_key.game_key = Some("3dsx-sha256:00".into());
        assert!(fixture.context.request(&wrong_key, candidate).is_none());
        let mut wrong_platform = fixture.plan.clone();
        wrong_platform.platform_id = Some("PSP".into());
        assert!(
            fixture
                .context
                .request(&wrong_platform, candidate)
                .is_none()
        );
        let other = plan_for(&fixture.context, &fixture.executable, "xroar");
        assert!(
            fixture
                .context
                .request(&fixture.plan, &other.candidates[0])
                .is_none()
        );
    }

    #[test]
    fn click_time_recheck_refuses_changed_bytes_and_a_symlinked_executable() {
        let fixture = fixture_with("game.3dsx", &sample_3dsx());
        let request = fixture
            .context
            .request(&fixture.plan, &fixture.plan.candidates[0])
            .unwrap();
        let roots = AzaharDiscoveryRoots {
            explicit_executable: Some(fixture.executable.clone()),
            path: None,
            config_root: None,
        };
        // Same size, different bytes after the review.
        let mut changed = sample_3dsx();
        let last = changed.len() - 1;
        changed[last] = 9;
        std::fs::write(&fixture.file, changed).unwrap();
        let error = request.clone().execute_with(&roots).err().unwrap();
        assert!(error.contains("changed"), "{error}");
        // Executable replaced by a symlink: discovery refuses it.
        std::fs::write(&fixture.file, sample_3dsx()).unwrap();
        let real = fixture.executable.with_file_name("real-azahar");
        std::fs::rename(&fixture.executable, &real).unwrap();
        std::os::unix::fs::symlink(&real, &fixture.executable).unwrap();
        assert!(request.execute_with(&roots).is_err());
    }

    #[test]
    fn execute_hands_off_to_the_existing_spawn_and_is_monitored() {
        let fixture = fixture_with("game.3dsx", &sample_3dsx());
        let request = fixture
            .context
            .request(&fixture.plan, &fixture.plan.candidates[0])
            .unwrap();
        let roots = AzaharDiscoveryRoots {
            explicit_executable: Some(fixture.executable.clone()),
            path: None,
            config_root: None,
        };
        let mut process = request.execute_with(&roots).unwrap();
        for _ in 0..200 {
            if process.poll().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("fixture process never exited");
    }
}
