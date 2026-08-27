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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditCacheConfig {
    Default,
    At(PathBuf),
    Disabled,
}

impl Default for AuditCacheConfig {
    fn default() -> Self {
        Self::Default
    }
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

    pub fn len(&self) -> usize {
        self.entries.len()
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

fn reclaimable_lock(path: &Path) -> bool {
    if let Ok(bytes) = fs::read(path) {
        if let Ok(record) = serde_json::from_slice::<LockRecord>(&bytes) {
            return !process_is_alive(record.pid);
        }
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
}
