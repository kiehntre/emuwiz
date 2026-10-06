//! Conservative persistent hashes for loose combined DAT audits.
//!
//! This cache is deliberately separate from provider verification state. It is
//! derived data only: a corrupt, stale, unavailable, or contended cache is
//! equivalent to an empty cache and the audit hashes the file normally.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const CACHE_SCHEMA_VERSION: u32 = 1;
pub const CACHE_FILE_NAME: &str = "loose-hashes.json";
pub const CACHE_DIRECTORY_NAME: &str = "audit-cache";
pub const MAX_CACHE_ENTRIES: usize = 100_000;
pub const MAX_CACHE_AGE_SECONDS: i64 = 180 * 24 * 60 * 60;
const STALE_LOCK_AGE_SECONDS: i64 = 60 * 60;

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AuditCacheMetrics {
    pub scanned_candidates: usize,
    pub cache_eligible: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub files_hashed: usize,
    pub invalidated_entries: usize,
    pub load_failures: usize,
    pub save_failures: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileFingerprint {
    // This is intentionally a metadata cache, not a content proof. On coarse
    // mtime filesystems (including FAT/exFAT and some network filesystems), an
    // in-place same-size mutation can theoretically preserve every field and
    // produce a stale hit. We accept that residual risk here rather than add a
    // content sample or an extra hash to every cache hit.
    path: String,
    file_type: FileType,
    size_bytes: u64,
    modified_unix_nanos: Option<i128>,
    #[cfg(unix)]
    device: Option<u64>,
    #[cfg(unix)]
    inode: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum FileType {
    Regular,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CacheEntry {
    fingerprint: FileFingerprint,
    crc32: String,
    md5: String,
    sha1: String,
    last_used_unix_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CacheDocument {
    schema_version: u32,
    entries: BTreeMap<String, CacheEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedHashes {
    pub size_bytes: u64,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AuditCacheConfig {
    #[default]
    Default,
    At(PathBuf),
    Disabled,
}

impl AuditCacheConfig {
    /// What an entry point that was handed no cache choice uses: the user's
    /// default cache in a normal build, and never the real one inside this
    /// crate's own tests. Callers in other crates make the same choice at their
    /// own call site; [`AuditHashCache::load_default`] also refuses the real
    /// cache in any cargo-built test binary as a backstop.
    pub fn convenience_default() -> Self {
        if cfg!(test) {
            Self::Disabled
        } else {
            Self::Default
        }
    }
}

/// Whether `exe` has the shape of a cargo-built test (or benchmark) binary:
/// `<target>/<profile>/deps/<crate>-<16 lowercase hex digits>` (an optional
/// `.exe` is ignored). Both halves are required. The directory alone is not
/// enough - an unrelated program installed under some folder that happens to
/// be called `deps` must keep its normal cache - and the hash suffix alone is
/// not enough either. `cargo run` binaries (`<target>/<profile>/<name>`),
/// installed copies and release builds never match.
fn is_cargo_test_binary_path(exe: &Path) -> bool {
    let in_deps = exe
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|directory| directory == "deps");
    let hashed_name = exe
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit_once('-'))
        .is_some_and(|(crate_name, hash)| {
            !crate_name.is_empty()
                && hash.len() == 16
                && hash
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        });
    in_deps && hashed_name
}

/// Whether this process is such a binary. Used only to keep tests away from
/// the user's real audit cache.
fn running_as_cargo_test_binary() -> bool {
    static IS_TEST_BINARY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IS_TEST_BINARY.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .is_some_and(|exe| is_cargo_test_binary_path(&exe))
    })
}

#[derive(Debug, Clone)]
pub struct AuditHashCache {
    path: PathBuf,
    entries: BTreeMap<String, CacheEntry>,
    enabled: bool,
    pub metrics: AuditCacheMetrics,
}

impl AuditHashCache {
    pub fn at(path: PathBuf) -> Self {
        Self {
            path,
            entries: BTreeMap::new(),
            enabled: true,
            metrics: AuditCacheMetrics::default(),
        }
    }

    pub fn disabled() -> Self {
        Self {
            path: PathBuf::new(),
            entries: BTreeMap::new(),
            enabled: false,
            metrics: AuditCacheMetrics::default(),
        }
    }

    pub fn from_config(config: &AuditCacheConfig) -> Self {
        match config {
            AuditCacheConfig::Default => Self::load_default(),
            AuditCacheConfig::At(path) => Self::load(path.clone()),
            AuditCacheConfig::Disabled => Self::disabled(),
        }
    }

    pub fn default_location() -> Result<PathBuf, String> {
        crate::app_dirs::data_dir()
            .map(|root| root.join(CACHE_DIRECTORY_NAME).join(CACHE_FILE_NAME))
            .map_err(|error| error.to_string())
    }

    pub fn load_default() -> Self {
        // A test binary must never read or write the user's real cache. A test
        // that really wants the default location says so by pointing
        // EMUWIZ_DATA_HOME at a temporary directory; one that wants a cache
        // passes `AuditCacheConfig::At`.
        if running_as_cargo_test_binary()
            && std::env::var_os(crate::app_dirs::DATA_ROOT_OVERRIDE_ENV).is_none()
        {
            log::warn!(
                "refusing the production audit cache from a test binary; use AuditCacheConfig::At or Disabled"
            );
            return Self::disabled();
        }
        match Self::default_location() {
            Ok(path) => Self::load(path),
            Err(_) => {
                let mut cache = Self::at(PathBuf::new());
                cache.metrics.load_failures = 1;
                cache
            }
        }
    }

    pub fn load(path: PathBuf) -> Self {
        let mut cache = Self::at(path.clone());
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return cache,
            Err(_) => {
                cache.metrics.load_failures = 1;
                return cache;
            }
        };
        let Ok(document) = serde_json::from_slice::<CacheDocument>(&bytes) else {
            cache.metrics.load_failures = 1;
            return cache;
        };
        if document.schema_version != CACHE_SCHEMA_VERSION {
            cache.metrics.load_failures = 1;
            return cache;
        }
        cache.entries = document.entries;
        cache.prune_expired(now_unix_seconds());
        cache
    }

    pub fn lookup(&mut self, path: &Path) -> Option<CachedHashes> {
        if !self.enabled {
            return None;
        }
        let fingerprint = match FileFingerprint::observe(path) {
            Some(fingerprint) => fingerprint,
            None => {
                self.metrics.cache_misses += 1;
                return None;
            }
        };
        let key = fingerprint.path.clone();
        let Some(entry) = self.entries.get_mut(&key) else {
            self.metrics.cache_misses += 1;
            return None;
        };
        if entry.fingerprint != fingerprint {
            self.entries.remove(&key);
            self.metrics.invalidated_entries += 1;
            self.metrics.cache_misses += 1;
            return None;
        }
        entry.last_used_unix_seconds = now_unix_seconds();
        self.metrics.cache_hits += 1;
        Some(CachedHashes {
            size_bytes: entry.fingerprint.size_bytes,
            crc32: entry.crc32.clone(),
            md5: entry.md5.clone(),
            sha1: entry.sha1.clone(),
        })
    }

    pub fn insert(&mut self, path: &Path, crc32: String, md5: String, sha1: String) {
        let Some(fingerprint) = FileFingerprint::observe(path) else {
            return;
        };
        self.entries.insert(
            fingerprint.path.clone(),
            CacheEntry {
                fingerprint,
                crc32,
                md5,
                sha1,
                last_used_unix_seconds: now_unix_seconds(),
            },
        );
        self.prune_to(MAX_CACHE_ENTRIES);
    }

    pub fn save(&mut self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.path.as_os_str().is_empty() {
            self.metrics.save_failures += 1;
            return Err("audit cache location is unavailable".to_string());
        }
        self.prune_expired(now_unix_seconds());
        let document = CacheDocument {
            schema_version: CACHE_SCHEMA_VERSION,
            entries: self.entries.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
        let directory = self
            .path
            .parent()
            .ok_or_else(|| "audit cache has no parent directory".to_string())?;
        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let lock_path = directory.join("loose-hashes.lock");
        let _lock = match acquire_lock(&lock_path) {
            Ok(lock) => lock,
            Err(error) => {
                self.metrics.save_failures += 1;
                return Err(error);
            }
        };
        // Holding the lock means no other writer is mid-save, so a temporary
        // file left by a process that no longer exists is an orphan.
        remove_orphaned_temporaries(directory);
        let temporary = directory.join(format!(
            ".{CACHE_FILE_NAME}.{}.{}.tmp",
            std::process::id(),
            TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> Result<(), String> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            file.write_all(&bytes).map_err(|error| error.to_string())?;
            file.flush().map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            fs::rename(&temporary, &self.path).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
            self.metrics.save_failures += 1;
        }
        result
    }

    /// Whether this cache reads and writes anything. A disabled cache is a
    /// no-op that always misses and always saves successfully.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn prune_to(&mut self, maximum: usize) {
        while self.entries.len() > maximum {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| (entry.last_used_unix_seconds, &entry.fingerprint.path))
                .map(|(key, _)| key.clone());
            if let Some(key) = oldest {
                self.entries.remove(&key);
            } else {
                break;
            }
        }
    }

    fn prune_expired(&mut self, now: i64) {
        self.entries.retain(|_, entry| {
            now.saturating_sub(entry.last_used_unix_seconds) <= MAX_CACHE_AGE_SECONDS
        });
        self.prune_to(MAX_CACHE_ENTRIES);
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LockRecord {
    pid: u32,
    created_unix_seconds: i64,
}

struct CacheLock {
    path: PathBuf,
    _file: std::fs::File,
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_lock(path: &Path) -> Result<CacheLock, String> {
    for attempt in 0..2 {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                let record = LockRecord {
                    pid: std::process::id(),
                    created_unix_seconds: now_unix_seconds(),
                };
                let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
                if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
                    let _ = fs::remove_file(path);
                    return Err(format!("audit cache lock could not be written: {error}"));
                }
                return Ok(CacheLock {
                    path: path.to_path_buf(),
                    _file: file,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                if reclaimable_lock(path) {
                    let _ = fs::remove_file(path);
                    continue;
                }
                return Err("audit cache is busy or its lock is not safely stale".to_string());
            }
            Err(error) => return Err(format!("audit cache lock could not be acquired: {error}")),
        }
    }
    Err("audit cache lock could not be acquired".to_string())
}

/// Parses exactly `.loose-hashes.json.<pid>.<sequence>.tmp` (ASCII digits only,
/// both parts non-empty) into its owner pid. Anything else is not ours.
fn temporary_owner_pid(name: &str) -> Option<u32> {
    let rest = name
        .strip_prefix(".")?
        .strip_prefix(CACHE_FILE_NAME)?
        .strip_prefix('.')?
        .strip_suffix(".tmp")?;
    let (pid, sequence) = rest.split_once('.')?;
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    (digits(pid) && digits(sequence)).then_some(())?;
    pid.parse().ok()
}

/// Removes temporary cache files whose writer is demonstrably dead. Called
/// only while the cache lock is held. Conservative on purpose: only regular
/// files with the exact temporary naming convention, never the live cache,
/// never a file whose pid is alive (or is this process), and any failure is
/// ignored - cleanup must never make a valid cache unusable.
fn remove_orphaned_temporaries(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let own = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(temporary_owner_pid) else {
            continue;
        };
        if pid == own || process_is_alive(pid) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if metadata.is_file() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn reclaimable_lock(path: &Path) -> bool {
    if let Ok(bytes) = fs::read(path)
        && let Ok(record) = serde_json::from_slice::<LockRecord>(&bytes)
    {
        return !process_is_alive(record.pid);
    }
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| {
            now_unix_seconds().saturating_sub(age.as_secs() as i64) >= STALE_LOCK_AGE_SECONDS
        })
        .unwrap_or(false)
}

fn process_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) distinguishes an existing process (including one we
        // cannot signal) from a demonstrably absent owner.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

impl FileFingerprint {
    fn observe(path: &Path) -> Option<Self> {
        let link_metadata = fs::symlink_metadata(path).ok()?;
        let file_type = if link_metadata.file_type().is_symlink() {
            FileType::Symlink
        } else if link_metadata.is_file() {
            FileType::Regular
        } else {
            return None;
        };
        let metadata = if file_type == FileType::Symlink {
            fs::metadata(path).ok()?
        } else {
            link_metadata
        };
        let path = normalize_absolute(path)?;
        let modified_unix_nanos = metadata.modified().ok().and_then(|time| {
            time.duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_nanos() as i128)
        });
        Some(Self {
            path: path.to_string_lossy().into_owned(),
            file_type,
            size_bytes: metadata.len(),
            modified_unix_nanos,
            #[cfg(unix)]
            device: Some(std::os::unix::fs::MetadataExt::dev(&metadata)),
            #[cfg(unix)]
            inode: Some(std::os::unix::fs::MetadataExt::ino(&metadata)),
        })
    }
}

fn normalize_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Some(normalized)
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_requires_absolute_path_and_normalises_components() {
        assert!(normalize_absolute(Path::new("relative/file")).is_none());
        assert_eq!(
            normalize_absolute(Path::new("/tmp/a/../b/./file")).unwrap(),
            PathBuf::from("/tmp/b/file")
        );
    }

    #[test]
    fn cache_bound_prunes_oldest_entries() {
        let mut cache = AuditHashCache::at(PathBuf::from("/tmp/unused-cache.json"));
        cache.entries = (0..4)
            .map(|index| {
                let path = format!("/synthetic/{index}");
                (
                    path.clone(),
                    CacheEntry {
                        fingerprint: FileFingerprint {
                            path,
                            file_type: FileType::Regular,
                            size_bytes: 1,
                            modified_unix_nanos: None,
                            #[cfg(unix)]
                            device: None,
                            #[cfg(unix)]
                            inode: None,
                        },
                        crc32: "00000000".to_string(),
                        md5: "0".repeat(32),
                        sha1: "0".repeat(40),
                        last_used_unix_seconds: index,
                    },
                )
            })
            .collect();
        cache.prune_to(2);
        assert_eq!(cache.len(), 2);
        assert!(!cache.entries.contains_key("/synthetic/0"));
        assert!(!cache.entries.contains_key("/synthetic/1"));
    }

    #[test]
    fn unchanged_file_is_a_persistent_hit_and_size_change_misses() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("game.rom");
        std::fs::write(&file, b"one").unwrap();
        let cache_path = root.path().join("cache").join(CACHE_FILE_NAME);

        let mut first = AuditHashCache::at(cache_path.clone());
        first.insert(&file, "11111111".into(), "22".repeat(16), "33".repeat(20));
        assert_eq!(first.metrics.cache_hits, 0);
        first.save().unwrap();

        let mut second = AuditHashCache::load(cache_path);
        assert_eq!(second.lookup(&file).unwrap().crc32, "11111111");
        assert_eq!(second.metrics.cache_hits, 1);

        std::fs::write(&file, b"changed-size").unwrap();
        assert!(second.lookup(&file).is_none());
        assert_eq!(second.metrics.invalidated_entries, 1);
    }

    #[test]
    fn precise_mtime_and_identity_fields_invalidate_a_cached_entry() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("game.rom");
        std::fs::write(&file, b"same").unwrap();
        let mut cache = AuditHashCache::at(root.path().join(CACHE_FILE_NAME));
        cache.insert(&file, "11".into(), "22".into(), "33".into());
        let key = normalize_absolute(&file)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        cache
            .entries
            .get_mut(&key)
            .unwrap()
            .fingerprint
            .modified_unix_nanos = Some(i128::MIN);
        assert!(cache.lookup(&file).is_none());

        cache.insert(&file, "11".into(), "22".into(), "33".into());
        #[cfg(unix)]
        {
            cache.entries.get_mut(&key).unwrap().fingerprint.inode = Some(u64::MAX);
            assert!(cache.lookup(&file).is_none());
        }
    }

    #[test]
    fn corrupt_cache_fails_open_and_reloads_after_save() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(CACHE_FILE_NAME);
        std::fs::write(&path, b"not-json").unwrap();
        let cache = AuditHashCache::load(path.clone());
        assert_eq!(cache.metrics.load_failures, 1);
        assert_eq!(cache.len(), 0);

        let mut cache = AuditHashCache::at(path.clone());
        let file = root.path().join("game.rom");
        std::fs::write(&file, b"game").unwrap();
        cache.insert(&file, "11".into(), "22".into(), "33".into());
        cache.save().unwrap();
        assert_eq!(AuditHashCache::load(path).len(), 1);
    }

    #[test]
    fn newer_schema_fails_open_without_using_entries() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(CACHE_FILE_NAME);
        let document = CacheDocument {
            schema_version: CACHE_SCHEMA_VERSION + 1,
            entries: BTreeMap::new(),
        };
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let cache = AuditHashCache::load(path);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.metrics.load_failures, 1);
    }

    #[test]
    fn live_lock_is_contention_but_dead_lock_is_reclaimed() {
        let root = tempfile::tempdir().unwrap();
        let lock = root.path().join("loose-hashes.lock");
        std::fs::write(
            &lock,
            serde_json::to_vec(&LockRecord {
                pid: std::process::id(),
                created_unix_seconds: now_unix_seconds(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(acquire_lock(&lock).is_err());

        std::fs::write(
            &lock,
            serde_json::to_vec(&LockRecord {
                pid: 2_000_000_000,
                created_unix_seconds: now_unix_seconds(),
            })
            .unwrap(),
        )
        .unwrap();
        let acquired = acquire_lock(&lock).unwrap();
        drop(acquired);
        assert!(!lock.exists());
    }

    #[cfg(unix)]
    #[test]
    fn malformed_old_lock_is_reclaimed_and_write_errors_clean_up() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let root = tempfile::tempdir().unwrap();
        let lock = root.path().join("loose-hashes.lock");
        std::fs::write(&lock, b"malformed").unwrap();
        let old = libc::timespec {
            tv_sec: 1,
            tv_nsec: 0,
        };
        let times = [old, old];
        let c_path = CString::new(lock.as_os_str().as_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) },
            0
        );
        let acquired = acquire_lock(&lock).unwrap();
        drop(acquired);
        assert!(!lock.exists());

        let target = root.path().join("existing-directory");
        std::fs::create_dir(&target).unwrap();
        let mut cache = AuditHashCache::at(target.clone());
        assert!(cache.save().is_err());
        assert!(!root.path().join("loose-hashes.lock").exists());
    }

    #[test]
    fn normal_save_removes_lock() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = AuditHashCache::at(root.path().join("cache").join(CACHE_FILE_NAME));
        cache.save().unwrap();
        assert!(!root.path().join("cache/loose-hashes.lock").exists());
    }

    // ---- orphaned temporary files and the production-cache tripwire -----------

    const DEAD_PID: u32 = 2_000_000_000;

    fn temp_name(pid: u32, sequence: u32) -> String {
        format!(".{CACHE_FILE_NAME}.{pid}.{sequence}.tmp")
    }

    fn cache_with_one_entry(directory: &Path) -> AuditHashCache {
        let file = directory.join("game.bin");
        std::fs::write(&file, b"abcd").unwrap();
        let mut cache = AuditHashCache::at(directory.join("cache").join(CACHE_FILE_NAME));
        cache.insert(&file, "ed82cd11".into(), "m".repeat(32), "s".repeat(40));
        cache
    }

    #[test]
    fn a_temporary_file_name_is_parsed_exactly_or_not_at_all() {
        assert_eq!(temporary_owner_pid(&temp_name(1234, 7)), Some(1234));
        for malformed in [
            ".loose-hashes.json.abc.1.tmp",
            ".loose-hashes.json.123.tmp",
            ".loose-hashes.json.123.4.5.tmp",
            ".loose-hashes.json.-1.2.tmp",
            ".loose-hashes.json.+5.2.tmp",
            ".loose-hashes.json..2.tmp",
            ".loose-hashes.json.5..tmp",
            "loose-hashes.json.5.2.tmp",
            ".loose-hashes.json.5.2.tmp.bak",
            ".loose-hashes.json.5.2",
            ".other.json.5.2.tmp",
            "loose-hashes.json",
            ".loose-hashes.json.99999999999999999999.1.tmp",
        ] {
            assert_eq!(temporary_owner_pid(malformed), None, "{malformed}");
        }
    }

    #[test]
    fn a_dead_process_orphan_is_removed_by_the_next_save() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = cache_with_one_entry(root.path());
        let directory = root.path().join("cache");
        std::fs::create_dir_all(&directory).unwrap();
        let orphan = directory.join(temp_name(DEAD_PID, 3));
        std::fs::write(&orphan, b"half written").unwrap();
        cache.save().unwrap();
        assert!(
            !orphan.exists(),
            "the orphan was left by a process that no longer exists"
        );
        assert!(directory.join(CACHE_FILE_NAME).is_file());
    }

    #[test]
    fn a_live_process_temporary_file_is_retained() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = cache_with_one_entry(root.path());
        let directory = root.path().join("cache");
        std::fs::create_dir_all(&directory).unwrap();
        // This very process is demonstrably alive, so its temp file is not an orphan.
        let live = directory.join(temp_name(std::process::id(), 900_000));
        std::fs::write(&live, b"in use").unwrap();
        cache.save().unwrap();
        assert!(live.exists());
    }

    #[test]
    fn malformed_and_unrelated_files_are_never_touched() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = cache_with_one_entry(root.path());
        let directory = root.path().join("cache");
        std::fs::create_dir_all(&directory).unwrap();
        let keep = [
            ".loose-hashes.json.abc.1.tmp".to_string(),
            format!(".loose-hashes.json.{DEAD_PID}.tmp"),
            format!(".loose-hashes.json.{DEAD_PID}.1.2.tmp"),
            "unrelated.tmp".to_string(),
            format!(".something-else.{DEAD_PID}.1.tmp"),
            "notes.txt".to_string(),
        ];
        for name in &keep {
            std::fs::write(directory.join(name), b"keep").unwrap();
        }
        cache.save().unwrap();
        for name in &keep {
            assert!(directory.join(name).exists(), "{name} must be left alone");
        }
    }

    #[test]
    fn a_dead_pid_directory_or_symlink_with_the_temp_name_is_not_removed() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = cache_with_one_entry(root.path());
        let directory = root.path().join("cache");
        std::fs::create_dir_all(&directory).unwrap();
        let as_directory = directory.join(temp_name(DEAD_PID, 1));
        std::fs::create_dir_all(&as_directory).unwrap();
        let target = root.path().join("precious.txt");
        std::fs::write(&target, b"do not delete").unwrap();
        let as_symlink = directory.join(temp_name(DEAD_PID, 2));
        std::os::unix::fs::symlink(&target, &as_symlink).unwrap();
        cache.save().unwrap();
        assert!(as_directory.is_dir());
        assert!(as_symlink.symlink_metadata().is_ok());
        assert_eq!(std::fs::read(&target).unwrap(), b"do not delete");
    }

    #[test]
    fn the_live_cache_is_never_removed_by_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("cache");
        std::fs::create_dir_all(&directory).unwrap();
        let live = directory.join(CACHE_FILE_NAME);
        std::fs::write(&live, b"existing cache").unwrap();
        remove_orphaned_temporaries(&directory);
        assert_eq!(std::fs::read(&live).unwrap(), b"existing cache");
    }

    #[test]
    fn an_active_lock_prevents_cleanup_and_the_save() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = cache_with_one_entry(root.path());
        let directory = root.path().join("cache");
        std::fs::create_dir_all(&directory).unwrap();
        let orphan = directory.join(temp_name(DEAD_PID, 5));
        std::fs::write(&orphan, b"maybe someone else's").unwrap();
        // Another live writer holds the lock.
        std::fs::write(
            directory.join("loose-hashes.lock"),
            serde_json::to_vec(&LockRecord {
                pid: std::process::id(),
                created_unix_seconds: now_unix_seconds(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(
            cache.save().is_err(),
            "the save is refused while the lock is live"
        );
        assert!(orphan.exists(), "cleanup never runs without the lock");
    }

    #[test]
    fn a_normal_save_renames_atomically_and_leaves_no_temporary_or_lock() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = cache_with_one_entry(root.path());
        cache.save().unwrap();
        let directory = root.path().join("cache");
        let names: Vec<String> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![CACHE_FILE_NAME.to_string()], "{names:?}");
        let reloaded = AuditHashCache::load(directory.join(CACHE_FILE_NAME));
        assert_eq!(reloaded.len(), 1);
    }

    #[test]
    fn a_failed_save_removes_its_own_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = cache_with_one_entry(root.path());
        // Renaming onto a non-empty directory fails after the temp file is written.
        let blocked = root.path().join("cache").join(CACHE_FILE_NAME);
        std::fs::create_dir_all(blocked.join("inside")).unwrap();
        assert!(cache.save().is_err());
        let leftovers: Vec<_> = std::fs::read_dir(root.path().join("cache"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_test_binary_never_gets_the_production_default_cache() {
        // This test process is itself a cargo test binary (it lives in `deps`).
        assert!(running_as_cargo_test_binary());
        for cache in [
            AuditHashCache::load_default(),
            AuditHashCache::from_config(&AuditCacheConfig::Default),
        ] {
            assert!(
                !cache.enabled,
                "the real cache must not be reachable from a test"
            );
        }
        let mut cache = AuditHashCache::from_config(&AuditCacheConfig::Default);
        let file = std::env::temp_dir().join("emuwiz-cache-tripwire-probe");
        std::fs::write(&file, b"x").unwrap();
        cache.insert(&file, "00000000".into(), "0".repeat(32), "0".repeat(40));
        assert!(
            cache.save().is_ok(),
            "a disabled cache saves nothing and succeeds"
        );
        let _ = std::fs::remove_file(&file);
        assert_eq!(
            AuditCacheConfig::convenience_default(),
            AuditCacheConfig::Disabled
        );
    }

    #[test]
    fn an_explicit_cache_path_still_works_in_tests() {
        let root = tempfile::tempdir().unwrap();
        let mut cache =
            AuditHashCache::from_config(&AuditCacheConfig::At(root.path().join(CACHE_FILE_NAME)));
        assert!(cache.enabled);
        let file = root.path().join("f.bin");
        std::fs::write(&file, b"abcd").unwrap();
        cache.insert(&file, "ed82cd11".into(), "m".repeat(32), "s".repeat(40));
        cache.save().unwrap();
        assert!(root.path().join(CACHE_FILE_NAME).is_file());
    }

    #[test]
    fn test_binary_detection_matches_cargo_test_layouts_and_nothing_else() {
        let yes = [
            "/work/target/debug/deps/archivefs_core-207406efa7f10f78",
            "/work/target/release/deps/archivefs_gui-84a5c293b3033299",
            "/work/target/x86_64-unknown-linux-gnu/debug/deps/emuwiz_cli-dfb5f136e036aafd",
            "/cache/emuwiz-targets/debug/deps/a-b-c_d-0123456789abcdef",
            "C:/work/target/debug/deps/archivefs_core-207406efa7f10f78.exe",
        ];
        for path in yes {
            assert!(is_cargo_test_binary_path(Path::new(path)), "{path}");
        }
        let no = [
            // `cargo run` and installed/release binaries.
            "/work/target/debug/emuwiz-v2",
            "/work/target/release/emuwiz-v2",
            "/work/target/debug/emuwiz",
            "/usr/bin/emuwiz-v2",
            "/home/user/.cargo/bin/emuwiz-v2",
            "/tmp/.mount_EmuWizAbc/usr/bin/emuwiz-v2",
            "/opt/EmuWiz/emuwiz-v2",
            // An unrelated folder that is merely called `deps`.
            "/opt/app/deps/emuwiz-v2",
            "/opt/app/deps/emuwiz",
            "/opt/app/deps/emuwiz-123",
            "/opt/app/deps/emuwiz-0123456789abcdeg",
            "/opt/app/deps/emuwiz-0123456789ABCDEF",
            "/opt/app/deps/emuwiz-0123456789abcdef0",
            "/opt/app/deps/-0123456789abcdef",
            // The right name shape, but not in `deps`.
            "/work/target/debug/archivefs_core-207406efa7f10f78",
            "/work/target/debug/examples/demo-207406efa7f10f78",
            "/opt/app/depsx/emuwiz-0123456789abcdef",
            "/",
            "",
        ];
        for path in no {
            assert!(!is_cargo_test_binary_path(Path::new(path)), "{path:?}");
        }
    }
}
