use super::*;
use std::os::unix::fs::PermissionsExt;

fn observed(set: &mut EvidenceSet, area: EvidenceArea) -> EvidenceId {
    set.observe(
        area,
        Trust::Measured,
        "test",
        Fact::EnvironmentVariable {
            name: "X".into(),
            value: None,
        },
    )
    .unwrap()
}

fn running_as_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

#[test]
fn ids_are_assigned_in_insertion_order_per_area() {
    let mut set = EvidenceSet::default();
    let ids = [
        observed(&mut set, EvidenceArea::ProcessExit),
        observed(&mut set, EvidenceArea::LogEvents),
        observed(&mut set, EvidenceArea::LogEvents),
        observed(&mut set, EvidenceArea::ProcessExit),
    ];
    let ids: Vec<&str> = ids.iter().map(EvidenceId::as_str).collect();
    assert_eq!(
        ids,
        [
            "process_exit-1",
            "log_events-1",
            "log_events-2",
            "process_exit-2"
        ]
    );
}

#[test]
fn ids_continue_after_a_round_trip_through_json() {
    let mut set = EvidenceSet::default();
    observed(&mut set, EvidenceArea::Graphics);
    let json = serde_json::to_string(&set).unwrap();
    let mut restored: EvidenceSet = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, set);
    let next = observed(&mut restored, EvidenceArea::Graphics);
    assert_eq!(next.as_str(), "graphics-2");
}

#[test]
fn a_full_set_refuses_more_and_says_it_was_truncated() {
    let mut set = EvidenceSet::default();
    for _ in 0..MAX_EVIDENCE_ITEMS {
        observed(&mut set, EvidenceArea::LogEvents);
    }
    assert!(!set.is_truncated());
    assert!(
        set.observe(EvidenceArea::LogEvents, Trust::Measured, "x", Fact::Absent)
            .is_none()
    );
    assert!(set.is_truncated());
    assert_eq!(set.items().len(), MAX_EVIDENCE_ITEMS);
}

#[test]
fn coverage_reports_every_area_including_those_never_collected() {
    let mut set = EvidenceSet::default();
    observed(&mut set, EvidenceArea::ProcessExit);
    set.unavailable(EvidenceArea::Graphics, "vulkan probe", "tool not installed");
    set.not_collected(
        EvidenceArea::Firmware,
        "no firmware check for this emulator",
    );
    let coverage = set.coverage();
    assert_eq!(coverage.len(), EvidenceArea::ALL.len());
    let of = |area| coverage.iter().find(|entry| entry.area == area).unwrap();
    assert!(of(EvidenceArea::ProcessExit).has_current_evidence());
    assert_eq!(
        of(EvidenceArea::Graphics).missing,
        ["unavailable: tool not installed"]
    );
    assert_eq!(
        of(EvidenceArea::Firmware).missing,
        ["not collected: no firmware check for this emulator"]
    );
    assert_eq!(of(EvidenceArea::GameIdentity).missing, ["not collected"]);
    assert!(!of(EvidenceArea::GameIdentity).has_current_evidence());
}

#[test]
fn source_and_reason_text_is_sanitised_and_bounded() {
    let mut set = EvidenceSet::default();
    let id = set
        .unavailable(EvidenceArea::Graphics, "probe\x1b[31m", "x".repeat(5_000))
        .unwrap();
    let item = set.get(&id).unwrap();
    assert!(!item.source.contains('\x1b'));
    let EvidenceState::Unavailable { reason } = &item.state else {
        panic!("expected unavailable");
    };
    assert!(reason.chars().count() <= text::SOURCE_LIMIT + 1);
}

#[test]
fn a_fingerprint_with_a_digest_detects_a_content_change_of_the_same_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retroarch.cfg");
    std::fs::write(&path, "video_driver = \"vulkan\"\n").unwrap();
    let (bytes, fingerprint) = SourceFingerprint::read_with_digest(&path, 1 << 20).unwrap();
    assert_eq!(bytes, b"video_driver = \"vulkan\"\n");
    assert_eq!(fingerprint.sha256.as_deref().map(str::len), Some(64));
    assert_eq!(fingerprint.check(), FingerprintCheck::Unchanged);

    std::fs::write(&path, "video_driver = \"glcore\"\n").unwrap();
    assert!(matches!(
        fingerprint.check(),
        FingerprintCheck::Changed { .. }
    ));
}

#[test]
fn a_fingerprint_reports_a_removed_or_replaced_file_as_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    std::fs::write(&path, "a").unwrap();
    let (_, fingerprint) = SourceFingerprint::read_with_digest(&path, 1 << 20).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        fingerprint.check(),
        FingerprintCheck::Unavailable { .. }
    ));

    // Replaced by a symbolic link to a file with the same content.
    let elsewhere = dir.path().join("elsewhere");
    std::fs::write(&elsewhere, "a").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    assert!(matches!(
        fingerprint.check(),
        FingerprintCheck::Unavailable { .. }
    ));
}

#[test]
fn reading_refuses_symbolic_links_directories_and_oversized_files() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::write(&real, "content").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(SourceFingerprint::read_with_digest(&link, 1 << 20).is_err());
    assert!(SourceFingerprint::capture(&link).is_err());
    assert!(SourceFingerprint::read_with_digest(dir.path(), 1 << 20).is_err());
    assert!(SourceFingerprint::capture(dir.path()).is_err());
    assert!(SourceFingerprint::read_with_digest(&real, 3).is_err());
    assert!(SourceFingerprint::read_with_digest(&real, 7).is_ok());
}

#[test]
fn an_identity_fingerprint_detects_replacement_without_reading_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("core.so");
    std::fs::write(&path, "one").unwrap();
    let fingerprint = SourceFingerprint::capture(&path).unwrap();
    assert!(fingerprint.sha256.is_none());
    assert_eq!(fingerprint.check(), FingerprintCheck::Unchanged);
    let replacement = dir.path().join("replacement");
    std::fs::write(&replacement, "two").unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    assert!(matches!(
        fingerprint.check(),
        FingerprintCheck::Changed { .. }
    ));
}

#[test]
fn refresh_marks_every_item_from_a_changed_file_stale_and_leaves_others() {
    let dir = tempfile::tempdir().unwrap();
    let changed = dir.path().join("changed.cfg");
    let stable = dir.path().join("stable.cfg");
    std::fs::write(&changed, "a = \"1\"\n").unwrap();
    std::fs::write(&stable, "b = \"2\"\n").unwrap();
    let mut set = EvidenceSet::default();
    let add = |set: &mut EvidenceSet, path: &std::path::Path| {
        let (_, fingerprint) = SourceFingerprint::read_with_digest(path, 1 << 20).unwrap();
        set.push(
            EvidenceArea::EmulatorConfiguration,
            EvidenceState::Observed,
            Trust::Measured,
            "settings",
            Some(fingerprint),
            Fact::Absent,
        )
        .unwrap()
    };
    let first = add(&mut set, &changed);
    let second = add(&mut set, &changed);
    let third = add(&mut set, &stable);
    assert!(set.refresh_freshness().is_empty());

    std::fs::write(&changed, "a = \"9\"\n").unwrap();
    assert_eq!(set.refresh_freshness(), [first.clone(), second]);
    assert!(matches!(
        set.get(&first).unwrap().state,
        EvidenceState::Stale { .. }
    ));
    assert!(set.get(&third).unwrap().is_observed());
    assert_eq!(set.observed(EvidenceArea::EmulatorConfiguration).count(), 1);
    let coverage = set.coverage();
    let configuration = coverage
        .iter()
        .find(|entry| entry.area == EvidenceArea::EmulatorConfiguration)
        .unwrap();
    assert_eq!((configuration.observed, configuration.stale), (1, 2));
    // Already-stale items are not reported again.
    assert!(set.refresh_freshness().is_empty());
}

#[test]
fn path_access_describes_what_is_there_without_following_links() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("game.gb");
    std::fs::write(&file, "rom").unwrap();
    let link = dir.path().join("link.gb");
    std::os::unix::fs::symlink(&file, &link).unwrap();

    let present = observe_path_access(PathRole::GameFile, &file);
    assert_eq!(present.presence, PathPresence::RegularFile);
    assert_eq!(present.readable, Some(true));

    let missing = observe_path_access(PathRole::GameFile, &dir.path().join("gone.gb"));
    assert_eq!(missing.presence, PathPresence::Missing);
    assert_eq!((missing.readable, missing.writable), (None, None));

    let linked = observe_path_access(PathRole::GameFile, &link);
    assert_eq!(linked.presence, PathPresence::SymbolicLink);
    assert_eq!((linked.readable, linked.writable), (None, None));

    let folder = observe_path_access(PathRole::SaveDirectory, dir.path());
    assert_eq!(folder.presence, PathPresence::Directory);
    assert_eq!(folder.writable, Some(true));
}

#[test]
fn path_access_reports_denied_permissions() {
    if running_as_root() {
        // Root passes every permission check; nothing to observe.
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("locked.gb");
    std::fs::write(&file, "rom").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    let saves = dir.path().join("saves");
    std::fs::create_dir(&saves).unwrap();
    std::fs::set_permissions(&saves, std::fs::Permissions::from_mode(0o555)).unwrap();

    let locked = observe_path_access(PathRole::GameFile, &file);
    assert_eq!(locked.presence, PathPresence::RegularFile);
    assert_eq!(locked.readable, Some(false));
    let read_only = observe_path_access(PathRole::SaveDirectory, &saves);
    assert_eq!(read_only.writable, Some(false));

    std::fs::set_permissions(&saves, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_stop_request_is_not_a_failure_and_a_fault_is() {
    let exit = |outcome| ProcessExit {
        outcome,
        duration_ms: None,
        launcher: None,
    };
    let signal = |signal| ExitOutcome::Signaled {
        signal,
        core_dumped: false,
    };
    assert!(!exit(ExitOutcome::Exited { code: 0 }).is_failure());
    assert!(exit(ExitOutcome::Exited { code: 1 }).is_failure());
    assert!(!exit(signal(libc::SIGTERM)).is_failure());
    assert!(exit(signal(libc::SIGSEGV)).is_failure());
    assert!(exit(signal(libc::SIGKILL)).is_failure());
    assert!(!exit(ExitOutcome::StillRunning).is_failure());
    assert!(
        exit(ExitOutcome::SpawnFailed {
            reason: SpawnFailure::ProgramNotFound,
            os_error: Some(libc::ENOENT),
        })
        .is_failure()
    );
}

#[test]
fn only_measured_and_reported_evidence_is_firm() {
    assert!(Trust::Measured.is_firm());
    assert!(Trust::Reported.is_firm());
    assert!(!Trust::UntrustedText.is_firm());
    assert!(!Trust::Asserted.is_firm());
}

#[test]
fn a_fingerprint_shows_a_sanitised_path_and_checks_the_real_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retro\u{202E}arch\x1b[2J.cfg");
    std::fs::write(&path, "a = \"1\"\n").unwrap();
    let (_, fingerprint) = SourceFingerprint::read_with_digest(&path, 1 << 20).unwrap();
    assert!(!fingerprint.path.contains('\u{202E}') && !fingerprint.path.contains('\x1b'));
    // The check still finds the file by its real name.
    assert_eq!(fingerprint.check(), FingerprintCheck::Unchanged);
    std::fs::write(&path, "a = \"2\"\n").unwrap();
    assert!(matches!(
        fingerprint.check(),
        FingerprintCheck::Changed { .. }
    ));

    // The real path is not part of the serialised form, so a restored
    // fingerprint opens nothing and says it cannot be compared.
    let json = serde_json::to_string(&fingerprint).unwrap();
    assert!(!json.contains("real_path"));
    let restored: SourceFingerprint = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        restored.check(),
        FingerprintCheck::Unavailable { .. }
    ));
}

#[test]
fn a_path_that_cannot_be_examined_is_unknown_not_something_else() {
    if running_as_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    let inside = locked.join("game.gb");
    std::fs::write(&inside, "rom").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let access = observe_path_access(PathRole::GameFile, &inside);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(access.presence, PathPresence::Unknown);
    assert_eq!((access.readable, access.writable), (None, None));
    assert!(
        Fact::PathAccess(access)
            .describe()
            .contains("could not be examined")
    );
}

#[test]
fn asserted_statements_are_counted_apart_from_observations() {
    let mut set = EvidenceSet::default();
    set.observe(
        EvidenceArea::Graphics,
        Trust::Asserted,
        "someone said",
        Fact::Absent,
    );
    let coverage = set.coverage();
    let graphics = coverage
        .iter()
        .find(|entry| entry.area == EvidenceArea::Graphics)
        .unwrap();
    assert_eq!((graphics.observed, graphics.asserted), (0, 1));
    assert!(!graphics.has_current_evidence());
    assert_eq!(graphics.missing, ["only asserted, not observed"]);
}

#[test]
fn an_exit_status_that_relays_a_stop_request_is_not_a_failure() {
    let exit = |code, launcher: Option<&str>| ProcessExit {
        outcome: ExitOutcome::Exited { code },
        duration_ms: None,
        launcher: launcher.map(str::to_string),
    };
    for stop in [129, 130, 131, 143] {
        // A launcher relays the stop request as an exit status.
        let relayed = exit(stop, Some("flatpak run"));
        assert!(!relayed.is_failure(), "{stop}");
        assert_eq!(relayed.relayed_stop_request(), Some(stop - 128));
        // With no launcher in between, the program chose the status itself.
        let direct = exit(stop, None);
        assert!(direct.is_failure(), "{stop}");
        assert_eq!(direct.relayed_stop_request(), None);
    }
    for failure in [1, 127, 134, 137, 139, 255] {
        assert!(exit(failure, None).is_failure(), "{failure}");
        assert!(exit(failure, Some("flatpak run")).is_failure(), "{failure}");
    }
    assert!(!exit(0, None).is_failure());
}

#[test]
fn files_whose_names_differ_only_in_replaced_characters_are_checked_separately() {
    let dir = tempfile::tempdir().unwrap();
    // The two names are shown identically once sanitised.
    let first = dir.path().join("settings\u{1}.cfg");
    let second = dir.path().join("settings\u{2}.cfg");
    let mut set = EvidenceSet::default();
    let mut ids = Vec::new();
    for path in [&first, &second] {
        std::fs::write(path, "video_driver = \"gl\"\n").unwrap();
        let (_, fingerprint) = SourceFingerprint::read_with_digest(path, 4096).unwrap();
        ids.push(
            set.push(
                EvidenceArea::EmulatorConfiguration,
                EvidenceState::Observed,
                Trust::Measured,
                "settings file",
                Some(fingerprint),
                Fact::Absent,
            )
            .unwrap(),
        );
    }
    let shown: Vec<&str> = set
        .items()
        .iter()
        .filter_map(|item| item.fingerprint.as_ref().map(|print| print.path.as_str()))
        .collect();
    assert_eq!(shown[0], shown[1]);

    std::fs::write(&second, "video_driver = \"vulkan\"\n").unwrap();
    assert_eq!(set.refresh_freshness(), [ids[1].clone()]);
}

#[test]
fn a_record_that_nothing_was_there_goes_stale_when_something_appears() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("override.cfg");
    let absent = SourceFingerprint::absent(&path);
    assert!(absent.absent);
    assert_eq!(absent.check(), FingerprintCheck::Unchanged);
    let mut set = EvidenceSet::default();
    let id = set
        .push(
            EvidenceArea::EmulatorConfiguration,
            EvidenceState::Observed,
            Trust::Measured,
            "settings file",
            Some(absent.clone()),
            Fact::ConfigLayerAbsent {
                emulator: "retroarch".into(),
                layer: ConfigLayer::Game,
            },
        )
        .unwrap();
    assert!(set.refresh_freshness().is_empty());
    // Looked for and absent is listed, so a mistyped path can be seen.
    let coverage = set.coverage();
    let settings = coverage
        .iter()
        .find(|area| area.area == EvidenceArea::EmulatorConfiguration)
        .unwrap();
    assert_eq!(settings.observed, 1);
    assert_eq!(
        settings.missing,
        ["no settings file exists at the place given for: override for this game"]
    );

    std::fs::write(&path, "video_driver = \"gl\"\n").unwrap();
    assert!(matches!(absent.check(), FingerprintCheck::Changed { .. }));
    assert_eq!(set.refresh_freshness(), [id]);
}

#[test]
fn supplied_facts_are_not_firm() {
    assert!(Trust::Measured.is_firm() && Trust::Reported.is_firm());
    assert!(!Trust::Supplied.is_firm());
    assert!(!Trust::UntrustedText.is_firm() && !Trust::Asserted.is_firm());
    assert!(Trust::Reported < Trust::Supplied && Trust::Supplied < Trust::UntrustedText);
    assert_eq!(
        serde_json::to_string(&Trust::Supplied).unwrap(),
        "\"supplied\""
    );
}
