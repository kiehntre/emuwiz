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
