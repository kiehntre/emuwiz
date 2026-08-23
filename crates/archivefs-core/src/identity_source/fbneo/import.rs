//! Local FBNeo DAT import and artifact provenance.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dat::index::{DatDiskIndex, DatIndex};
use crate::dat::limits::DatLimits;
use crate::dat::model::{DatEcosystem, ParsedDat};
use crate::dat::parser::ParseError;
use crate::dat::parsers::parse_dat_file;

#[derive(Debug)]
pub enum FBNeoImportError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    NotFBNeo {
        path: PathBuf,
        detected_ecosystem: DatEcosystem,
    },
    Parse(ParseError),
}

impl fmt::Display for FBNeoImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "cannot read {}: {error}", path.display()),
            Self::NotFBNeo {
                path,
                detected_ecosystem,
            } => write!(
                f,
                "{} does not identify itself as FBNeo (detected: {detected_ecosystem:?})",
                path.display()
            ),
            Self::Parse(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for FBNeoImportError {}

/// One locally imported FBNeo DAT. Header text is retained as provenance only;
/// it is not a canonical platform decision.
#[derive(Debug, Clone)]
pub struct ImportedFBNeoSource {
    pub source_name: Option<String>,
    pub upstream_version: Option<String>,
    pub artifact_sha256: String,
    pub artifact_name: String,
    pub artifact_path: PathBuf,
    pub entry_count: usize,
    pub rom_count: usize,
    pub disk_count: usize,
    pub dat: ParsedDat,
    pub index: DatIndex,
    pub disk_index: DatDiskIndex,
}

impl ImportedFBNeoSource {
    pub fn manifest_line(&self) -> String {
        format!(
            "source: {}\n  version: {}\n  artifact: {}\n  artifact_name: {}\n  entries: {}\n  roms: {}\n  disks: {}",
            self.source_name.as_deref().unwrap_or("unknown"),
            self.upstream_version.as_deref().unwrap_or("unknown"),
            self.artifact_sha256,
            self.artifact_name,
            self.entry_count,
            self.rom_count,
            self.disk_count,
        )
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn disk_count(dat: &ParsedDat) -> usize {
    dat.games
        .iter()
        .map(|game| {
            game.disks.len()
                + game
                    .parts
                    .iter()
                    .map(|part| {
                        part.disk_areas
                            .iter()
                            .map(|area| area.disks.len())
                            .sum::<usize>()
                    })
                    .sum::<usize>()
        })
        .sum()
}

/// Imports an explicitly FBNeo-branded local DAT through the shared parser.
/// The accepted source identity comes entirely from parsed header metadata,
/// never the delivered filename.
pub fn import_fbneo_dat(path: &Path) -> Result<ImportedFBNeoSource, FBNeoImportError> {
    let artifact_sha256 = sha256_file(path).map_err(|error| FBNeoImportError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let dat = parse_dat_file(path, DatLimits::default())
        .map_err(FBNeoImportError::Parse)?
        .dat;
    if dat.source.ecosystem != DatEcosystem::FBNeo {
        return Err(FBNeoImportError::NotFBNeo {
            path: path.to_path_buf(),
            detected_ecosystem: dat.source.ecosystem,
        });
    }
    let artifact_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let index = DatIndex::build(&dat);
    let disk_index = DatDiskIndex::build(&dat);
    Ok(ImportedFBNeoSource {
        source_name: dat.source.name.clone(),
        upstream_version: dat.source.version.clone(),
        artifact_sha256,
        artifact_name,
        artifact_path: path.to_path_buf(),
        entry_count: dat.source.entry_count,
        rom_count: dat.source.rom_count,
        disk_count: disk_count(&dat),
        dat,
        index,
        disk_index,
    })
}
