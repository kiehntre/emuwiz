//! Exact, journal-bound catalogue moves. No scans, registration or migrations.
//! The SQLite writer lock spans the filesystem syscall. Commit failure leaves
//! the sealed entry in flight; reconciliation retries only this bound identity.
use super::*;
use crate::catalogue_health::SourceRootBinding;
use crate::dat::rename_apply::model::{RenameTransaction, TransactionEntry, TransactionOperation};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;

const KEY: &str = "organisation_catalogue_v1";
// Scan-run discovery rows and immutable import receipts retain observed paths.
// Current MAME member evidence remains at its observed path, so it cannot
// silently acquire authority for a different archive/location.
const PROJECTIONS: [(&str, &str); 1] = [("dat_set_audit_results", "archive_path")];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Association {
    folder: i64,
    root: PathBuf,
    root_identity: SourceRootBinding,
    relative: Vec<u8>,
    absolute: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Binding {
    database: PathBuf,
    device: u64,
    inode: u64,
    archive: i64,
    old: Association,
    new: Association,
    projections: Vec<Vec<i64>>,
    fingerprint: bool,
}

fn row_ids(
    c: &Connection,
    table: &str,
    column: &str,
    path: &[u8],
) -> std::result::Result<Vec<i64>, String> {
    let mut stmt = c
        .prepare(&format!(
            "SELECT id FROM {table} WHERE {column}=?1 ORDER BY id"
        ))
        .map_err(|e| e.to_string())?;
    stmt.query_map([path], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| e.to_string())
}
fn fingerprint(c: &Connection, a: &Association) -> std::result::Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM scan_fingerprints WHERE source_folder_id=?1 AND relative_path=?2)", params![a.folder, a.relative], |r| r.get(0)).map_err(|e| e.to_string())
}
fn association(c: &Connection, path: &Path) -> std::result::Result<Association, String> {
    if !path.is_absolute()
        || path.components().any(|p| {
            matches!(
                p,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err("catalogue move requires an absolute, normalized path".into());
    }
    let mut stmt = c.prepare("SELECT id,path FROM source_folders WHERE removed_from_config_at IS NULL AND NOT EXISTS(SELECT 1 FROM source_enablement e WHERE e.source_folder_id=source_folders.id AND e.enabled=0)").map_err(|e| e.to_string())?;
    let roots = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                PathBuf::from(OsString::from_vec(r.get(1)?)),
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut selected = None;
    for root in roots {
        let (folder, root) = root.map_err(|e| e.to_string())?;
        if let Ok(relative) = path.strip_prefix(&root) {
            if selected.as_ref().is_some_and(|a: &Association| {
                a.root.components().count() >= root.components().count()
            }) {
                continue;
            }
            let root_identity =
                SourceRootBinding::inspect(&root).ok_or("catalogue source root is unavailable")?;
            selected = Some(Association {
                folder,
                relative: relative.as_os_str().as_bytes().to_vec(),
                absolute: path.as_os_str().as_bytes().to_vec(),
                root,
                root_identity,
            });
        }
    }
    selected.ok_or_else(|| {
        "destination must be inside an enabled, registered catalogue source folder".into()
    })
}

/// Attach before the first journal publication. Already-bound transactions are
/// refused: rebuilding trust from a changed live catalogue is never recovery.
pub(crate) fn bind(
    transaction: &mut RenameTransaction,
    database: &Database,
) -> std::result::Result<(), String> {
    let path = fs::canonicalize(database.path()).map_err(|e| e.to_string())?;
    let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    let c = &database.connection;
    let mut bindings = Vec::new();
    for entry in &transaction.entries {
        if entry.unknown.contains_key(KEY) {
            return Err("catalogue binding already exists".into());
        }
        if !matches!(entry.operation, TransactionOperation::RenameMove) {
            bindings.push(None);
            continue;
        }
        let old = association(c, &entry.source_path)?;
        let new = association(c, &entry.destination_path)?;
        let ids = row_ids(c, "archives", "absolute_path_cached", &old.absolute)?;
        if ids.len() != 1 {
            return Err(
                "source must have exactly one catalogue identity; regenerate the plan".into(),
            );
        }
        let projections = PROJECTIONS
            .iter()
            .map(|(t, p)| row_ids(c, t, p, &old.absolute))
            .collect::<std::result::Result<_, _>>()?;
        let binding = Binding {
            database: path.clone(),
            device: meta.dev(),
            inode: meta.ino(),
            archive: ids[0],
            fingerprint: fingerprint(c, &old)?,
            old,
            new,
            projections,
        };
        let guard = CatalogueMove::open(binding.clone(), entry, false, false)?;
        drop(guard);
        bindings.push(Some(
            serde_json::to_value(binding).map_err(|e| e.to_string())?,
        ));
    }
    for (entry, binding) in transaction.entries.iter_mut().zip(bindings) {
        if let Some(binding) = binding {
            entry.unknown.insert(KEY.into(), binding);
        }
    }
    Ok(())
}

pub(crate) struct CatalogueMove {
    connection: Connection,
    binding: Binding,
    reverse: bool,
    already: bool,
}
impl Drop for CatalogueMove {
    fn drop(&mut self) {
        let _ = self.connection.execute_batch("ROLLBACK");
    }
}
impl CatalogueMove {
    fn open(
        b: Binding,
        entry: &TransactionEntry,
        reverse: bool,
        recovery: bool,
    ) -> std::result::Result<Self, String> {
        if b.old.absolute != entry.source_path.as_os_str().as_bytes()
            || b.new.absolute != entry.destination_path.as_os_str().as_bytes()
            || b.projections.len() != PROJECTIONS.len()
            || !matches!(entry.operation, TransactionOperation::RenameMove)
        {
            return Err("catalogue binding does not match the sealed move".into());
        }
        let check_db = || -> std::result::Result<(), String> {
            let m = fs::symlink_metadata(&b.database).map_err(|e| e.to_string())?;
            if !m.is_file() || m.dev() != b.device || m.ino() != b.inode {
                return Err("catalogue database identity changed".into());
            }
            Ok(())
        };
        check_db()?;
        let c = open_connection_with(&b.database, false).map_err(|e| e.to_string())?;
        c.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        check_db()?;
        for a in [&b.old, &b.new] {
            let path: Vec<u8> = c.query_row("SELECT path FROM source_folders WHERE id=?1 AND removed_from_config_at IS NULL AND NOT EXISTS(SELECT 1 FROM source_enablement e WHERE e.source_folder_id=source_folders.id AND e.enabled=0)",[a.folder],|r|r.get(0)).map_err(|e|e.to_string())?;
            if path != a.root.as_os_str().as_bytes()
                || SourceRootBinding::inspect(&a.root).as_ref() != Some(&a.root_identity)
                || a.root
                    .join(OsString::from_vec(a.relative.clone()))
                    .as_os_str()
                    .as_bytes()
                    != a.absolute
            {
                return Err("catalogue source folder changed or is unavailable".into());
            }
        }
        let current: (i64,Vec<u8>,Vec<u8>) = c.query_row("SELECT source_folder_id,relative_path,absolute_path_cached FROM archives WHERE id=?1 AND last_verified_missing_at IS NULL",[b.archive],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e|e.to_string())?;
        let observed: (Option<i64>, Option<i64>) = c
            .query_row(
                "SELECT size_bytes,modified_time_unix_seconds FROM archives WHERE id=?1",
                [b.archive],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        if observed
            != (
                Some(i64::try_from(entry.identity.size_bytes).map_err(|e| e.to_string())?),
                Some(entry.identity.modified_unix),
            )
        {
            return Err(
                "catalogue file metadata is stale; regenerate after reviewing the source".into(),
            );
        }
        let contradictory: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM dat_set_audit_results WHERE (archive_path=?1 OR archive_path=?2) AND archive_id IS NOT ?3)",
            params![b.old.absolute,b.new.absolute,b.archive],|r|r.get(0)).map_err(|e|e.to_string())?;
        if contradictory {
            return Err("audit path belongs to another or unassociated catalogue game".into());
        }
        let (from, to) = if reverse {
            (&b.new, &b.old)
        } else {
            (&b.old, &b.new)
        };
        let matches = |a: &Association| {
            current.0 == a.folder && current.1 == a.relative && current.2 == a.absolute
        };
        let already = recovery && matches(to);
        if !matches(from) && !already {
            return Err("catalogue association changed; manual review required".into());
        }
        let (at, free) = if already { (to, from) } else { (from, to) };
        if row_ids(&c, "archives", "absolute_path_cached", &at.absolute)? != vec![b.archive]
            || !row_ids(&c, "archives", "absolute_path_cached", &free.absolute)?.is_empty()
        {
            return Err("catalogue absolute path collision".into());
        }
        let conflict: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM archives WHERE source_folder_id=?1 AND relative_path=?2 AND id<>?3)",params![free.folder,free.relative,b.archive],|r|r.get(0)).map_err(|e|e.to_string())?;
        if conflict {
            return Err("catalogue relative path collision".into());
        }
        for (i, (t, p)) in PROJECTIONS.iter().enumerate() {
            if row_ids(&c, t, p, &at.absolute)? != b.projections[i]
                || !row_ids(&c, t, p, &free.absolute)?.is_empty()
            {
                return Err(format!("catalogue {t} path associations changed"));
            }
        }
        if fingerprint(&c, at)? != b.fingerprint || fingerprint(&c, free)? {
            return Err("catalogue fingerprint path collision or stale association".into());
        }
        Ok(Self {
            connection: c,
            binding: b,
            reverse,
            already,
        })
    }
    pub(crate) fn commit(self) -> std::result::Result<(), String> {
        if !self.already {
            let b = &self.binding;
            let (from, to) = if self.reverse {
                (&b.new, &b.old)
            } else {
                (&b.old, &b.new)
            };
            let file = Path::new(OsStr::from_bytes(&to.absolute))
                .file_name()
                .ok_or("destination has no file name")?;
            let changed = self.connection.execute("UPDATE archives SET source_folder_id=?1,relative_path=?2,absolute_path_cached=?3,file_name_cached=?4 WHERE id=?5 AND source_folder_id=?6 AND relative_path=?7 AND absolute_path_cached=?8",params![to.folder,to.relative,to.absolute,file.as_bytes(),b.archive,from.folder,from.relative,from.absolute]).map_err(|e|e.to_string())?;
            if changed != 1 {
                return Err("catalogue compare-and-swap failed".into());
            }
            if b.fingerprint {
                self.connection.execute("UPDATE scan_fingerprints SET source_folder_id=?1,relative_path=?2 WHERE source_folder_id=?3 AND relative_path=?4",params![to.folder,to.relative,from.folder,from.relative]).map_err(|e|e.to_string())?;
            }
            for (i, (t, p)) in PROJECTIONS.iter().enumerate() {
                for id in &b.projections[i] {
                    let n = self
                        .connection
                        .execute(
                            &format!("UPDATE {t} SET {p}=?1 WHERE id=?2 AND {p}=?3"),
                            params![to.absolute, id, from.absolute],
                        )
                        .map_err(|e| e.to_string())?;
                    if n != 1 {
                        return Err("catalogue projection compare-and-swap failed".into());
                    }
                }
            }
        }
        self.connection
            .execute_batch("COMMIT")
            .map_err(|e| format!("catalogue commit failed; recovery required: {e}"))
    }
}

pub(crate) fn begin(
    entry: &TransactionEntry,
    reverse: bool,
    recovery: bool,
) -> std::result::Result<Option<CatalogueMove>, String> {
    let Some(value) = entry.unknown.get(KEY) else {
        return Ok(None);
    };
    let binding = serde_json::from_value(value.clone())
        .map_err(|e| format!("invalid catalogue binding: {e}"))?;
    CatalogueMove::open(binding, entry, reverse, recovery).map(Some)
}

/// Apply may only use a binding carried from preview, never the current row
/// occupying its pathname. Opening here also checks the supplied DB identity.
pub(crate) fn validate_bound(
    transaction: &RenameTransaction,
    database: &Database,
) -> std::result::Result<(), String> {
    let path = fs::canonicalize(database.path()).map_err(|e| e.to_string())?;
    for entry in &transaction.entries {
        if !matches!(entry.operation, TransactionOperation::RenameMove) {
            continue;
        }
        let value = entry
            .unknown
            .get(KEY)
            .ok_or("organisation preview has no catalogue binding; regenerate it")?;
        let binding: Binding = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if binding.database != path {
            return Err("organisation preview names a different database".into());
        }
        drop(CatalogueMove::open(binding, entry, false, false)?);
    }
    Ok(())
}
