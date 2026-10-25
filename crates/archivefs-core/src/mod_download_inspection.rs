//! Local handoff from a completed mod download to the existing inspectors.
//!
//! This module does not fetch, extract, apply, or execute anything.  The
//! content-addressed object is verified before any path-based inspector sees
//! it, and its bytes—not its URL or filename—choose the inspection route.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::archived_mod_package::{
    ArchivedModCompatibility, ArchivedModPackageInspection, inspect_archived_mod_package_for_game,
};
use crate::mod_download_transport::ModDownloadResult;
use crate::mod_package::SelectedGameForMod;
use crate::standalone_patch::{
    PatchCompatibility, StandalonePatchInspection, inspect_standalone_patch, match_patch_source,
};

const SIGNATURE_BYTES: usize = 16;
const HASH_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadedModPayloadClassification {
    StandalonePatch,
    ArchivePackage,
    ExecutableOrScript,
    OpaqueBinary,
    Unsupported,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum DownloadedModInspectionEvidence {
    StandalonePatch(StandalonePatchInspection),
    ArchivedPackage(ArchivedModPackageInspection),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadedModCompatibility {
    Compatible,
    Incompatible,
    ReviewRequired,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DownloadedModInspection {
    pub download: ModDownloadResult,
    pub classification: DownloadedModPayloadClassification,
    pub inspection: Option<DownloadedModInspectionEvidence>,
    pub inspection_sha256: String,
    pub compatibility: Option<DownloadedModCompatibility>,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

pub struct ModDownloadedInspectionRequest<'a> {
    pub download: &'a ModDownloadResult,
    pub selected_game: Option<&'a SelectedGameForMod>,
    /// A provider declaration is advisory. It is retained only to explain a
    /// conflict; it never selects an inspector.
    pub provider_declared_format: Option<&'a str>,
}

#[derive(Debug)]
pub enum ModDownloadInspectionFailure {
    MissingObject(PathBuf),
    NotRegularFile(PathBuf),
    Io(io::Error),
    CacheIdentityMismatch { expected: String, actual: String },
    SizeMismatch { expected: u64, actual: u64 },
    Inspector(String),
}

impl std::fmt::Display for ModDownloadInspectionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingObject(path) => {
                write!(f, "downloaded cache object is missing: {}", path.display())
            }
            Self::NotRegularFile(path) => write!(
                f,
                "downloaded cache object is not a regular file: {}",
                path.display()
            ),
            Self::Io(error) => write!(f, "cannot read downloaded cache object: {error}"),
            Self::CacheIdentityMismatch { expected, actual } => write!(
                f,
                "downloaded cache SHA-256 changed: expected {expected}, got {actual}"
            ),
            Self::SizeMismatch { expected, actual } => write!(
                f,
                "downloaded cache size changed: expected {expected}, got {actual}"
            ),
            Self::Inspector(error) => write!(f, "downloaded payload inspection failed: {error}"),
        }
    }
}

impl std::error::Error for ModDownloadInspectionFailure {}

impl From<io::Error> for ModDownloadInspectionFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn inspect_downloaded_mod(
    request: ModDownloadedInspectionRequest<'_>,
) -> Result<DownloadedModInspection, ModDownloadInspectionFailure> {
    let (size, sha256) = verify_cache_object(request.download)?;
    let classification = classify_downloaded_bytes(&request.download.path)?;
    let mut warnings = Vec::new();
    let mut blockers = Vec::new();

    if let Some(declared) = request.provider_declared_format
        && !declared_format_matches(declared, classification)
    {
        let message = format!(
            "provider declared {declared}, but downloaded bytes classify as {classification:?}"
        );
        warnings.push(message.clone());
        blockers.push("provider/byte format conflict requires review".into());
    }

    let (inspection, compatibility) = match classification {
        DownloadedModPayloadClassification::StandalonePatch => {
            let patch = inspect_standalone_patch(&request.download.path)
                .map_err(|error| ModDownloadInspectionFailure::Inspector(error.to_string()))?;
            let compatibility = request.selected_game.map(|game| {
                let base = fs::read(&game.identity.archive_path).ok();
                match_patch_source(&patch, base.as_deref(), false)
                    .compatibility
                    .into()
            });
            if patch.state != crate::standalone_patch::PatchInspectionState::Valid {
                blockers.push("standalone patch is structurally invalid".into());
            }
            (
                Some(DownloadedModInspectionEvidence::StandalonePatch(patch)),
                compatibility,
            )
        }
        DownloadedModPayloadClassification::ArchivePackage => {
            let package = inspect_archived_mod_package_for_game(
                &request.download.path,
                request.selected_game,
            )
            .map_err(ModDownloadInspectionFailure::Inspector)?;
            let compatibility = Some(package.compatibility.into());
            (
                Some(DownloadedModInspectionEvidence::ArchivedPackage(package)),
                compatibility,
            )
        }
        DownloadedModPayloadClassification::ExecutableOrScript => {
            warnings.push(
                "downloaded payload is executable/script content; it was not executed".into(),
            );
            blockers.push("executable/script payload requires manual review".into());
            (None, None)
        }
        DownloadedModPayloadClassification::OpaqueBinary
        | DownloadedModPayloadClassification::Unsupported
        | DownloadedModPayloadClassification::Unknown => {
            warnings.push("downloaded payload has no supported safe inspector".into());
            (None, None)
        }
    };

    Ok(DownloadedModInspection {
        download: request.download.clone(),
        classification,
        inspection,
        inspection_sha256: sha256,
        compatibility,
        warnings,
        blockers,
    })
    .map(|mut result| {
        if size != result.download.size_bytes {
            result
                .warnings
                .push("transport size and verified object size differ".into());
        }
        result
    })
}

fn verify_cache_object(
    download: &ModDownloadResult,
) -> Result<(u64, String), ModDownloadInspectionFailure> {
    let metadata = fs::symlink_metadata(&download.path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            ModDownloadInspectionFailure::MissingObject(download.path.clone())
        } else {
            ModDownloadInspectionFailure::Io(error)
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ModDownloadInspectionFailure::NotRegularFile(
            download.path.clone(),
        ));
    }
    let (size, actual) = hash_file(&download.path)?;
    if size != download.size_bytes {
        return Err(ModDownloadInspectionFailure::SizeMismatch {
            expected: download.size_bytes,
            actual: size,
        });
    }
    if !actual.eq_ignore_ascii_case(&download.sha256) {
        return Err(ModDownloadInspectionFailure::CacheIdentityMismatch {
            expected: download.sha256.clone(),
            actual,
        });
    }
    Ok((size, download.sha256.to_ascii_lowercase()))
}

fn hash_file(path: &Path) -> Result<(u64, String), io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; HASH_CHUNK_BYTES];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("file size overflow"))?;
        hasher.update(&buffer[..read]);
    }
    Ok((size, hex_digest(&hasher.finalize())))
}

fn classify_downloaded_bytes(path: &Path) -> Result<DownloadedModPayloadClassification, io::Error> {
    let mut file = File::open(path)?;
    let mut bytes = [0u8; SIGNATURE_BYTES];
    let count = file.read(&mut bytes)?;
    let b = &bytes[..count];
    if b.starts_with(b"PATCH")
        || b.starts_with(b"BPS1")
        || b.starts_with(b"UPS1")
        || b.starts_with(b"PPF")
        || b.starts_with(b"\xD6\xC3\xC4")
    {
        return Ok(DownloadedModPayloadClassification::StandalonePatch);
    }
    if b.starts_with(b"PK\x03\x04")
        || b.starts_with(b"PK\x05\x06")
        || b.starts_with(b"7z\xBC\xAF\x27\x1C")
        || b.starts_with(b"Rar!\x1A\x07")
    {
        return Ok(DownloadedModPayloadClassification::ArchivePackage);
    }
    if b.starts_with(b"MZ") || b.starts_with(b"\x7FELF") || b.starts_with(b"#!") {
        return Ok(DownloadedModPayloadClassification::ExecutableOrScript);
    }
    if b.is_empty() {
        Ok(DownloadedModPayloadClassification::Unknown)
    } else {
        Ok(DownloadedModPayloadClassification::OpaqueBinary)
    }
}

fn declared_format_matches(
    declared: &str,
    classification: DownloadedModPayloadClassification,
) -> bool {
    let declared = declared.trim().to_ascii_lowercase();
    match declared.as_str() {
        "zip" | "7z" | "7zip" | "rar" | "archive" => {
            classification == DownloadedModPayloadClassification::ArchivePackage
        }
        "ips" | "bps" | "ups" | "xdelta" | "vcdiff" | "ppf" | "patch" => {
            classification == DownloadedModPayloadClassification::StandalonePatch
        }
        "exe" | "dll" | "elf" | "script" | "executable" => {
            classification == DownloadedModPayloadClassification::ExecutableOrScript
        }
        _ => true,
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl From<PatchCompatibility> for DownloadedModCompatibility {
    fn from(value: PatchCompatibility) -> Self {
        match value {
            PatchCompatibility::Compatible => Self::Compatible,
            PatchCompatibility::Incompatible => Self::Incompatible,
            PatchCompatibility::ReviewRequired => Self::ReviewRequired,
            PatchCompatibility::Unknown => Self::Unknown,
        }
    }
}

impl From<ArchivedModCompatibility> for DownloadedModCompatibility {
    fn from(value: ArchivedModCompatibility) -> Self {
        match value {
            ArchivedModCompatibility::Compatible => Self::Compatible,
            ArchivedModCompatibility::Incompatible => Self::Incompatible,
            ArchivedModCompatibility::ReviewRequired => Self::ReviewRequired,
            ArchivedModCompatibility::Unknown => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_catalogue::ModCatalogueHashAlgorithm;
    use std::io::Write;

    fn downloaded(dir: &Path, bytes: &[u8]) -> ModDownloadResult {
        let path = dir.join("sha256-object");
        fs::write(&path, bytes).unwrap();
        let (_, sha256) = hash_file(&path).unwrap();
        ModDownloadResult {
            path,
            size_bytes: bytes.len() as u64,
            sha256: sha256.clone(),
            provenance: crate::mod_download_transport::ModDownloadProvenance {
                provider: "fixture".into(),
                original_url: "https://mods.example.test/payload".into(),
                final_url: "https://cdn.example.test/payload".into(),
                redirect_chain: vec!["https://cdn.example.test/payload".into()],
                payload_hosts: vec!["cdn.example.test".into()],
                retrieved_at_unix_secs: 1,
                declared_size: None,
                content_length: Some(bytes.len() as u64),
                actual_bytes: bytes.len() as u64,
                expected_hash: Some(crate::mod_catalogue::ModCatalogueHash {
                    algorithm: ModCatalogueHashAlgorithm::Sha256,
                    value: sha256,
                }),
                expected_hash_verified: true,
                local_sha256: hex_digest(&Sha256::digest(bytes)),
            },
        }
    }

    #[test]
    fn verified_ips_routes_to_standalone_inspector() {
        let dir = tempfile::tempdir().unwrap();
        let result = inspect_downloaded_mod(ModDownloadedInspectionRequest {
            download: &downloaded(dir.path(), b"PATCHEOF"),
            selected_game: None,
            provider_declared_format: Some("ips"),
        })
        .unwrap();
        assert_eq!(
            result.classification,
            DownloadedModPayloadClassification::StandalonePatch
        );
        assert!(matches!(
            result.inspection,
            Some(DownloadedModInspectionEvidence::StandalonePatch(_))
        ));
        assert_eq!(result.inspection_sha256, result.download.sha256);
        assert!(result.blockers.is_empty());
    }

    #[test]
    fn verified_zip_routes_to_archived_inspector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zip-object");
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("README.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"fixture\n").unwrap();
        writer.finish().unwrap();
        let bytes = fs::read(&path).unwrap();
        let download = downloaded(dir.path(), &bytes);
        let result = inspect_downloaded_mod(ModDownloadedInspectionRequest {
            download: &download,
            selected_game: None,
            provider_declared_format: Some("zip"),
        })
        .unwrap();
        assert_eq!(
            result.classification,
            DownloadedModPayloadClassification::ArchivePackage
        );
        assert!(matches!(
            result.inspection,
            Some(DownloadedModInspectionEvidence::ArchivedPackage(_))
        ));
    }

    #[test]
    fn cache_hash_mismatch_blocks_before_inspection() {
        let dir = tempfile::tempdir().unwrap();
        let mut download = downloaded(dir.path(), b"opaque");
        download.sha256 = "00".repeat(32);
        let error = inspect_downloaded_mod(ModDownloadedInspectionRequest {
            download: &download,
            selected_game: None,
            provider_declared_format: None,
        })
        .unwrap_err();
        assert!(matches!(
            error,
            ModDownloadInspectionFailure::CacheIdentityMismatch { .. }
        ));
    }

    #[test]
    fn executable_bytes_are_never_routed_to_an_executor() {
        let dir = tempfile::tempdir().unwrap();
        let result = inspect_downloaded_mod(ModDownloadedInspectionRequest {
            download: &downloaded(dir.path(), b"MZnot-an-installer"),
            selected_game: None,
            provider_declared_format: Some("zip"),
        })
        .unwrap();
        assert_eq!(
            result.classification,
            DownloadedModPayloadClassification::ExecutableOrScript
        );
        assert!(result.inspection.is_none());
        assert!(!result.blockers.is_empty());
    }

    #[test]
    fn opaque_result_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let download = downloaded(dir.path(), b"opaque");
        let a = inspect_downloaded_mod(ModDownloadedInspectionRequest {
            download: &download,
            selected_game: None,
            provider_declared_format: None,
        })
        .unwrap();
        let b = inspect_downloaded_mod(ModDownloadedInspectionRequest {
            download: &download,
            selected_game: None,
            provider_declared_format: None,
        })
        .unwrap();
        assert_eq!(a, b);
    }
}
