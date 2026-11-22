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
        path: None,
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
        path: None,
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
        gate_token: None,
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
    let mut changes = diff_saves_with(&before, &after, Some(&folders), &[], &[]);
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
    // What a launch through the gate costs: the started emulator's saves,
    // read afresh, once before the start (and the same again afterwards).
    for (program, flatpak) in [
        ("retroarch", false),
        ("retroarch", true),
        ("pcsx2", false),
        ("pcsx2", true),
        ("rpcs3", false),
        ("rpcs3", true),
        ("dolphin-emu", true),
        ("ppsspp", true),
    ] {
        let mut launch =
            super::super::tracker::classify_started(Path::new(&format!("/nowhere/{program}")), &[]);
        if flatpak {
            launch.profile = "flatpak".into();
        }
        for run in 1..=3 {
            let began = std::time::Instant::now();
            let baseline = tracker.baseline_before_start(&launch).unwrap();
            println!(
                "PERF {} {} run {run}: {:.1} ms, {} saves, {} unread",
                launch.emulator,
                launch.profile,
                began.elapsed().as_secs_f64() * 1000.0,
                baseline.marks.len(),
                baseline.unread.len()
            );
        }
    }
}

// ----- The gated launch: saves read, afresh, before the emulator exists -----

struct Gated {
    _dir: tempfile::TempDir,
    root: PathBuf,
    saves: PathBuf,
    tracker: LaunchTracker,
    table: One,
}

fn gated() -> Gated {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
    home_with_saves(&home);
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
    Gated {
        saves: home.join(".config/retroarch/saves"),
        root,
        tracker,
        table: One(Cell::new(true)),
        _dir: dir,
    }
}

fn retroarch() -> SessionLaunch {
    super::super::tracker::classify_started(Path::new("/nowhere/retroarch"), &[])
}

impl Gated {
    /// Request, read before the start, start. Returns the session id.
    fn start(&self) -> String {
        let session = self
            .tracker
            .begin_gated(SessionGame::default(), "RetroArch")
            .unwrap();
        let baseline = self.tracker.baseline_before_start(&retroarch()).unwrap();
        self.tracker
            .process_started_gated(
                &session.session_id,
                &started(),
                Ok((baseline, 3, 0)),
                &self.table,
            )
            .unwrap();
        session.session_id
    }

    fn end(&self, session_id: &str) -> SessionRecord {
        self.table.0.set(false);
        self.tracker
            .finished_in(session_id, Some(ExitSummary::Code(0)), None, &self.table)
            .unwrap()
    }
}

fn sorted(mut changes: Vec<SaveChange>) -> Vec<SaveChange> {
    changes.sort_by_key(|change| format!("{change:?}"));
    changes
}

#[test]
fn a_same_size_change_with_its_old_date_put_back_is_still_changed() {
    let run = gated();
    let played = run.saves.join("Played (USA).srm");
    let dated = fs::metadata(&played).unwrap().modified().unwrap();
    let session = run.start();
    // Same size, one byte different, modification time put back.
    let mut bytes = fs::read(&played).unwrap();
    bytes[4000] ^= 0x01;
    fs::write(&played, &bytes).unwrap();
    fs::File::options()
        .write(true)
        .open(&played)
        .unwrap()
        .set_modified(dated)
        .unwrap();
    assert_eq!(fs::metadata(&played).unwrap().modified().unwrap(), dated);
    let ended = run.end(&session);
    assert!(matches!(
        ended.save_tracking,
        Some(SaveTracking::Established { .. })
    ));
    assert_eq!(
        ended.save_changes,
        vec![SaveChange::Modified {
            label: "Played (USA).srm".into()
        }]
    );
}

#[test]
fn a_remembered_fingerprint_is_never_used_for_a_launch() {
    use super::super::discover::{DiscoveryCache, DiscoveryOptions, discover};
    let run = gated();
    let roots = DiscoveryRoots::under_home(run.saves.ancestors().nth(3).unwrap());
    // A memory of the saves whose file sizes, times and file numbers all
    // still match, but whose remembered content is wrong: exactly what a
    // change that left size and times alone would leave behind.
    let cache_file = run.root.join("discovery-cache.json");
    let mut cache = DiscoveryCache::default();
    fn options(cache: &mut DiscoveryCache) -> DiscoveryOptions<'_> {
        DiscoveryOptions {
            cache: Some(cache),
            ..Default::default()
        }
    }
    let real = discover(&roots, options(&mut cache)).unwrap().saves;
    cache.save(&cache_file).unwrap();
    let stale = "0".repeat(64);
    let mut text = fs::read_to_string(&cache_file).unwrap();
    for save in &real {
        text = text.replace(save.content_sha256.as_ref().unwrap(), &stale);
    }
    fs::write(&cache_file, text).unwrap();
    let mut poisoned = DiscoveryCache::load(&cache_file);
    let remembered = discover(&roots, options(&mut poisoned)).unwrap().saves;
    assert!(
        remembered
            .iter()
            .all(|save| save.content_sha256.as_deref() == Some(stale.as_str())),
        "the memory must be in effect for this test to mean anything"
    );

    // The read before the start reports what the files really hold.
    let baseline = run.tracker.baseline_before_start(&retroarch()).unwrap();
    let mut read: Vec<&str> = baseline
        .marks
        .iter()
        .map(|mark| mark.content_sha256.as_str())
        .collect();
    let mut truth: Vec<&str> = real
        .iter()
        .map(|save| save.content_sha256.as_deref().unwrap())
        .collect();
    read.sort();
    truth.sort();
    assert_eq!(read, truth);
    // And so does the read after: an untouched session reports no change.
    let session = run.start();
    assert!(run.end(&session).save_changes.is_empty());
}

#[test]
fn a_save_that_cannot_be_read_is_not_known_rather_than_new_or_gone() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid only reads the process's own id.
    if unsafe { libc::geteuid() } == 0 {
        return; // root reads everything; the case cannot be made.
    }
    let lock = |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));
    let run = gated();
    let untouched = run.saves.join("Untouched (USA).srm");

    // Unreadable before the start, readable afterwards: not "new".
    lock(&untouched, 0o000).unwrap();
    let session = run.start();
    let record = run.tracker.journal().get(&session).unwrap();
    assert_eq!(record.saves_before.len(), 1);
    assert_eq!(record.saves_unread_before.len(), 1, "{record:?}");
    lock(&untouched, 0o644).unwrap();
    fs::write(run.saves.join("Brand New (USA).srm"), [7u8; 64]).unwrap();
    assert_eq!(
        sorted(run.end(&session).save_changes),
        vec![
            SaveChange::Created {
                label: "Brand New (USA).srm".into()
            },
            SaveChange::CreatedBeforeNotKnown {
                label: "Untouched (USA).srm".into()
            },
        ]
    );

    // Readable before, unreadable afterwards: not "gone".
    let run = gated();
    let untouched = run.saves.join("Untouched (USA).srm");
    let session = run.start();
    lock(&untouched, 0o000).unwrap();
    let ended = run.end(&session);
    lock(&untouched, 0o644).unwrap();
    assert_eq!(
        ended.save_changes,
        vec![SaveChange::NotKnown {
            label: "Untouched (USA).srm".into()
        }]
    );
    assert!(!ended.needs_attention(), "nothing is known to be lost");
}

#[test]
fn only_the_started_emulators_saves_are_read_and_another_emulators_baseline_is_refused() {
    let run = gated();
    // A PPSSPP launch on this profile: RetroArch's saves are not its saves.
    let ppsspp = super::super::tracker::classify_started(Path::new("/nowhere/ppsspp"), &[]);
    let baseline = run.tracker.baseline_before_start(&ppsspp).unwrap();
    assert!(baseline.marks.is_empty() && baseline.unread.is_empty());
    // Handed to a RetroArch start, it is not taken for RetroArch's baseline.
    let session = run
        .tracker
        .begin_gated(SessionGame::default(), "RetroArch")
        .unwrap();
    let record = run
        .tracker
        .process_started_gated(
            &session.session_id,
            &started(),
            Ok((baseline, 1, 0)),
            &run.table,
        )
        .unwrap();
    assert!(matches!(
        record.save_tracking,
        Some(SaveTracking::Unavailable { .. })
    ));
    fs::write(run.saves.join("Played (USA).srm"), [9u8; 8192]).unwrap();
    let ended = run.end(&session.session_id);
    assert!(ended.save_changes.is_empty() && ended.saves_after.is_none());
}

#[test]
fn a_save_unreadable_at_both_ends_is_said_to_be_not_known() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid only reads the process's own id.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let run = gated();
    let untouched = run.saves.join("Untouched (USA).srm");
    fs::set_permissions(&untouched, fs::Permissions::from_mode(0o000)).unwrap();
    let session = run.start();
    let ended = run.end(&session);
    fs::set_permissions(&untouched, fs::Permissions::from_mode(0o644)).unwrap();
    // Not silence: silence would read as "nothing changed".
    assert_eq!(
        ended.save_changes,
        vec![SaveChange::NotKnown {
            label: "Untouched (USA).srm".into()
        }]
    );
}

#[test]
fn a_save_folder_that_cannot_be_listed_is_never_taken_for_an_empty_one() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid only reads the process's own id.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let lock = |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));

    // Before the start: no baseline at all, rather than an empty one.
    let run = gated();
    lock(&run.saves, 0o000).unwrap();
    let read = run.tracker.baseline_before_start(&retroarch());
    lock(&run.saves, 0o755).unwrap();
    assert!(read.is_err(), "{read:?}");

    // After the session: the end is recorded, nothing is compared, and no
    // save is called gone.
    let run = gated();
    let session = run.start();
    lock(&run.saves, 0o000).unwrap();
    let ended = run.end(&session);
    lock(&run.saves, 0o755).unwrap();
    assert_eq!(ended.state, SessionState::Ended);
    assert!(ended.saves_after.is_none() && ended.save_changes.is_empty());
    assert!(!ended.needs_attention());

    // The save folder moved between the two reads: nothing is compared.
    let run = gated();
    let session = run.start();
    let config = run.saves.parent().unwrap().join("retroarch.cfg");
    fs::create_dir_all(run.saves.parent().unwrap().join("elsewhere")).unwrap();
    fs::write(
        &config,
        format!(
            "savefile_directory = \"{}\"\n",
            run.saves.parent().unwrap().join("elsewhere").display()
        ),
    )
    .unwrap();
    let ended = run.end(&session);
    assert!(ended.saves_after.is_none() && ended.save_changes.is_empty());
}

#[test]
fn a_gated_session_is_untracked_until_its_read_is_handed_over() {
    let run = gated();
    let session = run
        .tracker
        .begin_gated(SessionGame::default(), "RetroArch")
        .unwrap();
    assert!(matches!(
        session.save_tracking,
        Some(SaveTracking::Unavailable { .. })
    ));
}

fn high_water_kb() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        })
        .unwrap_or(0)
}

/// By hand only: what a launch costs with `LAZARUS_HISTORY_N` sessions
/// already in the journal. Everything is synthetic and lives in a temporary
/// folder (under `TMPDIR`).
#[test]
#[ignore = "by hand: needs LAZARUS_HISTORY_N"]
fn launch_cost_with_a_long_history_by_hand() {
    use super::super::tracker::read_baseline;
    use std::time::Instant;
    let count: usize = std::env::var("LAZARUS_HISTORY_N").unwrap().parse().unwrap();
    let run = gated();
    let ms = |began: Instant| began.elapsed().as_secs_f64() * 1000.0;

    // One real finished session as the pattern, then `count` copies of it
    // under their own names and ids, all over.
    let pattern = run.end(&run.start());
    run.table.0.set(true);
    let text = serde_json::to_string_pretty(&pattern).unwrap();
    let sessions = run.root.join("sessions");
    for old in fs::read_dir(&sessions).unwrap().flatten() {
        fs::remove_file(old.path()).unwrap();
    }
    for index in 0..count {
        let id = format!("{index:016x}");
        let requested = 1_600_000_000 + index as i64;
        let record = text.replace(&pattern.session_id, &id).replace(
            &format!("\"requested_unix\": {}", pattern.requested_unix),
            &format!("\"requested_unix\": {requested}"),
        );
        fs::write(
            sessions.join(format!(
                "{requested:012}-{:09}-{id}.json",
                pattern.requested_order
            )),
            record,
        )
        .unwrap();
    }
    let journal = run.tracker.journal();
    println!("HISTORY n={count}");

    // What every look-up used to cost: reading the whole history.
    let began = Instant::now();
    assert_eq!(journal.all().len(), count);
    println!(
        "HISTORY n={count} whole history read once (old cost of each look-up): {:.1} ms",
        ms(began)
    );

    // A journal from before the open list: the list is built once.
    let _ = fs::remove_file(run.root.join("sessions-open/.complete"));
    let began = Instant::now();
    let first = run
        .tracker
        .begin_gated(SessionGame::default(), "RetroArch")
        .unwrap();
    println!(
        "HISTORY n={count} first launch after upgrade (list built once): {:.1} ms",
        ms(began)
    );
    run.tracker
        .finished(&first.session_id, None, Some("not started"))
        .unwrap();

    // An ordinary launch, step by step.
    let mut worst = 0f64;
    let mut step = |name: &str, began: Instant| {
        let took = ms(began);
        worst = worst.max(took);
        println!("HISTORY n={count} {name}: {took:.1} ms");
    };
    let began = Instant::now();
    let baseline = read_baseline(
        &DiscoveryRoots::under_home(run.saves.ancestors().nth(3).unwrap()),
        &retroarch(),
        None,
    )
    .unwrap();
    step("read before the start (what holds the launch)", began);
    let began = Instant::now();
    let session = run
        .tracker
        .begin_gated(SessionGame::default(), "RetroArch")
        .unwrap();
    step("session requested", began);
    let began = Instant::now();
    run.tracker
        .process_started_gated(
            &session.session_id,
            &started(),
            Ok((baseline, 1, 0)),
            &run.table,
        )
        .unwrap();
    step("process recorded", began);
    let began = Instant::now();
    for _ in 0..10 {
        journal
            .observe_running(&session.session_id, &run.table)
            .unwrap();
    }
    step("ten looks while running", began);
    let began = Instant::now();
    run.tracker.settle();
    step("settle", began);
    fs::write(run.saves.join("Played (USA).srm"), [5u8; 8192]).unwrap();
    let began = Instant::now();
    let ended = run.end(&session.session_id);
    step("session ended (second read and comparison)", began);
    assert_eq!(
        ended.save_changes,
        vec![SaveChange::Modified {
            label: "Played (USA).srm".into()
        }]
    );
    println!("HISTORY n={count} slowest bookkeeping step: {worst:.1} ms");

    // The read before a start while bookkeeping runs flat out beside it.
    run.table.0.set(true);
    let roots = DiscoveryRoots::under_home(run.saves.ancestors().nth(3).unwrap());
    let time_reads = |label: &str| {
        let mut times: Vec<f64> = (0..200)
            .map(|_| {
                let began = Instant::now();
                read_baseline(&roots, &retroarch(), None).unwrap();
                ms(began)
            })
            .collect();
        times.sort_by(f64::total_cmp);
        println!(
            "HISTORY n={count} read before the start, {label}: median {:.2} ms, worst {:.2} ms (200 reads)",
            times[100], times[199]
        );
    };
    time_reads("alone");
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        let (root, home) = (
            run.root.clone(),
            run.saves.ancestors().nth(3).unwrap().to_path_buf(),
        );
        let stop = &stop;
        let busy = scope.spawn(move || {
            // A second handle on the same journal, as another thread has.
            let tracker =
                LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
            let table = One(Cell::new(true));
            let mut rounds = 0u32;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let session = tracker
                    .begin_gated(SessionGame::default(), "RetroArch")
                    .unwrap();
                let _ = tracker
                    .journal()
                    .observe_running(&session.session_id, &table);
                tracker.settle();
                let _ = tracker.finished(&session.session_id, None, Some("not started"));
                // And the worst a reader of history can do: all of it.
                let _ = tracker.journal().all();
                rounds += 1;
            }
            rounds
        });
        time_reads("beside constant bookkeeping and whole-history reads");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        println!(
            "HISTORY n={count} bookkeeping rounds meanwhile: {}",
            busy.join().unwrap()
        );
    });
    // Nothing of the history was removed.
    let left = fs::read_dir(&sessions).unwrap().count();
    assert!(left >= count, "{left} records left of {count}");
    println!(
        "HISTORY n={count} records still present: {left}; peak memory {} MB",
        high_water_kb() / 1024
    );
}

#[test]
fn bookkeeping_reads_only_the_sessions_that_are_not_over() {
    let run = gated();
    // A finished session, then its record made unreadable as JSON: any
    // step that read the whole history would trip over it or skip it; the
    // steps of a launch never open it at all.
    let old = run.end(&run.start());
    run.table.0.set(true);
    let open_list = run.root.join("sessions-open");
    let listed = |dir: &Path| {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
            .count()
    };
    assert_eq!(listed(&open_list), 0, "a finished session is not listed");
    let session = run.start();
    assert_eq!(listed(&open_list), 1);
    // Found by its id without the history, and the old one still readable.
    assert_eq!(
        run.tracker.journal().get(&session).unwrap().session_id,
        session
    );
    assert_eq!(run.tracker.journal().get(&old.session_id).unwrap(), old);
    // Now the old record is made unreadable: every step of the open
    // session still works, because none of them opens it.
    let old_file = fs::read_dir(run.root.join("sessions"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.to_string_lossy().contains(&old.session_id))
        .unwrap();
    fs::write(&old_file, b"not a record").unwrap();
    run.tracker.settle();
    run.tracker.look_again(&session).ok();
    let ended = run.end(&session);
    assert_eq!(ended.state, SessionState::Ended);
    assert_eq!(listed(&open_list), 0);
    assert_eq!(
        listed(&run.root.join("sessions")),
        2,
        "no record was removed"
    );
    assert_eq!(
        fs::read(&old_file).unwrap(),
        b"not a record",
        "nor rewritten"
    );

    // A session left open by a build that keeps no list is still settled.
    let stray = run.start();
    for entry in fs::read_dir(&open_list).unwrap().flatten() {
        if entry.file_name().to_string_lossy().ends_with(".json") {
            fs::remove_file(entry.path()).unwrap();
        }
    }
    run.table.0.set(false);
    let settled = run.tracker.settle();
    assert!(settled.iter().any(|record| record.session_id == stray));
}

#[test]
fn a_flatpak_emulators_saves_kept_outside_its_own_folder_are_still_its_saves() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
    let config = home.join(".var/app/org.libretro.RetroArch/config/retroarch");
    let saves = home.join("my-saves");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(&saves).unwrap();
    fs::write(
        config.join("retroarch.cfg"),
        format!("savefile_directory = \"{}\"\n", saves.display()),
    )
    .unwrap();
    fs::write(saves.join("Played (USA).srm"), [1u8; 8192]).unwrap();
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
    let process = StartedProcess {
        executable: PathBuf::from("/usr/bin/flatpak"),
        arguments: vec!["run".into(), "org.libretro.RetroArch".into()],
        ..started()
    };
    let launch = super::super::tracker::classify_started(&process.executable, &process.arguments);
    assert_eq!(
        (launch.emulator.as_str(), launch.profile.as_str()),
        ("RetroArch", "flatpak")
    );
    let baseline = tracker.baseline_before_start(&launch).unwrap();
    assert_eq!(baseline.marks.len(), 1, "{baseline:?}");
    // A native RetroArch launch on the same profile does not take them.
    assert!(
        tracker
            .baseline_before_start(&retroarch())
            .unwrap()
            .marks
            .is_empty()
    );

    let table = One(Cell::new(true));
    let session = tracker
        .begin_gated(SessionGame::default(), "RetroArch")
        .unwrap();
    tracker
        .process_started_gated(&session.session_id, &process, Ok((baseline, 1, 0)), &table)
        .unwrap();
    fs::write(saves.join("Played (USA).srm"), [9u8; 8192]).unwrap();
    table.0.set(false);
    let ended = tracker
        .finished_in(
            &session.session_id,
            Some(ExitSummary::Code(0)),
            None,
            &table,
        )
        .unwrap();
    assert_eq!(
        ended.save_changes,
        vec![SaveChange::Modified {
            label: "Played (USA).srm".into()
        }]
    );
}

#[test]
fn a_memory_card_among_retroarchs_saves_is_compared_whole_at_a_launch() {
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
    let config = home.join(".config/retroarch");
    let saves = home.join("my-saves");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(&saves).unwrap();
    fs::write(
        config.join("retroarch.cfg"),
        format!("savefile_directory = \"{}\"\n", saves.display()),
    )
    .unwrap();
    // An empty card: it holds no save that could be listed.
    let card = saves.join("Some PlayStation Game.srm");
    let mut bytes = crate::memory_card_manager::tests::ps1_card(&[]);
    fs::write(&card, &bytes).unwrap();
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
    let baseline = tracker.baseline_before_start(&retroarch()).unwrap();
    assert_eq!(baseline.marks.len(), 1, "{baseline:?}");
    assert!(baseline.unread.is_empty());

    let table = One(Cell::new(true));
    let session = tracker
        .begin_gated(SessionGame::default(), "RetroArch")
        .unwrap();
    tracker
        .process_started_gated(
            &session.session_id,
            &started(),
            Ok((baseline, 1, 0)),
            &table,
        )
        .unwrap();
    // The game writes to the card.
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    fs::write(&card, &bytes).unwrap();
    table.0.set(false);
    let ended = tracker
        .finished_in(
            &session.session_id,
            Some(ExitSummary::Code(0)),
            None,
            &table,
        )
        .unwrap();
    assert_eq!(
        ended.save_changes,
        vec![SaveChange::Modified {
            label: "Some PlayStation Game.srm".into()
        }]
    );
}

#[test]
fn a_memory_card_folder_is_taken_whole_at_a_launch() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
    let cards = home.join(".var/app/net.pcsx2.PCSX2/config/PCSX2/memcards");
    fs::create_dir_all(&cards).unwrap();
    // Not even a card as far as its bytes go: it is still compared.
    fs::write(cards.join("Mcd001.ps2"), [3u8; 4096]).unwrap();
    fs::write(cards.join("Mcd002.ps2"), [4u8; 4096]).unwrap();
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
    let process = StartedProcess {
        executable: PathBuf::from("/usr/bin/flatpak"),
        arguments: vec!["run".into(), "net.pcsx2.PCSX2".into()],
        ..started()
    };
    let launch = super::super::tracker::classify_started(&process.executable, &process.arguments);
    let run_once = |before: &dyn Fn(), during: &dyn Fn()| {
        before();
        let read = tracker.baseline_before_start(&launch);
        let table = One(Cell::new(true));
        let session = tracker
            .begin_gated(SessionGame::default(), "PCSX2")
            .unwrap();
        tracker
            .process_started_gated(
                &session.session_id,
                &process,
                read.map(|baseline| (baseline, 1, 0))
                    .map_err(|error| error.to_string()),
                &table,
            )
            .unwrap();
        during();
        table.0.set(false);
        tracker
            .finished_in(
                &session.session_id,
                Some(ExitSummary::Code(0)),
                None,
                &table,
            )
            .unwrap()
    };
    // One card written during the session, the other left alone.
    let ended = run_once(&|| {}, &|| {
        fs::write(cards.join("Mcd001.ps2"), [9u8; 4096]).unwrap()
    });
    assert_eq!(
        ended.save_changes,
        vec![SaveChange::Modified {
            label: "Mcd001.ps2 (the whole card file)".into()
        }]
    );
    // SAFETY: geteuid only reads the process's own id.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let lock =
        |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    // A card that cannot be opened before the start and can afterwards is
    // not "new"; one that cannot be opened at either end is "not known".
    let ended = run_once(&|| lock(&cards.join("Mcd002.ps2"), 0o000), &|| {
        lock(&cards.join("Mcd002.ps2"), 0o644)
    });
    assert_eq!(
        ended.save_changes,
        vec![SaveChange::CreatedBeforeNotKnown {
            label: "Mcd002.ps2 (the whole card file)".into()
        }]
    );
    let ended = run_once(&|| lock(&cards.join("Mcd002.ps2"), 0o000), &|| {});
    lock(&cards.join("Mcd002.ps2"), 0o644);
    assert_eq!(
        ended.save_changes,
        vec![SaveChange::NotKnown {
            label: "Mcd002.ps2".into()
        }]
    );
    // A card folder that cannot be listed: no baseline, so nothing claimed.
    let ended = run_once(&|| lock(&cards, 0o000), &|| lock(&cards, 0o755));
    assert!(matches!(
        ended.save_tracking,
        Some(SaveTracking::Unavailable { .. })
    ));
    assert!(ended.save_changes.is_empty() && ended.saves_after.is_none());
}

#[test]
fn a_card_that_cannot_be_opened_never_makes_its_saves_look_new() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid only reads the process's own id.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let lock =
        |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
    // An RPCS3 PlayStation card, and a RetroArch card under a name saves do
    // not go by.
    let vmc = home.join(".config/rpcs3/dev_hdd0/savedata/vmc");
    let retro = home.join(".config/retroarch");
    fs::create_dir_all(&vmc).unwrap();
    fs::create_dir_all(retro.join("saves")).unwrap();
    fs::write(
        retro.join("retroarch.cfg"),
        "savefile_directory = \"default\"\n",
    )
    .unwrap();
    let cards = [
        (vmc.join("shared.VM1"), "rpcs3", "RPCS3"),
        (retro.join("saves/Game.mcr"), "retroarch", "RetroArch"),
    ];
    for (card, _, _) in &cards {
        fs::write(card, crate::memory_card_manager::tests::ps1_card(&[])).unwrap();
    }
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
    for (card, program, emulator) in &cards {
        let process = StartedProcess {
            executable: PathBuf::from(format!("/nowhere/{program}")),
            ..started()
        };
        let launch = super::super::tracker::classify_started(&process.executable, &[]);
        assert_eq!(&launch.emulator, emulator);
        // Readable: taken whole.
        let open = tracker.baseline_before_start(&launch).unwrap();
        assert!(
            open.marks
                .iter()
                .any(|mark| mark.path.as_ref() == Some(card)),
            "{emulator}: {open:?}"
        );
        // Unopenable before the start, readable after it.
        lock(card, 0o000);
        let baseline = tracker.baseline_before_start(&launch).unwrap();
        assert_eq!(baseline.unread, vec![card.clone()], "{emulator}");
        assert!(baseline.marks.is_empty(), "{emulator}: {baseline:?}");
        let table = One(Cell::new(true));
        let session = tracker
            .begin_gated(SessionGame::default(), emulator)
            .unwrap();
        tracker
            .process_started_gated(&session.session_id, &process, Ok((baseline, 1, 0)), &table)
            .unwrap();
        lock(card, 0o644);
        table.0.set(false);
        let ended = tracker
            .finished_in(
                &session.session_id,
                Some(ExitSummary::Code(0)),
                None,
                &table,
            )
            .unwrap();
        assert!(
            !ended.save_changes.is_empty()
                && ended
                    .save_changes
                    .iter()
                    .all(|change| matches!(change, SaveChange::CreatedBeforeNotKnown { .. })),
            "{emulator}: {:?}",
            ended.save_changes
        );
    }
}

#[test]
fn saves_on_a_card_are_never_new_or_gone_because_the_card_could_not_be_opened() {
    use crate::memory_card_manager::tests::{ps1_card, ps1_save};
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid only reads the process's own id.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let lock =
        |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
    // A PlayStation Classic's own card folder inside RPCS3, and a card with
    // no suffix among RetroArch's saves; both hold a save.
    let classic = home.join(".config/rpcs3/dev_hdd0/game/NPUJ00001/USRDIR/SAVEDATA");
    let retro = home.join(".config/retroarch");
    fs::create_dir_all(&classic).unwrap();
    fs::create_dir_all(retro.join("saves")).unwrap();
    fs::write(
        retro.join("retroarch.cfg"),
        "savefile_directory = \"default\"\n",
    )
    .unwrap();
    let cards = [
        (classic.join("SCEVMC0.VMP.VM1"), "rpcs3", "RPCS3"),
        (retro.join("saves/CARDFILE"), "retroarch", "RetroArch"),
    ];
    let bytes = ps1_card(&[ps1_save("BASLUS-00001GAME", 1, "GAME")]);
    for (card, _, _) in &cards {
        fs::write(card, &bytes).unwrap();
    }
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
    for (card, program, emulator) in &cards {
        let process = StartedProcess {
            executable: PathBuf::from(format!("/nowhere/{program}")),
            ..started()
        };
        let launch = super::super::tracker::classify_started(&process.executable, &[]);
        let session_with = |before: &dyn Fn(), during: &dyn Fn()| {
            before();
            let baseline = tracker.baseline_before_start(&launch).unwrap();
            let table = One(Cell::new(true));
            let session = tracker
                .begin_gated(SessionGame::default(), emulator)
                .unwrap();
            tracker
                .process_started_gated(&session.session_id, &process, Ok((baseline, 1, 0)), &table)
                .unwrap();
            during();
            table.0.set(false);
            tracker
                .finished_in(
                    &session.session_id,
                    Some(ExitSummary::Code(0)),
                    None,
                    &table,
                )
                .unwrap()
        };
        // Readable at both ends and untouched: nothing to say.
        let quiet = session_with(&|| {}, &|| {});
        assert!(
            quiet
                .saves_before
                .iter()
                .any(|mark| mark.path.as_ref() == Some(card)),
            "{emulator}: the card is tracked: {:?}",
            quiet.saves_before
        );
        assert!(
            quiet.save_changes.is_empty(),
            "{emulator}: {:?}",
            quiet.save_changes
        );
        // The save inside the card is listed too, not only the card file.
        assert!(
            quiet.saves_before.iter().any(|mark| {
                mark.path.as_ref() == Some(card)
                    && !mark.save_id.starts_with("whole:")
                    && mark.label.contains("BASLUS-00001GAME")
            }),
            "{emulator}: the card's save is listed: {:?}",
            quiet.saves_before
        );
        // Unopenable before, readable after: nothing is "new".
        let ended = session_with(&|| lock(card, 0o000), &|| lock(card, 0o644));
        assert!(
            !ended.save_changes.is_empty()
                && ended
                    .save_changes
                    .iter()
                    .all(|change| matches!(change, SaveChange::CreatedBeforeNotKnown { .. })),
            "{emulator}: {:?}",
            ended.save_changes
        );
        // Readable before, unopenable after: nothing is "gone".
        let ended = session_with(&|| {}, &|| lock(card, 0o000));
        lock(card, 0o644);
        assert!(
            !ended.save_changes.is_empty()
                && ended
                    .save_changes
                    .iter()
                    .all(|change| matches!(change, SaveChange::NotKnown { .. })),
            "{emulator}: {:?}",
            ended.save_changes
        );
        assert!(!ended.needs_attention());
    }
    // The card folder is not there at the second read (its drive went):
    // nothing is compared.
    let (card, _, _) = &cards[0];
    let process = StartedProcess {
        executable: PathBuf::from("/nowhere/rpcs3"),
        ..started()
    };
    let launch = super::super::tracker::classify_started(&process.executable, &[]);
    let baseline = tracker.baseline_before_start(&launch).unwrap();
    let table = One(Cell::new(true));
    let session = tracker
        .begin_gated(SessionGame::default(), "RPCS3")
        .unwrap();
    tracker
        .process_started_gated(&session.session_id, &process, Ok((baseline, 1, 0)), &table)
        .unwrap();
    fs::rename(
        card.parent().unwrap(),
        card.parent().unwrap().with_extension("away"),
    )
    .unwrap();
    table.0.set(false);
    let ended = tracker
        .finished_in(
            &session.session_id,
            Some(ExitSummary::Code(0)),
            None,
            &table,
        )
        .unwrap();
    assert!(ended.saves_after.is_none() && ended.save_changes.is_empty());
}

/// PCSX2's and DuckStation's own cards, in every state a card can be found
/// in: a card that cannot be opened, or whose bytes are not a card's, is
/// never an empty card and never a card read; what is said about it is
/// "not known", at the read before the start, at the read after, and in the
/// session after that, when it can be read again.
#[test]
fn a_card_in_any_state_is_never_taken_for_an_empty_or_a_read_one() {
    use crate::memory_card_manager::tests::{ps1_card, ps1_save};
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid only reads the process's own id.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let lock =
        |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (root, home) = (dir.path().join("continuity"), dir.path().join("home"));
    let made = dir.path().join("made");
    fs::create_dir_all(&made).unwrap();
    let ps2 = |payload: Option<&[u8]>| -> Vec<u8> {
        let saves: Vec<(&str, &[u8])> = payload
            .iter()
            .map(|bytes| ("BASLUS-20001", *bytes))
            .collect();
        let path = super::discover::ps2_card(&made, "card", &saves);
        fs::read(path).unwrap()
    };
    let ps1 = |title: Option<&str>| -> Vec<u8> {
        match title {
            Some(title) => ps1_card(&[ps1_save("BASLUS-00001GAME", 1, title)]),
            None => ps1_card(&[]),
        }
    };
    struct Case {
        emulator: &'static str,
        process: StartedProcess,
        folder: PathBuf,
        names: [&'static str; 2],
        empty: Vec<u8>,
        one: Vec<u8>,
        other: Vec<u8>,
    }
    let cases = [
        Case {
            emulator: "PCSX2",
            process: StartedProcess {
                executable: PathBuf::from("/usr/bin/flatpak"),
                arguments: vec!["run".into(), "net.pcsx2.PCSX2".into()],
                ..started()
            },
            folder: home.join(".var/app/net.pcsx2.PCSX2/config/PCSX2/memcards"),
            names: ["Mcd001.ps2", "Mcd002.ps2"],
            empty: ps2(None),
            one: ps2(Some(&[1u8; 2048])),
            other: ps2(Some(&[2u8; 2048])),
        },
        Case {
            emulator: "DuckStation",
            process: StartedProcess {
                executable: PathBuf::from("/nowhere/duckstation"),
                ..started()
            },
            folder: home.join(".local/share/duckstation/memcards"),
            names: ["shared_card_1.mcd", "Game (USA).mcd"],
            empty: ps1(None),
            one: ps1(Some("GAME")),
            other: ps1(Some("GAM2")),
        },
    ];
    let tracker = LaunchTracker::open(&root, DiscoveryRoots::under_home(&home), "dev").unwrap();
    for case in &cases {
        let who = case.emulator;
        fs::create_dir_all(&case.folder).unwrap();
        let (first, second) = (
            case.folder.join(case.names[0]),
            case.folder.join(case.names[1]),
        );
        let whole = |name: &str| format!("{name} (the whole card file)");
        let launch = super::super::tracker::classify_started(
            &case.process.executable,
            &case.process.arguments,
        );
        assert_eq!(launch.emulator, who);
        let session_with = |before: &dyn Fn(), during: &dyn Fn()| {
            before();
            let read = tracker.baseline_before_start(&launch);
            let table = One(Cell::new(true));
            let session = tracker.begin_gated(SessionGame::default(), who).unwrap();
            tracker
                .process_started_gated(
                    &session.session_id,
                    &case.process,
                    read.map(|baseline| (baseline, 1, 0))
                        .map_err(|error| error.to_string()),
                    &table,
                )
                .unwrap();
            during();
            table.0.set(false);
            tracker
                .finished_in(
                    &session.session_id,
                    Some(ExitSummary::Code(0)),
                    None,
                    &table,
                )
                .unwrap()
        };
        let all = |ended: &SessionRecord, wanted: &dyn Fn(&SaveChange) -> bool| {
            assert!(
                !ended.save_changes.is_empty() && ended.save_changes.iter().all(wanted),
                "{who}: {:?}",
                ended.save_changes
            );
        };
        let fresh = || {
            lock(&case.folder, 0o755);
            for card in [&first, &second] {
                if card.exists() {
                    lock(card, 0o644);
                }
            }
            fs::write(&first, &case.empty).unwrap();
            fs::write(&second, &case.one).unwrap();
        };

        // A valid, empty card and a card with a save, both left alone: both
        // were read, nothing is said.
        fresh();
        let ended = session_with(&|| {}, &|| {});
        assert!(matches!(
            ended.save_tracking,
            Some(SaveTracking::Established { .. })
        ));
        for card in [&first, &second] {
            assert!(
                ended.saves_before.iter().any(|mark| {
                    mark.save_id.starts_with("whole:") && mark.path.as_ref() == Some(card)
                }),
                "{who}: {:?}",
                ended.saves_before
            );
        }
        assert!(ended.saves_unread_before.is_empty() && ended.saves_unread_after.is_empty());
        assert!(
            ended.save_changes.is_empty(),
            "{who}: {:?}",
            ended.save_changes
        );

        // The game saves onto the valid, empty card: the card changed, and
        // what is on it now is new. Nothing is said of the other card.
        let ended = session_with(&|| {}, &|| fs::write(&first, &case.one).unwrap());
        assert!(
            ended.save_changes.contains(&SaveChange::Modified {
                label: whole(case.names[0])
            }),
            "{who}: {:?}",
            ended.save_changes
        );
        all(&ended, &|change| {
            matches!(
                change,
                SaveChange::Modified { .. } | SaveChange::Created { .. }
            )
        });
        assert!(
            !ended
                .save_changes
                .iter()
                .any(|change| format!("{change:?}").contains(case.names[1])),
            "{who}: {:?}",
            ended.save_changes
        );

        // A card that is not there at the start and is afterwards is new;
        // one that was there and is deleted is gone. Both were seen.
        fresh();
        fs::remove_file(&first).unwrap();
        let ended = session_with(&|| {}, &|| fs::write(&first, &case.one).unwrap());
        all(&ended, &|change| {
            matches!(change, SaveChange::Created { .. })
        });
        let ended = session_with(&|| {}, &|| fs::remove_file(&first).unwrap());
        all(&ended, &|change| {
            matches!(change, SaveChange::Missing { .. })
        });

        // Bytes that are not a card's: still compared, as a file.
        fresh();
        fs::write(&first, vec![5u8; case.empty.len()]).unwrap();
        let ended = session_with(&|| {}, &|| {});
        assert!(
            ended.save_changes.is_empty(),
            "{who}: {:?}",
            ended.save_changes
        );
        let ended = session_with(&|| {}, &|| {
            fs::write(&first, vec![6u8; case.empty.len()]).unwrap()
        });
        assert_eq!(
            ended.save_changes,
            vec![SaveChange::Modified {
                label: whole(case.names[0])
            }],
            "{who}"
        );

        // Cannot be opened before the start, can afterwards: not an empty
        // card that filled, so nothing is "new".
        fresh();
        let ended = session_with(&|| lock(&second, 0o000), &|| lock(&second, 0o644));
        assert_eq!(ended.saves_unread_before, vec![second.clone()], "{who}");
        all(&ended, &|change| {
            matches!(change, SaveChange::CreatedBeforeNotKnown { .. })
        });

        // Can be opened before, cannot afterwards: not a card that emptied,
        // so nothing is "gone".
        let ended = session_with(&|| {}, &|| lock(&second, 0o000));
        assert_eq!(ended.saves_unread_after, vec![second.clone()], "{who}");
        all(&ended, &|change| {
            matches!(change, SaveChange::NotKnown { .. })
        });
        assert!(!ended.needs_attention());

        // One card fails at both ends while the other is written: the one is
        // "not known", the other "changed"; neither is new or gone.
        let ended = session_with(&|| fs::write(&first, &case.one).unwrap(), &|| {
            fs::write(&first, &case.other).unwrap()
        });
        assert!(
            ended.save_changes.contains(&SaveChange::NotKnown {
                label: case.names[1].into()
            }) && ended.save_changes.contains(&SaveChange::Modified {
                label: whole(case.names[0])
            }),
            "{who}: {:?}",
            ended.save_changes
        );
        all(&ended, &|change| {
            matches!(
                change,
                SaveChange::NotKnown { .. } | SaveChange::Modified { .. }
            )
        });

        // The card can be read again: the next session is an ordinary one.
        lock(&second, 0o644);
        let ended = session_with(&|| {}, &|| {});
        assert!(ended.saves_unread_before.is_empty() && ended.saves_unread_after.is_empty());
        assert!(
            ended.save_changes.is_empty(),
            "{who}: {:?}",
            ended.save_changes
        );
        let ended = session_with(&|| {}, &|| fs::write(&second, &case.other).unwrap());
        assert!(
            ended.save_changes.contains(&SaveChange::Modified {
                label: whole(case.names[1])
            }),
            "{who}: {:?}",
            ended.save_changes
        );

        // The card folder itself cannot be listed: no read at all, so the
        // session is not tracked and nothing is claimed.
        let ended = session_with(&|| lock(&case.folder, 0o000), &|| lock(&case.folder, 0o755));
        assert!(matches!(
            ended.save_tracking,
            Some(SaveTracking::Unavailable { .. })
        ));
        assert!(ended.save_changes.is_empty() && ended.saves_after.is_none());
        // ... and at the end of a session only: nothing is compared.
        let ended = session_with(&|| {}, &|| lock(&case.folder, 0o000));
        lock(&case.folder, 0o755);
        assert!(ended.save_changes.is_empty() && ended.saves_after.is_none());
    }
}

// ----- A disk that comes or goes during a session -----

/// A save folder that is where a disk is mounted is still there, empty,
/// once the disk is taken away. The folder is then on another disk than at
/// the read before the start, and its saves are not "gone".
#[test]
fn saves_whose_disk_went_during_the_session_are_not_called_gone() {
    let empty = |run: &Gated| {
        for entry in fs::read_dir(&run.saves).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                fs::remove_file(path).unwrap();
            }
        }
    };
    // The folder was on another disk when the saves were read.
    let run = gated();
    let session = run
        .tracker
        .begin_gated(SessionGame::default(), "RetroArch")
        .unwrap()
        .session_id;
    let mut baseline = run.tracker.baseline_before_start(&retroarch()).unwrap();
    assert!(
        baseline
            .disks
            .iter()
            .any(|(folder, _)| folder == &run.saves)
    );
    for (_, disk) in &mut baseline.disks {
        *disk = disk.wrapping_add(1);
    }
    run.tracker
        .process_started_gated(&session, &started(), Ok((baseline, 3, 0)), &run.table)
        .unwrap();
    empty(&run);
    let ended = run.end(&session);
    assert_eq!(ended.state, SessionState::Ended);
    assert!(ended.saves_after.is_none() && ended.save_changes.is_empty());

    // The same emptying on the same disk: the saves really are gone.
    let run = gated();
    let session = run.start();
    empty(&run);
    let ended = run.end(&session);
    assert!(ended.saves_after.is_some());
    assert!(
        ended
            .save_changes
            .iter()
            .any(|change| matches!(change, SaveChange::Missing { .. })),
        "{:?}",
        ended.save_changes
    );
}

#[test]
fn a_folder_is_judged_by_the_disk_it_is_on() {
    use super::super::tracker::same_disks;
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let here = fs::metadata(dir.path()).unwrap().dev();
    let (saves, game) = (dir.path().join("saves"), dir.path().join("saves/game"));
    fs::create_dir_all(&game).unwrap();
    let before = vec![(saves.clone(), here), (game.clone(), here)];
    assert!(same_disks(&before, &before));
    // A folder deleted from a disk that is still there.
    fs::remove_dir(&game).unwrap();
    assert!(same_disks(&before, &[(saves.clone(), here)]));
    // A folder that was on another disk, there or not.
    assert!(!same_disks(&[(saves.clone(), here + 1)], &[]));
    assert!(!same_disks(&[(game.clone(), here + 1)], &[]));
    // A new folder on another disk than the folder it is in.
    assert!(!same_disks(
        &[(saves.clone(), here)],
        &[(saves.clone(), here), (saves.join("new"), here + 1)]
    ));
    assert!(same_disks(
        &[(saves.clone(), here)],
        &[(saves.clone(), here), (saves.join("new"), here)]
    ));
}

/// By hand only, with a real mount: run inside `unshare -Urm` so that a
/// memory disk can be mounted on, and taken off, the save folder.
#[test]
#[ignore = "by hand: needs LAZARUS_MOUNT_TEST=1 inside unshare -Urm"]
fn a_real_disk_taken_away_or_put_in_during_a_session_by_hand() {
    assert_eq!(std::env::var("LAZARUS_MOUNT_TEST").as_deref(), Ok("1"));
    let command = |program: &str, arguments: &[&std::ffi::OsStr]| {
        let status = std::process::Command::new(program)
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "{program} {arguments:?}");
    };
    let take = |folder: &Path| -> Vec<(PathBuf, Vec<u8>)> {
        fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .map(|path| {
                let bytes = fs::read(&path).unwrap();
                fs::remove_file(&path).unwrap();
                (path, bytes)
            })
            .collect()
    };

    // The saves are on a disk mounted on the save folder; it is taken away
    // while the game runs.
    let run = gated();
    let files = take(&run.saves);
    assert!(!files.is_empty());
    command(
        "mount",
        &[
            "-t".as_ref(),
            "tmpfs".as_ref(),
            "tmpfs".as_ref(),
            run.saves.as_os_str(),
        ],
    );
    for (path, bytes) in &files {
        fs::write(path, bytes).unwrap();
    }
    let session = run.start();
    let before = run.tracker.journal().get(&session).unwrap();
    command("umount", &[run.saves.as_os_str()]);
    assert_eq!(fs::read_dir(&run.saves).unwrap().count(), 0);
    let ended = run.end(&session);
    println!(
        "MOUNT disk taken away: read before {} saves; state {:?}; compared {}; changes {:?}",
        before.saves_before.len(),
        ended.state,
        ended.saves_after.is_some(),
        ended.save_changes
    );
    assert!(!before.saves_before.is_empty());
    assert!(ended.saves_after.is_none() && ended.save_changes.is_empty());

    // The other way round: a disk is put on the save folder while the game
    // runs and hides the saves that were there.
    let run = gated();
    let session = run.start();
    command(
        "mount",
        &[
            "-t".as_ref(),
            "tmpfs".as_ref(),
            "tmpfs".as_ref(),
            run.saves.as_os_str(),
        ],
    );
    fs::write(run.saves.join("From the other disk.srm"), [7u8; 8192]).unwrap();
    let ended = run.end(&session);
    command("umount", &[run.saves.as_os_str()]);
    println!(
        "MOUNT disk put in: compared {}; changes {:?}",
        ended.saves_after.is_some(),
        ended.save_changes
    );
    assert!(ended.saves_after.is_none() && ended.save_changes.is_empty());
}
