//! Blue Dragon/CoCo cassette action over core's TOSEC-verified identity and
//! the existing XRoar preflight. The identity is exact whole-file TOSEC
//! catalogue evidence; machine choice stays the explicit XRoar profile.
use super::*;
use archivefs_core::{
    launch::{
        DRAGON_COCO_PLATFORM_ID, DragonCocoCassetteEvidence, DragonCocoRefusal,
        DragonCocoRefusalKind, LaunchContentRef, XRoarLaunchExecutionError, XRoarLaunchRequest,
        preflight_and_launch_xroar, verify_dragon_coco_cassette,
    },
    patch_manager::XRoarProfileDiscoveryRoots,
};

#[derive(Clone)]
pub(crate) struct Context {
    pub selected: PathBuf,
    pub result: Result<DragonCocoCassetteEvidence, DragonCocoRefusal>,
}

#[derive(Clone)]
pub(crate) struct Request {
    evidence: DragonCocoCassetteEvidence,
    profile_id: String,
    expected_executable: PathBuf,
}

/// The registered TOSEC catalogues, plus every enabled source that could not be
/// consulted. A file's system can only be called unambiguous when every enabled
/// catalogue that might hold it was actually read.
#[derive(Default)]
pub(crate) struct RegisteredCatalogues {
    pub sources: Vec<archivefs_core::identity_source::tosec::ImportedTosecSource>,
    pub unconsulted: Vec<String>,
}

/// Consults each `(path, kind)`. A DAT that parses but is not a classic TOSEC
/// catalogue (another ecosystem, or TOSEC-ISO/PIX) is legitimately irrelevant;
/// a folder source or a DAT that cannot be read leaves ambiguity unresolved.
pub(crate) fn consult_catalogues<'a>(
    entries: impl IntoIterator<Item = (&'a Path, archivefs_core::dat::sources::DatSourceKind)>,
) -> RegisteredCatalogues {
    use archivefs_core::identity_source::tosec::{TosecImportError, import_tosec_dat};
    let mut out = RegisteredCatalogues::default();
    for (path, kind) in entries {
        let name = path.display().to_string();
        if kind != archivefs_core::dat::sources::DatSourceKind::File {
            out.unconsulted
                .push(format!("{name} (folder sources are not read)"));
            continue;
        }
        match import_tosec_dat(path) {
            Ok(source) => out.sources.push(source),
            Err(TosecImportError::NotTosec { .. } | TosecImportError::OutOfScope { .. }) => {}
            Err(error) => out.unconsulted.push(format!("{name} ({error})")),
        }
    }
    out
}

/// Every enabled registered DAT source. Disabled sources are the user's choice
/// and are not consulted.
pub(crate) fn load_registered_tosec_sources() -> RegisteredCatalogues {
    let Ok(config) = archivefs_core::dat::sources::load_dat_sources_config_default() else {
        return RegisteredCatalogues::default();
    };
    catalogues_from_config(&config)
}

fn catalogues_from_config(
    config: &archivefs_core::dat::sources::DatSourcesConfig,
) -> RegisteredCatalogues {
    let (registry, problems) = archivefs_core::dat::sources::DatSourceRegistry::from_config(config);
    let mut catalogues = consult_catalogues(
        registry
            .sorted_enabled()
            .into_iter()
            .map(|entry| (entry.path.as_path(), entry.kind)),
    );
    catalogues.unconsulted.extend(problems);
    catalogues
}

fn verify_against(
    path: &Path,
    catalogues: &RegisteredCatalogues,
) -> Result<DragonCocoCassetteEvidence, DragonCocoRefusal> {
    if !catalogues.unconsulted.is_empty() {
        return Err(DragonCocoRefusal {
            kind: DragonCocoRefusalKind::CatalogueIncomplete,
            detail: format!(
                "a registered DAT source could not be checked, so this file's system cannot be \
                 ruled out as ambiguous: {}",
                catalogues.unconsulted.join("; ")
            ),
        });
    }
    verify_dragon_coco_cassette(path, &catalogues.sources)
}

impl Context {
    /// Worker-only: reads registered DATs and hashes the cassette once.
    pub(crate) fn inspect(selected: PathBuf) -> Self {
        Self::inspect_with(selected, &load_registered_tosec_sources())
    }

    fn inspect_with(selected: PathBuf, catalogues: &RegisteredCatalogues) -> Self {
        let result = verify_against(&selected, catalogues);
        Self { selected, result }
    }

    /// Supplies identity only when nothing else resolved it: never overrides a
    /// resolved or conflicting identity from stronger evidence.
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
            adapter_id: "xroar",
            profile_id,
            profile_path: Some(executable),
        } = &candidate.target
        else {
            return None;
        };
        let ready = matches!(
            candidate.readiness,
            LaunchReadiness::Ready | LaunchReadiness::ReadyWithWarnings
        );
        let machine = profile_id
            .rsplit(':')
            .next()
            .and_then(archivefs_core::patch_manager::XRoarMachine::parse)?;
        (ready
            && evidence.supports_machine(machine)
            && candidate.blockers.is_empty()
            && !candidate.content.requires_mount
            && candidate.content.resolved_path.as_deref() == Some(self.selected.as_path())
            && evidence.path == self.selected
            && plan.platform_id.as_deref() == Some(DRAGON_COCO_PLATFORM_ID)
            && plan.game_key.as_deref() == Some(evidence.sha1.as_str()))
        .then(|| Request {
            evidence: evidence.clone(),
            profile_id: profile_id.clone(),
            expected_executable: executable.clone(),
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
                .is_some_and(|(path, adapter, _)| path == &self.selected && adapter == "xroar")
        {
            ui.label("Another standalone launch is active; wait before starting XRoar.");
            return;
        }
        if let Some(request) = self.request(plan, candidate) {
            show_standalone_launch_action(ui, state, StandaloneLaunchRequest::XRoar(request));
        }
    }
}

impl Request {
    pub(crate) fn key(&self) -> (PathBuf, String) {
        (self.evidence.path.clone(), "xroar".into())
    }

    /// Re-reads the registered catalogues and the cassette; the Play click never
    /// trusts the earlier review. Core then re-runs its own preflight.
    pub(crate) fn execute(
        self,
    ) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
        self.execute_with(
            &load_registered_tosec_sources(),
            &XRoarProfileDiscoveryRoots::from_environment(),
        )
    }

    fn execute_with(
        self,
        catalogues: &RegisteredCatalogues,
        roots: &XRoarProfileDiscoveryRoots,
    ) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
        let fresh = verify_against(&self.evidence.path, catalogues)
            .map_err(|refusal| format!("XRoar was not started: {}", refusal.detail))?;
        if fresh.sha1 != self.evidence.sha1
            || fresh.file_identity != self.evidence.file_identity
            || fresh.matches != self.evidence.matches
        {
            return Err("XRoar was not started: the cassette changed since it was verified".into());
        }
        let profile = archivefs_core::patch_manager::discover_xroar_profiles(roots)
            .profiles
            .into_iter()
            .find(|p| p.profile_id == self.profile_id)
            .ok_or("XRoar was not started: the selected profile is unavailable")?;
        if !fresh.supports_machine(profile.machine) {
            return Err("XRoar was not started: the selected machine does not match the cassette catalogue family".into());
        }
        let request = XRoarLaunchRequest {
            selected_content_path: fresh.path.clone(),
            expected_platform_id: DRAGON_COCO_PLATFORM_ID.into(),
            profile_id: self.profile_id,
            expected_executable: self.expected_executable,
            expected_content_identity: fresh.file_identity,
        };
        preflight_and_launch_xroar(&request, roots, &fresh.identity_status()).map_err(|error| {
            match error {
                XRoarLaunchExecutionError::Preflight(error) => {
                    format!("XRoar was not started: {}", error.detail)
                }
                XRoarLaunchExecutionError::Spawn(error) => {
                    format!("XRoar could not be started: {error}")
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::identity_source::tosec::{ImportedTosecSource, import_tosec_dat};
    use archivefs_core::launch::{FirmwareReadiness, StandaloneProfileInput, build_launch_plan};
    use archivefs_core::patch_manager::{
        XRoarExplicitProfile, XRoarFirmwareState, XRoarMachine, discover_xroar_profiles,
    };
    use sha1::{Digest, Sha1};

    const TAPE: &[u8] = b"synthetic dragon cassette bytes";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn dat(dir: &Path, name: &str, bytes: &[u8]) -> ImportedTosecSource {
        use md5::Md5;
        let crc = archivefs_core::identity_source::hashing::Crc32::of(bytes);
        let path = dir.join(format!("{}.dat", name.len()));
        std::fs::write(
            &path,
            format!(
                r#"<?xml version="1.0"?><datafile><header><name>{name}</name><description>{name} (TOSEC-v2025-01-15)</description><category>TOSEC</category><version>2025-01-15</version><author>T</author><homepage>TOSEC</homepage></header><game name="Game (1984)(Acme)"><rom name="g.cas" size="{}" crc="{crc}" md5="{}" sha1="{}"/></game></datafile>"#,
                bytes.len(),
                hex(&Md5::digest(bytes)),
                hex(&Sha1::digest(bytes)),
            ),
        )
        .unwrap();
        import_tosec_dat(&path).unwrap()
    }

    fn catalogues(sources: &[ImportedTosecSource]) -> RegisteredCatalogues {
        RegisteredCatalogues {
            sources: sources.to_vec(),
            unconsulted: Vec::new(),
        }
    }

    fn fake_xroar(dir: &Path, exit_code: i32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("xroar");
        std::fs::write(&path, format!("#!/bin/sh\nexit {exit_code}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn roots(
        executable: Option<&Path>,
        machine: XRoarMachine,
        selected: XRoarMachine,
        firmware: XRoarFirmwareState,
    ) -> XRoarProfileDiscoveryRoots {
        XRoarProfileDiscoveryRoots {
            explicit_profiles: executable
                .map(|path| {
                    vec![XRoarExplicitProfile {
                        executable: path.to_path_buf(),
                        machine,
                        firmware,
                    }]
                })
                .unwrap_or_default(),
            path_env: None,
            selected_machine: Some(selected),
            firmware_directory: None,
            firmware,
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        tape: PathBuf,
        sources: Vec<ImportedTosecSource>,
        roots: XRoarProfileDiscoveryRoots,
        context: Context,
        plan: LaunchPlan,
    }

    fn fixture_with(
        selected_machine: XRoarMachine,
        extra: Option<(&str, &[u8])>,
        firmware: XRoarFirmwareState,
    ) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let tape = dir.path().join("tape.cas");
        std::fs::write(&tape, TAPE).unwrap();
        let executable = fake_xroar(dir.path(), 0);
        let mut sources = vec![dat(dir.path(), "Dragon Data Dragon - Games - [CAS]", TAPE)];
        if let Some((name, bytes)) = extra {
            sources.push(dat(dir.path(), name, bytes));
        }
        let roots = roots(
            Some(&executable),
            XRoarMachine::Dragon32,
            selected_machine,
            firmware,
        );
        let context = Context::inspect_with(tape.clone(), &catalogues(&sources));
        let plan = match context.inputs(&CanonicalIdentityStatus::Unknown) {
            Some((identity, content)) => {
                let profiles: Vec<_> = discover_xroar_profiles(&roots)
                    .profiles
                    .iter()
                    .map(|profile| StandaloneProfileInput {
                        adapter_id: "xroar",
                        profile_id: profile.profile_id.clone(),
                        profile_path: Some(profile.executable.path.clone()),
                        eligible: profile.eligible,
                        firmware: match profile.firmware {
                            XRoarFirmwareState::Verified => FirmwareReadiness::Verified,
                            XRoarFirmwareState::PresentUnverified => {
                                FirmwareReadiness::PresentUnverified
                            }
                            XRoarFirmwareState::Missing => FirmwareReadiness::Missing,
                            XRoarFirmwareState::Unknown => FirmwareReadiness::Unknown,
                        },
                    })
                    .collect();
                let env =
                    archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
                        format_version: 1,
                        profiles: vec![],
                        diagnostics: vec![],
                    };
                build_launch_plan(&identity, &content, &profiles, &env, &[])
            }
            None => build_launch_plan(
                &CanonicalIdentityStatus::Unknown,
                &LaunchContentRef {
                    kind: None,
                    container: None,
                    resolved_path: None,
                    requires_mount: false,
                    provenance: String::new(),
                },
                &[],
                &archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
                    format_version: 1,
                    profiles: vec![],
                    diagnostics: vec![],
                },
                &[],
            ),
        };
        Fixture {
            _dir: dir,
            tape,
            sources,
            roots,
            context,
            plan,
        }
    }

    fn fixture() -> Fixture {
        fixture_with(
            XRoarMachine::Dragon32,
            None,
            XRoarFirmwareState::PresentUnverified,
        )
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
            xroar: Some(fixture.context.clone()),
            amiga_whdload: None,
            #[cfg(target_os = "linux")]
            shadps4: None,
        })
    }

    fn is_xroar_play(action: &GamerPlayAction) -> bool {
        matches!(
            action,
            GamerPlayAction::Launch(request)
                if matches!(**request, TypedLaunchRequest::Standalone(StandaloneLaunchRequest::XRoar(_)))
        )
    }

    fn wait_exit(mut process: archivefs_core::launch::process_spawn::WatchedProcess) -> bool {
        for _ in 0..200 {
            if process.poll().is_some() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn verified_cassette_reaches_the_blue_play_action() {
        let fixture = fixture();
        assert!(fixture.context.result.is_ok());
        assert!(is_xroar_play(&play_action(&fixture)));
    }

    #[test]
    fn a_dragon_match_cannot_launch_a_coco_profile() {
        let fixture = fixture();
        let mut request = request_for(&fixture);
        let executable = request.expected_executable.clone();
        let coco = roots(
            Some(&executable),
            XRoarMachine::CoCo2,
            XRoarMachine::CoCo2,
            XRoarFirmwareState::PresentUnverified,
        );
        request.profile_id = discover_xroar_profiles(&coco).profiles[0]
            .profile_id
            .clone();
        assert!(
            request
                .execute_with(&catalogues(&fixture.sources), &coco)
                .is_err()
        );
    }

    #[test]
    fn unverified_cassette_gets_no_play_button() {
        let dir = tempfile::tempdir().unwrap();
        let tape = dir.path().join("tape.cas");
        std::fs::write(&tape, b"some other tape").unwrap();
        let sources = vec![dat(dir.path(), "Dragon Data Dragon - Games - [CAS]", TAPE)];
        let context = Context::inspect_with(tape, &catalogues(&sources));
        assert!(context.result.is_err());
        assert!(context.inputs(&CanonicalIdentityStatus::Unknown).is_none());
        let none_registered =
            Context::inspect_with(dir.path().join("x.cas"), &RegisteredCatalogues::default());
        assert!(
            none_registered
                .inputs(&CanonicalIdentityStatus::Unknown)
                .is_none()
        );
    }

    #[test]
    fn cross_system_cas_collision_gets_no_play_button() {
        let fixture = fixture_with(
            XRoarMachine::Dragon32,
            Some(("MSX MSX - Games - [CAS]", TAPE)),
            XRoarFirmwareState::PresentUnverified,
        );
        assert!(fixture.context.result.is_err());
        assert!(!is_xroar_play(&play_action(&fixture)));
    }

    #[test]
    fn stronger_identity_is_never_overridden() {
        let fixture = fixture();
        let other = CanonicalIdentityStatus::Resolved(archivefs_core::launch::ResolvedIdentity {
            platform_id: "MSX".into(),
            game_key: "k".into(),
        });
        assert!(fixture.context.inputs(&other).is_none());
        assert!(
            fixture
                .context
                .inputs(&CanonicalIdentityStatus::Conflicting)
                .is_none()
        );
    }

    #[test]
    fn firmware_missing_or_unselected_machine_blocks_play() {
        let missing = fixture_with(XRoarMachine::Dragon32, None, XRoarFirmwareState::Missing);
        assert!(!is_xroar_play(&play_action(&missing)));
        // A different machine than the profile's: no eligible profile.
        let wrong_machine = fixture_with(
            XRoarMachine::CoCo3,
            None,
            XRoarFirmwareState::PresentUnverified,
        );
        assert!(!is_xroar_play(&play_action(&wrong_machine)));
    }

    #[test]
    fn missing_xroar_installation_has_no_candidate() {
        let mut fixture = fixture();
        fixture.roots = roots(
            None,
            XRoarMachine::Dragon32,
            XRoarMachine::Dragon32,
            XRoarFirmwareState::PresentUnverified,
        );
        assert!(discover_xroar_profiles(&fixture.roots).profiles.is_empty());
    }

    fn request_for(fixture: &Fixture) -> Request {
        let candidate = fixture
            .plan
            .candidates
            .iter()
            .find(|c| {
                matches!(
                    c.target,
                    LaunchTarget::Standalone {
                        adapter_id: "xroar",
                        ..
                    }
                )
            })
            .unwrap();
        fixture.context.request(&fixture.plan, candidate).unwrap()
    }

    #[test]
    fn launch_revalidates_then_spawns_and_reports_exit() {
        let fixture = fixture();
        let request = request_for(&fixture);
        let Ok(process) = request.execute_with(&catalogues(&fixture.sources), &fixture.roots)
        else {
            panic!("launch should start");
        };
        assert!(wait_exit(process), "process exit was not reported");
    }

    #[test]
    fn exited_cassette_offers_retry_and_rechecks_current_bytes() {
        let fixture = fixture();
        let request = request_for(&fixture);
        let process = request
            .clone()
            .execute_with(&catalogues(&fixture.sources), &fixture.roots)
            .unwrap();
        let mut state = StandaloneLaunchState::default();
        state.tracked = Some((
            fixture.tape.clone(),
            "xroar".into(),
            StandaloneLaunchStage::Running(StandaloneProcess::XRoar(process)),
        ));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.is_active() {
            state.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(
            state.tracked,
            Some((_, _, StandaloneLaunchStage::Exited(_)))
        ));
        let context = egui::Context::default();
        let mut frame = |events| {
            context.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        show_standalone_launch_action(
                            ui,
                            &mut state,
                            StandaloneLaunchRequest::XRoar(request.clone()),
                        );
                    });
                },
            )
        };
        fn button_center(shape: &egui::Shape) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == "Launch XRoar again (recheck)" => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(button_center),
                _ => None,
            }
        }
        frame(Vec::new());
        let output = frame(Vec::new());
        let position = output
            .shapes
            .iter()
            .find_map(|s| button_center(&s.shape))
            .expect("an exited XRoar cassette must offer retry without restarting Blue");
        // Retry must dispatch fresh verification, not replay the old command.
        std::fs::write(&fixture.tape, b"changed since the first play").unwrap();
        for pressed in [true, false] {
            frame(vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        assert!(
            state.is_active(),
            "retry must start the existing revalidation worker"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.is_active() {
            state.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(
            state.tracked,
            Some((_, _, StandaloneLaunchStage::Failed(_)))
        ));
    }

    #[test]
    fn checked_bios_is_bound_to_spawn_and_removed_bios_refuses() {
        let mut fixture = fixture();
        let bios = fixture._dir.path().join("bios");
        std::fs::create_dir(&bios).unwrap();
        fixture.roots.firmware_directory = Some(bios.clone());
        assert!(
            request_for(&fixture)
                .execute_with(&catalogues(&fixture.sources), &fixture.roots)
                .is_err()
        );
        std::fs::write(bios.join("d32.rom"), vec![0u8; 16384]).unwrap();
        let request = request_for(&fixture);
        let native = XRoarLaunchRequest {
            selected_content_path: fixture.tape.clone(),
            expected_platform_id: DRAGON_COCO_PLATFORM_ID.into(),
            profile_id: request.profile_id.clone(),
            expected_executable: request.expected_executable.clone(),
            expected_content_identity: request.evidence.file_identity,
        };
        let command = archivefs_core::launch::preflight_xroar_launch(
            &native,
            &fixture.roots,
            &fixture.context.result.as_ref().unwrap().identity_status(),
        )
        .unwrap();
        assert_eq!(command.arguments[0], "-no-c");
        assert!(
            command
                .arguments
                .windows(2)
                .any(|args| args[0] == "-extbas" && args[1] == bios.join("d32.rom").as_os_str())
        );
        let original = std::fs::read(&fixture.tape).unwrap();
        assert!(wait_exit(
            request
                .clone()
                .execute_with(&catalogues(&fixture.sources), &fixture.roots)
                .unwrap()
        ));
        assert_eq!(std::fs::read(&fixture.tape).unwrap(), original);
        std::fs::remove_file(bios.join("d32.rom")).unwrap();
        assert!(
            request
                .execute_with(&catalogues(&fixture.sources), &fixture.roots)
                .is_err()
        );
    }

    #[test]
    fn changed_cassette_is_refused_at_launch() {
        let fixture = fixture();
        let request = request_for(&fixture);
        std::fs::write(&fixture.tape, b"edited after review").unwrap();
        let error = request
            .execute_with(&catalogues(&fixture.sources), &fixture.roots)
            .err()
            .expect("refused");
        assert!(error.contains("XRoar was not started"), "{error}");
    }

    #[test]
    fn unreadable_cassette_is_refused_at_launch() {
        let fixture = fixture();
        let request = request_for(&fixture);
        std::fs::remove_file(&fixture.tape).unwrap();
        assert!(
            request
                .execute_with(&catalogues(&fixture.sources), &fixture.roots)
                .is_err()
        );
    }

    #[test]
    fn dat_evidence_disappearing_or_conflicting_is_refused_at_launch() {
        let fixture = fixture();
        let request = request_for(&fixture);
        assert!(
            request
                .clone()
                .execute_with(&RegisteredCatalogues::default(), &fixture.roots)
                .is_err()
        );
        let dir = tempfile::tempdir().unwrap();
        let mut conflicting = fixture.sources.clone();
        conflicting.push(dat(dir.path(), "MSX MSX - Games - [CAS]", TAPE));
        let error = request
            .execute_with(&catalogues(&conflicting), &fixture.roots)
            .err()
            .expect("refused");
        assert!(error.contains("ambiguous"), "{error}");
    }

    #[test]
    fn removed_xroar_executable_is_refused_at_launch() {
        let fixture = fixture();
        let request = request_for(&fixture);
        let empty = roots(
            None,
            XRoarMachine::Dragon32,
            XRoarMachine::Dragon32,
            XRoarFirmwareState::PresentUnverified,
        );
        assert!(
            request
                .execute_with(&catalogues(&fixture.sources), &empty)
                .is_err()
        );
    }

    #[test]
    fn an_unreadable_or_folder_catalogue_makes_the_cassette_unverifiable() {
        use archivefs_core::dat::sources::DatSourceKind;
        let dir = tempfile::tempdir().unwrap();
        let tape = dir.path().join("tape.cas");
        std::fs::write(&tape, TAPE).unwrap();
        let dragon = dat(dir.path(), "Dragon Data Dragon - Games - [CAS]", TAPE);
        let dragon_path = dragon.artifact_path.clone();
        let broken = dir.path().join("broken.dat");
        std::fs::write(&broken, b"\xff\xfe not a dat").unwrap();
        // A Dragon DAT alone verifies.
        let ok = consult_catalogues([(dragon_path.as_path(), DatSourceKind::File)]);
        assert!(ok.unconsulted.is_empty());
        assert!(Context::inspect_with(tape.clone(), &ok).result.is_ok());
        // An unreadable DAT or a folder source could hide an MSX/Atari match.
        for extra in [
            (broken.as_path(), DatSourceKind::File),
            (dir.path(), DatSourceKind::Folder),
        ] {
            let loaded = consult_catalogues([(dragon_path.as_path(), DatSourceKind::File), extra]);
            assert_eq!(loaded.unconsulted.len(), 1);
            let result = Context::inspect_with(tape.clone(), &loaded).result;
            assert_eq!(
                result.unwrap_err().kind,
                DragonCocoRefusalKind::CatalogueIncomplete
            );
        }
    }

    #[test]
    fn dropped_registry_entries_cannot_hide_a_foreign_collision() {
        use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
        let fixture = fixture();
        let dir = tempfile::tempdir().unwrap();
        let foreign = dat(dir.path(), "MSX MSX - Games - [CAS]", TAPE);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(DatSourceEntry::new(
                "one".into(),
                "Dragon".into(),
                fixture.sources[0].artifact_path.clone(),
                DatSourceKind::File,
            ))
            .unwrap();
        registry
            .add(DatSourceEntry::new(
                "two".into(),
                "MSX".into(),
                foreign.artifact_path.clone(),
                DatSourceKind::File,
            ))
            .unwrap();
        for bad_id in ["one", "../escape"] {
            let mut config = registry.to_config();
            config.sources.as_mut().unwrap()[1].id = bad_id.into();
            let loaded = catalogues_from_config(&config);
            assert!(!loaded.unconsulted.is_empty());
            assert_eq!(
                Context::inspect_with(fixture.tape.clone(), &loaded)
                    .result
                    .unwrap_err()
                    .kind,
                DragonCocoRefusalKind::CatalogueIncomplete
            );
            assert!(
                request_for(&fixture)
                    .execute_with(&loaded, &fixture.roots)
                    .is_err()
            );
        }
    }

    #[test]
    fn a_readable_non_tosec_dat_does_not_block_verification() {
        use archivefs_core::dat::sources::DatSourceKind;
        let dir = tempfile::tempdir().unwrap();
        let tape = dir.path().join("tape.cas");
        std::fs::write(&tape, TAPE).unwrap();
        let dragon = dat(dir.path(), "Dragon Data Dragon - Games - [CAS]", TAPE);
        let other = dir.path().join("other.dat");
        std::fs::write(
            &other,
            r#"<?xml version="1.0"?><datafile><header><name>Some Other Set</name><description>x</description></header><game name="g"><rom name="g.bin" size="1" sha1="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"/></game></datafile>"#,
        )
        .unwrap();
        let loaded = consult_catalogues([
            (dragon.artifact_path.as_path(), DatSourceKind::File),
            (other.as_path(), DatSourceKind::File),
        ]);
        assert!(loaded.unconsulted.is_empty(), "{:?}", loaded.unconsulted);
        assert!(Context::inspect_with(tape, &loaded).result.is_ok());
    }

    #[test]
    fn non_cas_selection_never_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("game.dsk");
        std::fs::write(&disk, TAPE).unwrap();
        let sources = vec![dat(dir.path(), "Dragon Data Dragon - Games - [CAS]", TAPE)];
        assert!(
            Context::inspect_with(disk, &catalogues(&sources))
                .result
                .is_err()
        );
    }
}
