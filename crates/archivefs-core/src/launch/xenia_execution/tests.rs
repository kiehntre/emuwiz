//! Tests for native Xenia launch preflight/execution.
//!
//! Unlike PCSX2/DuckStation/xemu/PPSSPP/RPCS3, Xenia's own
//! `resolve_xenia_launch_binding` never searches `$PATH` at all - it only
//! ever looks inside the profile's own configuration directory for a
//! native executable name (see `patch_manager::xenia_local`'s own doc
//! comment, and the fix landed alongside this module). That means, unlike
//! every sibling execution test suite in this crate, a fully genuine
//! end-to-end preflight success **is** safely reachable here without
//! touching any global process state: `roots.explicit_configuration_roots`
//! plus a real executable placed directly in that same fixture directory is
//! exactly the real, documented Xenia Canary portable-install shape, no
//! `$PATH`/global-env trick required. So this suite exercises the full
//! [`preflight_xenia_launch`] pipeline end to end, including genuine
//! binding success and drift-after-planning, alongside spawn mechanics
//! proven against a hand-built command exactly like the sibling suites.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::executable_signatures::XEX_MAGIC;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::patch_manager::XeniaProfileDiscoveryRoots;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-xenia-execution-{label}-{}-{sequence}",
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

/// A minimal, structurally valid XEX2 header carrying `title_id`/`media_id`
/// in its execution-info optional header - mirrors `game_identity`'s own
/// private `xex_fixture` test helper exactly, since that helper is not
/// itself exported.
fn xex_fixture(title_id: u32, media_id: u32) -> Vec<u8> {
    const EXECUTION_INFO_OFFSET: u32 = 0x30;
    const XEX_HEADER_COUNT_OFFSET: usize = 0x14;
    const XEX_OPT_HEADER_TABLE_OFFSET: usize = 0x18;
    const XEX_EXECUTION_INFO_KEY: u32 = 0x0004_0006;
    const XEX_EXECUTION_INFO_BYTES: usize = 0x18;

    let mut bytes = vec![0_u8; EXECUTION_INFO_OFFSET as usize + XEX_EXECUTION_INFO_BYTES];
    bytes[0..4].copy_from_slice(XEX_MAGIC.as_slice());
    bytes[XEX_HEADER_COUNT_OFFSET..XEX_HEADER_COUNT_OFFSET + 4]
        .copy_from_slice(&1_u32.to_be_bytes());
    let table_offset = XEX_OPT_HEADER_TABLE_OFFSET;
    bytes[table_offset..table_offset + 4].copy_from_slice(&XEX_EXECUTION_INFO_KEY.to_be_bytes());
    bytes[table_offset + 4..table_offset + 8].copy_from_slice(&EXECUTION_INFO_OFFSET.to_be_bytes());
    let info_offset = EXECUTION_INFO_OFFSET as usize;
    bytes[info_offset..info_offset + 4].copy_from_slice(&media_id.to_be_bytes());
    bytes[info_offset + 0xC..info_offset + 0x10].copy_from_slice(&title_id.to_be_bytes());
    bytes
}

const XEX_TITLE_ID_RAW: u32 = 0x4156_07D2;
const XEX_MEDIA_ID_RAW: u32 = 0x4C27_792A;
const XEX_TITLE_ID: &str = "415607D2";
const XEX_MEDIA_ID: &str = "4C27792A";

fn base_roots(fixture: &Fixture) -> XeniaProfileDiscoveryRoots {
    XeniaProfileDiscoveryRoots {
        explicit_configuration_roots: vec![fixture.root.clone()],
    }
}

/// A genuinely `Ready` native Xenia fixture: a real
/// `xenia-canary.config.toml` marker, a real native `xenia_canary`
/// executable directly inside the same directory (Xenia's own real,
/// documented portable-install shape - no `$PATH` involved), and a loose
/// `.xex` file whose verified title/media ID becomes the request's expected
/// values - computed via the same `inspect_catalogued_game_identity` the
/// module itself uses, never hand-typed.
struct ReadyFixture {
    fixture: Fixture,
    roots: XeniaProfileDiscoveryRoots,
    request: XeniaLaunchRequest,
}

fn build_ready_fixture(label: &str) -> ReadyFixture {
    let fixture = Fixture::new(label);
    fs::write(fixture.path("xenia-canary.config.toml"), b"").unwrap();
    let executable = fixture.write_executable("xenia_canary", b"#!/bin/sh\nexit 0\n");
    let content = fixture.write(
        "games/default.xex",
        &xex_fixture(XEX_TITLE_ID_RAW, XEX_MEDIA_ID_RAW),
    );

    let roots = base_roots(&fixture);
    let discovery = discover_xenia_profiles(&roots);
    let profile_id = discovery
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == fixture.root)
        .expect("fixture profile must be discovered")
        .profile_id
        .clone();

    let request = XeniaLaunchRequest {
        selected_content_path: content,
        expected_platform_id: "Xbox360".to_string(),
        expected_game_key: XEX_TITLE_ID.to_string(),
        expected_xex_title_id: Some(XEX_TITLE_ID.to_string()),
        expected_xex_media_id: Some(XEX_MEDIA_ID.to_string()),
        profile_id,
        expected_executable: executable,
    };
    ReadyFixture {
        fixture,
        roots,
        request,
    }
}

fn preflight(ready: &ReadyFixture) -> Result<XeniaCommand, XeniaLaunchPreflightError> {
    preflight_xenia_launch(&ready.request, &ready.roots)
}

// --- genuine end-to-end success -----------------------------------------------------------------

#[test]
fn valid_native_xex_succeeds() {
    let ready = build_ready_fixture("ready");
    let command = preflight(&ready).expect("a fully wired native fixture must preflight cleanly");
    assert_eq!(command.executable, ready.request.expected_executable);
    assert_eq!(
        command.arguments,
        vec![ready.request.selected_content_path.clone().into_os_string()]
    );
    assert_eq!(command.selection.platform_id, "Xbox360");
    assert_eq!(
        command.selection.verified_xex_title_id.as_deref(),
        Some(XEX_TITLE_ID)
    );
    assert_eq!(
        command.selection.verified_xex_media_id.as_deref(),
        Some(XEX_MEDIA_ID)
    );
    assert_eq!(
        command.selection.content_path,
        ready.request.selected_content_path
    );
}

// --- content path checks -------------------------------------------------------------------------

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
fn xex_removed_after_planning_is_rejected() {
    let ready = build_ready_fixture("xex-removed");
    fs::remove_file(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, XeniaLaunchPreflightErrorKind::ContentNotFound);
}

#[test]
fn xex_replaced_by_a_directory_is_rejected() {
    let ready = build_ready_fixture("xex-replaced-with-dir");
    fs::remove_file(&ready.request.selected_content_path).unwrap();
    fs::create_dir_all(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::ContentNotRegularFile
    );
}

#[test]
fn symlink_content_is_rejected() {
    let ready = build_ready_fixture("symlink-content");
    let link = ready.fixture.path("games/link.xex");
    symlink(&ready.request.selected_content_path, &link).unwrap();
    let mut request = ready.request.clone();
    request.selected_content_path = link;
    let error = preflight_xenia_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(error.kind, XeniaLaunchPreflightErrorKind::ContentIsSymlink);
}

#[test]
fn xbox_360_iso_is_refused() {
    let ready = build_ready_fixture("xbox360-iso");
    let iso = ready.fixture.write(
        "games/game.iso",
        &xex_fixture(XEX_TITLE_ID_RAW, XEX_MEDIA_ID_RAW),
    );
    let mut request = ready.request.clone();
    request.selected_content_path = iso;
    let error = preflight_xenia_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::ContentFormatUnsupported
    );
}

#[test]
fn original_xbox_xbe_content_is_refused() {
    // `.xbe` is the original-Xbox executable format, not Xbox 360's `.xex` -
    // refused purely by extension/format, never treated as a XEX.
    let ready = build_ready_fixture("xbe-content");
    let xbe = ready.fixture.write(
        "games/default.xbe",
        &xex_fixture(XEX_TITLE_ID_RAW, XEX_MEDIA_ID_RAW),
    );
    let mut request = ready.request.clone();
    request.selected_content_path = xbe;
    let error = preflight_xenia_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::ContentFormatUnsupported
    );
}

#[test]
fn mount_input_archive_content_is_rejected() {
    let ready = build_ready_fixture("archive-content");
    let zip = ready.fixture.write("games/game.zip", b"pk not a real zip");
    let mut request = ready.request.clone();
    request.selected_content_path = zip;
    let error = preflight_xenia_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::ContentRequiresMount
    );
}

// --- identity/title drift checks ------------------------------------------------------------------

#[test]
fn wrong_expected_game_key_is_rejected() {
    let mut ready = build_ready_fixture("wrong-game-key");
    ready.request.expected_game_key = "AAAAAAAA".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, XeniaLaunchPreflightErrorKind::IdentityMismatch);
}

#[test]
fn wrong_expected_media_id_is_rejected() {
    let mut ready = build_ready_fixture("wrong-media-id");
    ready.request.expected_xex_media_id = Some("AAAAAAAA".to_string());
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::XexIdentityMismatch
    );
}

#[test]
fn title_id_drift_after_planning_is_rejected() {
    // Simulate the XEX being swapped for a different, still-genuinely-valid
    // Xbox 360 title between when the user was shown "Ready" and this
    // click. The game key is the title ID for Xbox 360, so this surfaces as
    // an identity mismatch (checked first).
    let ready = build_ready_fixture("title-id-drift");
    fs::write(
        &ready.request.selected_content_path,
        xex_fixture(0x9999_9999, XEX_MEDIA_ID_RAW),
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, XeniaLaunchPreflightErrorKind::IdentityMismatch);
}

#[test]
fn wrong_platform_expectation_never_authorizes_a_native_xex() {
    let mut ready = build_ready_fixture("wrong-platform");
    ready.request.expected_platform_id = "Xbox".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, XeniaLaunchPreflightErrorKind::IdentityMismatch);
}

// --- profile lookup checks -------------------------------------------------------------------------

#[test]
fn profile_marker_removed_is_rejected() {
    // Xenia profiles are keyed by the caller-supplied explicit path itself,
    // not discovered by scanning - removing the config marker makes the
    // very same profile id ineligible rather than absent, which
    // `resolve_xenia_launch_binding` refuses (surfacing here as
    // `BindingUnavailable`), never silently treated as `ProfileNotFound`.
    let ready = build_ready_fixture("profile-removed");
    fs::remove_file(ready.fixture.path("xenia-canary.config.toml")).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn profile_root_drift_is_rejected() {
    let ready = build_ready_fixture("root-drift");
    let mut request = ready.request.clone();
    request.profile_id = "xenia-explicit-0000000000000000".to_string();
    let error = preflight_xenia_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(error.kind, XeniaLaunchPreflightErrorKind::ProfileNotFound);
}

// --- executable binding checks ----------------------------------------------------------------------

#[test]
fn executable_disappearing_is_rejected() {
    let ready = build_ready_fixture("executable-missing");
    fs::remove_file(&ready.request.expected_executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn executable_replaced_with_a_symlink_is_rejected() {
    let ready = build_ready_fixture("executable-symlink");
    let real = ready
        .fixture
        .write_executable("xenia_canary-real", b"#!/bin/sh\nexit 0\n");
    fs::remove_file(&ready.request.expected_executable).unwrap();
    symlink(&real, &ready.request.expected_executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn executable_losing_the_execute_bit_is_rejected() {
    let ready = build_ready_fixture("executable-not-executable");
    fs::set_permissions(
        &ready.request.expected_executable,
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn a_windows_exe_only_profile_is_never_authorized_through_preflight() {
    // The Windows `xenia_canary.exe` may genuinely exist beside a real
    // config marker, but a profile with only that binary and no native
    // Linux executable must never be authorized here - this crate never
    // assumes or configures Wine/Proton.
    let ready = build_ready_fixture("windows-exe-only");
    fs::remove_file(&ready.request.expected_executable).unwrap();
    fs::write(ready.fixture.path("xenia_canary.exe"), b"MZ fake pe").unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        XeniaLaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- final pre-spawn recheck units ------------------------------------------------------------------

#[test]
fn recheck_executable_rejects_a_missing_executable() {
    let fixture = Fixture::new("recheck-missing");
    let path = fixture.path("xenia_canary");
    let error = recheck_executable(&path).unwrap_err();
    assert_eq!(error.kind, XeniaLaunchPreflightErrorKind::ExecutableMissing);
}

#[test]
fn recheck_executable_accepts_a_real_executable() {
    let fixture = Fixture::new("recheck-ok");
    let path = fixture.write_executable("xenia_canary", b"#!/bin/sh\nexit 0\n");
    recheck_executable(&path).expect("a real, executable regular file must pass");
}

#[test]
fn inspect_and_capture_content_identity_rejects_a_removed_xex() {
    let fixture = Fixture::new("recheck-content-missing");
    let path = fixture.path("games/default.xex");
    let error = inspect_and_capture_content_identity(&path).unwrap_err();
    assert_eq!(error.kind, XeniaLaunchPreflightErrorKind::ContentNotFound);
}

// --- spawn: mechanics proven against a hand-built command, never a discovered one -------------------

fn hand_built_command(executable: PathBuf, content_path: PathBuf) -> XeniaCommand {
    XeniaCommand {
        executable,
        arguments: vec![content_path.clone().into_os_string()],
        working_directory: None,
        selection: crate::launch::xenia_command::XeniaCommandSelection {
            profile_id: "test".to_string(),
            platform_id: "Xbox360".to_string(),
            verified_xex_title_id: Some(XEX_TITLE_ID.to_string()),
            verified_xex_media_id: Some(XEX_MEDIA_ID.to_string()),
            content_path,
        },
    }
}

fn wait_for_exit(process: &mut LaunchedXeniaProcess) -> &ProcessExitReport {
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
        "xenia_canary",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let content_path = fixture.write("games/default.xex", b"synthetic content");
    let command = hand_built_command(executable, content_path.clone());
    let mut process = spawn_xenia(command).expect("the fake script must spawn");
    wait_for_exit(&mut process);
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines, vec![content_path.to_str().unwrap()]);
}

#[test]
fn successful_spawn_returns_a_real_pid_and_clean_exit() {
    let fixture = Fixture::new("spawn-success");
    let executable = fixture.write_executable("xenia_canary", b"#!/bin/sh\nexit 0\n");
    let content_path = fixture.write("games/default.xex", b"synthetic content");
    let command = hand_built_command(executable, content_path);
    let mut process = spawn_xenia(command).expect("the fake script must spawn");
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
        "xenia_canary",
        b"#!/bin/sh\necho 'synthetic xenia failure' 1>&2\nexit 7\n",
    );
    let content_path = fixture.write("games/default.xex", b"synthetic content");
    let command = hand_built_command(executable, content_path);
    let mut process = spawn_xenia(command).unwrap();
    let report = wait_for_exit(&mut process);
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic xenia failure"));
}

#[test]
fn spaces_and_unicode_paths_are_passed_intact() {
    let fixture = Fixture::new("spaces-unicode");
    let capture_path = fixture.path("argv-capture.txt");
    let executable = fixture.write_executable(
        "xenia_canary",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let unicode_name = "games/Halo 3 こんにちは 日本語, with spaces.xex";
    let content_path = fixture.write(unicode_name, b"synthetic content");
    let command = hand_built_command(executable, content_path.clone());
    let mut process = spawn_xenia(command).unwrap();
    wait_for_exit(&mut process);
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines, vec![content_path.to_str().unwrap()]);
}

#[test]
fn shell_metacharacters_in_content_path_are_never_interpreted() {
    let marker = std::env::temp_dir().join(format!(
        "archivefs-xenia-execution-shell-marker-{}",
        std::process::id()
    ));
    let _ = fs::remove_file(&marker);
    let fixture = Fixture::new("shell-metacharacters");
    let executable = fixture.write_executable("xenia_canary", b"#!/bin/sh\nexit 0\n");
    let dangerous_name = format!("games/game; touch {} #.xex", marker.display());
    let dangerous_path = fixture.write(&dangerous_name, b"synthetic content");
    let command = hand_built_command(executable, dangerous_path);
    let mut process = spawn_xenia(command).unwrap();
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
        fixture.path("does-not-exist"),
        fixture.path("games/default.xex"),
    );
    let result = spawn_xenia(command);
    assert!(matches!(result, Err(XeniaLaunchSpawnError::Spawn(_))));
}
