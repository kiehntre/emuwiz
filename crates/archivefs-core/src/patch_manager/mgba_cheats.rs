//! Native mGBA `.cheats` inspection and local merge/apply support.
//!
//! mGBA's native file is a small line-oriented collection of named sets.  The
//! emulator itself remains the authority for interpreting GameShark, Pro
//! Action Replay, CodeBreaker, VBA, and future directive/code variants.  This
//! adapter therefore normalizes only the unambiguous VBA-style `address:value`
//! form and retains every other code line verbatim.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use super::CheatOperation;

pub const MGBA_CHEAT_MAX_BYTES: usize = 1024 * 1024;
pub const MGBA_CHEAT_MAX_ENTRIES: usize = 1000;
pub const MGBA_CHEAT_MAX_LINES_PER_ENTRY: usize = 128;
const MGBA_CHEAT_MAX_LINE_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MgbaCheatFormat {
    GameShark,
    ProActionReplay,
    CodeBreaker,
    Vba,
    NativeOrUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MgbaCheatState {
    Enabled,
    Disabled,
    RuntimeOnly,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MgbaCheatCode {
    pub raw: String,
    pub normalized: Option<CheatOperation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MgbaCheatEntry {
    pub name: String,
    pub format: MgbaCheatFormat,
    pub state: MgbaCheatState,
    pub directives: Vec<String>,
    pub codes: Vec<MgbaCheatCode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MgbaCheatFile {
    pub entries: Vec<MgbaCheatEntry>,
    pub issues: Vec<MgbaCheatIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MgbaCheatIssue {
    MissingName,
    EmptyCode,
    UnsupportedCode(String),
    MalformedCode(String),
    TooManyLines,
    UnknownDirective(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MgbaCheatParseError {
    Empty,
    TooLarge {
        bytes: usize,
        limit: usize,
    },
    LineTooLong {
        line: usize,
        bytes: usize,
        limit: usize,
    },
    TooManyEntries {
        limit: usize,
    },
    InvalidUtf8,
    Io(String),
}

impl std::fmt::Display for MgbaCheatParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("mGBA cheat file is empty"),
            Self::TooLarge { bytes, limit } => {
                write!(f, "mGBA cheat file is {bytes} bytes; limit is {limit}")
            }
            Self::LineTooLong { line, bytes, limit } => write!(
                f,
                "mGBA cheat line {line} is {bytes} bytes; limit is {limit}"
            ),
            Self::TooManyEntries { limit } => {
                write!(f, "mGBA cheat file exceeds the {limit}-entry limit")
            }
            Self::InvalidUtf8 => f.write_str("mGBA cheat file is not UTF-8"),
            Self::Io(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for MgbaCheatParseError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MgbaCheatReadiness {
    Ready,
    ReadyWithOpaqueCodes,
    IdentityUnverified,
    DestinationUnsafe,
    Malformed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MgbaCheatIdentityEvidence {
    ExactRomSha256 { expected: String, actual: String },
    VerifiedGameIdentity { value: String },
    ProviderDeclared { value: String },
    TitleOnly { title: String },
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MgbaCheatIdentity {
    pub game_title: String,
    pub rom_path: PathBuf,
    pub evidence: MgbaCheatIdentityEvidence,
}

impl MgbaCheatIdentity {
    pub fn apply_readiness(&self) -> MgbaCheatReadiness {
        match self.evidence {
            MgbaCheatIdentityEvidence::ExactRomSha256 { .. }
            | MgbaCheatIdentityEvidence::VerifiedGameIdentity { .. } => MgbaCheatReadiness::Ready,
            _ => MgbaCheatReadiness::IdentityUnverified,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MgbaCheatApplyPlan {
    pub destination: PathBuf,
    pub expected_destination_sha256: Option<String>,
    pub output: Vec<u8>,
    pub output_sha256: String,
    pub identity: MgbaCheatIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MgbaCheatApplyReceipt {
    pub destination: PathBuf,
    pub output_sha256: String,
    pub previous_bytes: Option<Vec<u8>>,
    pub previous_sha256: Option<String>,
    pub created_file: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MgbaCheatWriteError {
    IdentityUnverified,
    DestinationMissingParent,
    DestinationSymlink,
    DestinationChanged,
    OutputMalformed,
    Io(String),
    RollbackExternallyModified,
}

pub type MgbaCheatWriteResult<T> = Result<T, MgbaCheatWriteError>;

pub fn parse_mgba_cheat_file(bytes: &[u8]) -> Result<MgbaCheatFile, MgbaCheatParseError> {
    if bytes.is_empty() {
        return Err(MgbaCheatParseError::Empty);
    }
    if bytes.len() > MGBA_CHEAT_MAX_BYTES {
        return Err(MgbaCheatParseError::TooLarge {
            bytes: bytes.len(),
            limit: MGBA_CHEAT_MAX_BYTES,
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| MgbaCheatParseError::InvalidUtf8)?;
    let mut entries = Vec::new();
    let mut pending_directives = Vec::new();
    let mut next_disabled = false;
    let mut current: Option<MgbaCheatEntry> = None;
    let mut issues = Vec::new();
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_no = line_index + 1;
        if raw_line.len() > MGBA_CHEAT_MAX_LINE_BYTES {
            return Err(MgbaCheatParseError::LineTooLong {
                line: line_no,
                bytes: raw_line.len(),
                limit: MGBA_CHEAT_MAX_LINE_BYTES,
            });
        }
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(directive) = line.strip_prefix('!') {
            if directive.eq_ignore_ascii_case("disabled") {
                next_disabled = true;
            } else if directive.eq_ignore_ascii_case("reset") {
                pending_directives.clear();
            } else {
                if !matches!(
                    directive.to_ascii_lowercase().as_str(),
                    "gsav1" | "arv1" | "arv2" | "arv3" | "cb" | "vba"
                ) {
                    issues.push(MgbaCheatIssue::UnknownDirective(directive.to_string()));
                }
                pending_directives.push(directive.to_string());
            }
            continue;
        }
        if let Some(name) = line.strip_prefix('#') {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            if entries.len() >= MGBA_CHEAT_MAX_ENTRIES {
                return Err(MgbaCheatParseError::TooManyEntries {
                    limit: MGBA_CHEAT_MAX_ENTRIES,
                });
            }
            let name = name.trim().to_string();
            if name.is_empty() {
                issues.push(MgbaCheatIssue::MissingName);
            }
            current = Some(MgbaCheatEntry {
                name,
                format: format_from_directives(&pending_directives),
                state: if next_disabled {
                    MgbaCheatState::Disabled
                } else {
                    MgbaCheatState::Enabled
                },
                directives: std::mem::take(&mut pending_directives),
                codes: Vec::new(),
            });
            next_disabled = false;
            continue;
        }
        let entry = current.get_or_insert_with(|| MgbaCheatEntry {
            name: String::new(),
            format: format_from_directives(&pending_directives),
            state: if next_disabled {
                MgbaCheatState::Disabled
            } else {
                MgbaCheatState::Enabled
            },
            directives: std::mem::take(&mut pending_directives),
            codes: Vec::new(),
        });
        if entry.codes.len() >= MGBA_CHEAT_MAX_LINES_PER_ENTRY {
            issues.push(MgbaCheatIssue::TooManyLines);
            continue;
        }
        let normalized = parse_direct_write(line)
            .map(|(address, value)| CheatOperation::Write8 { address, value });
        if normalized.is_none() {
            issues.push(if line.contains(':') {
                MgbaCheatIssue::MalformedCode(line.to_string())
            } else {
                MgbaCheatIssue::UnsupportedCode(line.to_string())
            });
        }
        entry.codes.push(MgbaCheatCode {
            raw: line.to_string(),
            normalized,
        });
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(MgbaCheatParseError::Empty);
    }
    for entry in &entries {
        if entry.codes.is_empty() {
            issues.push(MgbaCheatIssue::EmptyCode);
        }
    }
    Ok(MgbaCheatFile { entries, issues })
}

pub fn render_mgba_cheat_file(file: &MgbaCheatFile) -> Vec<u8> {
    let mut out = String::new();
    for entry in &file.entries {
        if entry.state == MgbaCheatState::Disabled {
            out.push_str("!disabled\n");
        }
        for directive in &entry.directives {
            out.push('!');
            out.push_str(directive);
            out.push('\n');
        }
        out.push_str("# ");
        out.push_str(&entry.name);
        out.push('\n');
        for code in &entry.codes {
            out.push_str(&code.raw);
            out.push('\n');
        }
    }
    out.into_bytes()
}

pub fn merge_mgba_cheat_entry(file: &mut MgbaCheatFile, entry: MgbaCheatEntry) -> bool {
    let key = entry_key(&entry);
    if file
        .entries
        .iter()
        .any(|existing| entry_key(existing) == key)
    {
        return false;
    }
    file.entries.push(entry);
    true
}

pub fn set_mgba_cheat_state(file: &mut MgbaCheatFile, index: usize, state: MgbaCheatState) -> bool {
    let Some(entry) = file.entries.get_mut(index) else {
        return false;
    };
    entry.state = state;
    true
}

pub fn remove_mgba_cheat_entry(file: &mut MgbaCheatFile, index: usize) -> Option<MgbaCheatEntry> {
    (index < file.entries.len()).then(|| file.entries.remove(index))
}

pub fn build_mgba_cheat_apply_plan(
    destination: &Path,
    existing: Option<&[u8]>,
    file: &MgbaCheatFile,
    identity: MgbaCheatIdentity,
) -> MgbaCheatWriteResult<MgbaCheatApplyPlan> {
    if identity.apply_readiness() != MgbaCheatReadiness::Ready {
        return Err(MgbaCheatWriteError::IdentityUnverified);
    }
    ensure_destination_safe(destination)?;
    let output = render_mgba_cheat_file(file);
    let reparsed =
        parse_mgba_cheat_file(&output).map_err(|_| MgbaCheatWriteError::OutputMalformed)?;
    if reparsed != *file {
        return Err(MgbaCheatWriteError::OutputMalformed);
    }
    let expected_destination_sha256 = existing.map(sha256_hex);
    let output_sha256 = sha256_hex(&output);
    Ok(MgbaCheatApplyPlan {
        destination: destination.to_path_buf(),
        expected_destination_sha256,
        output,
        output_sha256,
        identity,
    })
}

pub fn apply_mgba_cheat_plan(
    plan: &MgbaCheatApplyPlan,
) -> MgbaCheatWriteResult<MgbaCheatApplyReceipt> {
    if plan.identity.apply_readiness() != MgbaCheatReadiness::Ready {
        return Err(MgbaCheatWriteError::IdentityUnverified);
    }
    let parent = plan
        .destination
        .parent()
        .ok_or(MgbaCheatWriteError::DestinationMissingParent)?;
    fs::create_dir_all(parent).map_err(io_error)?;
    ensure_destination_safe(&plan.destination)?;
    let previous_bytes = fs::read(&plan.destination).ok();
    let previous_sha256 = previous_bytes.as_deref().map(sha256_hex);
    if previous_sha256 != plan.expected_destination_sha256 {
        return Err(MgbaCheatWriteError::DestinationChanged);
    }
    let temp = parent.join(format!(
        ".{}.emuwiz-{}.tmp",
        plan.destination.file_name().unwrap().to_string_lossy(),
        unique_suffix()
    ));
    let mut handle = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(io_error)?;
    if let Err(error) = handle
        .write_all(&plan.output)
        .and_then(|_| handle.sync_all())
    {
        let _ = fs::remove_file(&temp);
        return Err(io_error(error));
    }
    drop(handle);
    if let Err(error) = fs::rename(&temp, &plan.destination) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(error));
    }
    Ok(MgbaCheatApplyReceipt {
        destination: plan.destination.clone(),
        output_sha256: plan.output_sha256.clone(),
        previous_bytes,
        previous_sha256,
        created_file: plan.expected_destination_sha256.is_none(),
    })
}

impl MgbaCheatApplyReceipt {
    pub fn rollback(&self) -> MgbaCheatWriteResult<()> {
        let current = fs::read(&self.destination).map_err(io_error)?;
        if sha256_hex(&current) != self.output_sha256 {
            return Err(MgbaCheatWriteError::RollbackExternallyModified);
        }
        match &self.previous_bytes {
            Some(bytes) => {
                let temp = self.destination.with_extension("emuwiz-rollback.tmp");
                fs::write(&temp, bytes).map_err(io_error)?;
                fs::rename(&temp, &self.destination).map_err(io_error)?;
            }
            None => fs::remove_file(&self.destination).map_err(io_error)?,
        }
        Ok(())
    }
}

fn format_from_directives(directives: &[String]) -> MgbaCheatFormat {
    directives
        .iter()
        .rev()
        .find_map(|directive| match directive.to_ascii_lowercase().as_str() {
            "gsav1" => Some(MgbaCheatFormat::GameShark),
            "arv1" | "arv2" | "arv3" => Some(MgbaCheatFormat::ProActionReplay),
            "cb" => Some(MgbaCheatFormat::CodeBreaker),
            "vba" => Some(MgbaCheatFormat::Vba),
            _ => None,
        })
        .unwrap_or(MgbaCheatFormat::NativeOrUnknown)
}

fn parse_direct_write(line: &str) -> Option<(u64, u8)> {
    let (address, value) = line.split_once(':')?;
    if address.len() != 8
        || value.len() != 2
        || !address.chars().all(|c| c.is_ascii_hexdigit())
        || !value.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    Some((
        u64::from_str_radix(address, 16).ok()?,
        u8::from_str_radix(value, 16).ok()?,
    ))
}

fn entry_key(entry: &MgbaCheatEntry) -> String {
    format!(
        "{:?}|{}",
        entry.format as u8,
        entry
            .codes
            .iter()
            .map(|code| code.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn io_error(error: io::Error) -> MgbaCheatWriteError {
    MgbaCheatWriteError::Io(error.to_string())
}

fn ensure_destination_safe(destination: &Path) -> MgbaCheatWriteResult<()> {
    if !destination.is_absolute()
        || destination.as_os_str().is_empty()
        || destination.file_name().is_none()
        || destination
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(MgbaCheatWriteError::DestinationMissingParent);
    }
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(MgbaCheatWriteError::DestinationSymlink);
        }
    }
    let mut ancestor = destination.parent();
    while let Some(path) = ancestor {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() {
                return Err(MgbaCheatWriteError::DestinationSymlink);
            }
        }
        ancestor = path.parent();
    }
    Ok(())
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(path: &Path) -> MgbaCheatIdentity {
        MgbaCheatIdentity {
            game_title: "Test GBA".into(),
            rom_path: path.into(),
            evidence: MgbaCheatIdentityEvidence::VerifiedGameIdentity {
                value: "gba:test".into(),
            },
        }
    }

    #[test]
    fn parses_native_sets_and_preserves_opaque_lines() {
        let file = parse_mgba_cheat_file(
            b"!GSAv1\n# Infinite lives\n600DC550 00002001\n!disabled\n# Raw\n02000000:7F\n",
        )
        .unwrap();
        assert_eq!(file.entries.len(), 2);
        assert_eq!(file.entries[0].state, MgbaCheatState::Enabled);
        assert_eq!(file.entries[1].state, MgbaCheatState::Disabled);
        assert!(file.entries[0].codes[0].normalized.is_none());
        assert!(matches!(
            file.entries[1].codes[0].normalized,
            Some(CheatOperation::Write8 { .. })
        ));
    }

    #[test]
    fn malformed_and_bounds_are_reported_without_discarding_codes() {
        let file = parse_mgba_cheat_file(b"# A\nnot-a-code\n# B\n02000000:FF\n").unwrap();
        assert!(
            file.issues
                .iter()
                .any(|issue| matches!(issue, MgbaCheatIssue::UnsupportedCode(_)))
        );
        assert_eq!(file.entries[0].codes[0].raw, "not-a-code");
        assert!(parse_mgba_cheat_file(&vec![b'x'; MGBA_CHEAT_MAX_BYTES + 1]).is_err());
    }

    #[test]
    fn merge_is_deterministic_and_avoids_duplicates() {
        let mut file = parse_mgba_cheat_file(b"# A\n02000000:FF\n").unwrap();
        let entry = file.entries[0].clone();
        assert!(!merge_mgba_cheat_entry(&mut file, entry.clone()));
        assert!(!merge_mgba_cheat_entry(
            &mut file,
            MgbaCheatEntry {
                name: "B".into(),
                ..entry
            }
        ));
        let template = file.entries[0].clone();
        assert!(merge_mgba_cheat_entry(
            &mut file,
            MgbaCheatEntry {
                name: "B".into(),
                codes: vec![MgbaCheatCode {
                    raw: "02000001:01".into(),
                    normalized: Some(CheatOperation::Write8 {
                        address: 0x02000001,
                        value: 1,
                    }),
                }],
                ..template
            }
        ));
        assert_eq!(
            render_mgba_cheat_file(&file),
            b"# A\n02000000:FF\n# B\n02000001:01\n"
        );
    }

    #[test]
    fn per_entry_state_and_removal_are_local_and_deterministic() {
        let mut file = parse_mgba_cheat_file(b"# A\n02000000:FF\n# B\n02000001:01\n").unwrap();
        assert!(set_mgba_cheat_state(&mut file, 0, MgbaCheatState::Disabled));
        assert_eq!(file.entries[0].state, MgbaCheatState::Disabled);
        assert!(set_mgba_cheat_state(&mut file, 0, MgbaCheatState::Enabled));
        assert_eq!(remove_mgba_cheat_entry(&mut file, 1).unwrap().name, "B");
        assert!(!set_mgba_cheat_state(
            &mut file,
            9,
            MgbaCheatState::Disabled
        ));
    }

    #[test]
    fn apply_and_rollback_are_atomic_and_refuse_external_changes() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("game.cheats");
        let file = parse_mgba_cheat_file(b"# A\n02000000:FF\n").unwrap();
        let plan = build_mgba_cheat_apply_plan(
            &destination,
            None,
            &file,
            identity(&directory.path().join("game.gba")),
        )
        .unwrap();
        let receipt = apply_mgba_cheat_plan(&plan).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), plan.output);
        fs::write(&destination, b"external").unwrap();
        assert_eq!(
            receipt.rollback(),
            Err(MgbaCheatWriteError::RollbackExternallyModified)
        );
    }

    #[test]
    fn title_only_identity_cannot_apply() {
        let directory = tempfile::tempdir().unwrap();
        let file = parse_mgba_cheat_file(b"# A\n02000000:FF\n").unwrap();
        let mut target = identity(&directory.path().join("game.gba"));
        target.evidence = MgbaCheatIdentityEvidence::TitleOnly {
            title: "Test GBA".into(),
        };
        assert_eq!(
            build_mgba_cheat_apply_plan(&directory.path().join("game.cheats"), None, &file, target),
            Err(MgbaCheatWriteError::IdentityUnverified)
        );
    }
}
