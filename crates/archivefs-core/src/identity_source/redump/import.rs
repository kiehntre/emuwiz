//! Local Redump DAT import and deterministic artifact provenance.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dat::index::{DatDiskIndex, DatIndex};
use crate::dat::limits::DatLimits;
use crate::dat::model::{DatEcosystem, ParsedDat};
use crate::dat::parser::ParseError;
use crate::dat::parsers::parse_dat_file;

/// Errors importing a local DAT as a direct Redump source.
#[derive(Debug)]
pub enum RedumpImportError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    NotRedump {
        path: PathBuf,
        detected_ecosystem: DatEcosystem,
    },
    Parse(ParseError),
}

impl fmt::Display for RedumpImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "cannot read {}: {error}", path.display()),
            Self::NotRedump {
                path,
                detected_ecosystem,
            } => write!(
                f,
                "{} does not identify itself as Redump (detected: {:?})",
                path.display(),
                detected_ecosystem
            ),
            Self::Parse(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RedumpImportError {}

/// One imported direct Redump DAT.  `system_name` is exactly the DAT header
/// name when available; it is never invented from the file name.
#[derive(Debug, Clone)]
pub struct ImportedRedumpSource {
    pub system_name: Option<String>,
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

impl ImportedRedumpSource {
    /// Stable, human-readable provenance for a supplied DAT artifact.
    pub fn manifest_line(&self) -> String {
        format!(
            "system: {}\n  version: {}\n  artifact: {}\n  artifact_name: {}\n  entries: {}\n  roms: {}\n  disks: {}",
            self.system_name.as_deref().unwrap_or("unknown"),
            self.upstream_version.as_deref().unwrap_or("unknown"),
            self.artifact_sha256,
            self.artifact_name,
            self.entry_count,
            self.rom_count,
            self.disk_count,
        )
    }
}

/// Hashes a DAT using fixed-size reads so a large catalogue is never loaded
/// merely to establish the provenance of the delivered artifact.
fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
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

/// Imports one local Redump DAT through the shared DAT parser and indexes.
/// Rejection is based solely on parsed header metadata, never the path.
pub fn import_redump_dat(path: &Path) -> Result<ImportedRedumpSource, RedumpImportError> {
    let artifact_sha256 = sha256_file(path).map_err(|error| RedumpImportError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let dat = parse_dat_file(path, DatLimits::default())
        .map_err(RedumpImportError::Parse)?
        .dat;
    if dat.source.ecosystem != DatEcosystem::Redump {
        return Err(RedumpImportError::NotRedump {
            path: path.to_path_buf(),
            detected_ecosystem: dat.source.ecosystem,
        });
    }
    let artifact_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let disk_count = disk_count(&dat);
    let index = DatIndex::build(&dat);
    let disk_index = DatDiskIndex::build(&dat);
    Ok(ImportedRedumpSource {
        system_name: dat.source.name.clone(),
        upstream_version: dat.source.version.clone(),
        artifact_sha256,
        artifact_name,
        artifact_path: path.to_path_buf(),
        entry_count: dat.source.entry_count,
        rom_count: dat.source.rom_count,
        disk_count,
        dat,
        index,
        disk_index,
    })
}
