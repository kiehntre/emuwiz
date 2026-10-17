//! A bounded, in-memory cache of DAT lookup indexes for explicit support-file
//! content checks.
//!
//! Building a [`DatIndex`] means parsing and indexing a whole catalogue, so a
//! second "Check content against DAT" click should not repeat it. Each entry
//! is bound to the catalogue's stable reference and to a fingerprint of its
//! backing file (device, inode, size, modification time) plus the content
//! SHA-256 and revision the inventory records, where it has them. A changed
//! fingerprint is a miss: the caller re-resolves (and so revalidates) the
//! catalogue. Entries whose catalogue is no longer installed and enabled are
//! dropped by [`CatalogueIndexCache::retain_only`]. Memory is bounded by an
//! estimated byte budget with least-recently-used eviction; nothing is
//! persisted. Indexes are built outside the lock and shared as `Arc`s.
use crate::dat::index::DatIndex;
use crate::dat::model::ParsedDat;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Default retained budget: enough for several large catalogues.
pub const DEFAULT_CACHE_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

/// What a cached index is valid for. Any difference is a miss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueFingerprint {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified: i64,
    pub modified_ns: i64,
    pub content_sha256: Option<String>,
    pub revision: Option<String>,
}

impl CatalogueFingerprint {
    /// Fingerprint of a backing file (no symlinks followed), or `None` when it
    /// cannot be observed, in which case nothing is cached for it.
    pub fn of_file(
        path: &Path,
        content_sha256: Option<String>,
        revision: Option<String>,
    ) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path).ok()?;
        metadata.is_file().then(|| Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: metadata.mtime(),
            modified_ns: metadata.mtime_nsec(),
            content_sha256,
            revision,
        })
    }
}

/// One built, comparable catalogue.
#[derive(Debug)]
pub struct CachedCatalogue {
    pub label: String,
    pub provenance: String,
    pub index: DatIndex,
    /// ROM entries in the catalogue, and how many publish SHA-1 or SHA-256.
    pub total_roms: usize,
    pub comparable_roms: usize,
    /// Estimated retained size of `index`.
    pub estimated_bytes: u64,
}

impl CachedCatalogue {
    pub fn build(label: String, provenance: String, dat: &ParsedDat) -> Self {
        let mut total_roms = 0usize;
        let mut comparable_roms = 0usize;
        let mut estimated_bytes = 4096u64;
        for game in &dat.games {
            for rom in &game.roms {
                total_roms += 1;
                if rom.sha1.is_some() || rom.sha256.is_some() {
                    comparable_roms += 1;
                }
                // Names, hash strings and the per-key index overhead.
                estimated_bytes += (game.name.len() + rom.name.len() + 256) as u64;
            }
        }
        Self {
            label,
            provenance,
            index: DatIndex::build(dat),
            total_roms,
            comparable_roms,
            estimated_bytes,
        }
    }
}

struct Entry {
    fingerprint: CatalogueFingerprint,
    catalogue: Arc<CachedCatalogue>,
    last_used: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub retained_bytes: u64,
    pub entries: usize,
}

pub struct CatalogueIndexCache {
    budget_bytes: u64,
    entries: HashMap<String, Entry>,
    tick: u64,
    stats: CacheStats,
}

pub type SharedCatalogueIndexCache = Arc<Mutex<CatalogueIndexCache>>;

impl Default for CatalogueIndexCache {
    fn default() -> Self {
        Self::with_budget(DEFAULT_CACHE_BUDGET_BYTES)
    }
}

impl CatalogueIndexCache {
    pub fn with_budget(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            entries: HashMap::new(),
            tick: 0,
            stats: CacheStats::default(),
        }
    }

    pub fn shared(self) -> SharedCatalogueIndexCache {
        Arc::new(Mutex::new(self))
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            retained_bytes: self.retained(),
            entries: self.entries.len(),
            ..self.stats
        }
    }

    fn retained(&self) -> u64 {
        self.entries
            .values()
            .map(|entry| entry.catalogue.estimated_bytes)
            .sum()
    }

    /// The cached index for `key`, only if it was built from exactly
    /// `fingerprint`. A stale entry is removed.
    pub fn lookup(
        &mut self,
        key: &str,
        fingerprint: &CatalogueFingerprint,
    ) -> Option<Arc<CachedCatalogue>> {
        self.tick += 1;
        match self.entries.get_mut(key) {
            Some(entry) if entry.fingerprint == *fingerprint => {
                entry.last_used = self.tick;
                self.stats.hits += 1;
                Some(entry.catalogue.clone())
            }
            Some(_) => {
                self.entries.remove(key);
                self.stats.misses += 1;
                None
            }
            None => {
                self.stats.misses += 1;
                None
            }
        }
    }

    /// Keeps `catalogue` for `key`, evicting least-recently-used entries to
    /// stay within budget. A catalogue larger than the whole budget is not
    /// retained (the caller still uses it for the current check).
    pub fn insert(
        &mut self,
        key: String,
        fingerprint: CatalogueFingerprint,
        catalogue: Arc<CachedCatalogue>,
    ) -> bool {
        self.entries.remove(&key);
        if catalogue.estimated_bytes > self.budget_bytes {
            return false;
        }
        while self.retained() + catalogue.estimated_bytes > self.budget_bytes {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
            self.stats.evictions += 1;
        }
        self.tick += 1;
        self.entries.insert(
            key,
            Entry {
                fingerprint,
                catalogue,
                last_used: self.tick,
            },
        );
        true
    }

    /// Drops every entry whose catalogue is not in `current` (disabled,
    /// removed or no longer eligible).
    pub fn retain_only(&mut self, current: &BTreeSet<String>) {
        let before = self.entries.len();
        self.entries.retain(|key, _| current.contains(key));
        self.stats.evictions += (before - self.entries.len()) as u64;
    }
}

#[cfg(test)]
#[path = "catalogue_index_cache_tests.rs"]
mod tests;
