//! Disc sets in a bulk Move: detection, planning, approval, Apply/Undo and
//! the carry of whole-disc checks. Disposable folders and database only.
use super::disc_sets::{SheetKind, is_plain_reference, parse_references};
use super::game_rename::RetroArchSaveLocations;
use super::model::{OrganisationMode, OrganisationPlan, OrganisationStatus};
use super::plan::{
    OrganisationCandidate, OrganisationPlanRequest, build_organisation_plan_reporting,
};
use super::rename_guard::guard_bulk_rename_plan;
use super::transaction::{
    apply_organisation_transaction, build_organisation_transaction, live_resolution_for,
    prepare_organisation_catalogue, rollback_organisation_transaction,
};
use crate::dat::classification::ContentSelectionPolicy;
use crate::dat::library_identity_summary::DiscFileState;
use crate::dat::rename_apply::{RenameTransaction, RollbackResult, TransactionState};
use crate::identity_source::hashing::FileFingerprint;
use crate::safe_read::TrustedRoots;
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

struct Lib {
    temp: tempfile::TempDir,
    db: crate::Database,
    root: PathBuf,
}

impl Lib {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        std::fs::create_dir_all(root.join("incoming").join("psx")).unwrap();
        let mut db = crate::Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
        db.register_source_folders(std::slice::from_ref(&root))
            .unwrap();
        Self { temp, db, root }
    }

    /// A file in `incoming/` with no library row.
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root.join("incoming").join("psx").join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    /// A file in `incoming/` with a library row and a manual platform.
    fn game(&mut self, name: &str, text: &str, platform: &str) -> PathBuf {
        let path = self.file(name, text);
        let id = self.row(&path);
        self.db.set_manual_platform(id, platform).unwrap();
        path
    }

    fn row(&mut self, path: &Path) -> i64 {
        let source = self.db.list_source_folders().unwrap()[0].id;
        // A loose sheet is catalogued only after strict disc inspection;
        // these synthetic files are stored as plain game images.
        let archive = crate::Archive::from_path_in_root(path, &self.root).unwrap_or_else(|| {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            crate::Archive {
                path: path.to_path_buf(),
                kind: crate::ArchiveKind::DirectGameImage,
                identity: crate::ArchiveIdentity::from_path(path, &self.root, Some(&metadata)),
                health: crate::ArchiveHealth::Pending,
            }
        });
        self.db
            .upsert_archive(source, &self.root, &archive)
            .unwrap()
            .archive_id
    }

    fn id(&self, path: &Path) -> i64 {
        self.db
            .find_archive_id_by_absolute_path(path)
            .unwrap()
            .unwrap()
    }

    fn sql(&self) -> Connection {
        Connection::open(self.db.path()).unwrap()
    }

    fn journal(&self) -> PathBuf {
        self.temp.path().join("journal")
    }

    /// The bulk preview as the page builds it: plan (verified names would
    /// rename), guard, catalogue binding.
    fn preview(&self, mode: OrganisationMode, sources: &[&Path]) -> OrganisationPlan {
        let candidates: Vec<OrganisationCandidate> = sources
            .iter()
            .map(|source| OrganisationCandidate {
                source_path: source.to_path_buf(),
                resolution: live_resolution_for(&self.db, source, 1),
                canonical_name: Some(format!(
                    "Verified {}",
                    source.file_name().unwrap().to_string_lossy()
                )),
                content_classification: None,
                original_metadata: Default::default(),
            })
            .collect();
        let mut ignore = |_: crate::job_progress::JobProgress| {};
        let mut reporter = crate::job_progress::ProgressReporter::new(&mut ignore);
        let mut plan = build_organisation_plan_reporting(
            &OrganisationPlanRequest {
                master_root: &self.root,
                mode,
                content_policy: ContentSelectionPolicy::AllEntries,
                candidates: &candidates,
                generation: 1,
                unknown_folder: None,
                link_target_roots: None,
            },
            None,
            &mut reporter,
        )
        .unwrap();
        guard_bulk_rename_plan(&self.db, &mut plan, &RetroArchSaveLocations::default());
        prepare_organisation_catalogue(&mut plan, &self.db).unwrap();
        plan
    }

    fn apply(
        &self,
        plan: &OrganisationPlan,
        approved: &BTreeSet<String>,
    ) -> Result<RenameTransaction, String> {
        let mut tx = build_organisation_transaction(plan, approved, 1)?;
        let result = apply_organisation_transaction(
            &mut tx,
            approved,
            1,
            TrustedRoots::from_paths([self.root.clone()]),
            &self.journal(),
            &AtomicBool::new(false),
            plan.mode,
            &self.root,
            &self.db,
        );
        result.map(|_| tx).map_err(|error| error.to_string())
    }

    fn undo(&self, tx: &mut RenameTransaction) -> RollbackResult {
        let path =
            crate::dat::rename_apply::journal_path(&self.journal(), &tx.transaction_id).unwrap();
        *tx = crate::dat::rename_apply::read_journal(&path).unwrap();
        rollback_organisation_transaction(tx, &self.journal(), &AtomicBool::new(false), &self.root)
            .unwrap()
            .rollback
            .result
    }
}

fn rows_of(plan: &OrganisationPlan) -> BTreeMap<String, (OrganisationStatus, String)> {
    plan.entries
        .iter()
        .map(|entry| {
            (
                entry
                    .source_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                (entry.status, entry.reason.clone().unwrap_or_default()),
            )
        })
        .collect()
}

fn all_of(plan: &OrganisationPlan) -> BTreeSet<String> {
    plan.suggested()
        .map(|entry| entry.source_path.to_string_lossy().into_owned())
        .collect()
}

/// A PSX disc: `Game.cue` (in the library) naming two tracks (not).
fn cue_set(lib: &mut Lib) -> (PathBuf, PathBuf, PathBuf) {
    let cue = lib.game(
        "Game.cue",
        "FILE \"Game (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\nFILE Game_2.bin BINARY\n",
        "PSX",
    );
    let one = lib.file("Game (Track 1).bin", "track one");
    let two = lib.file("Game_2.bin", "track two");
    (cue, one, two)
}

#[test]
fn sheets_are_parsed_by_kind() {
    assert_eq!(
        parse_references(
            SheetKind::Cue,
            "\u{feff}REM x\nFILE \"Game (Track 1).bin\" BINARY\n  TRACK 01 AUDIO\nfile track2.bin WAVE\nFILE noType.bin\n"
        ),
        ["Game (Track 1).bin", "track2.bin", "noType.bin"]
    );
    assert_eq!(
        parse_references(
            SheetKind::Playlist,
            "#EXTM3U\n\nSaga (Disc 1).chd\r\n  # a comment\nSaga (Disc 2).chd\n"
        ),
        ["Saga (Disc 1).chd", "Saga (Disc 2).chd"]
    );
    assert_eq!(
        parse_references(
            SheetKind::Gdi,
            "3\n1 0 4 2352 track01.bin 0\n2 600 0 2352 \"track 02.raw\" 0\n\n3 45000 4 2352 track03.bin 0\n"
        ),
        ["track01.bin", "track 02.raw", "track03.bin"]
    );
    for unsafe_name in [
        "../x.bin",
        "a/b.bin",
        "/abs/x.bin",
        "a\\b.bin",
        "",
        "..",
        ".",
    ] {
        assert!(!is_plain_reference(unsafe_name), "{unsafe_name:?}");
    }
    assert!(is_plain_reference("Game (Track 1).bin"));
}

#[test]
fn a_cue_and_its_tracks_move_together_into_one_folder_with_names_unchanged() {
    let mut lib = Lib::new();
    let (cue, one, two) = cue_set(&mut lib);
    let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
    let folder = plan.entries[0]
        .destination_path
        .parent()
        .unwrap()
        .to_path_buf();
    assert_ne!(folder, lib.root.join("incoming").join("psx"));
    for entry in &plan.entries {
        assert_eq!(entry.status, OrganisationStatus::Suggested, "{entry:?}");
        assert_eq!(entry.disc_set.as_deref(), Some(cue.as_path()));
        assert_eq!(entry.platform.as_deref(), Some("PSX"));
        assert_eq!(
            entry.destination_path,
            folder.join(entry.source_path.file_name().unwrap()),
            "names unchanged, one folder"
        );
    }
    let track = plan.entries.iter().find(|e| e.source_path == one).unwrap();
    assert_eq!(track.platform_source, "Disc set: same as Game.cue");
}

#[test]
fn a_playlist_and_its_discs_move_together() {
    let mut lib = Lib::new();
    let m3u = lib.file(
        "Saga.m3u",
        "#EXTM3U\nSaga (Disc 1).chd\nSaga (Disc 2).chd\n",
    );
    let one = lib.game("Saga (Disc 1).chd", "disc one", "PSX");
    let two = lib.game("Saga (Disc 2).chd", "disc two", "PSX");
    let plan = lib.preview(OrganisationMode::MoveRealFile, &[&m3u, &one, &two]);
    let folder = plan.entries[0]
        .destination_path
        .parent()
        .unwrap()
        .to_path_buf();
    for entry in &plan.entries {
        assert_eq!(entry.status, OrganisationStatus::Suggested, "{entry:?}");
        assert_eq!(entry.disc_set.as_deref(), Some(m3u.as_path()));
        assert_eq!(
            entry.destination_path,
            folder.join(entry.source_path.file_name().unwrap())
        );
    }
    // The playlist has no library row: it moves as a set file, unbound.
    let mut tx = lib.apply(&plan, &all_of(&plan)).unwrap();
    for path in [&m3u, &one, &two] {
        assert!(!path.exists());
        assert!(folder.join(path.file_name().unwrap()).exists());
    }
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&folder.join("Saga (Disc 1).chd"))
            .unwrap()
            .is_some()
    );
    assert_eq!(lib.undo(&mut tx), RollbackResult::FullyRolledBack);
    for path in [&m3u, &one, &two] {
        assert!(path.exists());
    }
}

#[test]
fn an_incomplete_or_unsafe_set_is_blocked_as_a_whole() {
    // A missing track.
    let mut lib = Lib::new();
    let cue = lib.game(
        "Lost.cue",
        "FILE \"Lost 1.bin\" BINARY\nFILE \"Lost 2.bin\" BINARY\n",
        "PSX",
    );
    let one = lib.file("Lost 1.bin", "one");
    let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one]);
    let rows = rows_of(&plan);
    assert_eq!(rows["Lost.cue"].0, OrganisationStatus::Blocked);
    assert!(
        rows["Lost.cue"]
            .1
            .contains("Lost 2.bin, named by Lost.cue, is missing"),
        "{rows:?}"
    );
    assert_eq!(rows["Lost 1.bin"].0, OrganisationStatus::Blocked);

    // A track that is a link.
    let mut lib = Lib::new();
    let cue = lib.game(
        "Link.cue",
        "FILE \"Link 1.bin\" BINARY\nFILE \"Link 2.bin\" BINARY\n",
        "PSX",
    );
    let one = lib.file("Link 1.bin", "one");
    let real = lib.temp.path().join("elsewhere.bin");
    std::fs::write(&real, "two").unwrap();
    let two = lib.root.join("incoming").join("psx").join("Link 2.bin");
    std::os::unix::fs::symlink(&real, &two).unwrap();
    let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
    let rows = rows_of(&plan);
    assert!(
        rows.values()
            .all(|(status, _)| *status == OrganisationStatus::Blocked),
        "{rows:?}"
    );
    assert!(rows["Link.cue"].1.contains("shortcut (symlink)"));

    // A reference outside the sheet's folder.
    let mut lib = Lib::new();
    let cue = lib.game("Up.cue", "FILE \"../Up.bin\" BINARY\n", "PSX");
    std::fs::write(lib.root.join("Up.bin"), "up").unwrap();
    let plan = lib.preview(
        OrganisationMode::MoveRealFile,
        &[&cue, &lib.root.join("Up.bin")],
    );
    let rows = rows_of(&plan);
    assert_eq!(rows["Up.cue"].0, OrganisationStatus::Blocked);
    assert!(
        rows["Up.cue"].1.contains("not a plain file name"),
        "{rows:?}"
    );
}

#[test]
fn a_sheet_is_never_moved_without_its_files() {
    let mut lib = Lib::new();
    let (cue, _one, _two) = cue_set(&mut lib);
    let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue]);
    let rows = rows_of(&plan);
    assert_eq!(rows["Game.cue"].0, OrganisationStatus::Blocked);
    assert!(
        rows["Game.cue"].1.contains("not in this preview"),
        "{rows:?}"
    );
    assert!(plan.suggested().next().is_none());
}

#[test]
fn files_of_one_set_on_different_systems_block_the_set() {
    let mut lib = Lib::new();
    let cue = lib.game(
        "Mixed.cue",
        "FILE \"Mixed 1.bin\" BINARY\nFILE \"Mixed 2.bin\" BINARY\n",
        "PSX",
    );
    let one = lib.game("Mixed 1.bin", "one", "Saturn");
    let two = lib.file("Mixed 2.bin", "two");
    let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
    for (name, (status, reason)) in rows_of(&plan) {
        assert_eq!(status, OrganisationStatus::Blocked, "{name}");
        assert!(
            reason.contains("resolve to different systems"),
            "{name}: {reason}"
        );
    }
}

#[test]
fn a_sheet_outside_the_set_still_blocks_and_the_set_stays_together() {
    let mut lib = Lib::new();
    let (cue, one, two) = cue_set(&mut lib);
    // Not part of the preview, but it names a track of the set.
    lib.file("Other.m3u", "Game (Track 1).bin\n");
    let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
    let rows = rows_of(&plan);
    assert!(
        rows["Game (Track 1).bin"].1.contains("Other.m3u"),
        "{rows:?}"
    );
    assert!(
        rows.values()
            .all(|(status, _)| *status == OrganisationStatus::Blocked)
    );
    assert!(
        rows["Game.cue"]
            .1
            .contains("moves only with its whole disc set")
    );
}

#[test]
fn rename_mode_still_refuses_files_a_sheet_names_with_a_clear_reason() {
    let mut lib = Lib::new();
    let (cue, one, two) = cue_set(&mut lib);
    // Tracks that are library games themselves (so only the guard refuses).
    for track in [&one, &two] {
        let id = lib.row(track);
        lib.db.set_manual_platform(id, "PSX").unwrap();
    }
    let plan = lib.preview(OrganisationMode::RenameInPlace, &[&cue, &one, &two]);
    let rows = rows_of(&plan);
    for track in ["Game (Track 1).bin", "Game_2.bin"] {
        assert_eq!(rows[track].0, OrganisationStatus::Blocked);
        assert!(
            rows[track].1.contains("Game.cue names this file, so renaming it would stop the game launching. Disc-set files are not renamed in bulk; use Move"),
            "{rows:?}"
        );
    }
    assert!(plan.entries.iter().all(|entry| entry.disc_set.is_none()));
}

#[test]
fn part_of_a_set_is_refused_before_any_change_and_the_whole_set_moves_and_goes_back() {
    let mut lib = Lib::new();
    let (cue, one, two) = cue_set(&mut lib);
    let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
    let mut part = all_of(&plan);
    part.remove(two.to_string_lossy().as_ref());
    let refused = lib.apply(&plan, &part).unwrap_err();
    assert_eq!(
        refused,
        "Move the whole disc set together: Game.cue and its 2 files must all be selected. Nothing was changed."
    );
    assert!(cue.exists() && one.exists() && two.exists());
    assert!(!lib.journal().exists() || std::fs::read_dir(lib.journal()).unwrap().next().is_none());

    let mut tx = lib.apply(&plan, &all_of(&plan)).unwrap();
    assert_eq!(tx.state, TransactionState::Applied);
    let folder = plan.entries[0].destination_path.parent().unwrap();
    for path in [&cue, &one, &two] {
        assert!(!path.exists());
        assert!(folder.join(path.file_name().unwrap()).exists());
    }
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&folder.join("Game.cue"))
            .unwrap()
            .is_some()
    );
    assert_eq!(lib.undo(&mut tx), RollbackResult::FullyRolledBack);
    for path in [&cue, &one, &two] {
        assert!(path.exists());
    }
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&cue)
            .unwrap()
            .is_some()
    );
}

/// Whole-disc checks follow a complete set move (owner decision 2026-11-23).
mod carried_disc_checks {
    use super::*;

    fn disc_check(lib: &Lib, files: &[&Path]) -> i64 {
        let members: Vec<FileFingerprint> = files
            .iter()
            .map(|path| FileFingerprint::observe(path).unwrap())
            .collect();
        let facts = serde_json::json!({
            "audit_file": members[0],
            "disc_members": members,
            "untouched": "every other fact stays as it was",
        });
        let c = lib.sql();
        c.execute(
            "INSERT INTO library_dat_identities(archive_id,dat_source_id,verification_state,completeness,audited_at,facts_json,created_at,updated_at) \
             VALUES(?1,'dat-fixture-'||(SELECT COUNT(*) FROM library_dat_identities),'verified_single_match','exhaustive','t',?2,'t','t')",
            params![lib.id(files[0]), serde_json::to_vec(&facts).unwrap()],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    fn stored(lib: &Lib, row: i64) -> Vec<u8> {
        lib.sql()
            .query_row(
                "SELECT facts_json FROM library_dat_identities WHERE id=?1",
                [row],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn members(lib: &Lib, row: i64) -> (Vec<FileFingerprint>, FileFingerprint) {
        let facts: serde_json::Value = serde_json::from_slice(&stored(lib, row)).unwrap();
        assert_eq!(facts["untouched"], "every other fact stays as it was");
        (
            serde_json::from_value(facts["disc_members"].clone()).unwrap(),
            serde_json::from_value(facts["audit_file"].clone()).unwrap(),
        )
    }

    fn moved_to(plan: &OrganisationPlan, source: &Path) -> PathBuf {
        plan.entries
            .iter()
            .find(|entry| entry.source_path == source)
            .unwrap()
            .destination_path
            .clone()
    }

    fn rewrite_keeping_size_and_time(path: &Path, text: &str) {
        let meta = std::fs::metadata(path).unwrap();
        std::fs::write(path, text).unwrap();
        let times = std::fs::FileTimes::new()
            .set_modified(meta.modified().unwrap())
            .set_accessed(meta.accessed().unwrap());
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(times)
            .unwrap();
    }

    #[test]
    fn a_current_disc_check_moves_with_its_set_on_apply_and_back_on_undo() {
        let mut lib = Lib::new();
        let (cue, one, two) = cue_set(&mut lib);
        let row = disc_check(&lib, &[&cue, &one, &two]);
        let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
        let mut tx = lib.apply(&plan, &all_of(&plan)).unwrap();
        let (moved, audit) = members(&lib, row);
        let expected: Vec<PathBuf> = [&cue, &one, &two]
            .iter()
            .map(|path| moved_to(&plan, path))
            .collect();
        assert_eq!(
            moved.iter().map(|m| m.path.clone()).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(audit.path, expected[0]);
        assert!(
            moved
                .iter()
                .all(|m| DiscFileState::of(m) == DiscFileState::Unchanged)
        );

        assert_eq!(lib.undo(&mut tx), RollbackResult::FullyRolledBack);
        let (back, audit) = members(&lib, row);
        assert_eq!(
            back.iter().map(|m| m.path.clone()).collect::<Vec<_>>(),
            vec![cue.clone(), one.clone(), two.clone()]
        );
        assert_eq!(audit.path, cue);
        assert!(
            back.iter()
                .all(|m| DiscFileState::of(m) == DiscFileState::Unchanged)
        );
    }

    #[test]
    fn a_member_changed_after_the_preview_carries_nothing() {
        let mut lib = Lib::new();
        let (cue, one, two) = cue_set(&mut lib);
        let row = disc_check(&lib, &[&cue, &one, &two]);
        let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
        let before = stored(&lib, row);
        // Same size and modification time, other content.
        rewrite_keeping_size_and_time(&two, "TRACK TWO");
        let _ = lib.apply(&plan, &all_of(&plan));
        assert_eq!(stored(&lib, row), before);
    }

    #[test]
    fn a_member_touched_after_the_preview_moves_but_its_check_stays_stale() {
        let mut lib = Lib::new();
        let (cue, one, two) = cue_set(&mut lib);
        let row = disc_check(&lib, &[&cue, &one, &two]);
        let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
        let before = stored(&lib, row);
        std::thread::sleep(std::time::Duration::from_millis(20));
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&one, std::fs::Permissions::from_mode(0o600)).unwrap();
        lib.apply(&plan, &all_of(&plan)).unwrap();
        assert!(moved_to(&plan, &one).exists(), "the set moved");
        assert_eq!(stored(&lib, row), before, "the check was not current");
    }

    #[test]
    fn a_stale_disc_check_at_the_preview_is_not_carried() {
        let mut lib = Lib::new();
        let (cue, one, two) = cue_set(&mut lib);
        let row = disc_check(&lib, &[&cue, &one, &two]);
        let mut facts: serde_json::Value = serde_json::from_slice(&stored(&lib, row)).unwrap();
        facts["disc_members"][2]["object"]["changed_seconds"] = serde_json::json!(1);
        lib.sql()
            .execute(
                "UPDATE library_dat_identities SET facts_json=?1 WHERE id=?2",
                params![serde_json::to_vec(&facts).unwrap(), row],
            )
            .unwrap();
        let before = stored(&lib, row);
        let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
        let mut tx = lib.apply(&plan, &all_of(&plan)).unwrap();
        assert_eq!(stored(&lib, row), before);
        assert_eq!(lib.undo(&mut tx), RollbackResult::FullyRolledBack);
        assert_eq!(stored(&lib, row), before);
    }

    #[test]
    fn a_failed_apply_carries_nothing() {
        let mut lib = Lib::new();
        let (cue, one, two) = cue_set(&mut lib);
        let row = disc_check(&lib, &[&cue, &one, &two]);
        let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
        let before = stored(&lib, row);
        lib.sql()
            .execute_batch("CREATE TRIGGER fail_move BEFORE UPDATE OF absolute_path_cached ON archives BEGIN SELECT RAISE(ABORT,'fixture database failure'); END;")
            .unwrap();
        assert!(lib.apply(&plan, &all_of(&plan)).is_err());
        assert_eq!(stored(&lib, row), before);
    }

    #[test]
    fn an_unverified_disc_check_moves_but_is_never_made_verified() {
        let mut lib = Lib::new();
        let (cue, one, two) = cue_set(&mut lib);
        let row = disc_check(&lib, &[&cue, &one, &two]);
        lib.sql()
            .execute(
                "UPDATE library_dat_identities SET verification_state='no_match' WHERE id=?1",
                [row],
            )
            .unwrap();
        let plan = lib.preview(OrganisationMode::MoveRealFile, &[&cue, &one, &two]);
        lib.apply(&plan, &all_of(&plan)).unwrap();
        assert_eq!(members(&lib, row).0[0].path, moved_to(&plan, &cue));
        let (state, count): (String, i64) = lib
            .sql()
            .query_row(
                "SELECT verification_state,(SELECT COUNT(*) FROM library_dat_identities) FROM library_dat_identities WHERE id=?1",
                [row],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((state.as_str(), count), ("no_match", 1));
    }
}

/// Build linked library: a complete disc set is linked together into one
/// folder with every name unchanged, so the sheet's references resolve
/// beside its links; only an incomplete or unsafe set is refused, and only
/// that set (owner decision 2026-11-23).
mod linked {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn linked_root(lib: &Lib) -> PathBuf {
        let root = lib.temp.path().join("linked");
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// The bulk linked preview as the page builds it (verified names for
    /// rows that have one, the set's own names for disc sets).
    fn preview(lib: &Lib, sources: &[&Path]) -> OrganisationPlan {
        let root = linked_root(lib);
        let candidates: Vec<OrganisationCandidate> = sources
            .iter()
            .map(|source| OrganisationCandidate {
                source_path: source.to_path_buf(),
                resolution: live_resolution_for(&lib.db, source, 1),
                canonical_name: Some(format!(
                    "Verified {}",
                    source.file_name().unwrap().to_string_lossy()
                )),
                content_classification: None,
                original_metadata: Default::default(),
            })
            .collect();
        let mut ignore = |_: crate::job_progress::JobProgress| {};
        let mut reporter = crate::job_progress::ProgressReporter::new(&mut ignore);
        let mut plan = build_organisation_plan_reporting(
            &OrganisationPlanRequest {
                master_root: &root,
                mode: OrganisationMode::BuildLinkedLibrary,
                content_policy: ContentSelectionPolicy::AllEntries,
                candidates: &candidates,
                generation: 1,
                unknown_folder: None,
                link_target_roots: None,
            },
            None,
            &mut reporter,
        )
        .unwrap();
        guard_bulk_rename_plan(&lib.db, &mut plan, &RetroArchSaveLocations::default());
        prepare_organisation_catalogue(&mut plan, &lib.db).unwrap();
        plan
    }

    fn apply(
        lib: &Lib,
        plan: &OrganisationPlan,
        approved: &BTreeSet<String>,
    ) -> Result<RenameTransaction, String> {
        let mut tx = build_organisation_transaction(plan, approved, 1)?;
        apply_organisation_transaction(
            &mut tx,
            approved,
            1,
            TrustedRoots::from_paths([lib.root.clone(), plan.master_root.clone()]),
            &lib.journal(),
            &AtomicBool::new(false),
            plan.mode,
            &plan.master_root,
            &lib.db,
        )
        .map(|_| tx)
        .map_err(|error| error.to_string())
    }

    fn undo(lib: &Lib, plan: &OrganisationPlan, tx: &mut RenameTransaction) -> RollbackResult {
        let path =
            crate::dat::rename_apply::journal_path(&lib.journal(), &tx.transaction_id).unwrap();
        *tx = crate::dat::rename_apply::read_journal(&path).unwrap();
        rollback_organisation_transaction(
            tx,
            &lib.journal(),
            &AtomicBool::new(false),
            &plan.master_root,
        )
        .unwrap()
        .rollback
        .result
    }

    /// Inode, size and bytes of each original: links never change them.
    fn originals(paths: &[&PathBuf]) -> Vec<(u64, Vec<u8>)> {
        paths
            .iter()
            .map(|p| {
                let m = std::fs::symlink_metadata(p).unwrap();
                assert!(m.is_file(), "{} is still a regular file", p.display());
                (m.ino(), std::fs::read(p).unwrap())
            })
            .collect()
    }

    /// Every file of the set is linked, under its own name, in one folder,
    /// and every reference the sheet makes resolves beside the link.
    fn assert_linked_together(plan: &OrganisationPlan, sheet: &Path, members: &[&PathBuf]) {
        let folder = plan
            .entries
            .iter()
            .find(|e| e.source_path == sheet)
            .unwrap()
            .destination_path
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(folder.starts_with(&plan.master_root));
        for member in members {
            let link = folder.join(member.file_name().unwrap());
            let meta = std::fs::symlink_metadata(&link).unwrap();
            assert!(meta.file_type().is_symlink(), "{}", link.display());
            assert_eq!(std::fs::read_link(&link).unwrap(), **member);
        }
        let kind = super::super::disc_sets::sheet_kind(sheet).unwrap();
        let text = std::fs::read_to_string(folder.join(sheet.file_name().unwrap())).unwrap();
        for reference in parse_references(kind, &text) {
            assert!(
                folder.join(&reference).exists(),
                "{reference} resolves beside the linked sheet"
            );
        }
    }

    #[test]
    fn a_single_disc_cue_bin_game_is_linked_whole_and_undo_leaves_the_originals() {
        let mut lib = Lib::new();
        let (cue, one, two) = cue_set(&mut lib);
        let plain = lib.game("Solo.iso", "solo", "PSX");
        let before = originals(&[&cue, &one, &two, &plain]);
        let plan = preview(&lib, &[&cue, &one, &two, &plain]);
        let rows = rows_of(&plan);
        for name in ["Game.cue", "Game (Track 1).bin", "Game_2.bin"] {
            assert_eq!(
                rows[name].0,
                OrganisationStatus::Suggested,
                "{name}: {}",
                rows[name].1
            );
        }
        for entry in plan.entries.iter().filter(|e| e.source_path != plain) {
            assert_eq!(entry.disc_set.as_deref(), Some(cue.as_path()));
            assert_eq!(
                entry.destination_path.file_name(),
                entry.source_path.file_name()
            );
        }
        // An ordinary game is still linked under its verified name.
        let solo = plan
            .entries
            .iter()
            .find(|e| e.source_path == plain)
            .unwrap();
        assert_eq!(solo.status, OrganisationStatus::Suggested);
        assert_eq!(solo.disc_set, None);
        assert!(solo.destination_path.ends_with("Verified Solo.iso"));

        let mut tx = apply(&lib, &plan, &all_of(&plan)).unwrap();
        assert_eq!(tx.state, TransactionState::Applied);
        assert_linked_together(&plan, &cue, &[&cue, &one, &two]);
        assert!(
            std::fs::symlink_metadata(&solo.destination_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(originals(&[&cue, &one, &two, &plain]), before);

        assert_eq!(undo(&lib, &plan, &mut tx), RollbackResult::FullyRolledBack);
        for entry in &plan.entries {
            assert!(std::fs::symlink_metadata(&entry.destination_path).is_err());
        }
        assert_eq!(originals(&[&cue, &one, &two, &plain]), before);
    }

    #[test]
    fn a_multi_disc_playlist_and_a_dreamcast_gdi_are_linked_whole() {
        let mut lib = Lib::new();
        let m3u = lib.file(
            "Saga.m3u",
            "#EXTM3U\nSaga (Disc 1).chd\nSaga (Disc 2).chd\n",
        );
        let d1 = lib.game("Saga (Disc 1).chd", "disc one", "PSX");
        let d2 = lib.game("Saga (Disc 2).chd", "disc two", "PSX");
        let gdi = lib.game(
            "Crazy.gdi",
            "3\n1 0 4 2352 track01.bin 0\n2 600 0 2352 \"track 02.raw\" 0\n3 45000 4 2352 track03.bin 0\n",
            "Dreamcast",
        );
        let t1 = lib.file("track01.bin", "t1");
        let t2 = lib.file("track 02.raw", "t2");
        let t3 = lib.file("track03.bin", "t3");
        let all = [&m3u, &d1, &d2, &gdi, &t1, &t2, &t3];
        let before = originals(&all);
        let plan = preview(&lib, &all.map(|p| p.as_path()));
        for entry in &plan.entries {
            assert_eq!(entry.status, OrganisationStatus::Suggested, "{entry:?}");
        }
        let mut tx = apply(&lib, &plan, &all_of(&plan)).unwrap();
        assert_linked_together(&plan, &m3u, &[&m3u, &d1, &d2]);
        assert_linked_together(&plan, &gdi, &[&gdi, &t1, &t2, &t3]);
        let dreamcast = plan.entries.iter().find(|e| e.source_path == t2).unwrap();
        assert_eq!(dreamcast.platform.as_deref(), Some("Dreamcast"));
        assert_eq!(originals(&all), before);
        assert_eq!(undo(&lib, &plan, &mut tx), RollbackResult::FullyRolledBack);
        assert_eq!(originals(&all), before);
    }

    #[test]
    fn a_missing_track_refuses_only_that_set_with_the_reason() {
        let mut lib = Lib::new();
        let cue = lib.game(
            "Broken.cue",
            "FILE \"Broken (Track 1).bin\" BINARY\nFILE \"Broken (Track 2).bin\" BINARY\n",
            "PSX",
        );
        let present = lib.file("Broken (Track 1).bin", "track one");
        let plain = lib.game("Solo.iso", "solo", "PSX");
        let plan = preview(&lib, &[&cue, &present, &plain]);
        let rows = rows_of(&plan);
        for name in ["Broken.cue", "Broken (Track 1).bin"] {
            assert_eq!(rows[name].0, OrganisationStatus::Blocked, "{name}");
            assert!(
                rows[name].1.contains("cannot be linked as a whole"),
                "{}",
                rows[name].1
            );
            assert!(
                rows[name]
                    .1
                    .contains("Broken (Track 2).bin, named by Broken.cue, is missing"),
                "{}",
                rows[name].1
            );
            assert!(rows[name].1.contains("Nothing will change for this file"));
        }
        assert_eq!(rows["Solo.iso"].0, OrganisationStatus::Suggested);
        // Selecting a blocked file anyway never puts it in the operation.
        let mut sneak = all_of(&plan);
        sneak.insert(present.to_string_lossy().into_owned());
        if let Ok(tx) = build_organisation_transaction(&plan, &sneak, 1) {
            let json = serde_json::to_string(&tx).unwrap();
            assert!(!json.contains("Broken (Track 1).bin"), "{json}");
            assert!(!json.contains("Broken.cue"), "{json}");
        }
        let tx = apply(&lib, &plan, &all_of(&plan)).unwrap();
        assert_eq!(tx.state, TransactionState::Applied);
        let folder = plan
            .entries
            .iter()
            .find(|e| e.source_path == plain)
            .unwrap()
            .destination_path
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(
            !folder.join("Broken.cue").exists() && !folder.join("Broken (Track 1).bin").exists()
        );
    }

    #[test]
    fn a_collision_or_a_partial_selection_refuses_the_whole_set() {
        let mut lib = Lib::new();
        let (cue, one, two) = cue_set(&mut lib);
        // Something else already has one track's name in the linked folder.
        let plan = preview(&lib, &[&cue, &one, &two]);
        let folder = plan.entries[0]
            .destination_path
            .parent()
            .unwrap()
            .to_path_buf();
        // A partial selection is refused before any change.
        let mut part = all_of(&plan);
        part.remove(two.to_string_lossy().as_ref());
        let refused = apply(&lib, &plan, &part).unwrap_err();
        assert!(refused.contains("whole disc set"), "{refused}");
        assert!(!folder.exists() || std::fs::read_dir(&folder).unwrap().next().is_none());

        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Game_2.bin"), "someone else's file").unwrap();
        let plan = preview(&lib, &[&cue, &one, &two]);
        let rows = rows_of(&plan);
        assert_ne!(rows["Game_2.bin"].0, OrganisationStatus::Suggested);
        for name in ["Game.cue", "Game (Track 1).bin"] {
            assert_eq!(rows[name].0, OrganisationStatus::Blocked, "{name}");
            assert!(
                rows[name].1.contains("only with its whole disc set"),
                "{}",
                rows[name].1
            );
        }
        assert!(plan.suggested().next().is_none());
        assert_eq!(
            std::fs::read(folder.join("Game_2.bin")).unwrap(),
            b"someone else's file"
        );
        assert!(!folder.join("Game.cue").exists());
    }

    #[test]
    fn reorganising_existing_links_still_refuses_disc_set_files() {
        let mut lib = Lib::new();
        let (cue, one, _two) = cue_set(&mut lib);
        let mut plan = lib.preview(OrganisationMode::OrganiseSymlinkOnly, &[&cue, &one]);
        guard_bulk_rename_plan(&lib.db, &mut plan, &RetroArchSaveLocations::default());
        assert!(plan.suggested().next().is_none());
    }
}
