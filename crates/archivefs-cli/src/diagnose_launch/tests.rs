use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_string()).collect()
}

fn parsed(list: &[&str]) -> Parsed {
    match parse(&args(list)).unwrap() {
        Request::Diagnose { artefacts, .. } => *artefacts,
        other => panic!("expected a diagnosis request, got {other:?}"),
    }
}

#[test]
fn help_is_recognised_only_on_its_own() {
    assert_eq!(parse(&args(&["--help"])).unwrap(), Request::Help);
    assert_eq!(parse(&args(&["-h"])).unwrap(), Request::Help);
    assert!(parse(&args(&["--emulator", "retroarch", "--help"])).is_err());
}

#[test]
fn rules_takes_only_json() {
    assert_eq!(
        parse(&args(&["--rules"])).unwrap(),
        Request::Rules { json: false }
    );
    assert_eq!(
        parse(&args(&["--json", "--rules"])).unwrap(),
        Request::Rules { json: true }
    );
    assert!(parse(&args(&["--rules", "--emulator", "retroarch"])).is_err());
}

#[test]
fn an_emulator_is_required_and_nothing_unknown_is_accepted() {
    assert!(parse(&args(&[])).is_err());
    assert!(parse(&args(&["--exit-code", "1"])).is_err());
    assert!(parse(&args(&["--emulator", "  "])).is_err());
    assert!(parse(&args(&["--emulator", "retroarch", "--apply"])).is_err());
    assert!(parse(&args(&["--emulator", "retroarch", "stray"])).is_err());
    assert!(parse(&args(&["--emulator"])).is_err());
    assert!(parse(&args(&["--emulator", "a", "--emulator", "b"])).is_err());
}

#[test]
fn the_way_the_process_ended_is_given_once_and_validated() {
    assert_eq!(
        parsed(&["--emulator", "retroarch", "--exit-code", "1"]).exit,
        Some(ExitOutcome::Exited { code: 1 })
    );
    assert_eq!(
        parsed(&["--emulator", "retroarch", "--signal", "11"]).exit,
        Some(ExitOutcome::Signaled {
            signal: 11,
            core_dumped: false
        })
    );
    assert_eq!(
        parsed(&["--emulator", "retroarch", "--spawn-error", "not-found"]).exit,
        Some(ExitOutcome::SpawnFailed {
            reason: SpawnFailure::ProgramNotFound,
            os_error: None
        })
    );
    for bad in [
        &["--emulator", "r", "--exit-code", "one"][..],
        &["--emulator", "r", "--signal", "0"],
        &["--emulator", "r", "--signal", "999"],
        &["--emulator", "r", "--spawn-error", "exploded"],
        &["--emulator", "r", "--exit-code", "1", "--signal", "11"],
        &["--emulator", "r", "--duration-ms", "-5"],
    ] {
        assert!(parse(&args(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn settings_layers_and_environment_are_collected() {
    let parsed = parsed(&[
        "--emulator",
        "RetroArch",
        "--settings",
        "/a/retroarch.cfg",
        "--game-settings",
        "/a/game.cfg",
        "--env",
        "DISPLAY=:0",
        "--env",
        "EMPTY=",
        "--env-unset",
        "WAYLAND_DISPLAY",
        "--duration-ms",
        "1500",
    ]);
    assert_eq!(
        parsed.settings,
        [
            (PathBuf::from("/a/retroarch.cfg"), ConfigLayer::Main),
            (PathBuf::from("/a/game.cfg"), ConfigLayer::Game)
        ]
    );
    assert_eq!(
        parsed.environment,
        [
            ("DISPLAY".to_string(), Some(":0".to_string())),
            ("EMPTY".to_string(), Some(String::new())),
            ("WAYLAND_DISPLAY".to_string(), None)
        ]
    );
    assert_eq!(parsed.duration_ms, Some(1_500));
    // The emulator name is normalised when the artefacts are built.
    assert_eq!(artefacts(parsed).unwrap().emulator, "retroarch");

    assert!(parse(&args(&["--emulator", "r", "--env", "NOVALUE"])).is_err());
    assert!(parse(&args(&["--emulator", "r", "--env", "=x"])).is_err());
    assert!(parse(&args(&["--emulator", "r", "--env-unset", "A=B"])).is_err());
    assert!(
        parse(&args(&[
            "--emulator",
            "r",
            "--settings",
            "/a",
            "--settings",
            "/b"
        ]))
        .is_err()
    );
}

#[test]
fn an_unreadable_stderr_file_is_an_error_not_an_empty_capture() {
    let parsed = parsed(&[
        "--emulator",
        "retroarch",
        "--stderr-file",
        "/nonexistent/emuwiz/stderr.txt",
    ]);
    assert!(artefacts(parsed).unwrap_err().contains("cannot read"));
}

#[test]
fn the_text_report_separates_what_happened_from_why_and_lists_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("retroarch.cfg");
    std::fs::write(&config, "video_driver = \"vulkan\"\n").unwrap();
    let stderr = dir.path().join("stderr.txt");
    std::fs::write(
        &stderr,
        "Fatal error received in: \"video_driver_init_internal()\"\n",
    )
    .unwrap();
    let parsed = parsed(&[
        "--emulator",
        "retroarch",
        "--exit-code",
        "1",
        "--settings",
        config.to_str().unwrap(),
        "--stderr-file",
        stderr.to_str().unwrap(),
    ]);
    let report = diagnose(&collect(&artefacts(parsed).unwrap()));
    let text = format_report(&report);
    let position = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in:\n{text}"))
    };
    assert!(position("What happened") < position("Why (most firmly supported first)"));
    assert!(position("Why (most firmly supported first)") < position("Not observed"));
    position("Vulkan graphics could not start [possible]");
    // The exit status was typed, not observed: stated, and no more than
    // possible.
    position("The emulator exited with status 1 [possible]");
    position("Launch (as supplied; EmuWiz did not observe it): the emulator process failed");
    assert!(position("Launch (as supplied") < position("Verdict: "));
    position("EmuWiz did not observe it");
    position("(not applied)");
    position("Graphics card, driver and graphics API: not collected");
    position("nothing was started or changed");
}

#[test]
fn the_rules_listing_names_every_rule() {
    let listing = format_rules();
    for rule in rule_set().rules {
        assert!(listing.contains(rule.id), "{}", rule.id);
    }
}
