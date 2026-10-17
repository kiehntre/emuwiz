//! A sealed allowlist from the existing preview. This module never walks a root.
use super::preview::MediaPreview;
use crate::catalogue_health::{BoundRoot, SourceRootBinding};
use crate::identity_source::hashing::{FileFingerprint, LocalHashes, hash_file};
use crate::safe_read::TrustedRoots;
use crate::{Database, PersistedArchive, SourceFolderView};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

pub const REVIEW_LIFETIME: Duration = Duration::from_secs(30 * 60);
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Candidate {
    pub(crate) hashes: LocalHashes,
    pub(crate) platform: String,
    pub(crate) kind: String,
    pub(crate) companions: Vec<LocalHashes>,
    pub(crate) alternate: Option<String>,
    pub(crate) multipart: Option<String>,
    pub(crate) ambiguous: bool,
    pub(crate) relationships_complete: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct ImportEvidence {
    pub(crate) observed_at: Instant,
    pub(crate) root: PathBuf,
    pub(crate) binding: SourceRootBinding,
    pub(crate) known: Vec<PersistedArchive>,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) references: Vec<(PathBuf, FileFingerprint)>,
    pub(crate) exclusions: Vec<(PathBuf, String)>,
    pub(crate) complete: bool,
    pub(crate) catalogue_basis: Vec<String>,
}
/// Private fields ensure presentation edits cannot expand a reviewed allowlist.
#[derive(Clone, Debug)]
pub struct BoundedImportPlan {
    pub(crate) approval_sha256: String,
    pub(crate) exclusions_sha256: String,
    pub(crate) evidence: ImportEvidence,
    pub(crate) database: PathBuf,
    pub(crate) database_object: (u64, u64),
    pub(crate) config: PathBuf,
    pub(crate) config_hash: LocalHashes,
    pub(crate) sources: Vec<SourceFolderView>,
    pub(crate) aliases: Vec<crate::PlatformAlias>,
    pub(crate) prepared: Instant,
    pub(crate) existing: Vec<(PathBuf, String)>,
    pub(crate) source_bindings: Vec<(i64, Option<(String, i64)>)>,
}
#[derive(Clone, Debug)]
pub struct BoundedPreviewContext {
    pub(crate) database: PathBuf,
    pub(crate) database_object: (u64, u64),
    pub(crate) config: PathBuf,
    pub(crate) config_hash: LocalHashes,
    pub(crate) sources: Vec<SourceFolderView>,
    pub(crate) aliases: Vec<crate::PlatformAlias>,
    pub(crate) source_bindings: Vec<(i64, Option<(String, i64)>)>,
}
/// Capture the saved scope before starting the read-only preview worker.
pub fn begin_bounded_preview(
    db: &Database,
    config: &Path,
) -> Result<BoundedPreviewContext, String> {
    let sources = crate::build_source_folder_views(
        &crate::load_source_folder_configs_from(config).map_err(|e| e.to_string())?,
        &db.list_source_folders().map_err(|e| e.to_string())?,
    );
    Ok(BoundedPreviewContext {
        database: db.path().into(),
        database_object: object(db.path())?,
        config: config.into(),
        config_hash: config_hash(config)?,
        aliases: db.list_platform_aliases().map_err(|e| e.to_string())?,
        source_bindings: db.bounded_source_bindings()?,
        sources,
    })
}
#[derive(Clone, Debug, Default)]
pub struct BoundedImportResult {
    /// Durable SQLite operation identity, returned only after the batch commits.
    pub operation_id: Option<String>,
    /// Exact candidate refusals during Apply, distinct from context exclusions.
    pub refused: Vec<(PathBuf, String)>,
    pub inserted: Vec<(PathBuf, i64)>,
    pub skipped: Vec<(PathBuf, String)>,
}
impl BoundedImportResult {
    pub(crate) fn refuse(&mut self, item: (PathBuf, String)) {
        self.refused.push(item.clone());
        self.skipped.push(item);
    }
}
impl BoundedImportPlan {
    pub fn approval_sha256(&self) -> &str {
        &self.approval_sha256
    }
    pub fn exclusions_sha256(&self) -> &str {
        &self.exclusions_sha256
    }
    /// Approval identity: sorted exact Unix paths and full content checksums.
    /// Each path is prefixed by its u64 LE byte length, then size u64 LE and
    /// lowercase CRC32/MD5/SHA1 ASCII (8/32/40 bytes). Domain prefix is versioned.
    pub(crate) fn calculate_approval_sha256(&self) -> String {
        use sha2::{Digest, Sha256};
        use std::os::unix::ffi::OsStrExt;
        let mut hash = Sha256::new();
        hash.update(b"emuwiz-bounded-discovery-allowlist-v2\0");
        let mut candidates: Vec<_> = self
            .evidence
            .candidates
            .iter()
            .filter(|c| !c.ambiguous && c.relationships_complete)
            .collect();
        candidates.sort_by(|a, b| a.hashes.fingerprint.path.cmp(&b.hashes.fingerprint.path));
        for c in candidates {
            hash.update(stable_candidate_id(c).as_bytes());
            let path = c.hashes.fingerprint.path.as_os_str().as_bytes();
            hash.update((path.len() as u64).to_le_bytes());
            hash.update(path);
            hash.update(c.hashes.fingerprint.size_bytes.to_le_bytes());
            hash.update(c.hashes.crc32.to_ascii_lowercase().as_bytes());
            hash.update(c.hashes.md5.to_ascii_lowercase().as_bytes());
            hash.update(c.hashes.sha1.to_ascii_lowercase().as_bytes());
        }
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    /// Sorted excluded path membership, including ambiguous/incomplete candidates.
    pub(crate) fn calculate_exclusions_sha256(&self) -> String {
        use sha2::{Digest, Sha256};
        use std::os::unix::ffi::OsStrExt;
        let mut hash = Sha256::new();
        hash.update(b"emuwiz-bounded-spectrum-exclusions-v1\0");
        let paths: std::collections::BTreeSet<_> = self
            .evidence
            .exclusions
            .iter()
            .map(|(p, _)| p)
            .chain(
                self.evidence
                    .candidates
                    .iter()
                    .filter(|c| c.ambiguous || !c.relationships_complete)
                    .map(|c| &c.hashes.fingerprint.path),
            )
            .collect();
        for p in paths {
            let bytes = p.as_os_str().as_bytes();
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    pub fn root(&self) -> &Path {
        &self.evidence.root
    }
    pub fn database(&self) -> &Path {
        &self.database
    }
    pub fn candidate_count(&self) -> usize {
        self.evidence.candidates.len()
    }
    pub fn exclusions(&self) -> &[(PathBuf, String)] {
        &self.evidence.exclusions
    }
    pub fn projected_insertions(&self) -> usize {
        self.evidence
            .candidates
            .iter()
            .filter(|c| !c.ambiguous && c.relationships_complete)
            .count()
    }
    pub(crate) fn current_filesystem(&self) -> Result<(), String> {
        if self.prepared.elapsed() > REVIEW_LIFETIME {
            return Err("Preview expired after 30 minutes. Prepare and confirm again.".into());
        }
        if BoundRoot::open(self.root())
            .is_none_or(|b| b.binding != self.evidence.binding || !b.current())
        {
            return Err("Selected root or mount changed. Prepare again.".into());
        }
        if object(&self.database)? != self.database_object {
            return Err("Library database identity changed.".into());
        }
        if config_hash(&self.config)? != self.config_hash {
            return Err("Source configuration changed after preview.".into());
        }
        for (p, g) in &self.evidence.references {
            if FileFingerprint::observe(p).as_ref() != Some(g) {
                return Err(format!(
                    "Comparison reference changed or unavailable: {}. Prepare again.",
                    p.display()
                ));
            }
        }
        Ok(())
    }
}
pub(crate) fn object(path: &Path) -> Result<(u64, u64), String> {
    let g = FileFingerprint::observe(path)
        .ok_or_else(|| format!("Missing or unsafe database: {}", path.display()))?;
    let o = g
        .object
        .ok_or("Filesystem cannot establish database identity")?;
    Ok((o.device, o.inode))
}
fn config_hash(path: &Path) -> Result<LocalHashes, String> {
    let g = FileFingerprint::observe(path).ok_or("Missing or unsafe source configuration")?;
    if g.size_bytes > 4 * 1024 * 1024 {
        return Err("Source configuration exceeds bounded read limit.".into());
    }
    hash_file(path, &TrustedRoots::none(), None)
        .map_err(|e| format!("Cannot validate source configuration: {e:?}"))
}
/// Bind an unmodified native preview to one existing database and saved source
/// configuration. No caller-supplied path list is accepted.
pub fn prepare_bounded_import(
    preview: &MediaPreview,
    context: BoundedPreviewContext,
) -> Result<BoundedImportPlan, String> {
    let evidence = preview
        .import_evidence
        .clone()
        .ok_or("No sealed native preview evidence. Prepare again.")?;
    if !evidence.complete {
        return Err("Incomplete or cancelled discovery cannot be applied. Prepare again.".into());
    }
    let db = Database::open_read_only(&context.database).map_err(|e| e.to_string())?;
    let prepared = evidence.observed_at;
    // Only sealed reference membership and exact existing DB identities may
    // establish a retained comparison record; mutable presentation is ignored.
    let existing: Vec<_> = evidence
        .references
        .iter()
        .map(|(p, _)| p)
        .filter(|p| evidence.known.iter().any(|a| a.absolute_path == **p))
        .filter(|p| {
            !evidence
                .exclusions
                .iter()
                .any(|(excluded, _)| excluded == *p)
        })
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|p| {
            (
                p,
                "Pre-existing comparison record retained; never imported or modified.".into(),
            )
        })
        .collect();
    let mut plan = BoundedImportPlan {
        approval_sha256: String::new(),
        exclusions_sha256: String::new(),
        evidence,
        database: context.database,
        database_object: context.database_object,
        config: context.config,
        config_hash: context.config_hash,
        sources: context.sources,
        aliases: context.aliases,
        source_bindings: context.source_bindings,
        prepared,
        existing,
    };
    plan.approval_sha256 = plan.calculate_approval_sha256();
    plan.exclusions_sha256 = plan.calculate_exclusions_sha256();
    plan.current_filesystem()?;
    db.validate_bounded_import(&plan)?;
    Ok(plan)
}
/// Explicit confirmation is supplied by the GUI journey, never by a root scan.
/// Errors and cancellation roll back every insertion; skip reasons are returned
/// only with a committed result. No existing record or DAT history is updated.
pub fn apply_bounded_import(
    plan: &BoundedImportPlan,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<BoundedImportResult, String> {
    Database::apply_bounded_import(plan, cancel, progress)
}

/// Stable identity includes raw path, full checksums, platform, kind and companions.
pub(crate) fn stable_candidate_id(c: &Candidate) -> String {
    use sha2::{Digest, Sha256};
    use std::os::unix::ffi::OsStrExt;
    let mut h = Sha256::new();
    h.update(b"emuwiz-discovery-candidate-v2\0");
    for bytes in [c.platform.as_bytes(), c.kind.as_bytes()] {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    for proof in std::iter::once(&c.hashes).chain(c.companions.iter()) {
        let path = proof.fingerprint.path.as_os_str().as_bytes();
        h.update((path.len() as u64).to_le_bytes());
        h.update(path);
        h.update(proof.fingerprint.size_bytes.to_le_bytes());
        for hash in [&proof.crc32, &proof.md5, &proof.sha1] {
            h.update((hash.len() as u64).to_le_bytes());
            h.update(hash.as_bytes());
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
pub(crate) fn companions(path: &Path) -> Result<Vec<PathBuf>, String> {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "cue" => super::cue_bin::resolve_cue_all_files(path)
            .map_err(|e| format!("CUE companions refused: {e:?}")),
        "gdi" => super::gdi::resolve_gdi_all_tracks(path)
            .map_err(|e| format!("GDI companions refused: {e:?}")),
        "m3u" | "m3u8" => Err("Playlist/directory import remains a separate workflow.".into()),
        _ => Ok(Vec::new()),
    }
}

pub(crate) fn validate_companions(
    c: &Candidate,
    root: &Path,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let paths = companions(&c.hashes.fingerprint.path)?;
    if paths
        != c.companions
            .iter()
            .map(|h| h.fingerprint.path.clone())
            .collect::<Vec<_>>()
    {
        return Err("Companion closure differs from reviewed evidence.".into());
    }
    for h in &c.companions {
        if !h.fingerprint.path.starts_with(root) {
            return Err("Companion escapes reviewed root.".into());
        }
        let fresh = hash_file(&h.fingerprint.path, trusted, Some(cancel))
            .map_err(|e| format!("Companion unavailable: {e:?}"))?;
        if fresh != *h {
            return Err("Companion changed since review.".into());
        }
    }
    Ok(())
}
