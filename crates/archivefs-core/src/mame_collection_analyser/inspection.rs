//! Bounded read-only inventory for the Health page. No database or transaction.
//! ZIP decoding and hashes use the existing verified member reader. Loose files
//! use that same streaming hasher and safe-read policy. CHDs are presence only.
use std::fs::{self, Metadata};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::{MameCollectionInventory, MameObservedMember, MameObservedSet};
use crate::dat::archive::hash::hash_member_stream;
use crate::dat::archive::limits::{ArchiveLimits, MAX_ARCHIVE_RUN_LOGICAL_BYTES};
use crate::dat::archive::zip::ZipArchiveSource;
use crate::dat::archive::{ArchiveMemberSource, ArchiveMemberStatus, ArchiveRunBudget};
use crate::safe_read::{TrustedRoots, open_bounded_read};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceStamp {
    pub path: PathBuf,
    pub directory: bool,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
    #[cfg(unix)]
    pub changed_seconds: i64,
    #[cfg(unix)]
    pub changed_nanoseconds: i64,
}

impl SourceStamp {
    pub fn observe(path: &Path) -> Result<Self, String> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        {
            return Err("Choose an absolute path without '.' or '..'.".into());
        }
        for parent in path.ancestors() {
            if fs::symlink_metadata(parent)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
            {
                return Err(format!(
                    "Symbolic links are not inspected in Health V1: {}",
                    path.display()
                ));
            }
        }
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !meta.is_file() && !meta.is_dir() {
            return Err("Only regular files and directories are supported.".into());
        }
        Ok(Self::from_metadata(path, &meta))
    }

    fn from_metadata(path: &Path, meta: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            path: path.to_path_buf(),
            directory: meta.is_dir(),
            bytes: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            device: meta.dev(),
            #[cfg(unix)]
            inode: meta.ino(),
            #[cfg(unix)]
            changed_seconds: meta.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: meta.ctime_nsec(),
        }
    }

    pub fn still_matches(&self) -> bool {
        Self::observe(&self.path).is_ok_and(|s| s == *self)
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct InspectionLimits {
    pub max_containers: usize,
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_logical_bytes: u64,
    pub max_member_bytes: u64,
}

impl Default for InspectionLimits {
    fn default() -> Self {
        Self {
            max_containers: 50_000,
            max_entries: 500_000,
            max_depth: 8,
            max_logical_bytes: MAX_ARCHIVE_RUN_LOGICAL_BYTES,
            max_member_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct InspectionProgress {
    pub current: PathBuf,
    pub containers: usize,
    pub entries: usize,
    pub logical_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CollectionInspection {
    pub inventory: MameCollectionInventory,
    pub sources: Vec<SourceStamp>,
    pub inspected_at_unix_seconds: u64,
    pub limits: InspectionLimits,
    pub logical_bytes: u64,
    pub cancelled: bool,
}

impl CollectionInspection {
    /// Revalidate metadata/identity, not content. No unchanged stamp is used
    /// as a substitute for the fresh content hashes collected by this pass.
    pub fn sources_unchanged(&self, cancel: &AtomicBool) -> bool {
        self.sources
            .iter()
            .all(|s| !cancel.load(Ordering::Relaxed) && s.still_matches())
    }

    pub fn invalidate(&mut self, reason: impl Into<String>) {
        self.inventory.inspected_completely = false;
        self.inventory.warnings.push(reason.into());
        for set in &mut self.inventory.sets {
            for member in &mut set.members {
                member.crc32 = None;
                member.md5 = None;
                member.sha1 = None;
            }
        }
    }
}

pub fn inspect_collection(
    root: &Path,
    limits: InspectionLimits,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(InspectionProgress),
) -> Result<CollectionInspection, String> {
    if crate::dat::rename_apply::is_private_recovery_root(root) {
        return Err("Private recovery data cannot be inspected as a collection.".into());
    }
    let stamp = SourceStamp::observe(root)?;
    if !stamp.directory {
        return Err("Choose a collection folder, not a file.".into());
    }
    let mut scan = Scan {
        result: CollectionInspection {
            inventory: MameCollectionInventory {
                collection_root: root.to_path_buf(),
                inspected_completely: true,
                ..Default::default()
            },
            sources: vec![stamp],
            inspected_at_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            limits,
            logical_bytes: 0,
            cancelled: false,
        },
        entries: 0,
        budget: ArchiveRunBudget::new(limits.max_logical_bytes),
        cancel,
        progress,
    };
    #[cfg(not(unix))]
    scan.refuse("Health V1 cannot establish stable filesystem identity on this platform.");
    let children = scan.children(root);
    for path in children {
        if scan.stopped() {
            break;
        }
        if scan.result.inventory.sets.len() >= limits.max_containers {
            scan.refuse("Container limit reached; inspection is incomplete.");
            break;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            scan.refuse("Non-UTF-8 container name was not inspected.");
            continue;
        };
        let name = name.to_string();
        match SourceStamp::observe(&path) {
            Ok(stamp) if stamp.directory => {
                scan.result.sources.push(stamp);
                let mut members = Vec::new();
                scan.directory(&path, &path, 0, &mut members);
                scan.result.inventory.sets.push(MameObservedSet {
                    name,
                    location: path.clone(),
                    directory: true,
                    members,
                });
            }
            Ok(stamp)
                if path
                    .extension()
                    .is_some_and(|s| s.eq_ignore_ascii_case("zip")) =>
            {
                scan.result.sources.push(stamp);
                let members = scan.zip(&path);
                scan.result.inventory.sets.push(MameObservedSet {
                    name: path.file_stem().unwrap().to_str().unwrap().into(),
                    location: path.clone(),
                    directory: false,
                    members,
                });
            }
            Ok(_) => scan.refuse(format!(
                "Unsupported file at collection root (use ZIP sets or machine folders): {}",
                path.display()
            )),
            Err(error) => scan.refuse(format!("{}: {error}", path.display())),
        }
        scan.notify(&path);
    }
    // MAME commonly stores ROMs in g.zip and CHDs in g/*.chd. Join only
    // this unambiguous presence-only companion; competing ROM containers stay
    // separate so the analyser refuses duplicate container authority.
    join_chd_companions(&mut scan.result.inventory.sets);
    scan.result.logical_bytes = scan.budget.consumed();
    scan.result.cancelled = cancel.load(Ordering::Relaxed);
    if scan.result.cancelled {
        scan.result.invalidate(
            "Inspection cancelled; results are partial and cannot establish complete sets.",
        );
    } else if !scan.result.sources_unchanged(cancel) {
        scan.result
            .invalidate("A source changed during inspection; recheck the collection.");
    }
    Ok(scan.result)
}

struct Scan<'a> {
    result: CollectionInspection,
    entries: usize,
    budget: ArchiveRunBudget,
    cancel: &'a AtomicBool,
    progress: &'a mut dyn FnMut(InspectionProgress),
}

impl Scan<'_> {
    fn refuse(&mut self, reason: impl Into<String>) {
        self.result.inventory.inspected_completely = false;
        if self.result.inventory.warnings.len() < 200 {
            self.result.inventory.warnings.push(reason.into());
        }
    }
    fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    fn notify(&mut self, path: &Path) {
        (self.progress)(InspectionProgress {
            current: path.into(),
            containers: self.result.inventory.sets.len(),
            entries: self.entries,
            logical_bytes: self.budget.consumed(),
        });
    }
    fn children(&mut self, path: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        match fs::read_dir(path) {
            Err(e) => self.refuse(format!("Cannot list {}: {e}", path.display())),
            Ok(dir) => {
                for entry in dir {
                    if self.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    if self.entries >= self.result.limits.max_entries {
                        self.refuse("Entry limit reached; inspection is incomplete.");
                        break;
                    }
                    self.entries += 1;
                    match entry {
                        Ok(entry) => {
                            if !crate::dat::rename_apply::is_private_recovery_path(&entry.path()) {
                                paths.push(entry.path());
                            }
                        }
                        Err(e) => self.refuse(format!("Cannot list an entry: {e}")),
                    }
                }
            }
        }
        paths.sort();
        paths
    }
    fn directory(
        &mut self,
        base: &Path,
        path: &Path,
        depth: usize,
        members: &mut Vec<MameObservedMember>,
    ) {
        if depth > self.result.limits.max_depth {
            self.refuse("Directory depth limit reached.");
            return;
        }
        for child in self.children(path) {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            match SourceStamp::observe(&child) {
                Ok(stamp) => {
                    self.result.sources.push(stamp.clone());
                    if stamp.directory {
                        self.directory(base, &child, depth + 1, members);
                        continue;
                    }
                    let Some(name) = child.strip_prefix(base).ok().and_then(|p| p.to_str()) else {
                        self.refuse("Non-UTF-8 member name was not inspected.");
                        continue;
                    };
                    let mut member = MameObservedMember {
                        name: name.replace(std::path::MAIN_SEPARATOR, "/"),
                        size_bytes: Some(stamp.bytes),
                        crc32: None,
                        md5: None,
                        sha1: None,
                    };
                    if child
                        .extension()
                        .is_some_and(|s| s.eq_ignore_ascii_case("chd"))
                    {
                        // Filename/size establish presence only. Never hash a CHD
                        // as an ordinary file and claim its combined digest.
                    } else if child.extension().is_some_and(|s| {
                        matches!(
                            s.to_str().map(str::to_ascii_lowercase).as_deref(),
                            Some("zip" | "7z" | "rar")
                        )
                    }) {
                        self.refuse(format!(
                            "Nested archives are unsupported: {}",
                            child.display()
                        ));
                    } else if stamp.bytes > self.result.limits.max_member_bytes
                        || !self.budget.try_charge(stamp.bytes)
                    {
                        self.refuse(format!("Hashing limit reached: {}", child.display()));
                    } else {
                        let result = open_bounded_read(&child, &TrustedRoots::none())
                            .map_err(|e| e.detail())
                            .and_then(|file| {
                                hash_member_stream(file.into_file(), stamp.bytes, self.cancel)
                                    .map_err(|e| format!("{e:?}"))
                            });
                        match result {
                            Ok(hash) if hash.bytes_read == stamp.bytes && stamp.still_matches() => {
                                member.crc32 = Some(hash.hashes.crc32);
                                member.md5 = Some(hash.hashes.md5);
                                member.sha1 = Some(hash.hashes.sha1);
                            }
                            Ok(_) => self
                                .refuse(format!("File changed while hashing: {}", child.display())),
                            Err(e) => {
                                self.refuse(format!("Cannot verify {}: {e}", child.display()))
                            }
                        }
                    }
                    members.push(member);
                    self.notify(&child);
                }
                Err(e) => self.refuse(format!("{}: {e}", child.display())),
            }
        }
    }
    fn zip(&mut self, path: &Path) -> Vec<MameObservedMember> {
        let limits = ArchiveLimits {
            max_members: 16_384,
            max_member_logical_bytes: self.result.limits.max_member_bytes,
            ..Default::default()
        };
        let mut source =
            match ZipArchiveSource::open(path, &TrustedRoots::none(), limits, self.cancel) {
                Ok(source) => source,
                Err(e) => {
                    self.refuse(format!("Cannot verify ZIP {}: {e:?}", path.display()));
                    return Vec::new();
                }
            };
        if source.member_count() > self.result.limits.max_entries.saturating_sub(self.entries) {
            self.refuse("Archive would exceed the inspection entry limit.");
            return Vec::new();
        }
        self.entries += source.member_count();
        let outcome = source.verify_all(self.cancel, &mut self.budget);
        if !outcome.is_complete() {
            self.refuse(format!(
                "Incomplete ZIP {}: {:?}",
                path.display(),
                outcome.completion
            ));
        }
        outcome
            .members
            .into_iter()
            .map(|m| {
                let name = String::from_utf8(m.member_name_raw.clone());
                let is_chd = name
                    .as_ref()
                    .is_ok_and(|n| n.to_ascii_lowercase().ends_with(".chd"));
                let hashes =
                    if name.is_ok() && !is_chd && m.status == ArchiveMemberStatus::HashComplete {
                        m.hashes
                    } else {
                        if !is_chd {
                            self.refuse(format!(
                                "Unverified member in {}: {} ({:?})",
                                path.display(),
                                m.member_name_display,
                                m.status
                            ));
                        }
                        None
                    };
                MameObservedMember {
                    name: name.unwrap_or(m.member_name_display),
                    size_bytes: Some(m.logical_size),
                    crc32: hashes.as_ref().map(|h| h.crc32.clone()),
                    md5: hashes.as_ref().map(|h| h.md5.clone()),
                    sha1: hashes.map(|h| h.sha1),
                }
            })
            .collect()
    }
}

fn join_chd_companions(sets: &mut Vec<MameObservedSet>) {
    let mut zip_targets = std::collections::BTreeMap::<String, Vec<usize>>::new();
    for (index, set) in sets.iter().enumerate().filter(|(_, s)| !s.directory) {
        zip_targets.entry(set.name.clone()).or_default().push(index);
    }
    let mut joined = Vec::new();
    for i in 0..sets.len() {
        if !sets[i].directory
            || sets[i].members.is_empty()
            || !sets[i]
                .members
                .iter()
                .all(|m| m.name.to_ascii_lowercase().ends_with(".chd"))
        {
            continue;
        }
        if let Some(candidates) = zip_targets.get(&sets[i].name)
            && let [target] = candidates.as_slice()
        {
            let members = sets[i].members.clone();
            sets[*target].members.extend(members);
            joined.push(i);
        }
    }
    for i in joined.into_iter().rev() {
        sets.remove(i);
    }
}

#[cfg(test)]
mod tests;
