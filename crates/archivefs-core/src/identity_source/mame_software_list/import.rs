//! Local MAME software-list import and deterministic artifact provenance.

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
pub enum MameSoftwareListImportError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    NotMameSoftwareList {
        path: PathBuf,
        detected_ecosystem: DatEcosystem,
    },
    Parse(ParseError),
}

impl fmt::Display for MameSoftwareListImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "cannot read {}: {error}", path.display()),
            Self::NotMameSoftwareList {
                path,
                detected_ecosystem,
            } => write!(
                f,
                "{} is not a MAME software list (detected: {:?})",
                path.display(),
                detected_ecosystem
            ),
            Self::Parse(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for MameSoftwareListImportError {}

/// One imported local MAME software list. `software_list_name` is the MAME
/// list namespace key, not a resolved EmuWiz platform.
#[derive(Debug, Clone)]
pub struct ImportedMameSoftwareListSource {
    pub software_list_name: Option<String>,
    pub description: Option<String>,
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

impl ImportedMameSoftwareListSource {
    pub fn manifest_line(&self) -> String {
        format!(
            "software list: {}\n  version: {}\n  artifact: {}\n  artifact_name: {}\n  entries: {}\n  roms: {}\n  disks: {}",
            self.software_list_name.as_deref().unwrap_or("unknown"),
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
    let mut buffer = [0u8; 64 * 1024];
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

/// Imports a MAME software list solely when its parsed root element declares
/// one. The delivered filename has no classification authority.
pub fn import_mame_software_list(
    path: &Path,
) -> Result<ImportedMameSoftwareListSource, MameSoftwareListImportError> {
    let artifact_sha256 = sha256_file(path).map_err(|error| MameSoftwareListImportError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let dat = parse_dat_file(path, DatLimits::default())
        .map_err(MameSoftwareListImportError::Parse)?
        .dat;
    if dat.source.ecosystem != DatEcosystem::MAMESoftwareList {
        return Err(MameSoftwareListImportError::NotMameSoftwareList {
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
    Ok(ImportedMameSoftwareListSource {
        software_list_name: dat.source.name.clone(),
        description: dat.source.description.clone(),
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
