use super::*;
use crate::identity_source::hashing::Crc32;
use crate::launch::planning::ResolvedIdentity;
use crate::patch_manager::{XRoarExplicitProfile, XRoarMachine, discover_xroar_profiles};
use md5::Md5;
use sha1::{Digest, Sha1};
use std::fs::File;
use std::os::unix::fs::PermissionsExt;

const TAPE: &[u8] = b"verified original cassette";

struct Fixture {
    _dir: tempfile::TempDir,
    executable: PathBuf,
    tape: PathBuf,
    rom: PathBuf,
    report: PathBuf,
    roots: XRoarProfileDiscoveryRoots,
    request: XRoarLaunchRequest,
    identity: CanonicalIdentityStatus,
}

fn script(path: &Path, report: &Path, label: &str) {
    fs::write(path, format!(
        "#!/bin/sh\nprintf '{label}\\n' > '{}'\nwhile [ \"$#\" -gt 0 ]; do\ncase \"$1\" in\n-load-tape) /bin/cat \"$2\" >> '{}'; shift;;\n-extbas) /usr/bin/head -c 1 \"$2\" >> '{}'; shift;;\nesac\nshift\ndone\n",
        report.display(), report.display(), report.display()
    )).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("xroar");
    let tape = dir.path().join("original.cas");
    let rom = dir.path().join("d32.rom");
    let report = dir.path().join("consumed.txt");
    script(&executable, &report, "original-executable");
    fs::write(&tape, TAPE).unwrap();
    fs::write(&rom, vec![b'R'; 16384]).unwrap();
    let roots = XRoarProfileDiscoveryRoots {
        explicit_profiles: vec![XRoarExplicitProfile {
            executable: executable.clone(),
            machine: XRoarMachine::Dragon32,
            firmware: XRoarFirmwareState::PresentUnverified,
        }],
        selected_machine: Some(XRoarMachine::Dragon32),
        firmware_directory: Some(dir.path().into()),
        ..Default::default()
    };
    let profile = discover_xroar_profiles(&roots).profiles.remove(0);
    let request = XRoarLaunchRequest {
        selected_content_path: tape.clone(),
        expected_platform_id: "Dragon / Tandy CoCo".into(),
        profile_id: profile.profile_id,
        expected_executable: executable.clone(),
        expected_content_identity: capture_file_identity(&tape).unwrap().1,
        expected_content_md5: Md5::digest(TAPE)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        expected_content_crc32: Crc32::of(TAPE),
    };
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Dragon / Tandy CoCo".into(),
        game_key: Sha1::digest(TAPE)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    });
    Fixture {
        _dir: dir,
        executable,
        tape,
        rom,
        report,
        roots,
        request,
        identity,
    }
}

fn wait(mut process: WatchedProcess) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(report) = process.poll() {
            assert!(
                report.status.as_ref().unwrap().success(),
                "{:?}",
                report.stderr
            );
            break;
        }
        assert!(std::time::Instant::now() < deadline, "child did not exit");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn assert_original(f: &Fixture) {
    let mut expected = b"original-executable\n".to_vec();
    expected.extend_from_slice(TAPE);
    expected.push(b'R');
    assert_eq!(fs::read(&f.report).unwrap(), expected);
}

#[test]
fn prepared_launch_consumes_validated_objects_after_path_replacement() {
    let f = fixture();
    let prepared = preflight_xroar_launch(&f.request, &f.roots, &f.identity).unwrap();
    for path in [&f.executable, &f.tape, &f.rom] {
        fs::rename(path, path.with_extension("saved")).unwrap();
    }
    script(&f.executable, &f.report, "replacement-executable");
    fs::write(&f.tape, b"replacement cassette").unwrap();
    fs::write(&f.rom, vec![b'Z'; 16384]).unwrap();
    wait(spawn_xroar(&prepared).unwrap());
    assert_original(&f);
}

#[test]
fn prepared_launch_consumes_validated_bytes_after_in_place_edits() {
    let f = fixture();
    let prepared = preflight_xroar_launch(&f.request, &f.roots, &f.identity).unwrap();
    script(&f.executable, &f.report, "edited-executable");
    fs::write(&f.tape, b"edited cassette").unwrap();
    fs::write(&f.rom, vec![b'Z'; 16384]).unwrap();
    wait(spawn_xroar(&prepared).unwrap());
    assert_original(&f);
}

#[test]
fn all_catalogue_hashes_are_checked_on_the_captured_cassette() {
    for field in ["sha1", "md5", "crc32"] {
        let mut f = fixture();
        match field {
            "sha1" => {
                if let CanonicalIdentityStatus::Resolved(value) = &mut f.identity {
                    value.game_key = "0".repeat(40);
                }
            }
            "md5" => f.request.expected_content_md5 = "0".repeat(32),
            _ => f.request.expected_content_crc32 = "0".repeat(8),
        }
        assert!(
            preflight_xroar_launch(&f.request, &f.roots, &f.identity).is_err(),
            "{field}"
        );
        assert!(!f.report.exists());
    }
}

#[test]
fn matching_stat_cannot_hide_changed_cassette_bytes() {
    let f = fixture();
    let original = fs::metadata(&f.tape).unwrap().modified().unwrap();
    fs::write(&f.tape, vec![b'X'; TAPE.len()]).unwrap();
    File::open(&f.tape).unwrap().set_modified(original).unwrap();
    assert_eq!(
        capture_file_identity(&f.tape).unwrap().1,
        f.request.expected_content_identity
    );
    assert!(preflight_xroar_launch(&f.request, &f.roots, &f.identity).is_err());
}

#[test]
fn source_execute_permission_is_not_upgraded_or_ignored_on_retry() {
    let f = fixture();
    let prepared = preflight_xroar_launch(&f.request, &f.roots, &f.identity).unwrap();
    fs::set_permissions(&f.executable, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(spawn_xroar(&prepared).is_err());
    assert!(preflight_xroar_launch(&f.request, &f.roots, &f.identity).is_err());
    assert!(!f.report.exists());
    // Executable bits for another class do not authorize the owning user.
    if unsafe { libc::geteuid() } != 0 {
        fs::set_permissions(&f.executable, fs::Permissions::from_mode(0o655)).unwrap();
        assert!(preflight_xroar_launch(&f.request, &f.roots, &f.identity).is_err());
    }
}

#[test]
fn setuid_and_missing_explicit_rom_bindings_refuse() {
    let mut f = fixture();
    fs::set_permissions(&f.executable, fs::Permissions::from_mode(0o4755)).unwrap();
    assert!(preflight_xroar_launch(&f.request, &f.roots, &f.identity).is_err());
    fs::set_permissions(&f.executable, fs::Permissions::from_mode(0o755)).unwrap();
    f.roots.firmware_directory = None;
    assert!(preflight_xroar_launch(&f.request, &f.roots, &f.identity).is_err());
}

#[test]
fn closed_stdio_does_not_replace_bound_launch_assets() {
    const CHILD: &str = "EMUWIZ_XROAR_CLOSED_STDIO_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "launch::xroar_execution::tests::closed_stdio_does_not_replace_bound_launch_assets",
            ])
            .env(CHILD, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "closed-stdio child failed: {status}");
        return;
    }
    // This is a dedicated child; never close the parent test runner's stdio.
    for fd in 0..3 {
        unsafe {
            libc::close(fd);
        }
    }
    let f = fixture();
    let prepared = preflight_xroar_launch(&f.request, &f.roots, &f.identity).unwrap();
    wait(spawn_xroar(&prepared).unwrap());
    assert_original(&f);
    drop(prepared);
    drop(f);
    std::process::exit(0);
}

#[test]
fn dropping_gui_ownership_keeps_assets_until_the_watched_child_exits() {
    let f = fixture();
    let started = f._dir.path().join("started");
    let release = f._dir.path().join("release");
    let body = fs::read_to_string(&f.executable).unwrap().replacen("#!/bin/sh\n", &format!(
        "#!/bin/sh\n: > '{}'\ni=0\nwhile [ ! -f '{}' ] && [ \"$i\" -lt 200 ]; do /bin/sleep 0.01; i=$((i+1)); done\n",
        started.display(), release.display()), 1);
    fs::write(&f.executable, body).unwrap();
    let prepared = preflight_xroar_launch(&f.request, &f.roots, &f.identity).unwrap();
    let process = spawn_xroar(&prepared).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !started.exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    drop(prepared);
    drop(process);
    fs::write(release, b"continue").unwrap();
    while !fs::read(&f.report)
        .is_ok_and(|b| b.len() == b"original-executable\n".len() + TAPE.len() + 1)
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_original(&f);
}
