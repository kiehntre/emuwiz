use super::*;
use std::os::unix::fs::PermissionsExt;

fn machine(
    name: &str,
    status: &str,
    emulation: &str,
    extra_attrs: &str,
    inner: &str,
    driver_extra: &str,
) -> String {
    format!(
        r#"<mame build="0.264"><machine name="{name}" sourcefile="x/y.cpp" {extra_attrs}><description>{name}</description><driver status="{status}" emulation="{emulation}" {driver_extra}/>{inner}</machine><machine name="dev" isdevice="yes"><driver status="good"/></machine></mame>"#
    )
}

fn facts(xml: &str, set: &str) -> MameMachineFacts {
    parse_machine_facts(xml, set).unwrap().unwrap()
}

fn verify_text(text: &str, set: &str) -> VerifyReport {
    parse_verify_output(text, set)
}

fn good(set: &str) -> VerifyReport {
    verify_text(&format!("romset {set} is good\n"), set)
}

#[test]
fn working_and_complete_is_ready() {
    let f = facts(
        &machine("pac", "good", "good", "", r#"<control type="joy"/>"#, ""),
        "pac",
    );
    let v = evaluate_arcade_set("pac", Some(&f), &good("pac"), false);
    assert_eq!(v.readiness, ArcadeReadiness::Ready);
    assert!(v.warnings.is_empty() && v.blockers.is_empty());
}

#[test]
fn imperfect_is_ready_with_a_warning_naming_the_feature() {
    let f = facts(
        &machine(
            "gal",
            "imperfect",
            "good",
            "",
            r#"<feature type="sound" status="imperfect"/><feature type="graphics" status="unemulated"/>"#,
            "",
        ),
        "gal",
    );
    let v = evaluate_arcade_set("gal", Some(&f), &good("gal"), false);
    assert_eq!(v.readiness, ArcadeReadiness::ReadyWithWarning);
    assert!(
        v.warnings
            .iter()
            .any(|w| w.contains("sound") && w.contains("imperfect"))
    );
    assert!(
        v.warnings
            .iter()
            .any(|w| w.contains("graphics") && w.contains("not emulated"))
    );
}

#[test]
fn not_working_is_blocked_even_with_complete_roms() {
    for (status, emulation) in [("preliminary", "good"), ("good", "preliminary")] {
        let f = facts(&machine("bad", status, emulation, "", "", ""), "bad");
        let v = evaluate_arcade_set("bad", Some(&f), &good("bad"), false);
        assert_eq!(v.readiness, ArcadeReadiness::Blocked);
        assert_eq!(v.block_reason, Some(ArcadeBlockReason::NotWorking));
        assert!(v.blockers[0].contains("not working"));
    }
}

#[test]
fn incomplete_roms_block_and_list_the_missing_files() {
    let out = "romset blk is bad\nblk         : a.bin (16384 bytes) - NOT FOUND\nblk         : b.bin (16384 bytes) - NOT FOUND\n";
    let f = facts(&machine("blk", "good", "good", "", "", ""), "blk");
    let v = evaluate_arcade_set("blk", Some(&f), &verify_text(out, "blk"), false);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::RomsIncomplete));
    assert!(v.blockers[0].contains("a.bin") && v.blockers[0].contains("b.bin"));
}

#[test]
fn missing_bios_dependency_blocks_and_names_the_bios_set() {
    let out = "romset blk [parent] is bad\nblk         : ym2413_instruments.bin (144 bytes) - NOT FOUND (ym2413)\nblk         : sp-s2.sp1 (131072 bytes) - NOT FOUND (neogeo)\n";
    let f = facts(&machine("blk", "good", "good", "", "", ""), "blk");
    let v = evaluate_arcade_set("blk", Some(&f), &verify_text(out, "blk"), false);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::BiosOrDeviceMissing));
    assert!(v.blockers[0].contains("ym2413") && v.blockers[0].contains("neogeo"));
}

#[test]
fn wrong_version_and_no_own_files_and_unknown_set_are_distinct_blocks() {
    let f = facts(&machine("x", "good", "good", "", "", ""), "x");
    let wrong = "romset x is bad\nx           : a.bin (32768 bytes) - INCORRECT CHECKSUM:\nEXPECTED: CRC(11111111)\n   FOUND: CRC(22222222)\n";
    let v = evaluate_arcade_set("x", Some(&f), &verify_text(wrong, "x"), false);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::RomsWrongVersion));
    let v = evaluate_arcade_set(
        "x",
        Some(&f),
        &verify_text("romset \"x\" not found!\n", "x"),
        false,
    );
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::RomsIncomplete));
    assert!(v.blockers[0].contains("different MAME version"));
    let v = evaluate_arcade_set(
        "x",
        None,
        &verify_text("No matching systems found for 'x'\n", "x"),
        false,
    );
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::UnknownSet));
}

#[test]
fn best_available_is_ready_with_a_note_not_a_warning() {
    let f = facts(&machine("ba", "good", "good", "", "", ""), "ba");
    let v = evaluate_arcade_set(
        "ba",
        Some(&f),
        &verify_text("romset ba is best available\n", "ba"),
        false,
    );
    assert_eq!(v.readiness, ArcadeReadiness::Ready);
    assert!(v.warnings.is_empty() && !v.notes.is_empty());
}

#[test]
fn extra_rompath_fixture_is_ready_only_when_the_verify_result_says_so() {
    let f = facts(&machine("blk", "good", "good", "", "", ""), "blk");
    let before = "romset blk is bad\nblk         : ym2413_instruments.bin (144 bytes) - NOT FOUND (ym2413)\n";
    let after_ok = "romset blk is good\n";
    let after_still_bad = before;
    assert_eq!(
        evaluate_arcade_set("blk", Some(&f), &verify_text(before, "blk"), false).readiness,
        ArcadeReadiness::Blocked
    );
    assert_eq!(
        evaluate_arcade_set("blk", Some(&f), &verify_text(after_ok, "blk"), false).readiness,
        ArcadeReadiness::Ready
    );
    assert_eq!(
        evaluate_arcade_set("blk", Some(&f), &verify_text(after_still_bad, "blk"), false).readiness,
        ArcadeReadiness::Blocked
    );
}

#[test]
fn category_is_never_a_gate() {
    let video = facts(
        &machine("v", "good", "good", "", r#"<control type="joy"/>"#, ""),
        "v",
    );
    let fruit = facts(
        &machine(
            "f",
            "good",
            "good",
            r#"ismechanical="yes""#,
            r#"<control type="gambling"/>"#,
            "",
        ),
        "f",
    );
    let poker = facts(
        &machine("p", "good", "good", "", r#"<control type="gambling"/>"#, ""),
        "p",
    );
    let video_v = evaluate_arcade_set("v", Some(&video), &good("v"), false);
    let fruit_v = evaluate_arcade_set("f", Some(&fruit), &good("f"), false);
    let poker_v = evaluate_arcade_set("p", Some(&poker), &good("p"), false);
    assert_eq!(video_v.readiness, ArcadeReadiness::Ready);
    // Same MAME status and verify result: still playable, never blocked; the
    // category only adds an informational warning.
    for v in [&fruit_v, &poker_v] {
        assert_ne!(v.readiness, ArcadeReadiness::Blocked);
        assert!(v.block_reason.is_none());
        assert!(
            v.warnings
                .iter()
                .any(|w| w.contains("gambling/fruit machine"))
        );
    }
    // And the same blockers apply to every category alike.
    for (name, f) in [("v", &video), ("f", &fruit), ("p", &poker)] {
        let nw = MameMachineFacts {
            status: "preliminary".into(),
            ..f.clone()
        };
        assert_eq!(
            evaluate_arcade_set(name, Some(&nw), &good(name), false).block_reason,
            Some(ArcadeBlockReason::NotWorking)
        );
    }
}

#[test]
fn lightgun_and_mahjong_get_special_controls_warnings() {
    let gun = facts(
        &machine("g", "good", "good", "", r#"<control type="lightgun"/>"#, ""),
        "g",
    );
    let mj = facts(
        &machine("m", "good", "good", "", r#"<control type="mahjong"/>"#, ""),
        "m",
    );
    let g = evaluate_arcade_set("g", Some(&gun), &good("g"), false);
    let m = evaluate_arcade_set("m", Some(&mj), &good("m"), false);
    assert_eq!(g.readiness, ArcadeReadiness::ReadyWithWarning);
    assert!(g.warnings[0].contains("lightgun"));
    assert_eq!(m.readiness, ArcadeReadiness::ReadyWithWarning);
    assert!(m.warnings[0].contains("mahjong"));
}

#[test]
fn required_artwork_blocks_only_when_absent() {
    let f = facts(
        &machine("art", "good", "good", "", "", r#"requiresartwork="yes""#),
        "art",
    );
    assert!(f.requires_artwork);
    let blocked = evaluate_arcade_set("art", Some(&f), &good("art"), false);
    assert_eq!(
        blocked.block_reason,
        Some(ArcadeBlockReason::ArtworkMissing)
    );
    let ok = evaluate_arcade_set("art", Some(&f), &good("art"), true);
    assert_eq!(ok.readiness, ArcadeReadiness::Ready);
}

#[test]
fn artwork_presence_looks_only_in_the_set_folder() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!set_folder_has_artwork(dir.path()));
    std::fs::write(dir.path().join("default.lay"), b"x").unwrap();
    assert!(set_folder_has_artwork(dir.path()));
}

#[test]
fn bios_and_device_sets_are_not_games() {
    let f = facts(
        &machine(
            "neogeo",
            "good",
            "good",
            r#"isbios="yes" runnable="no""#,
            "",
            "",
        ),
        "neogeo",
    );
    let v = evaluate_arcade_set("neogeo", Some(&f), &good("neogeo"), false);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::NotAGame));
}

#[test]
fn parser_reads_only_the_named_machine_and_its_controls() {
    let f = facts(
        &machine(
            "pac",
            "imperfect",
            "good",
            r#"ismechanical="yes""#,
            r#"<display type="raster"/><input players="1"><control type="joy"/><control type="joy"/></input>"#,
            "",
        ),
        "pac",
    );
    assert!(f.is_mechanical && f.has_display && f.runnable);
    assert_eq!(f.control_types, vec!["joy".to_string()]);
    assert!(parse_machine_facts("<mame/>", "zz").unwrap().is_none());
}

#[test]
fn extra_rompath_validation_refuses_bad_paths() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f");
    std::fs::write(&file, b"x").unwrap();
    let loop_a = dir.path().join("a");
    let loop_b = dir.path().join("b");
    std::os::unix::fs::symlink(&loop_b, &loop_a).unwrap();
    std::os::unix::fs::symlink(&loop_a, &loop_b).unwrap();
    assert!(matches!(
        validate_extra_rompath(Path::new("relative/dir")),
        Err(ExtraRompathError::NotAbsolute(_))
    ));
    assert!(matches!(
        validate_extra_rompath(&dir.path().join("nope")),
        Err(ExtraRompathError::Missing(_))
    ));
    assert!(matches!(
        validate_extra_rompath(&file),
        Err(ExtraRompathError::NotDirectory(_))
    ));
    assert!(matches!(
        validate_extra_rompath(&loop_a),
        Err(ExtraRompathError::Unresolvable(_))
    ));
    assert!(matches!(
        validate_extra_rompath(Path::new("/tmp/a;b")),
        Err(ExtraRompathError::ContainsSeparator(_))
    ));
}

#[test]
fn extra_rompath_is_canonicalised_and_default_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("bios");
    std::fs::create_dir(&real).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let (good, bad) = validate_extra_rompaths(&[link, real.clone()]);
    assert!(bad.is_empty());
    assert_eq!(good, vec![std::fs::canonicalize(&real).unwrap()]);
    assert!(validate_extra_rompaths(&[]).0.is_empty());
    assert!(read_extra_rompath_file(&dir.path().join("absent.txt")).is_empty());
    let list = dir.path().join("l.txt");
    std::fs::write(&list, "# c\n\n/a\n /b \n").unwrap();
    assert_eq!(
        read_extra_rompath_file(&list),
        vec![PathBuf::from("/a"), PathBuf::from("/b")]
    );
}

#[test]
fn rompath_argument_puts_the_set_parent_first() {
    let arg = rompath_argument(
        Path::new("/lib/arcade"),
        &[PathBuf::from("/bios/a"), PathBuf::from("/bios/b")],
    );
    assert_eq!(arg, OsString::from("/lib/arcade;/bios/a;/bios/b"));
    assert_eq!(
        rompath_argument(Path::new("/lib/arcade"), &[]),
        OsString::from("/lib/arcade")
    );
}

fn fake_mame(dir: &Path) -> PathBuf {
    // A stand-in for MAME: "good" only when the rompath carries an extra
    // entry; prints a one-machine listxml.
    let path = dir.join("fake-mame");
    std::fs::write(
        &path,
        r#"#!/bin/sh
case "$*" in
  *-listxml*) echo '<mame><machine name="blk"><driver status="good" emulation="good"/></machine></mame>' ;;
  *-verifyroms*)
    case "$*" in
      *";"*) echo "romset blk is good" ;;
      *) echo "romset blk is bad"; echo "blk         : ym2413_instruments.bin (144 bytes) - NOT FOUND (ym2413)" ;;
    esac ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn probe_passes_the_extra_rompath_and_readiness_follows_the_verify_result() {
    let dir = tempfile::tempdir().unwrap();
    let exe = fake_mame(dir.path());
    let arcade = dir.path().join("arcade");
    let set_dir = arcade.join("blk");
    std::fs::create_dir_all(&set_dir).unwrap();
    let bios = dir.path().join("bios");
    std::fs::create_dir(&bios).unwrap();
    let cancel = AtomicBool::new(false);
    let none = probe_arcade_set(&exe, "blk", &set_dir, &[], Duration::from_secs(10), &cancel);
    assert_eq!(
        none.verdict.block_reason,
        Some(ArcadeBlockReason::BiosOrDeviceMissing)
    );
    let with = probe_arcade_set(
        &exe,
        "blk",
        &set_dir,
        std::slice::from_ref(&bios),
        Duration::from_secs(10),
        &cancel,
    );
    assert_eq!(with.verdict.readiness, ArcadeReadiness::Ready);
    // An invalid extra path is dropped, so it cannot unblock anything.
    let bad = probe_arcade_set(
        &exe,
        "blk",
        &set_dir,
        &[dir.path().join("missing")],
        Duration::from_secs(10),
        &cancel,
    );
    assert_eq!(bad.verdict.readiness, ArcadeReadiness::Blocked);
    // Nothing was written into the collection or the BIOS folder.
    assert_eq!(std::fs::read_dir(&bios).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&set_dir).unwrap().count(), 0);
}

#[test]
fn probe_is_bounded_and_cancellable() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("slow");
    std::fs::write(&exe, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let set_dir = dir.path().join("arcade").join("x");
    std::fs::create_dir_all(&set_dir).unwrap();
    let started = Instant::now();
    let probe = probe_arcade_set(
        &exe,
        "x",
        &set_dir,
        &[],
        Duration::from_millis(200),
        &AtomicBool::new(false),
    );
    assert_eq!(
        probe.verdict.block_reason,
        Some(ArcadeBlockReason::CheckFailed)
    );
    let probe = probe_arcade_set(
        &exe,
        "x",
        &set_dir,
        &[],
        Duration::from_secs(30),
        &AtomicBool::new(true),
    );
    assert_eq!(
        probe.verdict.block_reason,
        Some(ArcadeBlockReason::CheckFailed)
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    let unsafe_name = probe_arcade_set(
        &exe,
        "../x",
        &set_dir,
        &[],
        Duration::from_secs(1),
        &AtomicBool::new(false),
    );
    assert_eq!(
        unsafe_name.verdict.block_reason,
        Some(ArcadeBlockReason::CheckFailed)
    );
}
