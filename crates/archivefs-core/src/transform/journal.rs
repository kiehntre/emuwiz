//! Durable, versioned, hash-chained step journal for transformations.
//!
//! One transaction owns one directory `<journal_root>/<txn_id>/` holding:
//!
//! - `plan.json`: the immutable plan, written once with `create_new`, synced,
//!   and never rewritten. Its SHA-256 is the plan digest; it is the head of
//!   the record chain, so a replaced plan breaks every later record.
//! - `log.jsonl`: an append-only list of records. Every record carries its
//!   sequence number, the previous record's checksum and its own checksum,
//!   and is `fdatasync`ed before the caller is told it is durable. The caller
//!   writes an *intent* record before each mutation and a *done* record after
//!   it has been made durable, so the journal can never claim more than the
//!   world holds.
//!
//! Damage is classified, never guessed at:
//!
//! - a final line without its newline, or which fails to parse, is a *torn
//!   tail*: the write that produced it was not acknowledged. Recovery keeps
//!   the bytes as evidence and treats the record as absent. That is safe only
//!   because every mutation is decided by probing the world, not by trusting
//!   the last intent.
//! - any other defect (bad checksum, broken chain, gap, unknown version,
//!   modified plan) is [`JournalDamage::Corrupt`]. Corrupt journals are never
//!   acted on; recovery reports them as needing manual review.
//!
//! The journal is not part of any SQLite transaction. It is the only record
//! that spans the filesystem, the catalogue and the process lifetime.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::model::Event;

/// Journal format identifier. Older builds must refuse anything newer.
pub const JOURNAL_FORMAT: &str = "emuwiz-transform-journal";
pub const JOURNAL_VERSION: u32 = 1;
pub const PLAN_FILE: &str = "plan.json";
pub const LOG_FILE: &str = "log.jsonl";
/// Bound on a single log line so a hostile journal cannot exhaust memory.
const MAX_RECORD_BYTES: usize = 64 * 1024;
/// Bound on a whole log so recovery time and memory stay bounded.
const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;

/// One durable journal record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub format: String,
    pub version: u32,
    pub seq: u64,
    /// Hex SHA-256 of the previous record's `sum` (or of the plan for seq 0).
    pub prev: String,
    pub at_unix_ms: u64,
    pub event: Event,
    /// Hex SHA-256 over `format|version|seq|prev|at|event-json`.
    pub sum: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalDamage {
    /// The last line was not acknowledged. Its raw bytes are kept.
    TornTail { offset: u64, bytes: Vec<u8> },
    /// The journal cannot be trusted at all.
    Corrupt(String),
}

/// A loaded journal: the plan bytes, the verified records and any torn tail.
#[derive(Debug, Clone)]
pub struct LoadedJournal {
    pub dir: PathBuf,
    pub plan_bytes: Vec<u8>,
    pub plan_digest: [u8; 32],
    pub records: Vec<Record>,
    pub torn_tail: Option<(u64, Vec<u8>)>,
}

impl LoadedJournal {
    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.records.iter().map(|r| &r.event)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn record_sum(seq: u64, prev: &str, at: u64, event_json: &str) -> String {
    let mut h = Sha256::new();
    h.update(JOURNAL_FORMAT.as_bytes());
    h.update(b"|");
    h.update(JOURNAL_VERSION.to_le_bytes());
    h.update(b"|");
    h.update(seq.to_le_bytes());
    h.update(b"|");
    h.update(prev.as_bytes());
    h.update(b"|");
    h.update(at.to_le_bytes());
    h.update(b"|");
    h.update(event_json.as_bytes());
    hex(&h.finalize())
}

/// A transaction id is a single safe path component: 32 lowercase hex chars.
pub fn valid_txn_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn new_txn_id() -> io::Result<String> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| io::Error::other("system random source unavailable"))?;
    Ok(hex(&bytes))
}

pub(crate) fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Creates the journal root if needed (mode 0700) and refuses a symlinked or
/// foreign-permission root.
pub fn ensure_root(root: &Path) -> io::Result<()> {
    match fs::symlink_metadata(root) {
        Ok(meta) => check_private_dir(root, &meta),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Every level created here is a new directory entry in its
            // parent: each parent is fsynced, not only the last (D1).
            let missing: Vec<&Path> = root
                .ancestors()
                .take_while(|p| fs::symlink_metadata(p).is_err())
                .collect();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(root)?;
            }
            #[cfg(not(unix))]
            fs::create_dir_all(root)?;
            for dir in missing {
                if let Some(parent) = dir.parent() {
                    sync_dir(parent)?;
                }
            }
            check_private_dir(root, &fs::symlink_metadata(root)?)
        }
        Err(e) => Err(e),
    }
}

fn check_private_dir(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(io::Error::other(format!(
            "journal location {} is not a real directory",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o022 != 0 || meta.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::other(format!(
                "journal location {} is writable by others or not owned by this user",
                path.display()
            )));
        }
    }
    Ok(())
}

/// An open, writable journal. Only one writer per transaction exists at a
/// time; the caller holds the transaction's resource locks.
pub struct JournalWriter {
    dir: PathBuf,
    file: File,
    next_seq: u64,
    prev: String,
    fault: Option<super::fault::FaultPlan>,
}

impl JournalWriter {
    /// Creates a new transaction directory and writes its immutable plan.
    pub fn create(
        root: &Path,
        txn_id: &str,
        plan_bytes: &[u8],
        fault: Option<super::fault::FaultPlan>,
    ) -> io::Result<Self> {
        if !valid_txn_id(txn_id) {
            return Err(io::Error::other("invalid transaction id"));
        }
        ensure_root(root)?;
        let dir = root.join(txn_id);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&dir)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&dir)?;
        let mut plan = create_new_file(&dir.join(PLAN_FILE), 0o400)?;
        plan.write_all(plan_bytes)?;
        plan.sync_all()?;
        let log = create_new_file(&dir.join(LOG_FILE), 0o600)?;
        log.sync_all()?;
        drop(log);
        sync_dir(&dir)?;
        sync_dir(root)?;
        let digest: [u8; 32] = Sha256::digest(plan_bytes).into();
        let file = OpenOptions::new().append(true).open(dir.join(LOG_FILE))?;
        Ok(Self {
            dir,
            file,
            next_seq: 0,
            prev: hex(&digest),
            fault,
        })
    }

    /// Reopens an existing journal for appending after recovery has loaded
    /// it. A torn tail is truncated only after its bytes were captured by the
    /// caller in `loaded`, and the truncation itself is then journalled.
    pub fn reopen(
        loaded: &LoadedJournal,
        fault: Option<super::fault::FaultPlan>,
    ) -> io::Result<Self> {
        let path = loaded.dir.join(LOG_FILE);
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        if let Some((offset, _)) = &loaded.torn_tail {
            file.set_len(*offset)?;
            file.sync_all()?;
        }
        drop(file);
        let file = OpenOptions::new().append(true).open(&path)?;
        let (next_seq, prev) = match loaded.records.last() {
            Some(r) => (r.seq + 1, r.sum.clone()),
            None => (0, hex(&loaded.plan_digest)),
        };
        Ok(Self {
            dir: loaded.dir.clone(),
            file,
            next_seq,
            prev,
            fault,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Appends one record and makes it durable before returning.
    pub fn append(&mut self, event: Event) -> io::Result<Record> {
        let at = now_ms();
        let event_json = serde_json::to_string(&event).map_err(io::Error::other)?;
        let sum = record_sum(self.next_seq, &self.prev, at, &event_json);
        let record = Record {
            format: JOURNAL_FORMAT.into(),
            version: JOURNAL_VERSION,
            seq: self.next_seq,
            prev: self.prev.clone(),
            at_unix_ms: at,
            event,
            sum: sum.clone(),
        };
        let mut line = serde_json::to_vec(&record).map_err(io::Error::other)?;
        line.push(b'\n');
        if line.len() > MAX_RECORD_BYTES {
            return Err(io::Error::other("journal record exceeds its size bound"));
        }
        if let Some(fault) = &self.fault {
            fault.before_journal_write(self.next_seq, &mut self.file, &line)?;
        }
        self.file.write_all(&line)?;
        self.file.sync_data()?;
        self.next_seq += 1;
        self.prev = sum;
        if let Some(fault) = &self.fault {
            fault.after_journal_write(record.seq, &record.event)?;
        }
        Ok(record)
    }
}

fn create_new_file(path: &Path, mode: u32) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(not(unix))]
    let _ = mode;
    options.open(path)
}

fn read_bounded_nofollow(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if meta.len() > limit {
        return Err(io::Error::other(format!(
            "{} exceeds its size bound",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other(format!(
            "{} grew past its size bound",
            path.display()
        )));
    }
    Ok(bytes)
}

/// Loads and verifies a journal. Never modifies anything.
pub fn load(root: &Path, txn_id: &str) -> Result<LoadedJournal, JournalDamage> {
    let corrupt = |m: String| JournalDamage::Corrupt(m);
    if !valid_txn_id(txn_id) {
        return Err(corrupt(format!("{txn_id:?} is not a valid transaction id")));
    }
    let dir = root.join(txn_id);
    let meta =
        fs::symlink_metadata(&dir).map_err(|e| corrupt(format!("journal directory: {e}")))?;
    check_private_dir(&dir, &meta).map_err(|e| corrupt(e.to_string()))?;
    let plan_bytes = read_bounded_nofollow(&dir.join(PLAN_FILE), MAX_LOG_BYTES)
        .map_err(|e| corrupt(format!("plan: {e}")))?;
    let plan_digest: [u8; 32] = Sha256::digest(&plan_bytes).into();
    let log = read_bounded_nofollow(&dir.join(LOG_FILE), MAX_LOG_BYTES)
        .map_err(|e| corrupt(format!("log: {e}")))?;
    let mut records = Vec::new();
    let mut prev = hex(&plan_digest);
    let mut offset = 0usize;
    let mut torn_tail = None;
    while offset < log.len() {
        let rest = &log[offset..];
        let (line, complete) = match rest.iter().position(|&b| b == b'\n') {
            Some(end) => (&rest[..end], true),
            None => (rest, false),
        };
        let next = offset + line.len() + usize::from(complete);
        let is_last = next >= log.len();
        let parsed: Option<Record> = if complete && line.len() <= MAX_RECORD_BYTES {
            serde_json::from_slice(line).ok()
        } else {
            None
        };
        let Some(record) = parsed else {
            if is_last {
                torn_tail = Some((offset as u64, rest.to_vec()));
                break;
            }
            return Err(corrupt(format!("unreadable record at byte {offset}")));
        };
        if record.format != JOURNAL_FORMAT || record.version != JOURNAL_VERSION {
            return Err(corrupt(format!(
                "record {} has format {} v{}; this build reads {JOURNAL_FORMAT} v{JOURNAL_VERSION}",
                record.seq, record.format, record.version
            )));
        }
        if record.seq != records.len() as u64 || record.prev != prev {
            return Err(corrupt(format!(
                "record chain broken at sequence {}",
                record.seq
            )));
        }
        let event_json =
            serde_json::to_string(&record.event).map_err(|e| corrupt(e.to_string()))?;
        if record_sum(record.seq, &record.prev, record.at_unix_ms, &event_json) != record.sum {
            // A complete, parseable final record whose checksum fails was
            // still written in full: that is tampering or media damage, not
            // an unacknowledged write.
            return Err(corrupt(format!("record {} fails its checksum", record.seq)));
        }
        prev = record.sum.clone();
        records.push(record);
        offset = next;
    }
    Ok(LoadedJournal {
        dir,
        plan_bytes,
        plan_digest,
        records,
        torn_tail,
    })
}

/// Lists transaction ids under `root`; entries that are not valid ids are
/// returned separately so they are reported, not silently ignored.
pub fn list(root: &Path) -> io::Result<(Vec<String>, Vec<PathBuf>)> {
    let mut ids = Vec::new();
    let mut foreign = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((ids, foreign)),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if valid_txn_id(&name) {
            ids.push(name);
        } else if !name.ends_with(".lock") {
            foreign.push(entry.path());
        }
    }
    ids.sort();
    Ok((ids, foreign))
}
