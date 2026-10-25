//! Blue's selection-bound action over the existing Cemu previewed executor.
use super::*;
use archivefs_core::{
    launch::{
        LaunchBlocker, LaunchBlockerKind, LaunchContentRef, LaunchPlanSummary,
        cemu_readiness::{CemuReadinessInspection, launch_bridge},
    },
    patch_manager::CemuProfileDiscoveryRoots,
};

#[derive(Clone)]
pub(crate) struct Context {
    pub selected: PathBuf,
    pub inspection: std::sync::Arc<CemuReadinessInspection>,
    selection: Option<launch_bridge::Selection>,
    profile_refusals: std::collections::BTreeMap<String, String>,
    pub roots: CemuProfileDiscoveryRoots,
}
#[derive(Clone)]
pub(crate) struct Request {
    context: Context,
    profile_id: String,
}
impl Context {
    pub(crate) fn new(
        selected: PathBuf,
        inspection: CemuReadinessInspection,
        roots: CemuProfileDiscoveryRoots,
    ) -> Self {
        let selection = launch_bridge::Selection::inspect(&selected, &inspection);
        let profile_refusals = inspection
            .profiles
            .iter()
            .filter_map(|p| {
                runtime_profile_refusal(&roots, p).map(|reason| (p.profile_id.clone(), reason))
            })
            .collect();
        Self {
            selected,
            inspection: std::sync::Arc::new(inspection),
            roots,
            selection,
            profile_refusals,
        }
    }
    pub(crate) fn expired(&self) -> bool {
        self.selection.as_ref().is_some_and(|s| s.expired())
    }
    pub(crate) fn inputs(
        &self,
        previous: &CanonicalIdentityStatus,
    ) -> Option<(CanonicalIdentityStatus, LaunchContentRef)> {
        self.selection.as_ref()?.project(&self.selected, previous)
    }
    pub(crate) fn profile_is_bound(
        &self,
        profile: &archivefs_core::launch::cemu_readiness::CemuProfilePreview,
    ) -> bool {
        !self.expired() && !self.profile_refusals.contains_key(&profile.profile_id)
    }
    pub(crate) fn apply(&self, plan: &mut LaunchPlan) {
        for candidate in &mut plan.candidates {
            let LaunchTarget::Standalone {
                adapter_id: "cemu",
                profile_id,
                ..
            } = &candidate.target
            else {
                continue;
            };
            let reason = if self.expired() {
                Some("Wii U readiness expired; refresh Wii U readiness".to_owned())
            } else {
                self.profile_refusals.get(profile_id).cloned()
            };
            if let Some(reason) = reason {
                candidate.blockers.push(LaunchBlocker::new(
                    LaunchBlockerKind::CemuLayoutInvalid,
                    reason,
                ));
                candidate.readiness = LaunchReadiness::Blocked;
            }
        }
        plan.summary = LaunchPlanSummary {
            candidates: plan.candidates.len(),
            ..Default::default()
        };
        for candidate in &plan.candidates {
            match candidate.readiness {
                LaunchReadiness::Ready => plan.summary.ready += 1,
                LaunchReadiness::ReadyWithWarnings => plan.summary.ready_with_warnings += 1,
                LaunchReadiness::Blocked => plan.summary.blocked += 1,
            }
        }
    }
    pub(crate) fn request(
        &self,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
    ) -> Option<Request> {
        if candidate.readiness != LaunchReadiness::Ready
            || !candidate.blockers.is_empty()
            || !candidate.warnings.is_empty()
            || candidate.content.requires_mount
        {
            return None;
        }
        let identity = preview_identity(plan);
        let (_, content) = self.inputs(&identity)?;
        if content != candidate.content {
            return None;
        }
        let LaunchTarget::Standalone {
            adapter_id: "cemu",
            profile_id,
            profile_path,
        } = &candidate.target
        else {
            return None;
        };
        let profile = self
            .inspection
            .profiles
            .iter()
            .find(|p| &p.profile_id == profile_id)?;
        if profile_path.as_ref() != Some(&profile.profile_path)
            || !self.profile_is_bound(profile)
            || !profile.refusals.is_empty()
        {
            return None;
        }
        let command = profile.command.as_ref()?;
        if command.selection.content_path != self.inspection.content.source
            || !command
                .selection
                .title_id
                .as_deref()
                .zip(plan.game_key.as_deref())
                .is_some_and(|(observed, expected)| observed.eq_ignore_ascii_case(expected))
        {
            return None;
        }
        Some(Request {
            context: self.clone(),
            profile_id: profile_id.clone(),
        })
    }
    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
        state: &mut StandaloneLaunchState,
    ) {
        widgets::card(ui, |ui| {
            ui.strong("Cemu · extracted Wii U base game");
            ui.label("Launch uses structural title metadata and a bounded Cafe executable-format check. This is not hash/DAT verification or a guarantee the game will run.");
            ui.label("Already-decrypted content: no title key or firmware installation is required by this launch path. Cemu's configured MLC must be available.");
            for blocker in &candidate.blockers {
                show_blocker(ui, blocker);
            }
            if let Some(request) = self.request(plan, candidate) {
                let command = self
                    .inspection
                    .profiles
                    .iter()
                    .find(|p| p.profile_id == request.profile_id)
                    .and_then(|p| p.command.as_ref())
                    .unwrap();
                ui.label(format!("Profile: {}", request.profile_id));
                ui.monospace(format!("{:?} {:?}", command.executable, command.arguments));
                ui.small("Preview only. Clicking Launch rechecks this exact content, profile, executable and related content.");
                show_standalone_launch_action(ui, state, StandaloneLaunchRequest::Cemu(request));
            } else {
                ui.label("Launch refused: refresh Wii U readiness and configure the native default Cemu profile; this selection has no current, bound command.");
            }
        });
    }
}
impl Request {
    pub(super) fn key(&self) -> (PathBuf, String) {
        (self.context.selected.clone(), "Cemu".into())
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        let profile = self
            .context
            .inspection
            .profiles
            .iter()
            .find(|p| p.profile_id == self.profile_id)
            .ok_or("Cemu profile disappeared")?;
        if let Some(reason) = runtime_profile_refusal(&self.context.roots, profile) {
            return Err(reason);
        }
        launch_bridge::inputs(
            &self.context.selected,
            &CanonicalIdentityStatus::Unknown,
            &self.context.inspection,
        )
        .ok_or("Wii U content changed or is no longer a supported extracted base")?;
        Ok(())
    }
    pub(super) fn execute(self) -> Result<archivefs_core::launch::LaunchedCemuProcess, String> {
        self.validate()?;
        let environment =
            CemuProfileDiscoveryRoots::from_environment().map_err(|e| format!("{e:?}"))?;
        if environment.home != self.context.roots.home
            || environment.xdg_config_home != self.context.roots.xdg_config_home
        {
            return Err("Cemu runtime configuration environment changed; refresh readiness".into());
        }
        archivefs_core::launch::cemu_readiness::launch_previewed_cemu(
            &self.context.inspection,
            &self.profile_id,
            &self.context.roots,
        )
        .map_err(|e| format!("{e:?}"))
    }
}

fn runtime_profile_refusal(
    roots: &CemuProfileDiscoveryRoots,
    profile: &archivefs_core::launch::cemu_readiness::CemuProfilePreview,
) -> Option<String> {
    if !cfg!(target_os = "linux") {
        return Some("Blue Cemu launch currently supports native Linux profiles only".into());
    }
    if std::env::var_os("APPIMAGE").is_some()
        || profile.profile_path != roots.xdg_config_home.join("Cemu")
    {
        return Some(
            "Cemu launch requires the native default XDG profile without inherited APPIMAGE mode"
                .into(),
        );
    }
    let Some(executable) = profile.executable.as_ref() else {
        return Some("No Cemu executable is bound to this profile".into());
    };
    for path in [executable.as_path(), profile.profile_path.as_path()] {
        if !path.is_absolute() {
            return Some("Cemu profile and executable must use absolute paths".into());
        }
        let mut prefix = PathBuf::new();
        for component in path.components() {
            if !matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            ) {
                return Some("Cemu profile or executable path is not safely resolved".into());
            }
            prefix.push(component);
            if !std::fs::symlink_metadata(&prefix).is_ok_and(|m| !m.file_type().is_symlink()) {
                return Some(
                    "Cemu profile or executable has missing or symlinked components".into(),
                );
            }
        }
    }
    let Some(parent) = executable.parent() else {
        return Some("Cemu executable has no parent directory".into());
    };
    if !matches!(std::fs::symlink_metadata(parent.join("portable")),Err(e) if e.kind()==std::io::ErrorKind::NotFound)
    {
        return Some("Cemu portable mode may override the inspected XDG profile; use a native installation without an adjacent portable directory".into());
    }
    // Wrappers and AppImages may redirect configuration. The AppImage spec
    // defines AI\x01/AI\x02 at offset 8, independent of filename:
    // https://github.com/AppImage/AppImageSpec/blob/master/draft.md
    use std::io::Read;
    let mut header = [0u8; 16];
    if std::fs::File::open(executable)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_err()
        || &header[..4] != b"\x7fELF"
        || &header[8..10] == b"AI"
        || executable
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("AppImage"))
    {
        return Some("Blue Cemu launch requires a native ELF executable; wrapper and AppImage runtime profiles are not yet bound".into());
    }
    None
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use archivefs_core::launch::{self, FirmwareReadiness, StandaloneProfileInput};
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    // Discovery reads PATH. Re-exec only this test binary with disposable roots;
    // never mutate the parallel test runner's environment or execute Cemu.
    fn isolated(name: &str, run: impl FnOnce(&Path)) {
        if let Some(root) = std::env::var_os("EMUWIZ_CEMU_BRIDGE_FIXTURE") {
            run(Path::new(&root));
            return;
        }
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("bin")).unwrap();
        let exe = root.path().join("bin/Cemu");
        let mut header = [0u8; 64];
        header[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        fs::write(&exe, header).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env_clear()
            // All environment roots below are disposable; keep the harness
            // constructor from replacing these deliberately supplied roots.
            .env("EMUWIZ_TEST_REAL_ENVIRONMENT", "1")
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .env("XDG_STATE_HOME", root.path().join("state"))
            .env("XDG_RUNTIME_DIR", root.path().join("runtime"))
            .env("TMPDIR", root.path())
            .args(["--exact", name, "--nocapture"])
            .env("EMUWIZ_CEMU_BRIDGE_FIXTURE", root.path())
            .env("HOME", root.path().join("home"))
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("PATH", root.path().join("bin"))
            .env_remove("APPIMAGE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
    fn fixture(root: &Path) -> Context {
        let title = root.join("title");
        for dir in ["code", "content", "meta"] {
            fs::create_dir_all(title.join(dir)).unwrap();
        }
        let mut rpx = vec![0u8; 232];
        rpx[..9].copy_from_slice(b"\x7fELF\x01\x02\x01\xca\xfe");
        rpx[18..24].copy_from_slice(b"\0\x14\0\0\0\x01");
        rpx[32..36].copy_from_slice(&52u32.to_be_bytes());
        rpx[46..48].copy_from_slice(&40u16.to_be_bytes());
        rpx[48..50].copy_from_slice(&2u16.to_be_bytes());
        fs::write(title.join("code/game.rpx"), rpx).unwrap();
        fs::write(
            title.join("meta/meta.xml"),
            "<menu><title_id>0005000010101A00</title_id></menu>",
        )
        .unwrap();
        let profile = root.join("config/Cemu");
        fs::create_dir_all(&profile).unwrap();
        fs::create_dir_all(root.join("mlc")).unwrap();
        fs::write(
            profile.join("settings.xml"),
            format!(
                "<content><mlc_path>{}</mlc_path></content>",
                root.join("mlc").display()
            ),
        )
        .unwrap();
        let roots = CemuProfileDiscoveryRoots::from_environment().unwrap();
        let inspection =
            archivefs_core::launch::cemu_readiness::inspect_cemu_readiness(&title, None, &roots);
        Context::new(title.join("code/game.rpx"), inspection, roots)
    }
    fn plan(context: &Context) -> LaunchPlan {
        let (identity, content) = context.inputs(&CanonicalIdentityStatus::Unknown).unwrap();
        let profiles: Vec<_> = context
            .inspection
            .profiles
            .iter()
            .map(|p| StandaloneProfileInput {
                adapter_id: "cemu",
                profile_id: p.profile_id.clone(),
                profile_path: Some(p.profile_path.clone()),
                eligible: p.command.is_some() && context.profile_is_bound(p),
                firmware: FirmwareReadiness::NotRequired,
            })
            .collect();
        let environment =
            archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
                format_version: 2,
                profiles: vec![],
                diagnostics: vec![],
            };
        let mut plan = launch::build_launch_plan(&identity, &content, &profiles, &environment, &[]);
        archivefs_core::launch::cemu_readiness::apply_cemu_readiness(
            &mut plan,
            Some(&context.inspection),
        );
        context.apply(&mut plan);
        plan
    }
    fn input(plan: LaunchPlan, context: Context) -> LaunchReadinessInput {
        LaunchReadinessInput::Plan {
            plan,
            cemu: Some(context),
            flycast: None,
            xroar: None,
            vice: None,
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
            amiga_whdload: None,
            shadps4: None,
        }
    }
    #[test]
    fn cemu_blue_dispatch_binds_preview_without_starting_a_process() {
        isolated(
            "launch_readiness_page::cemu::tests::cemu_blue_dispatch_binds_preview_without_starting_a_process",
            |root| {
                let context = fixture(root);
                let plan = plan(&context);
                assert_eq!(plan.summary.ready, 1, "{plan:?}");
                let request = context.request(&plan, &plan.candidates[0]).unwrap();
                request.validate().unwrap();
                let command = context.inspection.profiles[0].command.as_ref().unwrap();
                assert_eq!(
                    command.arguments,
                    vec![
                        std::ffi::OsString::from("-g"),
                        root.join("title/code/game.rpx").into_os_string()
                    ]
                );
                assert!(matches!(
                    gamer_play_action(&input(plan, context)),
                    GamerPlayAction::Launch(_)
                ));
                // Stat/format changes after preview are rejected before the existing executor.
                fs::write(root.join("title/code/game.rpx"), b"not a Cafe executable").unwrap();
                assert!(request.validate().is_err());
            },
        );
    }
    #[test]
    fn cemu_actual_blue_builder_uses_selection_evidence_and_preserves_conflicts() {
        isolated(
            "launch_readiness_page::cemu::tests::cemu_actual_blue_builder_uses_selection_evidence_and_preserves_conflicts",
            |root| {
                use crate::{LoadState, selected_evidence_page as evidence};
                use archivefs_core::{
                    game_identity::*,
                    platform_evidence_fusion::{identity_orchestrator, identity_presentation},
                };
                let context = fixture(root);
                let selected = context.selected.clone();
                let mut app = crate::tests::app_for_operation_tests();
                app.archive_context.select_only(selected.clone());
                app.emulator_readiness.cemu =
                    crate::emulator_setup::cemu::Controller::with_test_context(context);
                let metadata = fs::metadata(&selected).unwrap();
                let mut identity =
                    archivefs_core::ArchiveIdentity::from_path(&selected, root, Some(&metadata));
                identity.platform = Some("WiiU".into());
                identity.platform_provenance =
                    Some(archivefs_core::PlatformProvenance::RegistryDetector);
                let archive = archivefs_core::Archive {
                    path: selected.clone(),
                    kind: archivefs_core::ArchiveKind::DirectGameImage,
                    identity,
                    health: archivefs_core::ArchiveHealth::Pending,
                };
                if let LoadState::Ready(data) = &mut app.state {
                    data.records = vec![archivefs_core::ArchiveRecord::new(
                        archivefs_core::MountPlan::new(archive, root.join("mount")),
                        archivefs_core::MountState::NotMountable,
                        archivefs_core::ArchiveMetadata::empty(),
                        archivefs_core::ArchiveHealth::Pending,
                    )];
                }
                let result = identity_orchestrator::inspect_identity(
                    identity_orchestrator::IdentityInspectionInput::default(),
                );
                let mut game = inspect_catalogued_game_identity(&selected, Some("WiiU"));
                game.platform = IdentityPlatform::WiiU;
                assert!(matches!(
                    launch::canonical_identity_from_game_report(&game).0,
                    CanonicalIdentityStatus::Unknown
                ));
                app.selected_evidence_ui.selected_evidence =
                    evidence::SelectedEvidenceState::Ready {
                        generation: 0,
                        hasheous: evidence::HasheousState::default(),
                        report: Box::new(evidence::SelectedEvidenceReport {
                            path: selected.clone(),
                            tape_analysis: None,
                            structural_facts: vec![],
                            identity: identity_presentation::present_identity(&result),
                            identity_result: result,
                            dat_identity: None,
                            game_identity_report: game,
                            hashes: None,
                            no_intro: evidence::NoIntroLookupResult::NotImported,
                            enrichment: evidence::SelectedEvidenceEnrichmentStatus::Complete,
                            base_observations: vec![],
                            structural_media: None,
                        }),
                    };
                let build = |app: &crate::ArchiveFsApp| {
                    let LoadState::Ready(data) = &app.state else {
                        panic!("fixture")
                    };
                    app.build_launch_readiness_input(Some(data))
                };
                let input = build(&app);
                let LaunchReadinessInput::Plan { plan, .. } = &input else {
                    panic!("Blue must bridge the selected extracted RPX")
                };
                assert_eq!(plan.game_key.as_deref(), Some("0005000010101A00"));
                assert!(
                    matches!(gamer_play_action(&input),GamerPlayAction::Launch(request) if matches!(*request,TypedLaunchRequest::Standalone(StandaloneLaunchRequest::Cemu(_))))
                );
                if let evidence::SelectedEvidenceState::Ready { report, .. } =
                    &mut app.selected_evidence_ui.selected_evidence
                {
                    report.game_identity_report.platform = IdentityPlatform::PlayStation;
                }
                assert!(!matches!(
                    gamer_play_action(&build(&app)),
                    GamerPlayAction::Launch(_)
                ));
                if let evidence::SelectedEvidenceState::Ready { report, .. } =
                    &mut app.selected_evidence_ui.selected_evidence
                {
                    report.game_identity_report.platform = IdentityPlatform::WiiU;
                    report.game_identity_report.evidence = ["a", "b"]
                        .into_iter()
                        .map(|value| IdentityEvidence {
                            kind: IdentityKind::LooseRomSha256,
                            status: IdentityStatus::Verified,
                            value: Some(value.into()),
                            confidence: IdentityConfidence::StructuredMetadata,
                            provenance: IdentityProvenance {
                                archive_path: selected.clone(),
                                member_path: None,
                                member_index: None,
                                method: "synthetic conflict".into(),
                            },
                            diagnostic: "fixture only".into(),
                        })
                        .collect();
                }
                assert!(matches!(
                    build(&app),
                    LaunchReadinessInput::IdentityConflicting
                ));
            },
        );
    }
    #[test]
    fn cemu_lowercase_title_metadata_produces_the_same_bound_blue_request() {
        isolated(
            "launch_readiness_page::cemu::tests::cemu_lowercase_title_metadata_produces_the_same_bound_blue_request",
            |root| {
                let original = fixture(root);
                fs::write(
                    root.join("title/meta/meta.xml"),
                    "<menu><title_id>0005000010101a00</title_id></menu>",
                )
                .unwrap();
                let inspection = archivefs_core::launch::cemu_readiness::inspect_cemu_readiness(
                    &root.join("title"),
                    None,
                    &original.roots,
                );
                let context = Context::new(original.selected, inspection, original.roots);
                let mut plan = plan(&context);
                assert_eq!(plan.summary.ready, 1);
                assert_eq!(plan.game_key.as_deref(), Some("0005000010101A00"));
                assert_eq!(
                    context.inspection.profiles[0]
                        .command
                        .as_ref()
                        .unwrap()
                        .selection
                        .title_id
                        .as_deref(),
                    Some("0005000010101a00")
                );
                assert!(
                    context.request(&plan, &plan.candidates[0]).is_some(),
                    "hexadecimal TITLE_ID case must not disable the same exact title"
                );
                plan.game_key = Some("0005000012345678".into());
                assert!(
                    context.request(&plan, &plan.candidates[0]).is_none(),
                    "case tolerance must not accept a different title"
                );
                plan.game_key = None;
                assert!(
                    context.request(&plan, &plan.candidates[0]).is_none(),
                    "missing identity must not match"
                );
            },
        );
    }
    #[test]
    fn cemu_inherited_appimage_cannot_execute_a_prepared_request() {
        let name = "launch_readiness_page::cemu::tests::cemu_inherited_appimage_cannot_execute_a_prepared_request";
        isolated(name, |root| {
            if std::env::var_os("APPIMAGE").is_none() {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", name, "--nocapture"])
                    .env("APPIMAGE", root.join("parent.AppImage"))
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
                return;
            }
            let context = fixture(root);
            let profile = context
                .inspection
                .profiles
                .iter()
                .find(|p| p.command.is_some())
                .unwrap();
            // Model the request retained from Preview before the parent
            // environment changed. Validation must still reject it live.
            let request = Request {
                profile_id: profile.profile_id.clone(),
                context,
            };
            assert!(request.validate().unwrap_err().contains("APPIMAGE"));
            assert!(matches!(request.execute(), Err(reason) if reason.contains("APPIMAGE")));
        });
    }
    #[test]
    fn cemu_runtime_changes_and_unbound_profiles_refuse_blue_launch() {
        isolated(
            "launch_readiness_page::cemu::tests::cemu_runtime_changes_and_unbound_profiles_refuse_blue_launch",
            |root| {
                let context = fixture(root);
                let plan = plan(&context);
                let request = context.request(&plan, &plan.candidates[0]).unwrap();
                let mut empty_xdg = context.roots.clone();
                empty_xdg.xdg_config_home = PathBuf::new();
                assert!(
                    runtime_profile_refusal(&empty_xdg, &context.inspection.profiles[0])
                        .unwrap()
                        .contains("XDG")
                );
                fs::create_dir(root.join("bin/portable")).unwrap();
                assert!(request.validate().unwrap_err().contains("portable"));
                let updated = Context::new(
                    context.selected.clone(),
                    (*context.inspection).clone(),
                    context.roots.clone(),
                );
                let blocked = self::plan(&updated);
                assert_eq!(blocked.summary.ready, 0);
                assert!(
                    blocked.candidates[0]
                        .blockers
                        .iter()
                        .any(|b| b.detail.contains("portable"))
                );
                assert!(!matches!(
                    gamer_play_action(&input(blocked, updated)),
                    GamerPlayAction::Launch(_)
                ));
                fs::remove_dir(root.join("bin/portable")).unwrap();
                let mut appimage = fs::read(root.join("bin/Cemu")).unwrap();
                appimage[8..11].copy_from_slice(b"AI\x02");
                fs::write(root.join("bin/Cemu"), appimage).unwrap();
                assert!(request.validate().unwrap_err().contains("AppImage"));
                let mut profile = context.inspection.profiles[0].clone();
                profile.profile_path = root.join("unselected-profile");
                assert!(
                    runtime_profile_refusal(&context.roots, &profile)
                        .unwrap()
                        .contains("XDG")
                );
            },
        );
    }
}
