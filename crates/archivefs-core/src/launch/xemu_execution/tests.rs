//! Tests for native xemu launch preflight/execution.
//!
//! # Two executable provenances, and why one leg is unit-tested
//!
//! [`crate::patch_manager::resolve_xemu_native_launch_binding`] authorizes
//! two provenances for a profile discovered at xemu's own standard XDG
//! location ([`crate::patch_manager::XemuInstallationType::Native`]):
//!
//! * a `Native` executable - one `discover_xemu_profiles` classified
//!   `Native` because it was found by literally searching the current
//!   process's real `PATH`. Fabricating that match needs a process-global
//!   `PATH` mutation that would race every concurrent test reading `PATH`
//!   (`std::env::set_var` is `unsafe`), so that single leg stays a pure
//!   unit - the same accepted limitation as `duckstation_execution`'s
//!   suite.
//! * an `Explicit` executable - an exact path supplied via
//!   `roots.explicit_executables` that the host already confirmed through
//!   its own provenance (an EmuWiz-managed AppImage). This *is* exercised
//!   end-to-end: the `ReadyFixture` is exactly that shape and
//!   `*_reaches_a_real_command` runs the full [`preflight_xemu_launch`]
//!   pipeline - including the command planner's own MCPX/BIOS/EEPROM/HDD
//!   gating - through to a produced [`XemuCommand`].
//!
//! Spawn mechanics themselves are still proven directly against a
//! hand-built [`XemuCommand`], mirroring
//! `duckstation_execution::tests::hand_built_command`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::executable_signatures::XBE_CERTIFICATE_READ_BYTES;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::launch::xemu_command::XemuCommandSelection;
use crate::patch_manager::XemuProfileDiscoveryRoots;
use crate::xdvdfs_traversal::test_support::synthetic_single_root_file_image;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-xemu-execution-{label}-{}-{sequence}",
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

/// A structurally valid `default.xbe` certificate carrying `title_id` -
/// mirrors `game_identity`'s own private `xbe_fixture` test helper exactly,
/// since that helper is not itself exported.
fn xbe_fixture(title_id: u32) -> Vec<u8> {
    const XBE_BASE_OFFSET: usize = 0x104;
    const XBE_CERT_ADDR_OFFSET: usize = 0x118;
    const XBE_CERT_TITLE_ID_OFFSET: usize = 0x8;
    let base = 0x10000_u32;
    let cert_file_offset = 0x200_usize;
    let cert_addr = base + cert_file_offset as u32;

    let mut bytes = vec![0_u8; cert_file_offset + XBE_CERTIFICATE_READ_BYTES];
    bytes[0..4].copy_from_slice(b"XBEH");
    bytes[XBE_BASE_OFFSET..XBE_BASE_OFFSET + 4].copy_from_slice(&base.to_le_bytes());
    bytes[XBE_CERT_ADDR_OFFSET..XBE_CERT_ADDR_OFFSET + 4].copy_from_slice(&cert_addr.to_le_bytes());
    bytes[cert_file_offset + XBE_CERT_TITLE_ID_OFFSET
        ..cert_file_offset + XBE_CERT_TITLE_ID_OFFSET + 4]
        .copy_from_slice(&title_id.to_le_bytes());
    bytes
}

/// A minimal, structurally valid XDVDFS Xbox disc image holding exactly one
/// root `DEFAULT.XBE` whose certificate carries `title_id`.
fn xbox_disc_image_fixture(title_id: u32) -> Vec<u8> {
    synthetic_single_root_file_image("DEFAULT.XBE", &xbe_fixture(title_id))
}

const XBOX_TITLE_ID: &str = "4D530058";
const XBOX_TITLE_ID_RAW: u32 = 0x4D53_0058;

fn base_roots(fixture: &Fixture) -> XemuProfileDiscoveryRoots {
    XemuProfileDiscoveryRoots {
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

/// A real, minimal `xemu.toml` with all four required system files
/// configured and present on disk beside it, so `XemuHealth` genuinely
/// reports `Present` for MCPX/flash BIOS/EEPROM/HDD - never faked past the
/// real config-reading code path.
fn write_healthy_config(profile_root: &std::path::Path) {
    fs::create_dir_all(profile_root.join("system")).unwrap();
    fs::write(profile_root.join("system/mcpx.bin"), b"mcpx").unwrap();
    fs::write(profile_root.join("system/bios.bin"), b"bios").unwrap();
    fs::write(profile_root.join("system/eeprom.bin"), b"eeprom").unwrap();
    fs::write(profile_root.join("system/hdd.qcow2"), b"hdd").unwrap();
    fs::write(
        profile_root.join("xemu.toml"),
        "[sys.files]\n\
         bootrom_path = 'system/mcpx.bin'\n\
         flashrom_path = 'system/bios.bin'\n\
         eeprom_path = 'system/eeprom.bin'\n\
         hdd_path = 'system/hdd.qcow2'\n",
    )
    .unwrap();
}

/// A ready native xemu fixture in the managed-AppImage shape: a Native XDG
/// profile with a real `xemu.toml` and all four system files present,
/// launched by a caller-confirmed explicit executable supplied via
/// `roots.explicit_executables` (classified `Explicit`, which
/// `resolve_xemu_native_launch_binding` now accepts for a `Native`
/// profile - see the module doc comment), and a loose original-Xbox disc
/// image whose verified title ID becomes the request's expected title
/// id/game key - computed via the same `inspect_catalogued_game_identity`
/// the module itself uses, never hand-typed.
struct ReadyFixture {
    fixture: Fixture,
    roots: XemuProfileDiscoveryRoots,
    profile_root: PathBuf,
    request: XemuLaunchRequest,
}

fn build_ready_fixture(label: &str) -> ReadyFixture {
    let fixture = Fixture::new(label);
    let mut roots = base_roots(&fixture);
    let profile_root = roots.xdg_config_home.join("xemu/xemu");
    fs::create_dir_all(&profile_root).unwrap();
    write_healthy_config(&profile_root);
    let executable = fixture.write_executable("bin/xemu", b"#!/bin/sh\nexit 0\n");
    roots.explicit_executables.push(executable.clone());
    let content = fixture.write(
        "games/game.iso",
        &xbox_disc_image_fixture(XBOX_TITLE_ID_RAW),
    );

    // `profile_id` is `xemu:<configuration_path>` (never hand-reconstructed)
    // - read it off a real discovery pass rather than assuming the format.
    let discovery = discover_xemu_profiles(&roots);
    let profile_id = discovery
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == profile_root)
        .expect("fixture profile must be discovered")
        .profile_id
        .clone();

    let request = XemuLaunchRequest {
        selected_content_path: content,
        expected_platform_id: "Xbox".to_string(),
        expected_game_key: XBOX_TITLE_ID.to_string(),
        expected_xbox_title_id: XBOX_TITLE_ID.to_string(),
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

fn preflight(ready: &ReadyFixture) -> Result<XemuCommand, XemuLaunchPreflightError> {
    preflight_xemu_launch(&ready.request, &ready.roots)
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
fn disc_image_removed_is_rejected() {
    let ready = build_ready_fixture("disc-removed");
    fs::remove_file(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, XemuLaunchPreflightErrorKind::ContentNotFound);
}

#[test]
fn disc_image_replaced_by_a_directory_is_rejected() {
    let ready = build_ready_fixture("disc-replaced-with-dir");
    fs::remove_file(&ready.request.selected_content_path).unwrap();
    fs::create_dir_all(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        XemuLaunchPreflightErrorKind::ContentNotRegularFile
    );
}

#[test]
fn symlink_content_is_rejected() {
    let ready = build_ready_fixture("symlink-content");
    let link = ready.fixture.path("games/link.iso");
    symlink(&ready.request.selected_content_path, &link).unwrap();
    let mut request = ready.request.clone();
    request.selected_content_path = link;
    let error = preflight_xemu_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(error.kind, XemuLaunchPreflightErrorKind::ContentIsSymlink);
}

#[test]
fn loose_xbe_is_rejected() {
    let ready = build_ready_fixture("loose-xbe");
    let xbe = ready
        .fixture
        .write("games/default.xbe", &xbe_fixture(XBOX_TITLE_ID_RAW));
    let mut request = ready.request.clone();
    request.selected_content_path = xbe;
    let error = preflight_xemu_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        XemuLaunchPreflightErrorKind::ContentFormatUnsupported
    );
}

#[test]
fn mount_input_archive_content_is_rejected() {
    let ready = build_ready_fixture("archive-content");
    let zip = ready.fixture.write("games/game.zip", b"pk not a real zip");
    let mut request = ready.request.clone();
    request.selected_content_path = zip;
    let error = preflight_xemu_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        XemuLaunchPreflightErrorKind::ContentRequiresMount
    );
}

// --- identity checks (step 4) --------------------------------------------------------------------

#[test]
fn wrong_expected_game_key_is_rejected() {
    let mut ready = build_ready_fixture("wrong-game-key");
    ready.request.expected_game_key = "AAAAAAAA".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, XemuLaunchPreflightErrorKind::IdentityMismatch);
}

#[test]
fn wrong_expected_xbox_title_id_is_rejected() {
    let mut ready = build_ready_fixture("wrong-title-id");
    ready.request.expected_xbox_title_id = "AAAAAAAA".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        XemuLaunchPreflightErrorKind::XboxTitleIdMismatch
    );
}

#[test]
fn xbox_360_expectation_never_authorizes_a_native_xbox_disc() {
    // This slice only ever authorizes original-Xbox content
    // (`XEMU_SUPPORTED_PLATFORM_ID`). A caller-supplied request expecting
    // "Xbox360" (e.g. a stale request built against the wrong candidate) is
    // refused as an identity mismatch rather than silently accepted against
    // a genuinely valid Xbox XDVDFS disc - the Xenia/xemu platform boundary
    // is never crossed here.
    let mut ready = build_ready_fixture("xbox-360");
    ready.request.expected_platform_id = "Xbox360".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, XemuLaunchPreflightErrorKind::IdentityMismatch);
}

// --- profile lookup checks (steps 5-6) -------------------------------------------------------------

#[test]
fn profile_directory_removed_is_rejected() {
    let ready = build_ready_fixture("profile-removed");
    fs::remove_dir_all(&ready.profile_root).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, XemuLaunchPreflightErrorKind::ProfileNotFound);
}

#[test]
fn profile_root_drift_is_rejected() {
    let ready = build_ready_fixture("root-drift");
    let mut request = ready.request.clone();
    // A stale profile id from a root that no longer matches xemu's current
    // default XDG resolution - never substituted with a different profile.
    request.profile_id = format!(
        "xemu:{}",
        ready.fixture.path("stale-config/xemu/xemu").display()
    );
    let error = preflight_xemu_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(error.kind, XemuLaunchPreflightErrorKind::ProfileNotFound);
}

// --- every earlier check passing reaches binding resolution and a real command --------------------

#[test]
fn a_fully_valid_request_with_a_caller_confirmed_executable_reaches_a_real_command() {
    // The `ReadyFixture` is the managed-AppImage shape: a Native XDG profile
    // (with all four system files healthy) launched by a caller-confirmed
    // explicit executable. Every earlier preflight step passes, binding
    // resolution now succeeds, and the command plan's own Xbox system-file
    // gating is satisfied, so a real `XemuCommand` is produced.
    let ready = build_ready_fixture("reaches-binding");
    let command = preflight(&ready).expect("a Native profile binds a caller-confirmed executable");
    assert_eq!(command.executable, ready.request.expected_executable);
    assert_eq!(
        command.arguments,
        vec![
            std::ffi::OsString::from("-dvd_path"),
            ready.request.selected_content_path.clone().into_os_string(),
        ]
    );
    assert_eq!(command.selection.platform_id, "Xbox");
    assert_eq!(command.selection.verified_xbox_title_id, XBOX_TITLE_ID);
}

#[test]
fn a_forced_untrusted_profile_never_binds_the_caller_confirmed_executable() {
    use crate::patch_manager::{
        XemuInstallationType, XemuLaunchBlockerKind, resolve_xemu_native_launch_binding,
    };
    let ready = build_ready_fixture("forced-untrusted");
    let base = discover_xemu_profiles(&ready.roots)
        .profiles
        .into_iter()
        .find(|profile| profile.configuration_path == ready.profile_root)
        .expect("fixture profile must be discovered");
    resolve_xemu_native_launch_binding(&base).expect("native profile binds the confirmed exe");
    for forced in [
        XemuInstallationType::Portable,
        XemuInstallationType::FlatpakUser,
        XemuInstallationType::Explicit,
    ] {
        let mut profile = base.clone();
        profile.installation_type = forced;
        let error = resolve_xemu_native_launch_binding(&profile)
            .expect_err("only a Native profile may bind a caller-confirmed executable");
        assert_eq!(
            error.kind,
            XemuLaunchBlockerKind::UnsupportedInstallationType
        );
    }
}

#[test]
fn without_xbox_system_files_a_trusted_executable_is_still_blocked() {
    // The caller-confirmed executable binds, but the Xbox MCPX/BIOS/EEPROM/
    // HDD readiness is independent: an unhealthy config must keep the launch
    // blocked rather than let a trusted executable imply readiness.
    let ready = build_ready_fixture("no-system-files");
    // Drop the MCPX/BIOS/EEPROM/HDD files the healthy config references,
    // keeping `xemu.toml`, so the command planner's own gating fires.
    fs::remove_dir_all(ready.profile_root.join("system")).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert!(
        matches!(
            error.kind,
            XemuLaunchPreflightErrorKind::CommandBlocked
                | XemuLaunchPreflightErrorKind::CandidateNotReady
        ),
        "system-file-unmet launch must stay blocked, got {:?}",
        error.kind
    );
}

// --- final pre-spawn recheck units (step 10) --------------------------------------------------------

#[test]
fn recheck_executable_rejects_a_missing_executable() {
    let fixture = Fixture::new("recheck-missing");
    let path = fixture.path("bin/xemu");
    let error = recheck_executable(&path).unwrap_err();
    assert_eq!(error.kind, XemuLaunchPreflightErrorKind::ExecutableMissing);
}

#[test]
fn recheck_executable_rejects_a_symlink() {
    let fixture = Fixture::new("recheck-symlink");
    let real = fixture.write_executable("bin/xemu-real", b"#!/bin/sh\nexit 0\n");
    let link = fixture.path("bin/xemu");
    symlink(&real, &link).unwrap();
    let error = recheck_executable(&link).unwrap_err();
    assert_eq!(error.kind, XemuLaunchPreflightErrorKind::ExecutableUnsafe);
}

#[test]
fn recheck_executable_rejects_a_non_executable_file() {
    let fixture = Fixture::new("recheck-not-executable");
    let path = fixture.write("bin/xemu", b"#!/bin/sh\nexit 0\n");
    let error = recheck_executable(&path).unwrap_err();
    assert_eq!(
        error.kind,
        XemuLaunchPreflightErrorKind::ExecutableNotExecutable
    );
}

#[test]
fn recheck_executable_accepts_a_real_executable() {
    let fixture = Fixture::new("recheck-ok");
    let path = fixture.write_executable("bin/xemu", b"#!/bin/sh\nexit 0\n");
    recheck_executable(&path).expect("a real, executable regular file must pass");
}

#[test]
fn inspect_and_capture_content_identity_rejects_a_removed_disc() {
    let fixture = Fixture::new("recheck-content-missing");
    let path = fixture.path("games/game.iso");
    let error = inspect_and_capture_content_identity(&path).unwrap_err();
    assert_eq!(error.kind, XemuLaunchPreflightErrorKind::ContentNotFound);
}

// --- spawn: mechanics proven against a hand-built command, never a discovered one ------------------

fn hand_built_command(executable: PathBuf, content_path: PathBuf) -> XemuCommand {
    XemuCommand {
        executable,
        arguments: vec![
            std::ffi::OsString::from("-dvd_path"),
            content_path.clone().into_os_string(),
        ],
        working_directory: None,
        selection: XemuCommandSelection {
            profile_id: "test".to_string(),
            platform_id: "Xbox".to_string(),
            verified_xbox_title_id: XBOX_TITLE_ID.to_string(),
            content_path,
        },
    }
}

fn wait_for_exit(process: &mut LaunchedXemuProcess) -> &ProcessExitReport {
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
        "bin/xemu",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let content_path = fixture.write("games/game.iso", b"synthetic content");
    let command = hand_built_command(executable, content_path.clone());
    let mut process = spawn_xemu(command).expect("the fake script must spawn");
    wait_for_exit(&mut process);
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines, vec!["-dvd_path", content_path.to_str().unwrap()]);
}

#[test]
fn successful_spawn_returns_a_real_pid_and_clean_exit() {
    let fixture = Fixture::new("spawn-success");
    let executable = fixture.write_executable("bin/xemu", b"#!/bin/sh\nexit 0\n");
    let content_path = fixture.write("games/game.iso", b"synthetic content");
    let command = hand_built_command(executable, content_path);
    let mut process = spawn_xemu(command).expect("the fake script must spawn");
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
        "bin/xemu",
        b"#!/bin/sh\necho 'synthetic xemu failure' 1>&2\nexit 7\n",
    );
    let content_path = fixture.write("games/game.iso", b"synthetic content");
    let command = hand_built_command(executable, content_path);
    let mut process = spawn_xemu(command).unwrap();
    let report = wait_for_exit(&mut process);
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic xemu failure"));
}

#[test]
fn spaces_and_unicode_paths_are_passed_intact() {
    let fixture = Fixture::new("spaces-unicode");
    let capture_path = fixture.path("argv-capture.txt");
    let executable = fixture.write_executable(
        "bin/xemu",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let unicode_name = "games/Halo こんにちは 日本語, with spaces.iso";
    let content_path = fixture.write(unicode_name, b"synthetic content");
    let command = hand_built_command(executable, content_path.clone());
    let mut process = spawn_xemu(command).unwrap();
    wait_for_exit(&mut process);
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines, vec!["-dvd_path", content_path.to_str().unwrap()]);
}

#[test]
fn shell_metacharacters_in_content_path_are_never_interpreted() {
    let marker = std::env::temp_dir().join(format!(
        "archivefs-xemu-execution-shell-marker-{}",
        std::process::id()
    ));
    let _ = fs::remove_file(&marker);
    let fixture = Fixture::new("shell-metacharacters");
    let executable = fixture.write_executable("bin/xemu", b"#!/bin/sh\nexit 0\n");
    let dangerous_name = format!("games/game; touch {} #.iso", marker.display());
    let dangerous_path = fixture.write(&dangerous_name, b"synthetic content");
    let command = hand_built_command(executable, dangerous_path);
    let mut process = spawn_xemu(command).unwrap();
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
    let result = spawn_xemu(command);
    assert!(matches!(result, Err(XemuLaunchSpawnError::Spawn(_))));
}
