//! File-level evidence about SQLite's rollback journal, WAL and shared-memory
//! sidecars, gathered *before* any writable SQLite connection exists.
//!
//! Why this is needed: a writable open of a database with a hot rollback
//! journal makes SQLite roll the journal back and delete it, and an open with a
//! damaged WAL makes SQLite silently stop reading at the first bad frame. Both
//! discard recovery information. Nothing here opens the database through
//! SQLite and nothing here writes, so inspection cannot itself recover or
//! checkpoint anything.
//!
//! Policy is conservative: only states SQLite itself documents as inert are
//! accepted (no journal, an empty journal, or a journal whose header is zeroed
//! so SQLite does not treat it as hot). Everything else is refused with a
//! message that names the sidecar. SQLite's own lock-aware decision is the
//! second line of defence: a read-only connection reports
//! `SQLITE_READONLY_ROLLBACK` for a journal it would have to roll back, which
//! callers already convert to a refusal.

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::{ArchiveFsError, Result};

const WAL_MAGIC_LITTLE: u32 = 0x377f_0682;
const WAL_MAGIC_BIG: u32 = 0x377f_0683;
const WAL_VERSION: u32 = 3_007_000;
const WAL_HEADER_BYTES: usize = 32;
const WAL_FRAME_HEADER_BYTES: usize = 24;
/// A WAL larger than this is refused rather than streamed in full.
const MAX_WAL_VERIFY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn refuse(message: String) -> ArchiveFsError {
    ArchiveFsError::Database(message)
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn open_nofollow(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

/// `None` when the sidecar does not exist; an error for anything unsafe.
fn regular_len(path: &Path, what: &str) -> Result<Option<u64>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
        Ok(_) => Err(refuse(format!(
            "the {what} {} is not a regular file; refusing to open the library database. \
             Nothing was changed.",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(refuse(format!(
            "could not inspect the {what} {}: {error}. Nothing was changed.",
            path.display()
        ))),
    }
}

/// What the rollback journal beside a database currently looks like, without
/// opening the database through SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RollbackJournalState {
    /// No `-journal` sidecar.
    Absent,
    /// Zero-length or header-zeroed: SQLite ignores it.
    Inert,
    /// A non-zero header: SQLite would roll it back on the next open unless a
    /// writer still holds its RESERVED lock. `bytes` is the journal length.
    LiveHeader { bytes: u64 },
}

/// Inspects the rollback journal sidecar. Never opens the database.
pub(super) fn rollback_journal_state(database_path: &Path) -> Result<RollbackJournalState> {
    let journal = sidecar(database_path, "-journal");
    let Some(len) = regular_len(&journal, "rollback journal")? else {
        return Ok(RollbackJournalState::Absent);
    };
    if len == 0 {
        return Ok(RollbackJournalState::Inert);
    }
    let mut file = open_nofollow(&journal).map_err(|error| {
        refuse(format!(
            "the rollback journal {} could not be read: {error}. Its state is unknown, so the \
             library database was not opened. Nothing was changed.",
            journal.display()
        ))
    })?;
    let mut head = [0_u8; 8];
    let wanted = head.len().min(len as usize);
    file.read_exact(&mut head[..wanted]).map_err(|error| {
        refuse(format!(
            "the rollback journal {} could not be read: {error}. Nothing was changed.",
            journal.display()
        ))
    })?;
    if head[..wanted].iter().all(|byte| *byte == 0) {
        return Ok(RollbackJournalState::Inert);
    }
    Ok(RollbackJournalState::LiveHeader { bytes: len })
}

/// Byte offsets of SQLite's advisory lock bytes in the main database file
/// (os_unix.c: `PENDING_BYTE` is 0x40000000; RESERVED follows it; the SHARED
/// range starts after that). They are fixed by the file format, not tunable.
const SQLITE_PENDING_BYTE: u64 = 0x4000_0000;
const SQLITE_RESERVED_BYTE: u64 = SQLITE_PENDING_BYTE + 1;

/// One record lock from `/proc/locks`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordLock {
    write: bool,
    /// `None` for open-file-description locks, which `/proc/locks` reports
    /// without an owning process.
    pid: Option<u32>,
    start: u64,
    /// Inclusive; `None` means to end of file.
    end: Option<u64>,
}

impl RecordLock {
    fn covers(&self, byte: u64) -> bool {
        self.start <= byte && self.end.is_none_or(|end| byte <= end)
    }
}

/// Every record lock currently held on `path`'s inode, read from
/// `/proc/locks`.
///
/// Why not `fcntl(F_GETLK)`: that needs a descriptor on the file, and on
/// POSIX closing *any* descriptor a process has on a file drops every record
/// lock that process holds on it. A probe that opened and closed the live
/// database would therefore silently release the locks of a SQLite
/// connection in this very process (its own `BEGIN IMMEDIATE`, or a running
/// scan's transaction). `/proc/locks` answers without touching the file.
/// The kernel lock table. Tests may point it elsewhere to model a system
/// where it cannot be read.
fn proc_locks_table() -> Result<String> {
    #[cfg(test)]
    let path = test_seam::current_proc_locks_path();
    #[cfg(not(test))]
    let path = PathBuf::from("/proc/locks");
    fs::read_to_string(&path).map_err(|error| {
        refuse(format!(
            "could not read {} to judge who holds the library database: {error}. \
             Nothing was changed.",
            path.display()
        ))
    })
}

#[cfg(test)]
pub(super) mod test_seam {
    use std::cell::RefCell;
    use std::path::PathBuf;

    thread_local! {
        static PROC_LOCKS: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    pub(crate) fn current_proc_locks_path() -> PathBuf {
        PROC_LOCKS.with(|slot| {
            slot.borrow()
                .clone()
                .unwrap_or_else(|| PathBuf::from("/proc/locks"))
        })
    }

    /// Reads lock state from `path` instead of `/proc/locks` on this thread
    /// until the guard drops.
    pub(crate) fn proc_locks_path(path: PathBuf) -> Guard {
        PROC_LOCKS.with(|slot| *slot.borrow_mut() = Some(path));
        Guard
    }

    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            PROC_LOCKS.with(|slot| *slot.borrow_mut() = None);
        }
    }
}

fn record_locks_on(path: &Path) -> Result<Vec<RecordLock>> {
    use std::os::unix::fs::MetadataExt;
    let inode = fs::symlink_metadata(path)
        .map_err(|error| {
            refuse(format!(
                "could not inspect {} to read its lock state: {error}. Nothing was changed.",
                path.display()
            ))
        })?
        .ino();
    Ok(parse_proc_locks(&proc_locks_table()?, inode))
}

/// The record locks on `inode` listed in a `/proc/locks` table.
fn parse_proc_locks(table: &str, inode: u64) -> Vec<RecordLock> {
    let mut locks = Vec::new();
    for line in table.lines() {
        // "N: POSIX  ADVISORY  WRITE 1234 08:01:5678 1073741825 1073741825"
        // Waiters are listed as "N: -> ..." and are not holders.
        let mut fields = line.split_whitespace();
        let _index = fields.next();
        let Some(kind) = fields.next() else {
            continue;
        };
        if kind == "->" {
            continue;
        }
        let (Some(_class), Some(access), Some(pid), Some(file), Some(start), Some(end)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let Some(entry_inode) = file.rsplit(':').next().and_then(|v| v.parse::<u64>().ok()) else {
            continue;
        };
        if entry_inode != inode || !(kind == "POSIX" || kind == "OFDLCK") {
            continue;
        }
        let Ok(start) = start.parse::<u64>() else {
            continue;
        };
        let end = if end == "EOF" {
            None
        } else {
            match end.parse::<u64>() {
                Ok(end) => Some(end),
                Err(_) => continue,
            }
        };
        locks.push(RecordLock {
            write: access == "WRITE",
            pid: pid.parse::<u32>().ok(),
            start,
            end,
        });
    }
    locks
}

/// Whether any SQLite connection, in this process or another, currently holds
/// a write lock (RESERVED or PENDING) on the database file. Never takes a
/// lock, never opens the file.
pub(super) fn sqlite_write_lock_held(database_path: &Path) -> Result<bool> {
    Ok(record_locks_on(database_path)?.iter().any(|lock| {
        lock.write && (lock.covers(SQLITE_RESERVED_BYTE) || lock.covers(SQLITE_PENDING_BYTE))
    }))
}

/// Whether *this process* holds any record lock on the database file. While
/// it does, no code in this process may open and close the file with plain
/// I/O (hashing, fsync): the close would drop those locks underneath the
/// SQLite connection that believes it still holds them.
pub(super) fn own_process_holds_lock(database_path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let inode = fs::symlink_metadata(database_path)
        .map_err(|error| {
            refuse(format!(
                "could not inspect {} to read its lock state: {error}. Nothing was changed.",
                database_path.display()
            ))
        })?
        .ino();
    own_process_holds_lock_on_inode(inode)
}

/// Like [`own_process_holds_lock`] for an inode that may no longer have a
/// pathname (a superseded database still open somewhere in this process).
pub(super) fn own_process_holds_lock_on_inode(inode: u64) -> Result<bool> {
    let pid = std::process::id();
    Ok(parse_proc_locks(&proc_locks_table()?, inode)
        .iter()
        .any(|lock| lock.pid == Some(pid)))
}

/// Why a rollback journal with a live header stopped an operation. Every
/// variant refuses; the distinction only changes what the user is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LiveJournalVerdict {
    /// A SQLite write lock is held on the database right now: a writer in
    /// some process is mid-transaction. Wait and retry.
    WriterActive,
    /// No write lock is visible. Either a writer crashed and left a hot
    /// journal, or a writer EmuWiz cannot see (another locking VFS, a network
    /// filesystem, an unreadable lock table) is still active. Not decided.
    ActivityUndetermined,
}

/// Classifies a live-header journal. The kernel's lock table is the writer
/// lifecycle signal: SQLite holds its RESERVED lock from the first write of a
/// transaction until commit or rollback, so "held" is proof of a live writer.
/// "Not held" proves nothing on its own (a commit between the journal read
/// and the lock read deletes the journal, and some configurations have no
/// visible locks), so the journal is re-read and the answer stays neutral.
pub(super) fn classify_live_journal(database_path: &Path) -> Result<Option<LiveJournalVerdict>> {
    let RollbackJournalState::LiveHeader { .. } = rollback_journal_state(database_path)? else {
        return Ok(None);
    };
    match sqlite_write_lock_held(database_path) {
        Ok(true) => Ok(Some(LiveJournalVerdict::WriterActive)),
        Ok(false) => match rollback_journal_state(database_path)? {
            RollbackJournalState::LiveHeader { .. } => {
                Ok(Some(LiveJournalVerdict::ActivityUndetermined))
            }
            // The writer committed in between; nothing is left to judge.
            RollbackJournalState::Absent | RollbackJournalState::Inert => Ok(None),
        },
        Err(_) => Ok(Some(LiveJournalVerdict::ActivityUndetermined)),
    }
}

/// Refuses when a rollback journal exists that SQLite might treat as hot or
/// that cannot be read. Accepts: absent, zero-length, or header-zeroed.
///
/// A journal with a live header always refuses. The message says "a writer
/// is active" only when the kernel shows SQLite's write lock held; otherwise
/// it says database activity cannot be determined. It never claims a crash,
/// never opens the database and never touches the journal.
pub(super) fn require_inert_rollback_journal(database_path: &Path) -> Result<()> {
    let journal = sidecar(database_path, "-journal");
    match classify_live_journal(database_path)? {
        None => Ok(()),
        Some(LiveJournalVerdict::WriterActive) => Err(refuse(format!(
            "Another EmuWiz worker or process is writing the library database right now (its \
             rollback journal {} is open and its write lock is held). Wait for that scan or \
             action to finish and try again. Nothing was changed.",
            journal.display()
        ))),
        Some(LiveJournalVerdict::ActivityUndetermined) => Err(refuse(format!(
            "Database activity cannot be determined; the library database was not opened. A \
             rollback journal ({}) with a live header sits beside it and no SQLite write lock \
             is visible: a writer may have been interrupted, or a writer EmuWiz cannot see may \
             still be running. Opening the database could make SQLite roll that journal back \
             and delete it, so EmuWiz will not. Close every other EmuWiz window and process, \
             preserve the database and the journal together, and follow the copy-first recovery \
             procedure in docs/DATABASE_RECOVERY.md. Nothing was changed.",
            journal.display()
        ))),
    }
}

struct WalChecksum {
    s0: u32,
    s1: u32,
}

impl WalChecksum {
    fn update(&mut self, bytes: &[u8], big_endian: bool) {
        for pair in bytes.chunks_exact(8) {
            let word = |chunk: &[u8]| {
                let array = [chunk[0], chunk[1], chunk[2], chunk[3]];
                if big_endian {
                    u32::from_be_bytes(array)
                } else {
                    u32::from_le_bytes(array)
                }
            };
            self.s0 = self.s0.wrapping_add(word(&pair[..4])).wrapping_add(self.s1);
            self.s1 = self.s1.wrapping_add(word(&pair[4..])).wrapping_add(self.s0);
        }
    }
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Verifies the write-ahead log, if any, against the main database header and
/// SQLite's documented frame checksum chain. A stale tail left by an earlier
/// WAL generation (salt mismatch) is legitimate; a torn or checksum-failing
/// frame of the current generation is not.
pub(super) fn require_consistent_wal(database_path: &Path) -> Result<()> {
    let wal = sidecar(database_path, "-wal");
    let shm = sidecar(database_path, "-shm");
    let shm_len = regular_len(&shm, "shared-memory file")?;
    let Some(wal_len) = regular_len(&wal, "write-ahead log")? else {
        if shm_len.is_some() {
            return Err(refuse(format!(
                "A shared-memory file {} exists but its write-ahead log is missing, so committed \
                 data may have been lost or another process is using the database. Refusing to \
                 continue. Nothing was changed.",
                shm.display()
            )));
        }
        return Ok(());
    };
    if wal_len == 0 {
        return Ok(());
    }
    if wal_len > MAX_WAL_VERIFY_BYTES {
        return Err(refuse(format!(
            "The write-ahead log {} is {wal_len} bytes, too large to verify before migration. \
             Checkpoint the database with its normal application first. Nothing was changed.",
            wal.display()
        )));
    }
    if wal_len < WAL_HEADER_BYTES as u64 {
        return Err(refuse(format!(
            "The write-ahead log {} is truncated ({wal_len} bytes) and cannot be reconciled. \
             Nothing was changed.",
            wal.display()
        )));
    }
    let wal_error = |what: &str| {
        refuse(format!(
            "The write-ahead log {} {what}; it cannot be reconciled safely, so the library \
             database was not touched. Preserve the database and its -wal file together. \
             Nothing was changed.",
            wal.display()
        ))
    };
    let mut file = open_nofollow(&wal).map_err(|error| {
        refuse(format!(
            "could not read the write-ahead log {}: {error}. Nothing was changed.",
            wal.display()
        ))
    })?;
    let mut header = [0_u8; WAL_HEADER_BYTES];
    file.read_exact(&mut header)
        .map_err(|error| wal_error(&format!("could not be read ({error})")))?;
    let magic = be32(&header[0..4]);
    let big_endian = match magic {
        WAL_MAGIC_LITTLE => false,
        WAL_MAGIC_BIG => true,
        _ => return Err(wal_error("does not start with SQLite's WAL magic number")),
    };
    if be32(&header[4..8]) != WAL_VERSION {
        return Err(wal_error("has an unsupported format version"));
    }
    let page_size = be32(&header[8..12]);
    if !(512..=65_536).contains(&page_size) || !page_size.is_power_of_two() {
        return Err(wal_error("declares an invalid page size"));
    }
    let mut running = WalChecksum { s0: 0, s1: 0 };
    running.update(&header[..24], big_endian);
    if (running.s0, running.s1) != (be32(&header[24..28]), be32(&header[28..32])) {
        return Err(wal_error("has a damaged header (checksum mismatch)"));
    }
    let salt = (be32(&header[16..20]), be32(&header[20..24]));

    // The main file must be a WAL-mode database with the same page size.
    let mut main = open_nofollow(database_path).map_err(|error| {
        refuse(format!(
            "could not read the library database header: {error}. Nothing was changed."
        ))
    })?;
    let mut db_header = [0_u8; 100];
    main.read_exact(&mut db_header)
        .map_err(|_| wal_error("exists beside a database whose header is missing or truncated"))?;
    if db_header[18] != 2 || db_header[19] != 2 {
        return Err(wal_error(
            "exists although the database file is not in WAL mode (the log would be ignored \
             and its committed content lost)",
        ));
    }
    let db_page = match u16::from_be_bytes([db_header[16], db_header[17]]) {
        1 => 65_536,
        other => u32::from(other),
    };
    if db_page != page_size {
        return Err(wal_error("has a page size that differs from the database"));
    }

    let frame_bytes = WAL_FRAME_HEADER_BYTES + page_size as usize;
    let mut frame = vec![0_u8; frame_bytes];
    let mut remaining = wal_len - WAL_HEADER_BYTES as u64;
    while remaining > 0 {
        if remaining < frame_bytes as u64 {
            return Err(wal_error("ends in a partial frame (torn write)"));
        }
        file.read_exact(&mut frame)
            .map_err(|error| wal_error(&format!("could not be read ({error})")))?;
        remaining -= frame_bytes as u64;
        let frame_salt = (be32(&frame[8..12]), be32(&frame[12..16]));
        if frame_salt != salt {
            // First frame of an older WAL generation: SQLite stops here too.
            return Ok(());
        }
        running.update(&frame[..8], big_endian);
        running.update(&frame[WAL_FRAME_HEADER_BYTES..], big_endian);
        if be32(&frame[0..4]) == 0
            || (running.s0, running.s1) != (be32(&frame[16..20]), be32(&frame[20..24]))
        {
            return Err(wal_error(
                "contains a frame of the current generation that fails its checksum (damaged)",
            ));
        }
    }
    Ok(())
}

/// Everything migration requires of the sidecars: an inert rollback journal and
/// a WAL that verifies against the main file.
pub(super) fn require_reconcilable_for_migration(database_path: &Path) -> Result<()> {
    require_inert_rollback_journal(database_path)?;
    require_consistent_wal(database_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database(path: &Path) -> rusqlite::Connection {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch("CREATE TABLE t(v INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();
        connection
    }

    /// /proc/locks is a seq_file: a read can miss an entry while other tests in
    /// this process add and drop their own locks, so a single read may
    /// transiently say "not held". Re-reads `check` for a bounded time; a
    /// condition that is truly false still fails the caller's assertion.
    fn eventually(mut check: impl FnMut() -> bool) -> bool {
        for _ in 0..50 {
            if check() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn write_lock_probe_sees_a_reserved_lock_held_in_this_process() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.sqlite3");
        let writer = database(&path);
        assert!(!sqlite_write_lock_held(&path).unwrap());
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(sqlite_write_lock_held(&path).unwrap());
        writer.execute_batch("ROLLBACK").unwrap();
        assert!(!sqlite_write_lock_held(&path).unwrap());
    }

    #[test]
    fn lock_probe_never_touches_the_file_so_it_cannot_drop_our_own_locks() {
        // POSIX drops a process's record locks on a file when it closes any
        // descriptor on it. The probe must therefore be side-effect free:
        // after probing, a second in-process writer must still be refused.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.sqlite3");
        let writer = database(&path);
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(own_process_holds_lock(&path).unwrap());
        for _ in 0..3 {
            assert!(sqlite_write_lock_held(&path).unwrap());
            assert!(own_process_holds_lock(&path).unwrap());
        }
        let second = rusqlite::Connection::open(&path).unwrap();
        second
            .busy_timeout(std::time::Duration::from_millis(100))
            .unwrap();
        let error = second.execute_batch("BEGIN IMMEDIATE").unwrap_err();
        assert!(error.to_string().contains("locked"), "{error}");
        // And the documented hazard really exists: a plain open+close of the
        // file by this process releases the writer's lock.
        drop(fs::File::open(&path).unwrap());
        assert!(!sqlite_write_lock_held(&path).unwrap());
        assert!(!own_process_holds_lock(&path).unwrap());
        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn live_journal_with_running_writer_is_refused_as_busy_not_hot() {
        // D1-01: a second writer meeting a spilled transaction must be told
        // the database is busy, not that it has a hot journal to recover.
        // SQLite leaves the journal header zeroed (inert) until the page
        // cache spills, so the writer is made to spill with a tiny cache.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.sqlite3");
        let writer = database(&path);
        writer
            .execute_batch("PRAGMA cache_size = 10; BEGIN IMMEDIATE;")
            .unwrap();
        let payload = "x".repeat(900);
        let mut spilled = false;
        for _ in 0..4000 {
            writer
                .execute("INSERT INTO t VALUES (?1)", [&payload])
                .unwrap();
            if matches!(
                rollback_journal_state(&path).unwrap(),
                RollbackJournalState::LiveHeader { .. }
            ) {
                spilled = true;
                break;
            }
        }
        assert!(
            spilled,
            "the writer's transaction never spilled to its journal"
        );
        // The writer took its lock synchronously (BEGIN IMMEDIATE above), so
        // it is held; only the /proc/locks read can be transiently wrong.
        assert!(
            eventually(|| sqlite_write_lock_held(&path).unwrap()),
            "the running writer's lock is not visible in /proc/locks"
        );
        assert!(
            eventually(
                || classify_live_journal(&path).unwrap() == Some(LiveJournalVerdict::WriterActive)
            ),
            "a live journal with a running writer was not classified WriterActive"
        );
        let mut error = String::new();
        assert!(
            eventually(|| {
                error = require_inert_rollback_journal(&path)
                    .unwrap_err()
                    .to_string();
                error.contains("writing the library database right now")
            }),
            "{error}"
        );
        assert!(!error.contains("cannot be determined"), "{error}");
        writer.execute_batch("COMMIT").unwrap();
        assert_eq!(classify_live_journal(&path).unwrap(), None);
        assert_eq!(
            rollback_journal_state(&path).unwrap(),
            RollbackJournalState::Absent
        );
        require_inert_rollback_journal(&path).unwrap();
    }

    #[test]
    fn proc_locks_parser_reads_holders_only_and_both_lock_families() {
        let table = "\
1: POSIX  ADVISORY  WRITE 4242 08:01:777 1073741825 1073741825
1: -> POSIX  ADVISORY  WRITE 4243 08:01:777 1073741825 1073741825
2: OFDLCK ADVISORY  READ  -1 08:01:777 1073741826 1073742335
3: FLOCK  ADVISORY  WRITE 4244 08:01:777 0 EOF
4: POSIX  ADVISORY  WRITE 4245 08:01:778 0 EOF
5: LEASE  ACTIVE    WRITE 4246 08:01:777 0 EOF
garbage line
";
        let locks = parse_proc_locks(table, 777);
        assert_eq!(
            locks,
            vec![
                RecordLock {
                    write: true,
                    pid: Some(4242),
                    start: 1073741825,
                    end: Some(1073741825)
                },
                RecordLock {
                    write: false,
                    pid: None,
                    start: 1073741826,
                    end: Some(1073742335)
                },
            ]
        );
        assert!(locks[0].covers(SQLITE_RESERVED_BYTE));
        assert!(!locks[0].covers(SQLITE_PENDING_BYTE));
        assert!(locks[1].covers(SQLITE_PENDING_BYTE + 2));
        assert!(parse_proc_locks("", 777).is_empty());
        assert_eq!(parse_proc_locks(table, 778).len(), 1);
        assert!(parse_proc_locks(table, 778)[0].covers(u64::MAX));
    }

    #[test]
    fn live_journal_without_a_visible_writer_is_undetermined_and_preserved() {
        // A crashed writer and an invisible writer look the same from here:
        // refuse with a neutral message, open nothing, touch nothing.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.sqlite3");
        drop(database(&path));
        let journal = sidecar(&path, "-journal");
        let header = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7, 0, 0, 0, 1];
        fs::write(&journal, header).unwrap();
        let database_before = fs::read(&path).unwrap();
        assert_eq!(
            classify_live_journal(&path).unwrap(),
            Some(LiveJournalVerdict::ActivityUndetermined)
        );
        let error = require_inert_rollback_journal(&path)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Database activity cannot be determined"),
            "{error}"
        );
        assert!(!error.to_lowercase().contains("crash"), "{error}");
        assert!(!error.contains("hot journal"), "{error}");
        assert!(error.contains("Nothing was changed"), "{error}");
        assert_eq!(
            fs::read(&journal).unwrap(),
            header,
            "journal must be preserved byte for byte"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            database_before,
            "database must not be opened or rolled back"
        );
        assert_eq!(fs::metadata(&journal).unwrap().len(), 12);
        // Zero-length and header-zeroed journals are inert.
        fs::write(&journal, [0_u8; 12]).unwrap();
        assert_eq!(
            rollback_journal_state(&path).unwrap(),
            RollbackJournalState::Inert
        );
        require_inert_rollback_journal(&path).unwrap();
        fs::write(&journal, b"").unwrap();
        assert_eq!(
            rollback_journal_state(&path).unwrap(),
            RollbackJournalState::Inert
        );
    }
}
