//! Night Watch D8-01 / S05-01: an external program is only ever started
//! from an absolute path that was checked first. A bare name or a relative
//! path would let `execvp` search `PATH` (including `.`, empty and relative
//! entries) under rules nobody validated.

use archivefs_core::launch::process_spawn::{PreparedProcessCommand, spawn_watched_process};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn command(executable: &str) -> PreparedProcessCommand {
    PreparedProcessCommand {
        executable: PathBuf::from(executable),
        arguments: vec!["-c".into(), "exit 0".into()],
        working_directory: None,
    }
}

#[cfg(unix)]
#[test]
fn spawning_a_non_absolute_executable_is_refused_before_fork() {
    for executable in ["sh", "./sh", "bin/sh", "../sh", ""] {
        let error = spawn_watched_process(&command(executable))
            .err()
            .unwrap_or_else(|| panic!("{executable:?} was started"));
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "{executable:?}"
        );
        assert!(error.to_string().contains("non-absolute"), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn spawning_an_absolute_executable_still_works() {
    let mut process = spawn_watched_process(&command("/bin/sh")).expect("absolute path starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while process.poll().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!process.is_running(), "/bin/sh -c 'exit 0' did not finish");
}
