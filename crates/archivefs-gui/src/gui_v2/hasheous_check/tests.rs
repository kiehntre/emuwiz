//! Check Hasheous against fake HTTP answers only; no test contacts Hasheous.
//! Hash-on-click tests read synthetic files in a temporary folder.
use super::super::identity_review::review_for;
use super::super::library::{IdentityContext, Library};
use super::super::tests::archive;
use super::*;
use crate::selected_evidence_page::run_hasheous_check;
use archivefs_core::dat::library_identity_summary::LibraryItemHashes;
use archivefs_core::identity_source::hasheous::client::{
    HasheousConfig, HasheousHttpResponse, HasheousRequestError, HasheousTransport,
};
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";
const CONTENT: &[u8] = b"synthetic rom";
const FOUND: &str = r#"{
    "platform": {"name": "Nintendo Game Boy"},
    "publisher": {"name": "Fixture Soft"},
    "signatures": {"NoIntros": [{"game": {"name": "Fixture Quest (Europe)"}}]}
}"#;

fn content_sha1() -> String {
    use sha1::Digest;
    sha1::Sha1::digest(CONTENT)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A DAT-checked game: its current saved hashes carry `sha1`.
fn saved(id: i64, sha1: Option<&str>) -> Game {
    let mut game = Library::new(vec![archive(id, "Fixture Quest", Some("GB"))])
        .games
        .remove(0);
    game.saved_hashes = Some(LibraryItemHashes {
        size_bytes: Some(4),
        crc32: Some("deadbeef".into()),
        md5: Some("0123456789abcdef0123456789abcdef".into()),
        sha1: sha1.map(str::to_string),
        sha256: None,
    });
    game
}

/// An unidentified game whose Library row describes `path` as scanned now.
fn unidentified(id: i64, path: &Path) -> Game {
    let mut row = archive(id, "Mystery", Some("GB"));
    let seen = FileFingerprint::observe(path).expect("fixture file");
    row.absolute_path = path.to_path_buf();
    row.size_bytes = Some(seen.size_bytes);
    row.modified_time_unix_seconds = seen.modified_unix_seconds;
    Library::new(vec![row]).games.remove(0)
}

struct Folder {
    dir: tempfile::TempDir,
}

impl Folder {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
    fn roots(&self) -> Roots {
        let root = self.dir.path().to_path_buf();
        Arc::new(move || TrustedRoots::from_paths([root.clone()]))
    }
}

/// A fake Hasheous endpoint that records every request body it is sent.
struct Fake {
    answer: Result<HasheousHttpResponse, HasheousRequestError>,
    bodies: Mutex<Vec<Vec<u8>>>,
}

impl Fake {
    fn status(status: u16, body: &str) -> Arc<Self> {
        Arc::new(Self {
            answer: Ok(HasheousHttpResponse {
                status,
                body: body.as_bytes().to_vec(),
                retry_after_secs: None,
            }),
            bodies: Mutex::new(Vec::new()),
        })
    }

    fn error(error: HasheousRequestError) -> Arc<Self> {
        Arc::new(Self {
            answer: Err(error),
            bodies: Mutex::new(Vec::new()),
        })
    }

    fn sent(&self) -> Vec<serde_json::Value> {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .map(|body| serde_json::from_slice(body).unwrap())
            .collect()
    }
}

impl HasheousTransport for Fake {
    fn post_json(
        &self,
        _url: &str,
        body: &[u8],
    ) -> Result<HasheousHttpResponse, HasheousRequestError> {
        self.bodies.lock().unwrap().push(body.to_vec());
        self.answer.clone()
    }
}

fn lookup_via(fake: &Arc<Fake>) -> Lookup {
    let fake = Arc::clone(fake);
    Arc::new(move |sha1| {
        let config = HasheousConfig {
            enabled: true,
            ..HasheousConfig::default()
        };
        run_hasheous_check(&config, fake.as_ref(), sha1)
    })
}

fn no_roots() -> Roots {
    Arc::new(TrustedRoots::none)
}

fn state(fake: &Arc<Fake>, roots: Roots) -> State {
    State::with(lookup_via(fake), roots)
}

fn settle(state: &mut State, game: &Game) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while matches!(state.view(game), View::Hashing(_) | View::LookingUp) {
        assert!(Instant::now() < deadline, "lookup never finished");
        std::thread::sleep(Duration::from_millis(5));
        state.poll();
    }
}

fn outcome_for(fake: &Arc<Fake>) -> String {
    let game = saved(1, Some(SHA1));
    let mut state = state(fake, no_roots());
    assert!(state.start(&game, None));
    settle(&mut state, &game);
    match state.view(&game) {
        View::Done { outcome, .. } => status_line(outcome),
        other => panic!("expected a finished lookup, got {other:?}"),
    }
}

#[test]
fn a_dat_checked_game_sends_its_saved_sha1_and_shows_the_match() {
    let fake = Fake::status(200, FOUND);
    let game = saved(1, Some(&SHA1.to_ascii_uppercase()));
    // No trusted roots: the saved SHA-1 path never reads the file.
    let mut state = state(&fake, no_roots());
    assert!(matches!(
        state.view(&game),
        View::Idle { hash_first: false }
    ));
    assert!(state.start(&game, None));
    settle(&mut state, &game);
    let View::Done {
        outcome,
        worked_out_here: false,
    } = state.view(&game)
    else {
        panic!("not finished");
    };
    assert_eq!(
        status_line(outcome),
        "Hasheous lists this SHA-1 in 1 catalogue entry"
    );
    let HasheousCheckOutcome::Found(observations) = outcome else {
        panic!("expected a match");
    };
    let summary = summarise(observations);
    assert_eq!(
        summary.matches,
        vec![ReportedMatch {
            source: "No-Intro".into(),
            title: Some("Fixture Quest (Europe)".into()),
        }]
    );
    assert_eq!(summary.platform.as_deref(), Some("Nintendo Game Boy"));
    assert_eq!(summary.publisher.as_deref(), Some("Fixture Soft"));
    // Only the saved SHA-1 was sent: no path, name, title or other hash.
    assert_eq!(fake.sent(), vec![serde_json::json!({ "sha1": SHA1 })]);
    let raw = String::from_utf8_lossy(&fake.bodies.lock().unwrap()[0]).to_string();
    assert!(!raw.contains("fixture") && !raw.contains("Fixture"));
}

#[test]
fn an_unidentified_game_is_hashed_on_click_and_only_its_sha1_is_sent() {
    let folder = Folder::new();
    let path = folder.file("Mystery.gb", CONTENT);
    let game = unidentified(1, &path);
    assert!(game.saved_hashes.is_none() && game.dat_exact.is_none());
    let fake = Fake::status(200, FOUND);
    let mut state = state(&fake, folder.roots());
    assert!(matches!(state.view(&game), View::Idle { hash_first: true }));
    assert!(fake.sent().is_empty(), "showing the button sent nothing");
    assert!(state.start(&game, None));
    settle(&mut state, &game);
    let View::Done {
        outcome: HasheousCheckOutcome::Found(_),
        worked_out_here: true,
    } = state.view(&game)
    else {
        panic!("expected a match, got {:?}", state.view(&game));
    };
    assert_eq!(
        fake.sent(),
        vec![serde_json::json!({ "sha1": content_sha1() })]
    );
    // The file is untouched and nothing was saved on the game.
    assert_eq!(std::fs::read(&path).unwrap(), CONTENT);
    assert!(game.saved_hashes.is_none());
}

#[test]
fn unsupported_or_unreadable_games_explain_why_and_send_nothing() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let folder = Folder::new();
    let mut state = State::with(
        Arc::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            HasheousCheckOutcome::NoMatch
        }),
        folder.roots(),
    );
    let mut cases = Vec::new();
    for (name, reason) in [
        ("Set.zip", ARCHIVE),
        ("Disc.7z", ARCHIVE),
        ("Disc.chd", COMPRESSED_DISC),
        ("Disc.cue", DESCRIPTOR),
        ("Disc.m3u", DESCRIPTOR),
    ] {
        cases.push((
            unidentified(cases.len() as i64 + 1, &folder.file(name, CONTENT)),
            reason,
        ));
    }
    let mut missing = unidentified(20, &folder.file("Gone.gb", CONTENT));
    missing.attention = true;
    cases.push((missing, UNAVAILABLE_FILE));
    let mut incomplete = unidentified(21, &folder.file("Half.gb", CONTENT));
    incomplete.inspection_incomplete = true;
    cases.push((incomplete, INCOMPLETE_SCAN));
    let mut no_size = unidentified(22, &folder.file("Unsized.gb", CONTENT));
    no_size.archive.size_bytes = None;
    cases.push((no_size, NO_SCAN_SIZE));
    // Saved hashes this session's own check found no longer hold.
    let mut invalidated = saved(23, Some(SHA1));
    invalidated.dat_proof_invalidated = true;
    cases.push((invalidated, FILE_CHANGED));
    for (game, reason) in &cases {
        assert!(
            matches!(state.view(game), View::Unavailable(r) if r == *reason),
            "{:?}: {:?}",
            game.archive.absolute_path,
            state.view(game)
        );
        assert!(!state.start(game, None));
    }
    // A saved row without a usable SHA-1 is not trusted: the file is read instead.
    let mut weak = unidentified(30, &folder.file("Weak.gb", CONTENT));
    weak.saved_hashes = Some(LibraryItemHashes {
        sha1: Some("not-a-sha1".into()),
        ..LibraryItemHashes::default()
    });
    assert!(matches!(state.view(&weak), View::Idle { hash_first: true }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_file_changed_since_the_scan_or_linked_outside_trusted_folders_is_refused() {
    let folder = Folder::new();
    let path = folder.file("Mystery.gb", CONTENT);
    let game = unidentified(1, &path);
    std::fs::write(&path, b"different, longer synthetic rom").unwrap();
    let fake = Fake::status(200, FOUND);
    let mut state = state(&fake, folder.roots());
    assert!(state.start(&game, None));
    settle(&mut state, &game);
    assert!(
        matches!(state.view(&game), View::Refused(CHANGED_SINCE_SCAN)),
        "{:?}",
        state.view(&game)
    );

    // The shared read policy: a library symlink may not lead outside the
    // trusted game folders.
    let other = Folder::new();
    let target = other.file("Elsewhere.gb", CONTENT);
    let link = folder.dir.path().join("Linked.gb");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let outside = unidentified(2, &link);
    assert!(state.start(&outside, None));
    settle(&mut state, &outside);
    assert!(
        matches!(state.view(&outside), View::Refused(reason) if reason.starts_with("This file could not be read")),
        "{:?}",
        state.view(&outside)
    );
    assert!(fake.sent().is_empty());
}

#[test]
fn cancelling_while_hashing_sends_nothing() {
    let folder = Folder::new();
    let game = unidentified(1, &folder.file("Mystery.gb", CONTENT));
    let (release, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let root = folder.dir.path().to_path_buf();
    // The worker waits here, before it opens the file, until released.
    let roots: Roots = Arc::new(move || {
        gate.lock().unwrap().recv().unwrap();
        TrustedRoots::from_paths([root.clone()])
    });
    let fake = Fake::status(200, FOUND);
    let mut state = state(&fake, roots);
    assert!(state.start(&game, None));
    assert!(matches!(state.view(&game), View::Hashing(None)));
    assert!(!state.start(&game, None), "a second click starts nothing");
    state.cancel(&game);
    release.send(()).unwrap();
    settle(&mut state, &game);
    assert!(matches!(state.view(&game), View::Cancelled));
    assert!(fake.sent().is_empty());
}

#[test]
fn no_match_unavailable_service_and_network_failure_read_differently() {
    let no_match = outcome_for(&Fake::status(404, ""));
    let server = outcome_for(&Fake::status(500, ""));
    let busy = outcome_for(&Fake::status(429, ""));
    let offline = outcome_for(&Fake::error(HasheousRequestError::Network {
        detail: "connection refused".into(),
    }));
    let slow = outcome_for(&Fake::error(HasheousRequestError::Timeout));
    assert_eq!(no_match, "Hasheous has no entry for this SHA-1");
    assert!(
        server.starts_with("Hasheous could not be checked:"),
        "{server}"
    );
    assert!(server.contains("500"), "{server}");
    assert!(busy.contains("slow down"), "{busy}");
    assert!(offline.contains("connection refused"), "{offline}");
    assert!(slow.contains("did not answer in time"), "{slow}");
    let all = [&no_match, &server, &busy, &offline, &slow];
    for (i, a) in all.iter().enumerate() {
        assert!(!a.contains("lists this SHA-1"), "{a}");
        for b in &all[i + 1..] {
            assert_ne!(a, b);
        }
    }
    // A malformed provider body is a failure, never a match.
    let garbage = outcome_for(&Fake::status(200, "<html>not json</html>"));
    assert!(
        garbage.starts_with("Hasheous could not be checked:"),
        "{garbage}"
    );
}

#[test]
fn a_late_answer_stays_with_the_game_and_file_it_was_asked_for() {
    let (release, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let mut state = State::with(
        Arc::new(move |_| {
            gate.lock().unwrap().recv().unwrap();
            HasheousCheckOutcome::NoMatch
        }),
        no_roots(),
    );
    let first = saved(1, Some(SHA1));
    let second = saved(2, Some(SHA1));
    assert!(state.start(&first, None));
    assert!(!state.start(&first, None));
    // The person moves to another game before Hasheous answers.
    assert!(matches!(state.view(&second), View::Idle { .. }));
    release.send(()).unwrap();
    settle(&mut state, &first);
    assert!(matches!(
        state.view(&first),
        View::Done {
            outcome: HasheousCheckOutcome::NoMatch,
            ..
        }
    ));
    assert!(matches!(state.view(&second), View::Idle { .. }));
    // After a re-check saves different hashes, the old answer is not shown.
    assert!(matches!(
        state.view(&saved(1, Some(&"f".repeat(40)))),
        View::Idle { .. }
    ));

    // A worked-out answer belongs to the scanned file: a rescan that records a
    // different file hides it.
    let folder = Folder::new();
    let path = folder.file("Mystery.gb", CONTENT);
    let game = unidentified(3, &path);
    let mut hashed = State::with(lookup_via(&Fake::status(404, "")), folder.roots());
    hashed.start(&game, None);
    settle(&mut hashed, &game);
    assert!(matches!(hashed.view(&game), View::Done { .. }));
    let mut rescanned = game.clone();
    rescanned.archive.size_bytes = Some(999);
    assert!(matches!(hashed.view(&rescanned), View::Idle { .. }));
}

#[test]
fn a_match_changes_no_game_metadata_identity_or_dat_verification() {
    let folder = Folder::new();
    let context = IdentityContext::default();
    for game in [
        saved(1, Some(SHA1)),
        unidentified(2, &folder.file("Mystery.gb", CONTENT)),
    ] {
        let before = (format!("{game:?}"), review_for(&game, &context, None));
        let mut state = state(&Fake::status(200, FOUND), folder.roots());
        state.start(&game, None);
        settle(&mut state, &game);
        assert!(matches!(
            state.view(&game),
            View::Done {
                outcome: HasheousCheckOutcome::Found(_),
                ..
            }
        ));
        let after = (format!("{game:?}"), review_for(&game, &context, None));
        assert_eq!(before, after);
        // The remote match is never DAT verification.
        assert!(game.dat_exact.is_none() && game.dat_state.is_none());
        assert!(!after.1.is_verified());
    }
    assert!(NOT_VERIFICATION.contains("not checked against your catalogues"));
    assert!(NOT_VERIFICATION.contains("does not verify the game"));
}

#[test]
fn provider_text_is_flattened_and_bounded() {
    let long = format!("Line one\nline\u{1b}[31m two {}", "x".repeat(400));
    let cleaned = clean(&long);
    assert!(!cleaned.chars().any(char::is_control), "{cleaned:?}");
    assert!(cleaned.chars().count() <= 121);
    assert!(cleaned.ends_with('…'));
}
