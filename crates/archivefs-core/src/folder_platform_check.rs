//! Smart ROM Folder Check (read only): finds games whose bytes belong to a
//! different system than the one they are filed under.
//!
//! This module only reads. It never writes the library database, the DAT
//! source configuration or any game file, and it never changes an
//! assignment. Its output is held in memory by the caller.
//!
//! Evidence tiers (a file name or extension alone is never evidence):
//! - Confirmed: the member's SHA-1 is in a catalogue whose derived system
//!   differs from the assigned one, and no content evidence contradicts it.
//! - Probable: content structure (a validated header, or a structural
//!   signature) points at a different system and nothing contradicts it, but
//!   no catalogue hash matched.
//! - Ambiguous: signals conflict, or contradict a catalogue hit.
//!
//! The system a catalogue describes is derived read-only from the DAT's own
//! header name through the reviewed recognition table; configuration files
//! are never written.

use crate::dat::catalogue_recognition::{CatalogueRecognition, recognise_catalogue, same_platform};
use crate::dat::index::DatIndex;
use crate::dat::model::DatEcosystem;
use crate::game_identity::{MAX_ARCHIVE_MEMBERS, MAX_PATH_BYTES};
use crate::safe_read::{TrustedRoots, open_bounded_read};
use sha1::{Digest, Sha1};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
mod tests;

/// Largest file (or ZIP member) this check will read.
pub const MAX_CHECK_BYTES: u64 = 16 * 1024 * 1024;
/// Largest decompression ratio accepted for a ZIP member.
pub const MAX_CHECK_ZIP_RATIO: u64 = 2_048;
/// Games examined per run; the person continues explicitly.
pub const DEFAULT_RUN_CAP: usize = 1_000;

pub const DERIVED_PROVENANCE: &str = "derived from DAT header name";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Confirmed,
    Probable,
    Ambiguous,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "Confirmed",
            Self::Probable => "Probable",
            Self::Ambiguous => "Ambiguous",
        }
    }
}

// ---------------------------------------------------------------------------
// Catalogue platform derivation (read only)
// ---------------------------------------------------------------------------

/// The system a catalogue describes, derived from its own header name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedPlatform {
    pub platform: &'static str,
    pub provenance: &'static str,
}

/// Derives the system of an ordinary game list from its header name and
/// ecosystem. Refuses (None) when the name is missing, ambiguous, unmapped,
/// several-system or not an ordinary game list. Writes nothing.
pub fn derive_catalogue_platform(
    header_name: Option<&str>,
    ecosystem: Option<DatEcosystem>,
) -> Option<DerivedPlatform> {
    match recognise_catalogue(header_name, ecosystem) {
        CatalogueRecognition::Games { platform } => Some(DerivedPlatform {
            platform,
            provenance: DERIVED_PROVENANCE,
        }),
        _ => None,
    }
}

/// Canonical systems that are single cartridge images, the only ones whose
/// bytes this check reads.
pub fn cartridge_platforms() -> Vec<&'static str> {
    [
        "nes",
        "snes",
        "megadrive",
        "gb",
        "gbc",
        "gba",
        "n64",
        "32x",
        "mastersystem",
        "gamegear",
        "atari2600",
        "atari7800",
        "lynx",
        "ngp",
        "ngpc",
    ]
    .iter()
    .filter_map(|alias| crate::canonical_platform_for_alias(alias))
    .collect()
}

pub struct Catalogue {
    pub platform: &'static str,
    pub dat_name: String,
    pub dat_version: Option<String>,
    pub source_id: String,
    pub provenance: &'static str,
    pub index: DatIndex,
}

/// Builds a lookup catalogue from a parsed DAT when its derived system is one
/// of `wanted`; refuses otherwise.
pub fn catalogue_from_parsed(
    dat: &crate::dat::model::ParsedDat,
    source_id: &str,
    fallback_name: &str,
    wanted: &[&'static str],
) -> Option<Catalogue> {
    let derived =
        derive_catalogue_platform(dat.source.name.as_deref(), Some(dat.source.ecosystem))?;
    if !wanted.iter().any(|p| same_platform(p, derived.platform)) {
        return None;
    }
    Some(Catalogue {
        platform: derived.platform,
        dat_name: dat
            .source
            .name
            .clone()
            .unwrap_or_else(|| fallback_name.to_string()),
        dat_version: dat.source.version.clone(),
        source_id: source_id.to_string(),
        provenance: derived.provenance,
        index: DatIndex::build(dat),
    })
}

/// Parses the configured catalogues whose derived system is a cartridge
/// system. Reads DAT files only; skips (never guesses) the rest. The flag is
/// false when cancelled part-way: such a partial list must not be cached.
pub fn load_cartridge_catalogues(
    configured: &std::collections::HashMap<
        String,
        crate::dat::configured_catalogues::ConfiguredCatalogue,
    >,
    cancel: &AtomicBool,
) -> (Vec<Catalogue>, bool) {
    let wanted = cartridge_platforms();
    let mut ids: Vec<&String> = configured.keys().collect();
    ids.sort();
    let mut out = Vec::new();
    for id in ids {
        if cancel.load(Ordering::Relaxed) {
            return (out, false);
        }
        let row = &configured[id];
        if row.kind != crate::dat::sources::DatSourceKind::File {
            continue;
        }
        let Ok(outcome) = crate::dat::parsers::parse_dat_file(
            &row.path,
            crate::dat::limits::DatLimits::default(),
        ) else {
            continue;
        };
        if let Some(catalogue) =
            catalogue_from_parsed(&outcome.dat, &row.source_id, &row.display_name, &wanted)
        {
            out.push(catalogue);
        }
    }
    (out, true)
}

// ---------------------------------------------------------------------------
// Bounded reading
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadRefusal {
    Unreadable(String),
    TooLarge(u64),
    NotSingleMember(&'static str),
    Unsafe(&'static str),
    Corrupt(String),
}

impl ReadRefusal {
    pub fn describe(&self) -> String {
        match self {
            Self::Unreadable(e) => format!("could not be read: {e}"),
            Self::TooLarge(n) => format!("{n} bytes is larger than this check reads"),
            Self::NotSingleMember(w) | Self::Unsafe(w) => (*w).to_string(),
            Self::Corrupt(e) => format!("is corrupt or inconsistent: {e}"),
        }
    }
}

/// Reads one file for the check: a loose file, or a ZIP holding exactly one
/// file member (whatever its extension). Bounded by [`MAX_CHECK_BYTES`].
pub fn read_check_bytes(path: &Path, trusted: &TrustedRoots) -> Result<Vec<u8>, ReadRefusal> {
    read_check_file(path, trusted).map(|(_, bytes)| bytes)
}

/// Like [`read_check_bytes`], also returning the ZIP member's name (None for a
/// loose file). The name is only used for explanatory wording.
pub fn read_check_file(
    path: &Path,
    trusted: &TrustedRoots,
) -> Result<(Option<String>, Vec<u8>), ReadRefusal> {
    let safe = open_bounded_read(path, trusted)
        .map_err(|refusal| ReadRefusal::Unreadable(refusal.detail()))?;
    let is_zip = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    let length = safe.len();
    let file = safe.into_file();
    if is_zip {
        return read_single_zip_member_named(file, length).map(|(n, b)| (Some(n), b));
    }
    if length > MAX_CHECK_BYTES {
        return Err(ReadRefusal::TooLarge(length));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_CHECK_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ReadRefusal::Unreadable(e.to_string()))?;
    if bytes.len() as u64 != length {
        return Err(ReadRefusal::Corrupt(
            "the file changed while it was read".into(),
        ));
    }
    Ok((None, bytes))
}

/// Small bounded helper with the same safety rules as
/// `zip_cartridge_select`, but without requiring a supported extension:
/// member-count cap, name sanitation, no encryption, no symlink, declared
/// size and ratio caps, and bytes actually read are counted.
pub fn read_single_zip_member(file: std::fs::File, outer_len: u64) -> Result<Vec<u8>, ReadRefusal> {
    read_single_zip_member_named(file, outer_len).map(|(_, bytes)| bytes)
}

/// [`read_single_zip_member`] plus the member's name.
pub fn read_single_zip_member_named(
    file: std::fs::File,
    _outer_len: u64,
) -> Result<(String, Vec<u8>), ReadRefusal> {
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| ReadRefusal::Corrupt(e.to_string()))?;
    if archive.len() > MAX_ARCHIVE_MEMBERS {
        return Err(ReadRefusal::Unsafe("has too many ZIP members"));
    }
    if archive.validate_header_consistency().is_err() {
        return Err(ReadRefusal::Unsafe(
            "has duplicated, overlapping or inconsistent ZIP entries",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut chosen: Option<(usize, Vec<u8>, u64)> = None;
    for index in 0..archive.len() {
        let raw = archive
            .by_index_raw(index)
            .map_err(|e| ReadRefusal::Corrupt(e.to_string()))?;
        if raw.encrypted() {
            return Err(ReadRefusal::Unsafe("has an encrypted ZIP entry"));
        }
        let name = raw.name_raw().to_vec();
        let name_ok = !name.is_empty()
            && name.len() <= MAX_PATH_BYTES
            && !name.contains(&0)
            && !name.contains(&b'\\')
            && !name.starts_with(b"/")
            && !name.contains(&b':')
            && std::str::from_utf8(&name).is_ok()
            && raw.enclosed_name().is_some();
        if !name_ok {
            return Err(ReadRefusal::Unsafe("has an unsafe ZIP member name"));
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(ReadRefusal::Unsafe("has duplicate ZIP member names"));
        }
        if raw.is_symlink() {
            return Err(ReadRefusal::Unsafe("has a symbolic-link ZIP member"));
        }
        if raw.is_dir() {
            continue;
        }
        if chosen.is_some() {
            return Err(ReadRefusal::NotSingleMember("holds more than one file"));
        }
        let declared = raw.size();
        if declared > MAX_CHECK_BYTES {
            return Err(ReadRefusal::TooLarge(declared));
        }
        if declared
            > raw
                .compressed_size()
                .saturating_mul(MAX_CHECK_ZIP_RATIO)
                .max(4096)
        {
            return Err(ReadRefusal::Unsafe("has a suspicious compression ratio"));
        }
        chosen = Some((index, name, declared));
    }
    let Some((index, name, declared)) = chosen else {
        return Err(ReadRefusal::NotSingleMember("holds no file"));
    };
    let entry = archive
        .by_index(index)
        .map_err(|e| ReadRefusal::Corrupt(e.to_string()))?;
    if entry.name_raw() != name.as_slice() {
        return Err(ReadRefusal::Corrupt("the ZIP entry changed".into()));
    }
    let mut bytes = Vec::with_capacity(declared as usize);
    entry
        .take(declared.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| ReadRefusal::Corrupt(e.to_string()))?;
    if bytes.len() as u64 != declared {
        return Err(ReadRefusal::Corrupt(
            "the member size differs from its header".into(),
        ));
    }
    Ok((String::from_utf8_lossy(&name).into_owned(), bytes))
}

pub fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------
// Content signals (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub platform: String,
    pub reason: String,
}

fn signal(platform: &str, reason: &str) -> Signal {
    Signal {
        platform: platform.to_string(),
        reason: reason.to_string(),
    }
}

/// Validated headers found in `bytes`, for any system. A header counts only
/// when its own self-check (checksum, logo, magic) validates.
pub fn header_signals(bytes: &[u8]) -> Vec<Signal> {
    let mut out = Vec::new();
    if crate::nes_header_evidence::parse_ines_header(bytes).is_some() {
        out.push(signal("NES", "iNES header (NES\\x1a) present"));
    }
    let snes_ok = crate::snes_header_evidence::best_snes_header_candidate(bytes).is_some()
        || (bytes.len() % 1024 == 512
            && crate::snes_header_evidence::best_snes_header_candidate(&bytes[512..]).is_some());
    if snes_ok {
        out.push(signal(
            "SNES",
            "SNES header with a valid checksum/complement pair",
        ));
    }
    if crate::gb_header_evidence::parse_gb_header(bytes)
        .is_some_and(|gb| gb.logo_valid && gb.header_checksum_valid)
    {
        out.push(signal(
            "Game Boy",
            "Game Boy header with the Nintendo logo and a valid header checksum",
        ));
    }
    if crate::gba_header_evidence::parse_gba_header(bytes)
        .is_some_and(|gba| gba.fixed_value_valid && gba.complement_check_valid)
    {
        out.push(signal(
            "Game Boy Advance",
            "GBA header with the fixed value and a valid complement check",
        ));
    }
    if let Some(platform) = crate::megadrive_header_evidence::parse_megadrive_header(bytes)
        .filter(|md| md.console_name_recognized)
        .and_then(|md| crate::megadrive_header_evidence::console_name_platform(&md.console_name))
    {
        out.push(signal(
            platform,
            "SEGA console name in the cartridge header",
        ));
    }
    out
}

/// Magic numbers of other platforms that veto the NES structure heuristic.
/// A veto only ever removes a Probable result, so a rare accidental match in
/// random NES code is the safe direction.
fn foreign_magic(bytes: &[u8]) -> bool {
    let at = |offset: usize, magic: &[u8]| bytes.get(offset..offset + magic.len()) == Some(magic);
    // Master System / Game Gear: "TMR SEGA" at one of three header offsets.
    [0x1FF0, 0x3FF0, 0x7FF0].iter().any(|o| at(*o, b"TMR SEGA"))
        // Mega Drive / Genesis / 32X: "SEGA" in the header at 0x100 or 0x101.
        || at(0x100, b"SEGA") || at(0x101, b"SEGA")
        // Game Boy Nintendo logo (first 8 bytes) at 0x104.
        || at(0x104, &[0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B])
        // GBA: fixed byte 0x96 at 0xB2 together with the logo start 0x24FFAE51.
        || at(0x04, &[0x24, 0xFF, 0xAE, 0x51])
        // N64 in any byte order.
        || [[0x80, 0x37, 0x12, 0x40], [0x37, 0x80, 0x40, 0x12], [0x40, 0x12, 0x37, 0x80], [0x12, 0x40, 0x80, 0x37]]
            .iter()
            .any(|m| at(0, m))
        // Atari 7800 (header byte 1..10), Lynx, Neo Geo Pocket.
        || at(1, b"ATARI7800") || at(0, b"LYNX")
        || at(0, b"COPYRIGHT BY SNK") || at(0, b" LICENSED BY SNK")
        // Plausible SNES internal header (printable title, map mode 0x2x/0x3x)
        // at the LoROM or HiROM location, even if its checksum is wrong.
        || [0x7FC0usize, 0xFFC0].iter().any(|base| {
            bytes.get(*base..*base + 0x20).is_some_and(|h| {
                h[..21].iter().all(|b| (0x20..0x7F).contains(b))
                    && matches!(h[0x15] & 0xF0, 0x20 | 0x30)
            })
        })
}

/// Structural signature for an unheadered NES image (heuristic, never above
/// Probable). Deliberately strict:
/// - size is a multiple of 8192 and at most 2 MiB; no iNES header;
/// - no validated header and no foreign magic for any other platform;
/// - only the fixed last 16 KiB bank is examined: NMI and IRQ vectors at the
///   file end are >= $8000, RESET is >= $C000, and NMI differs from RESET
///   (unused-vector layouts, as on the Atari 2600, repeat one address);
/// - the code at RESET starts with a typical NES power-on sequence: SEI, CLD
///   (78 D8), CLD, SEI (D8 78), or SEI, LDX #imm (78 A2), the idiom of the
///   standard "SEI / CLD / LDX #$FF / TXS" init.
pub fn nes_structure(bytes: &[u8]) -> Option<Signal> {
    const BANK: usize = 16 * 1024;
    if bytes.len() < BANK || !bytes.len().is_multiple_of(8192) || bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    if crate::nes_header_evidence::parse_ines_header(bytes).is_some()
        || !header_signals(bytes).is_empty()
        || foreign_magic(bytes)
    {
        return None;
    }
    let end = bytes.len();
    let table = &bytes[end - 6..];
    let word = |i: usize| usize::from(u16::from_le_bytes([table[i], table[i + 1]]));
    let (nmi, reset, irq) = (word(0), word(2), word(4));
    if nmi < 0x8000 || irq < 0x8000 || reset < 0xC000 || nmi == reset {
        return None;
    }
    let target = end - BANK + (reset - 0xC000);
    let start = bytes.get(target..target + 2)?;
    if target + 2 > end - 6 || !matches!(start, [0x78, 0xD8] | [0xD8, 0x78] | [0x78, 0xA2]) {
        return None;
    }
    Some(signal(
        "NES",
        "size is a multiple of 8192, no iNES header or other system's signature, and the fixed last 16 KiB bank has NES-style vectors whose RESET code starts with SEI/CLD/LDX",
    ))
}

// ---------------------------------------------------------------------------
// Classification (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashHit {
    pub platform: String,
    pub dat_name: String,
    pub dat_version: Option<String>,
    pub source_id: String,
    pub entry_name: String,
    pub rom_name: String,
    pub sha1: String,
    pub provenance: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evidence {
    pub assigned: String,
    pub hash_hits: Vec<HashHit>,
    pub signatures: Vec<Signal>,
    pub structure: Vec<Signal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub tier: Tier,
    pub detected: String,
    pub lines: Vec<String>,
}

fn consistent(assigned: &str, other: &str) -> bool {
    if same_platform(assigned, other) {
        return true;
    }
    // Related systems whose catalogues and headers legitimately overlap.
    const FAMILIES: [[&str; 2]; 4] = [
        ["gb", "gbc"],
        ["mastersystem", "gamegear"],
        ["megadrive", "32x"],
        ["ngp", "ngpc"],
    ];
    FAMILIES.iter().any(|family| {
        let ids: Vec<Option<&str>> = family
            .iter()
            .map(|alias| crate::canonical_platform_for_alias(alias))
            .collect();
        let has = |x: &str| ids.iter().flatten().any(|id| same_platform(id, x));
        has(assigned) && has(other)
    })
}

/// Classifies one game. `None` means no mismatch was found (including: the
/// catalogue or content agrees with the folder, or there is no evidence).
pub fn classify(e: &Evidence) -> Option<Classification> {
    let assigned_has_header = e
        .signatures
        .iter()
        .any(|s| consistent(&e.assigned, &s.platform));
    let other_hits: Vec<&HashHit> = e
        .hash_hits
        .iter()
        .filter(|h| !consistent(&e.assigned, &h.platform))
        .collect();
    if e.hash_hits
        .iter()
        .any(|h| consistent(&e.assigned, &h.platform))
    {
        return None; // the folder's own catalogue knows this file
    }
    if !other_hits.is_empty() {
        let mut platforms: Vec<&str> = other_hits.iter().map(|h| h.platform.as_str()).collect();
        platforms.sort_unstable();
        platforms.dedup();
        let mut lines: Vec<String> = other_hits
            .iter()
            .map(|h| {
                format!(
                    "Catalogue {}{}: entry \"{}\" (ROM {}), SHA-1 {} ({}).",
                    h.dat_name,
                    h.dat_version
                        .as_deref()
                        .map(|v| format!(" version {v}"))
                        .unwrap_or_default(),
                    h.entry_name,
                    h.rom_name,
                    h.sha1,
                    h.provenance,
                )
            })
            .collect();
        let detected = platforms[0].to_string();
        let mut tier = Tier::Confirmed;
        if platforms.len() > 1 {
            tier = Tier::Ambiguous;
            lines.push("Catalogues for different systems both contain this file.".into());
        }
        if assigned_has_header {
            tier = Tier::Ambiguous;
            lines.push("The file also carries a valid header for the folder's own system, which contradicts the catalogue.".into());
        }
        if e.signatures
            .iter()
            .any(|s| !consistent(&detected, &s.platform) && !consistent(&e.assigned, &s.platform))
        {
            tier = Tier::Ambiguous;
            lines.push("The file's header points at a third system.".into());
        }
        return Some(Classification {
            tier,
            detected,
            lines,
        });
    }
    if assigned_has_header {
        return None; // content agrees with the folder
    }
    let mut candidates: Vec<&Signal> = e
        .signatures
        .iter()
        .chain(e.structure.iter())
        .filter(|s| !consistent(&e.assigned, &s.platform))
        .collect();
    candidates.sort_by(|a, b| a.platform.cmp(&b.platform));
    let mut names: Vec<&str> = candidates.iter().map(|s| s.platform.as_str()).collect();
    names.dedup();
    match names.len() {
        0 => None,
        1 => Some(Classification {
            tier: Tier::Probable,
            detected: names[0].to_string(),
            lines: candidates
                .iter()
                .map(|s| {
                    format!(
                        "{} (content structure; no catalogue hash matched).",
                        s.reason
                    )
                })
                .collect(),
        }),
        _ => Some(Classification {
            tier: Tier::Ambiguous,
            detected: names[0].to_string(),
            lines: candidates
                .iter()
                .map(|s| format!("Points at {}: {}", s.platform, s.reason))
                .collect(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Detector
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub archive_id: i64,
    pub title: String,
    pub path: PathBuf,
    /// Canonical assigned system.
    pub assigned: String,
    /// Recorded source of the assignment (for example `folder_alias`).
    pub assigned_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub archive_id: i64,
    pub title: String,
    pub folder: PathBuf,
    pub assigned: String,
    pub assigned_source: Option<String>,
    pub tier: Tier,
    pub detected: String,
    pub lines: Vec<String>,
    /// The file (or ZIP member) really has a `.unh` extension. Explanatory
    /// wording only; never evidence.
    pub has_unh: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    pub findings: Vec<Finding>,
    pub checked: usize,
    /// Games per folder that were read, to show the disagreeing fraction.
    pub checked_per_folder: std::collections::BTreeMap<PathBuf, usize>,
    pub unreadable: usize,
    /// Larger than [`MAX_CHECK_BYTES`].
    pub too_large: usize,
    /// Multi-member, unsafe or inconsistent ZIPs, or changed files.
    pub skipped: usize,
    pub cancelled: bool,
}

pub fn folder_of(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Checks `candidates[start..start + cap]`. Read-only; polls `cancel`
/// between games and reports the running count through `progress`.
pub fn check_candidates(
    candidates: &[Candidate],
    start: usize,
    cap: usize,
    catalogues: &[Catalogue],
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize),
) -> RunOutput {
    let mut out = RunOutput::default();
    for candidate in candidates.iter().skip(start).take(cap) {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        out.checked += 1;
        progress(out.checked);
        let folder = folder_of(&candidate.path);
        let (member, bytes) = match read_check_file(&candidate.path, trusted) {
            Ok(read) => read,
            Err(ReadRefusal::Unreadable(_)) => {
                out.unreadable += 1;
                continue;
            }
            Err(ReadRefusal::TooLarge(_)) => {
                out.too_large += 1;
                continue;
            }
            Err(_) => {
                out.skipped += 1;
                continue;
            }
        };
        let has_unh = member
            .as_deref()
            .map(Path::new)
            .or(Some(candidate.path.as_path()))
            .and_then(Path::extension)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("unh"));
        *out.checked_per_folder.entry(folder.clone()).or_default() += 1;
        let sha1 = sha1_hex(&bytes);
        let mut hash_hits = Vec::new();
        for catalogue in catalogues {
            for rom in catalogue.index.lookup_sha1(&sha1) {
                hash_hits.push(HashHit {
                    platform: catalogue.platform.to_string(),
                    dat_name: catalogue.dat_name.clone(),
                    dat_version: catalogue.dat_version.clone(),
                    source_id: catalogue.source_id.clone(),
                    entry_name: rom.game_name.clone(),
                    rom_name: rom.rom_name.clone(),
                    sha1: sha1.clone(),
                    provenance: catalogue.provenance.to_string(),
                });
            }
        }
        let evidence = Evidence {
            assigned: candidate.assigned.clone(),
            hash_hits,
            signatures: header_signals(&bytes),
            structure: nes_structure(&bytes).into_iter().collect(),
        };
        if let Some(found) = classify(&evidence) {
            out.findings.push(Finding {
                archive_id: candidate.archive_id,
                title: candidate.title.clone(),
                folder,
                assigned: candidate.assigned.clone(),
                assigned_source: candidate.assigned_source.clone(),
                tier: found.tier,
                detected: found.detected,
                lines: found.lines,
                has_unh,
            });
        }
    }
    out
}
