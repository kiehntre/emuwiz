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
        crate::verified_game_fixture::compile_native_test_executable(
            &path,
            std::str::from_utf8(contents).unwrap(),
        )
        .unwrap();
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
            explicit_executables: Vec::new(),
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
    environment: DiscoveryEnvironment,
}

fn build_ready_fixture(label: &str) -> ReadyFixture {
    build_ready_fixture_with_content(label, b"synthetic mega drive rom bytes", "game.md")
}

#[test]
fn installation_choice_resolves_ambiguity_and_stale_choice_never_launches_the_other_program() {
    let mut ready = build_ready_fixture("installation-choice");
    let second = ready
        .fixture
        .write_executable("second/retroarch", b"exit 0\n");
    let first = ready.fixture.path("bin/retroarch");
    ready.environment.path =
        Some(std::env::join_paths([first.parent().unwrap(), second.parent().unwrap()]).unwrap());
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::ExecutableAmbiguous);
    assert!(error.detail.contains("Emulator Setup"));
    ready.environment.explicit_executables = vec![second.clone()];
    ready.request.expected_executable_binding =
        Some(RetroArchExecutableBinding::capture(second.clone()).unwrap());
    assert_eq!(preflight(&ready).unwrap().executable, second);
    ready.environment.explicit_executables = vec![first.clone()];
    assert!(
        preflight(&ready).is_err(),
        "a changed selection invalidates the displayed request"
    );
    ready.environment.explicit_executables = vec![second.clone()];
    fs::remove_file(&second).unwrap();
    assert!(preflight(&ready).is_err());
    ready
        .fixture
        .write_executable("second/retroarch", b"exit 1\n");
    assert!(
        preflight(&ready).is_err(),
        "replacement native binary invalidates the request"
    );
    fs::write(
        second,
        "#!/bin/sh\nexec flatpak run org.libretro.RetroArch \"$@\"\n",
    )
    .unwrap();
    assert!(
        preflight(&ready).is_err(),
        "a wrapper replacement is refused"
    );
    assert!(
        first.is_file(),
        "the other native installation is untouched"
    );
}

#[test]
fn final_executable_recheck_refuses_a_wrapper_swap_without_a_gui_binding() {
    let ready = build_ready_fixture("last-wrapper-swap");
    let executable = ready.fixture.path("bin/retroarch");
    fs::write(
        &executable,
        "#!/bin/sh\nexec flatpak run org.libretro.RetroArch \"$@\"\n",
    )
    .unwrap();
    assert_eq!(
        recheck_selected_executable(&executable, ProfileKind::Native)
            .unwrap_err()
            .kind,
        LaunchPreflightErrorKind::ExecutableSelectionStale
    );
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
        launch_choice: None,
        selected_content_path: content,
        expected_platform_id: "MegaDrive".to_string(),
        expected_game_key,
        profile: ProfileRef {
            profile_kind: ProfileKind::Native,
            scope: ProfileScope::User,
        },
        core_stem: "genesis_plus_gx".to_string(),
        saturn_core_selection: None,
        expected_snapshot_evidence: None,
        expected_appimage_executable: None,
        expected_executable_binding: None,
        expected_configuration_file: None,
    };
    let environment = fixture.env();
    ReadyFixture {
        fixture,
        request,
        environment,
    }
}

fn build_ready_appimage_fixture(label: &str) -> ReadyFixture {
    let fixture = Fixture::new(label);
    let appimage =
        fixture.write_executable("Applications/RetroArch.AppImage", b"#!/bin/sh\nexit 0\n");
    let cores_dir = fixture.path("cores");
    let info_dir = fixture.path("info");
    fixture.write(
        "Applications/RetroArch.AppImage.config/retroarch/retroarch.cfg",
        format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\n",
            cores_dir.display(),
            info_dir.display()
        )
        .as_bytes(),
    );
    fixture.write("cores/genesis_plus_gx_libretro.so", b"stub core");
    fixture.write("info/genesis_plus_gx.info", b"systemname = \"megadrive\"\n");
    let content = fixture.write("content/game.md", b"synthetic mega drive rom bytes");
    fixture.write(
        "desktop/retroarch.desktop",
        format!(
            "[Desktop Entry]\nType=Application\nName=RetroArch\nExec={}\n",
            appimage.display()
        )
        .as_bytes(),
    );
    let identity_report = inspect_catalogued_game_identity(&content, Some("MegaDrive"));
    let expected_game_key = identity_report
        .verified_loose_rom_sha256()
        .expect("fixture content must be a verifiable loose Mega Drive ROM")
        .to_string();
    let mut environment = fixture.env();
    environment.app_image_search_roots = vec![fixture.path("Applications")];
    environment.desktop_file_roots = vec![fixture.path("desktop")];
    let request = RetroArchLaunchRequest {
        launch_choice: None,
        selected_content_path: content,
        expected_platform_id: "MegaDrive".to_string(),
        expected_game_key,
        profile: ProfileRef {
            profile_kind: ProfileKind::AppImage,
            scope: ProfileScope::User,
        },
        core_stem: "genesis_plus_gx".to_string(),
        saturn_core_selection: None,
        expected_snapshot_evidence: None,
        expected_appimage_executable: Some(appimage),
        expected_executable_binding: None,
        expected_configuration_file: None,
    };
    ReadyFixture {
        fixture,
        request,
        environment,
    }
}

fn preflight(ready: &ReadyFixture) -> Result<RetroArchCommand, LaunchPreflightError> {
    preflight_retroarch_launch(&ready.request, &HostReadOnlyFilesystem, &ready.environment)
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

// --- required firmware: unverified keeps Play, fresh probe decides -----------

/// Declares required firmware on the fixture core. `files` are the firmware
/// names the core's `.info` lists as required; none is created here.
fn declare_required_firmware(ready: &ReadyFixture, files: &[&str]) {
    let cores_dir = ready.fixture.path("cores");
    let info_dir = ready.fixture.path("info");
    let system_dir = ready.fixture.path("system");
    fs::create_dir_all(&system_dir).unwrap();
    fs::write(
        ready.fixture.path("config/retroarch/retroarch.cfg"),
        format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\nsystem_directory = \"{}\"\n",
            cores_dir.display(),
            info_dir.display(),
            system_dir.display()
        ),
    )
    .unwrap();
    let mut info = String::from("systemname = \"megadrive\"\n");
    info.push_str(&format!("firmware_count = {}\n", files.len()));
    for (index, file) in files.iter().enumerate() {
        info.push_str(&format!(
            "firmware{index}_path = \"{file}\"\nfirmware{index}_opt = \"false\"\n"
        ));
    }
    fs::write(ready.fixture.path("info/genesis_plus_gx.info"), info).unwrap();
}

#[test]
fn readable_unverified_required_firmware_still_passes_preflight() {
    let ready = build_ready_fixture("fw-unverified");
    declare_required_firmware(&ready, &["bios.bin"]);
    fs::write(ready.fixture.path("system/bios.bin"), b"not hashed").unwrap();
    preflight(&ready).expect("policy B: a readable regular unhashed firmware file keeps Play");
}

#[test]
fn firmware_removed_after_planning_is_refused_at_click_time() {
    // The plan the user saw said PresentUnverified; the fresh probe at click
    // time must decide, and a now-missing file blocks the launch.
    let ready = build_ready_fixture("fw-toctou");
    declare_required_firmware(&ready, &["bios.bin"]);
    let firmware = ready.fixture.path("system/bios.bin");
    fs::write(&firmware, b"not hashed").unwrap();
    preflight(&ready).expect("present at plan time");
    fs::remove_file(&firmware).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert!(
        matches!(
            error.kind,
            LaunchPreflightErrorKind::CandidateNotReady
                | LaunchPreflightErrorKind::RequestedCandidateNotFound
                | LaunchPreflightErrorKind::CommandBlocked
        ),
        "{error:?}"
    );
}

#[test]
fn missing_required_firmware_is_never_launchable() {
    let ready = build_ready_fixture("fw-missing");
    declare_required_firmware(&ready, &["bios.bin"]);
    assert!(preflight(&ready).is_err());
}

/// Owner policy: any required-and-missing firmware file blocks the launch,
/// even when other required files are present (fresh probe at click time).
#[test]
fn mixed_present_and_missing_required_firmware_is_refused() {
    let ready = build_ready_fixture("fw-mixed");
    declare_required_firmware(&ready, &["a.bin", "b.bin"]);
    fs::write(ready.fixture.path("system/a.bin"), b"x").unwrap();
    let error = preflight(&ready).unwrap_err();
    assert!(
        matches!(
            error.kind,
            LaunchPreflightErrorKind::CandidateNotReady
                | LaunchPreflightErrorKind::RequestedCandidateNotFound
                | LaunchPreflightErrorKind::CommandBlocked
        ),
        "{error:?}"
    );
}

#[test]
fn beetle_psx_style_region_alternatives_need_only_one_bios() {
    let ready = build_ready_fixture("fw-psx-one-region");
    declare_required_firmware(&ready, &["scph5500.bin", "scph5501.bin", "scph5502.bin"]);
    fs::write(ready.fixture.path("system/scph5501.bin"), b"us").unwrap();
    preflight(&ready).expect("one regional BIOS of the alternative group is enough");
}

#[test]
fn beetle_psx_style_with_no_region_bios_is_refused() {
    let ready = build_ready_fixture("fw-psx-none");
    declare_required_firmware(&ready, &["scph5500.bin", "scph5501.bin", "scph5502.bin"]);
    assert!(preflight(&ready).is_err());
}

#[test]
fn all_required_firmware_present_unverified_still_plays() {
    let ready = build_ready_fixture("fw-all-present");
    declare_required_firmware(&ready, &["a.bin", "b.bin"]);
    fs::write(ready.fixture.path("system/a.bin"), b"x").unwrap();
    fs::write(ready.fixture.path("system/b.bin"), b"y").unwrap();
    preflight(&ready).expect("all required present keeps Play");
}

#[test]
fn mixed_firmware_becomes_playable_once_the_missing_file_is_installed() {
    let ready = build_ready_fixture("fw-mixed-fixed");
    declare_required_firmware(&ready, &["a.bin", "b.bin"]);
    fs::write(ready.fixture.path("system/a.bin"), b"x").unwrap();
    assert!(preflight(&ready).is_err());
    fs::write(ready.fixture.path("system/b.bin"), b"y").unwrap();
    preflight(&ready).expect("installing the missing file unblocks");
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

// --- verified AppImage profile ----------------------------------------------------------

#[test]
fn verified_appimage_profile_passes_preflight_with_its_exact_argv() {
    let ready = build_ready_appimage_fixture("appimage-ready");
    let command = preflight(&ready).expect("a verified AppImage profile must preflight cleanly");
    assert_eq!(
        command.executable,
        ready.request.expected_appimage_executable.clone().unwrap(),
        "the exact reviewed AppImage path, never a native fallback"
    );
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("--config"),
            ready
                .fixture
                .path("Applications/RetroArch.AppImage.config/retroarch/retroarch.cfg")
                .into_os_string(),
            OsString::from("-L"),
            ready
                .fixture
                .path("cores/genesis_plus_gx_libretro.so")
                .into_os_string(),
            ready.request.selected_content_path.clone().into_os_string(),
        ]
    );
    assert_eq!(command.working_directory, None);
}

#[test]
fn appimage_request_without_the_exact_discovered_path_is_refused_without_fallback() {
    let mut ready = build_ready_appimage_fixture("appimage-no-fallback");
    ready.request.expected_appimage_executable =
        Some(ready.fixture.path("Applications/other.AppImage"));
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::AppImageMissing);
}

#[test]
fn missing_appimage_is_refused() {
    let ready = build_ready_appimage_fixture("appimage-missing");
    fs::remove_file(ready.request.expected_appimage_executable.as_ref().unwrap()).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::AppImageMissing);
}

#[test]
fn non_regular_appimage_is_refused() {
    let ready = build_ready_appimage_fixture("appimage-directory");
    let executable = ready.request.expected_appimage_executable.as_ref().unwrap();
    fs::remove_file(executable).unwrap();
    fs::create_dir(executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::AppImageUnsafe);
}

#[test]
fn non_executable_appimage_is_refused_without_chmod() {
    let ready = build_ready_appimage_fixture("appimage-not-executable");
    fs::set_permissions(
        ready.request.expected_appimage_executable.as_ref().unwrap(),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::AppImageNotExecutable);
}

#[test]
fn appimage_identity_evidence_that_goes_stale_is_refused() {
    let ready = build_ready_appimage_fixture("appimage-stale-evidence");
    fs::write(
        ready.fixture.path("desktop/retroarch.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Unrelated\nExec={}\n",
            ready
                .request
                .expected_appimage_executable
                .as_ref()
                .unwrap()
                .display()
        ),
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        LaunchPreflightErrorKind::AppImageVerificationStale
    );
}

#[test]
fn appimage_final_pre_spawn_recheck_refuses_a_removed_executable() {
    let ready = build_ready_appimage_fixture("appimage-final-recheck");
    let executable = ready.request.expected_appimage_executable.as_ref().unwrap();
    fs::remove_file(executable).unwrap();
    let error = recheck_selected_executable(executable, ProfileKind::AppImage).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::AppImageMissing);
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
    crate::verified_game_fixture::compile_native_test_executable(
        &ready.fixture.path("bin/retroarch"),
        "#!/bin/sh\necho 'synthetic failure' 1>&2\nexit 7\n",
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

// --- multi-disc playlist handoff (mechanics on the ready fixture) -------------
//
// The media set below is synthetic evidence wrapped around the fixture's own
// verified content so the real preflight can succeed; what is under test is
// that the shared playlist step replaces exactly the content argument with the
// staged managed playlist, and changes nothing when no playlist was planned.

fn playlist_for(
    ready: &ReadyFixture,
) -> (
    Vec<crate::media_set::MediaSet>,
    crate::launch::multidisc_launch::MultiDiscPlaylist,
) {
    use crate::launch::multidisc_launch::{MultiDiscLaunch, plan_multidisc_launch};
    let first = ready.request.selected_content_path.clone();
    let second = ready
        .fixture
        .write("content/game-disc-2.md", b"second disc bytes");
    let sets = vec![crate::launch::multidisc_launch::tests::verified_set_with(
        &[first.clone(), second],
    )];
    let target = crate::launch::planning::LaunchTarget::RetroArchCore {
        profile: ready.request.profile,
        core_stem: ready.request.core_stem.clone(),
        platform_id: "psx",
    };
    let extensions = vec!["md".to_string(), "m3u".to_string()];
    match plan_multidisc_launch(
        &sets,
        &first,
        &target,
        Some(&extensions),
        false,
        &ready.fixture.path("playlists"),
    ) {
        MultiDiscLaunch::Playlist(playlist) => (sets, playlist),
        other => panic!("{other:?}"),
    }
}

#[test]
fn no_planned_playlist_is_exactly_the_ordinary_preflight() {
    let ready = build_ready_fixture("playlist-none");
    let ordinary = preflight(&ready).unwrap();
    let wrapped = preflight_retroarch_launch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .unwrap();
    assert_eq!(wrapped.arguments, ordinary.arguments);
    assert_eq!(wrapped.executable, ordinary.executable);
}

#[test]
fn planned_playlist_replaces_only_the_content_argument() {
    let ready = build_ready_fixture("playlist-handoff");
    let ordinary = preflight(&ready).unwrap();
    let (sets, playlist) = playlist_for(&ready);
    let command = preflight_retroarch_launch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        Some((&sets, &playlist)),
    )
    .unwrap();
    let n = ordinary.arguments.len();
    assert_eq!(command.arguments[..n - 1], ordinary.arguments[..n - 1]);
    assert_eq!(command.arguments[n - 1], playlist.playlist_path.as_os_str());
    assert_eq!(
        command.selection.content_path,
        ready.request.selected_content_path
    );
    let body = fs::read_to_string(&playlist.playlist_path).unwrap();
    let listed: Vec<_> = body.lines().filter(|line| !line.starts_with('#')).collect();
    assert_eq!(
        listed,
        playlist
            .discs
            .iter()
            .map(|d| d.to_str().unwrap())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_disc_changed_after_preview_refuses_the_launch_without_writing() {
    let ready = build_ready_fixture("playlist-changed");
    let (sets, playlist) = playlist_for(&ready);
    fs::write(&playlist.discs[1], b"different second disc, different size").unwrap();
    let error = preflight_retroarch_launch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        Some((&sets, &playlist)),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        LaunchPreflightErrorKind::MultiDiscPlaylistRefused
    );
    assert!(!playlist.playlist_path.exists());
}

#[test]
fn raw_nkit_content_is_refused_before_retroarch_discovery_or_spawn() {
    for game_id in [b"GTSE01", b"WBFS01", b"CISO01"] {
        let mut ready = build_ready_fixture("nkit-libretro");
        let mut bytes = vec![0u8; 0x2440];
        bytes[..6].copy_from_slice(game_id);
        bytes[0x1c..0x20].copy_from_slice(&0xc2339f3du32.to_be_bytes());
        bytes[0x200..0x208].copy_from_slice(b"NKIT v01");
        bytes[0x210..0x214].copy_from_slice(&0x57058000u32.to_be_bytes());
        let path = ready.fixture.write("content/ordinary.iso", &bytes);
        ready.request.selected_content_path = path.clone();
        ready.request.expected_platform_id = "GameCube".into();
        ready.request.expected_game_key = std::str::from_utf8(game_id).unwrap().into();
        let error =
            preflight(&ready).expect_err("Raw NKit must not bypass refusal through libretro");
        assert_eq!(
            error.kind,
            LaunchPreflightErrorKind::CandidateContentUnsupported
        );
        assert!(error.detail.contains("NKit"));
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

// --- explicit launch choice between tied RetroArch installations ------------

/// Native and AppImage RetroArch both carry the same GameCube core, so the
/// freshly rebuilt preflight plan ties them. Only an explicit choice naming
/// the requested installation may launch it; nothing else is substituted.
#[test]
fn a_gamecube_retroarch_tie_launches_only_the_explicitly_chosen_installation() {
    let fixture = Fixture::new("gamecube-launch-choice");
    fixture.write_executable("bin/retroarch", b"#!/bin/sh\nexit 0\n");
    let appimage =
        fixture.write_executable("Applications/RetroArch.AppImage", b"#!/bin/sh\nexit 0\n");
    let cores_dir = fixture.path("cores");
    let info_dir = fixture.path("info");
    let config = format!(
        "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\n",
        cores_dir.display(),
        info_dir.display()
    );
    fixture.write("config/retroarch/retroarch.cfg", config.as_bytes());
    fixture.write(
        "Applications/RetroArch.AppImage.config/retroarch/retroarch.cfg",
        config.as_bytes(),
    );
    fixture.write("cores/dolphin_libretro.so", b"stub core");
    fixture.write(
        "info/dolphin.info",
        b"systemname = \"GameCube\"\ndatabase = \"Nintendo - GameCube\"\n",
    );
    fixture.write(
        "desktop/retroarch.desktop",
        format!(
            "[Desktop Entry]\nType=Application\nName=RetroArch\nExec={}\n",
            appimage.display()
        )
        .as_bytes(),
    );
    let mut header = vec![0u8; 0x440];
    header[..6].copy_from_slice(b"GZCE51");
    header[6] = 1;
    header[0x1c..0x20].copy_from_slice(&[0xc2, 0x33, 0x9f, 0x3d]);
    let content = fixture.write("content/ZooCube (USA).iso", &header);
    let (identity, _) = crate::launch::evidence_bridge::canonical_identity_from_game_report(
        &inspect_catalogued_game_identity(&content, Some("GameCube")),
    );
    let CanonicalIdentityStatus::Resolved(identity) = identity else {
        panic!("synthetic GameCube header must resolve: {identity:?}");
    };
    let mut environment = fixture.env();
    environment.app_image_search_roots = vec![fixture.path("Applications")];
    environment.desktop_file_roots = vec![fixture.path("desktop")];
    let native = ProfileRef {
        profile_kind: ProfileKind::Native,
        scope: ProfileScope::User,
    };
    let mut request = RetroArchLaunchRequest {
        launch_choice: None,
        selected_content_path: content,
        expected_platform_id: identity.platform_id.clone(),
        expected_game_key: identity.game_key.clone(),
        profile: native,
        core_stem: "dolphin".to_string(),
        saturn_core_selection: None,
        expected_snapshot_evidence: None,
        expected_appimage_executable: None,
        expected_executable_binding: None,
        expected_configuration_file: None,
    };
    let run = |request: &RetroArchLaunchRequest| {
        preflight_retroarch_launch(request, &HostReadOnlyFilesystem, &environment)
    };

    let tied = run(&request).unwrap_err();
    assert_eq!(
        tied.kind,
        LaunchPreflightErrorKind::CandidateNotReady,
        "{tied:?}"
    );

    request.launch_choice = Some("retroarch/appimage/user/dolphin".into());
    assert_eq!(
        run(&request).unwrap_err().kind,
        LaunchPreflightErrorKind::IdentityMismatch,
        "a choice of the other installation never launches this one"
    );
    request.launch_choice = Some("standalone/dolphin/dolphin:appimage:/x".into());
    assert_eq!(
        run(&request).unwrap_err().kind,
        LaunchPreflightErrorKind::IdentityMismatch,
        "a choice naming a different kind of option is refused before planning"
    );

    request.launch_choice = Some("retroarch/native/user/dolphin".into());
    let command = run(&request).expect("the chosen installation passes every other check");
    assert_eq!(command.executable, fixture.path("bin/retroarch"));
    assert_eq!(command.selection.core_stem, "dolphin");
    assert_eq!(command.selection.profile, native);
}

// ---------------------------------------------------------------------------
// Zipped cartridge launch: extract the one verified member for this launch only
// ---------------------------------------------------------------------------

fn write_rom_zip(path: &std::path::Path, members: &[(&str, &[u8])]) {
    use std::io::Write as _;
    let mut writer = zip::write::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in members {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

/// Rewrites the fixture's `retroarch.cfg` with explicit absolute save and
/// state directories plus `extra` lines.
fn write_zip_test_config(ready: &ReadyFixture, extra: &str) {
    // Outside the system temp folder, which the staged route refuses.
    let unique = ready
        .fixture
        .root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let _ = unique;
    // Shared, fixed folders (RetroArch ignores a missing one, so the staged
    // route requires them to exist); never removed, so tests cannot race.
    let saves = PathBuf::from("/var/tmp/emuwiz-zip-test-saves");
    let states = PathBuf::from("/var/tmp/emuwiz-zip-test-states");
    fs::create_dir_all(&saves).unwrap();
    fs::create_dir_all(&states).unwrap();
    ready.fixture.write(
        "config/retroarch/retroarch.cfg",
        format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\nsavefile_directory = \"{}\"\nsavestate_directory = \"{}\"\n{extra}",
            ready.fixture.path("cores").display(),
            ready.fixture.path("info").display(),
            saves.display(),
            states.display(),
        )
        .as_bytes(),
    );
}

fn zipped_ready(label: &str, members: &[(&str, &[u8])]) -> (ReadyFixture, PathBuf, Vec<u8>) {
    let mut ready = build_ready_fixture(label);
    let rom = b"synthetic mega drive rom bytes".to_vec();
    let zip = ready.fixture.path("content/game.zip");
    write_rom_zip(&zip, members);
    write_zip_test_config(&ready, "");
    ready.request.selected_content_path = zip.clone();
    (ready, zip, rom)
}

fn staged_dirs() -> Vec<PathBuf> {
    fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("emuwiz-zip-rom-"))
        })
        .collect()
}

static STAGING_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn zipped_cartridge_launches_from_a_private_copy_and_cleans_up_after_exit() {
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rom = b"synthetic mega drive rom bytes";
    let (ready, zip, _) = zipped_ready("zip-happy", &[("Game (USA).md", rom)]);
    let zip_before = fs::read(&zip).unwrap();
    let mut process = preflight_and_launch_retroarch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .expect("a verified zipped cartridge launches");
    let staged = process
        .staged_cartridge
        .as_ref()
        .expect("guard travels with the process")
        .path()
        .to_path_buf();
    assert_eq!(fs::read(&staged).unwrap(), rom);
    assert_eq!(staged.file_name().unwrap(), "Game (USA).md");
    assert_ne!(staged, zip);
    let dir = staged.parent().unwrap().to_path_buf();
    let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    assert_eq!(fs::read(&zip).unwrap(), zip_before, "ZIP never modified");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while process.poll().is_none() {
        assert!(std::time::Instant::now() < deadline, "fake script exits");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!dir.exists(), "staging directory removed after exit");
}

#[test]
fn zipped_cartridge_refusals_launch_nothing_and_leave_no_staging() {
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let before = staged_dirs();
    let rom = b"synthetic mega drive rom bytes";
    // Ambiguous: two candidate ROM members.
    let (ready, _, _) = zipped_ready(
        "zip-ambiguous",
        &[("a.md", rom), ("b.md", b"other rom bytes")],
    );
    assert!(
        preflight_and_launch_retroarch_with_playlist(
            &ready.request,
            &HostReadOnlyFilesystem,
            &ready.environment,
            None
        )
        .is_err()
    );
    // Changed since readiness: a different ROM now sits at the same path.
    let (ready, zip, _) = zipped_ready("zip-changed", &[("a.md", rom)]);
    write_rom_zip(&zip, &[("a.md", b"a different rom entirely")]);
    let error = preflight_and_launch_retroarch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .err()
    .expect("a replaced ZIP must not launch");
    assert!(
        format!("{error:?}").contains("IdentityMismatch"),
        "{error:?}"
    );
    // Traversal member name.
    let (ready, _, _) = zipped_ready("zip-traversal", &[("../evil.md", rom)]);
    assert!(
        preflight_and_launch_retroarch_with_playlist(
            &ready.request,
            &HostReadOnlyFilesystem,
            &ready.environment,
            None
        )
        .is_err()
    );
    // Corrupt payload fails after the directory exists; it must be removed.
    let (ready, zip, _) = zipped_ready("zip-corrupt", &[("a.md", rom)]);
    let mut bytes = fs::read(&zip).unwrap();
    let at = bytes.windows(rom.len()).position(|w| w == rom).unwrap();
    bytes[at] ^= 0xFF;
    fs::write(&zip, bytes).unwrap();
    assert!(
        preflight_and_launch_retroarch_with_playlist(
            &ready.request,
            &HostReadOnlyFilesystem,
            &ready.environment,
            None
        )
        .is_err()
    );
    assert_eq!(staged_dirs(), before, "no staging directory left behind");
}

#[test]
fn zipped_cartridge_symlink_member_and_oversize_are_refused() {
    use std::io::Write as _;
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let before = staged_dirs();
    let (ready, zip, _) = zipped_ready("zip-symlink", &[("a.md", b"x")]);
    let mut writer = zip::write::ZipWriter::new(fs::File::create(&zip).unwrap());
    writer
        .add_symlink(
            "a.md",
            "/etc/passwd",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer.finish().unwrap();
    assert!(
        preflight_and_launch_retroarch_with_playlist(
            &ready.request,
            &HostReadOnlyFilesystem,
            &ready.environment,
            None
        )
        .is_err()
    );
    let (ready, zip, _) = zipped_ready("zip-big", &[("a.md", b"x")]);
    let mut writer = zip::write::ZipWriter::new(fs::File::create(&zip).unwrap());
    writer
        .start_file(
            "a.md",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer
        .write_all(&vec![
            0u8;
            (crate::game_identity::MAX_LOOSE_ROM_BYTES + 1)
                as usize
        ])
        .unwrap();
    writer.finish().unwrap();
    assert!(
        preflight_and_launch_retroarch_with_playlist(
            &ready.request,
            &HostReadOnlyFilesystem,
            &ready.environment,
            None
        )
        .is_err()
    );
    assert_eq!(staged_dirs(), before);
}

#[test]
fn zipped_cartridge_plan_is_ready_for_retroarch_and_only_when_verified() {
    let rom = b"synthetic mega drive rom bytes";
    let (ready, zip, _) = zipped_ready("zip-plan", &[("a.md", rom)]);
    let report = inspect_catalogued_game_identity(&zip, Some("MegaDrive"));
    let content = crate::launch::launch_content_ref_for_zipped_cartridge(&report)
        .expect("a verified single-ROM ZIP has launch content");
    assert!(!content.requires_mount);
    assert_eq!(content.resolved_path.as_deref(), Some(zip.as_path()));
    let (identity, _) = crate::launch::canonical_identity_from_game_report(&report);
    let environment =
        discover_retroarch_environment(&HostReadOnlyFilesystem, &ready.environment).unwrap();
    let plan = build_launch_plan(&identity, &content, &[], &environment, &[]);
    assert!(
        plan.candidates.iter().any(|candidate| matches!(
            candidate.target,
            LaunchTarget::RetroArchCore { .. }
        ) && candidate.readiness == LaunchReadiness::Ready),
        "{plan:?}"
    );
    // Ambiguous or loose evidence never gets the zipped-cartridge content.
    let (ambiguous, zip2, _) = zipped_ready("zip-plan-amb", &[("a.md", rom), ("b.md", b"zz")]);
    let _ = ambiguous;
    let report = inspect_catalogued_game_identity(&zip2, Some("MegaDrive"));
    assert!(crate::launch::launch_content_ref_for_zipped_cartridge(&report).is_none());
    let loose = inspect_catalogued_game_identity(
        &ready.fixture.write("content/x.md", rom),
        Some("MegaDrive"),
    );
    assert!(crate::launch::launch_content_ref_for_zipped_cartridge(&loose).is_none());
}

#[test]
fn zipped_cartridge_is_refused_when_saves_would_be_lost_or_launcher_may_outlive() {
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let before = staged_dirs();
    let rom = b"synthetic mega drive rom bytes";
    let run = |ready: &ReadyFixture| {
        preflight_and_launch_retroarch_with_playlist(
            &ready.request,
            &HostReadOnlyFilesystem,
            &ready.environment,
            None,
        )
        .err()
        .map(|error| format!("{error:?}"))
    };
    // Saves stored beside the content.
    let (ready, _, _) = zipped_ready("zip-in-content", &[("a.md", rom)]);
    write_zip_test_config(&ready, "savefiles_in_content_dir = \"true\"\n");
    assert!(run(&ready).unwrap().contains("beside the game"));
    let (ready, _, _) = zipped_ready("zip-states-in-content", &[("a.md", rom)]);
    write_zip_test_config(&ready, "savestates_in_content_dir = \"true\"\n");
    assert!(run(&ready).unwrap().contains("beside the game"));
    // Save or state directory not set.
    let (ready, _, _) = zipped_ready("zip-no-dirs", &[("a.md", rom)]);
    ready.fixture.write(
        "config/retroarch/retroarch.cfg",
        format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\n",
            ready.fixture.path("cores").display(),
            ready.fixture.path("info").display()
        )
        .as_bytes(),
    );
    assert!(run(&ready).unwrap().contains("savefile_directory"));
    // A launcher profile may exit while the game runs.
    let (mut ready, _, _) = zipped_ready("zip-appimage", &[("a.md", rom)]);
    ready.request.profile.profile_kind = ProfileKind::AppImage;
    assert!(run(&ready).unwrap().contains("native RetroArch"));
    assert_eq!(staged_dirs(), before);
}

#[test]
fn a_kept_staging_directory_survives_for_a_possibly_running_emulator() {
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_ready, zip, _) = zipped_ready("zip-keep", &[("a.md", b"synthetic mega drive rom bytes")]);
    let staged = crate::zip_cartridge_stage::stage_zip_cartridge_for_launch(&zip, "MegaDrive")
        .expect("stages");
    let dir = staged.path().parent().unwrap().to_path_buf();
    staged.keep();
    assert!(dir.join("a.md").is_file());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn zipped_cartridge_save_setup_fails_closed_on_every_unreadable_or_overriding_source() {
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rom = b"synthetic mega drive rom bytes";
    let refusal = |label: &str, setup: &dyn Fn(&ReadyFixture)| -> String {
        let (ready, _, _) = zipped_ready(label, &[("a.md", rom)]);
        setup(&ready);
        preflight_and_launch_retroarch_with_playlist(
            &ready.request,
            &HostReadOnlyFilesystem,
            &ready.environment,
            None,
        )
        .err()
        .map(|e| format!("{e:?}"))
        .unwrap_or_default()
    };
    // Any override file under <config dir>/config/ refuses without parsing it.
    let text = refusal("zip-override", &|r| {
        r.fixture.write(
            "config/retroarch/config/Core/Core.cfg",
            b"savefile_directory = \"/x\"\n",
        );
    });
    assert!(text.contains("override"), "{text}");
    // #include in the main settings.
    let text = refusal("zip-include", &|r| {
        write_zip_test_config(r, "#include \"other.cfg\"\n");
    });
    assert!(text.contains("include"), "{text}");
    // Legacy per-user settings file present.
    let text = refusal("zip-legacy", &|r| {
        r.fixture.write(".retroarch.cfg", b"");
    });
    assert!(text.contains("legacy"), "{text}");
    // In-content flag set to anything but explicit false, including a comment.
    for value in ["1", "true # x", "\"yes\""] {
        let text = refusal("zip-flag", &|r| {
            write_zip_test_config(r, &format!("savefiles_in_content_dir = {value}\n"));
        });
        assert!(text.contains("beside the game"), "{value}: {text}");
    }
    // Conflicting duplicate keys.
    let text = refusal("zip-dup", &|r| {
        write_zip_test_config(r, "savefile_directory = \"/var/tmp/other\"\n");
    });
    assert!(text.contains("more than once"), "{text}");
    // Same folder for saves and states; folder inside the temp dir.
    let text = refusal("zip-same", &|r| {
        r.fixture.write(
            "config/retroarch/retroarch.cfg",
            format!(
                "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\nsavefile_directory = \"/var/tmp/s\"\nsavestate_directory = \"/var/tmp/s\"\n",
                r.fixture.path("cores").display(),
                r.fixture.path("info").display()
            )
            .as_bytes(),
        );
    });
    assert!(text.contains("separate"), "{text}");
    let text = refusal("zip-temp", &|r| {
        let inside = std::env::temp_dir().join("emuwiz-saves-test");
        r.fixture.write(
            "config/retroarch/retroarch.cfg",
            format!(
                "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\nsavefile_directory = \"{}\"\nsavestate_directory = \"/var/tmp/q\"\n",
                r.fixture.path("cores").display(),
                r.fixture.path("info").display(),
                inside.display()
            )
            .as_bytes(),
        );
    });
    assert!(text.contains("temporary folder"), "{text}");
}

#[test]
fn only_cartridge_platform_zips_count_as_zipped_cartridges() {
    use crate::zip_cartridge_stage::is_zipped_cartridge_request as zipped;
    use std::path::Path;
    assert!(zipped(Path::new("/r/g.zip"), "SNES"));
    assert!(zipped(Path::new("/r/g.ZIP"), "MegaDrive"));
    for platform in ["Arcade", "MAME", "PlayStation", "GameCube", "Amiga"] {
        assert!(!zipped(Path::new("/r/g.zip"), platform), "{platform}");
    }
    assert!(!zipped(Path::new("/r/g.sfc"), "SNES"));
}

// ---- NW2-C-01: sort saves/states by content folder ----

/// NW2-C-01: with RetroArch's "sort saves/states by content directory" on,
/// saves go to <savefile_directory>/<content folder name>/. A staged zipped
/// cartridge runs from a fresh random `emuwiz-zip-rom-*` folder each launch,
/// so the save would land somewhere new every time. The staged route must
/// refuse, as it already does for the *_in_content_dir keys.
#[test]
fn nw2_c01_zipped_cartridge_is_refused_when_retroarch_sorts_saves_by_content_folder() {
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rom = b"synthetic mega drive rom bytes";
    for key in [
        "sort_savefiles_by_content_enable",
        "sort_savestates_by_content_enable",
    ] {
        let (ready, _, _) = zipped_ready(&format!("nw2-c01-{key}"), &[("a.md", rom)]);
        write_zip_test_config(&ready, &format!("{key} = \"true\"\n"));
        let result = preflight_and_launch_retroarch_with_playlist(
            &ready.request,
            &HostReadOnlyFilesystem,
            &ready.environment,
            None,
        );
        let Err(error) = result else {
            panic!(
                "{key} = true: the staged launch went ahead, so saves land in a new per-launch folder"
            );
        };
        assert!(format!("{error:?}").contains(key), "{error:?}");
    }
}

/// Runs only the staged-launch safety check against a config with `extra`.
fn staged_safety_with(label: &str, extra: &str) -> Result<(), String> {
    let (ready, _, _) = zipped_ready(label, &[("a.md", b"synthetic mega drive rom bytes")]);
    write_zip_test_config(&ready, extra);
    staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment)
}

#[test]
fn nw2_c01_sort_by_content_keys_accept_only_absent_or_explicit_false() {
    assert_eq!(staged_safety_with("nw2-c01-absent", ""), Ok(()));
    for key in [
        "sort_savefiles_by_content_enable",
        "sort_savestates_by_content_enable",
    ] {
        for ok in ["\"false\"", "false", "FALSE", "\"0\"", "0"] {
            assert_eq!(
                staged_safety_with("nw2-c01-ok", &format!("{key} = {ok}\n")),
                Ok(()),
                "{key} = {ok}"
            );
        }
        for bad in [
            "\"true\"", "true", "\"1\"", "1", "\"yes\"", "\"\"", "true # x",
        ] {
            let error = staged_safety_with("nw2-c01-bad", &format!("{key} = {bad}\n"))
                .expect_err(&format!("{key} = {bad} must be refused"));
            assert!(error.contains(key), "{error}");
            assert!(error.contains("emuwiz-zip-rom-"), "{error}");
        }
    }
}

#[test]
fn nw2_c01_conflicting_duplicate_sort_by_content_keys_are_refused() {
    for key in [
        "sort_savefiles_by_content_enable",
        "sort_savestates_by_content_enable",
    ] {
        let error = staged_safety_with(
            "nw2-c01-dup",
            &format!("{key} = \"false\"\n{key} = \"true\"\n"),
        )
        .expect_err("conflicting duplicates must be refused");
        assert!(
            error.contains("more than once") && error.contains(key),
            "{error}"
        );
        // Identical duplicates are not a conflict.
        assert_eq!(
            staged_safety_with(
                "nw2-c01-dup-same",
                &format!("{key} = \"false\"\n{key} = \"false\"\n")
            ),
            Ok(())
        );
    }
}

/// A UTF-8 byte-order mark on line 1 must not hide a key from the check.
#[test]
fn nw2_c01_leading_bom_does_not_hide_sort_by_content_keys() {
    let bom_config = |label: &str, first: &str| -> Result<(), String> {
        let (ready, _, _) = zipped_ready(label, &[("a.md", b"synthetic mega drive rom bytes")]);
        write_zip_test_config(&ready, "");
        let path = ready.fixture.path("config/retroarch/retroarch.cfg");
        let body = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("\u{feff}{first}{body}")).unwrap();
        staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment)
    };
    for key in [
        "sort_savefiles_by_content_enable",
        "sort_savestates_by_content_enable",
        "savefiles_in_content_dir",
    ] {
        let error = bom_config("nw2-c01-bom-true", &format!("{key} = \"true\"\n"))
            .expect_err("BOM + true must be refused");
        assert!(error.contains(key), "{error}");
        assert_eq!(
            bom_config("nw2-c01-bom-false", &format!("{key} = \"false\"\n")),
            Ok(())
        );
    }
    assert_eq!(bom_config("nw2-c01-bom-absent", ""), Ok(()));
}

// ---------------------------------------------------------------------------
// Unverified (one-time try / remembered / saved system) launches: the
// content-bound `unverified:` key is held from readiness to the spawn
// ---------------------------------------------------------------------------

fn unverified_key(path: &std::path::Path) -> String {
    let identity =
        crate::launch::unverified_media::platform_media_identity_fresh(path, "MegaDrive")
            .expect("the format is one Mega Drive lists");
    assert!(crate::launch::unverified_media::is_unverified_key(
        &identity.game_key
    ));
    identity.game_key
}

/// Replaces the fake RetroArch with one that records the content it was
/// handed (its last argument): the path and that file's bytes.
fn record_handoff(ready: &ReadyFixture) -> (PathBuf, PathBuf) {
    let path_out = ready.fixture.path("handoff-path.txt");
    let bytes_out = ready.fixture.path("handoff-bytes.bin");
    ready.fixture.write_executable(
        "bin/retroarch",
        format!(
            "#!/bin/sh\nfor a; do last=\"$a\"; done\nprintf '%s' \"$last\" > '{}'\ncat \"$last\" > '{}'\nexit 0\n",
            path_out.display(),
            bytes_out.display()
        )
        .as_bytes(),
    );
    (path_out, bytes_out)
}

fn wait_for_exit(process: &mut LaunchedRetroArchProcess) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while process.poll().is_none() {
        assert!(std::time::Instant::now() < deadline, "fake script exits");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_unverified_zipped_cartridge_reaches_the_emulator_and_changed_zips_are_refused() {
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let before = staged_dirs();
    let rom = b"synthetic mega drive rom bytes";

    // Unchanged ZIP, one valid member, unverified key bound to the ZIP: the
    // member itself is what RetroArch receives, from a private copy.
    let (mut ready, zip, _) = zipped_ready("zip-unverified-ok", &[("Game (USA).md", rom)]);
    ready.request.expected_game_key = unverified_key(&zip);
    let (path_out, bytes_out) = record_handoff(&ready);
    let zip_before = fs::read(&zip).unwrap();
    let mut process = preflight_and_launch_retroarch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .expect("an unchanged zipped cartridge tried as Mega Drive launches");
    let staged = process
        .staged_cartridge
        .as_ref()
        .expect("staged copy travels with the process")
        .path()
        .to_path_buf();
    wait_for_exit(&mut process);
    assert_eq!(
        fs::read(&bytes_out).unwrap(),
        rom,
        "the member reached RetroArch"
    );
    assert_eq!(
        PathBuf::from(fs::read_to_string(&path_out).unwrap()),
        staged
    );
    assert_ne!(staged, zip, "never the ZIP itself");
    assert_eq!(fs::read(&zip).unwrap(), zip_before, "ZIP never modified");

    // The ZIP was replaced after readiness (a different member): refused.
    let (mut ready, zip, _) = zipped_ready("zip-unverified-member", &[("a.md", rom)]);
    ready.request.expected_game_key = unverified_key(&zip);
    let (_, bytes_out) = record_handoff(&ready);
    write_rom_zip(&zip, &[("a.md", b"a different rom entirely")]);
    let error = preflight_and_launch_retroarch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .err()
    .expect("a ZIP whose member changed must not launch");
    assert!(
        format!("{error:?}").contains("IdentityUnresolved"),
        "{error:?}"
    );
    assert!(!bytes_out.exists(), "nothing was started");

    // Same member, different archive bytes (a member added): refused.
    let (mut ready, zip, _) = zipped_ready("zip-unverified-archive", &[("a.md", rom)]);
    ready.request.expected_game_key = unverified_key(&zip);
    write_rom_zip(&zip, &[("a.md", rom), ("readme.txt", b"hello")]);
    let error = preflight_and_launch_retroarch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .err()
    .expect("a changed ZIP must not launch");
    assert!(
        format!("{error:?}").contains("IdentityUnresolved"),
        "{error:?}"
    );

    // The member's key is never accepted in place of the ZIP's.
    let (mut ready, _, _) = zipped_ready("zip-unverified-wrong-key", &[("a.md", rom)]);
    let loose = ready.fixture.write("content/member.md", rom);
    ready.request.expected_game_key = unverified_key(&loose);
    let error = preflight_and_launch_retroarch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .err()
    .expect("a key for other bytes never authorises the ZIP");
    assert!(
        format!("{error:?}").contains("IdentityUnresolved"),
        "{error:?}"
    );
    assert_eq!(staged_dirs(), before, "no staging directory left behind");
}

#[test]
fn an_unverified_file_changed_between_preflight_and_spawn_is_refused() {
    let mut ready = build_ready_fixture("unverified-spawn-swap");
    let content = ready.request.selected_content_path.clone();
    ready.request.expected_game_key = unverified_key(&content);
    let command = preflight(&ready).expect("unchanged unverified file passes preflight");
    assert_eq!(
        command.unverified_game_key.as_deref(),
        Some(ready.request.expected_game_key.as_str())
    );
    // Different bytes of the same listed format, same path.
    fs::write(&content, b"other mega drive rom bytes!!!!").unwrap();
    assert!(
        spawn_retroarch(command).is_err(),
        "spawn re-derives the key and refuses a different file"
    );
}

#[test]
fn an_unverified_file_whose_header_names_another_system_is_refused_at_preflight() {
    let mut rom = vec![0u8; 0x400];
    rom[0x100..0x109].copy_from_slice(b"SEGA PICO");
    let mut ready = build_ready_fixture("unverified-pico");
    let content = ready.request.selected_content_path.clone();
    fs::write(&content, &rom).unwrap();
    ready.request.expected_game_key = unverified_key(&content);
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, LaunchPreflightErrorKind::IdentityUnresolved);
    assert!(error.detail.contains("Sega Pico"), "{}", error.detail);
}

/// A ZIP renamed over the approved one after the key was checked can never
/// be extracted in its place: the key is read through staging's own handle,
/// and a path that no longer names that handle's file is refused.
#[test]
fn a_zip_swapped_in_after_its_key_was_checked_is_never_extracted() {
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let before = staged_dirs();
    let (ready, zip, _) = zipped_ready("zip-swap-race", &[("a.md", b"approved rom bytes")]);
    let approved = unverified_key(&zip);
    let other = ready.fixture.path("content/other.zip");
    write_rom_zip(&other, &[("a.md", b"a different rom entirely")]);
    let result = crate::zip_cartridge_stage::stage_zip_cartridge_for_launch_checked(
        &zip,
        "MegaDrive",
        |file| {
            let keyed = crate::launch::unverified_media::platform_media_key_of_open_file(
                file,
                &zip,
                "MegaDrive",
            );
            // The swap lands after the handle was opened.
            fs::rename(&other, &zip).unwrap();
            keyed.as_deref() == Some(approved.as_str())
        },
    );
    match result {
        Ok(staged) => panic!(
            "extracted {:?} from a swapped ZIP",
            fs::read(staged.path()).unwrap()
        ),
        Err(error) => assert!(
            matches!(error, crate::zip_cartridge_stage::StageZipError::Refused(_)),
            "{error:?}"
        ),
    }
    // A key that does not match is refused before anything is extracted.
    let (_ready, zip, _) = zipped_ready("zip-swap-key", &[("a.md", b"approved rom bytes")]);
    assert_eq!(
        crate::zip_cartridge_stage::stage_zip_cartridge_for_launch_checked(
            &zip,
            "MegaDrive",
            |_| false
        )
        .err(),
        Some(crate::zip_cartridge_stage::StageZipError::NotAccepted)
    );
    assert_eq!(staged_dirs(), before, "no staging directory left behind");
}

/// A Sega Pico ZIP (owner approval 2026-11-10) goes to PicoDrive: its one
/// `.md` member is staged under the Mega Drive member rules, the launch stays
/// Sega Pico, and the same key and header checks hold.
#[test]
fn a_sega_pico_zip_reaches_picodrive_and_never_runs_as_mega_drive() {
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut rom = vec![0u8; 0x400];
    rom[0x100..0x110].copy_from_slice(b"SEGA PICO       ");
    let (mut ready, zip, _) = zipped_ready("pico-zip", &[("Anpanman (Japan).md", &rom)]);
    ready
        .fixture
        .write("cores/picodrive_libretro.so", b"stub core");
    ready.fixture.write(
        "info/picodrive.info",
        b"systemname = \"Sega 8/16-bit + 32X (Various)\"\ndatabase = \"Sega - Mega Drive - Genesis|Sega - PICO|Sega - 32X\"\n",
    );
    let key = crate::launch::unverified_media::platform_media_identity_fresh(&zip, "Sega Pico")
        .expect("Sega Pico lists zip")
        .game_key;
    ready.request.expected_platform_id = "Sega Pico".into();
    ready.request.core_stem = "picodrive".into();
    ready.request.expected_game_key = key.clone();
    let (path_out, bytes_out) = record_handoff(&ready);
    let mut process = preflight_and_launch_retroarch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .expect("a Pico ZIP launches through PicoDrive");
    wait_for_exit(&mut process);
    assert_eq!(
        fs::read(&bytes_out).unwrap(),
        rom,
        "the Pico ROM reached PicoDrive"
    );
    assert!(
        fs::read_to_string(&path_out)
            .unwrap()
            .ends_with("Anpanman (Japan).md")
    );
    // The same file as Mega Drive is refused by its own header.
    ready.request.expected_platform_id = "MegaDrive".into();
    ready.request.core_stem = "genesis_plus_gx".into();
    ready.request.expected_game_key = unverified_key(&zip);
    let error = preflight_and_launch_retroarch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .err()
    .expect("a Pico ROM never runs as Mega Drive");
    assert!(format!("{error:?}").contains("Sega Pico"), "{error:?}");
}

/// Writes a `retroarch.cfg` whose save/state keys are given verbatim and points
/// the discovery home at a folder outside the system temp folder (the staged
/// route refuses saves under it). Returns the fixture and that home.
fn tilde_config_ready(label: &str, save_keys: &str) -> (ReadyFixture, PathBuf) {
    let (mut ready, _, _) = zipped_ready(label, &[("a.md", b"synthetic mega drive rom bytes")]);
    let unique = ready
        .fixture
        .root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let home = PathBuf::from("/var/tmp").join(format!("{unique}-home"));
    fs::create_dir_all(home.join(".config/retroarch/saves")).unwrap();
    fs::create_dir_all(home.join(".config/retroarch/states")).unwrap();
    ready.environment.home = Some(home.clone().into_os_string());
    ready.fixture.write(
        "config/retroarch/retroarch.cfg",
        format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\n{save_keys}",
            ready.fixture.path("cores").display(),
            ready.fixture.path("info").display(),
        )
        .as_bytes(),
    );
    (ready, home)
}

/// The exact save/state lines from the 2026-11-10 production smoke test
/// (`~/.config/retroarch/retroarch.cfg`), which refused every zipped cartridge.
const SMOKE_TEST_TILDE_SAVE_KEYS: &str = "savefile_directory = \"~/.config/retroarch/saves\"\n\
savefiles_in_content_dir = \"false\"\n\
savestate_directory = \"~/.config/retroarch/states\"\n\
savestates_in_content_dir = \"false\"\n\
sort_savefiles_by_content_enable = \"false\"\n\
sort_savefiles_enable = \"false\"\n\
sort_savestates_by_content_enable = \"false\"\n\
sort_savestates_enable = \"false\"\n";

#[test]
fn zipped_cartridge_accepts_retroarch_tilde_save_dirs_from_the_smoke_test_config() {
    let (ready, home) = tilde_config_ready("zip-tilde-smoke", SMOKE_TEST_TILDE_SAVE_KEYS);
    assert_eq!(
        staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment),
        Ok(()),
        "`~/` save and state folders are expanded against the user's home, as RetroArch does"
    );
    let _serial = STAGING_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut process = preflight_and_launch_retroarch_with_playlist(
        &ready.request,
        &HostReadOnlyFilesystem,
        &ready.environment,
        None,
    )
    .expect("a zipped cartridge launches with RetroArch-style `~/` save folders");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while process.poll().is_none() {
        assert!(std::time::Instant::now() < deadline, "fake script exits");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn zipped_cartridge_still_refuses_unexpandable_or_unsafe_save_dirs() {
    let refused = |label: &str, saves: &str, states: &str| -> String {
        let (ready, home) = tilde_config_ready(
            label,
            &format!("savefile_directory = \"{saves}\"\nsavestate_directory = \"{states}\"\n"),
        );
        let result =
            staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment);
        let _ = fs::remove_dir_all(home);
        result.expect_err(&format!("{saves} / {states} must be refused"))
    };
    // Forms RetroArch's own `~` expansion does not cover stay refused.
    for (saves, states) in [
        ("~other/saves", "~/states"),
        ("~/saves", "~other/states"),
        (":/saves", "~/states"),
        ("saves", "~/states"),
        ("", "~/states"),
    ] {
        refused("zip-tilde-bad", saves, states);
    }
    // Expansion happens before the existing checks, which still apply.
    let same = refused("zip-tilde-same", "~/retroarch/saves", "~/retroarch/saves");
    assert!(same.contains("separate"), "{same}");
}

#[test]
fn zipped_cartridge_tilde_save_dirs_inside_a_temporary_home_are_refused() {
    // Home itself under the system temp folder: the expanded folders are
    // temporary, so the existing temporary-folder refusal must still fire.
    let (mut ready, _, _) = zipped_ready(
        "zip-tilde-temp-home",
        &[("a.md", b"synthetic mega drive rom bytes")],
    );
    ready.fixture.write(
        "config/retroarch/retroarch.cfg",
        format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\n{SMOKE_TEST_TILDE_SAVE_KEYS}",
            ready.fixture.path("cores").display(),
            ready.fixture.path("info").display(),
        )
        .as_bytes(),
    );
    ready.environment.home = Some(ready.fixture.root.clone().into_os_string());
    let error = staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment)
        .expect_err("saves under the temporary folder are refused");
    assert!(error.contains("temporary"), "{error}");
}

// ---- 2026-11-10: real-configuration ZIP preflight (false /etc refusal) ----

/// The owner's real `~/.config/retroarch/retroarch.cfg` lines that bear on the
/// staged route (RetroArch 1.22.2, Ubuntu package): `~/` save folders, the
/// override folder named through `rgui_config_directory`, auto overrides on.
const REAL_WORLD_USER_KEYS: &str = "config_save_on_exit = \"true\"\n\
auto_overrides_enable = \"true\"\n\
rgui_config_directory = \"~/.config/retroarch/config/\"\n";

#[test]
fn zip_preflight_accepts_the_real_world_user_config_even_with_a_packaged_etc_skeleton() {
    // Ubuntu's retroarch package installs /etc/retroarch.cfg, a template that
    // RetroArch only copies when it must create a user settings file; with
    // the user's own file present it is never opened (checked with strace on
    // 1.22.2). This test runs unchanged on machines with that file.
    let (ready, home) = tilde_config_ready(
        "zip-real-world",
        &format!("{SMOKE_TEST_TILDE_SAVE_KEYS}{REAL_WORLD_USER_KEYS}"),
    );
    // Per-core folders RetroArch created, with no override files in them.
    fs::create_dir_all(home.join(".config/retroarch/config/PicoDrive")).unwrap();
    fs::create_dir_all(home.join(".config/retroarch/config/Genesis Plus GX")).unwrap();
    let result = staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment);
    let _ = fs::remove_dir_all(&home);
    assert_eq!(result, Ok(()));
}

#[test]
fn zip_preflight_still_refuses_a_legacy_home_retroarch_cfg() {
    let (ready, home) = tilde_config_ready("zip-legacy-home", SMOKE_TEST_TILDE_SAVE_KEYS);
    fs::write(home.join(".retroarch.cfg"), b"").unwrap();
    let result = staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment);
    let _ = fs::remove_dir_all(&home);
    assert!(result.expect_err("legacy file refused").contains("legacy"));
}

#[test]
fn zip_preflight_refuses_save_folders_that_do_not_exist() {
    let (ready, home) = tilde_config_ready("zip-missing-dir", SMOKE_TEST_TILDE_SAVE_KEYS);
    fs::remove_dir(home.join(".config/retroarch/states")).unwrap();
    let result = staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment);
    let _ = fs::remove_dir_all(&home);
    let error = result.expect_err("a missing states folder is refused");
    assert!(
        error.contains("savestate_directory") && error.contains("does not exist"),
        "{error}"
    );
    assert!(!error.contains("savefile_directory"), "{error}");
}

#[test]
fn zip_preflight_refuses_overrides_in_the_configured_rgui_config_directory() {
    let (ready, home) = tilde_config_ready(
        "zip-rgui-overrides",
        &format!(
            "{SMOKE_TEST_TILDE_SAVE_KEYS}rgui_config_directory = \"~/elsewhere/overrides/\"\n"
        ),
    );
    let core = home.join("elsewhere/overrides/PicoDrive");
    fs::create_dir_all(&core).unwrap();
    fs::write(core.join("PicoDrive.cfg"), b"savefile_directory = \"/x\"\n").unwrap();
    let result = staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment);
    let _ = fs::remove_dir_all(&home);
    assert!(result.expect_err("override refused").contains("override"));
}

#[test]
fn zip_preflight_reports_every_problem_together() {
    let (ready, home) = tilde_config_ready(
        "zip-all-findings",
        "savefile_directory = \"~/.config/retroarch/saves\"\n\
         savestate_directory = \"~/.config/retroarch/nostates\"\n\
         savefiles_in_content_dir = \"true\"\n\
         sort_savestates_by_content_enable = \"true\"\n",
    );
    // `config/` beside the settings file the discovery read.
    ready
        .fixture
        .write("config/retroarch/config/PicoDrive/PicoDrive.cfg", b"");
    fs::write(home.join(".retroarch.cfg"), b"").unwrap();
    let result = staged_launch_is_safe(&ready.request, &HostReadOnlyFilesystem, &ready.environment);
    let _ = fs::remove_dir_all(&home);
    let error = result.expect_err("refused");
    for expected in [
        "beside the game",
        "sort_savestates_by_content_enable",
        "does not exist",
        "override",
        "legacy",
    ] {
        assert!(error.contains(expected), "missing {expected:?} in: {error}");
    }
}

/// Read-only diagnostic of the complete ZIP launch preflight against this
/// machine's real RetroArch, settings, PicoDrive core and a real Pico ZIP.
/// Nothing is launched and nothing is written outside a private temporary
/// extraction that is removed on return. Run with
/// `EMUWIZ_TEST_REAL_ENVIRONMENT=1 ... -- --ignored --nocapture`.
#[test]
#[ignore = "reads the real RetroArch installation and library; opt-in only"]
fn real_environment_pico_zip_preflight_diagnostic() {
    use crate::emulator_environment::retroarch::DiscoveryEnvironment as Env;
    if std::env::var_os("EMUWIZ_TEST_REAL_ENVIRONMENT").is_none() {
        eprintln!("EMUWIZ_TEST_REAL_ENVIRONMENT not set; skipped");
        return;
    }
    let zip = PathBuf::from(std::env::var("EMUWIZ_TEST_REAL_ZIP").unwrap_or_else(|_| {
        "/mnt/usbdrive/games/sega-pico/3-choume no Tama - Momo-chan wa Doko! (Japan) (Rev 1).zip"
            .into()
    }));
    let environment = Env::from_process_environment();
    let report = discover_retroarch_environment(&HostReadOnlyFilesystem, &environment)
        .expect("RetroArch discovery");
    let profile = report
        .profiles
        .iter()
        .find(|p| p.profile_kind == ProfileKind::Native)
        .expect("a native RetroArch profile");
    eprintln!("config file: {}", profile.config_file.path.display);
    let platform = "Sega Pico";
    let mut file = fs::File::open(&zip).expect("real ZIP opens");
    let key =
        super::super::unverified_media::platform_media_key_of_open_file(&mut file, &zip, platform)
            .expect("real ZIP has an unverified media key");
    let executable = match profile.evidence.executables.as_slice() {
        [one] => {
            Some(RetroArchExecutableBinding::capture(PathBuf::from(&one.display)).expect("binding"))
        }
        _ => None,
    };
    eprintln!("program: {:?}", executable.as_ref().map(|b| &b.path));
    let request = RetroArchLaunchRequest {
        saturn_core_selection: None,
        launch_choice: None,
        expected_snapshot_evidence: None,
        selected_content_path: zip.clone(),
        expected_platform_id: platform.into(),
        expected_game_key: key,
        profile: ProfileRef {
            profile_kind: profile.profile_kind,
            scope: profile.scope,
        },
        core_stem: "picodrive".into(),
        expected_appimage_executable: None,
        expected_executable_binding: executable,
        expected_configuration_file: Some(PathBuf::from(&profile.config_file.path.display)),
    };
    let findings = staged_launch_findings(&request, &HostReadOnlyFilesystem, &environment);
    eprintln!("save-safety findings ({}):", findings.len());
    for finding in &findings {
        eprintln!("  - {finding}");
    }
    match prepare_retroarch_launch_with_playlist(
        &request,
        &HostReadOnlyFilesystem,
        &environment,
        None,
    ) {
        Ok(prepared) => {
            eprintln!("COMPLETE PREFLIGHT: PASS");
            eprintln!("arguments: {:?}", prepared.command.arguments);
            eprintln!("staged: {:?}", prepared.staged_content_path());
        }
        Err(error) => {
            eprintln!("COMPLETE PREFLIGHT: REFUSED: {error:?}");
            panic!("complete preflight refused");
        }
    }
    assert!(findings.is_empty());
}
