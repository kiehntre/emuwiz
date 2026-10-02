//! "Forget confirmed-missing games": removes dead catalogue rows, nothing else.
//!
//! Eligibility is not re-derived here. A row is `ConfirmedMissing` only when
//! the existing catalogue authority agrees on every link of the chain:
//! [`preview_catalogue_health`] says the path is absent with no move candidate,
//! [`Database::source_health`] says the owning source is `Healthy` (bound,
//! reachable, latest scan for this generation complete), the persisted source
//! enablement says it is enabled and not barred since that scan, and the row
//! carries a `missing` observation written by *that* latest scan. At apply time
//! the write-boundary rule [`assert_missing_authority`] is run again inside the
//! transaction.
//!
//! No file, directory, artwork, save or manual is ever opened for writing. Undo
//! uses a receipt file beside the database (the `restore.rs` precedent); no new
//! schema and no second transaction framework.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::catalogue_health::{assert_missing_authority, latest_actual_attempt};
use super::{ArchiveFsError, Database, Result, db_error, now_utc_string};
use crate::catalogue_health::{
    BoundRoot, CatalogueHealth, ScanCoverageState, SourceHealthState, SourceRootBinding,
    observe_owned, preview_catalogue_health, source_root_identity,
};
use crate::emulator_environment::FsProbe;

/// Prefix of every refusal caused by a plan that no longer matches the database.
pub const FORGET_PLAN_STALE: &str = "STALE PLAN";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingClassification {
    /// The only class the forget operation accepts.
    ConfirmedMissing,
    PossiblyMoved,
    SourceUnavailable,
    ScanIncomplete,
    ReviewRequired,
    NotMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForgetEvidence {
    pub scan_run_id: i64,
    pub source_generation: i64,
    pub missing_recorded_at: String,
}

/// Rows that would be removed with the entry (all catalogue state, no files).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RelatedRows {
    pub platform_assignments: usize,
    pub scan_observations: usize,
    pub dat_identities: usize,
    pub verified_identity_facts: usize,
    pub screenscraper_enrichments: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForgetMissingEntry {
    pub archive_id: i64,
    pub display_name: String,
    pub platform: Option<String>,
    pub source_folder_id: i64,
    #[serde(serialize_with = "lossy_path")]
    pub source_path: PathBuf,
    #[serde(serialize_with = "lossy_path")]
    pub absolute_path: PathBuf,
    pub classification: MissingClassification,
    pub reason: String,
    /// Present exactly for `ConfirmedMissing`.
    pub evidence: Option<ForgetEvidence>,
    /// Rows removed with a confirmed entry. Always empty otherwise.
    pub removes: RelatedRows,
    /// A confirmed entry is journalled in a receipt before removal, so undo can
    /// restore the entry and every row listed in `removes`.
    pub undo_restorable: bool,
}

fn lossy_path<S: serde::Serializer>(path: &Path, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&path.to_string_lossy())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ForgetMissingCounts {
    pub total: usize,
    pub confirmed_missing: usize,
    pub possibly_moved: usize,
    pub source_unavailable: usize,
    pub scan_incomplete: usize,
    pub review_required: usize,
    pub not_missing: usize,
}

/// Read-only, deterministic preview. Carries the catalogue epoch and every
/// piece of evidence it used; apply refuses unless a fresh preview matches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgetMissingPlan {
    pub counts: ForgetMissingCounts,
    /// Every non-`NotMissing` row, ordered by archive id.
    pub entries: Vec<ForgetMissingEntry>,
    database_path: PathBuf,
    epoch: i64,
    configured_roots: Vec<PathBuf>,
}

impl ForgetMissingPlan {
    pub fn confirmed(&self) -> impl Iterator<Item = &ForgetMissingEntry> {
        self.entries
            .iter()
            .filter(|e| e.classification == MissingClassification::ConfirmedMissing)
    }

    /// Identifies exactly this reviewed selection (the confirmed rows, their
    /// evidence and the catalogue epoch). A CLI can hand it back to apply.
    pub fn token(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.database_path.as_os_str().as_bytes());
        hash.update(self.epoch.to_le_bytes());
        for e in self.confirmed() {
            let ev = e.evidence.as_ref().expect("confirmed rows carry evidence");
            hash.update(e.archive_id.to_le_bytes());
            hash.update(e.source_folder_id.to_le_bytes());
            hash.update(e.absolute_path.as_os_str().as_bytes());
            hash.update(ev.scan_run_id.to_le_bytes());
            hash.update(ev.source_generation.to_le_bytes());
            hash.update(ev.missing_recorded_at.as_bytes());
        }
        hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgetMissingResult {
    pub forgotten: usize,
    pub archive_ids: Vec<i64>,
    pub removed: RelatedRows,
    /// `None` when there was nothing to forget.
    pub receipt_path: Option<PathBuf>,
}

struct SourceAuthority {
    health: SourceHealthState,
    enabled: bool,
    barred_through: i64,
    latest: Option<(i64, ScanCoverageState, Option<i64>)>,
}

impl Database {
    /// Classifies every catalogue row and selects the confirmed-missing ones.
    /// Performs no writes.
    pub fn preview_forget_confirmed_missing(
        &self,
        configured_roots: &[PathBuf],
    ) -> Result<ForgetMissingPlan> {
        let report = preview_catalogue_health(self, configured_roots)?;
        let epoch = report.epoch.ok_or_else(|| {
            ArchiveFsError::Database(
                "catalogue authority is unavailable on this schema; nothing can be forgotten"
                    .into(),
            )
        })?;
        let health: HashMap<_, _> = self
            .source_health(configured_roots)?
            .into_iter()
            .map(|h| (h.source_id, h))
            .collect();
        let mut authority = HashMap::new();
        for source in &report.sources {
            let id = source.source_id;
            let enablement: Option<(bool, i64)> = self
                .connection
                .query_row(
                    "SELECT enabled,barred_through_run FROM source_enablement WHERE source_folder_id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| db_error("read source enablement", e))?;
            let latest = latest_actual_attempt(&self.connection, id)?;
            let (enabled, barred_through) = enablement.unwrap_or((true, 0));
            authority.insert(
                id,
                SourceAuthority {
                    health: health
                        .get(&id)
                        .map_or(SourceHealthState::SourceUnavailable, |h| h.state),
                    enabled,
                    barred_through,
                    latest,
                },
            );
        }
        let audited: HashSet<i64> = {
            let mut stmt = self
                .connection
                .prepare("SELECT DISTINCT archive_id FROM dat_set_audit_results WHERE archive_id IS NOT NULL")
                .map_err(|e| db_error("read audit references", e))?;
            stmt.query_map([], |r| r.get(0))
                .and_then(|rows| rows.collect::<std::result::Result<_, _>>())
                .map_err(|e| db_error("read audit references", e))?
        };

        let mut counts = ForgetMissingCounts::default();
        let mut entries = Vec::new();
        for row in &report.rows {
            let a = &row.archive;
            let auth = authority.get(&a.source_folder_id);
            let (classification, reason, evidence) = match row.health {
                CatalogueHealth::PresentVerified | CatalogueHealth::PresentNotVerified => (
                    MissingClassification::NotMissing,
                    "the file is present".to_string(),
                    None,
                ),
                CatalogueHealth::PossiblyMoved => (
                    MissingClassification::PossiblyMoved,
                    format!(
                        "{} possible new location(s) found; review as a move, not a removal",
                        row.move_candidates.len()
                    ),
                    None,
                ),
                CatalogueHealth::OrphanedSource => (
                    MissingClassification::SourceUnavailable,
                    "the owning source is no longer configured".into(),
                    None,
                ),
                CatalogueHealth::NotChecked | CatalogueHealth::Missing => {
                    let confirmed_row = row.health == CatalogueHealth::Missing;
                    classify_absent(
                        a.last_verified_missing_at.as_deref(),
                        confirmed_row,
                        auth,
                        audited.contains(&a.id),
                    )
                }
            };
            counts.total += 1;
            match classification {
                MissingClassification::ConfirmedMissing => counts.confirmed_missing += 1,
                MissingClassification::PossiblyMoved => counts.possibly_moved += 1,
                MissingClassification::SourceUnavailable => counts.source_unavailable += 1,
                MissingClassification::ScanIncomplete => counts.scan_incomplete += 1,
                MissingClassification::ReviewRequired => counts.review_required += 1,
                MissingClassification::NotMissing => {
                    counts.not_missing += 1;
                    continue;
                }
            }
            let confirmed = classification == MissingClassification::ConfirmedMissing;
            let removes = if confirmed {
                self.related_rows(a.id)?
            } else {
                RelatedRows::default()
            };
            let source_path = report
                .sources
                .iter()
                .find(|s| s.source_id == a.source_folder_id)
                .map(|s| s.root.clone())
                .unwrap_or_default();
            entries.push(ForgetMissingEntry {
                archive_id: a.id,
                display_name: a.display_name.clone(),
                platform: a.platform.clone(),
                source_folder_id: a.source_folder_id,
                source_path,
                absolute_path: a.absolute_path.clone(),
                classification,
                reason,
                evidence,
                removes,
                undo_restorable: confirmed,
            });
        }
        if self.catalogue_health_epoch()? != Some(epoch) {
            return Err(ArchiveFsError::Database(
                "the catalogue changed while previewing; preview again".into(),
            ));
        }
        entries.sort_by_key(|e| e.archive_id);
        Ok(ForgetMissingPlan {
            counts,
            entries,
            database_path: self.path.clone(),
            epoch,
            configured_roots: configured_roots.to_vec(),
        })
    }

    fn related_rows(&self, id: i64) -> Result<RelatedRows> {
        let count = |table: &str| -> Result<usize> {
            self.connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE archive_id=?1"),
                    [id],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n as usize)
                .map_err(|e| db_error("count related rows", e))
        };
        Ok(RelatedRows {
            platform_assignments: count("platform_assignments")?,
            scan_observations: count("archive_scan_observations")?,
            dat_identities: count("library_dat_identities")?,
            verified_identity_facts: count("verified_identity_facts")?,
            screenscraper_enrichments: count("screenscraper_enrichments")?,
        })
    }

    /// Applies exactly the reviewed `plan`: all confirmed rows or nothing.
    /// Any difference from a fresh preview is refused as a stale plan, then the
    /// authority is checked again inside the write transaction.
    pub fn apply_forget_confirmed_missing(
        &mut self,
        plan: &ForgetMissingPlan,
    ) -> Result<ForgetMissingResult> {
        if plan.database_path != self.path {
            return Err(ArchiveFsError::Database(
                "plan belongs to another database".into(),
            ));
        }
        let selected: Vec<&ForgetMissingEntry> = plan.confirmed().collect();
        if selected.is_empty() {
            return Ok(ForgetMissingResult {
                forgotten: 0,
                archive_ids: Vec::new(),
                removed: RelatedRows::default(),
                receipt_path: None,
            });
        }
        let fresh = self.preview_forget_confirmed_missing(&plan.configured_roots)?;
        if fresh.epoch != plan.epoch || fresh.token() != plan.token() {
            return Err(stale(
                "the library changed since the preview; preview again",
            ));
        }
        let ids: Vec<i64> = selected.iter().map(|e| e.archive_id).collect();
        let snapshot = self.snapshot_rows(&ids)?;
        let receipt_path = write_new_receipt(&self.path, &plan.token(), &snapshot)?;

        let outcome = self.forget_in_transaction(plan, &selected, &ids);
        match outcome {
            Ok(()) => {
                // A failure to mark the receipt is harmless: undo accepts an
                // unmarked receipt whose rows are all absent.
                let _ = set_receipt_state(&receipt_path, ReceiptState::Applied);
                let mut removed = RelatedRows::default();
                for e in &selected {
                    removed.platform_assignments += e.removes.platform_assignments;
                    removed.scan_observations += e.removes.scan_observations;
                    removed.dat_identities += e.removes.dat_identities;
                    removed.verified_identity_facts += e.removes.verified_identity_facts;
                    removed.screenscraper_enrichments += e.removes.screenscraper_enrichments;
                }
                Ok(ForgetMissingResult {
                    forgotten: ids.len(),
                    archive_ids: ids,
                    removed,
                    receipt_path: Some(receipt_path),
                })
            }
            Err(error) => {
                // Rolled back: nothing was removed, so the journal is void.
                let _ = fs::remove_file(&receipt_path);
                Err(error)
            }
        }
    }

    fn forget_in_transaction(
        &mut self,
        plan: &ForgetMissingPlan,
        selected: &[&ForgetMissingEntry],
        ids: &[i64],
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| db_error("begin forget missing", e))?;
        let epoch: i64 = tx
            .query_row(
                "SELECT revision FROM catalogue_health_epoch WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| db_error("revalidate plan epoch", e))?;
        if epoch != plan.epoch {
            return Err(stale("the catalogue changed since the preview"));
        }
        let mut checked = HashSet::new();
        for e in selected {
            let ev = e.evidence.as_ref().expect("confirmed rows carry evidence");
            if checked.insert(e.source_folder_id) {
                let binding = SourceRootBinding::inspect(&e.source_path)
                    .ok_or_else(|| stale("a source can no longer be inspected"))?;
                assert_missing_authority(
                    &tx,
                    e.source_folder_id,
                    ev.scan_run_id,
                    e.source_path.as_os_str().as_bytes(),
                    &binding,
                )
                .map_err(|error| stale(&error.to_string()))?;
            }
            let current: Option<(Vec<u8>, i64, Option<String>, String)> = tx
                .query_row(
                    "SELECT absolute_path_cached,source_folder_id,last_verified_missing_at,archive_kind FROM archives WHERE id=?1",
                    [e.archive_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_err(|e| db_error("revalidate forget row", e))?;
            let Some((path, source, flag, kind)) = current else {
                return Err(stale("an entry no longer exists"));
            };
            if path != e.absolute_path.as_os_str().as_bytes()
                || source != e.source_folder_id
                || flag.as_deref() != Some(ev.missing_recorded_at.as_str())
            {
                return Err(stale("an entry changed since the preview"));
            }
            let still_absent = observe_owned(
                &e.source_path,
                source_root_identity(&e.source_path),
                &e.absolute_path,
                kind == "arcade_set_directory",
            )
            .probe
                == FsProbe::Missing;
            if !still_absent {
                return Err(stale("a file reappeared since the preview"));
            }
        }
        for id in ids {
            for table in CHILD_TABLES {
                tx.execute(&format!("DELETE FROM {table} WHERE archive_id=?1"), [id])
                    .map_err(|e| db_error("remove related catalogue rows", e))?;
            }
            let removed = tx
                .execute("DELETE FROM archives WHERE id=?1", [id])
                .map_err(|e| db_error("remove catalogue entry", e))?;
            if removed != 1 {
                return Err(stale("an entry changed during removal"));
            }
        }
        // Last look before commit: sources must still be what the plan used.
        for e in selected {
            if BoundRoot::open(&e.source_path).map(|r| r.identity)
                != source_root_identity(&e.source_path)
            {
                return Err(stale("a source changed before commit"));
            }
        }
        tx.commit()
            .map_err(|e| db_error("commit forget missing", e))
    }

    fn snapshot_rows(&self, ids: &[i64]) -> Result<Vec<TableSnapshot>> {
        let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        let mut out = vec![dump(&self.connection, "archives", "id", &list)?];
        for table in CHILD_TABLES {
            out.push(dump(&self.connection, table, "archive_id", &list)?);
        }
        Ok(out)
    }

    /// Restores a forgotten batch from its receipt, all or nothing. Refuses if
    /// already restored, if any id or path is occupied again, or if its source
    /// folder is gone.
    pub fn undo_forget_missing(&mut self, receipt_path: &Path) -> Result<usize> {
        let mut receipt = read_receipt(receipt_path)?;
        if receipt.database_path != self.path {
            return Err(ArchiveFsError::Database(
                "receipt belongs to another database".into(),
            ));
        }
        if receipt.state == ReceiptState::Undone {
            return Err(ArchiveFsError::Database(
                "this forget operation was already undone".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| db_error("begin undo forget", e))?;
        let archives = receipt
            .tables
            .iter()
            .find(|t| t.table == "archives")
            .ok_or_else(|| ArchiveFsError::Database("receipt has no catalogue rows".into()))?;
        let col = |name: &str| archives.columns.iter().position(|c| c == name);
        let (id_i, src_i, rel_i, abs_i) = (
            col("id"),
            col("source_folder_id"),
            col("relative_path"),
            col("absolute_path_cached"),
        );
        let (Some(id_i), Some(src_i), Some(rel_i), Some(abs_i)) = (id_i, src_i, rel_i, abs_i)
        else {
            return Err(ArchiveFsError::Database("receipt is malformed".into()));
        };
        for row in &archives.rows {
            let id = row[id_i].as_i64().unwrap_or(-1);
            let exists = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Result<bool> {
                tx.query_row(sql, p, |r| r.get(0))
                    .map_err(|e| db_error("check undo collision", e))
            };
            if exists("SELECT EXISTS(SELECT 1 FROM archives WHERE id=?1)", &[&id])? {
                return Err(ArchiveFsError::Database(format!(
                    "catalogue id {id} is occupied again; nothing was restored"
                )));
            }
            let source = row[src_i].as_i64().unwrap_or(-1);
            if !exists(
                "SELECT EXISTS(SELECT 1 FROM source_folders WHERE id=?1 AND removed_from_config_at IS NULL)",
                &[&source],
            )? {
                return Err(ArchiveFsError::Database(format!(
                    "source folder {source} is gone; nothing was restored"
                )));
            }
            let (rel, abs) = (decode_blob(&row[rel_i]), decode_blob(&row[abs_i]));
            if exists(
                "SELECT EXISTS(SELECT 1 FROM archives WHERE (source_folder_id=?1 AND relative_path=?2) OR absolute_path_cached=?3)",
                &[&source, &rel, &abs],
            )? {
                return Err(ArchiveFsError::Database(
                    "the path is occupied by another catalogue entry; nothing was restored".into(),
                ));
            }
        }
        for table in &receipt.tables {
            let names = table.columns.join(",");
            let marks = vec!["?"; table.columns.len()].join(",");
            let sql = format!("INSERT INTO {} ({names}) VALUES ({marks})", table.table);
            for row in &table.rows {
                let values: Vec<rusqlite::types::Value> = row.iter().map(to_sql).collect();
                tx.execute(&sql, rusqlite::params_from_iter(values))
                    .map_err(|e| db_error("restore catalogue rows", e))?;
            }
        }
        let restored = archives.rows.len();
        tx.commit().map_err(|e| db_error("commit undo forget", e))?;
        receipt.state = ReceiptState::Undone;
        // If this fails the rows exist again, so a repeat undo collides and refuses.
        let _ = write_receipt_state(receipt_path, &receipt);
        Ok(restored)
    }
}

/// Tables owned by an archive row; removed with it and restored with it. DAT set
/// audit verdicts also reference archives but are a separate dataset: an entry
/// they reference is `ReviewRequired`, never silently stripped.
const CHILD_TABLES: [&str; 5] = [
    "screenscraper_enrichments",
    "library_dat_identities",
    "verified_identity_facts",
    "platform_assignments",
    "archive_scan_observations",
];

fn classify_absent(
    flag: Option<&str>,
    path_missing: bool,
    auth: Option<&SourceAuthority>,
    audited: bool,
) -> (MissingClassification, String, Option<ForgetEvidence>) {
    use MissingClassification::*;
    let no = |c, why: &str| (c, why.to_string(), None);
    let Some(auth) = auth else {
        return no(SourceUnavailable, "the owning source is not known");
    };
    match auth.health {
        SourceHealthState::SourceUnavailable => {
            return no(
                SourceUnavailable,
                "the source folder cannot be reached right now",
            );
        }
        SourceHealthState::RebindRequired => {
            return no(
                ReviewRequired,
                "the source's storage must be reviewed before it is trusted",
            );
        }
        SourceHealthState::NeedsScan => {
            return no(ScanIncomplete, "no scan has covered this source yet");
        }
        SourceHealthState::PartialScan | SourceHealthState::CoverageIncomplete => {
            return no(
                ScanIncomplete,
                "the latest scan did not fully cover this source",
            );
        }
        SourceHealthState::NotGameScanned => {
            return no(ScanIncomplete, "this source is not scanned for games");
        }
        SourceHealthState::Healthy => {}
    }
    if !auth.enabled {
        return no(SourceUnavailable, "the source is disabled");
    }
    let Some((run, ScanCoverageState::Complete, Some(generation))) = auth.latest else {
        return no(ScanIncomplete, "the latest scan attempt was not complete");
    };
    if run <= auth.barred_through {
        return no(
            ScanIncomplete,
            "the source was disabled after its latest scan; a fresh scan is required",
        );
    }
    if !path_missing {
        return no(
            ScanIncomplete,
            "absence cannot be proven for this entry right now",
        );
    }
    // A later complete scan clears the flag of any file it sees, so a flag that
    // survives the latest complete scan means that scan did not see the entry.
    let Some(recorded) = flag else {
        return no(
            ScanIncomplete,
            "no complete scan has recorded this entry as absent",
        );
    };
    if audited {
        return no(
            ReviewRequired,
            "a DAT set audit result references this entry; review it before removal",
        );
    }
    (
        ConfirmedMissing,
        format!(
            "absent from source folder at the latest complete scan (run {run}); \
             no moved copy found; source healthy and enabled"
        ),
        Some(ForgetEvidence {
            scan_run_id: run,
            source_generation: generation,
            missing_recorded_at: recorded.to_string(),
        }),
    )
}

fn stale(why: &str) -> ArchiveFsError {
    ArchiveFsError::Database(format!("{FORGET_PLAN_STALE}: {why}; nothing was removed"))
}

// --- receipt journal -------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReceiptState {
    Pending,
    Applied,
    Undone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TableSnapshot {
    table: String,
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ForgetReceipt {
    version: u32,
    state: ReceiptState,
    database_path: PathBuf,
    created_at: String,
    tables: Vec<TableSnapshot>,
}

fn dump(connection: &Connection, table: &str, column: &str, ids: &str) -> Result<TableSnapshot> {
    let mut stmt = connection
        .prepare(&format!(
            "SELECT * FROM {table} WHERE {column} IN ({ids}) ORDER BY rowid"
        ))
        .map_err(|e| db_error("snapshot rows", e))?;
    let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let rows = stmt
        .query_map([], |r| {
            (0..columns.len())
                .map(|i| {
                    Ok(match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => Value::from(n),
                        rusqlite::types::ValueRef::Real(f) => Value::from(f),
                        rusqlite::types::ValueRef::Text(t) => {
                            Value::String(String::from_utf8_lossy(t).into_owned())
                        }
                        rusqlite::types::ValueRef::Blob(b) => serde_json::json!({
                            "blob": b.iter().map(|x| format!("{x:02x}")).collect::<String>()
                        }),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(|e| db_error("snapshot rows", e))?;
    Ok(TableSnapshot {
        table: table.into(),
        columns,
        rows,
    })
}

fn decode_blob(value: &Value) -> Vec<u8> {
    let hex = value.get("blob").and_then(Value::as_str).unwrap_or("");
    (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

fn to_sql(value: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as Sql;
    match value {
        Value::Null => Sql::Null,
        Value::Number(n) => n
            .as_i64()
            .map_or_else(|| Sql::Real(n.as_f64().unwrap_or(0.0)), Sql::Integer),
        Value::String(s) => Sql::Text(s.clone()),
        Value::Object(_) => Sql::Blob(decode_blob(value)),
        other => Sql::Text(other.to_string()),
    }
}

fn receipt_error(what: &str, error: impl std::fmt::Display) -> ArchiveFsError {
    ArchiveFsError::Database(format!("forget receipt: could not {what}: {error}"))
}

fn write_new_receipt(db: &Path, token: &str, tables: &[TableSnapshot]) -> Result<PathBuf> {
    let name = db.file_name().unwrap_or_default().to_string_lossy();
    let unix = now_utc_string().replace([':', '-'], "");
    let path = db.with_file_name(format!(
        "{name}.forget-missing-{unix}-{}.json",
        &token[..12]
    ));
    let receipt = ForgetReceipt {
        version: 1,
        state: ReceiptState::Pending,
        database_path: db.to_path_buf(),
        created_at: now_utc_string(),
        tables: tables.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&receipt).map_err(|e| receipt_error("encode", e))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| receipt_error("create", e))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| receipt_error("write", e))?;
    Ok(path)
}

fn read_receipt(path: &Path) -> Result<ForgetReceipt> {
    let bytes = fs::read(path).map_err(|e| receipt_error("read", e))?;
    serde_json::from_slice(&bytes).map_err(|e| receipt_error("parse", e))
}

fn write_receipt_state(path: &Path, receipt: &ForgetReceipt) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|e| receipt_error("encode", e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)
        .and_then(|_| fs::rename(&tmp, path))
        .map_err(|e| receipt_error("update", e))
}

fn set_receipt_state(path: &Path, state: ReceiptState) -> Result<()> {
    let mut receipt = read_receipt(path)?;
    receipt.state = state;
    write_receipt_state(path, &receipt)
}
