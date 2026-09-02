//! Tests for native / caller-confirmed PPSSPP launch preflight/execution.
//!
//! # The two binding provenances, and how each is exercised here
//!
//! [`crate::patch_manager::resolve_ppsspp_native_launch_binding`] authorizes
//! two [`crate::patch_manager::PpssppInstallationType`] executable
//! provenances - `Native` (a PPSSPP binary name on `PATH` / a documented
//! user directory) and `Explicit` (an exact path a host integration already
//! confirmed, e.g. a local AppImage in `roots.explicit_executables`) - and a
//! `Native` *profile* (PPSSPP's own standard config location) may bind
//! either.
//!
//! Only a genuine `PATH`-discovered `Native` executable is untestable
//! end-to-end here: fabricating one means mutating this test binary's
//! process-global `PATH`, which races every other test that reads it
//! (`std::env::set_var` is `unsafe` for exactly this reason), so that one
//! leg stays a pure unit in `crate::patch_manager::ppsspp_local`'s tests.
//! Everything else runs the full identity -> discovery -> binding ->
//! command-plan -> preflight -> spawn chain on a real synthetic PSP ISO9660
//! disc:
//!
//! * `build_ready_fixture` - a `Native` XDG-config profile launched by a
//!   caller-confirmed executable (the common managed-AppImage shape).
//! * `build_explicit_appimage_fixture` - a caller-supplied `Explicit`
//!   configuration root launched by a caller-confirmed `PPSSPP.AppImage`.
//!
//! Bare spawn mechanics are still also proven directly against a hand-built
//! [`PpssppCommand`], mirroring `xemu_execution::tests::hand_built_command`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::diagnostics::profiles::{
    LinuxEmulatorInstallationEvidence, MANAGED_APPIMAGE_INSTALLATION_FORM,
    managed_appimage_executable_for,
};
use crate::emulator_environment::EncodedPath;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::patch_manager::{PpssppProfileDiscoveryRoots, resolve_ppsspp_native_launch_binding};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-ppsspp-execution-{label}-{}-{sequence}",
            std::process::id()
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const ISO_SECTOR_SIZE: usize = 2_048;

fn directory_record(name: &[u8], extent: u32, size: u32, directory: bool) -> Vec<u8> {
    let length = 33 + name.len() + usize::from(name.len().is_multiple_of(2));
    let mut record = vec![0_u8; length];
    record[0] = length as u8;
    record[2..6].copy_from_slice(&extent.to_le_bytes());
    record[6..10].copy_from_slice(&extent.to_be_bytes());
    record[10..14].copy_from_slice(&size.to_le_bytes());
    record[14..18].copy_from_slice(&size.to_be_bytes());
    record[25] = if directory { 2 } else { 0 };
    record[28..30].copy_from_slice(&1_u16.to_le_bytes());
    record[30..32].copy_from_slice(&1_u16.to_be_bytes());
    record[32] = name.len() as u8;
    record[33..33 + name.len()].copy_from_slice(name);
    record
}

/// A minimal, structurally valid `PARAM.SFO` carrying one `DISC_ID` value -
/// mirrors `game_identity`'s own private `psp_sfo` test helper exactly,
/// since that helper is not itself exported.
fn psp_sfo(disc_id: &[u8]) -> Vec<u8> {
    let mut value = disc_id.to_vec();
    value.push(0);
    let key = b"DISC_ID\0";
    let key_start = 20 + 16;
    let data_start = key_start + 8;
    let mut out = vec![0_u8; data_start];
    out[0..4].copy_from_slice(crate::param_sfo::SFO_MAGIC);
    out[4..8].copy_from_slice(&0x0101_u32.to_le_bytes());
    out[8..12].copy_from_slice(&(key_start as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(data_start as u32).to_le_bytes());
    out[16..20].copy_from_slice(&1_u32.to_le_bytes());
    out[20..22].copy_from_slice(&0_u16.to_le_bytes());
    out[22..24].copy_from_slice(&0x0204_u16.to_le_bytes());
    out[24..28].copy_from_slice(&(value.len() as u32).to_le_bytes());
    out[28..32].copy_from_slice(&(value.len() as u32).to_le_bytes());
    out[32..36].copy_from_slice(&0_u32.to_le_bytes());
    out[key_start..key_start + key.len()].copy_from_slice(key);
    out.extend_from_slice(&value);
    out
}

/// A minimal, structurally valid PSP ISO9660 image: a primary volume
/// descriptor, a volume descriptor set terminator, a `PSP_GAME` directory
/// holding a real `UMD_DATA.BIN;1` marker and a `PARAM.SFO` carrying
/// `disc_id` - mirrors `game_identity`'s own private `psp_iso` test helper
/// exactly, since that helper is not itself exported.
fn psp_iso_bytes(disc_id: &[u8]) -> Vec<u8> {
    let sfo = psp_sfo(disc_id);
    let mut iso = vec![0_u8; 28 * ISO_SECTOR_SIZE];
    let pvd = 16 * ISO_SECTOR_SIZE;
    iso[pvd] = 1;
    iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
    iso[pvd + 6] = 1;
    let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
    iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
    let terminator = 17 * ISO_SECTOR_SIZE;
    iso[terminator] = 255;
    iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
    iso[terminator + 6] = 1;
    let root_offset = 20 * ISO_SECTOR_SIZE;
    let dir = directory_record(b"PSP_GAME", 21, ISO_SECTOR_SIZE as u32, true);
    iso[root_offset..root_offset + dir.len()].copy_from_slice(&dir);
    let umd = directory_record(b"UMD_DATA.BIN;1", 23, 3, false);
    let umd_offset = root_offset + dir.len();
    iso[umd_offset..umd_offset + umd.len()].copy_from_slice(&umd);
    let psp_offset = 21 * ISO_SECTOR_SIZE;
    let sfo_record = directory_record(b"PARAM.SFO;1", 22, sfo.len() as u32, false);
    iso[psp_offset..psp_offset + sfo_record.len()].copy_from_slice(&sfo_record);
    iso[22 * ISO_SECTOR_SIZE..22 * ISO_SECTOR_SIZE + sfo.len()].copy_from_slice(&sfo);
    iso[23 * ISO_SECTOR_SIZE..23 * ISO_SECTOR_SIZE + 3].copy_from_slice(b"UMD");
    iso
}

const PSP_DISC_ID: &str = "ULUS10000";

fn base_roots(fixture: &Fixture) -> PpssppProfileDiscoveryRoots {
    PpssppProfileDiscoveryRoots {
        home: fixture.path("home"),
        xdg_config_home: fixture.path("config"),
        xdg_data_home: fixture.path("data"),
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: std::collections::BTreeMap::new(),
        appimage_directory: None,
    }
}

/// The standard XDG `Native` PPSSPP fixture - the common managed-AppImage
/// shape: a real `PSP/SYSTEM/ppsspp.ini` evidence file at PPSSPP's own
/// config location (so the discovered profile is `Native` and eligible), a
/// fake executable supplied via `explicit_executables` (a caller-confirmed
/// exact path, which a `Native` profile may bind), and a loose PSP ISO
/// whose verified disc ID becomes the request's expected disc id/game key -
/// computed via the same `inspect_catalogued_game_identity` the module
/// itself uses, never hand-typed.
struct ReadyFixture {
    fixture: Fixture,
    roots: PpssppProfileDiscoveryRoots,
    profile_root: PathBuf,
    request: PpssppLaunchRequest,
}

fn build_ready_fixture(label: &str) -> ReadyFixture {
    let fixture = Fixture::new(label);
    let mut roots = base_roots(&fixture);
    let profile_root = roots.xdg_config_home.join("ppsspp");
    fs::create_dir_all(profile_root.join("PSP/SYSTEM")).unwrap();
    fs::write(profile_root.join("PSP/SYSTEM/ppsspp.ini"), b"[General]\n").unwrap();
    let executable = fixture.write_executable("bin/ppsspp", b"#!/bin/sh\nexit 0\n");
    roots.explicit_executables.push(executable.clone());
    let content = fixture.write("games/game.iso", &psp_iso_bytes(PSP_DISC_ID.as_bytes()));

    // `profile_id` is `ppsspp:<configuration_path>` (never hand-reconstructed)
    // - read it off a real discovery pass rather than assuming the format.
    let discovery = discover_ppsspp_profiles(&roots);
    let profile_id = discovery
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == profile_root)
        .expect("fixture profile must be discovered")
        .profile_id
        .clone();

    let request = PpssppLaunchRequest {
        selected_content_path: content,
        expected_platform_id: "PSP".to_string(),
        expected_game_key: PSP_DISC_ID.to_string(),
        expected_psp_disc_id: PSP_DISC_ID.to_string(),
        profile_id,
        expected_executable: executable,
    };
    ReadyFixture {
        fixture,
        roots,
        profile_root,
        request,
    }
}

fn preflight(ready: &ReadyFixture) -> Result<PpssppCommand, PpssppLaunchPreflightError> {
    preflight_ppsspp_launch(&ready.request, &ready.roots)
}

// --- content path checks (steps 1-3) -----------------------------------------------------------

#[test]
fn content_identity_capture_detects_a_swapped_file() {
    let ready = build_ready_fixture("content-changed");
    let path = ready.request.selected_content_path.clone();
    let original = fs::read(&path).unwrap();
    let before = CapturedFileIdentity::capture(&fs::symlink_metadata(&path).unwrap());
    fs::write(&path, b"different bytes entirely, same path").unwrap();
    let after = CapturedFileIdentity::capture(&fs::symlink_metadata(&path).unwrap());
    assert_ne!(
        before, after,
        "captured identity must change when the file changes"
    );
    fs::write(&path, &original).unwrap();
}

#[test]
fn content_removed_is_rejected() {
    let ready = build_ready_fixture("content-removed");
    fs::remove_file(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::ContentNotFound);
}

#[test]
fn content_replaced_by_a_directory_is_rejected() {
    let ready = build_ready_fixture("content-replaced-with-dir");
    fs::remove_file(&ready.request.selected_content_path).unwrap();
    fs::create_dir_all(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        PpssppLaunchPreflightErrorKind::ContentNotRegularFile
    );
}

#[test]
fn symlink_content_is_rejected() {
    let ready = build_ready_fixture("symlink-content");
    let link = ready.fixture.path("games/link.iso");
    symlink(&ready.request.selected_content_path, &link).unwrap();
    let mut request = ready.request.clone();
    request.selected_content_path = link;
    let error = preflight_ppsspp_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::ContentIsSymlink);
}

#[test]
fn unsupported_psp_container_remains_refused() {
    // CSO/CHD/PBP-style extensions must never be accepted - only a direct
    // `.iso` is supported in this phase.
    let ready = build_ready_fixture("unsupported-format");
    for extension in ["cso", "chd", "pbp"] {
        let path = ready.fixture.write(
            &format!("games/game.{extension}"),
            &psp_iso_bytes(PSP_DISC_ID.as_bytes()),
        );
        let mut request = ready.request.clone();
        request.selected_content_path = path;
        let error = preflight_ppsspp_launch(&request, &ready.roots).unwrap_err();
        assert_eq!(
            error.kind,
            PpssppLaunchPreflightErrorKind::ContentFormatUnsupported,
            "extension .{extension} must be refused"
        );
    }
}

#[test]
fn mount_input_archive_content_is_rejected() {
    let ready = build_ready_fixture("archive-content");
    let zip = ready.fixture.write("games/game.zip", b"pk not a real zip");
    let mut request = ready.request.clone();
    request.selected_content_path = zip;
    let error = preflight_ppsspp_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        PpssppLaunchPreflightErrorKind::ContentRequiresMount
    );
}

// --- identity checks (step 4) --------------------------------------------------------------------

#[test]
fn wrong_expected_game_key_is_rejected() {
    let mut ready = build_ready_fixture("wrong-game-key");
    ready.request.expected_game_key = "AAAAAAAAAA".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::IdentityMismatch);
}

#[test]
fn wrong_expected_psp_disc_id_is_rejected() {
    let mut ready = build_ready_fixture("wrong-disc-id");
    ready.request.expected_psp_disc_id = "AAAAAAAAAA".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        PpssppLaunchPreflightErrorKind::PspDiscIdMismatch
    );
}

#[test]
fn disc_id_drift_after_planning_is_rejected() {
    // Simulate the disc being swapped for a different, still-genuinely-valid
    // PSP ISO between when the user was shown "Ready" and this click. For
    // PSP, the resolved game key *is* the verified disc ID, so this drift
    // surfaces as an identity mismatch (checked first) rather than a
    // same-game/different-disc-ID mismatch - either way, the fresh disc ID
    // never matches what the request expected, so the launch is refused.
    let ready = build_ready_fixture("disc-id-drift");
    fs::write(
        &ready.request.selected_content_path,
        psp_iso_bytes(b"ULUS99999"),
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::IdentityMismatch);
}

#[test]
fn wrong_platform_expectation_never_authorizes_a_native_psp_disc() {
    let mut ready = build_ready_fixture("wrong-platform");
    ready.request.expected_platform_id = "PS2".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::IdentityMismatch);
}

// --- profile lookup checks (steps 5-6) -------------------------------------------------------------

#[test]
fn profile_directory_removed_is_rejected() {
    let ready = build_ready_fixture("profile-removed");
    fs::remove_dir_all(&ready.profile_root).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::ProfileNotFound);
}

#[test]
fn profile_root_drift_is_rejected() {
    let ready = build_ready_fixture("root-drift");
    let mut request = ready.request.clone();
    // A stale profile id from a root that no longer matches PPSSPP's current
    // default XDG resolution - never substituted with a different profile.
    request.profile_id = format!(
        "ppsspp:{}",
        ready.fixture.path("stale-config/ppsspp").display()
    );
    let error = preflight_ppsspp_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::ProfileNotFound);
}

// --- every earlier check passing reaches binding resolution, never skips it ------------------------

#[test]
fn a_native_xdg_profile_with_a_caller_confirmed_executable_reaches_a_real_command() {
    // The common managed shape: PPSSPP is installed at its own XDG config
    // location (`Native`, eligible) and launched by a caller-confirmed exact
    // executable path. Every preflight step passes and a real command is
    // produced.
    let ready = build_ready_fixture("native-plus-confirmed");
    let command = preflight(&ready).expect("a Native profile binds a caller-confirmed executable");
    assert_eq!(command.executable, ready.request.expected_executable);
    assert_eq!(
        command.arguments,
        vec![ready.request.selected_content_path.clone().into_os_string()]
    );
    assert_eq!(command.selection.platform_id, "PSP");
}

// --- final pre-spawn recheck units (step 10) --------------------------------------------------------

#[test]
fn recheck_executable_rejects_a_missing_executable() {
    let fixture = Fixture::new("recheck-missing");
    let path = fixture.path("bin/ppsspp");
    let error = recheck_executable(&path).unwrap_err();
    assert_eq!(
        error.kind,
        PpssppLaunchPreflightErrorKind::ExecutableMissing
    );
}

#[test]
fn recheck_executable_rejects_a_symlink() {
    let fixture = Fixture::new("recheck-symlink");
    let real = fixture.write_executable("bin/ppsspp-real", b"#!/bin/sh\nexit 0\n");
    let link = fixture.path("bin/ppsspp");
    symlink(&real, &link).unwrap();
    let error = recheck_executable(&link).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::ExecutableUnsafe);
}

#[test]
fn recheck_executable_rejects_a_non_executable_file() {
    let fixture = Fixture::new("recheck-not-executable");
    let path = fixture.write("bin/ppsspp", b"#!/bin/sh\nexit 0\n");
    let error = recheck_executable(&path).unwrap_err();
    assert_eq!(
        error.kind,
        PpssppLaunchPreflightErrorKind::ExecutableNotExecutable
    );
}

#[test]
fn recheck_executable_accepts_a_real_executable() {
    let fixture = Fixture::new("recheck-ok");
    let path = fixture.write_executable("bin/ppsspp", b"#!/bin/sh\nexit 0\n");
    recheck_executable(&path).expect("a real, executable regular file must pass");
}

#[test]
fn inspect_and_capture_content_identity_rejects_a_removed_disc() {
    let fixture = Fixture::new("recheck-content-missing");
    let path = fixture.path("games/game.iso");
    let error = inspect_and_capture_content_identity(&path).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::ContentNotFound);
}

// --- spawn: mechanics proven against a hand-built command, never a discovered one ------------------

fn hand_built_command(executable: PathBuf, content_path: PathBuf) -> PpssppCommand {
    PpssppCommand {
        executable,
        arguments: vec![content_path.clone().into_os_string()],
        working_directory: None,
        selection: crate::launch::ppsspp_command::PpssppCommandSelection {
            profile_id: "test".to_string(),
            platform_id: "PSP".to_string(),
            verified_psp_disc_id: PSP_DISC_ID.to_string(),
            content_path,
        },
    }
}

fn wait_for_exit(process: &mut LaunchedPpssppProcess) -> &ProcessExitReport {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if process.poll().is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    process.poll().expect("exit report must be cached now")
}

#[test]
fn fake_executable_receives_exact_argv() {
    let fixture = Fixture::new("argv-capture");
    let capture_path = fixture.path("argv-capture.txt");
    let executable = fixture.write_executable(
        "bin/ppsspp",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let content_path = fixture.write("games/game.iso", b"synthetic content");
    let command = hand_built_command(executable, content_path.clone());
    let mut process = spawn_ppsspp(command).expect("the fake script must spawn");
    wait_for_exit(&mut process);
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines, vec![content_path.to_str().unwrap()]);
}

#[test]
fn successful_spawn_returns_a_real_pid_and_clean_exit() {
    let fixture = Fixture::new("spawn-success");
    let executable = fixture.write_executable("bin/ppsspp", b"#!/bin/sh\nexit 0\n");
    let content_path = fixture.write("games/game.iso", b"synthetic content");
    let command = hand_built_command(executable, content_path);
    let mut process = spawn_ppsspp(command).expect("the fake script must spawn");
    assert!(process.pid > 0);
    assert!(process.is_running() || process.poll().is_some());
    let report = wait_for_exit(&mut process);
    let status = report.status.as_ref().expect("wait() must succeed");
    assert!(status.success(), "the fake script exits 0");
}

#[test]
fn non_zero_exit_and_stderr_are_reported() {
    let fixture = Fixture::new("spawn-nonzero");
    let executable = fixture.write_executable(
        "bin/ppsspp",
        b"#!/bin/sh\necho 'synthetic ppsspp failure' 1>&2\nexit 7\n",
    );
    let content_path = fixture.write("games/game.iso", b"synthetic content");
    let command = hand_built_command(executable, content_path);
    let mut process = spawn_ppsspp(command).unwrap();
    let report = wait_for_exit(&mut process);
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic ppsspp failure"));
}

#[test]
fn spaces_and_unicode_paths_are_passed_intact() {
    let fixture = Fixture::new("spaces-unicode");
    let capture_path = fixture.path("argv-capture.txt");
    let executable = fixture.write_executable(
        "bin/ppsspp",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let unicode_name = "games/Crisis Core こんにちは 日本語, with spaces.iso";
    let content_path = fixture.write(unicode_name, b"synthetic content");
    let command = hand_built_command(executable, content_path.clone());
    let mut process = spawn_ppsspp(command).unwrap();
    wait_for_exit(&mut process);
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines, vec![content_path.to_str().unwrap()]);
}

#[test]
fn shell_metacharacters_in_content_path_are_never_interpreted() {
    let marker = std::env::temp_dir().join(format!(
        "archivefs-ppsspp-execution-shell-marker-{}",
        std::process::id()
    ));
    let _ = fs::remove_file(&marker);
    let fixture = Fixture::new("shell-metacharacters");
    let executable = fixture.write_executable("bin/ppsspp", b"#!/bin/sh\nexit 0\n");
    let dangerous_name = format!("games/game; touch {} #.iso", marker.display());
    let dangerous_path = fixture.write(&dangerous_name, b"synthetic content");
    let command = hand_built_command(executable, dangerous_path);
    let mut process = spawn_ppsspp(command).unwrap();
    wait_for_exit(&mut process);
    assert!(
        !marker.exists(),
        "shell metacharacters in a content path must never be interpreted"
    );
    let _ = fs::remove_file(&marker);
}

#[test]
fn spawn_failure_is_reported() {
    let fixture = Fixture::new("spawn-failure");
    let command = hand_built_command(
        fixture.path("bin/does-not-exist"),
        fixture.path("games/game.iso"),
    );
    let result = spawn_ppsspp(command);
    assert!(matches!(result, Err(PpssppLaunchSpawnError::Spawn(_))));
}

// --- Explicit (caller-confirmed local AppImage) end-to-end -------------------
//
// The `Explicit` provenance has no `PATH` dependency, so - unlike the
// `Native` leg - the full chain
//   identity -> discover_ppsspp_profiles -> resolve_ppsspp_native_launch_binding
//   -> build_launch_plan_from_results -> build_ppsspp_command_plan
//   -> preflight_ppsspp_launch -> spawn_ppsspp
// runs here against a harmless fake `PPSSPP.AppImage`. No real emulator,
// GPU, DISPLAY, audio, or network.

struct ExplicitAppImageFixture {
    fixture: Fixture,
    roots: PpssppProfileDiscoveryRoots,
    appimage: PathBuf,
    content: PathBuf,
    request: PpssppLaunchRequest,
}

/// Writes a fake `PPSSPP.AppImage` (a `/bin/sh` script that appends its argv,
/// one per line, to `argv-capture.txt` and exits 0), a real explicit PPSSPP
/// configuration root with `PSP/SYSTEM/ppsspp.ini`, and a loose PSP ISO, then
/// resolves the discovered `Explicit` profile id.
fn build_explicit_appimage_fixture(label: &str) -> ExplicitAppImageFixture {
    let fixture = Fixture::new(label);
    let mut roots = base_roots(&fixture);

    // A configuration root that is deliberately not the XDG default, so it is
    // discovered as an `Explicit` profile rather than the standard `Native`
    // one.
    let config_root = fixture.path("apps/PPSSPP/config");
    fs::create_dir_all(config_root.join("PSP/SYSTEM")).unwrap();
    fs::write(config_root.join("PSP/SYSTEM/ppsspp.ini"), b"[General]\n").unwrap();
    roots.explicit_configuration_roots.push(config_root.clone());

    let capture = fixture.path("argv-capture.txt");
    let appimage = fixture.write_executable(
        "apps/PPSSPP/PPSSPP.AppImage",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nexit 0\n",
            capture.display()
        )
        .as_bytes(),
    );
    roots.explicit_executables.push(appimage.clone());

    let content = fixture.write("games/game.iso", &psp_iso_bytes(PSP_DISC_ID.as_bytes()));

    let profile_id = discover_ppsspp_profiles(&roots)
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == config_root)
        .expect("the explicit configuration root must be discovered")
        .profile_id
        .clone();

    let request = PpssppLaunchRequest {
        selected_content_path: content.clone(),
        expected_platform_id: "PSP".to_string(),
        expected_game_key: PSP_DISC_ID.to_string(),
        expected_psp_disc_id: PSP_DISC_ID.to_string(),
        profile_id,
        expected_executable: appimage.clone(),
    };

    ExplicitAppImageFixture {
        fixture,
        roots,
        appimage,
        content,
        request,
    }
}

#[test]
fn explicit_appimage_reaches_a_real_command_and_spawns_with_exact_argv() {
    let ready = build_explicit_appimage_fixture("appimage-e2e");

    let command = preflight_ppsspp_launch(&ready.request, &ready.roots)
        .expect("a fully wired explicit AppImage fixture must preflight cleanly");

    assert_eq!(
        command.executable, ready.appimage,
        "the spawned executable is exactly the caller-confirmed AppImage"
    );
    assert_eq!(
        command.arguments,
        vec![ready.content.clone().into_os_string()],
        "the only argument is the verified PSP ISO, passed verbatim"
    );
    assert_eq!(command.selection.content_path, ready.content);
    assert_eq!(command.selection.platform_id, "PSP");
    assert!(command.working_directory.is_none());

    let mut process = spawn_ppsspp(command).expect("the fake AppImage must spawn");
    let report = wait_for_exit(&mut process);
    assert!(
        report
            .status
            .as_ref()
            .expect("child wait succeeds")
            .success(),
        "the fake AppImage exits 0"
    );

    let captured = fs::read_to_string(ready.fixture.path("argv-capture.txt")).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(
        lines,
        vec![ready.content.to_str().unwrap()],
        "argv reaching the process boundary is exactly [<content path>]"
    );
}

#[test]
fn explicit_appimage_binding_drift_is_refused() {
    let mut ready = build_explicit_appimage_fixture("appimage-drift");
    // The user authorised one executable; the freshly resolved binding now
    // points at a different one.
    ready.request.expected_executable = ready.fixture.path("apps/PPSSPP/other.AppImage");
    let error = preflight_ppsspp_launch(&ready.request, &ready.roots).unwrap_err();
    assert_eq!(error.kind, PpssppLaunchPreflightErrorKind::BindingDrift);
}

#[test]
fn explicit_appimage_removed_between_discovery_and_preflight_is_refused() {
    let ready = build_explicit_appimage_fixture("appimage-vanished");
    fs::remove_file(&ready.appimage).unwrap();
    let error = preflight_ppsspp_launch(&ready.request, &ready.roots).unwrap_err();
    // A vanished executable is no longer a discovered candidate, so the fresh
    // binding cannot be produced - fail closed, no spawn.
    assert_eq!(
        error.kind,
        PpssppLaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn explicit_appimage_config_evidence_missing_keeps_the_profile_ineligible() {
    let ready = build_explicit_appimage_fixture("appimage-no-config-evidence");
    // Remove the PPSSPP configuration evidence; the profile is no longer
    // eligible, so no candidate is Ready and preflight refuses.
    fs::remove_file(
        ready
            .fixture
            .path("apps/PPSSPP/config/PSP/SYSTEM/ppsspp.ini"),
    )
    .unwrap();
    let error = preflight_ppsspp_launch(&ready.request, &ready.roots).unwrap_err();
    assert!(matches!(
        error.kind,
        PpssppLaunchPreflightErrorKind::ProfileNotFound
            | PpssppLaunchPreflightErrorKind::BindingUnavailable
            | PpssppLaunchPreflightErrorKind::CandidateNotReady
    ));
}

// --- the production wiring seam: managed evidence -> roots -> binding -------

/// One `install.json`-backed EmuWiz-managed AppImage evidence entry for
/// `emulator`, pointing at `executable` (the exact shape
/// `diagnostics::profiles::discover_managed_emulator_installations`
/// produces).
fn managed_evidence(
    emulator: &str,
    executable: &std::path::Path,
) -> LinuxEmulatorInstallationEvidence {
    LinuxEmulatorInstallationEvidence {
        emulator: emulator.to_string(),
        installation_form: MANAGED_APPIMAGE_INSTALLATION_FORM.to_string(),
        executable: Some(EncodedPath::from_path(executable)),
        profile: None,
        detail: String::new(),
    }
}

#[test]
fn managed_ppsspp_appimage_evidence_reaches_a_trusted_binding_through_normal_discovery() {
    let fixture = Fixture::new("managed-evidence-seam");
    let mut roots = base_roots(&fixture);

    // PPSSPP installed at its own XDG config location.
    let profile_root = roots.xdg_config_home.join("ppsspp");
    fs::create_dir_all(profile_root.join("PSP/SYSTEM")).unwrap();
    fs::write(profile_root.join("PSP/SYSTEM/ppsspp.ini"), b"[General]\n").unwrap();

    // The managed AppImage on disk, exactly as the download flow would leave it.
    let appimage = fixture.write_executable(
        "data/emuwiz/emulators/ppsspp/ppsspp.AppImage",
        b"#!/bin/sh\nexit 0\n",
    );

    // The production seam: validated evidence -> explicit_executables.
    let evidence = [managed_evidence("PPSSPP", &appimage)];
    roots
        .explicit_executables
        .extend(managed_appimage_executable_for(&evidence, "PPSSPP"));
    assert_eq!(roots.explicit_executables, vec![appimage.clone()]);

    let discovery = discover_ppsspp_profiles(&roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == profile_root)
        .expect("the XDG profile is discovered");
    assert!(profile.eligible);

    let binding = resolve_ppsspp_native_launch_binding(profile).expect("a trusted binding");
    assert_eq!(binding.executable, appimage);
}

#[test]
fn without_managed_evidence_the_ppsspp_roots_and_binding_are_unchanged() {
    let fixture = Fixture::new("no-managed-evidence-seam");
    let mut roots = base_roots(&fixture);
    let profile_root = roots.xdg_config_home.join("ppsspp");
    fs::create_dir_all(profile_root.join("PSP/SYSTEM")).unwrap();
    fs::write(profile_root.join("PSP/SYSTEM/ppsspp.ini"), b"[General]\n").unwrap();

    // A *guessed* `~/Applications` AppImage - the non-managed form - is
    // present as evidence but must never be promoted.
    let guessed = fixture.write_executable(
        "Applications/PPSSPP/PPSSPP.AppImage",
        b"#!/bin/sh\nexit 0\n",
    );
    let evidence = [LinuxEmulatorInstallationEvidence {
        emulator: "PPSSPP".to_string(),
        installation_form: "AppImage".to_string(),
        executable: Some(EncodedPath::from_path(&guessed)),
        profile: None,
        detail: String::new(),
    }];
    roots
        .explicit_executables
        .extend(managed_appimage_executable_for(&evidence, "PPSSPP"));
    assert!(roots.explicit_executables.is_empty());

    let discovery = discover_ppsspp_profiles(&roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == profile_root)
        .expect("the XDG profile is discovered");
    // No executable candidate at all -> still Blocked, exactly as before.
    assert_eq!(
        resolve_ppsspp_native_launch_binding(profile)
            .unwrap_err()
            .kind,
        crate::patch_manager::PpssppLaunchBlockerKind::ExecutableMissing,
    );
}
