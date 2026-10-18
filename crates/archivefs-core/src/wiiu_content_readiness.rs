//! Read-only Wii U content-readiness projection.
//!
//! Given a discovered path (a WUD/WUX/WUA file, or a directory that may hold
//! extracted/Loadiine-style content or a raw NUS package layout) plus
//! caller-supplied key-store evidence, this module projects a typed
//! [`WiiUContentProjection`]: what representation the content actually is,
//! whether it is encrypted, whether a decryption key is available, what
//! content role it plays (base game / update / DLC), how it relates to a
//! candidate base title, and whether Cemu could conceptually launch it
//! *without any further work*.
//!
//! This module never converts, decrypts, downloads, fetches title keys, or
//! mutates anything. It is a pure projection over already-available bytes on
//! disk (bounded reads only) and evidence the caller supplies. It reuses
//! [`crate::wiiu_disc::inspect_wii_u_disc`] for WUD/WUX/WUA container
//! inspection rather than duplicating that logic.
//!
//! "Valid Wii U content" and "directly launchable by the currently selected
//! Cemu path" are deliberately distinct: [`WiiUContentRepresentation`] can be
//! a real, structurally sound representation while [`WiiUContentReadiness`]
//! still reports `RequiresDecryption`, `RequiresConversion`, or `MissingKey`.
//! Filename-only evidence (a bare `.wux`/`.rpx` extension with no supporting
//! structure) never produces [`WiiUContentReadiness::DirectlyLaunchable`].

use std::fs;
pub mod launch_selection;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::wiiu_disc::{WiiUDiscFormat, WiiUDiscInspection, inspect_wii_u_disc};

const MAX_DIR_ENTRIES: usize = 4096;
const MAX_DIR_DEPTH: usize = 3;
const MAX_META_XML_BYTES: usize = 64 * 1024;
const MAX_HEADER_PROBE_BYTES: usize = 1024;

/// Signature type recorded at the start of a Wii U ticket/TMD for which this
/// module knows the fixed-offset `title_id` field location. Any other
/// signature type is treated as an unknown layout - the offset is never
/// guessed.
pub(crate) const SIG_TYPE_RSA2048_SHA256: u32 = 0x0001_0004;
pub(crate) const TICKET_TITLE_ID_OFFSET: usize = 0x1DC;
pub(crate) const TMD_TITLE_ID_OFFSET: usize = 0x18C;

/// How a piece of discovered Wii U content is physically packaged. Detected
/// from structure, not solely from a file extension or folder name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WiiUContentRepresentation {
    /// Raw Wii U optical disc image container.
    Wud,
    /// Compressed WUD container ([`crate::wiiu_disc`] header format).
    Wux,
    /// Modern Wii U archive container. This repo does not yet parse WUA
    /// internals, so recognition stops at "this is a WUA file" - inner
    /// content is never verified or faked.
    Wua,
    /// A raw NUS-style package/content layout: `title.tmd`, `title.tik`,
    /// `title.cert`, and numbered `.app` content files, either loose or
    /// under a directory.
    Nus,
    /// A Loadiine-style extracted directory: `meta/meta.xml` with a
    /// readable `title_id`, plus `code/*.rpx` and a `content/` directory.
    LoadiineExtracted,
    /// An extracted game directory with `code/*.rpx` and `content/` but no
    /// readable `meta/meta.xml` - structurally plausible but with weaker
    /// identity evidence than [`Self::LoadiineExtracted`].
    RpxExtractedGame,
    /// Present but does not match any recognized structural shape (e.g. a
    /// folder that merely happens to contain a `.rpx` file with no
    /// supporting `code`/`content` layout).
    Unknown,
}

/// The role discovered content plays relative to a title, where evidence
/// permits. Never inferred from a filename alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WiiUContentRole {
    BaseGame,
    Update,
    Dlc,
    Unknown,
}

/// Whether the inspected content's payload is encrypted, decrypted, or
/// simply not something this projection can determine yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WiiUEncryptionState {
    /// Payload bytes are readable as plaintext game data (Loadiine-style
    /// extracted dumps: no ticket, no title-key indirection).
    Decrypted,
    /// Payload is encrypted at rest (WUD/WUX inner filesystem, or an NUS
    /// package with a ticket/title key indirection).
    Encrypted,
    /// The representation does not expose enough structure to say either
    /// way (e.g. WUA, or an unrecognized layout).
    Unknown,
}

/// Whether a decryption key this content would need is available, given
/// only caller-supplied evidence. This module never fetches, derives, or
/// stores key material itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WiiUKeyReadiness {
    /// Content is already decrypted, or its representation has no key
    /// requirement at all.
    NotApplicable,
    /// Caller-supplied evidence confirms a usable key is available locally.
    AvailableLocally,
    /// Caller-supplied evidence confirms no usable key is available.
    Missing,
    /// Encrypted content whose key applicability was never confirmed either
    /// way by the caller.
    Unknown,
}

/// Caller-supplied evidence about whether a key this content needs is
/// available. Deliberately opaque to this module: it never looks inside a
/// key store, never contacts any service, and never mutates one. The caller
/// is responsible for having established this out of band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WiiUKeyStoreEvidence {
    /// The caller did not check, or checking does not apply.
    NotChecked,
    /// The caller confirmed a required key is available locally.
    ConfirmedAvailable,
    /// The caller confirmed a required key is absent.
    ConfirmedMissing,
}

/// Confidence behind an identity field, so a caller can tell a real
/// structural read from a weaker heuristic. Directory-naming evidence is
/// always weaker than a parsed binary/XML field and is never sufficient on
/// its own for a `DirectlyLaunchable` verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WiiUIdentityConfidence {
    /// Parsed from a structured field: `meta.xml`'s `title_id`, or a
    /// ticket/TMD `title_id` at its known fixed offset for a recognized
    /// signature type.
    StructuredField,
    /// Derived only from a directory's own name matching a 16-hex-digit
    /// title-id shape. Weaker: kept explicit, never promoted to
    /// `StructuredField`, never used to grant `DirectlyLaunchable`.
    DirectoryNamingConvention,
}

/// Title/product identity retained from already-available structural
/// evidence - never invented, never fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUContentIdentity {
    /// 16 uppercase hex characters, e.g. `00050000101B7900`.
    pub title_id: Option<String>,
    /// e.g. `WUP-P-ARDE`, when `meta.xml` exposes it.
    pub product_code: Option<String>,
    /// Raw region evidence exactly as read (`meta.xml`'s `region` field, an
    /// 8-hex-digit bitmask). Not decoded into a country name - that mapping
    /// is not this module's job.
    pub region_raw: Option<String>,
    pub confidence: WiiUIdentityConfidence,
    pub source: String,
}

/// How update/DLC content relates to a candidate base title, when the
/// caller supplies one to check against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WiiUContentRelationship {
    /// Role is `BaseGame` or `Unknown`; a base/update/DLC relationship does
    /// not apply.
    NotApplicable,
    /// This content's low-order title id matches the candidate base title's
    /// low-order id: a real relationship, not a filename guess.
    Linked { base_low_title_id: String },
    /// The caller supplied a candidate base title id and this content's
    /// title id proves a different game family. Refused, never launched or
    /// paired.
    Mismatched {
        expected_low_title_id: String,
        found_low_title_id: String,
    },
    /// Role is `Update`/`Dlc` but there isn't enough evidence (missing own
    /// title id, or no candidate base title id supplied) to confirm or
    /// refuse a relationship.
    Ambiguous,
}

/// Structural facts gathered from a bounded directory walk. Never itself a
/// verdict - [`project_wiiu_content_readiness`] turns these into one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WiiUDirectoryEvidence {
    pub has_meta_dir: bool,
    pub has_meta_xml: bool,
    pub meta_title_id: Option<String>,
    pub meta_product_code: Option<String>,
    pub meta_region_raw: Option<String>,
    pub title_version: Option<String>,
    pub conflicting_metadata: bool,
    pub has_code_dir: bool,
    pub rpx_entry_count: usize,
    pub has_content_dir: bool,
    pub content_entry_count: usize,
    pub has_tmd: bool,
    pub tmd_title_id: Option<String>,
    pub has_tik: bool,
    pub tik_title_id: Option<String>,
    pub has_cert: bool,
    pub loose_app_count: usize,
    pub loose_rpx_anywhere: usize,
    pub truncated: bool,
}

/// Read-only evidence this projection was built from: either a container
/// file inspected via [`crate::wiiu_disc`], or a bounded directory scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUContentEvidence {
    pub source: PathBuf,
    pub is_directory: bool,
    pub disc: Option<WiiUDiscInspection>,
    pub directory: Option<WiiUDirectoryEvidence>,
}

/// Specific reasons this projection is incomplete, refused, or uncertain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WiiUContentIssue {
    PathUnavailable(String),
    ConflictingMetadata,
    AmbiguousRpx,
    InvalidTitleVersion,
    EmptyInput,
    MissingMetaIdentity,
    ArbitraryRpxLayoutNotValid,
    IncompleteNusLayout,
    TicketSignatureTypeUnsupported(u32),
    TmdSignatureTypeUnsupported(u32),
    TitleIdMismatchAcrossTmdAndTicket { tmd: String, ticket: String },
    WuaInnerContentNotInspected,
    DiscInnerStructureUnavailable,
    DirectoryWalkTruncated,
    NoStructuralIdentityEvidence,
}

/// Overall verdict for whether the currently selected Cemu launch path could
/// conceptually run this content *right now*, with no further conversion or
/// decryption step. Distinct from "is this valid Wii U content" - a
/// perfectly valid representation can still require work first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WiiUContentReadiness {
    DirectlyLaunchable,
    RequiresDecryption,
    RequiresConversion,
    MissingKey,
    IncompleteContent,
    WrongContentRole,
    Ambiguous,
    UnsupportedRepresentation,
}

/// The full read-only projection for one discovered Wii U content path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUContentProjection {
    pub source: PathBuf,
    pub representation: WiiUContentRepresentation,
    pub encryption_state: WiiUEncryptionState,
    pub key_readiness: WiiUKeyReadiness,
    pub role: WiiUContentRole,
    pub identity: Option<WiiUContentIdentity>,
    pub relationship: WiiUContentRelationship,
    pub readiness: WiiUContentReadiness,
    pub issues: Vec<WiiUContentIssue>,
    pub structural_complete: bool,
    pub evidence: WiiUContentEvidence,
    pub provenance: String,
}

fn is_hex16(value: &str) -> bool {
    value.len() == 16 && value.chars().all(|c| c.is_ascii_hexdigit())
}

fn high_id(title_id: &str) -> Option<&str> {
    is_hex16(title_id).then(|| &title_id[..8])
}

/// Returns the low-order 8 hex digits shared across a base game/update/DLC
/// family. Accepts either a full 16-hex title id or a bare 8-hex low id, so
/// a caller can supply whichever it already has - never a filename.
fn low_id(title_id: &str) -> Option<&str> {
    if is_hex16(title_id) {
        Some(&title_id[8..])
    } else if title_id.len() == 8 && title_id.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(title_id)
    } else {
        None
    }
}

fn classify_role_from_title_id(title_id: &str) -> WiiUContentRole {
    match high_id(title_id).map(str::to_ascii_lowercase).as_deref() {
        Some("00050000") => WiiUContentRole::BaseGame,
        Some("0005000e") => WiiUContentRole::Update,
        Some("0005000c") => WiiUContentRole::Dlc,
        _ => WiiUContentRole::Unknown,
    }
}

fn read_bounded(path: &Path, cap: usize) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    let mut buffer = Vec::new();
    file.take(cap as u64).read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

/// Parses a `title_id` at a known fixed offset from a ticket or TMD file,
/// only when the leading signature type is one this module knows the
/// layout for. Never guesses an offset for an unrecognized signature type.
fn parse_fixed_offset_title_id(bytes: &[u8], offset: usize) -> Option<(u32, Option<String>)> {
    if bytes.len() < 4 {
        return None;
    }
    let sig_type = u32::from_be_bytes(bytes[0..4].try_into().ok()?);
    if sig_type != SIG_TYPE_RSA2048_SHA256 {
        return Some((sig_type, None));
    }
    if bytes.len() < offset + 8 {
        return Some((sig_type, None));
    }
    let id = bytes[offset..offset + 8]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<String>();
    Some((sig_type, Some(id)))
}

/// Parse only a complete menu XML document, with unique flat metadata fields.
fn metadata_fields(bytes: &[u8]) -> Option<std::collections::BTreeMap<String, String>> {
    use quick_xml::{Reader, events::Event};
    let text = std::str::from_utf8(bytes).ok()?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<String> = Vec::new();
    let mut root_seen = false;
    let mut fields = std::collections::BTreeMap::new();
    let wanted = ["title_id", "title_version", "product_code", "region"];
    loop {
        match reader.read_event().ok()? {
            Event::Start(start) => {
                let name = std::str::from_utf8(start.name().as_ref()).ok()?.to_string();
                if stack.is_empty() {
                    if root_seen || name != "menu" {
                        return None;
                    }
                    root_seen = true;
                }
                if wanted.contains(&name.as_str()) {
                    if stack.len() != 1 || fields.insert(name.clone(), String::new()).is_some() {
                        return None;
                    }
                }
                if stack.last().is_some_and(|n| wanted.contains(&n.as_str())) {
                    return None;
                }
                stack.push(name);
            }
            Event::Empty(start) => {
                let name = std::str::from_utf8(start.name().as_ref()).ok()?.to_string();
                if stack.is_empty() || stack.last().is_some_and(|n| wanted.contains(&n.as_str())) {
                    return None;
                }
                if wanted.contains(&name.as_str())
                    && (stack.len() != 1 || fields.insert(name, String::new()).is_some())
                {
                    return None;
                }
            }
            Event::Text(text) => {
                if stack.is_empty() {
                    return None;
                }
                if let Some(name) = stack.last().filter(|n| wanted.contains(&n.as_str())) {
                    fields
                        .get_mut(name)?
                        .push_str(&quick_xml::escape::unescape(&text.decode().ok()?).ok()?);
                }
            }
            Event::End(_) => {
                stack.pop()?;
            }
            Event::Eof => return (root_seen && stack.is_empty()).then_some(fields),
            Event::DocType(_) | Event::CData(_) | Event::GeneralRef(_) => return None,
            _ => {}
        }
    }
}
fn parse_meta_xml(bytes: &[u8]) -> (Option<String>, Option<String>, Option<String>) {
    let Some(fields) = metadata_fields(bytes) else {
        return (None, None, None);
    };
    let title_id = fields
        .get("title_id")
        .map(|v| v.trim().to_ascii_uppercase())
        .filter(|v| is_hex16(v));
    (
        title_id,
        fields.get("product_code").cloned(),
        fields.get("region").cloned(),
    )
}

fn scan_directory(root: &Path) -> WiiUDirectoryEvidence {
    let mut evidence = WiiUDirectoryEvidence::default();
    let mut visited = 0usize;
    walk(root, 0, &mut evidence, &mut visited);
    evidence
}

fn walk(dir: &Path, depth: usize, evidence: &mut WiiUDirectoryEvidence, visited: &mut usize) {
    if depth > MAX_DIR_DEPTH {
        evidence.truncated = true;
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        evidence.truncated = true;
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            evidence.truncated = true;
            continue;
        };
        *visited += 1;
        if *visited > MAX_DIR_ENTRIES {
            evidence.truncated = true;
            return;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            evidence.truncated = true;
            continue;
        };
        if meta.file_type().is_symlink() {
            evidence.truncated = true;
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let lower = name.to_ascii_lowercase();
        let relative_depth = depth;
        if meta.is_dir() {
            match lower.as_str() {
                "meta" if relative_depth == 0 => evidence.has_meta_dir = true,
                "code" if relative_depth == 0 => evidence.has_code_dir = true,
                "content" if relative_depth == 0 => evidence.has_content_dir = true,
                _ => {}
            }
            if depth == 0 && matches!(lower.as_str(), "code" | "meta") {
                walk(&path, depth + 1, evidence, visited);
            }
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        if lower.ends_with(".rpx") {
            evidence.loose_rpx_anywhere += 1;
            if relative_depth == 1
                && path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("code"))
            {
                evidence.rpx_entry_count += 1;
            }
        }
        if lower == "meta.xml"
            && relative_depth == 1
            && path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("meta"))
        {
            evidence.conflicting_metadata |= evidence.has_meta_xml;
            evidence.has_meta_xml = true;
            if meta.len() <= MAX_META_XML_BYTES as u64
                && let Some(bytes) = read_bounded(&path, MAX_META_XML_BYTES)
            {
                let (title_id, product_code, region) = parse_meta_xml(&bytes);
                evidence.meta_title_id = title_id;
                evidence.meta_product_code = product_code;
                evidence.meta_region_raw = region;
                let fields = metadata_fields(&bytes);
                evidence.conflicting_metadata |= fields.is_none();
                evidence.title_version = fields.and_then(|f| f.get("title_version").cloned());
            }
        }
        if relative_depth == 1
            && path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("content"))
        {
            evidence.content_entry_count += 1;
        }
        if relative_depth == 0 {
            if lower == "title.tmd" || lower == "tmd" || lower == "00000000.tmd" {
                evidence.conflicting_metadata |= evidence.has_tmd;
                evidence.has_tmd = true;
                if let Some(bytes) = read_bounded(&path, MAX_HEADER_PROBE_BYTES)
                    && let Some((_sig, id)) =
                        parse_fixed_offset_title_id(&bytes, TMD_TITLE_ID_OFFSET)
                {
                    evidence.tmd_title_id = id;
                }
            } else if lower == "title.tik" || lower == "tik" || lower == "00000000.tik" {
                evidence.conflicting_metadata |= evidence.has_tik;
                evidence.has_tik = true;
                if let Some(bytes) = read_bounded(&path, MAX_HEADER_PROBE_BYTES)
                    && let Some((_sig, id)) =
                        parse_fixed_offset_title_id(&bytes, TICKET_TITLE_ID_OFFSET)
                {
                    evidence.tik_title_id = id;
                }
            } else if lower == "title.cert" || lower == "cert" || lower == "00000000.cert" {
                evidence.has_cert = true;
            } else if lower.ends_with(".app") {
                evidence.loose_app_count += 1;
            }
        }
    }
}

fn identity_from_evidence(
    directory: &WiiUDirectoryEvidence,
    directory_name_hint: Option<&str>,
) -> Option<WiiUContentIdentity> {
    if let Some(title_id) = directory
        .meta_title_id
        .clone()
        .or_else(|| directory.tik_title_id.clone())
        .or_else(|| directory.tmd_title_id.clone())
    {
        let source = if directory.meta_title_id.is_some() {
            "meta/meta.xml title_id field"
        } else if directory.tik_title_id.is_some() {
            "title.tik title_id at fixed offset 0x1DC (RSA-2048/SHA-256 signature)"
        } else {
            "title.tmd title_id at fixed offset 0x18C (RSA-2048/SHA-256 signature)"
        };
        return Some(WiiUContentIdentity {
            title_id: Some(title_id),
            product_code: directory.meta_product_code.clone(),
            region_raw: directory.meta_region_raw.clone(),
            confidence: WiiUIdentityConfidence::StructuredField,
            source: source.into(),
        });
    }
    if let Some(name) = directory_name_hint
        && is_hex16(name)
    {
        return Some(WiiUContentIdentity {
            title_id: Some(name.to_ascii_uppercase()),
            product_code: None,
            region_raw: None,
            confidence: WiiUIdentityConfidence::DirectoryNamingConvention,
            source: "directory name matches a 16-hex-digit title id shape only".into(),
        });
    }
    None
}

fn relationship_for(
    role: WiiUContentRole,
    identity: Option<&WiiUContentIdentity>,
    expected_base_title_id: Option<&str>,
) -> WiiUContentRelationship {
    if !matches!(role, WiiUContentRole::Update | WiiUContentRole::Dlc) {
        return WiiUContentRelationship::NotApplicable;
    }
    let Some(own_low) = identity
        .filter(|i| i.confidence == WiiUIdentityConfidence::StructuredField)
        .and_then(|i| i.title_id.as_deref())
        .and_then(low_id)
    else {
        return WiiUContentRelationship::Ambiguous;
    };
    let Some(expected_low) = expected_base_title_id.and_then(low_id) else {
        return WiiUContentRelationship::Ambiguous;
    };
    if own_low.eq_ignore_ascii_case(expected_low) {
        WiiUContentRelationship::Linked {
            base_low_title_id: own_low.to_ascii_uppercase(),
        }
    } else {
        WiiUContentRelationship::Mismatched {
            expected_low_title_id: expected_low.to_ascii_uppercase(),
            found_low_title_id: own_low.to_ascii_uppercase(),
        }
    }
}

fn project_directory(
    source: PathBuf,
    directory: WiiUDirectoryEvidence,
    key_store: WiiUKeyStoreEvidence,
    expected_base_title_id: Option<&str>,
) -> WiiUContentProjection {
    let mut issues = Vec::new();
    if directory.truncated {
        issues.push(WiiUContentIssue::DirectoryWalkTruncated);
    }
    if directory
        .title_version
        .as_ref()
        .is_some_and(|v| v.trim().parse::<u16>().is_err())
    {
        issues.push(WiiUContentIssue::InvalidTitleVersion);
    }

    let name_hint = source
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned);

    let declared_role = directory
        .meta_title_id
        .as_deref()
        .map(classify_role_from_title_id);
    let is_addon = matches!(
        declared_role,
        Some(WiiUContentRole::Update | WiiUContentRole::Dlc)
    ) && directory.has_content_dir
        && directory.has_meta_xml;
    let is_loadiine = directory.has_meta_xml
        && directory.meta_title_id.is_some()
        && directory.has_code_dir
        && directory.rpx_entry_count > 0
        && directory.has_content_dir;
    let is_rpx_extracted = !is_loadiine
        && directory.has_code_dir
        && directory.rpx_entry_count > 0
        && directory.has_content_dir;
    let is_nus_complete = directory.has_tmd
        && directory.has_tik
        && directory.has_cert
        && directory.loose_app_count > 0;
    let is_nus_partial = (directory.has_tmd || directory.has_tik || directory.has_cert)
        && !is_nus_complete
        && !is_loadiine
        && !is_rpx_extracted;

    let (representation, structural_complete) = if is_loadiine || is_addon {
        (WiiUContentRepresentation::LoadiineExtracted, true)
    } else if is_rpx_extracted {
        issues.push(WiiUContentIssue::MissingMetaIdentity);
        (WiiUContentRepresentation::RpxExtractedGame, false)
    } else if is_nus_complete {
        (WiiUContentRepresentation::Nus, true)
    } else if is_nus_partial {
        issues.push(WiiUContentIssue::IncompleteNusLayout);
        (WiiUContentRepresentation::Nus, false)
    } else if directory.loose_rpx_anywhere > 0 {
        issues.push(WiiUContentIssue::ArbitraryRpxLayoutNotValid);
        (WiiUContentRepresentation::Unknown, false)
    } else {
        (WiiUContentRepresentation::Unknown, false)
    };

    if let Some(tik_id) = &directory.tik_title_id
        && let Some(tmd_id) = &directory.tmd_title_id
        && !tik_id.eq_ignore_ascii_case(tmd_id)
    {
        issues.push(WiiUContentIssue::TitleIdMismatchAcrossTmdAndTicket {
            tmd: tmd_id.clone(),
            ticket: tik_id.clone(),
        });
    }

    let mut identity = identity_from_evidence(&directory, name_hint.as_deref());
    let ids: Vec<_> = [
        &directory.meta_title_id,
        &directory.tmd_title_id,
        &directory.tik_title_id,
    ]
    .into_iter()
    .flatten()
    .collect();
    if directory.conflicting_metadata || ids.windows(2).any(|ids| ids[0] != ids[1]) {
        issues.push(WiiUContentIssue::ConflictingMetadata);
        identity = None;
    }
    if directory.rpx_entry_count > 1 {
        issues.push(WiiUContentIssue::AmbiguousRpx);
    }
    if identity.is_none() {
        issues.push(WiiUContentIssue::NoStructuralIdentityEvidence);
    }

    let role = identity
        .as_ref()
        .and_then(|i| i.title_id.as_deref())
        .map(classify_role_from_title_id)
        .unwrap_or(WiiUContentRole::Unknown);

    let encryption_state = match representation {
        WiiUContentRepresentation::LoadiineExtracted
        | WiiUContentRepresentation::RpxExtractedGame => WiiUEncryptionState::Decrypted,
        WiiUContentRepresentation::Nus => WiiUEncryptionState::Encrypted,
        _ => WiiUEncryptionState::Unknown,
    };

    let key_readiness = match encryption_state {
        WiiUEncryptionState::Decrypted => WiiUKeyReadiness::NotApplicable,
        WiiUEncryptionState::Encrypted => match key_store {
            WiiUKeyStoreEvidence::ConfirmedAvailable => WiiUKeyReadiness::AvailableLocally,
            WiiUKeyStoreEvidence::ConfirmedMissing => WiiUKeyReadiness::Missing,
            WiiUKeyStoreEvidence::NotChecked => WiiUKeyReadiness::Unknown,
        },
        WiiUEncryptionState::Unknown => WiiUKeyReadiness::Unknown,
    };

    let relationship = relationship_for(role, identity.as_ref(), expected_base_title_id);
    if matches!(relationship, WiiUContentRelationship::Mismatched { .. }) {
        // Structural fact only; readiness derivation below treats a
        // mismatched relationship as a refusal via WrongContentRole so a
        // caller never wires the two together.
    }

    let mut readiness = derive_readiness(
        representation,
        structural_complete,
        role,
        encryption_state,
        key_readiness,
        &relationship,
        false,
    );
    if directory.truncated
        || directory.rpx_entry_count > 1
        || identity
            .as_ref()
            .is_some_and(|i| i.confidence != WiiUIdentityConfidence::StructuredField)
        || issues.contains(&WiiUContentIssue::ConflictingMetadata)
        || issues.contains(&WiiUContentIssue::InvalidTitleVersion)
    {
        readiness = WiiUContentReadiness::Ambiguous;
    }

    let evidence = WiiUContentEvidence {
        source: source.clone(),
        is_directory: true,
        disc: None,
        directory: Some(directory),
    };

    WiiUContentProjection {
        source,
        representation,
        encryption_state,
        key_readiness,
        role,
        identity,
        relationship,
        readiness,
        issues,
        structural_complete,
        evidence,
        provenance: "bounded directory structural scan; no decryption, no key access, no writes"
            .into(),
    }
}

fn representation_from_disc(format: WiiUDiscFormat) -> WiiUContentRepresentation {
    match format {
        WiiUDiscFormat::Wud => WiiUContentRepresentation::Wud,
        WiiUDiscFormat::Wux => WiiUContentRepresentation::Wux,
        WiiUDiscFormat::Wua => WiiUContentRepresentation::Wua,
        WiiUDiscFormat::Unknown => WiiUContentRepresentation::Unknown,
    }
}

fn project_file(
    source: PathBuf,
    key_store: WiiUKeyStoreEvidence,
    expected_base_title_id: Option<&str>,
) -> WiiUContentProjection {
    let disc = inspect_wii_u_disc(&source);
    let representation = representation_from_disc(disc.format);
    let mut issues = Vec::new();

    let structural_complete = disc.structural_complete;
    let encryption_state = match representation {
        WiiUContentRepresentation::Wud | WiiUContentRepresentation::Wux => {
            WiiUEncryptionState::Encrypted
        }
        WiiUContentRepresentation::Wua => {
            issues.push(WiiUContentIssue::WuaInnerContentNotInspected);
            WiiUEncryptionState::Unknown
        }
        _ => WiiUEncryptionState::Unknown,
    };
    if matches!(
        representation,
        WiiUContentRepresentation::Wud | WiiUContentRepresentation::Wux
    ) {
        issues.push(WiiUContentIssue::DiscInnerStructureUnavailable);
    }

    let key_readiness = match encryption_state {
        WiiUEncryptionState::Encrypted => match key_store {
            WiiUKeyStoreEvidence::ConfirmedAvailable => WiiUKeyReadiness::AvailableLocally,
            WiiUKeyStoreEvidence::ConfirmedMissing => WiiUKeyReadiness::Missing,
            WiiUKeyStoreEvidence::NotChecked => WiiUKeyReadiness::Unknown,
        },
        _ => WiiUKeyReadiness::Unknown,
    };

    // WUD/WUX inner filesystem is encrypted; without decrypting it (which
    // this module never does), no title id can be read, so role/identity
    // stay Unknown/None here regardless of what the caller passes in.
    let role = WiiUContentRole::Unknown;
    let relationship = relationship_for(role, None, expected_base_title_id);

    let readiness = derive_readiness(
        representation,
        structural_complete,
        role,
        encryption_state,
        key_readiness,
        &relationship,
        matches!(representation, WiiUContentRepresentation::Wua),
    );

    let evidence = WiiUContentEvidence {
        source: source.clone(),
        is_directory: false,
        disc: Some(disc),
        directory: None,
    };

    WiiUContentProjection {
        source,
        representation,
        encryption_state,
        key_readiness,
        role,
        identity: None,
        relationship,
        readiness,
        issues,
        structural_complete,
        evidence,
        provenance: "reuses crate::wiiu_disc bounded container inspection; no decryption".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn derive_readiness(
    representation: WiiUContentRepresentation,
    structural_complete: bool,
    role: WiiUContentRole,
    encryption_state: WiiUEncryptionState,
    key_readiness: WiiUKeyReadiness,
    relationship: &WiiUContentRelationship,
    limited_inspection: bool,
) -> WiiUContentReadiness {
    if representation == WiiUContentRepresentation::Unknown {
        return WiiUContentReadiness::UnsupportedRepresentation;
    }
    if limited_inspection {
        // e.g. WUA: a real, recognized representation, but this repo does
        // not parse its inner content, so completeness cannot be judged
        // either way - never claim IncompleteContent (a positive finding of
        // missing structure) or DirectlyLaunchable (verified launchability)
        // for something that was never actually inspected.
        return WiiUContentReadiness::Ambiguous;
    }
    if !structural_complete {
        return WiiUContentReadiness::IncompleteContent;
    }
    if matches!(relationship, WiiUContentRelationship::Mismatched { .. }) {
        return WiiUContentReadiness::WrongContentRole;
    }
    if matches!(role, WiiUContentRole::Update | WiiUContentRole::Dlc) {
        return WiiUContentReadiness::WrongContentRole;
    }
    if representation == WiiUContentRepresentation::Nus {
        // This repo's conversion planner only understands WUD<->WUX; a
        // structurally complete NUS layout still needs to become one of
        // those (or an extracted directory) before existing launch/
        // conversion planners can consume it.
        return WiiUContentReadiness::RequiresConversion;
    }
    match encryption_state {
        WiiUEncryptionState::Encrypted => match key_readiness {
            WiiUKeyReadiness::Missing => WiiUContentReadiness::MissingKey,
            WiiUKeyReadiness::AvailableLocally => WiiUContentReadiness::RequiresDecryption,
            WiiUKeyReadiness::Unknown | WiiUKeyReadiness::NotApplicable => {
                WiiUContentReadiness::Ambiguous
            }
        },
        WiiUEncryptionState::Decrypted => match role {
            WiiUContentRole::BaseGame => WiiUContentReadiness::DirectlyLaunchable,
            WiiUContentRole::Unknown => WiiUContentReadiness::Ambiguous,
            WiiUContentRole::Update | WiiUContentRole::Dlc => {
                WiiUContentReadiness::WrongContentRole
            }
        },
        WiiUEncryptionState::Unknown => WiiUContentReadiness::Ambiguous,
    }
}

/// Projects a full [`WiiUContentProjection`] for `path`, using
/// `key_store` as caller-supplied evidence about key availability and
/// `expected_base_title_id` (full 16-hex title id, or just its low 8 hex
/// digits) as an optional candidate base title to check an Update/DLC
/// relationship against.
///
/// Read-only: performs no writes, no downloads, no key fetches, no
/// decryption, and no conversion. Bounded: directory walks and header reads
/// are capped.
pub fn project_wiiu_content_readiness(
    path: &Path,
    key_store: WiiUKeyStoreEvidence,
    expected_base_title_id: Option<&str>,
) -> WiiUContentProjection {
    let source = path.to_path_buf();
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return unavailable(source, format!("{e}"));
        }
    };
    if metadata.file_type().is_symlink() {
        return unavailable(source, "refusing to follow a symlink source".into());
    }
    if metadata.is_dir() {
        let directory = scan_directory(path);
        project_directory(source, directory, key_store, expected_base_title_id)
    } else if metadata.is_file() {
        if metadata.len() == 0 {
            return empty_file(source);
        }
        project_file(source, key_store, expected_base_title_id)
    } else {
        unavailable(source, "not a regular file or directory".into())
    }
}

fn unavailable(source: PathBuf, detail: String) -> WiiUContentProjection {
    WiiUContentProjection {
        source: source.clone(),
        representation: WiiUContentRepresentation::Unknown,
        encryption_state: WiiUEncryptionState::Unknown,
        key_readiness: WiiUKeyReadiness::Unknown,
        role: WiiUContentRole::Unknown,
        identity: None,
        relationship: WiiUContentRelationship::NotApplicable,
        readiness: WiiUContentReadiness::UnsupportedRepresentation,
        issues: vec![WiiUContentIssue::PathUnavailable(detail)],
        structural_complete: false,
        evidence: WiiUContentEvidence {
            source,
            is_directory: false,
            disc: None,
            directory: None,
        },
        provenance: "path unavailable for inspection".into(),
    }
}

fn empty_file(source: PathBuf) -> WiiUContentProjection {
    WiiUContentProjection {
        source: source.clone(),
        representation: WiiUContentRepresentation::Unknown,
        encryption_state: WiiUEncryptionState::Unknown,
        key_readiness: WiiUKeyReadiness::Unknown,
        role: WiiUContentRole::Unknown,
        identity: None,
        relationship: WiiUContentRelationship::NotApplicable,
        readiness: WiiUContentReadiness::IncompleteContent,
        issues: vec![WiiUContentIssue::EmptyInput],
        structural_complete: false,
        evidence: WiiUContentEvidence {
            source,
            is_directory: false,
            disc: None,
            directory: None,
        },
        provenance: "bounded Wii U content readiness inspection".into(),
    }
}

/// File-identity stamp (relative path, device/inode, size, mtime) of the inspected Wii U
/// content, captured when readiness evidence was gathered. Read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUContentStamp {
    root: PathBuf,
    entries: Option<Vec<(PathBuf, crate::launch::process_spawn::CapturedFileIdentity)>>,
}

const STAMP_MAX_ENTRIES: usize = 8192;

impl WiiUContentStamp {
    pub fn capture(path: &Path) -> Self {
        Self {
            root: path.to_path_buf(),
            entries: stamp_entries(path),
        }
    }

    /// True when the content changed, vanished, or could not be re-read since
    /// capture. Old readiness must then not be treated as current.
    pub fn is_stale(&self) -> bool {
        match (&self.entries, stamp_entries(&self.root)) {
            (Some(before), Some(now)) => *before != now,
            _ => true,
        }
    }
}

fn stamp_entries(
    root: &Path,
) -> Option<Vec<(PathBuf, crate::launch::process_spawn::CapturedFileIdentity)>> {
    let metadata = fs::symlink_metadata(root).ok()?;
    let mut out = Vec::new();
    if metadata.is_file() {
        out.push((
            PathBuf::new(),
            crate::launch::process_spawn::CapturedFileIdentity::capture(&metadata),
        ));
        return Some(out);
    }
    if !metadata.is_dir() {
        return None;
    }
    out.push((
        PathBuf::new(),
        crate::launch::process_spawn::CapturedFileIdentity::capture(&metadata),
    ));
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 32 {
            return None;
        }
        for entry in fs::read_dir(&dir).ok()? {
            let path = entry.ok()?.path();
            let meta = fs::symlink_metadata(&path).ok()?;
            if meta.file_type().is_symlink() {
                return None;
            }
            let relative = path.strip_prefix(root).ok()?.to_path_buf();
            out.push((
                relative,
                crate::launch::process_spawn::CapturedFileIdentity::capture(&meta),
            ));
            if meta.is_dir() {
                stack.push((path, depth + 1));
            }
            if out.len() > STAMP_MAX_ENTRIES {
                return None;
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    fn ticket_bytes(title_id_hex: &str) -> Vec<u8> {
        let mut bytes = vec![0_u8; TICKET_TITLE_ID_OFFSET + 8];
        bytes[0..4].copy_from_slice(&SIG_TYPE_RSA2048_SHA256.to_be_bytes());
        let id = hex_to_bytes(title_id_hex);
        bytes[TICKET_TITLE_ID_OFFSET..TICKET_TITLE_ID_OFFSET + 8].copy_from_slice(&id);
        bytes
    }

    fn tmd_bytes(title_id_hex: &str) -> Vec<u8> {
        let mut bytes = vec![0_u8; TMD_TITLE_ID_OFFSET + 8];
        bytes[0..4].copy_from_slice(&SIG_TYPE_RSA2048_SHA256.to_be_bytes());
        let id = hex_to_bytes(title_id_hex);
        bytes[TMD_TITLE_ID_OFFSET..TMD_TITLE_ID_OFFSET + 8].copy_from_slice(&id);
        bytes
    }

    fn hex_to_bytes(hex: &str) -> [u8; 8] {
        let mut out = [0_u8; 8];
        for i in 0..8 {
            out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    fn make_loadiine_base_game(root: &Path, title_id: &str) {
        write(
            &root.join("meta/meta.xml"),
            format!(
                "<menu>\n  <title_id type=\"hexBinary\" length=\"8\">{title_id}</title_id>\n  <product_code type=\"string\" length=\"32\">WUP-P-TEST</product_code>\n  <region type=\"hexBinary\" length=\"4\">00000002</region>\n</menu>\n"
            )
            .as_bytes(),
        );
        write(&root.join("code/game.rpx"), b"synthetic-rpx");
        write(&root.join("content/data.pack"), b"synthetic-content");
    }

    #[test]
    fn valid_extracted_base_game_is_directly_launchable() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("BaseGame");
        make_loadiine_base_game(&root, "0005000010101A00");
        let before = fs::read(root.join("code/game.rpx")).unwrap();

        let projection =
            project_wiiu_content_readiness(&root, WiiUKeyStoreEvidence::NotChecked, None);

        assert_eq!(
            projection.representation,
            WiiUContentRepresentation::LoadiineExtracted
        );
        assert_eq!(projection.role, WiiUContentRole::BaseGame);
        assert_eq!(projection.encryption_state, WiiUEncryptionState::Decrypted);
        assert_eq!(
            projection.readiness,
            WiiUContentReadiness::DirectlyLaunchable
        );
        assert!(projection.structural_complete);
        assert_eq!(fs::read(root.join("code/game.rpx")).unwrap(), before);
    }

    #[test]
    fn wua_is_recognized_with_limited_inspection() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("game.wua");
        write(&path, &[1_u8; 64]);

        let projection =
            project_wiiu_content_readiness(&path, WiiUKeyStoreEvidence::NotChecked, None);

        assert_eq!(projection.representation, WiiUContentRepresentation::Wua);
        assert_eq!(projection.readiness, WiiUContentReadiness::Ambiguous);
        assert!(
            projection
                .issues
                .contains(&WiiUContentIssue::WuaInnerContentNotInspected)
        );
    }

    #[test]
    fn wud_is_recognized() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("game.wud");
        // Current main requires structural magic rather than the .wud name.
        let mut header = vec![0_u8; crate::wiiu_disc::WUD_HEADER_SIZE as usize];
        header[0x10000..0x10004].copy_from_slice(&0xcc54_9eb9_u32.to_be_bytes());
        write(&path, &header);

        let projection =
            project_wiiu_content_readiness(&path, WiiUKeyStoreEvidence::NotChecked, None);

        assert_eq!(projection.representation, WiiUContentRepresentation::Wud);
        assert_eq!(projection.encryption_state, WiiUEncryptionState::Encrypted);
    }

    #[test]
    fn wux_is_recognized() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("game.wux");
        let mut header = [0_u8; 32];
        header[..4].copy_from_slice(b"WUX0");
        header[4..8].copy_from_slice(&0x1099_d02e_u32.to_le_bytes());
        header[8..12].copy_from_slice(&0x100_u32.to_le_bytes());
        header[16..24].copy_from_slice(&0x100_u64.to_le_bytes());
        let mut bytes = header.to_vec();
        bytes.extend_from_slice(&[0_u8; 252]);
        bytes.extend_from_slice(&[0_u8; 256]);
        write(&path, &bytes);

        let projection =
            project_wiiu_content_readiness(&path, WiiUKeyStoreEvidence::NotChecked, None);

        assert_eq!(projection.representation, WiiUContentRepresentation::Wux);
        assert_eq!(projection.encryption_state, WiiUEncryptionState::Encrypted);
    }

    #[test]
    fn encrypted_content_with_missing_key_is_missing_key() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("NusTitle");
        write(&root.join("title.tmd"), &tmd_bytes("0005000010101A00"));
        write(&root.join("title.tik"), &ticket_bytes("0005000010101A00"));
        write(&root.join("title.cert"), b"synthetic-cert");
        write(&root.join("00000000.app"), b"synthetic-encrypted-content");

        let projection =
            project_wiiu_content_readiness(&root, WiiUKeyStoreEvidence::ConfirmedMissing, None);

        assert_eq!(projection.representation, WiiUContentRepresentation::Nus);
        assert!(projection.structural_complete);
        assert_eq!(projection.encryption_state, WiiUEncryptionState::Encrypted);
        assert_eq!(projection.key_readiness, WiiUKeyReadiness::Missing);
        // NUS always requires conversion in this repo's current pipeline,
        // regardless of key state, so it never silently becomes launchable
        // just because a key exists.
        assert_eq!(
            projection.readiness,
            WiiUContentReadiness::RequiresConversion
        );
    }

    #[test]
    fn encrypted_content_with_key_evidence_is_never_marked_decrypted() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("NusTitleWithKey");
        write(&root.join("title.tmd"), &tmd_bytes("0005000010101A00"));
        write(&root.join("title.tik"), &ticket_bytes("0005000010101A00"));
        write(&root.join("title.cert"), b"synthetic-cert");
        write(&root.join("00000000.app"), b"synthetic-encrypted-content");

        let projection =
            project_wiiu_content_readiness(&root, WiiUKeyStoreEvidence::ConfirmedAvailable, None);

        assert_eq!(projection.encryption_state, WiiUEncryptionState::Encrypted);
        assert_eq!(projection.key_readiness, WiiUKeyReadiness::AvailableLocally);
        assert_ne!(
            projection.readiness,
            WiiUContentReadiness::DirectlyLaunchable
        );
    }

    #[test]
    fn incomplete_nus_layout_is_incomplete_content() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("PartialNus");
        write(&root.join("title.tmd"), &tmd_bytes("0005000010101A00"));
        // No ticket, no cert, no content app files.

        let projection =
            project_wiiu_content_readiness(&root, WiiUKeyStoreEvidence::NotChecked, None);

        assert_eq!(projection.representation, WiiUContentRepresentation::Nus);
        assert!(!projection.structural_complete);
        assert_eq!(
            projection.readiness,
            WiiUContentReadiness::IncompleteContent
        );
        assert!(
            projection
                .issues
                .contains(&WiiUContentIssue::IncompleteNusLayout)
        );
    }

    #[test]
    fn update_matching_base_title_is_linked() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("UpdateContent");
        write(
            &root.join("meta/meta.xml"),
            b"<menu><title_id type=\"hexBinary\" length=\"8\">0005000E10101A00</title_id></menu>",
        );
        // Update-role meta alone (no code/content) keeps representation
        // Unknown-safe but identity is still readable for the relationship
        // check; use a full extracted-style layout to exercise identity.
        write(&root.join("code/patch.rpx"), b"synthetic");
        write(&root.join("content/patch.pack"), b"synthetic");

        let projection = project_wiiu_content_readiness(
            &root,
            WiiUKeyStoreEvidence::NotChecked,
            Some("0005000010101A00"),
        );

        assert_eq!(projection.role, WiiUContentRole::Update);
        assert_eq!(
            projection.relationship,
            WiiUContentRelationship::Linked {
                base_low_title_id: "10101A00".into()
            }
        );
        assert_eq!(projection.readiness, WiiUContentReadiness::WrongContentRole);
    }

    #[test]
    fn update_mismatching_base_title_is_refused() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("UpdateMismatch");
        write(
            &root.join("meta/meta.xml"),
            b"<menu><title_id type=\"hexBinary\" length=\"8\">0005000E10101A00</title_id></menu>",
        );
        write(&root.join("code/patch.rpx"), b"synthetic");
        write(&root.join("content/patch.pack"), b"synthetic");

        let projection = project_wiiu_content_readiness(
            &root,
            WiiUKeyStoreEvidence::NotChecked,
            Some("00050000FFFFFFFF"),
        );

        assert!(matches!(
            projection.relationship,
            WiiUContentRelationship::Mismatched { .. }
        ));
        assert_eq!(projection.readiness, WiiUContentReadiness::WrongContentRole);
    }

    #[test]
    fn dlc_matching_base_title_is_linked() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("DlcContent");
        write(
            &root.join("meta/meta.xml"),
            b"<menu><title_id type=\"hexBinary\" length=\"8\">0005000C10101A00</title_id></menu>",
        );
        write(&root.join("code/dlc.rpx"), b"synthetic");
        write(&root.join("content/dlc.pack"), b"synthetic");

        let projection = project_wiiu_content_readiness(
            &root,
            WiiUKeyStoreEvidence::NotChecked,
            Some("10101A00"),
        );

        assert_eq!(projection.role, WiiUContentRole::Dlc);
        assert_eq!(
            projection.relationship,
            WiiUContentRelationship::Linked {
                base_low_title_id: "10101A00".into()
            }
        );
    }

    #[test]
    fn filename_only_wux_with_invalid_structure_is_not_ready() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("game.wux");
        write(&path, b"not a real wux file at all");

        let projection =
            project_wiiu_content_readiness(&path, WiiUKeyStoreEvidence::NotChecked, None);

        assert_eq!(
            projection.representation,
            WiiUContentRepresentation::Unknown
        );
        assert!(!projection.structural_complete);
        assert_ne!(
            projection.readiness,
            WiiUContentReadiness::DirectlyLaunchable
        );
        assert_eq!(
            projection.readiness,
            WiiUContentReadiness::UnsupportedRepresentation
        );
    }

    #[test]
    fn arbitrary_rpx_folder_is_not_automatically_valid() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("RandomFolder");
        write(&root.join("subdir/whatever.rpx"), b"synthetic");
        write(&root.join("readme.txt"), b"not a real dump");

        let projection =
            project_wiiu_content_readiness(&root, WiiUKeyStoreEvidence::NotChecked, None);

        assert_eq!(
            projection.representation,
            WiiUContentRepresentation::Unknown
        );
        assert_ne!(
            projection.readiness,
            WiiUContentReadiness::DirectlyLaunchable
        );
        assert!(
            projection
                .issues
                .contains(&WiiUContentIssue::NoStructuralIdentityEvidence)
        );
    }

    #[test]
    fn unknown_representation_is_unsupported() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("EmptyDir");
        fs::create_dir_all(&root).unwrap();

        let projection =
            project_wiiu_content_readiness(&root, WiiUKeyStoreEvidence::NotChecked, None);

        assert_eq!(
            projection.representation,
            WiiUContentRepresentation::Unknown
        );
        assert_eq!(
            projection.readiness,
            WiiUContentReadiness::UnsupportedRepresentation
        );
    }

    #[test]
    fn projection_performs_no_writes_or_downloads() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("Immutable");
        make_loadiine_base_game(&root, "0005000010101A00");

        let mut before = Vec::new();
        for entry in walkdir_files(&root) {
            before.push((entry.clone(), fs::read(&entry).unwrap()));
        }

        let _ =
            project_wiiu_content_readiness(&root, WiiUKeyStoreEvidence::ConfirmedAvailable, None);
        let _ = project_wiiu_content_readiness(
            &root,
            WiiUKeyStoreEvidence::ConfirmedMissing,
            Some("0005000010101A00"),
        );

        for (path, bytes) in before {
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        // No stray files created anywhere under the temp root.
        let after: Vec<_> = walkdir_files(dir.path());
        assert_eq!(after.len(), walkdir_files(&root).len());
    }

    fn walkdir_files(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out
    }
}
