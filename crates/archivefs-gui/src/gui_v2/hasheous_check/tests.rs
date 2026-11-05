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
        member: None,
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
        member: None,
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
        ("Set.rar", ARCHIVE),
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
    assert_eq!(no_match, NO_MATCH);
    assert!(NO_MATCH.contains("does not mean the game is unknown"));
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

// -- A ZIP holding one cartridge ROM -----------------------------------------

/// Writes a ZIP of `(name, bytes, stored)` members into the folder.
fn zip_file(folder: &Folder, name: &str, members: &[(&str, &[u8], bool)]) -> PathBuf {
    use std::io::Write;
    let path = folder.dir.path().join(name);
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (member, bytes, stored) in members {
        let method = if *stored {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        writer
            .start_file(
                *member,
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
    path
}

/// Runs one click for `game` against a fake Hasheous; returns what it shows
/// and every request body sent.
fn zip_check(folder: &Folder, game: &Game) -> (String, Vec<serde_json::Value>) {
    let fake = Fake::status(200, FOUND);
    let mut state = state(&fake, folder.roots());
    assert!(matches!(state.view(game), View::Idle { hash_first: true }));
    assert!(state.start(game, None));
    settle(&mut state, game);
    let shown = match state.view(game) {
        View::Done {
            member: Some(member),
            ..
        } => format!("matched {member}"),
        View::Refused(reason) => reason.to_string(),
        other => format!("{other:?}"),
    };
    (shown, fake.sent())
}

#[test]
fn a_zip_with_one_rom_sends_that_roms_sha1_not_the_zips() {
    let folder = Folder::new();
    let path = zip_file(
        &folder,
        "Mystery.zip",
        &[
            ("Mystery.gb", CONTENT, false),
            ("readme.txt", b"notes", false),
        ],
    );
    let game = unidentified(1, &path);
    let (shown, sent) = zip_check(&folder, &game);
    assert_eq!(shown, "matched Mystery.gb");
    assert_eq!(sent, vec![serde_json::json!({ "sha1": content_sha1() })]);
    // The ZIP itself is untouched and nothing was extracted beside it.
    let names: Vec<_> = std::fs::read_dir(folder.dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("Mystery.zip")]);
    assert!(zip_worked_out("Mystery.gb").contains("no header removed"));
}

#[test]
fn ambiguous_empty_or_wrong_system_zips_are_refused_before_reading() {
    let folder = Folder::new();
    let two = zip_file(
        &folder,
        "Two.zip",
        &[("A.gb", CONTENT, false), ("B.gb", b"other rom", false)],
    );
    let none = zip_file(&folder, "None.zip", &[("readme.txt", b"notes", false)]);
    let wrong = zip_file(&folder, "Wrong.zip", &[("Game.sfc", CONTENT, false)]);
    let nested = zip_file(&folder, "Dir.zip", &[("sub/../../Game.gb", CONTENT, true)]);
    for (path, expected) in [
        (two, zip_rom::SEVERAL_ROMS),
        (none, zip_rom::NO_ROM),
        (wrong, zip_rom::NO_ROM),
        (nested, zip_rom::UNSAFE_ENTRY),
    ] {
        let (shown, sent) = zip_check(&folder, &unidentified(1, &path));
        assert_eq!(shown, expected, "{path:?}");
        assert!(sent.is_empty());
    }
}

#[test]
fn malformed_corrupt_encrypted_and_oversized_zips_are_refused() {
    let folder = Folder::new();
    let garbage = folder.file("Garbage.zip", b"PK\x03\x04 this is not a zip");
    // A stored member whose bytes no longer match its recorded CRC.
    let corrupt = zip_file(&folder, "Corrupt.zip", &[("Game.gb", CONTENT, true)]);
    let mut bytes = std::fs::read(&corrupt).unwrap();
    let at = bytes
        .windows(CONTENT.len())
        .position(|window| window == CONTENT)
        .unwrap();
    bytes[at] ^= 0xff;
    std::fs::write(&corrupt, &bytes).unwrap();
    // The general-purpose "encrypted" flag set in every header.
    let encrypted = zip_file(&folder, "Locked.zip", &[("Game.gb", CONTENT, true)]);
    let mut bytes = std::fs::read(&encrypted).unwrap();
    patch_headers(&mut bytes, |header, flags_at, _| header[flags_at] |= 1);
    std::fs::write(&encrypted, &bytes).unwrap();
    // A member that declares more than a cartridge ROM may be.
    let oversized = zip_file(&folder, "Huge.zip", &[("Game.gb", CONTENT, true)]);
    let mut bytes = std::fs::read(&oversized).unwrap();
    let huge = (zip_rom::MAX_ROM_BYTES + 1) as u32;
    patch_headers(&mut bytes, |header, _, size_at| {
        header[size_at..size_at + 4].copy_from_slice(&huge.to_le_bytes())
    });
    std::fs::write(&oversized, &bytes).unwrap();
    for (path, expected) in [
        (garbage, zip_rom::DAMAGED),
        (corrupt, zip_rom::DAMAGED),
        (encrypted, zip_rom::UNSAFE_ENTRY),
        (oversized, zip_rom::TOO_LARGE),
    ] {
        let (shown, sent) = zip_check(&folder, &unidentified(1, &path));
        assert_eq!(shown, expected, "{path:?}");
        assert!(sent.is_empty(), "{path:?}");
    }
}

/// Applies `edit(header, flags offset, uncompressed-size offset)` to every
/// local and central ZIP header.
fn patch_headers(bytes: &mut [u8], edit: impl Fn(&mut [u8], usize, usize)) {
    let mut at = 0;
    while at + 4 <= bytes.len() {
        match &bytes[at..at + 4] {
            [0x50, 0x4b, 0x03, 0x04] => edit(&mut bytes[at..], 6, 22),
            [0x50, 0x4b, 0x01, 0x02] => edit(&mut bytes[at..], 8, 24),
            _ => {}
        }
        at += 1;
    }
}

#[test]
fn a_zip_changed_since_the_scan_is_not_read() {
    let folder = Folder::new();
    let path = zip_file(&folder, "Mystery.zip", &[("Mystery.gb", CONTENT, false)]);
    let game = unidentified(1, &path);
    zip_file(
        &folder,
        "Mystery.zip",
        &[("Mystery.gb", b"a different, longer synthetic rom", false)],
    );
    let (shown, sent) = zip_check(&folder, &game);
    assert_eq!(shown, CHANGED_SINCE_SCAN);
    assert!(sent.is_empty());
}

#[test]
fn cancelling_a_zip_read_sends_nothing() {
    let folder = Folder::new();
    let game = unidentified(
        1,
        &zip_file(&folder, "Mystery.zip", &[("Mystery.gb", CONTENT, false)]),
    );
    let (release, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let root = folder.dir.path().to_path_buf();
    let roots: Roots = Arc::new(move || {
        gate.lock().unwrap().recv().unwrap();
        TrustedRoots::from_paths([root.clone()])
    });
    let fake = Fake::status(200, FOUND);
    let mut state = state(&fake, roots);
    assert!(state.start(&game, None));
    state.cancel(&game);
    release.send(()).unwrap();
    settle(&mut state, &game);
    assert!(
        matches!(state.view(&game), View::Cancelled),
        "{:?}",
        state.view(&game)
    );
    assert!(fake.sent().is_empty());
}

// -- File-read boundary --------------------------------------------------------

/// A Library row as it would look for `path` with a synthetic scan record.
fn row_for(id: i64, path: &Path) -> Game {
    let mut row = archive(id, "Row", Some("GB"));
    row.absolute_path = path.to_path_buf();
    row.size_bytes = Some(4);
    row.modified_time_unix_seconds = Some(1);
    Library::new(vec![row]).games.remove(0)
}

#[test]
fn a_library_row_naming_a_non_game_file_is_never_read_or_sent() {
    let folder = Folder::new();
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&reads);
    let root = folder.dir.path().to_path_buf();
    let fake = Fake::status(200, FOUND);
    let mut state = State::with(
        lookup_via(&fake),
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            TrustedRoots::from_paths([root.clone()])
        }),
    );
    for name in ["id_ed25519", "secrets.txt", "key.pem", "notes.json"] {
        let game = unidentified(1, &folder.file(name, CONTENT));
        assert!(
            matches!(state.view(&game), View::Unavailable(NOT_GAME_FILE)),
            "{name}: {:?}",
            state.view(&game)
        );
        assert!(!state.start(&game, None));
    }
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(fake.sent().is_empty());
}

#[test]
fn directories_and_special_files_are_refused_without_opening_them() {
    let folder = Folder::new();
    let dir = folder.dir.path().join("Folder.gb");
    std::fs::create_dir(&dir).unwrap();
    let fifo = folder.dir.path().join("Pipe.gb");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let fake = Fake::status(200, FOUND);
    let mut state = state(&fake, folder.roots());
    let device = folder.dir.path().join("Device.gb");
    std::os::unix::fs::symlink("/dev/null", &device).unwrap();
    for path in [dir, fifo, device] {
        let game = row_for(1, &path);
        assert!(state.start(&game, None), "{path:?}");
        // A FIFO is never opened, so this cannot block.
        settle(&mut state, &game);
        assert!(
            matches!(state.view(&game), View::Refused(UNAVAILABLE_FILE)),
            "{path:?}: {:?}",
            state.view(&game)
        );
    }
    assert!(fake.sent().is_empty());
}

/// Replaces `path` with same-length bytes and the same modification time,
/// as a different file object, between the identity check and the read.
fn replaced_during_read(folder: &Folder, game: &Game, path: &Path, replacement: &[u8]) -> String {
    let (release, gate) = mpsc::channel::<()>();
    let (entered, waiting) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let entered = Mutex::new(entered);
    let root = folder.dir.path().to_path_buf();
    // Entered only after the worker has taken its "before" identity.
    let roots: Roots = Arc::new(move || {
        entered.lock().unwrap().send(()).unwrap();
        gate.lock().unwrap().recv().unwrap();
        TrustedRoots::from_paths([root.clone()])
    });
    let fake = Fake::status(200, FOUND);
    let mut state = state(&fake, roots);
    assert!(state.start(game, None));
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let modified = std::fs::metadata(path).unwrap().modified().unwrap();
    let staged = path.with_extension("staged");
    std::fs::write(&staged, replacement).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&staged)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    std::fs::rename(&staged, path).unwrap();
    assert_eq!(
        std::fs::metadata(path).unwrap().len() as usize,
        replacement.len()
    );
    release.send(()).unwrap();
    settle(&mut state, game);
    assert!(fake.sent().is_empty(), "a replaced file's hash was sent");
    match state.view(game) {
        View::Refused(reason) => reason.to_string(),
        other => format!("{other:?}"),
    }
}

#[test]
fn a_same_size_replacement_during_the_read_is_refused() {
    let folder = Folder::new();
    let path = folder.file("Mystery.gb", CONTENT);
    let game = unidentified(1, &path);
    let shown = replaced_during_read(&folder, &game, &path, b"SYNTHETIC ROM");
    assert_eq!(shown, CHANGED_SINCE_SCAN);

    let zip = zip_file(&folder, "Mystery.zip", &[("Mystery.gb", CONTENT, true)]);
    let zipped = unidentified(2, &zip);
    let twin = zip_file(
        &folder,
        "Twin.zip",
        &[("Mystery.gb", b"SYNTHETIC ROM", true)],
    );
    let bytes = std::fs::read(&twin).unwrap();
    assert_eq!(bytes.len() as u64, std::fs::metadata(&zip).unwrap().len());
    let shown = replaced_during_read(&folder, &zipped, &zip, &bytes);
    assert_eq!(shown, CHANGED_SINCE_SCAN);
}

#[test]
fn a_headered_nes_rom_sends_its_whole_file_sha1_header_included() {
    use sha1::Digest;
    let folder = Folder::new();
    // Synthetic: a 16-byte iNES-style header followed by 32 bytes of "PRG".
    let mut rom = b"NES\x1a\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00".to_vec();
    rom.extend_from_slice(&[0xea; 32]);
    let game = unidentified(1, &folder.file("Synthetic.nes", &rom));
    let fake = Fake::status(404, "");
    let mut state = state(&fake, folder.roots());
    state.start(&game, None);
    settle(&mut state, &game);
    let hex = |bytes: &[u8]| -> String {
        sha1::Sha1::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    assert_eq!(fake.sent(), vec![serde_json::json!({ "sha1": hex(&rom) })]);
    assert_ne!(hex(&rom), hex(&rom[16..]), "no header is removed");
    assert!(matches!(
        state.view(&game),
        View::Done {
            outcome: HasheousCheckOutcome::NoMatch,
            ..
        }
    ));
    assert!(WHOLE_FILE_NOTE.contains("including any ROM header"));
}
