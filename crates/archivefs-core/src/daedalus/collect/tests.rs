use super::*;
use crate::daedalus::evidence::{EvidenceState, PathPresence};
use crate::daedalus::rules::{Confidence, Verdict, diagnose};
use crate::daedalus::test_fixtures as fixtures;

fn missing_areas(set: &EvidenceSet) -> Vec<EvidenceArea> {
    set.coverage()
        .into_iter()
        .filter(|entry| !entry.has_current_evidence())
        .map(|entry| entry.area)
        .collect()
}

#[test]
fn nothing_supplied_means_everything_is_recorded_as_not_collected() {
    let set = collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        ..LaunchArtefacts::default()
    });
    assert!(
        set.items()
            .iter()
            .all(|item| { matches!(item.state, EvidenceState::NotCollected { .. }) })
    );
    let missing = missing_areas(&set);
    for area in [
        EvidenceArea::ProcessExit,
        EvidenceArea::LogEvents,
        EvidenceArea::EmulatorConfiguration,
        EvidenceArea::Filesystem,
        EvidenceArea::LaunchEnvironment,
    ] {
        assert!(missing.contains(&area), "{area:?}");
    }
    let report = diagnose(&set);
    assert_eq!(report.verdict, Verdict::NoProblemObserved);
    assert!(report.causes.is_empty());
}

#[test]
fn a_retroarch_failure_is_diagnosed_from_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("retroarch.cfg");
    std::fs::write(
        &config,
        "video_driver = \"vulkan\"\naudio_driver = \"null\"\n",
    )
    .unwrap();
    let log = dir.path().join("retroarch.log");
    std::fs::write(&log, fixtures::RA_VULKAN_NO_DRIVER).unwrap();
    let set = collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        game: Some("2048".into()),
        exit: Some(ExitOutcome::Exited { code: 1 }),
        duration: Some(Duration::from_millis(1_488)),
        log_file: Some(log),
        settings: vec![(config, ConfigLayer::Main)],
        environment: vec![("DISPLAY".into(), Some(":99".into()))],
        ..LaunchArtefacts::default()
    });
    assert_eq!(set.subject.game.as_deref(), Some("2048"));
    let report = diagnose(&set);
    let cause = report
        .causes
        .iter()
        .find(|cause| cause.rule == "retroarch.video.vulkan_unavailable")
        .unwrap();
    assert_eq!(cause.confidence, Confidence::Possible);
    assert_eq!(report.verdict, Verdict::PartlyExplained);
    assert!(missing_areas(&set).contains(&EvidenceArea::Graphics));
}

#[test]
fn stderr_is_scanned_for_both_the_emulator_and_the_host() {
    let mut stderr = fixtures::RA_VULKAN_NO_DRIVER_QUIET.as_bytes().to_vec();
    stderr.extend_from_slice(fixtures::HOST_BWRAP.as_bytes());
    let set = collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        exit: Some(ExitOutcome::Exited { code: 1 }),
        stderr: Some(stderr),
        ..LaunchArtefacts::default()
    });
    let signatures: Vec<&str> = set
        .items()
        .iter()
        .filter_map(|item| match &item.fact {
            Fact::LogEvent(event) => Some(event.signature.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        signatures,
        [
            "retroarch.fatal.video_driver_init",
            "host.bwrap.setup_failed"
        ]
    );
}

#[test]
fn paths_are_looked_at_and_never_opened() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game.gb");
    std::fs::write(&game, "rom").unwrap();
    let before = std::fs::metadata(&game).unwrap().accessed().unwrap();
    let set = collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        game_file: Some(game.clone()),
        core: Some(dir.path().join("absent_libretro.so")),
        save_directory: Some(dir.path().to_path_buf()),
        ..LaunchArtefacts::default()
    });
    let presences: Vec<PathPresence> = set
        .items()
        .iter()
        .filter_map(|item| match &item.fact {
            Fact::PathAccess(access) => Some(access.presence),
            _ => None,
        })
        .collect();
    assert_eq!(
        presences,
        [
            PathPresence::RegularFile,
            PathPresence::Missing,
            PathPresence::Directory
        ]
    );
    assert_eq!(
        std::fs::metadata(&game).unwrap().accessed().unwrap(),
        before
    );
    assert_eq!(std::fs::read(&game).unwrap(), b"rom");
    // The missing core is an established cause with no log at all.
    let report = diagnose(&set);
    assert!(report.causes.iter().any(|cause| {
        cause.rule == "retroarch.core.not_loaded" && cause.confidence == Confidence::Established
    }));
}

#[test]
fn an_unreadable_or_linked_log_is_unavailable_not_silently_empty() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.log");
    std::fs::write(&real, fixtures::RA_VULKAN_NO_DRIVER).unwrap();
    let link = dir.path().join("retroarch.log");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    for log in [
        link,
        dir.path().join("absent.log"),
        dir.path().to_path_buf(),
    ] {
        let set = collect(&LaunchArtefacts {
            emulator: "retroarch".into(),
            log_file: Some(log),
            ..LaunchArtefacts::default()
        });
        assert!(set.items().iter().any(|item| {
            item.area == EvidenceArea::LogEvents
                && matches!(item.state, EvidenceState::Unavailable { .. })
        }));
        assert!(
            !set.items()
                .iter()
                .any(|item| matches!(item.fact, Fact::LogEvent(_)))
        );
    }
}

#[test]
fn an_emulator_without_an_adapter_gets_host_signatures_and_honest_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("PCSX2.ini");
    std::fs::write(&config, "[EmuCore/GS]\nRenderer = 14\n").unwrap();
    let set = collect(&LaunchArtefacts {
        emulator: "pcsx2".into(),
        exit: Some(ExitOutcome::Exited { code: 127 }),
        stderr: Some(fixtures::HOST_LOADER.as_bytes().to_vec()),
        settings: vec![(config, ConfigLayer::Main)],
        ..LaunchArtefacts::default()
    });
    let report = diagnose(&set);
    assert!(
        report
            .causes
            .iter()
            .any(|cause| cause.rule == "dependency.shared_library_missing")
    );
    // No settings reader for PCSX2: the gap is stated, no value is invented.
    assert!(
        !set.items()
            .iter()
            .any(|item| matches!(item.fact, Fact::ConfigValue { .. }))
    );
    assert!(set.items().iter().any(|item| {
        matches!(&item.state, EvidenceState::NotCollected { reason } if reason.contains("pcsx2"))
    }));
}

#[test]
fn a_long_log_file_is_read_only_up_to_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("big.log");
    let line = "[INFO] [Playlist]: padding\n";
    let mut content = String::with_capacity(text::LOG_BYTES_LIMIT + 4_096);
    while content.len() <= text::LOG_BYTES_LIMIT + 1_024 {
        content.push_str(line);
    }
    std::fs::write(&log, &content).unwrap();
    let bytes = read_log_prefix(&log).unwrap();
    assert_eq!(bytes.len(), text::LOG_BYTES_LIMIT + 1);
    let set = collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        log_file: Some(log),
        ..LaunchArtefacts::default()
    });
    assert!(
        set.items().iter().any(|item| {
            matches!(&item.fact, Fact::LogCoverage(coverage) if coverage.truncated)
        })
    );
}

#[test]
fn what_the_caller_states_is_recorded_as_supplied_and_a_saved_log_is_complete() {
    // 80 KiB of saved output: larger than a launcher capture, and complete.
    let mut stderr = Vec::new();
    while stderr.len() < 80 * 1024 {
        stderr.extend_from_slice(b"[INFO] [Playlist]: padding\n");
    }
    let base = LaunchArtefacts {
        emulator: "retroarch".into(),
        exit: Some(ExitOutcome::Exited { code: 1 }),
        stderr: Some(stderr),
        environment: vec![("DISPLAY".into(), Some(":0".into()))],
        provenance: Provenance::ObservedByEmuWiz,
        ..LaunchArtefacts::default()
    };
    let cut = |set: &EvidenceSet| {
        set.items()
            .iter()
            .any(|item| matches!(&item.fact, Fact::LogCoverage(coverage) if coverage.truncated))
    };
    let trusts = |set: &EvidenceSet| -> Vec<Trust> {
        set.items()
            .iter()
            .filter(|item| {
                matches!(
                    item.area,
                    EvidenceArea::ProcessExit | EvidenceArea::LaunchEnvironment
                )
            })
            .map(|item| item.trust)
            .collect()
    };

    let supplied = collect(&LaunchArtefacts {
        provenance: Provenance::SuppliedByCaller,
        ..base.clone()
    });
    assert_eq!(trusts(&supplied), [Trust::Supplied, Trust::Supplied]);
    assert!(!cut(&supplied));
    assert!(
        diagnose(&supplied)
            .notes
            .iter()
            .any(|note| note.contains("EmuWiz did not observe it"))
    );

    // The same bytes as a launcher capture are known to be cut at 64 KiB.
    let observed = collect(&base);
    assert_eq!(trusts(&observed), [Trust::Measured, Trust::Measured]);
    assert!(cut(&observed));
}

#[test]
fn a_pipe_or_a_device_where_a_file_was_expected_is_refused_without_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("retroarch.log");
    let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: a valid NUL-terminated path; mkfifo does not retain it.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let started = std::time::Instant::now();
    assert!(read_log_prefix(&fifo).is_err());
    let set = collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        log_file: Some(fifo.clone()),
        settings: vec![(fifo, ConfigLayer::Main)],
        ..LaunchArtefacts::default()
    });
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        !set.items()
            .iter()
            .any(|item| matches!(item.fact, Fact::LogEvent(_)))
    );
    assert!(
        set.items()
            .iter()
            .filter(|item| matches!(item.state, EvidenceState::Unavailable { .. }))
            .count()
            >= 2
    );
}

#[test]
fn a_caller_that_says_nothing_is_never_taken_to_have_observed_the_launch() {
    assert_eq!(
        LaunchArtefacts::default().provenance,
        Provenance::SuppliedByCaller
    );
    let dir = tempfile::tempdir().unwrap();
    let set = collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        exit: Some(ExitOutcome::Exited { code: 1 }),
        core: Some(dir.path().join("missing_libretro.so")),
        ..LaunchArtefacts::default()
    });
    let exit = set
        .items()
        .iter()
        .find(|item| item.area == EvidenceArea::ProcessExit)
        .unwrap();
    assert_eq!(exit.trust, Trust::Supplied);
    // The path was looked at by EmuWiz; its role is the caller's word.
    let path = set
        .items()
        .iter()
        .find_map(|item| match &item.fact {
            Fact::PathAccess(access) => Some((item, access)),
            _ => None,
        })
        .unwrap();
    assert_eq!(path.0.trust, Trust::Measured);
    assert!(path.1.role_supplied);
    assert!(path.0.source.contains("named by whoever asked"));

    let observed = collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        core: Some(dir.path().join("missing_libretro.so")),
        provenance: Provenance::ObservedByEmuWiz,
        ..LaunchArtefacts::default()
    });
    assert!(
        observed.items().iter().all(|item| {
            !matches!(&item.fact, Fact::PathAccess(access) if access.role_supplied)
        })
    );
}

#[test]
fn a_supplied_file_exactly_as_long_as_a_launcher_capture_may_have_been_cut() {
    let limit = crate::launch::process_spawn::PROCESS_STDERR_CAPTURE_LIMIT;
    let cut = |bytes: Vec<u8>| {
        collect(&LaunchArtefacts {
            emulator: "retroarch".into(),
            stderr: Some(bytes),
            ..LaunchArtefacts::default()
        })
        .items()
        .iter()
        .any(|item| matches!(&item.fact, Fact::LogCoverage(coverage) if coverage.truncated))
    };
    assert!(cut(vec![b'x'; limit]));
    assert!(!cut(vec![b'x'; limit - 1]));
    assert!(!cut(vec![b'x'; limit + 1]));
}
