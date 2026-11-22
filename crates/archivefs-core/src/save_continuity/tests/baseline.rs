//! A "before" mark read while a launch is already under way is the starting
//! content only when that is proven, save by save.

use super::super::discover::DiscoveryRoots;
use super::super::session::*;
use super::super::tracker::{LaunchTracker, clock_offset_ms, launch_start_unix};
use crate::launch::process_log::{ExitSummary, StartedProcess};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

/// One process that a test can end.
struct One(Cell<bool>);

impl ProcessTable for One {
    fn start_ticks(&self, pid: u32) -> Option<u64> {
        (pid == 4242 && self.0.get()).then_some(77)
    }
}

fn late_mark(id: &str, sha: &str, last_changed_unix: Option<i64>) -> SaveMark {
    SaveMark {
        save_id: id.into(),
        label: id.into(),
        content_sha256: sha.into(),
        last_changed_unix,
        before_launch: Some(false),
        folder: None,
    }
}

fn plain_mark(id: &str, sha: &str) -> SaveMark {
    SaveMark {
        save_id: id.into(),
        label: id.into(),
        content_sha256: sha.into(),
        last_changed_unix: None,
        before_launch: None,
        folder: None,
    }
}

fn launch() -> SessionLaunch {
    SessionLaunch {
        emulator: "ProbeEmu".into(),
        profile: "native".into(),
        ..Default::default()
    }
}

#[test]
fn a_mark_is_proven_only_when_its_file_last_changed_clearly_before_the_launch() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path(), "device-a").unwrap();
    let before = vec![
        late_mark("early", "1", Some(1_000)),
        late_mark("edge", "2", Some(1_000 + 100 - BASELINE_MARGIN_SECONDS)),
        late_mark(
            "too-close",
            "3",
            Some(1_000 + 100 - BASELINE_MARGIN_SECONDS + 1),
        ),
        late_mark("after-start", "4", Some(1_000 + 105)),
        late_mark("unreadable", "5", None),
    ];
    let session = journal
        .request(SessionGame::default(), launch(), before)
        .unwrap();
    let judged = journal
        .judge_baselines(&session.session_id, Some(1_100))
        .unwrap();
    let proven: Vec<(&str, Option<bool>)> = judged
        .saves_before
        .iter()
        .map(|mark| (mark.save_id.as_str(), mark.before_launch))
        .collect();
    assert_eq!(
        proven,
        vec![
            ("early", Some(true)),
            ("edge", Some(true)),
            ("too-close", Some(false)),
            ("after-start", Some(false)),
            ("unreadable", Some(false)),
        ]
    );
    // Stored, not only returned.
    assert_eq!(journal.get(&session.session_id).unwrap(), judged);
}

#[test]
fn without_a_known_launch_time_nothing_is_proven() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path(), "device-a").unwrap();
    let session = journal
        .request(
            SessionGame::default(),
            launch(),
            vec![late_mark("early", "1", Some(1_000))],
        )
        .unwrap();
    let judged = journal.judge_baselines(&session.session_id, None).unwrap();
    assert_eq!(judged.saves_before[0].before_launch, Some(false));
}

#[test]
fn an_unproven_mark_is_never_reported_as_the_starting_content() {
    let proven = |id: &str, sha: &str| SaveMark {
        before_launch: Some(true),
        ..late_mark(id, sha, Some(1))
    };
    let before = [
        proven("proven-same", "a"),
        proven("proven-changed", "b"),
        late_mark("late-same", "c", Some(5)),
        late_mark("late-changed", "d", Some(5)),
        plain_mark("read-first-changed", "e"),
        plain_mark("read-first-same", "f"),
    ];
    let after = [
        plain_mark("proven-same", "a"),
        plain_mark("proven-changed", "b2"),
        plain_mark("late-same", "c"),
        plain_mark("late-changed", "d2"),
        plain_mark("read-first-changed", "e2"),
        plain_mark("read-first-same", "f"),
    ];
    let mut changes = diff_saves(&before, &after);
    changes.sort_by_key(|change| format!("{change:?}"));
    assert_eq!(
        changes,
        vec![
            SaveChange::Modified {
                label: "proven-changed".into()
            },
            SaveChange::Modified {
                label: "read-first-changed".into()
            },
            SaveChange::ModifiedBeforeNotKnown {
                label: "late-changed".into()
            },
            SaveChange::NotKnown {
                label: "late-same".into()
            },
        ]
    );
}

fn home_with_saves(home: &Path) {
    let config = home.join(".config/retroarch");
    fs::create_dir_all(config.join("saves")).unwrap();
    fs::write(
        config.join("retroarch.cfg"),
        "savefile_directory = \"default\"\n",
    )
    .unwrap();
    fs::write(config.join("saves/Played (USA).srm"), [1u8; 8192]).unwrap();
    fs::write(config.join("saves/Untouched (USA).srm"), [2u8; 8192]).unwrap();
}

fn started() -> StartedProcess {
    StartedProcess {
        sequence: 1,
        pid: 4242,
        start_ticks: Some(77),
        // Not a readable file: treated as a launcher named like RetroArch.
        executable: PathBuf::from("/nowhere/retroarch"),
        arguments: Vec::new(),
        exit: None,
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Runs one tracked launch over real files. `launch_start` is when the
/// launch's first process is said to have started.
fn tracked(launch_start: Option<i64>) -> SessionRecord {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
    home_with_saves(&home);
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
    let table = One(Cell::new(true));
    let session = tracker.begin(SessionGame::default(), "RetroArch").unwrap();
    assert_eq!(session.saves_before.len(), 2);
    assert!(
        session
            .saves_before
            .iter()
            .all(|mark| mark.before_launch == Some(false) && mark.last_changed_unix.is_some()),
        "unproven until the launch time is known: {:?}",
        session.saves_before
    );
    tracker
        .process_started_at(&session.session_id, &started(), &table, launch_start)
        .unwrap();
    // The game writes one save and creates another; one is left alone.
    let saves = home.join(".config/retroarch/saves");
    fs::write(saves.join("Played (USA).srm"), [9u8; 8192]).unwrap();
    fs::write(saves.join("Brand New (USA).srm"), [7u8; 8192]).unwrap();
    table.0.set(false);
    tracker
        .finished_in(
            &session.session_id,
            Some(ExitSummary::Code(0)),
            None,
            &table,
        )
        .unwrap()
}

#[test]
fn files_untouched_since_well_before_the_launch_give_a_plain_report() {
    // The files were written just now; the launch is said to start later.
    let ended = tracked(Some(now() + 60));
    assert!(
        ended
            .saves_before
            .iter()
            .all(|mark| mark.before_launch == Some(true))
    );
    let mut changes = ended.save_changes.clone();
    changes.sort_by_key(|change| format!("{change:?}"));
    assert_eq!(
        changes,
        vec![
            SaveChange::Created {
                label: "Brand New (USA).srm".into()
            },
            SaveChange::Modified {
                label: "Played (USA).srm".into()
            },
        ]
    );
}

#[test]
fn files_changed_around_the_launch_start_are_reported_as_not_known() {
    // The files were written in the same moment the launch is said to
    // start: they may already be the emulator's own writing.
    let ended = tracked(Some(now()));
    let mut changes = ended.save_changes.clone();
    changes.sort_by_key(|change| format!("{change:?}"));
    assert_eq!(
        changes,
        vec![
            SaveChange::CreatedBeforeNotKnown {
                label: "Brand New (USA).srm".into()
            },
            SaveChange::ModifiedBeforeNotKnown {
                label: "Played (USA).srm".into()
            },
            SaveChange::NotKnown {
                label: "Untouched (USA).srm".into()
            },
        ]
    );
    assert!(tracked(None).save_changes.iter().all(|change| !matches!(
        change,
        SaveChange::Modified { .. } | SaveChange::Created { .. } | SaveChange::Missing { .. }
    )));
}

#[test]
fn new_and_gone_are_plain_only_with_proof() {
    let folder = |path: &str, before_launch| FolderMark {
        path: PathBuf::from(path),
        last_changed_unix: Some(1),
        before_launch,
    };
    let in_folder = |id: &str, path: &str| SaveMark {
        folder: Some(PathBuf::from(path)),
        ..plain_mark(id, "x")
    };
    let before = [
        late_mark("made-by-the-emulator-then-removed", "a", Some(9)),
        SaveMark {
            before_launch: Some(true),
            ..late_mark("really-gone", "b", Some(1))
        },
    ];
    let after = [
        in_folder("in-a-quiet-folder", "/saves/quiet"),
        in_folder("in-a-busy-folder", "/saves/busy"),
        in_folder("in-a-folder-not-seen-before", "/saves/other"),
        plain_mark("folder-not-stated", "x"),
    ];
    let folders = [folder("/saves/quiet", true), folder("/saves/busy", false)];
    let mut changes = diff_saves_with(&before, &after, Some(&folders));
    changes.sort_by_key(|change| format!("{change:?}"));
    assert_eq!(
        changes,
        vec![
            SaveChange::Created {
                label: "in-a-quiet-folder".into()
            },
            SaveChange::CreatedBeforeNotKnown {
                label: "folder-not-stated".into()
            },
            SaveChange::CreatedBeforeNotKnown {
                label: "in-a-busy-folder".into()
            },
            SaveChange::CreatedBeforeNotKnown {
                label: "in-a-folder-not-seen-before".into()
            },
            SaveChange::Missing {
                label: "really-gone".into()
            },
            SaveChange::NotKnown {
                label: "made-by-the-emulator-then-removed".into()
            },
        ]
    );
}

#[test]
fn a_backdated_modification_time_proves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
    home_with_saves(&home);
    let started_at = now();
    // The file claims to be an hour old; the system's own change time
    // still says "just now".
    let save = home.join(".config/retroarch/saves/Played (USA).srm");
    fs::File::options()
        .write(true)
        .open(&save)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
    let session = tracker.begin(SessionGame::default(), "RetroArch").unwrap();
    for mark in &session.saves_before {
        assert!(mark.last_changed_unix.unwrap() >= started_at, "{mark:?}");
    }
    let folders = session.folders_before.unwrap();
    assert_eq!(folders.len(), 1);
    assert!(folders[0].last_changed_unix.unwrap() >= started_at);
    assert!(!folders[0].before_launch);
}

#[test]
fn the_launch_start_is_stated_only_on_an_unstepped_clock() {
    let before = clock_offset_ms();
    assert!(before.is_some());
    let earliest = now();
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let stat = fs::read_to_string(format!("/proc/{}/stat", child.id())).unwrap();
    let ticks: u64 = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    let start = launch_start_unix(Some(ticks), before, clock_offset_ms(), now());
    child.kill().unwrap();
    child.wait().unwrap();
    let start = start.expect("a real process on an unstepped clock has a start time");
    assert!(
        (earliest - 1..=now()).contains(&start),
        "{start} vs {earliest}"
    );

    // The wall clock was stepped (either way) since the note: nothing.
    let offset = before.unwrap();
    for step in [10_000, -10_000] {
        assert_eq!(
            launch_start_unix(Some(ticks), Some(offset - step), Some(offset), now()),
            None
        );
    }
    // Not noted before the launch, or no start ticks: nothing.
    assert_eq!(
        launch_start_unix(Some(ticks), None, clock_offset_ms(), now()),
        None
    );
    assert_eq!(
        launch_start_unix(None, before, clock_offset_ms(), now()),
        None
    );
    // A start that would lie in the future is not a start time.
    assert_eq!(
        launch_start_unix(Some(ticks), before, before, earliest - 3600),
        None
    );
}

/// By hand only: how long the launch-time read of the saves takes over a
/// given home folder, first with no remembered contents and then with them.
/// `LAZARUS_PERF_HOME` is the home to read (nothing is written there);
/// `LAZARUS_PERF_ROOT` is an empty scratch folder for the journal.
#[test]
#[ignore = "by hand: needs LAZARUS_PERF_HOME and LAZARUS_PERF_ROOT"]
fn launch_time_save_reading_by_hand() {
    let home = PathBuf::from(std::env::var("LAZARUS_PERF_HOME").unwrap());
    let root = PathBuf::from(std::env::var("LAZARUS_PERF_ROOT").unwrap());
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "perf").unwrap();
    for run in ["no remembered contents", "remembered", "remembered again"] {
        let began = std::time::Instant::now();
        let session = tracker.begin(SessionGame::default(), "RetroArch").unwrap();
        println!(
            "PERF {run}: {:.3} s, {} saves marked",
            began.elapsed().as_secs_f64(),
            session.saves_before.len()
        );
    }
}
