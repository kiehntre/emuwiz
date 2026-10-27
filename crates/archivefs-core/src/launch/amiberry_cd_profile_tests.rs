use super::*;
use crate::launch::planning::ResolvedIdentity;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};
struct Fixture {
    root: tempfile::TempDir,
    exe: PathBuf,
    profile: PathBuf,
    media: PathBuf,
    identity: CanonicalIdentityStatus,
}
fn fixture(model: &str, cue: bool) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let exe = root.path().join("amiberry");
    std::fs::write(
        &exe,
        b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.argv\"\necho fixture >&2\nexit 9\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let media = root.path().join(if cue {
        "disc with spaces.cue"
    } else {
        "disc with spaces.iso"
    });
    if cue {
        std::fs::write(
            &media,
            b"FILE \"track one.bin\" BINARY\n TRACK 01 MODE1/2352\n INDEX 01 00:00:00\n",
        )
        .unwrap();
        std::fs::write(root.path().join("track one.bin"), vec![0; 2352]).unwrap();
    } else {
        std::fs::write(&media, b"synthetic ISO, not a commercial game").unwrap();
    }
    std::fs::write(root.path().join("main.rom"), b"synthetic firmware fixture").unwrap();
    std::fs::write(
        root.path().join("extended.rom"),
        b"synthetic extended firmware fixture",
    )
    .unwrap();
    let profile = root.path().join("machine with spaces.uae");
    std::fs::write(&profile,format!("chipset_compatible={model}\ncdimage0={}\nkickstart_rom_file={}\nkickstart_ext_rom_file={}\n",media.display(),root.path().join("main.rom").display(),root.path().join("extended.rom").display())).unwrap();
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: if model == "CD32" {
            AMIGA_CD32_PLATFORM_ID
        } else {
            AMIGA_CDTV_PLATFORM_ID
        }
        .into(),
        game_key: "synthetic".into(),
    });
    Fixture {
        root,
        exe,
        profile,
        media,
        identity,
    }
}
fn review(f: &Fixture) -> ReviewedAmiberryCdProfile {
    review_amiberry_cd_profile(&f.exe, &f.profile, &f.media, &f.identity).unwrap()
}
#[test]
fn cd32_cdtv_iso_and_cue_use_existing_profile_command_and_preserve_sources() {
    for model in ["CD32", "CDTV"] {
        for cue in [false, true] {
            let f = fixture(model, cue);
            let r = review(&f);
            assert_eq!(r.request.machine_model, model);
            assert_eq!(r.request.media_dependencies.len(), usize::from(cue));
            let paths: Vec<_> = std::iter::once(&f.profile)
                .chain(std::iter::once(&f.media))
                .chain(std::iter::once(&r.request.firmware_main.path))
                .chain(std::iter::once(&r.request.firmware_extended.path))
                .chain(r.request.media_dependencies.iter().map(|b| &b.path))
                .map(|p| (p.clone(), std::fs::read(p).unwrap()))
                .collect();
            let mut process = r.launch(&f.identity).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while process.poll().is_none() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(
                process.poll().unwrap().status.as_ref().unwrap().code(),
                Some(9)
            );
            let args = std::fs::read_to_string(f.root.path().join("amiberry.argv")).unwrap();
            assert_eq!(args, format!("--config\n{}\n", f.profile.display()));
            for (path, bytes) in paths {
                assert_eq!(std::fs::read(path).unwrap(), bytes);
            }
        }
    }
}
#[test]
fn wrong_machine_wrong_disc_generic_identity_and_chd_are_refused() {
    let f = fixture("CD32", false);
    let wrong = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: AMIGA_CDTV_PLATFORM_ID.into(),
        game_key: "synthetic".into(),
    });
    assert!(
        review_amiberry_cd_profile(&f.exe, &f.profile, &f.media, &wrong)
            .unwrap_err()
            .contains("distinct machines")
    );
    assert!(
        review_amiberry_cd_profile(
            &f.exe,
            &f.profile,
            &f.media,
            &CanonicalIdentityStatus::Unknown
        )
        .is_err()
    );
    let other = f.root.path().join("other.iso");
    std::fs::write(&other, b"other").unwrap();
    assert!(
        review_amiberry_cd_profile(&f.exe, &f.profile, &other, &f.identity)
            .unwrap_err()
            .contains("different CD")
    );
}
#[test]
fn firmware_executable_profile_media_and_companion_changes_require_new_review() {
    for item in ["main", "extended", "exe", "profile", "media", "companion"] {
        let f = fixture("CD32", true);
        let r = review(&f);
        let path = match item {
            "main" => &r.request.firmware_main.path,
            "extended" => &r.request.firmware_extended.path,
            "exe" => &f.exe,
            "profile" => &f.profile,
            "media" => &f.media,
            _ => &r.request.media_dependencies[0].path,
        };
        std::fs::write(path, b"changed after approval").unwrap();
        assert!(r.launch(&f.identity).is_err(), "{item}");
        assert!(!f.root.path().join("amiberry.argv").exists());
    }
}
#[test]
fn unreviewed_dependencies_duplicate_keys_relative_paths_and_symlinks_fail_closed() {
    for extra in [
        "filesystem2=rw,DH0:/real/data",
        "lua=/tmp/script",
        "statefile=/tmp/state",
        "quickstart=CDTV,0",
        "chipset_compatible=CDTV",
        "cdimage0=relative.iso",
        "amiga_model=CD32",
    ] {
        let f = fixture("CD32", false);
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&f.profile)
                .unwrap(),
            "{extra}"
        )
        .unwrap();
        assert!(
            review_amiberry_cd_profile(&f.exe, &f.profile, &f.media, &f.identity).is_err(),
            "{extra}"
        );
    }
    let f = fixture("CD32", false);
    let linked = f.root.path().join("linked.uae");
    std::os::unix::fs::symlink(&f.profile, &linked).unwrap();
    assert!(review_amiberry_cd_profile(&f.exe, &linked, &f.media, &f.identity).is_err());
}
#[test]
fn missing_companion_and_firmware_are_named() {
    let f = fixture("CDTV", true);
    std::fs::remove_file(f.root.path().join("track one.bin")).unwrap();
    assert!(
        review_amiberry_cd_profile(&f.exe, &f.profile, &f.media, &f.identity)
            .unwrap_err()
            .contains("track one.bin")
    );
    let f = fixture("CD32", false);
    std::fs::remove_file(f.root.path().join("extended.rom")).unwrap();
    assert!(
        review_amiberry_cd_profile(&f.exe, &f.profile, &f.media, &f.identity)
            .unwrap_err()
            .contains("extended.rom")
    );
}

#[test]
fn native_key_casing_unicode_whitespace_controls_and_buffer_limits_refuse() {
    for (from, to) in [
        ("chipset_compatible", "CHIPSET_COMPATIBLE"),
        ("cdimage0", "CDIMAGE0"),
        ("kickstart_rom_file", "KICKSTART_ROM_FILE"),
        ("chipset_compatible", "\u{a0}chipset_compatible"),
    ] {
        let f = fixture("CD32", false);
        let text = std::fs::read_to_string(&f.profile).unwrap();
        std::fs::write(&f.profile, text.replace(from, to)).unwrap();
        assert!(
            review_amiberry_cd_profile(&f.exe, &f.profile, &f.media, &f.identity).is_err(),
            "{to}"
        );
    }
    for suffix in [
        format!("config_description={}\n", "x".repeat(2600)),
        "config_description=bad\0text\n".into(),
    ] {
        let f = fixture("CD32", false);
        use std::io::Write;
        write!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&f.profile)
                .unwrap(),
            "{suffix}"
        )
        .unwrap();
        assert!(review_amiberry_cd_profile(&f.exe, &f.profile, &f.media, &f.identity).is_err());
    }
    let f = fixture("CD32", false);
    let mut long = f.root.path().to_path_buf();
    for i in 0..6 {
        long = long.join(format!("segment{i}{}", "x".repeat(35)));
        std::fs::create_dir(&long).unwrap();
    }
    let media = long.join("disc.iso");
    std::fs::write(&media, b"synthetic").unwrap();
    let text = std::fs::read_to_string(&f.profile)
        .unwrap()
        .replace(f.media.to_str().unwrap(), media.to_str().unwrap());
    std::fs::write(&f.profile, text).unwrap();
    assert!(review_amiberry_cd_profile(&f.exe, &f.profile, &media, &f.identity).is_err());
}
