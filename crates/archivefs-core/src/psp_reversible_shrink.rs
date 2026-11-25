//! PSP ISO to CSO v1 reversible-shrink proof.
//!
//! The source is opened read-only and streamed in 2048-byte sectors. A CSO is
//! accepted only after its header/index/data can be read, the decoded stream
//! has the original length, and its SHA-256 equals the source hash. The
//! output is never replaced or deleted automatically.
//!
//! # Format (Q1 correction)
//!
//! The written file is standard CSO v1 as PPSSPP, PCSX2 and maxcso read it:
//! raw deflate blocks; index bit 31 set means the block is stored *plain*;
//! byte offset = `(index & 0x7fff_ffff) << align`, with `align = 2` in the
//! header. Files written before this correction ("legacy EmuWiz CSO") used
//! zlib-wrapped blocks, set bit 31 on *compressed* blocks and declared
//! `align = 0` while storing `offset / 4`. No standard reader can read them.
//! [`classify_cso_dialect`] tells the two apart from the header and the first
//! index entry, and [`restore_legacy_emuwiz_cso`] decodes a legacy file into a
//! *new* ISO without modifying it.
//!
//! # Results are kept apart
//!
//! [`PspCsoChecks`] records four separate results and never merges them:
//! format validity (standard header and index, every block decodable),
//! content integrity (EmuWiz's standard reader restores the source SHA-256),
//! independent reversibility (an external maxcso restores the source
//! SHA-256), and emulator compatibility, which this module never tests and
//! always reports as [`EmulatorCompatibility::NotTested`]. EmuWiz reading its
//! own output is never called independent verification.
//!
//! # Publication
//!
//! Conversion is refused before anything is written when no independent
//! validator ([`PspShrinkError::DependencyMissing`]), no safe diagnostics
//! location on the output's filesystem
//! ([`PspShrinkError::DiagnosticsUnavailable`]) or not enough free space
//! ([`PspShrinkError::InsufficientSpace`]) is available. Work happens in a
//! private stage in that diagnostics location ([`diagnostics`]); the CSO
//! appears at `output` (same-filesystem, no-clobber hard link) only after all
//! three checks pass. A failure publishes nothing and keeps the failed CSO,
//! its intent and `diagnostic.json` under `failed/`, listed in the manifest
//! ([`PspShrinkError::ConversionFailed`]). Retained evidence is never deleted
//! automatically. The source is opened read-only and is never moved,
//! replaced or deleted.

use flate2::Compression;
use flate2::read::{DeflateDecoder, ZlibDecoder};
use flate2::write::DeflateEncoder;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

mod bounded;
pub mod diagnostics;
pub use diagnostics::{
    CONFIGURE_HINT, DiagnosticsPlacement, DiagnosticsRoot, DiagnosticsSource, DiagnosticsUsage,
    diagnostics_usage,
};

pub const CSO_BLOCK_SIZE: usize = 2048;
const CSO_HEADER_SIZE: u64 = 24;
const CSO_INDEX_ENTRY_SIZE: u64 = 4;
/// Standard CSO v1: bit 31 marks a block stored without compression.
const CSO_PLAIN_FLAG: u32 = 0x8000_0000;
/// Legacy EmuWiz CSO (before the Q1 correction): bit 31 marked a *compressed*
/// block. Used only by the legacy reader.
const LEGACY_COMPRESSED_FLAG: u32 = 0x8000_0000;
const CSO_OFFSET_MASK: u32 = 0x7fff_ffff;
/// Header `align`: byte offset = index << 2. Keeps the 4-byte padding the
/// writer has always used and lifts the 2 GiB limit of `align = 0`.
pub const CSO_ALIGN_SHIFT: u8 = 2;
const TOOL_NAME: &str = "EmuWiz in-process CSO v1 writer";
/// Changed with the Q1 correction so receipts tell the formats apart. Legacy
/// files carry `emuwiz-poc/1; flate2/zlib-rs`.
const TOOL_VERSION: &str = "emuwiz-cso/2; raw-deflate; align 2; flate2/zlib-rs";
pub const LEGACY_TOOL_VERSION: &str = "emuwiz-poc/1; flate2/zlib-rs";

#[derive(Debug)]
pub enum PspShrinkError {
    Io(io::Error),
    InvalidSource(String),
    OutputExists(PathBuf),
    InvalidCso(String),
    Verification(String),
    /// No approved independent validator is available; nothing was written.
    DependencyMissing(String),
    /// No safe same-filesystem diagnostics location; nothing was written.
    DiagnosticsUnavailable(String),
    /// The output's filesystem failed the hard-link check that safe,
    /// no-overwrite publication depends on. Checked before the source is
    /// hashed or anything is converted; nothing was written.
    DestinationUnsuitable {
        problem: DestinationProblem,
        detail: String,
    },
    /// Not enough free space for the CSO, the transient restore and the
    /// retained evidence; nothing was written.
    InsufficientSpace {
        needed: u64,
        available: u64,
    },
    /// The conversion failed after work started. Nothing was published at
    /// the output path; the failed CSO and its report are kept at `evidence`.
    ConversionFailed {
        reason: String,
        evidence: PathBuf,
        manifest: PathBuf,
        /// Set when the manifest line could not be written. The evidence is
        /// still kept at `evidence`.
        manifest_error: Option<String>,
    },
}

/// Why the destination filesystem failed the hard-link check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationProblem {
    /// The filesystem cannot make hard links (for example FAT32 or exFAT).
    HardLinksUnsupported,
    /// Creating the probe file or link was not permitted.
    PermissionDenied,
    /// No space or quota left.
    StorageFull,
    /// Read-only filesystem.
    ReadOnly,
    /// Any other filesystem error.
    Other,
}

impl fmt::Display for PspShrinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::InvalidSource(message) => write!(f, "invalid PSP ISO source: {message}"),
            Self::OutputExists(path) => write!(f, "output already exists: {}", path.display()),
            Self::InvalidCso(message) => write!(f, "invalid CSO: {message}"),
            Self::Verification(message) => write!(f, "reversible verification failed: {message}"),
            Self::DependencyMissing(message) => write!(
                f,
                "CSO conversion needs an independent validator: {message}. Nothing was written."
            ),
            Self::DiagnosticsUnavailable(message) => write!(
                f,
                "no safe place for conversion evidence on this disk: {message}. Nothing was written."
            ),
            Self::DestinationUnsuitable { problem, detail } => {
                let what = match problem {
                    DestinationProblem::HardLinksUnsupported => {
                        "the output's disk cannot make hard links, which EmuWiz needs to publish a verified CSO without any risk of replacing a file. Choose an output folder on a filesystem that supports hard links (for example ext4, Btrfs or XFS; FAT32 and exFAT do not)"
                    }
                    DestinationProblem::PermissionDenied => {
                        "EmuWiz is not allowed to create files in its diagnostics folder on the output's disk"
                    }
                    DestinationProblem::StorageFull => "the output's disk is full or over quota",
                    DestinationProblem::ReadOnly => "the output's disk is mounted read-only",
                    DestinationProblem::Other => "the output's disk reported an error",
                };
                write!(f, "{what} ({detail}). Nothing was converted or written.")
            }
            Self::InsufficientSpace { needed, available } => write!(
                f,
                "not enough free space: {needed} bytes needed, {available} available. Nothing was written."
            ),
            Self::ConversionFailed {
                reason,
                evidence,
                manifest,
                manifest_error,
            } => {
                write!(
                    f,
                    "conversion failed and nothing was published: {reason}. The failed CSO and its report are kept at {}",
                    evidence.display()
                )?;
                if let Some(error) = manifest_error {
                    write!(
                        f,
                        " (they could not be listed in {}: {error})",
                        manifest.display()
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for PspShrinkError {}

impl From<io::Error> for PspShrinkError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Emulator compatibility is a separate result that conversion never claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmulatorCompatibility {
    NotTested,
}

/// The separate results of one conversion. A [`PspShrinkResult`] exists only
/// when the first three passed; each still carries its own evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspCsoChecks {
    /// Standard CSO v1, align 2, and EmuWiz's standard reader decoded every
    /// block to 2048 bytes.
    pub format_valid: bool,
    /// SHA-256 of EmuWiz's own restore (content integrity, not independent).
    pub emuwiz_restored_sha256: String,
    /// The external validator that decoded the CSO, e.g. `maxcso v1.13.0 at /usr/local/bin/maxcso`.
    pub independent_validator: String,
    /// SHA-256 of the external validator's restore (independent reversibility).
    pub independent_restored_sha256: String,
    pub emulator_compatibility: EmulatorCompatibility,
}

/// The default location of the reviewed maxcso binary. Never looked up on PATH.
pub const DEFAULT_MAXCSO_PATH: &str = "/usr/local/bin/maxcso";

/// An approved independent CSO decoder, run as a separate process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndependentCsoValidator {
    kind: ValidatorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ValidatorKind {
    Maxcso {
        path: PathBuf,
        version: String,
    },
    /// Unit tests only: a deliberately simple decoder in the test module,
    /// cross-checked against a maxcso-made fixture. Never constructible
    /// outside `cfg(test)`.
    #[cfg(test)]
    TestReference,
    /// Unit tests only: an independent reader that disagrees.
    #[cfg(test)]
    TestDisagrees,
    /// Unit tests only: runs `program args` through the same bounded
    /// runner as maxcso, with `timeout` instead of the size-based one.
    #[cfg(test)]
    TestCommand {
        program: PathBuf,
        args: Vec<String>,
        timeout: std::time::Duration,
    },
}

impl IndependentCsoValidator {
    /// Uses the maxcso at `path` (absolute) after confirming that it runs and
    /// reports a maxcso version. Anything else is `DependencyMissing`.
    pub fn maxcso_at(path: &Path) -> Result<Self, PspShrinkError> {
        let missing = |why: &str| {
            PspShrinkError::DependencyMissing(format!(
                "maxcso was not found at {} ({why}); install maxcso there to enable CSO conversion",
                path.display()
            ))
        };
        if !path.is_absolute() {
            return Err(missing("the path is not absolute"));
        }
        if !path.is_file() {
            return Err(missing("no file"));
        }
        let mut command = std::process::Command::new(path);
        command.arg("--version");
        let version =
            version_from(command, bounded::VERSION_TIMEOUT).map_err(|why| missing(&why))?;
        Ok(Self {
            kind: ValidatorKind::Maxcso {
                path: path.to_path_buf(),
                version,
            },
        })
    }

    /// [`Self::maxcso_at`] with [`DEFAULT_MAXCSO_PATH`].
    pub fn locate() -> Result<Self, PspShrinkError> {
        Self::maxcso_at(Path::new(DEFAULT_MAXCSO_PATH))
    }

    pub fn describe(&self) -> String {
        match &self.kind {
            ValidatorKind::Maxcso { path, version } => format!("{version} at {}", path.display()),
            #[cfg(test)]
            ValidatorKind::TestReference => "test reference decoder (unit tests only)".to_string(),
            #[cfg(test)]
            ValidatorKind::TestDisagrees => {
                "disagreeing test decoder (unit tests only)".to_string()
            }
            #[cfg(test)]
            ValidatorKind::TestCommand { program, .. } => {
                format!("test command {} (unit tests only)", program.display())
            }
        }
    }

    /// Decodes `cso` (of an `iso_size`-byte ISO) into `workdir` and returns
    /// the SHA-256 of the result. The restored copy is removed before returning.
    fn restored_sha256(
        &self,
        cso: &Path,
        workdir: &Path,
        iso_size: u64,
    ) -> Result<String, PspShrinkError> {
        match &self.kind {
            ValidatorKind::Maxcso { path, .. } => {
                independent_restore_sha256_with_maxcso(path, cso, workdir, iso_size)
            }
            #[cfg(test)]
            ValidatorKind::TestReference => tests::reference_restore_sha256(cso),
            #[cfg(test)]
            ValidatorKind::TestDisagrees => Ok("0".repeat(64)),
            #[cfg(test)]
            ValidatorKind::TestCommand {
                program,
                args,
                timeout,
            } => {
                let mut command = std::process::Command::new(program);
                command.args(args);
                restore_with_bounded_command(command, &workdir.join(MAXCSO_RESTORE_NAME), *timeout)
            }
        }
    }
}

/// Runs a `--version` command within `timeout` and returns its maxcso
/// version line, or why there is none.
fn version_from(
    command: std::process::Command,
    timeout: std::time::Duration,
) -> Result<String, String> {
    match bounded::run_bounded(command, timeout) {
        // maxcso 1.13.0 prints its version to stderr and exits 1, so the exit
        // status is not used; the version line is the identity check.
        Ok(finished) => maxcso_version_line(&finished.stdout_tail, &finished.stderr_tail)
            .ok_or_else(|| "it did not report a maxcso version".to_string()),
        Err(bounded::BoundedError::Spawn(_)) => Err("it could not be run".to_string()),
        Err(bounded::BoundedError::Wait(error)) => Err(format!("waiting for it failed: {error}")),
        Err(bounded::BoundedError::TimedOut {
            after,
            stop_error,
            reaped,
            pid,
            ..
        }) => Err(format!(
            "it (process {pid}) did not answer --version within {} s{}",
            after.as_secs(),
            bounded::stop_summary(stop_error.as_deref(), reaped)
        )),
    }
}

/// The first line beginning `maxcso v` on stdout, else on stderr.
fn maxcso_version_line(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    [stdout, stderr].into_iter().find_map(|stream| {
        String::from_utf8_lossy(stream)
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("maxcso v"))
            .map(str::to_string)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspShrinkInspection {
    pub source: PathBuf,
    pub source_size: u64,
    pub source_sha256: String,
    pub identity_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspShrinkProvenance {
    pub source: PathBuf,
    pub output: PathBuf,
    pub source_sha256: String,
    pub restored_sha256: Option<String>,
    pub tool: String,
    pub tool_version: String,
    pub options: String,
    pub format: String,
    pub block_size: u32,
    pub identity_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspShrinkResult {
    pub inspection: PspShrinkInspection,
    pub compressed_size: Option<u64>,
    pub restored_sha256: Option<String>,
    pub checks: PspCsoChecks,
    pub provenance: PspShrinkProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestFailure {
    None,
    Tool,
    HashMismatch,
    /// Simulates a process exit after the CSO was written: returns with no
    /// handling at all, as a crash would.
    Interrupt,
}

/// Inspect only. No output is opened for writing and the source is never
/// modified. A PSP ISO must be sector aligned; boot evidence is retained as
/// conservative source facts for the later PSP identity adapter.
pub fn inspect_psp_iso(source: &Path) -> Result<PspShrinkInspection, PspShrinkError> {
    let mut input = File::open(source)?;
    let size = input.metadata()?.len();
    if size == 0 || size % CSO_BLOCK_SIZE as u64 != 0 {
        return Err(PspShrinkError::InvalidSource(format!(
            "size {size} is not a non-zero multiple of {CSO_BLOCK_SIZE}"
        )));
    }
    let source_sha256 = hash_reader(&mut input)?;
    Ok(PspShrinkInspection {
        source: source.to_path_buf(),
        source_size: size,
        source_sha256,
        identity_evidence: vec![
            "source format: PSP ISO candidate".to_string(),
            "source opened read-only".to_string(),
            "existing PSP boot/layout evidence remains the identity integration point".to_string(),
        ],
    })
}

/// Free space a conversion needs on the output filesystem: the CSO (at most
/// the source plus its index), one transient restored ISO (the two restores
/// run one after the other and each is removed before the next), and a
/// margin. A failed CSO stays as evidence, which the first term covers.
pub fn required_free_bytes(source_size: u64) -> u64 {
    let blocks = source_size / CSO_BLOCK_SIZE as u64;
    let cso_max = source_size + CSO_HEADER_SIZE + (blocks + 1) * CSO_INDEX_ENTRY_SIZE + 4 * blocks;
    cso_max
        .saturating_add(source_size)
        .saturating_add(diagnostics::FREE_SPACE_MARGIN)
}

const STAGED_CSO: &str = "output.cso";
const RESTORED_ISO: &str = "restored.iso";

/// Convert an ISO to standard CSO v1 and prove byte-exact restoration with
/// both EmuWiz's standard reader and the independent `validator`.
///
/// Refused before anything is written when the validator, a safe
/// same-filesystem diagnostics location or the free space is missing. Work
/// happens in a private stage on the output's filesystem. Every check passes
/// -> no-clobber hard link to `output`. Anything fails -> nothing at
/// `output`; the failed CSO and its report are kept and listed in the
/// manifest. The source ISO is only ever opened for reading.
pub fn convert_psp_iso_to_cso(
    source: &Path,
    output: &Path,
    validator: &IndependentCsoValidator,
    placement: &DiagnosticsPlacement,
) -> Result<PspShrinkResult, PspShrinkError> {
    convert_internal(source, output, validator, placement, TestFailure::None)
}

fn convert_internal(
    source: &Path,
    output: &Path,
    validator: &IndependentCsoValidator,
    placement: &DiagnosticsPlacement,
    failure: TestFailure,
) -> Result<PspShrinkResult, PspShrinkError> {
    if fs::symlink_metadata(output).is_ok() {
        return Err(PspShrinkError::OutputExists(output.to_path_buf()));
    }
    let output_dir = match output.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    // The destination checks (safe diagnostics folder, hard-link probe on the
    // output's filesystem) come before the source is hashed or converted.
    let root = DiagnosticsRoot::resolve(output_dir, placement)?;
    let inspection = inspect_psp_iso(source)?;
    if failure == TestFailure::Tool {
        return Err(PspShrinkError::Verification(
            "conversion tool failed before producing trusted output".to_string(),
        ));
    }
    let needed = required_free_bytes(inspection.source_size);
    let available = root.available_bytes()?;
    if available < needed {
        return Err(PspShrinkError::InsufficientSpace { needed, available });
    }

    let options = "CSO v1; independent 2048-byte blocks; raw deflate level 6; align 2";
    let intent = serde_json::json!({
        "kind": "emuwiz-psp-iso-to-cso",
        "source": source.to_string_lossy(),
        "source_size": inspection.source_size,
        "source_sha256": inspection.source_sha256,
        "intended_output": output.to_string_lossy(),
        "writer": TOOL_NAME,
        "writer_version": TOOL_VERSION,
        "options": options,
        "independent_validator": validator.describe(),
        "diagnostics_location": format!("{:?} {}", root.source(), root.path().display()),
        "diagnostics_candidates_skipped": root.skipped(),
        "started_unix": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "pid": std::process::id(),
    });
    let stage = root.create_stage(&intent)?;
    let staged = stage.path.join(STAGED_CSO);

    let written = (|| -> Result<u64, PspShrinkError> {
        let mut input = File::open(source)?;
        let mut cso = OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&staged)?;
        write_cso_v1(&mut input, &mut cso, inspection.source_size)?;
        cso.flush()?;
        cso.sync_all()?;
        Ok(cso.metadata()?.len())
    })();
    let compressed_size = match written {
        Ok(size) => size,
        Err(error) => {
            let reason = format!("writing the CSO: {error}");
            return Err(retain(&root, stage, &intent, &reason));
        }
    };
    if failure == TestFailure::Interrupt {
        // As a process exit would: no handling, the stage stays in `staging/`.
        return Err(PspShrinkError::Verification(
            "simulated interruption".into(),
        ));
    }
    let checks = match verify_staged(&stage.path, &staged, &inspection, validator, failure) {
        Ok(checks) => checks,
        Err(reason) => return Err(retain(&root, stage, &intent, &reason)),
    };

    // No-clobber publication on the same filesystem.
    if let Err(error) = fs::hard_link(&staged, output) {
        let reason = if error.kind() == io::ErrorKind::AlreadyExists {
            format!(
                "a file appeared at {} during conversion; it was not replaced",
                output.display()
            )
        } else {
            format!("publishing to {}: {error}", output.display())
        };
        return Err(retain(&root, stage, &intent, &reason));
    }
    stage.remove_after_publication();

    let provenance = PspShrinkProvenance {
        source: source.to_path_buf(),
        output: output.to_path_buf(),
        source_sha256: inspection.source_sha256.clone(),
        restored_sha256: Some(checks.emuwiz_restored_sha256.clone()),
        tool: TOOL_NAME.to_string(),
        tool_version: TOOL_VERSION.to_string(),
        options: options.to_string(),
        format: "CSO v1".to_string(),
        block_size: CSO_BLOCK_SIZE as u32,
        identity_evidence: inspection.identity_evidence.clone(),
    };
    Ok(PspShrinkResult {
        inspection,
        compressed_size: Some(compressed_size),
        restored_sha256: Some(checks.emuwiz_restored_sha256.clone()),
        checks,
        provenance,
    })
}

/// Keeps a failed stage as evidence. The transient restored ISO (derived,
/// full-size, its hash already in the report) is the only file removed.
fn retain(
    root: &DiagnosticsRoot,
    stage: diagnostics::Stage,
    intent: &serde_json::Value,
    reason: &str,
) -> PspShrinkError {
    let _ = fs::remove_file(stage.path.join(RESTORED_ISO));
    let _ = fs::remove_file(stage.path.join(MAXCSO_RESTORE_NAME));
    let staged = stage.path.join(STAGED_CSO);
    let (cso_size, cso_sha256) = match File::open(&staged) {
        Ok(mut file) => (
            file.metadata().map(|meta| meta.len()).ok(),
            hash_reader(&mut file).ok(),
        ),
        Err(_) => (None, None),
    };
    let report = serde_json::json!({
        "result": "failed; nothing was published at the intended output",
        "reason": reason,
        "intent": intent,
        "failed_cso": if cso_size.is_some() { STAGED_CSO } else { "(no CSO was written)" },
        "failed_cso_size": cso_size,
        "failed_cso_sha256": cso_sha256,
        "emulator_compatibility": "not tested",
    });
    root.retain_failure(stage, &report, reason)
}

/// Runs the three publication checks in order. Each failure names its check.
fn verify_staged(
    stage: &Path,
    staged: &Path,
    inspection: &PspShrinkInspection,
    validator: &IndependentCsoValidator,
    failure: TestFailure,
) -> Result<PspCsoChecks, String> {
    // 1. Format validity.
    match classify_cso_dialect(staged) {
        Ok(CsoDialect::Standard { align }) if align == CSO_ALIGN_SHIFT => {}
        Ok(other) => {
            return Err(format!(
                "format validity: written CSO classified as {other:?}"
            ));
        }
        Err(error) => return Err(format!("format validity: {error}")),
    }
    let restored = stage.join(RESTORED_ISO);
    restore_standard_cso(staged, &restored, inspection.source_size)
        .map_err(|error| format!("format validity: EmuWiz standard reader: {error}"))?;

    // 2. Content integrity (EmuWiz's own reader; not independent).
    let io_failure = |error: io::Error| format!("content integrity: {error}");
    if failure == TestFailure::HashMismatch {
        let mut altered = OpenOptions::new()
            .write(true)
            .open(&restored)
            .map_err(io_failure)?;
        altered.write_all(&[0xff]).map_err(io_failure)?;
        altered.sync_all().map_err(io_failure)?;
    }
    let emuwiz_restored_sha256 =
        hash_reader(&mut File::open(&restored).map_err(io_failure)?).map_err(io_failure)?;
    fs::remove_file(&restored).map_err(io_failure)?;
    if emuwiz_restored_sha256 != inspection.source_sha256 {
        return Err(format!(
            "content integrity: EmuWiz restore SHA-256 {emuwiz_restored_sha256} differs from source {}",
            inspection.source_sha256
        ));
    }

    // 3. Independent reversibility.
    let independent_restored_sha256 = validator
        .restored_sha256(staged, stage, inspection.source_size)
        .map_err(|error| {
            format!(
                "independent reversibility ({}): {error}",
                validator.describe()
            )
        })?;
    if independent_restored_sha256 != inspection.source_sha256 {
        return Err(format!(
            "independent reversibility ({}): restore SHA-256 {independent_restored_sha256} differs from source {}",
            validator.describe(),
            inspection.source_sha256
        ));
    }
    Ok(PspCsoChecks {
        format_valid: true,
        emuwiz_restored_sha256,
        independent_validator: validator.describe(),
        independent_restored_sha256,
        emulator_compatibility: EmulatorCompatibility::NotTested,
    })
}

fn hash_reader(reader: &mut File) -> io::Result<String> {
    reader.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn write_cso_v1(
    input: &mut File,
    output: &mut File,
    source_size: u64,
) -> Result<(), PspShrinkError> {
    let block_count = source_size / CSO_BLOCK_SIZE as u64;
    let table_bytes = (block_count + 1)
        .checked_mul(CSO_INDEX_ENTRY_SIZE)
        .ok_or_else(|| PspShrinkError::InvalidSource("CSO index table overflow".to_string()))?;
    let data_start = CSO_HEADER_SIZE + table_bytes;
    let mut indexes = vec![
        0u32;
        usize::try_from(block_count + 1).map_err(|_| {
            PspShrinkError::InvalidSource("too many ISO blocks for CSO v1".to_string())
        })?
    ];
    output.write_all(b"CISO")?;
    output.write_all(&24u32.to_le_bytes())?;
    output.write_all(&source_size.to_le_bytes())?;
    output.write_all(&(CSO_BLOCK_SIZE as u32).to_le_bytes())?;
    output.write_all(&[1, CSO_ALIGN_SHIFT, 0, 0])?;
    output.seek(SeekFrom::Start(data_start))?;
    let mut block = vec![0u8; CSO_BLOCK_SIZE];
    let mut position = data_start;
    let data_index_count = indexes.len() - 1;
    for index in &mut indexes[..data_index_count] {
        input.read_exact(&mut block)?;
        let mut compressed = DeflateEncoder::new(Vec::new(), Compression::new(6));
        compressed.write_all(&block)?;
        let compressed = compressed.finish()?;
        let (payload, flag) = if compressed.len() < block.len() {
            (compressed.as_slice(), 0)
        } else {
            (block.as_slice(), CSO_PLAIN_FLAG)
        };
        if !position.is_multiple_of(4) {
            return Err(PspShrinkError::InvalidCso(
                "unaligned CSO data position".to_string(),
            ));
        }
        let offset = position >> CSO_ALIGN_SHIFT;
        let offset = u32::try_from(offset)
            .map_err(|_| PspShrinkError::InvalidCso("CSO v1 offset overflow".to_string()))?;
        if offset & !CSO_OFFSET_MASK != 0 {
            return Err(PspShrinkError::InvalidCso(
                "CSO v1 offset exceeds 31 bits".to_string(),
            ));
        }
        *index = offset | flag;
        output.write_all(payload)?;
        position += payload.len() as u64;
        if !position.is_multiple_of(4) {
            let padding = [0u8; 4];
            output.write_all(&padding[..(4 - position as usize % 4)])?;
            position += 4 - position % 4;
        }
    }
    let final_index = indexes.len() - 1;
    indexes[final_index] = u32::try_from(position >> CSO_ALIGN_SHIFT)
        .map_err(|_| PspShrinkError::InvalidCso("CSO v1 final offset overflow".to_string()))?;
    output.seek(SeekFrom::Start(CSO_HEADER_SIZE))?;
    for index in indexes {
        output.write_all(&index.to_le_bytes())?;
    }
    Ok(())
}

/// Decodes a **standard** CSO v1 (any valid `align`, as PPSSPP, PCSX2 and
/// maxcso read it) into a new file at `restored_path`. Read-only on the CSO.
/// Used for EmuWiz's own restore proof and checked against a maxcso-made
/// fixture so writer and reader cannot share a private interpretation.
pub fn restore_standard_cso(
    cso_path: &Path,
    restored_path: &Path,
    source_size: u64,
) -> Result<(), PspShrinkError> {
    let mut restored = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(restored_path)
        .map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => {
                PspShrinkError::OutputExists(restored_path.to_path_buf())
            }
            _ => PspShrinkError::Io(error),
        })?;
    let result = decode_standard(cso_path, &mut restored, source_size);
    if result.is_err() {
        // Only the partial file this call created (create_new above).
        drop(restored);
        let _ = fs::remove_file(restored_path);
    }
    result
}

fn decode_standard(
    cso_path: &Path,
    restored: &mut File,
    source_size: u64,
) -> Result<(), PspShrinkError> {
    let mut cso = File::open(cso_path)?;
    let mut header = [0u8; 24];
    cso.read_exact(&mut header)
        .map_err(|_| PspShrinkError::InvalidCso("truncated CSO header".to_string()))?;
    if &header[0..4] != b"CISO" || u32::from_le_bytes(header[4..8].try_into().unwrap()) != 24 {
        return Err(PspShrinkError::InvalidCso(
            "bad CSO magic or header size".to_string(),
        ));
    }
    let declared_size = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let block_size = u32::from_le_bytes(header[16..20].try_into().unwrap());
    if declared_size != source_size
        || block_size != CSO_BLOCK_SIZE as u32
        || header[20] != 1
        || header[21] > 30
    {
        return Err(PspShrinkError::InvalidCso(
            "unsupported CSO v1 header".to_string(),
        ));
    }
    let align = u32::from(header[21]);
    let unit = 1u64 << align;
    let block_count = source_size / CSO_BLOCK_SIZE as u64;
    let table_len = usize::try_from(block_count + 1)
        .map_err(|_| PspShrinkError::InvalidCso("CSO index table too large".to_string()))?;
    let cso_len = cso.metadata()?.len();
    let data_start = CSO_HEADER_SIZE + (table_len as u64 * CSO_INDEX_ENTRY_SIZE);
    // The index table must fit in the file before it is allocated.
    if data_start > cso_len {
        return Err(PspShrinkError::InvalidCso(
            "truncated CSO index table".to_string(),
        ));
    }
    let mut indexes = vec![0u32; table_len];
    for index in &mut indexes {
        let mut raw = [0u8; 4];
        cso.read_exact(&mut raw)
            .map_err(|_| PspShrinkError::InvalidCso("truncated CSO index table".to_string()))?;
        *index = u32::from_le_bytes(raw);
    }
    let mut output_size = 0u64;
    for pair in indexes.windows(2) {
        let start = u64::from(pair[0] & CSO_OFFSET_MASK) << align;
        let end = u64::from(pair[1] & CSO_OFFSET_MASK) << align;
        if start < data_start || end < start || end > cso_len {
            return Err(PspShrinkError::InvalidCso(
                "CSO block range is outside the file".to_string(),
            ));
        }
        // A stored block is 2048 bytes plus alignment padding; a compressed
        // block is smaller than that (writers store plain otherwise). Larger
        // ranges are malformed and are refused before any allocation.
        if end - start > CSO_BLOCK_SIZE as u64 + unit {
            return Err(PspShrinkError::InvalidCso(
                "CSO block range is too large".to_string(),
            ));
        }
        let len = (end - start) as usize;
        cso.seek(SeekFrom::Start(start))?;
        let mut payload = vec![0u8; len];
        cso.read_exact(&mut payload)?;
        let mut block = if pair[0] & CSO_PLAIN_FLAG == 0 {
            // Bounded: one byte past a block is enough to see an over-long one.
            let mut decoder =
                DeflateDecoder::new(payload.as_slice()).take(CSO_BLOCK_SIZE as u64 + 1);
            let mut expanded = Vec::with_capacity(CSO_BLOCK_SIZE + 1);
            decoder.read_to_end(&mut expanded).map_err(|_| {
                PspShrinkError::InvalidCso("compressed CSO block cannot be decoded".to_string())
            })?;
            expanded
        } else {
            // A plain block may be followed by alignment padding only.
            if payload.len() < CSO_BLOCK_SIZE || (payload.len() - CSO_BLOCK_SIZE) as u64 >= unit {
                return Err(PspShrinkError::InvalidCso(
                    "plain CSO block has the wrong length".to_string(),
                ));
            }
            payload.truncate(CSO_BLOCK_SIZE);
            payload
        };
        if block.len() != CSO_BLOCK_SIZE {
            return Err(PspShrinkError::InvalidCso(
                "CSO block does not expand to 2048 bytes".to_string(),
            ));
        }
        restored.write_all(&block)?;
        output_size += block.len() as u64;
        block.clear();
    }
    if output_size != source_size {
        return Err(PspShrinkError::Verification(
            "restored length differs from source".to_string(),
        ));
    }
    restored.sync_all()?;
    Ok(())
}

const MAXCSO_RESTORE_NAME: &str = "maxcso-restored.iso";

/// Independent check: decodes `cso` with an external maxcso binary (ISC
/// licence, run as a separate process, never bundled) into `workdir` and
/// returns the SHA-256 of what it produced. `maxcso` is a pinned absolute
/// path chosen by the caller (the tool probe), never looked up on PATH.
/// maxcso gets 120 s plus 1 s per 16 MiB of ISO (`iso_size`) and is stopped
/// through its own process handle if it takes longer.
pub fn independent_restore_sha256_with_maxcso(
    maxcso: &Path,
    cso: &Path,
    workdir: &Path,
    iso_size: u64,
) -> Result<String, PspShrinkError> {
    if !maxcso.is_absolute() {
        return Err(PspShrinkError::Verification(
            "maxcso path must be absolute".into(),
        ));
    }
    let restored = workdir.join(MAXCSO_RESTORE_NAME);
    let mut command = std::process::Command::new(maxcso);
    command
        .arg("--decompress")
        .arg(cso)
        .arg("-o")
        .arg(&restored);
    restore_with_bounded_command(command, &restored, bounded::decompress_timeout(iso_size))
}

/// Runs a decoder that writes `restored`, within `timeout`, and returns the
/// SHA-256 of what it wrote. `restored` must not exist beforehand and is
/// removed afterwards in every case. A failure or timeout is a
/// `Verification` error carrying the last part of the decoder's stderr.
fn restore_with_bounded_command(
    command: std::process::Command,
    restored: &Path,
    timeout: std::time::Duration,
) -> Result<String, PspShrinkError> {
    if fs::symlink_metadata(restored).is_ok() {
        return Err(PspShrinkError::OutputExists(restored.to_path_buf()));
    }
    let outcome = bounded::run_bounded(command, timeout);
    let result = match outcome {
        Ok(finished) if finished.status.success() && restored.is_file() => File::open(restored)
            .and_then(|mut file| hash_reader(&mut file))
            .map_err(PspShrinkError::from),
        Ok(finished) => Err(PspShrinkError::Verification(format!(
            "maxcso could not decode the CSO ({}; stderr: {})",
            finished.status,
            bounded::tail_text(&finished.stderr_tail)
        ))),
        Err(bounded::BoundedError::TimedOut {
            after,
            stop_error,
            reaped,
            pid,
            stderr_tail,
        }) => Err(PspShrinkError::Verification(format!(
            "maxcso (process {pid}) did not finish within {} s{}; stderr: {}",
            after.as_secs(),
            bounded::stop_summary(stop_error.as_deref(), reaped),
            bounded::tail_text(&stderr_tail)
        ))),
        Err(bounded::BoundedError::Spawn(error)) => Err(PspShrinkError::Verification(format!(
            "maxcso could not be started: {error}"
        ))),
        Err(bounded::BoundedError::Wait(error)) => Err(PspShrinkError::Verification(format!(
            "waiting for maxcso failed: {error}"
        ))),
    };
    match fs::remove_file(restored) {
        Ok(()) => result,
        Err(error) if error.kind() == io::ErrorKind::NotFound => result,
        Err(error) => {
            // Reported on every path, never dropped behind an earlier error.
            let note = format!(
                "the independently restored copy {} could not be removed: {error}",
                restored.display()
            );
            Err(PspShrinkError::Verification(match result {
                Ok(_) => note,
                Err(PspShrinkError::Verification(message)) => format!("{message}; {note}"),
                Err(other) => format!("{other}; {note}"),
            }))
        }
    }
}

/// Which CSO convention a file follows, decided from the header and the
/// first index entry only (at most `24 + 4 + 2` bytes are read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsoDialect {
    /// Standard CSO v1 with the declared `align`.
    Standard { align: u8 },
    /// Written by EmuWiz before the Q1 correction. Not readable by PPSSPP,
    /// PCSX2 or maxcso; recoverable with [`restore_legacy_emuwiz_cso`].
    LegacyEmuWiz,
    /// Not a CSO v1 this module recognises.
    Unknown(String),
}

/// Classifies `path` without modifying it. Read-only and bounded.
///
/// The first index entry of any CSO v1 points at the first data byte,
/// `24 + 4 * (blocks + 1)`, rounded up to the alignment unit. A legacy EmuWiz
/// file declares `align = 0` but stores that offset divided by four, which a
/// standard `align = 0` file can never do (the offset is never zero).
pub fn classify_cso_dialect(path: &Path) -> Result<CsoDialect, PspShrinkError> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 24];
    if file.read_exact(&mut header).is_err() {
        return Ok(CsoDialect::Unknown("shorter than a CSO header".into()));
    }
    if &header[0..4] != b"CISO" {
        return Ok(CsoDialect::Unknown("no CISO magic".into()));
    }
    let total = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let block_size = u32::from_le_bytes(header[16..20].try_into().unwrap());
    let (version, align) = (header[20], header[21]);
    if version > 1 || block_size == 0 || total % u64::from(block_size) != 0 {
        return Ok(CsoDialect::Unknown("not a CSO v1 layout".into()));
    }
    let blocks = total / u64::from(block_size);
    // Every size here comes from an untrusted header: checked arithmetic
    // throughout, and an implausible header is Unknown, never a panic or wrap.
    let implausible = || Ok(CsoDialect::Unknown("implausible CSO header sizes".into()));
    let Some(data_start) = blocks
        .checked_add(1)
        .and_then(|entries| entries.checked_mul(CSO_INDEX_ENTRY_SIZE))
        .and_then(|table| table.checked_add(CSO_HEADER_SIZE))
    else {
        return implausible();
    };
    let mut first = [0u8; 4];
    file.read_exact(&mut first)
        .map_err(|_| PspShrinkError::InvalidCso("truncated CSO index table".into()))?;
    let first = u64::from(u32::from_le_bytes(first) & CSO_OFFSET_MASK);
    if align == 0 && first == data_start {
        return Ok(CsoDialect::Standard { align });
    }
    if align == 0 && first.checked_mul(4) == Some(data_start) {
        return Ok(CsoDialect::LegacyEmuWiz);
    }
    if align < 32 {
        let unit = 1u64 << u32::from(align);
        let (Some(start), Some(end)) = (
            first
                .checked_shl(u32::from(align))
                .filter(|_| first.leading_zeros() >= u32::from(align)),
            data_start.checked_add(unit),
        ) else {
            return implausible();
        };
        if start >= data_start && start < end {
            return Ok(CsoDialect::Standard { align });
        }
    }
    Ok(CsoDialect::Unknown(
        "first index entry does not point at the data".into(),
    ))
}

/// Full structural confirmation of a legacy EmuWiz CSO, made by decoding
/// every block without writing anything. [`classify_cso_dialect`] is only a
/// header-level hint; recovery is offered only after this succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyConfirmation {
    pub iso_size: u64,
    pub blocks: u64,
    /// Blocks carrying the legacy "compressed" flag; each one is a complete
    /// zlib stream whose Adler-32 checksum matched.
    pub compressed_blocks: u64,
    /// SHA-256 of the decoded ISO bytes. Proves nothing about the game by
    /// itself; compare it with a recorded source hash or a DAT hash.
    pub decoded_sha256: String,
}

/// Confirms that `cso_path` is a legacy EmuWiz CSO: header hint, every index
/// range inside the file and in order, every flagged block a valid zlib
/// stream (Adler-32 checked) of exactly 2048 bytes, every other block 2048
/// bytes, and the last index ending exactly at the end of the file. Read-only.
pub fn confirm_legacy_emuwiz_cso(cso_path: &Path) -> Result<LegacyConfirmation, PspShrinkError> {
    decode_legacy(cso_path, &mut io::sink())
}

/// Decodes a confirmed legacy EmuWiz CSO into a **new** ISO at `restored`
/// (which must not exist). The CSO is opened read-only and never modified.
/// The whole file is confirmed first, so a non-legacy or damaged file never
/// produces a partial ISO; if writing fails, only `restored` is removed.
pub fn restore_legacy_emuwiz_cso(
    cso_path: &Path,
    restored: &Path,
) -> Result<LegacyConfirmation, PspShrinkError> {
    let confirmed = confirm_legacy_emuwiz_cso(cso_path)?;
    let mut out = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(restored)
        .map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => PspShrinkError::OutputExists(restored.to_path_buf()),
            _ => PspShrinkError::Io(error),
        })?;
    let result = decode_legacy(cso_path, &mut out).and_then(|written| {
        out.sync_all()?;
        if written != confirmed {
            return Err(PspShrinkError::Verification(
                "the legacy CSO changed while it was being recovered".into(),
            ));
        }
        Ok(written)
    });
    if result.is_err() {
        // This call created `restored` (create_new above); remove only that.
        let _ = fs::remove_file(restored);
    }
    result
}

fn decode_legacy<W: Write>(
    cso_path: &Path,
    out: &mut W,
) -> Result<LegacyConfirmation, PspShrinkError> {
    if classify_cso_dialect(cso_path)? != CsoDialect::LegacyEmuWiz {
        return Err(PspShrinkError::InvalidCso(
            "not a legacy EmuWiz CSO".to_string(),
        ));
    }
    let mut cso = File::open(cso_path)?;
    let mut header = [0u8; 24];
    cso.read_exact(&mut header)?;
    let total = u64::from_le_bytes(header[8..16].try_into().unwrap());
    if u32::from_le_bytes(header[16..20].try_into().unwrap()) != CSO_BLOCK_SIZE as u32
        || total == 0
        || total % CSO_BLOCK_SIZE as u64 != 0
    {
        return Err(PspShrinkError::InvalidCso(
            "legacy CSO must hold whole 2048-byte blocks".into(),
        ));
    }
    let blocks = total / CSO_BLOCK_SIZE as u64;
    let cso_len = cso.metadata()?.len();
    // The index table must fit in the file before it is allocated.
    let table_bytes = (blocks + 1) * CSO_INDEX_ENTRY_SIZE;
    if CSO_HEADER_SIZE + table_bytes > cso_len {
        return Err(PspShrinkError::InvalidCso(
            "truncated CSO index table".into(),
        ));
    }
    let mut indexes = vec![0u32; (blocks + 1) as usize];
    for index in &mut indexes {
        let mut raw = [0u8; 4];
        cso.read_exact(&mut raw)
            .map_err(|_| PspShrinkError::InvalidCso("truncated CSO index table".into()))?;
        *index = u32::from_le_bytes(raw);
    }
    let data_start = CSO_HEADER_SIZE + table_bytes;
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut compressed_blocks = 0u64;
    let mut last_end = data_start;
    for pair in indexes.windows(2) {
        let start = u64::from(pair[0] & CSO_OFFSET_MASK) * 4;
        let end = u64::from(pair[1] & CSO_OFFSET_MASK) * 4;
        if start != last_end || end < start || end > cso_len {
            return Err(PspShrinkError::InvalidCso(
                "legacy block range is out of order or outside the file".into(),
            ));
        }
        last_end = end;
        if end - start > CSO_BLOCK_SIZE as u64 + 3 {
            return Err(PspShrinkError::InvalidCso(
                "legacy block range is too large".into(),
            ));
        }
        cso.seek(SeekFrom::Start(start))?;
        let mut payload = vec![0u8; (end - start) as usize];
        cso.read_exact(&mut payload)?;
        let block = if pair[0] & LEGACY_COMPRESSED_FLAG != 0 {
            compressed_blocks += 1;
            let mut expanded = Vec::with_capacity(CSO_BLOCK_SIZE + 1);
            // ZlibDecoder checks the zlib header and the Adler-32 trailer.
            ZlibDecoder::new(payload.as_slice())
                .take(CSO_BLOCK_SIZE as u64 + 1)
                .read_to_end(&mut expanded)
                .map_err(|_| {
                    PspShrinkError::InvalidCso("legacy block is not a valid zlib stream".into())
                })?;
            expanded
        } else {
            // A legacy plain block is 2048 bytes; it is always 4-aligned.
            if payload.len() != CSO_BLOCK_SIZE {
                return Err(PspShrinkError::InvalidCso(
                    "legacy plain block is not 2048 bytes".into(),
                ));
            }
            payload
        };
        if block.len() != CSO_BLOCK_SIZE {
            return Err(PspShrinkError::InvalidCso(
                "legacy block does not expand to 2048 bytes".into(),
            ));
        }
        hasher.update(&block);
        out.write_all(&block)?;
        written += block.len() as u64;
    }
    if last_end != cso_len || written != total {
        return Err(PspShrinkError::InvalidCso(
            "legacy CSO does not end where its index ends".into(),
        ));
    }
    Ok(LegacyConfirmation {
        iso_size: total,
        blocks,
        compressed_blocks,
        decoded_sha256: hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}

#[cfg(test)]
mod tests;
