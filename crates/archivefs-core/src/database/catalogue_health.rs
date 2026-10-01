//! Coverage proofs and explicit presence-only reconciliation. No cleanup or relink.
use super::*;
use crate::catalogue_health::{
    BoundRoot, CatalogueHealthReport, NestedBoundaryObservation, NestedState, REBIND_REVIEW_AGAIN,
    RebindReason, ScanCoverageState, SourceHealth, SourceHealthState, SourceRebindReview,
    SourceRootBinding, SourceScanCoverage, source_root_identity,
};

impl Database {
    /// Presence preview also supports the immediately preceding schema. It
    /// reads no new table and never migrates the user's database.
    pub fn open_catalogue_health_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let connection = open_read_only_connection(path)?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| db_error("read catalogue preview schema", e))?;
        if !(21..=latest_known_version(MIGRATIONS)).contains(&version) {
            return Err(ArchiveFsError::Database(
                "unsupported catalogue preview schema; no migration performed".into(),
            ));
        }
        Ok(Self {
            connection,
            path: path.to_path_buf(),
        })
    }

    pub(crate) fn catalogue_health_epoch(&self) -> Result<Option<i64>> {
        if self.schema_version()? < 23 {
            return Ok(None);
        }
        self.connection
            .query_row(
                "SELECT revision FROM catalogue_health_epoch WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map(Some)
            .map_err(|e| db_error("read catalogue preview epoch", e))
    }

    pub(crate) fn catalogue_source_binding(&self, source: i64) -> Result<Option<(String, i64)>> {
        if self.schema_version()? < 23 {
            return Ok(None);
        }
        self.source_binding(source)
    }

    pub(super) fn source_binding(&self, source: i64) -> Result<Option<(String, i64)>> {
        self.connection.query_row("SELECT root_identity_json,generation FROM source_scan_bindings WHERE source_folder_id=?1", [source], |r| Ok((r.get(0)?,r.get(1)?)))
            .optional().map_err(|e| db_error("read source scan binding",e))
    }

    pub(super) fn source_binding_current(&self, source: i64, root: &Path) -> Result<bool> {
        let Some((expected, _)) = self.source_binding(source)? else {
            return Ok(false);
        };
        Ok(SourceRootBinding::inspect(root)
            .and_then(|r| serde_json::to_string(&r).ok())
            .as_deref()
            == Some(expected.as_str()))
    }

    /// Remembers newly observed nested filesystem boundaries. A boundary is
    /// accepted only while the exact mount the walker saw is still there, both
    /// before and after the record is written; otherwise nothing is stored and
    /// it is returned as unproven. An existing record is never overwritten.
    pub(crate) fn record_nested_boundaries(
        &self,
        source: i64,
        root: &BoundRoot,
        observed: &[NestedBoundaryObservation],
    ) -> Result<Vec<(PathBuf, &'static str)>> {
        let mut unproven = Vec::new();
        for seen in observed {
            let Ok(relative) = seen.path.strip_prefix(root.root_path()) else {
                continue;
            };
            let key = relative.as_os_str().as_bytes();
            let stored: Option<Option<String>> = self
                .connection
                .query_row(
                    "SELECT binding_json FROM source_nested_boundaries WHERE source_folder_id=?1 AND relative_path=?2",
                    params![source, key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| db_error("read nested boundary", e))?;
            // Accepted records are checked by `unproven_nested_boundaries`.
            if matches!(stored, Some(Some(_))) {
                continue;
            }
            let same_mount = |state: &NestedState| {
                state.is_mount_root()
                    && state.mount_id == seen.mount_id
                    && state.binding.device == seen.device
                    && state.binding.inode == seen.inode
            };
            let Some(state) = root
                .nested_state(&seen.path)
                .ok()
                .filter(|state| same_mount(state))
            else {
                self.connection
                    .execute(
                        "INSERT OR IGNORE INTO source_nested_boundaries(source_folder_id,relative_path,binding_json) VALUES(?1,?2,NULL)",
                        params![source, key],
                    )
                    .map_err(|e| db_error("quarantine unproven nested boundary", e))?;
                unproven.push((
                    seen.path.clone(),
                    "mount disappeared or changed before its identity could be recorded",
                ));
                continue;
            };
            let json = serde_json::to_string(&state.binding)
                .map_err(|e| ArchiveFsError::Database(e.to_string()))?;
            self.connection
                .execute(
                    "INSERT INTO source_nested_boundaries(source_folder_id,relative_path,binding_json) VALUES(?1,?2,?3) ON CONFLICT(source_folder_id,relative_path) DO UPDATE SET binding_json=excluded.binding_json",
                    params![source, key, json],
                )
                .map_err(|e| db_error("record nested filesystem boundary", e))?;
            if !root
                .nested_state(&seen.path)
                .ok()
                .is_some_and(|after| same_mount(&after))
            {
                self.connection
                    .execute(
                        "UPDATE source_nested_boundaries SET binding_json=NULL WHERE source_folder_id=?1 AND relative_path=?2",
                        params![source, key],
                    )
                    .map_err(|e| db_error("withdraw unproven nested boundary acceptance", e))?;
                unproven.push((
                    seen.path.clone(),
                    "mount disappeared or changed while its identity was recorded",
                ));
            }
        }
        Ok(unproven)
    }

    pub(crate) fn unproven_nested_boundaries(
        &self,
        source: i64,
        root: &BoundRoot,
    ) -> Result<Vec<(PathBuf, &'static str)>> {
        unproven_nested_boundaries_on(&self.connection, source, root)
    }

    /// Schema-21/22 databases have no boundary table; nothing was ever remembered.
    pub(crate) fn preview_unproven_nested_boundaries(
        &self,
        source: i64,
        root: &BoundRoot,
    ) -> Result<Vec<(PathBuf, &'static str)>> {
        if self.schema_version()? < 23 {
            return Ok(Vec::new());
        }
        self.unproven_nested_boundaries(source, root)
    }

    /// First actual scan establishes a binding. Subsequent mismatches require
    /// an explicit reviewed rebind, never an automatic acceptance of a new disk.
    pub(super) fn bind_scan_source(&self, source: i64, identity: (u64, u64)) -> Result<bool> {
        let bytes: Vec<u8> = self
            .connection
            .query_row(
                "SELECT path FROM source_folders WHERE id=?1",
                [source],
                |r| r.get(0),
            )
            .map_err(|e| db_error("read source binding path", e))?;
        let root = PathBuf::from(OsString::from_vec(bytes));
        let Some(binding) = SourceRootBinding::inspect(&root) else {
            return Ok(false);
        };
        if (binding.device, binding.inode) != identity {
            return Ok(false);
        }
        let json =
            serde_json::to_string(&binding).map_err(|e| ArchiveFsError::Database(e.to_string()))?;
        if let Some((old, _)) = self.source_binding(source)? {
            return Ok(old == json);
        }
        // Only a source the catalogue knows nothing about may bind itself, and only
        // on its very first scan. Anything with history needs a reviewed rebind:
        // never trust whatever storage happens to be at the old path.
        if source_has_catalogue_history(&self.connection, source)? {
            return Ok(false);
        }
        write_rebind(&self.connection, source, &None, &json)
    }

    /// Compatibility name for reviewed callers. The opaque review is the
    /// authority; the implementation is exactly the canonical confirmation.
    pub fn rebind_source_after_review(&mut self, review: &SourceRebindReview) -> Result<()> {
        self.confirm_source_rebind(review)
    }

    pub(crate) fn catalogue_archive_hashes(&self) -> Result<HashMap<i64, String>> {
        let mut stmt = self
            .connection
            .prepare("SELECT id, archive_hash FROM archives WHERE archive_hash IS NOT NULL")
            .map_err(|e| db_error("prepare catalogue hash evidence", e))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| db_error("read catalogue hash evidence", e))?;
        let mut hashes = HashMap::new();
        for row in rows {
            let (id, hash) = row.map_err(|e| db_error("decode catalogue hash evidence", e))?;
            if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                hashes.insert(id, hash.to_ascii_lowercase());
            }
        }
        Ok(hashes)
    }

    pub(crate) fn initial_scan_coverage(
        &self,
        folders: &[RegisteredSourceFolder],
    ) -> Result<Vec<SourceScanCoverage>> {
        let mut stmt = self
            .connection
            .prepare("SELECT id,path,removed_from_config_at FROM source_folders ORDER BY id")
            .map_err(|e| db_error("prepare source coverage", e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| db_error("read source coverage", e))?;
        let mut coverage = Vec::new();
        for row in rows {
            let (id, path, removed) = row.map_err(|e| db_error("decode source coverage", e))?;
            coverage.push(SourceScanCoverage {
                source_id: id,
                root_identity: None,
                root: PathBuf::from(OsString::from_vec(path)),
                state: if removed.is_some() {
                    ScanCoverageState::Removed
                } else {
                    ScanCoverageState::NotAttempted
                },
                excluded_roots: folders
                    .iter()
                    .find(|f| f.id == id)
                    .map(|f| f.excluded_source_roots.clone())
                    .unwrap_or_default(),
                diagnostic: None,
            });
        }
        Ok(coverage)
    }

    pub(super) fn record_scan_coverage(
        &self,
        run: i64,
        coverage: &SourceScanCoverage,
    ) -> Result<()> {
        let state = serde_json::to_string(&coverage.state)
            .map_err(|e| ArchiveFsError::Database(e.to_string()))?;
        let excluded: Vec<_> = coverage
            .excluded_roots
            .iter()
            .map(|p| p.as_os_str().as_bytes().to_vec())
            .collect();
        let root_identity = serde_json::to_string(&coverage.root_identity)
            .map_err(|e| ArchiveFsError::Database(e.to_string()))?;
        let excluded = serde_json::to_string(&excluded)
            .map_err(|e| ArchiveFsError::Database(e.to_string()))?;
        self.connection.execute("INSERT INTO scan_source_coverage(scan_run_id,source_folder_id,state,excluded_roots_json,diagnostic,root_identity_json,source_generation) VALUES(?1,?2,?3,?4,?5,?6,(SELECT generation FROM source_scan_bindings WHERE source_folder_id=?2)) ON CONFLICT(scan_run_id,source_folder_id) DO UPDATE SET state=excluded.state, excluded_roots_json=excluded.excluded_roots_json,diagnostic=excluded.diagnostic,root_identity_json=excluded.root_identity_json,source_generation=excluded.source_generation", params![run,coverage.source_id,state,excluded,coverage.diagnostic,root_identity]).map_err(|e| db_error("record source coverage",e))?;
        Ok(())
    }

    pub fn scan_coverage(&self, run: i64) -> Result<Vec<SourceScanCoverage>> {
        let mut stmt=self.connection.prepare("SELECT c.source_folder_id,s.path,c.state,c.excluded_roots_json,c.diagnostic,c.root_identity_json FROM scan_source_coverage c JOIN source_folders s ON s.id=c.source_folder_id WHERE c.scan_run_id=?1 ORDER BY c.source_folder_id").map_err(|e| db_error("prepare recorded coverage",e))?;
        let rows = stmt
            .query_map([run], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| db_error("read recorded coverage", e))?;
        let mut coverage = Vec::new();
        for row in rows {
            let (source_id, root, state, excluded, diagnostic, root_identity) =
                row.map_err(|e| db_error("decode recorded coverage", e))?;
            let excluded: Vec<Vec<u8>> = serde_json::from_str(&excluded)
                .map_err(|e| ArchiveFsError::Database(e.to_string()))?;
            coverage.push(SourceScanCoverage {
                source_id,
                root_identity: serde_json::from_str(&root_identity)
                    .map_err(|e| ArchiveFsError::Database(e.to_string()))?,
                root: PathBuf::from(OsString::from_vec(root)),
                state: serde_json::from_str(&state)
                    .map_err(|e| ArchiveFsError::Database(e.to_string()))?,
                excluded_roots: excluded
                    .into_iter()
                    .map(|b| PathBuf::from(OsString::from_vec(b)))
                    .collect(),
                diagnostic,
            });
        }
        Ok(coverage)
    }

    /// Final outer transaction boundary: status/history writes between folder
    /// persistence and COMMIT must not leave a root replacement unchecked.
    pub(super) fn validate_scan_source_commit(&self, run: i64) -> Result<()> {
        for covered in self.scan_coverage(run)? {
            if matches!(
                covered.state,
                ScanCoverageState::Complete | ScanCoverageState::Partial
            ) && (source_root_identity(&covered.root) != covered.root_identity
                || !self.source_binding_current(covered.source_id, &covered.root)?)
            {
                return Err(ArchiveFsError::Database(
                    "source changed at scan commit; catalogue evidence preserved".into(),
                ));
            }
            // Authority that was proven earlier must still hold at commit.
            if covered.state == ScanCoverageState::Complete
                && let Some(root) = BoundRoot::open(&covered.root)
                && !self
                    .unproven_nested_boundaries(covered.source_id, &root)?
                    .is_empty()
            {
                return Err(ArchiveFsError::Database(
                    "nested filesystem boundary changed at scan commit; catalogue evidence preserved"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    /// Explicit apply of a same-database preview. Rechecks every correction
    /// before any update and commits all corrections/history atomically.
    /// Does not change identities, metadata, paths or source membership.
    /// Present orphaned rows may lose stale absence evidence but stay orphaned.
    pub fn apply_presence_reconciliation(
        &mut self,
        report: &CatalogueHealthReport,
    ) -> Result<usize> {
        crate::validate_configured_source_roots(&report.configured_roots)?;
        if report.database_path != self.path {
            return Err(ArchiveFsError::Database(
                "presence preview belongs to another database".into(),
            ));
        }
        // Even an empty plan must not validate a materially stale preview.
        // Legacy read-only previews need a fresh preview after schema upgrade.
        if report.epoch.is_none() || report.epoch != self.catalogue_health_epoch()? {
            return Err(ArchiveFsError::Database(
                "presence preview changed; preview again before applying".into(),
            ));
        }
        let corrections: Vec<_> = report
            .rows
            .iter()
            .filter(|r| r.observation.is_present() && r.archive.last_verified_missing_at.is_some())
            .collect();
        if corrections.is_empty() {
            return Ok(0);
        }
        let roots: HashMap<_, _> = report
            .sources
            .iter()
            .filter_map(|s| {
                let root = BoundRoot::open(&s.root)?;
                (Some(root.identity) == s.root_identity
                    && report.source_bindings.get(&s.source_id) == Some(&root.binding))
                .then_some((s.source_id, root))
            })
            .collect();
        let now = now_utc_string();
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| db_error("begin presence reconciliation", e))?;
        let epoch: i64 = tx
            .query_row(
                "SELECT revision FROM catalogue_health_epoch WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| db_error("revalidate preview epoch", e))?;
        if report.epoch != Some(epoch) {
            return Err(ArchiveFsError::Database(
                "presence preview changed; preview again before applying".into(),
            ));
        }
        for row in &corrections {
            let source = report
                .sources
                .iter()
                .find(|s| s.source_id == row.archive.source_folder_id)
                .ok_or_else(|| ArchiveFsError::Database("preview source binding absent".into()))?;
            let current: (Vec<u8>,i64,String,Option<String>)=tx.query_row("SELECT absolute_path_cached,source_folder_id,archive_kind,last_verified_missing_at FROM archives WHERE id=?1",[row.archive.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(|e|db_error("revalidate preview row",e))?;
            if current
                != (
                    row.archive.absolute_path.as_os_str().as_bytes().to_vec(),
                    row.archive.source_folder_id,
                    row.archive.archive_kind.clone(),
                    row.archive.last_verified_missing_at.clone(),
                )
            {
                return Err(ArchiveFsError::Database(
                    "preview representation changed".into(),
                ));
            }
            // The epoch binds every archive/source mutation, including ABA
            // delete/recreate, representation, identity and new scan history.
            if roots
                .get(&source.source_id)
                .map(|root| {
                    root.probe(
                        &row.archive.absolute_path,
                        row.archive.archive_kind == "arcade_set_directory",
                    )
                })
                .as_ref()
                != Some(&row.observation)
            {
                return Err(ArchiveFsError::Database(
                    "presence preview changed; preview again before applying".into(),
                ));
            }
        }
        tx.execute("INSERT INTO scan_runs(started_at,finished_at,triggered_by,status,archives_seen,archives_updated) VALUES(?1,?1,'catalogue-presence-reconciliation','completed',?2,?2)",params![now,corrections.len() as i64]).map_err(|e| db_error("record presence reconciliation",e))?;
        let run = tx.last_insert_rowid();
        for row in &corrections {
            let source = report
                .sources
                .iter()
                .find(|s| s.source_id == row.archive.source_folder_id)
                .unwrap();
            if roots
                .get(&source.source_id)
                .map(|root| {
                    root.probe(
                        &row.archive.absolute_path,
                        row.archive.archive_kind == "arcade_set_directory",
                    )
                })
                .as_ref()
                != Some(&row.observation)
            {
                return Err(ArchiveFsError::Database(
                    "presence changed at commit; preview again".into(),
                ));
            }
            tx.execute("UPDATE archives SET last_verified_missing_at=NULL,last_seen_at=?2,updated_at=?2 WHERE id=?1",params![row.archive.id,now]).map_err(|e| db_error("clear stale missing evidence",e))?;
            tx.execute("INSERT INTO archive_scan_observations(scan_run_id,archive_id,observation,size_bytes,modified_time_unix_seconds,observed_at) VALUES(?1,?2,'restored',?3,?4,?5)",params![run,row.archive.id,row.observation.size.map(|v|v as i64),row.observation.modified,now]).map_err(|e| db_error("record current presence evidence",e))?;
        }
        for row in &corrections {
            let source = report
                .sources
                .iter()
                .find(|s| s.source_id == row.archive.source_folder_id)
                .unwrap();
            if roots
                .get(&source.source_id)
                .map(|root| {
                    root.probe(
                        &row.archive.absolute_path,
                        row.archive.archive_kind == "arcade_set_directory",
                    )
                })
                .as_ref()
                != Some(&row.observation)
            {
                return Err(ArchiveFsError::Database(
                    "presence changed before reconciliation commit".into(),
                ));
            }
        }
        for source in &report.sources {
            if source.root_identity.is_some()
                && source_root_identity(&source.root) != source.root_identity
            {
                return Err(ArchiveFsError::Database(
                    "source changed at reconciliation commit".into(),
                ));
            }
        }
        tx.commit()
            .map_err(|e| db_error("commit presence reconciliation", e))?;
        Ok(corrections.len())
    }
}

impl Database {
    /// One configured, active source: its path and whether it was ever scanned.
    fn source_review_row(&self, source: i64) -> Result<Option<(PathBuf, bool, Option<String>)>> {
        self.connection
            .query_row(
                "SELECT path, removed_from_config_at IS NOT NULL, last_successful_scan_at \
                 FROM source_folders WHERE id=?1",
                [source],
                |r| {
                    Ok((
                        PathBuf::from(OsString::from_vec(r.get::<_, Vec<u8>>(0)?)),
                        r.get::<_, bool>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| db_error("read source for review", e))
    }

    /// Whether a source needs a person's review before it can be scanned again,
    /// and everything that person needs to judge it. Read-only. Errors in plain
    /// language when there is nothing to review or it cannot be reviewed now.
    pub fn review_source_rebind(&self, source: i64) -> Result<SourceRebindReview> {
        if self.schema_version()? < 23 {
            return Err(ArchiveFsError::Database(
                "this library database has no source binding to review".into(),
            ));
        }
        let epoch = self.catalogue_health_epoch()?.ok_or_else(|| {
            ArchiveFsError::Database("this library database has no source binding to review".into())
        })?;
        let (root, _, last_success) = self
            .source_review_row(source)?
            .filter(|(_, removed, _)| !removed)
            .ok_or_else(|| ArchiveFsError::Database("that source is not configured".into()))?;
        if fs::read_dir(&root).is_err() {
            return Err(ArchiveFsError::Database(
                "the source folder cannot be reached right now; reconnect it and try again".into(),
            ));
        }
        let current = SourceRootBinding::inspect(&root).ok_or_else(|| {
            ArchiveFsError::Database(
                "the source folder cannot be inspected right now; reconnect it and try again"
                    .into(),
            )
        })?;
        let bound = self.source_binding(source)?;
        let (generation, recorded, reason) = match bound {
            None if source_has_catalogue_history(&self.connection, source)? => {
                (0, None, RebindReason::NeverBound)
            }
            None => {
                return Err(ArchiveFsError::Database(
                    "this source has never been scanned; its first scan records its storage \
                     automatically, so there is nothing to review"
                        .into(),
                ));
            }
            Some((json, generation)) => {
                if serde_json::to_string(&current).ok().as_deref() == Some(json.as_str()) {
                    return Err(ArchiveFsError::Database(
                        "this source is on the same storage it was bound to; nothing to review"
                            .into(),
                    ));
                }
                (
                    generation,
                    serde_json::from_str(&json).ok(),
                    RebindReason::BackingChanged,
                )
            }
        };
        let archive_count: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM archives WHERE source_folder_id=?1",
                [source],
                |r| r.get(0),
            )
            .map_err(|e| db_error("count archives for review", e))?;
        Ok(SourceRebindReview {
            source_id: source,
            path: root,
            generation,
            recorded,
            current,
            reason,
            archive_count,
            last_successful_scan_at: last_success,
            epoch,
        })
    }

    /// Commits a reviewed rebind. Every part of the review is checked again
    /// inside one immediate transaction: the source is still configured at the
    /// same path, its accepted generation and recorded binding are exactly what
    /// was reviewed, and the folder is still on the reviewed storage. Anything
    /// else refuses with [`REBIND_REVIEW_AGAIN`].
    ///
    /// Writes only `source_scan_bindings`. It never edits archives, identity,
    /// observations, coverage or Missing evidence, and it records no scan: a new
    /// complete scan is still required before anything can be marked Missing.
    pub fn confirm_source_rebind(&mut self, review: &SourceRebindReview) -> Result<()> {
        let stale = || ArchiveFsError::Database(REBIND_REVIEW_AGAIN.into());
        if SourceRootBinding::inspect(&review.path).as_ref() != Some(&review.current) {
            return Err(stale());
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| db_error("begin reviewed source rebind", e))?;
        let (path, removed): (Vec<u8>, bool) = tx
            .query_row(
                "SELECT path, removed_from_config_at IS NOT NULL FROM source_folders WHERE id=?1",
                [review.source_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| db_error("recheck reviewed source", e))?
            .ok_or_else(stale)?;
        if removed || path != review.path.as_os_str().as_bytes() {
            return Err(stale());
        }
        // The catalogue epoch is a monotonic counter bumped by triggers on every
        // mutation of sources, archives, scans, coverage, bindings, identity facts
        // and platform assignments. A source removed and re-added, a changed role,
        // another rebind, or any scan all move it, which is the only reliable way to
        // see a remove/re-add (the row ends up identical). Reviews are meant to be
        // confirmed straight away, so any intervening write means: review it again.
        let epoch: i64 = tx
            .query_row(
                "SELECT revision FROM catalogue_health_epoch WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| db_error("recheck catalogue epoch", e))?;
        if epoch != review.epoch {
            return Err(stale());
        }
        let existing: Option<(String, i64)> = tx
            .query_row(
                "SELECT root_identity_json,generation FROM source_scan_bindings WHERE source_folder_id=?1",
                [review.source_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| db_error("recheck source binding", e))?;
        let new_json = serde_json::to_string(&review.current)
            .map_err(|e| ArchiveFsError::Database(e.to_string()))?;
        let reviewed_state_holds = match &existing {
            None => review.generation == 0 && review.recorded.is_none(),
            Some((json, generation)) => {
                review.generation > 0
                    && *generation == review.generation
                    && Some(json.clone())
                        == review
                            .recorded
                            .as_ref()
                            .and_then(|r| serde_json::to_string(r).ok())
                    && *json != new_json
            }
        };
        if !reviewed_state_holds
            || !write_rebind(&tx, review.source_id, &existing, &new_json)?
            || SourceRootBinding::inspect(&review.path).as_ref() != Some(&review.current)
        {
            return Err(stale());
        }
        tx.commit()
            .map_err(|e| db_error("commit reviewed source rebind", e))
    }

    /// Source-level catalogue state for every configured, active source, from
    /// the same facts the scan and reconciliation use: the accepted binding and
    /// generation, the latest coverage recorded for that generation, and the
    /// remembered nested boundaries. Read-only and cheap; never walks archives.
    pub fn source_health(&self, configured_roots: &[PathBuf]) -> Result<Vec<SourceHealth>> {
        let schema_23 = self.schema_version()? >= 23;
        let mut health = Vec::new();
        for source in self.initial_scan_coverage(&[])? {
            if source.state == ScanCoverageState::Removed
                || !configured_roots.contains(&source.root)
            {
                continue;
            }
            let historical =
                schema_23 && source_has_catalogue_history(&self.connection, source.source_id)?;
            let bound = if schema_23 {
                self.source_binding(source.source_id)?
            } else {
                None
            };
            let generation = bound.as_ref().map_or(0, |(_, g)| *g);
            let mut entry = SourceHealth {
                source_id: source.source_id,
                path: source.root.clone(),
                state: SourceHealthState::NeedsScan,
                rebind: None,
                generation,
                detail: None,
            };
            let root = BoundRoot::open(&source.root).filter(|_| fs::read_dir(&source.root).is_ok());
            let Some(root) = root else {
                entry.state = SourceHealthState::SourceUnavailable;
                entry.detail = Some("the folder cannot be reached right now".into());
                health.push(entry);
                continue;
            };
            match &bound {
                None if historical => {
                    entry.state = SourceHealthState::RebindRequired;
                    entry.rebind = Some(RebindReason::NeverBound);
                    health.push(entry);
                    continue;
                }
                None => {
                    health.push(entry);
                    continue;
                }
                Some((json, _))
                    if serde_json::to_string(&root.binding).ok().as_deref()
                        != Some(json.as_str()) =>
                {
                    entry.state = SourceHealthState::RebindRequired;
                    entry.rebind = Some(RebindReason::BackingChanged);
                    health.push(entry);
                    continue;
                }
                Some(_) => {}
            }
            // A scan that did not attempt this source (a targeted scan of another
            // folder) is not an observation of it; the latest real attempt decides.
            let covered = latest_actual_attempt(&self.connection, source.source_id)?
                .and_then(|(_, state, recorded)| (recorded == Some(generation)).then_some(state));
            let Some(covered) = covered else {
                health.push(entry);
                continue;
            };
            let unproven = self.unproven_nested_boundaries(source.source_id, &root)?;
            entry.state = if !unproven.is_empty() {
                entry.detail = Some(format!(
                    "{} nested storage location(s) cannot be proven unchanged",
                    unproven.len()
                ));
                SourceHealthState::CoverageIncomplete
            } else {
                match covered {
                    ScanCoverageState::Complete => SourceHealthState::Healthy,
                    ScanCoverageState::Partial => SourceHealthState::PartialScan,
                    ScanCoverageState::Skipped => SourceHealthState::NotGameScanned,
                    _ => SourceHealthState::CoverageIncomplete,
                }
            };
            health.push(entry);
        }
        Ok(health)
    }
}

// --- The one authority rule -----------------------------------------------------
//
// NO new Missing evidence unless the source is presently Healthy for the exact
// current binding, generation and latest completed scan attempt. Everything below
// is shared by the binding gate, the reviewed rebind, source health and the Missing
// write boundary, so there is exactly one definition of each fact.

/// Coverage rows that record a scan *not looking at* a source. A targeted scan of
/// another folder writes `NotAttempted` for this one; that is not an observation of
/// it and never supersedes its latest real attempt.
const NOT_AN_ATTEMPT: &str = "('\"not_attempted\"','\"removed\"')";

/// Whether this source is *historical*: the catalogue already knew it before it
/// could be bound, so its current storage may not be what that history came from
/// and it must never be bound automatically. Three independent facts say so:
/// migration found catalogue rows or a successful scan for it when the schema was
/// applied (`source_review_required`), it has a successful-scan timestamp, or a
/// scan attempted it *under an accepted generation* (a first scan that found the
/// drive offline observed nothing, so it is not history). `last_successful_scan_at` alone is not enough: a
/// migrated catalogue can have rows and no timestamp, which is exactly why the
/// migration records the fact. A source added afterwards has none of these, so its
/// first scan may bind it.
pub(super) fn source_has_catalogue_history(connection: &Connection, source: i64) -> Result<bool> {
    connection
        .query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM source_review_required WHERE source_folder_id=?1) \
                 OR COALESCE((SELECT last_successful_scan_at IS NOT NULL FROM source_folders WHERE id=?1),0) \
                 OR EXISTS(SELECT 1 FROM scan_source_coverage WHERE source_folder_id=?1 AND source_generation IS NOT NULL AND state NOT IN {NOT_AN_ATTEMPT})"
            ),
            [source],
            |r| r.get(0),
        )
        .map_err(|e| db_error("check source catalogue history", e))
}

/// The most recent scan that actually attempted this source: `(run, outcome,
/// generation recorded for it)`. An unreadable outcome counts as `Failed`.
pub(super) fn latest_actual_attempt(
    connection: &Connection,
    source: i64,
) -> Result<Option<(i64, ScanCoverageState, Option<i64>)>> {
    let row: Option<(i64, String, Option<i64>)> = connection
        .query_row(
            &format!(
                "SELECT scan_run_id, state, source_generation FROM scan_source_coverage \
                 WHERE source_folder_id=?1 AND state NOT IN {NOT_AN_ATTEMPT} \
                 ORDER BY scan_run_id DESC LIMIT 1"
            ),
            [source],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| db_error("read latest source attempt", e))?;
    Ok(row.map(|(run, state, generation)| {
        let state = serde_json::from_str(&state).unwrap_or(ScanCoverageState::Failed);
        (run, state, generation)
    }))
}

/// The generation a new binding must take: strictly above the one it replaces and
/// above every generation coverage was ever recorded for, so no historical
/// coverage can collide with it. Checked: it errors rather than wrapping.
pub(super) fn next_source_generation(
    connection: &Connection,
    source: i64,
    current: i64,
) -> Result<i64> {
    let recorded: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(source_generation),0) FROM scan_source_coverage WHERE source_folder_id=?1",
            [source],
            |r| r.get(0),
        )
        .map_err(|e| db_error("read historical source generations", e))?;
    current.max(recorded).checked_add(1).ok_or_else(|| {
        ArchiveFsError::Database("source generation space is exhausted; nothing was changed".into())
    })
}

/// Refuses unless `scan_run_id` is the latest actual attempt for this source, it
/// completed, and it was recorded for the source's *current* binding generation.
/// Called at the write boundary itself, so no caller can supply stale coverage.
pub(super) fn assert_missing_authority(
    connection: &Connection,
    source: i64,
    scan_run_id: i64,
    expected_root: &[u8],
    expected_binding: &SourceRootBinding,
) -> Result<()> {
    let current: Option<(Vec<u8>, String, i64, String, bool)> = connection
        .query_row(
            "SELECT sf.path,b.root_identity_json,b.generation,sf.source_role, \
             sf.removed_from_config_at IS NULL \
             FROM source_folders sf JOIN source_scan_bindings b ON b.source_folder_id=sf.id \
             WHERE sf.id=?1 AND NOT EXISTS(SELECT 1 FROM source_review_required r WHERE r.source_folder_id=sf.id)",
            [source],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(|e| db_error("read binding for missing authority", e))?;
    let Some((path, binding_json, generation, role_json, configured)) = current else {
        return Err(ArchiveFsError::Database(
            "source is removed, unconfigured, or requires reviewed rebind; no missing evidence written"
                .into(),
        ));
    };
    let role = crate::database::SourceRole::from_db_str(&role_json);
    let accepted: SourceRootBinding = serde_json::from_str(&binding_json).map_err(|e| {
        ArchiveFsError::Database(format!("decode source binding for missing authority: {e}"))
    })?;
    if !configured
        || !role.may_enter_game_scan()
        || path != expected_root
        || accepted != *expected_binding
    {
        return Err(ArchiveFsError::Database(
            "source configuration or storage binding changed; no missing evidence written".into(),
        ));
    }
    match latest_actual_attempt(connection, source)? {
        Some((run, _, _)) if run != scan_run_id => Err(ArchiveFsError::Database(
            "coverage superseded by a later scan attempt; no missing evidence written".into(),
        )),
        Some((_, ScanCoverageState::Complete, Some(recorded))) if recorded == generation => Ok(()),
        Some((_, ScanCoverageState::Complete, _)) => Err(ArchiveFsError::Database(
            "source generation changed; new scan required".into(),
        )),
        _ => Err(ArchiveFsError::Database(
            "latest scan attempt was not complete; no missing evidence written".into(),
        )),
    }
}

/// Acquire SQLite's writer reservation inside the Missing savepoint before
/// authoritative reads. The epoch value is unchanged; config and binding
/// writers cannot commit until Missing commits or rolls back.
pub(super) fn lock_missing_authority(connection: &Connection) -> Result<()> {
    let changed = connection
        .execute(
            "UPDATE catalogue_health_epoch SET revision=revision WHERE id=1",
            [],
        )
        .map_err(|e| db_error("lock missing authority", e))?;
    if changed != 1 {
        return Err(ArchiveFsError::Database(
            "catalogue authority state is unavailable; no missing evidence written".into(),
        ));
    }
    Ok(())
}

/// The only place a source binding is written after first capture. `existing` is
/// what the caller verified; the write is conditional on it, and the generation is
/// allocated by [`next_source_generation`].
fn write_rebind(
    connection: &Connection,
    source: i64,
    existing: &Option<(String, i64)>,
    new_json: &str,
) -> Result<bool> {
    let generation =
        next_source_generation(connection, source, existing.as_ref().map_or(0, |(_, g)| *g))?;
    let changed = match existing {
        None => connection.execute(
            "INSERT INTO source_scan_bindings(source_folder_id,root_identity_json,generation) \
             SELECT ?1,?2,?3 WHERE NOT EXISTS (SELECT 1 FROM source_scan_bindings WHERE source_folder_id=?1)",
            params![source, new_json, generation],
        ),
        Some((_, old)) => connection.execute(
            "UPDATE source_scan_bindings SET root_identity_json=?2,generation=?3 \
             WHERE source_folder_id=?1 AND generation=?4",
            params![source, new_json, generation, old],
        ),
    }
    .map_err(|e| db_error("write reviewed source binding", e))?;
    if changed == 1 {
        connection
            .execute(
                "DELETE FROM source_review_required WHERE source_folder_id=?1",
                [source],
            )
            .map_err(|e| db_error("clear source review requirement", e))?;
    }
    Ok(changed == 1)
}

/// Previously observed nested boundaries that are gone, no longer a mount root
/// of their own, replaced, or cannot be inspected. Anything beneath such a path
/// is not authoritative. Works on a plain connection so it can run inside the
/// very savepoint that writes Missing evidence.
pub(super) fn unproven_nested_boundaries_on(
    connection: &Connection,
    source: i64,
    root: &BoundRoot,
) -> Result<Vec<(PathBuf, &'static str)>> {
    let mut stmt = connection
        .prepare("SELECT relative_path,binding_json FROM source_nested_boundaries WHERE source_folder_id=?1 ORDER BY relative_path")
        .map_err(|e| db_error("prepare nested boundaries", e))?;
    let rows: Vec<(Vec<u8>, Option<String>)> = stmt
        .query_map([source], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| db_error("read nested boundaries", e))?
        .collect::<rusqlite::Result<_>>()
        .map_err(|e| db_error("decode nested boundaries", e))?;
    let mut unproven = Vec::new();
    for (relative, json) in rows {
        let path = root
            .root_path()
            .join(PathBuf::from(OsString::from_vec(relative)));
        let expected: Option<SourceRootBinding> =
            json.and_then(|json| serde_json::from_str(&json).ok());
        match root.nested_state(&path) {
            Ok(state) if state.is_mount_root() && expected.as_ref() == Some(&state.binding) => {}
            Ok(_) => unproven.push((path, "filesystem there changed or was unmounted")),
            Err(_) => unproven.push((path, "mountpoint is missing or cannot be inspected")),
        }
    }
    Ok(unproven)
}
