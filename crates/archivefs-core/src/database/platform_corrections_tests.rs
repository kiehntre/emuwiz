//! Platform Corrections preview / apply / Undo (owner decisions 2026-11-26/27).
//! Synthetic fixtures only: a temporary library with real files on disk.

use super::seams::{self, Point};
use super::*;
use crate::dat::audit::{AuditEntry, AuditReport, AuditSummary, AuditVerdict};
use crate::dat::sources::audit_run::DatAuditOutcome;

const NES_HEADER: &str = "Nintendo - Nintendo Entertainment System";

struct Library {
    dir: tempfile::TempDir,
    db: Database,
    dat: PathBuf,
}

impl Library {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        fs::create_dir_all(&roms).unwrap();
        let db = Database::open_or_create(dir.path().join("library.sqlite3")).unwrap();
        db.connection
            .execute(
                "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at,last_scan_status) \
                 VALUES(1,CAST(?1 AS BLOB),'2026-01-01','2026-01-01','success')",
                [roms.to_string_lossy().to_string()],
            )
            .unwrap();
        let dat = dir.path().join("nes.dat");
        fs::write(
            &dat,
            b"<datafile><header><name>NES</name></header></datafile>",
        )
        .unwrap();
        Self { dir, db, dat }
    }

    fn roms(&self) -> PathBuf {
        self.dir.path().join("roms")
    }

    /// A real game file, guessed onto `platform` by `source`, with a scanner
    /// cache row.
    fn game(&mut self, id: i64, platform: &str, source: &str) -> String {
        let name = format!("game{id}.nes");
        let path = self.roms().join(&name);
        fs::write(&path, format!("rom bytes {id}")).unwrap();
        let path = path.to_string_lossy().to_string();
        self.db
            .connection
            .execute(
                "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,\
                 archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) \
                 VALUES(?1,1,CAST(?2 AS BLOB),CAST(?3 AS BLOB),CAST(?2 AS BLOB),'file',?2,?2,\
                 '2026-01-01','2026-01-02','2026-01-01','2026-01-02')",
                params![id, name, path],
            )
            .unwrap();
        self.db
            .connection
            .execute(
                "INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) \
                 VALUES(?1,?2,?3,1,'2026-01-03')",
                params![id, platform, source],
            )
            .unwrap();
        self.db
            .connection
            .execute(
                "INSERT INTO scan_fingerprints(source_folder_id,relative_path,size_bytes,modified_time_ns,\
                 archive_kind,display_name,normalized_name,platform,platform_provenance,scanner_version,\
                 parser_version,cache_version,updated_at) \
                 VALUES(1,CAST(?1 AS BLOB),9,1,'file',?1,?1,?2,?3,'1','1',1,'2026-01-03')",
                params![name, platform, source],
            )
            .unwrap();
        path
    }

    fn audit(&self, entries: Vec<AuditEntry>) -> DatAuditOutcome {
        let bytes = fs::read(&self.dat).unwrap();
        DatAuditOutcome {
            population: Default::default(),
            source_id: "nes".to_string(),
            source_display_name: format!("No-Intro: {NES_HEADER}"),
            dat_path: self.dat.to_string_lossy().to_string(),
            scan_root: self.roms().to_string_lossy().to_string(),
            catalogue_names: vec![NES_HEADER.to_string()],
            catalogue_entries: 1,
            catalogue_roms: 1,
            catalogue_version: Some("20260825-070327".to_string()),
            catalogue_sha256: Some(hex(&Sha256::digest(&bytes))),
            catalogue_inventory: None,
            catalogue_author: None,
            catalogue_homepage: None,
            catalogue_ecosystem: Some(crate::dat::model::DatEcosystem::NoIntro),
            catalogue_variant: None,
            unreadable_catalogues: Vec::new(),
            report: AuditReport {
                summary: AuditSummary::default(),
                entries,
            },
            evidence_sources: Vec::new(),
            archives: Vec::new(),
            sets: Vec::new(),
            unhashed: Vec::new(),
            files_scanned: 1,
            bytes_hashed: 9,
            archive_bytes_hashed: 0,
            truncated: false,
            policy: None,
            content: Default::default(),
            platform: Some("NES".to_string()),
            cache: Default::default(),
            known_hashes: Default::default(),
            loose_discs: Default::default(),
        }
    }

    fn preview(&self, entries: Vec<AuditEntry>) -> PlatformCorrectionPreview {
        self.db
            .preview_platform_corrections(&self.audit(entries), 1)
            .unwrap()
    }

    /// Everything a correction may touch, as stored.
    fn state(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (table, order) in [
            ("platform_assignments", "id"),
            ("library_dat_identities", "id"),
            ("scan_fingerprints", "relative_path"),
            ("dat_recheck_attempts", "archive_id"),
        ] {
            let mut stmt = self
                .db
                .connection
                .prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))
                .unwrap();
            let width = stmt.column_count();
            let rows = stmt
                .query_map([], |row| {
                    let mut line = format!("{table}:");
                    for i in 0..width {
                        line.push_str(&format!("{:?}|", row.get::<_, SqlValue>(i)?));
                    }
                    Ok(line)
                })
                .unwrap();
            out.extend(rows.map(|r| r.unwrap()));
        }
        out
    }

    fn current(&self, id: i64) -> Option<(String, String)> {
        current_assignment(&self.db.connection, id)
            .unwrap()
            .map(|a| (a.platform, a.source))
    }

    fn receipts(&self) -> Vec<PathBuf> {
        let prefix = receipt_prefix(&self.db.path);
        let mut found: Vec<PathBuf> = fs::read_dir(self.dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&prefix)
            })
            .collect();
        found.sort();
        found
    }
}

fn exact(path: &str, algorithm: &'static str) -> AuditEntry {
    AuditEntry {
        local_path: path.to_string(),
        local_filename: path.rsplit('/').next().unwrap().to_string(),
        verdict: AuditVerdict::Exact {
            game_name: format!("Listed {}", path.rsplit('/').next().unwrap()),
            rom_name: "Listed.nes".into(),
            algorithm,
        },
    }
}

fn verdict(path: &str, verdict: AuditVerdict) -> AuditEntry {
    AuditEntry {
        local_path: path.to_string(),
        local_filename: path.rsplit('/').next().unwrap().to_string(),
        verdict,
    }
}

fn group_of(preview: &PlatformCorrectionPreview, id: i64) -> Option<PlatformCorrectionGroup> {
    preview
        .candidates
        .iter()
        .find(|c| c.archive_id == id)
        .map(|c| c.group)
}

fn is_nes(platform: &str) -> bool {
    crate::canonical_platform_for_alias(platform) == crate::canonical_platform_for_alias("NES")
}

/// NES games in the SNES folder: Ready, preselected, with their evidence.
#[test]
fn the_preview_sorts_corrections_into_ready_review_and_ambiguous() {
    let mut lib = Library::new();
    let folder = lib.game(1, "SNES", "folder_alias");
    let extension = lib.game(2, "SNES", "registry_detector");
    let manual = lib.game(3, "SNES", "manual");
    let header = lib.game(4, "SNES", "header_identity");
    let verified = lib.game(5, "SNES", "verified_dat");
    let unknown = lib.game(6, "SNES", "someone_else");
    let md5 = lib.game(7, "SNES", "folder_alias");
    let crc = lib.game(8, "SNES", "folder_alias");
    let several = lib.game(9, "SNES", "folder_alias");
    let on_nes = lib.game(10, "NES", "folder_alias");
    let preview = lib.preview(vec![
        exact(&folder, "SHA-1"),
        exact(&extension, "SHA-256/SHA-1"),
        exact(&manual, "SHA-1"),
        exact(&header, "SHA-1"),
        exact(&verified, "SHA-1"),
        exact(&unknown, "SHA-1"),
        exact(&md5, "MD5"),
        exact(&crc, "CRC32"),
        verdict(
            &several,
            AuditVerdict::ExactMultipleCandidates {
                algorithm: "SHA-1",
                count: 2,
                game_names: vec!["A".into(), "B".into()],
            },
        ),
        exact(&on_nes, "SHA-1"),
    ]);
    use PlatformCorrectionGroup::*;
    assert_eq!(group_of(&preview, 1), Some(Ready));
    assert_eq!(group_of(&preview, 2), Some(Ready));
    for id in [3, 4, 5, 6, 7] {
        assert_eq!(group_of(&preview, id), Some(NeedsReview), "{id}");
    }
    assert_eq!(group_of(&preview, 8), None, "CRC never proposes a system");
    assert_eq!(group_of(&preview, 9), Some(Ambiguous));
    assert_eq!(group_of(&preview, 10), None, "already on NES");
    assert_eq!(preview.preselected(), vec![1, 2]);
    let ready = &preview.candidates[0];
    assert_eq!(ready.current.platform, "SNES");
    assert_eq!(ready.current.source, "folder_alias");
    assert!(is_nes(&ready.proposed_platform));
    assert_eq!(ready.algorithm, "SHA-1");
    assert_eq!(ready.game_name.as_deref(), Some("Listed game1.nes"));
    assert!(
        ready.reason.contains("guessed by the scanner"),
        "{}",
        ready.reason
    );
    let manual = preview
        .candidates
        .iter()
        .find(|c| c.archive_id == 3)
        .unwrap();
    assert!(
        manual.reason.contains("you chose this system"),
        "{}",
        manual.reason
    );
    let md5 = preview
        .candidates
        .iter()
        .find(|c| c.archive_id == 7)
        .unwrap();
    assert!(md5.reason.contains("MD5 only"), "{}", md5.reason);
    // Read-only.
    assert!(lib.receipts().is_empty());
    assert_eq!(lib.current(1), Some(("SNES".into(), "folder_alias".into())));
}

/// Only Ready corrections can be applied; a chosen-by-hand system is never
/// changed by the corrections, whatever is selected.
#[test]
fn only_ready_corrections_can_be_applied() {
    let mut lib = Library::new();
    let manual = lib.game(1, "SNES", "manual");
    let preview = lib.preview(vec![exact(&manual, "SHA-1")]);
    let before = lib.state();
    let error = lib
        .db
        .apply_platform_corrections(&preview, &[1], WhenSomeChanged::RefuseAll)
        .unwrap_err();
    assert!(
        error.to_string().contains("not a ready correction"),
        "{error}"
    );
    assert_eq!(lib.state(), before);
    assert!(lib.receipts().is_empty());
}

/// Apply corrects, keeps the result, keeps the history and the receipt;
/// Undo puts every touched record back exactly. Game files are untouched.
#[test]
fn apply_then_undo_restores_everything_exactly() {
    let mut lib = Library::new();
    let one = lib.game(1, "SNES", "folder_alias");
    let two = lib.game(2, "SNES", "registry_detector");
    let files_before: Vec<Vec<u8>> = [&one, &two].iter().map(|p| fs::read(p).unwrap()).collect();
    let before = lib.state();
    let preview = lib.preview(vec![exact(&one, "SHA-1"), exact(&two, "SHA-1")]);
    let applied = lib
        .db
        .apply_platform_corrections(&preview, &preview.preselected(), WhenSomeChanged::RefuseAll)
        .unwrap();
    assert_eq!(applied.corrected, vec![1, 2]);
    assert!(applied.refused.is_empty());
    let receipt = applied.receipt_path.clone().unwrap();
    for id in [1, 2] {
        let (platform, source) = lib.current(id).unwrap();
        assert!(is_nes(&platform), "{platform}");
        assert_eq!(source, VERIFIED_DAT_PLATFORM_SOURCE);
        assert!(
            lib.db
                .library_dat_identity_for_item(id, "nes")
                .unwrap()
                .is_some()
        );
    }
    let cached: String = lib
        .db
        .connection
        .query_row(
            "SELECT platform FROM scan_fingerprints WHERE relative_path = CAST('game1.nes' AS BLOB)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(is_nes(&cached), "the scanner cache follows the correction");
    // A rescan's guess never replaces the corrected system.
    lib.db
        .assign_platform(1, Some("SNES"), "folder_alias")
        .unwrap();
    assert_eq!(lib.current(1).unwrap().1, VERIFIED_DAT_PLATFORM_SOURCE);
    // The receipt is listed as undoable.
    let listed = lib.db.platform_correction_receipts().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].undoable.len(), 2);
    assert_eq!(
        listed[0].undoable[0].previous_source.as_deref(),
        Some("folder_alias")
    );

    let undone = lib
        .db
        .undo_platform_corrections(&receipt, None, WhenSomeChanged::RefuseAll)
        .unwrap();
    assert_eq!(undone.undone, vec![1, 2]);
    assert!(undone.refused.is_empty());
    assert_eq!(
        lib.state(),
        before,
        "every touched record is back as it was"
    );
    let files_after: Vec<Vec<u8>> = [&one, &two].iter().map(|p| fs::read(p).unwrap()).collect();
    assert_eq!(files_after, files_before, "game files are never touched");
    assert_eq!(fs::read_dir(lib.roms()).unwrap().count(), 2);
    let listed = lib.db.platform_correction_receipts().unwrap();
    assert!(listed[0].undoable.is_empty());
    assert_eq!(listed[0].undone, vec![1, 2]);
    // A second Undo changes nothing.
    let again = lib
        .db
        .undo_platform_corrections(&receipt, None, WhenSomeChanged::RefuseAll)
        .unwrap();
    assert!(again.undone.is_empty());
    assert_eq!(lib.state(), before);
}

/// A result this catalogue already held is restored exactly by Undo.
#[test]
fn undo_restores_the_previous_result_exactly() {
    let mut lib = Library::new();
    let game = lib.game(1, "NES", "folder_alias");
    // An earlier NES verification with an older catalogue revision.
    let mut older = lib.audit(vec![exact(&game, "SHA-1")]);
    older.catalogue_version = Some("20250101-000000".into());
    lib.db
        .persist_library_dat_identities_from_audit(&older)
        .unwrap();
    assert!(
        lib.db
            .library_dat_identity_for_item(1, "nes")
            .unwrap()
            .is_some()
    );
    // A later scan guessed SNES from the folder.
    lib.db
        .connection
        .execute_batch(
            "UPDATE platform_assignments SET is_current = 0 WHERE archive_id = 1; \
             INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) \
             VALUES(1,'SNES','folder_alias',1,'2026-01-04'); \
             UPDATE scan_fingerprints SET platform = 'SNES' WHERE source_folder_id = 1;",
        )
        .unwrap();
    // ...and a re-check note for this catalogue.
    lib.db
        .connection
        .execute(
            "INSERT INTO dat_recheck_attempts(archive_id,dat_source_id,archive_path,attempted_at,reason,catalogue_sha256) \
             VALUES(1,'nes',CAST('x' AS BLOB),'2026-01-05','unreadable',NULL)",
            [],
        )
        .unwrap();
    let before = lib.state();
    let preview = lib.preview(vec![exact(&game, "SHA-1")]);
    assert_eq!(preview.preselected(), vec![1]);
    assert!(preview.candidates[0].existing_result.is_some());
    let applied = lib
        .db
        .apply_platform_corrections(&preview, &[1], WhenSomeChanged::RefuseAll)
        .unwrap();
    assert_ne!(lib.state(), before);
    lib.db
        .undo_platform_corrections(
            &applied.receipt_path.unwrap(),
            None,
            WhenSomeChanged::RefuseAll,
        )
        .unwrap();
    assert_eq!(lib.state(), before);
}

/// Revalidated just before applying: a game whose system, file or result
/// changed since the preview is not corrected; by default nothing is.
#[test]
fn a_game_changed_since_the_preview_is_not_corrected() {
    let mut lib = Library::new();
    let moved = lib.game(1, "SNES", "folder_alias");
    let rewritten = lib.game(2, "SNES", "folder_alias");
    let fine = lib.game(3, "SNES", "folder_alias");
    let preview = lib.preview(vec![
        exact(&moved, "SHA-1"),
        exact(&rewritten, "SHA-1"),
        exact(&fine, "SHA-1"),
    ]);
    assert_eq!(preview.preselected(), vec![1, 2, 3]);
    lib.db.set_manual_platform(1, "SNES").unwrap();
    fs::write(&rewritten, b"different bytes now").unwrap();
    let before = lib.state();
    let refused = lib
        .db
        .apply_platform_corrections(&preview, &[1, 2, 3], WhenSomeChanged::RefuseAll)
        .unwrap();
    assert!(refused.corrected.is_empty());
    assert_eq!(refused.refused.len(), 2);
    assert!(
        refused.refused[0].reason.contains("system changed"),
        "{:?}",
        refused.refused
    );
    assert!(
        refused.refused[1].reason.contains("file changed"),
        "{:?}",
        refused.refused
    );
    assert_eq!(lib.state(), before);
    assert!(lib.receipts().is_empty());
    let rest = lib
        .db
        .apply_platform_corrections(&preview, &[1, 2, 3], WhenSomeChanged::ProceedWithTheRest)
        .unwrap();
    assert_eq!(rest.corrected, vec![3]);
    assert_eq!(
        lib.current(1),
        Some(("SNES".into(), "manual".into())),
        "a chosen system is never overridden"
    );
    assert_eq!(lib.current(2), Some(("SNES".into(), "folder_alias".into())));
}

/// The catalogue evidence is re-checked: a catalogue whose bytes changed
/// since the preview corrects nothing.
#[test]
fn a_changed_catalogue_corrects_nothing() {
    let mut lib = Library::new();
    let game = lib.game(1, "SNES", "folder_alias");
    let preview = lib.preview(vec![exact(&game, "SHA-1")]);
    fs::write(&lib.dat, b"<datafile>edited</datafile>").unwrap();
    let before = lib.state();
    let error = lib
        .db
        .apply_platform_corrections(&preview, &[1], WhenSomeChanged::ProceedWithTheRest)
        .unwrap_err();
    assert!(
        error.to_string().contains("catalogue file changed"),
        "{error}"
    );
    assert_eq!(lib.state(), before);
    assert!(lib.receipts().is_empty());
}

/// Undo never overwrites a later change: a game re-assigned by hand after
/// the correction is refused; the default refuses the whole batch, "undo
/// the rest" undoes the others, and one game can be undone alone.
#[test]
fn undo_refuses_a_game_changed_after_the_correction() {
    let mut lib = Library::new();
    let paths: Vec<String> = (1..=3)
        .map(|id| lib.game(id, "SNES", "folder_alias"))
        .collect();
    let preview = lib.preview(paths.iter().map(|p| exact(p, "SHA-1")).collect());
    let receipt = lib
        .db
        .apply_platform_corrections(&preview, &[1, 2, 3], WhenSomeChanged::RefuseAll)
        .unwrap()
        .receipt_path
        .unwrap();
    lib.db.set_manual_platform(1, "Famicom").unwrap();
    let after_manual = lib.state();
    let refused = lib
        .db
        .undo_platform_corrections(&receipt, None, WhenSomeChanged::RefuseAll)
        .unwrap();
    assert!(refused.undone.is_empty());
    assert_eq!(refused.refused.len(), 1);
    assert!(
        refused.refused[0]
            .reason
            .contains("changed after the correction"),
        "{:?}",
        refused.refused
    );
    assert_eq!(lib.state(), after_manual, "nothing was undone");
    let one = lib
        .db
        .undo_platform_corrections(&receipt, Some(&[2]), WhenSomeChanged::RefuseAll)
        .unwrap();
    assert_eq!(one.undone, vec![2]);
    assert_eq!(lib.current(2), Some(("SNES".into(), "folder_alias".into())));
    assert!(is_nes(&lib.current(3).unwrap().0));
    let rest = lib
        .db
        .undo_platform_corrections(&receipt, None, WhenSomeChanged::ProceedWithTheRest)
        .unwrap();
    assert_eq!(rest.undone, vec![3]);
    assert_eq!(
        lib.current(1).unwrap().1,
        "manual",
        "the later choice stays"
    );
    // A re-check of the same catalogue after the correction changes the
    // result: that game's Undo is refused too.
    let listed = lib.db.platform_correction_receipts().unwrap();
    assert_eq!(listed[0].changed.len(), 1);
    assert_eq!(listed[0].undone, vec![2, 3]);
}

/// A full disk (or any receipt write failure) corrects nothing and leaves
/// no receipt.
#[test]
fn a_failed_receipt_write_corrects_nothing() {
    let mut lib = Library::new();
    let game = lib.game(1, "SNES", "folder_alias");
    let preview = lib.preview(vec![exact(&game, "SHA-1")]);
    let before = lib.state();
    seams::set_write_error(Some(libc::ENOSPC));
    let error = lib
        .db
        .apply_platform_corrections(&preview, &[1], WhenSomeChanged::RefuseAll)
        .unwrap_err();
    seams::set_write_error(None);
    assert!(
        error.to_string().contains("nothing was corrected"),
        "{error}"
    );
    assert_eq!(lib.state(), before);
    assert!(lib.receipts().is_empty());
    assert!(lib.db.connection.is_autocommit());
}

/// Interrupted after the receipt was written but before the commit: nothing
/// was applied, and the pending receipt settles as void (nothing offered).
#[test]
fn an_interruption_before_the_commit_applies_nothing() {
    let mut lib = Library::new();
    let game = lib.game(1, "SNES", "folder_alias");
    let preview = lib.preview(vec![exact(&game, "SHA-1")]);
    let before = lib.state();
    seams::set_stop(Some(Point::AfterReceiptBeforeCommit), false);
    let _ = lib
        .db
        .apply_platform_corrections(&preview, &[1], WhenSomeChanged::RefuseAll)
        .unwrap_err();
    seams::set_stop(None, false);
    assert_eq!(lib.state(), before);
    assert_eq!(lib.receipts().len(), 1, "the pending receipt is on disk");
    assert!(lib.db.platform_correction_receipts().unwrap().is_empty());
    assert!(read_receipt(&lib.receipts()[0]).unwrap().state == ReceiptState::Void);
}

/// Interrupted after the commit but before the receipt was marked: the
/// correction is applied and its Undo is still offered.
#[test]
fn an_interruption_after_the_commit_keeps_the_undo() {
    let mut lib = Library::new();
    let game = lib.game(1, "SNES", "folder_alias");
    let before = lib.state();
    let preview = lib.preview(vec![exact(&game, "SHA-1")]);
    seams::set_stop(Some(Point::AfterCommitBeforeMark), false);
    let _ = lib
        .db
        .apply_platform_corrections(&preview, &[1], WhenSomeChanged::RefuseAll)
        .unwrap_err();
    seams::set_stop(None, false);
    assert!(is_nes(&lib.current(1).unwrap().0));
    let listed = lib.db.platform_correction_receipts().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].undoable.len(), 1);
    lib.db
        .undo_platform_corrections(&listed[0].receipt_path, None, WhenSomeChanged::RefuseAll)
        .unwrap();
    assert_eq!(lib.state(), before);
}

/// Interrupted during Undo: nothing is undone, and Undo is still offered.
#[test]
fn an_interrupted_undo_undoes_nothing() {
    let mut lib = Library::new();
    let game = lib.game(1, "SNES", "folder_alias");
    let preview = lib.preview(vec![exact(&game, "SHA-1")]);
    let receipt = lib
        .db
        .apply_platform_corrections(&preview, &[1], WhenSomeChanged::RefuseAll)
        .unwrap()
        .receipt_path
        .unwrap();
    let applied = lib.state();
    seams::set_stop(Some(Point::UndoBeforeCommit), false);
    let _ = lib
        .db
        .undo_platform_corrections(&receipt, None, WhenSomeChanged::RefuseAll)
        .unwrap_err();
    seams::set_stop(None, false);
    assert_eq!(lib.state(), applied);
    assert_eq!(
        lib.db.platform_correction_receipts().unwrap()[0]
            .undoable
            .len(),
        1
    );
}

/// The preview says when a correction ends a game's Discovery Import Undo.
#[test]
fn the_preview_flags_games_whose_discovery_import_undo_would_end() {
    let mut lib = Library::new();
    let imported = lib.game(1, "SNES", "folder_alias");
    let other = lib.game(2, "SNES", "folder_alias");
    lib.db
        .connection
        .execute_batch(
            "INSERT INTO discovery_import_operations(operation_id,state,created_at,receipt_json) \
             VALUES('op','committed','2026-01-01',CAST('{}' AS BLOB)); \
             INSERT INTO discovery_import_records(operation_id,archive_id,source_folder_id,relative_path,\
             absolute_path,snapshot_json,touched) VALUES('op',1,1,CAST('game1.nes' AS BLOB),\
             CAST('x' AS BLOB),CAST('{}' AS BLOB),0);",
        )
        .unwrap();
    let preview = lib.preview(vec![exact(&imported, "SHA-1"), exact(&other, "SHA-1")]);
    assert!(preview.candidates[0].loses_discovery_import_undo);
    assert!(!preview.candidates[1].loses_discovery_import_undo);
}

/// Unexpected termination (a real process abort) at each point of Apply:
/// the database and the receipt agree afterwards.
#[test]
fn a_process_killed_during_apply_leaves_database_and_receipt_consistent() {
    for (point, applied) in [("before-commit", false), ("after-commit", true)] {
        let mut lib = Library::new();
        let game = lib.game(1, "SNES", "folder_alias");
        let before = lib.state();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "database::platform_corrections::tests::killed_apply_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("EMUWIZ_PLATFORM_CORRECTION_CHILD", point)
            .env("EMUWIZ_PLATFORM_CORRECTION_DIR", lib.dir.path())
            .env("EMUWIZ_PLATFORM_CORRECTION_GAME", &game)
            .status()
            .unwrap();
        assert!(
            !status.success(),
            "{point}: the child must die at the point"
        );
        // Reopen as after a restart.
        let db = Database::open_or_create(lib.dir.path().join("library.sqlite3")).unwrap();
        lib.db = db;
        let listed = lib.db.platform_correction_receipts().unwrap();
        if applied {
            assert!(is_nes(&lib.current(1).unwrap().0), "{point}");
            assert_eq!(
                listed.len(),
                1,
                "{point}: an applied correction always has its Undo"
            );
            lib.db
                .undo_platform_corrections(
                    &listed[0].receipt_path,
                    None,
                    WhenSomeChanged::RefuseAll,
                )
                .unwrap();
            assert_eq!(lib.state(), before, "{point}");
        } else {
            assert_eq!(lib.state(), before, "{point}");
            assert!(listed.is_empty(), "{point}");
        }
    }
}

/// Runs only as the child of the test above.
#[test]
fn killed_apply_child() {
    let Ok(point) = std::env::var("EMUWIZ_PLATFORM_CORRECTION_CHILD") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("EMUWIZ_PLATFORM_CORRECTION_DIR").unwrap());
    let game = std::env::var("EMUWIZ_PLATFORM_CORRECTION_GAME").unwrap();
    let mut db = Database::open_or_create(dir.join("library.sqlite3")).unwrap();
    let lib = Library {
        dir: tempfile::tempdir().unwrap(),
        db: Database::open_or_create(dir.join("library.sqlite3")).unwrap(),
        dat: dir.join("nes.dat"),
    };
    let mut outcome = lib.audit(vec![exact(&game, "SHA-1")]);
    outcome.scan_root = dir.join("roms").to_string_lossy().to_string();
    let preview = db.preview_platform_corrections(&outcome, 1).unwrap();
    seams::set_stop(
        Some(if point == "before-commit" {
            Point::AfterReceiptBeforeCommit
        } else {
            Point::AfterCommitBeforeMark
        }),
        true,
    );
    let _ = db.apply_platform_corrections(&preview, &[1], WhenSomeChanged::RefuseAll);
    unreachable!("the seam aborts the process");
}

/// Emulator choices are keyed by (system, game) and resolved from the
/// current system. A correction never writes them: the old system's choice
/// stops applying while corrected, and applies again after Undo, with the
/// preferences file unchanged throughout.
#[test]
fn emulator_choices_follow_the_system_through_apply_and_undo() {
    use crate::launch::launch_choice::{
        LaunchChoiceInputs, LaunchChoiceSource, LaunchPreference, effective_choice,
        load_launch_preferences_at, set_launch_preference_at,
    };
    let mut lib = Library::new();
    let game = lib.game(1, "SNES", "folder_alias");
    let prefs = lib.dir.path().join("launch_preferences.json");
    let snes = crate::canonical_platform_for_alias("SNES").unwrap();
    set_launch_preference_at(&prefs, &LaunchPreference::game(snes, "GAME-1", "snes9x")).unwrap();
    let prefs_bytes = fs::read(&prefs).unwrap();
    let choice = |lib: &Library| {
        let loaded = load_launch_preferences_at(&prefs).unwrap();
        let platform = lib.current(1).unwrap().0;
        let id = crate::canonical_platform_for_alias(&platform).unwrap();
        effective_choice(
            id,
            Some("GAME-1"),
            LaunchChoiceInputs {
                once: None,
                preferences: Ok(&loaded),
            },
        )
        .unwrap()
        .map(|(source, option)| (source, option.to_string()))
    };
    assert_eq!(
        choice(&lib),
        Some((LaunchChoiceSource::Game, "snes9x".to_string()))
    );
    let preview = lib.preview(vec![exact(&game, "SHA-1")]);
    let receipt = lib
        .db
        .apply_platform_corrections(&preview, &[1], WhenSomeChanged::RefuseAll)
        .unwrap()
        .receipt_path
        .unwrap();
    assert_eq!(
        choice(&lib),
        None,
        "the SNES choice does not apply to the NES game"
    );
    assert_eq!(fs::read(&prefs).unwrap(), prefs_bytes);
    lib.db
        .undo_platform_corrections(&receipt, None, WhenSomeChanged::RefuseAll)
        .unwrap();
    assert_eq!(
        choice(&lib),
        Some((LaunchChoiceSource::Game, "snes9x".to_string()))
    );
    assert_eq!(fs::read(&prefs).unwrap(), prefs_bytes);
}
