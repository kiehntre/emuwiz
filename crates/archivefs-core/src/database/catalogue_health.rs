//! Coverage proofs and explicit presence-only reconciliation. No cleanup or relink.
use super::*;
use crate::catalogue_health::{
    CatalogueHealthReport, ScanCoverageState, SourceScanCoverage, observe_path,
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
        self.connection.execute("INSERT INTO scan_source_coverage(scan_run_id,source_folder_id,state,excluded_roots_json,diagnostic,root_identity_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scan_run_id,source_folder_id) DO UPDATE SET state=excluded.state, excluded_roots_json=excluded.excluded_roots_json,diagnostic=excluded.diagnostic,root_identity_json=excluded.root_identity_json", params![run,coverage.source_id,state,excluded,coverage.diagnostic,root_identity]).map_err(|e| db_error("record source coverage",e))?;
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

    /// Explicit apply of a same-database preview. Rechecks every correction
    /// before any update and commits all corrections/history atomically.
    /// Does not change identities, metadata, paths or source membership.
    /// Present orphaned rows may lose stale absence evidence but stay orphaned.
    pub fn apply_presence_reconciliation(
        &mut self,
        report: &CatalogueHealthReport,
    ) -> Result<usize> {
        if report.database_path != self.path {
            return Err(ArchiveFsError::Database(
                "presence preview belongs to another database".into(),
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
        let now = now_utc_string();
        let tx = self
            .connection
            .transaction()
            .map_err(|e| db_error("begin presence reconciliation", e))?;
        for row in &corrections {
            let current:Option<(Vec<u8>,Option<String>)>=tx.query_row("SELECT absolute_path_cached,last_verified_missing_at FROM archives WHERE id=?1",[row.archive.id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e| db_error("recheck presence row",e))?;
            if current
                != Some((
                    row.archive.absolute_path.as_os_str().as_bytes().to_vec(),
                    row.archive.last_verified_missing_at.clone(),
                ))
                || observe_path(
                    &row.archive.absolute_path,
                    row.archive.archive_kind == "arcade_set_directory",
                ) != row.observation
            {
                return Err(ArchiveFsError::Database(
                    "presence preview changed; preview again before applying".into(),
                ));
            }
        }
        tx.execute("INSERT INTO scan_runs(started_at,finished_at,triggered_by,status,archives_seen,archives_updated) VALUES(?1,?1,'catalogue-presence-reconciliation','completed',?2,?2)",params![now,corrections.len() as i64]).map_err(|e| db_error("record presence reconciliation",e))?;
        let run = tx.last_insert_rowid();
        for row in &corrections {
            tx.execute("UPDATE archives SET last_verified_missing_at=NULL,last_seen_at=?2,updated_at=?2 WHERE id=?1",params![row.archive.id,now]).map_err(|e| db_error("clear stale missing evidence",e))?;
            tx.execute("INSERT INTO archive_scan_observations(scan_run_id,archive_id,observation,size_bytes,modified_time_unix_seconds,observed_at) VALUES(?1,?2,'restored',?3,?4,?5)",params![run,row.archive.id,row.observation.size.map(|v|v as i64),row.observation.modified,now]).map_err(|e| db_error("record current presence evidence",e))?;
        }
        tx.commit()
            .map_err(|e| db_error("commit presence reconciliation", e))?;
        Ok(corrections.len())
    }
}
