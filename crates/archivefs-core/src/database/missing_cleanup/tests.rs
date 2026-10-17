use super::*;

fn by_ids(ids: &[i64]) -> Vec<MissingCleanupTarget> {
    ids.iter()
        .copied()
        .map(MissingCleanupTarget::by_id)
        .collect()
}

struct Fixture {
    _temp: tempfile::TempDir,
    db: Database,
    root: PathBuf,
    paths: Vec<PathBuf>,
    ids: Vec<i64>,
}
impl Fixture {
    fn missing() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        fs::create_dir(&root).unwrap();
        let paths: Vec<_> = ["a.zip", "b.zip"]
            .into_iter()
            .map(|n| root.join(n))
            .collect();
        for path in &paths {
            fs::write(path, b"synthetic game").unwrap();
        }
        let config = Config {
            source_folders: vec![root.clone()],
            mount_root: temp.path().join("mount"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        };
        let mut db = Database::open_or_create(temp.path().join("synthetic.sqlite3")).unwrap();
        super::super::scan_and_persist(&mut db, &config, "present").unwrap();
        let ids = db.load_archives().unwrap().iter().map(|a| a.id).collect();
        for path in &paths {
            fs::remove_file(path).unwrap();
        }
        super::super::scan_and_persist(&mut db, &config, "missing").unwrap();
        for id in &ids {
            db.connection.execute("INSERT INTO library_dat_identities (archive_id,dat_source_id,verification_state,completeness,audited_at,facts_json,created_at,updated_at) VALUES(?1,'synthetic','verified_single_match','exhaustive','now',X'7B7D','now','now')", [id]).unwrap();
        }
        Self {
            _temp: temp,
            db,
            root,
            paths,
            ids,
        }
    }
    fn snapshot(&self) -> (Vec<PersistedArchive>, Vec<i64>) {
        let counts = [
            "archives",
            "platform_assignments",
            "archive_scan_observations",
            "library_dat_identities",
            "verified_identity_facts",
        ]
        .into_iter()
        .map(|t| {
            self.db
                .connection
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap()
        })
        .collect();
        (self.db.load_archives().unwrap(), counts)
    }
    fn refusal(&mut self, expected: &str) {
        let before = self.snapshot();
        let error = self
            .db
            .remove_missing_archives(&self.ids)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
        assert_eq!(self.snapshot(), before);
    }
}

#[test]
fn returned_file_refuses_entire_selection_and_preserves_identity() {
    let mut f = Fixture::missing();
    fs::write(&f.paths[1], b"returned").unwrap();
    f.refusal("present on storage");
    assert_eq!(fs::read(&f.paths[1]).unwrap(), b"returned");
}

#[test]
fn unavailable_storage_is_not_absence() {
    let mut f = Fixture::missing();
    fs::rename(&f.root, f.root.with_extension("offline")).unwrap();
    f.refusal("unavailable");
}

#[test]
fn replaced_root_requires_reviewed_rebind_and_new_scan() {
    let mut f = Fixture::missing();
    fs::rename(&f.root, f.root.with_extension("old")).unwrap();
    fs::create_dir(&f.root).unwrap();
    f.refusal("binding changed");
}

#[test]
fn symlink_root_is_not_authoritative() {
    let mut f = Fixture::missing();
    let old = f.root.with_extension("old");
    fs::rename(&f.root, &old).unwrap();
    std::os::unix::fs::symlink(&old, &f.root).unwrap();
    f.refusal("unavailable, unreadable or replaced");
}

#[test]
fn stale_generation_refuses_cleanup() {
    let mut f = Fixture::missing();
    f.db.connection
        .execute(
            "UPDATE source_scan_bindings SET generation=generation+1",
            [],
        )
        .unwrap();
    f.refusal("generation changed");
}

#[test]
fn incomplete_latest_attempt_refuses_cleanup() {
    let mut f = Fixture::missing();
    f.db.connection.execute("UPDATE scan_source_coverage SET state='\"partial\"' WHERE scan_run_id=(SELECT MAX(scan_run_id) FROM scan_source_coverage)", []).unwrap();
    f.refusal("not complete");
}

#[test]
fn missing_binding_refuses_legacy_historical_evidence() {
    let mut f = Fixture::missing();
    f.db.connection
        .execute("DELETE FROM source_scan_bindings", [])
        .unwrap();
    f.refusal("requires reviewed rebind");
}

#[test]
fn valid_current_absence_removes_all_reviewed_rows_only() {
    let mut f = Fixture::missing();
    let result = f.db.remove_missing_archives(&f.ids).unwrap();
    assert_eq!(result.removed, 2);
    assert_eq!(f.snapshot().1, vec![0, 0, 0, 0, 0]);
    assert!(f.root.is_dir());
    assert!(f.paths.iter().all(|p| !p.exists()));
}

#[test]
fn file_returning_after_review_or_after_deletion_rolls_back() {
    for stage in [Boundary::Reviewed, Boundary::Deleted] {
        let mut f = Fixture::missing();
        let before = f.snapshot();
        let path = f.paths[1].clone();
        let error = remove_with_hooks(
            &mut f.db,
            &by_ids(&f.ids),
            |r, p, d| r.probe(p, d).probe,
            |b, _| {
                if b == stage {
                    fs::write(&path, b"returned at boundary").unwrap();
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("present on storage"));
        assert_eq!(f.snapshot(), before);
        assert_eq!(fs::read(path).unwrap(), b"returned at boundary");
    }
}

#[test]
fn root_replaced_at_mutation_boundary_refuses() {
    let mut f = Fixture::missing();
    let before = f.snapshot();
    let root = f.root.clone();
    let error = remove_with_hooks(
        &mut f.db,
        &by_ids(&f.ids),
        |r, p, d| r.probe(p, d).probe,
        |stage, _| {
            if stage == Boundary::Reviewed {
                fs::rename(&root, root.with_extension("old")).unwrap();
                fs::create_dir(&root).unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("replaced"));
    assert_eq!(f.snapshot(), before);
}

#[test]
fn permission_and_io_probe_failures_never_authorise_cleanup() {
    for fault in [FsProbe::Inaccessible, FsProbe::IoError] {
        let mut f = Fixture::missing();
        let before = f.snapshot();
        let target = f.paths[1].clone();
        let error = remove_with_hooks(
            &mut f.db,
            &by_ids(&f.ids),
            |r, p, d| {
                if p == target {
                    fault
                } else {
                    r.probe(p, d).probe
                }
            },
            |_, _| Ok(()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot be safely observed"));
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn interrupted_after_deletion_rolls_back_all_metadata() {
    let mut f = Fixture::missing();
    let before = f.snapshot();
    let error = remove_with_hooks(
        &mut f.db,
        &by_ids(&f.ids),
        |r, p, d| r.probe(p, d).probe,
        |stage, _| {
            if stage == Boundary::Deleted {
                Err(refused("synthetic interruption"))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("synthetic interruption"));
    assert_eq!(f.snapshot(), before);
}

#[test]
fn writer_reservation_blocks_competing_generation_changes() {
    let mut f = Fixture::missing();
    let other = Connection::open(f.db.path()).unwrap();
    other.busy_timeout(Duration::ZERO).unwrap();
    remove_with_hooks(&mut f.db, &by_ids(&f.ids), |r,p,d| r.probe(p,d).probe, |stage,_| {
        if stage == Boundary::Reviewed {
            let error = other.execute("UPDATE source_scan_bindings SET generation=generation+1", []).unwrap_err();
            assert!(matches!(error, rusqlite::Error::SqliteFailure(e,_) if e.code == ErrorCode::DatabaseBusy));
        }
        Ok(())
    }).unwrap();
}

#[test]
fn interrupted_observation_run_cannot_authorise_removal() {
    let mut f = Fixture::missing();
    f.db.connection
        .execute(
            "UPDATE scan_runs SET status='interrupted' WHERE id=(SELECT MAX(id) FROM scan_runs)",
            [],
        )
        .unwrap();
    f.refusal("not successfully finished");
}

#[test]
fn changed_source_ownership_refuses_removal() {
    let mut f = Fixture::missing();
    let child = f.root.join("delegated");
    fs::create_dir(&child).unwrap();
    f.db.register_source_folders(&[f.root.clone(), child])
        .unwrap();
    f.refusal("source ownership changed");
}

#[test]
fn generation_changed_inside_transaction_rolls_back_deleted_rows() {
    let mut f = Fixture::missing();
    let before = f.snapshot();
    let error = remove_with_hooks(
        &mut f.db,
        &by_ids(&f.ids),
        |r, p, d| r.probe(p, d).probe,
        |stage, tx| {
            if stage == Boundary::Deleted {
                tx.execute(
                    "UPDATE source_scan_bindings SET generation=generation+1",
                    [],
                )
                .unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("generation changed"));
    assert_eq!(f.snapshot(), before);
}

// ---- identity binding, verified deletion and audit-evidence policy ----

fn audit_row(db: &Database, archive_id: Option<i64>, path: &str) {
    db.connection
        .execute(
            "INSERT INTO dat_set_audit_results (archive_id,archive_path,source_id,game_name,set_state_json,dependency_state_json,audited_at) \
             VALUES(?1,?2,'arcade-dat','game','\"incomplete\"','\"missing\"','2026-01-02')",
            params![archive_id, path.as_bytes()],
        )
        .unwrap();
}

fn audit_rows(db: &Database) -> Vec<(Option<i64>, Vec<u8>)> {
    let mut statement = db
        .connection
        .prepare("SELECT archive_id,archive_path FROM dat_set_audit_results ORDER BY id")
        .unwrap();
    statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

impl Fixture {
    fn database_path(&self) -> PathBuf {
        self._temp.path().join("synthetic.sqlite3")
    }
    fn config(&self) -> Config {
        Config {
            source_folders: vec![self.root.clone()],
            mount_root: self._temp.path().join("mount"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        }
    }
}

/// The review's two-connection scenario. The user selects `b.zip` (id 2, the
/// highest id). Another connection removes that record; a later scan catalogues
/// `different.zip`, which SQLite gives the REUSED id 2, and then it goes missing
/// too. A stale selection must not be allowed to remove `different.zip`.
fn reuse_id_two() -> (Fixture, PersistedArchive, PathBuf) {
    let mut f = Fixture::missing();
    let selected =
        f.db.load_archives()
            .unwrap()
            .into_iter()
            .find(|a| a.absolute_path == f.paths[1])
            .unwrap();
    assert_eq!(selected.id, 2, "fixture must give b.zip the highest id");
    let config = f.config();
    let mut other = Database::open_or_create(f.database_path()).unwrap();
    other.remove_missing_archives(&[selected.id]).unwrap();
    let different = f.root.join("different.zip");
    fs::write(&different, b"another game").unwrap();
    super::super::scan_and_persist(&mut other, &config, "new").unwrap();
    fs::remove_file(&different).unwrap();
    super::super::scan_and_persist(&mut other, &config, "gone-again").unwrap();
    other.close().unwrap();
    let reused =
        f.db.load_archives()
            .unwrap()
            .into_iter()
            .find(|a| a.absolute_path == different)
            .unwrap();
    assert_eq!(reused.id, selected.id, "SQLite must have reused the id");
    assert!(reused.last_verified_missing_at.is_some());
    f.paths[1] = selected.absolute_path.clone();
    (f, selected, different)
}

#[test]
fn a_reused_archive_id_never_authorises_removing_a_different_record() {
    let (mut f, selected, different) = reuse_id_two();
    let before = f.snapshot();
    for (label, target) in [
        (
            "exact snapshot identity",
            MissingCleanupTarget::exact(&selected),
        ),
        (
            "path",
            MissingCleanupTarget::by_path(&selected.absolute_path),
        ),
        (
            "id and path",
            MissingCleanupTarget {
                archive_id: Some(selected.id),
                absolute_path: Some(selected.absolute_path.clone()),
                ..Default::default()
            },
        ),
    ] {
        let error =
            f.db.remove_missing_archive_targets(&[target])
                .unwrap_err()
                .to_string();
        assert!(
            error.contains("no catalogue record exists at that path")
                || error.contains("stale")
                || error.contains("no longer matches"),
            "{label}: {error}"
        );
        assert_eq!(f.snapshot(), before, "{label}: catalogue changed");
    }
    // different.zip survives, unchanged, with its row and id.
    assert!(
        f.db.load_archives()
            .unwrap()
            .iter()
            .any(|a| a.absolute_path == different && a.id == selected.id)
    );
}

#[test]
fn a_changed_source_or_relative_path_association_is_refused() {
    let mut f = Fixture::missing();
    let a = f.db.load_archives().unwrap().remove(0);
    let before = f.snapshot();
    for target in [
        MissingCleanupTarget {
            source_folder_id: Some(a.source_folder_id + 100),
            ..MissingCleanupTarget::exact(&a)
        },
        MissingCleanupTarget {
            relative_path: Some(PathBuf::from("elsewhere.zip")),
            ..MissingCleanupTarget::exact(&a)
        },
        MissingCleanupTarget {
            absolute_path: None,
            relative_path: Some(PathBuf::from("elsewhere.zip")),
            ..MissingCleanupTarget::by_id(a.id)
        },
    ] {
        let error = f.db.remove_missing_archive_targets(&[target]).unwrap_err();
        assert!(error.to_string().contains("stale"), "{error}");
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn a_mixed_selection_with_one_stale_member_removes_nothing() {
    let (mut f, selected, _different) = reuse_id_two();
    let a =
        f.db.load_archives()
            .unwrap()
            .into_iter()
            .find(|a| a.absolute_path == f.paths[0])
            .unwrap();
    let before = f.snapshot();
    let error =
        f.db.remove_missing_archive_targets(&[
            MissingCleanupTarget::exact(&a),
            MissingCleanupTarget::exact(&selected),
        ])
        .unwrap_err();
    assert!(error.to_string().contains("refused"), "{error}");
    assert_eq!(f.snapshot(), before, "the valid member must survive too");
}

#[test]
fn exact_and_path_selections_that_are_still_current_are_removed() {
    let mut f = Fixture::missing();
    let all = f.db.load_archives().unwrap();
    let result =
        f.db.remove_missing_archive_targets(&[
            MissingCleanupTarget::exact(&all[0]),
            MissingCleanupTarget::by_path(&all[1].absolute_path),
            MissingCleanupTarget::by_id(all[0].id), // duplicate of the first
        ])
        .unwrap();
    assert_eq!((result.requested, result.removed), (2, 2));
    assert!(f.db.load_archives().unwrap().is_empty());
}

#[test]
fn a_delete_suppressed_by_a_trigger_rolls_everything_back() {
    let mut f = Fixture::missing();
    f.db.connection
        .execute_batch(
            "CREATE TRIGGER suppress_archive_delete BEFORE DELETE ON archives \
             WHEN old.id = 2 BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
    let before = f.snapshot();
    let error =
        f.db.remove_missing_archives(&f.ids)
            .unwrap_err()
            .to_string();
    assert!(error.contains("suppressed or widened"), "{error}");
    assert!(
        error.contains("no catalogue entries were removed"),
        "must say nothing was removed: {error}"
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn a_child_delete_suppressed_by_a_trigger_rolls_everything_back() {
    let mut f = Fixture::missing();
    f.db.connection
        .execute_batch(
            "CREATE TRIGGER suppress_identity_delete BEFORE DELETE ON library_dat_identities \
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
    let before = f.snapshot();
    let error =
        f.db.remove_missing_archives(&f.ids)
            .unwrap_err()
            .to_string();
    assert!(
        error.contains("library_dat_identities") && error.contains("affected 0 of 1"),
        "{error}"
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn a_delete_that_also_removes_an_unselected_record_rolls_back() {
    let mut f = Fixture::missing();
    f.db.connection
        .execute_batch(
            "CREATE TRIGGER widen_delete AFTER DELETE ON archives WHEN old.id = 1 BEGIN \
               DELETE FROM library_dat_identities WHERE archive_id = 2; \
               DELETE FROM verified_identity_facts WHERE archive_id = 2; \
               DELETE FROM platform_assignments WHERE archive_id = 2; \
               DELETE FROM archive_scan_observations WHERE archive_id = 2; \
               DELETE FROM screenscraper_enrichments WHERE archive_id = 2; \
               DELETE FROM archives WHERE id = 2; END;",
        )
        .unwrap();
    let before = f.snapshot();
    let error = f.db.remove_missing_archives(&[f.ids[0]]).unwrap_err();
    assert!(
        error.to_string().contains("other than the selection")
            || error.to_string().contains("beyond"),
        "{error}"
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn persisted_arcade_audit_evidence_blocks_cleanup_and_is_never_discarded() {
    let mut f = Fixture::missing();
    audit_row(&f.db, Some(f.ids[0]), "/roms/a.zip");
    let before = f.snapshot();
    let error =
        f.db.remove_missing_archives(&f.ids)
            .unwrap_err()
            .to_string();
    assert!(error.contains("audit"), "{error}");
    assert!(error.contains("does not discard"), "{error}");
    assert_eq!(
        f.snapshot(),
        before,
        "the clean member must survive the mixed refusal"
    );
    assert_eq!(
        audit_rows(&f.db),
        vec![(Some(f.ids[0]), b"/roms/a.zip".to_vec())]
    );
    // The same selection through the identity-bound API is refused identically.
    let all = f.db.load_archives().unwrap();
    let targets: Vec<_> = all.iter().map(MissingCleanupTarget::exact).collect();
    assert!(f.db.remove_missing_archive_targets(&targets).is_err());
    assert_eq!(f.snapshot(), before);
}

#[test]
fn archives_without_audit_evidence_are_removed_and_other_audit_evidence_is_untouched() {
    let mut f = Fixture::missing();
    audit_row(&f.db, Some(f.ids[0]), "/roms/a.zip");
    // Evidence already detached from any archive (NULL link) does not block.
    audit_row(&f.db, None, "/roms/b.zip");
    let result = f.db.remove_missing_archives(&[f.ids[1]]).unwrap();
    assert_eq!(result.removed, 1);
    assert_eq!(
        audit_rows(&f.db),
        vec![
            (Some(f.ids[0]), b"/roms/a.zip".to_vec()),
            (None, b"/roms/b.zip".to_vec())
        ]
    );
    let remaining = f.db.load_archives().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, f.ids[0]);
}

#[test]
fn excessive_identity_totals_refuse_without_panicking_or_deleting() {
    let f = Fixture::missing();
    let before = f.snapshot();
    assert!(delete_verified(&f.db.connection, &[i64::MAX, 1]).is_err());
    assert_eq!(f.snapshot(), before);
}
