//! Read-only RetroBIOS metadata import and readiness enrichment.
//!
//! This module deliberately accepts only caller-supplied JSON snapshots. It
//! never fetches a URL, opens a release asset, stores firmware bytes, or
//! changes a [`crate::bios_projection::BiosRequirement`]. RetroBIOS is a
//! supplementary source: emulator/vendor requirements and existing EmuWiz
//! DAT evidence remain authoritative.

use std::fmt;

use serde_json::Value;

use crate::bios_projection::{BiosEvidence, BiosMatchStatus, BiosRequirement};
use crate::launch::readiness::FirmwareReadiness;

pub const RETROBIOS_PROVIDER_ID: &str = "retrobios";
pub const RETROBIOS_UPSTREAM_URL: &str = "https://github.com/Abdess/retrobios";
pub const RETROBIOS_MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
pub const RETROBIOS_MAX_ITEMS: usize = 20_000;
pub const RETROBIOS_MAX_ALIASES: usize = 32;
pub const RETROBIOS_MAX_STRING_BYTES: usize = 1024;

/// The verification behavior described by an upstream platform/emulator.
/// Presence-only evidence never becomes `FirmwareReadiness::Verified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetroBiosVerificationMode {
    ExactHash,
    Size,
    PresenceOnly,
    Crypto,
    Unknown,
}

/// Acquisition permission is intentionally independent from local readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetroBiosAcquisitionState {
    Redistributable,
    OfficialVendorSource,
    BrowserHandoff,
    UserMustProvide,
    UnknownLicense,
    DoNotAutomate,
}

/// Requiredness imported from a platform manifest or emulator profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetroBiosRequiredness {
    Required,
    Optional,
    HleFallback,
    NotRequired,
    Unknown,
}

/// Explicit EmuWiz target identities. Unknown strings are not normalized into
/// one of these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetroBiosCanonicalTarget {
    Ps1DuckStation,
    Ps1RetroArchCore,
    Ps2Pcsx2,
    Ps2RetroArchCore,
    Saturn,
    DreamcastFlycast,
    DreamcastRetroArchCore,
    AmigaKickstart,
    AtariStTos,
    ArcadeDependency,
    XboxXemu,
    Rpcs3Firmware,
    PpssppNotRequired,
    DolphinOptionalIpl,
    Unmapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetroBiosSignatureState {
    Verified,
    Present,
    Absent,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroBiosSnapshotProvenance {
    pub provider_id: String,
    pub upstream_url: String,
    pub upstream_ref: String,
    pub snapshot_digest: String,
    pub snapshot_timestamp: String,
    pub imported_at: String,
    pub signature_state: RetroBiosSignatureState,
    pub mirror_url: Option<String>,
    pub mirror_used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetroBiosHashes {
    pub sha256: Option<String>,
    pub sha1: Option<String>,
    pub md5: Option<String>,
    pub crc32: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroBiosMetadataItem {
    pub canonical_filename: String,
    pub aliases: Vec<String>,
    pub destination: Option<String>,
    pub emulator: String,
    pub core: Option<String>,
    pub platform: String,
    pub system: String,
    pub requiredness: RetroBiosRequiredness,
    pub region: Option<String>,
    pub revision: Option<String>,
    pub version: Option<String>,
    pub size_bytes: Option<u64>,
    pub hashes: RetroBiosHashes,
    pub verification_mode: RetroBiosVerificationMode,
    pub source_path: String,
    pub acquisition_state: RetroBiosAcquisitionState,
    pub canonical_target: RetroBiosCanonicalTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroBiosMetadataSnapshot {
    pub schema_version: u32,
    pub provenance: RetroBiosSnapshotProvenance,
    pub items: Vec<RetroBiosMetadataItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroBiosJoin {
    pub provider_available: bool,
    pub canonical_target: RetroBiosCanonicalTarget,
    pub match_status: BiosMatchStatus,
    pub readiness: FirmwareReadiness,
    pub requiredness: RetroBiosRequiredness,
    pub acquisition_state: RetroBiosAcquisitionState,
    pub provenance: Option<RetroBiosSnapshotProvenance>,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetroBiosImportError {
    SnapshotTooLarge,
    InvalidJson(String),
    InvalidRoot,
    InvalidProvider(String),
    MissingField(&'static str),
    InvalidField(String),
    MutableOrUnpinnedRef(String),
    TooManyItems,
    TooManyAliases,
}

impl fmt::Display for RetroBiosImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SnapshotTooLarge => {
                write!(f, "RetroBIOS metadata snapshot exceeds the bounded limit")
            }
            Self::InvalidJson(detail) => write!(f, "invalid RetroBIOS metadata JSON: {detail}"),
            Self::InvalidRoot => write!(f, "RetroBIOS metadata root must be an object"),
            Self::InvalidProvider(provider) => {
                write!(f, "unexpected RetroBIOS provider id: {provider}")
            }
            Self::MissingField(field) => write!(f, "RetroBIOS metadata is missing {field}"),
            Self::InvalidField(field) => write!(f, "invalid RetroBIOS metadata field: {field}"),
            Self::MutableOrUnpinnedRef(reference) => write!(
                f,
                "RetroBIOS metadata ref is mutable or unpinned: {reference}"
            ),
            Self::TooManyItems => write!(f, "RetroBIOS metadata snapshot contains too many items"),
            Self::TooManyAliases => write!(f, "RetroBIOS metadata item contains too many aliases"),
        }
    }
}

impl std::error::Error for RetroBiosImportError {}

pub fn parse_snapshot(
    bytes: &[u8],
    imported_at: impl Into<String>,
) -> Result<RetroBiosMetadataSnapshot, RetroBiosImportError> {
    if bytes.len() > RETROBIOS_MAX_SNAPSHOT_BYTES {
        return Err(RetroBiosImportError::SnapshotTooLarge);
    }
    let root: Value = serde_json::from_slice(bytes)
        .map_err(|error| RetroBiosImportError::InvalidJson(error.to_string()))?;
    let object = root.as_object().ok_or(RetroBiosImportError::InvalidRoot)?;
    let provider_id = required_string(object, "provider")?;
    if provider_id != RETROBIOS_PROVIDER_ID {
        return Err(RetroBiosImportError::InvalidProvider(provider_id));
    }
    let schema_version = object
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or(RetroBiosImportError::MissingField("schema_version"))?;
    let upstream = object
        .get("upstream")
        .and_then(Value::as_object)
        .ok_or(RetroBiosImportError::MissingField("upstream"))?;
    let upstream_url = required_string(upstream, "url")?;
    if upstream_url != RETROBIOS_UPSTREAM_URL {
        return Err(RetroBiosImportError::InvalidField(format!(
            "upstream.url must be {RETROBIOS_UPSTREAM_URL}"
        )));
    }
    let upstream_ref = required_string(upstream, "ref")?;
    if is_mutable_or_unpinned_ref(&upstream_ref) {
        return Err(RetroBiosImportError::MutableOrUnpinnedRef(upstream_ref));
    }
    let snapshot_digest = required_string(upstream, "snapshot_digest")?;
    if !is_hex_digest(&snapshot_digest, 64) {
        return Err(RetroBiosImportError::InvalidField(
            "upstream.snapshot_digest must be SHA-256 hex".to_string(),
        ));
    }
    let snapshot_timestamp = required_string(upstream, "timestamp")?;
    let mirror_used = upstream
        .get("mirror_used")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mirror_url = optional_string(upstream, "mirror_url")?;
    let signature_state = parse_signature_state(
        upstream
            .get("signature_state")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
    )?;
    let values = object
        .get("items")
        .and_then(Value::as_array)
        .ok_or(RetroBiosImportError::MissingField("items"))?;
    if values.len() > RETROBIOS_MAX_ITEMS {
        return Err(RetroBiosImportError::TooManyItems);
    }
    let mut items = Vec::with_capacity(values.len());
    for value in values {
        items.push(parse_item(value)?);
    }
    Ok(RetroBiosMetadataSnapshot {
        schema_version: u32::try_from(schema_version)
            .map_err(|_| RetroBiosImportError::InvalidField("schema_version".to_string()))?,
        provenance: RetroBiosSnapshotProvenance {
            provider_id,
            upstream_url,
            upstream_ref,
            snapshot_digest,
            snapshot_timestamp,
            imported_at: imported_at.into(),
            signature_state,
            mirror_url,
            mirror_used,
        },
        items,
    })
}

fn parse_item(value: &Value) -> Result<RetroBiosMetadataItem, RetroBiosImportError> {
    let object = value
        .as_object()
        .ok_or_else(|| RetroBiosImportError::InvalidField("items[]".to_string()))?;
    let canonical_filename = bounded_string(required_string(object, "filename")?)?;
    let aliases = object
        .get("aliases")
        .and_then(Value::as_array)
        .map(|values| {
            if values.len() > RETROBIOS_MAX_ALIASES {
                return Err(RetroBiosImportError::TooManyAliases);
            }
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| RetroBiosImportError::InvalidField("aliases[]".to_string()))
                        .and_then(|value| bounded_string(value.to_string()))
                })
                .collect()
        })
        .transpose()?
        .unwrap_or_default();
    let emulator = bounded_string(required_string(object, "emulator")?)?;
    let core = optional_bounded_string(object, "core")?;
    let platform = bounded_string(required_string(object, "platform")?)?;
    let system = bounded_string(required_string(object, "system")?)?;
    let destination = optional_bounded_string(object, "destination")?;
    let region = optional_bounded_string(object, "region")?;
    let revision = optional_bounded_string(object, "revision")?;
    let version = optional_bounded_string(object, "version")?;
    let source_path = bounded_string(required_string(object, "source_path")?)?;
    let size_bytes = object.get("size").and_then(Value::as_u64);
    let requiredness = parse_requiredness(object)?;
    let verification_mode = parse_verification_mode(
        object
            .get("verification_mode")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
    )?;
    let hashes = parse_hashes(object)?;
    let canonical_target = map_canonical_target(
        &system,
        &emulator,
        core.as_deref(),
        &platform,
        &requiredness,
    );
    let acquisition_state = object
        .get("acquisition_state")
        .and_then(Value::as_str)
        .map(parse_acquisition_state)
        .transpose()?
        .unwrap_or_else(|| default_acquisition_state(canonical_target));
    Ok(RetroBiosMetadataItem {
        canonical_filename,
        aliases,
        destination,
        emulator,
        core,
        platform,
        system,
        requiredness,
        region,
        revision,
        version,
        size_bytes,
        hashes,
        verification_mode,
        source_path,
        acquisition_state,
        canonical_target,
    })
}

fn parse_hashes(
    object: &serde_json::Map<String, Value>,
) -> Result<RetroBiosHashes, RetroBiosImportError> {
    let nested = object.get("hashes").and_then(Value::as_object);
    let value = |name: &str| -> Result<Option<String>, RetroBiosImportError> {
        let raw = object
            .get(name)
            .or_else(|| nested.and_then(|values| values.get(name)));
        raw.map(|raw| {
            raw.as_str()
                .ok_or_else(|| RetroBiosImportError::InvalidField(format!("hashes.{name}")))
                .and_then(|raw| {
                    let expected = match name {
                        "sha256" => 64,
                        "sha1" => 40,
                        "md5" => 32,
                        "crc32" => 8,
                        _ => unreachable!(),
                    };
                    if !is_hex_digest(raw, expected) {
                        return Err(RetroBiosImportError::InvalidField(format!("hashes.{name}")));
                    }
                    Ok(raw.to_ascii_lowercase())
                })
        })
        .transpose()
    };
    Ok(RetroBiosHashes {
        sha256: value("sha256")?,
        sha1: value("sha1")?,
        md5: value("md5")?,
        crc32: value("crc32")?,
    })
}

fn parse_requiredness(
    object: &serde_json::Map<String, Value>,
) -> Result<RetroBiosRequiredness, RetroBiosImportError> {
    if object.get("not_required").and_then(Value::as_bool) == Some(true) {
        return Ok(RetroBiosRequiredness::NotRequired);
    }
    if object.get("hle_fallback").and_then(Value::as_bool) == Some(true) {
        return Ok(RetroBiosRequiredness::HleFallback);
    }
    if object.get("required").and_then(Value::as_bool) == Some(true) {
        return Ok(RetroBiosRequiredness::Required);
    }
    if object.get("optional").and_then(Value::as_bool) == Some(true) {
        return Ok(RetroBiosRequiredness::Optional);
    }
    Ok(RetroBiosRequiredness::Unknown)
}

fn parse_signature_state(value: &str) -> Result<RetroBiosSignatureState, RetroBiosImportError> {
    match value {
        "verified" => Ok(RetroBiosSignatureState::Verified),
        "present" => Ok(RetroBiosSignatureState::Present),
        "absent" => Ok(RetroBiosSignatureState::Absent),
        "unknown" => Ok(RetroBiosSignatureState::Unknown),
        _ => Err(RetroBiosImportError::InvalidField(
            "signature_state".to_string(),
        )),
    }
}

fn parse_verification_mode(value: &str) -> Result<RetroBiosVerificationMode, RetroBiosImportError> {
    match value {
        "exact_hash" => Ok(RetroBiosVerificationMode::ExactHash),
        "size" => Ok(RetroBiosVerificationMode::Size),
        "presence_only" => Ok(RetroBiosVerificationMode::PresenceOnly),
        "crypto" => Ok(RetroBiosVerificationMode::Crypto),
        "unknown" => Ok(RetroBiosVerificationMode::Unknown),
        _ => Err(RetroBiosImportError::InvalidField(
            "verification_mode".to_string(),
        )),
    }
}

fn parse_acquisition_state(value: &str) -> Result<RetroBiosAcquisitionState, RetroBiosImportError> {
    match value {
        "Redistributable" => Ok(RetroBiosAcquisitionState::Redistributable),
        "OfficialVendorSource" => Ok(RetroBiosAcquisitionState::OfficialVendorSource),
        "BrowserHandoff" => Ok(RetroBiosAcquisitionState::BrowserHandoff),
        "UserMustProvide" => Ok(RetroBiosAcquisitionState::UserMustProvide),
        "UnknownLicense" => Ok(RetroBiosAcquisitionState::UnknownLicense),
        "DoNotAutomate" => Ok(RetroBiosAcquisitionState::DoNotAutomate),
        _ => Err(RetroBiosImportError::InvalidField(
            "acquisition_state".to_string(),
        )),
    }
}

fn default_acquisition_state(target: RetroBiosCanonicalTarget) -> RetroBiosAcquisitionState {
    match target {
        RetroBiosCanonicalTarget::Rpcs3Firmware => RetroBiosAcquisitionState::OfficialVendorSource,
        RetroBiosCanonicalTarget::PpssppNotRequired
        | RetroBiosCanonicalTarget::DolphinOptionalIpl => RetroBiosAcquisitionState::DoNotAutomate,
        RetroBiosCanonicalTarget::Unmapped => RetroBiosAcquisitionState::UnknownLicense,
        _ => RetroBiosAcquisitionState::UserMustProvide,
    }
}

/// Explicitly maps reviewed upstream identities. This is intentionally a
/// closed table: unknown slugs are `Unmapped`, never guessed by normalization.
pub fn map_canonical_target(
    system: &str,
    emulator: &str,
    core: Option<&str>,
    _platform: &str,
    requiredness: &RetroBiosRequiredness,
) -> RetroBiosCanonicalTarget {
    match (system, emulator, core) {
        ("sony-playstation", "duckstation", _) => RetroBiosCanonicalTarget::Ps1DuckStation,
        ("sony-playstation-2", "pcsx2", _) => RetroBiosCanonicalTarget::Ps2Pcsx2,
        ("sega-saturn", "mednafen" | "kronos" | "yabause" | "yabasanshiro", _) => {
            RetroBiosCanonicalTarget::Saturn
        }
        ("sega-dreamcast", "flycast", _) => RetroBiosCanonicalTarget::DreamcastFlycast,
        ("amiga", "amiberry" | "fs-uae" | "winuae", _) => RetroBiosCanonicalTarget::AmigaKickstart,
        ("atari-st", "hatari", _) => RetroBiosCanonicalTarget::AtariStTos,
        ("microsoft-xbox", "xemu", _) => RetroBiosCanonicalTarget::XboxXemu,
        ("sony-playstation-3", "rpcs3", _) => RetroBiosCanonicalTarget::Rpcs3Firmware,
        ("sony-psp", "ppsspp", _) => RetroBiosCanonicalTarget::PpssppNotRequired,
        ("nintendo-gamecube", "dolphin", _) if *requiredness != RetroBiosRequiredness::Required => {
            RetroBiosCanonicalTarget::DolphinOptionalIpl
        }
        ("arcade", emulator, Some(core))
            if matches!(emulator, "mame" | "fbneo")
                && matches!(core, "mame" | "mame2010" | "mame2003-plus" | "fbneo") =>
        {
            RetroBiosCanonicalTarget::ArcadeDependency
        }
        ("arcade", "mame" | "fbneo", None) => RetroBiosCanonicalTarget::ArcadeDependency,
        (
            "sony-playstation",
            "retroarch",
            Some("swanstation" | "pcsx-rearmed" | "mednafen_psx_hw_libretro"),
        ) => RetroBiosCanonicalTarget::Ps1RetroArchCore,
        ("sony-playstation-2", "retroarch", Some("pcsx2")) => {
            RetroBiosCanonicalTarget::Ps2RetroArchCore
        }
        ("sega-saturn", "retroarch", Some("beetle_saturn" | "yabasanshiro")) => {
            RetroBiosCanonicalTarget::Saturn
        }
        ("sega-dreamcast", "retroarch", Some("flycast")) => {
            RetroBiosCanonicalTarget::DreamcastRetroArchCore
        }
        _ => RetroBiosCanonicalTarget::Unmapped,
    }
}

/// Joins one imported record to existing local BIOS evidence. The returned
/// readiness starts from the existing adapter/DAT result and never lets a
/// supplementary record override `NotRequired`, authoritative `Verified`, or
/// authoritative `Missing`. `None` for the record represents provider
/// unavailability and leaves readiness unchanged.
pub fn join_bios_evidence(
    requirement: &BiosRequirement,
    local: Option<&BiosEvidence>,
    authoritative_readiness: FirmwareReadiness,
    record: Option<&RetroBiosMetadataItem>,
    provenance: Option<&RetroBiosSnapshotProvenance>,
) -> RetroBiosJoin {
    let target = record
        .map(|record| record.canonical_target)
        .unwrap_or(RetroBiosCanonicalTarget::Unmapped);
    let (requiredness, acquisition_state) = record
        .map(|record| (record.requiredness, record.acquisition_state))
        .unwrap_or((
            RetroBiosRequiredness::Unknown,
            RetroBiosAcquisitionState::UnknownLicense,
        ));
    let Some(record) = record else {
        return RetroBiosJoin {
            provider_available: false,
            canonical_target: target,
            match_status: BiosMatchStatus::Unknown,
            readiness: authoritative_readiness,
            requiredness,
            acquisition_state,
            provenance: provenance.cloned(),
            explanation: "RetroBIOS metadata unavailable; existing readiness preserved".to_string(),
        };
    };
    let Some(local) = local else {
        return RetroBiosJoin {
            provider_available: true,
            canonical_target: target,
            match_status: if authoritative_readiness == FirmwareReadiness::Missing {
                BiosMatchStatus::Missing
            } else {
                BiosMatchStatus::Unknown
            },
            readiness: authoritative_readiness,
            requiredness,
            acquisition_state,
            provenance: provenance.cloned(),
            explanation: "no local candidate; RetroBIOS metadata does not establish absence"
                .to_string(),
        };
    };
    let filename_matches = record.canonical_filename == local.filename
        || record.aliases.iter().any(|alias| alias == &local.filename)
        || requirement
            .expected_filenames
            .iter()
            .any(|name| name == &local.filename);
    let size_matches = record
        .size_bytes
        .is_none_or(|size| size == local.size_bytes);
    let hash_matches = record
        .hashes
        .sha256
        .as_deref()
        .zip(local.sha256.as_deref())
        .is_some_and(|(expected, actual)| expected.eq_ignore_ascii_case(actual));
    let exact = record.verification_mode == RetroBiosVerificationMode::ExactHash
        && size_matches
        && hash_matches;
    let mismatch = filename_matches
        && record.verification_mode == RetroBiosVerificationMode::ExactHash
        && (!size_matches || !hash_matches);
    let (match_status, supplementary_readiness, explanation) = if exact {
        (
            BiosMatchStatus::VerifiedMatch,
            FirmwareReadiness::Verified,
            "local file matches RetroBIOS accepted hash and size".to_string(),
        )
    } else if mismatch {
        (
            BiosMatchStatus::HashMismatch,
            FirmwareReadiness::PresentUnverified,
            "filename matches, but RetroBIOS hash or size does not".to_string(),
        )
    } else if filename_matches {
        (
            BiosMatchStatus::FilenameOnly,
            FirmwareReadiness::PresentUnverified,
            "local filename matches; RetroBIOS verification mode does not prove content"
                .to_string(),
        )
    } else {
        (
            BiosMatchStatus::Unknown,
            FirmwareReadiness::Unknown,
            "local candidate does not match the RetroBIOS filename or aliases".to_string(),
        )
    };
    let readiness = match authoritative_readiness {
        FirmwareReadiness::NotRequired
        | FirmwareReadiness::Verified
        | FirmwareReadiness::Missing => authoritative_readiness,
        _ => supplementary_readiness,
    };
    RetroBiosJoin {
        provider_available: true,
        canonical_target: target,
        match_status,
        readiness,
        requiredness,
        acquisition_state,
        provenance: provenance.cloned(),
        explanation,
    }
}

fn required_string(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<String, RetroBiosImportError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(RetroBiosImportError::MissingField(field))
}

fn optional_string(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, RetroBiosImportError> {
    object
        .get(field)
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| RetroBiosImportError::InvalidField(field.to_string()))
        })
        .transpose()
}

fn optional_bounded_string(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, RetroBiosImportError> {
    optional_string(object, field)?
        .map(bounded_string)
        .transpose()
}

fn bounded_string(value: String) -> Result<String, RetroBiosImportError> {
    if value.is_empty() || value.len() > RETROBIOS_MAX_STRING_BYTES {
        return Err(RetroBiosImportError::InvalidField(
            "bounded string".to_string(),
        ));
    }
    Ok(value)
}

fn is_hex_digest(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_mutable_or_unpinned_ref(reference: &str) -> bool {
    reference.is_empty()
        || matches!(reference, "main" | "master" | "HEAD" | "head")
        || reference.contains([' ', '\t', '\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bios_projection::{
        BiosContentClass, BiosProjectionMethod, BiosProjectionTarget, BiosTargetState,
    };
    use serde_json::json;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";
    const MD5: &str = "0123456789abcdef0123456789abcdef";
    const CRC32: &str = "0123abcd";

    fn base_item() -> serde_json::Value {
        json!({
            "filename": "scph5501.bin",
            "aliases": ["SCPH-5501.BIN"],
            "destination": "bios/scph5501.bin",
            "emulator": "duckstation",
            "platform": "RetroArch",
            "system": "sony-playstation",
            "required": true,
            "region": "US",
            "revision": "5501",
            "version": "3.0",
            "size": 524288,
            "hashes": {"sha256": DIGEST, "sha1": SHA1, "md5": MD5, "crc32": CRC32},
            "verification_mode": "exact_hash",
            "source_path": "platforms/retroarch.yml#sony-playstation/scph5501.bin",
            "acquisition_state": "UserMustProvide"
        })
    }

    fn snapshot(items: Vec<Value>, reference: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "provider": "retrobios",
            "upstream": {
                "url": RETROBIOS_UPSTREAM_URL,
                "ref": reference,
                "snapshot_digest": DIGEST,
                "timestamp": "2026-09-20T00:00:00Z",
                "signature_state": "verified",
                "mirror_used": false
            },
            "items": items
        }))
        .unwrap()
    }

    fn requirement() -> BiosRequirement {
        BiosRequirement {
            name: "PS1 BIOS".to_string(),
            emulator: "DuckStation".to_string(),
            expected_filenames: vec!["scph5501.bin".to_string()],
            expected_sha256: Some(DIGEST.to_string()),
            target: BiosProjectionTarget {
                description: "DuckStation BIOS".to_string(),
                path: None,
                current_state: BiosTargetState::ExistingRegularFile,
            },
            content_class: BiosContentClass::ImmutableFirmware,
            method: BiosProjectionMethod::ExternalSystemData,
        }
    }

    fn local(filename: &str, digest: Option<&str>) -> BiosEvidence {
        BiosEvidence {
            relative_path: filename.into(),
            filename: filename.to_string(),
            size_bytes: 524288,
            sha256: digest.map(ToOwned::to_owned),
            source: crate::bios_projection::BiosEvidenceSource::Hash,
            match_status: BiosMatchStatus::Unknown,
            platform: Some("PS1".to_string()),
        }
    }

    #[test]
    fn duckstation_exact_ps1_hash_is_supplementary_verified() {
        let parsed = parse_snapshot(&snapshot(vec![base_item()], "v2026.09.04"), "now").unwrap();
        let item = &parsed.items[0];
        assert_eq!(
            item.canonical_target,
            RetroBiosCanonicalTarget::Ps1DuckStation
        );
        let joined = join_bios_evidence(
            &requirement(),
            Some(&local("scph5501.bin", Some(DIGEST))),
            FirmwareReadiness::PresentUnverified,
            Some(item),
            Some(&parsed.provenance),
        );
        assert_eq!(joined.match_status, BiosMatchStatus::VerifiedMatch);
        assert_eq!(joined.readiness, FirmwareReadiness::Verified);
    }

    #[test]
    fn filename_only_is_present_unverified() {
        let mut item = base_item();
        item["verification_mode"] = json!("presence_only");
        let parsed = parse_snapshot(&snapshot(vec![item], "v2026.09.04"), "now").unwrap();
        let joined = join_bios_evidence(
            &requirement(),
            Some(&local("scph5501.bin", None)),
            FirmwareReadiness::PresentUnverified,
            Some(&parsed.items[0]),
            Some(&parsed.provenance),
        );
        assert_eq!(joined.match_status, BiosMatchStatus::FilenameOnly);
        assert_eq!(joined.readiness, FirmwareReadiness::PresentUnverified);
    }

    #[test]
    fn pcsx2_preserves_region_and_revision() {
        let mut item = base_item();
        item["emulator"] = json!("pcsx2");
        item["system"] = json!("sony-playstation-2");
        item["filename"] = json!("SCPH-39001.BIN");
        item["region"] = json!("US");
        item["revision"] = json!("39001");
        let parsed = parse_snapshot(&snapshot(vec![item], "v2026.09.04"), "now").unwrap();
        let record = &parsed.items[0];
        assert_eq!(record.canonical_target, RetroBiosCanonicalTarget::Ps2Pcsx2);
        assert_eq!(record.region.as_deref(), Some("US"));
        assert_eq!(record.revision.as_deref(), Some("39001"));
    }

    #[test]
    fn ppsspp_does_not_override_not_required() {
        let mut item = base_item();
        item["emulator"] = json!("ppsspp");
        item["system"] = json!("sony-psp");
        let parsed = parse_snapshot(&snapshot(vec![item], "v2026.09.04"), "now").unwrap();
        let joined = join_bios_evidence(
            &requirement(),
            None,
            FirmwareReadiness::NotRequired,
            Some(&parsed.items[0]),
            Some(&parsed.provenance),
        );
        assert_eq!(
            joined.canonical_target,
            RetroBiosCanonicalTarget::PpssppNotRequired
        );
        assert_eq!(joined.readiness, FirmwareReadiness::NotRequired);
    }

    #[test]
    fn dolphin_optional_ipl_stays_optional() {
        let mut item = base_item();
        item["emulator"] = json!("dolphin");
        item["system"] = json!("nintendo-gamecube");
        item.as_object_mut().unwrap().remove("required");
        item["optional"] = json!(true);
        let parsed = parse_snapshot(&snapshot(vec![item], "v2026.09.04"), "now").unwrap();
        assert_eq!(
            parsed.items[0].requiredness,
            RetroBiosRequiredness::Optional
        );
        assert_eq!(
            parsed.items[0].canonical_target,
            RetroBiosCanonicalTarget::DolphinOptionalIpl
        );
    }

    #[test]
    fn rpcs3_does_not_become_generic_bios_requirement() {
        let mut item = base_item();
        item["emulator"] = json!("rpcs3");
        item["system"] = json!("sony-playstation-3");
        item.as_object_mut().unwrap().remove("acquisition_state");
        let parsed = parse_snapshot(&snapshot(vec![item], "v2026.09.04"), "now").unwrap();
        assert_eq!(
            parsed.items[0].canonical_target,
            RetroBiosCanonicalTarget::Rpcs3Firmware
        );
        assert_eq!(
            parsed.items[0].acquisition_state,
            RetroBiosAcquisitionState::OfficialVendorSource
        );
    }

    #[test]
    fn mame_device_preserves_context() {
        let mut item = base_item();
        item["emulator"] = json!("mame");
        item["core"] = json!("mame");
        item["platform"] = json!("Arcade");
        item["system"] = json!("arcade");
        item["source_path"] = json!("emulators/mame.yml#sets/neogeo/device");
        let parsed = parse_snapshot(&snapshot(vec![item], "v2026.09.04"), "now").unwrap();
        assert_eq!(
            parsed.items[0].canonical_target,
            RetroBiosCanonicalTarget::ArcadeDependency
        );
        assert!(parsed.items[0].source_path.contains("device"));
    }

    #[test]
    fn atari_tos_revision_is_retained() {
        let mut item = base_item();
        item["emulator"] = json!("hatari");
        item["system"] = json!("atari-st");
        item["revision"] = json!("TOS 1.04");
        let parsed = parse_snapshot(&snapshot(vec![item], "v2026.09.04"), "now").unwrap();
        assert_eq!(
            parsed.items[0].canonical_target,
            RetroBiosCanonicalTarget::AtariStTos
        );
        assert_eq!(parsed.items[0].revision.as_deref(), Some("TOS 1.04"));
    }

    #[test]
    fn amiga_is_hashable_but_conservatively_user_provided() {
        let mut item = base_item();
        item["emulator"] = json!("amiberry");
        item["system"] = json!("amiga");
        item["version"] = json!("Kickstart 3.1");
        let parsed = parse_snapshot(&snapshot(vec![item], "v2026.09.04"), "now").unwrap();
        assert_eq!(
            parsed.items[0].canonical_target,
            RetroBiosCanonicalTarget::AmigaKickstart
        );
        assert_eq!(
            parsed.items[0].acquisition_state,
            RetroBiosAcquisitionState::UserMustProvide
        );
        assert_eq!(parsed.items[0].hashes.sha256.as_deref(), Some(DIGEST));
    }

    #[test]
    fn provider_unavailable_preserves_readiness() {
        let joined =
            join_bios_evidence(&requirement(), None, FirmwareReadiness::Missing, None, None);
        assert!(!joined.provider_available);
        assert_eq!(joined.readiness, FirmwareReadiness::Missing);
        assert_eq!(joined.match_status, BiosMatchStatus::Unknown);
    }

    #[test]
    fn mutable_snapshot_ref_is_rejected() {
        let error = parse_snapshot(&snapshot(vec![base_item()], "main"), "now").unwrap_err();
        assert!(matches!(
            error,
            RetroBiosImportError::MutableOrUnpinnedRef(_)
        ));
    }

    #[test]
    fn acquisition_state_is_separate_from_readiness() {
        let parsed = parse_snapshot(&snapshot(vec![base_item()], "v2026.09.04"), "now").unwrap();
        let joined = join_bios_evidence(
            &requirement(),
            Some(&local("scph5501.bin", Some(DIGEST))),
            FirmwareReadiness::PresentUnverified,
            Some(&parsed.items[0]),
            Some(&parsed.provenance),
        );
        assert_eq!(joined.readiness, FirmwareReadiness::Verified);
        assert_eq!(
            joined.acquisition_state,
            RetroBiosAcquisitionState::UserMustProvide
        );
    }
}
