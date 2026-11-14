use super::*;

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::ThreadId;

/// A profile for these tests: its answer is the content of `folder/bios.bin`.
#[derive(Debug, Clone)]
struct Profile {
    folder: PathBuf,
}

fn folder(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "emuwiz-firmware-answers-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

type Answer = (String, ThreadId);

/// Asks the cache the way the drawn readiness card does: the answer is read
/// from the profile's folder, on whichever thread the cache chooses.
fn ask(
    cache: &FirmwareAnswerCache<Answer>,
    profiles: &Vec<Profile>,
    evidence: &Vec<String>,
    now: Instant,
) -> Vec<Answer> {
    ask_as(FirmwareUse::Shown, cache, profiles, evidence, now)
}

/// Asks the way Play, the Launch page and every other caller do.
fn ask_fresh(
    cache: &FirmwareAnswerCache<Answer>,
    profiles: &Vec<Profile>,
    evidence: &Vec<String>,
    now: Instant,
) -> Vec<Answer> {
    ask_as(FirmwareUse::Fresh, cache, profiles, evidence, now)
}

fn ask_as(
    usage: FirmwareUse,
    cache: &FirmwareAnswerCache<Answer>,
    profiles: &Vec<Profile>,
    evidence: &Vec<String>,
    now: Instant,
) -> Vec<Answer> {
    cache.answers(
        usage,
        &(profiles, evidence),
        now,
        |index| vec![profiles[index].folder.clone()],
        || {
            let profiles = profiles.clone();
            let evidence = evidence.clone();
            move |index: usize| {
                let read = std::fs::read_to_string(profiles[index].folder.join("bios.bin"))
                    .unwrap_or_else(|_| "missing".into());
                (
                    format!("{read}/{}", evidence.join("+")),
                    std::thread::current().id(),
                )
            }
        },
    )
}

/// The watched paths as they are, judged an hour from now: every fixture
/// file then counts as changed long ago.
fn settled(paths: &[PathBuf]) -> Option<Vec<Watched>> {
    stamp(paths, SystemTime::now() + Duration::from_secs(3600))
}

fn texts(answers: &[Answer]) -> Vec<&str> {
    answers.iter().map(|answer| answer.0.as_str()).collect()
}

#[test]
fn answers_are_reused_for_as_long_as_the_files_are_unchanged() {
    let dir = folder("unchanged");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let evidence = Vec::new();
    let cache = FirmwareAnswerCache::with_settled_files();
    let start = Instant::now();

    let first = ask(&cache, &profiles, &evidence, start);
    assert_eq!(texts(&first), ["one/"]);
    assert_eq!(
        first[0].1,
        std::thread::current().id(),
        "with nothing to return, the first answer is computed where it is asked for"
    );
    assert_eq!(cache.computed(), 1);

    // Long past the old two-second window: the file is not read again.
    for seconds in [1, 2, 3, 10, 59] {
        let later = start + Duration::from_secs(seconds);
        assert_eq!(texts(&ask(&cache, &profiles, &evidence, later)), ["one/"]);
    }
    assert_eq!(cache.computed(), 1);
    assert_eq!(cache.refreshes(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_changed_file_is_verified_again_on_another_thread() {
    let dir = folder("changed");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let evidence = Vec::new();
    let cache = FirmwareAnswerCache::with_settled_files();
    let now = Instant::now();
    ask(&cache, &profiles, &evidence, now);

    std::fs::write(dir.join("bios.bin"), "two, and longer").unwrap();
    // The request that notices the change still returns what is held...
    let during = ask(&cache, &profiles, &evidence, now);
    assert_eq!(texts(&during), ["one/"]);
    assert_eq!(cache.refreshes(), 1);
    // ...and the new answer arrives from a thread of its own.
    cache.settle();
    let after = ask(&cache, &profiles, &evidence, now);
    assert_eq!(texts(&after), ["two, and longer/"]);
    assert_ne!(
        after[0].1,
        std::thread::current().id(),
        "the file was read again, but not on the asking thread"
    );
    assert_eq!(cache.computed(), 2);
    // Settled again: nothing more is read.
    ask(&cache, &profiles, &evidence, now + Duration::from_secs(30));
    assert_eq!((cache.computed(), cache.refreshes()), (2, 1));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn each_kind_of_change_to_a_watched_folder_is_noticed() {
    let dir = folder("kinds");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let watch = vec![dir.clone(), dir.join("absent.ini")];
    let mut before = settled(&watch).expect("a small folder can be watched");
    assert_eq!(settled(&watch).as_ref(), Some(&before), "nothing changed");

    let mut changed = |what: &str| {
        let after = settled(&watch).expect("still watchable");
        assert_ne!(after, before, "{what}");
        before = after;
    };
    std::fs::write(dir.join("other.bin"), "x").unwrap();
    changed("a new entry");
    std::fs::write(dir.join("bios.bin"), "one!").unwrap();
    changed("a different size");
    let replacement = dir.join("replacement");
    std::fs::write(&replacement, "two!").unwrap();
    std::fs::rename(&replacement, dir.join("bios.bin")).unwrap();
    changed("the same size from another file");
    std::fs::remove_file(dir.join("other.bin")).unwrap();
    changed("an entry removed");
    std::fs::write(dir.join("absent.ini"), "[Filenames]").unwrap();
    changed("a watched file that did not exist before");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(dir.join("bios.bin"))
        .unwrap()
        .permissions()
        .mode();
    std::fs::set_permissions(
        dir.join("bios.bin"),
        std::fs::Permissions::from_mode(mode ^ 0o040),
    )
    .unwrap();
    changed("permissions");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_watched_link_is_followed_to_what_it_leads_to() {
    let dir = folder("link");
    let (first, second) = (dir.join("first"), dir.join("second"));
    for target in [&first, &second] {
        std::fs::create_dir_all(target).unwrap();
        std::fs::write(target.join("bios.bin"), "x").unwrap();
    }
    let link = dir.join("bios");
    std::os::unix::fs::symlink(&first, &link).unwrap();
    let watch = vec![link.clone()];
    let before = settled(&watch).unwrap();
    assert!(
        before[0].entries.is_some(),
        "the folder behind the link is listed"
    );

    std::fs::write(first.join("more.bin"), "y").unwrap();
    let grown = settled(&watch).unwrap();
    assert_ne!(grown, before, "a new entry behind the link");

    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&second, &link).unwrap();
    assert_ne!(
        settled(&watch).unwrap(),
        grown,
        "the link now leads elsewhere"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn new_evidence_is_answered_from_what_is_held_until_it_is_verified() {
    let dir = folder("evidence");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let cache = FirmwareAnswerCache::with_settled_files();
    let now = Instant::now();
    ask(&cache, &profiles, &Vec::new(), now);

    let evidence = vec!["record".to_string()];
    let during = ask(&cache, &profiles, &evidence, now);
    assert_eq!(
        texts(&during),
        ["one/"],
        "the same profiles: the held answer"
    );
    cache.settle();
    let after = ask(&cache, &profiles, &evidence, now);
    assert_eq!(texts(&after), ["one/record"]);
    assert_ne!(after[0].1, std::thread::current().id());
    assert_eq!((cache.computed(), cache.refreshes()), (2, 1));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_changed_profile_list_is_answered_at_once() {
    let (one, two) = (folder("list-one"), folder("list-two"));
    std::fs::write(one.join("bios.bin"), "one").unwrap();
    std::fs::write(two.join("bios.bin"), "two").unwrap();
    let mut profiles = vec![Profile {
        folder: one.clone(),
    }];
    let evidence = Vec::new();
    let cache = FirmwareAnswerCache::with_settled_files();
    let now = Instant::now();
    ask(&cache, &profiles, &evidence, now);

    profiles.push(Profile {
        folder: two.clone(),
    });
    let answers = ask(&cache, &profiles, &evidence, now);
    assert_eq!(
        texts(&answers),
        ["one/", "two/"],
        "one answer per profile, never a shorter list from before"
    );
    assert_eq!(answers[1].1, std::thread::current().id());
    assert_eq!((cache.computed(), cache.refreshes()), (3, 0));

    // The same number of profiles, but a different one: also at once.
    profiles[1].folder = one.clone();
    assert_eq!(
        texts(&ask(&cache, &profiles, &evidence, now)),
        ["one/", "one/"]
    );
    assert_eq!((cache.computed(), cache.refreshes()), (5, 0));
    for dir in [one, two] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn unchanged_files_are_still_verified_again_after_the_backstop() {
    let dir = folder("backstop");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let evidence = Vec::new();
    let cache = FirmwareAnswerCache::with_settled_files();
    let start = Instant::now();
    ask(&cache, &profiles, &evidence, start);

    let just_before = start + FIRMWARE_ANSWER_BACKSTOP - Duration::from_millis(1);
    ask(&cache, &profiles, &evidence, just_before);
    assert_eq!(cache.refreshes(), 0);
    let held = ask(
        &cache,
        &profiles,
        &evidence,
        start + FIRMWARE_ANSWER_BACKSTOP,
    );
    assert_eq!(texts(&held), ["one/"]);
    assert_eq!(cache.refreshes(), 1, "on its own thread, not here");
    cache.settle();
    assert_eq!(cache.computed(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_folder_too_large_to_watch_falls_back_to_the_timer_off_the_asking_thread() {
    let dir = folder("large");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    for index in 0..MAX_WATCHED_ENTRIES {
        std::fs::write(dir.join(format!("filler-{index}")), "").unwrap();
    }
    assert_eq!(settled(std::slice::from_ref(&dir)), None);
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let evidence = Vec::new();
    let cache = FirmwareAnswerCache::with_settled_files();
    let start = Instant::now();
    ask(&cache, &profiles, &evidence, start);

    let inside = start + FIRMWARE_ANSWER_RECHECK - Duration::from_millis(1);
    ask(&cache, &profiles, &evidence, inside);
    assert_eq!((cache.computed(), cache.refreshes()), (1, 0));
    std::fs::write(dir.join("bios.bin"), "two!").unwrap();
    let held = ask(
        &cache,
        &profiles,
        &evidence,
        start + FIRMWARE_ANSWER_RECHECK,
    );
    assert_eq!(texts(&held), ["one/"]);
    cache.settle();
    let after = ask(
        &cache,
        &profiles,
        &evidence,
        start + FIRMWARE_ANSWER_RECHECK,
    );
    assert_eq!(texts(&after), ["two!/"]);
    assert_ne!(after[0].1, std::thread::current().id());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_interface_is_told_when_verified_answers_arrive() {
    let dir = folder("wake");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let evidence = Vec::new();
    let cache = FirmwareAnswerCache::with_settled_files();
    let context = egui::Context::default();
    cache.wake_with(&context);
    cache.wake_with(&egui::Context::default()); // only the first is kept
    let now = Instant::now();

    ask(&cache, &profiles, &evidence, now);
    assert_eq!(
        crate::emulator_setup::answers::count(&context),
        0,
        "an answer computed where it was asked for is already in the input"
    );
    std::fs::write(dir.join("bios.bin"), "two!").unwrap();
    ask(&cache, &profiles, &evidence, now);
    cache.settle();
    // The count is what makes the remembered readiness input build again.
    let deadline = Instant::now() + Duration::from_secs(20);
    while crate::emulator_setup::answers::count(&context) == 0 {
        assert!(Instant::now() < deadline, "the interface was never told");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(crate::emulator_setup::answers::count(&context), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn one_verification_runs_at_a_time_however_often_it_is_asked_for() {
    let dir = folder("once");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let cache = FirmwareAnswerCache::<String>::with_settled_files();
    let now = Instant::now();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Arc::new(Mutex::new(gate));
    let ask_slowly = |wait: bool| {
        let gate = Arc::clone(&gate);
        cache.answers(
            FirmwareUse::Shown,
            &profiles,
            now,
            |index| vec![profiles[index].folder.clone()],
            || {
                let profiles = profiles.clone();
                move |index: usize| {
                    if wait {
                        let _ = gate.lock().unwrap().recv();
                    }
                    std::fs::read_to_string(profiles[index].folder.join("bios.bin")).unwrap()
                }
            },
        )
    };
    assert_eq!(ask_slowly(false), ["one"]);
    std::fs::write(dir.join("bios.bin"), "two!").unwrap();
    // Ten frames ask while the verification is still reading.
    for _ in 0..10 {
        assert_eq!(ask_slowly(true), ["one"]);
    }
    assert_eq!(cache.refreshes(), 1);
    release.send(()).unwrap();
    cache.settle();
    assert_eq!(ask_slowly(false), ["two!"]);
    assert_eq!(cache.computed(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_file_changed_moments_ago_is_not_trusted_to_stay_the_same() {
    let dir = folder("unsettled");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let watch = vec![dir.clone()];
    let written = std::fs::metadata(dir.join("bios.bin"))
        .unwrap()
        .modified()
        .unwrap();
    // Judged at the moment of the write, or within the margin after it: a
    // second write in the same clock step would leave the same size and the
    // same times, so the state is not relied on.
    assert_eq!(stamp(&watch, written), None);
    assert_eq!(stamp(&watch, written + Duration::from_secs(1)), None);
    // Once the last change is older than the margin, it is.
    let settled_at = written + UNSETTLED_FILE_AGE + Duration::from_secs(1);
    assert!(stamp(&watch, settled_at).is_some());

    // The cache then falls back to verifying again on the timer, on its own
    // thread, for as long as the file has not settled.
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let evidence = Vec::new();
    let cache = FirmwareAnswerCache::with_unsettled_files();
    let start = Instant::now();
    ask(&cache, &profiles, &evidence, start);
    ask(&cache, &profiles, &evidence, start + Duration::from_secs(1));
    assert_eq!((cache.computed(), cache.refreshes()), (1, 0));
    let held = ask(
        &cache,
        &profiles,
        &evidence,
        start + FIRMWARE_ANSWER_RECHECK,
    );
    assert_eq!(texts(&held), ["one/"]);
    assert_eq!(cache.refreshes(), 1);
    cache.settle();
    assert_eq!(cache.computed(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn recheck_has_the_answers_verified_again_without_a_changed_file() {
    let dir = folder("recheck");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let evidence = Vec::new();
    let cache = FirmwareAnswerCache::with_settled_files();
    let now = Instant::now();
    ask(&cache, &profiles, &evidence, now);

    cache.verify_again();
    let held = ask(&cache, &profiles, &evidence, now);
    assert_eq!(texts(&held), ["one/"], "what is held, not a wait");
    assert_eq!(cache.refreshes(), 1);
    cache.settle();
    let after = ask(&cache, &profiles, &evidence, now);
    assert_ne!(after[0].1, std::thread::current().id());
    assert_eq!((cache.computed(), cache.refreshes()), (2, 1));
    // Once verified, it is not due any more.
    ask(&cache, &profiles, &evidence, now);
    assert_eq!((cache.computed(), cache.refreshes()), (2, 1));
    let _ = std::fs::remove_dir_all(dir);
}

/// Play, the Launch page, the bezel preview, the legacy pages and the
/// background workers get what they got before files were watched: an answer
/// less than two seconds old for the same profiles and evidence, or one
/// computed there and then. Never a held one, and never from another thread.
#[test]
fn everything_but_the_drawn_card_gets_fresh_answers_as_before() {
    let dir = folder("fresh");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let cache = FirmwareAnswerCache::with_settled_files();
    let here = std::thread::current().id();
    let start = Instant::now();

    let first = ask_fresh(&cache, &profiles, &Vec::new(), start);
    assert_eq!((texts(&first), first[0].1), (vec!["one/"], here));
    // Inside the two-second window the answer is reused, as it always was.
    let inside = start + FIRMWARE_ANSWER_RECHECK - Duration::from_millis(1);
    ask_fresh(&cache, &profiles, &Vec::new(), inside);
    assert_eq!(cache.computed(), 1);
    // After it, the file is read again here, although nothing changed.
    let after = start + FIRMWARE_ANSWER_RECHECK;
    let again = ask_fresh(&cache, &profiles, &Vec::new(), after);
    assert_eq!((texts(&again), again[0].1), (vec!["one/"], here));
    assert_eq!(cache.computed(), 2);

    // New evidence is answered at once, here: this is what lets Play go
    // ahead the moment the evidence has arrived.
    let evidence = vec!["record".to_string()];
    let with_evidence = ask_fresh(&cache, &profiles, &evidence, after);
    assert_eq!(
        (texts(&with_evidence), with_evidence[0].1),
        (vec!["one/record"], here)
    );
    assert_eq!(cache.computed(), 3);

    // The drawn card has since been shown a held answer for a changed file...
    std::fs::write(dir.join("bios.bin"), "two!").unwrap();
    let much_later = after + Duration::from_secs(30);
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Arc::new(Mutex::new(gate));
    let held = cache.answers(
        FirmwareUse::Shown,
        &(&profiles, &evidence),
        much_later,
        |index| vec![profiles[index].folder.clone()],
        || {
            let gate = Arc::clone(&gate);
            move |_: usize| {
                let _ = gate.lock().unwrap().recv();
                (
                    "from the slow verification".to_string(),
                    std::thread::current().id(),
                )
            }
        },
    );
    assert_eq!(texts(&held), ["one/record"]);
    assert_eq!(cache.refreshes(), 1);
    // ...but a click on Play does not get it, and does not wait for the
    // verification that is running: it reads the file itself.
    let clicked = ask_fresh(&cache, &profiles, &evidence, much_later);
    assert_eq!((texts(&clicked), clicked[0].1), (vec!["two!/record"], here));
    // The slow verification was started before that read: its answer is not
    // taken, by either kind of caller.
    release.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        texts(&ask_fresh(&cache, &profiles, &evidence, much_later)),
        ["two!/record"]
    );
    assert_eq!(
        texts(&ask(&cache, &profiles, &evidence, much_later)),
        ["two!/record"]
    );
    assert_eq!(cache.refreshes(), 1, "fresh answers never start a thread");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_verification_that_dies_does_not_stop_later_ones() {
    let dir = folder("panic");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let cache = FirmwareAnswerCache::<String>::with_settled_files();
    let now = Instant::now();
    let ask_with = |fail: bool| {
        cache.answers(
            FirmwareUse::Shown,
            &profiles,
            now,
            |index| vec![profiles[index].folder.clone()],
            || {
                let profiles = profiles.clone();
                move |index: usize| {
                    assert!(!fail, "this verification fails (expected by the test)");
                    std::fs::read_to_string(profiles[index].folder.join("bios.bin")).unwrap()
                }
            },
        )
    };
    assert_eq!(ask_with(false), ["one"]);
    std::fs::write(dir.join("bios.bin"), "two!").unwrap();
    assert_eq!(
        ask_with(true),
        ["one"],
        "the held answer, while it is verified"
    );
    cache.settle(); // the failed thread has let go
    assert_eq!(cache.refreshes(), 1);
    assert_eq!(
        ask_with(false),
        ["one"],
        "still held: nothing was delivered"
    );
    assert_eq!(cache.refreshes(), 2, "and another verification has started");
    cache.settle();
    assert_eq!(ask_with(false), ["two!"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_interface_is_not_woken_for_answers_that_did_not_change() {
    let dir = folder("same");
    std::fs::write(dir.join("bios.bin"), "one").unwrap();
    let profiles = vec![Profile {
        folder: dir.clone(),
    }];
    let cache = FirmwareAnswerCache::<String>::with_settled_files();
    let context = egui::Context::default();
    cache.wake_with(&context);
    let start = Instant::now();
    let ask_at = |now: Instant| {
        cache.answers(
            FirmwareUse::Shown,
            &profiles,
            now,
            |index| vec![profiles[index].folder.clone()],
            || {
                let profiles = profiles.clone();
                move |index: usize| {
                    std::fs::read_to_string(profiles[index].folder.join("bios.bin")).unwrap()
                }
            },
        )
    };
    ask_at(start);
    // The backstop verifies again and finds the same answer.
    ask_at(start + FIRMWARE_ANSWER_BACKSTOP);
    cache.settle();
    assert_eq!((cache.computed(), cache.refreshes()), (2, 1));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        crate::emulator_setup::answers::count(&context),
        0,
        "the remembered readiness input has nothing new to show"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn no_profiles_means_no_answers_and_no_work() {
    let cache = FirmwareAnswerCache::<String>::with_settled_files();
    let none: Vec<Profile> = Vec::new();
    for usage in [FirmwareUse::Shown, FirmwareUse::Fresh] {
        let answers = cache.answers(
            usage,
            &none,
            Instant::now(),
            |_| unreachable!("nothing to watch"),
            || |_: usize| -> String { unreachable!("nothing to compute") },
        );
        assert!(answers.is_empty());
    }
    assert_eq!((cache.computed(), cache.refreshes()), (0, 0));
}
