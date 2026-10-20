//! Native melonDS launch from the Playing Library: request building from the
//! readiness plan, then the core's fresh preflight and spawn, against a fake
//! melonDS script and private temp roots. No real emulator, ROM or user file
//! is involved.

use super::*;
use archivefs_core::launch::{LaunchContentKind, LaunchContentRef, LaunchTarget};
use archivefs_core::patch_manager::{MelonDsProfileDiscoveryRoots, discover_melonds_profiles};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

const KEY: &str = "DS-TEST";

struct Lab {
    dir: tempfile::TempDir,
    roots: MelonDsProfileDiscoveryRoots,
    content: PathBuf,
    exe: PathBuf,
    config: PathBuf,
}

impl Lab {
    fn new(exit_code: i32, config: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("melonDS.toml");
        std::fs::write(&config_path, config).unwrap();
        let exe = dir.path().join("melonDS");
        write_exe(&exe, exit_code);
        let content = dir.path().join("A DS Game.nds");
        std::fs::write(&content, rom_bytes()).unwrap();
        let roots = MelonDsProfileDiscoveryRoots {
            home: dir.path().to_path_buf(),
            xdg_config_home: dir.path().join("unused"),
            explicit_configuration_roots: vec![config_dir],
            portable_configuration_roots: vec![],
            explicit_executables: vec![exe.clone()],
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        };
        Self {
            dir,
            roots,
            content,
            exe,
            config: config_path,
        }
    }

    fn argv(&self) -> PathBuf {
        PathBuf::from(format!("{}.argv", self.exe.display()))
    }

    fn context(&self, key: Option<&str>) -> MelonDsLaunchContext {
        MelonDsLaunchContext {
            discovery: discover_melonds_profiles(&self.roots),
            roots: self.roots.clone(),
            verified_game_key: key.map(str::to_string),
        }
    }

    fn candidate(&self, context: &MelonDsLaunchContext) -> LaunchCandidate {
        let profile_id = context.discovery.profiles[0].profile_id.clone();
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "melonds",
                profile_id,
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::Cartridge),
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(self.content.clone()),
                requires_mount: false,
                provenance: "test".into(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: Vec::new(),
            warnings: Vec::new(),
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::SoleEligible,
        }
    }

    fn request(&self) -> Option<StandaloneLaunchRequest> {
        let context = self.context(Some(KEY));
        self.request_from(&context, &self.candidate(&context))
    }

    fn request_from(
        &self,
        context: &MelonDsLaunchContext,
        candidate: &LaunchCandidate,
    ) -> Option<StandaloneLaunchRequest> {
        let mut plan = plan_for(candidate.clone());
        plan.platform_id = Some("Nintendo DS".into());
        plan.game_key = Some(KEY.into());
        standalone_launch_request(
            &plan,
            candidate,
            None,
            None,
            None,
            None,
            None,
            Some(context),
        )
    }
}

fn plan_for(candidate: LaunchCandidate) -> LaunchPlan {
    LaunchPlan {
        platform_id: Some("Nintendo DS".into()),
        game_key: Some(KEY.into()),
        summary: archivefs_core::launch::LaunchPlanSummary {
            candidates: 1,
            ready: 1,
            ready_with_warnings: 0,
            blocked: 0,
        },
        candidates: vec![candidate],
        media_topology: None,
    }
}

fn write_exe(path: &Path, exit_code: i32) {
    // Records what melonDS was given. The cheat workspace is only inspected
    // when XDG_CONFIG_HOME points into an EmuWiz `cheats-*` workspace, so a
    // developer's real settings are never read.
    std::fs::write(
        path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.argv\"\n\
case \"$XDG_CONFIG_HOME\" in *cheats-*)\n\
  cp \"$XDG_CONFIG_HOME/melonDS/melonDS.toml\" \"$0.toml\"\n\
  d=$(sed -n 's/^CheatFilePath = \"\\(.*\\)\"$/\\1/p' \"$XDG_CONFIG_HOME/melonDS/melonDS.toml\")\n\
  cp \"$d\"/*.mch \"$0.seen\";;\n\
esac\necho oops >&2\nexit {exit_code}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A minimal Nintendo DS header: the game code sits at 0x0C.
fn rom_bytes() -> Vec<u8> {
    let mut rom = vec![0u8; 0x200];
    rom[0x0C..0x10].copy_from_slice(b"EWZP");
    rom
}

fn direct_boot() -> &'static str {
    "Emu.DirectBoot = true\n"
}

fn run(
    request: StandaloneLaunchRequest,
) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
    match request.execute()? {
        StandaloneProcess::MelonDs(process) => Ok(process),
        _ => panic!("a melonDS request must start a melonDS process"),
    }
}

fn refused(request: StandaloneLaunchRequest) -> String {
    match run(request) {
        Err(error) => error,
        Ok(_) => panic!("the launch must be refused"),
    }
}

fn wait(
    process: &mut archivefs_core::launch::process_spawn::WatchedProcess,
) -> archivefs_core::launch::process_spawn::ProcessExitReport {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(report) = process.poll() {
            return ProcessExitReportCopy::of(report);
        }
        assert!(Instant::now() < deadline, "fake melonDS did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `ProcessExitReport` is not `Clone`; keep what the assertions need.
struct ProcessExitReportCopy;
impl ProcessExitReportCopy {
    fn of(
        report: &archivefs_core::launch::process_spawn::ProcessExitReport,
    ) -> archivefs_core::launch::process_spawn::ProcessExitReport {
        archivefs_core::launch::process_spawn::ProcessExitReport {
            status: match &report.status {
                Ok(status) => Ok(*status),
                Err(error) => Err(std::io::Error::new(error.kind(), error.to_string())),
            },
            stderr: report.stderr.clone(),
        }
    }
}

#[test]
fn a_verified_ready_ds_game_launches_the_exact_rom_in_the_native_emulator() {
    let lab = Lab::new(0, direct_boot());
    let request = lab.request().expect("ready DS candidate builds a request");
    assert_eq!(request.key(), (lab.content.clone(), "melonds".to_string()));
    assert_eq!(request.adapter_name(), "melonDS");
    let mut process = run(request).unwrap();
    let report = wait(&mut process);
    assert!(report.status.unwrap().success());
    let argv = std::fs::read_to_string(lab.argv()).unwrap();
    assert_eq!(argv.trim_end(), lab.content.to_str().unwrap());
    // Nothing the user owns was touched.
    assert_eq!(std::fs::read(&lab.content).unwrap(), rom_bytes());
    assert_eq!(std::fs::read_to_string(&lab.config).unwrap(), direct_boot());
}

#[test]
fn no_request_without_a_matching_independently_verified_game_key() {
    let lab = Lab::new(0, direct_boot());
    for key in [None, Some("SOMETHING-ELSE"), Some("")] {
        let context = lab.context(key);
        assert!(
            lab.request_from(&context, &lab.candidate(&context))
                .is_none(),
            "key {key:?} must not authorise a launch"
        );
    }
}

#[test]
fn unready_mounted_archived_or_wrong_container_content_never_builds_a_request() {
    let lab = Lab::new(0, direct_boot());
    let context = lab.context(Some(KEY));
    let build = |edit: &dyn Fn(&mut LaunchCandidate)| {
        let mut candidate = lab.candidate(&context);
        edit(&mut candidate);
        lab.request_from(&context, &candidate)
    };
    assert!(build(&|c| c.readiness = LaunchReadiness::Blocked).is_none());
    assert!(build(&|c| c.readiness = LaunchReadiness::ReadyWithWarnings).is_none());
    assert!(build(&|c| c.content.requires_mount = true).is_none());
    assert!(build(&|c| c.content.container = Some(LaunchContainerKind::Archive)).is_none());
    assert!(build(&|c| c.content.container = Some(LaunchContainerKind::CueBin)).is_none());
    assert!(build(&|c| c.content.resolved_path = None).is_none());
    assert!(build(&|_| {}).is_some());
}

#[test]
fn a_symlinked_rom_is_not_offered_for_launch() {
    let lab = Lab::new(0, direct_boot());
    let real = lab.dir.path().join("real.nds");
    std::fs::rename(&lab.content, &real).unwrap();
    std::os::unix::fs::symlink(&real, &lab.content).unwrap();
    assert!(lab.request().is_none());
}

#[test]
fn a_rom_changed_after_authorisation_is_refused_and_melonds_never_starts() {
    let lab = Lab::new(0, direct_boot());
    let request = lab.request().unwrap();
    std::fs::write(&lab.content, b"a different and longer nds fixture").unwrap();
    let error = refused(request);
    assert!(error.contains("melonDS was not started"), "{error}");
    assert!(error.contains("ContentChangedBeforeSpawn"), "{error}");
    assert!(!lab.argv().exists());
}

#[test]
fn a_replaced_executable_is_refused() {
    let lab = Lab::new(0, direct_boot());
    let request = lab.request().unwrap();
    write_exe(&lab.exe, 5);
    std::fs::write(&lab.exe, b"#!/bin/sh\nexit 9\n# changed\n").unwrap();
    let error = refused(request);
    assert!(error.contains("BindingDrift"), "{error}");
    assert!(!lab.argv().exists());
}

#[test]
fn a_changed_configuration_is_refused() {
    let lab = Lab::new(0, direct_boot());
    let request = lab.request().unwrap();
    std::fs::write(&lab.config, "Emu.DirectBoot = true\nExtra = 1\n").unwrap();
    let error = refused(request);
    assert!(error.contains("ConfigurationChanged"), "{error}");
    assert!(!lab.argv().exists());
}

#[test]
fn an_identity_that_no_longer_matches_is_refused() {
    let lab = Lab::new(0, direct_boot());
    let Some(StandaloneLaunchRequest::MelonDs(mut request, roots)) = lab.request() else {
        panic!("expected a melonDS request");
    };
    request.expected_platform_id = "Game Boy Advance".into();
    let error = refused(StandaloneLaunchRequest::MelonDs(request, roots));
    assert!(error.contains("IdentityMismatch"), "{error}");
    assert!(!lab.argv().exists());
}

#[test]
fn missing_bios_in_external_firmware_mode_is_refused_before_launch() {
    let lab = Lab::new(0, "Emu.DirectBoot = false\nEmu.ExternalBIOSEnable = true\n");
    // Build the request directly: readiness would already show the BIOS
    // blocker, and the core must still refuse if it were bypassed.
    let context = lab.context(Some(KEY));
    let request = lab.request_from(&context, &lab.candidate(&context));
    let error = match request {
        None => panic!("the request builder must not hide the core's firmware refusal"),
        Some(request) => refused(request),
    };
    assert!(error.contains("FirmwareUnavailable"), "{error}");
    assert!(!lab.argv().exists());
}

#[test]
fn an_unavailable_executable_is_refused() {
    let lab = Lab::new(0, direct_boot());
    let request = lab.request().unwrap();
    std::fs::remove_file(&lab.exe).unwrap();
    let error = refused(request);
    assert!(error.contains("melonDS was not started"), "{error}");
    assert!(!lab.argv().exists());
}

#[test]
fn a_missing_rom_is_refused() {
    let lab = Lab::new(0, direct_boot());
    let request = lab.request().unwrap();
    std::fs::remove_file(&lab.content).unwrap();
    let error = refused(request);
    assert!(error.contains("ContentNotFound"), "{error}");
    assert!(!lab.argv().exists());
}

#[test]
fn a_process_failure_is_reported_as_a_failure_with_its_stderr() {
    let lab = Lab::new(3, direct_boot());
    let mut process = run(lab.request().unwrap()).unwrap();
    let report = wait(&mut process);
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(3));
    let line = melonds_exit_line(&report);
    assert!(line.contains("melonDS failed"), "{line}");
    assert!(line.contains("oops"), "{line}");
    assert!(!line.contains("exited normally"), "{line}");
}

#[test]
fn a_clean_exit_never_claims_the_game_ran() {
    let lab = Lab::new(0, direct_boot());
    let mut process = run(lab.request().unwrap()).unwrap();
    let report = wait(&mut process);
    let line = melonds_exit_line(&report);
    assert!(line.contains("does not prove the game ran"), "{line}");
}

#[test]
fn the_shared_launch_state_reports_a_refusal_as_a_failed_launch() {
    let lab = Lab::new(0, direct_boot());
    let mut state = StandaloneLaunchState::default();
    let request = lab.request().unwrap();
    std::fs::write(&lab.content, b"changed after authorisation!").unwrap();
    state.start(request);
    let deadline = Instant::now() + Duration::from_secs(20);
    while state.is_active() {
        assert!(Instant::now() < deadline);
        state.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    state.poll();
    let detail = state.failure_detail().expect("stale ROM must be refused");
    assert!(detail.contains("melonDS was not started"), "{detail}");
    assert!(!lab.argv().exists());
}

// ---- Stage 2: optional per-launch cheats ---------------------------------

use super::melonds_cheats::{Choice, activation_label, session_summary, unsupported_reason};
use archivefs_core::launch::cheat_runtime::CheatRuntimeRoots;

const MCH: &str = "CAT 0 Gameplay\nCODE 0 Free Play\n02000000 00000001\nCODE 0 Other\n02000004 00000002\n\nCAT 1 Exclusive\nCODE 0 Alpha\n02000008 00000003\nCODE 0 Beta\n0200000C 00000004\n";

impl Lab {
    fn cheat_file(&self, body: &str) -> PathBuf {
        let dir = self.dir.path().join("mine");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("A DS Game.mch");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn cheat_root(&self) -> PathBuf {
        self.dir.path().join("cheat-root")
    }

    fn state_with(&self, choice: Option<Choice>) -> StandaloneLaunchState {
        let mut state = StandaloneLaunchState::default();
        state.melonds_cheats = choice;
        state.cheat_roots = Some(CheatRuntimeRoots {
            approved_root: self.cheat_root(),
        });
        state
    }

    fn seen(&self) -> PathBuf {
        PathBuf::from(format!("{}.seen", self.exe.display()))
    }
}

fn tick(choice: &mut Choice, names: &[&str]) {
    for entry in &mut choice.entries {
        entry.chosen = names.contains(&entry.name.as_str());
    }
}

fn run_state(state: &mut StandaloneLaunchState, request: StandaloneLaunchRequest) {
    state.start(request);
    let deadline = Instant::now() + Duration::from_secs(20);
    while state.is_active() {
        assert!(Instant::now() < deadline, "launch did not finish");
        state.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    state.poll();
}

fn leftovers(root: &Path) -> usize {
    std::fs::read_dir(root).map(|d| d.count()).unwrap_or(0)
}

#[test]
fn nothing_is_ticked_by_default_and_unsupported_cheats_cannot_be_ticked() {
    let lab = Lab::new(0, direct_boot());
    let source = lab.cheat_file(&format!("{MCH}\nCODE 0 Empty\n"));
    let choice = Choice::load(&lab.content, &source);
    // An entry with no code makes the file malformed for the core; either the
    // file is refused outright or the entry is shown but unavailable.
    match choice {
        Err(error) => assert!(error.contains("not a usable"), "{error}"),
        Ok(mut choice) => {
            assert_eq!(choice.chosen_count(), 0);
            for entry in &mut choice.entries {
                entry.chosen = true;
            }
            let empty = choice.entries.iter().find(|e| e.name == "Empty");
            assert!(empty.is_none_or(|e| e.unsupported.is_some()));
        }
    }
    let good = Choice::load(&lab.content, &lab.cheat_file(MCH)).unwrap();
    assert_eq!(good.chosen_count(), 0, "nothing is ticked by default");
    assert!(
        good.chosen_names()
            .unwrap_err()
            .contains("Tick at least one")
    );
}

#[test]
fn a_cheat_entry_without_code_is_visible_but_unavailable() {
    let lab = Lab::new(0, direct_boot());
    let mut choice = Choice::load(&lab.content, &lab.cheat_file(MCH)).unwrap();
    choice.entries[0].unsupported = Some("it has no code");
    choice.entries[0].chosen = true;
    choice.entries[1].chosen = true;
    assert_eq!(choice.chosen_names().unwrap(), vec!["Other".to_string()]);
}

#[test]
fn malformed_oversized_linked_and_misnamed_cheat_files_are_refused_before_any_launch() {
    let lab = Lab::new(0, direct_boot());
    let bad = lab.cheat_file("CODE 0 NoCategoryNoCode\nnot hex\n");
    assert!(Choice::load(&lab.content, &bad).is_err());
    let binary = lab.cheat_file("");
    std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
    assert!(Choice::load(&lab.content, &binary).is_err());
    let big = lab.cheat_file(&"# x\n".repeat(200_000));
    assert!(
        Choice::load(&lab.content, &big)
            .unwrap_err()
            .contains("larger")
    );
    let real = lab.cheat_file(MCH);
    let wrong_name = real.with_file_name("Other Game.mch");
    std::fs::copy(&real, &wrong_name).unwrap();
    let error = Choice::load(&lab.content, &wrong_name).unwrap_err();
    assert!(error.contains("A DS Game.mch"), "{error}");
    std::fs::remove_file(&real).unwrap();
    let target = lab.dir.path().join("target.mch");
    std::fs::write(&target, MCH).unwrap();
    std::os::unix::fs::symlink(&target, &real).unwrap();
    assert!(
        Choice::load(&lab.content, &real)
            .unwrap_err()
            .contains("regular file")
    );
}

#[test]
fn ticked_cheats_reach_melonds_and_unticked_ones_do_not() {
    let lab = Lab::new(0, direct_boot());
    let source = lab.cheat_file(MCH);
    let mut choice = Choice::load(&lab.content, &source).unwrap();
    tick(&mut choice, &["Other", "Beta"]);
    let mut state = lab.state_with(Some(choice));
    run_state(&mut state, lab.request().unwrap());
    assert_eq!(state.failure_detail(), None);
    let seen = std::fs::read_to_string(lab.seen()).unwrap();
    assert!(seen.contains("Other") && seen.contains("Beta"), "{seen}");
    assert!(
        !seen.contains("Free Play") && !seen.contains("Alpha"),
        "unticked cheats must not be passed: {seen}"
    );
    // The launch config switched cheats on, in the temporary copy only.
    let toml = std::fs::read_to_string(format!("{}.toml", lab.exe.display())).unwrap();
    assert!(toml.contains("EnableCheats = true"), "{toml}");
    // Originals untouched; the temporary workspace is gone after exit.
    assert_eq!(std::fs::read_to_string(&source).unwrap(), MCH);
    assert_eq!(std::fs::read(&lab.content).unwrap(), rom_bytes());
    assert_eq!(std::fs::read_to_string(&lab.config).unwrap(), direct_boot());
    assert_eq!(leftovers(&lab.cheat_root()), 0);
    let Some((_, _, StandaloneLaunchStage::Exited(StandaloneProcess::MelonDsCheats(session)))) =
        state.tracked.as_ref()
    else {
        panic!("a cheat launch must end in the cheat-session exit state");
    };
    let lines = session_summary(session).join("\n");
    assert!(lines.contains("exited normally"), "{lines}");
    assert!(lines.contains("did not see the cheats work"), "{lines}");
    assert!(activation_label().contains("has not seen one work"));
}

#[test]
fn an_ordinary_launch_without_a_cheat_file_is_unchanged() {
    let lab = Lab::new(0, direct_boot());
    let mut state = lab.state_with(None);
    run_state(&mut state, lab.request().unwrap());
    assert_eq!(state.failure_detail(), None);
    assert!(lab.argv().exists());
    assert!(
        !lab.seen().exists(),
        "no cheat workspace for a plain launch"
    );
    assert!(matches!(
        state.tracked.as_ref(),
        Some((
            _,
            _,
            StandaloneLaunchStage::Exited(StandaloneProcess::MelonDs(_))
        ))
    ));
    assert_eq!(leftovers(&lab.cheat_root()), 0);
}

#[test]
fn a_cheat_file_changed_after_it_was_chosen_is_refused_and_melonds_never_starts() {
    let lab = Lab::new(0, direct_boot());
    let source = lab.cheat_file(MCH);
    let mut choice = Choice::load(&lab.content, &source).unwrap();
    tick(&mut choice, &["Other"]);
    // Same size, different bytes: only a content hash can notice.
    std::fs::write(&source, MCH.replace("00000002", "000000FF")).unwrap();
    let mut state = lab.state_with(Some(choice));
    run_state(&mut state, lab.request().unwrap());
    let detail = state.failure_detail().expect("must be refused");
    assert!(detail.contains("melonDS was not started"), "{detail}");
    assert!(detail.contains("changed after you chose"), "{detail}");
    assert!(!lab.argv().exists());
    assert_eq!(leftovers(&lab.cheat_root()), 0);
}

#[test]
fn a_rom_changed_after_authorisation_is_refused_even_with_cheats() {
    let lab = Lab::new(0, direct_boot());
    let mut choice = Choice::load(&lab.content, &lab.cheat_file(MCH)).unwrap();
    tick(&mut choice, &["Other"]);
    let request = lab.request().unwrap();
    let mut rom = rom_bytes();
    rom.extend_from_slice(b"tampered");
    std::fs::write(&lab.content, rom).unwrap();
    let mut state = lab.state_with(Some(choice));
    run_state(&mut state, request);
    let detail = state.failure_detail().expect("must be refused");
    assert!(
        detail.contains("changed since the launch was authorised"),
        "{detail}"
    );
    assert!(!lab.argv().exists());
    assert_eq!(leftovers(&lab.cheat_root()), 0);
}

#[test]
fn a_cheat_choice_for_a_different_game_is_refused() {
    let lab = Lab::new(0, direct_boot());
    let mut choice = Choice::load(&lab.content, &lab.cheat_file(MCH)).unwrap();
    tick(&mut choice, &["Other"]);
    let Some(StandaloneLaunchRequest::MelonDs(mut request, roots)) = lab.request() else {
        panic!("expected a melonDS request");
    };
    request.selected_content_path = lab.dir.path().join("Another Game.nds");
    let outcome = super::melonds_cheats::launch(
        &request,
        &roots,
        &choice,
        &CheatRuntimeRoots {
            approved_root: lab.cheat_root(),
        },
    );
    let Err(error) = outcome else {
        panic!("a choice for another game must be refused");
    };
    assert!(error.contains("different game"), "{error}");
    assert!(!lab.argv().exists());
}

#[test]
fn a_wrong_platform_identity_is_refused_with_cheats_too() {
    let lab = Lab::new(0, direct_boot());
    let mut choice = Choice::load(&lab.content, &lab.cheat_file(MCH)).unwrap();
    tick(&mut choice, &["Other"]);
    let Some(StandaloneLaunchRequest::MelonDs(mut request, roots)) = lab.request() else {
        panic!("expected a melonDS request");
    };
    request.expected_platform_id = "Game Boy Advance".into();
    let mut state = lab.state_with(Some(choice));
    run_state(&mut state, StandaloneLaunchRequest::MelonDs(request, roots));
    let detail = state.failure_detail().expect("must be refused");
    assert!(detail.contains("IdentityMismatch"), "{detail}");
    assert!(!lab.argv().exists());
}

#[test]
fn nothing_ticked_or_two_ticked_in_an_exclusive_category_is_refused_before_launch() {
    let lab = Lab::new(0, direct_boot());
    let mut choice = Choice::load(&lab.content, &lab.cheat_file(MCH)).unwrap();
    let mut state = lab.state_with(Some(choice.clone()));
    run_state(&mut state, lab.request().unwrap());
    assert!(
        state
            .failure_detail()
            .unwrap()
            .contains("Tick at least one")
    );
    tick(&mut choice, &["Alpha", "Beta"]);
    let mut state = lab.state_with(Some(choice));
    run_state(&mut state, lab.request().unwrap());
    assert!(state.failure_detail().unwrap().contains("allows only one"));
    assert!(!lab.argv().exists());
}

#[test]
fn missing_melonds_or_configuration_stops_a_cheat_launch_and_leaves_no_workspace() {
    let lab = Lab::new(0, direct_boot());
    let mut choice = Choice::load(&lab.content, &lab.cheat_file(MCH)).unwrap();
    tick(&mut choice, &["Other"]);
    let request = lab.request().unwrap();
    std::fs::remove_file(&lab.exe).unwrap();
    let mut state = lab.state_with(Some(choice.clone()));
    run_state(&mut state, request);
    assert!(
        state
            .failure_detail()
            .unwrap()
            .contains("melonDS was not started")
    );
    let lab = Lab::new(0, direct_boot());
    let request = lab.request().unwrap();
    std::fs::write(&lab.config, "Emu.DirectBoot = true\nExtra = 1\n").unwrap();
    let mut choice = Choice::load(&lab.content, &lab.cheat_file(MCH)).unwrap();
    tick(&mut choice, &["Other"]);
    let mut state = lab.state_with(Some(choice));
    run_state(&mut state, request);
    assert!(
        state
            .failure_detail()
            .unwrap()
            .contains("ConfigurationChanged")
    );
    assert_eq!(leftovers(&lab.cheat_root()), 0);
}

#[test]
fn a_failing_melonds_with_cheats_is_reported_as_failed_and_still_cleaned_up() {
    let lab = Lab::new(3, direct_boot());
    let mut choice = Choice::load(&lab.content, &lab.cheat_file(MCH)).unwrap();
    tick(&mut choice, &["Free Play"]);
    let mut state = lab.state_with(Some(choice));
    run_state(&mut state, lab.request().unwrap());
    assert_eq!(state.failure_detail(), None, "the launch itself succeeded");
    let Some((_, _, StandaloneLaunchStage::Exited(StandaloneProcess::MelonDsCheats(session)))) =
        state.tracked.as_ref()
    else {
        panic!("expected an exited cheat session");
    };
    let lines = session_summary(session).join("\n");
    assert!(lines.contains("failed (exit code Some(3))"), "{lines}");
    assert!(!lines.contains("exited normally"), "{lines}");
    assert_eq!(leftovers(&lab.cheat_root()), 0);
}

#[test]
fn flatpak_melonds_is_shown_as_unsupported_for_cheats() {
    let lab = Lab::new(0, direct_boot());
    let Some(StandaloneLaunchRequest::MelonDs(mut request, _)) = lab.request() else {
        panic!("expected a melonDS request");
    };
    assert!(unsupported_reason(&request).is_none());
    request.expected_installation =
        archivefs_core::launch::installation::LaunchInstallation::Flatpak {
            app_id: "net.kuribo64.melonDS".into(),
        };
    let reason = unsupported_reason(&request).expect("Flatpak cannot take cheats");
    assert!(reason.contains("launch without cheats"), "{reason}");
}

#[test]
fn cheats_sharing_a_name_are_refused_so_ticking_one_never_enables_another() {
    let lab = Lab::new(0, direct_boot());
    let same_root = "CODE 0 Dupe\n02000000 00000001\nCODE 0 Dupe\n02000004 00000002\n";
    let across =
        "CAT 0 One\nCODE 0 Dupe\n02000000 00000001\n\nCAT 0 Two\nCODE 0 Dupe\n02000004 00000002\n";
    for body in [same_root, across] {
        let error = Choice::load(&lab.content, &lab.cheat_file(body)).unwrap_err();
        assert!(error.contains("not a usable"), "{error}");
    }
}

#[test]
fn a_cheat_path_swapped_for_a_link_between_choosing_and_launching_is_refused() {
    let lab = Lab::new(0, direct_boot());
    let source = lab.cheat_file(MCH);
    let mut choice = Choice::load(&lab.content, &source).unwrap();
    tick(&mut choice, &["Other"]);
    let identical_copy = lab.dir.path().join("copy.mch");
    std::fs::write(&identical_copy, MCH).unwrap();
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(&identical_copy, &source).unwrap();
    let mut state = lab.state_with(Some(choice));
    run_state(&mut state, lab.request().unwrap());
    let detail = state.failure_detail().expect("a link must be refused");
    assert!(detail.contains("regular file"), "{detail}");
    assert!(!lab.argv().exists());
}

#[test]
fn a_configuration_that_stops_being_a_real_file_after_discovery_offers_no_launch() {
    // Discovery saw a regular configuration; before the request is built it is
    // replaced by a link, or removed. An absent identity would make the core
    // skip its configuration check, so no request may be built at all.
    for replace_with_link in [true, false] {
        let lab = Lab::new(0, direct_boot());
        let context = lab.context(Some(KEY));
        let candidate = lab.candidate(&context);
        let moved = lab.dir.path().join("config-elsewhere.toml");
        std::fs::rename(&lab.config, &moved).unwrap();
        if replace_with_link {
            std::os::unix::fs::symlink(&moved, &lab.config).unwrap();
        }
        assert!(
            lab.request_from(&context, &candidate).is_none(),
            "link={replace_with_link}"
        );
    }
}

#[test]
fn a_melonds_that_is_a_link_offers_no_launch() {
    let lab = Lab::new(0, direct_boot());
    let context = lab.context(Some(KEY));
    let candidate = lab.candidate(&context);
    let real = lab.dir.path().join("melonDS-real");
    std::fs::rename(&lab.exe, &real).unwrap();
    std::os::unix::fs::symlink(&real, &lab.exe).unwrap();
    assert!(lab.request_from(&context, &candidate).is_none());
}

// ---- Blocking-open races -------------------------------------------------

use super::melonds_cheats::{read_cheat_bytes_after_check, rom_identity};

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "mkfifo is needed for this fixture");
}

/// Fails the test instead of hanging the suite if `work` blocks.
fn within<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the open blocked")
}

#[test]
fn a_cheat_file_swapped_for_a_fifo_before_it_is_opened_is_refused_without_blocking() {
    let lab = Lab::new(0, direct_boot());
    let source = lab.cheat_file(MCH);
    let target = source.clone();
    let error = within(move || {
        read_cheat_bytes_after_check(&source, || {
            // The race: after the regular-file check, before the open.
            std::fs::remove_file(&target).unwrap();
            mkfifo(&target);
        })
        .unwrap_err()
    });
    assert!(error.contains("no longer a regular file"), "{error}");
}

#[test]
fn a_cheat_file_swapped_for_a_link_before_it_is_opened_is_not_followed() {
    let lab = Lab::new(0, direct_boot());
    let source = lab.cheat_file(MCH);
    let real = lab.dir.path().join("real-copy.mch");
    std::fs::write(&real, MCH).unwrap();
    let target = source.clone();
    let error = within(move || {
        read_cheat_bytes_after_check(&source, || {
            std::fs::remove_file(&target).unwrap();
            std::os::unix::fs::symlink(&real, &target).unwrap();
        })
        .unwrap_err()
    });
    assert!(error.contains("cannot be read"), "{error}");
}

#[test]
fn a_cheat_file_swapped_for_another_file_before_it_is_opened_is_refused() {
    let lab = Lab::new(0, direct_boot());
    let source = lab.cheat_file(MCH);
    let target = source.clone();
    let error = within(move || {
        read_cheat_bytes_after_check(&source, || {
            std::fs::remove_file(&target).unwrap();
            std::fs::write(&target, MCH).unwrap();
        })
        .unwrap_err()
    });
    assert!(error.contains("changed while"), "{error}");
}

#[test]
fn a_fifo_given_as_the_cheat_file_or_the_rom_never_blocks() {
    let lab = Lab::new(0, direct_boot());
    let fifo = lab.dir.path().join("A DS Game.mch");
    mkfifo(&fifo);
    let (rom, cheat) = (lab.content.clone(), fifo.clone());
    let error = within(move || Choice::load(&rom, &cheat).unwrap_err());
    assert!(error.contains("regular file"), "{error}");
    // The ROM replaced by a FIFO after it was authorised.
    let rom_fifo = lab.dir.path().join("Swapped.nds");
    mkfifo(&rom_fifo);
    let authorised = match lab.request() {
        Some(StandaloneLaunchRequest::MelonDs(request, _)) => request.content_identity,
        _ => panic!("expected a melonDS request"),
    };
    let error = within(move || rom_identity(&rom_fifo, &authorised).unwrap_err());
    assert!(error.contains("cannot be read"), "{error}");
}

#[test]
fn a_valid_cheat_file_and_rom_still_open_and_hash_normally() {
    let lab = Lab::new(0, direct_boot());
    let source = lab.cheat_file(MCH);
    let bytes = read_cheat_bytes_after_check(&source, || {}).unwrap();
    assert_eq!(bytes, MCH.as_bytes());
    let authorised = match lab.request() {
        Some(StandaloneLaunchRequest::MelonDs(request, _)) => request.content_identity,
        _ => panic!("expected a melonDS request"),
    };
    let identity = rom_identity(&lab.content, &authorised).unwrap();
    assert_eq!(identity.game_code.as_deref(), Some("EWZP"));
    assert!(identity.rom_sha256.is_some());
}
