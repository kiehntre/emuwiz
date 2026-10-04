//! Per-launch cheat runtime executor (Phase 1 backbone).
//!
//! This is the executor half of the per-launch cheat design
//! (`docs/design/CHEAT_PER_LAUNCH_WORKSPACE_V1.md`). The planner in
//! [`cheat_launch_plan`](super::cheat_launch_plan) says *what* should happen;
//! this module makes it happen safely for exactly one launch:
//!
//! ```text
//! PLANNER      -> CheatRuntimePlan   (what to create, which arguments to add)
//! ADAPTER      -> builds that plan from emulator-specific material
//! EXECUTOR     -> validates, materialises an EmuWiz-owned workspace, hands the
//!                 extra arguments to the canonical launcher, records a receipt
//! LAUNCHER     -> the existing canonical spawn path (never a second launcher)
//! CLEANUP      -> removes only the verified EmuWiz-owned workspace
//! ```
//!
//! # What this does not claim
//!
//! Creating a config file is not proof that a cheat ran. The receipt keeps
//! those apart: `Materialised` (files exist), `PassedToEmulator` (the process
//! was started with the prepared arguments), `ProcessStarted`, and
//! `RuntimeEffectUnknown`. Phase 1 has no in-game verification, so a
//! successful launch only ever means "the emulator was launched with the
//! prepared cheat configuration" - never "the cheat worked".
//!
//! # Safety rules
//!
//! * Temporary, per-launch, explicit, reversible. Nothing permanent is
//!   edited; real emulator configuration, saves, cheat files and source media
//!   are only ever read (and fingerprinted before and after).
//! * Everything is created under one fresh directory directly below the
//!   approved root, named `cheats-<launch id>`. An existing path is never
//!   reused, no path may leave the workspace, and files are created with
//!   `create_new` and owner-only permissions.
//! * Cleanup removes a directory only after checking that it is a direct,
//!   non-symlink child of the approved root with a valid EmuWiz marker for
//!   the same launch. A path supplied by saved state is never trusted on its
//!   own. Failure is reported and the tree is left for explicit cleanup; it is
//!   never widened, and a running emulator's files are never deleted.
//! * A crash leaves the workspace behind. [`scan_cheat_runtime_workspaces`]
//!   (read-only) classifies what is there and [`cleanup_stale_workspace`]
//!   removes one explicitly; foreign or unreadable directories are never
//!   touched.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::cheat_launch_plan::{
    LaunchStateClass, LaunchStateExpectation, StateBaseline, StateExpectation, StateOutcome,
    ViolationSeverity, capture_baseline, verify_expectation,
};
use super::retroarch_resource_projection::{
    approved_retroarch_data_launch_root, approved_retroarch_launch_root,
};

pub const WORKSPACE_MARKER_NAME: &str = ".emuwiz-cheat-runtime.json";
pub const WORKSPACE_DIR_PREFIX: &str = "cheats-";
pub const MARKER_SCHEMA_VERSION: u32 = 1;
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;
/// Workspaces younger than this are never reported as stale.
pub const DEFAULT_STALE_GRACE_SECS: u64 = 120;

const MAX_MARKER_BYTES: u64 = 64 * 1024;
const MAX_RUNTIME_FILE_BYTES: usize = 1024 * 1024;
const MAX_RUNTIME_FILES: usize = 32;
const MAX_SOURCE_HASH_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RECEIPT_ERRORS: usize = 16;
const MAX_ERROR_CHARS: usize = 300;
const MAX_LAUNCH_ID_CHARS: usize = 64;

// ---------------------------------------------------------------------
// Truthful stages and the receipt
// ---------------------------------------------------------------------

/// How far one launch got. Each stage means exactly what its name says and no
/// more; none of them is evidence that a cheat took effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheatRuntimeStage {
    /// A validated plan exists. Nothing was created.
    Planned,
    /// The workspace and its files exist on disk.
    Materialised,
    /// The process was started with the prepared arguments.
    PassedToEmulator,
    /// A process id exists for that launch.
    ProcessStarted,
    /// The emulator is running or ran; whether any cheat took effect is not
    /// known and nothing in Phase 1 can establish it.
    RuntimeEffectUnknown,
    /// The EmuWiz-owned workspace was removed.
    CleanedUp,
}

/// Whether the in-game effect is known. Phase 1 can only say `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuntimeEffect {
    /// No runtime-effect test exists for this launch.
    Unknown,
    /// No cheat was part of this launch.
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MaterialisationOutcome {
    NotAttempted,
    Materialised {
        /// Workspace-relative names of what was created.
        created: Vec<String>,
    },
    Refused {
        reason: RuntimeRefusal,
    },
    Failed {
        detail: String,
        rolled_back: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HandoffOutcome {
    NotAttempted,
    /// Materialisation did not succeed, so nothing was handed over.
    NotReached,
    PassedToEmulator,
    SpawnFailed {
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitSummary {
    pub success: bool,
    pub code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSummary {
    pub pid: u32,
    pub exit: Option<ExitSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CleanupOutcome {
    NotAttempted,
    /// There was nothing to clean (no cheats, or nothing was created).
    NotNeeded,
    Completed,
    Failed {
        detail: String,
    },
    /// The emulator may still be using the files; they are left in place.
    DeferredEmulatorRunning,
}

/// One selected cheat as recorded in the receipt. Source paths are not kept.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReceiptCheat {
    pub logical_id: String,
    pub variant_id: String,
    pub title: String,
    pub provider: String,
    pub source_id: String,
}

/// One before/after comparison. Only the file name is kept, not the full path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateCheckRecord {
    pub label: String,
    pub class: LaunchStateClass,
    pub outcome: StateCheckOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum StateCheckOutcome {
    Unchanged,
    /// Changed in a way the plan allows (real saves).
    ChangedAsExpected,
    Violation {
        severity: ViolationSeverity,
    },
}

/// Bounded, secret-free account of one launch. No ROM or cheat-source paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatRuntimeReceipt {
    pub schema_version: u32,
    pub launch_id: String,
    pub game_identity: String,
    pub adapter_id: String,
    pub cheats: Vec<ReceiptCheat>,
    /// An EmuWiz temporary directory, not a user path.
    pub workspace_root: Option<PathBuf>,
    /// Stages reached, in order. Never contains a stage that did not happen.
    pub stages: Vec<CheatRuntimeStage>,
    pub materialisation: MaterialisationOutcome,
    pub handoff: HandoffOutcome,
    pub process: Option<ProcessSummary>,
    pub runtime_effect: RuntimeEffect,
    pub state_checks: Vec<StateCheckRecord>,
    pub cleanup: CleanupOutcome,
    /// Plain-language things the user should know (for example that their own
    /// emulator settings were not inherited). Not failures.
    pub notes: Vec<String>,
    pub errors: Vec<String>,
}

impl CheatRuntimeReceipt {
    fn new(plan: &CheatRuntimePlan) -> Self {
        Self {
            schema_version: RECEIPT_SCHEMA_VERSION,
            launch_id: plan.launch_id.clone(),
            game_identity: plan.game_identity.clone(),
            adapter_id: plan.adapter_id.clone(),
            cheats: plan.cheats.clone(),
            workspace_root: None,
            stages: vec![CheatRuntimeStage::Planned],
            materialisation: MaterialisationOutcome::NotAttempted,
            handoff: HandoffOutcome::NotAttempted,
            process: None,
            runtime_effect: RuntimeEffect::Unknown,
            state_checks: Vec::new(),
            cleanup: CleanupOutcome::NotAttempted,
            notes: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn reach(&mut self, stage: CheatRuntimeStage) {
        if !self.stages.contains(&stage) {
            self.stages.push(stage);
        }
    }

    fn error(&mut self, text: impl AsRef<str>) {
        if self.errors.len() < MAX_RECEIPT_ERRORS {
            self.errors
                .push(text.as_ref().chars().take(MAX_ERROR_CHARS).collect());
        }
    }

    fn note(&mut self, text: impl AsRef<str>) {
        if self.notes.len() < MAX_RECEIPT_ERRORS {
            self.notes
                .push(text.as_ref().chars().take(MAX_ERROR_CHARS).collect());
        }
    }

    /// The furthest stage reached.
    #[must_use]
    pub fn highest_stage(&self) -> CheatRuntimeStage {
        self.stages
            .iter()
            .copied()
            .max()
            .unwrap_or(CheatRuntimeStage::Planned)
    }

    /// True once a state check found a violation.
    #[must_use]
    pub fn has_violation(&self) -> bool {
        self.state_checks
            .iter()
            .any(|check| matches!(check.outcome, StateCheckOutcome::Violation { .. }))
    }
}

// ---------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------

/// Why a runtime plan was refused before anything was created.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RuntimeRefusal {
    /// The live game is not the game the plan was made for.
    GameMismatch,
    /// The live emulator is not the emulator the plan was made for.
    EmulatorMismatch,
    /// The plan changed after it was built.
    StalePlan,
    /// A source cheat file no longer matches what was planned.
    CheatEvidenceChanged {
        source: String,
    },
    /// A required source cheat file is gone or unreadable.
    CheatMaterialMissing {
        source: String,
    },
    InvalidLaunchId,
    WorkspaceOutsideApprovedRoot,
    ApprovedRootUnsafe,
    /// Something already exists at the workspace path; it is never reused.
    WorkspaceExists,
    PathEscapesWorkspace {
        path: String,
    },
    SymlinkInWorkspace {
        path: String,
    },
    PlanShape {
        detail: String,
    },
}

impl std::fmt::Display for RuntimeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for RuntimeRefusal {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaterialiseError {
    Refused(RuntimeRefusal),
    Failed { detail: String, rolled_back: bool },
}

// ---------------------------------------------------------------------
// The generic runtime plan
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFileKind {
    /// Generated emulator configuration.
    Config,
    /// Generated cheat material.
    CheatMaterial,
}

/// One file to create. `sha256` is checked against `bytes` before writing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeFile {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub kind: RuntimeFileKind,
}

impl RuntimeFile {
    #[must_use]
    pub fn new(path: PathBuf, bytes: Vec<u8>, kind: RuntimeFileKind) -> Self {
        let sha256 = sha256_hex(&bytes);
        Self {
            path,
            bytes,
            sha256,
            kind,
        }
    }
}

/// A read-only source the plan was built from, re-checked before launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBinding {
    pub path: PathBuf,
    pub expected_sha256: String,
}

/// What the live launch says it is. Checked against the plan; never trusted
/// from the moment the plan was built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveLaunchBinding {
    pub game_identity: String,
    pub adapter_id: String,
}

/// Where workspaces may live.
#[derive(Clone, Debug)]
pub struct CheatRuntimeRoots {
    pub approved_root: PathBuf,
}

impl Default for CheatRuntimeRoots {
    /// Phase 1 uses the approved root the RetroArch planner already pins.
    fn default() -> Self {
        Self {
            approved_root: approved_retroarch_launch_root(),
        }
    }
}

impl CheatRuntimeRoots {
    /// Every root a workspace may live under (the temporary root and, when
    /// resolvable, the EmuWiz data root used for Flatpak). A stale-workspace
    /// scan must look at each of them.
    #[must_use]
    pub fn all_known() -> Vec<Self> {
        let mut roots = vec![Self::default()];
        roots.extend(
            approved_retroarch_data_launch_root().map(|approved_root| Self { approved_root }),
        );
        roots
    }
}

/// An emulator-agnostic description of one launch's temporary material.
/// Adapters build it; the executor materialises it. Fields are crate-private
/// so a plan cannot be altered after its digest is computed.
#[derive(Clone, Debug)]
pub struct CheatRuntimePlan {
    pub(crate) launch_id: String,
    pub(crate) adapter_id: String,
    pub(crate) game_identity: String,
    pub(crate) root: PathBuf,
    pub(crate) directories: Vec<PathBuf>,
    pub(crate) files: Vec<RuntimeFile>,
    pub(crate) extra_arguments: Vec<OsString>,
    pub(crate) sources: Vec<SourceBinding>,
    pub(crate) expectations: Vec<LaunchStateExpectation>,
    pub(crate) cheats: Vec<ReceiptCheat>,
    pub(crate) digest: String,
}

/// Inputs an adapter supplies to build a plan.
#[derive(Clone, Debug)]
pub struct CheatRuntimePlanParts {
    pub launch_id: String,
    pub adapter_id: String,
    pub game_identity: String,
    pub root: PathBuf,
    pub directories: Vec<PathBuf>,
    pub files: Vec<RuntimeFile>,
    pub extra_arguments: Vec<OsString>,
    pub sources: Vec<SourceBinding>,
    pub expectations: Vec<LaunchStateExpectation>,
    pub cheats: Vec<ReceiptCheat>,
}

/// The workspace directory name for a launch id.
#[must_use]
pub fn workspace_name(launch_id: &str) -> String {
    format!("{WORKSPACE_DIR_PREFIX}{launch_id}")
}

fn valid_launch_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_LAUNCH_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn normal_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

impl CheatRuntimePlan {
    /// Validates the shape and computes the digest. Nothing touches the disk.
    pub fn new(parts: CheatRuntimePlanParts) -> Result<Self, RuntimeRefusal> {
        if !valid_launch_id(&parts.launch_id) {
            return Err(RuntimeRefusal::InvalidLaunchId);
        }
        if !normal_absolute(&parts.root)
            || parts.root.file_name().and_then(|n| n.to_str())
                != Some(workspace_name(&parts.launch_id).as_str())
        {
            return Err(RuntimeRefusal::WorkspaceOutsideApprovedRoot);
        }
        if parts.files.is_empty() || parts.files.len() > MAX_RUNTIME_FILES {
            return Err(RuntimeRefusal::PlanShape {
                detail: format!("{} files", parts.files.len()),
            });
        }
        for path in parts
            .directories
            .iter()
            .chain(parts.files.iter().map(|file| &file.path))
        {
            if !normal_absolute(path) || path == &parts.root || !path.starts_with(&parts.root) {
                return Err(RuntimeRefusal::PathEscapesWorkspace {
                    path: path.display().to_string(),
                });
            }
        }
        for file in &parts.files {
            if file.bytes.len() > MAX_RUNTIME_FILE_BYTES || sha256_hex(&file.bytes) != file.sha256 {
                return Err(RuntimeRefusal::PlanShape {
                    detail: format!("file {}", file.path.display()),
                });
            }
        }
        let mut plan = Self {
            launch_id: parts.launch_id,
            adapter_id: parts.adapter_id,
            game_identity: parts.game_identity,
            root: parts.root,
            directories: parts.directories,
            files: parts.files,
            extra_arguments: parts.extra_arguments,
            sources: parts.sources,
            expectations: parts.expectations,
            cheats: parts.cheats,
            digest: String::new(),
        };
        plan.directories.sort();
        plan.directories.dedup();
        plan.digest = plan.compute_digest();
        Ok(plan)
    }

    #[must_use]
    pub fn launch_id(&self) -> &str {
        &self.launch_id
    }

    #[must_use]
    pub fn adapter_id(&self) -> &str {
        &self.adapter_id
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The arguments to add to the canonical launcher's command.
    #[must_use]
    pub fn extra_arguments(&self) -> &[OsString] {
        &self.extra_arguments
    }

    #[must_use]
    pub fn files(&self) -> &[RuntimeFile] {
        &self.files
    }

    fn compute_digest(&self) -> String {
        #[derive(Serialize)]
        struct View<'a> {
            launch_id: &'a str,
            adapter_id: &'a str,
            game_identity: &'a str,
            root: &'a Path,
            directories: &'a [PathBuf],
            files: Vec<(&'a Path, &'a str, RuntimeFileKind)>,
            extra_arguments: Vec<String>,
            sources: &'a [SourceBinding],
            expectations: &'a [LaunchStateExpectation],
            cheats: &'a [ReceiptCheat],
        }
        let view = View {
            launch_id: &self.launch_id,
            adapter_id: &self.adapter_id,
            game_identity: &self.game_identity,
            root: &self.root,
            directories: &self.directories,
            files: self
                .files
                .iter()
                .map(|f| (f.path.as_path(), f.sha256.as_str(), f.kind))
                .collect(),
            extra_arguments: self
                .extra_arguments
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            sources: &self.sources,
            expectations: &self.expectations,
            cheats: &self.cheats,
        };
        sha256_hex(&serde_json::to_vec(&view).unwrap_or_default())
    }

    fn verify_digest(&self) -> Result<(), RuntimeRefusal> {
        if self.compute_digest() == self.digest {
            Ok(())
        } else {
            Err(RuntimeRefusal::StalePlan)
        }
    }
}

// ---------------------------------------------------------------------
// Ownership marker and owner lease
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerState {
    Materialised,
    LaunchAttempted,
    EmulatorStarted,
    SpawnFailed,
    CleanupStarted,
}

/// Proof that EmuWiz created a workspace, for whom, and by which process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMarker {
    pub schema_version: u32,
    pub created_by: String,
    pub launch_id: String,
    pub game_identity: String,
    pub adapter_id: String,
    pub plan_digest: String,
    pub cheat_ids: Vec<String>,
    pub created_unix: u64,
    pub owner_pid: u32,
    pub owner_start_ticks: Option<u64>,
    pub state: MarkerState,
    pub emulator_pid: Option<u32>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn proc_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).is_dir()
}

/// Field 22 of `/proc/<pid>/stat`: stable for the life of a process, and
/// different for a later process that reuses the id.
fn start_ticks(pid: u32) -> Option<u64> {
    let text = fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).ok()?;
    let after = text.rsplit_once(')')?.1;
    after.split_whitespace().nth(19)?.parse().ok()
}

/// Whether the recorded owner is still the same live process. Unknown is
/// treated as alive: an uncertain owner is never swept.
fn owner_is_alive(pid: u32, recorded_ticks: Option<u64>) -> bool {
    if !proc_alive(pid) {
        return false;
    }
    match (recorded_ticks, start_ticks(pid)) {
        (Some(recorded), Some(now)) => recorded == now,
        _ => true,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hash_source(path: &Path) -> Result<String, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_SOURCE_HASH_BYTES
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_SOURCE_HASH_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| ())?;
    Ok(sha256_hex(&bytes))
}

// ---------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupError {
    OutsideApprovedRoot,
    NotAWorkspaceName,
    RootMissing,
    RootIsSymlink,
    NotADirectory,
    MarkerMissing,
    MarkerInvalid,
    WrongLaunch,
    Failed(String),
}

/// An EmuWiz-owned, per-launch directory.
#[derive(Debug)]
pub struct CheatRuntimeWorkspace {
    root: PathBuf,
    approved_root: PathBuf,
    marker: WorkspaceMarker,
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)
}

fn private_file(path: &Path) -> std::io::Result<fs::File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

fn write_marker_new(root: &Path, marker: &WorkspaceMarker) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(marker).map_err(std::io::Error::other)?;
    let mut file = private_file(&root.join(WORKSPACE_MARKER_NAME))?;
    file.write_all(&bytes)?;
    file.sync_all()
}

/// Replaces the marker atomically (new file, then rename) inside the workspace.
fn update_marker(root: &Path, marker: &WorkspaceMarker) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(marker).map_err(std::io::Error::other)?;
    let temporary = root.join(".emuwiz-cheat-runtime.json.tmp");
    let _ = fs::remove_file(&temporary);
    let mut file = private_file(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, root.join(WORKSPACE_MARKER_NAME))
}

fn read_marker(root: &Path) -> Result<WorkspaceMarker, CleanupError> {
    let path = root.join(WORKSPACE_MARKER_NAME);
    let metadata = fs::symlink_metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CleanupError::MarkerMissing
        } else {
            CleanupError::Failed(e.to_string())
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_MARKER_BYTES
    {
        return Err(CleanupError::MarkerInvalid);
    }
    let bytes = fs::read(&path).map_err(|e| CleanupError::Failed(e.to_string()))?;
    let marker: WorkspaceMarker =
        serde_json::from_slice(&bytes).map_err(|_| CleanupError::MarkerInvalid)?;
    if marker.schema_version != MARKER_SCHEMA_VERSION || marker.created_by != "emuwiz" {
        return Err(CleanupError::MarkerInvalid);
    }
    Ok(marker)
}

/// Checks everything that must be true before a directory may be removed, and
/// returns its marker. This is the only gate to deletion.
fn verify_owned(approved_root: &Path, root: &Path) -> Result<WorkspaceMarker, CleanupError> {
    if !normal_absolute(root) || !normal_absolute(approved_root) {
        return Err(CleanupError::OutsideApprovedRoot);
    }
    if root.parent() != Some(approved_root) {
        return Err(CleanupError::OutsideApprovedRoot);
    }
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(CleanupError::NotAWorkspaceName)?;
    let id = name
        .strip_prefix(WORKSPACE_DIR_PREFIX)
        .filter(|id| valid_launch_id(id))
        .ok_or(CleanupError::NotAWorkspaceName)?;
    let metadata = fs::symlink_metadata(root).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CleanupError::RootMissing
        } else {
            CleanupError::Failed(e.to_string())
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(CleanupError::RootIsSymlink);
    }
    if !metadata.is_dir() {
        return Err(CleanupError::NotADirectory);
    }
    let marker = read_marker(root)?;
    if marker.launch_id != id {
        return Err(CleanupError::WrongLaunch);
    }
    Ok(marker)
}

impl CheatRuntimeWorkspace {
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn marker(&self) -> &WorkspaceMarker {
        &self.marker
    }

    /// Validates a plan and creates its workspace. Fails closed: on any error
    /// everything created so far is removed and nothing is launched.
    pub fn materialise(
        plan: &CheatRuntimePlan,
        binding: &LiveLaunchBinding,
        roots: &CheatRuntimeRoots,
    ) -> Result<(Self, Vec<String>), MaterialiseError> {
        let refuse = |r: RuntimeRefusal| Err(MaterialiseError::Refused(r));
        if binding.game_identity != plan.game_identity {
            return refuse(RuntimeRefusal::GameMismatch);
        }
        if binding.adapter_id != plan.adapter_id {
            return refuse(RuntimeRefusal::EmulatorMismatch);
        }
        if let Err(r) = plan.verify_digest() {
            return refuse(r);
        }
        let approved = &roots.approved_root;
        if !normal_absolute(approved) || plan.root.parent() != Some(approved.as_path()) {
            return refuse(RuntimeRefusal::WorkspaceOutsideApprovedRoot);
        }
        // Cheat evidence must still be what the plan was built from.
        for source in &plan.sources {
            match hash_source(&source.path) {
                Ok(hash) if hash == source.expected_sha256 => {}
                Ok(_) => {
                    return refuse(RuntimeRefusal::CheatEvidenceChanged {
                        source: file_label(&source.path),
                    });
                }
                Err(()) => {
                    return refuse(RuntimeRefusal::CheatMaterialMissing {
                        source: file_label(&source.path),
                    });
                }
            }
        }
        // The approved root is EmuWiz's own temporary area. It may be created,
        // but never a symlink.
        match fs::symlink_metadata(approved) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
                return refuse(RuntimeRefusal::ApprovedRootUnsafe);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(approved)
                    .map_err(|e| MaterialiseError::Failed {
                        detail: e.to_string(),
                        rolled_back: true,
                    })?;
            }
            Err(e) => {
                return Err(MaterialiseError::Failed {
                    detail: e.to_string(),
                    rolled_back: true,
                });
            }
        }
        match fs::symlink_metadata(&plan.root) {
            Ok(_) => return refuse(RuntimeRefusal::WorkspaceExists),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(MaterialiseError::Failed {
                    detail: e.to_string(),
                    rolled_back: true,
                });
            }
        }

        let mut created: Vec<PathBuf> = Vec::new();
        let fail = |created: &mut Vec<PathBuf>, root: &Path, detail: String| {
            let rolled_back = rollback(created, root);
            MaterialiseError::Failed {
                detail,
                rolled_back,
            }
        };

        if let Err(e) = private_dir(&plan.root) {
            return Err(if e.kind() == std::io::ErrorKind::AlreadyExists {
                MaterialiseError::Refused(RuntimeRefusal::WorkspaceExists)
            } else {
                MaterialiseError::Failed {
                    detail: e.to_string(),
                    rolled_back: true,
                }
            });
        }
        created.push(plan.root.clone());

        let marker = WorkspaceMarker {
            schema_version: MARKER_SCHEMA_VERSION,
            created_by: "emuwiz".into(),
            launch_id: plan.launch_id.clone(),
            game_identity: plan.game_identity.clone(),
            adapter_id: plan.adapter_id.clone(),
            plan_digest: plan.digest.clone(),
            cheat_ids: plan
                .cheats
                .iter()
                .map(|c| format!("{}:{}", c.logical_id, c.variant_id))
                .collect(),
            created_unix: now_unix(),
            owner_pid: std::process::id(),
            owner_start_ticks: start_ticks(std::process::id()),
            state: MarkerState::Materialised,
            emulator_pid: None,
        };
        if let Err(e) = write_marker_new(&plan.root, &marker) {
            return Err(fail(&mut created, &plan.root, e.to_string()));
        }
        created.push(plan.root.join(WORKSPACE_MARKER_NAME));

        for directory in &plan.directories {
            // Parents first (sorted), each a real directory inside the workspace.
            let parent = directory.parent().unwrap_or(&plan.root);
            if !is_real_dir(parent) {
                return Err(fail(
                    &mut created,
                    &plan.root,
                    format!("parent of {} is not a real directory", directory.display()),
                ));
            }
            if let Err(e) = private_dir(directory) {
                return Err(fail(&mut created, &plan.root, e.to_string()));
            }
            created.push(directory.clone());
        }
        for file in &plan.files {
            let parent = file.path.parent().unwrap_or(&plan.root);
            if !is_real_dir(parent) {
                return Err(fail(
                    &mut created,
                    &plan.root,
                    format!("parent of {} is not a real directory", file.path.display()),
                ));
            }
            let result = private_file(&file.path).and_then(|mut f| {
                f.write_all(&file.bytes)?;
                f.sync_all()
            });
            if let Err(e) = result {
                return Err(fail(&mut created, &plan.root, e.to_string()));
            }
            created.push(file.path.clone());
            match fs::read(&file.path) {
                Ok(read) if sha256_hex(&read) == file.sha256 => {}
                _ => {
                    return Err(fail(
                        &mut created,
                        &plan.root,
                        format!("{} did not read back as written", file.path.display()),
                    ));
                }
            }
        }

        let names = created
            .iter()
            .skip(1)
            .filter_map(|p| p.strip_prefix(&plan.root).ok())
            .map(|p| p.display().to_string())
            .filter(|n| n != WORKSPACE_MARKER_NAME)
            .collect();
        Ok((
            Self {
                root: plan.root.clone(),
                approved_root: approved.clone(),
                marker,
            },
            names,
        ))
    }

    /// Records a state change in the marker so a crash leaves truthful
    /// recovery information.
    pub fn set_state(
        &mut self,
        state: MarkerState,
        emulator_pid: Option<u32>,
    ) -> std::io::Result<()> {
        // Never rewrite a marker whose ownership can no longer be proven.
        let on_disk = verify_owned(&self.approved_root, &self.root)
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        if on_disk.plan_digest != self.marker.plan_digest {
            return Err(std::io::Error::other("marker belongs to another plan"));
        }
        self.marker.state = state;
        if emulator_pid.is_some() {
            self.marker.emulator_pid = emulator_pid;
        }
        update_marker(&self.root, &self.marker)
    }

    /// Removes this workspace after verifying it is still the EmuWiz-owned
    /// directory it claims to be. Never called while the emulator may be
    /// using it; the session enforces that.
    pub fn cleanup(&self) -> Result<(), CleanupError> {
        let marker = verify_owned(&self.approved_root, &self.root)?;
        if marker.plan_digest != self.marker.plan_digest {
            return Err(CleanupError::WrongLaunch);
        }
        fs::remove_dir_all(&self.root).map_err(|e| CleanupError::Failed(e.to_string()))
    }
}

/// Receipt-driven rollback: removes only what was created, children first,
/// without recursion. Returns whether everything was removed.
fn rollback(created: &mut Vec<PathBuf>, root: &Path) -> bool {
    let mut clean = true;
    while let Some(path) = created.pop() {
        let result = match fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => fs::remove_dir(&path),
            Ok(_) => fs::remove_file(&path),
            Err(_) => Ok(()),
        };
        if result.is_err() {
            clean = false;
        }
    }
    let _ = root;
    clean
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------
// Executor and session
// ---------------------------------------------------------------------

/// The part of a spawned process the executor needs. The canonical launcher's
/// process type implements it; nothing here starts a process itself.
pub trait RuntimeProcess {
    fn pid(&self) -> u32;
    /// Non-blocking. `Some` once the process has exited.
    fn poll_exit(&mut self) -> Option<ExitSummary>;
}

/// One launch's live state: its receipt, workspace and process. Dropping it
/// never deletes anything; cleanup is explicit and happens after the emulator
/// exits, so a still-running emulator's files are never raced.
pub struct CheatRuntimeSession<P: RuntimeProcess> {
    receipt: CheatRuntimeReceipt,
    workspace: Option<CheatRuntimeWorkspace>,
    process: Option<P>,
    expectations: Vec<(LaunchStateExpectation, StateBaseline)>,
    finished: bool,
}

impl<P: RuntimeProcess> CheatRuntimeSession<P> {
    #[must_use]
    pub fn receipt(&self) -> &CheatRuntimeReceipt {
        &self.receipt
    }

    #[must_use]
    pub fn process(&self) -> Option<&P> {
        self.process.as_ref()
    }

    pub fn process_mut(&mut self) -> Option<&mut P> {
        self.process.as_mut()
    }

    /// Adds a plain-language note to the receipt.
    pub fn note(&mut self, text: impl AsRef<str>) {
        self.receipt.note(text);
    }

    /// True once there is nothing left to do for this launch.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Non-blocking; safe to call every frame. When the emulator has exited it
    /// runs the post-exit checks and cleanup exactly once.
    pub fn poll(&mut self) -> &CheatRuntimeReceipt {
        if !self.finished
            && let Some(process) = self.process.as_mut()
            && let Some(exit) = process.poll_exit()
        {
            if let Some(summary) = self.receipt.process.as_mut() {
                summary.exit = Some(exit);
            }
            self.finalise();
        }
        &self.receipt
    }

    /// Cleans up now if (and only if) the emulator is not running. While it is
    /// running the files are left in place.
    pub fn cleanup_now(&mut self) -> CleanupOutcome {
        if self.finished {
            return self.receipt.cleanup.clone();
        }
        if let Some(process) = self.process.as_mut() {
            if process.poll_exit().is_none() {
                self.receipt.cleanup = CleanupOutcome::DeferredEmulatorRunning;
                return self.receipt.cleanup.clone();
            }
        }
        self.finalise();
        self.receipt.cleanup.clone()
    }

    fn finalise(&mut self) {
        // Comparisons that must hold while the files still exist.
        self.run_checks(false);
        match self.workspace.take() {
            None => {
                if self.receipt.cleanup == CleanupOutcome::NotAttempted {
                    self.receipt.cleanup = CleanupOutcome::NotNeeded;
                }
            }
            Some(mut workspace) => {
                let _ = workspace.set_state(MarkerState::CleanupStarted, None);
                match workspace.cleanup() {
                    Ok(()) => {
                        self.receipt.cleanup = CleanupOutcome::Completed;
                        self.receipt.reach(CheatRuntimeStage::CleanedUp);
                    }
                    Err(error) => {
                        // Keep the workspace so a later explicit cleanup can retry.
                        self.receipt.cleanup = CleanupOutcome::Failed {
                            detail: format!("{error:?}"),
                        };
                        self.receipt.error(format!("cleanup failed: {error:?}"));
                        self.workspace = Some(workspace);
                    }
                }
            }
        }
        // Scratch material must be gone.
        self.run_checks(true);
        self.finished = true;
    }

    fn run_checks(&mut self, after_cleanup: bool) {
        for (expectation, baseline) in &self.expectations {
            let must_not_exist = expectation.expectation == StateExpectation::MustNotExistAfter;
            if must_not_exist != after_cleanup {
                continue;
            }
            let outcome = match verify_expectation(expectation, baseline) {
                StateOutcome::Unchanged => StateCheckOutcome::Unchanged,
                StateOutcome::Changed { .. } => StateCheckOutcome::ChangedAsExpected,
                StateOutcome::Violation { severity, detail } => {
                    if self.receipt.errors.len() < MAX_RECEIPT_ERRORS {
                        self.receipt.errors.push(
                            format!("{}: {detail}", file_label(&expectation.path))
                                .chars()
                                .take(MAX_ERROR_CHARS)
                                .collect(),
                        );
                    }
                    StateCheckOutcome::Violation { severity }
                }
            };
            self.receipt.state_checks.push(StateCheckRecord {
                label: file_label(&expectation.path),
                class: expectation.class,
                outcome,
            });
        }
    }
}

/// Runs one launch end to end: validate, baseline, materialise, hand the
/// prepared arguments to the canonical launcher, and keep the receipt.
///
/// `with_arguments` adds the plan's extra arguments to the canonical
/// launcher's already-built command, and `spawn` is that launcher's own spawn.
/// Nothing is started if materialisation fails.
pub fn execute_cheat_runtime<C, P: RuntimeProcess>(
    plan: &CheatRuntimePlan,
    binding: &LiveLaunchBinding,
    roots: &CheatRuntimeRoots,
    command: C,
    with_arguments: impl FnOnce(C, &[OsString]) -> C,
    spawn: impl FnOnce(C) -> Result<P, String>,
) -> CheatRuntimeSession<P> {
    let mut receipt = CheatRuntimeReceipt::new(plan);
    let mut session = CheatRuntimeSession {
        receipt: receipt.clone(),
        workspace: None,
        process: None,
        expectations: Vec::new(),
        finished: false,
    };

    // Fingerprint what must not change before anything is created.
    let expectations: Vec<(LaunchStateExpectation, StateBaseline)> = plan
        .expectations
        .iter()
        .map(|e| (e.clone(), capture_baseline(e)))
        .collect();

    let (mut workspace, created) = match CheatRuntimeWorkspace::materialise(plan, binding, roots) {
        Ok(ok) => ok,
        Err(MaterialiseError::Refused(reason)) => {
            receipt.error(format!("refused: {reason}"));
            receipt.materialisation = MaterialisationOutcome::Refused { reason };
            receipt.handoff = HandoffOutcome::NotReached;
            receipt.cleanup = CleanupOutcome::NotNeeded;
            session.receipt = receipt;
            session.finished = true;
            return session;
        }
        Err(MaterialiseError::Failed {
            detail,
            rolled_back,
        }) => {
            receipt.error(format!("materialisation failed: {detail}"));
            receipt.materialisation = MaterialisationOutcome::Failed {
                detail: detail.chars().take(MAX_ERROR_CHARS).collect(),
                rolled_back,
            };
            receipt.handoff = HandoffOutcome::NotReached;
            receipt.cleanup = if rolled_back {
                CleanupOutcome::NotNeeded
            } else {
                CleanupOutcome::Failed {
                    detail: "rollback incomplete; remaining files are EmuWiz-owned scratch".into(),
                }
            };
            session.receipt = receipt;
            session.finished = true;
            return session;
        }
    };
    receipt.workspace_root = Some(workspace.root().to_path_buf());
    receipt.materialisation = MaterialisationOutcome::Materialised { created };
    receipt.reach(CheatRuntimeStage::Materialised);

    // Recovery information must be truthful before anything can crash.
    let _ = workspace.set_state(MarkerState::LaunchAttempted, None);
    receipt.handoff = HandoffOutcome::NotAttempted;

    let command = with_arguments(command, plan.extra_arguments());
    match spawn(command) {
        Ok(process) => {
            let pid = process.pid();
            let _ = workspace.set_state(MarkerState::EmulatorStarted, Some(pid));
            receipt.handoff = HandoffOutcome::PassedToEmulator;
            receipt.reach(CheatRuntimeStage::PassedToEmulator);
            receipt.process = Some(ProcessSummary { pid, exit: None });
            receipt.reach(CheatRuntimeStage::ProcessStarted);
            // The in-game effect is not known and nothing here can find out.
            receipt.runtime_effect = RuntimeEffect::Unknown;
            receipt.reach(CheatRuntimeStage::RuntimeEffectUnknown);
            session.process = Some(process);
            session.workspace = Some(workspace);
            session.expectations = expectations;
            session.receipt = receipt;
        }
        Err(detail) => {
            let _ = workspace.set_state(MarkerState::SpawnFailed, None);
            receipt.handoff = HandoffOutcome::SpawnFailed {
                detail: detail.chars().take(MAX_ERROR_CHARS).collect(),
            };
            receipt.error(format!("spawn failed: {detail}"));
            session.workspace = Some(workspace);
            session.expectations = expectations;
            session.receipt = receipt;
            // Nothing ran, so there is nothing to wait for: clean up now.
            session.finalise();
        }
    }
    session
}

// ---------------------------------------------------------------------
// Stale workspaces (read-only detection + explicit cleanup)
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDisposition {
    /// EmuWiz-owned, its owner and emulator are gone: safe to clean up.
    OwnedStale,
    /// EmuWiz-owned and its owner or emulator is still running.
    OwnedActive,
    /// EmuWiz-owned but too new to judge.
    OwnedYoung,
    /// Not provably ours (no marker, bad marker, wrong name). Never touched.
    Foreign,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceScanEntry {
    pub path: PathBuf,
    pub disposition: WorkspaceDisposition,
    pub marker: Option<WorkspaceMarker>,
    pub detail: String,
}

/// Read-only. Lists what is under the approved root and says what each entry
/// is. Creates, changes and deletes nothing.
#[must_use]
pub fn scan_cheat_runtime_workspaces(
    roots: &CheatRuntimeRoots,
    now_unix_seconds: u64,
    grace_seconds: u64,
) -> Vec<WorkspaceScanEntry> {
    let approved = &roots.approved_root;
    let Ok(entries) = fs::read_dir(approved) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        // Only directories that look like cheat workspaces are considered at
        // all; BIOS projections and anything else stay invisible to this scan.
        if !name.starts_with(WORKSPACE_DIR_PREFIX) {
            continue;
        }
        let foreign = |detail: &str| WorkspaceScanEntry {
            path: path.clone(),
            disposition: WorkspaceDisposition::Foreign,
            marker: None,
            detail: detail.into(),
        };
        let marker = match verify_owned(approved, &path) {
            Ok(marker) => marker,
            Err(error) => {
                out.push(foreign(&format!("not provably EmuWiz-owned: {error:?}")));
                continue;
            }
        };
        let age = now_unix_seconds.saturating_sub(marker.created_unix);
        let (disposition, detail) = if owner_is_alive(marker.owner_pid, marker.owner_start_ticks) {
            (
                WorkspaceDisposition::OwnedActive,
                "owning EmuWiz process is running",
            )
        } else if marker.emulator_pid.is_some_and(|pid| proc_alive(pid)) {
            (
                WorkspaceDisposition::OwnedActive,
                "the emulator is still running",
            )
        } else if age < grace_seconds {
            (
                WorkspaceDisposition::OwnedYoung,
                "created too recently to judge",
            )
        } else {
            (
                WorkspaceDisposition::OwnedStale,
                "owner and emulator are gone",
            )
        };
        out.push(WorkspaceScanEntry {
            path,
            disposition,
            marker: Some(marker),
            detail: detail.into(),
        });
    }
    out
}

/// Explicitly removes one stale workspace. Re-verifies ownership and
/// staleness itself; an entry from an old scan is not trusted.
pub fn cleanup_stale_workspace(
    roots: &CheatRuntimeRoots,
    path: &Path,
    now_unix_seconds: u64,
    grace_seconds: u64,
) -> Result<(), CleanupError> {
    let marker = verify_owned(&roots.approved_root, path)?;
    let entry = scan_cheat_runtime_workspaces(roots, now_unix_seconds, grace_seconds)
        .into_iter()
        .find(|e| e.path == path && e.marker.as_ref() == Some(&marker));
    match entry.map(|e| e.disposition) {
        Some(WorkspaceDisposition::OwnedStale) => {
            fs::remove_dir_all(path).map_err(|e| CleanupError::Failed(e.to_string()))
        }
        _ => Err(CleanupError::Failed(
            "workspace is not provably stale; left in place".into(),
        )),
    }
}

#[cfg(test)]
mod tests;
