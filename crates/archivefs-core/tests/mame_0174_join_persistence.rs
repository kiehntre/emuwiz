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
