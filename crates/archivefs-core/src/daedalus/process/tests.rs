use super::*;
use crate::daedalus::evidence::{Fact, LogMeaning};
use crate::daedalus::retroarch;
use crate::daedalus::test_fixtures as fixtures;
use crate::launch::process_spawn::PROCESS_STDERR_CAPTURE_LIMIT;

fn status(raw: i32) -> std::io::Result<ExitStatus> {
    Ok(ExitStatus::from_raw(raw))
}

#[test]
fn exit_codes_and_signals_are_told_apart() {
    assert_eq!(exit_outcome(&status(0)), ExitOutcome::Exited { code: 0 });
    // wait(2) status: exit code in the high byte.
    assert_eq!(
        exit_outcome(&status(1 << 8)),
        ExitOutcome::Exited { code: 1 }
    );
    assert_eq!(
        exit_outcome(&status(134 << 8)),
        ExitOutcome::Exited { code: 134 }
    );
    assert_eq!(
        exit_outcome(&status(libc::SIGSEGV)),
        ExitOutcome::Signaled {
            signal: libc::SIGSEGV,
            core_dumped: false
        }
    );
    assert_eq!(
        exit_outcome(&status(libc::SIGABRT | 0x80)),
        ExitOutcome::Signaled {
            signal: libc::SIGABRT,
            core_dumped: true
        }
    );
    let failed: std::io::Result<ExitStatus> = Err(std::io::Error::other("wait failed"));
    assert_eq!(exit_outcome(&failed), ExitOutcome::StatusUnavailable);
}

#[test]
fn spawn_errors_keep_the_operating_system_reason() {
    let missing = std::io::Error::from_raw_os_error(libc::ENOENT);
    assert_eq!(
        spawn_failure(&missing),
        ExitOutcome::SpawnFailed {
            reason: SpawnFailure::ProgramNotFound,
            os_error: Some(libc::ENOENT)
        }
    );
    let denied = std::io::Error::from_raw_os_error(libc::EACCES);
    assert!(matches!(
        spawn_failure(&denied),
        ExitOutcome::SpawnFailed {
            reason: SpawnFailure::PermissionDenied,
            ..
        }
    ));
    let other = std::io::Error::from_raw_os_error(libc::ENOEXEC);
    assert!(matches!(
        spawn_failure(&other),
        ExitOutcome::SpawnFailed {
            reason: SpawnFailure::Other,
            ..
        }
    ));
}

#[test]
fn signals_are_classified_and_unknown_ones_are_not_guessed() {
    assert_eq!(signal_info(libc::SIGSEGV).unwrap().kind, SignalKind::Fault);
    assert_eq!(signal_info(libc::SIGABRT).unwrap().name, "SIGABRT");
    assert_eq!(
        signal_info(libc::SIGTERM).unwrap().kind,
        SignalKind::StopRequest
    );
    assert_eq!(
        signal_info(libc::SIGKILL).unwrap().kind,
        SignalKind::ForcedKill
    );
    assert!(signal_info(63).is_none());
    assert!(signal_info(0).is_none());
}

#[test]
fn a_relayed_signal_is_only_a_possibility_within_the_signal_range() {
    assert_eq!(possibly_relayed_signal(134), Some(libc::SIGABRT));
    assert_eq!(possibly_relayed_signal(139), Some(libc::SIGSEGV));
    assert_eq!(possibly_relayed_signal(1), None);
    assert_eq!(possibly_relayed_signal(127), None);
    assert_eq!(possibly_relayed_signal(128), None);
    assert_eq!(possibly_relayed_signal(255), None);
    assert_eq!(possibly_relayed_signal(i32::MIN), None);
}

#[test]
fn an_exit_report_becomes_a_measured_exit_plus_scanned_stderr() {
    let report = ProcessExitReport {
        status: status(1 << 8),
        stderr: fixtures::RA_VULKAN_NO_DRIVER_QUIET.as_bytes().to_vec(),
    };
    let mut set = EvidenceSet::default();
    let exit = record_exit_report(
        &mut set,
        &report,
        Some(Duration::from_millis(1_488)),
        Some("flatpak run\x1b"),
        retroarch::EMULATOR,
        retroarch::SIGNATURES,
    )
    .unwrap();
    let item = set.get(&exit).unwrap();
    assert_eq!(item.trust, Trust::Measured);
    let Fact::ProcessExit(recorded) = &item.fact else {
        panic!("expected a process exit");
    };
    assert_eq!(recorded.outcome, ExitOutcome::Exited { code: 1 });
    assert_eq!(recorded.duration_ms, Some(1_488));
    assert!(!recorded.launcher.as_deref().unwrap().contains('\x1b'));
    let events: Vec<LogMeaning> = set
        .items()
        .iter()
        .filter_map(|item| match &item.fact {
            Fact::LogEvent(event) => Some(event.meaning),
            _ => None,
        })
        .collect();
    assert_eq!(events, [LogMeaning::VideoDriverInitFailed]);
}

#[test]
fn a_full_stderr_capture_is_reported_as_cut_short() {
    let mut set = EvidenceSet::default();
    record_stderr(
        &mut set,
        &[b'x'; PROCESS_STDERR_CAPTURE_LIMIT],
        "retroarch",
        &[],
    );
    let cut = set
        .items()
        .iter()
        .any(|item| matches!(&item.fact, Fact::LogCoverage(coverage) if coverage.truncated));
    assert!(cut);

    let mut set = EvidenceSet::default();
    record_stderr(&mut set, b"short\n", "retroarch", &[]);
    let cut = set
        .items()
        .iter()
        .any(|item| matches!(&item.fact, Fact::LogCoverage(coverage) if coverage.truncated));
    assert!(!cut);
}

#[test]
fn a_diagnostic_capture_is_cut_short_only_where_it_says_output_was_left_out() {
    use crate::launch::process_spawn::{
        OUTPUT_OMITTED_MARKER_PREFIX, OUTPUT_OMITTED_MARKER_SUFFIX,
    };
    let cut_short = |bytes: &[u8]| {
        let mut set = EvidenceSet::default();
        record_stderr(&mut set, bytes, "retroarch", &[]);
        set.items()
            .iter()
            .any(|item| matches!(&item.fact, Fact::LogCoverage(coverage) if coverage.truncated))
    };

    // More than a plain capture holds, and complete: a start-and-end capture
    // kept every byte, so nothing was left out.
    let whole = b"line\n".repeat(PROCESS_STDERR_CAPTURE_LIMIT / 5 + 100);
    assert!(whole.len() > PROCESS_STDERR_CAPTURE_LIMIT);
    assert!(!cut_short(&whole));

    let mut with_gap = b"start\n".to_vec();
    with_gap.extend_from_slice(
        format!("{OUTPUT_OMITTED_MARKER_PREFIX}4096{OUTPUT_OMITTED_MARKER_SUFFIX}\n").as_bytes(),
    );
    with_gap.extend_from_slice(b"end\n");
    assert!(cut_short(&with_gap));

    // The marker is a whole line. The same words inside a longer line are
    // something the program printed.
    let inline =
        format!("core said {OUTPUT_OMITTED_MARKER_PREFIX}1{OUTPUT_OMITTED_MARKER_SUFFIX} ok\n");
    assert!(!cut_short(inline.as_bytes()));
}
