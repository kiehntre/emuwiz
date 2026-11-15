use super::*;
use crate::gui_v2::tests::archive;
use archivefs_core::PersistedArchive;
use archivefs_core::emulator_environment::retroarch::content_history::read_content_history_for_config;

fn row(id: i64, path: &str) -> PersistedArchive {
    let mut row = archive(id, &format!("Game {id}"), Some("Game Boy"));
    row.absolute_path = path.into();
    row.relative_path = path.trim_start_matches("/fixture/").into();
    row
}

/// Writes a `retroarch.cfg` naming `history.lpl` with `items`, and reads it
/// through the real core reader.
fn history(items: &str) -> (tempfile::TempDir, RetroArchContentHistory) {
    let home = tempfile::tempdir().unwrap();
    let cfg = home.path().join(".config/retroarch/retroarch.cfg");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(&cfg, "content_history_path = \"~/history.lpl\"\n").unwrap();
    std::fs::write(
        home.path().join("history.lpl"),
        format!(r#"{{"version": "1.5", "items": [{items}]}}"#),
    )
    .unwrap();
    let read = read_content_history_for_config(&HostReadOnlyFilesystem, &cfg, home.path());
    (home, read)
}

fn context() -> Context {
    Context {
        home: Some("/home/player".into()),
        temp_dir: "/tmp".into(),
        amiga_roots: vec!["/data/amiga-hdf-working".into()],
    }
}

fn snapshot(library: &Library, items: &str, missing: &[&str]) -> Snapshot {
    let (_home, read) = history(items);
    let missing: Vec<PathBuf> = missing.iter().map(PathBuf::from).collect();
    build_snapshot(Some(read), &PathIndex::new(library), &context(), &|path| {
        Some(!missing.iter().any(|gone| gone == path))
    })
}

#[test]
fn exact_paths_match_in_file_order_and_nothing_else_does() {
    let library = Library::new(vec![
        row(1, "/fixture/gb/Tetris (World).gb"),
        row(2, "/fixture/gb/Zelda.gb"),
        row(3, "/home/player/roms/Kirby.gb"),
    ]);
    let snapshot = snapshot(
        &library,
        r#"
        {"path": "/fixture/gb/Zelda.gb", "label": "", "core_name": "Gambatte"},
        {"path": "/fixture//gb/./Tetris (World).gb", "label": "Tetris"},
        {"path": "~/roms/Kirby.gb"},
        {"path": "/other/gb/Tetris (World).gb", "label": "Tetris (World)", "crc32": "46DF91AD|crc"},
        {"path": "/fixture/gb/zelda.gb", "label": "Zelda"}
        "#,
        &[],
    );
    let played: Vec<_> = snapshot.rows.iter().map(|row| row.played.clone()).collect();
    assert_eq!(
        played,
        vec![
            Played::Game(2),
            Played::Game(1),
            Played::Game(3),
            // Same file name, label and a CRC: still not this game.
            Played::NotInLibrary,
            // Case differs: a different path.
            Played::NotInLibrary,
        ]
    );
    assert_eq!(snapshot.games(), vec![2, 1, 3]);
    assert_eq!(snapshot.rows[0].name, "Zelda.gb", "empty label: file name");
    assert_eq!(snapshot.rows[0].core.as_deref(), Some("Gambatte"));
    assert_eq!(snapshot.rows[1].name, "Tetris");
    assert_eq!(snapshot.notice(), None);
    assert_eq!(snapshot.unmatched().count(), 2);
}

#[test]
fn emuwiz_copies_missing_files_and_unusable_paths_are_labelled_honestly() {
    let library = Library::new(vec![row(1, "/fixture/gb/Tetris.gb")]);
    let snapshot = snapshot(
        &library,
        r#"
        {"path": "/tmp/emuwiz-zip-rom-AbC123/Tetris.gb"},
        {"path": "/elsewhere/emuwiz-zip-rom-AbC123/Tetris.gb"},
        {"path": "/data/amiga-hdf-working/94d21bd3/Zak.hdf"},
        {"path": "/scratch/home/data/amiga-hdf-working/8d5b948c/1943.hdf"},
        {"path": "/gone/Old.gb"},
        {"path": "relative/Thing.gb"},
        {"label": "No path at all"},
        {"path": "/fixture/gb/Tetris.gb"}
        "#,
        &["/gone/Old.gb", "/elsewhere/emuwiz-zip-rom-AbC123/Tetris.gb"],
    );
    let played: Vec<_> = snapshot.rows.iter().map(|row| row.played.clone()).collect();
    assert_eq!(
        played,
        vec![
            Played::TemporaryCopy,
            // The prefix alone, outside the temp folder, proves nothing.
            Played::FileMissing,
            Played::AmigaWorkingCopy,
            Played::AmigaWorkingCopy,
            Played::FileMissing,
            Played::NoUsablePath,
            Played::NoUsablePath,
            Played::Game(1),
        ]
    );
    assert_eq!(snapshot.rows[6].name, "No path at all");
    assert_eq!(snapshot.games(), vec![1]);
}

#[test]
fn archive_members_match_their_container_and_repeats_collapse() {
    let library = Library::new(vec![row(7, "/fixture/gb/Pack.zip")]);
    let snapshot = snapshot(
        &library,
        r#"
        {"path": "/fixture/gb/Pack.zip#Inner.gb", "core_name": "Gambatte"},
        {"path": "/fixture/gb/Pack.zip#Inner.gb", "core_name": "SameBoy"},
        {"path": "/fixture/gb/Pack.zip#Other.gb"}
        "#,
        &[],
    );
    assert_eq!(snapshot.rows.len(), 2, "one row per recorded path");
    assert_eq!(snapshot.rows[0].name, "Inner.gb");
    assert_eq!(snapshot.rows[0].core.as_deref(), Some("Gambatte"));
    assert_eq!(snapshot.games(), vec![7], "each game once");
}

#[test]
fn a_moved_game_matches_its_current_copy_and_shared_paths_match_nothing() {
    let mut old = row(1, "/fixture/gb/Old Name.zip");
    old.archive_kind = "zip".into();
    old.size_bytes = Some(100);
    old.last_verified_missing_at = Some("2026-09-20T00:00:00Z".into());
    let mut new = row(2, "/fixture/gb/New Name.zip");
    new.archive_kind = "zip".into();
    new.size_bytes = Some(100);
    let library = Library::with_history(
        vec![old, new],
        &[(
            "/fixture/gb/Old Name.zip".into(),
            "/fixture/gb/New Name.zip".into(),
        )],
        None,
    );
    assert_eq!(library.resolve(1), 2);
    let moved = snapshot(&library, r#"{"path": "/fixture/gb/Old Name.zip"}"#, &[]);
    assert_eq!(moved.rows[0].played, Played::Game(2));

    let mut twin = row(4, "/fixture/gb/Same.gb");
    twin.source_folder_id = 2;
    let library = Library::new(vec![row(3, "/fixture/gb/Same.gb"), twin]);
    let shared = snapshot(&library, r#"{"path": "/fixture/gb/Same.gb"}"#, &[]);
    assert_eq!(shared.rows[0].played, Played::Ambiguous);
    assert!(shared.games().is_empty());
}

#[test]
fn missing_empty_and_malformed_histories_show_nothing_invented() {
    let library = Library::new(vec![row(1, "/fixture/gb/Tetris.gb")]);
    let none = build_snapshot(None, &PathIndex::new(&library), &context(), &|_| None);
    assert!(none.hidden() && none.rows.is_empty());

    let empty = snapshot(&library, "", &[]);
    assert!(!empty.hidden());
    assert!(empty.rows.is_empty());
    assert_eq!(
        empty.notice().as_deref(),
        Some("RetroArch's play history is empty.")
    );

    let home = tempfile::tempdir().unwrap();
    let cfg = home.path().join("retroarch.cfg");
    let read = |body: &str| {
        std::fs::write(home.path().join("history.lpl"), body).unwrap();
        build_snapshot(
            Some(read_content_history_for_config(
                &HostReadOnlyFilesystem,
                &cfg,
                home.path(),
            )),
            &PathIndex::new(&library),
            &context(),
            &|_| None,
        )
    };
    assert!(
        read("").hidden(),
        "no retroarch.cfg: RetroArch is not set up"
    );
    std::fs::write(&cfg, "content_history_path = \"~/history.lpl\"\n").unwrap();
    let malformed = read("{ \"items\": [ broken");
    assert!(malformed.rows.is_empty());
    assert!(malformed.notice().unwrap().contains("could not be read"));
    std::fs::remove_file(home.path().join("history.lpl")).unwrap();
    let missing = build_snapshot(
        Some(read_content_history_for_config(
            &HostReadOnlyFilesystem,
            &cfg,
            home.path(),
        )),
        &PathIndex::new(&library),
        &context(),
        &|_| None,
    );
    assert_eq!(
        missing.notice().as_deref(),
        Some("RetroArch has not recorded any played games yet.")
    );

    std::fs::write(
        &cfg,
        "history_list_enable = \"false\"\ncontent_history_path = \"~/history.lpl\"\n",
    )
    .unwrap();
    let off = read(r#"{"items": [{"path": "/fixture/gb/Tetris.gb"}]}"#);
    assert_eq!(off.games(), vec![1]);
    assert!(off.notice().unwrap().contains("turned off"));
}

#[test]
fn history_is_bounded_to_the_row_limit() {
    let library = Library::default();
    let items: Vec<String> = (0..MAX_ROWS + 5)
        .map(|index| format!(r#"{{"path": "/games/{index}.gb"}}"#))
        .collect();
    let snapshot = snapshot(&library, &items.join(","), &[]);
    assert_eq!(snapshot.rows.len(), MAX_ROWS);
    assert!(snapshot.truncated);
}

#[test]
fn normalise_resolves_tilde_and_refuses_other_relative_paths() {
    let home = Path::new("/home/p");
    assert_eq!(normalise("~", Some(home)), Some(PathBuf::from("/home/p")));
    assert_eq!(
        normalise("~/a//./b.gb", Some(home)),
        Some(PathBuf::from("/home/p/a/b.gb"))
    );
    assert_eq!(normalise("~/a.gb", None), None);
    assert_eq!(normalise("a.gb", Some(home)), None);
    assert_eq!(normalise(":/a.gb", Some(home)), None);
    // `..` is kept: resolving it could cross a symlink.
    assert_eq!(
        normalise("/a/../b.gb", None),
        Some(PathBuf::from("/a/../b.gb"))
    );
}

#[test]
fn reading_never_writes_the_history_or_config() {
    let (home, _) = history(r#"{"path": "/fixture/gb/Tetris.gb"}"#);
    let file = home.path().join("history.lpl");
    let cfg = home.path().join(".config/retroarch/retroarch.cfg");
    let before = (std::fs::read(&file).unwrap(), std::fs::read(&cfg).unwrap());
    let times = (
        std::fs::metadata(&file).unwrap().modified().unwrap(),
        std::fs::metadata(&cfg).unwrap().modified().unwrap(),
    );
    let library = Library::new(vec![row(1, "/fixture/gb/Tetris.gb")]);
    let read = read_content_history_for_config(&HostReadOnlyFilesystem, &cfg, home.path());
    let _ = build_snapshot(Some(read), &PathIndex::new(&library), &context(), &|_| None);
    assert_eq!(
        before,
        (std::fs::read(&file).unwrap(), std::fs::read(&cfg).unwrap())
    );
    assert_eq!(
        times,
        (
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            std::fs::metadata(&cfg).unwrap().modified().unwrap(),
        )
    );
}
