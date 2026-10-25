//! Minimal participating-process fixture admission. One independently configured
//! authority, object identities AND both name slots; claims survive process death.
//! The stable flock serializes participants, never an unaware writer.
use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use std::os::unix::ffi::OsStrExt;

#[derive(Clone, Serialize, Deserialize)]
pub struct Configuration {
    directory: PathBuf,
    directory_id: (u64, u64),
    lock_id: (u64, u64),
    database_id: (u64, u64),
}
pub(super) struct Guard {
    config: Configuration,
    directory: File,
    lock: File,
    db: Connection,
    process: u32,
    owner: Option<(String, [u8; 32])>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        // Match Store: a concurrent fork may retain this open-file description
        // until exec. Only the owning process may end its operation lock; durable
        // unresolved claims are deliberately untouched by this destructor.
        if self.process == std::process::id() {
            unsafe {
                libc::flock(self.lock.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
impl Configuration {
    pub fn create(path: &Path) -> io::Result<Self> {
        let directory = open_dir(path)?;
        private(&directory)?;
        let lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(at(&directory, "lock"))?;
        lock.sync_all()?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(at(&directory, "owners.sqlite"))?;
        file.sync_all()?;
        let db = Connection::open(at(&directory, "owners.sqlite")).map_err(fail)?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE TABLE owners(id TEXT PRIMARY KEY, binding BLOB NOT NULL, released INTEGER NOT NULL); CREATE TABLE claims(key TEXT PRIMARY KEY, owner TEXT NOT NULL);").map_err(fail)?;
        directory.sync_all()?;
        Ok(Self {
            directory: path.into(),
            directory_id: identity(&directory)?,
            lock_id: identity(&lock)?,
            database_id: identity(&file)?,
        })
    }
    pub(super) fn acquire(&self) -> io::Result<Guard> {
        let directory = open_dir(&self.directory)?;
        private(&directory)?;
        if identity(&directory)? != self.directory_id {
            return Err(fail("admission directory rebound"));
        }
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(at(&directory, "lock"))?;
        if identity(&lock_file)? != self.lock_id || lock_file.metadata()?.nlink() != 1 {
            return Err(fail("admission lock rebound"));
        }
        lock(&lock_file).map_err(|_| fail("participating operation busy"))?;
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(at(&directory, "owners.sqlite"))?;
        if identity(&f)? != self.database_id
            || !f.metadata()?.is_file()
            || f.metadata()?.nlink() != 1
        {
            return Err(fail("admission database rebound"));
        }
        let db = Connection::open_with_flags(
            at(&directory, "owners.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(fail)?;
        db.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")
            .map_err(fail)?;
        let check: String = db
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(fail)?;
        if check != "ok" {
            return Err(fail("corrupt admission database"));
        }
        let guard = Guard {
            config: self.clone(),
            directory,
            lock: lock_file,
            db,
            process: std::process::id(),
            owner: None,
        };
        guard.bound()?;
        Ok(guard)
    }
}
/// What the trusted admission store says, read under the participants'
/// serialization lock but without opening the database for writing. It never
/// admits, resumes, releases or repairs anything.
pub(super) struct Inspection {
    /// Transactions admitted and not released (normally zero or one per root).
    pub(super) unreleased: Vec<String>,
    /// (binding, released) of the inspected transaction, if a row exists.
    pub(super) owner: Option<(Vec<u8>, bool)>,
    /// Every object and name-slot claim of the inspected transaction is held by it.
    pub(super) claims_complete: bool,
    pub(super) owner_claims: i64,
    pub(super) successors: Vec<Successor>,
    pub(super) released_owners: Vec<(String, Vec<u8>)>,
}
/// Holds the participants' lock for the duration of a read-only inspection,
/// so no participating operation can change evidence underneath it.
pub(super) struct InspectionLock {
    lock: File,
    process: u32,
}
impl Drop for InspectionLock {
    fn drop(&mut self) {
        if self.process == std::process::id() {
            unsafe {
                libc::flock(self.lock.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
impl Configuration {
    /// Read-only restart inspection. Refuses (WouldBlock) while a live
    /// participant holds the operation lock rather than reading a moving state.
    pub(super) fn inspect(
        &self,
        base: Option<&Authority>,
    ) -> io::Result<(Inspection, InspectionLock)> {
        let directory = open_dir(&self.directory)?;
        private(&directory)?;
        if identity(&directory)? != self.directory_id {
            return Err(fail("admission directory rebound"));
        }
        // O_NONBLOCK: a substituted FIFO must not hang a read-only inspection.
        let lock_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(at(&directory, "lock"))?;
        if identity(&lock_file)? != self.lock_id
            || !lock_file.metadata()?.is_file()
            || lock_file.metadata()?.nlink() != 1
        {
            return Err(fail("admission lock rebound"));
        }
        if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "participating operation busy",
            ));
        }
        let hold = InspectionLock {
            lock: lock_file,
            process: std::process::id(),
        };
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(at(&directory, "owners.sqlite"))?;
        if identity(&f)? != self.database_id
            || !f.metadata()?.is_file()
            || f.metadata()?.nlink() != 1
        {
            return Err(fail("admission database rebound"));
        }
        // Read-only: an interrupted participant write (hot journal) cannot be
        // rolled back here; say so instead of surfacing SQLite's wording.
        let unreadable = |e: rusqlite::Error| {
            fail(format!(
                "admission database cannot be read without writing (an interrupted participant write may be pending; only the participating path may recover it): {e}"
            ))
        };
        let db = Connection::open_with_flags(
            at(&directory, "owners.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(unreadable)?;
        let check: String = db
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(unreadable)?;
        if check != "ok" {
            return Err(fail("corrupt admission database"));
        }
        let unreleased = db
            .prepare("SELECT id FROM owners WHERE released=0 ORDER BY id")
            .and_then(|mut s| {
                s.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(fail)?;
        let (owner, claims_complete) = match base {
            None => (None, false),
            Some(base) => {
                let owner: Option<(Vec<u8>, bool)> = db
                    .query_row(
                        "SELECT binding,released FROM owners WHERE id=?1",
                        [base.transaction_id()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(fail)?;
                let mut complete = true;
                for key in keys(base)? {
                    let holder: Option<String> = db
                        .query_row("SELECT owner FROM claims WHERE key=?1", [key], |r| r.get(0))
                        .optional()
                        .map_err(fail)?;
                    complete &= holder.as_deref() == Some(base.transaction_id());
                }
                (owner, complete)
            }
        };
        if identity(&open_dir(&self.directory)?)? != self.directory_id {
            return Err(fail("admission directory rebound"));
        }
        Ok((
            Inspection {
                unreleased,
                owner,
                claims_complete,
                owner_claims: match base {
                    Some(b) => db
                        .query_row(
                            "SELECT count(*) FROM claims WHERE owner=?1",
                            [b.transaction_id()],
                            |r| r.get(0),
                        )
                        .map_err(fail)?,
                    None => 0,
                },
                successors: successors(&db)?,
                released_owners: db.prepare("SELECT id,binding FROM owners WHERE released=1 AND NOT EXISTS(SELECT 1 FROM claims WHERE owner=owners.id)").map_err(fail)?.query_map([],|r|Ok((r.get(0)?,r.get(1)?))).map_err(fail)?.collect::<Result<Vec<_>,_>>().map_err(fail)?,
            },
            hold,
        ))
    }
}
fn keys(base: &Authority) -> io::Result<Vec<String>> {
    let mut keys = std::collections::BTreeSet::new();
    for entry in &base.transaction.entries {
        keys.insert(format!(
            "object:{}:{}",
            entry.identity.dev, entry.identity.ino
        ));
        for path in [&entry.source_path, &entry.destination_path] {
            let parent = open_dir(path.parent().ok_or_else(|| fail("missing parent"))?)?;
            let (dev, ino) = identity(&parent)?;
            let name = path
                .file_name()
                .ok_or_else(|| fail("missing name"))?
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            keys.insert(format!("slot:{dev}:{ino}:{name}"));
        }
    }
    Ok(keys.into_iter().collect())
}
impl Guard {
    fn bound(&self) -> io::Result<()> {
        if self.process != std::process::id()
            || identity(&open_dir(&self.config.directory)?)? != self.config.directory_id
            || identity(&self.directory)? != self.config.directory_id
            || identity(&self.lock)? != self.config.lock_id
            || identity(&File::open(at(&self.directory, "lock"))?)? != self.config.lock_id
            || identity(&File::open(at(&self.directory, "owners.sqlite"))?)?
                != self.config.database_id
        {
            return Err(fail("admission authority changed"));
        }
        Ok(())
    }
    pub(super) fn admit(&mut self, base: &Authority) -> io::Result<()> {
        self.bound()?;
        let id = base.transaction_id();
        let binding = base.binding();
        let keys = keys(base)?;
        let tx = self.db.transaction().map_err(fail)?;
        if tx
            .query_row("SELECT id FROM owners WHERE id=?1", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(fail)?
            .is_some()
        {
            return Err(fail("transaction id already admitted"));
        }
        for key in &keys {
            if tx
                .query_row("SELECT owner FROM claims WHERE key=?1", [key], |r| {
                    r.get::<_, String>(0)
                })
                .optional()
                .map_err(fail)?
                .is_some()
            {
                return Err(fail("object or name slot owned by unresolved transaction"));
            }
        }
        // Admission revalidation is inside the same lock held through execution.
        for e in &base.transaction.entries {
            if crate::dat::rename_apply::identity::capture_identity(&e.source_path)? != e.identity {
                return Err(fail("admission source identity changed"));
            }
            match std::fs::symlink_metadata(&e.destination_path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                _ => return Err(fail("admission destination is occupied or unreadable")),
            }
        }
        tx.execute(
            "INSERT INTO owners VALUES(?1,?2,0)",
            params![id, binding.as_slice()],
        )
        .map_err(fail)?;
        for key in keys {
            tx.execute("INSERT INTO claims VALUES(?1,?2)", params![key, id])
                .map_err(fail)?;
        }
        tx.commit().map_err(fail)?;
        self.directory.sync_all()?;
        self.owner = Some((id.into(), binding));
        self.verify(base)
    }
    pub(super) fn resume(&mut self, base: &Authority) -> io::Result<()> {
        self.owner = Some((base.transaction_id().into(), base.binding()));
        self.verify(base)
    }
    pub(super) fn verify(&self, base: &Authority) -> io::Result<()> {
        self.bound()?;
        if self.owner.as_ref() != Some(&(base.transaction_id().into(), base.binding())) {
            return Err(fail("foreign operation owner"));
        }
        let row: Option<(Vec<u8>, bool)> = self
            .db
            .query_row(
                "SELECT binding,released FROM owners WHERE id=?1",
                [base.transaction_id()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(fail)?;
        if row != Some((base.binding().to_vec(), false)) {
            return Err(fail("missing, foreign or released ownership"));
        }
        for key in keys(base)? {
            let owner: Option<String> = self
                .db
                .query_row("SELECT owner FROM claims WHERE key=?1", [key], |r| r.get(0))
                .optional()
                .map_err(fail)?;
            if owner.as_deref() != Some(base.transaction_id()) {
                return Err(fail("missing ownership claim"));
            }
        }
        Ok(())
    }
    // Caller has verified the complete compact seal and exact restored objects.
    pub(super) fn release(&mut self, base: &Authority) -> io::Result<()> {
        self.verify(base)?;
        let tx = self.db.transaction().map_err(fail)?;
        tx.execute("DELETE FROM claims WHERE owner=?1", [base.transaction_id()])
            .map_err(fail)?;
        tx.execute(
            "UPDATE owners SET released=1 WHERE id=?1",
            [base.transaction_id()],
        )
        .map_err(fail)?;
        tx.commit().map_err(fail)?;
        self.directory.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn configuration() -> Configuration {
        let directory = tempfile::Builder::new()
            .prefix("emuwiz-r2-admission-drop-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
            .keep();
        Configuration::create(&directory).unwrap()
    }

    #[test]
    fn owner_drop_unlocks_shared_descriptor_without_releasing_claims() {
        let config = configuration();
        let guard = config.acquire().unwrap();
        // Marker rows make accidental terminal release on Drop observable.
        guard
            .db
            .execute(
                "INSERT INTO owners VALUES('unresolved',?1,0)",
                [[0u8; 32].as_slice()],
            )
            .unwrap();
        guard
            .db
            .execute(
                "INSERT INTO claims VALUES('object:fixture','unresolved')",
                [],
            )
            .unwrap();
        let retained = guard.lock.try_clone().unwrap();
        // dup and fork share an open-file description; CLOEXEC alone does not
        // release its flock until the child reaches exec or closes the descriptor.
        assert!(
            config
                .acquire()
                .err()
                .unwrap()
                .to_string()
                .contains("participating operation busy")
        );
        drop(guard);
        let next = config
            .acquire()
            .expect("owner drop must unlock even with a duplicate retained");
        assert_eq!(
            next.db
                .query_row("SELECT count(*) FROM claims", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            next.db
                .query_row(
                    "SELECT released FROM owners WHERE id='unresolved'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(retained);
    }

    #[test]
    fn synthetic_foreign_process_drop_does_not_unlock_owner_description() {
        let config = configuration();
        let mut guard = config.acquire().unwrap();
        let retained = guard.lock.try_clone().unwrap();
        // Exercise the foreign-PID branch without running SQLite's destructor
        // after fork in a multithreaded test process. This is not a custody test.
        guard.process = guard.process.wrapping_add(1);
        drop(guard);
        assert!(
            config
                .acquire()
                .err()
                .unwrap()
                .to_string()
                .contains("participating operation busy")
        );
        assert_eq!(
            unsafe { libc::flock(retained.as_raw_fd(), libc::LOCK_UN) },
            0
        );
        drop(retained);
        assert!(config.acquire().is_ok());
    }
}

/// Append-only deployment successor binding; existing ownership/claims remain
/// authoritative. No transaction state is copied into this lifecycle table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Successor {
    pub predecessor: String,
    pub binding: [u8; 32],
    pub state: PathBuf,
    pub state_id: (u64, u64),
    pub configuration: [u8; 32],
    pub settlement: [u8; 32],
    pub reviewed_confirmation: String,
}
fn successors(db: &Connection) -> io::Result<Vec<Successor>> {
    let exists: bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='r2_successors')",[],|r|r.get(0)).map_err(fail)?;
    if !exists {
        return Ok(vec![]);
    }
    let mut stmt = db
        .prepare("SELECT predecessor,record FROM r2_successors ORDER BY predecessor")
        .map_err(fail)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })
        .map_err(fail)?;
    rows.map(|r| {
        let (id, bytes) = r.map_err(fail)?;
        let record: Successor = serde_json::from_slice(&bytes).map_err(fail)?;
        if record.predecessor != id {
            return Err(fail("successor record key differs"));
        }
        Ok(record)
    })
    .collect()
}
impl Guard {
    pub(super) fn successors(&self) -> io::Result<Vec<Successor>> {
        self.bound()?;
        successors(&self.db)
    }
    pub(super) fn require_idle(&self) -> io::Result<()> {
        self.bound()?;
        let n:i64=self.db.query_row("SELECT (SELECT count(*) FROM owners WHERE released=0)+(SELECT count(*) FROM claims)",[],|r|r.get(0)).map_err(fail)?;
        if n != 0 {
            return Err(fail("unresolved ownership or claims; root reuse refused"));
        }
        Ok(())
    }
    pub(super) fn require_released(&self, base: &Authority) -> io::Result<()> {
        self.bound()?;
        let row: Option<(Vec<u8>, bool)> = self
            .db
            .query_row(
                "SELECT binding,released FROM owners WHERE id=?1",
                [base.transaction_id()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(fail)?;
        let claims: i64 = self
            .db
            .query_row(
                "SELECT count(*) FROM claims WHERE owner=?1",
                [base.transaction_id()],
                |r| r.get(0),
            )
            .map_err(fail)?;
        if row != Some((base.binding().to_vec(), true)) || claims != 0 {
            return Err(fail("previous transaction not durably released"));
        }
        Ok(())
    }
    pub(super) fn publish_successor(
        &mut self,
        base: &Authority,
        record: &Successor,
    ) -> io::Result<()> {
        self.require_released(base)?;
        self.require_idle()?;
        if record.predecessor != base.transaction_id() || record.binding != base.binding() {
            return Err(fail("foreign successor predecessor"));
        }
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS r2_successors(predecessor TEXT PRIMARY KEY,record BLOB NOT NULL)").map_err(fail)?;
        if let Some(old) = self
            .successors()?
            .into_iter()
            .find(|r| r.predecessor == record.predecessor)
        {
            if old != *record {
                return Err(fail("predecessor already has a different successor"));
            }
            return Ok(());
        }
        let tx = self.db.transaction().map_err(fail)?;
        tx.execute(
            "INSERT INTO r2_successors VALUES(?1,?2)",
            params![
                record.predecessor,
                serde_json::to_vec(record).map_err(fail)?
            ],
        )
        .map_err(fail)?;
        edge("reuse-successor-row-staged");
        tx.commit().map_err(fail)?;
        edge("reuse-successor-row-committed");
        self.directory.sync_all()
    }
}
