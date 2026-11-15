//! Explicit, bounded archive inspection and unpacking.
//!
//! ZIP inspection remains available; ZIP Apply is blocked for preservation
//! safety. 7Z and RAR are listed and extracted through a detected system
//! 7-Zip, under the trust boundary below. (RAR used to be listed through
//! `unrar`; its real `lt` output is `Name: value`, not the `Pathname = value`
//! form the old parser expected, so that path never produced a plan. 7-Zip
//! lists and extracts RAR4/RAR5 natively and is the one backend now. What
//! the detected build can decode is read from its own `i` listing, never
//! assumed; a 7-Zip built without RAR codecs is reported as
//! [`ArchiveEligibility::ToolLacksFormatSupport`], not as missing.
//! Multi-volume sets are refused as [`ArchiveEligibility::MultipartIncomplete`].
//! A security-tested `unrar`/libarchive fallback covering multi-volume sets
//! is a recorded follow-up, kept out of this path on purpose.)
//!
//! # The trust boundary
//!
//! An archive is untrusted input and the external tool is a program this
//! build did not write. Every operation here therefore holds to:
//!
//! - **The tool is a checked absolute path.** Discovery goes through
//!   [`crate::external_tools`]: absolute `PATH` directories only, regular
//!   executable only. A `7z` dropped in the working directory is never run.
//! - **The archive is pinned by descriptor, not by name.** Inspection opens
//!   the archive once through [`crate::safe_read`] and lists
//!   `/proc/self/fd/<N>` from that descriptor. Its device, inode, size and
//!   modification time become the plan's [`ArchiveSourceIdentity`].
//!   Extraction pins the archive again and refuses if the identity differs
//!   (Night Watch S02-04): a plan approved for one set of bytes is never
//!   executed against another.
//! - **The listing is parsed strictly.** Only records after 7-Zip's
//!   `----------` separator count; every line must be `Key = Value`; a key
//!   repeated within a record, a record without `Path`, `Size` or
//!   `Attributes`, or a name containing a control character refuses the
//!   whole archive. A member name with an embedded newline can therefore not
//!   hide a member from the plan (Night Watch S01-01).
//! - **Member names obey one rule.** [`crate::dat::archive::member_path`]:
//!   relative, plain components, no `..`/`.`, no control, wildcard or
//!   backslash characters. Symlink, hardlink, alternate-stream and split
//!   members are refused by type. Names that collide exactly or
//!   case-insensitively refuse the archive.
//! - **Resources are bounded.** Archive size, member count, expanded size,
//!   compression ratio, listing output size, wall-clock time, CPU time and
//!   address space of the tool, and the size of any single file it may
//!   write (`RLIMIT_FSIZE` = the largest listed member, at least
//!   [`MIN_OUTPUT_FILE_CAP_BYTES`]). Free space for the
//!   whole expansion is checked before extraction begins.
//! - **The destination is reached relative to a pinned directory.** The
//!   destination's parent is opened with a no-symlink walk, the stage is a
//!   fresh unique directory inside it addressed as `/proc/self/fd/<N>`, the
//!   extracted tree is verified against the plan (regular files only, exact
//!   paths, exact sizes, link count one), and publication is one
//!   `renameat2(RENAME_NOREPLACE)` relative to that parent descriptor. The
//!   stage is removed on every failure path, including panic unwind, by
//!   [`tempfile::TempDir`]; nothing is published unless verification passed.
//!
//! Anything ambiguous is refused with a named reason, never guessed.
//!
//! # Known limits (recorded, not hidden)
//!
//! - The listing is provisional until extraction verifies the tree. A 7-Zip
//!   build that prints raw control characters (p7zip 16.02; 7-Zip 23.01
//!   replaces them with `_`) could, with a name containing a newline that
//!   splits one record into two *complete* records, show a member name that
//!   does not exist; extraction then refuses the real name (control
//!   character) after decompression. Nothing escapes; the plan can be wrong.
//! - An archive with trailing data makes 7-Zip append a `Warnings: 1` line,
//!   which the strict parser refuses as a malformed listing.
//! - Only the call sites listed in the Night Watch findings were migrated to
//!   [`crate::external_tools`]; other `PATH` searches in the crate (emulator
//!   adapters, local package helpers) are a separate follow-up.
//! - The plan's identity is device/inode/size/mtime. An archive rewritten in
//!   place with its mtime restored is not detected; the verified names and
//!   sizes still bound what such a rewrite can achieve.

use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::dat::archive::external_process::{ProcessError, ProcessLimits, run_supervised};
use crate::dat::archive::member_path;

pub const MAX_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 100_000;
pub const MAX_MEMBER_NAME_BYTES: usize = member_path::MAX_MEMBER_NAME_BYTES;
pub const MAX_EXPANDED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_COMPRESSION_RATIO: u64 = 10_000;
/// Wall-clock bound for a tool version/capability probe.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(5);
/// Wall-clock bound for listing one archive.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(120);
/// Wall-clock bound for extracting one archive. Sized for the largest
/// permitted expansion ([`MAX_EXPANDED_BYTES`]) on slow storage, not for a
/// typical archive: a legitimate large dump must not be refused after a
/// full decompression.
pub const EXTRACT_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// Longest `-slt` listing accepted: [`MAX_ENTRIES`] records of generous size.
pub const LIST_STDOUT_LIMIT: u64 = 128 * 1024 * 1024;
/// Extraction output is progress chatter only; anything more is a fault.
pub const EXTRACT_STDOUT_LIMIT: u64 = 4 * 1024 * 1024;
/// Free space required beyond the planned expansion before extraction starts.
pub const FREE_SPACE_MARGIN_BYTES: u64 = 64 * 1024 * 1024;
/// Smallest per-file size cap (`RLIMIT_FSIZE`) given to the extraction
/// tool. The cap is the largest listed member or this floor, whichever is
/// greater: its job is to stop a member that lied about its size by
/// gigabytes, while the exact per-file sizes are verified afterwards anyway.
pub const MIN_OUTPUT_FILE_CAP_BYTES: u64 = 1024 * 1024;
const LIST_LIMITS: ProcessLimits = ProcessLimits {
    address_space_bytes: 1024 * 1024 * 1024,
    cpu_seconds: 120,
};
const EXTRACT_LIMITS: ProcessLimits = ProcessLimits {
    address_space_bytes: 4 * 1024 * 1024 * 1024,
    cpu_seconds: 2 * 60 * 60,
};
/// The marker every stage directory carries while extraction runs, naming
/// the EmuWiz process that owns it. A stage whose owner is gone (power loss,
/// `kill -9`) is swept by the next extraction into the same parent.
const STAGE_MARKER_NAME: &str = ".emuwiz-archive-stage-owner";
const STAGE_PREFIX: &str = ".emuwiz-archive-stage-";
/// Mode of a published destination directory (the stage is created `0o700`
/// by `tempfile`; a library folder must stay readable to other accounts).
const PUBLISHED_DIRECTORY_MODE: u32 = 0o755;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Rar,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArchiveOperation {
    ExtractArchive,
    CreateArchive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArchiveEligibility {
    Ready,
    ToolMissing,
    PasswordRequired,
    EncryptedUnsupported,
    PathConflict,
    TraversalDetected,
    UnsafeEntry,
    TooLarge,
    TooManyEntries,
    Unsupported,
    ReviewRequired,
    UnsafeMember,
    UnsupportedMemberType,
    MultipartIncomplete,
    MalformedArchive,
    ExtractionVerificationFailed,
    /// The external tool exceeded its wall-clock, CPU, memory or output bound.
    ToolLimitExceeded,
    /// The archive on disk is not the one the plan was made from.
    SourceChanged,
    /// The destination (or its parent) is not a safe place to publish into.
    DestinationUnsafe,
    /// The file system does not have room for the planned expansion.
    InsufficientSpace,
    /// A 7-Zip is installed but its build does not advertise the codec this
    /// format needs (a RAR archive on a 7-Zip built without RAR support).
    /// Deliberately distinct from [`Self::ToolMissing`]: the tool is there,
    /// it just cannot do this.
    ToolLacksFormatSupport,
    /// The tool opened the archive but cannot decode a compression method
    /// inside it (reported by 7-Zip as "Unsupported Method").
    UnsupportedCodec,
    /// The external tool could not be started or supervised (spawn failure,
    /// pipe error, cleanup failure). Says nothing about the archive.
    ToolFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArchiveToolStatus {
    Found,
    Missing,
    VersionUnknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveToolRecord {
    pub name: String,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub status: ArchiveToolStatus,
    pub capabilities: Vec<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ArchiveToolInventory {
    pub tools: Vec<ArchiveToolRecord>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveEntryPlan {
    pub path: String,
    pub directory: bool,
    pub logical_size: u64,
}

/// What identifies the archive bytes a plan was made from: the open file's
/// device and inode (the thing `/proc/self/fd/<N>` pins), plus its size and
/// modification time (which change when the same inode is rewritten in
/// place). Extraction re-captures this from its own pinned descriptor and
/// refuses on any difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ArchiveSourceIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified_secs: i64,
    pub modified_nanos: i64,
}

impl ArchiveSourceIdentity {
    pub fn capture(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                size: metadata.len(),
                modified_secs: metadata.mtime(),
                modified_nanos: metadata.mtime_nsec(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                device: 0,
                inode: 0,
                size: metadata.len(),
                modified_secs: 0,
                modified_nanos: 0,
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchivePlan {
    pub operation: ArchiveOperation,
    pub format: ArchiveFormat,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub entries: Vec<ArchiveEntryPlan>,
    pub compressed_bytes: Option<u64>,
    pub expanded_bytes: u64,
    pub eligibility: ArchiveEligibility,
    pub warnings: Vec<String>,
    /// Identity of the archive the entries were listed from. `None` for a
    /// plan that was never bound (a hand-built plan); extraction refuses it.
    pub source_identity: Option<ArchiveSourceIdentity>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveOperationResult {
    pub operation: ArchiveOperation,
    pub format: ArchiveFormat,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub member_count: usize,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub expanded_bytes: u64,
    pub verification: String,
    pub warnings: Vec<String>,
}

/// Finds an archive tool by bare name in the absolute `PATH` directories
/// only ([`crate::external_tools`]); the found file must additionally be a
/// non-symlink regular executable, as this inventory has always required.
fn which(name: &str) -> Option<PathBuf> {
    crate::external_tools::process_path_directories()
        .iter()
        .map(|p| p.join(name))
        .find(|p| {
            fs::symlink_metadata(p)
                .map(|m| m.is_file() && !m.file_type().is_symlink())
                .unwrap_or(false)
                && crate::external_tools::is_regular_executable(p)
        })
}
fn tool(name: &str, capabilities: &[&str]) -> ArchiveToolRecord {
    let path = which(name);
    let Some(path) = path.clone() else {
        return ArchiveToolRecord {
            name: name.into(),
            path: None,
            version: None,
            status: ArchiveToolStatus::Missing,
            capabilities: capabilities.iter().map(|s| (*s).into()).collect(),
        };
    };
    let mut child = Command::new(&path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok();
    let started = Instant::now();
    let output = loop {
        let Some(process) = child.as_mut() else {
            break None;
        };
        if process.try_wait().ok().flatten().is_some() {
            break child
                .take()
                .and_then(|process| process.wait_with_output().ok());
        }
        if started.elapsed() >= TOOL_TIMEOUT {
            let _ = process.kill();
            let _ = process.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let version = output.and_then(|o| {
        let mut bytes = o.stdout;
        bytes.extend(o.stderr);
        bytes.truncate(8192);
        String::from_utf8(bytes)
            .ok()
            .and_then(|s| s.lines().find(|l| !l.trim().is_empty()).map(str::to_owned))
    });
    ArchiveToolRecord {
        name: name.into(),
        path: Some(path),
        status: if version.is_some() {
            ArchiveToolStatus::Found
        } else {
            ArchiveToolStatus::VersionUnknown
        },
        version,
        capabilities: capabilities.iter().map(|s| (*s).into()).collect(),
    }
}
pub fn inventory_archive_tools() -> ArchiveToolInventory {
    ArchiveToolInventory {
        tools: vec![
            tool("zip", &["create"]),
            tool("unzip", &["extract", "test"]),
            tool("7z", &["create", "extract", "test"]),
            tool("7zz", &["create", "extract", "test"]),
            tool("unrar", &["extract", "test"]),
            tool("bsdtar", &["extract"]),
        ],
    }
}

/// Which RAR generations the detected 7-Zip advertised in its `i` listing
/// (`Rar` is the RAR 1.5-4.x codec, `Rar5` the RAR5 codec). Both are
/// detected rather than assumed from the executable's name, because distro
/// builds of 7-Zip may omit RAR entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RarSupport {
    pub rar4: bool,
    pub rar5: bool,
}

impl RarSupport {
    pub const NONE: Self = Self {
        rar4: false,
        rar5: false,
    };
    pub const ALL: Self = Self {
        rar4: true,
        rar5: true,
    };

    pub fn any(self) -> bool {
        self.rar4 || self.rar5
    }
}

/// The external backend 7Z/RAR plans run through: one checked 7-Zip
/// executable and what it advertised.
///
/// RAR is handled by 7-Zip's own RAR codecs; a separate `unrar`/libarchive
/// fallback (which would also cover multi-volume sets) is a recorded
/// follow-up, not part of this security-reviewed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveTools {
    seven_zip: Option<PathBuf>,
    rar: RarSupport,
    list_timeout: Duration,
    extract_timeout: Duration,
}

impl ArchiveTools {
    /// No backend at all: every 7Z/RAR plan reports [`ArchiveEligibility::ToolMissing`].
    pub fn none() -> Self {
        Self {
            seven_zip: None,
            rar: RarSupport::NONE,
            list_timeout: LIST_TIMEOUT,
            extract_timeout: EXTRACT_TIMEOUT,
        }
    }

    /// A caller-supplied executable, for tests and for callers that have
    /// already validated one. The path must be absolute, or it is ignored.
    pub fn with_seven_zip(executable: PathBuf, rar: RarSupport) -> Self {
        if !executable.is_absolute() {
            return Self::none();
        }
        Self {
            seven_zip: Some(executable),
            rar,
            ..Self::none()
        }
    }

    /// What the detected 7-Zip can do with RAR archives.
    pub fn rar_support(&self) -> RarSupport {
        self.rar
    }

    /// Whether the detected 7-Zip advertised the handler the pinned bytes
    /// need (`Rar` and `Rar5` are separate codecs).
    fn check_handler_support(&self, handler: &str) -> Result<(), ArchiveEligibility> {
        let supported = match handler {
            "7z" => true,
            "Rar" => self.rar.rar4,
            "Rar5" => self.rar.rar5,
            _ => false,
        };
        if supported {
            Ok(())
        } else {
            Err(ArchiveEligibility::ToolLacksFormatSupport)
        }
    }

    /// Tighter wall-clock bounds than the defaults (never looser).
    pub fn with_timeouts(mut self, list: Duration, extract: Duration) -> Self {
        self.list_timeout = list.min(LIST_TIMEOUT);
        self.extract_timeout = extract.min(EXTRACT_TIMEOUT);
        self
    }

    /// Probes `7zz`, then `7z`, then `/usr/lib/7zip/7z` through
    /// [`crate::external_tools::resolve_executable`] and keeps the first
    /// that answers `<exe> i` (its capability listing) within
    /// [`TOOL_TIMEOUT`]. RAR5 support is taken from that listing, never
    /// assumed from the executable's name.
    pub fn discover() -> Self {
        for candidate in [
            PathBuf::from("7zz"),
            PathBuf::from("7z"),
            PathBuf::from("/usr/lib/7zip/7z"),
        ] {
            let Some(executable) = crate::external_tools::resolve_executable(&candidate) else {
                continue;
            };
            let mut command = Command::new(&executable);
            command.arg("i").env("LC_ALL", "C");
            let mut captured = Vec::new();
            let outcome = run_supervised(
                command,
                ProcessLimits::default(),
                TOOL_TIMEOUT,
                1024 * 1024,
                |chunk| {
                    captured.extend_from_slice(chunk);
                    Ok(())
                },
                None,
            );
            if !matches!(&outcome, Ok(outcome) if outcome.status.success()) {
                continue;
            }
            let Ok(text) = String::from_utf8(captured) else {
                continue;
            };
            let has_format = |name: &str| {
                text.lines()
                    .any(|line| line.split_ascii_whitespace().any(|field| field == name))
            };
            if !has_format("7z") {
                continue;
            }
            return Self {
                seven_zip: Some(executable),
                rar: RarSupport {
                    rar4: has_format("Rar"),
                    rar5: has_format("Rar5"),
                },
                ..Self::none()
            };
        }
        Self::none()
    }

    pub fn seven_zip(&self) -> Option<&Path> {
        self.seven_zip.as_deref()
    }

    fn executable_for(&self, format: ArchiveFormat) -> Result<&Path, ArchiveEligibility> {
        let executable = match format {
            ArchiveFormat::Zip => return Err(ArchiveEligibility::Unsupported),
            ArchiveFormat::SevenZ | ArchiveFormat::Rar => self
                .seven_zip
                .as_deref()
                .ok_or(ArchiveEligibility::ToolMissing)?,
        };
        if format == ArchiveFormat::Rar && !self.rar.any() {
            // Installed but incapable is not "missing": say so.
            return Err(ArchiveEligibility::ToolLacksFormatSupport);
        }
        Ok(executable)
    }
}

/// The format of `path`. The file's signature decides when the file can be
/// read; the extension is only a fallback for a file that cannot be read
/// (a name-only classification). A ZIP renamed `.7z` is therefore a ZIP,
/// and never reaches the external tool.
pub fn classify_archive(path: &Path) -> Option<ArchiveFormat> {
    let by_extension = path.extension().and_then(|extension| {
        match extension.to_str()?.to_ascii_lowercase().as_str() {
            "zip" => Some(ArchiveFormat::Zip),
            "7z" => Some(ArchiveFormat::SevenZ),
            "rar" => Some(ArchiveFormat::Rar),
            _ => None,
        }
    });
    // Only a regular file is opened for its signature: a FIFO in the
    // archive's place would block `open` forever (and is refused later).
    if !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return by_extension;
    }
    let Ok(mut file) = fs::File::open(path) else {
        return by_extension;
    };
    let mut signature = [0u8; SIGNATURE_BYTES];
    let Ok(count) = std::io::Read::read(&mut file, &mut signature) else {
        return by_extension;
    };
    classify_signature(&signature[..count]).or(by_extension)
}

const SIGNATURE_BYTES: usize = 8;

fn classify_signature(signature: &[u8]) -> Option<ArchiveFormat> {
    if signature.starts_with(b"PK\x03\x04")
        || signature.starts_with(b"PK\x05\x06")
        || signature.starts_with(b"PK\x07\x08")
    {
        Some(ArchiveFormat::Zip)
    } else if signature.starts_with(b"7z\xBC\xAF\x27\x1C") {
        Some(ArchiveFormat::SevenZ)
    } else if signature.starts_with(b"Rar!\x1A\x07") {
        Some(ArchiveFormat::Rar)
    } else {
        None
    }
}

/// The exact 7-Zip handler name for the pinned bytes, used both as the
/// `-t<name>` switch (so 7-Zip never auto-detects one of its forty other
/// container parsers) and as the `Type =` the listing's summary record must
/// report. RAR 1.5-4.x and RAR5 are different handlers and the signature's
/// version byte tells them apart.
fn sevenz_handler(
    format: ArchiveFormat,
    signature: &[u8],
) -> Result<&'static str, ArchiveEligibility> {
    match (format, classify_signature(signature)) {
        (ArchiveFormat::SevenZ, Some(ArchiveFormat::SevenZ)) => Ok("7z"),
        (ArchiveFormat::Rar, Some(ArchiveFormat::Rar)) => match signature.get(6) {
            Some(0x00) => Ok("Rar"),
            Some(0x01) => Ok("Rar5"),
            _ => Err(ArchiveEligibility::Unsupported),
        },
        _ => Err(ArchiveEligibility::Unsupported),
    }
}

/// The accepted destination-relative path for a member name, under the
/// crate-wide rule ([`crate::dat::archive::member_path`]). A name that
/// would need normalising (`./a`, `a//b`, `a/../b`, a backslash) is refused,
/// not repaired: the string handed to the tool must be the string it
/// listed. ZIP names written by Windows tools with backslashes are therefore
/// refused too, and said so.
pub fn validate_member_path(name: &str) -> Result<PathBuf, ArchiveEligibility> {
    member_path::safe_member_relative_path(name).map_err(|refusal| match refusal {
        member_path::MemberNameRefusal::Absolute
        | member_path::MemberNameRefusal::DrivePrefix
        | member_path::MemberNameRefusal::ParentComponent => ArchiveEligibility::TraversalDetected,
        member_path::MemberNameRefusal::Empty
        | member_path::MemberNameRefusal::TooLong
        | member_path::MemberNameRefusal::EmptyComponent
        | member_path::MemberNameRefusal::DotComponent => ArchiveEligibility::UnsafeEntry,
        member_path::MemberNameRefusal::ControlCharacter
        | member_path::MemberNameRefusal::Backslash
        | member_path::MemberNameRefusal::WildcardCharacter
        | member_path::MemberNameRefusal::LeadingDash => ArchiveEligibility::UnsafeMember,
    })
}

/// A refusal message that says which member and why, so a Windows-style
/// name (`Game\disc.iso`) or a traversal attempt is reported as exactly
/// that rather than as a corrupt or unsupported archive.
fn explain_member_refusal(name: &str, eligibility: ArchiveEligibility) -> String {
    let why = member_path::check_member_name(name)
        .err()
        .map(|refusal| refusal.detail())
        .unwrap_or("the member name was refused");
    let hint = if name.contains('\\') {
        " (archives whose names use backslash separators are refused rather than reinterpreted; repack them with forward slashes)"
    } else {
        ""
    };
    format!("unsafe archive member {name:?}: {why}{hint} [{eligibility:?}]")
}

/// Inspects `source` and plans its extraction into `destination`. 7Z/RAR
/// archives need a system 7-Zip, discovered only when the archive is one of
/// those formats; a ZIP is inspected without starting any process.
pub fn inspect_archive_plan(source: &Path, destination: &Path) -> Result<ArchivePlan, String> {
    let format =
        classify_archive(source).ok_or_else(|| "unsupported archive format".to_string())?;
    if format == ArchiveFormat::Zip {
        return inspect_archive_plan_with_tools(source, destination, &ArchiveTools::none());
    }
    inspect_archive_plan_with_tools(source, destination, &ArchiveTools::discover())
}

/// [`inspect_archive_plan`] with an explicit backend.
pub fn inspect_archive_plan_with_tools(
    source: &Path,
    destination: &Path,
    tools: &ArchiveTools,
) -> Result<ArchivePlan, String> {
    let format =
        classify_archive(source).ok_or_else(|| "unsupported archive format".to_string())?;
    let size = fs::metadata(source).map_err(|e| e.to_string())?;
    if !size.is_file() || size.len() > MAX_ARCHIVE_BYTES {
        return Err("archive is missing, not regular, or too large".into());
    }
    if format != ArchiveFormat::Zip {
        return inspect_external_archive_plan(source, destination, format, tools);
    }
    // `classify_archive` above already settled the format, by extension or by
    // signature. Re-deriving it from the filename inside the inspector would
    // reject a ZIP that arrived without one - which is exactly how every
    // content-addressed downloaded mod payload is stored.
    let report =
        crate::inspector::inspect_zip_archive_with_limit(source, crate::INSPECTOR_ENTRY_LIMIT)
            .map_err(|e| e.to_string())?;
    if report.truncated || report.total_entries_in_archive > MAX_ENTRIES {
        return Err("archive contains too many entries".into());
    }
    let mut entries = Vec::new();
    let mut expanded = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    for entry in report.entries {
        // ZIP directory entries carry a trailing `/`; the name rule refuses
        // an empty trailing component, so strip it for directories only.
        let is_directory = entry.kind == crate::InspectorEntryKind::Directory;
        let name = if is_directory {
            entry.name.strip_suffix('/').unwrap_or(&entry.name)
        } else {
            entry.name.as_str()
        };
        let safe = validate_member_path(name).map_err(|e| explain_member_refusal(name, e))?;
        let key = member_path::case_fold_key(&safe.to_string_lossy());
        if !seen.insert(key) {
            return Err("case-folding member collision".into());
        }
        expanded = expanded
            .checked_add(entry.uncompressed_size)
            .ok_or("expanded size overflow")?;
        if expanded > MAX_EXPANDED_BYTES
            || (entry.compressed_size.unwrap_or(0) > 0
                && entry.uncompressed_size / entry.compressed_size.unwrap_or(1)
                    > MAX_COMPRESSION_RATIO)
        {
            return Err("archive expansion exceeds safety policy".into());
        }
        entries.push(ArchiveEntryPlan {
            path: safe.to_string_lossy().into(),
            directory: is_directory,
            logical_size: entry.uncompressed_size,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ArchivePlan {
        operation: ArchiveOperation::ExtractArchive,
        format,
        source: source.into(),
        destination: destination.into(),
        entries,
        compressed_bytes: Some(size.len()),
        expanded_bytes: expanded,
        eligibility: ArchiveEligibility::Ready,
        warnings: Vec::new(),
        source_identity: Some(ArchiveSourceIdentity::capture(&size)),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalEntry {
    path: String,
    kind: ExternalEntryKind,
    size: u64,
    encrypted: bool,
}

/// The pinned, read-only archive handle every external listing and
/// extraction runs against, with the identity captured from it.
struct PinnedSource {
    file: fs::File,
    identity: ArchiveSourceIdentity,
    /// The first bytes of the pinned file, read through the pinned handle.
    signature: Vec<u8>,
}

/// Opens `source` once through [`crate::safe_read`] on its canonical path
/// (so a symlinked library still works, while a swap during the final walk
/// is refused) and captures the identity of the open file.
fn pin_source(source: &Path) -> Result<PinnedSource, String> {
    let canonical = fs::canonicalize(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let mut safe =
        crate::safe_read::open_bounded_read(&canonical, &crate::safe_read::TrustedRoots::none())
            .map_err(|refusal| format!("archive refused: {}", refusal.detail()))?;
    if safe.len() > MAX_ARCHIVE_BYTES {
        return Err("archive is too large".into());
    }
    let signature_length = usize::try_from(safe.len())
        .unwrap_or(usize::MAX)
        .min(SIGNATURE_BYTES);
    let signature = if signature_length == 0 {
        Vec::new()
    } else {
        safe.read_exact_at(0, signature_length, SIGNATURE_BYTES)
            .ok_or("archive signature could not be read")?
    };
    let file = safe.into_file();
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    Ok(PinnedSource {
        file,
        identity: ArchiveSourceIdentity::capture(&metadata),
        signature,
    })
}

#[cfg(unix)]
fn proc_self_fd(file: &fs::File) -> String {
    use std::os::fd::AsRawFd;
    // Resolved by the *child* after `exec`, so this must be the number the
    // child inherits - never the parent's pid.
    format!("/proc/self/fd/{}", file.as_raw_fd())
}

/// A `pre_exec` step that lets the child inherit exactly `fds` (clears
/// `FD_CLOEXEC` on them, in the child only) and caps the size of any file
/// it may write. Async-signal-safe: two `fcntl`/`setrlimit` calls, no
/// allocation.
#[cfg(unix)]
fn inherit_fds_and_cap_file_size(
    fds: Vec<i32>,
    max_file_bytes: u64,
) -> Box<dyn Fn() -> std::io::Result<()> + Send + Sync> {
    Box::new(move || {
        // If EmuWiz itself dies (kill -9, crash) while the tool runs, the tool
        // must not keep writing into the stage as an orphan.
        // SAFETY: `prctl` is async-signal-safe and affects only the child.
        if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        for fd in &fds {
            // SAFETY: `fcntl` on a descriptor the parent keeps open for the
            // duration of the spawn; it changes only the child's copy.
            if unsafe { libc::fcntl(*fd, libc::F_SETFD, 0) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        let limit = libc::rlimit {
            rlim_cur: max_file_bytes,
            rlim_max: max_file_bytes,
        };
        // SAFETY: `limit` is an initialised stack value; this runs in the
        // forked child and affects only that child.
        if unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &limit) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    })
}

fn classify_tool_failure(error: ProcessError) -> ArchiveEligibility {
    match error {
        ProcessError::Timeout | ProcessError::OutputLimitExceeded { .. } => {
            ArchiveEligibility::ToolLimitExceeded
        }
        // A tool that could not be run says nothing about the archive; it is
        // never reported as a malformed archive.
        ProcessError::Io { .. }
        | ProcessError::InvalidLimits
        | ProcessError::Sink { .. }
        | ProcessError::CleanupFailure { .. } => ArchiveEligibility::ToolFailed,
    }
}

fn mentions_password(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("password") || lower.contains("encrypted") || lower.contains("wrong password")
}

/// Lists the pinned archive with `7z l -slt` and returns the raw text.
#[cfg(unix)]
fn run_sevenz_listing(
    executable: &Path,
    source: &PinnedSource,
    handler: &str,
    timeout: Duration,
) -> Result<String, ArchiveEligibility> {
    let mut command = Command::new(executable);
    command.env("LC_ALL", "C").env("LANG", "C");
    command.args(["l", "-slt", "-sccUTF-8", "-p-", "-bd"]);
    command.arg(format!("-t{handler}"));
    command.arg("--");
    command.arg(proc_self_fd(&source.file));
    let mut captured = Vec::new();
    use std::os::fd::AsRawFd;
    let outcome = run_supervised(
        command,
        LIST_LIMITS,
        timeout,
        LIST_STDOUT_LIMIT,
        |chunk| {
            captured.extend_from_slice(chunk);
            Ok(())
        },
        Some(inherit_fds_and_cap_file_size(
            vec![source.file.as_raw_fd()],
            0,
        )),
    )
    .map_err(classify_tool_failure)?;
    let text = String::from_utf8(captured).map_err(|_| ArchiveEligibility::MalformedArchive)?;
    if !outcome.status.success() {
        let stderr = String::from_utf8_lossy(&outcome.stderr);
        if mentions_password(&text) || mentions_password(&stderr) {
            return Err(ArchiveEligibility::PasswordRequired);
        }
        if mentions_unsupported_method(&text) || mentions_unsupported_method(&stderr) {
            return Err(ArchiveEligibility::UnsupportedCodec);
        }
        return Err(ArchiveEligibility::MalformedArchive);
    }
    // A listing can succeed while a member records a method this build
    // cannot decode; 7-Zip prints the warning to stderr.
    if mentions_unsupported_method(&String::from_utf8_lossy(&outcome.stderr)) {
        return Err(ArchiveEligibility::UnsupportedCodec);
    }
    Ok(text)
}

fn mentions_unsupported_method(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("unsupported method") || lower.contains("unsupported compression method")
}

#[cfg(not(unix))]
fn run_sevenz_listing(
    _executable: &Path,
    _source: &PinnedSource,
    _handler: &str,
    _timeout: Duration,
) -> Result<String, ArchiveEligibility> {
    Err(ArchiveEligibility::Unsupported)
}

/// Splits one `Key = Value` listing line. 7-Zip prints an empty value as
/// `Key = ` (trailing space), which a `\r`-stripped line keeps; a line that
/// is neither form is not a property line and refuses the listing.
fn split_property_line(line: &str) -> Result<(&str, &str), ArchiveEligibility> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let (key, value) = match line.split_once(" = ") {
        Some(pair) => pair,
        None => match line.strip_suffix(" =") {
            Some(key) => (key, ""),
            None => return Err(ArchiveEligibility::MalformedArchive),
        },
    };
    if key.is_empty()
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == ' ')
    {
        return Err(ArchiveEligibility::MalformedArchive);
    }
    Ok((key, value))
}

/// Classifies a 7-Zip `Attributes` value. 7-Zip prints an optional group of
/// Windows attribute letters (`D`, `A`, `R`, `H`, `S`, `_`...) and, for
/// Unix-origin members, a space followed by a `ls -l` style mode string.
/// The mode string decides when present; otherwise the Windows letters do.
/// Anything that is neither a plain file nor a directory is refused.
fn classify_attributes(
    attributes: &str,
    folder: Option<&str>,
) -> Result<ExternalEntryKind, ArchiveEligibility> {
    let trimmed = attributes.trim();
    let mut parts = trimmed.split_ascii_whitespace();
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    let unix_mode = if first.len() == 10 && first.starts_with(['-', 'd', 'l', 'b', 'c', 'p', 's']) {
        Some(first)
    } else {
        second.filter(|mode| mode.len() == 10)
    };
    let windows_letters = if unix_mode == Some(first) { "" } else { first };
    let kind = if let Some(mode) = unix_mode {
        match mode.chars().next() {
            Some('-') => ExternalEntryKind::File,
            Some('d') => ExternalEntryKind::Directory,
            _ => return Err(ArchiveEligibility::UnsupportedMemberType),
        }
    } else if windows_letters.contains('D') {
        ExternalEntryKind::Directory
    } else if trimmed.is_empty() && folder.is_none() {
        return Err(ArchiveEligibility::UnsupportedMemberType);
    } else {
        ExternalEntryKind::File
    };
    // `Folder = +/-` must agree with the attributes when both are present.
    match (folder, kind) {
        (Some("+"), ExternalEntryKind::File) | (Some("-"), ExternalEntryKind::Directory) => {
            Err(ArchiveEligibility::MalformedArchive)
        }
        _ => Ok(kind),
    }
}

/// Parses a `7z l -slt` listing strictly (see the module doc). The summary
/// record before the separator must report `Type = <handler>`: the handler
/// the tool was pinned to, so a container parsed by some other handler
/// never produces a plan.
fn parse_sevenz_slt_listing(
    listing: &str,
    handler: &str,
) -> Result<Vec<ExternalEntry>, ArchiveEligibility> {
    let mut lines = listing.lines();
    // Everything before the separator is tool banner and the archive's own
    // summary record; only member records follow it.
    let mut summary_type = None;
    let mut separated = false;
    for line in lines.by_ref() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line == "----------" {
            separated = true;
            break;
        }
        if let Some(value) = line.strip_prefix("Type = ") {
            summary_type = Some(value.trim().to_string());
        }
    }
    if !separated {
        return Err(ArchiveEligibility::MalformedArchive);
    }
    if summary_type.as_deref() != Some(handler) {
        return Err(ArchiveEligibility::Unsupported);
    }
    let mut entries = Vec::new();
    let mut record = std::collections::BTreeMap::<&str, &str>::new();
    let flush = |record: &mut std::collections::BTreeMap<&str, &str>,
                 entries: &mut Vec<ExternalEntry>|
     -> Result<(), ArchiveEligibility> {
        if record.is_empty() {
            return Ok(());
        }
        let (Some(path), Some(size_text), Some(attributes)) = (
            record.get("Path").copied(),
            record.get("Size").copied(),
            record.get("Attributes").copied(),
        ) else {
            return Err(ArchiveEligibility::MalformedArchive);
        };
        // The member-name rule is applied by the plan builder, which can name
        // the member and the reason; the parser only guards line structure
        // (an embedded newline cannot survive the duplicate-key and
        // mandatory-key checks above and below).
        for link_key in ["Symbolic Link", "Hard Link", "Copy Link"] {
            if record
                .get(link_key)
                .is_some_and(|value| !value.trim().is_empty())
            {
                return Err(ArchiveEligibility::UnsupportedMemberType);
            }
        }
        if record
            .get("Alternate Stream")
            .is_some_and(|value| value.trim() == "+")
            || record.get("Anti").is_some_and(|value| value.trim() == "+")
        {
            return Err(ArchiveEligibility::UnsupportedMemberType);
        }
        if record
            .get("Split Before")
            .is_some_and(|value| value.trim() == "+")
            || record
                .get("Split After")
                .is_some_and(|value| value.trim() == "+")
        {
            return Err(ArchiveEligibility::MultipartIncomplete);
        }
        let folder = record.get("Folder").map(|value| value.trim());
        let kind = classify_attributes(attributes, folder)?;
        let size = if size_text.trim().is_empty() && kind == ExternalEntryKind::Directory {
            0
        } else {
            size_text
                .trim()
                .parse::<u64>()
                .map_err(|_| ArchiveEligibility::MalformedArchive)?
        };
        let encrypted = record
            .get("Encrypted")
            .is_some_and(|value| value.trim() == "+");
        if entries.len() >= MAX_ENTRIES {
            return Err(ArchiveEligibility::TooManyEntries);
        }
        entries.push(ExternalEntry {
            path: path.to_string(),
            kind,
            size,
            encrypted,
        });
        record.clear();
        Ok(())
    };
    for line in lines {
        let stripped = line.strip_suffix('\r').unwrap_or(line);
        if stripped.is_empty() {
            flush(&mut record, &mut entries)?;
            continue;
        }
        let (key, value) = split_property_line(stripped)?;
        if record.insert(key, value).is_some() {
            // A key twice in one record is exactly what a member name with
            // an embedded newline produces.
            return Err(ArchiveEligibility::MalformedArchive);
        }
    }
    flush(&mut record, &mut entries)?;
    if entries.is_empty() {
        return Err(ArchiveEligibility::MalformedArchive);
    }
    Ok(entries)
}

fn inspect_external_archive_plan(
    source: &Path,
    destination: &Path,
    format: ArchiveFormat,
    tools: &ArchiveTools,
) -> Result<ArchivePlan, String> {
    if looks_like_multipart_archive(source) {
        return Err(format!(
            "archive inspection failed: {:?}: {} looks like one volume of a multi-volume set (.part1.rar, .r00 ...); multi-volume archives are not supported, supply a single-volume archive",
            ArchiveEligibility::MultipartIncomplete,
            source.display()
        ));
    }
    let executable = tools
        .executable_for(format)
        .map_err(|reason| format!("archive inspection failed: {reason:?}"))?;
    let pinned = pin_source(source)?;
    let handler = sevenz_handler(format, &pinned.signature).map_err(|reason| {
        format!(
            "archive inspection failed: {reason:?}: the file's signature is not a {format:?} archive"
        )
    })?;
    tools.check_handler_support(handler).map_err(|reason| {
        format!(
            "archive inspection failed: {reason:?}: the installed 7-Zip does not advertise the {handler} codec"
        )
    })?;
    let listing = run_sevenz_listing(executable, &pinned, handler, tools.list_timeout)
        .map_err(|reason| format!("archive inspection failed: {reason:?}"))?;
    let entries = parse_sevenz_slt_listing(&listing, handler)
        .map_err(|reason| format!("archive inspection failed: {reason:?}"))?;
    let compressed_bytes = pinned.identity.size;
    let mut planned = Vec::with_capacity(entries.len());
    let mut seen = std::collections::BTreeSet::new();
    let mut expanded = 0u64;
    let mut encrypted = false;
    for entry in entries {
        let safe = validate_member_path(&entry.path)
            .map_err(|reason| explain_member_refusal(&entry.path, reason))?;
        let key = member_path::case_fold_key(&safe.to_string_lossy());
        if !seen.insert(key) {
            return Err("case-folding member collision".into());
        }
        encrypted |= entry.encrypted;
        expanded = expanded
            .checked_add(entry.size)
            .ok_or("expanded size overflow")?;
        if expanded > MAX_EXPANDED_BYTES
            || (compressed_bytes > 0 && expanded / compressed_bytes > MAX_COMPRESSION_RATIO)
        {
            return Err("archive expansion exceeds safety policy".into());
        }
        planned.push(ArchiveEntryPlan {
            path: safe.to_string_lossy().into_owned(),
            directory: entry.kind == ExternalEntryKind::Directory,
            logical_size: entry.size,
        });
    }
    planned.sort_by(|a, b| a.path.cmp(&b.path));
    let mut warnings = Vec::new();
    let eligibility = if encrypted {
        warnings.push("Encrypted archive requires an explicit password workflow.".into());
        ArchiveEligibility::PasswordRequired
    } else if let Err(reason) = check_destination(destination) {
        warnings.push(format!("Destination is not usable: {reason}"));
        ArchiveEligibility::DestinationUnsafe
    } else {
        ArchiveEligibility::Ready
    };
    Ok(ArchivePlan {
        operation: ArchiveOperation::ExtractArchive,
        format,
        source: source.into(),
        destination: destination.into(),
        entries: planned,
        compressed_bytes: Some(compressed_bytes),
        expanded_bytes: expanded,
        eligibility,
        warnings,
        source_identity: Some(pinned.identity),
    })
}

fn looks_like_multipart_archive(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".rar")
        && let Some(part) = lower
            .strip_suffix(".rar")
            .and_then(|stem| stem.rsplit_once(".part"))
    {
        return part.1.chars().all(|ch| ch.is_ascii_digit());
    }
    lower.len() >= 4
        && !lower.ends_with(".rar")
        && lower.as_bytes()[lower.len() - 4] == b'.'
        && lower.as_bytes()[lower.len() - 3] == b'r'
        && lower[lower.len() - 2..]
            .chars()
            .all(|ch| ch.is_ascii_digit())
}

/// The destination shape every extraction requires: an absolute path of
/// plain components whose parent is an existing directory and which does
/// not itself exist yet. Returns the destination's parent and file name.
fn check_destination(destination: &Path) -> Result<(PathBuf, std::ffi::OsString), String> {
    if !destination.is_absolute() {
        return Err("destination must be an absolute path".into());
    }
    if !destination
        .components()
        .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err("destination must not contain `.` or `..` components".into());
    }
    let name = destination
        .file_name()
        .ok_or("destination has no file name")?
        .to_os_string();
    let parent = destination
        .parent()
        .ok_or("destination has no parent")?
        .to_path_buf();
    if !fs::symlink_metadata(&parent)
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        // Note: a symlinked parent is accepted here (a library may live
        // behind a symlink); extraction pins its canonical directory and
        // stays relative to that descriptor from then on.
        if !parent.is_dir() {
            return Err("destination parent is not a directory".into());
        }
    }
    if fs::symlink_metadata(destination).is_ok() {
        return Err("destination already exists; refusing overwrite".into());
    }
    Ok((parent, name))
}

/// Walks the staged tree through the pinned stage descriptor and returns
/// every destination-relative path with the total byte count. Refuses a
/// symlink, a special file, a hardlinked file (link count above one) or a
/// path that is not plain.
struct StagedTree {
    files: std::collections::BTreeMap<String, u64>,
    directories: std::collections::BTreeSet<String>,
    bytes: u64,
}

fn collect_tree(root: &Path) -> Result<StagedTree, String> {
    fn visit(
        root: &Path,
        current: &Path,
        tree: &mut StagedTree,
        count: &mut usize,
    ) -> Result<(), String> {
        for entry in fs::read_dir(current).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.file_type().is_symlink() || (!meta.is_dir() && !meta.is_file()) {
                return Err("extracted symlink or special file detected".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.is_file() && meta.nlink() > 1 {
                    return Err("extracted hardlinked file detected".into());
                }
            }
            *count += 1;
            if *count > MAX_ENTRIES {
                return Err("extracted tree has too many entries".into());
            }
            let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
            if !member_path::is_plain_relative_path(relative) {
                return Err("extracted path is not a plain relative path".into());
            }
            let relative = relative.to_string_lossy().into_owned();
            if member_path::check_member_name(&relative).is_err() {
                return Err(format!("extracted path refused: {relative:?}"));
            }
            if meta.is_file() {
                if tree.files.insert(relative, meta.len()).is_some() {
                    return Err("duplicate extracted path".into());
                }
                tree.bytes = tree
                    .bytes
                    .checked_add(meta.len())
                    .ok_or("output size overflow")?;
            } else {
                if !tree.directories.insert(relative) {
                    return Err("duplicate extracted path".into());
                }
                visit(root, &path, tree, count)?;
            }
        }
        Ok(())
    }
    let mut tree = StagedTree {
        files: std::collections::BTreeMap::new(),
        directories: std::collections::BTreeSet::new(),
        bytes: 0,
    };
    let mut count = 0;
    visit(root, root, &mut tree, &mut count)?;
    Ok(tree)
}

/// Verifies the staged tree against the plan: exactly the planned files with
/// exactly their sizes; every planned directory present; no directory that
/// is neither planned nor an ancestor of a planned entry (7-Zip creates
/// parents of nested members even when the archive lists no directory
/// items, which is the common shape of `7z a x.7z Game/disc.iso`).
fn verify_staged_tree(plan: &ArchivePlan, tree: &StagedTree) -> Result<(), String> {
    let refuse = |detail: &str| {
        Err(format!(
            "extraction verification failed ({:?}): {detail}",
            ArchiveEligibility::ExtractionVerificationFailed
        ))
    };
    let expected_files: std::collections::BTreeMap<&str, u64> = plan
        .entries
        .iter()
        .filter(|entry| !entry.directory)
        .map(|entry| (entry.path.as_str(), entry.logical_size))
        .collect();
    if tree.files.len() != expected_files.len() {
        return refuse("unexpected or missing files");
    }
    for (path, size) in &tree.files {
        match expected_files.get(path.as_str()) {
            Some(expected) if expected == size => {}
            Some(_) => return refuse(&format!("size mismatch for {path}")),
            None => return refuse(&format!("unexpected file {path}")),
        }
    }
    let mut allowed_directories = std::collections::BTreeSet::new();
    for entry in &plan.entries {
        if entry.directory {
            allowed_directories.insert(entry.path.clone());
        }
        let mut ancestor = Path::new(&entry.path);
        while let Some(parent) = ancestor.parent() {
            if parent.as_os_str().is_empty() {
                break;
            }
            allowed_directories.insert(parent.to_string_lossy().into_owned());
            ancestor = parent;
        }
    }
    for directory in &tree.directories {
        if !allowed_directories.contains(directory) {
            return refuse(&format!("unexpected directory {directory}"));
        }
    }
    for entry in plan.entries.iter().filter(|entry| entry.directory) {
        if !tree.directories.contains(&entry.path) {
            return refuse(&format!("missing directory {}", entry.path));
        }
    }
    if tree.bytes != plan.expanded_bytes {
        return refuse("total size mismatch");
    }
    Ok(())
}

/// Removes stage directories in `parent` left by an EmuWiz process that no
/// longer exists. Only directories with this module's prefix *and* its
/// owner marker are touched; a stage whose owner is still alive (another
/// extraction into the same parent) is left alone.
#[cfg(unix)]
fn sweep_stale_stages(parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_stage = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(STAGE_PREFIX));
        if !is_stage || !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        // The marker is read like any untrusted file in a user-writable
        // directory: a regular, small file, opened without blocking (a FIFO or
        // a symlink to a device planted there must not hang or flood this).
        let marker_path = path.join(STAGE_MARKER_NAME);
        let Ok(marker_meta) = fs::symlink_metadata(&marker_path) else {
            continue;
        };
        if !marker_meta.is_file() || marker_meta.len() > 256 {
            continue;
        }
        let Ok(marker_file) = crate::safe_read::open_regular_file_nonblocking(&marker_path, false)
        else {
            continue;
        };
        let mut marker = String::new();
        let mut bounded = std::io::Read::take(marker_file, 256);
        if std::io::Read::read_to_string(&mut bounded, &mut marker).is_err() {
            continue;
        }
        let Some(pid) = marker
            .trim()
            .strip_prefix("emuwiz-archive-stage owner-pid=")
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() || Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }
        let _ = fs::remove_dir_all(&path);
    }
}

fn write_stage_marker(stage: &Path) -> Result<(), String> {
    fs::write(
        stage.join(STAGE_MARKER_NAME),
        format!("emuwiz-archive-stage owner-pid={}\n", std::process::id()),
    )
    .map_err(|e| format!("could not mark staging directory: {e}"))
}

#[cfg(unix)]
fn available_bytes(directory: &Path) -> Result<u64, String> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(directory.as_os_str().as_bytes())
        .map_err(|_| "destination path contains NUL".to_string())?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is a valid NUL-terminated path and `stats` is a
    // writable, correctly sized struct.
    if unsafe { libc::statvfs(name.as_ptr(), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // The statvfs field types differ by platform; widen explicitly.
    fn widen<T: Into<u64>>(value: T) -> u64 {
        value.into()
    }
    Ok(widen(stats.f_bavail).saturating_mul(widen(stats.f_frsize)))
}

#[cfg(unix)]
fn rename_noreplace(
    parent: &fs::File,
    from: &std::ffi::OsStr,
    to: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in name"))?;
    let to = std::ffi::CString::new(to.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in name"))?;
    // SAFETY: both names are valid NUL-terminated single components and
    // `parent` is a live directory descriptor; RENAME_NOREPLACE makes the
    // kernel refuse (EEXIST) rather than replace an existing destination.
    let result = unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn extract_external(
    plan: &ArchivePlan,
    tools: &ArchiveTools,
) -> Result<ArchiveOperationResult, String> {
    use std::os::fd::AsRawFd;
    if !matches!(plan.format, ArchiveFormat::SevenZ | ArchiveFormat::Rar)
        || plan.eligibility != ArchiveEligibility::Ready
    {
        return Err("archive plan is not executable".into());
    }
    let expected_identity = plan
        .source_identity
        .ok_or("archive plan is not bound to its source archive; inspect it first")?;
    let executable = tools
        .executable_for(plan.format)
        .map_err(|e| format!("tool unavailable: {e:?}"))?;

    // 1. Pin the destination parent by descriptor and check the name is free.
    let (parent, destination_name) = check_destination(&plan.destination)?;
    let parent_canonical = fs::canonicalize(&parent).map_err(|e| e.to_string())?;
    let parent_fd = crate::safe_read::open_directory_no_symlinks(&parent_canonical)
        .map_err(|refusal| format!("destination parent refused: {}", refusal.detail()))?;
    sweep_stale_stages(&parent_canonical);

    // 2. Pin the archive again and require the identity the plan was made from.
    let pinned = pin_source(&plan.source)?;
    if pinned.identity != expected_identity {
        return Err(format!(
            "archive changed since it was inspected ({:?}); inspect it again",
            ArchiveEligibility::SourceChanged
        ));
    }
    let handler = sevenz_handler(plan.format, &pinned.signature).map_err(|reason| {
        format!("archive refused ({reason:?}): signature does not match the plan")
    })?;
    tools
        .check_handler_support(handler)
        .map_err(|reason| format!("tool unavailable: {reason:?}"))?;

    // 3. Room for the whole expansion, before anything is written.
    let available = available_bytes(&parent_canonical)?;
    let required = plan
        .expanded_bytes
        .checked_add(FREE_SPACE_MARGIN_BYTES)
        .ok_or("expanded size overflow")?;
    if available < required {
        return Err(format!(
            "insufficient free space for extraction ({:?}): {available} bytes available, {required} required",
            ArchiveEligibility::InsufficientSpace
        ));
    }

    // 4. A fresh, unique stage inside the pinned parent. `TempDir` removes it
    //    on every early return and on panic unwind until `keep()`.
    let stage = tempfile::Builder::new()
        .prefix(STAGE_PREFIX)
        .tempdir_in(&parent_canonical)
        .map_err(|e| format!("could not create staging directory: {e}"))?;
    let stage_name = stage
        .path()
        .file_name()
        .ok_or("staging directory has no name")?
        .to_os_string();
    let stage_fd = crate::safe_read::open_subdirectory_no_follow(&parent_fd, &stage_name)
        .map_err(|e| format!("could not pin staging directory: {e}"))?;
    let stage_via_fd = PathBuf::from(proc_self_fd(&stage_fd));
    write_stage_marker(&stage_via_fd)?;

    // 5. Extract from the pinned archive into the pinned stage, bounded.
    let largest_member = plan
        .entries
        .iter()
        .map(|entry| entry.logical_size)
        .max()
        .unwrap_or(0)
        .max(MIN_OUTPUT_FILE_CAP_BYTES);
    let mut command = Command::new(executable);
    command.env("LC_ALL", "C").env("LANG", "C");
    command.args(["x", "-y", "-p-", "-sccUTF-8", "-bd", "-bb0", "-spd"]);
    command.arg(format!("-t{handler}"));
    command.arg(format!("-o{}", stage_via_fd.display()));
    command.arg("--").arg(proc_self_fd(&pinned.file));
    let outcome = run_supervised(
        command,
        EXTRACT_LIMITS,
        tools.extract_timeout,
        EXTRACT_STDOUT_LIMIT,
        |_| Ok(()),
        Some(inherit_fds_and_cap_file_size(
            vec![pinned.file.as_raw_fd(), stage_fd.as_raw_fd()],
            largest_member,
        )),
    )
    .map_err(|error| {
        format!(
            "external extraction failed ({:?}): {error}; staged output discarded",
            classify_tool_failure(error.clone())
        )
    })?;
    if !outcome.status.success() {
        return Err("external extraction failed; staged output discarded".into());
    }

    // 6. Verify the staged tree against the plan, through the stage descriptor.
    //    The owner marker is removed first (it is not archive content); a
    //    marker the tool replaced with something else fails the check.
    let marker = stage_via_fd.join(STAGE_MARKER_NAME);
    let marker_meta = fs::symlink_metadata(&marker).map_err(|e| e.to_string())?;
    if !marker_meta.is_file() {
        return Err("staging marker was replaced; staged output discarded".into());
    }
    fs::remove_file(&marker).map_err(|e| e.to_string())?;
    let tree = collect_tree(&stage_via_fd)?;
    verify_staged_tree(plan, &tree)?;
    let output_bytes = tree.bytes;

    // 7. Publish by one exclusive rename relative to the pinned parent, with
    //    a library-readable mode (the stage was created private).
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `fchmod` on the live stage descriptor only changes its mode.
        if unsafe { libc::fchmod(stage_fd.as_raw_fd(), PUBLISHED_DIRECTORY_MODE) } != 0 {
            return Err(format!(
                "could not set the destination mode: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    let stage_path = stage.keep();
    if let Err(error) = rename_noreplace(&parent_fd, &stage_name, &destination_name) {
        let _ = fs::remove_dir_all(&stage_path);
        return Err(format!("could not publish extracted archive: {error}"));
    }
    Ok(ArchiveOperationResult {
        operation: ArchiveOperation::ExtractArchive,
        format: plan.format,
        source: plan.source.clone(),
        destination: plan.destination.clone(),
        member_count: plan.entries.len(),
        source_bytes: plan.compressed_bytes.unwrap_or(0),
        output_bytes,
        expanded_bytes: plan.expanded_bytes,
        verification:
            "pinned archive identity, strict listing, supervised extraction into a pinned stage, regular-file paths and sizes verified, exclusive publish"
                .into(),
        warnings: vec!["Source archive remains untouched.".into()],
    })
}

#[cfg(not(unix))]
fn extract_external(
    _plan: &ArchivePlan,
    _tools: &ArchiveTools,
) -> Result<ArchiveOperationResult, String> {
    Err("external archive extraction is supported on Unix only".into())
}

pub fn extract_archive(plan: &ArchivePlan) -> Result<ArchiveOperationResult, String> {
    match plan.format {
        ArchiveFormat::Zip => extract_zip(plan),
        ArchiveFormat::SevenZ | ArchiveFormat::Rar => {
            extract_external(plan, &ArchiveTools::discover())
        }
    }
}

/// [`extract_archive`] with an explicit backend.
pub fn extract_archive_with_tools(
    plan: &ArchivePlan,
    tools: &ArchiveTools,
) -> Result<ArchiveOperationResult, String> {
    match plan.format {
        ArchiveFormat::Zip => extract_zip(plan),
        ArchiveFormat::SevenZ | ArchiveFormat::Rar => extract_external(plan, tools),
    }
}

/// ZIP-only preservation gate; the legacy String error API is kept compatible.
/// No plan validation, filesystem access, staging or cleanup occurs here.
pub fn extract_zip(_plan: &ArchivePlan) -> Result<ArchiveOperationResult, String> {
    Err(crate::zip_converter::ZipError::ApplyUnavailable.to_string())
}

/// ZIP-only preservation gate; sources and destinations are never inspected.
pub fn create_zip(
    _source_root: &Path,
    _destination: &Path,
) -> Result<ArchiveOperationResult, String> {
    Err(crate::zip_converter::ZipError::ApplyUnavailable.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "\n7-Zip 23.01 (x64)\n\nScanning the drive for archives:\n1 file, 12 bytes\n\nListing archive: /proc/self/fd/7\n\n--\nPath = /proc/self/fd/7\nType = 7z\nPhysical Size = 12\n\n----------\n";

    fn member(path: &str, size: &str, attributes: &str) -> String {
        format!(
            "Path = {path}\nSize = {size}\nPacked Size = 1\nModified = 2024-01-01 00:00:00\nAttributes = {attributes}\nCRC = 00000000\nEncrypted = -\nMethod = LZMA2:12\nBlock = 0\n\n"
        )
    }

    #[test]
    fn rejects_unsafe_names() {
        assert!(validate_member_path("../x").is_err());
        assert!(validate_member_path("/x").is_err());
        assert!(validate_member_path("C:/x").is_err());
    }
    #[test]
    fn recognizes_formats() {
        assert_eq!(
            classify_archive(Path::new("a.zip")),
            Some(ArchiveFormat::Zip)
        );
        assert_eq!(
            classify_archive(Path::new("a.rar")),
            Some(ArchiveFormat::Rar)
        );
    }

    #[test]
    fn portable_member_paths_reject_windows_traversal_absolute_and_ambiguous_names() {
        assert_eq!(
            validate_member_path(r"..\outside.bin"),
            Err(ArchiveEligibility::UnsafeMember)
        );
        assert_eq!(
            validate_member_path(r"C:\outside.bin"),
            Err(ArchiveEligibility::UnsafeMember)
        );
        assert_eq!(
            validate_member_path("C:/outside.bin"),
            Err(ArchiveEligibility::TraversalDetected)
        );
        assert_eq!(
            validate_member_path("../outside.bin"),
            Err(ArchiveEligibility::TraversalDetected)
        );
        assert_eq!(
            validate_member_path("safe/../x.bin"),
            Err(ArchiveEligibility::TraversalDetected)
        );
        assert_eq!(
            validate_member_path("./x.bin"),
            Err(ArchiveEligibility::UnsafeEntry)
        );
        assert_eq!(
            validate_member_path("a//x.bin"),
            Err(ArchiveEligibility::UnsafeEntry)
        );
        assert_eq!(
            validate_member_path("x?.bin"),
            Err(ArchiveEligibility::UnsafeMember)
        );
        assert_eq!(
            validate_member_path("new\nline.bin"),
            Err(ArchiveEligibility::UnsafeMember)
        );
        assert_eq!(
            validate_member_path("safe/nested.bin"),
            Ok(PathBuf::from("safe/nested.bin"))
        );
    }

    #[test]
    fn parses_a_real_sevenz_listing_and_ignores_the_summary_record() {
        let listing = format!(
            "{HEADER}{}{}",
            member("game", "0", "D drwxr-xr-x"),
            member("game/disc.iso", "4", "A -rw-r--r--")
        );
        let entries = parse_sevenz_slt_listing(&listing, "7z").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, ExternalEntryKind::Directory);
        assert_eq!(entries[1].path, "game/disc.iso");
        assert_eq!(entries[1].size, 4);
        assert_eq!(entries[1].kind, ExternalEntryKind::File);
    }

    #[test]
    fn parses_rar_members_listed_by_sevenz_with_unix_only_attributes() {
        let listing = format!(
            "{HEADER}Path = helloworld.txt\nFolder = -\nSize = 29\nPacked Size = 29\nModified = 2018-09-26 04:43:42.437184854\nCreated = \nAccessed = \nAttributes =  -rw-r--r--\nAlternate Stream = -\nEncrypted = -\nSolid = -\nSplit Before = -\nSplit After = -\nCRC = 95A043B4\nHost OS = Unix\nMethod = m0:17\nCharacteristics = CRC Time:uMn\nSymbolic Link = \nHard Link = \nCopy Link = \nVolume Index = \nChecksum = \nNT Security = \n\n"
        );
        let entries = parse_sevenz_slt_listing(&listing, "7z").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, ExternalEntryKind::File);
        assert_eq!(entries[0].size, 29);
    }

    /// Night Watch S01-01: a member name with an embedded newline must not
    /// be able to hide a member, forge a record or slip past the name rule.
    #[test]
    fn embedded_newlines_in_member_names_refuse_the_listing() {
        // Newline that forges a second `Size` line: duplicate key.
        let forged_key =
            format!("{HEADER}Path = evil\nSize = 1\nSize = 2\nAttributes = A -rw-r--r--\n\n");
        assert_eq!(
            parse_sevenz_slt_listing(&forged_key, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
        // Newline that produces a line with no ` = `.
        let junk_line = format!(
            "{HEADER}Path = evil\n../../etc/passwd\nSize = 1\nAttributes = A -rw-r--r--\n\n"
        );
        assert_eq!(
            parse_sevenz_slt_listing(&junk_line, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
        // Newline that produces a blank line, splitting the record so the
        // tail (the real member) has no Path and the head has no Size.
        let split = format!(
            "{HEADER}Path = shown\n\nPath = hidden/../../x\nSize = 1\nAttributes = A -rw-r--r--\n\n"
        );
        assert!(parse_sevenz_slt_listing(&split, "7z").is_err());
        // A second `Path` line in a record: duplicate key.
        let two_paths =
            format!("{HEADER}Path = shown\nPath = hidden\nSize = 1\nAttributes = A -rw-r--r--\n\n");
        assert_eq!(
            parse_sevenz_slt_listing(&two_paths, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
        // A carriage return inside the name survives parsing as a name and is
        // then refused by the member-name rule as a control character.
        let cr = format!("{HEADER}Path = a\rb\nSize = 1\nAttributes = A -rw-r--r--\n\n");
        let entries = parse_sevenz_slt_listing(&cr, "7z").unwrap();
        assert_eq!(
            validate_member_path(&entries[0].path),
            Err(ArchiveEligibility::UnsafeMember)
        );
    }

    #[test]
    fn records_missing_mandatory_keys_refuse_instead_of_being_skipped() {
        let no_size = format!("{HEADER}Path = a.bin\nAttributes = A -rw-r--r--\n\n");
        assert_eq!(
            parse_sevenz_slt_listing(&no_size, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
        let no_attributes = format!("{HEADER}Path = a.bin\nSize = 1\n\n");
        assert_eq!(
            parse_sevenz_slt_listing(&no_attributes, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
        let no_path = format!("{HEADER}Size = 1\nAttributes = A -rw-r--r--\n\n");
        assert_eq!(
            parse_sevenz_slt_listing(&no_path, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
        let no_separator = member("a.bin", "1", "A -rw-r--r--");
        assert_eq!(
            parse_sevenz_slt_listing(&no_separator, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
        assert_eq!(
            parse_sevenz_slt_listing(HEADER, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
        let bad_size = format!("{HEADER}{}", member("a.bin", "lots", "A -rw-r--r--"));
        assert_eq!(
            parse_sevenz_slt_listing(&bad_size, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
    }

    #[test]
    fn link_special_stream_and_split_members_are_refused_by_type() {
        for attributes in [
            "A lrwxrwxrwx",
            "A brw-rw----",
            "A crw-rw-rw-",
            "A prw-------",
            "A srwxrwxrwx",
            "A -rw-r--r-- extra",
        ] {
            let listing = format!("{HEADER}{}", member("x", "1", attributes));
            let result = parse_sevenz_slt_listing(&listing, "7z");
            if attributes.ends_with("extra") {
                assert!(result.is_ok(), "{attributes}");
            } else {
                assert_eq!(
                    result,
                    Err(ArchiveEligibility::UnsupportedMemberType),
                    "{attributes}"
                );
            }
        }
        let symlink = format!(
            "{HEADER}Path = l\nSize = 1\nAttributes = A -rw-r--r--\nSymbolic Link = /etc/passwd\n\n"
        );
        assert_eq!(
            parse_sevenz_slt_listing(&symlink, "7z"),
            Err(ArchiveEligibility::UnsupportedMemberType)
        );
        let hardlink =
            format!("{HEADER}Path = h\nSize = 1\nAttributes = A -rw-r--r--\nHard Link = other\n\n");
        assert_eq!(
            parse_sevenz_slt_listing(&hardlink, "7z"),
            Err(ArchiveEligibility::UnsupportedMemberType)
        );
        let stream = format!(
            "{HEADER}Path = stream\nSize = 1\nAttributes = A -rw-r--r--\nAlternate Stream = +\n\n"
        );
        assert_eq!(
            parse_sevenz_slt_listing(&stream, "7z"),
            Err(ArchiveEligibility::UnsupportedMemberType)
        );
        let split =
            format!("{HEADER}Path = s\nSize = 1\nAttributes = A -rw-r--r--\nSplit After = +\n\n");
        assert_eq!(
            parse_sevenz_slt_listing(&split, "7z"),
            Err(ArchiveEligibility::MultipartIncomplete)
        );
        let empty_attributes = format!("{HEADER}Path = e\nSize = 1\nAttributes = \n\n");
        assert_eq!(
            parse_sevenz_slt_listing(&empty_attributes, "7z"),
            Err(ArchiveEligibility::UnsupportedMemberType)
        );
        let folder_disagrees =
            format!("{HEADER}Path = d\nFolder = +\nSize = 1\nAttributes = A -rw-r--r--\n\n");
        assert_eq!(
            parse_sevenz_slt_listing(&folder_disagrees, "7z"),
            Err(ArchiveEligibility::MalformedArchive)
        );
    }

    #[test]
    fn unsafe_and_wildcard_member_names_survive_parsing_and_are_refused_with_a_reason() {
        for (name, word) in [
            ("../escape", "`..`"),
            ("/abs", "absolute"),
            ("a/../b", "`..`"),
            ("x?.bin", "wildcard"),
            ("*.bin", "wildcard"),
            ("back\\slash", "backslash"),
            ("C:/x", "drive"),
            ("-switch", "dash"),
            ("tab\there", "control"),
        ] {
            let listing = format!("{HEADER}{}", member(name, "1", "A -rw-r--r--"));
            let entries = parse_sevenz_slt_listing(&listing, "7z").unwrap();
            let refusal = validate_member_path(&entries[0].path).unwrap_err();
            let explanation = explain_member_refusal(&entries[0].path, refusal);
            assert!(explanation.contains(word), "{name}: {explanation}");
            assert!(
                explanation.contains("unsafe archive member"),
                "{name}: {explanation}"
            );
        }
    }

    #[test]
    fn encryption_is_recorded_and_case_collisions_surface_in_the_plan_stage() {
        let encrypted = format!(
            "{HEADER}Path = game.bin\nSize = 1\nAttributes = A -rw-r--r--\nEncrypted = +\n\n"
        );
        let entries = parse_sevenz_slt_listing(&encrypted, "7z").unwrap();
        assert!(entries[0].encrypted);
        let collision = format!(
            "{HEADER}{}{}",
            member("A", "1", "A -rw-r--r--"),
            member("a", "1", "A -rw-r--r--")
        );
        // The parser keeps both; the plan builder refuses the collision.
        assert_eq!(parse_sevenz_slt_listing(&collision, "7z").unwrap().len(), 2);
    }

    #[test]
    fn member_count_is_bounded_during_parsing() {
        let mut listing = String::from(HEADER);
        for index in 0..=MAX_ENTRIES {
            listing.push_str(&member(&format!("m{index}"), "1", "A -rw-r--r--"));
        }
        assert_eq!(
            parse_sevenz_slt_listing(&listing, "7z"),
            Err(ArchiveEligibility::TooManyEntries)
        );
    }

    #[test]
    fn tools_refuse_a_relative_executable_and_name_an_incapable_sevenz_honestly() {
        assert_eq!(
            ArchiveTools::with_seven_zip(PathBuf::from("7z"), RarSupport::ALL),
            ArchiveTools::none()
        );
        let tools = ArchiveTools::with_seven_zip(PathBuf::from("/usr/bin/7z"), RarSupport::NONE);
        assert!(tools.executable_for(ArchiveFormat::SevenZ).is_ok());
        // Installed but built without RAR: not "missing".
        assert_eq!(
            tools.executable_for(ArchiveFormat::Rar),
            Err(ArchiveEligibility::ToolLacksFormatSupport)
        );
        assert_eq!(
            tools.executable_for(ArchiveFormat::Zip),
            Err(ArchiveEligibility::Unsupported)
        );
        assert_eq!(
            ArchiveTools::none().executable_for(ArchiveFormat::SevenZ),
            Err(ArchiveEligibility::ToolMissing)
        );
        assert_eq!(
            ArchiveTools::none().executable_for(ArchiveFormat::Rar),
            Err(ArchiveEligibility::ToolMissing)
        );
        let rar5_only = ArchiveTools::with_seven_zip(
            PathBuf::from("/usr/bin/7z"),
            RarSupport {
                rar4: false,
                rar5: true,
            },
        );
        assert!(rar5_only.executable_for(ArchiveFormat::Rar).is_ok());
        assert!(mentions_unsupported_method(
            "ERROR: Unsupported Method : game.bin"
        ));
    }

    #[test]
    fn the_summary_type_must_match_the_pinned_handler() {
        let listing = format!("{HEADER}{}", member("a.bin", "1", "A -rw-r--r--"));
        assert!(parse_sevenz_slt_listing(&listing, "7z").is_ok());
        assert_eq!(
            parse_sevenz_slt_listing(&listing, "Rar5"),
            Err(ArchiveEligibility::Unsupported)
        );
        let zip_listed = listing.replace("Type = 7z", "Type = zip");
        assert_eq!(
            parse_sevenz_slt_listing(&zip_listed, "7z"),
            Err(ArchiveEligibility::Unsupported)
        );
    }

    #[test]
    fn handlers_follow_the_signature_not_the_name() {
        assert_eq!(
            sevenz_handler(ArchiveFormat::SevenZ, b"7z\xBC\xAF\x27\x1C\x00\x04"),
            Ok("7z")
        );
        assert_eq!(
            sevenz_handler(ArchiveFormat::Rar, b"Rar!\x1A\x07\x00\xCF"),
            Ok("Rar")
        );
        assert_eq!(
            sevenz_handler(ArchiveFormat::Rar, b"Rar!\x1A\x07\x01\x00"),
            Ok("Rar5")
        );
        // A ZIP (or anything else) under a .7z/.rar name is refused, never
        // handed to 7-Zip's auto-detection.
        assert_eq!(
            sevenz_handler(ArchiveFormat::SevenZ, b"PK\x03\x04\x14\x00"),
            Err(ArchiveEligibility::Unsupported)
        );
        assert_eq!(
            sevenz_handler(ArchiveFormat::Rar, b"7z\xBC\xAF\x27\x1C"),
            Err(ArchiveEligibility::Unsupported)
        );
        assert_eq!(
            sevenz_handler(ArchiveFormat::SevenZ, b""),
            Err(ArchiveEligibility::Unsupported)
        );
        let temp = tempfile::tempdir().unwrap();
        let renamed = temp.path().join("really-a-zip.7z");
        std::fs::write(&renamed, b"PK\x03\x04 rest").unwrap();
        assert_eq!(classify_archive(&renamed), Some(ArchiveFormat::Zip));
        assert_eq!(
            classify_archive(&temp.path().join("missing.rar")),
            Some(ArchiveFormat::Rar)
        );
    }

    #[test]
    fn staged_tree_verification_accepts_implied_parents_and_refuses_strangers() {
        let entry = |path: &str, directory: bool, logical_size: u64| ArchiveEntryPlan {
            path: path.into(),
            directory,
            logical_size,
        };
        let plan = ArchivePlan {
            operation: ArchiveOperation::ExtractArchive,
            format: ArchiveFormat::SevenZ,
            source: PathBuf::from("/a.7z"),
            destination: PathBuf::from("/out"),
            entries: vec![
                entry("Game/Nested/x.bin", false, 2),
                entry("Game/disc.iso", false, 5),
                entry("Empty", true, 0),
            ],
            compressed_bytes: Some(10),
            expanded_bytes: 7,
            eligibility: ArchiveEligibility::Ready,
            warnings: Vec::new(),
            source_identity: None,
        };
        let tree = |files: &[(&str, u64)], dirs: &[&str]| StagedTree {
            files: files.iter().map(|(p, s)| (p.to_string(), *s)).collect(),
            directories: dirs.iter().map(|d| d.to_string()).collect(),
            bytes: files.iter().map(|(_, s)| s).sum(),
        };
        let full = [("Game/Nested/x.bin", 2), ("Game/disc.iso", 5)];
        // No directory items listed, parents implied: accepted.
        assert!(verify_staged_tree(&plan, &tree(&full, &["Game", "Game/Nested", "Empty"])).is_ok());
        // Planned directory missing: refused.
        assert!(verify_staged_tree(&plan, &tree(&full, &["Game", "Game/Nested"])).is_err());
        // A directory nobody planned or implied: refused.
        assert!(
            verify_staged_tree(
                &plan,
                &tree(&full, &["Game", "Game/Nested", "Empty", "Stray"])
            )
            .is_err()
        );
        // Wrong size, extra file, missing file: refused.
        let dirs = ["Game", "Game/Nested", "Empty"];
        assert!(
            verify_staged_tree(
                &plan,
                &tree(&[("Game/Nested/x.bin", 3), ("Game/disc.iso", 5)], &dirs)
            )
            .is_err()
        );
        assert!(
            verify_staged_tree(
                &plan,
                &tree(
                    &[("Game/Nested/x.bin", 2), ("Game/disc.iso", 5), ("x", 0)],
                    &dirs
                )
            )
            .is_err()
        );
        assert!(verify_staged_tree(&plan, &tree(&[("Game/disc.iso", 5)], &dirs)).is_err());
    }

    #[test]
    fn tool_failures_are_never_reported_as_malformed_archives() {
        assert_eq!(
            classify_tool_failure(ProcessError::Io {
                detail: "spawn".into()
            }),
            ArchiveEligibility::ToolFailed
        );
        assert_eq!(
            classify_tool_failure(ProcessError::Timeout),
            ArchiveEligibility::ToolLimitExceeded
        );
        assert_eq!(
            classify_tool_failure(ProcessError::CleanupFailure { detail: "x".into() }),
            ArchiveEligibility::ToolFailed
        );
    }

    #[test]
    fn destination_rules() {
        let temp = tempfile::tempdir().unwrap();
        assert!(check_destination(Path::new("relative/out")).is_err());
        assert!(check_destination(&temp.path().join("../out")).is_err());
        assert!(check_destination(&temp.path().join("missing-parent/out")).is_err());
        std::fs::write(temp.path().join("exists"), b"").unwrap();
        assert!(check_destination(&temp.path().join("exists")).is_err());
        assert!(check_destination(&temp.path().join("fresh")).is_ok());
    }
}
