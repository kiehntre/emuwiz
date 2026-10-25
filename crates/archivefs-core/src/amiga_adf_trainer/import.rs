//! Bounded import of trainer declarations.
//!
//! The file is size-capped, parsed once, and then each entry is converted on its
//! own: a malformed entry becomes a diagnostic and never affects its neighbours.
//! Every number goes through checked parsing and per-width range checks; every
//! string is length- and control-character-checked. Nothing is executed and
//! nothing in an entry can name a command.

use std::fs;
use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::model::*;
use crate::media_set::IdentityKey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    Io(String),
    NotARegularFile,
    TooLarge {
        bytes: usize,
        max: usize,
    },
    /// The file as a whole is not a trainer declaration file.
    Malformed(String),
    UnsupportedSchema(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) => write!(f, "could not read the trainer file: {m}"),
            Self::NotARegularFile => f.write_str("not a regular file"),
            Self::TooLarge { bytes, max } => {
                write!(f, "trainer file is {bytes} bytes; limit is {max}")
            }
            Self::Malformed(m) => write!(f, "not a trainer declaration file: {m}"),
            Self::UnsupportedSchema(s) => write!(f, "unsupported trainer schema {s:?}"),
        }
    }
}

impl std::error::Error for ImportError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportDiagnostic {
    /// Position of the entry in the source file.
    pub index: usize,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmigaTrainerImport {
    pub trainers: Vec<AmigaTrainer>,
    pub diagnostics: Vec<ImportDiagnostic>,
    /// Entries rejected, including any beyond the diagnostics bound.
    pub rejected: usize,
    /// Entries beyond [`MAX_TRAINERS_PER_FILE`], counted and not read.
    pub dropped_over_limit: usize,
    pub source_name: String,
    /// SHA-256 of the complete source file, for provenance.
    pub source_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    schema: String,
    entries: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NumText {
    Text(String),
    Number(u64),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    platform: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    mechanism: String,
    target: RawTarget,
    #[serde(default)]
    writes: Vec<RawWrite>,
    source: RawSource,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTarget {
    media: Vec<RawMedia>,
    scope: String,
    #[serde(default)]
    disk: Option<NumText>,
    #[serde(default)]
    release: Option<RawRelease>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    revision: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMedia {
    sha256: String,
    #[serde(default)]
    disk: Option<NumText>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRelease {
    namespace: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWrite {
    width: String,
    address: NumText,
    value: NumText,
    #[serde(default)]
    original: Option<NumText>,
    #[serde(default)]
    timing: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    name: String,
    #[serde(default)]
    reference: Option<String>,
}

fn text(field: &str, value: &str, max: usize, required: bool) -> Result<String, String> {
    let value = value.trim();
    if required && value.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if value.len() > max {
        return Err(format!("{field} is {} bytes; limit is {max}", value.len()));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field} contains a control character"));
    }
    Ok(value.to_string())
}

fn optional_text(
    field: &str,
    value: Option<&String>,
    max: usize,
) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(v) => {
            let cleaned = text(field, v, max, false)?;
            Ok((!cleaned.is_empty()).then_some(cleaned))
        }
    }
}

/// Checked numeric parse: `0x` hex digits or plain decimal digits, nothing else
/// (no sign, no separators, no whitespace inside), never wider than a `u64`.
fn number(field: &str, value: &NumText) -> Result<u64, String> {
    match value {
        NumText::Number(n) => Ok(*n),
        NumText::Text(raw) => {
            let raw = raw.trim();
            if raw.is_empty() || raw.len() > MAX_NUMBER_TEXT_BYTES {
                return Err(format!("{field} is empty or too long"));
            }
            let (digits, radix) = match raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
                Some(hex) => (hex, 16),
                None => (raw, 10),
            };
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return Err(format!(
                    "{field} {raw:?} is not a plain decimal or 0x-hex number"
                ));
            }
            u64::from_str_radix(digits, radix)
                .map_err(|_| format!("{field} {raw:?} is out of range"))
        }
    }
}

fn disk_number(field: &str, value: &NumText) -> Result<u16, String> {
    let n = number(field, value)?;
    u16::try_from(n)
        .ok()
        .filter(|n| (1..=255).contains(n))
        .ok_or_else(|| format!("{field} must be between 1 and 255"))
}

fn sha256(field: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(format!("{field} must be exactly 64 hex characters"))
    }
}

fn write(raw: &RawWrite) -> Result<AmigaMemoryWrite, String> {
    let width = match raw.width.trim().to_ascii_lowercase().as_str() {
        "byte" => AmigaMemoryWidth::Byte,
        "word" => AmigaMemoryWidth::Word,
        "long" => AmigaMemoryWidth::Long,
        other => return Err(format!("unknown width {other:?}")),
    };
    let address = u32::try_from(number("address", &raw.address)?)
        .map_err(|_| "address does not fit in 32 bits".to_string())?;
    if address % width.bytes() != 0 {
        return Err(format!(
            "address {address:#x} is not aligned for a {width:?} write"
        ));
    }
    // The whole write must stay inside the 32-bit address space.
    address
        .checked_add(width.bytes() - 1)
        .ok_or_else(|| "write runs past the end of the address space".to_string())?;
    let in_width = |field: &str, v: &NumText| -> Result<u32, String> {
        u32::try_from(number(field, v)?)
            .ok()
            .filter(|v| *v <= width.max_value())
            .ok_or_else(|| format!("{field} does not fit a {width:?} write"))
    };
    let value = in_width("value", &raw.value)?;
    let original = raw
        .original
        .as_ref()
        .map(|v| in_width("original", v))
        .transpose()?;
    let timing = match raw
        .timing
        .as_deref()
        .map(|t| t.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("") | Some("unknown") => AmigaTrainerTiming::Unknown,
        Some("immediate") => AmigaTrainerTiming::Immediate,
        Some("after_boot") => AmigaTrainerTiming::AfterBoot,
        Some("every_frame") => AmigaTrainerTiming::EveryFrame,
        Some(other) => return Err(format!("unknown timing {other:?}")),
    };
    Ok(AmigaMemoryWrite {
        width,
        address,
        value,
        original,
        timing,
    })
}

fn convert(index: usize, raw: RawEntry, source_name: &str) -> Result<AmigaTrainer, String> {
    let mechanism = match raw.mechanism.trim().to_ascii_lowercase().as_str() {
        "memory_write" => AmigaTrainerMechanism::MemoryWrite,
        "boot_menu" => AmigaTrainerMechanism::BootMenu,
        "trained_disk" => AmigaTrainerMechanism::TrainedDisk,
        "action_replay_code" => AmigaTrainerMechanism::ActionReplayCode,
        "save_state_edit" => AmigaTrainerMechanism::SaveStateEdit,
        "disk_patch" => AmigaTrainerMechanism::DiskPatch,
        other => return Err(format!("unknown mechanism {other:?}")),
    };
    if raw.writes.len() > MAX_WRITES_PER_TRAINER {
        return Err(format!(
            "{} writes; limit is {MAX_WRITES_PER_TRAINER}",
            raw.writes.len()
        ));
    }
    let writes = raw
        .writes
        .iter()
        .map(write)
        .collect::<Result<Vec<_>, _>>()?;
    if mechanism == AmigaTrainerMechanism::MemoryWrite && writes.is_empty() {
        return Err("a memory-write trainer needs at least one write".into());
    }
    if raw.target.media.is_empty() || raw.target.media.len() > MAX_MEDIA_REFS_PER_TRAINER {
        return Err(format!(
            "target.media needs 1 to {MAX_MEDIA_REFS_PER_TRAINER} images"
        ));
    }
    let mut media = Vec::new();
    for m in &raw.target.media {
        let sha = sha256("target.media.sha256", &m.sha256)?;
        if media
            .iter()
            .any(|existing: &AmigaMediaRef| existing.sha256 == sha)
        {
            return Err("target.media lists the same image twice".into());
        }
        let disk = m
            .disk
            .as_ref()
            .map(|d| disk_number("target.media.disk", d))
            .transpose()?;
        media.push(AmigaMediaRef { sha256: sha, disk });
    }
    let region = optional_text(
        "target.region",
        raw.target.region.as_ref(),
        MAX_SOURCE_FIELD_BYTES,
    )?;
    let revision = optional_text(
        "target.revision",
        raw.target.revision.as_ref(),
        MAX_SOURCE_FIELD_BYTES,
    )?;
    let scope = match raw.target.scope.trim().to_ascii_lowercase().as_str() {
        "whole_title" => AmigaTrainerScope::WholeTitle,
        "disk" => AmigaTrainerScope::Disk(match &raw.target.disk {
            Some(d) => disk_number("target.disk", d)?,
            None => return Err("scope \"disk\" needs target.disk".into()),
        }),
        "revision" => {
            if revision.is_none() {
                return Err("scope \"revision\" needs target.revision".into());
            }
            AmigaTrainerScope::Revision
        }
        other => return Err(format!("unknown scope {other:?}")),
    };
    let release = match &raw.target.release {
        None => None,
        Some(r) => Some(IdentityKey::new(
            text(
                "target.release.namespace",
                &r.namespace,
                MAX_SOURCE_FIELD_BYTES,
                true,
            )?,
            text(
                "target.release.value",
                &r.value,
                MAX_SOURCE_FIELD_BYTES,
                true,
            )?,
        )),
    };
    Ok(AmigaTrainer {
        index,
        platform: text("platform", &raw.platform, MAX_SOURCE_FIELD_BYTES, true)?,
        title: text("title", &raw.title, MAX_TITLE_BYTES, true)?,
        description: optional_text(
            "description",
            raw.description.as_ref(),
            MAX_DESCRIPTION_BYTES,
        )?,
        mechanism,
        target: AmigaTrainerTarget {
            media,
            scope,
            release,
            region,
            revision,
        },
        writes,
        source: AmigaTrainerSource {
            name: text(
                "source.name",
                &raw.source.name,
                MAX_SOURCE_FIELD_BYTES,
                true,
            )
            .unwrap_or_else(|_| source_name.to_string()),
            reference: optional_text(
                "source.reference",
                raw.source.reference.as_ref(),
                MAX_SOURCE_FIELD_BYTES,
            )?,
        },
    })
}

/// Import declarations from bytes already in memory.
pub fn import_amiga_trainers_from_bytes(
    bytes: &[u8],
    source_name: &str,
) -> Result<AmigaTrainerImport, ImportError> {
    if bytes.len() > MAX_TRAINER_SOURCE_BYTES {
        return Err(ImportError::TooLarge {
            bytes: bytes.len(),
            max: MAX_TRAINER_SOURCE_BYTES,
        });
    }
    let file: RawFile =
        serde_json::from_slice(bytes).map_err(|e| ImportError::Malformed(e.to_string()))?;
    if file.schema != AMIGA_TRAINER_SCHEMA {
        return Err(ImportError::UnsupportedSchema(
            file.schema.chars().take(64).collect(),
        ));
    }
    let source_sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let source_name: String = source_name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_SOURCE_FIELD_BYTES)
        .collect();
    let mut out = AmigaTrainerImport {
        trainers: Vec::new(),
        diagnostics: Vec::new(),
        rejected: 0,
        dropped_over_limit: file.entries.len().saturating_sub(MAX_TRAINERS_PER_FILE),
        source_name: source_name.clone(),
        source_sha256,
    };
    for (index, value) in file
        .entries
        .into_iter()
        .take(MAX_TRAINERS_PER_FILE)
        .enumerate()
    {
        let result = serde_json::from_value::<RawEntry>(value)
            .map_err(|e| e.to_string())
            .and_then(|raw| convert(index, raw, &source_name));
        match result {
            Ok(trainer) => out.trainers.push(trainer),
            Err(reason) => {
                out.rejected += 1;
                if out.diagnostics.len() < MAX_REPORTED_ITEMS {
                    out.diagnostics.push(ImportDiagnostic { index, reason });
                }
            }
        }
    }
    Ok(out)
}

/// Import declarations from a file, read-only and size-bounded.
pub fn import_amiga_trainers_from_file(path: &Path) -> Result<AmigaTrainerImport, ImportError> {
    let link = fs::symlink_metadata(path).map_err(|e| ImportError::Io(e.to_string()))?;
    if link.file_type().is_symlink() || !link.is_file() {
        return Err(ImportError::NotARegularFile);
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| ImportError::Io(e.to_string()))?
        .take(MAX_TRAINER_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ImportError::Io(e.to_string()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    import_amiga_trainers_from_bytes(&bytes, &name)
}
