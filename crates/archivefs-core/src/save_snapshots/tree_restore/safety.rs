//! Reviewed, identity-bound Replace restore. All data access is bounded and read-only
//! until apply. Providers are trusted backend services, never UI checkbox values.
//! No process detector currently proves system-wide closure; the default refuses.
use super::*;
use crate::patch_manager::{EmulatorProfileConfidence, ResolvedEmulatorProfile};
use crate::save_snapshots::SaveSnapshotManifest;

pub const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const SPACE_MARGIN: u64 = 8 * 1024 * 1024;

/// A resolved stable identity supplied by the identity/resolver layer, never a filename.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedSaveGame {
    Unique(String),
    Ambiguous,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectorySaveBinding {
    pub game: ResolvedSaveGame,
    pub emulator: String,
    pub profile: String,
    pub artifact_type: SaveArtifactType,
}
impl DirectorySaveBinding {
    /// Consumes an already-resolved profile. This does not discover a profile or
    /// assign game identity. Snapshot creation must use the same stable profile key.
    pub fn from_resolved_profile(
        game: ResolvedSaveGame,
        emulator: String,
        profile: &ResolvedEmulatorProfile,
    ) -> Result<Self, TreeRestoreError> {
        if matches!(
            profile.confidence,
            EmulatorProfileConfidence::Speculative | EmulatorProfileConfidence::KnownPath
        ) {
            return Err(refused(TreeRestoreRefusal::MissingBinding));
        }
        let key = profile
            .active_explicit_profile
            .as_ref()
            .unwrap_or(&profile.configuration_root);
        if !key.is_absolute() {
            return Err(refused(TreeRestoreRefusal::MissingBinding));
        }
        let key = key
            .to_str()
            .ok_or_else(|| refused(TreeRestoreRefusal::MissingBinding))?;
        Ok(Self {
            game,
            emulator,
            profile: key.into(),
            artifact_type: SaveArtifactType::SaveDirectory,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmulatorQuiescence {
    Closed,
    Running,
    Unknown,
}

/// Implementations must observe external processes and the resolved profile;
/// absence of an EmuWiz child is insufficient evidence for Closed. Unknown is
/// mandatory on unsupported emulators, incomplete observations or isolation.
pub trait QuiescenceProvider {
    fn observe(&self, binding: &DirectorySaveBinding) -> EmulatorQuiescence;
}

/// Safe production default. Existing child tracking cannot prove external
/// emulator closure. A future system-level detector plugs into this seam.
pub struct UnavailableQuiescence;
impl QuiescenceProvider for UnavailableQuiescence {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        EmulatorQuiescence::Unknown
    }
}

pub trait RestoreSpaceProvider {
    /// Available bytes on the filesystem containing this existing directory.
    fn available_bytes(&self, directory: &Path) -> io::Result<u64>;
}
pub struct FilesystemSpace;
impl RestoreSpaceProvider for FilesystemSpace {
    fn available_bytes(&self, directory: &Path) -> io::Result<u64> {
        use std::ffi::CString;
        let path = CString::new(directory.as_os_str().as_encoded_bytes())
            .map_err(|_| io::Error::other("invalid filesystem path"))?;
        let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: path is NUL terminated; statvfs initializes info on success.
        if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let info = unsafe { info.assume_init() };
        Ok((info.f_bavail as u64).saturating_mul(info.f_frsize as u64))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TreeFingerprint {
    exists: bool,
    directories: Vec<PathBuf>,
    files: Vec<TreeFile>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SafetyRecord {
    pub binding: DirectorySaveBinding,
    pub before: TreeFingerprint,
    pub target: TreeFingerprint,
    pub removed: Vec<PathBuf>,
}

/// Confirmation applies this exact opaque plan. Replanning requires a new preview.
#[derive(Clone, Debug)]
pub struct DirectoryRestorePreview {
    pub binding: DirectorySaveBinding,
    pub snapshot_id: String,
    pub snapshot_unix_seconds: u64,
    pub destination: PathBuf,
    pub files_to_create: Vec<PathBuf>,
    pub files_to_replace: Vec<PathBuf>,
    pub files_to_remove: Vec<PathBuf>,
    pub restored_bytes: u64,
    pub preserved_bytes: u64,
    pub required_space_bytes: u64,
    pub available_space_bytes: Option<u64>,
    pub quiescence: EmulatorQuiescence,
    pub refusals: Vec<TreeRestoreRefusal>,
    pub preservation_and_undo: bool,
    plan: TreeRestorePlan,
    manifest: SaveSnapshotManifest,
    storage: PathBuf,
    root_stamp: Option<(u64, u64)>,
    manifest_disk_sha256: String,
    required_space: u64,
}
impl DirectoryRestorePreview {
    pub fn ready(&self) -> bool {
        self.plan.ready()
    }
}

fn refused(reason: TreeRestoreRefusal) -> TreeRestoreError {
    TreeRestoreError::Refused(vec![reason])
}
fn binding_check(
    binding: Option<&DirectorySaveBinding>,
    m: &SaveSnapshotManifest,
) -> Result<DirectorySaveBinding, TreeRestoreError> {
    let b = binding.ok_or_else(|| refused(TreeRestoreRefusal::MissingBinding))?;
    let game = match &b.game {
        ResolvedSaveGame::Unique(id) if !id.trim().is_empty() => id,
        ResolvedSaveGame::Ambiguous => return Err(refused(TreeRestoreRefusal::AmbiguousIdentity)),
        _ => return Err(refused(TreeRestoreRefusal::MissingBinding)),
    };
    if b.emulator.trim().is_empty() || b.profile.trim().is_empty() {
        return Err(refused(TreeRestoreRefusal::MissingBinding));
    }
    if b.artifact_type != SaveArtifactType::SaveDirectory || m.artifact_type != b.artifact_type {
        return Err(refused(TreeRestoreRefusal::WrongArtifactType));
    }
    if m.game_identity.as_ref() != Some(game) {
        return Err(refused(TreeRestoreRefusal::WrongGameIdentity));
    }
    if m.emulator.as_ref() != Some(&b.emulator) {
        return Err(refused(TreeRestoreRefusal::WrongEmulator));
    }
    if m.emulator_profile.as_ref() != Some(&b.profile) {
        return Err(refused(TreeRestoreRefusal::WrongProfile));
    }
    Ok(b.clone())
}

pub(crate) fn read_bounded_regular(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    if !matches!(kind_of(path)?, Some(Kind::File)) {
        return Err(io::Error::other("not a regular file"));
    }
    let file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(io::Error::other("bounded file too large"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("bounded file too large"));
    }
    Ok(bytes)
}
fn checked_manifest(snapshot: &SaveSnapshot) -> Result<(), TreeRestoreError> {
    let path = snapshot.storage_path.join("manifest.json");
    if !safe_target_path(&path) || !super::super::safe_existing_parent(path.parent().unwrap()) {
        return Err(refused(TreeRestoreRefusal::SnapshotUnreadable));
    }
    let size = fs::symlink_metadata(&path)
        .map_err(|_| refused(TreeRestoreRefusal::SnapshotUnreadable))?
        .len();
    if size > MAX_MANIFEST_BYTES {
        return Err(refused(TreeRestoreRefusal::ManifestTooLarge));
    }
    let bytes = read_bounded_regular(&path, MAX_MANIFEST_BYTES)
        .map_err(|_| refused(TreeRestoreRefusal::SnapshotUnreadable))?;
    let disk: SaveSnapshotManifest =
        serde_json::from_slice(&bytes).map_err(|_| refused(TreeRestoreRefusal::ManifestChanged))?;
    if disk != snapshot.manifest
        || serde_json::from_slice::<serde_json::Value>(&bytes).ok()
            != serde_json::to_value(&snapshot.manifest).ok()
    {
        return Err(refused(TreeRestoreRefusal::ManifestChanged));
    }
    // Validate every path and ancestor before the existing verifier opens it.
    if disk.format_version != 1 {
        return Err(refused(TreeRestoreRefusal::SnapshotUnreadable));
    }
    if disk.artifacts.len() > MAX_SNAPSHOT_FILES {
        return Err(refused(TreeRestoreRefusal::TooManyEntries));
    }
    let mut total = 0u64;
    for a in &disk.artifacts {
        if !valid_relative(&a.relative_path) {
            return Err(refused(TreeRestoreRefusal::UnsafeRelativePath(
                a.relative_path.clone(),
            )));
        }
        let source = snapshot.storage_path.join("files").join(&a.relative_path);
        if !safe_target_path(&source)
            || !super::super::safe_existing_parent(source.parent().unwrap())
        {
            return Err(refused(TreeRestoreRefusal::SnapshotUnreadable));
        }
        if kind_of(&source).ok() != Some(Some(Kind::File)) {
            return Err(refused(TreeRestoreRefusal::SnapshotUnreadable));
        }
        total = total
            .checked_add(a.size_bytes)
            .ok_or_else(|| refused(TreeRestoreRefusal::TooManyEntries))?;
    }
    if total > crate::save_snapshots::MAX_SNAPSHOT_BYTES || total != disk.source_size_bytes {
        return Err(refused(TreeRestoreRefusal::SnapshotUnreadable));
    }
    Ok(())
}

fn disk_manifest_digest(snapshot: &SaveSnapshot) -> Result<String, TreeRestoreError> {
    let bytes = read_bounded_regular(
        &snapshot.storage_path.join("manifest.json"),
        MAX_MANIFEST_BYTES,
    )
    .map_err(|_| refused(TreeRestoreRefusal::SnapshotUnreadable))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn normalized(path: &Path) -> Result<PathBuf, TreeRestoreError> {
    // Only normal absolute paths, and no symlink ancestors. This deliberately
    // refuses aliases rather than trying to follow them during publication.
    if !safe_target_path(path) {
        return Err(refused(TreeRestoreRefusal::DestinationUnsafe(
            path.display().to_string(),
        )));
    }
    let mut base = path;
    let mut suffix = Vec::new();
    while !base.exists() {
        suffix.push(base.file_name().ok_or_else(|| {
            refused(TreeRestoreRefusal::DestinationUnsafe(
                "missing ancestor".into(),
            ))
        })?);
        base = base.parent().ok_or_else(|| {
            refused(TreeRestoreRefusal::DestinationUnsafe(
                "missing ancestor".into(),
            ))
        })?;
    }
    if fs::symlink_metadata(base)
        .map_err(|e| io_err(base, e))?
        .file_type()
        .is_symlink()
    {
        return Err(refused(TreeRestoreRefusal::DestinationUnsafe(
            "symlink".into(),
        )));
    }
    if !super::super::safe_existing_parent(base) {
        return Err(refused(TreeRestoreRefusal::DestinationUnsafe(
            "unsafe ancestor".into(),
        )));
    }
    let mut result = fs::canonicalize(base).map_err(|e| io_err(base, e))?;
    for part in suffix.into_iter().rev() {
        result.push(part);
    }
    Ok(result)
}
fn disjoint(a: &Path, b: &Path) -> Result<(), TreeRestoreError> {
    let a = normalized(a)?;
    let b = normalized(b)?;
    if a.starts_with(&b) || b.starts_with(&a) {
        return Err(refused(TreeRestoreRefusal::SnapshotDestinationOverlap));
    }
    Ok(())
}

pub(crate) fn fingerprint(root: &Path) -> Result<TreeFingerprint, String> {
    match kind_of(root).map_err(|e| e.to_string())? {
        None => {
            return Ok(TreeFingerprint {
                exists: false,
                directories: vec![],
                files: vec![],
            });
        }
        Some(Kind::Dir) => {}
        _ => return Err("destination is not a real directory".into()),
    }
    let device = fs::metadata(root).map_err(|e| e.to_string())?.dev();
    let (tree, special) = walk_tree(root).map_err(|e| e.to_string())?;
    if special != 0 {
        return Err("tree contains symlink/special entries".into());
    }
    let mut out = TreeFingerprint {
        exists: true,
        directories: vec![],
        files: vec![],
    };
    for (p, kind) in tree {
        if fs::symlink_metadata(root.join(&p))
            .map_err(|e| e.to_string())?
            .dev()
            != device
        {
            return Err("tree crosses filesystems".into());
        }
        if !valid_relative(&p) {
            return Err("tree depth exceeds bound".into());
        }
        if kind == Kind::Dir {
            out.directories.push(p);
        } else {
            let (size_bytes, sha256, _) =
                hash_and_metadata(&root.join(&p)).map_err(|e| e.to_string())?;
            out.files.push(TreeFile {
                relative_path: p,
                size_bytes,
                sha256,
            });
        }
    }
    Ok(out)
}
fn root_stamp(path: &Path) -> io::Result<Option<(u64, u64)>> {
    Ok(match fs::symlink_metadata(path) {
        Ok(m) => Some((m.dev(), m.ino())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    })
}

pub(crate) fn augment_plan(plan: &mut TreeRestorePlan, binding: DirectorySaveBinding) {
    let before = match fingerprint(&plan.destination_root) {
        Ok(v) => v,
        Err(e) => {
            plan.refusals.push(TreeRestoreRefusal::DestinationUnsafe(e));
            return;
        }
    };
    let mut dirs = BTreeSet::new();
    let mut files = Vec::new();
    for entry in &plan.entries {
        let mut parent = entry.relative_path.parent();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            dirs.insert(p.to_path_buf());
            parent = p.parent();
        }
        files.push(TreeFile {
            relative_path: entry.relative_path.clone(),
            size_bytes: entry.size_bytes,
            sha256: entry.sha256.clone(),
        });
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let target = TreeFingerprint {
        exists: true,
        directories: dirs.into_iter().collect(),
        files,
    };
    if target.directories.len() + target.files.len() > MAX_SNAPSHOT_FILES * 4 {
        plan.refusals.push(TreeRestoreRefusal::TooManyEntries);
    }
    let kept: BTreeSet<_> = target
        .directories
        .iter()
        .chain(target.files.iter().map(|f| &f.relative_path))
        .cloned()
        .collect();
    let mut removed = Vec::<PathBuf>::new();
    let mut existing: Vec<_> = before
        .directories
        .iter()
        .chain(before.files.iter().map(|f| &f.relative_path))
        .cloned()
        .collect();
    existing.sort_by_key(|p| (p.components().count(), p.clone()));
    for p in existing {
        if kept.contains(&p)
            || target.files.iter().any(|f| p.starts_with(&f.relative_path))
            || removed.iter().any(|r| p.starts_with(r))
        {
            continue;
        }
        removed.push(p);
    }
    plan.untouched_files.clear();
    plan.safety = Some(SafetyRecord {
        binding,
        before,
        target,
        removed,
    });
}
fn requirement(state: EmulatorQuiescence) -> SaveQuiescenceRequirement {
    match state {
        EmulatorQuiescence::Closed => SaveQuiescenceRequirement::ConfirmedClosed,
        EmulatorQuiescence::Running => SaveQuiescenceRequirement::Running,
        EmulatorQuiescence::Unknown => SaveQuiescenceRequirement::Unknown,
    }
}
fn check_quiescence(state: EmulatorQuiescence) -> Result<(), TreeRestoreError> {
    match state {
        EmulatorQuiescence::Closed => Ok(()),
        EmulatorQuiescence::Running => Err(refused(TreeRestoreRefusal::EmulatorRunning)),
        EmulatorQuiescence::Unknown => Err(refused(TreeRestoreRefusal::EmulatorStateUnknown)),
    }
}

pub fn preview_directory_restore(
    snapshot: &SaveSnapshot,
    binding: Option<&DirectorySaveBinding>,
    destination: &Path,
    quiescence: &dyn QuiescenceProvider,
    space: &dyn RestoreSpaceProvider,
) -> Result<DirectoryRestorePreview, TreeRestoreError> {
    let binding = binding_check(binding, &snapshot.manifest)?;
    checked_manifest(snapshot)?;
    disjoint(&snapshot.storage_path, destination)?;
    if !destination
        .parent()
        .is_some_and(super::super::safe_existing_parent)
    {
        return Err(refused(TreeRestoreRefusal::DestinationUnsafe(
            "existing real parent required".into(),
        )));
    }
    normalized(destination)?;
    let manifest_disk_sha256 = disk_manifest_digest(snapshot)?;
    let state = quiescence.observe(&binding);
    let mut plan = plan_tree_restore(snapshot, destination, requirement(state));
    augment_plan(&mut plan, binding.clone());
    let record_bytes = serde_json::to_vec_pretty(&(
        &plan.entries,
        &plan.directories_to_create,
        &plan.displaced_files,
        &plan.safety,
    ))
    .map_err(|_| refused(TreeRestoreRefusal::TooManyEntries))?;
    if record_bytes.len() > 8 * 1024 * 1024 {
        return Err(refused(TreeRestoreRefusal::TooManyEntries));
    }
    let record = plan.safety.as_ref();
    let restored = snapshot
        .manifest
        .artifacts
        .iter()
        .map(|a| a.size_bytes)
        .sum::<u64>();
    let preserved = record.map_or(0, |r| {
        r.before
            .files
            .iter()
            .map(|f| f.size_bytes)
            .fold(0u64, u64::saturating_add)
    });
    // Full restored staging + full preimage copy fallback + allocation/journal
    // overhead. This is conservative, not a disk reservation or cross-FS promise.
    let entries = record.map_or(0, |r| {
        r.before.files.len()
            + r.before.directories.len()
            + r.target.files.len()
            + r.target.directories.len()
    });
    let required = restored
        .checked_add(preserved)
        .and_then(|n| n.checked_add(SPACE_MARGIN + (entries as u64) * 8192))
        .ok_or_else(|| refused(TreeRestoreRefusal::SpaceUnknown))?;
    let available = destination
        .parent()
        .and_then(|p| space.available_bytes(p).ok());
    match available {
        Some(a) if a < required => plan.refusals.push(TreeRestoreRefusal::InsufficientSpace {
            required,
            available: a,
        }),
        None => plan.refusals.push(TreeRestoreRefusal::SpaceUnknown),
        _ => {}
    }
    let files_to_remove = record.map_or_else(Vec::new, |r| {
        r.before
            .files
            .iter()
            .filter(|f| {
                !r.target
                    .files
                    .iter()
                    .any(|t| t.relative_path == f.relative_path)
            })
            .map(|f| f.relative_path.clone())
            .collect()
    });
    Ok(DirectoryRestorePreview {
        binding,
        snapshot_id: snapshot.manifest.snapshot_id.clone(),
        snapshot_unix_seconds: snapshot.manifest.snapshot_unix_seconds,
        destination: destination.to_path_buf(),
        files_to_create: plan
            .entries
            .iter()
            .filter(|e| e.action == EntryAction::Create)
            .map(|e| e.relative_path.clone())
            .collect(),
        files_to_replace: plan
            .entries
            .iter()
            .filter(|e| e.action != EntryAction::Create)
            .map(|e| e.relative_path.clone())
            .collect(),
        files_to_remove,
        restored_bytes: restored,
        preserved_bytes: preserved,
        required_space_bytes: required,
        available_space_bytes: available,
        quiescence: state,
        refusals: plan.refusals.clone(),
        preservation_and_undo: plan.ready(),
        root_stamp: root_stamp(destination).map_err(|e| io_err(destination, e))?,
        required_space: required,
        manifest_disk_sha256,
        plan,
        manifest: snapshot.manifest.clone(),
        storage: snapshot.storage_path.clone(),
    })
}

pub fn apply_directory_restore(
    snapshot: &SaveSnapshot,
    preview: &DirectoryRestorePreview,
    quiescence: &dyn QuiescenceProvider,
    space: &dyn RestoreSpaceProvider,
    options: &TreeRestoreOptions,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    apply_reviewed(snapshot, preview, quiescence, space, options, &no_hook)
}
fn apply_reviewed(
    snapshot: &SaveSnapshot,
    preview: &DirectoryRestorePreview,
    quiescence: &dyn QuiescenceProvider,
    space: &dyn RestoreSpaceProvider,
    options: &TreeRestoreOptions,
    hook: Hook<'_>,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    if !preview.plan.ready() {
        return Err(TreeRestoreError::Refused(preview.plan.refusals.clone()));
    }
    if snapshot.manifest != preview.manifest || snapshot.storage_path != preview.storage {
        return Err(refused(TreeRestoreRefusal::ManifestChanged));
    }
    let record = preview
        .plan
        .safety
        .as_ref()
        .expect("ready gated plan has safety record");
    binding_check(Some(&record.binding), &snapshot.manifest)?;
    checked_manifest(snapshot)?;
    if disk_manifest_digest(snapshot)? != preview.manifest_disk_sha256 {
        return Err(refused(TreeRestoreRefusal::ManifestChanged));
    }
    disjoint(&snapshot.storage_path, &preview.plan.destination_root)?;
    // Journal must not be eaten by restore, nor modify immutable snapshot storage.
    disjoint(&options.journal_root, &preview.plan.destination_root)?;
    disjoint(&options.journal_root, &snapshot.storage_path)?;
    let parent = preview.plan.destination_root.parent().unwrap();
    let journal_path = normalized(&options.journal_root)?;
    let mut ancestor = journal_path.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().unwrap();
    }
    if fs::metadata(ancestor)
        .map_err(|e| io_err(ancestor, e))?
        .dev()
        != fs::metadata(parent).map_err(|e| io_err(parent, e))?.dev()
    {
        return Err(refused(TreeRestoreRefusal::CrossFilesystem));
    }
    check_quiescence(quiescence.observe(&record.binding))?;
    if root_stamp(&preview.plan.destination_root).map_err(|e| io_err(parent, e))?
        != preview.root_stamp
        || fingerprint(&preview.plan.destination_root).ok().as_ref() != Some(&record.before)
    {
        return Err(refused(TreeRestoreRefusal::DestinationChanged));
    }
    let available = space
        .available_bytes(parent)
        .map_err(|_| refused(TreeRestoreRefusal::SpaceUnknown))?;
    if available < preview.required_space {
        return Err(refused(TreeRestoreRefusal::InsufficientSpace {
            required: preview.required_space,
            available,
        }));
    }
    let refusal = std::cell::RefCell::new(None);
    let recheck = || -> Result<(), TreeRestoreError> {
        check_quiescence(quiescence.observe(&record.binding))?;
        checked_manifest(snapshot)?;
        if disk_manifest_digest(snapshot)? != preview.manifest_disk_sha256 {
            return Err(refused(TreeRestoreRefusal::ManifestChanged));
        }
        if root_stamp(&preview.plan.destination_root).map_err(|e| io_err(parent, e))?
            != preview.root_stamp
            || fingerprint(&preview.plan.destination_root).ok().as_ref() != Some(&record.before)
        {
            return Err(refused(TreeRestoreRefusal::DestinationChanged));
        }
        let required = preview
            .required_space
            .saturating_sub(preview.manifest.source_size_bytes);
        let available = space
            .available_bytes(parent)
            .map_err(|_| refused(TreeRestoreRefusal::SpaceUnknown))?;
        if available < required {
            return Err(refused(TreeRestoreRefusal::InsufficientSpace {
                required,
                available,
            }));
        }
        check_quiescence(quiescence.observe(&record.binding))?;
        Ok(())
    };
    let guarded_hook = |phase| {
        hook(phase)?;
        if phase == Phase::BeforePublish {
            if let Err(error) = recheck() {
                let detail = error.to_string();
                refusal.replace(Some(error));
                return Err(io::Error::other(detail));
            }
        }
        Ok(())
    };
    let result = apply_with_hook(snapshot, &preview.plan, options, &guarded_hook);
    // Preserve a journal/recovery error if refusing safely could not be recorded.
    if matches!(result, Err(TreeRestoreError::RecoveryRequired { .. })) {
        return result;
    }
    if let Some(error) = refusal.into_inner() {
        return Err(error);
    }
    result
}

fn bound_journal(path: &Path) -> Result<TreeRestoreJournal, TreeRestoreError> {
    if !safe_target_path(path) || !super::super::safe_existing_parent(path.parent().unwrap()) {
        return Err(refused(TreeRestoreRefusal::DestinationUnsafe(
            "journal path".into(),
        )));
    }
    let journal = load_tree_restore_journal(path)?;
    let record = journal
        .safety
        .as_ref()
        .ok_or_else(|| refused(TreeRestoreRefusal::MissingBinding))?;
    if !matches!(&record.binding.game,ResolvedSaveGame::Unique(id) if !id.trim().is_empty())
        || record.binding.emulator.trim().is_empty()
        || record.binding.profile.trim().is_empty()
        || record.binding.artifact_type != SaveArtifactType::SaveDirectory
    {
        return Err(refused(TreeRestoreRefusal::MissingBinding));
    }
    normalized(&journal.destination_root)?;
    normalized(&journal.work_dir)?;
    if journal.work_dir.exists() {
        let (objects, special) = walk_tree_with_limit(&journal.work_dir, MAX_SNAPSHOT_FILES * 20)
            .map_err(|e| io_err(&journal.work_dir, e))?;
        let device = fs::metadata(&journal.work_dir)
            .map_err(|e| io_err(&journal.work_dir, e))?
            .dev();
        if special != 0
            || objects.keys().any(|p| {
                fs::symlink_metadata(journal.work_dir.join(p)).map_or(true, |m| m.dev() != device)
            })
        {
            return Err(refused(TreeRestoreRefusal::DestinationUnsafe(
                "unsafe transaction storage".into(),
            )));
        }
    }
    fingerprint(&journal.destination_root)
        .map_err(|e| refused(TreeRestoreRefusal::DestinationUnsafe(e)))?;
    let parent = journal.destination_root.parent().ok_or_else(|| {
        refused(TreeRestoreRefusal::DestinationUnsafe(
            "root destination".into(),
        ))
    })?;
    let expected_work = parent.join(format!(".emuwiz-restore-{}", journal.transaction_id));
    if journal.transaction_id.contains('/')
        || journal.work_dir != expected_work
        || journal.lock_path != lock_path_for(&journal.destination_root)
        || journal.journal_path != path
        || record
            .before
            .directories
            .iter()
            .chain(record.target.directories.iter())
            .chain(
                record
                    .before
                    .files
                    .iter()
                    .chain(record.target.files.iter())
                    .map(|f| &f.relative_path),
            )
            .chain(journal.displaced_files.iter().map(|f| &f.relative_path))
            .any(|p| !valid_relative(p))
        || journal
            .entries
            .iter()
            .any(|e| !valid_relative(&e.relative_path))
        || journal
            .created_directories
            .iter()
            .chain(record.removed.iter())
            .any(|p| !valid_relative(p))
    {
        return Err(refused(TreeRestoreRefusal::DestinationUnsafe(
            "journal paths changed".into(),
        )));
    }
    Ok(journal)
}
fn journal_gate(
    path: &Path,
    provider: &dyn QuiescenceProvider,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    let mut journal = bound_journal(path)?;
    let binding = &journal.safety.as_ref().unwrap().binding;
    if let Err(error) = check_quiescence(provider.observe(binding)) {
        journal.detail = Some(format!("operation refused: {error}"));
        persist_journal(&journal)?;
        return Err(error);
    }
    Ok(journal)
}
pub fn undo_directory_restore(
    path: &Path,
    provider: &dyn QuiescenceProvider,
    now: u64,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    let j = journal_gate(path, provider)?;
    let gate = |phase| {
        if phase == Phase::BeforeUndo {
            check_quiescence(provider.observe(&j.safety.as_ref().unwrap().binding))
                .map_err(|e| io::Error::other(e.to_string()))?;
            // Refuse additions/changes since the initial conflict check too.
            if !undo_conflicts(&j).is_empty() {
                return Err(io::Error::other("destination/preimage changed before undo"));
            }
            check_quiescence(provider.observe(&j.safety.as_ref().unwrap().binding))
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        Ok(())
    };
    undo_with_hook(path, SaveQuiescenceRequirement::ConfirmedClosed, now, &gate)
}
pub fn recover_directory_restore(
    path: &Path,
    provider: &dyn QuiescenceProvider,
    now: u64,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    let j = journal_gate(path, provider)?;
    if !matches!(
        j.status,
        TreeRestoreStatus::Published | TreeRestoreStatus::RolledBack | TreeRestoreStatus::Undone
    ) {
        match kind_of(&j.lock_path).map_err(|e| io_err(&j.lock_path, e))? {
            None => acquire_lock(&j.lock_path, &j.transaction_id)?,
            Some(Kind::File)
                if read_bounded_regular(&j.lock_path, 1024).ok().as_deref()
                    == Some(j.transaction_id.as_bytes()) => {}
            _ => return Err(refused(TreeRestoreRefusal::TransactionInProgress)),
        }
    }
    let gate = |phase| {
        if phase == Phase::BeforeRecovery {
            check_quiescence(provider.observe(&j.safety.as_ref().unwrap().binding))
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        Ok(())
    };
    recover_with_hook(path, now, &gate)
}

pub(crate) fn verify_preserved(
    j: &TreeRestoreJournal,
    record: &SafetyRecord,
) -> Result<(), String> {
    let source = |p: &Path| {
        if let Some(r) = record.removed.iter().find(|r| p.starts_with(r)) {
            return j
                .work_dir
                .join("preserved/removed")
                .join(r)
                .join(p.strip_prefix(r).unwrap());
        }
        if let Some(e) = j.entries.iter().find(|e| {
            e.action == EntryAction::ReplaceDirectoryWithFile && p.starts_with(&e.relative_path)
        }) {
            return j
                .work_dir
                .join("preserved/dirs")
                .join(&e.relative_path)
                .join(p.strip_prefix(&e.relative_path).unwrap());
        }
        if j.displaced_files.iter().any(|f| f.relative_path == p) {
            return j.work_dir.join("preserved/displaced-files").join(p);
        }
        j.work_dir.join("preserved/files").join(p)
    };
    for f in &record.before.files {
        let p = source(&f.relative_path);
        if !file_matches(&p, &f.sha256) {
            return Err(format!(
                "{}: preserved preimage changed",
                f.relative_path.display()
            ));
        }
    }
    for dir in &record.before.directories {
        if record.removed.iter().any(|r| dir.starts_with(r))
            || j.entries.iter().any(|e| {
                e.action == EntryAction::ReplaceDirectoryWithFile
                    && dir.starts_with(&e.relative_path)
            })
        {
            if kind_of(&source(dir)).ok() != Some(Some(Kind::Dir)) {
                return Err("preserved directory shape changed".into());
            }
        }
    }
    // Detect new files/empty directories in entire preserved directory subtrees.
    for r in record.removed.iter().chain(
        j.entries
            .iter()
            .filter(|e| e.action == EntryAction::ReplaceDirectoryWithFile)
            .map(|e| &e.relative_path),
    ) {
        if !record.before.directories.contains(r) {
            continue;
        }
        let expected = TreeFingerprint {
            exists: true,
            directories: record
                .before
                .directories
                .iter()
                .filter(|p| *p != r && p.starts_with(r))
                .map(|p| p.strip_prefix(r).unwrap().to_path_buf())
                .collect(),
            files: record
                .before
                .files
                .iter()
                .filter(|f| f.relative_path.starts_with(r))
                .map(|f| TreeFile {
                    relative_path: f.relative_path.strip_prefix(r).unwrap().to_path_buf(),
                    size_bytes: f.size_bytes,
                    sha256: f.sha256.clone(),
                })
                .collect(),
        };
        if fingerprint(&source(r))? != expected {
            return Err("preserved tree shape changed".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
