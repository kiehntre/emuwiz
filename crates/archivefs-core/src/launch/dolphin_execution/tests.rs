//! Tests for native Dolphin launch preflight/execution.
//!
//! Every fixture is a real temp directory on disk (a fake `dolphin-emu`
//! shell script, a real `Dolphin.ini`, a synthetic but structurally valid
//! GameCube ISO header) driven through the exact same real discovery/
//! identity/planning functions production code uses -
//! `discover_dolphin_local_profiles`, `resolve_dolphin_native_launch_binding`,
//! `inspect_catalogued_game_identity`, `build_launch_plan`,
//! `build_dolphin_command_plan` - never a shortcut or a mocked plan. Spawn
//! tests genuinely `fork`/`exec` the fake script; no real installed Dolphin
//! is required anywhere.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::patch_manager::{DolphinLocalDiscoveryRoots, DolphinUserDirectoryMode};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-dolphin-execution-{label}-{}-{sequence}",
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

/// A structurally valid, minimal Dolphin disc header: 6-byte Game ID,
/// disc/revision bytes, and the GameCube magic word at `0x1c` - the only
/// bytes `inspect_catalogued_game_identity` ever reads for a direct ISO.
fn gamecube_iso_bytes(game_id: &[u8; 6], revision: u8) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x20];
    bytes[..6].copy_from_slice(game_id);
    bytes[6] = 1;
    bytes[7] = revision;
    bytes[0x1c..0x20].copy_from_slice(&[0xc2, 0x33, 0x9f, 0x3d]);
    bytes
}

fn base_roots(fixture: &Fixture) -> DolphinLocalDiscoveryRoots {
    DolphinLocalDiscoveryRoots {
        home: fixture.path("home"),
        xdg_config_home: fixture.path("config"),
        xdg_data_home: fixture.path("data"),
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
        appimage_directory: None,
        dolphin_emu_userpath_override: None,
    }
}

/// A fully wired, genuinely `Ready` `Explicit`/`ExplicitRoot` native Dolphin
/// fixture: a fake executable script, a genuine single Dolphin user root
/// (`Config/Dolphin.ini` beneath it, proving `-u <root>` semantics), and a
/// loose GameCube ISO whose verified Game ID becomes the request's expected
/// Game ID - computed via the same `inspect_catalogued_game_identity` the
/// module itself uses, never hand-typed.
///
/// `Explicit`, not `Native`, because only `explicit_configuration_roots`/
/// `explicit_executables` let a test deterministically control which
/// installation type an executable resolves to - `Native` executables are
/// only ever found via a real `PATH` scan
/// (`discover_dolphin_local_executables`), which this suite never touches so
/// the real host environment can never leak into a test.
struct ReadyFixture {
    fixture: Fixture,
    roots: DolphinLocalDiscoveryRoots,
    profile_root: PathBuf,
    request: DolphinLaunchRequest,
}

fn build_ready_fixture(label: &str) -> ReadyFixture {
    let fixture = Fixture::new(label);
    let profile_root = fixture.path("dolphin-portable");
    fs::create_dir_all(profile_root.join("Config")).unwrap();
    fs::write(profile_root.join("Dolphin.ini"), b"[Core]\n").unwrap();
    fs::write(profile_root.join("Config/Dolphin.ini"), b"[Core]\n").unwrap();
    let executable = fixture.write_executable("bin/dolphin-emu", b"#!/bin/sh\nexit 0\n");
    let content = fixture.write("games/game.iso", &gamecube_iso_bytes(b"GALE01", 0));

    let mut roots = base_roots(&fixture);
    roots
        .explicit_configuration_roots
        .push(profile_root.clone());
    roots.explicit_executables.push(executable.clone());

    let request = DolphinLaunchRequest {
        selected_content_path: content,
        expected_game_id: "GALE01".to_string(),
        profile_id: format!("dolphin:{}", profile_root.display()),
        expected_executable: executable,
        expected_user_directory_mode: DolphinUserDirectoryMode::ExplicitRoot(profile_root.clone()),
    };
    ReadyFixture {
        fixture,
        roots,
        profile_root,
        request,
    }
}

fn preflight(ready: &ReadyFixture) -> Result<DolphinCommand, DolphinLaunchPreflightError> {
    preflight_dolphin_launch(&ready.request, &ready.roots)
}

// --- native direct-content Ready candidate passes preflight -----------------

#[test]
fn valid_native_gamecube_iso_succeeds() {
    let ready = build_ready_fixture("ready");
    let command =
        preflight(&ready).expect("a fully wired explicit-root fixture must preflight cleanly");
    assert_eq!(command.executable, ready.request.expected_executable);
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("-u"),
            ready.profile_root.clone().into_os_string(),
            OsString::from("-e"),
            ready.request.selected_content_path.clone().into_os_string(),
        ]
    );
    assert_eq!(command.selection.platform_id, "GameCube");
    assert_eq!(command.selection.game_id, "GALE01");
    assert_eq!(
        command.selection.content_path,
        ready.request.selected_content_path
    );
}

// --- changed content identity is detected by the same mechanism used at spawn time ----------

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

// --- symlink content rejected ------------------------------------------------------------------

#[test]
fn symlink_content_is_rejected() {
    let ready = build_ready_fixture("symlink-content");
    let link = ready.fixture.path("games/link.iso");
    symlink(&ready.request.selected_content_path, &link).unwrap();
    let mut request = ready.request.clone();
    request.selected_content_path = link;
    let error = preflight_dolphin_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::ContentIsSymlink
    );
}

// --- unsupported content format rejected ---------------------------------------------------------

#[test]
fn non_iso_gcm_extension_is_rejected() {
    let ready = build_ready_fixture("unsupported-format");
    let rvz = ready
        .fixture
        .write("games/game.rvz", &gamecube_iso_bytes(b"GALE01", 0));
    let mut request = ready.request.clone();
    request.selected_content_path = rvz;
    let error = preflight_dolphin_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::ContentFormatUnsupported
    );
}

#[test]
fn mount_input_archive_content_is_rejected() {
    let ready = build_ready_fixture("archive-content");
    let zip = ready.fixture.write("games/game.zip", b"pk not a real zip");
    let mut request = ready.request.clone();
    request.selected_content_path = zip;
    let error = preflight_dolphin_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::ContentRequiresMount
    );
}

// --- identity mismatch rejected -----------------------------------------------------------------

#[test]
fn wrong_expected_game_id_is_rejected() {
    let mut ready = build_ready_fixture("wrong-game-id");
    ready.request.expected_game_id = "RALE01".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::IdentityMismatch
    );
}

// --- profile disappears rejected ----------------------------------------------------------------

#[test]
fn profile_directory_removed_is_rejected() {
    let ready = build_ready_fixture("profile-removed");
    fs::remove_dir_all(&ready.profile_root).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- executable changes/disappears rejected -------------------------------------------------------

#[test]
fn executable_disappearing_is_rejected() {
    let ready = build_ready_fixture("executable-missing");
    fs::remove_file(&ready.request.expected_executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn executable_replaced_with_a_symlink_is_rejected() {
    let ready = build_ready_fixture("executable-symlink");
    let real = ready
        .fixture
        .write_executable("bin/dolphin-emu-real", b"#!/bin/sh\nexit 0\n");
    fs::remove_file(&ready.request.expected_executable).unwrap();
    symlink(&real, &ready.request.expected_executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- binding drift between readiness and click rejected --------------------------------------------

#[test]
fn binding_drift_between_readiness_and_click_is_rejected() {
    let mut ready = build_ready_fixture("binding-drift");
    // A different executable path than the one that will freshly resolve -
    // simulating a GUI that showed the user a binding, then the real
    // binding changed (or the GUI's own remembered value was stale) before
    // they clicked Launch.
    ready.request.expected_executable = ready.fixture.path("bin/a-different-dolphin-emu");
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, DolphinLaunchPreflightErrorKind::BindingDrift);
}

#[test]
fn binding_mode_drift_is_rejected() {
    let mut ready = build_ready_fixture("binding-mode-drift");
    ready.request.expected_user_directory_mode = DolphinUserDirectoryMode::DefaultNative;
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, DolphinLaunchPreflightErrorKind::BindingDrift);
}

// --- explicit root replaced/symlinked rejected -----------------------------------------------------

#[test]
fn explicit_root_replaced_with_a_symlink_is_rejected() {
    let ready = build_ready_fixture("explicit-root-symlink");
    let real_root = ready.fixture.path("real-portable");
    fs::rename(&ready.profile_root, &real_root).unwrap();
    symlink(&real_root, &ready.profile_root).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- DOLPHIN_EMU_USERPATH / legacy layout conflicts reject DefaultNative --------------------------

/// A `Native` installation-type fixture built entirely from `roots` fields
/// (XDG config/data dirs + `Dolphin.ini`), with no executable candidate at
/// all. `DOLPHIN_EMU_USERPATH`/legacy-layout checks happen before Dolphin's
/// own executable resolution in
/// `resolve_dolphin_native_launch_binding::resolve_default_native_binding`,
/// so this fixture proves the rejection through the real preflight pipeline
/// without ever needing a `PATH`-discovered `Native` executable.
struct NativeConflictFixture {
    fixture: Fixture,
    roots: DolphinLocalDiscoveryRoots,
    request: DolphinLaunchRequest,
}

fn build_native_conflict_fixture(label: &str) -> NativeConflictFixture {
    let fixture = Fixture::new(label);
    let config_root = fixture.path("config/dolphin-emu");
    let data_root = fixture.path("data/dolphin-emu");
    fs::create_dir_all(&config_root).unwrap();
    fs::create_dir_all(&data_root).unwrap();
    fs::write(config_root.join("Dolphin.ini"), b"[Core]\n").unwrap();
    let content = fixture.write("games/game.iso", &gamecube_iso_bytes(b"GALE01", 0));

    let roots = base_roots(&fixture);
    let request = DolphinLaunchRequest {
        selected_content_path: content,
        expected_game_id: "GALE01".to_string(),
        profile_id: format!("dolphin:{}", config_root.display()),
        expected_executable: PathBuf::from("/usr/bin/dolphin-emu"),
        expected_user_directory_mode: DolphinUserDirectoryMode::DefaultNative,
    };
    NativeConflictFixture {
        fixture,
        roots,
        request,
    }
}

#[test]
fn dolphin_emu_userpath_override_rejects_default_native() {
    let mut ready = build_native_conflict_fixture("userpath-override");
    ready.roots.dolphin_emu_userpath_override = Some(ready.fixture.path("override-root"));
    let error = preflight_dolphin_launch(&ready.request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn legacy_dolphin_emu_directory_rejects_default_native() {
    let ready = build_native_conflict_fixture("legacy-precedence");
    fs::create_dir_all(ready.roots.home.join(".dolphin-emu")).unwrap();
    let error = preflight_dolphin_launch(&ready.request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        DolphinLaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- spawn: fake executable receives exact argv ------------------------------------------------------

#[test]
fn fake_executable_receives_exact_argv() {
    let ready = build_ready_fixture("argv-capture");
    let capture_path = ready.fixture.path("argv-capture.txt");
    ready.fixture.write_executable(
        "bin/dolphin-emu",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let command = preflight(&ready).unwrap();
    let mut process = spawn_dolphin(command).expect("the fake script must spawn");
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
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(
        lines,
        vec![
            "-u",
            ready.profile_root.to_str().unwrap(),
            "-e",
            ready.request.selected_content_path.to_str().unwrap()
        ]
    );
}

// --- spawn: successful spawn returns PID, clean exit reported -----------------------------------------

#[test]
fn successful_spawn_returns_a_real_pid_and_clean_exit() {
    let ready = build_ready_fixture("spawn-success");
    let command = preflight(&ready).unwrap();
    let mut process = spawn_dolphin(command).expect("the fake script must spawn");
    assert!(process.pid > 0);
    assert!(process.is_running() || process.poll().is_some());

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let report = loop {
        if let Some(report) = process.poll() {
            break report;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let status = report.status.as_ref().expect("wait() must succeed");
    assert!(status.success(), "the fake script exits 0");
}

// --- spawn: non-zero exit and bounded stderr reported -------------------------------------------------

#[test]
fn non_zero_exit_and_stderr_are_reported() {
    let ready = build_ready_fixture("spawn-nonzero");
    ready.fixture.write_executable(
        "bin/dolphin-emu",
        b"#!/bin/sh\necho 'synthetic dolphin failure' 1>&2\nexit 7\n",
    );
    let command = preflight(&ready).unwrap();
    let mut process = spawn_dolphin(command).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let report = loop {
        if let Some(report) = process.poll() {
            break report;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic dolphin failure"));
}

// --- spawn: shell metacharacters never interpreted -------------------------------------------------

#[test]
fn shell_metacharacters_in_content_path_are_never_interpreted() {
    let marker = std::env::temp_dir().join(format!(
        "archivefs-dolphin-execution-shell-marker-{}",
        std::process::id()
    ));
    let _ = fs::remove_file(&marker);
    let ready = build_ready_fixture("shell-metacharacters");
    let dangerous_name = format!("game; touch {} #.iso", marker.display());
    let dangerous_path = ready.fixture.write(
        &format!("games/{dangerous_name}"),
        &gamecube_iso_bytes(b"GALE01", 0),
    );
    let mut request = ready.request.clone();
    request.selected_content_path = dangerous_path;
    let command = preflight_dolphin_launch(&request, &ready.roots).unwrap();
    let mut process = spawn_dolphin(command).unwrap();
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
    assert!(
        !marker.exists(),
        "shell metacharacters in a content path must never be interpreted"
    );
    let _ = fs::remove_file(&marker);
}

// --- spawn failure reported -----------------------------------------------------------------------

#[test]
fn spawn_failure_is_reported() {
    let ready = build_ready_fixture("spawn-failure");
    let mut command = preflight(&ready).unwrap();
    command.executable = ready.fixture.path("bin/does-not-exist");
    let result = spawn_dolphin(command);
    assert!(matches!(result, Err(DolphinLaunchSpawnError::Spawn(_))));
}
