//! Local MAME `-listxml` import and artifact provenance.

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
pub enum MameListxmlImportError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    NotMameListxml {
        path: PathBuf,
        detected_ecosystem: DatEcosystem,
    },
    Parse(ParseError),
}

impl fmt::Display for MameListxmlImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "cannot read {}: {error}", path.display()),
            Self::NotMameListxml {
                path,
                detected_ecosystem,
            } => write!(
                f,
                "{} is not MAME listxml (detected: {detected_ecosystem:?})",
                path.display()
            ),
            Self::Parse(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for MameListxmlImportError {}

/// One locally imported MAME `-listxml` dump. Machine shortnames/descriptions
/// are retained as provenance only; they are not a canonical platform
/// decision - see [`crate::dat::identity::gather_dat_platform_evidence`].
#[derive(Debug, Clone)]
pub struct ImportedMameListxmlSource {
    pub artifact_sha256: String,
    pub artifact_name: String,
    pub upstream_version: Option<String>,
    pub dat: ParsedDat,
    pub index: DatIndex,
    pub disk_index: DatDiskIndex,
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

/// Imports a local `mame -listxml` dump through the shared parser. Accepted
/// identity comes entirely from the parsed `<mame>` root, never the
/// delivered filename.
pub fn import_mame_listxml(
    path: &Path,
) -> Result<ImportedMameListxmlSource, MameListxmlImportError> {
    let artifact_sha256 = sha256_file(path).map_err(|error| MameListxmlImportError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let dat = parse_dat_file(path, DatLimits::default())
        .map_err(MameListxmlImportError::Parse)?
        .dat;
    if dat.source.ecosystem != DatEcosystem::MAMEArcade {
        return Err(MameListxmlImportError::NotMameListxml {
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
    Ok(ImportedMameListxmlSource {
        artifact_sha256,
        artifact_name,
        upstream_version: None,
        dat,
        index,
        disk_index,
    })
}
