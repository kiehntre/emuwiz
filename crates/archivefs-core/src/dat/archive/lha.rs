//! Bounded, read-only LHA/LZH member hashing through an optional 7-Zip backend.
//!
//! LHA is not implemented by the in-process ZIP or 7z decoders.  Rather than
//! pretending an `.lha` filename identifies a WHDLoad package, this adapter
//! uses a locally installed 7-Zip only after it has positively advertised the
//! `Lzh` decoder.  The archive is opened once under [`TrustedRoots`], pinned
//! by file descriptor, and every child receives that descriptor through
//! `/proc/self/fd/N`; no pathname is reopened and no member is ever extracted
//! to disk.
//!
//! The provider is deliberately optional.  Missing 7-Zip is reported as an
//! unsupported LHA evidence path, not as corruption of a user package.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::identity_source::hashing::Crc32;
use crate::safe_read::{TrustedRoots, open_bounded_read};

use super::external_process::{ProcessError, ProcessLimits, run_supervised};
use super::limits::ArchiveLimits;
use super::{
    ArchiveMemberEvidence, ArchiveMemberHashes, ArchiveMemberSource, ArchiveMemberStatus,
    ArchivePassCompletion, ArchivePassOutcome, ArchivePassStopReason, ArchiveRunBudget,
};

const DISCOVERY_STDOUT_LIMIT: u64 = 2 * 1024 * 1024;
const LIST_STDOUT_LIMIT: u64 = 8 * 1024 * 1024;

/// A discovered user-installed 7-Zip executable which explicitly advertises
/// an LHA/LZH (`Lzh`) decoder.
#[derive(Debug, Clone)]
pub struct LhaProvider {
    executable: PathBuf,
    process_limits: ProcessLimits,
}

impl LhaProvider {
    /// Probes the same local 7-Zip candidates as the optional RAR provider.
    /// No shell is used and the probe is only performed when an explicit audit
    /// encounters an `.lha` file.
    pub fn discover(timeout: Duration) -> Result<Self, LhaError> {
        for candidate in [
            PathBuf::from("7zz"),
            PathBuf::from("7z"),
            PathBuf::from("/usr/lib/7zip/7z"),
        ] {
            let Some(executable) = resolve_executable(&candidate) else {
                continue;
            };
            let mut command = Command::new(&executable);
            command.arg("i");
            let mut output = Vec::new();
            let result = run_supervised(
                command,
                ProcessLimits::default(),
                timeout,
                DISCOVERY_STDOUT_LIMIT,
                |chunk| {
                    output.extend_from_slice(chunk);
                    Ok(())
                },
                None,
            );
            let Ok(result) = result else { continue };
            if !result.status.success() {
                continue;
            }
            let Ok(text) = String::from_utf8(output) else {
                continue;
            };
            if text
                .lines()
                .any(|line| line.split_ascii_whitespace().any(|field| field == "Lzh"))
            {
                return Ok(Self {
                    executable,
                    process_limits: ProcessLimits::default(),
                });
            }
        }
        Err(LhaError::BackendNotFound)
    }

    pub fn open(
        &self,
        path: &Path,
        trusted: &TrustedRoots,
        limits: ArchiveLimits,
        timeout: Duration,
    ) -> Result<LhaArchiveSource, LhaError> {
        let safe = open_bounded_read(path, trusted).map_err(|error| LhaError::Open {
            detail: format!("read policy refused LHA archive: {error:?}"),
        })?;
        let file = safe.into_file();
        let metadata = file.metadata().map_err(io_error)?;
        if !metadata.is_file() {
            return Err(LhaError::Open {
                detail: "archive path is not a regular file".to_string(),
            });
        }

        let archive_type =
            list_archive_type(&self.executable, self.process_limits, &file, timeout)?;
        if archive_type != "Lzh" {
            return Err(LhaError::Unsupported {
                detail: format!("7-Zip identified this as {archive_type}, not Lzh"),
            });
        }
        let members = list_members(&self.executable, self.process_limits, &file, timeout)?;
        if members.len() > limits.max_members {
            return Err(LhaError::RefusedLimits {
                reason: "member count",
            });
        }
        let mut paths = BTreeSet::new();
        for member in &members {
            if !paths.insert(member.path.clone()) {
                return Err(LhaError::Unsupported {
                    detail: format!("duplicate LHA member path: {}", member.path),
                });
            }
        }
        Ok(LhaArchiveSource {
            archive_path: path.to_path_buf(),
            file,
            members,
            limits,
            executable: self.executable.clone(),
            process_limits: self.process_limits,
            timeout,
            opened_len: metadata.len(),
            opened_modified: metadata.modified().ok(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LhaMember {
    path: String,
    logical_size: u64,
    packed_size: u64,
    method: String,
}

/// One fd-pinned, bounded LHA archive source.
pub struct LhaArchiveSource {
    archive_path: PathBuf,
    file: File,
    members: Vec<LhaMember>,
    limits: ArchiveLimits,
    executable: PathBuf,
    process_limits: ProcessLimits,
    timeout: Duration,
    opened_len: u64,
    opened_modified: Option<std::time::SystemTime>,
}

impl std::fmt::Debug for LhaArchiveSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LhaArchiveSource")
            .field("archive_path", &self.archive_path)
            .field("members", &self.members)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl ArchiveMemberSource for LhaArchiveSource {
    fn archive_format(&self) -> &'static str {
        "lha"
    }

    fn member_count(&self) -> usize {
        self.members.len()
    }

    fn verify_all(
        &mut self,
        cancel: &AtomicBool,
        run_budget: &mut ArchiveRunBudget,
    ) -> ArchivePassOutcome {
        let mut members = Vec::with_capacity(self.members.len());
        let mut archive_logical = 0_u64;
        let mut completion = ArchivePassCompletion::Complete;
        for (index, member) in self.members.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                completion = ArchivePassCompletion::Incomplete {
                    reason: ArchivePassStopReason::Cancelled,
                };
                break;
            }
            let raw = member.path.as_bytes().to_vec();
            let nested = is_nested_name(&member.path);
            let evidence = |status, hashes| ArchiveMemberEvidence {
                archive_path: self.archive_path.clone(),
                member_name_raw: raw.clone(),
                member_name_display: member.path.clone(),
                index,
                logical_size: member.logical_size,
                is_nested_archive: nested,
                status,
                hashes,
            };
            if !safe_member_name(&member.path) {
                members.push(evidence(
                    ArchiveMemberStatus::NotVerified {
                        reason: "unsafe member path",
                    },
                    None,
                ));
                continue;
            }
            if nested {
                members.push(evidence(ArchiveMemberStatus::NestedArchive, None));
                continue;
            }
            if member.method.trim().is_empty() {
                members.push(evidence(
                    ArchiveMemberStatus::UnsupportedCodec {
                        method: "missing LHA method".to_string(),
                    },
                    None,
                ));
                continue;
            }
            if member.logical_size == 0 {
                members.push(evidence(ArchiveMemberStatus::EmptyFile, None));
                continue;
            }
            if member.logical_size > self.limits.max_member_logical_bytes {
                members.push(evidence(
                    ArchiveMemberStatus::RefusedLimits {
                        reason: "member size",
                    },
                    None,
                ));
                continue;
            }
            if ratio_exceeded(
                member.logical_size,
                member.packed_size,
                self.limits.max_compression_ratio,
            ) {
                members.push(evidence(
                    ArchiveMemberStatus::RefusedLimits {
                        reason: "compression ratio",
                    },
                    None,
                ));
                continue;
            }
            let Some(next) = archive_logical.checked_add(member.logical_size) else {
                members.push(evidence(
                    ArchiveMemberStatus::RefusedLimits {
                        reason: "archive logical budget",
                    },
                    None,
                ));
                continue;
            };
            if next > self.limits.max_archive_logical_bytes {
                members.push(evidence(
                    ArchiveMemberStatus::RefusedLimits {
                        reason: "archive logical budget",
                    },
                    None,
                ));
                continue;
            }
            if !run_budget.try_charge(member.logical_size) {
                members.push(evidence(
                    ArchiveMemberStatus::RefusedLimits {
                        reason: "run logical budget",
                    },
                    None,
                ));
                completion = ArchivePassCompletion::Incomplete {
                    reason: ArchivePassStopReason::RunLogicalBudget,
                };
                break;
            }
            archive_logical = next;
            match hash_member(
                &self.executable,
                self.process_limits,
                &self.file,
                &member.path,
                member.logical_size,
                self.timeout,
                cancel,
            ) {
                Ok(hashes) => {
                    members.push(evidence(ArchiveMemberStatus::HashComplete, Some(hashes)))
                }
                Err(LhaError::Cancelled) => {
                    completion = ArchivePassCompletion::Incomplete {
                        reason: ArchivePassStopReason::Cancelled,
                    };
                    break;
                }
                Err(LhaError::RefusedLimits { reason }) => members.push(evidence(
                    ArchiveMemberStatus::RefusedLimits { reason },
                    None,
                )),
                Err(error) => members.push(evidence(
                    ArchiveMemberStatus::Corrupt {
                        detail: error.to_string(),
                    },
                    None,
                )),
            }
        }
        if !self.outer_identity_unchanged() {
            completion = ArchivePassCompletion::Incomplete {
                reason: ArchivePassStopReason::OuterFileChanged,
            };
        }
        ArchivePassOutcome {
            members,
            total_members: self.members.len(),
            completion,
        }
    }
}

impl LhaArchiveSource {
    fn outer_identity_unchanged(&self) -> bool {
        std::fs::metadata(&self.archive_path)
            .ok()
            .is_some_and(|metadata| {
                metadata.len() == self.opened_len
                    && metadata.modified().ok() == self.opened_modified
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LhaError {
    BackendNotFound,
    Open { detail: String },
    Corrupt { detail: String },
    Unsupported { detail: String },
    RefusedLimits { reason: &'static str },
    Cancelled,
    Timeout,
    ProcessOutputLimit { limit: u64 },
    BackendFailure { status: Option<i32>, detail: String },
    Listing { detail: String },
    SizeMismatch { declared: u64, received: u64 },
}

impl std::fmt::Display for LhaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for LhaError {}

fn list_archive_type(
    executable: &Path,
    limits: ProcessLimits,
    file: &File,
    timeout: Duration,
) -> Result<String, LhaError> {
    let text = run_listing(executable, limits, file.as_raw_fd(), false, timeout)?;
    let mut in_header = false;
    let mut properties = BTreeMap::new();
    for line in text.lines() {
        if line == "--" {
            in_header = true;
            continue;
        }
        if line == "----------" {
            break;
        }
        if in_header && !line.is_empty() {
            let (key, value) = property(line)?;
            if properties
                .insert(key.to_string(), value.to_string())
                .is_some()
            {
                return Err(LhaError::Listing {
                    detail: format!("duplicate archive property {key}"),
                });
            }
        }
    }
    properties.remove("Type").ok_or_else(|| LhaError::Listing {
        detail: "7-Zip LHA type field is missing".to_string(),
    })
}

fn list_members(
    executable: &Path,
    limits: ProcessLimits,
    file: &File,
    timeout: Duration,
) -> Result<Vec<LhaMember>, LhaError> {
    let text = run_listing(executable, limits, file.as_raw_fd(), true, timeout)?;
    let mut blocks = Vec::new();
    let mut block = BTreeMap::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !block.is_empty() {
                blocks.push(std::mem::take(&mut block));
            }
            continue;
        }
        let (key, value) = property(line)?;
        if block.insert(key.to_string(), value.to_string()).is_some() {
            return Err(LhaError::Listing {
                detail: format!("duplicate member property {key}"),
            });
        }
    }
    blocks
        .into_iter()
        .map(|properties| {
            let path = required(&properties, "Path")?.to_string();
            if path.is_empty() || path.chars().any(char::is_control) {
                return Err(LhaError::Listing {
                    detail: "empty or control-character LHA member path".to_string(),
                });
            }
            if required(&properties, "Folder")? != "-" {
                return Err(LhaError::Unsupported {
                    detail: format!("directory member is refused: {path}"),
                });
            }
            Ok(LhaMember {
                path,
                logical_size: parse_u64(required(&properties, "Size")?, "Size")?,
                packed_size: parse_u64(required(&properties, "Packed Size")?, "Packed Size")?,
                method: required(&properties, "Method")?.to_string(),
            })
        })
        .collect()
}

fn hash_member(
    executable: &Path,
    limits: ProcessLimits,
    file: &File,
    path: &str,
    declared_size: u64,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<ArchiveMemberHashes, LhaError> {
    let fd = file.as_raw_fd();
    let mut command = Command::new(executable);
    command.args(extract_args(fd, path));
    let mut hasher = StreamingHasher::new();
    let mut received = 0_u64;
    let outcome = run_supervised(
        command,
        limits,
        timeout,
        declared_size,
        |chunk| {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }
            received = received
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| "member byte count overflow".to_string())?;
            if received > declared_size {
                return Err("member output exceeds declared size".to_string());
            }
            hasher.update(chunk);
            Ok(())
        },
        Some(pin_fd_pre_exec(fd)),
    );
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(ProcessError::Sink { detail }) if detail == "cancelled" => {
            return Err(LhaError::Cancelled);
        }
        Err(error) => return Err(process_error(error)),
    };
    if !outcome.status.success() {
        return Err(LhaError::BackendFailure {
            status: outcome.status.code(),
            detail: String::from_utf8_lossy(&outcome.stderr).into_owned(),
        });
    }
    if received != declared_size {
        return Err(LhaError::SizeMismatch {
            declared: declared_size,
            received,
        });
    }
    Ok(hasher.finish())
}

fn run_listing(
    executable: &Path,
    limits: ProcessLimits,
    fd: RawFd,
    bare: bool,
    timeout: Duration,
) -> Result<String, LhaError> {
    let mut command = Command::new(executable);
    command.args(list_args(bare, fd));
    let mut output = Vec::new();
    let outcome = run_supervised(
        command,
        limits,
        timeout,
        LIST_STDOUT_LIMIT,
        |chunk| {
            output.extend_from_slice(chunk);
            Ok(())
        },
        Some(pin_fd_pre_exec(fd)),
    )
    .map_err(process_error)?;
    if !outcome.status.success() {
        return Err(LhaError::Corrupt {
            detail: format!(
                "7-Zip LHA listing failed: {}",
                String::from_utf8_lossy(&outcome.stderr)
            ),
        });
    }
    String::from_utf8(output).map_err(|_| LhaError::Listing {
        detail: "7-Zip LHA listing is not UTF-8".to_string(),
    })
}

fn list_args(bare: bool, fd: RawFd) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["l".into()];
    if bare {
        args.push("-ba".into());
    }
    for flag in ["-slt", "-p-", "-y", "-bd", "-bb0", "--"] {
        args.push(flag.into());
    }
    args.push(proc_self_fd(fd));
    args
}

fn extract_args(fd: RawFd, path: &str) -> Vec<OsString> {
    let mut args = Vec::new();
    for flag in ["x", "-so", "-p-", "-y", "-bd", "-bb0", "-spd", "-ssc", "--"] {
        args.push(flag.into());
    }
    args.push(proc_self_fd(fd));
    args.push(path.into());
    args
}

fn proc_self_fd(fd: RawFd) -> OsString {
    format!("/proc/self/fd/{fd}").into()
}

fn pin_fd_pre_exec(fd: RawFd) -> Box<dyn Fn() -> io::Result<()> + Send + Sync> {
    Box::new(move || {
        // SAFETY: this is the child-only `pre_exec` callback used by the
        // supervised process helper; `fcntl` is async-signal-safe and only
        // clears close-on-exec on this one pinned read descriptor.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, 0) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

fn property(line: &str) -> Result<(&str, &str), LhaError> {
    if line.chars().any(char::is_control) {
        return Err(LhaError::Listing {
            detail: "control character in 7-Zip listing".to_string(),
        });
    }
    let (key, value) = line.split_once(" = ").ok_or_else(|| LhaError::Listing {
        detail: format!("unparseable 7-Zip listing line: {line:?}"),
    })?;
    if key.is_empty() || key.trim() != key {
        return Err(LhaError::Listing {
            detail: format!("ambiguous 7-Zip property key: {key:?}"),
        });
    }
    Ok((key, value))
}

fn required<'a>(properties: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str, LhaError> {
    properties
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| LhaError::Listing {
            detail: format!("required 7-Zip property is missing: {key}"),
        })
}

fn parse_u64(value: &str, field: &str) -> Result<u64, LhaError> {
    value.parse().map_err(|_| LhaError::Listing {
        detail: format!("{field} is not an unsigned integer: {value:?}"),
    })
}

fn process_error(error: ProcessError) -> LhaError {
    match error {
        ProcessError::Io { detail } => LhaError::Open { detail },
        ProcessError::Timeout => LhaError::Timeout,
        ProcessError::OutputLimitExceeded { limit } => LhaError::ProcessOutputLimit { limit },
        ProcessError::InvalidLimits => LhaError::Open {
            detail: "invalid process limits".to_string(),
        },
        ProcessError::Sink { detail } => LhaError::BackendFailure {
            status: None,
            detail,
        },
        ProcessError::CleanupFailure { detail } => LhaError::BackendFailure {
            status: None,
            detail,
        },
    }
}

fn io_error(error: io::Error) -> LhaError {
    LhaError::Open {
        detail: error.to_string(),
    }
}

fn resolve_executable(candidate: &Path) -> Option<PathBuf> {
    if candidate.components().count() > 1 {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(candidate))
            .find(|path| path.is_file())
    })
}

fn safe_member_name(name: &str) -> bool {
    !name.contains('\\')
        && !name.contains('*')
        && !name.contains('?')
        && Path::new(name)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn is_nested_name(name: &str) -> bool {
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    [
        "zip", "7z", "rar", "lha", "lzh", "tar", "gz", "bz2", "xz", "zst",
    ]
    .iter()
    .any(|nested| extension.eq_ignore_ascii_case(nested))
}

fn ratio_exceeded(logical: u64, packed: u64, maximum: u64) -> bool {
    packed == 0
        || logical
            .checked_div(packed)
            .is_none_or(|ratio| ratio > maximum)
}

struct StreamingHasher {
    crc32: Crc32,
    md5: Md5,
    sha1: Sha1,
    sha256: Sha256,
}

impl StreamingHasher {
    fn new() -> Self {
        Self {
            crc32: Crc32::new(),
            md5: Md5::new(),
            sha1: Sha1::new(),
            sha256: Sha256::new(),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.crc32.update(bytes);
        self.md5.update(bytes);
        self.sha1.update(bytes);
        self.sha256.update(bytes);
    }

    fn finish(self) -> ArchiveMemberHashes {
        ArchiveMemberHashes {
            crc32: self.crc32.finish_hex(),
            md5: hex(&self.md5.finalize()),
            sha1: hex(&self.sha1.finalize()),
            sha256: hex(&self.sha256.finalize()),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_lha(name: &str, payload: &[u8]) -> Vec<u8> {
        // Level-0 LHA with a stored (`-lh0-`) member. Keeping the fixture
        // hand-built makes the production reader test independent of a
        // writer crate or a shell archiver.
        assert!(name.len() <= u8::MAX as usize);
        let header_size = name.len() + 23;
        assert!(header_size <= u8::MAX as usize);
        let mut bytes = vec![header_size as u8, 0];
        bytes.extend_from_slice(b"-lh0-");
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.push(0x20); // archive attribute
        bytes.push(0); // level 0
        bytes.push(name.len() as u8);
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&lha_crc16(payload).to_le_bytes());
        bytes.push(0); // host OS
        bytes[1] = bytes[2..]
            .iter()
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        bytes.extend_from_slice(payload);
        bytes.push(0); // no further headers
        bytes
    }

    fn lha_crc16(bytes: &[u8]) -> u16 {
        let mut crc = 0_u16;
        for byte in bytes {
            crc ^= u16::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xa001
                } else {
                    crc >> 1
                };
            }
        }
        crc
    }

    #[test]
    fn lha_listing_parser_requires_the_expected_member_facts() {
        let members = parse_members_for_test(
            "Path = Game/Game.Slave\nFolder = -\nSize = 12\nPacked Size = 7\nMethod = -lh5-\n",
        )
        .unwrap();
        assert_eq!(members[0].path, "Game/Game.Slave");
        assert_eq!(members[0].logical_size, 12);
    }

    #[test]
    fn unsafe_member_paths_and_globs_are_not_extractable() {
        for name in ["../Game.Slave", "/Game.Slave", "dir\\Game.Slave", "*.Slave"] {
            assert!(!safe_member_name(name), "{name}");
        }
        assert!(safe_member_name("Game/Game.Slave"));
    }

    #[test]
    fn extraction_uses_the_pinned_fd_and_never_an_output_directory() {
        let args = extract_args(7, "Game/Game.Slave")
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect::<Vec<_>>();
        assert!(args.contains(&"/proc/self/fd/7".to_string()));
        assert!(args.contains(&"-so".to_string()));
        assert!(!args.iter().any(|arg| arg.starts_with("-o")));
    }

    #[test]
    fn optional_backend_hashes_a_real_lha_member_without_extracting_to_disk() {
        let Ok(provider) = LhaProvider::discover(Duration::from_secs(10)) else {
            // LHA remains correctly unavailable on systems where the user
            // has not installed a capable 7-Zip. The parser tests above
            // remain deterministic there.
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("game.lha");
        std::fs::write(&path, stored_lha("Game/Game.Slave", b"fixture slave bytes")).unwrap();
        let trusted = TrustedRoots::from_paths(std::iter::once(directory.path()));
        let cancel = AtomicBool::new(false);
        let mut source = provider
            .open(
                &path,
                &trusted,
                ArchiveLimits::default(),
                Duration::from_secs(10),
            )
            .unwrap();
        let mut budget = ArchiveRunBudget::new(1024);
        let result = source.verify_all(&cancel, &mut budget);
        assert!(result.is_complete());
        assert_eq!(result.members.len(), 1);
        assert_eq!(result.members[0].status, ArchiveMemberStatus::HashComplete);
        assert_eq!(
            result.members[0].hashes.as_ref().unwrap().sha1,
            "f945cb84114db3c422f4f5e3996f138052267cf5"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            stored_lha("Game/Game.Slave", b"fixture slave bytes")
        );
    }

    fn parse_members_for_test(text: &str) -> Result<Vec<LhaMember>, LhaError> {
        let mut blocks = Vec::new();
        let mut block = BTreeMap::new();
        for line in text.lines().chain(std::iter::once("")) {
            if line.is_empty() {
                if !block.is_empty() {
                    blocks.push(std::mem::take(&mut block));
                }
                continue;
            }
            let (key, value) = property(line)?;
            block.insert(key.to_string(), value.to_string());
        }
        blocks
            .into_iter()
            .map(|properties| {
                Ok(LhaMember {
                    path: required(&properties, "Path")?.to_string(),
                    logical_size: parse_u64(required(&properties, "Size")?, "Size")?,
                    packed_size: parse_u64(required(&properties, "Packed Size")?, "Packed Size")?,
                    method: required(&properties, "Method")?.to_string(),
                })
            })
            .collect()
    }
}
