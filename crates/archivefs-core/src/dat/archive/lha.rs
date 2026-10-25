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
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::identity_source::hashing::Crc32;
use crate::safe_read::{TrustedRoots, open_bounded_read};

use super::external_process::{ProcessError, ProcessLimits, run_supervised};
use super::lha_header::{Crc16, LhaEntryKind, LhaRawEntry, scan_headers};
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
        let listing = list_members(&self.executable, self.process_limits, &file, timeout)?;
        if listing.len() > limits.max_members {
            return Err(LhaError::RefusedLimits {
                reason: "member count",
            });
        }
        // The raw headers are authoritative for member type; 7-Zip supplies
        // bytes.  Scanning is metadata-only (never reads member payloads).
        let raw = scan_headers(&file, metadata.len(), limits.max_members).map_err(|error| {
            LhaError::Corrupt {
                detail: format!("LHA header scan refused the archive: {error}"),
            }
        })?;
        let members = correlate(raw, listing)?;
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
    /// Member type proven by the raw LHA header and correlated with the
    /// 7-Zip listing entry that will supply the bytes.
    kind: LhaEntryKind,
    /// The header's CRC-16 of the decoded bytes; the streamed payload must
    /// reproduce it.
    crc16: u16,
}

/// What a streamed member must reproduce: its declared size and the CRC-16
/// from its raw header.
#[derive(Debug, Clone, Copy)]
struct MemberCheck {
    declared_size: u64,
    crc16: u16,
}

impl MemberCheck {
    fn of(member: &LhaMember) -> Self {
        Self {
            declared_size: member.logical_size,
            crc16: member.crc16,
        }
    }
}

/// One 7-Zip `-slt` member block.  Its `Folder`, `Size`, `Packed Size`,
/// `Method` and `CRC` are the only facts compared with the raw header;
/// 7-Zip exposes no member type.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ListedMember {
    path: String,
    is_folder: bool,
    logical_size: u64,
    packed_size: u64,
    method: String,
    crc: Option<u16>,
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
            // Only a header-proven regular file may reach checksum
            // verification; links, specials and unproven types never do.
            if let Some(reason) = member.kind.refusal_reason() {
                members.push(evidence(ArchiveMemberStatus::NotVerified { reason }, None));
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
                MemberCheck::of(member),
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

/// One archive member's name and declared logical size - enough for a
/// caller (e.g. WHDLoad archive-member inspection) to decide which member(s)
/// to bounded-read, without exposing the crate-private [`LhaMember`]
/// representation.
#[derive(Debug, Clone, Copy)]
pub struct LhaMemberInfo<'a> {
    pub path: &'a str,
    pub logical_size: u64,
    /// Header-proven type; only [`LhaEntryKind::Regular`] may be read.
    pub kind: &'a LhaEntryKind,
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

    /// Every member's name and declared logical size, in the same
    /// deterministic order `verify_all` uses. Read-only metadata already
    /// collected by [`LhaProvider::open`]'s listing pass - this never
    /// re-lists or re-opens the archive.
    pub fn member_infos(&self) -> impl Iterator<Item = LhaMemberInfo<'_>> {
        self.members.iter().map(|member| LhaMemberInfo {
            path: &member.path,
            logical_size: member.logical_size,
            kind: &member.kind,
        })
    }

    /// Bounded, read-only extraction of exactly one member's bytes into
    /// memory - never to disk, and never more than `max_bytes`. Reuses the
    /// same fd-pinned `/proc/self/fd` stdout-streaming extraction
    /// `verify_all`'s hashing path already uses ([`extract_args`] /
    /// [`run_supervised`]); this simply collects the streamed bytes instead
    /// of hashing them. An unsafe (traversal/absolute) member path or a
    /// member whose declared size exceeds `max_bytes` is refused before the
    /// backend is ever invoked - this is the same [`safe_member_name`] check
    /// `verify_all` applies to every member.
    pub fn read_member(
        &self,
        path: &str,
        max_bytes: u64,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, LhaError> {
        let member = self
            .members
            .iter()
            .find(|member| member.path == path)
            .ok_or_else(|| LhaError::Unsupported {
                detail: format!("no such archive member: {path}"),
            })?;
        if !safe_member_name(&member.path) {
            return Err(LhaError::Unsupported {
                detail: format!("unsafe member path: {}", member.path),
            });
        }
        if let Some(reason) = member.kind.refusal_reason() {
            return Err(LhaError::Unsupported {
                detail: format!("{reason}: {}", member.path),
            });
        }
        if member.logical_size > max_bytes {
            return Err(LhaError::RefusedLimits {
                reason: "member size",
            });
        }
        read_member_bytes(
            &self.executable,
            self.process_limits,
            &self.file,
            &member.path,
            MemberCheck::of(member),
            self.timeout,
            cancel,
        )
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
) -> Result<Vec<ListedMember>, LhaError> {
    let text = run_listing(executable, limits, file.as_raw_fd(), true, timeout)?;
    parse_listing(&text)
}

fn parse_listing(text: &str) -> Result<Vec<ListedMember>, LhaError> {
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
            let is_folder = match required(&properties, "Folder")? {
                "-" => false,
                "+" => true,
                other => {
                    return Err(LhaError::Listing {
                        detail: format!("unrecognised Folder value {other:?}"),
                    });
                }
            };
            let crc = properties
                .get("CRC")
                .map(|value| {
                    u16::from_str_radix(value, 16).map_err(|_| LhaError::Listing {
                        detail: format!("CRC is not hexadecimal: {value:?}"),
                    })
                })
                .transpose()?;
            Ok(ListedMember {
                path,
                is_folder,
                logical_size: parse_u64(required(&properties, "Size")?, "Size")?,
                packed_size: parse_u64(required(&properties, "Packed Size")?, "Packed Size")?,
                method: required(&properties, "Method")?.to_string(),
                crc,
            })
        })
        .collect()
}

/// Joins the raw-header entries with 7-Zip's listing, one-to-one by position.
/// Position alone is never trusted: every entry must also agree on size,
/// packed size, method, CRC-16, folder-ness and (where both are provably the
/// same text) name.  Any disagreement is a backend conflict and the whole
/// archive is refused - nothing is "best effort".  A directory entry also
/// refuses the archive, as it always has.
fn correlate(
    raw: Vec<LhaRawEntry>,
    listing: Vec<ListedMember>,
) -> Result<Vec<LhaMember>, LhaError> {
    let conflict = |index: usize, what: &str| LhaError::Unsupported {
        detail: format!("LHA header and 7-Zip listing disagree at member {index}: {what}"),
    };
    if raw.len() != listing.len() {
        return Err(LhaError::Unsupported {
            detail: format!(
                "LHA header scan found {} members but 7-Zip listed {}",
                raw.len(),
                listing.len()
            ),
        });
    }
    let mut members = Vec::with_capacity(raw.len());
    for (index, (entry, listed)) in raw.into_iter().zip(listing).enumerate() {
        let raw_directory = matches!(entry.kind, LhaEntryKind::Directory);
        if entry.original_size != listed.logical_size {
            return Err(conflict(index, "size"));
        }
        if entry.packed_size != listed.packed_size {
            return Err(conflict(index, "packed size"));
        }
        if entry.method_str() != listed.method {
            return Err(conflict(index, "method"));
        }
        // 7-Zip's folder flag is the `-lhd-` method; links share it.
        if (entry.method == *b"-lhd-") != listed.is_folder {
            return Err(conflict(index, "directory flag"));
        }
        if raw_directory {
            return Err(LhaError::Unsupported {
                detail: format!("directory member is refused: {}", listed.path),
            });
        }
        if listed.crc.is_some_and(|crc| crc != entry.crc16) {
            return Err(conflict(index, "CRC"));
        }
        let mut kind = entry.kind.clone();
        // Names are compared as UTF-8 only when the raw bytes are exactly
        // valid UTF-8; an unprovable correspondence is never regular.
        match entry.name_utf8() {
            Some(name) if name == listed.path => {}
            Some(name) if name.is_ascii() && listed.path.is_ascii() => {
                return Err(conflict(index, "name"));
            }
            _ if kind.is_regular() => {
                kind = LhaEntryKind::Unknown("member name not provably identical");
            }
            _ => {}
        }
        members.push(LhaMember {
            path: listed.path,
            logical_size: listed.logical_size,
            packed_size: listed.packed_size,
            method: listed.method,
            kind,
            crc16: entry.crc16,
        });
    }
    Ok(members)
}

fn check_header_crc(computed: Crc16, expected: u16) -> Result<(), LhaError> {
    let actual = computed.finish();
    if actual != expected {
        return Err(LhaError::Corrupt {
            detail: format!(
                "streamed bytes have CRC-16 {actual:04x}, header declares {expected:04x}"
            ),
        });
    }
    Ok(())
}

fn hash_member(
    executable: &Path,
    limits: ProcessLimits,
    file: &File,
    path: &str,
    expected: MemberCheck,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<ArchiveMemberHashes, LhaError> {
    let MemberCheck {
        declared_size,
        crc16: expected_crc16,
    } = expected;
    let fd = file.as_raw_fd();
    let mut command = Command::new(executable);
    command.args(extract_args(fd, path));
    let mut hasher = StreamingHasher::new();
    let mut header_crc = Crc16::default();
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
            header_crc.update(chunk);
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
    check_header_crc(header_crc, expected_crc16)?;
    Ok(hasher.finish())
}

/// Bounded stdout-streaming extraction of one member's exact bytes into
/// memory, mirroring [`hash_member`] above but collecting a buffer instead
/// of a running hash. `declared_size` is trusted only as an upper bound
/// enforced twice: once as the supervisor's own `max_stdout` ceiling, and
/// again per-chunk against the running `received` total, exactly as
/// `hash_member` does - a member cannot make this read more than its own
/// declared size, whatever the backend actually emits.
fn read_member_bytes(
    executable: &Path,
    limits: ProcessLimits,
    file: &File,
    path: &str,
    expected: MemberCheck,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, LhaError> {
    let MemberCheck {
        declared_size,
        crc16: expected_crc16,
    } = expected;
    let fd = file.as_raw_fd();
    let mut command = Command::new(executable);
    command.args(extract_args(fd, path));
    let mut buffer = Vec::with_capacity(declared_size as usize);
    let mut header_crc = Crc16::default();
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
            buffer.extend_from_slice(chunk);
            header_crc.update(chunk);
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
    check_header_crc(header_crc, expected_crc16)?;
    Ok(buffer)
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
        && !name.contains('\0')
        // Every `/` component must be a plain, non-empty name: no `.`/`..`,
        // no empty segment, no Windows drive prefix (`C:`), and no absolute
        // root.  `Path::components` alone silently drops `.` and empty
        // segments and accepts `C:` on Unix.
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !matches!(part.as_bytes(), [letter, b':', ..] if letter.is_ascii_alphabetic())
        })
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
        for name in [
            "../Game.Slave",
            "/Game.Slave",
            "dir\\Game.Slave",
            "*.Slave",
            "a/./b",
            "a//b",
            "a/",
            "C:/x",
            "z:x",
            "a/C:/x",
            "a/../b",
        ] {
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

    fn parse_members_for_test(text: &str) -> Result<Vec<ListedMember>, LhaError> {
        parse_listing(text)
    }

    // ---- member-type safety: raw header authoritative, 7-Zip supplies bytes ----

    use crate::dat::archive::lha_header::fixtures::{Entry, archive};

    const ROM: &[u8] = b"synthetic rom-like payload \x00\x01\x02\xfe\xff";

    fn provider() -> Option<LhaProvider> {
        LhaProvider::discover(Duration::from_secs(10)).ok()
    }

    fn open_bytes(
        provider: &LhaProvider,
        bytes: &[u8],
    ) -> (tempfile::TempDir, Result<LhaArchiveSource, LhaError>) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.lha");
        std::fs::write(&path, bytes).unwrap();
        let trusted = TrustedRoots::from_paths(std::iter::once(directory.path()));
        let result = provider.open(
            &path,
            &trusted,
            ArchiveLimits::default(),
            Duration::from_secs(20),
        );
        (directory, result)
    }

    fn verify(source: &mut LhaArchiveSource) -> ArchivePassOutcome {
        let cancel = AtomicBool::new(false);
        source.verify_all(&cancel, &mut ArchiveRunBudget::new(1 << 30))
    }

    fn statuses(outcome: &ArchivePassOutcome) -> Vec<&ArchiveMemberStatus> {
        outcome
            .members
            .iter()
            .map(|member| &member.status)
            .collect()
    }

    #[test]
    fn regular_members_verify_at_every_header_level_with_the_real_backend() {
        let Some(provider) = provider() else { return };
        // 7-Zip 23.01 cannot open level 3; that must be a clean refusal.
        for level in 0..=3 {
            for entry in [
                Entry::file("Game.rom", ROM).level(level),
                Entry::unix("Dir/Game.rom", ROM, 0o100644).level(level),
            ] {
                let (_dir, source) = open_bytes(&provider, &archive(&[entry]));
                match source {
                    Ok(mut source) => {
                        let outcome = verify(&mut source);
                        assert_eq!(
                            statuses(&outcome),
                            vec![&ArchiveMemberStatus::HashComplete],
                            "level {level}"
                        );
                    }
                    Err(error) => assert_eq!(level, 3, "unexpected refusal: {error:?}"),
                }
            }
        }
    }

    #[test]
    fn symlink_members_never_reach_checksum_verification_at_any_level() {
        let Some(provider) = provider() else { return };
        for level in 0..=2 {
            let bytes = archive(&[
                Entry::unix("Game.rom|target", b"target", 0o120777).level(level),
                Entry::unix("Real.rom", ROM, 0o100644).level(level),
            ]);
            let (_dir, source) = open_bytes(&provider, &bytes);
            let mut source = source.unwrap();
            let outcome = verify(&mut source);
            assert!(matches!(
                outcome.members[0].status,
                ArchiveMemberStatus::NotVerified {
                    reason: "LHA symbolic-link member"
                }
            ));
            assert!(outcome.members[0].hashes.is_none(), "level {level}");
            // The sibling regular member is unaffected.
            assert!(outcome.members[1].is_hash_complete(), "level {level}");
            // read_member (WHDLoad inspection) refuses the link as well.
            let cancel = AtomicBool::new(false);
            assert!(matches!(
                source.read_member("Game.rom|target", 1024, &cancel),
                Err(LhaError::Unsupported { .. })
            ));
            assert!(source.read_member("Real.rom", 1024, &cancel).is_ok());
        }
    }

    #[test]
    fn special_and_unproven_members_are_refused_not_hashed() {
        let Some(provider) = provider() else { return };
        let mut unix_no_mode = Entry::file("NoMode.rom", ROM);
        unix_no_mode.host = b'U';
        let mut link_separator = Entry::file("a|b.rom", ROM);
        link_separator.host = b'M';
        let bytes = archive(&[
            Entry::unix("Fifo", b"", 0o010644),
            unix_no_mode,
            link_separator,
            Entry::unix("Real.rom", ROM, 0o100644),
        ]);
        let (_dir, source) = open_bytes(&provider, &bytes);
        let outcome = verify(&mut source.unwrap());
        let refused: Vec<_> = outcome
            .members
            .iter()
            .map(|member| member.hashes.is_some())
            .collect();
        assert_eq!(refused, vec![false, false, false, true]);
        assert!(matches!(
            outcome.members[0].status,
            ArchiveMemberStatus::NotVerified {
                reason: "LHA special member"
            }
        ));
        for index in [1, 2] {
            assert!(matches!(
                outcome.members[index].status,
                ArchiveMemberStatus::NotVerified {
                    reason: "LHA member type unproven"
                }
            ));
        }
    }

    #[test]
    fn directory_members_refuse_the_archive_as_before() {
        let Some(provider) = provider() else { return };
        for level in 0..=2 {
            let bytes = archive(&[
                Entry::directory("Game").level(level),
                Entry::file("Game/x", ROM),
            ]);
            let (_dir, source) = open_bytes(&provider, &bytes);
            assert!(
                matches!(&source, Err(LhaError::Unsupported { detail }) if detail.contains("directory")),
                "level {level}: {:?}",
                source.err()
            );
        }
    }

    #[test]
    fn duplicate_traversal_and_absolute_names_stay_refused() {
        let Some(provider) = provider() else { return };
        let (_d, duplicate) = open_bytes(
            &provider,
            &archive(&[
                Entry::file("Game.rom", ROM),
                Entry::file("Game.rom", b"other bytes"),
            ]),
        );
        assert!(matches!(duplicate, Err(LhaError::Unsupported { .. })));
        for name in [
            "../Game.rom",
            "/Game.rom",
            "a/../../Game.rom",
            "C:/Game.rom",
            "a\\..\\Game.rom",
        ] {
            let (_d, source) = open_bytes(&provider, &archive(&[Entry::unix(name, ROM, 0o100644)]));
            // Either the backend/correlation refuses the archive or the
            // member is NotVerified - never hashed, even though it is a
            // regular file by header.
            if let Ok(mut source) = source {
                let outcome = verify(&mut source);
                assert!(outcome.members.iter().all(|m| m.hashes.is_none()), "{name}");
                let cancel = AtomicBool::new(false);
                assert!(source.read_member(name, 1024, &cancel).is_err(), "{name}");
            }
        }
    }

    #[test]
    fn unicode_names_are_correlated_or_refused_never_guessed() {
        let Some(provider) = provider() else { return };
        let bytes = archive(&[Entry::unix("Spiel/Größe-ゲーム.rom", ROM, 0o100644).level(1)]);
        let (_dir, source) = open_bytes(&provider, &bytes);
        if let Ok(mut source) = source {
            let outcome = verify(&mut source);
            // Hashed only if 7-Zip reproduced the exact UTF-8 name; any
            // re-encoding downgrades the member to unproven.
            let name = outcome.members[0].member_name_display.clone();
            if name == "Spiel/Größe-ゲーム.rom" {
                assert!(outcome.members[0].is_hash_complete());
            } else {
                assert!(outcome.members[0].hashes.is_none());
            }
        }
    }

    #[test]
    fn truncated_and_malformed_archives_are_refused_at_open() {
        let Some(provider) = provider() else { return };
        let good = archive(&[Entry::unix("Game.rom", ROM, 0o100644).level(1)]);
        for cut in [10, 25, good.len() - 3] {
            let (_dir, source) = open_bytes(&provider, &good[..cut]);
            assert!(source.is_err(), "cut {cut}");
        }
        let mut bad_chain = good.clone();
        // Level-1 base header is 27 + name bytes; its last two bytes are the
        // first extended-header size. The base checksum does not cover them
        // at a different place, so recompute it.
        let size_at = 27 + "Game.rom".len() - 2;
        bad_chain[size_at..size_at + 2].copy_from_slice(&0xffff_u16.to_le_bytes());
        bad_chain[1] = bad_chain[2..2 + bad_chain[0] as usize]
            .iter()
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        let (_dir, source) = open_bytes(&provider, &bad_chain);
        assert!(source.is_err());
    }

    #[test]
    fn crc_failures_are_corrupt_and_never_hashed() {
        let Some(provider) = provider() else { return };
        // Payload altered after the header CRC was computed.
        let mut tampered = archive(&[Entry::file("Game.rom", ROM)]);
        let last = tampered.len() - 2;
        tampered[last] ^= 0x55;
        let (_dir, source) = open_bytes(&provider, &tampered);
        if let Ok(mut source) = source {
            let outcome = verify(&mut source);
            assert!(matches!(
                outcome.members[0].status,
                ArchiveMemberStatus::Corrupt { .. }
            ));
            assert!(outcome.members[0].hashes.is_none());
        }
        // A header declaring the wrong CRC for intact bytes.
        let mut wrong = Entry::file("Game.rom", ROM);
        wrong.declared_crc = Some(0x1234);
        let (_dir, source) = open_bytes(&provider, &archive(&[wrong]));
        if let Ok(mut source) = source {
            assert!(verify(&mut source).members[0].hashes.is_none());
        }
    }

    #[test]
    fn unsupported_compression_methods_are_not_hashed() {
        let Some(provider) = provider() else { return };
        let mut entry = Entry::file("Game.rom", ROM);
        entry.method = *b"-lz9-";
        let (_dir, source) = open_bytes(&provider, &archive(&[entry]));
        if let Ok(mut source) = source {
            let outcome = verify(&mut source);
            assert!(outcome.members[0].hashes.is_none());
            assert!(!outcome.members[0].is_hash_complete());
        }
    }

    #[test]
    fn a_large_regular_member_streams_with_exact_hashes() {
        let Some(provider) = provider() else { return };
        let payload: Vec<u8> = (0..16 * 1024 * 1024_u32)
            .map(|i| (i.wrapping_mul(31) >> 3) as u8)
            .collect();
        let (_dir, source) = open_bytes(
            &provider,
            &archive(&[Entry::unix("Big.rom", &payload, 0o100644).level(2)]),
        );
        let outcome = verify(&mut source.unwrap());
        assert!(outcome.members[0].is_hash_complete());
        let mut expected = StreamingHasher::new();
        expected.update(&payload);
        assert_eq!(outcome.members[0].hashes.as_ref(), Some(&expected.finish()));
    }

    // ---- backend disagreement: any conflict refuses, never "best effort" ----

    fn raw_regular(name: &str) -> LhaRawEntry {
        use crate::dat::archive::lha_header::{fixtures::Entry, scan_headers};
        let bytes = archive(&[Entry::unix(name, ROM, 0o100644)]);
        scan_headers(bytes.as_slice(), bytes.len() as u64, 8)
            .unwrap()
            .remove(0)
    }

    fn listed_like(entry: &LhaRawEntry) -> ListedMember {
        ListedMember {
            path: String::from_utf8(entry.name.clone()).unwrap(),
            is_folder: false,
            logical_size: entry.original_size,
            packed_size: entry.packed_size,
            method: entry.method_str(),
            crc: Some(entry.crc16),
        }
    }

    #[test]
    fn a_consistent_listing_correlates_and_keeps_the_header_type() {
        let raw = raw_regular("Game.rom");
        let listed = listed_like(&raw);
        let members = correlate(vec![raw], vec![listed]).unwrap();
        assert_eq!(members[0].kind, LhaEntryKind::Regular);
    }

    #[test]
    fn header_symlink_with_an_ordinary_looking_listing_stays_a_symlink() {
        let mut raw = raw_regular("Game.rom|target");
        raw.kind = LhaEntryKind::Symlink;
        let listed = listed_like(&raw); // 7-Zip: Folder = -, plain file
        let members = correlate(vec![raw], vec![listed]).unwrap();
        assert_eq!(members[0].kind, LhaEntryKind::Symlink);
        assert!(members[0].kind.refusal_reason().is_some());
    }

    #[test]
    fn every_header_listing_disagreement_refuses_the_archive() {
        let raw = raw_regular("Game.rom");
        let base = listed_like(&raw);
        #[allow(clippy::type_complexity)]
        let mutations: Vec<(&str, Box<dyn Fn(&mut ListedMember)>)> = vec![
            ("size", Box::new(|l| l.logical_size += 1)),
            ("packed", Box::new(|l| l.packed_size += 1)),
            ("method", Box::new(|l| l.method = "-lh5-".to_string())),
            ("crc", Box::new(|l| l.crc = Some(l.crc.unwrap() ^ 1))),
            ("folder", Box::new(|l| l.is_folder = true)),
            ("name", Box::new(|l| l.path = "Other.rom".to_string())),
        ];
        for (what, mutate) in mutations {
            let mut listed = base.clone();
            mutate(&mut listed);
            assert!(
                correlate(vec![raw.clone()], vec![listed]).is_err(),
                "{what} disagreement must refuse"
            );
        }
        // Member-count disagreement, and reordering of distinct members.
        assert!(correlate(vec![raw.clone()], vec![]).is_err());
        assert!(correlate(vec![], vec![base.clone()]).is_err());
        let other = raw_regular("Other.rom");
        assert!(
            correlate(
                vec![raw.clone(), other.clone()],
                vec![listed_like(&other), listed_like(&raw)]
            )
            .is_err()
        );
    }

    #[test]
    fn non_ascii_name_disagreement_downgrades_the_member_instead_of_trusting_it() {
        let raw = raw_regular("Größe.rom");
        let mut listed = listed_like(&raw);
        listed.path = "GrÃ¶Ãe.rom".to_string(); // 7-Zip re-encoded the name
        let members = correlate(vec![raw], vec![listed]).unwrap();
        assert!(matches!(members[0].kind, LhaEntryKind::Unknown(_)));
    }

    #[test]
    fn listing_without_a_folder_flag_or_with_a_bad_crc_is_refused() {
        assert!(parse_listing("Path = a\nSize = 1\nPacked Size = 1\nMethod = -lh0-\n").is_err());
        assert!(
            parse_listing("Path = a\nFolder = ?\nSize = 1\nPacked Size = 1\nMethod = -lh0-\n")
                .is_err()
        );
        assert!(
            parse_listing(
                "Path = a\nFolder = -\nSize = 1\nPacked Size = 1\nCRC = zz\nMethod = -lh0-\n"
            )
            .is_err()
        );
    }

    // ---- Amiga host ('A'): links are -lhd- + extended header 0x60/0x61 ----

    #[test]
    fn amiga_links_are_non_regular_per_member_and_never_streamed() {
        let Some(provider) = provider() else { return };
        for level in 1..=2 {
            let bytes = archive(&[
                Entry::amiga("Real.rom", ROM).level(level),
                Entry::amiga_link("Soft.rom", 0x61, "/Real.rom").level(level),
                Entry::amiga_link("Hard.rom", 0x60, "Real.rom").level(level),
            ]);
            let (_dir, source) = open_bytes(&provider, &bytes);
            let mut source = source.unwrap_or_else(|error| panic!("level {level}: {error:?}"));
            let outcome = verify(&mut source);
            assert!(outcome.members[0].is_hash_complete(), "level {level}");
            for (index, reason) in [(1, "LHA symbolic-link member"), (2, "LHA hard-link member")] {
                assert_eq!(
                    outcome.members[index].status,
                    ArchiveMemberStatus::NotVerified { reason },
                    "level {level}"
                );
                assert!(outcome.members[index].hashes.is_none());
            }
            let cancel = AtomicBool::new(false);
            for link in ["Soft.rom", "Hard.rom"] {
                assert!(source.read_member(link, 1024, &cancel).is_err());
            }
        }
    }

    #[test]
    fn amiga_link_disguised_with_an_ordinary_method_is_still_refused() {
        let Some(provider) = provider() else { return };
        // A link marker on a stored (-lh0-) entry whose bytes are the target
        // path: 7-Zip lists and would stream it as a file.
        let mut disguised = Entry::amiga_link("Soft.rom", 0x61, "/Real.rom").level(1);
        disguised.method = *b"-lh0-";
        disguised.payload = b"/Real.rom".to_vec();
        let (_dir, source) = open_bytes(&provider, &archive(&[disguised]));
        let mut source = source.unwrap();
        let outcome = verify(&mut source);
        assert!(outcome.members[0].hashes.is_none());
        assert!(matches!(
            outcome.members[0].status,
            ArchiveMemberStatus::NotVerified { .. }
        ));
    }

    #[test]
    fn amiga_entry_with_an_unrecognised_extended_header_is_unproven() {
        let Some(provider) = provider() else { return };
        let mut entry = Entry::amiga("Real.rom", ROM).level(1);
        entry
            .extra_extended
            .push((0x62, b"future link kind".to_vec()));
        let (_dir, source) = open_bytes(&provider, &archive(&[entry]));
        let outcome = verify(&mut source.unwrap());
        assert!(outcome.members[0].hashes.is_none());
        assert!(matches!(
            outcome.members[0].status,
            ArchiveMemberStatus::NotVerified {
                reason: "LHA member type unproven"
            }
        ));
    }

    #[test]
    fn raw_link_versus_a_plain_file_listing_refuses_the_archive() {
        // Header says -lhd- (link/directory), 7-Zip lists an ordinary file.
        let raw = {
            use crate::dat::archive::lha_header::scan_headers;
            let bytes = archive(&[Entry::amiga_link("Soft.rom", 0x61, "/Real.rom").level(1)]);
            scan_headers(bytes.as_slice(), bytes.len() as u64, 8)
                .unwrap()
                .remove(0)
        };
        let mut listed = listed_like(&raw);
        listed.is_folder = false;
        assert!(correlate(vec![raw.clone()], vec![listed]).is_err());
        // Consistent listing: kept, per-member non-regular.
        let mut listed = listed_like(&raw);
        listed.is_folder = true;
        let members = correlate(vec![raw], vec![listed]).unwrap();
        assert_eq!(members[0].kind, LhaEntryKind::Symlink);
    }
}
