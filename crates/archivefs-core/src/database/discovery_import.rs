//! Database-only recovery adapter for confirmed native Discovery imports.
//! SQLite owns atomicity and crash recovery. The existing operation model is
//! only a projection; it never becomes a second journal or deletion authority.
use super::*;
use crate::identity_source::hashing::FileObjectStamp;
use crate::ingestion::bounded_import::{BoundedImportPlan, BoundedImportResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const REVIEWED_SCHEMA: i64 = 25;
const MAX_RECEIPT_BYTES: usize = 16 * 1024 * 1024;
const MAX_BATCH: usize = crate::ingestion::saved_discovery::MAX_SELECTED;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryImportItem {
    pub archive_id: i64,
    /// Exact Unix path bytes, never a lossy display path.
    pub path_bytes: Vec<u8>,
    pub platform_assignment_ids: Vec<i64>,
}
impl DiscoveryImportItem {
    pub fn path(&self) -> PathBuf {
        PathBuf::from(OsString::from_vec(self.path_bytes.clone()))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryImportExclusion {
    pub path_bytes: Vec<u8>,
    pub reason: String,
    pub existing_record: bool,
    pub refused: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Receipt {
    schema: u32,
    root_bytes: Vec<u8>,
    source_binding: crate::catalogue_health::SourceRootBinding,
    accepted_bindings: Vec<(i64, Option<(String, i64)>)>,
    configuration: FileProof,
    database_object: (u64, u64),
    approval_sha256: String,
    exclusions_sha256: String,
    catalogue_basis: Vec<String>,
    inserted: Vec<DiscoveryImportItem>,
    exclusions: Vec<DiscoveryImportExclusion>,
}
#[derive(Clone, Debug)]
pub struct DiscoveryImportOperation {
    pub operation_id: String,
    pub created_at: String,
    pub undone_at: Option<String>,
    pub undone: bool,
    pub items: Vec<DiscoveryImportItem>,
    pub skipped: usize,
    pub refused: usize,
    pub excluded: usize,
    pub root: PathBuf,
    pub approval_sha256: String,
    pub exclusions_sha256: String,
    pub catalogue_basis: Vec<String>,
    pub exclusions: Vec<DiscoveryImportExclusion>,
}
#[derive(Clone, Debug)]
pub struct DiscoveryImportUndoPreview {
    pub operation: DiscoveryImportOperation,
    pub blockers: Vec<String>,
    operation_id: String,
    sealed_items: Vec<DiscoveryImportItem>,
    authorised: bool,
    database: PathBuf,
    database_object: (u64, u64),
    receipt_sha256: String,
    prepared: std::time::Instant,
}
impl DiscoveryImportUndoPreview {
    pub fn can_undo(&self) -> bool {
        !self.expired()
            && self.authorised
            && self.operation.operation_id == self.operation_id
            && self.operation.items == self.sealed_items
            && !self.operation.undone
            && !self.operation.items.is_empty()
            && self.blockers.is_empty()
    }
    pub fn expired(&self) -> bool {
        self.prepared.elapsed() > crate::ingestion::bounded_import::REVIEW_LIFETIME
    }
    pub fn affected_records(&self) -> usize {
        self.sealed_items.len()
    }
    pub fn affected_dependencies(&self) -> usize {
        self.sealed_items
            .iter()
            .map(|i| i.platform_assignment_ids.len())
            .sum()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryImportUndoResult {
    pub operation_id: String,
    pub removed_records: usize,
    pub removed_assignments: usize,
    pub already_undone: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct RowProof {
    id: i64,
    sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct FileProof {
    path_bytes: Vec<u8>,
    size_bytes: u64,
    modified_unix_seconds: Option<i64>,
    object: Option<FileObjectStamp>,
    crc32: String,
    md5: String,
    sha1: String,
}
impl FileProof {
    fn of(h: &crate::identity_source::hashing::LocalHashes) -> Self {
        Self {
            path_bytes: h.fingerprint.path.as_os_str().as_bytes().to_vec(),
            size_bytes: h.fingerprint.size_bytes,
            modified_unix_seconds: h.fingerprint.modified_unix_seconds,
            object: h.fingerprint.object,
            crc32: h.crc32.clone(),
            md5: h.md5.clone(),
            sha1: h.sha1.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct RecordProof {
    archive: RowProof,
    assignments: Vec<RowProof>,
    candidate_id: String,
    file: FileProof,
    companions: Vec<FileProof>,
    platform: String,
    kind: String,
}
fn string_error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn schema_digest(c: &Connection) -> std::result::Result<String, String> {
    let mut stmt=c.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type,name LIMIT 513").map_err(string_error)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(string_error)?;
    let values = rows
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(string_error)?;
    if values.len() > 512 {
        return Err("Unknown or excessive database schema; Undo requires review.".into());
    }
    Ok(digest(&serde_json::to_vec(&values).map_err(string_error)?))
}
fn require_reviewed_schema(c: &Connection) -> std::result::Result<(), String> {
    static EXPECTED: OnceLock<std::result::Result<String, String>> = OnceLock::new();
    if latest_known_version(MIGRATIONS) != REVIEWED_SCHEMA {
        return Err(
            "Discovery Undo dependency policy requires review for this schema version.".into(),
        );
    }
    if schema_version(c).map_err(string_error)? != REVIEWED_SCHEMA {
        return Err(
            "Recorded database schema version changed; import/Undo requires review.".into(),
        );
    }
    let versions = c
        .prepare("SELECT version,description FROM schema_migrations ORDER BY version")
        .map_err(string_error)?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(string_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(string_error)?;
    if versions
        != MIGRATIONS
            .iter()
            .map(|m| (m.version, m.description.to_string()))
            .collect::<Vec<_>>()
    {
        return Err(
            "Recorded database migration version/history differs from the reviewed schema.".into(),
        );
    }
    let expected = EXPECTED
        .get_or_init(|| {
            let mut c = Connection::open_in_memory().map_err(string_error)?;
            apply_migrations(&mut c, MIGRATIONS).map_err(string_error)?;
            schema_digest(&c)
        })
        .as_ref()
        .map_err(Clone::clone)?;
    if &schema_digest(c)? != expected {
        return Err("Unknown or modified database schema, columns, relationships or triggers. Import/Undo refused pending review.".into());
    }
    Ok(())
}
fn row_proof(c: &Connection, table: &str, id: i64) -> std::result::Result<RowProof, String> {
    // Only fixed, reviewed table names ever reach this helper.
    let mut stmt = c
        .prepare(&format!("SELECT * FROM {table} WHERE id=?1"))
        .map_err(string_error)?;
    let columns = stmt.column_count();
    let mut rows = stmt.query([id]).map_err(string_error)?;
    let row = rows
        .next()
        .map_err(string_error)?
        .ok_or_else(|| format!("Missing {table} record {id}."))?;
    let mut h = Sha256::new();
    for n in 0..columns {
        use rusqlite::types::ValueRef;
        match row.get_ref(n).map_err(string_error)? {
            ValueRef::Null => h.update([0]),
            ValueRef::Integer(v) => {
                h.update([1]);
                h.update(v.to_le_bytes());
            }
            ValueRef::Real(v) => {
                h.update([2]);
                h.update(v.to_bits().to_le_bytes());
            }
            ValueRef::Text(v) | ValueRef::Blob(v) => {
                h.update([
                    if matches!(row.get_ref(n).map_err(string_error)?, ValueRef::Text(_)) {
                        3
                    } else {
                        4
                    },
                ]);
                h.update((v.len() as u64).to_le_bytes());
                h.update(v);
            }
        }
    }
    Ok(RowProof {
        id,
        sha256: h.finalize().iter().map(|b| format!("{b:02x}")).collect(),
    })
}
fn assignments(c: &Connection, id: i64) -> std::result::Result<Vec<RowProof>, String> {
    let mut stmt = c
        .prepare("SELECT id FROM platform_assignments WHERE archive_id=?1 ORDER BY id LIMIT 33")
        .map_err(string_error)?;
    let ids = stmt
        .query_map([id], |r| r.get::<_, i64>(0))
        .map_err(string_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(string_error)?;
    if ids.len() > 32 {
        return Err("Too many platform dependencies for a reviewed import.".into());
    }
    ids.into_iter()
        .map(|i| row_proof(c, "platform_assignments", i))
        .collect()
}
impl Database {
    pub(crate) fn configure_discovery_import_durability(&self) -> std::result::Result<(), String> {
        let mode: String = self
            .connection
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .map_err(string_error)?;
        if !matches!(mode.as_str(), "delete" | "truncate" | "persist" | "wal") {
            return Err("SQLite durable journalling is unavailable; import/Undo refused.".into());
        }
        self.connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(string_error)?;
        Ok(())
    }
    pub(crate) fn require_discovery_database_identity(
        &self,
        expected: (u64, u64),
    ) -> std::result::Result<(), String> {
        if crate::ingestion::bounded_import::object(&self.path)? != expected {
            return Err("Selected database filesystem identity changed.".into());
        }
        let mut moved = 1i32;
        // SAFETY: this worker exclusively owns the live connection; main is a
        // static C string and HAS_MOVED accepts a writable int for this call.
        let rc = unsafe {
            rusqlite::ffi::sqlite3_file_control(
                self.connection.handle(),
                c"main".as_ptr(),
                rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
                (&mut moved as *mut i32).cast(),
            )
        };
        if rc != rusqlite::ffi::SQLITE_OK || moved != 0 {
            return Err(
                "SQLite cannot prove that its open database still owns the selected pathname."
                    .into(),
            );
        }
        Ok(())
    }
    pub(crate) fn require_discovery_import_schema(&self) -> std::result::Result<(), String> {
        require_reviewed_schema(&self.connection)
    }
    /// Called before COMMIT, while the existing import writer lock is held.
    pub(crate) fn record_discovery_import(
        &self,
        plan: &BoundedImportPlan,
        result: &BoundedImportResult,
        existing: &BTreeSet<PathBuf>,
    ) -> std::result::Result<String, String> {
        self.require_discovery_import_schema()?;
        if result.inserted.len() > MAX_BATCH {
            return Err(format!(
                "Import exceeds the reviewed {MAX_BATCH}-record recovery bound. Select a smaller batch."
            ));
        }
        let op: String = self
            .connection
            .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
            .map_err(string_error)?;
        let mut items = Vec::new();
        let mut proofs = Vec::new();
        for (path, id) in &result.inserted {
            let c = plan
                .evidence
                .candidates
                .iter()
                .find(|c| c.hashes.fingerprint.path == *path)
                .ok_or("Inserted record is outside reviewed evidence")?;
            let proof = RecordProof {
                archive: row_proof(&self.connection, "archives", *id)?,
                assignments: assignments(&self.connection, *id)?,
                candidate_id: crate::ingestion::bounded_import::stable_candidate_id(c),
                file: FileProof::of(&c.hashes),
                companions: c.companions.iter().map(FileProof::of).collect(),
                platform: c.platform.clone(),
                kind: c.kind.clone(),
            };
            let (source, relative): (i64, Vec<u8>) = self
                .connection
                .query_row(
                    "SELECT source_folder_id,relative_path FROM archives WHERE id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(string_error)?;
            items.push(DiscoveryImportItem {
                archive_id: *id,
                path_bytes: path.as_os_str().as_bytes().to_vec(),
                platform_assignment_ids: proof.assignments.iter().map(|p| p.id).collect(),
            });
            proofs.push((
                id,
                source,
                relative,
                serde_json::to_vec(&proof).map_err(string_error)?,
            ));
        }
        let receipt = Receipt {
            schema: 1,
            root_bytes: plan.root().as_os_str().as_bytes().to_vec(),
            source_binding: plan.evidence.binding.clone(),
            accepted_bindings: plan.source_bindings.clone(),
            configuration: FileProof::of(&plan.config_hash),
            database_object: plan.database_object,
            approval_sha256: plan.approval_sha256().into(),
            exclusions_sha256: plan.exclusions_sha256().into(),
            catalogue_basis: plan.evidence.catalogue_basis.clone(),
            inserted: items,
            exclusions: result
                .skipped
                .iter()
                .map(|(p, why)| DiscoveryImportExclusion {
                    path_bytes: p.as_os_str().as_bytes().to_vec(),
                    reason: why.clone(),
                    existing_record: existing.contains(p),
                    refused: result.refused.iter().any(|(path, _)| path == p),
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&receipt).map_err(string_error)?;
        if bytes.len() + proofs.iter().map(|p| p.3.len()).sum::<usize>() > MAX_RECEIPT_BYTES {
            return Err(
                "Import receipt exceeds bounded recovery evidence capacity; use a smaller batch."
                    .into(),
            );
        }
        self.connection.execute("INSERT INTO discovery_import_operations(operation_id,state,created_at,receipt_json) VALUES (?1,'committed',?2,?3)",params![op,now_utc_string(),bytes]).map_err(string_error)?;
        for ((id, source, relative, proof), item) in proofs.iter().zip(&receipt.inserted) {
            self.connection.execute("INSERT INTO discovery_import_records(operation_id,archive_id,source_folder_id,relative_path,absolute_path,snapshot_json) VALUES(?1,?2,?3,?4,?5,?6)",params![op,id,source,relative,item.path_bytes,proof]).map_err(string_error)?;
        }
        Ok(op)
    }
    fn import_operation(
        &self,
        id: &str,
    ) -> std::result::Result<(DiscoveryImportOperation, Vec<u8>), String> {
        let (state,created,undone,bytes):(String,String,Option<String>,Option<Vec<u8>>)=self.connection.query_row("SELECT state,created_at,undone_at,CASE WHEN length(receipt_json)<=?2 THEN receipt_json END FROM discovery_import_operations WHERE operation_id=?1",params![id,MAX_RECEIPT_BYTES as i64],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(|_|"Import receipt is missing or unavailable; no deletion is authorised.".to_string())?;
        let bytes = bytes.ok_or("Import receipt exceeds recovery evidence bound")?;
        let r: Receipt = serde_json::from_slice(&bytes).map_err(string_error)?;
        if r.schema != 1
            || r.inserted.len() > MAX_BATCH
            || !matches!(state.as_str(), "committed" | "undone")
            || (state == "undone") != undone.is_some()
        {
            return Err("Import receipt has unsupported or inconsistent recovery evidence.".into());
        }
        let unique: BTreeSet<_> = r.inserted.iter().map(|i| i.archive_id).collect();
        if unique.len() != r.inserted.len() {
            return Err("Import receipt repeats a record identity.".into());
        }
        let skipped = r.exclusions.iter().filter(|e| e.existing_record).count();
        let refused = r.exclusions.iter().filter(|e| e.refused).count();
        if r.exclusions.iter().any(|e| e.existing_record && e.refused) {
            return Err("Contradictory import outcome classifications.".into());
        }
        let excluded = r.exclusions.len() - skipped - refused;
        Ok((
            DiscoveryImportOperation {
                operation_id: id.into(),
                created_at: created,
                undone_at: undone,
                undone: state == "undone",
                items: r.inserted,
                skipped,
                refused,
                excluded,
                root: PathBuf::from(OsString::from_vec(r.root_bytes)),
                approval_sha256: r.approval_sha256,
                exclusions_sha256: r.exclusions_sha256,
                catalogue_basis: r.catalogue_basis,
                exclusions: r.exclusions,
            },
            bytes,
        ))
    }
    /// Paged durable History. No ROM access and no migration occurs here.
    pub fn discovery_import_history(
        &self,
        offset: usize,
        limit: usize,
    ) -> std::result::Result<Vec<DiscoveryImportOperation>, String> {
        if self.schema_version().map_err(string_error)? < REVIEWED_SCHEMA {
            return Ok(Vec::new());
        }
        let mut stmt=self.connection.prepare("SELECT operation_id,length(receipt_json) FROM discovery_import_operations ORDER BY created_at DESC,operation_id DESC LIMIT ?1 OFFSET ?2").map_err(string_error)?;
        let ids = stmt
            .query_map(
                params![
                    limit.min(50) as i64,
                    i64::try_from(offset).map_err(string_error)?
                ],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .map_err(string_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(string_error)?;
        let mut budget = 32 * 1024 * 1024usize;
        let mut operations = Vec::new();
        for (id, len) in ids {
            let size = usize::try_from(len).map_err(string_error)?;
            budget = budget.checked_sub(size).ok_or(
                "History page exceeds its 32 MiB read budget; request fewer import batches.",
            )?;
            operations.push(self.import_operation(&id)?.0);
        }
        Ok(operations)
    }

    fn check_import_undo(
        &self,
        op: &DiscoveryImportOperation,
    ) -> std::result::Result<Vec<String>, String> {
        require_reviewed_schema(&self.connection)?;
        let mut blockers = Vec::new();
        if op.undone {
            return Ok(blockers);
        }
        if op.items.is_empty() {
            blockers.push("This batch inserted no records; nothing can be removed.".into());
        }
        let (count,bytes):(i64,i64)=self.connection.query_row("SELECT count(*),coalesce(sum(length(snapshot_json)),0) FROM discovery_import_records WHERE operation_id=?1",[&op.operation_id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(string_error)?;
        if count < 0
            || count as usize != op.items.len()
            || bytes < 0
            || bytes as usize > MAX_RECEIPT_BYTES
        {
            return Err("Import ownership evidence is incomplete or exceeds its read bound; entire Undo refused.".into());
        }
        for item in &op.items {
            let (path,touched,proof):(Vec<u8>,i64,Option<Vec<u8>>)=self.connection.query_row("SELECT absolute_path,touched,CASE WHEN length(snapshot_json)<=?3 THEN snapshot_json END FROM discovery_import_records WHERE operation_id=?1 AND archive_id=?2",params![op.operation_id,item.archive_id,MAX_RECEIPT_BYTES as i64],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(string_error)?;
            let proof: RecordProof =
                serde_json::from_slice(&proof.ok_or("Oversized ownership evidence")?)
                    .map_err(string_error)?;
            let label = format!("Record {} ({})", item.archive_id, item.path().display());
            if path != item.path_bytes
                || proof.archive.id != item.archive_id
                || proof.assignments.iter().map(|p| p.id).collect::<Vec<_>>()
                    != item.platform_assignment_ids
            {
                return Err("Receipt and record ownership evidence disagree.".into());
            }
            if touched != 0 {
                blockers.push(format!("{label}: record or related evidence was modified after import (including reverted edits, reused IDs or relationship changes)."));
            }
            if touched == 0
                && (row_proof(&self.connection, "archives", item.archive_id)
                    .ok()
                    .as_ref()
                    != Some(&proof.archive)
                    || assignments(&self.connection, item.archive_id).ok().as_ref()
                        != Some(&proof.assignments))
            {
                blockers.push(format!(
                    "{label}: created row or platform assignment changed or is missing."
                ));
                continue;
            }
            for table in [
                "archive_scan_observations",
                "library_dat_identities",
                "verified_identity_facts",
                "screenscraper_enrichments",
                "dat_set_audit_results",
            ] {
                let count: i64 = self
                    .connection
                    .query_row(
                        &format!("SELECT count(*) FROM {table} WHERE archive_id=?1"),
                        [item.archive_id],
                        |r| r.get(0),
                    )
                    .map_err(string_error)?;
                if count != 0 {
                    blockers.push(format!("{label}: {count} dependent {table} rows are protected; none will be deleted."));
                }
            }
        }
        Ok(blockers)
    }
    /// Consistent read transaction; the sealed preview pins this DB object and
    /// exact receipt. A fresh write transaction repeats every check on Confirm.
    pub fn preview_discovery_import_undo(
        &self,
        id: &str,
    ) -> std::result::Result<DiscoveryImportUndoPreview, String> {
        self.connection
            .execute_batch("BEGIN")
            .map_err(string_error)?;
        let r = (|| {
            let object = crate::ingestion::bounded_import::object(&self.path)?;
            let (operation, bytes) = self.import_operation(id)?;
            let blockers = self.check_import_undo(&operation)?;
            self.require_discovery_database_identity(object)?;
            Ok(DiscoveryImportUndoPreview {
                operation_id: operation.operation_id.clone(),
                sealed_items: operation.items.clone(),
                authorised: !operation.undone && !operation.items.is_empty() && blockers.is_empty(),
                operation,
                blockers,
                database: self.path.clone(),
                database_object: object,
                receipt_sha256: digest(&bytes),
                prepared: std::time::Instant::now(),
            })
        })();
        let _ = self.connection.execute_batch("ROLLBACK");
        r
    }
    pub fn undo_discovery_import(
        plan: &DiscoveryImportUndoPreview,
    ) -> std::result::Result<DiscoveryImportUndoResult, String> {
        if plan.prepared.elapsed() > crate::ingestion::bounded_import::REVIEW_LIFETIME {
            return Err("Undo preview expired. Preview and confirm again.".into());
        }
        if crate::ingestion::bounded_import::object(&plan.database)? != plan.database_object {
            return Err("Selected database identity changed; Undo refused.".into());
        }
        let mut db = Self {
            connection: open_connection_with(&plan.database, false).map_err(string_error)?,
            path: plan.database.clone(),
        };
        // No schema migration and no filesystem/media executor is used.
        db.configure_discovery_import_durability()?;
        db.begin_catalogue_refresh().map_err(string_error)?;
        let result = (|| {
            db.require_discovery_database_identity(plan.database_object)?;
            let (op, bytes) = db.import_operation(&plan.operation_id)?;
            if digest(&bytes) != plan.receipt_sha256 {
                return Err("Receipt changed after preview; Undo refused.".into());
            }
            require_reviewed_schema(&db.connection)?;
            if op.undone {
                return Ok(DiscoveryImportUndoResult {
                    operation_id: op.operation_id,
                    removed_records: 0,
                    removed_assignments: 0,
                    already_undone: true,
                });
            }
            if op.items != plan.sealed_items {
                return Err("Import batch membership differs from the sealed Undo preview; no records removed.".into());
            }
            if !plan.can_undo() {
                return Err(
                    "This preview does not authorise Undo. Preview again and review every refusal."
                        .into(),
                );
            }
            let blockers = db.check_import_undo(&op)?;
            if !blockers.is_empty() {
                return Err(format!(
                    "Entire batch refused; no records removed. {}",
                    blockers.join(" ")
                ));
            }
            let mut removed_assignments = 0;
            for item in &op.items {
                for id in &item.platform_assignment_ids {
                    let n = db
                        .connection
                        .execute(
                            "DELETE FROM platform_assignments WHERE id=?1 AND archive_id=?2",
                            params![id, item.archive_id],
                        )
                        .map_err(string_error)?;
                    if n != 1 {
                        return Err("Platform dependency changed; entire Undo rolled back.".into());
                    }
                    removed_assignments += n;
                }
                if db
                    .connection
                    .execute("DELETE FROM archives WHERE id=?1", [item.archive_id])
                    .map_err(string_error)?
                    != 1
                {
                    return Err("Created record changed; entire Undo rolled back.".into());
                }
            }
            #[cfg(test)]
            test_fault(1)?;
            db.require_discovery_database_identity(plan.database_object)?;
            if db.connection.execute("UPDATE discovery_import_operations SET state='undone',undone_at=?2 WHERE operation_id=?1 AND state='committed'",params![op.operation_id,now_utc_string()]).map_err(string_error)?!=1{return Err("Import state changed; entire Undo rolled back.".into());}
            Ok(DiscoveryImportUndoResult {
                operation_id: op.operation_id,
                removed_records: op.items.len(),
                removed_assignments,
                already_undone: false,
            })
        })();
        match result {
            Ok(r) => {
                #[cfg(test)]
                test_fault(2)?;
                if let Err(e) = db.commit_catalogue_refresh() {
                    db.rollback_catalogue_refresh();
                    return Err(format!(
                        "SQLite could not confirm the Undo commit: {e}. Reload durable History to determine the outcome before retrying; use the existing database recovery workflow if required."
                    ));
                }
                #[cfg(test)]
                test_fault(3)?;
                Ok(r)
            }
            Err(e) => {
                db.rollback_catalogue_refresh();
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
thread_local! { static TEST_FAULT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
fn test_fault(stage: u8) -> std::result::Result<(), String> {
    TEST_FAULT.with(|v| match (v.get(), stage) {
        (1, 1) => Err("Injected mid-Undo failure".into()),
        (2, 2) => std::process::exit(92),
        (3, 3) => std::process::exit(93),
        (4, 4) => std::process::exit(94),
        (5, 5) => std::process::exit(95),
        _ => Ok(()),
    })
}

#[cfg(test)]
pub(super) fn import_test_fault(stage: u8) -> std::result::Result<(), String> {
    test_fault(stage)
}
