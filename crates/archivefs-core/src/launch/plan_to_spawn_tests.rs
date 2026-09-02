//! Cross-module continuity for the RetroArch launch chain:
//!
//! ```text
//! selected content file
//!   -> game_identity::inspect_catalogued_game_identity      (identity)
//!   -> launch::canonical_identity_from_game_report          (evidence bridge)
//!   -> discover_retroarch_environment                       (emulator/profile discovery)
//!   -> launch::build_launch_plan                            (planning + readiness + platform map)
//!   -> derive RetroArchLaunchRequest from the candidate     (the GUI launch-request step)
//!   -> launch::build_retroarch_command_plan                 (command construction)
//!   -> launch::preflight_and_launch_retroarch               (process invocation boundary)
//! ```
//!
//! The per-module `*_command` / `*_execution` test suites already cover each
//! step in isolation. What no existing test asserts is that the pieces
//! *agree* when run as one flow: that the candidate the planner recommends
//! is the exact launch the executor performs, argument-for-argument, and
//! that a change to reality between "plan built" and "launch clicked" fails
//! closed at that boundary rather than launching something stale.
//!
//! RetroArch is used because `preflight_retroarch_launch` genuinely rebuilds
//! the whole plan/command internally from a fresh environment discovery, so
//! this is a true end-to-end revalidation and not a mock. Every fixture is
//! a real temp directory with a fake `retroarch` shell script and a stub
//! core `.so`; the spawn step genuinely `fork`/`exec`s the fake script,
//! which exits 0 immediately. No network, no real emulator, no DISPLAY.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::emulator_environment::HostReadOnlyFilesystem;
use crate::emulator_environment::retroarch::{
    DiscoveryEnvironment, ProfileKind, discover_retroarch_environment,
};
use crate::game_identity::inspect_catalogued_game_identity;
use crate::launch::{
    CandidatePreference, CanonicalIdentityStatus, LaunchCandidate, LaunchContainerKind,
    LaunchContentRef, LaunchExecutionError, LaunchPlan, LaunchPreflightErrorKind, LaunchReadiness,
    LaunchTarget, RetroArchLaunchRequest, build_launch_plan, build_retroarch_command_plan,
    canonical_identity_from_game_report, preflight_and_launch_retroarch,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-launch-plan-to-spawn-{label}-{}-{}-{sequence}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn write_executable(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.write(relative, contents);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn env(&self) -> DiscoveryEnvironment {
        DiscoveryEnvironment {
            home: Some(self.root.clone().into_os_string()),
            xdg_config_home: Some(self.path("config").into_os_string()),
            path: Some(self.path("bin").into_os_string()),
            user_flatpak_root: self.path("user-flatpak"),
            system_flatpak_root: self.path("system-flatpak"),
            app_image_search_roots: Vec::new(),
            desktop_file_roots: Vec::new(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A genuinely `Ready` native RetroArch fixture for a loose Mega Drive ROM
/// plus the fully-resolved chain outputs, so a test can assert continuity
/// between them.
struct Wired {
    fixture: Fixture,
    content: PathBuf,
    identity: CanonicalIdentityStatus,
    plan: LaunchPlan,
    request: RetroArchLaunchRequest,
}

fn write_core(fixture: &Fixture, stem: &str) {
    fixture.write(&format!("cores/{stem}_libretro.so"), b"stub core");
    fixture.write(
        &format!("info/{stem}.info"),
        b"systemname = \"megadrive\"\n",
    );
}

fn build_wired(label: &str, extra_cores: &[&str]) -> Wired {
    let fixture = Fixture::new(label);
    fixture.write_executable("bin/retroarch", b"#!/bin/sh\nexit 0\n");
    let cores_dir = fixture.path("cores");
    let info_dir = fixture.path("info");
    fixture.write(
        "config/retroarch/retroarch.cfg",
        format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\n",
            cores_dir.display(),
            info_dir.display()
        )
        .as_bytes(),
    );
    write_core(&fixture, "genesis_plus_gx");
    for stem in extra_cores {
        write_core(&fixture, stem);
    }
    let content = fixture.write("content/game.md", b"synthetic mega drive rom bytes");

    // 1. identity
    let report = inspect_catalogued_game_identity(&content, Some("MegaDrive"));
    let expected_key = report
        .verified_loose_rom_sha256()
        .expect("fixture content must verify as a loose Mega Drive ROM")
        .to_string();

    // 2. evidence bridge
    let (identity, _facts) = canonical_identity_from_game_report(&report);
    match &identity {
        CanonicalIdentityStatus::Resolved(resolved) => {
            assert_eq!(resolved.platform_id, "MegaDrive");
            assert_eq!(resolved.game_key, expected_key);
        }
        other => panic!("identity did not resolve: {other:?}"),
    }

    // 3. environment discovery
    let environment =
        discover_retroarch_environment(&HostReadOnlyFilesystem, &fixture.env()).unwrap();

    // 4. plan
    let content_ref = LaunchContentRef {
        kind: None,
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(content.clone()),
        requires_mount: false,
        provenance: "plan-to-spawn continuity fixture: direct regular file".to_string(),
    };
    let plan = build_launch_plan(&identity, &content_ref, &[], &environment, &[]);

    // 5. derive the launch request from the recommended candidate exactly
    //    the way the GUI's `retroarch_launch_request` does.
    let candidate = recommended_retroarch_candidate(&plan);
    let LaunchTarget::RetroArchCore {
        profile, core_stem, ..
    } = &candidate.target
    else {
        unreachable!("recommended candidate is a RetroArch core")
    };
    assert_eq!(profile.profile_kind, ProfileKind::Native);
    let request = RetroArchLaunchRequest {
        selected_content_path: content.clone(),
        expected_platform_id: plan.platform_id.clone().unwrap(),
        expected_game_key: plan.game_key.clone().unwrap(),
        profile: *profile,
        core_stem: core_stem.clone(),
    };

    Wired {
        fixture,
        content,
        identity,
        plan,
        request,
    }
}

fn recommended_retroarch_candidate(plan: &LaunchPlan) -> &LaunchCandidate {
    let candidate = plan
        .candidates
        .iter()
        .find(|candidate| {
            candidate.readiness == LaunchReadiness::Ready
                && candidate.blockers.is_empty()
                && candidate.warnings.is_empty()
                && matches!(candidate.preference, CandidatePreference::SoleEligible)
                && matches!(candidate.target, LaunchTarget::RetroArchCore { .. })
        })
        .expect("exactly one clean, recommended RetroArch candidate");
    candidate
}

fn wait_for_clean_exit(process: &mut crate::launch::LaunchedRetroArchProcess) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(report) = process.poll() {
            let status = report
                .status
                .as_ref()
                .expect("wait() on the child succeeds");
            assert!(status.success(), "the fake retroarch script exits 0");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the fake script did not exit in time"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn argv_strings(arguments: &[std::ffi::OsString]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn plan_recommended_candidate_is_byte_for_byte_what_gets_spawned() {
    let wired = build_wired("continuity", &[]);
    let candidate = recommended_retroarch_candidate(&wired.plan);

    // The command the planner would show for this candidate, discovered
    // from the same unchanged fixture the executor will itself rebuild
    // from.
    let planner_environment =
        discover_retroarch_environment(&HostReadOnlyFilesystem, &wired.fixture.env()).unwrap();
    let command_plan =
        build_retroarch_command_plan(&wired.identity, candidate, &planner_environment);
    assert!(
        command_plan.blockers.is_empty(),
        "planner command is unblocked"
    );
    let planned = command_plan.command.expect("planner produced a command");

    // ...must be exactly what the executor actually spawns.
    let mut process = preflight_and_launch_retroarch(
        &wired.request,
        &HostReadOnlyFilesystem,
        &wired.fixture.env(),
    )
    .expect("the fully wired chain launches");

    assert_eq!(
        process.command_facts.executable, planned.executable,
        "spawned executable == planned executable"
    );
    assert_eq!(
        process.command_facts.executable,
        wired.fixture.path("bin/retroarch"),
        "and it is the fake retroarch on the fixture PATH, nothing from the real machine"
    );
    assert_eq!(
        process.command_facts.arguments, planned.arguments,
        "spawned argv == planned argv, token for token"
    );
    assert_eq!(
        process.command_facts.content_path, wired.content,
        "the content path carried to spawn is the exact file the plan was built for"
    );
    assert_eq!(
        process.command_facts.core_stem, "genesis_plus_gx",
        "the recommended core stem survives to the process boundary"
    );

    let argv = argv_strings(&process.command_facts.arguments);
    assert_eq!(
        argv.last().map(String::as_str),
        wired.content.to_str(),
        "the content path is the final argv token, passed verbatim (never re-parsed)"
    );
    assert!(
        argv.iter()
            .any(|token| token.contains("genesis_plus_gx_libretro.so")),
        "the loaded core library is the recommended one: {argv:?}"
    );

    wait_for_clean_exit(&mut process);
}

#[test]
fn content_swapped_between_plan_and_launch_is_refused_at_the_boundary_without_spawning() {
    let wired = build_wired("content-drift", &[]);

    // The user saw a clean, Ready plan for this game. Before they click
    // Launch, the file at that exact path is replaced with different bytes
    // (a different verified ROM identity).
    fs::write(&wired.content, b"a completely different mega drive rom").unwrap();

    let result = preflight_and_launch_retroarch(
        &wired.request,
        &HostReadOnlyFilesystem,
        &wired.fixture.env(),
    );

    match result {
        Err(LaunchExecutionError::Preflight(error)) => assert_eq!(
            error.kind,
            LaunchPreflightErrorKind::IdentityMismatch,
            "the fresh re-identification no longer matches the approved game key"
        ),
        Err(other) => panic!("expected a preflight IdentityMismatch, got {other:?}"),
        Ok(_) => panic!("a swapped game must never launch"),
    }
    // `preflight_and_launch_retroarch` returns the preflight error before it
    // ever reaches `spawn_retroarch`, so no process was started.
}

#[test]
fn a_second_installed_core_never_silently_substitutes_the_recommended_one_at_spawn() {
    // Two cores resolve to Mega Drive, but `genesis_plus_gx` is the
    // reviewed hint, so the plan still recommends exactly it. The request
    // derived from that candidate must launch that core - never the other
    // installed one.
    let wired = build_wired("no-substitution", &["picodrive"]);
    assert!(
        wired.plan.candidates.len() >= 2,
        "the second core is also a candidate, just not the recommended one"
    );

    let mut process = preflight_and_launch_retroarch(
        &wired.request,
        &HostReadOnlyFilesystem,
        &wired.fixture.env(),
    )
    .expect("the recommended-core chain launches");

    assert_eq!(process.command_facts.core_stem, "genesis_plus_gx");
    let argv = argv_strings(&process.command_facts.arguments);
    assert!(
        argv.iter()
            .any(|token| token.contains("genesis_plus_gx_libretro.so")),
        "spawned argv loads the recommended core: {argv:?}"
    );
    assert!(
        !argv
            .iter()
            .any(|token| token.contains("picodrive_libretro.so")),
        "spawned argv never silently switches to the other installed core: {argv:?}"
    );

    wait_for_clean_exit(&mut process);
}
