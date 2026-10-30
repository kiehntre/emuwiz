//! Disposable filesystem + SQLite journeys, including restart and failure gaps.
use super::*;
use crate::dat::rename_apply::{EntryState, RenameTransaction, TransactionState};
use rusqlite::{Connection, params};

struct Fixture {
    temp: tempfile::TempDir,
    db: crate::Database,
    plan: OrganisationPlan,
    tx: RenameTransaction,
    approved: BTreeSet<String>,
    source: PathBuf,
    destination: PathBuf,
    id: i64,
    unrelated: i64,
}
impl Fixture {
    fn new(mode: OrganisationMode) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        let incoming = root.join("incoming");
        std::fs::create_dir_all(&incoming).unwrap();
        let cand = candidate_with_name(
            &incoming,
            "Old.zip",
            "New.zip",
            resolved("PSP", PlatformIdentitySource::Manual),
        );
        let mut db = crate::Database::open_or_create(temp.path().join("fixture.sqlite3")).unwrap();
        db.register_source_folders(std::slice::from_ref(&root))
            .unwrap();
        let folder = db.list_source_folders().unwrap()[0].id;
        let archive = crate::Archive::from_path_in_root(&cand.source_path, &root).unwrap();
        let id = db
            .upsert_archive(folder, &root, &archive)
            .unwrap()
            .archive_id;
        let other = candidate(
            &root,
            "Unrelated.zip",
            resolved("PSP", PlatformIdentitySource::Manual),
        );
        let archive = crate::Archive::from_path_in_root(&other.source_path, &root).unwrap();
        let unrelated = db
            .upsert_archive(folder, &root, &archive)
            .unwrap()
            .archive_id;
        let mut plan = plan_for(&root, mode, std::slice::from_ref(&cand), 1);
        prepare_organisation_catalogue(&mut plan, &db).unwrap();
        let approved = approved_of(&[&cand.source_path]);
        let tx = build_organisation_transaction(&plan, &approved, 1).unwrap();
        let source = cand.source_path;
        let destination = tx.entries[0].destination_path.clone();
        Self {
            temp,
            db,
            plan,
            tx,
            approved,
            source,
            destination,
            id,
            unrelated,
        }
    }
    fn sql(&self) -> Connection {
        Connection::open(self.db.path()).unwrap()
    }
    fn journal(&self) -> PathBuf {
        self.temp.path().join("journal")
    }
    fn apply(
        &mut self,
    ) -> Result<crate::dat::rename_apply::ApplyOutcome, crate::dat::rename_apply::ApplyError> {
        let journal = self.journal();
        super::super::transaction::apply_organisation_transaction(
            &mut self.tx,
            &self.approved,
            1,
            TrustedRoots::from_paths([self.plan.master_root.clone()]),
            &journal,
            &no_cancel(),
            self.plan.mode,
            &self.plan.master_root,
            &self.db,
        )
    }
    fn undo(&mut self) -> Result<OrganisationRollbackOutcome, String> {
        let journal = self.journal();
        rollback_organisation_transaction(
            &mut self.tx,
            &journal,
            &no_cancel(),
            &self.plan.master_root,
        )
    }
    fn path_id(&self, p: &Path) -> Option<i64> {
        self.db.find_archive_id_by_absolute_path(p).unwrap()
    }
    fn reload(&mut self) {
        let path = crate::dat::rename_apply::journal_path(&self.journal(), &self.tx.transaction_id)
            .unwrap();
        self.tx = crate::dat::rename_apply::read_journal(&path).unwrap();
    }
    fn reconcile(&mut self) -> Vec<crate::dat::rename_apply::reconcile::RecoveryIssue> {
        let journal = self.journal();
        crate::dat::rename_apply::reconcile::reconcile_recovery(&mut self.tx, &journal).unwrap()
    }
    fn fault(&self) {
        self.sql().execute_batch("CREATE TRIGGER fail_move BEFORE UPDATE OF absolute_path_cached ON archives BEGIN SELECT RAISE(ABORT,'fixture database failure'); END;").unwrap();
    }
    fn clear_fault(&self) {
        self.sql().execute_batch("DROP TRIGGER fail_move;").unwrap();
    }
    // All columns except the four path associations must survive byte for byte.
    fn metadata(&self, id: i64) -> Vec<String> {
        let c = self.sql();
        let mut stmt = c.prepare("SELECT * FROM archives WHERE id=?1").unwrap();
        let columns: Vec<_> = stmt.column_names().iter().map(|n| n.to_string()).collect();
        stmt.query_row([id], |r| {
            Ok(columns
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    ![
                        "source_folder_id",
                        "relative_path",
                        "absolute_path_cached",
                        "file_name_cached",
                    ]
                    .contains(&n.as_str())
                })
                .map(|(i, _)| format!("{:?}", r.get_ref(i).unwrap()))
                .collect())
        })
        .unwrap()
    }
}

#[test]
fn apply_and_undo_rebind_same_game_and_preserve_metadata() {
    for mode in [
        OrganisationMode::RenameInPlace,
        OrganisationMode::MoveRealFile,
    ] {
        let mut f = Fixture::new(mode);
        let before = f.metadata(f.id);
        let unrelated = f.metadata(f.unrelated);
        let result = f.apply().unwrap();
        assert_eq!(result.transaction.state, TransactionState::Applied);
        assert!(f.destination.exists());
        assert!(!f.source.exists());
        assert_eq!(f.path_id(&f.destination), Some(f.id));
        assert_eq!(f.path_id(&f.source), None);
        assert_eq!(f.metadata(f.id), before);
        assert_eq!(f.metadata(f.unrelated), unrelated);
        f.reload();
        assert_eq!(
            f.undo().unwrap().rollback.result,
            crate::dat::rename_apply::RollbackResult::FullyRolledBack
        );
        assert!(f.source.exists());
        assert!(!f.destination.exists());
        assert_eq!(f.path_id(&f.source), Some(f.id));
        assert_eq!(f.path_id(&f.destination), None);
        assert_eq!(f.metadata(f.id), before);
        assert_eq!(f.metadata(f.unrelated), unrelated);
        f.undo().unwrap();
    }
}

#[test]
fn replacement_after_preview_and_legacy_preview_are_refused() {
    let f = Fixture::new(OrganisationMode::RenameInPlace);
    std::fs::rename(&f.source, f.temp.path().join("original")).unwrap();
    std::fs::write(&f.source, b"replacement").unwrap();
    assert!(
        build_organisation_transaction(&f.plan, &f.approved, 1)
            .unwrap_err()
            .contains("identity changed")
    );
    let mut plan = f.plan.clone();
    plan.entries[0].source_identity = None;
    assert!(
        build_organisation_transaction(&plan, &f.approved, 1)
            .unwrap_err()
            .contains("no source identity")
    );
}

#[test]
fn stale_source_and_destination_collisions_refuse_without_moves() {
    for fault in 0..4 {
        let mut f = Fixture::new(OrganisationMode::RenameInPlace);
        match fault {
            0 => {
                f.sql()
                    .execute(
                        "UPDATE archives SET absolute_path_cached=?1 WHERE id=?2",
                        params![b"/stale".to_vec(), f.id],
                    )
                    .unwrap();
            }
            1 => {
                std::fs::write(&f.destination, b"occupied").unwrap();
            }
            2 => {
                f.sql()
                    .execute(
                        "UPDATE archives SET absolute_path_cached=?1 WHERE id=?2",
                        params![f.destination.as_os_str().as_encoded_bytes(), f.unrelated],
                    )
                    .unwrap();
            }
            _ => {
                std::fs::write(&f.source, b"changed after build").unwrap();
            }
        }
        let result = f.apply();
        assert!(result.is_err(), "fault {fault}");
        assert!(f.source.exists());
        if fault != 1 {
            assert!(!f.destination.exists());
        }
    }
}

#[test]
fn failed_database_apply_is_in_flight_and_recovers_after_restart() {
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.fault();
    assert!(f.apply().is_err());
    assert!(f.destination.exists());
    assert_eq!(f.path_id(&f.source), Some(f.id));
    f.reload();
    assert_eq!(f.tx.entries[0].state, EntryState::Applying);
    let issues = f.reconcile();
    assert!(issues.iter().any(|i| i.detail.contains("catalogue")));
    assert_eq!(f.tx.entries[0].state, EntryState::Applying);
    f.clear_fault();
    f.reload();
    f.reconcile();
    assert_eq!(f.tx.state, TransactionState::Applied);
    assert_eq!(f.path_id(&f.destination), Some(f.id));
    f.undo().unwrap();
    assert_eq!(f.path_id(&f.source), Some(f.id));
}

#[test]
fn failed_database_undo_is_in_flight_and_generic_undo_recovers() {
    let mut f = Fixture::new(OrganisationMode::MoveRealFile);
    f.apply().unwrap();
    f.fault();
    assert!(f.undo().is_err());
    assert!(f.source.exists());
    assert_eq!(f.path_id(&f.destination), Some(f.id));
    f.reload();
    assert_eq!(f.tx.entries[0].state, EntryState::RollingBack);
    f.clear_fault();
    let journal = f.journal();
    let outcome =
        crate::dat::rename_apply::rollback_transaction(&mut f.tx, &journal, &no_cancel()).unwrap();
    assert_eq!(
        outcome.result,
        crate::dat::rename_apply::RollbackResult::FullyRolledBack
    );
    assert_eq!(f.path_id(&f.source), Some(f.id));
}

#[test]
fn undo_stale_association_and_recreated_source_refuse() {
    for db_change in [false, true] {
        let mut f = Fixture::new(OrganisationMode::RenameInPlace);
        f.apply().unwrap();
        if db_change {
            f.sql()
                .execute(
                    "UPDATE archives SET absolute_path_cached=?1 WHERE id=?2",
                    params![b"/changed".to_vec(), f.id],
                )
                .unwrap();
        } else {
            std::fs::write(&f.source, b"new unrelated file").unwrap();
        }
        let result = f.undo();
        assert!(
            result.is_err()
                || result.unwrap().rollback.result
                    != crate::dat::rename_apply::RollbackResult::FullyRolledBack
        );
        assert!(f.destination.exists());
    }
}

#[test]
fn recovery_accepts_committed_db_before_terminal_journal() {
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.apply().unwrap();
    f.tx.state = TransactionState::Applying;
    f.tx.entries[0].state = EntryState::Applying;
    crate::dat::rename_apply::write_journal(&f.journal(), &f.tx).unwrap();
    f.reload();
    f.reconcile();
    assert_eq!(f.tx.state, TransactionState::Applied);
    assert_eq!(f.path_id(&f.destination), Some(f.id));
}

#[test]
fn missing_or_replaced_database_is_never_created_or_adopted() {
    for replace in [false, true] {
        let mut f = Fixture::new(OrganisationMode::RenameInPlace);
        f.apply().unwrap();
        let path = f.db.path().to_path_buf();
        std::fs::rename(&path, f.temp.path().join("preserved.sqlite3")).unwrap();
        if replace {
            std::fs::write(&path, b"unrelated database object").unwrap();
        }
        assert!(f.undo().is_err());
        assert!(f.destination.exists());
        if replace {
            assert_eq!(std::fs::read(&path).unwrap(), b"unrelated database object");
        } else {
            assert!(!path.exists());
        }
    }
}

#[test]
fn linked_library_keeps_original_catalogue_association() {
    let mut f = Fixture::new(OrganisationMode::BuildLinkedLibrary);
    f.apply().unwrap();
    assert_eq!(f.path_id(&f.source), Some(f.id));
    assert_eq!(f.path_id(&f.destination), None);
    f.undo().unwrap();
    assert!(f.source.exists());
    assert_eq!(f.path_id(&f.source), Some(f.id));
}

#[test]
fn catalogue_game_replaced_after_preview_and_stale_metadata_refuse() {
    for replace_id in [false, true] {
        let mut f = Fixture::new(OrganisationMode::RenameInPlace);
        if replace_id {
            f.sql()
                .execute("UPDATE archives SET id=9001 WHERE id=?1", [f.id])
                .unwrap();
        } else {
            f.sql()
                .execute("UPDATE archives SET size_bytes=99999 WHERE id=?1", [f.id])
                .unwrap();
        }
        assert!(f.apply().is_err());
        assert!(f.source.exists());
        assert!(!f.destination.exists());
    }
}

#[test]
fn actual_commit_failure_keeps_every_catalogue_update_uncommitted() {
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.sql().execute_batch("CREATE TABLE fixture_deferred (archive_id INTEGER REFERENCES archives(id) DEFERRABLE INITIALLY DEFERRED);
        CREATE TRIGGER fixture_commit_failure AFTER UPDATE OF absolute_path_cached ON archives BEGIN INSERT INTO fixture_deferred VALUES(-999); END;").unwrap();
    let error = f.apply().unwrap_err().to_string();
    assert!(error.contains("commit failed"), "{error}");
    assert!(f.destination.exists());
    assert_eq!(f.path_id(&f.source), Some(f.id));
    f.reload();
    assert_eq!(f.tx.entries[0].state, EntryState::Applying);
    f.sql()
        .execute_batch("DROP TRIGGER fixture_commit_failure")
        .unwrap();
    f.reconcile();
    assert_eq!(f.path_id(&f.destination), Some(f.id));
}

#[test]
fn interrupted_intent_without_filesystem_mutation_keeps_source_association() {
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.tx.state = TransactionState::Applying;
    f.tx.entries[0].state = EntryState::Applying;
    crate::dat::rename_apply::write_journal(&f.journal(), &f.tx).unwrap();
    f.reload();
    f.reconcile();
    assert_eq!(f.path_id(&f.source), Some(f.id));
    assert!(f.source.exists());
    assert!(!f.destination.exists());
}

#[test]
fn catalogue_binding_cannot_enter_protocol_without_catalogue_completion() {
    let f = Fixture::new(OrganisationMode::RenameInPlace);
    let bytes = serde_json::to_vec(&f.tx).unwrap();
    assert!(matches!(
        crate::dat::rename_apply::compact_recovery::Authority::decode_legacy(&bytes, 1024 * 1024),
        Err(crate::dat::rename_apply::compact_recovery::Error::Binding)
    ));
    assert!(f.source.exists());
    assert!(!f.destination.exists());
    assert!(!f.journal().exists());
}

#[test]
fn cross_source_folder_apply_and_undo_restore_exact_associations() {
    let mut f = Fixture::new(OrganisationMode::MoveRealFile);
    let master = f.temp.path().join("organised");
    std::fs::create_dir(&master).unwrap();
    f.db.register_source_folders(&[f.plan.master_root.clone(), master.clone()])
        .unwrap();
    f.plan.master_root = master.clone();
    f.plan.entries[0].destination_path = master.join("Sony PlayStation Portable/New.zip");
    f.plan.entries[0].catalogue_binding = None;
    prepare_organisation_catalogue(&mut f.plan, &f.db).unwrap();
    f.tx = build_organisation_transaction(&f.plan, &f.approved, 1).unwrap();
    f.destination = f.tx.entries[0].destination_path.clone();
    let old: (i64, Vec<u8>) = f
        .sql()
        .query_row(
            "SELECT source_folder_id,relative_path FROM archives WHERE id=?1",
            [f.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let trusted = TrustedRoots::from_paths([f.temp.path().to_path_buf()]);
    let journal = f.journal();
    super::super::transaction::apply_organisation_transaction(
        &mut f.tx,
        &f.approved,
        1,
        trusted,
        &journal,
        &no_cancel(),
        f.plan.mode,
        &master,
        &f.db,
    )
    .unwrap();
    let new: (i64, Vec<u8>) = f
        .sql()
        .query_row(
            "SELECT source_folder_id,relative_path FROM archives WHERE id=?1",
            [f.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_ne!(old.0, new.0);
    assert_eq!(new.1, b"Sony PlayStation Portable/New.zip");
    f.undo().unwrap();
    let restored: (i64, Vec<u8>) = f
        .sql()
        .query_row(
            "SELECT source_folder_id,relative_path FROM archives WHERE id=?1",
            [f.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(restored, old);
}

#[test]
fn fingerprints_and_owned_audits_follow_identity_but_scan_history_does_not() {
    let mut f = Fixture::new(OrganisationMode::MoveRealFile);
    let c = f.sql();
    c.execute("INSERT INTO scan_fingerprints(source_folder_id,relative_path,size_bytes,modified_time_ns,archive_kind,display_name,normalized_name,scanner_version,parser_version,cache_version,updated_at) SELECT source_folder_id,relative_path,size_bytes,123,archive_kind,'preserved display','preserved normalized','scanner','parser',1,'original-time' FROM archives WHERE id=?1",[f.id]).unwrap();
    c.execute("INSERT INTO dat_set_audit_results(archive_id,archive_path,source_id,game_name,set_state_json,dependency_state_json,audited_at) SELECT id,absolute_path_cached,'dat-fixture','game','{}','{}','original-audit' FROM archives WHERE id=?1",[f.id]).unwrap();
    c.execute_batch(
        "INSERT INTO scan_runs(started_at,triggered_by) VALUES('original-scan','fixture');",
    )
    .unwrap();
    c.execute("INSERT INTO discovery_details(scan_run_id,path,container,validation_state,explanation) VALUES(last_insert_rowid(),?1,'zip','original-state','original-evidence')",[f.source.as_os_str().as_encoded_bytes()]).unwrap();
    f.plan.entries[0].catalogue_binding = None;
    prepare_organisation_catalogue(&mut f.plan, &f.db).unwrap();
    f.tx = build_organisation_transaction(&f.plan, &f.approved, 1).unwrap();
    let rows = || {
        (c.query_row("SELECT display_name,normalized_name,modified_time_ns,updated_at FROM scan_fingerprints",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?))).unwrap(),
         c.query_row("SELECT archive_id,source_id,game_name,set_state_json,dependency_state_json,audited_at FROM dat_set_audit_results",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?))).unwrap(),
         c.query_row("SELECT path FROM discovery_details",[],|r|r.get::<_,Vec<u8>>(0)).unwrap())
    };
    let before = rows();
    f.apply().unwrap();
    assert_eq!(rows(), before);
    let audit_path: Vec<u8> = c
        .query_row("SELECT archive_path FROM dat_set_audit_results", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(audit_path, f.destination.as_os_str().as_encoded_bytes());
    let relative: Vec<u8> = c
        .query_row("SELECT relative_path FROM scan_fingerprints", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(relative, b"Sony PlayStation Portable/New.zip");
    f.undo().unwrap();
    assert_eq!(rows(), before);
    let audit_path: Vec<u8> = c
        .query_row("SELECT archive_path FROM dat_set_audit_results", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(audit_path, f.source.as_os_str().as_encoded_bytes());
}

#[test]
fn unrelated_audit_and_fingerprint_collisions_are_never_rewritten() {
    for audit in [false, true] {
        let mut f = Fixture::new(OrganisationMode::RenameInPlace);
        let c = f.sql();
        if audit {
            c.execute("INSERT INTO dat_set_audit_results(archive_id,archive_path,source_id,game_name,set_state_json,dependency_state_json,audited_at) VALUES(?1,?2,'unrelated','unrelated','{}','{}','original')",params![f.unrelated,f.source.as_os_str().as_encoded_bytes()]).unwrap();
        } else {
            c.execute("INSERT INTO scan_fingerprints(source_folder_id,relative_path,size_bytes,modified_time_ns,archive_kind,display_name,normalized_name,scanner_version,parser_version,cache_version,updated_at) SELECT source_folder_id,?2,1,1,archive_kind,'unrelated','unrelated','scanner','parser',1,'original' FROM archives WHERE id=?1",params![f.id,b"incoming/New.zip".to_vec()]).unwrap();
        }
        let before: String = c
            .query_row(
                if audit {
                    "SELECT quote(archive_path)||game_name||audited_at FROM dat_set_audit_results"
                } else {
                    "SELECT quote(relative_path)||display_name||updated_at FROM scan_fingerprints"
                },
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(f.apply().is_err());
        assert!(f.source.exists());
        assert!(!f.destination.exists());
        let after: String = c
            .query_row(
                if audit {
                    "SELECT quote(archive_path)||game_name||audited_at FROM dat_set_audit_results"
                } else {
                    "SELECT quote(relative_path)||display_name||updated_at FROM scan_fingerprints"
                },
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
    }
}

#[test]
fn backing_replacement_during_mutation_cannot_commit_catalogue_success() {
    for database in [false, true] {
        let mut f = Fixture::new(OrganisationMode::RenameInPlace);
        f.tx.state = TransactionState::Applying;
        f.tx.entries[0].state = EntryState::Applying;
        crate::dat::rename_apply::write_journal(&f.journal(), &f.tx).unwrap();
        let guard = crate::database::organisation_paths::begin(&f.tx.entries[0], false, false)
            .unwrap()
            .unwrap();
        // Disposable simulation of the mutation window while the SQLite
        // writer lock is held; namespace renames are not SQLite writers.
        std::fs::rename(&f.source, &f.destination).unwrap();
        if database {
            let path = f.db.path().to_path_buf();
            std::fs::rename(&path, f.temp.path().join("preserved-db")).unwrap();
            std::fs::write(&path, b"replacement database object").unwrap();
        } else {
            std::fs::rename(&f.plan.master_root, f.temp.path().join("preserved-games")).unwrap();
            std::fs::create_dir(&f.plan.master_root).unwrap();
        }
        let error = guard.commit().unwrap_err();
        assert!(error.contains("identity changed"), "{error}");
        assert_eq!(f.path_id(&f.source), Some(f.id));
        f.reload();
        assert_eq!(f.tx.entries[0].state, EntryState::Applying);
    }
}

#[test]
fn changed_expected_destination_is_refused_before_apply() {
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.tx.entries[0].destination_path = f.source.with_file_name("Unreviewed.zip");
    assert!(f.apply().unwrap_err().to_string().contains("sealed move"));
    assert!(f.source.exists());
    assert!(!f.destination.exists());
}

// --- History's Undo by transaction id -------------------------------------

fn journal_file(f: &Fixture) -> PathBuf {
    crate::dat::rename_apply::journal_path(&f.journal(), &f.tx.transaction_id).unwrap()
}

#[test]
fn history_selection_opens_the_exact_sealed_journal_after_a_restart_and_undoes_it() {
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.apply().unwrap();
    f.reload(); // restart: History holds the record read from disk
    let history = f.tx.clone();
    let mut opened = open_history_transaction_for_undo(&f.journal(), &history).unwrap();
    assert_eq!(opened.transaction_id, history.transaction_id);
    assert!(is_organisation_transaction(&opened));
    assert!(undo_blocker(&opened).is_none());
    let outcome = rollback_organisation_transaction(
        &mut opened,
        &f.journal(),
        &no_cancel(),
        &f.plan.master_root,
    )
    .unwrap();
    assert_eq!(
        outcome.rollback.result,
        crate::dat::rename_apply::RollbackResult::FullyRolledBack
    );
    assert!(f.source.exists() && !f.destination.exists());
    assert_eq!(f.path_id(&f.source), Some(f.id));
}

#[test]
fn history_selection_refuses_a_wrong_or_unknown_id_and_a_mismatched_record() {
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.apply().unwrap();
    f.reload();
    let mut other_id = f.tx.clone();
    other_id.transaction_id = crate::dat::rename_apply::journal::new_transaction_id(42);
    assert!(open_history_transaction_for_undo(&f.journal(), &other_id).is_err());
    let mut unsafe_id = f.tx.clone();
    unsafe_id.transaction_id = "../escape".into();
    assert!(open_history_transaction_for_undo(&f.journal(), &unsafe_id).is_err());
    // History's record disagrees with the journal on disk.
    let mut stale = f.tx.clone();
    stale.created_at_unix += 1;
    let error = open_history_transaction_for_undo(&f.journal(), &stale).unwrap_err();
    assert!(error.contains("no longer matches"), "{error}");
    assert!(f.destination.exists() && !f.source.exists());
}

#[test]
fn history_selection_refuses_a_journal_changed_after_it_was_sealed_and_undo_stays_refused() {
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.apply().unwrap();
    f.reload();
    let history = f.tx.clone();
    let path = journal_file(&f);
    let mut edited: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edited["created_at_unix"] = serde_json::json!(1);
    std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
    assert!(open_history_transaction_for_undo(&f.journal(), &history).is_err());
    // The shared Undo applies its own seal gate to the same edited journal.
    let mut held = history.clone();
    let error = rollback_organisation_transaction(
        &mut held,
        &f.journal(),
        &no_cancel(),
        &f.plan.master_root,
    )
    .unwrap_err();
    assert!(
        crate::dat::rename_apply::is_journal_integrity_loss(&error),
        "{error}"
    );
    assert!(f.destination.exists() && !f.source.exists());
}

#[test]
fn history_selection_refuses_a_journal_replaced_by_a_symlink() {
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.apply().unwrap();
    f.reload();
    let history = f.tx.clone();
    let path = journal_file(&f);
    let copy = f.temp.path().join("copy.json");
    std::fs::copy(&path, &copy).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&copy, &path).unwrap();
    assert!(open_history_transaction_for_undo(&f.journal(), &history).is_err());
}

#[test]
fn history_selection_refuses_foreign_and_unsupported_transactions() {
    // A linked library (symlink objects) is not an Organisation rename/move.
    let mut linked = Fixture::new(OrganisationMode::BuildLinkedLibrary);
    linked.apply().unwrap();
    linked.reload();
    let error = open_history_transaction_for_undo(&linked.journal(), &linked.tx).unwrap_err();
    assert!(error.contains("not a ROM Organisation"), "{error}");

    // A plain journalled rename with no catalogue binding is foreign.
    let mut f = Fixture::new(OrganisationMode::RenameInPlace);
    f.apply().unwrap();
    f.reload();
    let mut foreign = f.tx.clone();
    for entry in &mut foreign.entries {
        entry.unknown.clear();
    }
    assert!(!is_organisation_transaction(&foreign));
    let mut workflow = f.tx.clone();
    workflow.unknown.insert(
        "workflow".into(),
        serde_json::json!("mame_merged_reconstruction"),
    );
    assert!(!is_organisation_transaction(&workflow));
    let mut copy = f.tx.clone();
    copy.entries[0].operation = crate::dat::rename_apply::model::TransactionOperation::CreateCopy {
        expected_source: copy.entries[0].source_path.clone(),
        destination_root: f.plan.master_root.clone(),
    };
    assert!(!is_organisation_transaction(&copy));
    assert!(!is_organisation_transaction(&RenameTransaction {
        entries: Vec::new(),
        ..f.tx.clone()
    }));
}

#[test]
fn history_selection_of_a_partial_undo_stays_actionable_and_a_finished_one_is_explained() {
    let mut f = Fixture::new(OrganisationMode::MoveRealFile);
    f.apply().unwrap();
    f.fault();
    assert!(f.undo().is_err());
    f.reload();
    assert_eq!(f.tx.entries[0].state, EntryState::RollingBack);
    let reopened = open_history_transaction_for_undo(&f.journal(), &f.tx).unwrap();
    assert!(
        undo_blocker(&reopened).is_none(),
        "an interrupted Undo must stay retryable"
    );
    f.clear_fault();
    let mut retry = reopened;
    rollback_organisation_transaction(&mut retry, &f.journal(), &no_cancel(), &f.plan.master_root)
        .unwrap();
    f.reload();
    let finished = open_history_transaction_for_undo(&f.journal(), &f.tx).unwrap();
    let reason = undo_blocker(&finished).expect("nothing left to undo");
    assert!(reason.contains("already undone"), "{reason}");
}
