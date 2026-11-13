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
/// Marks a set member (CUE/GDI track, M3U disc) that moves with its lead in
/// the same transaction: `{"lead": "<lead source path>"}`.
pub const COMPANION_KEY: &str = "organisation_companion_v1";
/// Marks a game the person picked that the library has no row for (for
/// example a disc set outside a PlayStation folder): it moves with no
/// catalogue path to change.
pub const UNCATALOGUED_KEY: &str = "organisation_uncatalogued_v1";
/// Optional request, set by the platform-correction workflow before binding:
/// `{"platform": "<canonical id>", "source": "verified_dat" | "header_identity"}`.
/// The new assignment commits in the same SQLite transaction as the path
/// change and Undo restores the assignment that was current at bind time.
pub const PLATFORM_CHANGE_KEY: &str = "organisation_platform_v1";
const PLATFORM_CHANGE_SOURCES: [&str; 2] = [VERIFIED_DAT_PLATFORM_SOURCE, "header_identity"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PlatformChange {
    platform: String,
    source: String,
    /// The `platform_assignments` row current at bind time, if any.
    previous: Option<i64>,
}

#[derive(Deserialize)]
struct PlatformChangeRequest {
    platform: String,
    source: String,
}

fn current_assignment(
    c: &Connection,
    archive: i64,
) -> std::result::Result<Option<(i64, String, String)>, String> {
    c.query_row(
        "SELECT id,platform,source FROM platform_assignments WHERE archive_id=?1 AND is_current=1",
        [archive],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .map_err(|e| e.to_string())
}
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
    #[serde(default)]
    platform: Option<PlatformChange>,
}

impl Binding {
    fn verify_storage(&self) -> std::result::Result<(), String> {
        let m = fs::symlink_metadata(&self.database)
            .map_err(|e| format!("catalogue database unavailable; recovery required: {e}"))?;
        if !m.is_file() || m.dev() != self.device || m.ino() != self.inode {
            return Err("catalogue database identity changed; recovery required".into());
        }
        for a in [&self.old, &self.new] {
            if SourceRootBinding::inspect(&a.root).as_ref() != Some(&a.root_identity) {
                return Err("catalogue source root identity changed; recovery required".into());
            }
        }
        Ok(())
    }
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
        // A set's track or disc file with no library row of its own moves
        // with its set and has no catalogue path to change.
        let companion = entry.unknown.contains_key(COMPANION_KEY)
            || entry.unknown.contains_key(UNCATALOGUED_KEY);
        if companion && ids.is_empty() {
            if !row_ids(c, "archives", "absolute_path_cached", &new.absolute)?.is_empty() {
                return Err("catalogue absolute path collision".into());
            }
            bindings.push(None);
            continue;
        }
        if companion && entry.unknown.contains_key(PLATFORM_CHANGE_KEY) {
            return Err("a set member cannot carry its own platform change".into());
        }
        if ids.len() != 1 {
            return Err(
                "source must have exactly one catalogue identity; regenerate the plan".into(),
            );
        }
        let projections = PROJECTIONS
            .iter()
            .map(|(t, p)| row_ids(c, t, p, &old.absolute))
            .collect::<std::result::Result<_, _>>()?;
        let platform = match entry.unknown.get(PLATFORM_CHANGE_KEY) {
            None => None,
            Some(value) => {
                let request: PlatformChangeRequest = serde_json::from_value(value.clone())
                    .map_err(|e| format!("invalid platform change: {e}"))?;
                if !PLATFORM_CHANGE_SOURCES.contains(&request.source.as_str())
                    || crate::platform::platform_by_id(&request.platform).is_none()
                {
                    return Err(
                        "platform change must name a registry platform and evidence source".into(),
                    );
                }
                let current = current_assignment(c, ids[0])?;
                if current.as_ref().is_some_and(|(_, platform, source)| {
                    source == MANUAL_PLATFORM_SOURCE && *platform != request.platform
                }) {
                    return Err(
                        "this game has a manual system choice that the new evidence contradicts; \
                         review it before moving"
                            .into(),
                    );
                }
                Some(PlatformChange {
                    platform: request.platform,
                    source: request.source,
                    previous: current.map(|(id, _, _)| id),
                })
            }
        };
        let binding = Binding {
            database: path.clone(),
            device: meta.dev(),
            inode: meta.ino(),
            archive: ids[0],
            fingerprint: fingerprint(c, &old)?,
            old,
            new,
            projections,
            platform,
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
        b.verify_storage()?;
        let c = open_connection_with(&b.database, false).map_err(|e| e.to_string())?;
        c.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        b.verify_storage()?;
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
        if let Some(change) = &b.platform {
            let current = current_assignment(&c, b.archive)?;
            let applied = current.as_ref().is_some_and(|(_, platform, source)| {
                *platform == change.platform && *source == change.source
            });
            // Forward and not yet committed: the bind-time row is still current.
            // Reverse, or recovery after commit: the new row is current.
            let expect_applied = reverse != already;
            let ok = if expect_applied {
                applied
            } else {
                current.map(|(id, _, _)| id) == change.previous
            };
            if !ok {
                return Err("platform assignment changed since preview; regenerate it".into());
            }
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
        // SQLite serializes database writers, not namespace replacements.
        self.binding.verify_storage()?;
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
            if let Some(change) = &b.platform {
                self.commit_platform(change, to)?;
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
        self.binding.verify_storage()?;
        self.connection
            .execute_batch("COMMIT")
            .map_err(|e| format!("catalogue commit failed; recovery required: {e}"))?;
        self.binding.verify_storage()
    }
}

impl CatalogueMove {
    fn commit_platform(
        &self,
        change: &PlatformChange,
        at: &Association,
    ) -> std::result::Result<(), String> {
        let c = &self.connection;
        let archive = self.binding.archive;
        let e = |e: rusqlite::Error| e.to_string();
        let platform: Option<String> = if self.reverse {
            let removed = c
                .execute(
                    "DELETE FROM platform_assignments WHERE archive_id=?1 AND is_current=1 \
                     AND platform=?2 AND source=?3",
                    params![archive, change.platform, change.source],
                )
                .map_err(e)?;
            if removed != 1 {
                return Err("platform assignment compare-and-swap failed".into());
            }
            match change.previous {
                Some(id) => {
                    c.execute(
                        "UPDATE platform_assignments SET is_current=1 WHERE id=?1 AND archive_id=?2",
                        params![id, archive],
                    )
                    .map_err(e)?;
                    c.query_row(
                        "SELECT platform FROM platform_assignments WHERE id=?1",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(e)?
                }
                None => None,
            }
        } else {
            if let Some(id) = change.previous {
                let n = c
                    .execute(
                        "UPDATE platform_assignments SET is_current=0 WHERE id=?1 AND is_current=1",
                        [id],
                    )
                    .map_err(e)?;
                if n != 1 {
                    return Err("platform assignment compare-and-swap failed".into());
                }
            }
            c.execute(
                "INSERT INTO platform_assignments(archive_id,platform,source,confidence,is_current,assigned_at) \
                 VALUES (?1,?2,?3,NULL,1,?4)",
                params![archive, change.platform, change.source, now_utc_string()],
            )
            .map_err(e)?;
            Some(change.platform.clone())
        };
        if self.binding.fingerprint {
            c.execute(
                "UPDATE scan_fingerprints SET platform=?1 WHERE source_folder_id=?2 AND relative_path=?3",
                params![platform, at.folder, at.relative],
            )
            .map_err(e)?;
        }
        Ok(())
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
        if (entry.unknown.contains_key(COMPANION_KEY)
            || entry.unknown.contains_key(UNCATALOGUED_KEY))
            && !entry.unknown.contains_key(KEY)
        {
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

/// A library row whose path is a game folder moved as one unit.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FolderRow {
    archive: i64,
    old: Association,
    new: Association,
    fingerprint: bool,
}

/// The library rows for a game folder move: rows *at* `source` move with it;
/// the count of present rows *inside* `source` is returned so the caller can
/// refuse a folder that holds other games.
pub(crate) fn folder_rows(
    database: &Database,
    source: &Path,
    destination: &Path,
) -> std::result::Result<(Vec<FolderRow>, usize), String> {
    let c = &database.connection;
    let old_absolute = source.as_os_str().as_bytes().to_vec();
    let mut prefix = old_absolute.clone();
    prefix.push(b'/');
    let inside: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM archives WHERE last_verified_missing_at IS NULL \
             AND length(absolute_path_cached) > ?2 AND substr(absolute_path_cached,1,?2)=?1",
            params![prefix, prefix.len() as i64],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut stmt = c
        .prepare(
            "SELECT id FROM archives WHERE absolute_path_cached=?1 \
             AND last_verified_missing_at IS NULL ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let ids: Vec<i64> = stmt
        .query_map([&old_absolute], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for archive in ids {
        let old = association(c, source)?;
        let new = association(c, destination)?;
        rows.push(FolderRow {
            archive,
            fingerprint: fingerprint(c, &old)?,
            old,
            new,
        });
    }
    Ok((rows, usize::try_from(inside).unwrap_or(usize::MAX)))
}

/// An open SQLite write transaction holding a folder move's path changes.
/// Dropped without [`FolderCatalogueMove::commit`] it rolls back.
pub(crate) struct FolderCatalogueMove {
    connection: Connection,
}

impl Drop for FolderCatalogueMove {
    fn drop(&mut self) {
        let _ = self.connection.execute_batch("ROLLBACK");
    }
}

impl FolderCatalogueMove {
    /// Stages the compare-and-swap path changes (forward, or back for Undo).
    pub(crate) fn begin(
        database_path: &Path,
        rows: &[FolderRow],
        reverse: bool,
    ) -> std::result::Result<Self, String> {
        let connection = open_connection_with(database_path, false).map_err(|e| e.to_string())?;
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let guard = Self { connection };
        for row in rows {
            let (from, to) = if reverse {
                (&row.new, &row.old)
            } else {
                (&row.old, &row.new)
            };
            let c = &guard.connection;
            let taken: bool = c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM archives WHERE absolute_path_cached=?1 OR (source_folder_id=?2 AND relative_path=?3))",
                    params![to.absolute, to.folder, to.relative],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if taken {
                return Err("another library row already uses the destination path".into());
            }
            let file = Path::new(OsStr::from_bytes(&to.absolute))
                .file_name()
                .ok_or("destination has no name")?;
            let changed = c
                .execute(
                    "UPDATE archives SET source_folder_id=?1,relative_path=?2,absolute_path_cached=?3,file_name_cached=?4 \
                     WHERE id=?5 AND source_folder_id=?6 AND relative_path=?7 AND absolute_path_cached=?8",
                    params![to.folder, to.relative, to.absolute, file.as_bytes(), row.archive, from.folder, from.relative, from.absolute],
                )
                .map_err(|e| e.to_string())?;
            if changed != 1 {
                return Err("the library record for this folder changed; nothing was moved".into());
            }
            if row.fingerprint {
                c.execute(
                    "UPDATE scan_fingerprints SET source_folder_id=?1,relative_path=?2 WHERE source_folder_id=?3 AND relative_path=?4",
                    params![to.folder, to.relative, from.folder, from.relative],
                )
                .map_err(|e| e.to_string())?;
            }
            for (table, column) in PROJECTIONS {
                c.execute(
                    &format!("UPDATE {table} SET {column}=?1 WHERE {column}=?2"),
                    params![to.absolute, from.absolute],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        Ok(guard)
    }

    pub(crate) fn commit(self) -> std::result::Result<(), String> {
        self.connection
            .execute_batch("COMMIT")
            .map_err(|e| format!("library update failed: {e}"))
    }
}

/// Whether the rows sit at their moved (`true`) or original (`false`) path,
/// or neither (`None`). Used by recovery after an interruption.
pub(crate) fn folder_rows_moved(
    database_path: &Path,
    rows: &[FolderRow],
) -> std::result::Result<Option<bool>, String> {
    let c = open_connection_with(database_path, false).map_err(|e| e.to_string())?;
    let mut moved = 0;
    let mut original = 0;
    for row in rows {
        let at: Vec<u8> = c
            .query_row(
                "SELECT absolute_path_cached FROM archives WHERE id=?1",
                [row.archive],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if at == row.new.absolute {
            moved += 1;
        } else if at == row.old.absolute {
            original += 1;
        }
    }
    Ok(if moved == rows.len() {
        Some(true)
    } else if original == rows.len() {
        Some(false)
    } else {
        None
    })
}
