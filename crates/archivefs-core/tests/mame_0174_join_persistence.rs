archivefs_core::install_test_environment!();

use std::path::PathBuf;

use archivefs_core::Database;
use archivefs_core::dat::mame_arcade_join::{
    ArcadeJoinClass, ArcadeJoinEvidence, ArcadeJoinReport, ArcadeJoinSummary,
};

#[test]
fn mame_join_persistence_is_bound_to_dat_digest() {
    let root = tempfile::tempdir().unwrap();
    let report = ArcadeJoinReport {
        dat_path: PathBuf::from("mame-0.174.dat"),
        dat_sha256: "digest-0174".into(),
        dat_version: "0.174".into(),
        dat_machine_count: 1,
        scan_root: PathBuf::from("arcade"),
        evidence: vec![ArcadeJoinEvidence {
            logical_set_name: "pacman".into(),
            dat_set_name: Some("pacman".into()),
            class: ArcadeJoinClass::ExactSetMatch,
            description: Some("Pac-Man".into()),
            manufacturer: None,
            year: Some("1980".into()),
            clone_of: None,
            rom_of: None,
            parent_description: None,
            runnable: Some("yes".into()),
            mechanical: false,
            is_bios: false,
            is_device: false,
            disk_declarations: Some(Vec::new()),
            expected_member_count: 0,
            members: Vec::new(),
            dependencies: Vec::new(),
            launchable_normal_game: true,
            dat_version: "0.174".into(),
            dat_sha256: "digest-0174".into(),
            dat_path: "mame-0.174.dat".into(),
            audited_at: "test".into(),
        }],
        summary: ArcadeJoinSummary::default(),
        layout_estimate: "split".into(),
    };
    let mut db = Database::open_or_create(root.path().join("library.sqlite3")).unwrap();
    assert_eq!(db.persist_mame_arcade_join(&report).unwrap(), 1);
    let source_id = "mame-arcade:0.174:digest-0174";
    let stored = db.set_audit_results_for_source(source_id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].dat_revision.as_deref(), Some("digest-0174"));
    assert_eq!(db.mame_arcade_join_for_dat("digest-0174").unwrap().len(), 1);
    assert!(
        db.mame_arcade_join_for_dat("different-dat")
            .unwrap()
            .is_empty()
    );
    assert!(
        db.set_audit_results_for_source("mame-arcade:0.175:different-dat")
            .unwrap()
            .is_empty()
    );
}

fn evidence(name: &str, class: ArcadeJoinClass) -> ArcadeJoinEvidence {
    let found = class != ArcadeJoinClass::NotFound;
    ArcadeJoinEvidence {
        logical_set_name: name.into(),
        dat_set_name: found.then(|| name.into()),
        class,
        description: None,
        manufacturer: None,
        year: None,
        clone_of: None,
        rom_of: None,
        parent_description: None,
        runnable: None,
        mechanical: false,
        is_bios: false,
        is_device: false,
        disk_declarations: Some(Vec::new()),
        expected_member_count: 0,
        members: Vec::new(),
        dependencies: Vec::new(),
        launchable_normal_game: found,
        dat_version: "0.174".into(),
        dat_sha256: "digest-0174".into(),
        dat_path: "mame-0.174.dat".into(),
        audited_at: "test".into(),
    }
}

/// Run 210: `arcade/covers`, `arcade/manuals` and `arcade/screenshots` are
/// artwork folders the MAME 0.174 DAT does not describe (class NotFound).
/// They were stored as complete, satisfied DAT sets. A folder the DAT does
/// not describe must never be recorded as a DAT set, and rows an earlier
/// scan wrote that way are removed by the next join.
#[test]
fn artwork_folders_absent_from_the_dat_are_never_recorded_as_sets() {
    let root = tempfile::tempdir().unwrap();
    let report = ArcadeJoinReport {
        dat_path: PathBuf::from("mame-0.174.dat"),
        dat_sha256: "digest-0174".into(),
        dat_version: "0.174".into(),
        dat_machine_count: 1,
        scan_root: PathBuf::from("/games/arcade"),
        evidence: vec![
            evidence("covers", ArcadeJoinClass::NotFound),
            evidence("manuals", ArcadeJoinClass::NotFound),
            evidence("pacman", ArcadeJoinClass::ExactSetMatch),
            evidence("screenshots", ArcadeJoinClass::NotFound),
        ],
        summary: ArcadeJoinSummary::default(),
        layout_estimate: "split".into(),
    };
    let db_path = root.path().join("library.sqlite3");
    let source_id = "mame-arcade:0.174:digest-0174";
    {
        let _db = Database::open_or_create(&db_path).unwrap();
    }
    // The six rows run 210 left behind, written by the earlier behaviour.
    let connection = rusqlite::Connection::open(&db_path).unwrap();
    for name in ["covers", "manuals", "screenshots"] {
        connection
            .execute(
                "INSERT INTO dat_set_audit_results (archive_id, archive_path, source_id, game_name, platform, set_state_json, dependency_state_json, ecosystem, dat_revision, audited_at, stale, exhaustive) VALUES (NULL, ?1, ?2, ?3, 'arcade', '\"complete\"', '\"satisfied\"', '\"m_a_m_e_arcade\"', 'digest-0174', 'run-210', 0, 1)",
                rusqlite::params![format!("/games/arcade/{name}").into_bytes(), source_id, name],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO dat_expected_entries (dat_source_id, canonical_identity, display_name, source_revision, ecosystem, metadata_json, created_at, updated_at) VALUES (?1, ?2, ?2, 'digest-0174', 'mame', '{\"class\":\"NotFound\"}', 'run-210', 'run-210')",
                rusqlite::params![source_id, name],
            )
            .unwrap();
    }
    drop(connection);

    let mut db = Database::open_or_create(&db_path).unwrap();
    assert_eq!(db.persist_mame_arcade_join(&report).unwrap(), 1);
    let stored = db.set_audit_results_for_source(source_id).unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|row| row.game_name.as_str())
            .collect::<Vec<_>>(),
        ["pacman"]
    );
    drop(db);
    let connection = rusqlite::Connection::open(&db_path).unwrap();
    let expected: Vec<String> = connection
        .prepare("SELECT canonical_identity FROM dat_expected_entries WHERE dat_source_id = ?1 ORDER BY 1")
        .unwrap()
        .query_map([source_id], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(expected, ["pacman"]);
}
