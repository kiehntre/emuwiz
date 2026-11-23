//! Output capture: what is kept, that it is bounded, and that a process is
//! never blocked or misreported because of it.
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::time::{Duration, Instant};

const START: usize = 256;
const END: usize = 256;

fn keep(input: &[u8]) -> Vec<u8> {
    read_start_and_end(input, START, END)
}

fn marker_count(captured: &[u8]) -> usize {
    String::from_utf8_lossy(captured)
        .lines()
        .filter(|line| {
            line.starts_with(OUTPUT_OMITTED_MARKER_PREFIX)
                && line.ends_with(OUTPUT_OMITTED_MARKER_SUFFIX)
        })
        .count()
}

fn omitted_bytes(captured: &[u8]) -> u64 {
    String::from_utf8_lossy(captured)
        .lines()
        .find_map(|line| {
            line.strip_prefix(OUTPUT_OMITTED_MARKER_PREFIX)?
                .strip_suffix(OUTPUT_OMITTED_MARKER_SUFFIX)?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

fn numbered_lines(count: usize) -> Vec<u8> {
    (0..count)
        .flat_map(|number| format!("line {number:06}\n").into_bytes())
        .collect()
}

#[test]
fn output_that_fits_is_kept_unchanged() {
    assert_eq!(keep(b""), b"");
    assert_eq!(keep(b"one line\n"), b"one line\n");
    assert_eq!(keep(b"no final newline"), b"no final newline");
    // Exactly the two limits together still fits: nothing is left out.
    let exact = numbered_lines((START + END) / 12);
    assert_eq!(exact.len(), START + END - (START + END) % 12);
    assert_eq!(keep(&exact), exact);
    assert_eq!(marker_count(&keep(&exact)), 0);
}

#[test]
fn long_output_keeps_whole_lines_from_both_ends_and_says_what_is_missing() {
    let input = numbered_lines(10_000);
    let captured = keep(&input);
    assert!(captured.len() <= START + END + 80, "{}", captured.len());
    assert_eq!(marker_count(&captured), 1);
    let text = String::from_utf8(captured.clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.first(), Some(&"line 000000"));
    assert_eq!(lines.last(), Some(&"line 009999"));
    // Every kept line is a complete line of the input.
    for line in &lines {
        assert!(
            line.starts_with(OUTPUT_OMITTED_MARKER_PREFIX)
                || (line.len() == 11 && line.starts_with("line ")),
            "{line:?}"
        );
    }
    // Kept bytes plus the stated omission account for every input byte.
    let marker_line = lines
        .iter()
        .find(|line| line.starts_with(OUTPUT_OMITTED_MARKER_PREFIX))
        .unwrap();
    let kept = captured.len() - (marker_line.len() + 1);
    assert_eq!(kept as u64 + omitted_bytes(&captured), input.len() as u64);
}

#[test]
fn one_enormous_line_is_not_presented_in_pieces() {
    let mut input = b"first\n".to_vec();
    input.extend(std::iter::repeat_n(b'x', 100_000));
    input.extend(b"\nlast\n");
    let captured = keep(&input);
    let text = String::from_utf8(captured.clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.first(), Some(&"first"));
    assert_eq!(lines.last(), Some(&"last"));
    assert!(!text.contains("xx"), "a fragment of the long line was kept");
    assert!(captured.len() <= START + END + 80);

    // With no line break at all nothing but the marker can be kept.
    let captured = keep(&vec![b'q'; 50_000]);
    assert_eq!(marker_count(&captured), 1);
    assert_eq!(omitted_bytes(&captured), 50_000);
    assert!(!String::from_utf8_lossy(&captured).contains('q'));
}

/// What is kept, not what the process uses: the reader works through one
/// fixed 8 KiB buffer, which is read from the code and not measured here.
#[test]
fn what_is_kept_stays_within_its_bound_however_the_output_arrives() {
    /// Yields `total` bytes in reads of `step` bytes without ever holding
    /// them, so the test itself cannot hide an unbounded buffer.
    struct Stream {
        left: usize,
        step: usize,
    }
    impl Read for Stream {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.left.min(self.step).min(buffer.len());
            for (index, byte) in buffer[..count].iter_mut().enumerate() {
                *byte = if index % 64 == 63 { b'\n' } else { b'z' };
            }
            self.left -= count;
            Ok(count)
        }
    }
    for (step, total) in [
        (1, 300_000),
        (7, 2 * 1024 * 1024),
        (4096, 64 * 1024 * 1024),
        (1 << 20, 64 * 1024 * 1024),
    ] {
        let captured = read_start_and_end(
            Stream { left: total, step },
            PROCESS_STDERR_CAPTURE_LIMIT,
            DIAGNOSTIC_TAIL_CAPTURE_LIMIT,
        );
        assert!(
            captured.len() <= PROCESS_STDERR_CAPTURE_LIMIT + DIAGNOSTIC_TAIL_CAPTURE_LIMIT + 80,
            "step {step}: {}",
            captured.len()
        );
        assert_eq!(marker_count(&captured), 1, "step {step}");
    }
}

#[test]
fn a_read_error_keeps_what_arrived_before_it() {
    struct Failing(usize);
    impl Read for Failing {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.0 += 1;
            match self.0 {
                1 => {
                    buffer[..6].copy_from_slice(b"early\n");
                    Ok(6)
                }
                2 => Err(std::io::Error::from(std::io::ErrorKind::Interrupted)),
                3 => {
                    buffer[..5].copy_from_slice(b"more\n");
                    Ok(5)
                }
                _ => Err(std::io::Error::other("pipe broke")),
            }
        }
    }
    assert_eq!(keep_from(Failing(0)), b"early\nmore\n");
}

fn keep_from(reader: impl Read) -> Vec<u8> {
    read_start_and_end(reader, START, END)
}

fn script(directory: &std::path::Path, body: &str) -> PreparedProcessCommand {
    let path = directory.join("emulator.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    PreparedProcessCommand {
        executable: path,
        arguments: Vec::new(),
        working_directory: None,
    }
}

fn finished(process: &mut WatchedProcess) -> &ProcessExitReport {
    let deadline = Instant::now() + Duration::from_secs(60);
    while process.poll().is_none() {
        assert!(Instant::now() < deadline, "the test process did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    process.exit_report().unwrap()
}

#[test]
fn a_diagnostic_capture_keeps_both_streams_in_the_order_they_were_written() {
    let dir = tempfile::tempdir().unwrap();
    let command = script(
        dir.path(),
        "echo 'out 1'\necho 'err 1' 1>&2\necho 'out 2'\necho 'err 2' 1>&2\nexit 5",
    );
    let mut process = spawn_watched_process_captured(&command, OutputCapture::Diagnostic).unwrap();
    let report = finished(&mut process);
    assert_eq!(report.status.as_ref().unwrap().code(), Some(5));
    assert_eq!(report.stderr, b"out 1\nerr 1\nout 2\nerr 2\n");
}

#[test]
fn the_output_of_a_finished_process_can_be_let_go_keeping_how_it_ended() {
    let directory = tempfile::tempdir().unwrap();
    let command = script(directory.path(), "echo kept-until-seen >&2; exit 0");
    let mut process = spawn_watched_process(&command).unwrap();
    // While it runs there is nothing to let go of, and asking is harmless.
    process.discard_captured_output();
    let report = finished(&mut process);
    assert!(report.status.as_ref().unwrap().success());
    assert_eq!(report.stderr, b"kept-until-seen\n");

    process.discard_captured_output();
    let report = process.exit_report().unwrap();
    assert!(report.status.as_ref().unwrap().success());
    assert!(report.stderr.is_empty());
    assert_eq!(report.stderr.capacity(), 0);
}

#[test]
fn the_default_capture_still_discards_standard_output() {
    let dir = tempfile::tempdir().unwrap();
    let command = script(dir.path(), "echo 'to stdout'\necho 'to stderr' 1>&2");
    let mut process = spawn_watched_process(&command).unwrap();
    assert_eq!(finished(&mut process).stderr, b"to stderr\n");
    let mut process = spawn_watched_process_captured(&command, OutputCapture::StderrStart).unwrap();
    assert_eq!(finished(&mut process).stderr, b"to stderr\n");
}

#[test]
fn a_process_that_prints_nothing_reports_its_exit_and_an_empty_capture() {
    let dir = tempfile::tempdir().unwrap();
    for (body, code) in [("exit 0", 0), ("exit 9", 9)] {
        let command = script(dir.path(), body);
        let mut process =
            spawn_watched_process_captured(&command, OutputCapture::Diagnostic).unwrap();
        let report = finished(&mut process);
        assert_eq!(report.status.as_ref().unwrap().code(), Some(code));
        assert!(report.stderr.is_empty());
    }
}

#[test]
fn a_very_noisy_process_is_never_blocked_and_its_capture_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    // About 9 MiB across both streams, far more than a pipe holds: the
    // process can only finish if its output is being read to the end.
    let command = script(
        dir.path(),
        "pad=$(printf '%0100d' 0)\necho 'first line'\ni=0\n\
         while [ $i -lt 40000 ]; do echo \"out $i $pad\"; echo \"err $i $pad\" 1>&2; i=$((i+1)); done\n\
         echo 'last line' 1>&2\nexit 3",
    );
    let started = Instant::now();
    let mut process = spawn_watched_process_captured(&command, OutputCapture::Diagnostic).unwrap();
    let report = finished(&mut process);
    assert_eq!(report.status.as_ref().unwrap().code(), Some(3));
    assert!(started.elapsed() < Duration::from_secs(60));
    assert!(
        report.stderr.len() <= PROCESS_STDERR_CAPTURE_LIMIT + DIAGNOSTIC_TAIL_CAPTURE_LIMIT + 80,
        "{}",
        report.stderr.len()
    );
    assert_eq!(marker_count(&report.stderr), 1);
    assert!(omitted_bytes(&report.stderr) > 8_000_000);
    let text = String::from_utf8_lossy(&report.stderr);
    assert_eq!(text.lines().next(), Some("first line"));
    assert_eq!(text.lines().last(), Some("last line"));
}

#[test]
fn output_written_before_a_stop_request_or_a_forced_stop_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    // `exec` makes the watched process the one that sleeps, so nothing else
    // holds the output pipe open after it is stopped. The marker file says
    // the line has been written, so the test never stops the process early.
    let ready = dir.path().join("ready");
    let command = script(
        dir.path(),
        &format!(
            "echo 'started' 1>&2\n: > '{}'\nexec sleep 300",
            ready.display()
        ),
    );
    let wait_until_ready = || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "the test process did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::remove_file(&ready).unwrap();
    };

    let mut stopped = spawn_watched_process_captured(&command, OutputCapture::Diagnostic).unwrap();
    wait_until_ready();
    let outcome =
        stopped.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::from_secs(20));
    assert!(
        matches!(
            outcome,
            GracefulStopOutcome::ExitedAfterRequest | GracefulStopOutcome::AlreadyExited
        ),
        "{outcome:?}"
    );
    let report = finished(&mut stopped);
    assert_eq!(
        report.status.as_ref().unwrap().signal(),
        Some(libc::SIGTERM)
    );
    assert_eq!(report.stderr, b"started\n");

    let mut forced = spawn_watched_process_captured(&command, OutputCapture::Diagnostic).unwrap();
    wait_until_ready();
    // This test's own child, by the pid this test was handed for it.
    let pid = libc::pid_t::try_from(forced.pid).unwrap();
    // SAFETY: kill(2) on a pid this test just spawned and still owns.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    let report = finished(&mut forced);
    assert_eq!(
        report.status.as_ref().unwrap().signal(),
        Some(libc::SIGKILL)
    );
    assert_eq!(report.stderr, b"started\n");
}

/// What happened to a child whose reader went away while it was still
/// running: how it ended, and whether it got as far as writing its save.
fn child_that_loses_its_reader(protect: bool) -> (std::process::ExitStatus, bool) {
    let directory = tempfile::tempdir().unwrap();
    let save = directory.path().join("game.srm");
    let (reader, writer) = std::io::pipe().unwrap();
    let mut process = Command::new("/bin/sh");
    // As an emulator quitting does: say something, then write the save.
    process
        .arg("-c")
        .arg("sleep 0.4; echo 'saving the game' ; printf saved > \"$1\"")
        .arg("sh")
        .arg(&save)
        .stdin(Stdio::null())
        .stdout(writer.try_clone().unwrap())
        .stderr(writer);
    if protect {
        survive_a_lost_reader(&mut process);
    }
    let mut child = process.spawn().unwrap();
    // The command still holds its own copies of the write end; let go of
    // them, then of the only reader, while the child is still asleep.
    drop(process);
    drop(reader);
    let status = child.wait().unwrap();
    (status, save.is_file())
}

#[test]
fn an_emulator_that_loses_its_reader_still_quits_normally_and_writes_its_save() {
    let (status, saved) = child_that_loses_its_reader(true);
    assert!(status.success(), "{status:?}");
    assert!(saved, "the save was not written");
}

/// The hazard itself, so the test above cannot pass by accident: without
/// the protection the same child is killed at its first word and never
/// reaches its save.
#[test]
fn without_that_protection_the_same_emulator_is_killed_before_it_saves() {
    let (status, saved) = child_that_loses_its_reader(false);
    assert_eq!(status.signal(), Some(libc::SIGPIPE), "{status:?}");
    assert!(!saved);
}

#[test]
fn a_diagnostic_capture_starts_its_process_ignoring_a_lost_reader_and_no_other_capture_does() {
    let directory = tempfile::tempdir().unwrap();
    // Raising the signal on itself kills the process unless it was started
    // ignoring it.
    let command = script(directory.path(), "kill -PIPE $$\necho survived\n");
    let mut diagnostic =
        spawn_watched_process_captured(&command, OutputCapture::Diagnostic).unwrap();
    let report = finished(&mut diagnostic);
    assert!(report.status.as_ref().unwrap().success(), "{report:?}");
    assert!(String::from_utf8_lossy(&report.stderr).contains("survived"));
    // Every other launch is started exactly as before.
    let mut ordinary =
        spawn_watched_process_captured(&command, OutputCapture::StderrStart).unwrap();
    let report = finished(&mut ordinary);
    assert_eq!(
        report.status.as_ref().unwrap().signal(),
        Some(libc::SIGPIPE),
        "{report:?}"
    );
}
