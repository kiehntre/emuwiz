//! Bounded, local-only ROM-hack catalogue inspection and import.
//! Inspection is side-effect free; callers persist records only after explicit confirmation.
use crate::mod_catalogue::{
    ModCatalogueCategory, ModCatalogueHash, ModCatalogueHashAlgorithm, ModCatalogueIdentity,
    ModCatalogueProvenance, ModCatalogueProvider, ModCatalogueRecord, ModDestinationIntent,
    RomHackCatalogueMetadata, RomHackHeaderExpectation,
};
use crate::mod_package::{ModCanonicalPlatform, ModIdentityKind};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

pub const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_IMPORT_JSON_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_IMPORT_RECORDS: usize = 4096;
pub const MAX_IMPORT_ARCHIVE_ENTRIES: usize = 128;
#[derive(Debug)]
pub enum RomHackCatalogueImportError {
    Io(String),
    TooLarge(String),
    Malformed(String),
    Unsafe(String),
}
impl std::fmt::Display for RomHackCatalogueImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "catalogue I/O error: {s}"),
            Self::TooLarge(s) => write!(f, "catalogue is too large: {s}"),
            Self::Malformed(s) => write!(f, "malformed ROM-hack catalogue: {s}"),
            Self::Unsafe(s) => write!(f, "unsafe ROM-hack catalogue: {s}"),
        }
    }
}
impl std::error::Error for RomHackCatalogueImportError {}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalCatalogueInputFormat {
    Json,
    Zip,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalCatalogueShape {
    DirectArray,
    Records,
    Items,
    Hacks,
    Results,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum CatalogueField {
    HackTitle,
    BaseGameTitle,
    Platform,
    Author,
    Version,
    PatchFormat,
    Crc32,
    Sha1,
    Sha256,
    SourceSize,
    Region,
    Revision,
    HeaderExpectation,
    ReleaseDate,
    SourceRecordId,
}
impl CatalogueField {
    pub const ALL: [Self; 15] = [
        Self::HackTitle,
        Self::BaseGameTitle,
        Self::Platform,
        Self::Author,
        Self::Version,
        Self::PatchFormat,
        Self::Crc32,
        Self::Sha1,
        Self::Sha256,
        Self::SourceSize,
        Self::Region,
        Self::Revision,
        Self::HeaderExpectation,
        Self::ReleaseDate,
        Self::SourceRecordId,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::HackTitle => "hack title",
            Self::BaseGameTitle => "base game title",
            Self::Platform => "platform",
            Self::Author => "author",
            Self::Version => "version",
            Self::PatchFormat => "patch format",
            Self::Crc32 => "CRC32",
            Self::Sha1 => "SHA-1",
            Self::Sha256 => "SHA-256",
            Self::SourceSize => "source size",
            Self::Region => "region",
            Self::Revision => "revision",
            Self::HeaderExpectation => "header expectation",
            Self::ReleaseDate => "release date",
            Self::SourceRecordId => "source record ID",
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct RomHackCatalogueMapping {
    pub paths: BTreeMap<CatalogueField, String>,
}
#[derive(Clone, Debug)]
pub struct LocalCatalogueJsonCandidate {
    pub member_name: String,
    pub score: u32,
    pub shape: Option<LocalCatalogueShape>,
    pub records: usize,
}
#[derive(Clone, Debug)]
pub struct LocalCatalogueRejectedRecord {
    pub index: usize,
    pub reason: String,
}
#[derive(Clone, Debug)]
pub struct RomHackCatalogueImportPreview {
    pub source_path: PathBuf,
    pub input_format: LocalCatalogueInputFormat,
    pub snapshot_sha256: String,
    pub shape: Option<LocalCatalogueShape>,
    pub candidates: Vec<LocalCatalogueJsonCandidate>,
    pub selected_member: Option<String>,
    pub records_detected: usize,
    pub records_valid: usize,
    pub rejected: Vec<LocalCatalogueRejectedRecord>,
    pub mapped_fields: BTreeMap<CatalogueField, String>,
    pub ignored_fields: Vec<String>,
    pub duplicate_record_ids: Vec<String>,
    pub missing_identity: Vec<usize>,
    pub sample_records: Vec<ModCatalogueRecord>,
    pub requires_mapping: bool,
}
#[derive(Clone, Debug, serde::Deserialize)]
pub struct LocalRomHackCatalogueDocument {
    pub format_version: u32,
    pub source_name: String,
    #[serde(default)]
    pub source_url: Option<String>,
    pub records: Vec<LocalRomHackCatalogueEntry>,
}
#[derive(Clone, Debug, serde::Deserialize)]
pub struct LocalRomHackCatalogueEntry {
    pub record_id: String,
    pub hack_title: String,
    #[serde(default)]
    pub base_game_title: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub patch_format: Option<String>,
    #[serde(default)]
    pub required_base_crc32: Option<u32>,
    #[serde(default)]
    pub required_base_hashes: Vec<ModCatalogueHash>,
    #[serde(default)]
    pub required_base_size: Option<u64>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub header_expectation: Option<RomHackHeaderExpectation>,
    #[serde(default)]
    pub reference_url: Option<String>,
}

pub fn inspect_local_rom_hack_catalogue(
    path: impl AsRef<Path>,
) -> Result<RomHackCatalogueImportPreview, RomHackCatalogueImportError> {
    inspect(path.as_ref(), None, None)
}
pub fn inspect_local_rom_hack_catalogue_member(
    path: impl AsRef<Path>,
    member: &str,
) -> Result<RomHackCatalogueImportPreview, RomHackCatalogueImportError> {
    inspect(path.as_ref(), Some(member), None)
}
pub fn import_local_rom_hack_catalogue(
    path: impl AsRef<Path>,
) -> Result<Vec<ModCatalogueRecord>, RomHackCatalogueImportError> {
    let p = inspect_local_rom_hack_catalogue(&path)?;
    if p.requires_mapping
        || (p.input_format == LocalCatalogueInputFormat::Zip && p.selected_member.is_none())
    {
        return Err(RomHackCatalogueImportError::Malformed(
            "catalogue needs an explicit member or field mapping".into(),
        ));
    }
    commit_local_rom_hack_catalogue(
        path,
        p.selected_member.as_deref(),
        &RomHackCatalogueMapping::default(),
    )
}
pub fn commit_local_rom_hack_catalogue(
    path: impl AsRef<Path>,
    member: Option<&str>,
    mapping: &RomHackCatalogueMapping,
) -> Result<Vec<ModCatalogueRecord>, RomHackCatalogueImportError> {
    let p = inspect(path.as_ref(), member, Some(mapping))?;
    if p.requires_mapping
        || (p.input_format == LocalCatalogueInputFormat::Zip && p.selected_member.is_none())
    {
        return Err(RomHackCatalogueImportError::Malformed(
            "explicit mapping or ZIP member selection is required".into(),
        ));
    }
    let (_, json, snapshot) = read_input(path.as_ref(), p.selected_member.as_deref())?;
    let v: Value = serde_json::from_slice(&json)
        .map_err(|e| RomHackCatalogueImportError::Malformed(e.to_string()))?;
    let (items, _, _) = items(&v)?;
    let source = v
        .get("source_name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            path.as_ref()
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Local catalogue")
        });
    let url = v
        .get("source_url")
        .and_then(Value::as_str)
        .unwrap_or("https://emuwiz.invalid/local-catalogue");
    let auto = detect(&items);
    let active = if mapping.paths.is_empty() {
        &auto
    } else {
        mapping
    };
    items
        .into_iter()
        .enumerate()
        .filter_map(|(i, v)| normalize(v, active, i).ok())
        .map(|e| convert(source, url, &snapshot, e))
        .collect()
}
fn inspect(
    path: &Path,
    member: Option<&str>,
    mapping: Option<&RomHackCatalogueMapping>,
) -> Result<RomHackCatalogueImportPreview, RomHackCatalogueImportError> {
    let meta = fs::metadata(path).map_err(|e| RomHackCatalogueImportError::Io(e.to_string()))?;
    if meta.len() > MAX_IMPORT_BYTES {
        return Err(RomHackCatalogueImportError::TooLarge(
            "input exceeds the 64 MiB bound".into(),
        ));
    }
    let bytes = fs::read(path).map_err(|e| RomHackCatalogueImportError::Io(e.to_string()))?;
    let hash = digest(&bytes);
    let format = if bytes.starts_with(b"PK\x03\x04") {
        LocalCatalogueInputFormat::Zip
    } else {
        LocalCatalogueInputFormat::Json
    };
    let candidates = if format == LocalCatalogueInputFormat::Zip {
        zip_candidates(&bytes)?
    } else {
        Vec::new()
    };
    let selected = if format == LocalCatalogueInputFormat::Zip {
        choose(&candidates, member)?
    } else {
        None
    };
    let json = if format == LocalCatalogueInputFormat::Zip {
        let Some(member) = selected.as_deref() else {
            return Ok(RomHackCatalogueImportPreview {
                source_path: path.into(),
                input_format: format,
                snapshot_sha256: hash,
                shape: None,
                candidates,
                selected_member: None,
                records_detected: 0,
                records_valid: 0,
                rejected: Vec::new(),
                mapped_fields: BTreeMap::new(),
                ignored_fields: Vec::new(),
                duplicate_record_ids: Vec::new(),
                missing_identity: Vec::new(),
                sample_records: Vec::new(),
                requires_mapping: true,
            });
        };
        read_zip_member(&bytes, member)?
    } else {
        bytes.clone()
    };
    if json.len() > MAX_IMPORT_JSON_BYTES {
        return Err(RomHackCatalogueImportError::TooLarge(
            "JSON document exceeds the 8 MiB bound".into(),
        ));
    }
    let value: Value = serde_json::from_slice(&json)
        .map_err(|e| RomHackCatalogueImportError::Malformed(e.to_string()))?;
    let (raw, shape, keys) = items(&value)?;
    if raw.len() > MAX_IMPORT_RECORDS {
        return Err(RomHackCatalogueImportError::TooLarge(
            "record count exceeds the 4096 record bound".into(),
        ));
    }
    let auto = detect(&raw);
    let active = match mapping {
        Some(mapping) if !mapping.paths.is_empty() => mapping,
        _ => &auto,
    };
    let source = value
        .get("source_name")
        .and_then(Value::as_str)
        .unwrap_or("Local catalogue");
    let url = value
        .get("source_url")
        .and_then(Value::as_str)
        .unwrap_or("https://emuwiz.invalid/local-catalogue");
    let mut rejected = Vec::new();
    let mut samples = Vec::new();
    let mut seen = BTreeSet::new();
    let mut duplicates = Vec::new();
    let mut missing = Vec::new();
    for (i, v) in raw.iter().enumerate() {
        match normalize(v, active, i).and_then(|e| convert(source, url, &hash, e)) {
            Ok(r) => {
                if !seen.insert(r.provider.record_id.clone()) {
                    duplicates.push(r.provider.record_id.clone())
                }
                if r.declared_identity.is_empty()
                    && r.rom_hack
                        .as_ref()
                        .and_then(|m| m.required_base_crc32)
                        .is_none()
                {
                    missing.push(i)
                }
                if samples.len() < 3 {
                    samples.push(r)
                }
            }
            Err(e) => rejected.push(LocalCatalogueRejectedRecord {
                index: i,
                reason: e.to_string(),
            }),
        }
    }
    let mut ignored: BTreeSet<String> = keys.into_iter().collect();
    for p in active.paths.values() {
        ignored.remove(p);
    }
    Ok(RomHackCatalogueImportPreview {
        source_path: path.into(),
        input_format: format,
        snapshot_sha256: hash,
        shape,
        candidates,
        selected_member: selected.clone(),
        records_detected: raw.len(),
        records_valid: raw.len() - rejected.len(),
        rejected,
        mapped_fields: active.paths.clone(),
        ignored_fields: ignored.into_iter().collect(),
        duplicate_record_ids: duplicates,
        missing_identity: missing,
        sample_records: samples,
        requires_mapping: format == LocalCatalogueInputFormat::Zip && selected.is_none()
            || mapping.is_none() && ambiguous(&raw, &auto),
    })
}
fn read_input(
    path: &Path,
    member: Option<&str>,
) -> Result<(LocalCatalogueInputFormat, Vec<u8>, String), RomHackCatalogueImportError> {
    let b = fs::read(path).map_err(|e| RomHackCatalogueImportError::Io(e.to_string()))?;
    let h = digest(&b);
    if b.starts_with(b"PK\x03\x04") {
        Ok((
            LocalCatalogueInputFormat::Zip,
            read_zip_member(
                &b,
                member.ok_or_else(|| {
                    RomHackCatalogueImportError::Malformed("ZIP member selection required".into())
                })?,
            )?,
            h,
        ))
    } else {
        Ok((LocalCatalogueInputFormat::Json, b, h))
    }
}
type CatalogueItems<'a> = (Vec<&'a Value>, Option<LocalCatalogueShape>, Vec<String>);
fn items(v: &Value) -> Result<CatalogueItems<'_>, RomHackCatalogueImportError> {
    match v {
        Value::Array(a) => Ok((
            a.iter().collect(),
            Some(LocalCatalogueShape::DirectArray),
            Vec::new(),
        )),
        Value::Object(o) => {
            for k in ["records", "items", "hacks", "results"] {
                if let Some(Value::Array(a)) = o.get(k) {
                    return Ok((
                        a.iter().collect(),
                        Some(match k {
                            "records" => LocalCatalogueShape::Records,
                            "items" => LocalCatalogueShape::Items,
                            "hacks" => LocalCatalogueShape::Hacks,
                            _ => LocalCatalogueShape::Results,
                        }),
                        o.keys().cloned().collect(),
                    ));
                }
            }
            Err(RomHackCatalogueImportError::Malformed(
                "expected a record array or records/items/hacks/results wrapper".into(),
            ))
        }
        _ => Err(RomHackCatalogueImportError::Malformed(
            "catalogue root must be an array or object".into(),
        )),
    }
}
fn choose(
    c: &[LocalCatalogueJsonCandidate],
    requested: Option<&str>,
) -> Result<Option<String>, RomHackCatalogueImportError> {
    if let Some(n) = requested {
        if c.iter().any(|x| x.member_name == n) {
            return Ok(Some(n.into()));
        }
        return Err(RomHackCatalogueImportError::Malformed(
            "selected ZIP member is not a JSON candidate".into(),
        ));
    }
    if c.len() == 1 {
        Ok(Some(c[0].member_name.clone()))
    } else {
        Ok(None)
    }
}
fn zip_candidates(
    b: &[u8],
) -> Result<Vec<LocalCatalogueJsonCandidate>, RomHackCatalogueImportError> {
    let mut z = zip::ZipArchive::new(Cursor::new(b))
        .map_err(|e| RomHackCatalogueImportError::Malformed(e.to_string()))?;
    if z.len() > MAX_IMPORT_ARCHIVE_ENTRIES {
        return Err(RomHackCatalogueImportError::TooLarge(
            "archive entry count exceeds the 128 entry bound".into(),
        ));
    }
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut e = z
            .by_index(i)
            .map_err(|x| RomHackCatalogueImportError::Malformed(x.to_string()))?;
        if e.enclosed_name().is_none() {
            return Err(RomHackCatalogueImportError::Unsafe(
                "archive contains a traversal or absolute member path".into(),
            ));
        }
        if e.is_dir()
            || !e.name().to_ascii_lowercase().ends_with(".json")
            || e.size() > MAX_IMPORT_JSON_BYTES as u64
        {
            continue;
        }
        let mut j = Vec::new();
        e.read_to_end(&mut j)
            .map_err(|x| RomHackCatalogueImportError::Malformed(x.to_string()))?;
        if let Ok(v) = serde_json::from_slice::<Value>(&j)
            && let Ok((a, s, _)) = items(&v)
        {
            out.push(LocalCatalogueJsonCandidate {
                member_name: e.name().into(),
                score: if matches!(
                    s,
                    Some(LocalCatalogueShape::Records | LocalCatalogueShape::Hacks)
                ) {
                    100
                } else {
                    80
                } + a.len().min(20) as u32,
                shape: s,
                records: a.len(),
            });
        }
    }
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.member_name.cmp(&b.member_name))
    });
    if out.is_empty() {
        return Err(RomHackCatalogueImportError::Malformed(
            "archive contains no usable JSON catalogue".into(),
        ));
    }
    Ok(out)
}
fn read_zip_member(b: &[u8], name: &str) -> Result<Vec<u8>, RomHackCatalogueImportError> {
    let mut z = zip::ZipArchive::new(Cursor::new(b))
        .map_err(|e| RomHackCatalogueImportError::Malformed(e.to_string()))?;
    for i in 0..z.len() {
        let mut e = z
            .by_index(i)
            .map_err(|x| RomHackCatalogueImportError::Malformed(x.to_string()))?;
        if e.name() == name {
            let mut o = Vec::new();
            e.read_to_end(&mut o)
                .map_err(|x| RomHackCatalogueImportError::Malformed(x.to_string()))?;
            return Ok(o);
        }
    }
    Err(RomHackCatalogueImportError::Malformed(
        "ZIP member not found".into(),
    ))
}
fn aliases(f: CatalogueField) -> &'static [&'static str] {
    match f {
        CatalogueField::HackTitle => &["hack_title", "title", "name"],
        CatalogueField::BaseGameTitle => &["base_game_title", "base_game", "original_game", "game"],
        CatalogueField::Platform => &["platform", "system", "console"],
        CatalogueField::Author => &["author", "authors"],
        CatalogueField::Version => &["version", "release_version"],
        CatalogueField::PatchFormat => &["patch_format", "format"],
        CatalogueField::Crc32 => &["required_base_crc32", "crc32", "crc", "source_crc"],
        CatalogueField::Sha1 => &["source_sha1", "sha1"],
        CatalogueField::Sha256 => &["source_sha256", "sha256"],
        CatalogueField::SourceSize => &["required_base_size", "source_size", "size"],
        CatalogueField::Region => &["region", "country"],
        CatalogueField::Revision => &["revision", "rev"],
        CatalogueField::HeaderExpectation => &["header_expectation", "header"],
        CatalogueField::ReleaseDate => &["release_date", "released"],
        CatalogueField::SourceRecordId => &["record_id", "source_record_id", "id", "slug"],
    }
}
fn merged(v: &Value) -> Option<serde_json::Map<String, Value>> {
    let mut o = v.as_object()?.clone();
    if let Some(Value::Object(m)) = o.get("metadata").cloned() {
        for (k, v) in m {
            o.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Some(o)
}
fn detect(a: &[&Value]) -> RomHackCatalogueMapping {
    let mut m = RomHackCatalogueMapping::default();
    for f in CatalogueField::ALL {
        let c: BTreeSet<_> = aliases(f)
            .iter()
            .filter(|k| {
                a.iter()
                    .any(|v| merged(v).and_then(|o| o.get(**k).cloned()).is_some())
            })
            .copied()
            .collect();
        if c.len() == 1 {
            m.paths.insert(f, c.into_iter().next().unwrap().into());
        }
    }
    m
}
fn ambiguous(a: &[&Value], m: &RomHackCatalogueMapping) -> bool {
    [CatalogueField::HackTitle, CatalogueField::SourceRecordId]
        .iter()
        .any(|f| {
            let c = aliases(*f)
                .iter()
                .filter(|k| {
                    a.iter()
                        .any(|v| merged(v).and_then(|o| o.get(**k).cloned()).is_some())
                })
                .count();
            c > 1 && !m.paths.contains_key(f)
        })
}
#[derive(Clone, Debug)]
struct Entry {
    id: String,
    title: String,
    base: Option<String>,
    platform: Option<String>,
    author: Option<String>,
    version: Option<String>,
    patch: Option<String>,
    crc: Option<u32>,
    hashes: Vec<ModCatalogueHash>,
    size: Option<u64>,
    region: Option<String>,
    revision: Option<String>,
    header: Option<RomHackHeaderExpectation>,
    release: Option<String>,
    description: Option<String>,
}
fn normalize(
    v: &Value,
    m: &RomHackCatalogueMapping,
    i: usize,
) -> Result<Entry, RomHackCatalogueImportError> {
    let o = merged(v).ok_or_else(|| {
        RomHackCatalogueImportError::Malformed(format!("record {i} is not an object"))
    })?;
    let get = |f| m.paths.get(&f).and_then(|k| o.get(k));
    let text = |f| {
        get(f)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let title = text(CatalogueField::HackTitle).ok_or_else(|| {
        RomHackCatalogueImportError::Malformed(format!("record {i} has no unambiguous hack title"))
    })?;
    let id = text(CatalogueField::SourceRecordId).unwrap_or_else(|| format!("local-{i}-{title}"));
    let crc = get(CatalogueField::Crc32).map(parse_u32).transpose()?;
    let size = get(CatalogueField::SourceSize).map(parse_u64).transpose()?;
    let mut hashes = Vec::new();
    for (f, a) in [
        (CatalogueField::Sha1, ModCatalogueHashAlgorithm::Sha1),
        (CatalogueField::Sha256, ModCatalogueHashAlgorithm::Sha256),
    ] {
        if let Some(x) = text(f) {
            hashes.push(ModCatalogueHash {
                algorithm: a,
                value: x,
            })
        }
    }
    let header = text(CatalogueField::HeaderExpectation)
        .map(|x| match x.to_ascii_lowercase().as_str() {
            "headered" => Ok(RomHackHeaderExpectation::Headered),
            "headerless" => Ok(RomHackHeaderExpectation::Headerless),
            "either" => Ok(RomHackHeaderExpectation::Either),
            _ => Err(RomHackCatalogueImportError::Malformed(format!(
                "record {i} has unknown header expectation"
            ))),
        })
        .transpose()?;
    let author = text(CatalogueField::Author).or_else(|| {
        get(CatalogueField::Author)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
    });
    Ok(Entry {
        id,
        title,
        base: text(CatalogueField::BaseGameTitle),
        platform: text(CatalogueField::Platform),
        author,
        version: text(CatalogueField::Version),
        patch: text(CatalogueField::PatchFormat),
        crc,
        hashes,
        size,
        region: text(CatalogueField::Region),
        revision: text(CatalogueField::Revision),
        header,
        release: text(CatalogueField::ReleaseDate),
        description: o
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}
fn parse_u32(v: &Value) -> Result<u32, RomHackCatalogueImportError> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| {
            v.as_str()
                .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        })
        .ok_or_else(|| RomHackCatalogueImportError::Malformed("invalid CRC32".into()))
}
fn parse_u64(v: &Value) -> Result<u64, RomHackCatalogueImportError> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| RomHackCatalogueImportError::Malformed("invalid source size".into()))
}
fn convert(
    source: &str,
    url: &str,
    snapshot: &str,
    e: Entry,
) -> Result<ModCatalogueRecord, RomHackCatalogueImportError> {
    let platform = e.platform.as_deref().map(parse_platform).transpose()?;
    let ids = e
        .hashes
        .iter()
        .filter(|h| h.algorithm == ModCatalogueHashAlgorithm::Sha256)
        .map(|h| ModCatalogueIdentity {
            kind: ModIdentityKind::LooseRomSha256,
            value: h.value.clone(),
        })
        .collect();
    let r = ModCatalogueRecord {
        provider: ModCatalogueProvider {
            name: source.into(),
            record_id: e.id,
            source_page_url: url.into(),
            schema_version: Some("local_rom_hack_v1".into()),
            imported_at: None,
            snapshot_sha256: Some(snapshot.into()),
        },
        display_title: e.title,
        author: e.author,
        version: e.version,
        description: e.description,
        title_hint: e.base.clone(),
        platform,
        category: ModCatalogueCategory::GameMod,
        payloads: Vec::new(),
        declared_identity: ids,
        declared_region: e.region,
        declared_revision: e.revision,
        destination_intent: ModDestinationIntent::Manual,
        instructions: None,
        provenance: ModCatalogueProvenance {
            source_terms_url: None,
            licence: None,
            author_or_uploader: None,
            note: Some("user-supplied local ROM-hack metadata; no payload is included".into()),
        },
        rom_hack: Some(RomHackCatalogueMetadata {
            base_game_title: e.base,
            release_date: e.release,
            patch_format: e.patch,
            required_base_crc32: e.crc,
            required_base_hashes: e.hashes,
            required_base_size: e.size,
            header_expectation: e.header,
            local_patch_path: None,
            associated_patch_sha256: None,
        }),
    };
    r.validate().map_err(|e| {
        RomHackCatalogueImportError::Malformed(
            e.into_iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    Ok(r)
}
fn parse_platform(v: &str) -> Result<ModCanonicalPlatform, RomHackCatalogueImportError> {
    match v.trim().to_ascii_lowercase().as_str() {
        "snes" | "super_nes" => Ok(ModCanonicalPlatform::Snes),
        "megadrive" | "genesis" => Ok(ModCanonicalPlatform::MegaDrive),
        "ps2" | "playstation2" => Ok(ModCanonicalPlatform::PlayStation2),
        "ps3" | "playstation3" => Ok(ModCanonicalPlatform::PlayStation3),
        "gamecube" => Ok(ModCanonicalPlatform::GameCube),
        "wii" => Ok(ModCanonicalPlatform::Wii),
        "xbox360" => Ok(ModCanonicalPlatform::Xbox360),
        x => Err(RomHackCatalogueImportError::Malformed(format!(
            "unsupported platform {x:?}"
        ))),
    }
}
fn digest(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn aliases_json() -> Vec<u8> {
        br#"[{"name":"Hack","game":"Game","system":"snes","author":"A","crc":"1234ABCD","release_version":"1"}]"#.to_vec()
    }

    #[test]
    fn direct_array_aliases_and_preview_are_supported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalogue.json");
        fs::write(&path, aliases_json()).unwrap();
        let preview = inspect_local_rom_hack_catalogue(&path).unwrap();
        assert_eq!(preview.shape, Some(LocalCatalogueShape::DirectArray));
        assert_eq!(preview.records_valid, 1);
        assert!(!preview.requires_mapping);
        assert_eq!(
            import_local_rom_hack_catalogue(&path).unwrap()[0].display_title,
            "Hack"
        );
    }

    #[test]
    fn wrappers_and_nested_metadata_are_supported() {
        let dir = tempfile::tempdir().unwrap();
        for key in ["records", "hacks"] {
            let path = dir.path().join(format!("{key}.json"));
            fs::write(
                &path,
                format!(r#"{{"{key}":[{{"metadata":{{"title":"Hack"}},"id":"x"}}]}}"#),
            )
            .unwrap();
            assert_eq!(
                inspect_local_rom_hack_catalogue(&path)
                    .unwrap()
                    .records_valid,
                1
            );
        }
    }

    #[test]
    fn ambiguous_alias_requires_explicit_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ambiguous.json");
        fs::write(&path, br#"[{"title":"A","name":"B"}]"#).unwrap();
        assert!(
            inspect_local_rom_hack_catalogue(&path)
                .unwrap()
                .requires_mapping
        );
        let mut mapping = RomHackCatalogueMapping::default();
        mapping
            .paths
            .insert(CatalogueField::HackTitle, "title".into());
        assert_eq!(
            commit_local_rom_hack_catalogue(&path, None, &mapping)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn multiple_zip_candidates_require_explicit_choice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalogue.zip");
        let file = fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for name in ["one.json", "two.json"] {
            writer.start_file(name, options).unwrap();
            writer.write_all(&aliases_json()).unwrap();
        }
        writer.finish().unwrap();
        let preview = inspect_local_rom_hack_catalogue(&path).unwrap();
        assert_eq!(preview.candidates.len(), 2);
        assert!(preview.selected_member.is_none());
    }

    #[test]
    fn malformed_records_are_rejected_without_writing_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.json");
        fs::write(&path, br#"[{"title":"ok"},3]"#).unwrap();
        let before = fs::read(&path).unwrap();
        let preview = inspect_local_rom_hack_catalogue(&path).unwrap();
        assert_eq!(preview.records_valid, 1);
        assert_eq!(preview.rejected.len(), 1);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
