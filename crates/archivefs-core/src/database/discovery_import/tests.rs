// The independent reviewer's tests further down are kept as written,
// including a few style points Clippy would rather see written differently.
#![allow(clippy::let_and_return, clippy::unnecessary_cast)]

use super::*;
use crate::ingestion::{
    bounded_import::{apply_bounded_import, begin_bounded_preview, prepare_bounded_import},
    preview::preview_media,
};
use std::sync::atomic::{AtomicBool, Ordering};

struct Fixture {
    _t: tempfile::TempDir,
    db: PathBuf,
    root: PathBuf,
    cfg: PathBuf,
}
fn snapshot(tag: u8) -> Vec<u8> {
    let mut b = vec![0; 49182];
    b[7] = 128;
    b[27] = 255;
    b[28] = 255;
    b[29] = 1;
    b[100] = tag;
    b
}
impl Fixture {
    fn new() -> Self {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("games/zxs");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("existing.z80"), snapshot(1)).unwrap();
        let db = t.path().join("library.sqlite3");
        let cfg = t.path().join("config.toml");
        let mut d = Database::open_or_create(&db).unwrap();
        crate::scan_and_persist(
            &mut d,
            &Config {
                configured_source_roots: None,
                source_folders: vec![root.clone()],
                mount_root: t.path().join("mount"),
                ratarmount_bin: String::new(),
                master_rom_root: None,
            },
            "synthetic-seed",
        )
        .unwrap();
        crate::save_source_folder_configs_to(
            &cfg,
            &[crate::SourceFolderConfig {
                path: root.clone(),
                enabled: true,
                created_at: None,
            }],
            &t.path().join("mount"),
            "",
            None,
        )
        .unwrap();
        Self {
            _t: t,
            db,
            root,
            cfg,
        }
    }
    fn db(&self) -> Database {
        Database::open_read_only(&self.db).unwrap()
    }
    fn write(&self, name: &str, tag: u8) {
        fs::write(self.root.join(name), snapshot(tag)).unwrap();
    }
    fn plan(&self) -> BoundedImportPlan {
        let d = self.db();
        let context = begin_bounded_preview(&d, &self.cfg).unwrap();
        let config = Config {
            configured_source_roots: None,
            source_folders: vec![self.root.clone()],
            mount_root: PathBuf::new(),
            ratarmount_bin: String::new(),
            master_rom_root: None,
        };
        let dat = self._t.path().join("synthetic.dat");
        fs::write(&dat,r#"<datafile><header><name>Sinclair ZX Spectrum - Games - [Z80]</name></header><game name="Synthetic reference"><rom name="reference.z80" size="49182" crc="12345678"/></game></datafile>"#).unwrap();
        let catalogue = crate::ingestion::preview::SpectrumCatalogue::from_parsed(
            &crate::dat::parsers::parse_dat_file(&dat, Default::default())
                .unwrap()
                .dat,
        )
        .unwrap();
        let p = preview_media(
            &config,
            &[],
            &d.load_archives().unwrap(),
            &[catalogue],
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap();
        prepare_bounded_import(&p, context).unwrap()
    }
    fn import(&self) -> BoundedImportResult {
        apply_bounded_import(&self.plan(), &AtomicBool::new(false), &mut |_, _| {}).unwrap()
    }
    fn preview(&self, id: &str) -> DiscoveryImportUndoPreview {
        self.db().preview_discovery_import_undo(id).unwrap()
    }
    fn mutate(&self, sql: &str) {
        Connection::open(&self.db)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
}
#[test]
fn import_undo_preserves_preexisting_rows_files_and_durable_provenance() {
    let f = Fixture::new();
    let old = f.db().load_archives().unwrap();
    f.write("new.z80", 2);
    f.write("second.z80", 3);
    let r = f.import();
    assert_eq!(r.inserted.len(), 2);
    let id = r.operation_id.unwrap();
    let history = f.db().discovery_import_history(0, 50).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].operation_id, id);
    assert!(!history[0].approval_sha256.is_empty());
    let p = f.preview(&id);
    assert!(p.can_undo(), "{:?}", p.blockers);
    assert_eq!(p.affected_records(), 2);
    assert_eq!(p.affected_dependencies(), 2);
    let result = Database::undo_discovery_import(&p).unwrap();
    assert_eq!(result.removed_records, 2);
    assert_eq!(result.removed_assignments, 2);
    assert_eq!(f.db().load_archives().unwrap(), old);
    assert_eq!(fs::read(f.root.join("new.z80")).unwrap(), snapshot(2));
    assert_eq!(fs::read(f.root.join("second.z80")).unwrap(), snapshot(3));
    assert!(f.db().discovery_import_history(0, 50).unwrap()[0].undone);
    let repeated = Database::undo_discovery_import(&p).unwrap();
    assert!(repeated.already_undone);
    assert_eq!(repeated.removed_records, 0);
}
#[test]
fn mixed_existing_and_refused_results_have_exact_counts_and_no_existing_ownership() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    fs::write(f.root.join("notes.txt"), b"not a game").unwrap();
    let mut plan = f.plan();
    // A globally preexisting exact path outside the preview's source ownership.
    let d = Connection::open(&f.db).unwrap();
    d.execute(
        "UPDATE archives SET absolute_path_cached=?1",
        [f.root.join("new.z80").as_os_str().as_bytes()],
    )
    .unwrap();
    drop(d);
    // Rebuild the reviewed known snapshot, as normal preparation requires.
    plan.evidence.known = f.db().load_archives().unwrap();
    let r = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert!(r.inserted.is_empty());
    let op = &f.db().discovery_import_history(0, 50).unwrap()[0];
    assert_eq!(op.skipped, 2);
    assert_eq!(op.refused, 0);
    assert_eq!(op.excluded, 1);
    assert!(!f.preview(r.operation_id.as_deref().unwrap()).can_undo());
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
}
#[test]
fn changed_then_reverted_rows_and_assignments_refuse_whole_batch() {
    for table in ["archives", "platform_assignments"] {
        let f = Fixture::new();
        f.write("new.z80", 2);
        f.write("other.z80", 3);
        let r = f.import();
        let id = r.inserted[0].1;
        let p = f.preview(r.operation_id.as_deref().unwrap());
        let sql = if table == "archives" {
            format!("UPDATE archives SET display_name=display_name WHERE id={id}")
        } else {
            format!("UPDATE platform_assignments SET platform=platform WHERE archive_id={id}")
        };
        f.mutate(&sql);
        assert!(
            Database::undo_discovery_import(&p)
                .unwrap_err()
                .contains("Entire batch refused")
        );
        assert_eq!(f.db().load_archives().unwrap().len(), 3);
        assert!(!f.preview(r.operation_id.as_deref().unwrap()).can_undo());
    }
}
#[test]
fn later_dat_metadata_scan_and_path_dependencies_are_protected() {
    for kind in [
        "dat",
        "enrichment",
        "path_dat",
        "discovery",
        "fingerprint",
        "topology",
        "identity",
    ] {
        let f = Fixture::new();
        f.write("new.z80", 2);
        let r = f.import();
        let id = r.inserted[0].1;
        let op = r.operation_id.as_deref().unwrap();
        let p = f.preview(op);
        let c = Connection::open(&f.db).unwrap();
        match kind {
            "dat" => {
                c.execute("INSERT INTO library_dat_identities(archive_id,dat_source_id,verification_state,completeness,audited_at,facts_json,created_at,updated_at) VALUES(?1,'dat','verified','complete','now',X'00','now','now')",[id]).unwrap();
            }
            "enrichment" => {
                c.execute(
                    "INSERT INTO screenscraper_enrichments VALUES(?1,X'00',X'00','now')",
                    [id],
                )
                .unwrap();
            }
            "path_dat" => {
                c.execute("INSERT INTO dat_set_audit_results(archive_path,source_id,game_name,set_state_json,dependency_state_json,audited_at) VALUES(?1,'dat','new','{}','{}','now')",[f.root.join("new.z80").as_os_str().as_bytes()]).unwrap();
            }
            "discovery" => {
                c.execute("INSERT INTO discovery_details(scan_run_id,path,container,validation_state,explanation) VALUES(1,?1,'file','valid','new')",[f.root.join("new.z80").as_os_str().as_bytes()]).unwrap();
            }
            "fingerprint" => {
                c.execute("INSERT INTO scan_fingerprints(source_folder_id,relative_path,size_bytes,modified_time_ns,archive_kind,display_name,normalized_name,scanner_version,parser_version,cache_version,updated_at) VALUES(1,?1,1,1,'DirectGameImage','new','new','test','test',1,'now')",[b"new.z80".as_slice()]).unwrap();
            }
            "topology" => {
                c.execute("INSERT INTO media_topology_evidence(set_namespace,set_value,producer,producer_version,producer_schema,source_fingerprint,generation,created_at,refreshed_at,evidence_json) VALUES('test','set','test','1',1,'x',1,'now','now',X'00')",[]).unwrap();
            }
            "identity" => {
                c.execute(
                    "UPDATE archives SET identity_report_json=X'00' WHERE id=?1",
                    [id],
                )
                .unwrap();
            }
            _ => unreachable!(),
        };
        drop(c);
        assert!(Database::undo_discovery_import(&p).is_err(), "{kind}");
        assert!(!f.preview(op).can_undo(), "{kind}");
        assert_eq!(f.db().load_archives().unwrap().len(), 2);
    }
}
#[test]
fn unrelated_edits_are_preserved_and_do_not_block_selective_undo() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let p = f.preview(r.operation_id.as_deref().unwrap());
    f.mutate("UPDATE archives SET display_name='User name retained' WHERE id=1");
    Database::undo_discovery_import(&p).unwrap();
    let rows = f.db().load_archives().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].display_name, "User name retained");
}
#[test]
fn ownership_evidence_cannot_be_reset_or_deleted() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    f.mutate("UPDATE archives SET display_name=display_name WHERE id=2");
    let c = Connection::open(&f.db).unwrap();
    assert!(
        c.execute("UPDATE discovery_import_records SET touched=0", [])
            .is_err()
    );
    assert!(
        c.execute("DELETE FROM discovery_import_records", [])
            .is_err()
    );
    assert!(
        c.execute(
            "UPDATE discovery_import_operations SET receipt_json=X'00'",
            []
        )
        .is_err()
    );
    assert!(c.execute("INSERT OR REPLACE INTO discovery_import_records SELECT operation_id,archive_id,source_folder_id,relative_path,absolute_path,snapshot_json,0 FROM discovery_import_records",[]).is_err());
    assert!(c.execute("INSERT OR REPLACE INTO discovery_import_operations SELECT * FROM discovery_import_operations",[]).is_err());
    assert!(!f.preview(r.operation_id.as_deref().unwrap()).can_undo());
}
#[test]
fn unknown_schema_and_unreviewed_cascades_or_triggers_refuse_before_changes() {
    for sql in [
        "CREATE TABLE sqliteXnotes(archive_id INTEGER REFERENCES archives(id) ON DELETE CASCADE); INSERT INTO sqliteXnotes VALUES(2)",
        "CREATE TABLE new_grouping(archive_id INTEGER REFERENCES archives(id) ON DELETE CASCADE)",
        "ALTER TABLE archives ADD COLUMN user_notes TEXT",
        "CREATE TRIGGER surprise AFTER DELETE ON archives BEGIN DELETE FROM platform_assignments; END",
    ] {
        let f = Fixture::new();
        f.write("new.z80", 2);
        let r = f.import();
        let p = f.preview(r.operation_id.as_deref().unwrap());
        f.mutate(sql);
        assert!(
            Database::undo_discovery_import(&p)
                .unwrap_err()
                .contains("schema")
        );
        assert_eq!(f.db().load_archives().unwrap().len(), 2);
    }
}
#[test]
fn reused_archive_id_does_not_recover_ownership() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let id = r.inserted[0].1;
    let c = Connection::open(&f.db).unwrap();
    assert!(c.execute_batch(&format!("CREATE TEMP TABLE saved AS SELECT * FROM archives WHERE id={id}; DELETE FROM platform_assignments WHERE archive_id={id}; DELETE FROM archives WHERE id={id}; INSERT INTO archives SELECT * FROM saved;")).is_err());
    drop(c);
    assert!(!f.preview(r.operation_id.as_deref().unwrap()).can_undo());
}
#[test]
fn cancelled_import_creates_neither_rows_nor_success_receipt() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let plan = f.plan();
    let cancel = AtomicBool::new(false);
    let r = apply_bounded_import(&plan, &cancel, &mut |n, total| {
        if n == total {
            cancel.store(true, Ordering::Relaxed)
        }
    });
    assert!(r.is_err());
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
    assert!(f.db().discovery_import_history(0, 50).unwrap().is_empty());
}
#[test]
fn database_replacement_and_expired_preview_refuse() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let mut p = f.preview(r.operation_id.as_deref().unwrap());
    p.prepared -= std::time::Duration::from_secs(1801);
    assert!(p.expired());
    assert!(!p.can_undo());
    assert!(
        Database::undo_discovery_import(&p)
            .unwrap_err()
            .contains("expired")
    );
    let p = f.preview(r.operation_id.as_deref().unwrap());
    fs::rename(&f.db, f.db.with_extension("old")).unwrap();
    fs::copy(f.db.with_extension("old"), &f.db).unwrap();
    assert!(
        Database::undo_discovery_import(&p)
            .unwrap_err()
            .contains("identity")
    );
}
#[test]
fn mid_undo_failure_rolls_back_rows_assignments_receipt_and_touch_markers() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    f.write("second.z80", 3);
    let r = f.import();
    let p = f.preview(r.operation_id.as_deref().unwrap());
    TEST_FAULT.with(|v| v.set(1));
    let result = Database::undo_discovery_import(&p);
    TEST_FAULT.with(|v| v.set(0));
    assert!(result.is_err());
    assert_eq!(f.db().load_archives().unwrap().len(), 3);
    assert!(f.preview(r.operation_id.as_deref().unwrap()).can_undo());
    Database::undo_discovery_import(&p).unwrap();
}
#[test]
fn sqlite_writer_serialisation_revalidates_concurrent_edit() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let p = f.preview(r.operation_id.as_deref().unwrap());
    let c = Connection::open(&f.db).unwrap();
    c.execute_batch(
        "BEGIN IMMEDIATE; UPDATE archives SET display_name='Concurrent edit' WHERE id=2;",
    )
    .unwrap();
    let worker = std::thread::spawn(move || Database::undo_discovery_import(&p));
    std::thread::sleep(std::time::Duration::from_millis(50));
    c.execute_batch("COMMIT").unwrap();
    assert!(
        worker
            .join()
            .unwrap()
            .unwrap_err()
            .contains("Entire batch refused")
    );
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
}
#[test]
fn crash_child() {
    let Ok(path) = std::env::var("EMUWIZ_UNDO_CRASH_DB") else {
        return;
    };
    let fault: u8 = std::env::var("EMUWIZ_UNDO_CRASH_STAGE")
        .unwrap()
        .parse()
        .unwrap();
    TEST_FAULT.with(|v| v.set(fault));
    if fault >= 4 {
        let f = Fixture {
            _t: tempfile::tempdir().unwrap(),
            db: path.into(),
            root: std::env::var("EMUWIZ_UNDO_CRASH_ROOT").unwrap().into(),
            cfg: std::env::var("EMUWIZ_UNDO_CRASH_CONFIG").unwrap().into(),
        };
        f.import();
    } else {
        let op = std::env::var("EMUWIZ_UNDO_CRASH_OP").unwrap();
        let db = Database::open_read_only(path).unwrap();
        let p = db.preview_discovery_import_undo(&op).unwrap();
        drop(db);
        Database::undo_discovery_import(&p).unwrap();
    }
    panic!("fault did not terminate child");
}

#[test]
fn interrupted_sqlite_undo_is_atomic_before_and_after_commit() {
    for fault in [2, 3] {
        let f = Fixture::new();
        f.write("new.z80", 2);
        let r = f.import();
        let id = r.operation_id.as_deref().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "database::discovery_import::tests::crash_child",
                "--nocapture",
            ])
            .env("EMUWIZ_UNDO_CRASH_DB", &f.db)
            .env("EMUWIZ_UNDO_CRASH_OP", id)
            .env("EMUWIZ_UNDO_CRASH_STAGE", fault.to_string())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(90 + fault as i32));
        // Disposable DB only: let SQLite perform its own hot-journal recovery.
        // Production uses the existing copy-first recovery workflow if required.
        let recovered = Connection::open(&f.db).unwrap();
        let _: i64 = recovered
            .query_row("SELECT count(*) FROM archives", [], |r| r.get(0))
            .unwrap();
        drop(recovered);
        let op = &f.db().discovery_import_history(0, 50).unwrap()[0];
        assert_eq!(op.undone, fault == 3);
        assert_eq!(
            f.db().load_archives().unwrap().len(),
            if fault == 2 { 2 } else { 1 }
        );
        if fault == 2 {
            assert!(f.preview(id).can_undo());
            Database::undo_discovery_import(&f.preview(id)).unwrap();
        }
    }
}

#[test]
fn undo_never_reuses_created_ids_for_later_imports_or_delayed_evidence() {
    let f = Fixture::new();
    f.write("first.z80", 2);
    let a = f.import();
    let old_id = a.inserted[0].1;
    Database::undo_discovery_import(&f.preview(a.operation_id.as_deref().unwrap())).unwrap();
    let b = f.import();
    assert!(b.inserted.iter().all(|(_, id)| *id > old_id));
    let c = Connection::open(&f.db).unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    assert!(
        c.execute(
            "INSERT INTO screenscraper_enrichments VALUES(?1,X'00',X'00','late')",
            [old_id]
        )
        .is_err()
    );
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
}
#[test]
fn normal_previewed_existing_records_are_counted_but_never_owned() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    assert_eq!(r.inserted.len(), 1);
    let op = &f.db().discovery_import_history(0, 50).unwrap()[0];
    assert_eq!(op.skipped, 1);
    assert_eq!(op.refused, 0);
    assert!(op.items.iter().all(|i| i.archive_id != 1));
}

#[test]
fn retained_ids_cannot_be_reused_by_update_or_replace() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let old = r.inserted[0].1;
    Database::undo_discovery_import(&f.preview(r.operation_id.as_deref().unwrap())).unwrap();
    let c = Connection::open(&f.db).unwrap();
    assert!(
        c.execute("UPDATE archives SET id=?1 WHERE id=1", [old])
            .is_err()
    );
    assert_eq!(f.db().load_archives().unwrap()[0].id, 1);
}

#[test]
fn recorded_version_only_drift_blocks_a_stale_undo_preview() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let p = f.preview(r.operation_id.as_deref().unwrap());
    f.mutate(&format!(
        "INSERT INTO schema_migrations VALUES({},'unreviewed data-only change','now')",
        REVIEWED_SCHEMA + 1
    ));
    assert!(
        Database::undo_discovery_import(&p)
            .unwrap_err()
            .contains("version")
    );
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
}
#[test]
fn actual_import_process_interruption_keeps_rows_and_receipt_atomic() {
    for fault in [4u8, 5] {
        let f = Fixture::new();
        f.write("new.z80", 2);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "database::discovery_import::tests::crash_child",
                "--nocapture",
            ])
            .env("EMUWIZ_UNDO_CRASH_DB", &f.db)
            .env("EMUWIZ_UNDO_CRASH_ROOT", &f.root)
            .env("EMUWIZ_UNDO_CRASH_CONFIG", &f.cfg)
            .env("EMUWIZ_UNDO_CRASH_STAGE", fault.to_string())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(90 + fault as i32));
        let recovered = Connection::open(&f.db).unwrap();
        let _: i64 = recovered
            .query_row("SELECT count(*) FROM archives", [], |r| r.get(0))
            .unwrap();
        drop(recovered);
        let history = f.db().discovery_import_history(0, 50).unwrap();
        assert_eq!(history.len(), if fault == 4 { 0 } else { 1 });
        assert_eq!(
            f.db().load_archives().unwrap().len(),
            if fault == 4 { 1 } else { 2 }
        );
        if fault == 5 {
            assert!(f.preview(&history[0].operation_id).can_undo());
            Database::undo_discovery_import(&f.preview(&history[0].operation_id)).unwrap();
        }
        assert_eq!(fs::read(f.root.join("new.z80")).unwrap(), snapshot(2));
    }
}
#[test]
fn presentation_changes_cannot_retarget_the_sealed_undo() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let a = f.import();
    let mut p = f.preview(a.operation_id.as_deref().unwrap());
    p.operation.operation_id = "forged-other-operation".into();
    p.operation.items.clear();
    // Presentation may narrow/disable a preview, never retarget its executor.
    assert!(Database::undo_discovery_import(&p).is_err());
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
}

#[test]
fn partial_presentation_narrowing_never_deletes_more_than_confirmed() {
    let f = Fixture::new();
    f.write("first.z80", 2);
    f.write("second.z80", 3);
    let r = f.import();
    let mut p = f.preview(r.operation_id.as_deref().unwrap());
    assert_eq!(p.affected_records(), 2);
    p.operation.items.pop();
    assert!(!p.can_undo());
    assert_eq!(p.affected_records(), 2);
    assert!(Database::undo_discovery_import(&p).is_err());
    assert_eq!(f.db().load_archives().unwrap().len(), 3);
}
#[test]
fn offline_source_media_does_not_change_database_only_undo() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let p = f.preview(r.operation_id.as_deref().unwrap());
    let offline = f.root.with_extension("offline");
    fs::rename(&f.root, &offline).unwrap();
    Database::undo_discovery_import(&p).unwrap();
    assert_eq!(fs::read(offline.join("new.z80")).unwrap(), snapshot(2));
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
}
#[test]
fn operation_projection_uses_receipt_and_only_fresh_preview_offers_undo() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let p = f.preview(r.operation_id.as_deref().unwrap());
    let op = &f.db().discovery_import_history(0, 50).unwrap()[0];
    use crate::operation::*;
    let historical = discovery_import_operation(op, None);
    assert_eq!(historical.kind, OperationKind::DiscoveryImport);
    assert_eq!(
        historical.recovery.actions.rollback,
        ActionAvailability::Unavailable
    );
    assert_eq!(
        discovery_import_operation(op, Some(&p))
            .recovery
            .actions
            .rollback,
        ActionAvailability::Available
    );
    Database::undo_discovery_import(&p).unwrap();
    let undone = &f.db().discovery_import_history(0, 50).unwrap()[0];
    assert_eq!(
        discovery_import_operation(undone, None).state,
        OperationState::RolledBack
    );
}

// ---- Independent review tests (Discovery gate review of 8abe9b90), unmodified ----

/// A saved pre-proof DAT row, the same shape `dat_evidence_history_tests.rs`
/// stores (`LEGACY_FACTS`).
const GATE_FACTS: &str = r#"{"verification_state":{"state":"verified_single_match","algorithm":"sha1"},"source":{"source_id":"158","source_name":"Fixture","ecosystem":"no_intro","variant":"unknown","source_revision":"1","author":null,"catalogue_names":["Fixture"],"dat_path":"/nowhere/158.dat"},"canonical":{"canonical_dat_name":"Game","canonical_rom_name":"Game.gba","region":null,"revision":null},"hash_evidence":{"matched_algorithm":"sha1","matched_value":"aa","available_algorithms":["SHA-1"]},"ambiguous_candidates":[],"candidate_provenance":[],"matched_entries":[],"audited_hashes":{"size_bytes":4,"crc32":null,"md5":null,"sha1":"aa","sha256":null},"audited_at":"2026-10-08T00:50:48Z","completeness":"exhaustive"}"#;

fn gate_identity(sha1: &str) -> crate::dat::library_identity_summary::PersistedLibraryDatIdentity {
    let mut value: serde_json::Value = serde_json::from_str(GATE_FACTS).unwrap();
    value["audited_hashes"]["sha1"] = serde_json::Value::String(sha1.to_string());
    serde_json::from_value(value).unwrap()
}

fn gate_count(f: &Fixture, sql: &str) -> i64 {
    Connection::open(&f.db)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

fn gate_rows(f: &Fixture, sql: &str) -> Vec<String> {
    let c = Connection::open(&f.db).unwrap();
    let mut statement = c.prepare(sql).unwrap();
    let rows = statement
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect::<Vec<_>>();
    rows
}

/// One replaced-evidence row for `id`, as raw SQL (no trailing semicolon).
fn gate_history_sql(id: i64) -> String {
    format!(
        "INSERT INTO library_dat_identity_history(archive_id,archive_path,dat_source_id,verification_state,completeness,audited_at,revision_marked_stale,facts_json,row_created_at,row_updated_at,replaced_at,replaced_by_state) VALUES({id},x'2f78','158','verified_single_match','exhaustive','then',0,x'7b7d','then','then','now','no_match')"
    )
}

/// One re-check note for `id`, as raw SQL (no trailing semicolon).
fn gate_note_sql(id: i64) -> String {
    format!(
        "INSERT INTO dat_recheck_attempts(archive_id,dat_source_id,archive_path,attempted_at,reason,catalogue_sha256) VALUES({id},'158',x'2f78','now','The file changed while it was being checked',NULL)"
    )
}

fn gate_new_table_rows(f: &Fixture, id: i64) -> (i64, i64) {
    (
        gate_count(
            f,
            &format!("SELECT count(*) FROM library_dat_identity_history WHERE archive_id={id}"),
        ),
        gate_count(
            f,
            &format!("SELECT count(*) FROM dat_recheck_attempts WHERE archive_id={id}"),
        ),
    )
}

/// The same trusted one-disc report `topology_evidence_tests.rs` builds.
fn gate_topology_report(path: &Path) -> crate::media_set::TopologyReport {
    use crate::media_set::*;
    let mut record = media_record(path, Some("PlayStation"));
    record.availability = MediaAvailability::Observed;
    let mut evidence = MediaEvidence::new(EvidenceKind::TrustedDat, "test-topology");
    evidence.provenance.version = Some("1".into());
    evidence.release = Some(IdentityKey::new("test-release", "orbit"));
    evidence.medium = Some(IdentityKey::new("test-medium", "disc-1"));
    evidence.equivalence = Equivalence::AuthorityMapping;
    evidence.ordinal = Some(MediaOrdinal {
        number: 1,
        unit: OrdinalUnit::Disc,
    });
    evidence.expected_count = Some(ExpectedCount {
        count: 1,
        unit: OrdinalUnit::Disc,
    });
    record.evidence.push(evidence);
    TopologyReport {
        sets: resolve_index(index_media(vec![record])).sets,
        stats: TopologyStats::default(),
    }
}

// ---- success when safe: nothing about the imported game changed -------------

#[test]
fn gate_schema_27_untouched_import_undoes_and_leaves_the_new_tables_empty() {
    let f = Fixture::new();
    assert_eq!(f.db().schema_version().unwrap(), 27);
    assert_eq!(REVIEWED_SCHEMA, 27);
    f.write("new.z80", 2);
    let r = f.import();
    let id = r.inserted[0].1;
    assert_eq!(gate_new_table_rows(&f, id), (0, 0));
    let p = f.preview(r.operation_id.as_deref().unwrap());
    assert!(p.can_undo(), "{:?}", p.blockers);
    let result = Database::undo_discovery_import(&p).unwrap();
    assert_eq!(result.removed_records, 1);
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
    assert_eq!(
        gate_count(&f, "SELECT count(*) FROM library_dat_identity_history"),
        0
    );
    assert_eq!(
        gate_count(&f, "SELECT count(*) FROM dat_recheck_attempts"),
        0
    );
}

#[test]
fn gate_new_table_activity_for_other_games_never_blocks_undo() {
    // Raw inserts, updates and deletes for the pre-existing game (id 1).
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let id = r.inserted[0].1;
    assert_ne!(id, 1);
    let p = f.preview(r.operation_id.as_deref().unwrap());
    f.mutate(&format!(
        "{}; {}; {}; UPDATE library_dat_identity_history SET replaced_by_state='ambiguous' WHERE archive_id=1; UPDATE dat_recheck_attempts SET reason='again' WHERE archive_id=1; DELETE FROM library_dat_identity_history WHERE id=(SELECT min(id) FROM library_dat_identity_history WHERE archive_id=1);",
        gate_history_sql(1),
        gate_history_sql(1),
        gate_note_sql(1)
    ));
    assert_eq!(
        gate_count(
            &f,
            "SELECT count(*) FROM discovery_import_records WHERE touched=1"
        ),
        0
    );
    Database::undo_discovery_import(&p).unwrap();
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
    assert_eq!(gate_new_table_rows(&f, 1), (1, 1), "another game's rows");
}

#[test]
fn gate_verification_only_and_stale_evidence_changes_to_another_game_keep_undo_available() {
    // The real write paths, all aimed at the pre-existing game (id 1): a
    // first DAT row, a replacement (which keeps the old row), a re-check
    // note, and the catalogue-changed stale mark.
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let p = f.preview(r.operation_id.as_deref().unwrap());
    let mut d = Database::open_or_create(&f.db).unwrap();
    d.persist_library_dat_identity(1, &gate_identity("aa"))
        .unwrap();
    d.persist_library_dat_identity(1, &gate_identity("bb"))
        .unwrap();
    d.record_dat_recheck_attempts(
        "158",
        None,
        &[(
            1,
            f.root.join("existing.z80"),
            "The file changed while it was being checked".to_string(),
        )],
    )
    .unwrap();
    assert_eq!(
        d.mark_library_dat_identities_stale_for_source("158")
            .unwrap(),
        1
    );
    drop(d);
    assert_eq!(gate_new_table_rows(&f, 1), (1, 1));
    let fresh = f.preview(r.operation_id.as_deref().unwrap());
    assert!(fresh.can_undo(), "{:?}", fresh.blockers);
    // The preview taken before all of that is still good: nothing in the
    // batch changed.
    Database::undo_discovery_import(&p).unwrap();
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
    assert_eq!(gate_new_table_rows(&f, 1), (1, 1));
    assert_eq!(
        gate_count(
            &f,
            "SELECT count(*) FROM library_dat_identities WHERE archive_id=1"
        ),
        1
    );
}

#[test]
fn gate_unchanged_disc_order_refreshes_keep_undo_available() {
    // NOTE for the runner: if this fails with a "modified after import"
    // blocker, check whether `Database::open_or_create` itself writes to a
    // current database; report which statement marked the record.
    let f = Fixture::new();
    let disc = f._t.path().join("orbit.chd");
    fs::write(&disc, b"disc").unwrap();
    let mut d = Database::open_or_create(&f.db).unwrap();
    let first = d
        .persist_media_topology_evidence_checked(
            &gate_topology_report(&disc),
            "test-topology",
            "1",
            7,
        )
        .unwrap();
    assert_eq!((first.written, first.unchanged), (1, 0));
    drop(d);
    f.write("new.z80", 2);
    let r = f.import();
    let op = r.operation_id.as_deref().unwrap();
    for generation in 8..12u64 {
        let mut d = Database::open_or_create(&f.db).unwrap();
        let again = d
            .persist_media_topology_evidence_checked(
                &gate_topology_report(&disc),
                "test-topology",
                "1",
                generation,
            )
            .unwrap();
        assert_eq!((again.written, again.unchanged), (1, 1), "{generation}");
    }
    let p = f.preview(op);
    assert!(p.can_undo(), "{:?}", p.blockers);
    Database::undo_discovery_import(&p).unwrap();
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
}

#[test]
fn gate_a_changed_disc_order_refresh_still_refuses_a_sealed_undo() {
    let f = Fixture::new();
    let disc = f._t.path().join("orbit.chd");
    fs::write(&disc, b"disc").unwrap();
    let mut d = Database::open_or_create(&f.db).unwrap();
    d.persist_media_topology_evidence_checked(
        &gate_topology_report(&disc),
        "test-topology",
        "1",
        7,
    )
    .unwrap();
    drop(d);
    f.write("new.z80", 2);
    let r = f.import();
    let op = r.operation_id.as_deref().unwrap();
    let p = f.preview(op);
    assert!(p.can_undo(), "{:?}", p.blockers);
    fs::write(&disc, b"another disc entirely").unwrap();
    let mut d = Database::open_or_create(&f.db).unwrap();
    let changed = d
        .persist_media_topology_evidence_checked(
            &gate_topology_report(&disc),
            "test-topology",
            "1",
            7,
        )
        .unwrap();
    assert_eq!((changed.written, changed.unchanged), (1, 0));
    drop(d);
    assert!(
        Database::undo_discovery_import(&p)
            .unwrap_err()
            .contains("Entire batch refused")
    );
    assert!(!f.preview(op).can_undo());
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
}

// ---- refusal when unsafe: the imported game itself has new-table rows -------

#[test]
fn gate_rows_in_either_new_table_for_an_imported_game_refuse_undo_and_survive() {
    for kind in [
        "history",
        "note",
        "note_replace",
        "history_moved_in",
        "history_added_then_removed",
        "history_moved_away",
    ] {
        let f = Fixture::new();
        f.write("new.z80", 2);
        f.write("other.z80", 3);
        let r = f.import();
        assert_eq!(r.inserted.len(), 2);
        let id = r.inserted[0].1;
        let op = r.operation_id.as_deref().unwrap();
        let p = f.preview(op);
        assert!(p.can_undo(), "{kind}: {:?}", p.blockers);
        let (sql, table, left) = match kind {
            "history" => (gate_history_sql(id), "library_dat_identity_history", 1),
            "note" => (gate_note_sql(id), "dat_recheck_attempts", 1),
            "note_replace" => (
                gate_note_sql(id).replacen("INSERT INTO", "INSERT OR REPLACE INTO", 1),
                "dat_recheck_attempts",
                1,
            ),
            "history_moved_in" => (
                format!(
                    "{}; UPDATE library_dat_identity_history SET archive_id={id} WHERE archive_id=1",
                    gate_history_sql(1)
                ),
                "library_dat_identity_history",
                1,
            ),
            "history_added_then_removed" => (
                format!(
                    "{}; DELETE FROM library_dat_identity_history WHERE archive_id={id}",
                    gate_history_sql(id)
                ),
                "library_dat_identity_history",
                0,
            ),
            "history_moved_away" => (
                format!(
                    "{}; UPDATE library_dat_identity_history SET archive_id=1 WHERE archive_id={id}",
                    gate_history_sql(id)
                ),
                "library_dat_identity_history",
                0,
            ),
            _ => unreachable!(),
        };
        f.mutate(&sql);
        assert_eq!(
            gate_count(
                &f,
                &format!("SELECT touched FROM discovery_import_records WHERE archive_id={id}")
            ),
            1,
            "{kind}"
        );
        // The sealed preview, and a fresh one, both refuse the whole batch.
        assert!(
            Database::undo_discovery_import(&p)
                .unwrap_err()
                .contains("Entire batch refused"),
            "{kind}"
        );
        let fresh = f.preview(op);
        assert!(!fresh.can_undo(), "{kind}");
        if left == 1 {
            assert!(
                fresh.blockers.iter().any(|b| b.contains(table)),
                "{kind}: {:?}",
                fresh.blockers
            );
        }
        assert!(Database::undo_discovery_import(&fresh).is_err(), "{kind}");
        // Nothing was removed: both games, and the row itself, are there.
        assert_eq!(f.db().load_archives().unwrap().len(), 3, "{kind}");
        assert_eq!(
            gate_count(
                &f,
                &format!("SELECT count(*) FROM {table} WHERE archive_id={id}")
            ),
            left,
            "{kind}"
        );
        assert!(!f.db().discovery_import_history(0, 50).unwrap()[0].undone);
    }
}

#[test]
fn gate_verifying_the_imported_game_refuses_undo_and_keeps_every_evidence_row() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let id = r.inserted[0].1;
    let op = r.operation_id.as_deref().unwrap();
    let sealed = f.preview(op);
    assert!(sealed.can_undo());
    // First check of the imported game (the single-game path's write).
    let mut d = Database::open_or_create(&f.db).unwrap();
    d.persist_library_dat_identity(id, &gate_identity("aa"))
        .unwrap();
    drop(d);
    assert!(
        Database::undo_discovery_import(&sealed)
            .unwrap_err()
            .contains("Entire batch refused")
    );
    let after_first = f.preview(op);
    assert!(
        after_first
            .blockers
            .iter()
            .any(|b| b.contains("library_dat_identities")),
        "{:?}",
        after_first.blockers
    );
    assert_eq!(gate_new_table_rows(&f, id), (0, 0));
    // A later check replaces it: the old row is kept, and is protected too.
    let mut d = Database::open_or_create(&f.db).unwrap();
    d.persist_library_dat_identity(id, &gate_identity("bb"))
        .unwrap();
    d.record_dat_recheck_attempts(
        "158",
        None,
        &[(id, f.root.join("new.z80"), "Not checked".to_string())],
    )
    .unwrap();
    drop(d);
    assert_eq!(gate_new_table_rows(&f, id), (1, 1));
    let after_second = f.preview(op);
    assert!(!after_second.can_undo());
    for table in ["library_dat_identity_history", "dat_recheck_attempts"] {
        assert!(
            after_second.blockers.iter().any(|b| b.contains(table)),
            "{table}: {:?}",
            after_second.blockers
        );
    }
    assert!(Database::undo_discovery_import(&after_second).is_err());
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
    assert_eq!(gate_new_table_rows(&f, id), (1, 1));
    assert_eq!(
        gate_count(
            &f,
            &format!("SELECT count(*) FROM library_dat_identities WHERE archive_id={id}")
        ),
        1
    );
}

#[test]
fn gate_a_recheck_note_alone_for_the_imported_game_refuses_undo() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let id = r.inserted[0].1;
    let op = r.operation_id.as_deref().unwrap();
    let sealed = f.preview(op);
    let mut d = Database::open_or_create(&f.db).unwrap();
    d.record_dat_recheck_attempts(
        "158",
        Some("00"),
        &[(id, f.root.join("new.z80"), "Not checked".to_string())],
    )
    .unwrap();
    drop(d);
    assert!(Database::undo_discovery_import(&sealed).is_err());
    assert!(!f.preview(op).can_undo());
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
    assert_eq!(gate_new_table_rows(&f, id), (0, 1));
}

// ---- a re-check around an Undo preview / Undo -------------------------------

#[test]
fn gate_a_recheck_writer_holding_the_lock_is_serialised_with_undo() {
    // (a) The writer changes the imported game: Undo waits, then refuses.
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let id = r.inserted[0].1;
    let p = f.preview(r.operation_id.as_deref().unwrap());
    let c = Connection::open(&f.db).unwrap();
    c.execute_batch(&format!("BEGIN IMMEDIATE; {};", gate_history_sql(id)))
        .unwrap();
    let worker = std::thread::spawn(move || Database::undo_discovery_import(&p));
    std::thread::sleep(std::time::Duration::from_millis(50));
    c.execute_batch("COMMIT").unwrap();
    assert!(
        worker
            .join()
            .unwrap()
            .unwrap_err()
            .contains("Entire batch refused")
    );
    drop(c);
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
    assert_eq!(gate_new_table_rows(&f, id), (1, 0));

    // (b) The writer changes only another game: Undo waits, then succeeds,
    // and the other game's rows are untouched.
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let p = f.preview(r.operation_id.as_deref().unwrap());
    let c = Connection::open(&f.db).unwrap();
    c.execute_batch(&format!(
        "BEGIN IMMEDIATE; {}; {};",
        gate_history_sql(1),
        gate_note_sql(1)
    ))
    .unwrap();
    let worker = std::thread::spawn(move || Database::undo_discovery_import(&p));
    std::thread::sleep(std::time::Duration::from_millis(50));
    c.execute_batch("COMMIT").unwrap();
    let result = worker.join().unwrap().unwrap();
    assert_eq!(result.removed_records, 1);
    drop(c);
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
    assert_eq!(gate_new_table_rows(&f, 1), (1, 1));
}

#[test]
fn gate_a_preview_taken_beside_an_uncommitted_recheck_cannot_authorise_undo() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let id = r.inserted[0].1;
    let op = r.operation_id.as_deref().unwrap();
    let c = Connection::open(&f.db).unwrap();
    c.execute_batch(&format!("BEGIN IMMEDIATE; {};", gate_history_sql(id)))
        .unwrap();
    // Either the read-only open is refused beside a live writer, or the
    // preview reads only committed state. Both are safe; neither may lead
    // to a deletion once the writer commits.
    let beside = match Database::open_read_only(&f.db) {
        Ok(d) => d.preview_discovery_import_undo(op).ok(),
        Err(_) => None,
    };
    c.execute_batch("COMMIT").unwrap();
    drop(c);
    if let Some(p) = beside {
        assert!(Database::undo_discovery_import(&p).is_err());
    }
    assert!(!f.preview(op).can_undo());
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
    assert_eq!(gate_new_table_rows(&f, id), (1, 0));
}

// ---- interrupted transactions ----------------------------------------------

#[test]
fn gate_a_rolled_back_recheck_leaves_no_mark_and_undo_still_succeeds() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let id = r.inserted[0].1;
    let op = r.operation_id.as_deref().unwrap();
    f.mutate(&format!(
        "BEGIN IMMEDIATE; {}; {}; UPDATE archives SET display_name='half written' WHERE id={id}; ROLLBACK;",
        gate_history_sql(id),
        gate_note_sql(id)
    ));
    assert_eq!(
        gate_count(
            &f,
            "SELECT count(*) FROM discovery_import_records WHERE touched=1"
        ),
        0
    );
    assert_eq!(gate_new_table_rows(&f, id), (0, 0));
    let p = f.preview(op);
    assert!(p.can_undo(), "{:?}", p.blockers);
    Database::undo_discovery_import(&p).unwrap();
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
}

#[test]
fn gate_a_failed_undo_rolls_back_beside_another_games_kept_evidence() {
    let f = Fixture::new();
    f.write("new.z80", 2);
    let r = f.import();
    let op = r.operation_id.as_deref().unwrap();
    f.mutate(&format!("{}; {};", gate_history_sql(1), gate_note_sql(1)));
    let p = f.preview(op);
    assert!(p.can_undo(), "{:?}", p.blockers);
    TEST_FAULT.with(|v| v.set(1));
    let result = Database::undo_discovery_import(&p);
    TEST_FAULT.with(|v| v.set(0));
    assert!(result.is_err());
    assert_eq!(f.db().load_archives().unwrap().len(), 2);
    assert_eq!(gate_new_table_rows(&f, 1), (1, 1));
    assert!(f.preview(op).can_undo());
    Database::undo_discovery_import(&p).unwrap();
    assert_eq!(f.db().load_archives().unwrap().len(), 1);
    assert_eq!(gate_new_table_rows(&f, 1), (1, 1));
}

#[test]
fn gate_a_killed_undo_is_atomic_beside_another_games_kept_evidence() {
    for fault in [2, 3] {
        let f = Fixture::new();
        f.write("new.z80", 2);
        let r = f.import();
        let id = r.operation_id.as_deref().unwrap();
        f.mutate(&format!("{}; {};", gate_history_sql(1), gate_note_sql(1)));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "database::discovery_import::tests::crash_child",
                "--nocapture",
            ])
            .env("EMUWIZ_UNDO_CRASH_DB", &f.db)
            .env("EMUWIZ_UNDO_CRASH_OP", id)
            .env("EMUWIZ_UNDO_CRASH_STAGE", fault.to_string())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(90 + fault as i32));
        // Disposable DB only: let SQLite perform its own hot-journal recovery.
        let recovered = Connection::open(&f.db).unwrap();
        let _: i64 = recovered
            .query_row("SELECT count(*) FROM archives", [], |r| r.get(0))
            .unwrap();
        drop(recovered);
        assert_eq!(
            f.db().discovery_import_history(0, 50).unwrap()[0].undone,
            fault == 3
        );
        assert_eq!(
            f.db().load_archives().unwrap().len(),
            if fault == 2 { 2 } else { 1 }
        );
        assert_eq!(gate_new_table_rows(&f, 1), (1, 1));
        if fault == 2 {
            assert!(f.preview(id).can_undo());
            Database::undo_discovery_import(&f.preview(id)).unwrap();
            assert_eq!(gate_new_table_rows(&f, 1), (1, 1));
        }
    }
}

// ---- migrated 26 -> 27 versus fresh ----------------------------------------

#[test]
fn gate_migration_26_to_27_keeps_discovery_records_and_protections_and_matches_fresh() {
    let mut c = Connection::open_in_memory().unwrap();
    apply_migrations(&mut c, &MIGRATIONS[..26]).unwrap();
    assert_eq!(schema_version(&c).unwrap(), 26);
    assert!(require_reviewed_schema(&c).is_err(), "26 is not reviewed");
    c.execute_batch(
        "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,x'2f726f6d73','now','now');
         INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) VALUES(7,1,x'61',x'2f61',x'61','direct_game_image','a','a','now','now','now','now');
         INSERT INTO discovery_import_operations VALUES('op','committed','then',NULL,x'7b7d');
         INSERT INTO discovery_import_operations VALUES('gone','undone','then','later',x'7b7d');
         INSERT INTO discovery_import_records VALUES('op',7,1,x'61',x'2f61',x'7b7d',0);
         INSERT INTO discovery_import_records VALUES('gone',8,1,x'62',x'2f62',x'7b7d',1);",
    )
    .unwrap();
    let dump = |c: &Connection| -> Vec<String> {
        let mut out = Vec::new();
        for sql in [
            "SELECT quote(operation_id)||quote(state)||quote(created_at)||quote(undone_at)||quote(receipt_json) FROM discovery_import_operations ORDER BY 1",
            "SELECT quote(operation_id)||quote(archive_id)||quote(source_folder_id)||quote(relative_path)||quote(absolute_path)||quote(snapshot_json)||quote(touched) FROM discovery_import_records ORDER BY 1",
        ] {
            let mut statement = c.prepare(sql).unwrap();
            let rows = statement
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .map(|row| row.unwrap())
                .collect::<Vec<_>>();
            out.extend(rows);
        }
        out
    };
    let before = dump(&c);
    assert_eq!(before.len(), 4);
    apply_migrations(&mut c, MIGRATIONS).unwrap();
    assert_eq!(schema_version(&c).unwrap(), 27);
    assert_eq!(dump(&c), before, "the upgrade changed Discovery evidence");

    // Identical to a fresh schema 27 for the gate: digest and history.
    let mut fresh = Connection::open_in_memory().unwrap();
    apply_migrations(&mut fresh, MIGRATIONS).unwrap();
    assert_eq!(schema_digest(&c).unwrap(), schema_digest(&fresh).unwrap());
    require_reviewed_schema(&c).unwrap();
    require_reviewed_schema(&fresh).unwrap();
    let triggers = |c: &Connection| -> Vec<String> {
        let mut statement = c
            .prepare("SELECT name||'|'||tbl_name||'|'||sql FROM sqlite_schema WHERE type='trigger' ORDER BY name")
            .unwrap();
        let rows = statement
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect::<Vec<_>>();
        rows
    };
    assert_eq!(triggers(&c), triggers(&fresh));

    // The retained-evidence protections still bite on the migrated database.
    assert!(
        c.execute("UPDATE discovery_import_records SET touched=0", [])
            .is_err()
    );
    assert!(
        c.execute("DELETE FROM discovery_import_records", [])
            .is_err()
    );
    assert!(
        c.execute("DELETE FROM discovery_import_operations", [])
            .is_err()
    );
    assert!(
        c.execute(
            "UPDATE discovery_import_operations SET receipt_json=x'00'",
            []
        )
        .is_err()
    );
    // The id tombstone of the undone record (8) still refuses reuse.
    assert!(
        c.execute(
            "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) VALUES(8,1,x'62',x'2f62',x'62','direct_game_image','b','b','now','now','now','now')",
            [],
        )
        .is_err()
    );
    // And the new triggers work there: only game 7's record is marked.
    c.execute_batch(&format!("{};", gate_note_sql(99))).unwrap();
    let touched = |c: &Connection, id: i64| -> i64 {
        c.query_row(
            "SELECT touched FROM discovery_import_records WHERE archive_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(touched(&c, 7), 0, "a note for another game");
    c.execute_batch(&format!("{};", gate_history_sql(7)))
        .unwrap();
    assert_eq!(touched(&c, 7), 1);
}

#[test]
fn gate_an_import_made_before_schema_27_undoes_after_the_upgrade_unless_it_was_modified() {
    for modified in [false, true] {
        let f = Fixture::new();
        f.write("new.z80", 2);
        let r = f.import();
        let op = r.operation_id.clone().unwrap();
        let id = r.inserted[0].1;
        if modified {
            f.mutate(&format!(
                "UPDATE archives SET display_name=display_name WHERE id={id}"
            ));
        }
        let records = "SELECT quote(operation_id)||quote(archive_id)||quote(source_folder_id)||quote(relative_path)||quote(absolute_path)||quote(snapshot_json)||quote(touched) FROM discovery_import_records ORDER BY 1";
        let receipts = "SELECT quote(operation_id)||quote(state)||quote(created_at)||quote(undone_at)||quote(receipt_json) FROM discovery_import_operations ORDER BY 1";
        let before = (gate_rows(&f, records), gate_rows(&f, receipts));
        // Take the library back to exactly what a schema-26 build leaves: the
        // real receipt and ownership rows, none of schema 27's objects.
        f.mutate(
            "DROP TABLE library_dat_identity_history; DROP TABLE dat_recheck_attempts; DELETE FROM schema_migrations WHERE version=27; PRAGMA user_version=26;",
        );
        assert!(
            Database::open_read_only(&f.db).is_err(),
            "a schema-26 library must not be read as current"
        );
        let report = crate::upgrade_library_database(&f.db).unwrap();
        assert_eq!((report.from_version, report.to_version), (26, 27));
        assert_eq!((gate_rows(&f, records), gate_rows(&f, receipts)), before);
        let c = Connection::open(&f.db).unwrap();
        require_reviewed_schema(&c).unwrap();
        let mut fresh = Connection::open_in_memory().unwrap();
        apply_migrations(&mut fresh, MIGRATIONS).unwrap();
        assert_eq!(schema_digest(&c).unwrap(), schema_digest(&fresh).unwrap());
        drop(c);
        let p = f.preview(&op);
        if modified {
            assert!(!p.can_undo());
            assert!(Database::undo_discovery_import(&p).is_err());
            assert_eq!(f.db().load_archives().unwrap().len(), 2);
        } else {
            assert!(p.can_undo(), "{:?}", p.blockers);
            Database::undo_discovery_import(&p).unwrap();
            assert_eq!(f.db().load_archives().unwrap().len(), 1);
            assert!(f.db().discovery_import_history(0, 50).unwrap()[0].undone);
        }
    }
}

// ---- the gate itself --------------------------------------------------------

/// Tripwire for the next schema (see REVIEW.md point 10). A table that names
/// an `archive_id` is per-game data. This fails the moment one is added, until
/// a person extends the reviewed list here AND gives the table touch triggers
/// (and decides whether `check_import_undo` must count it).
#[test]
fn gate_every_table_naming_an_archive_id_is_on_the_reviewed_list_with_touch_triggers() {
    let mut c = Connection::open_in_memory().unwrap();
    apply_migrations(&mut c, MIGRATIONS).unwrap();
    let tables: Vec<String> = {
        let mut statement = c
            .prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name")
            .unwrap();
        let rows = statement
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect::<Vec<_>>();
        rows
    };
    let mut naming = Vec::new();
    let mut without_foreign_key = Vec::new();
    for table in &tables {
        let names: bool = c
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name='archive_id')"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        if !names {
            continue;
        }
        naming.push(table.clone());
        let bound: bool = c
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_list('{table}') WHERE \"table\"='archives' AND \"from\"='archive_id')"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        if !bound {
            without_foreign_key.push(table.clone());
        }
        if table == "discovery_import_records" {
            continue;
        }
        for event in ["INSERT", "UPDATE", "DELETE"] {
            let count: i64 = c
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name=?1 AND sql LIKE ?2 AND sql LIKE '%UPDATE discovery_import_records SET touched=1%'",
                    rusqlite::params![table, format!("%AFTER {event} ON {table}%")],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                count, 1,
                "{table} lacks one Discovery touch trigger on {event}"
            );
        }
    }
    assert_eq!(
        naming,
        [
            "archive_scan_observations",
            "dat_recheck_attempts",
            "dat_set_audit_results",
            "discovery_import_records",
            "library_dat_identities",
            "library_dat_identity_history",
            "platform_assignments",
            "screenscraper_enrichments",
            "verified_identity_facts",
        ],
        "a table naming archive ids was added or removed: review Discovery Undo"
    );
    // The per-game tables that can outlive their game (no foreign key). Each
    // one needs an answer to "what if the id is given to another game?".
    assert_eq!(
        without_foreign_key,
        [
            "dat_recheck_attempts",
            "discovery_import_records",
            "library_dat_identity_history",
        ]
    );
}

#[test]
fn gate_the_collections_tables_of_pr_278_are_refused_until_reviewed() {
    // The shape open PR #278 adds: an archive_id with no foreign key and no
    // touch trigger. Present without a reviewed schema, import and Undo stop.
    for sql in [
        "CREATE TABLE game_collections (id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, name TEXT NOT NULL, name_key TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL); CREATE TABLE game_collection_items (collection_id INTEGER NOT NULL REFERENCES game_collections(id) ON DELETE CASCADE, archive_id INTEGER NOT NULL, archive_created_at TEXT NOT NULL, source_folder_id INTEGER NOT NULL, relative_path BLOB NOT NULL, absolute_path BLOB NOT NULL, size_bytes INTEGER, title TEXT NOT NULL, platform TEXT, added_at TEXT NOT NULL, PRIMARY KEY (collection_id, archive_id, archive_created_at));",
        "CREATE INDEX gate_extra_index ON library_dat_identity_history(archive_path)",
        "DROP TRIGGER discovery_touch_dat_recheck_attempts_insert",
        "DROP TRIGGER discovery_touch_library_dat_identity_history_update",
    ] {
        let f = Fixture::new();
        f.write("new.z80", 2);
        let r = f.import();
        let p = f.preview(r.operation_id.as_deref().unwrap());
        f.mutate(sql);
        assert!(
            Database::undo_discovery_import(&p)
                .unwrap_err()
                .contains("schema"),
            "{sql}"
        );
        assert_eq!(f.db().load_archives().unwrap().len(), 2);
        // A new import is refused as well (the same gate).
        assert!(f.db().require_discovery_import_schema().is_err(), "{sql}");
    }
}

// ---- finding F1: rows that outlive a forgotten game -------------------------

/// A non-Discovery game that had replaced evidence and a re-check note is
/// forgotten the way `forget_missing` / `missing_cleanup` do it (their child
/// tables, then the row). Schema 27's two tables have no foreign key and are
/// in neither `CHILD_TABLES` list, so its rows stay. Returns the freed id.
fn gate_forgotten_game_leaves_rows_behind(f: &Fixture) -> i64 {
    let c = Connection::open(&f.db).unwrap();
    c.execute(
        "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) VALUES(2,(SELECT source_folder_id FROM archives WHERE id=1),?1,?2,?1,'direct_game_image','gone','gone','then','then','then','then')",
        rusqlite::params![b"gone.z80".as_slice(), b"/elsewhere/gone.z80".as_slice()],
    )
    .unwrap();
    c.execute_batch(&format!(
        "{}; {}; DELETE FROM screenscraper_enrichments WHERE archive_id=2; DELETE FROM library_dat_identities WHERE archive_id=2; DELETE FROM verified_identity_facts WHERE archive_id=2; DELETE FROM platform_assignments WHERE archive_id=2; DELETE FROM archive_scan_observations WHERE archive_id=2; DELETE FROM archives WHERE id=2;",
        gate_history_sql(2),
        gate_note_sql(2)
    ))
    .unwrap();
    2
}

/// Passes whatever Undo decides: rows of the two new tables are never removed
/// by Undo, and Undo either removes exactly the imported game or nothing.
#[test]
fn gate_rows_left_by_a_forgotten_game_are_never_deleted_by_undo() {
    let f = Fixture::new();
    let freed = gate_forgotten_game_leaves_rows_behind(&f);
    assert_eq!(gate_new_table_rows(&f, freed), (1, 1));
    f.write("new.z80", 2);
    let r = f.import();
    assert_eq!(r.inserted.len(), 1);
    let p = f.preview(r.operation_id.as_deref().unwrap());
    let undone = Database::undo_discovery_import(&p).is_ok();
    assert_eq!(
        f.db().load_archives().unwrap().len(),
        if undone { 1 } else { 2 }
    );
    assert_eq!(gate_new_table_rows(&f, freed), (1, 1));
}

// The reviewer's `f1_…` test stood here. It asserted that a later import is
// given a forgotten game's id; that can no longer happen, and the reviewer's
// re-review of 9e87d192 asked for it to be removed in favour of the `fix_f1_…`
// tests below.

// ---- Author's regression tests for review finding F1 (not the reviewer's) ----
//
// A later game is never given the id of a deleted game that left kept records.

/// A game that was deleted after leaving kept DAT evidence, a re-check note,
/// or both. Returns its (now free) id.
fn fix_f1_deleted_game_with(f: &Fixture, history: bool, note: bool) -> i64 {
    let c = Connection::open(&f.db).unwrap();
    c.execute(
        "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) VALUES(2,(SELECT source_folder_id FROM archives WHERE id=1),?1,?2,?1,'direct_game_image','gone','gone','then','then','then','then')",
        rusqlite::params![b"gone.z80".as_slice(), b"/elsewhere/gone.z80".as_slice()],
    )
    .unwrap();
    if history {
        c.execute_batch(&gate_history_sql(2)).unwrap();
    }
    if note {
        c.execute_batch(&gate_note_sql(2)).unwrap();
    }
    c.execute_batch("DELETE FROM archive_scan_observations WHERE archive_id=2; DELETE FROM platform_assignments WHERE archive_id=2; DELETE FROM archives WHERE id=2;")
        .unwrap();
    2
}

#[test]
fn fix_f1_a_later_import_never_inherits_a_deleted_games_id_or_records_and_can_be_undone() {
    for (history, note) in [(true, false), (false, true), (true, true)] {
        let f = Fixture::new();
        let freed = fix_f1_deleted_game_with(&f, history, note);
        let kept = gate_new_table_rows(&f, freed);
        assert_eq!(kept, (i64::from(history), i64::from(note)));
        f.write("new.z80", 2);
        let r = f.import();
        let id = r.inserted[0].1;
        assert!(
            id > freed,
            "the deleted game's id {freed} was given out again"
        );
        assert_eq!(gate_new_table_rows(&f, id), (0, 0));
        assert_eq!(
            gate_count(
                &f,
                &format!("SELECT touched FROM discovery_import_records WHERE archive_id={id}")
            ),
            0
        );
        // Nothing about the imported game changed: its Undo is still safe,
        // and it removes that game only.
        let p = f.preview(r.operation_id.as_deref().unwrap());
        assert!(p.can_undo(), "{:?}", p.blockers);
        Database::undo_discovery_import(&p).unwrap();
        assert_eq!(f.db().load_archives().unwrap().len(), 1);
        // The deleted game's records are exactly where they were.
        assert_eq!(gate_new_table_rows(&f, freed), kept);
    }
}

#[test]
fn fix_f1_without_kept_records_a_freed_id_is_still_reused_as_before() {
    // The allocator only skips ids that something still refers to.
    let f = Fixture::new();
    let freed = fix_f1_deleted_game_with(&f, false, false);
    f.write("new.z80", 2);
    let r = f.import();
    assert_eq!(r.inserted[0].1, freed);
    let p = f.preview(r.operation_id.as_deref().unwrap());
    assert!(p.can_undo(), "{:?}", p.blockers);
}

#[test]
fn fix_f1_a_game_removed_by_undo_keeps_its_id_reserved_for_good() {
    let f = Fixture::new();
    f.write("first.z80", 2);
    let r = f.import();
    let first = r.inserted[0].1;
    Database::undo_discovery_import(&f.preview(r.operation_id.as_deref().unwrap())).unwrap();
    f.write("second.z80", 3);
    let r = f.import();
    let second = r
        .inserted
        .iter()
        .map(|(_, id)| *id)
        .max()
        .expect("the second import added a game");
    assert!(
        second > first,
        "an undone import's id {first} was given out again"
    );
}
