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
        std::fs::write(&content, b"nds fixture").unwrap();
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
    std::fs::write(
        path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.argv\"\necho oops >&2\nexit {exit_code}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
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
    assert_eq!(std::fs::read(&lab.content).unwrap(), b"nds fixture");
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
