//! Catalogue cleanup of missing archive records, bound to what the user chose.
//!
//! What this guarantees, and where it stops:
//!
//! * **Identity.** `archives.id` is a plain `INTEGER PRIMARY KEY`, so SQLite may
//!   hand a deleted row's id to a later archive. Path selections (GUI, CLI
//!   `--path`) are resolved to records *inside* the reserved write transaction
//!   and every identity field the caller holds (id, path, source folder,
//!   relative path) must match the stored record; a stale selection is refused
//!   as a whole. An explicit `--id` is the caller's own statement and has no
//!   further identity to compare, so it is only as current as the id the user
//!   typed: the freshness checks below still apply to whatever record holds it.
//! * **Verified deletion.** Each DELETE must affect exactly the rows counted
//!   beforehand, no related row may remain, and no other archive or child row
//!   may have changed (a trigger that suppresses, widens or redirects a delete is
//!   an error, not a success). Any failure drops the transaction.
//! * **Audit evidence.** Persisted arcade DAT audit verdicts
//!   (`dat_set_audit_results`) are separate, path-keyed historical evidence. They
//!   are never deleted or silently detached by cleanup; a selection that still has
//!   any is refused whole, as in the forget-missing flow.
//! * **Freshness.** Source authority (scan generation, binding, ownership,
//!   availability) and path absence are checked under SQLite's writer
//!   reservation and repeated immediately before commit.
//!
//! Limitations, stated plainly:
//!
//! * SQLite's writer lock serialises *database* writers only. Other EmuWiz
//!   processes that write the catalogue are excluded while the transaction is
//!   open, but they cannot be made to wait before the transaction starts, and
//!   any tool that writes the file without taking the lock is outside this
//!   guarantee.
//! * Filesystem writers do not take part in SQLite locking. A file can still
//!   appear between the final no-symlink probe and the commit; the window is
//!   small and the consequence is a catalogue row removed for a file that a
//!   later scan will simply re-add. No game file is ever written or deleted.
//! * The trigger/constraint checks detect deletes that were suppressed or
//!   widened; they cannot reason about arbitrary side effects an AFTER trigger
//!   makes to unrelated tables other than the ones listed in `CHILD_TABLES`.

use super::*;
use crate::catalogue_health::BoundRoot;
use crate::emulator_environment::FsProbe;

/// Identity evidence for one cleanup selection. `archives.id` is a plain
/// `INTEGER PRIMARY KEY` (no AUTOINCREMENT), so SQLite reuses an id after its
/// row is deleted: an id alone cannot say *which* catalogue record the user
/// chose. Targets therefore carry what the user actually selected, and it is
/// resolved and compared **inside** the reserved write transaction.
///
/// * `path` only: resolved to a record in the transaction (GUI/CLI `--path`).
/// * `id` only: explicit-id compatibility (CLI `--id`); the id is the user's
///   statement and no further identity is available to compare.
/// * any combination: every field supplied must match the stored record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingCleanupTarget {
    pub archive_id: Option<i64>,
    pub absolute_path: Option<PathBuf>,
    pub source_folder_id: Option<i64>,
    pub relative_path: Option<PathBuf>,
}

impl MissingCleanupTarget {
    pub fn by_id(archive_id: i64) -> Self {
        Self {
            archive_id: Some(archive_id),
            ..Self::default()
        }
    }

    pub fn by_path(path: impl Into<PathBuf>) -> Self {
        Self {
            absolute_path: Some(path.into()),
            ..Self::default()
        }
    }

    /// Everything a snapshot knows about the record the user saw and selected.
    pub fn exact(archive: &PersistedArchive) -> Self {
        Self {
            archive_id: Some(archive.id),
            absolute_path: Some(archive.absolute_path.clone()),
            source_folder_id: Some(archive.source_folder_id),
            relative_path: Some(archive.relative_path.clone()),
        }
    }

    fn describe(&self) -> String {
        match (&self.absolute_path, self.archive_id) {
            (Some(path), Some(id)) => format!("{} (id {id})", path.display()),
            (Some(path), None) => path.display().to_string(),
            (None, Some(id)) => format!("id {id}"),
            (None, None) => "an empty selection".into(),
        }
    }
}

/// Owned child tables, deleted explicitly (never by an implicit cascade) and
/// verified row-for-row. Same set the forget-missing flow owns.
const CHILD_TABLES: [&str; 5] = [
    "screenscraper_enrichments",
    "library_dat_identities",
    "verified_identity_facts",
    "platform_assignments",
    "archive_scan_observations",
];

struct Candidate {
    id: i64,
    source: i64,
    run: i64,
    root: BoundRoot,
    path: PathBuf,
    directory: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Reviewed,
    Deleted,
}

fn refused(reason: impl std::fmt::Display) -> ArchiveFsError {
    ArchiveFsError::Database(format!(
        "Catalogue cleanup refused: {reason}. Historical Missing is not current cleanup eligibility; no catalogue entries were removed"
    ))
}

pub(super) fn remove(db: &mut Database, ids: &[i64]) -> Result<MissingArchiveRemovalResult> {
    let targets: Vec<_> = ids
        .iter()
        .copied()
        .map(MissingCleanupTarget::by_id)
        .collect();
    remove_targets(db, &targets)
}

pub(super) fn remove_targets(
    db: &mut Database,
    targets: &[MissingCleanupTarget],
) -> Result<MissingArchiveRemovalResult> {
    remove_with_hooks(
        db,
        targets,
        |root, path, directory| root.probe(path, directory).probe,
        |_, _| Ok(()),
    )
}

// Private deterministic test seams: production always uses the real pinned
// probe and no callback. No caller can inject observation evidence through the
// public cleanup API.
fn remove_with_hooks(
    db: &mut Database,
    targets: &[MissingCleanupTarget],
    mut probe: impl FnMut(&BoundRoot, &Path, bool) -> FsProbe,
    mut boundary: impl FnMut(Boundary, &Connection) -> Result<()>,
) -> Result<MissingArchiveRemovalResult> {
    if targets.is_empty() {
        return Err(refused("at least one archive id or path is required"));
    }
    // Reserve the writer BEFORE reading any source, row or generation evidence.
    let tx = db
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| db_error("reserve catalogue cleanup transaction", e))?;
    catalogue_health::lock_missing_authority(&tx)?;
    // Resolve every selection to a record NOW, under the writer reservation, and
    // compare the identity the user selected with what is stored. A path or
    // snapshot that points at a record which was deleted and whose id was reused
    // by another archive is refused here, before anything is read or removed.
    let mut ids = Vec::with_capacity(targets.len());
    for target in targets {
        let id = resolve_target(&tx, target)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let mut candidates = Vec::new();
    for id in &ids {
        let row: Option<(i64, Vec<u8>, Vec<u8>, String, Option<String>, Vec<u8>)> = tx.query_row(
            "SELECT a.source_folder_id,a.absolute_path_cached,a.relative_path,a.archive_kind,a.last_verified_missing_at,s.path \
             FROM archives a JOIN source_folders s ON s.id=a.source_folder_id WHERE a.id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))
        ).optional().map_err(|e| db_error("read cleanup candidate", e))?;
        let Some((source, bytes, relative, kind, missing_at, root_bytes)) = row else {
            return Err(refused(format!("archive id {id} not found")));
        };
        if missing_at.is_none() {
            return Err(refused(format!(
                "archive id {id} is currently present in the catalogue"
            )));
        }
        let path = PathBuf::from(OsString::from_vec(bytes));
        let root_path = PathBuf::from(OsString::from_vec(root_bytes));
        let relative = PathBuf::from(OsString::from_vec(relative));
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || root_path.join(&relative) != path
        {
            return Err(refused(format!(
                "archive id {id} has inconsistent source/path identity"
            )));
        }
        let root = BoundRoot::open(&root_path).ok_or_else(|| {
            refused(format!(
                "source {} is unavailable, unreadable or replaced",
                root_path.display()
            ))
        })?;
        let (run, _, _) = catalogue_health::latest_actual_attempt(&tx, source)?
            .ok_or_else(|| refused("source lacks complete generation evidence"))?;
        candidates.push(Candidate {
            id: *id,
            source,
            run,
            root,
            path,
            directory: kind == "arcade_set_directory",
        });
    }
    validate(&tx, &candidates, &mut probe)?;
    boundary(Boundary::Reviewed, &tx)?;
    // Recheck even if a file returned after the initial review inside this call.
    validate(&tx, &candidates, &mut probe)?;
    delete_verified(&tx, &ids)?;
    boundary(Boundary::Deleted, &tx)?;
    // A late failure drops the transaction, restoring all rows and metadata.
    validate(&tx, &candidates, &mut probe)?;
    tx.commit()
        .map_err(|e| db_error("commit catalogue cleanup", e))?;
    Ok(MissingArchiveRemovalResult {
        requested: ids.len(),
        removed: ids.len(),
        archive_ids: ids,
    })
}

/// Resolves one selection to an archive id inside the write transaction and
/// proves the stored record is the one the user selected.
fn resolve_target(tx: &Connection, target: &MissingCleanupTarget) -> Result<i64> {
    let id = match (&target.absolute_path, target.archive_id) {
        (None, None) => return Err(refused("a selection needs an archive id or a path")),
        (None, Some(id)) => id,
        (Some(path), expected_id) => {
            let mut statement = tx
                .prepare("SELECT id FROM archives WHERE absolute_path_cached=?1")
                .map_err(|e| db_error("prepare cleanup path lookup", e))?;
            let found: Vec<i64> = statement
                .query_map([path.as_os_str().as_bytes()], |r| r.get(0))
                .and_then(|rows| rows.collect::<rusqlite::Result<_>>())
                .map_err(|e| db_error("look up cleanup selection by path", e))?;
            match (found.as_slice(), expected_id) {
                ([], _) => {
                    return Err(refused(format!(
                        "no archive found with exact path {}: no catalogue record exists at that path",
                        path.display()
                    )));
                }
                ([id], None) => *id,
                ([id], Some(expected)) if *id == expected => *id,
                ([id], Some(expected)) => {
                    return Err(refused(format!(
                        "{} is now catalogue record {id}, not the record {expected} that was selected; the selection is stale",
                        path.display()
                    )));
                }
                (_, _) => {
                    return Err(refused(format!(
                        "{} matches more than one catalogue record",
                        path.display()
                    )));
                }
            }
        }
    };
    let stored: Option<(i64, Vec<u8>, Vec<u8>)> = tx
        .query_row(
            "SELECT source_folder_id,relative_path,absolute_path_cached FROM archives WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| db_error("read cleanup selection identity", e))?;
    let Some((source, relative, absolute)) = stored else {
        return Err(refused(format!("archive id {id} not found")));
    };
    let stale = |what: &str| {
        refused(format!(
            "{} no longer matches catalogue record {id}: its {what} changed; the selection is stale",
            target.describe()
        ))
    };
    if target
        .source_folder_id
        .is_some_and(|expected| expected != source)
    {
        return Err(stale("source folder"));
    }
    if target
        .relative_path
        .as_ref()
        .is_some_and(|expected| expected.as_os_str().as_bytes() != relative.as_slice())
    {
        return Err(stale("relative path"));
    }
    if target
        .absolute_path
        .as_ref()
        .is_some_and(|expected| expected.as_os_str().as_bytes() != absolute.as_slice())
    {
        return Err(stale("path"));
    }
    Ok(id)
}

fn count(tx: &Connection, sql: &str, id: Option<i64>) -> Result<i64> {
    let result = match id {
        Some(id) => tx.query_row(sql, [id], |r| r.get(0)),
        None => tx.query_row(sql, [], |r| r.get(0)),
    };
    result.map_err(|e| db_error("count cleanup rows", e))
}

/// Deletes the selected records and every owned child row, and proves it:
/// each statement must affect exactly the rows counted beforehand, nothing may
/// remain, and nothing belonging to any other archive may have changed. A
/// trigger or constraint that suppresses or widens a delete therefore fails
/// the operation (the caller's transaction is dropped, restoring everything)
/// instead of being reported as a successful removal.
///
/// Persisted arcade DAT audit verdicts (`dat_set_audit_results`) are a separate,
/// path-keyed evidence dataset that merely links to an archive. The policy,
/// matching the forget-missing flow, is that they are never discarded or
/// silently detached by cleanup: a selection that still has any is refused.
fn delete_verified(tx: &Connection, ids: &[i64]) -> Result<()> {
    for id in ids {
        let audits = count(
            tx,
            "SELECT COUNT(*) FROM dat_set_audit_results WHERE archive_id=?1",
            Some(*id),
        )?;
        if audits > 0 {
            return Err(refused(format!(
                "archive id {id} has {audits} persisted arcade DAT audit result(s); cleanup does not discard historical audit evidence. Review it first",
            )));
        }
    }
    let selected_sum = ids.iter().try_fold(0_i64, |sum, id| {
        sum.checked_add(*id)
            .ok_or_else(|| refused("selected record identity total overflowed"))
    })?;
    let archives_before = count(tx, "SELECT COUNT(*) FROM archives", None)?;
    let sum_before = count(tx, "SELECT COALESCE(SUM(id),0) FROM archives", None)?;
    let mut child_totals = Vec::new();
    for table in CHILD_TABLES {
        child_totals.push(count(tx, &format!("SELECT COUNT(*) FROM {table}"), None)?);
    }
    let mut child_removed = vec![0_i64; CHILD_TABLES.len()];
    for id in ids {
        for (index, table) in CHILD_TABLES.iter().enumerate() {
            let expected = count(
                tx,
                &format!("SELECT COUNT(*) FROM {table} WHERE archive_id=?1"),
                Some(*id),
            )?;
            let affected = tx
                .execute(&format!("DELETE FROM {table} WHERE archive_id=?1"), [id])
                .map_err(|e| db_error("remove freshly verified missing catalogue record", e))?;
            if affected as i64 != expected {
                return Err(refused(format!(
                    "removing {table} rows for archive id {id} affected {affected} of {expected} row(s)"
                )));
            }
            child_removed[index] += expected;
        }
        let affected = tx
            .execute("DELETE FROM archives WHERE id=?1", [id])
            .map_err(|e| db_error("remove freshly verified missing catalogue record", e))?;
        if affected != 1 {
            return Err(refused(format!(
                "removing archive id {id} affected {affected} row(s) instead of 1; the delete was suppressed or widened"
            )));
        }
    }
    // Postconditions, independent of the statement counts above.
    for id in ids {
        let mut remaining = count(tx, "SELECT COUNT(*) FROM archives WHERE id=?1", Some(*id))?;
        for table in CHILD_TABLES {
            remaining += count(
                tx,
                &format!("SELECT COUNT(*) FROM {table} WHERE archive_id=?1"),
                Some(*id),
            )?;
        }
        remaining += count(
            tx,
            "SELECT COUNT(*) FROM dat_set_audit_results WHERE archive_id=?1",
            Some(*id),
        )?;
        if remaining != 0 {
            return Err(refused(format!(
                "archive id {id} still has {remaining} related row(s) after removal"
            )));
        }
    }
    let archives_after = count(tx, "SELECT COUNT(*) FROM archives", None)?;
    let sum_after = count(tx, "SELECT COALESCE(SUM(id),0) FROM archives", None)?;
    if archives_after != archives_before - ids.len() as i64
        || sum_after != sum_before - selected_sum
    {
        return Err(refused(
            "the catalogue lost or gained records other than the selection; nothing was removed",
        ));
    }
    for (index, table) in CHILD_TABLES.iter().enumerate() {
        let after = count(tx, &format!("SELECT COUNT(*) FROM {table}"), None)?;
        if after != child_totals[index] - child_removed[index] {
            return Err(refused(format!(
                "{table} changed beyond the selected records; nothing was removed"
            )));
        }
    }
    Ok(())
}

fn validate(
    connection: &Connection,
    candidates: &[Candidate],
    probe: &mut impl FnMut(&BoundRoot, &Path, bool) -> FsProbe,
) -> Result<()> {
    for c in candidates {
        let root = c.root.root_path();
        if !c.root.current() || fs::read_dir(root).is_err() {
            return Err(refused(format!(
                "source {} became unavailable, unreadable or replaced",
                root.display()
            )));
        }
        catalogue_health::assert_missing_authority(
            connection,
            c.source,
            c.run,
            root.as_os_str().as_bytes(),
            &c.root.binding,
        )
        .map_err(|e| refused(format!("source authority is not current: {e}")))?;
        let finished: bool = connection
            .query_row(
                "SELECT status IN ('completed','partial') AND finished_at IS NOT NULL FROM scan_runs WHERE id=?1",
                [c.run],
                |r| r.get(0),
            )
            .map_err(|e| db_error("read cleanup scan completion", e))?;
        if !finished {
            return Err(refused(
                "source observation run was not successfully finished",
            ));
        }
        if !catalogue_health::unproven_nested_boundaries_on(connection, c.source, &c.root)?
            .is_empty()
        {
            return Err(refused("source has an unproven nested filesystem boundary"));
        }
        let (identity_json, excluded_json): (String, String) = connection.query_row(
            "SELECT root_identity_json,excluded_roots_json FROM scan_source_coverage WHERE source_folder_id=?1 AND scan_run_id=?2",
            params![c.source, c.run], |r| Ok((r.get(0)?, r.get(1)?))
        ).map_err(|e| db_error("read cleanup coverage", e))?;
        let identity: Option<(u64, u64)> =
            serde_json::from_str(&identity_json).map_err(|e| refused(e))?;
        if identity != Some(c.root.identity) {
            return Err(refused("coverage root identity changed"));
        }
        let recorded: Vec<Vec<u8>> =
            serde_json::from_str(&excluded_json).map_err(|e| refused(e))?;
        let mut recorded: Vec<PathBuf> = recorded
            .into_iter()
            .map(|b| PathBuf::from(OsString::from_vec(b)))
            .collect();
        let mut stmt = connection
            .prepare(
                "SELECT path FROM source_folders WHERE id!=?1 AND removed_from_config_at IS NULL",
            )
            .map_err(|e| db_error("read current cleanup ownership", e))?;
        let roots = stmt
            .query_map([c.source], |r| r.get::<_, Vec<u8>>(0))
            .map_err(|e| db_error("read cleanup source roots", e))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| db_error("decode cleanup source roots", e))?;
        let mut current: Vec<PathBuf> = roots
            .into_iter()
            .map(|b| PathBuf::from(OsString::from_vec(b)))
            .filter(|p| p.starts_with(root))
            .collect();
        current.sort();
        recorded.sort();
        if current != recorded || current.iter().any(|p| c.path.starts_with(p)) {
            return Err(refused(
                "source ownership changed or candidate belongs to a delegated source",
            ));
        }
        match probe(&c.root, &c.path, c.directory) {
            FsProbe::Missing => {}
            FsProbe::PresentFile | FsProbe::PresentDirectory => {
                return Err(refused(format!(
                    "archive id {} is currently present on storage",
                    c.id
                )));
            }
            state => {
                return Err(refused(format!(
                    "archive id {} cannot be safely observed ({state:?})",
                    c.id
                )));
            }
        }
        if !c.root.current() {
            return Err(refused("source changed during cleanup inspection"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
