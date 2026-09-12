//! GUI review drafts, not installation approvals. Only a report digest and
//! explicit choices are stored: no report, source path, code, URL or token.
//! Like other GUI sidecars, uses app_dirs' existing data-directory resolution.
//! Reopening a byte-identical report is the only restoration entry point.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use archivefs_core::patch_manager::{CheatReconciliationOutcome, reconcile_cheats_for_game};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{CheatReconciliationResult, ReviewChoice, choice_allowed};

const VERSION: u32 = 1;
const MAX_REPORT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STATE_BYTES: u64 = 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedReview {
    version: u32,
    report_sha256: String,
    choices: BTreeMap<usize, ReviewChoice>,
}

pub(super) fn read_report(path: &Path) -> Result<(CheatReconciliationResult, String), String> {
    let bytes = read_bounded(path, MAX_REPORT_BYTES)
        .map_err(|error| format!("Could not read the report: {error}"))?;
    let report: CheatReconciliationResult = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid reconciliation report: {error}"))?;
    // Reuse the current core contract instead of trusting JSON group indices
    // or resurrecting decisions for a report from an incompatible core.
    let empty =
        report.entries.is_empty() && report.groups.is_empty() && report.auto_winner.is_none();
    let compatible = empty
        || match reconcile_cheats_for_game(report.entries.clone()) {
            CheatReconciliationOutcome::Ready(current) => {
                // The existing CLI can export a relationship-filtered subset.
                // Keep that contract: each supplied group must still be an exact,
                // ordered member of the current core output, without duplicates.
                let mut groups = current.groups.iter();
                current.game_identity == report.game_identity
                    && current.platform == report.platform
                    && report.auto_winner.is_none()
                    && report
                        .groups
                        .iter()
                        .all(|group| groups.any(|candidate| candidate == group))
            }
            CheatReconciliationOutcome::Unavailable { .. } => false,
        };
    if !compatible {
        return Err("The report does not match the current reconciliation result. Generate it again before reviewing.".into());
    }
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((report, digest))
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Expected a regular file, not a link or directory.".into());
    }
    if metadata.len() > limit {
        return Err("The file exceeds the review size limit.".into());
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("The file changed while opening it.".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("The file exceeds the review size limit.".into());
    }
    Ok(bytes)
}

pub(super) struct ReviewStore {
    root: PathBuf,
}

impl ReviewStore {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path(&self, digest: &str) -> Result<PathBuf, String> {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Invalid review report identity.".into());
        }
        Ok(self.root.join(format!("{digest}.json")))
    }

    fn check_root(&self) -> Result<bool, String> {
        let metadata = match fs::symlink_metadata(&self.root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("The review storage folder must be a directory, not a link.".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err("The review storage folder must be private (permissions 0700).".into());
            }
        }
        Ok(true)
    }

    pub(super) fn load(
        &self,
        digest: &str,
        report: &CheatReconciliationResult,
    ) -> Result<BTreeMap<usize, ReviewChoice>, String> {
        let path = self.path(digest)?;
        if !self.check_root()? {
            return Ok(BTreeMap::new());
        }
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(error) => return Err(error.to_string()),
            Ok(_) => {}
        }
        let bytes = read_bounded(&path, MAX_STATE_BYTES)?;
        let saved: SavedReview = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Saved choices are damaged; the file was left unchanged: {e}"))?;
        if saved.version != VERSION {
            return Err(format!(
                "Saved choices use unsupported version {}. The file was left unchanged.",
                saved.version
            ));
        }
        if saved.report_sha256 != digest {
            return Err("Saved choices belong to a different report; nothing was restored.".into());
        }
        validate_choices(report, &saved.choices)?;
        Ok(saved.choices)
    }

    // The directory inode is stable across JSON replacement. Kernel locking
    // releases on process death, so there is no PID lock to delete/reclaim.
    // Unsupported locking fails visibly; it never falls back to unlocked writes.
    fn lock(&self) -> Result<File, String> {
        if !self.check_root()? {
            let parent = self
                .root
                .parent()
                .ok_or("Review storage has no parent folder.")?;
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&self.root) {
                Ok(()) => {
                    if let Ok(directory) = File::open(parent) {
                        let _ = directory.sync_all();
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        self.check_root()?;
        let lock = File::open(&self.root).map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|e| format!("Review storage is busy or cannot be locked. Try again: {e}"))?;
        Ok(lock)
    }

    pub(super) fn save(
        &self,
        digest: &str,
        report: &CheatReconciliationResult,
        expected: &BTreeMap<usize, ReviewChoice>,
        choices: &BTreeMap<usize, ReviewChoice>,
    ) -> Result<(), String> {
        let path = self.path(digest)?;
        validate_choices(report, choices)?;
        let payload = serde_json::to_vec_pretty(&SavedReview {
            version: VERSION,
            report_sha256: digest.into(),
            choices: choices.clone(),
        })
        .map_err(|e| e.to_string())?;
        if payload.len() as u64 > MAX_STATE_BYTES {
            return Err("Too many review choices to save safely.".into());
        }
        let _lock = self.lock()?;
        let current = self.load(digest, report)?;
        if &current != expected {
            return Err("Saved choices changed in another window or were removed. Reopen the report to review the current choices; nothing was overwritten.".into());
        }
        self.publish(&path, &payload)
    }

    fn publish(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let temp = self.root.join(format!(
            ".review-{}-{}.tmp",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // create_new cannot truncate an existing file or follow a planted link.
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        let result = (|| -> std::io::Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, path)?;
            // Mirrors existing sidecars: directory fsync where supported.
            if let Ok(parent) = File::open(&self.root) {
                let _ = parent.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result.map_err(|e| format!("Could not save choices; retry before closing: {e}"))
    }
}

fn validate_choices(
    report: &CheatReconciliationResult,
    choices: &BTreeMap<usize, ReviewChoice>,
) -> Result<(), String> {
    if choices
        .iter()
        .any(|(group, choice)| !choice_allowed(report, *group, *choice))
    {
        return Err(
            "Saved choices do not match reviewable groups in this report; nothing was restored."
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
