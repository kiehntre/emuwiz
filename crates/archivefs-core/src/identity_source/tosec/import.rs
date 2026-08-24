//! Local TOSEC DAT import: bounded parser reuse and artifact provenance.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dat::index::DatIndex;
use crate::dat::limits::DatLimits;
use crate::dat::model::{DatEcosystem, ParsedDat};
use crate::dat::parser::ParseError;
use crate::dat::parsers::parse_dat_file;

/// Errors importing a local DAT as a classic TOSEC source.
#[derive(Debug)]
pub enum TosecImportError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    NotTosec {
        path: PathBuf,
        detected_ecosystem: DatEcosystem,
    },
    /// TOSEC-ISO and TOSEC-PIX are deliberately outside the classic-media
    /// v1 authority boundary. This is based only on parsed DAT header text.
    OutOfScope {
        path: PathBuf,
        catalogue_kind: &'static str,
    },
    Parse(ParseError),
}

impl fmt::Display for TosecImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "cannot read {}: {error}", path.display()),
            Self::NotTosec {
                path,
                detected_ecosystem,
            } => write!(
                f,
                "{} does not identify itself as TOSEC (detected: {:?})",
                path.display(),
                detected_ecosystem
            ),
            Self::OutOfScope {
                path,
                catalogue_kind,
            } => write!(
                f,
                "{} is a deferred TOSEC {catalogue_kind} catalogue, not classic-media scope",
                path.display()
            ),
            Self::Parse(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TosecImportError {}

/// One imported classic TOSEC DAT. The file SHA-256 identifies the delivered
/// catalogue artifact; the reused [`DatIndex`] identifies its ROM members.
#[derive(Debug, Clone)]
pub struct ImportedTosecSource {
    pub system_name: String,
    pub upstream_version: Option<String>,
    pub artifact_sha256: String,
    pub artifact_name: String,
    pub artifact_path: PathBuf,
    pub entry_count: usize,
    pub rom_count: usize,
    pub dat: ParsedDat,
    pub index: DatIndex,
}

impl ImportedTosecSource {
    /// Deterministic local provenance summary. No filename-derived system or
    /// version is ever inserted here.
    pub fn manifest_line(&self) -> String {
        format!(
            "{}\n  version: {}\n  artifact: {}\n  entries: {}\n  roms: {}",
            self.system_name,
            self.upstream_version.as_deref().unwrap_or("unknown"),
            self.artifact_sha256,
            self.entry_count,
            self.rom_count,
        )
    }
}

/// Streams the DAT artifact rather than retaining a second giant in-memory
/// copy while it is being parsed/indexed.
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

fn deferred_catalogue_kind(dat: &ParsedDat) -> Option<&'static str> {
    let header = format!(
        "{} {}",
        dat.source.name.as_deref().unwrap_or(""),
        dat.source.description.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase();
    if header.contains("tosec-iso") || header.contains("tosec iso") {
        Some("ISO")
    } else if header.contains("tosec-pix") || header.contains("tosec pix") {
        Some("PIX")
    } else {
        None
    }
}

/// The parser's ecosystem classifier intentionally uses broad metadata
/// detection so ordinary DAT browsing can describe likely TOSEC files. An
/// imported TOSEC authority needs a narrower proof: either an explicit
/// TOSEC author, or a header field whose value itself begins with TOSEC.
/// This rejects incidental text such as `"Not TOSEC"` without trusting a
/// filename or release-pack directory name.
fn header_identifies_tosec_dataset(dat: &ParsedDat) -> bool {
    let author_is_tosec = dat
        .source
        .author
        .as_deref()
        .is_some_and(|author| author.trim().eq_ignore_ascii_case("tosec"));
    let header_leads_with_tosec = [
        dat.source.name.as_deref(),
        dat.source.description.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.trim_start().to_ascii_lowercase().starts_with("tosec"));

    author_is_tosec || header_leads_with_tosec
}

/// Imports one local classic TOSEC DAT. Parsing remains entirely in
/// [`parse_dat_file`] with [`DatLimits::default`]; this adapter only rejects
/// non-TOSEC internal metadata, records the local artifact, and builds the
/// existing collision-preserving [`DatIndex`].
pub fn import_tosec_dat(path: &Path) -> Result<ImportedTosecSource, TosecImportError> {
    let artifact_sha256 = sha256_file(path).map_err(|error| TosecImportError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let outcome = parse_dat_file(path, DatLimits::default()).map_err(TosecImportError::Parse)?;
    let dat = outcome.dat;

    if dat.source.ecosystem != DatEcosystem::Tosec || !header_identifies_tosec_dataset(&dat) {
        return Err(TosecImportError::NotTosec {
            path: path.to_path_buf(),
            detected_ecosystem: dat.source.ecosystem,
        });
    }
    if let Some(catalogue_kind) = deferred_catalogue_kind(&dat) {
        return Err(TosecImportError::OutOfScope {
            path: path.to_path_buf(),
            catalogue_kind,
        });
    }

    let artifact_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let system_name = dat
        .source
        .name
        .clone()
        .unwrap_or_else(|| "TOSEC (unnamed DAT)".to_string());
    let index = DatIndex::build(&dat);

    Ok(ImportedTosecSource {
        system_name,
        upstream_version: dat.source.version.clone(),
        artifact_sha256,
        artifact_name,
        artifact_path: path.to_path_buf(),
        entry_count: dat.source.entry_count,
        rom_count: dat.source.rom_count,
        dat,
        index,
    })
}
