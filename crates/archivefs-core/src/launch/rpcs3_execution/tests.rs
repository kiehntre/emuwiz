//! Tests for native RPCS3 launch preflight/execution.
//!
//! # Why executable-drift/binding-success is unit-tested, not full end-to-end
//!
//! [`crate::patch_manager::resolve_rpcs3_native_launch_binding`] only ever
//! authorizes an executable candidate whose
//! [`crate::patch_manager::Rpcs3InstallationType`] is `Native` - and,
//! exactly like [`crate::launch::xemu_execution`]'s,
//! `crate::launch::ppsspp_execution`'s, and `duckstation_execution`'s own
//! executable discovery, `discover_rpcs3_profiles` only ever classifies an
//! executable `Native` when it is found by literally searching the current
//! process's real `PATH` (`roots.explicit_executables` is deliberately
//! classified `Explicit`, a different, unsupported installation type - see
//! `discover_rpcs3_profiles`'s own executable-discovery logic). Mutating
//! this test binary's real, process-global `PATH` at runtime to fabricate a
//! `Native` match would race every other concurrently running test in this
//! same binary that also reads `PATH` - `std::env::set_var` is `unsafe` for
//! exactly this reason. Exactly the same limitation is already accepted,
//! unchanged, in the xemu/PPSSPP/DuckStation test suites: spawn mechanics
//! are proven against a hand-built command, never a full preflight-derived
//! one, and no preflight test in any of those suites ever exercises a
//! genuine binding success either.
//!
//! So here: every content/identity/profile-lookup preflight step that fires
//! *before* binding resolution (steps 1-6) is proven through the real, full
//! [`preflight_rpcs3_launch`] pipeline, on real synthetic PS3 content (both
//! a direct ISO9660 disc image and an extracted `PS3_GAME` folder, each with
//! a real `PARAM.SFO` `TITLE_ID` and a `USRDIR/EBOOT.BIN` SELF header) via
//! the exact same `inspect_catalogued_game_identity` and
//! `discover_rpcs3_profiles` production code uses. The final pre-spawn
//! recheck (step 10) is already covered as pure units -
//! `recheck_executable`/`inspect_and_capture_content_identity` directly
//! here - so no coverage is lost, only the single PATH-dependent
//! "genuinely Native-installed RPCS3" leg is done through units instead of
//! one shared, actually-installed fixture. Spawn mechanics themselves never
//! depend on any of this: they are proven directly against a hand-built
//! [`Rpcs3Command`], mirroring `xemu_execution::tests::hand_built_command`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::patch_manager::Rpcs3ProfileDiscoveryRoots;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-rpcs3-execution-{label}-{}-{sequence}",
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

/// A minimal, structurally valid `PARAM.SFO` carrying one `TITLE_ID` value -
/// mirrors `game_identity`'s own private `ps3_sfo` test helper exactly,
/// since that helper is not itself exported.
fn ps3_sfo(title_id: &str) -> Vec<u8> {
    let key = b"TITLE_ID\0";
    let mut value = title_id.as_bytes().to_vec();
    value.push(0);
    let key_start = 36_u32;
    let data_start = key_start + key.len() as u32;
    let mut bytes = vec![0_u8; data_start as usize + value.len()];
    bytes[..4].copy_from_slice(crate::param_sfo::SFO_MAGIC);
    bytes[4..8].copy_from_slice(&0x0001_0100_u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&key_start.to_le_bytes());
    bytes[12..16].copy_from_slice(&data_start.to_le_bytes());
    bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
    bytes[20..22].copy_from_slice(&0_u16.to_le_bytes());
    bytes[22..24].copy_from_slice(&0x0204_u16.to_le_bytes());
    bytes[24..28].copy_from_slice(&(value.len() as u32).to_le_bytes());
    bytes[28..32].copy_from_slice(&(value.len() as u32).to_le_bytes());
    bytes[32..36].copy_from_slice(&0_u32.to_le_bytes());
    bytes[key_start as usize..data_start as usize].copy_from_slice(key);
    bytes[data_start as usize..].copy_from_slice(&value);
    bytes
}

/// A real, minimal `PS3_GAME/PARAM.SFO` + `USRDIR/EBOOT.BIN` (SELF magic)
/// extracted-folder layout - mirrors `game_identity`'s own private
/// `ps3_folder` test helper exactly, since that helper is not itself
/// exported.
fn ps3_folder(root: &std::path::Path, title_id: &str, self_magic: bool) {
    let game = root.join("PS3_GAME");
    fs::create_dir_all(game.join("USRDIR")).unwrap();
    fs::write(game.join("PARAM.SFO"), ps3_sfo(title_id)).unwrap();
    let eboot: &[u8] = if self_magic {
        b"SCE\0valid"
    } else {
        b"not-self"
    };
    fs::write(game.join("USRDIR").join("EBOOT.BIN"), eboot).unwrap();
}

/// A minimal, structurally valid PS3 ISO9660 disc image: a primary volume
/// descriptor, a volume descriptor set terminator, a `PS3_GAME` directory
/// holding `PARAM.SFO` (carrying `title_id`) and `USRDIR/EBOOT.BIN` (a real
/// SELF header) - mirrors `game_identity`'s own private `ps3_iso` test
/// helper exactly, since that helper is not itself exported.
fn ps3_iso_bytes(title_id: &str) -> Vec<u8> {
    let mut iso = vec![0_u8; 25 * ISO_SECTOR_SIZE];
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
    let game = directory_record(b"PS3_GAME", 21, ISO_SECTOR_SIZE as u32, true);
    iso[root_offset..root_offset + game.len()].copy_from_slice(&game);

    let game_offset = 21 * ISO_SECTOR_SIZE;
    let dot = directory_record(&[0], 21, ISO_SECTOR_SIZE as u32, true);
    let parent = directory_record(&[1], 20, ISO_SECTOR_SIZE as u32, true);
    let sfo = directory_record(b"PARAM.SFO", 22, ps3_sfo(title_id).len() as u32, false);
    let usrdir = directory_record(b"USRDIR", 23, ISO_SECTOR_SIZE as u32, true);
    let mut cursor = game_offset;
    for record in [&dot, &parent, &sfo, &usrdir] {
        iso[cursor..cursor + record.len()].copy_from_slice(record);
        cursor += record.len();
    }
    let usrdir_offset = 23 * ISO_SECTOR_SIZE;
    iso[usrdir_offset..usrdir_offset + dot.len()].copy_from_slice(&dot);
    iso[usrdir_offset + dot.len()..usrdir_offset + dot.len() + parent.len()]
        .copy_from_slice(&parent);
    let eboot = directory_record(b"EBOOT.BIN", 24, 9, false);
    let eboot_start = usrdir_offset + dot.len() + parent.len();
    iso[eboot_start..eboot_start + eboot.len()].copy_from_slice(&eboot);
    let sfo_bytes = ps3_sfo(title_id);
    iso[22 * ISO_SECTOR_SIZE..22 * ISO_SECTOR_SIZE + sfo_bytes.len()].copy_from_slice(&sfo_bytes);
    iso[24 * ISO_SECTOR_SIZE..24 * ISO_SECTOR_SIZE + 9].copy_from_slice(b"SCE\0valid");
    iso
}

const PS3_TITLE_ID: &str = "BLUS30000";

fn base_roots(fixture: &Fixture) -> Rpcs3ProfileDiscoveryRoots {
    Rpcs3ProfileDiscoveryRoots {
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

/// A profile-discoverable (but never `Native`-executable-bound, see the
/// module doc comment) native RPCS3 fixture: a real `config.yml` evidence
/// file, a fake `explicit` executable (never authorized by
/// `resolve_rpcs3_native_launch_binding`, which is exactly what the
/// early-preflight-step tests below rely on to reach `BindingUnavailable`
/// only after every earlier check has already passed), and either a direct
/// PS3 ISO or an extracted PS3 folder whose verified TITLE_ID becomes the
/// request's expected title id/game key - computed via the same
/// `inspect_catalogued_game_identity` the module itself uses, never
/// hand-typed.
struct ReadyFixture {
    fixture: Fixture,
    roots: Rpcs3ProfileDiscoveryRoots,
    profile_root: PathBuf,
    request: Rpcs3LaunchRequest,
}

fn build_ready_fixture(
    label: &str,
    make_content: impl FnOnce(&Fixture) -> PathBuf,
) -> ReadyFixture {
    let fixture = Fixture::new(label);
    let mut roots = base_roots(&fixture);
    let profile_root = roots.xdg_config_home.join("rpcs3");
    fs::create_dir_all(&profile_root).unwrap();
    fs::write(profile_root.join("config.yml"), b"---\n").unwrap();
    let executable = fixture.write_executable("bin/rpcs3", b"#!/bin/sh\nexit 0\n");
    roots.explicit_executables.push(executable.clone());
    let content_path = make_content(&fixture);

    let discovery = discover_rpcs3_profiles(&roots);
    let profile_id = discovery
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == profile_root)
        .expect("fixture profile must be discovered")
        .profile_id
        .clone();

    let request = Rpcs3LaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "PS3".to_string(),
        expected_game_key: PS3_TITLE_ID.to_string(),
        expected_ps3_title_id: PS3_TITLE_ID.to_string(),
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

fn build_ready_iso_fixture(label: &str) -> ReadyFixture {
    build_ready_fixture(label, |fixture| {
        fixture.write("games/game.iso", &ps3_iso_bytes(PS3_TITLE_ID))
    })
}

fn build_ready_folder_fixture(label: &str) -> ReadyFixture {
    build_ready_fixture(label, |fixture| {
        let content = fixture.path("games/BLUS30000");
        ps3_folder(&content, PS3_TITLE_ID, true);
        content
    })
}

fn preflight(ready: &ReadyFixture) -> Result<Rpcs3Command, Rpcs3LaunchPreflightError> {
    preflight_rpcs3_launch(&ready.request, &ready.roots)
}

// --- content path checks (steps 1-3), direct ISO -----------------------------------------------

#[test]
fn content_identity_capture_detects_a_swapped_iso() {
    let ready = build_ready_iso_fixture("content-changed");
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
fn iso_removed_is_rejected() {
    let ready = build_ready_iso_fixture("iso-removed");
    fs::remove_file(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::ContentNotFound);
}

#[test]
fn iso_replaced_by_a_directory_is_rejected() {
    let ready = build_ready_iso_fixture("iso-replaced-with-dir");
    fs::remove_file(&ready.request.selected_content_path).unwrap();
    fs::create_dir_all(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::ContentNotRegularFile
    );
}

#[test]
fn iso_symlink_content_is_rejected() {
    let ready = build_ready_iso_fixture("symlink-content");
    let link = ready.fixture.path("games/link.iso");
    symlink(&ready.request.selected_content_path, &link).unwrap();
    let mut request = ready.request.clone();
    request.selected_content_path = link;
    let error = preflight_rpcs3_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::ContentIsSymlink);
}

#[test]
fn unsupported_pkg_remains_refused() {
    let ready = build_ready_iso_fixture("unsupported-pkg");
    let pkg = ready
        .fixture
        .write("games/game.pkg", &ps3_iso_bytes(PS3_TITLE_ID));
    let mut request = ready.request.clone();
    request.selected_content_path = pkg;
    let error = preflight_rpcs3_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::ContentFormatUnsupported
    );
}

#[test]
fn mount_input_archive_content_is_rejected() {
    let ready = build_ready_iso_fixture("archive-content");
    let zip = ready.fixture.write("games/game.zip", b"pk not a real zip");
    let mut request = ready.request.clone();
    request.selected_content_path = zip;
    let error = preflight_rpcs3_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::ContentFormatUnsupported
    );
}

// --- content path checks, extracted folder ------------------------------------------------------

#[test]
fn extracted_folder_preflight_reaches_binding_resolution() {
    let ready = build_ready_folder_fixture("folder-ready");
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn extracted_folder_removed_is_rejected() {
    let ready = build_ready_folder_fixture("folder-removed");
    fs::remove_dir_all(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::ContentNotFound);
}

#[test]
fn extracted_folder_replaced_by_a_file_is_rejected() {
    let ready = build_ready_folder_fixture("folder-replaced-with-file");
    fs::remove_dir_all(&ready.request.selected_content_path).unwrap();
    fs::write(&ready.request.selected_content_path, b"not a folder").unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::ContentNotRegularFile
    );
}

#[test]
fn param_sfo_removed_is_rejected() {
    // No PARAM.SFO at all means fresh identity re-inspection cannot resolve
    // any identity, not merely a missing TITLE_ID fact - surfaces as
    // `IdentityUnresolved`.
    let ready = build_ready_folder_fixture("param-sfo-removed");
    fs::remove_file(
        ready
            .request
            .selected_content_path
            .join("PS3_GAME/PARAM.SFO"),
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::IdentityUnresolved
    );
}

#[test]
fn param_sfo_title_id_changed_is_rejected() {
    let ready = build_ready_folder_fixture("param-sfo-changed");
    fs::write(
        ready
            .request
            .selected_content_path
            .join("PS3_GAME/PARAM.SFO"),
        ps3_sfo("BLUS99999"),
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::IdentityMismatch);
}

#[test]
fn eboot_invalid_self_magic_is_rejected() {
    // A structurally invalid EBOOT.BIN means the existing PS3 identity
    // layer refuses to resolve any identity at all for this content (it
    // requires PARAM.SFO TITLE_ID *and* a valid SELF EBOOT together) -
    // surfaces as `IdentityUnresolved`, not a title-ID-specific error.
    let ready = build_ready_folder_fixture("eboot-invalid-self");
    fs::write(
        ready
            .request
            .selected_content_path
            .join("PS3_GAME/USRDIR/EBOOT.BIN"),
        b"not-self",
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::IdentityUnresolved
    );
}

// --- identity checks (step 4) --------------------------------------------------------------------

#[test]
fn wrong_expected_game_key_is_rejected() {
    let mut ready = build_ready_iso_fixture("wrong-game-key");
    ready.request.expected_game_key = "BLUS99999".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::IdentityMismatch);
}

#[test]
fn wrong_expected_ps3_title_id_is_rejected() {
    let mut ready = build_ready_iso_fixture("wrong-title-id");
    ready.request.expected_ps3_title_id = "BLUS99999".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::Ps3TitleIdMismatch
    );
}

#[test]
fn wrong_platform_expectation_never_authorizes_a_native_ps3_disc() {
    let mut ready = build_ready_iso_fixture("wrong-platform");
    ready.request.expected_platform_id = "PS2".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::IdentityMismatch);
}

// --- profile lookup checks (steps 5-6) -------------------------------------------------------------

#[test]
fn profile_directory_removed_is_rejected() {
    let ready = build_ready_iso_fixture("profile-removed");
    fs::remove_dir_all(&ready.profile_root).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::ProfileNotFound);
}

#[test]
fn profile_root_drift_is_rejected() {
    let ready = build_ready_iso_fixture("root-drift");
    let mut request = ready.request.clone();
    request.profile_id = format!(
        "rpcs3:{}",
        ready.fixture.path("stale-config/rpcs3").display()
    );
    let error = preflight_rpcs3_launch(&request, &ready.roots).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::ProfileNotFound);
}

// --- every earlier check passing reaches binding resolution, never skips it ------------------------

#[test]
fn a_fully_valid_iso_request_reaches_binding_resolution() {
    let ready = build_ready_iso_fixture("reaches-binding");
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- final pre-spawn recheck units (step 10) --------------------------------------------------------

#[test]
fn recheck_executable_rejects_a_missing_executable() {
    let fixture = Fixture::new("recheck-missing");
    let path = fixture.path("bin/rpcs3");
    let error = recheck_executable(&path).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::ExecutableMissing);
}

#[test]
fn recheck_executable_rejects_a_symlink() {
    let fixture = Fixture::new("recheck-symlink");
    let real = fixture.write_executable("bin/rpcs3-real", b"#!/bin/sh\nexit 0\n");
    let link = fixture.path("bin/rpcs3");
    symlink(&real, &link).unwrap();
    let error = recheck_executable(&link).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::ExecutableUnsafe);
}

#[test]
fn recheck_executable_rejects_a_non_executable_file() {
    let fixture = Fixture::new("recheck-not-executable");
    let path = fixture.write("bin/rpcs3", b"#!/bin/sh\nexit 0\n");
    let error = recheck_executable(&path).unwrap_err();
    assert_eq!(
        error.kind,
        Rpcs3LaunchPreflightErrorKind::ExecutableNotExecutable
    );
}

#[test]
fn recheck_executable_accepts_a_real_executable() {
    let fixture = Fixture::new("recheck-ok");
    let path = fixture.write_executable("bin/rpcs3", b"#!/bin/sh\nexit 0\n");
    recheck_executable(&path).expect("a real, executable regular file must pass");
}

#[test]
fn inspect_and_capture_content_identity_rejects_a_removed_iso() {
    let fixture = Fixture::new("recheck-content-missing");
    let path = fixture.path("games/game.iso");
    let error = inspect_and_capture_content_identity(&path).unwrap_err();
    assert_eq!(error.kind, Rpcs3LaunchPreflightErrorKind::ContentNotFound);
}

#[test]
fn inspect_and_capture_content_identity_accepts_a_real_folder() {
    let fixture = Fixture::new("recheck-folder-ok");
    let path = fixture.path("games/BLUS30000");
    ps3_folder(&path, PS3_TITLE_ID, true);
    inspect_and_capture_content_identity(&path)
        .expect("a real extracted PS3 folder must pass the shape check");
}

// --- spawn: mechanics proven against a hand-built command, never a discovered one ------------------

fn hand_built_command(executable: PathBuf, content_path: PathBuf) -> Rpcs3Command {
    Rpcs3Command {
        executable,
        arguments: vec![content_path.clone().into_os_string()],
        working_directory: None,
        selection: crate::launch::rpcs3_command::Rpcs3CommandSelection {
            profile_id: "test".to_string(),
            platform_id: "PS3".to_string(),
            verified_ps3_title_id: PS3_TITLE_ID.to_string(),
            content_path,
        },
    }
}

fn wait_for_exit(process: &mut LaunchedRpcs3Process) -> &ProcessExitReport {
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
        "bin/rpcs3",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let content_path = fixture.write("games/game.iso", b"synthetic content");
    let command = hand_built_command(executable, content_path.clone());
    let mut process = spawn_rpcs3(command).expect("the fake script must spawn");
    wait_for_exit(&mut process);
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines, vec![content_path.to_str().unwrap()]);
}

#[test]
fn successful_spawn_returns_a_real_pid_and_clean_exit() {
    let fixture = Fixture::new("spawn-success");
    let executable = fixture.write_executable("bin/rpcs3", b"#!/bin/sh\nexit 0\n");
    let content_path = fixture.write("games/game.iso", b"synthetic content");
    let command = hand_built_command(executable, content_path);
    let mut process = spawn_rpcs3(command).expect("the fake script must spawn");
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
        "bin/rpcs3",
        b"#!/bin/sh\necho 'synthetic rpcs3 failure' 1>&2\nexit 7\n",
    );
    let content_path = fixture.write("games/game.iso", b"synthetic content");
    let command = hand_built_command(executable, content_path);
    let mut process = spawn_rpcs3(command).unwrap();
    let report = wait_for_exit(&mut process);
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic rpcs3 failure"));
}

#[test]
fn spaces_and_unicode_paths_are_passed_intact() {
    let fixture = Fixture::new("spaces-unicode");
    let capture_path = fixture.path("argv-capture.txt");
    let executable = fixture.write_executable(
        "bin/rpcs3",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let unicode_name = "games/Gran Turismo こんにちは 日本語, with spaces.iso";
    let content_path = fixture.write(unicode_name, b"synthetic content");
    let command = hand_built_command(executable, content_path.clone());
    let mut process = spawn_rpcs3(command).unwrap();
    wait_for_exit(&mut process);
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines, vec![content_path.to_str().unwrap()]);
}

#[test]
fn shell_metacharacters_in_content_path_are_never_interpreted() {
    let marker = std::env::temp_dir().join(format!(
        "archivefs-rpcs3-execution-shell-marker-{}",
        std::process::id()
    ));
    let _ = fs::remove_file(&marker);
    let fixture = Fixture::new("shell-metacharacters");
    let executable = fixture.write_executable("bin/rpcs3", b"#!/bin/sh\nexit 0\n");
    let dangerous_name = format!("games/game; touch {} #.iso", marker.display());
    let dangerous_path = fixture.write(&dangerous_name, b"synthetic content");
    let command = hand_built_command(executable, dangerous_path);
    let mut process = spawn_rpcs3(command).unwrap();
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
    let result = spawn_rpcs3(command);
    assert!(matches!(result, Err(Rpcs3LaunchSpawnError::Spawn(_))));
}
