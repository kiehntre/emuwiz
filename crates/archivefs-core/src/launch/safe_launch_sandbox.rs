//! Opt-in, Linux small-media preservation copies, not an OS security sandbox.
//!
//! Adapters remain responsible for platform/readiness/executable validation and
//! for auditing their config contents and CLI (including absolute output paths).
//! Nothing here marks an emulator read-only, parses media, exports saves, or
//! changes existing adapters. Preparation must happen off the GUI render thread.

mod fs;
#[cfg(test)]
mod tests;

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::process_spawn::{
    CapturedFileIdentity, PreparedProcessCommand, ProcessExitReport, WatchedProcess,
    spawn_watched_process_isolated,
};
use crate::diagnostics::environment::{ResourceRole, StorageResource, assess_storage};

pub const MAX_MEMBERS: usize = 16;
pub const MAX_MEMBER_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_SET_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub const FAILED_RETENTION: Duration = Duration::from_secs(300);
const MAX_WORKSPACES: usize = 16;
const MARKER: &str = ".emuwiz-owned";
const LEASE: &str = ".lease";
const BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMediaSafety {
    /// Only for an independently audited read-only adapter, not an unaudited
    /// default. This copy API refuses it; legacy adapters need not declare it.
    DirectReadOnly,
    ScratchCopy,
    ScratchCopyWithIsolatedConfig,
    UnsafeUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigIsolation {
    None,
    XdgEnvironment,
    /// One literal argv flag, followed by one scratch config path. This is
    /// deliberately NOT an interpolated shell/template string.
    ExplicitConfigPath {
        flag: &'static str,
    },
    Combined {
        flag: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistentStatePolicy {
    /// Adapter has audited this mode as a disposable session. All new saves
    /// and disk changes are discarded; caller must explain that before launch.
    DisposableSession,
    /// Undeclared or persistent-save routing not implemented: refuse opt-in.
    NotYetHandled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchMediaSafetyDeclaration {
    pub safety: LaunchMediaSafety,
    pub config_isolation: ConfigIsolation,
    pub persistent_state: PersistentStatePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaRole {
    PrimaryMedia,
    SecondaryMedia,
    /// A read-only input the emulator consumes through the OWNED scratch
    /// config, not argv (for example a ROM named by a config key). The config
    /// seed must name its scratch path (`media/<scratch file name>`); `spawn`
    /// verifies that against the validated scratch config instead of
    /// requiring an argv token. Requires a `KnownProfile` seed.
    ConfigReferenced,
    Config,
    State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Floppy,
    Tape,
    Cartridge,
    Executable,
    /// System ROM/BIOS images attached by an adapter's explicit profile.
    Firmware,
    /// Refused even when small: there is no HDD/optical launch contract in V1.
    HardDisk,
    Optical,
    ReferenceManifest,
}

#[derive(Debug, Clone)]
pub struct MediaMember {
    pub source: PathBuf,
    pub role: MediaRole,
    pub kind: MediaKind,
    /// Adapter-declared suffix for scratch naming, NOT platform evidence.
    pub suffix: String,
}

#[derive(Debug, Clone)]
pub enum ProfileSeed {
    Empty,
    /// Caller attests this is a reviewed seed, not arbitrary live emulator
    /// settings. Absolute writable paths in seed contents must be audited by
    /// that adapter; generic config parsing would be unsafe here.
    KnownProfile {
        source: PathBuf,
        scratch_relative: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProvenance {
    pub original_path: PathBuf,
    pub original_identity: CapturedFileIdentity,
    pub sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScratchPathMapping {
    pub role: MediaRole,
    pub original: SourceProvenance,
    pub scratch_path: PathBuf,
    pub scratch_identity: CapturedFileIdentity,
}

#[derive(Debug, Clone)]
struct PlannedMember {
    role: MediaRole,
    source: SourceProvenance,
    name: OsString,
}

#[derive(Debug, Clone)]
pub struct SandboxPlan {
    declaration: LaunchMediaSafetyDeclaration,
    members: Vec<PlannedMember>,
    config_name: OsString,
    total_bytes: u64,
}

impl SandboxPlan {
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
    pub fn sources(&self) -> impl Iterator<Item = &SourceProvenance> {
        self.members.iter().map(|m| &m.source)
    }
    pub fn declaration(&self) -> LaunchMediaSafetyDeclaration {
        self.declaration
    }
    pub fn revalidate_sources(&self) -> Result<(), SafeLaunchSandboxError> {
        for member in &self.members {
            verify_source(&member.source)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationFailureReason {
    SourceIdentityDrifted,
    SourceContentDrifted,
    ScratchContentMismatch,
}

#[derive(Debug)]
pub enum SafeLaunchSandboxError {
    UnsafeMediaPolicy {
        reason: &'static str,
    },
    UnsafePath {
        path: PathBuf,
        source: io::Error,
    },
    ScratchSpaceUnavailable {
        workspace_root: PathBuf,
        source: io::Error,
    },
    InsufficientTemporarySpace {
        required_bytes: u64,
        available_bytes: u64,
    },
    PreparationBusy {
        workspace_root: PathBuf,
    },
    ScratchCopyFailed {
        member: PathBuf,
        source: io::Error,
    },
    ScratchVerificationFailed {
        member: PathBuf,
        reason: VerificationFailureReason,
    },
    ConfigIsolationFailed {
        reason: &'static str,
    },
    CleanupFailed {
        workspace_root: PathBuf,
        source: io::Error,
    },
    SpawnFailed(io::Error),
}

impl std::fmt::Display for SafeLaunchSandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreparationBusy { .. } => write!(
                f,
                "Another protected launch is preparing copies. Retry when it finishes; original media will not be launched."
            ),
            Self::InsufficientTemporarySpace {
                required_bytes,
                available_bytes,
            } => write!(
                f,
                "Protected launch needs {required_bytes} temporary bytes; only {available_bytes} available. Original media will not be launched."
            ),
            _ => write!(
                f,
                "Protected launch refused: {self:?}. No fallback to original media."
            ),
        }
    }
}
impl std::error::Error for SafeLaunchSandboxError {}

fn policy(reason: &'static str) -> SafeLaunchSandboxError {
    SafeLaunchSandboxError::UnsafeMediaPolicy { reason }
}
fn unsafe_path(path: &Path, source: io::Error) -> SafeLaunchSandboxError {
    SafeLaunchSandboxError::UnsafePath {
        path: path.to_owned(),
        source,
    }
}
fn drift(path: &Path, reason: VerificationFailureReason) -> SafeLaunchSandboxError {
    SafeLaunchSandboxError::ScratchVerificationFailed {
        member: path.to_owned(),
        reason,
    }
}

fn open_source(path: &Path) -> Result<File, SafeLaunchSandboxError> {
    let file = fs::absolute(path, false).map_err(|e| unsafe_path(path, e))?;
    if !file.metadata().map_err(|e| unsafe_path(path, e))?.is_file() {
        return Err(policy("source must be a regular file"));
    }
    Ok(file)
}

fn digest(file: &mut File, size: u64) -> io::Result<[u8; 32]> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut remaining = size;
    let mut buffer = [0; BUFFER_BYTES];
    while remaining != 0 {
        let request = remaining.min(BUFFER_BYTES as u64) as usize;
        file.read_exact(&mut buffer[..request])?;
        hash.update(&buffer[..request]);
        remaining -= request as u64;
    }
    let mut extra = [0];
    if file.read(&mut extra)? != 0 {
        return Err(io::Error::other("source grew during bounded read"));
    }
    Ok(hash.finalize().into())
}

fn capture(path: &Path, limit: u64) -> Result<SourceProvenance, SafeLaunchSandboxError> {
    let mut file = open_source(path)?;
    let identity =
        CapturedFileIdentity::capture(&file.metadata().map_err(|e| unsafe_path(path, e))?);
    if identity.size == 0 || identity.size > limit {
        return Err(policy("empty or oversized V1 member"));
    }
    let sha256 = digest(&mut file, identity.size).map_err(|e| unsafe_path(path, e))?;
    let source = SourceProvenance {
        original_path: path.to_owned(),
        original_identity: identity,
        sha256,
    };
    let reopened = open_source(path)?;
    if CapturedFileIdentity::capture(&reopened.metadata().map_err(|e| unsafe_path(path, e))?)
        != identity
    {
        return Err(drift(
            path,
            VerificationFailureReason::SourceIdentityDrifted,
        ));
    }
    Ok(source)
}

fn verify_source(source: &SourceProvenance) -> Result<(), SafeLaunchSandboxError> {
    let observed = capture(&source.original_path, MAX_MEMBER_BYTES)?;
    if observed.original_identity != source.original_identity {
        return Err(drift(
            &source.original_path,
            VerificationFailureReason::SourceIdentityDrifted,
        ));
    }
    if observed.sha256 != source.sha256 {
        return Err(drift(
            &source.original_path,
            VerificationFailureReason::SourceContentDrifted,
        ));
    }
    Ok(())
}

/// Read-only planning. No extension identity or companion discovery. A caller
/// must already know the complete bounded media set and its platform.
pub fn plan_sandbox(
    declaration: LaunchMediaSafetyDeclaration,
    members: &[MediaMember],
    seed: ProfileSeed,
) -> Result<SandboxPlan, SafeLaunchSandboxError> {
    if !matches!(
        declaration.safety,
        LaunchMediaSafety::ScratchCopy | LaunchMediaSafety::ScratchCopyWithIsolatedConfig
    ) {
        return Err(policy(
            "this entry point only prepares explicitly opted-in scratch launches",
        ));
    }
    if declaration.persistent_state != PersistentStatePolicy::DisposableSession {
        return Err(policy(
            "persistent save routing has not been audited/implemented",
        ));
    }
    if (declaration.safety == LaunchMediaSafety::ScratchCopy)
        != (declaration.config_isolation == ConfigIsolation::None)
    {
        return Err(SafeLaunchSandboxError::ConfigIsolationFailed {
            reason: "safety and config isolation declarations disagree",
        });
    }
    let flag = match declaration.config_isolation {
        ConfigIsolation::ExplicitConfigPath { flag } | ConfigIsolation::Combined { flag } => {
            Some(flag)
        }
        _ => None,
    };
    if flag.is_some_and(|flag| {
        !flag.starts_with('-')
            || flag.len() > 64
            || !flag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    }) {
        return Err(SafeLaunchSandboxError::ConfigIsolationFailed {
            reason: "config flag must be one literal, bounded argv token",
        });
    }
    if members.is_empty()
        || members.len() > MAX_MEMBERS
        || members
            .iter()
            .filter(|m| m.role == MediaRole::PrimaryMedia)
            .count()
            != 1
    {
        return Err(policy(
            "explicit bounded member list with exactly one primary is required",
        ));
    }
    let mut plan = SandboxPlan {
        declaration,
        members: Vec::new(),
        config_name: "profile.cfg".into(),
        total_bytes: 0,
    };
    for (index, member) in members.iter().enumerate() {
        if !matches!(
            member.kind,
            MediaKind::Floppy
                | MediaKind::Tape
                | MediaKind::Cartridge
                | MediaKind::Executable
                | MediaKind::Firmware
        ) || !matches!(
            member.role,
            MediaRole::PrimaryMedia | MediaRole::SecondaryMedia | MediaRole::ConfigReferenced
        ) {
            return Err(policy(
                "HDD, optical, reference manifests and implicit state members are not supported in V1",
            ));
        }
        if member.suffix.is_empty()
            || member.suffix.len() > 8
            || !member
                .suffix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(policy("invalid declared scratch suffix"));
        }
        // Refuse common reference-bearing types even if mislabeled by a caller.
        if ["cue", "m3u", "gdi", "ccd"].contains(&member.suffix.as_str()) {
            return Err(policy(
                "reference-bearing manifests require a separate audited rebase contract",
            ));
        }
        let source = capture(&member.source, MAX_MEMBER_BYTES)?;
        if plan.members.iter().any(|m| {
            m.source.original_identity.device == source.original_identity.device
                && m.source.original_identity.inode == source.original_identity.inode
        }) {
            return Err(policy("duplicate source member"));
        }
        plan.total_bytes += source.original_identity.size;
        if plan.total_bytes > MAX_SET_BYTES {
            return Err(policy("whole-media-set V1 size cap exceeded"));
        }
        let prefix = if member.role == MediaRole::PrimaryMedia {
            "primary"
        } else {
            "secondary"
        };
        plan.members.push(PlannedMember {
            role: member.role,
            source,
            name: format!("{prefix}-{index:02}.{}", member.suffix).into(),
        });
    }
    if matches!(seed, ProfileSeed::Empty)
        && members
            .iter()
            .any(|member| member.role == MediaRole::ConfigReferenced)
    {
        return Err(policy(
            "config-referenced members require a reviewed config seed that names them",
        ));
    }
    if let ProfileSeed::KnownProfile {
        source,
        scratch_relative,
    } = seed
    {
        if declaration.config_isolation == ConfigIsolation::None {
            return Err(policy("config seed requires config isolation"));
        }
        let Some(name) = scratch_relative.to_str() else {
            return Err(policy("seed destination must be a simple bounded name"));
        };
        if name.is_empty()
            || name.len() > 64
            || name.starts_with('.')
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(policy("config seed path escape refused"));
        }
        plan.config_name = name.into();
        let source = capture(&source, MAX_CONFIG_BYTES)?;
        plan.total_bytes += source.original_identity.size;
        if plan.total_bytes > MAX_SET_BYTES {
            return Err(policy("media plus config exceeds V1 size cap"));
        }
        plan.members.push(PlannedMember {
            role: MediaRole::Config,
            source,
            name: plan.config_name.clone(),
        });
    }
    plan.revalidate_sources()?;
    Ok(plan)
}

#[derive(Debug, Serialize, Deserialize)]
struct OwnershipMarker {
    version: u32,
    transaction: String,
    uid: u32,
    device: u64,
    inode: u64,
    created: u64,
    phase: WorkspacePhase,
}

#[derive(Debug, Serialize, Deserialize)]
enum WorkspacePhase {
    Preparing,
    SpawnIntent,
    Running(u32),
    Failed { expires: u64 },
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn valid_id(id: &str) -> bool {
    let Some((time, random)) = id.split_once('-') else {
        return false;
    };
    !time.is_empty()
        && time.len() <= 20
        && time.bytes().all(|b| b.is_ascii_digit())
        && random.len() == 16
        && random
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn transaction_id() -> io::Result<String> {
    let mut random = [0; 8];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{}-{suffix}", now()))
}

#[derive(Debug, Default)]
pub struct CleanupReport {
    pub removed: Vec<PathBuf>,
    pub skipped: Vec<(PathBuf, String)>,
}

struct ManagerInner {
    root: PathBuf,
    directory: File,
}

#[derive(Clone)]
pub struct SandboxManager {
    inner: Arc<ManagerInner>,
}

impl SandboxManager {
    /// Creates only emuwiz/launch under an existing, explicit base. Refuses
    /// preexisting nonprivate or symlinked directories; never chmods them.
    /// Startup cleanup is scoped to this root and returned for diagnostics.
    pub fn open(base: &Path) -> Result<(Self, CleanupReport), SafeLaunchSandboxError> {
        let init = || -> io::Result<Self> {
            let parent = fs::absolute(base, true)?;
            let emuwiz = fs::mkdir(&parent, OsStr::new("emuwiz"), true)?;
            let directory = fs::mkdir(&emuwiz, OsStr::new("launch"), true)?;
            // Establish required cleanup syscall support before admitting any
            // workspace, not for the first time after an emulator exits.
            fs::cleanup_directory(&emuwiz, OsStr::new("launch"))?;
            Ok(Self {
                inner: Arc::new(ManagerInner {
                    root: base.join("emuwiz/launch"),
                    directory,
                }),
            })
        };
        let manager = init().map_err(|source| SafeLaunchSandboxError::ScratchSpaceUnavailable {
            workspace_root: base.to_owned(),
            source,
        })?;
        let report = manager.startup_cleanup();
        Ok((manager, report))
    }

    pub fn from_environment() -> Result<(Self, CleanupReport), SafeLaunchSandboxError> {
        if let Some(base) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
            let base = PathBuf::from(base);
            match fs::absolute(&base, true) {
                Ok(file) => {
                    fs::private(&file, true).map_err(|e| unsafe_path(&base, e))?;
                    return Self::open(&base);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(unsafe_path(&base, e)),
            }
        }
        let base = std::env::var_os("TMPDIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self::open(&base)
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    fn validate_root(&self) -> io::Result<()> {
        let reopened = fs::absolute(self.root(), true)?;
        fs::private(&reopened, true)?;
        if !same_directory(&reopened, &self.inner.directory)? {
            return Err(io::Error::other("workspace root replaced"));
        }
        Ok(())
    }

    pub fn prepare(&self, plan: &SandboxPlan) -> Result<PreparedSandbox, SafeLaunchSandboxError> {
        self.prepare_with_capacity(plan, |root| {
            assess_storage(&[StorageResource::new(ResourceRole::TransactionStorage, root)])
                .filesystems
                .iter()
                .find_map(|f| f.stat.map(|s| s.available_bytes))
        })
    }

    /// Adapter integration tests exercise the real whole-set preflight without
    /// filling a host filesystem. No production capacity override is exposed.
    #[cfg(test)]
    pub(crate) fn prepare_with_test_capacity(
        &self,
        plan: &SandboxPlan,
        available: u64,
    ) -> Result<PreparedSandbox, SafeLaunchSandboxError> {
        self.prepare_with_capacity(plan, |_| Some(available))
    }

    fn prepare_with_capacity(
        &self,
        plan: &SandboxPlan,
        capacity: impl FnOnce(&Path) -> Option<u64>,
    ) -> Result<PreparedSandbox, SafeLaunchSandboxError> {
        self.validate_root()
            .map_err(|e| unsafe_path(self.root(), e))?;
        // Serialises preparation across processes using this root, so copies
        // from two local requests cannot both preflight the same free space.
        let preparation_lock = fs::child(
            &self.inner.directory,
            OsStr::new(".prepare-lock"),
            libc::O_RDWR | libc::O_CREAT,
        )
        .map_err(|e| unsafe_path(self.root(), e))?;
        fs::private(&preparation_lock, false).map_err(|e| unsafe_path(self.root(), e))?;
        // Another fork can briefly inherit an unrelated CLOEXEC descriptor
        // until exec, even after its parent finished preparing. Tolerate that
        // transient lease, but never wait indefinitely for a genuine owner.
        let deadline = std::time::Instant::now() + Duration::from_millis(100);
        loop {
            match fs::lock(&preparation_lock) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return Err(SafeLaunchSandboxError::PreparationBusy {
                            workspace_root: self.root().to_owned(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => return Err(unsafe_path(self.root(), e)),
            }
        }
        plan.revalidate_sources()?;
        let existing =
            fs::entries(&self.inner.directory, 128).map_err(|e| unsafe_path(self.root(), e))?;
        if existing
            .iter()
            .filter(|v| v.to_str().is_some_and(valid_id))
            .count()
            >= MAX_WORKSPACES
        {
            return Err(policy(
                "too many retained/active workspaces; inspect cleanup report before retrying",
            ));
        }
        let required_bytes = plan.total_bytes + (plan.total_bytes / 10).max(1024 * 1024);
        let available_bytes = capacity(self.root()).ok_or_else(|| {
            SafeLaunchSandboxError::ScratchSpaceUnavailable {
                workspace_root: self.root().to_owned(),
                source: io::Error::other("temporary capacity unavailable"),
            }
        })?;
        if available_bytes < required_bytes {
            return Err(SafeLaunchSandboxError::InsufficientTemporarySpace {
                required_bytes,
                available_bytes,
            });
        }
        // No transaction or destination media exists before the whole-set guard.
        let workspace = Workspace::create(self.clone()).map_err(|source| {
            SafeLaunchSandboxError::ScratchSpaceUnavailable {
                workspace_root: self.root().to_owned(),
                source,
            }
        })?;
        let mut prepared = PreparedSandbox {
            workspace: Some(workspace),
            plan: plan.clone(),
            mappings: Vec::new(),
            subdirectories: Vec::new(),
        };
        let result = prepared.copy_members();
        if let Err(error) = result {
            // Cleanup is explicit so a cleanup failure is not hidden by Drop.
            prepared.cleanup()?;
            return Err(error);
        }
        Ok(prepared)
    }

    /// Safe startup/Doctor sweep. Never adopts arbitrary directories, old
    /// empty markers, invalid records, symlinks, locked or live-child roots.
    pub fn startup_cleanup(&self) -> CleanupReport {
        let mut report = CleanupReport::default();
        if let Err(e) = self.validate_root() {
            report.skipped.push((self.root().to_owned(), e.to_string()));
            return report;
        }
        let entries = match fs::entries(&self.inner.directory, 128) {
            Ok(v) => v,
            Err(e) => {
                report.skipped.push((self.root().to_owned(), e.to_string()));
                return report;
            }
        };
        for name in entries {
            if name == ".prepare-lock" {
                continue;
            }
            let path = self.root().join(&name);
            let attempt = (|| -> io::Result<()> {
                let id = name
                    .to_str()
                    .filter(|s| valid_id(s))
                    .ok_or_else(|| io::Error::other("unowned name"))?;
                let mut workspace = Workspace::open(self.clone(), id)?;
                match workspace.marker.phase {
                    WorkspacePhase::SpawnIntent => {
                        return Err(io::Error::other(
                            "interrupted spawn; process ownership uncertain, manual inspection required",
                        ));
                    }
                    WorkspacePhase::Running(pid) if process_exists(pid) => {
                        return Err(io::Error::other("child may still be running"));
                    }
                    _ => {}
                }
                let deadline = match workspace.marker.phase {
                    WorkspacePhase::Failed { expires } => expires,
                    _ => workspace
                        .marker
                        .created
                        .saturating_add(FAILED_RETENTION.as_secs()),
                };
                if now() < deadline {
                    return Err(io::Error::other("retention window not expired"));
                }
                workspace.remove()
            })();
            match attempt {
                Ok(()) => report.removed.push(path),
                Err(e) => report.skipped.push((path, e.to_string())),
            }
        }
        report
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn process_exists(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return true;
    }
    // SAFETY: signal 0 probes existence only, no signal is delivered.
    unsafe {
        libc::kill(pid as i32, 0) == 0
            || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}
fn same_directory(a: &File, b: &File) -> io::Result<bool> {
    let (a, b) = (a.metadata()?, b.metadata()?);
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

struct Workspace {
    manager: SandboxManager,
    path: PathBuf,
    directory: File,
    lease: File,
    marker: OwnershipMarker,
}

impl Workspace {
    fn create(manager: SandboxManager) -> io::Result<Self> {
        let id = transaction_id()?;
        let directory = fs::mkdir(&manager.inner.directory, OsStr::new(&id), false)?;
        let result = (|| {
            let lease = fs::create(&directory, OsStr::new(LEASE))?;
            fs::lock(&lease)?;
            let metadata = directory.metadata()?;
            let marker = OwnershipMarker {
                version: 1,
                transaction: id.clone(),
                uid: metadata.uid(),
                device: metadata.dev(),
                inode: metadata.ino(),
                created: now(),
                phase: WorkspacePhase::Preparing,
            };
            let mut file = fs::create(&directory, OsStr::new(MARKER))?;
            file.write_all(&serde_json::to_vec(&marker)?)?;
            file.sync_all()?;
            Ok((lease, marker))
        })();
        match result {
            Ok((lease, marker)) => Ok(Self {
                path: manager.root().join(&id),
                manager,
                directory,
                lease,
                marker,
            }),
            Err(e) => {
                // We just created this root via exclusive mkdirat and still
                // hold its descriptor; no unowned path is recursively removed.
                let _ = fs::unlink(&directory, OsStr::new(MARKER), false);
                let _ = fs::unlink(&directory, OsStr::new(LEASE), false);
                let _ = fs::unlink(&manager.inner.directory, OsStr::new(&id), true);
                Err(e)
            }
        }
    }

    fn open(manager: SandboxManager, id: &str) -> io::Result<Self> {
        let directory = fs::cleanup_directory(&manager.inner.directory, OsStr::new(id))?;
        fs::private(&directory, true)?;
        let lease = fs::child(&directory, OsStr::new(LEASE), libc::O_RDWR)?;
        fs::private(&lease, false)?;
        fs::lock(&lease)?;
        let marker = Self::read_marker(&directory)?;
        let workspace = Self {
            path: manager.root().join(id),
            manager,
            directory,
            lease,
            marker,
        };
        workspace.validate()?;
        Ok(workspace)
    }

    fn read_marker(directory: &File) -> io::Result<OwnershipMarker> {
        let file = fs::child(directory, OsStr::new(MARKER), libc::O_RDONLY)?;
        fs::private(&file, false)?;
        if file.metadata()?.len() > 4096 {
            return Err(io::Error::other("oversized ownership marker"));
        }
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }

    fn validate(&self) -> io::Result<()> {
        self.manager.validate_root()?;
        let reopened = fs::cleanup_directory(
            &self.manager.inner.directory,
            self.path
                .file_name()
                .ok_or_else(|| io::Error::other("missing transaction name"))?,
        )?;
        fs::private(&reopened, true)?;
        if !same_directory(&reopened, &self.directory)? {
            return Err(io::Error::other("workspace replaced"));
        }
        let lease = fs::child(&self.directory, OsStr::new(LEASE), libc::O_RDONLY)?;
        fs::private(&lease, false)?;
        if !same_directory(&lease, &self.lease)? {
            return Err(io::Error::other("workspace lease replaced"));
        }
        let marker = Self::read_marker(&self.directory)?;
        let metadata = self.directory.metadata()?;
        if marker.version != 1
            || !valid_id(&marker.transaction)
            || self.path.file_name() != Some(OsStr::new(&marker.transaction))
            || marker.uid != metadata.uid()
            || marker.device != metadata.dev()
            || marker.inode != metadata.ino()
        {
            return Err(io::Error::other("unowned or replaced workspace marker"));
        }
        Ok(())
    }

    fn phase(&mut self, phase: WorkspacePhase) -> io::Result<()> {
        self.validate()?;
        self.marker.phase = phase;
        let mut file = fs::create(&self.directory, OsStr::new(".marker-next"))?;
        file.write_all(&serde_json::to_vec(&self.marker)?)?;
        file.sync_all()?;
        fs::replace_marker(
            &self.directory,
            OsStr::new(".marker-next"),
            OsStr::new(MARKER),
        )
    }

    fn remove(&mut self) -> io::Result<()> {
        self.validate()?;
        fs::clear(&self.directory, self.marker.device, 0, &mut 4096)?;
        self.validate()?;
        // Preserve ownership proof until all content is removed successfully.
        fs::unlink(&self.directory, OsStr::new(MARKER), false)?;
        fs::unlink(&self.directory, OsStr::new(LEASE), false)?;
        fs::unlink(
            &self.manager.inner.directory,
            OsStr::new(&self.marker.transaction),
            true,
        )
    }
}

pub struct PreparedSandbox {
    workspace: Option<Workspace>,
    plan: SandboxPlan,
    mappings: Vec<ScratchPathMapping>,
    subdirectories: Vec<(String, File)>,
}

impl PreparedSandbox {
    pub fn mappings(&self) -> &[ScratchPathMapping] {
        &self.mappings
    }
    pub fn original_plan(&self) -> &SandboxPlan {
        &self.plan
    }
    pub fn workspace_path(&self) -> &Path {
        &self.workspace.as_ref().expect("owned until consumed").path
    }

    fn copy_members(&mut self) -> Result<(), SafeLaunchSandboxError> {
        let workspace = self.workspace.as_ref().expect("owned");
        for name in ["media", "config", "data", "cache", "state"] {
            let directory = fs::mkdir(&workspace.directory, OsStr::new(name), false)
                .map_err(|e| unsafe_path(&workspace.path, e))?;
            self.subdirectories.push((name.to_owned(), directory));
        }
        for member in &self.plan.members {
            let subdir = if member.role == MediaRole::Config {
                "config"
            } else {
                "media"
            };
            let directory = fs::directory(&workspace.directory, OsStr::new(subdir))
                .map_err(|e| unsafe_path(&workspace.path, e))?;
            let dest_path = workspace.path.join(subdir).join(&member.name);
            let result = (|| -> Result<(), SafeLaunchSandboxError> {
                verify_source(&member.source)?;
                let mut source = open_source(&member.source.original_path)?;
                if CapturedFileIdentity::capture(
                    &source
                        .metadata()
                        .map_err(|e| unsafe_path(&member.source.original_path, e))?,
                ) != member.source.original_identity
                {
                    return Err(drift(
                        &member.source.original_path,
                        VerificationFailureReason::SourceIdentityDrifted,
                    ));
                }
                let mut copy = || -> io::Result<()> {
                    let mut dest = fs::create(&directory, &member.name)?;
                    let mut remaining = member.source.original_identity.size;
                    let mut buffer = [0; BUFFER_BYTES];
                    while remaining > 0 {
                        let count = remaining.min(BUFFER_BYTES as u64) as usize;
                        source.read_exact(&mut buffer[..count])?;
                        dest.write_all(&buffer[..count])?;
                        remaining -= count as u64;
                    }
                    dest.sync_all()?;
                    if digest(&mut dest, member.source.original_identity.size)?
                        != member.source.sha256
                    {
                        return Err(io::Error::other("scratch hash mismatch"));
                    }
                    Ok(())
                };
                copy().map_err(|source| SafeLaunchSandboxError::ScratchCopyFailed {
                    member: dest_path.clone(),
                    source,
                })?;
                verify_source(&member.source)?;
                Ok(())
            })();
            result?;
            let scratch = open_source(&dest_path)?;
            fs::private(&scratch, false).map_err(|e| unsafe_path(&dest_path, e))?;
            let scratch_identity = CapturedFileIdentity::capture(
                &scratch.metadata().map_err(|e| unsafe_path(&dest_path, e))?,
            );
            self.mappings.push(ScratchPathMapping {
                role: member.role,
                original: member.source.clone(),
                scratch_path: dest_path,
                scratch_identity,
            });
        }
        self.revalidate()
    }

    /// Required again immediately before spawning, including scratch hashes.
    pub fn revalidate(&self) -> Result<(), SafeLaunchSandboxError> {
        let workspace = self.workspace.as_ref().expect("owned");
        workspace
            .validate()
            .map_err(|e| unsafe_path(&workspace.path, e))?;
        for (name, expected) in &self.subdirectories {
            let directory = fs::directory(&workspace.directory, OsStr::new(name))
                .map_err(|e| unsafe_path(&workspace.path, e))?;
            fs::private(&directory, true).map_err(|e| unsafe_path(&workspace.path, e))?;
            if !same_directory(&directory, expected).map_err(|e| unsafe_path(&workspace.path, e))? {
                return Err(policy("workspace subdirectory replaced"));
            }
        }
        self.plan.revalidate_sources()?;
        for mapping in &self.mappings {
            let scratch = open_source(&mapping.scratch_path)?;
            fs::private(&scratch, false).map_err(|e| unsafe_path(&mapping.scratch_path, e))?;
            let observed = capture(&mapping.scratch_path, MAX_MEMBER_BYTES)?;
            if observed.sha256 != mapping.original.sha256
                || observed.original_identity != mapping.scratch_identity
                || (observed.original_identity.device == mapping.original.original_identity.device
                    && observed.original_identity.inode == mapping.original.original_identity.inode)
            {
                return Err(drift(
                    &mapping.scratch_path,
                    VerificationFailureReason::ScratchContentMismatch,
                ));
            }
        }
        Ok(())
    }

    /// Bytes of the validated scratch config (empty without a seed), bounded.
    fn scratch_config_text(&self) -> Result<Vec<u8>, SafeLaunchSandboxError> {
        let Some(mapping) = self.mappings.iter().find(|m| m.role == MediaRole::Config) else {
            return Ok(Vec::new());
        };
        let file = open_source(&mapping.scratch_path)?;
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| unsafe_path(&mapping.scratch_path, e))?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(policy("scratch config exceeds its bound"));
        }
        Ok(bytes)
    }

    /// Typed config arguments and environment for adapters. No shell strings.
    pub fn config_arguments(&self) -> Vec<OsString> {
        match self.plan.declaration.config_isolation {
            ConfigIsolation::ExplicitConfigPath { flag } | ConfigIsolation::Combined { flag } => {
                vec![
                    flag.into(),
                    self.workspace_path()
                        .join("config")
                        .join(&self.plan.config_name)
                        .into_os_string(),
                ]
            }
            _ => Vec::new(),
        }
    }

    /// The per-child environment `spawn` applies. Exposed to the crate so
    /// real-emulator smoke tests can replay exactly what a launch would see.
    pub(crate) fn environment(&self) -> Vec<(OsString, OsString)> {
        if !matches!(
            self.plan.declaration.config_isolation,
            ConfigIsolation::XdgEnvironment | ConfigIsolation::Combined { .. }
        ) {
            return Vec::new();
        }
        [
            ("HOME", ""),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_STATE_HOME", "state"),
            ("TMPDIR", "cache"),
        ]
        .into_iter()
        .map(|(key, path)| {
            (
                key.into(),
                self.workspace_path().join(path).into_os_string(),
            )
        })
        .collect()
    }

    pub fn cleanup(mut self) -> Result<(), SafeLaunchSandboxError> {
        let mut workspace = self.workspace.take().expect("owned");
        workspace
            .remove()
            .map_err(|source| SafeLaunchSandboxError::CleanupFailed {
                workspace_root: workspace.path,
                source,
            })
    }

    /// Adapter constructs argv from the mappings, after its own fresh platform,
    /// executable, firmware and profile checks. This API checks that every media
    /// member is attached as a separate scratch argv token, refuses source argv,
    /// sets private cwd/config env and owns cleanup until the watched child exits.
    pub fn spawn(
        mut self,
        mut command: PreparedProcessCommand,
    ) -> Result<SandboxedProcess, SafeLaunchSandboxError> {
        self.revalidate()?;
        let config = self.config_arguments();
        // The validated scratch config is the only way a config-referenced
        // member reaches the emulator, so it must name every one of them and
        // must never name an original path.
        let config_text = self.scratch_config_text()?;
        for mapping in &self.mappings {
            if command
                .arguments
                .iter()
                .any(|a| Path::new(a) == mapping.original.original_path)
            {
                return Err(policy("source path in scratch launch argv"));
            }
            if mapping.original.original_path.as_os_str().len() > 1
                && contains(
                    &config_text,
                    mapping
                        .original
                        .original_path
                        .as_os_str()
                        .as_encoded_bytes(),
                )
            {
                return Err(policy("source path in scratch config"));
            }
            match mapping.role {
                MediaRole::Config | MediaRole::State => {}
                MediaRole::ConfigReferenced => {
                    let name = mapping
                        .scratch_path
                        .file_name()
                        .ok_or_else(|| policy("scratch member has no name"))?;
                    let mut reference = b"media/".to_vec();
                    reference.extend_from_slice(name.as_encoded_bytes());
                    if !contains(&config_text, &reference) {
                        return Err(policy(
                            "config-referenced member is not named by the config",
                        ));
                    }
                    if command
                        .arguments
                        .iter()
                        .any(|a| Path::new(a) == mapping.scratch_path)
                    {
                        return Err(policy("config-referenced member must not also be argv"));
                    }
                }
                _ => {
                    if !command
                        .arguments
                        .iter()
                        .any(|a| Path::new(a) == mapping.scratch_path)
                    {
                        return Err(policy("not every explicit media member is attached"));
                    }
                }
            }
        }
        if !config.is_empty() && !command.arguments.windows(config.len()).any(|v| v == config) {
            return Err(SafeLaunchSandboxError::ConfigIsolationFailed {
                reason: "required scratch config argv missing",
            });
        }
        command.working_directory = Some(self.workspace_path().to_owned());
        let environment = self.environment();
        let inherited_lease = self
            .workspace
            .as_ref()
            .expect("owned")
            .lease
            .try_clone()
            .map_err(SafeLaunchSandboxError::SpawnFailed)?;
        let mut workspace = self.workspace.take().expect("owned");
        // Retain ambiguous spawn intent after an abrupt parent crash. Never
        // guess that a potentially live child no longer owns these copies.
        if let Err(e) = workspace.phase(WorkspacePhase::SpawnIntent) {
            let path = workspace.path.clone();
            workspace
                .remove()
                .map_err(|source| SafeLaunchSandboxError::CleanupFailed {
                    workspace_root: path.clone(),
                    source,
                })?;
            return Err(unsafe_path(&path, e));
        }
        let holder = Arc::new(Mutex::new(Some(workspace)));
        let outcome = Arc::new(Mutex::new(CleanupOutcome::Running));
        let on_start = Arc::clone(&holder);
        let on_exit = Arc::clone(&holder);
        let on_exit_outcome = Arc::clone(&outcome);
        let result = spawn_watched_process_isolated(
            &command,
            &environment,
            &inherited_lease,
            move |pid| {
                if let Some(workspace) = on_start.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
                {
                    // A damaged marker remains an explicit cleanup refusal.
                    if let Err(e) = workspace.phase(WorkspacePhase::Running(pid)) {
                        log::warn!("Protected launch marker update failed: {e}");
                    }
                }
            },
            move |report| {
                if let Some(workspace) = on_exit.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    finish_workspace(workspace, report, on_exit_outcome);
                }
            },
        );
        match result {
            Ok(process) => Ok(SandboxedProcess {
                process,
                outcome,
                original_plan: self.plan.clone(),
                mappings: self.mappings.clone(),
            }),
            Err(error) => {
                if let Some(mut workspace) = holder.lock().unwrap_or_else(|e| e.into_inner()).take()
                {
                    workspace
                        .remove()
                        .map_err(|source| SafeLaunchSandboxError::CleanupFailed {
                            workspace_root: workspace.path,
                            source,
                        })?;
                }
                Err(SafeLaunchSandboxError::SpawnFailed(error))
            }
        }
    }
}

impl Drop for PreparedSandbox {
    fn drop(&mut self) {
        if let Some(mut workspace) = self.workspace.take() {
            if let Err(e) = workspace.remove() {
                log::warn!(
                    "Protected launch cleanup refused for {}: {e}",
                    workspace.path.display()
                );
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    Running,
    Removed,
    Retained { path: PathBuf, expires_unix: u64 },
    Refused { path: PathBuf, reason: String },
}

pub struct SandboxedProcess {
    pub process: WatchedProcess,
    pub original_plan: SandboxPlan,
    pub mappings: Vec<ScratchPathMapping>,
    outcome: Arc<Mutex<CleanupOutcome>>,
}
impl SandboxedProcess {
    pub fn cleanup_outcome(&self) -> CleanupOutcome {
        self.outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

fn finish_workspace(
    workspace: Workspace,
    report: &ProcessExitReport,
    outcome: Arc<Mutex<CleanupOutcome>>,
) {
    finish_workspace_with_retention(workspace, report, outcome, FAILED_RETENTION);
}

fn finish_workspace_with_retention(
    mut workspace: Workspace,
    report: &ProcessExitReport,
    outcome: Arc<Mutex<CleanupOutcome>>,
    retention: Duration,
) {
    let publish = |value| *outcome.lock().unwrap_or_else(|e| e.into_inner()) = value;
    match &report.status {
        Ok(status) if status.success() => match workspace.remove() {
            Ok(()) => publish(CleanupOutcome::Removed),
            Err(e) => publish(CleanupOutcome::Refused {
                path: workspace.path.clone(),
                reason: e.to_string(),
            }),
        },
        Ok(_) => {
            let expires = now().saturating_add(retention.as_secs());
            if let Err(e) = workspace.phase(WorkspacePhase::Failed { expires }) {
                publish(CleanupOutcome::Refused {
                    path: workspace.path.clone(),
                    reason: e.to_string(),
                });
                return;
            }
            publish(CleanupOutcome::Retained {
                path: workspace.path.clone(),
                expires_unix: expires,
            });
            // One bounded retention worker per workspace (root cap: 16).
            // Holds the lease; startup sweeps cannot race this cleanup. Child
            // watcher returns immediately, so polling never waits five minutes.
            std::thread::spawn(move || {
                std::thread::sleep(retention);
                let result = match workspace.remove() {
                    Ok(()) => CleanupOutcome::Removed,
                    Err(e) => CleanupOutcome::Refused {
                        path: workspace.path.clone(),
                        reason: e.to_string(),
                    },
                };
                *outcome.lock().unwrap_or_else(|e| e.into_inner()) = result;
            });
        }
        Err(e) => publish(CleanupOutcome::Refused {
            path: workspace.path.clone(),
            reason: format!("child exit not established: {e}"),
        }),
    }
}
