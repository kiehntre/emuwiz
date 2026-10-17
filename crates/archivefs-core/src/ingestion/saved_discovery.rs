//! Persisted Discovery evidence is read-only input, never a deserialized Apply
//! capability. A pinned file digest detects edits; selected files, source
//! authority and native archive/companion evidence are validated before resealing.
use super::bounded_import::*;
use super::preview::MediaPreview;
use crate::identity_source::hashing::{FileFingerprint, LocalHashes, hash_file};
use crate::safe_read::TrustedRoots;
use crate::{Archive, ArchiveKind, Database};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
pub const MAX_SAVED_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_SAVED_CANDIDATES: usize = 100_000;
pub const MAX_SELECTED: usize = 1000;
#[derive(Clone, Debug)]
pub struct SelectionRow {
    pub id: String,
    pub path: PathBuf,
    pub platform: String,
    pub kind: String,
    pub eligible: bool,
    pub alternate: Option<String>,
    pub multipart: Option<String>,
    pub companion_count: usize,
}
#[derive(Clone, Debug)]
pub struct SavedDiscovery {
    data: SavedData,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    payload: String,
    payload_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedData {
    root: PathBuf,
    catalogue_basis: Vec<String>,
    binding: crate::catalogue_health::SourceRootBinding,
    database: PathBuf,
    database_object: (u64, u64),
    config: PathBuf,
    config_hash: LocalHashes,
    sources_sha256: String,
    aliases_sha256: String,
    bindings: Vec<(i64, Option<(String, i64)>)>,
    known_ids: Vec<i64>,
    known_sha256: String,
    candidates: Vec<Candidate>,
    references: Vec<(PathBuf, FileFingerprint)>,
    exclusions: Vec<(PathBuf, String)>,
}
#[derive(Clone, Debug)]
pub struct SubsetPreview {
    pub plan: BoundedImportPlan,
    pub saved_eligible: usize,
    pub requested: usize,
    pub rejected: Vec<(PathBuf, String)>,
    pub unselected: usize,
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn serialized_digest<T: Serialize>(value: &T) -> Result<String, String> {
    Ok(digest(&serde_json::to_vec(value).map_err(|e| {
        format!("Evidence is not safely serializable: {e}")
    })?))
}
fn rows(candidates: &[Candidate]) -> Vec<SelectionRow> {
    candidates
        .iter()
        .map(|c| SelectionRow {
            id: stable_candidate_id(c),
            path: c.hashes.fingerprint.path.clone(),
            platform: c.platform.clone(),
            kind: c.kind.clone(),
            eligible: !c.ambiguous && c.relationships_complete,
            alternate: c.alternate.clone(),
            multipart: c.multipart.clone(),
            companion_count: c.companions.len(),
        })
        .collect()
}
impl BoundedImportPlan {
    pub fn selection_rows(&self) -> Vec<SelectionRow> {
        rows(&self.evidence.candidates)
    }
    pub fn catalogue_basis(&self) -> &[String] {
        &self.evidence.catalogue_basis
    }
    pub fn platform_counts(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for c in &self.evidence.candidates {
            *m.entry(c.platform.clone()).or_default() += 1;
        }
        m
    }
    /// Retain native reviewed evidence for exact in-memory subset selection.
    pub fn saved_evidence(&self) -> Result<SavedDiscovery, String> {
        Ok(SavedDiscovery {
            data: SavedData {
                root: self.root().into(),
                catalogue_basis: self.evidence.catalogue_basis.clone(),
                binding: self.evidence.binding.clone(),
                database: self.database.clone(),
                database_object: self.database_object,
                config: self.config.clone(),
                config_hash: self.config_hash.clone(),
                sources_sha256: serialized_digest(&self.sources)?,
                aliases_sha256: serialized_digest(&self.aliases)?,
                bindings: self.source_bindings.clone(),
                known_ids: self.evidence.known.iter().map(|a| a.id).collect(),
                known_sha256: serialized_digest(&self.evidence.known)?,
                candidates: self.evidence.candidates.clone(),
                references: self.evidence.references.clone(),
                exclusions: self.evidence.exclusions.clone(),
            },
        })
    }
    /// Create-new only, outside collection roots; never overwrite evidence or ROMs.
    /// The returned whole-file SHA256 must be retained separately for reload.
    pub fn save_evidence(&self, path: &Path) -> Result<String, String> {
        self.current_filesystem()?;
        if !path.is_absolute() || self.sources.iter().any(|s| path.starts_with(&s.path)) {
            return Err("Save evidence outside the collection, using an absolute path.".into());
        }
        let parent = path.parent().ok_or("Evidence parent missing")?;
        if std::fs::canonicalize(parent).map_err(|e| e.to_string())? != parent {
            return Err("Evidence parent must be canonical.".into());
        }
        let data = self.saved_evidence()?.data;
        let payload = serde_json::to_string(&data).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec(&Envelope {
            version: 1,
            payload_sha256: digest(payload.as_bytes()),
            payload,
        })
        .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_SAVED_BYTES {
            return Err("Evidence exceeds bounded storage limit.".into());
        }
        let parent = crate::catalogue_health::BoundRoot::open(parent)
            .ok_or("Evidence parent is unsafe or unavailable")?;
        let mut file = parent
            .create_new_child(path.file_name().ok_or("Evidence filename missing")?)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        if !parent.current() {
            return Err("Evidence parent changed during save; no reload pin issued.".into());
        }
        Ok(digest(&bytes))
    }
}
pub fn load_saved_evidence(path: &Path, expected_sha256: &str) -> Result<SavedDiscovery, String> {
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Supply the separately retained SHA-256 from Save evidence.".into());
    }
    let file = crate::safe_read::open_bounded_read(path, &TrustedRoots::none())
        .map_err(|e| format!("Cannot safely open evidence: {e:?}"))?;
    let mut bytes = Vec::new();
    file.into_file()
        .take(MAX_SAVED_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_SAVED_BYTES {
        return Err("Evidence exceeds read limit.".into());
    }
    if !digest(&bytes).eq_ignore_ascii_case(expected_sha256) {
        return Err("Saved evidence does not match pinned SHA-256.".into());
    }
    let e: Envelope = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if e.version != 1 || digest(e.payload.as_bytes()) != e.payload_sha256 {
        return Err("Unknown or corrupt saved evidence.".into());
    }
    let data: SavedData = serde_json::from_str(&e.payload).map_err(|e| e.to_string())?;
    if data.candidates.len() > MAX_SAVED_CANDIDATES
        || data.references.len() > 200_000
        || data.exclusions.len() > 200_000
    {
        return Err("Saved evidence item limit exceeded.".into());
    }
    let safe_absolute = |p: &Path| {
        p.is_absolute()
            && !p
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    };
    if !safe_absolute(&data.root)
        || !safe_absolute(&data.database)
        || !safe_absolute(&data.config)
        || data.known_ids.len() > 200_000
        || data.bindings.len() > 10_000
        || data.catalogue_basis.len() > 10_000
        || data.known_ids.iter().collect::<BTreeSet<_>>().len() != data.known_ids.len()
        || data
            .exclusions
            .iter()
            .map(|(p, _)| p)
            .collect::<BTreeSet<_>>()
            .len()
            != data.exclusions.len()
        || data
            .exclusions
            .iter()
            .any(|(p, reason)| !safe_absolute(p) || !p.starts_with(&data.root) || reason.is_empty())
        || data
            .references
            .iter()
            .any(|(p, g)| !safe_absolute(p) || *p != g.path)
    {
        return Err("Invalid saved scope, references, identities or exclusions.".into());
    }
    let mut paths = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let excluded: BTreeSet<_> = data.exclusions.iter().map(|(p, _)| p).collect();
    for c in &data.candidates {
        let p = &c.hashes.fingerprint.path;
        if !p.starts_with(&data.root)
            || p.components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
            || !paths.insert(p)
            || !ids.insert(stable_candidate_id(c))
            || excluded.contains(p)
        {
            return Err("Duplicate, excluded or escaping candidate in saved evidence.".into());
        }
        if crate::canonical_platform_for_alias(&c.platform) != Some(c.platform.as_str())
            || ArchiveKind::from_storage(&c.kind).is_none()
            || c.companions.len() > 128
        {
            return Err("Unsupported candidate platform/kind/companion count.".into());
        }
        for h in std::iter::once(&c.hashes).chain(c.companions.iter()) {
            if !h.fingerprint.path.starts_with(&data.root)
                || h.fingerprint
                    .path
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
                || h.bytes_hashed != h.fingerprint.size_bytes
                || [(8, &h.crc32), (32, &h.md5), (40, &h.sha1)]
                    .iter()
                    .any(|(n, v)| v.len() != *n || !v.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("Invalid content proof or escaping companion.".into());
            }
        }
    }
    Ok(SavedDiscovery { data })
}
impl SavedDiscovery {
    pub fn rows(&self) -> Vec<SelectionRow> {
        rows(&self.data.candidates)
    }
    pub fn root(&self) -> &Path {
        &self.data.root
    }
    pub fn excluded_count(&self) -> usize {
        self.data.exclusions.len()
    }
    /// Only explicit saved identities are inspected. No scanner or directory walk.
    pub fn prepare_subset(
        &self,
        ids: &[String],
        database: &Path,
        config: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<SubsetPreview, String> {
        if ids.is_empty() || ids.len() > MAX_SELECTED {
            return Err("Select between 1 and 1000 stable candidate identities.".into());
        }
        let unique: BTreeSet<_> = ids.iter().collect();
        if unique.len() != ids.len() {
            return Err("Repeated selection identities are not accepted.".into());
        }
        if database != self.data.database
            || config != self.data.config
            || object(database)? != self.data.database_object
        {
            return Err("Saved evidence targets a different database/configuration.".into());
        }
        let db = Database::open_read_only(database).map_err(|e| e.to_string())?;
        let context = begin_bounded_preview(&db, config)?;
        if serialized_digest(&context.sources)? != self.data.sources_sha256
            || serialized_digest(&context.aliases)? != self.data.aliases_sha256
            || context.source_bindings != self.data.bindings
            || context.config_hash != self.data.config_hash
        {
            return Err("Saved source authority changed; no subset sealed.".into());
        }
        let known: Vec<_> = db
            .load_archives()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|a| a.absolute_path.starts_with(&self.data.root))
            .collect();
        let old_ids: BTreeSet<_> = self.data.known_ids.iter().copied().collect();
        let old: Vec<_> = known
            .iter()
            .filter(|a| old_ids.contains(&a.id))
            .cloned()
            .collect();
        if serialized_digest(&old)? != self.data.known_sha256 {
            return Err(
                "Previously represented identities changed; saved evidence is stale.".into(),
            );
        }
        let map: BTreeMap<_, _> = self
            .data
            .candidates
            .iter()
            .map(|c| (stable_candidate_id(c), c))
            .collect();
        if ids.iter().any(|id| !map.contains_key(id)) {
            return Err("Selection contains an identity outside the saved candidate set.".into());
        }
        let selected_platforms: BTreeSet<_> =
            ids.iter().map(|id| map[id].platform.clone()).collect();
        let mut archive_budget =
            crate::dat::archive::ArchiveRunBudget::new(super::preview::MAX_PREVIEW_DECODE_BYTES);
        let mut bytes = 0u64;
        let (members, new_references) = super::saved_archive_checks::references(
            &known,
            &selected_platforms,
            &self.data.root,
            cancel,
            &mut archive_budget,
            &mut bytes,
        )?;
        let mut rejected = Vec::new();
        let mut candidates = Vec::new();
        let mut exclusions = self.data.exclusions.clone();
        for c in &self.data.candidates {
            if !unique.contains(&stable_candidate_id(c)) {
                exclusions.push((
                    c.hashes.fingerprint.path.clone(),
                    "Not selected in this exact bounded batch.".into(),
                ));
            }
        }
        let trusted = TrustedRoots::from_paths([&self.data.root]);
        let source = context
            .sources
            .iter()
            .filter(|s| self.data.root.starts_with(&s.path))
            .max_by_key(|s| s.path.components().count())
            .ok_or("No configured source authority")?;
        for (n, id) in ids.iter().enumerate() {
            progress(n, ids.len());
            if cancel.load(Ordering::Relaxed) {
                return Err("Subset preparation cancelled; no database changes.".into());
            }
            let c = *map.get(id).unwrap();
            let path = &c.hashes.fingerprint.path;
            let checked = (|| {
                if c.ambiguous || !c.relationships_complete {
                    return Err(
                        "Saved candidate was ambiguous or incomplete; not eligible.".to_string()
                    );
                }
                if known.iter().any(|a| a.absolute_path == *path) {
                    return Err("Already represented by a protected library record.".into());
                }
                bytes = bytes
                    .checked_add(c.hashes.bytes_hashed)
                    .ok_or("Read budget overflow")?;
                for h in &c.companions {
                    bytes = bytes
                        .checked_add(h.bytes_hashed)
                        .ok_or("Read budget overflow")?;
                }
                if bytes > super::preview::MAX_COMPARISON_INPUT_BYTES {
                    return Err("Selected file read budget exceeded.".into());
                }
                if hash_file(path, &trusted, Some(cancel))
                    .map_err(|e| format!("Changed/missing/protected candidate: {e:?}"))?
                    != c.hashes
                {
                    return Err("Candidate content or filesystem identity changed.".into());
                }
                let a = Archive::from_path_in_root(path, &source.path)
                    .ok_or("Existing parser refused candidate")?;
                if a.kind.storage_name() != c.kind
                    || a.kind == ArchiveKind::ArcadeSetDirectory
                    || a.identity
                        .platform
                        .as_deref()
                        .and_then(crate::canonical_platform_for_alias)
                        != Some(c.platform.as_str())
                {
                    return Err("Platform/kind conflict with native recognition.".into());
                }
                validate_companions(c, &self.data.root, &trusted, cancel)?;
                if matches!(
                    a.kind,
                    ArchiveKind::Zip | ArchiveKind::SevenZip | ArchiveKind::Rar | ArchiveKind::Lha
                ) {
                    bytes = bytes
                        .checked_add(c.hashes.bytes_hashed)
                        .ok_or("Archive read budget overflow")?;
                    if bytes > super::preview::MAX_COMPARISON_INPUT_BYTES {
                        return Err("Selected archive read budget exceeded.".into());
                    }
                    super::saved_archive_checks::read_members(
                        path,
                        &c.platform,
                        &trusted,
                        cancel,
                        &mut archive_budget,
                    )?;
                }
                if let Some(proofs) = members.get(&(
                    c.platform.clone(),
                    c.hashes.bytes_hashed,
                    c.hashes.sha1.clone(),
                )) {
                    bytes = bytes
                        .checked_add(c.hashes.bytes_hashed)
                        .ok_or("Read budget overflow")?;
                    if bytes > super::preview::MAX_COMPARISON_INPUT_BYTES {
                        return Err("Archive comparison read budget exceeded.".into());
                    }
                    let full = crate::repair::exact_duplicate::hash_full_file_sha256(
                        path,
                        &trusted,
                        Some(cancel),
                    )
                    .map_err(|e| format!("Archive comparison unavailable: {e:?}"))?;
                    if proofs.contains(&full.sha256) {
                        return Err("Exact archive-member copy is already represented; excluded from subset.".into());
                    }
                }
                Ok(())
            })();
            match checked {
                Ok(()) => candidates.push(c.clone()),
                Err(reason) => {
                    rejected.push((path.clone(), reason.clone()));
                    exclusions.push((path.clone(), reason));
                }
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Subset preparation cancelled.".into());
        }
        let mut references = self.data.references.clone();
        references.extend(new_references);
        let mut preview = MediaPreview::default();
        preview.import_evidence = Some(ImportEvidence {
            observed_at: Instant::now(),
            catalogue_basis: self.data.catalogue_basis.clone(),
            root: self.data.root.clone(),
            binding: self.data.binding.clone(),
            known,
            candidates,
            references,
            exclusions,
            complete: true,
        });
        progress(ids.len(), ids.len());
        if cancel.load(Ordering::Relaxed) {
            return Err("Subset preparation cancelled; no database changes.".into());
        }
        for c in &preview.import_evidence.as_ref().unwrap().candidates {
            for h in std::iter::once(&c.hashes).chain(c.companions.iter()) {
                if FileFingerprint::observe(&h.fingerprint.path).as_ref() != Some(&h.fingerprint) {
                    return Err("Selected media changed before subset seal.".into());
                }
            }
        }
        let plan = prepare_bounded_import(&preview, context)?;
        Ok(SubsetPreview {
            saved_eligible: self
                .data
                .candidates
                .iter()
                .filter(|c| !c.ambiguous && c.relationships_complete)
                .count(),
            requested: ids.len(),
            unselected: self.data.candidates.len() - ids.len(),
            plan,
            rejected,
        })
    }
}
