//! Bounded import of user-supplied ROM-hack metadata.
//!
//! This boundary accepts only local JSON or a local ZIP containing one JSON
//! document.  It never follows URLs, downloads payloads, or turns metadata
//! into an apply operation.  Imported entries reuse `ModCatalogueRecord` so
//! the existing SQLite key, validation, and browse projections remain shared.

use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::mod_catalogue::{
    ModCatalogueCategory, ModCatalogueHash, ModCatalogueHashAlgorithm, ModCatalogueIdentity,
    ModCatalogueProvenance, ModCatalogueProvider, ModCatalogueRecord, ModDestinationIntent,
    RomHackCatalogueMetadata, RomHackHeaderExpectation,
};
use crate::mod_package::{ModCanonicalPlatform, ModIdentityKind};

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
            Self::Io(detail) => write!(f, "catalogue I/O error: {detail}"),
            Self::TooLarge(detail) => write!(f, "catalogue is too large: {detail}"),
            Self::Malformed(detail) => write!(f, "malformed ROM-hack catalogue: {detail}"),
            Self::Unsafe(detail) => write!(f, "unsafe ROM-hack catalogue: {detail}"),
        }
    }
}

impl std::error::Error for RomHackCatalogueImportError {}

#[derive(Clone, Debug, Deserialize)]
pub struct LocalRomHackCatalogueDocument {
    pub format_version: u32,
    pub source_name: String,
    #[serde(default)]
    pub source_url: Option<String>,
    pub records: Vec<LocalRomHackCatalogueEntry>,
}

#[derive(Clone, Debug, Deserialize)]
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

pub fn import_local_rom_hack_catalogue(
    path: impl AsRef<Path>,
) -> Result<Vec<ModCatalogueRecord>, RomHackCatalogueImportError> {
    let path = path.as_ref();
    let metadata =
        fs::metadata(path).map_err(|e| RomHackCatalogueImportError::Io(e.to_string()))?;
    if metadata.len() > MAX_IMPORT_BYTES {
        return Err(RomHackCatalogueImportError::TooLarge(
            "input exceeds the 64 MiB bound".into(),
        ));
    }
    let bytes = fs::read(path).map_err(|e| RomHackCatalogueImportError::Io(e.to_string()))?;
    let snapshot_sha256 = digest(&bytes);
    let json = if bytes.starts_with(b"PK\x03\x04") {
        read_zip_json(&bytes)?
    } else {
        bytes
    };
    if json.len() > MAX_IMPORT_JSON_BYTES {
        return Err(RomHackCatalogueImportError::TooLarge(
            "JSON document exceeds the 8 MiB bound".into(),
        ));
    }
    let document: LocalRomHackCatalogueDocument = serde_json::from_slice(&json)
        .map_err(|e| RomHackCatalogueImportError::Malformed(e.to_string()))?;
    if document.format_version != 1 {
        return Err(RomHackCatalogueImportError::Malformed(
            "unsupported format_version".into(),
        ));
    }
    if document.records.len() > MAX_IMPORT_RECORDS {
        return Err(RomHackCatalogueImportError::TooLarge(
            "record count exceeds the 4096 record bound".into(),
        ));
    }
    if document.source_name.trim().is_empty() || document.source_name.len() > 4096 {
        return Err(RomHackCatalogueImportError::Malformed(
            "source_name is empty or too long".into(),
        ));
    }
    let source_url = document
        .source_url
        .unwrap_or_else(|| "https://emuwiz.invalid/local-catalogue".into());
    let mut records = Vec::with_capacity(document.records.len());
    for entry in document.records {
        records.push(convert_entry(
            &document.source_name,
            &source_url,
            &snapshot_sha256,
            entry,
        )?);
    }
    Ok(records)
}

fn read_zip_json(bytes: &[u8]) -> Result<Vec<u8>, RomHackCatalogueImportError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| RomHackCatalogueImportError::Malformed(e.to_string()))?;
    if archive.len() > MAX_IMPORT_ARCHIVE_ENTRIES {
        return Err(RomHackCatalogueImportError::TooLarge(
            "archive entry count exceeds the 128 entry bound".into(),
        ));
    }
    let mut json_index = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| RomHackCatalogueImportError::Malformed(e.to_string()))?;
        if entry.enclosed_name().is_none() {
            return Err(RomHackCatalogueImportError::Unsafe(
                "archive contains a traversal or absolute member path".into(),
            ));
        }
        if !entry.is_dir() && entry.name().to_ascii_lowercase().ends_with(".json") {
            if json_index.is_some() && !entry.name().eq_ignore_ascii_case("rom-hacks.json") {
                return Err(RomHackCatalogueImportError::Malformed(
                    "archive contains multiple JSON catalogue documents".into(),
                ));
            }
            json_index = Some(index);
        }
    }
    let index = json_index.ok_or_else(|| {
        RomHackCatalogueImportError::Malformed("archive contains no JSON catalogue".into())
    })?;
    let mut entry = archive
        .by_index(index)
        .map_err(|e| RomHackCatalogueImportError::Malformed(e.to_string()))?;
    let declared = entry.size();
    if declared > MAX_IMPORT_JSON_BYTES as u64 {
        return Err(RomHackCatalogueImportError::TooLarge(
            "JSON member exceeds the 8 MiB bound".into(),
        ));
    }
    let mut json = Vec::with_capacity(declared as usize);
    entry
        .read_to_end(&mut json)
        .map_err(|e| RomHackCatalogueImportError::Malformed(e.to_string()))?;
    Ok(json)
}

fn convert_entry(
    source_name: &str,
    source_url: &str,
    snapshot_sha256: &str,
    entry: LocalRomHackCatalogueEntry,
) -> Result<ModCatalogueRecord, RomHackCatalogueImportError> {
    if entry.record_id.trim().is_empty() || entry.hack_title.trim().is_empty() {
        return Err(RomHackCatalogueImportError::Malformed(
            "record_id and hack_title are required".into(),
        ));
    }
    let platform = entry.platform.as_deref().map(parse_platform).transpose()?;
    let declared_identity = entry
        .required_base_hashes
        .iter()
        .filter(|hash| hash.algorithm == ModCatalogueHashAlgorithm::Sha256)
        .map(|hash| ModCatalogueIdentity {
            kind: ModIdentityKind::LooseRomSha256,
            value: hash.value.clone(),
        })
        .collect();
    let metadata = RomHackCatalogueMetadata {
        base_game_title: entry.base_game_title.clone(),
        release_date: entry.release_date,
        patch_format: entry.patch_format,
        required_base_crc32: entry.required_base_crc32,
        required_base_hashes: entry.required_base_hashes,
        required_base_size: entry.required_base_size,
        header_expectation: entry.header_expectation,
        local_patch_path: None,
        associated_patch_sha256: None,
    };
    let record = ModCatalogueRecord {
        provider: ModCatalogueProvider {
            name: source_name.into(),
            record_id: entry.record_id,
            source_page_url: entry.reference_url.unwrap_or_else(|| source_url.into()),
            schema_version: Some("local_rom_hack_v1".into()),
            imported_at: None,
            snapshot_sha256: Some(snapshot_sha256.into()),
        },
        display_title: entry.hack_title,
        author: entry.author,
        version: entry.version,
        description: entry.description,
        title_hint: metadata.base_game_title.clone(),
        platform,
        category: ModCatalogueCategory::GameMod,
        payloads: Vec::new(),
        declared_identity,
        declared_region: entry.region,
        declared_revision: entry.revision,
        destination_intent: ModDestinationIntent::Manual,
        instructions: None,
        provenance: ModCatalogueProvenance {
            source_terms_url: None,
            licence: None,
            author_or_uploader: None,
            note: Some("user-supplied local ROM-hack metadata; no payload is included".into()),
        },
        rom_hack: Some(metadata),
    };
    record.validate().map_err(|errors| {
        RomHackCatalogueImportError::Malformed(
            errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    Ok(record)
}

fn parse_platform(value: &str) -> Result<ModCanonicalPlatform, RomHackCatalogueImportError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "snes" | "super_nes" => Ok(ModCanonicalPlatform::Snes),
        "megadrive" | "genesis" => Ok(ModCanonicalPlatform::MegaDrive),
        "ps2" | "playstation2" => Ok(ModCanonicalPlatform::PlayStation2),
        "ps3" | "playstation3" => Ok(ModCanonicalPlatform::PlayStation3),
        "gamecube" => Ok(ModCanonicalPlatform::GameCube),
        "wii" => Ok(ModCanonicalPlatform::Wii),
        "xbox360" => Ok(ModCanonicalPlatform::Xbox360),
        other => Err(RomHackCatalogueImportError::Malformed(format!(
            "unsupported platform {other:?}"
        ))),
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn document() -> Vec<u8> {
        br#"{
          "format_version": 1,
          "source_name": "Synthetic local archive",
          "records": [{
            "record_id": "hack-1",
            "hack_title": "Synthetic Quest Redux",
            "base_game_title": "Synthetic Quest",
            "platform": "snes",
            "author": "Local Author",
            "version": "1.2",
            "patch_format": "bps",
            "required_base_crc32": 305419896,
            "required_base_size": 4194304,
            "region": "US",
            "revision": "1.0",
            "header_expectation": "headerless"
          }]
        }"#
        .to_vec()
    }

    #[test]
    fn bounded_json_import_is_deterministic_and_content_addressed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("catalogue.json");
        fs::write(&path, document()).unwrap();
        let first = import_local_rom_hack_catalogue(&path).unwrap();
        let second = import_local_rom_hack_catalogue(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
        let expected_snapshot = digest(&document());
        assert_eq!(
            first[0].provider.snapshot_sha256.as_deref(),
            Some(expected_snapshot.as_str())
        );
        assert_eq!(
            first[0].rom_hack.as_ref().unwrap().patch_format.as_deref(),
            Some("bps")
        );
    }

    #[test]
    fn malformed_document_and_unsafe_zip_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let malformed = temp.path().join("bad.json");
        fs::write(&malformed, b"not json").unwrap();
        assert!(matches!(
            import_local_rom_hack_catalogue(&malformed),
            Err(RomHackCatalogueImportError::Malformed(_))
        ));

        let zip_path = temp.path().join("unsafe.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("../catalogue.json", options).unwrap();
        writer.write_all(&document()).unwrap();
        writer.finish().unwrap();
        assert!(matches!(
            import_local_rom_hack_catalogue(&zip_path),
            Err(RomHackCatalogueImportError::Unsafe(_))
        ));
    }
}
