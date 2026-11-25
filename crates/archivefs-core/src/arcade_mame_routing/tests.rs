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
                .any(|w| w.contains("gambling/AWP machine"))
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
fn artwork_blocks_only_demonstrated_unusable_sets_and_only_until_installed() {
    for set in ["solarwar", "videopin", "triplhnt", "m4conn4"] {
        let f = facts(
            &machine(set, "good", "good", "", "", r#"requiresartwork="yes""#),
            set,
        );
        let blocked = evaluate_arcade_set(set, Some(&f), &good(set), false);
        assert_eq!(
            blocked.block_reason,
            Some(ArcadeBlockReason::ArtworkMissing),
            "{set}"
        );
        assert!(blocked.blockers[0].contains("requires external artwork, not installed"));
        // Data-driven: once artwork is located the set is no longer blocked.
        let ok = evaluate_arcade_set(set, Some(&f), &good(set), true);
        assert_ne!(ok.readiness, ArcadeReadiness::Blocked);
    }
    // gollygho runs without it: Play with a warning naming artwork + lightgun.
    let f = facts(
        &machine(
            "gollygho",
            "good",
            "good",
            "",
            r#"<control type="lightgun"/>"#,
            r#"requiresartwork="yes""#,
        ),
        "gollygho",
    );
    let v = evaluate_arcade_set("gollygho", Some(&f), &good("gollygho"), false);
    assert_eq!(v.readiness, ArcadeReadiness::ReadyWithWarning);
    assert!(v.warnings.iter().any(|w| w.contains("artwork")));
    assert!(v.warnings.iter().any(|w| w.contains("lightgun")));
    // An unlisted requiresartwork set is not auto-blocked.
    let f = facts(
        &machine("other", "good", "good", "", "", r#"requiresartwork="yes""#),
        "other",
    );
    let v = evaluate_arcade_set("other", Some(&f), &good("other"), false);
    assert_eq!(v.readiness, ArcadeReadiness::ReadyWithWarning);
    assert!(v.warnings[0].contains("may need external artwork"));
}

#[test]
fn wrong_dump_only_follows_demonstrated_outcomes_and_otherwise_blocks_naming_the_rom() {
    let text = |set: &str, file: &str, size: u64| {
        format!(
            "romset {set} is bad\n{set:<12}: {file} ({size} bytes) - INCORRECT CHECKSUM:\nEXPECTED: CRC(1)\n   FOUND: CRC(2)\n"
        )
    };
    for (set, file) in [
        ("3wonders", "ioc1.ic1"),
        ("1943b", "bm05.4k"),
        ("gauntletr4", "136037-104.6p"),
        ("sf2cejc", "ioc1.ic7"),
    ] {
        let f = facts(&machine(set, "good", "good", "", "", ""), set);
        let v = evaluate_arcade_set(
            set,
            Some(&f),
            &verify_text(&text(set, file, 260), set),
            false,
        );
        assert_eq!(v.readiness, ArcadeReadiness::ReadyWithWarning, "{set}");
        assert!(v.warnings[0].contains(file) && v.warnings[0].contains("notice"));
    }
    for (set, file) in [("sailormnot", "smprg.u45"), ("island2a", "m48t35")] {
        let f = facts(&machine(set, "good", "good", "", "", ""), set);
        let v = evaluate_arcade_set(
            set,
            Some(&f),
            &verify_text(&text(set, file, 524288), set),
            false,
        );
        assert_eq!(
            v.block_reason,
            Some(ArcadeBlockReason::RomsWrongVersion),
            "{set}"
        );
        assert!(v.blockers[0].contains(file));
    }
    // No demonstrated outcome: blocked even for a tiny file (size is not proof).
    let f = facts(&machine("nodata", "good", "good", "", "", ""), "nodata");
    let v = evaluate_arcade_set(
        "nodata",
        Some(&f),
        &verify_text(&text("nodata", "ioc1.ic1", 260), "nodata"),
        false,
    );
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::RomsWrongVersion));
    assert!(v.blockers[0].contains("ioc1.ic1") && v.blockers[0].contains("may refuse"));
}

#[test]
fn known_failed_sets_are_never_called_playable_whatever_their_category() {
    let f = facts(
        &machine(
            "cluedo",
            "good",
            "good",
            "",
            r#"<control type="gambling"/>"#,
            "",
        ),
        "cluedo",
    );
    let v = evaluate_arcade_set("cluedo", Some(&f), &good("cluedo"), true);
    assert_eq!(v.readiness, ArcadeReadiness::Blocked);
    assert!(v.blockers[0].contains("black screen"));
}

#[test]
fn chd_lines_without_byte_counts_and_mixed_problems_are_all_named() {
    let out = "romset a51 [parent] is bad\n\
a51         : a51site4-2_01 - NOT FOUND\n\
a51         : lindbios.bin (1024 bytes) - NOT FOUND (lindbios)\n\
a51         : sound_roms - NOT FOUND (lindbios)\n\
a51         : w.bin (10 bytes) - INCORRECT LENGTH: 5 bytes\n\
a51         : z.bin (10 bytes) - INCORRECT CHECKSUM:\nEXPECTED: CRC(1)\n   FOUND: CRC(2)\n";
    let report = verify_text(out, "a51");
    assert_eq!(report.problems.len(), 5);
    assert!(report.problems[0].is_disk && report.problems[0].file == "a51site4-2_01");
    let f = facts(&machine("a51", "good", "good", "", "", ""), "a51");
    let v = evaluate_arcade_set("a51", Some(&f), &report, false);
    let text = &v.blockers[0];
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::RomsIncomplete));
    assert!(text.contains("disk image a51site4-2_01"));
    assert!(text.contains("lindbios"));
    assert!(
        text.contains("w.bin has the wrong length")
            && text.contains("z.bin has an incorrect checksum")
    );
}

#[test]
fn dependency_plus_wrong_dump_lists_both() {
    let out = "romset q is bad\nq           : b.bin (1 bytes) - NOT FOUND (neogeo)\nq           : w.bin (10 bytes) - INCORRECT CHECKSUM:\n";
    let f = facts(&machine("q", "good", "good", "", "", ""), "q");
    let v = evaluate_arcade_set("q", Some(&f), &verify_text(out, "q"), false);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::BiosOrDeviceMissing));
    assert!(v.blockers[0].contains("neogeo") && v.blockers[0].contains("w.bin"));
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
    // Each probe is traced so an intermittent failure (issue #479) reports
    // why: spawn error with its raw OS error, timeout, or MAME's output.
    let (none, trace) =
        probe_arcade_set_traced(&exe, "blk", &set_dir, &[], Duration::from_secs(10), &cancel);
    assert_eq!(
        none.verdict.block_reason,
        Some(ArcadeBlockReason::BiosOrDeviceMissing),
        "probe without the extra rompath: {none:#?}\n{trace:#?}"
    );
    let (with, trace) = probe_arcade_set_traced(
        &exe,
        "blk",
        &set_dir,
        std::slice::from_ref(&bios),
        Duration::from_secs(10),
        &cancel,
    );
    assert_eq!(
        with.verdict.readiness,
        ArcadeReadiness::Ready,
        "probe with the extra rompath: {with:#?}\n{trace:#?}"
    );
    // An invalid extra path is dropped, so it cannot unblock anything.
    let (bad, trace) = probe_arcade_set_traced(
        &exe,
        "blk",
        &set_dir,
        &[dir.path().join("missing")],
        Duration::from_secs(10),
        &cancel,
    );
    assert_eq!(
        bad.verdict.readiness,
        ArcadeReadiness::Blocked,
        "probe with an invalid extra rompath: {bad:#?}\n{trace:#?}"
    );
    // Nothing was written into the collection or the BIOS folder.
    assert_eq!(std::fs::read_dir(&bios).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&set_dir).unwrap().count(), 0);
}

// The diagnostic tests below write no new scripts. A freshly written script
// executed at once is the pattern of the intermittent failure in issue #479
// (cause unconfirmed), so they use a missing path, /bin/false, or /bin/sh -c.

#[test]
fn the_traced_probe_returns_exactly_the_production_probe() {
    let dir = tempfile::tempdir().unwrap();
    let set_dir = dir.path().join("arcade").join("blk");
    std::fs::create_dir_all(&set_dir).unwrap();
    let cancel = AtomicBool::new(false);
    let missing = dir.path().join("no-such-mame");
    for exe in [Path::new("/bin/false"), missing.as_path()] {
        let plain = probe_arcade_set(exe, "blk", &set_dir, &[], Duration::from_secs(10), &cancel);
        let (traced, trace) =
            probe_arcade_set_traced(exe, "blk", &set_dir, &[], Duration::from_secs(10), &cancel);
        assert_eq!(plain, traced, "{exe:?}: {trace:#?}");
    }
}

#[test]
fn a_spawn_failure_keeps_the_raw_os_error_and_the_same_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let set_dir = dir.path().join("arcade").join("blk");
    std::fs::create_dir_all(&set_dir).unwrap();
    let (probe, trace) = probe_arcade_set_traced(
        &dir.path().join("no-such-mame"),
        "blk",
        &set_dir,
        &[],
        Duration::from_secs(5),
        &AtomicBool::new(false),
    );
    assert_eq!(
        probe.verdict.block_reason,
        Some(ArcadeBlockReason::CheckFailed)
    );
    assert_eq!(
        probe.verdict.blockers,
        vec!["MAME's ROM check did not finish (timed out or cancelled)".to_string()]
    );
    match &trace.verify_run {
        Some(Err(RunFailure::Spawn(error))) => {
            assert_eq!(error.raw_os_error(), Some(libc::ENOENT), "{error:?}");
        }
        other => panic!("expected a spawn failure, got {other:#?}"),
    }
    assert!(trace.listxml_run.is_none(), "{trace:#?}");
}

#[test]
fn run_bounded_tells_timeout_and_cancel_apart() {
    let sh = Path::new("/bin/sh");
    let args = [OsString::from("-c"), OsString::from("sleep 30")];
    let started = Instant::now();
    match run_bounded(
        sh,
        &args,
        Duration::from_millis(200),
        &AtomicBool::new(false),
    ) {
        Err(RunFailure::TimedOut { after }) => assert!(after >= Duration::from_millis(200)),
        other => panic!("expected a timeout, got {other:#?}"),
    }
    match run_bounded(sh, &args, Duration::from_secs(30), &AtomicBool::new(true)) {
        Err(RunFailure::Cancelled { .. }) => {}
        other => panic!("expected a cancellation, got {other:#?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn unrecognised_verify_output_is_captured_and_still_blocks_as_check_failed() {
    let args = [
        OsString::from("-c"),
        OsString::from("echo 'not a verify answer'"),
    ];
    let output = match run_bounded(
        Path::new("/bin/sh"),
        &args,
        Duration::from_secs(10),
        &AtomicBool::new(false),
    ) {
        Ok(output) => output,
        other => panic!("expected a finished run, got {other:#?}"),
    };
    assert!(output.status.success(), "{output:#?}");
    assert_eq!(output.text, "not a verify answer\n");
    let verify = parse_verify_output(&output.text, "x");
    assert_eq!(verify.state, VerifyState::Unrecognised);
    let facts = facts(
        r#"<mame><machine name="x"><driver status="good" emulation="good"/></machine></mame>"#,
        "x",
    );
    let verdict = evaluate_arcade_set("x", Some(&facts), &verify, false);
    assert_eq!(verdict.block_reason, Some(ArcadeBlockReason::CheckFailed));
    assert_eq!(
        verdict.blockers,
        vec!["MAME's ROM check for x gave no usable answer".to_string()]
    );
}

#[test]
fn parent_folder_with_a_semicolon_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let exe = fake_mame(dir.path());
    let set_dir = dir.path().join("a;b").join("blk");
    std::fs::create_dir_all(&set_dir).unwrap();
    let probe = probe_arcade_set(
        &exe,
        "blk",
        &set_dir,
        &[],
        Duration::from_secs(5),
        &AtomicBool::new(false),
    );
    assert_eq!(
        probe.verdict.block_reason,
        Some(ArcadeBlockReason::CheckFailed)
    );
    assert!(probe.verdict.blockers[0].contains("';'"));
}

#[test]
fn a_grandchild_holding_the_pipe_does_not_hang_the_probe() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("leaky");
    std::fs::write(
        &exe,
        "#!/bin/sh\ncase \"$*\" in *-listxml*) echo '<mame><machine name=\"x\"><driver status=\"good\" emulation=\"good\"/></machine></mame>';; *) echo 'romset x is good';; esac\n(sleep 30 &)\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let set_dir = dir.path().join("arcade").join("x");
    std::fs::create_dir_all(&set_dir).unwrap();
    let started = Instant::now();
    let probe = probe_arcade_set(
        &exe,
        "x",
        &set_dir,
        &[],
        Duration::from_secs(20),
        &AtomicBool::new(false),
    );
    assert_eq!(probe.verdict.readiness, ArcadeReadiness::Ready);
    assert!(started.elapsed() < Duration::from_secs(10));
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

#[test]
fn wrong_dump_evidence_applies_only_to_the_evidenced_bad_file() {
    let report = |set: &str, files: &[(&str, u64)]| {
        let mut out = format!("romset {set} is bad\n");
        for (file, size) in files {
            out.push_str(&format!(
                "{set:<12}: {file} ({size} bytes) - INCORRECT CHECKSUM:\nEXPECTED: CRC(1)\n   FOUND: CRC(2)\n"
            ));
        }
        verify_text(&out, set)
    };
    let f = facts(&machine("3wonders", "good", "good", "", "", ""), "3wonders");
    // The evidenced PLD: warning.
    let ok = evaluate_arcade_set(
        "3wonders",
        Some(&f),
        &report("3wonders", &[("ioc1.ic1", 260)]),
        false,
    );
    assert_eq!(ok.readiness, ArcadeReadiness::ReadyWithWarning);
    // A different bad file (a program ROM) in the same set: Blocked.
    let other = evaluate_arcade_set(
        "3wonders",
        Some(&f),
        &report("3wonders", &[("prg.bin", 524288)]),
        false,
    );
    assert_eq!(
        other.block_reason,
        Some(ArcadeBlockReason::RomsWrongVersion)
    );
    assert!(other.blockers[0].contains("prg.bin"));
    // The evidenced PLD plus an extra bad file: Blocked.
    let both = evaluate_arcade_set(
        "3wonders",
        Some(&f),
        &report("3wonders", &[("ioc1.ic1", 260), ("prg.bin", 524288)]),
        false,
    );
    assert_eq!(both.readiness, ArcadeReadiness::Blocked);
}

fn neogeo_style(set: &str) -> MameMachineFacts {
    // Two BIOS versions; "euro" is the first and therefore the default.
    let inner = r#"<biosset name="euro" description="Europe"/><biosset name="asia" description="Asia"/><rom name="sp-s2.sp1" bios="euro" size="131072"/><rom name="sp-s3.sp1" bios="asia" size="131072"/><rom name="000-lo.lo" size="131072"/><rom name="044-p1.p1" size="1048576"/>"#;
    facts(
        &machine(set, "good", "good", r#"romof="neogeo""#, inner, ""),
        set,
    )
}

#[test]
fn files_for_other_bios_versions_only_never_block() {
    let f = neogeo_style("aofx");
    assert_eq!(names(&f), vec!["sp-s3.sp1"]);
    let out = "aofx        : sp-s3.sp1 (131072 bytes) - NOT FOUND (neogeo)\nromset aofx [neogeo] is bad\n";
    let v = evaluate_arcade_set("aofx", Some(&f), &verify_text(out, "aofx"), false);
    assert_eq!(v.block_reason, None);
    assert_ne!(v.readiness, ArcadeReadiness::Blocked);
    assert!(
        v.warnings
            .iter()
            .any(|w| w.contains("sp-s3.sp1") && w.contains("default BIOS"))
    );
}

#[test]
fn a_missing_default_bios_file_still_blocks() {
    let f = neogeo_style("aofx");
    let out = "aofx        : sp-s2.sp1 (131072 bytes) - NOT FOUND (neogeo)\naofx        : sp-s3.sp1 (131072 bytes) - NOT FOUND (neogeo)\nromset aofx [neogeo] is bad\n";
    let v = evaluate_arcade_set("aofx", Some(&f), &verify_text(out, "aofx"), false);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::BiosOrDeviceMissing));
}

#[test]
fn other_missing_files_still_block_when_alternative_bios_files_are_also_missing() {
    let f = neogeo_style("aofx");
    let out = "aofx        : sp-s3.sp1 (131072 bytes) - NOT FOUND (neogeo)\naofx        : 044-p1.p1 (1048576 bytes) - NOT FOUND\nromset aofx [neogeo] is bad\n";
    let v = evaluate_arcade_set("aofx", Some(&f), &verify_text(out, "aofx"), false);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::RomsIncomplete));
    assert!(v.blockers[0].contains("044-p1.p1"));
}

#[test]
fn a_file_shared_with_the_default_bios_version_is_never_optional() {
    let inner = r#"<biosset name="a" default="yes"/><biosset name="b"/><rom name="shared.bin" bios="a"/><rom name="shared.bin" bios="b"/><rom name="only_b.bin" bios="b"/>"#;
    let f = facts(&machine("m", "good", "good", "", inner, ""), "m");
    assert_eq!(names(&f), vec!["only_b.bin"]);
}

#[test]
fn an_explicit_default_that_is_not_first_is_the_one_kept_required() {
    let inner = r#"<biosset name="old"/><biosset name="new" default="yes"/><rom name="old.bin" bios="old"/><rom name="new.bin" bios="new"/>"#;
    let f = facts(&machine("m", "good", "good", "", inner, ""), "m");
    assert_eq!(names(&f), vec!["old.bin"]);
}

#[test]
fn a_file_naming_an_undeclared_bios_version_makes_nothing_optional() {
    let inner = r#"<biosset name="a" default="yes"/><biosset name="b"/><rom name="b.bin" bios="b"/><rom name="ghost.bin" bios="zz"/>"#;
    let f = facts(&machine("m", "good", "good", "", inner, ""), "m");
    assert!(f.alternative_bios_files.is_empty());
}

#[test]
fn truncated_machine_data_makes_nothing_optional_and_still_blocks() {
    // Output cut off after the alternative entry: the default entry for
    // sp-s2.sp1 may simply be missing from what was read.
    let xml = r#"<mame build="0.264"><machine name="aofx" romof="neogeo"><driver status="good" emulation="good"/><biosset name="euro"/><biosset name="asia"/><rom name="sp-s3.sp1" bios="asia" size="131072"/>"#;
    let f = facts(xml, "aofx");
    assert!(f.alternative_bios_files.is_empty());
    let out = "aofx        : sp-s3.sp1 (131072 bytes) - NOT FOUND (neogeo)\nromset aofx [neogeo] is bad\n";
    let v = evaluate_arcade_set("aofx", Some(&f), &verify_text(out, "aofx"), false);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::BiosOrDeviceMissing));
}

fn names(f: &MameMachineFacts) -> Vec<&str> {
    f.alternative_bios_files
        .iter()
        .map(|a| a.name.as_str())
        .collect()
}

#[test]
fn an_unrecognised_problem_still_blocks_when_alternative_bios_files_are_missing() {
    let f = neogeo_style("aofx");
    let out = "aofx        : sp-s3.sp1 (131072 bytes) - NOT FOUND (neogeo)\naofx        : 044-p1.p1 (1048576 bytes) - NEEDS REDUMP\nromset aofx [neogeo] is bad\n";
    let v = evaluate_arcade_set("aofx", Some(&f), &verify_text(out, "aofx"), false);
    assert_eq!(v.readiness, ArcadeReadiness::Blocked);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::RomsIncomplete));
}

#[test]
fn a_no_good_dump_line_alongside_alternative_bios_files_is_still_ready() {
    // MAME's own "best available" case: no good dump is known for a part.
    let f = neogeo_style("aofx");
    let out = "aofx        : sp-s3.sp1 (131072 bytes) - NOT FOUND (neogeo)\naofx        : 044-p1.p1 (1048576 bytes) - NOT FOUND - NO GOOD DUMP KNOWN\nromset aofx [neogeo] is bad\n";
    let v = evaluate_arcade_set("aofx", Some(&f), &verify_text(out, "aofx"), false);
    assert_eq!(v.block_reason, None);
}

#[test]
fn a_same_named_file_of_another_size_or_a_disk_is_never_exempted() {
    let f = neogeo_style("aofx");
    for line in [
        "aofx        : sp-s3.sp1 (65536 bytes) - NOT FOUND (somedevice)",
        "aofx        : sp-s3.sp1 - NOT FOUND",
    ] {
        let out = format!("{line}\nromset aofx [neogeo] is bad\n");
        let v = evaluate_arcade_set("aofx", Some(&f), &verify_text(&out, "aofx"), false);
        assert_eq!(v.readiness, ArcadeReadiness::Blocked, "{line}");
    }
}

#[test]
fn a_device_rom_with_the_same_name_and_size_is_never_exempted() {
    // Same name and size as the "asia" BIOS file, but MAME attributes the
    // missing file to a device this machine uses: the device needs it.
    let inner = r#"<biosset name="euro"/><biosset name="asia"/><rom name="sp-s2.sp1" bios="euro" size="131072"/><rom name="sp-s3.sp1" bios="asia" size="131072"/><device_ref name="z80"/><device_ref name="coincidence"/>"#;
    let f = facts(&machine("devx", "good", "good", "", inner, ""), "devx");
    assert_eq!(names(&f), vec!["sp-s3.sp1"]);
    assert_eq!(f.device_refs, vec!["z80", "coincidence"]);
    let out =
        "devx        : sp-s3.sp1 (131072 bytes) - NOT FOUND (coincidence)\nromset devx is bad\n";
    let v = evaluate_arcade_set("devx", Some(&f), &verify_text(out, "devx"), false);
    assert_eq!(v.readiness, ArcadeReadiness::Blocked);
    assert_eq!(v.block_reason, Some(ArcadeBlockReason::BiosOrDeviceMissing));
}

#[test]
fn no_bios_versions_a_self_closing_target_or_a_read_error_make_nothing_optional() {
    let none = facts(
        &machine(
            "m",
            "good",
            "good",
            "",
            r#"<rom name="x.bin" size="4"/>"#,
            "",
        ),
        "m",
    );
    assert!(none.alternative_bios_files.is_empty());
    let self_closing = r#"<mame><machine name="m"/><machine name="n"><biosset name="a"/><biosset name="b"/><rom name="b.bin" bios="b" size="4"/></machine></mame>"#;
    assert!(facts(self_closing, "m").alternative_bios_files.is_empty());
    // Malformed markup after the target's BIOS data: facts may survive the
    // read error, but nothing is optional.
    let broken = r#"<mame><machine name="m"><driver status="good" emulation="good"/><biosset name="a"/><biosset name="b"/><rom name="b.bin" bios="b" size="4"/><rom name="c.bin" </machine></mame>"#;
    if let Ok(Some(f)) = parse_machine_facts(broken, "m") {
        assert!(f.alternative_bios_files.is_empty());
    }
}

// --- Effective BIOS version (C4) -------------------------------------------

/// Neo Geo-like set: two BIOS versions ("euro" default), BIOS root `neogeo`,
/// one horizontal raster screen, driver source `x/y.cpp`.
fn bios_set(name: &str, extra_attrs: &str) -> MameMachineFacts {
    let inner = r#"<biosset name="euro"/><biosset name="us"/><rom name="sp-s2.sp1" bios="euro" size="131072"/><rom name="sp-u2.sp1" bios="us" size="131072"/><rom name="p1.p1" size="1048576"/><display type="raster" rotate="0"/>"#;
    facts(&machine(name, "good", "good", extra_attrs, inner, ""), name)
}

fn settings(global: &str, files: &[(&str, &str)]) -> BiosSettings {
    BiosSettings {
        global: global.into(),
        files: files
            .iter()
            .map(|(name, bios)| {
                (
                    name.to_string(),
                    (
                        PathBuf::from(format!("/ini/{name}.ini")),
                        Some(bios.to_string()),
                    ),
                )
            })
            .collect(),
        changes_reading: Default::default(),
        evidence: BiosSettingsEvidence::default(),
    }
}

#[test]
fn parentage_bios_versions_and_screens_are_recorded() {
    let f = bios_set("kof98a", r#"cloneof="kof98" romof="kof98""#);
    assert_eq!(f.clone_of.as_deref(), Some("kof98"));
    assert_eq!(f.rom_of.as_deref(), Some("kof98"));
    assert_eq!(f.bios_versions, vec!["euro", "us"]);
    assert_eq!(f.default_bios.as_deref(), Some("euro"));
    assert_eq!(f.displays, vec![("raster".to_string(), "0".to_string())]);
}

#[test]
fn only_settings_that_reach_this_set_decide_its_bios_version() {
    let aof = bios_set("aof", r#"romof="neogeo""#);
    let neo = ["neogeo".to_string()];
    let ok = |s: &BiosSettings| effective_bios_check(&aof, &neo, s);
    // Nothing set, or set only for other sets or other screen kinds.
    assert_eq!(ok(&settings("", &[])), Ok(()));
    assert_eq!(ok(&settings("", &[("sf2", "us"), ("AOF", "us")])), Ok(()));
    assert_eq!(
        ok(&settings("", &[("vertical", "us"), ("vector", "us")])),
        Ok(())
    );
    assert_eq!(ok(&settings("", &[("aof", "")])), Ok(()));
    // The default named explicitly, or a name this set does not declare
    // (MAME reverts to the default).
    assert_eq!(ok(&settings("", &[("aof", "EURO")])), Ok(()));
    assert_eq!(ok(&settings("unibios40", &[])), Ok(()));
    // Settings that reach this set and choose another version.
    for (global, file) in [
        ("us", None),
        ("", Some(("aof", "us"))),
        ("", Some(("aof", "US"))),
        ("", Some(("aof", "1"))),
        ("", Some(("neogeo", "us"))),
        ("", Some(("source/y", "us"))),
        ("", Some(("horizont", "us"))),
        ("", Some(("raster", "us"))),
        ("", Some(("arcade", "us"))),
    ] {
        let s = settings(global, file.as_slice());
        let why = ok(&s).unwrap_err();
        assert!(why.contains("BIOS version"), "{global:?} {file:?}: {why}");
    }
    // The set's own default overrides a global choice; a file that only may
    // apply does not.
    assert_eq!(ok(&settings("us", &[("aof", "euro")])), Ok(()));
    assert!(ok(&settings("us", &[("horizont", "euro")])).is_err());
    // A clone reads its parent's ini and its BIOS root's.
    let kof98a = bios_set("kof98a", r#"cloneof="kof98" romof="kof98""#);
    for name in ["kof98", "neogeo", "kof98a"] {
        assert!(
            effective_bios_check(&kof98a, &neo, &settings("", &[(name, "us")])).is_err(),
            "{name}"
        );
    }
    // No declared default: nothing is confirmed.
    let plain = facts(&machine("p", "good", "good", "", "", ""), "p");
    assert!(effective_bios_check(&plain, &[], &settings("", &[])).is_err());
}

#[test]
fn bios_settings_are_read_once_from_the_first_file_of_each_name() {
    let home = tempfile::tempdir().unwrap();
    let mame = home.path().join(".mame");
    let other = home.path().join("etc");
    std::fs::create_dir_all(mame.join("source")).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(mame.join("mame.ini"), "rompath /x\n#bios us\n").unwrap();
    std::fs::write(mame.join("aof.ini"), "bios \"us\" # note\n").unwrap();
    std::fs::write(mame.join("sf2.ini"), "skip_warnings 1\n").unwrap();
    std::fs::write(mame.join("source").join("neogeo.ini"), "bios   euro\n").unwrap();
    std::fs::write(mame.join("notes.txt"), "bios us\n").unwrap();
    // Hidden behind ~/.mame/sf2.ini, which MAME opens first.
    std::fs::write(other.join("sf2.ini"), "bios us\n").unwrap();
    let out = format!(
        "readconfig 1\ninipath $HOME/.mame;{};/no/such\nbios \n",
        other.display()
    );
    let s = read_bios_settings(&out, Some(home.path())).unwrap();
    assert_eq!(s.global, "");
    let bios = |name: &str| s.files.get(name).map(|(_, b)| b.as_deref());
    assert_eq!(bios("aof"), Some(Some("us")));
    assert_eq!(bios("source/neogeo"), Some(Some("euro")));
    assert_eq!(bios("sf2"), Some(None));
    assert_eq!(bios("mame"), Some(None));
    assert_eq!(bios("notes"), None);
}

#[test]
fn unreadable_or_environment_dependent_settings_fail_closed() {
    let home = tempfile::tempdir().unwrap();
    for (text, home) in [
        ("inipath .;$HOME/.mame\nbios \n", Some(home.path())),
        ("inipath $HOMEX/.mame\nbios \n", Some(home.path())),
        ("inipath $MAMEDIR\nbios \n", Some(home.path())),
        ("inipath $HOME/.mame\nbios \n", None),
        ("inipath $HOME/.mame\n", Some(home.path())),
        ("", Some(home.path())),
    ] {
        assert!(read_bios_settings(text, home).is_err(), "{text:?}");
    }
}

#[test]
fn an_unconfirmed_bios_version_withdraws_the_exemption_and_blocks() {
    let f = bios_set("aof", r#"romof="neogeo""#);
    let out =
        "aof         : sp-u2.sp1 (131072 bytes) - NOT FOUND (neogeo)\nromset aof [neogeo] is bad\n";
    let verify = verify_text(out, "aof");
    let ready = evaluate_arcade_set("aof", Some(&f), &verify, false);
    assert_ne!(ready.readiness, ArcadeReadiness::Blocked);
    let (kept, evidence) =
        confirm_effective_bios("aof", Some(&f), &verify, false, ready.clone(), || {
            Ok(settings("", &[("sf2", "us")]))
        });
    assert_eq!(kept, ready);
    assert!(
        evidence.is_some(),
        "a ready verdict that used the exemption carries its evidence"
    );
    for (result, keeps_evidence) in [
        (Ok(settings("", &[("neogeo", "us")])), true),
        (Err("MAME's settings could not be read".to_string()), false),
    ] {
        let (withdrawn, evidence) =
            confirm_effective_bios("aof", Some(&f), &verify, false, ready.clone(), || result);
        assert_eq!(evidence.is_some(), keeps_evidence);
        assert_eq!(withdrawn.readiness, ArcadeReadiness::Blocked);
        assert_eq!(
            withdrawn.block_reason,
            Some(ArcadeBlockReason::BiosOrDeviceMissing)
        );
        assert!(withdrawn.notes.iter().any(|n| n.contains("default BIOS")));
    }
}

#[test]
fn settings_are_read_only_when_readiness_depends_on_the_exemption() {
    let f = bios_set("aof", r#"romof="neogeo""#);
    for out in [
        "romset aof [neogeo] is good\n",
        "aof         : p1.p1 (1048576 bytes) - NOT FOUND\nromset aof [neogeo] is bad\n",
    ] {
        let verify = verify_text(out, "aof");
        let verdict = evaluate_arcade_set("aof", Some(&f), &verify, false);
        let (same, evidence) =
            confirm_effective_bios("aof", Some(&f), &verify, false, verdict.clone(), || {
                panic!("settings must not be read for {out:?}")
            });
        assert_eq!(same, verdict);
        assert!(evidence.is_none());
    }
}

#[test]
fn an_unrelated_games_bios_preference_never_withdraws_the_exemption() {
    // Every other set's own ini chooses another version; only aof's own
    // chain (aof, neogeo, source/y, screens, global) could reach aof.
    let f = bios_set("aof", r#"romof="neogeo""#);
    let out =
        "aof         : sp-u2.sp1 (131072 bytes) - NOT FOUND (neogeo)\nromset aof [neogeo] is bad\n";
    let verify = verify_text(out, "aof");
    let ready = evaluate_arcade_set("aof", Some(&f), &verify, false);
    let unrelated = settings(
        "",
        &[
            ("sf2", "us"),
            ("kof98", "us"),
            ("kof98a", "1"),
            ("pacman", "us"),
            ("source/neogeo", "us"),
            ("aofx", "us"),
        ],
    );
    let (kept, evidence) =
        confirm_effective_bios("aof", Some(&f), &verify, false, ready.clone(), || {
            Ok(unrelated)
        });
    assert_eq!(kept, ready);
    assert_ne!(kept.readiness, ArcadeReadiness::Blocked);
    assert!(evidence.is_some());
}

#[test]
fn a_clone_whose_grandparent_is_unknown_is_not_confirmed() {
    let neo = ["neogeo".to_string()];
    let kof98a = bios_set("kof98a", r#"cloneof="kof98" romof="kof98""#);
    assert_eq!(
        effective_bios_check(&kof98a, &neo, &settings("", &[])),
        Ok(())
    );
    for providers in [
        vec![],
        vec!["kof98".to_string()],
        vec!["kof98a".to_string()],
    ] {
        assert!(
            effective_bios_check(&kof98a, &providers, &settings("", &[])).is_err(),
            "{providers:?}"
        );
    }
}

#[test]
fn changed_bios_settings_are_seen_by_the_recorded_evidence() {
    let home = tempfile::tempdir().unwrap();
    let mame = home.path().join(".mame");
    std::fs::create_dir_all(&mame).unwrap();
    std::fs::write(mame.join("mame.ini"), "rompath /x\n").unwrap();
    let out = "readconfig 1\ninipath $HOME/.mame\nbios \n";
    let read = || read_bios_settings(out, Some(home.path())).unwrap().evidence;
    let evidence = read();
    assert!(evidence.is_current());
    // Editing a file (its size changes).
    std::fs::write(mame.join("mame.ini"), "rompath /x\nbios us\n").unwrap();
    assert!(!evidence.is_current());
    // Adding a file, or creating the `source/` folder.
    let evidence = read();
    std::fs::write(mame.join("aof.ini"), "bios us\n").unwrap();
    assert!(!evidence.is_current());
    let evidence = read();
    std::fs::create_dir(mame.join("source")).unwrap();
    assert!(!evidence.is_current());
    // Removing a file.
    let evidence = read();
    std::fs::remove_file(mame.join("aof.ini")).unwrap();
    assert!(!evidence.is_current());
    assert!(read().is_current());
    // Same size, modification time put back: the status-change time moves.
    let ini = mame.join("mame.ini");
    let evidence = read();
    let before = std::fs::metadata(&ini).unwrap().modified().unwrap();
    std::fs::write(&ini, "rompath /x\nbios eu\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&ini)
        .unwrap()
        .set_modified(before)
        .unwrap();
    assert!(!evidence.is_current());
    // Atomic replacement by rename, identical content and size.
    let evidence = read();
    std::fs::write(mame.join("new.tmp"), "rompath /x\nbios eu\n").unwrap();
    std::fs::rename(mame.join("new.tmp"), &ini).unwrap();
    assert!(!evidence.is_current());
    // A symlinked ini retargeted to another file of the same size.
    let a = home.path().join("a.ini");
    let b = home.path().join("b.ini");
    std::fs::write(&a, "bios us\n").unwrap();
    std::fs::write(&b, "bios eu\n").unwrap();
    std::os::unix::fs::symlink(&a, mame.join("aof.ini")).unwrap();
    let evidence = read();
    std::fs::remove_file(mame.join("aof.ini")).unwrap();
    std::os::unix::fs::symlink(&b, mame.join("aof.ini")).unwrap();
    assert!(!evidence.is_current());
}

#[test]
fn ini_values_are_read_as_mame_0_264_reads_them() {
    // Measured: `bios us#x` and `bios us #x` select us; quotes are allowed.
    assert_eq!(ini_value("us#x"), "us");
    assert_eq!(ini_value("  us   # note"), "us");
    assert_eq!(ini_value("\"us\" # note"), "us");
    assert_eq!(ini_value("\"\""), "");
    assert_eq!(ini_value(""), "");
    let home = tempfile::tempdir().unwrap();
    let mame = home.path().join(".mame");
    std::fs::create_dir_all(&mame).unwrap();
    // Measured: option names match exactly (`BIOS us` is ignored), the last
    // `bios` line wins, and a bare `bios` line resets to the default.
    std::fs::write(mame.join("aof.ini"), "bios us#x\n").unwrap();
    std::fs::write(mame.join("kof98.ini"), "bios us\nBIOS euro\n").unwrap();
    std::fs::write(mame.join("sf2.ini"), "BIOS us\n").unwrap();
    std::fs::write(mame.join("pacman.ini"), "bios us\nbios\n").unwrap();
    let out = "readconfig 1\ninipath $HOME/.mame\nbios \n";
    let s = read_bios_settings(out, Some(home.path())).unwrap();
    let bios = |name: &str| s.files.get(name).map(|(_, b)| b.as_deref());
    assert_eq!(bios("aof"), Some(Some("us")));
    assert_eq!(bios("kof98"), Some(Some("us")));
    assert_eq!(bios("sf2"), Some(None));
    assert_eq!(bios("pacman"), Some(Some("")));
    let aof = bios_set("aof", r#"romof="neogeo""#);
    let neo = ["neogeo".to_string()];
    assert!(effective_bios_check(&aof, &neo, &s).is_err());
}

#[test]
fn readconfig_off_makes_the_layers_above_it_unknown() {
    let home = tempfile::tempdir().unwrap();
    let mame = home.path().join(".mame");
    std::fs::create_dir_all(&mame).unwrap();
    // Measured: with `readconfig 0` in neogeo.ini (or mame.ini), MAME never
    // reads aof.ini, so aof.ini's `bios euro` does not decide anything.
    std::fs::write(mame.join("neogeo.ini"), "readconfig 0\nbios us\n").unwrap();
    std::fs::write(mame.join("aof.ini"), "bios euro\n").unwrap();
    std::fs::write(mame.join("sf2.ini"), "readconfig 0\n").unwrap();
    let out = "readconfig 1\ninipath $HOME/.mame\nbios \n";
    let s = read_bios_settings(out, Some(home.path())).unwrap();
    assert!(s.changes_reading.contains("neogeo"));
    let aof = bios_set("aof", r#"romof="neogeo""#);
    let neo = ["neogeo".to_string()];
    let why = effective_bios_check(&aof, &neo, &s).unwrap_err();
    assert!(why.contains("readconfig"), "{why}");
    // Measured: the short name `rc 0` stops reading too, and an `inipath`
    // line in a lower file moves where aof.ini is opened from.
    for text in ["rc 0\nbios us\n", "inipath /elsewhere\n"] {
        std::fs::write(mame.join("neogeo.ini"), text).unwrap();
        let s = read_bios_settings(out, Some(home.path())).unwrap();
        assert!(effective_bios_check(&aof, &neo, &s).is_err(), "{text:?}");
    }
    std::fs::write(mame.join("horizont.ini"), "inipath /elsewhere\n").unwrap();
    let s = read_bios_settings(out, Some(home.path())).unwrap();
    assert!(effective_bios_check(&aof, &neo, &s).is_err());
    std::fs::remove_file(mame.join("horizont.ini")).unwrap();
    // Another set's `readconfig 0` does not reach aof.
    std::fs::remove_file(mame.join("neogeo.ini")).unwrap();
    let s = read_bios_settings(out, Some(home.path())).unwrap();
    assert_eq!(effective_bios_check(&aof, &neo, &s), Ok(()));
    // Global readconfig off, or not reported: nothing is confirmed.
    for text in [
        "readconfig 0\ninipath $HOME/.mame\nbios \n",
        "inipath $HOME/.mame\nbios \n",
    ] {
        assert!(
            read_bios_settings(text, Some(home.path())).is_err(),
            "{text:?}"
        );
    }
}

#[test]
fn a_set_with_a_bios_parent_but_no_named_root_is_not_confirmed() {
    let aof = bios_set("aof", r#"romof="neogeo""#);
    for providers in [vec![], vec!["aof".to_string()]] {
        assert!(
            effective_bios_check(&aof, &providers, &settings("", &[])).is_err(),
            "{providers:?}"
        );
    }
}

#[test]
fn a_set_blocked_by_a_bios_setting_is_checked_again_once_it_is_corrected() {
    let home = tempfile::tempdir().unwrap();
    let mame = home.path().join(".mame");
    std::fs::create_dir_all(&mame).unwrap();
    std::fs::write(mame.join("aof.ini"), "bios us\n").unwrap();
    let out = "readconfig 1\ninipath $HOME/.mame\nbios \n";
    let f = bios_set("aof", r#"romof="neogeo""#);
    let verify = verify_text(
        "aof         : sp-u2.sp1 (131072 bytes) - NOT FOUND (neogeo)\nromset aof [neogeo] is bad\n",
        "aof",
    );
    let ready = evaluate_arcade_set("aof", Some(&f), &verify, false);
    let check = || {
        confirm_effective_bios("aof", Some(&f), &verify, false, ready.clone(), || {
            read_bios_settings(out, Some(home.path()))
        })
    };
    let (blocked, evidence) = check();
    assert_eq!(blocked.readiness, ArcadeReadiness::Blocked);
    let evidence = evidence.expect("a verdict blocked by a setting keeps its evidence");
    assert!(evidence.is_current());
    // The user puts the default back: the blocked verdict is now stale.
    std::fs::write(mame.join("aof.ini"), "bios euro\n").unwrap();
    assert!(!evidence.is_current());
    let (again, _) = check();
    assert_eq!(again, ready);
}

#[test]
fn an_empty_bios_value_resets_to_the_default() {
    let home = tempfile::tempdir().unwrap();
    let mame = home.path().join(".mame");
    std::fs::create_dir_all(&mame).unwrap();
    let out = "readconfig 1\ninipath $HOME/.mame\nbios \n";
    let aof = bios_set("aof", r#"romof="neogeo""#);
    let neo = ["neogeo".to_string()];
    std::fs::write(mame.join("neogeo.ini"), "bios us\n").unwrap();
    // Measured with 0.264: with `bios us` in neogeo.ini, a bare `bios` or
    // `bios ""` in aof.ini starts the default BIOS.
    for own in ["bios\n", "bios \"\"\n", "bios us\nbios\n"] {
        std::fs::write(mame.join("aof.ini"), own).unwrap();
        let s = read_bios_settings(out, Some(home.path())).unwrap();
        assert_eq!(effective_bios_check(&aof, &neo, &s), Ok(()), "{own:?}");
    }
    // Without a `bios` line in aof.ini, neogeo.ini's `us` still decides.
    std::fs::write(mame.join("aof.ini"), "skip_warnings 1\n").unwrap();
    let s = read_bios_settings(out, Some(home.path())).unwrap();
    assert!(effective_bios_check(&aof, &neo, &s).is_err());
    // An empty value in a file that only possibly applies decides nothing:
    // the global `us` below it still withdraws the exemption.
    std::fs::remove_file(mame.join("aof.ini")).unwrap();
    std::fs::remove_file(mame.join("neogeo.ini")).unwrap();
    std::fs::write(mame.join("horizont.ini"), "bios\n").unwrap();
    let global_us = "readconfig 1\ninipath $HOME/.mame\nbios us\n";
    let s = read_bios_settings(global_us, Some(home.path())).unwrap();
    assert!(effective_bios_check(&aof, &neo, &s).is_err());
}

#[test]
fn an_empty_bios_value_never_overrides_a_possible_higher_layer() {
    let home = tempfile::tempdir().unwrap();
    let mame = home.path().join(".mame");
    std::fs::create_dir_all(mame.join("source")).unwrap();
    // The test machine's sourcefile is x/y.cpp, so its source ini is y.ini.
    let aof = bios_set("aof", r#"romof="neogeo""#);
    let neo = ["neogeo".to_string()];
    // neogeo.ini (the BIOS root, which only possibly applies) ranks above
    // source/y.ini, so its `us` still withdraws the exemption.
    std::fs::write(mame.join("neogeo.ini"), "bios us\n").unwrap();
    std::fs::write(mame.join("source").join("y.ini"), "bios\n").unwrap();
    let global_us = "readconfig 1\ninipath $HOME/.mame\nbios us\n";
    let s = read_bios_settings(global_us, Some(home.path())).unwrap();
    assert!(effective_bios_check(&aof, &neo, &s).is_err());
    // Without it, the empty value in source/y.ini (which certainly applies)
    // resets to the default above the global `us`.
    std::fs::remove_file(mame.join("neogeo.ini")).unwrap();
    let s = read_bios_settings(global_us, Some(home.path())).unwrap();
    assert_eq!(effective_bios_check(&aof, &neo, &s), Ok(()));
}
