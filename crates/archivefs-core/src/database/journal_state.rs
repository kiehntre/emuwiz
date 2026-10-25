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

/// Refuses when a rollback journal exists that SQLite might treat as hot or
/// that cannot be read. Accepts: absent, zero-length, or header-zeroed.
pub(super) fn require_inert_rollback_journal(database_path: &Path) -> Result<()> {
    let journal = sidecar(database_path, "-journal");
    let Some(len) = regular_len(&journal, "rollback journal")? else {
        return Ok(());
    };
    if len == 0 {
        return Ok(());
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
        return Ok(());
    }
    Err(refuse(format!(
        "A rollback journal ({}, {len} bytes) with a live header sits beside the library \
         database. It may be a hot journal from an interrupted write, and opening the database \
         would make SQLite roll it back and delete it. EmuWiz will not recover it silently: \
         preserve the database and the journal together, close other users cleanly, and follow \
         the copy-first recovery procedure. Nothing was changed.",
        journal.display()
    )))
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
