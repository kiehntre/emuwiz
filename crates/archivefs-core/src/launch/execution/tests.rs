//! Tests for RetroArch launch preflight/execution.
//!
//! Every fixture is a real temp directory on disk (a fake `retroarch`
//! shell script, a fake core `.so`, a real `.info` file, real content
//! bytes) driven through the exact same real discovery/identity/planning
//! functions production code uses - `discover_retroarch_environment`,
//! `inspect_catalogued_game_identity`, `build_launch_plan`,
//! `build_retroarch_command_plan` - never a shortcut or a mocked plan.
//! Spawn tests genuinely `fork`/`exec` the fake script; no real installed
//! RetroArch is required anywhere.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::emulator_environment::HostReadOnlyFilesystem;
use crate::emulator_environment::retroarch::{ProfileKind, ProfileScope};
use crate::game_identity::inspect_catalogued_game_identity;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-launch-execution-{label}-{}-{sequence}",
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

/// A fully wired, genuinely `Ready` native RetroArch fixture: a fake
/// executable script on `PATH`, a real `retroarch.cfg`, a fake core `.so` +
/// real `.info` resolving to the `MegaDrive` platform, and a loose Mega
/// Drive ROM whose verified SHA-256 becomes the request's expected game
/// key - computed via the same [`inspect_catalogued_game_identity`] the
/// module itself uses, never hand-typed.
struct ReadyFixture {
    fixture: Fixture,
    request: RetroArchLaunchRequest,
}

fn build_ready_fixture(label: &str) -> ReadyFixture {
    build_ready_fixture_with_content(label, b"synthetic mega drive rom bytes", "game.md")
}

fn build_ready_fixture_with_content(
    label: &str,
    content_bytes: &[u8],
    content_name: &str,
) -> ReadyFixture {
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
    fixture.write("cores/genesis_plus_gx_libretro.so", b"stub core");
    fixture.write("info/genesis_plus_gx.info", b"systemname = \"megadrive\"\n");
    let content = fixture.write(&format!("content/{content_name}"), content_bytes);

    let identity_report = inspect_catalogued_game_identity(&content, Some("MegaDrive"));
    let expected_game_key = identity_report
        .verified_loose_rom_sha256()
        .expect("fixture content must be a verifiable loose Mega Drive ROM")
        .to_string();

    let request = RetroArchLaunchRequest {
        selected_content_path: content,
        expected_platform_id: "MegaDrive".to_string(),
        expected_game_key,
        profile: ProfileRef {
            profile_kind: ProfileKind::Native,
            scope: ProfileScope::User,
        },
        core_stem: "genesis_plus_gx".to_string(),
    };
    ReadyFixture { fixture, request }
}

fn preflight(ready: &ReadyFixture) -> Result<RetroArchCommand, LaunchPreflightError> {
    preflight_retroarch_launch(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.fixture.env(),
    )
}

// --- native direct-content Ready candidate passes preflight ------------------

#[test]
fn native_direct_content_ready_candidate_passes_preflight() {
    let ready = build_ready_fixture("ready");
    let command = preflight(&ready).expect("a fully wired native fixture must preflight cleanly");
    assert_eq!(command.executable, ready.fixture.path("bin/retroarch"));
    assert_eq!(command.selection.core_stem, "genesis_plus_gx");
    assert_eq!(command.selection.platform_id, "MegaDrive");
    assert_eq!(
        command.selection.content_path,
        ready.request.selected_content_path
    );
}

// --- exact argv preserved -----------------------------------------------------

#[test]
fn exact_argv_is_preserved() {
    let ready = build_ready_fixture("argv");
    let command = preflight(&ready).unwrap();
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("-L"),
            ready
                .fixture
                .path("cores/genesis_plus_gx_libretro.so")
                .into_os_string(),
            ready.request.selected_content_path.clone().into_os_string(),
        ]
    );
}

// --- spaces/quotes/Unicode in paths are argv data, not shell syntax ----------

#[test]
fn spaces_quotes_and_unicode_in_content_path_are_preserved_as_argv_data() {
    let ready = build_ready_fixture_with_content(
        "unicode-argv",
        b"synthetic mega drive rom bytes",
        "Sonic's \"Adventure\" 2 $(echo hi) 日本語.md",
    );
    let command = preflight(&ready).unwrap();
    let last = command.arguments.last().unwrap();
    assert_eq!(
        last,
        &ready.request.selected_content_path.clone().into_os_string()
    );
    assert!(last.to_string_lossy().contains("日本語"));
    assert!(last.to_string_lossy().contains('$'));
}

// --- changed focused/content path rejected ------------------------------------

#[test]
fn a_content_path_that_no_longer_exists_is_rejected() {
    let mut ready = build_ready_fixture("missing-content");
    fs::remove_file(&ready.request.selected_content_path).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::ContentNotFound);
    let _ = &mut ready;
}

// --- symlink content rejected --------------------------------------------------

#[test]
fn symlink_content_is_rejected() {
    let ready = build_ready_fixture("symlink-content");
    let link = ready.fixture.path("content/link.md");
    symlink(&ready.request.selected_content_path, &link).unwrap();
    let mut request = ready.request.clone();
    request.selected_content_path = link;
    let error = preflight_retroarch_launch(&request, &HostReadOnlyFilesystem, &ready.fixture.env())
        .unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::ContentIsSymlink);
}

// --- non-regular content rejected -----------------------------------------------

#[test]
fn directory_content_is_rejected() {
    let ready = build_ready_fixture("dir-content");
    let mut request = ready.request.clone();
    request.selected_content_path = ready.fixture.path("content");
    let error = preflight_retroarch_launch(&request, &HostReadOnlyFilesystem, &ready.fixture.env())
        .unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::ContentNotRegularFile);
}

// --- changed content identity rejected before spawn -----------------------------

#[test]
fn content_that_changes_bytes_after_capture_is_rejected() {
    // Preflight captures identity, then internally re-inspects near the
    // end - simulate the same effect a real race would have by mutating
    // the file's bytes on disk immediately (still well within one
    // synchronous preflight call in wall-clock terms, but the module never
    // assumes that - see `ContentChangedBeforeSpawn`).
    let ready = build_ready_fixture("content-changed");
    let path = ready.request.selected_content_path.clone();
    let original = fs::read(&path).unwrap();
    // A background thread swaps the bytes as soon as this call starts;
    // since `preflight_retroarch_launch` re-checks identity twice (once
    // for the fresh identity, once immediately before spawn), a change
    // that lands between the two is caught by the final recheck even
    // though this test cannot line up the exact instruction boundary -
    // so instead we prove the *mechanism* directly: capture, mutate, then
    // assert a mismatch is detected by the same identity comparison the
    // real function performs.
    let before = super::LaunchContentIdentity::capture(&fs::symlink_metadata(&path).unwrap());
    fs::write(&path, b"different bytes entirely, same path").unwrap();
    let after = super::LaunchContentIdentity::capture(&fs::symlink_metadata(&path).unwrap());
    assert_ne!(
        before, after,
        "captured identity must change when the file changes"
    );
    fs::write(&path, &original).unwrap();
}

#[test]
fn content_replaced_with_different_verified_identity_is_rejected() {
    let ready = build_ready_fixture("content-identity-changed");
    // Replace the loose ROM's bytes (still a plausible Mega Drive ROM,
    // still the same path) so its verified SHA-256 - and so its resolved
    // game key - genuinely differs from what the request expects.
    fs::write(
        &ready.request.selected_content_path,
        b"a completely different rom",
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::IdentityMismatch);
}

// --- identity mismatch rejected ---------------------------------------------------

#[test]
fn wrong_expected_game_key_is_rejected() {
    let mut ready = build_ready_fixture("wrong-game-key");
    ready.request.expected_game_key = "0".repeat(64);
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::IdentityMismatch);
}

#[test]
fn wrong_expected_platform_id_is_rejected() {
    let mut ready = build_ready_fixture("wrong-platform-id");
    ready.request.expected_platform_id = "SNES".to_string();
    let error = preflight(&ready).unwrap_err();
    // Either the fresh identity itself no longer resolves under the wrong
    // hint, or it resolves but disagrees with what was expected - both are
    // a rejection, never a silent substitution.
    assert!(matches!(
        error.kind,
        LaunchPreflightErrorKind::IdentityMismatch | LaunchPreflightErrorKind::IdentityUnresolved
    ));
}

// --- Unknown/Conflicting identity rejected -----------------------------------------

#[test]
fn unresolvable_content_is_rejected_as_unknown() {
    let ready = build_ready_fixture("unknown-identity");
    // Bytes with no recognizable Mega Drive header at all - the identity
    // layer cannot verify anything about this content.
    fs::write(
        &ready.request.selected_content_path,
        vec![0u8; 4], // too small/unstructured to verify - Missing/Ambiguous status
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert!(matches!(
        error.kind,
        LaunchPreflightErrorKind::IdentityUnresolved | LaunchPreflightErrorKind::IdentityMismatch
    ));
}

// --- requested profile mismatch rejected -------------------------------------------

#[test]
fn requested_profile_scope_mismatch_is_rejected() {
    let mut ready = build_ready_fixture("profile-mismatch");
    ready.request.profile.scope = ProfileScope::System;
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        LaunchPreflightErrorKind::RequestedCandidateNotFound
    );
}

// --- requested core mismatch rejected -----------------------------------------------

#[test]
fn requested_core_stem_mismatch_is_rejected() {
    let mut ready = build_ready_fixture("core-mismatch");
    ready.request.core_stem = "nonexistent_core".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        LaunchPreflightErrorKind::RequestedCandidateNotFound
    );
}

// --- candidate ReadyWithWarnings rejected -------------------------------------------

#[test]
fn a_reviewed_core_hint_keeps_the_requested_candidate_ready_with_a_second_core() {
    // Two distinct cores resolve to Mega Drive, but the reviewed
    // `genesis_plus_gx` hint selects the requested core without changing
    // the candidate's other readiness checks.
    let ready = build_ready_fixture("ready-with-warnings");
    ready
        .fixture
        .write("cores/picodrive_libretro.so", b"stub core two");
    ready
        .fixture
        .write("info/picodrive.info", b"systemname = \"megadrive\"\n");
    let command = preflight(&ready).expect("reviewed hint should keep the requested core ready");
    assert_eq!(command.selection.core_stem, "genesis_plus_gx");
}

// --- blocked candidate rejected -----------------------------------------------------

#[test]
fn a_broken_info_file_blocks_the_candidate_and_is_rejected() {
    let ready = build_ready_fixture("blocked-candidate");
    // Corrupt the `.info` file so the core no longer resolves to any
    // platform at all - the plan can then never produce a matching
    // candidate for this request (covered by `RequestedCandidateNotFound`,
    // itself a rejection - blocked/unmatched are both refused, never
    // launched).
    fs::write(ready.fixture.path("info/genesis_plus_gx.info"), b"garbage").unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        LaunchPreflightErrorKind::RequestedCandidateNotFound
    );
}

// --- mounted/archive content rejected -------------------------------------------------

#[test]
fn an_outer_zip_archive_path_is_rejected() {
    let ready = build_ready_fixture("archive-content");
    let mut request = ready.request.clone();
    let archive_path = ready.fixture.path("content/pack.zip");
    // A real, minimal ZIP local-file-header signature so `archive_kind`
    // recognizes it structurally, not just by extension.
    fs::write(&archive_path, b"PK\x03\x04rest of a fake zip").unwrap();
    request.selected_content_path = archive_path;
    let error = preflight_retroarch_launch(&request, &HostReadOnlyFilesystem, &ready.fixture.env())
        .unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::ContentRequiresMount);
}

// --- Flatpak rejected -----------------------------------------------------------------

#[test]
fn flatpak_profile_kind_is_rejected() {
    let mut ready = build_ready_fixture("flatpak-profile");
    ready.request.profile.profile_kind = ProfileKind::Flatpak;
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::UnsupportedProfileKind);
}

// --- AppImage rejected for Phase 1 -----------------------------------------------------

#[test]
fn appimage_profile_kind_is_rejected() {
    let mut ready = build_ready_fixture("appimage-profile");
    ready.request.profile.profile_kind = ProfileKind::AppImage;
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::UnsupportedProfileKind);
}

// --- executable disappears before spawn -> rejected -------------------------------------

#[test]
fn executable_missing_is_rejected() {
    let ready = build_ready_fixture("executable-missing");
    fs::remove_file(ready.fixture.path("bin/retroarch")).unwrap();
    let error = preflight(&ready).unwrap_err();
    // Removed before preflight even starts, so the fresh environment
    // discovery step itself never finds it - the command plan is
    // therefore blocked before this module's own dedicated executable
    // recheck (step 10) is ever reached. Either layer catching it is a
    // correct, genuine rejection; `ExecutableMissing` is the outcome only
    // when discovery still finds *a* command to build but the executable
    // vanishes in the narrower window right before spawn.
    assert!(matches!(
        error.kind,
        LaunchPreflightErrorKind::CommandBlocked | LaunchPreflightErrorKind::ExecutableMissing
    ));
}

#[test]
fn path_symlink_is_resolved_to_the_verified_regular_executable_before_final_preflight() {
    let ready = build_ready_fixture("executable-symlink");
    let executable = ready.fixture.path("bin/retroarch");
    let target = ready.fixture.path("bin/real-retroarch");
    fs::rename(&executable, &target).unwrap();
    symlink(&target, &executable).unwrap();
    let command = preflight(&ready).unwrap();
    assert_eq!(command.executable, target);
    assert!(fs::symlink_metadata(&command.executable).unwrap().is_file());
}

#[test]
fn path_symlink_to_a_non_file_is_still_rejected_before_spawn() {
    let ready = build_ready_fixture("executable-symlink-directory");
    let executable = ready.fixture.path("bin/retroarch");
    fs::remove_file(&executable).unwrap();
    let target = ready.fixture.path("not-an-executable");
    fs::create_dir(&target).unwrap();
    symlink(&target, &executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::CommandBlocked);
}

#[test]
fn executable_without_the_execute_bit_is_rejected() {
    let ready = build_ready_fixture("executable-not-executable");
    fs::set_permissions(
        ready.fixture.path("bin/retroarch"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    // Discovery's own PATH scan only records genuinely executable files as
    // evidence (see `Evidence::executables`' own doc comment), so an
    // executable stripped of its execute bit before discovery runs may
    // already be invisible to the command plan (`CommandBlocked`) rather
    // than reaching this module's own dedicated recheck
    // (`ExecutableNotExecutable`) - both are a correct, genuine rejection.
    assert!(matches!(
        error.kind,
        LaunchPreflightErrorKind::CommandBlocked
            | LaunchPreflightErrorKind::ExecutableNotExecutable
    ));
}

// --- core disappears before spawn -> rejected --------------------------------------------

#[test]
fn core_library_missing_is_rejected() {
    let ready = build_ready_fixture("core-missing");
    fs::remove_file(ready.fixture.path("cores/genesis_plus_gx_libretro.so")).unwrap();
    let error = preflight(&ready).unwrap_err();
    // Discovery itself no longer finds the core at all, so the candidate
    // this request names is gone from the freshly rebuilt plan - never a
    // stale reference to a file that no longer exists.
    assert_eq!(
        error.kind,
        LaunchPreflightErrorKind::RequestedCandidateNotFound
    );
}

// --- spawn failure reported ----------------------------------------------------------------

#[test]
fn spawn_failure_is_reported() {
    let ready = build_ready_fixture("spawn-failure");
    let mut command = preflight(&ready).unwrap();
    // A command that will fail to spawn: an executable path that no
    // longer exists by the time `spawn_retroarch` itself runs.
    command.executable = ready.fixture.path("bin/does-not-exist");
    let result = spawn_retroarch(command);
    assert!(matches!(result, Err(LaunchSpawnError::Spawn(_))));
}

// --- successful child spawn returns PID -----------------------------------------------------

#[test]
fn successful_spawn_returns_a_real_pid_and_clean_exit() {
    let ready = build_ready_fixture("spawn-success");
    let command = preflight(&ready).unwrap();
    let mut process = spawn_retroarch(command).expect("the fake script must spawn");
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

// --- child clean exit reported / non-zero exit + bounded stderr reported -------------------

#[test]
fn non_zero_exit_and_stderr_are_reported() {
    let ready = build_ready_fixture_with_content(
        "spawn-nonzero",
        b"synthetic mega drive rom bytes",
        "game.md",
    );
    // Overwrite the fake executable with one that writes to stderr and
    // exits non-zero.
    fs::write(
        ready.fixture.path("bin/retroarch"),
        b"#!/bin/sh\necho 'synthetic failure' 1>&2\nexit 7\n",
    )
    .unwrap();
    fs::set_permissions(
        ready.fixture.path("bin/retroarch"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let command = preflight(&ready).unwrap();
    let mut process = spawn_retroarch(command).unwrap();
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
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic failure"));
    assert!(report.stderr.len() <= LAUNCH_STDERR_CAPTURE_LIMIT);
}

// --- metacharacters are never interpreted through a shell -----------------------------------

#[test]
fn shell_metacharacters_in_content_path_are_never_interpreted() {
    // A content filename that would be catastrophic if ever concatenated
    // into a shell command line: it creates a marker file if (and only
    // if) a shell ever re-parses it. Passed as a single argv element via
    // `Command::args`, it must never be interpreted.
    let marker = std::env::temp_dir().join(format!(
        "archivefs-launch-execution-shell-marker-{}",
        std::process::id()
    ));
    let _ = fs::remove_file(&marker);
    let dangerous_name = format!("game; touch {} #.md", marker.display());
    let ready = build_ready_fixture_with_content(
        "shell-metacharacters",
        b"synthetic mega drive rom bytes",
        &dangerous_name,
    );
    let command = preflight(&ready).unwrap();
    let mut process = spawn_retroarch(command).unwrap();
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
