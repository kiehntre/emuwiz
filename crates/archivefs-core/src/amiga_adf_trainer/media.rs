//! The exact ADF image a trainer would be prepared for.
//!
//! Main has no producer of a verified *game* identity for a loose `.adf`
//! (the identity inspector admits Amiga identity only from a WHDLoad slave), and
//! the structural floppy inspector deliberately never asserts one. The sound
//! anchor for a trainer is therefore the exact image bytes: a whole-image
//! SHA-256 of a file that also validated as an AmigaDOS floppy, expressed in the
//! canonical [`IdentityEvidence`] vocabulary so the canonical cheat applicability
//! model consumes it unchanged. Nothing here writes, renames or extends the image.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::amiga_disk::{AmigaDosFamily, inspect_amiga_floppy};
use crate::disk_format::MAX_RAW_FLOPPY_BYTES;
use crate::game_identity::{
    IdentityConfidence, IdentityEvidence, IdentityKind, IdentityProvenance, IdentityStatus,
};

/// Largest image read. Amiga floppies are 880 KiB (DD) or 1.7 MiB (HD).
pub const ADF_MAX_BYTES: u64 = MAX_RAW_FLOPPY_BYTES;

/// Why an image could not be accepted as ADF media.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdfMediaError {
    Io(String),
    NotARegularFile,
    TooLarge {
        len: u64,
        max: u64,
    },
    /// The bytes did not validate as an AmigaDOS floppy; an `.adf` extension
    /// alone is never trusted (Acorn ADFS images share it).
    NotAnAmigaFloppy(String),
    /// The file changed while it was being read.
    ChangedWhileReading,
}

impl std::fmt::Display for AdfMediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) => write!(f, "could not read the image: {m}"),
            Self::NotARegularFile => f.write_str("not a regular file"),
            Self::TooLarge { len, max } => write!(f, "image is {len} bytes; limit is {max}"),
            Self::NotAnAmigaFloppy(m) => write!(f, "not a valid AmigaDOS floppy: {m}"),
            Self::ChangedWhileReading => f.write_str("the image changed while it was read"),
        }
    }
}

impl std::error::Error for AdfMediaError {}

/// TOSEC-style dump flags that mark a disk as *alternate media*, not the
/// original. A trained or cracked image is a different disk, never a cheat
/// record EmuWiz applied, and is never generated, downloaded or merged here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum AlternateMedia {
    /// No alternate-media flag was found. This does not prove the image is original.
    NoFlag,
    Trained,
    Cracked,
    Hacked,
    Modified,
}

impl AlternateMedia {
    #[must_use]
    pub const fn is_alternate(self) -> bool {
        !matches!(self, Self::NoFlag)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoFlag => "no alternate-media flag",
            Self::Trained => "trained disk",
            Self::Cracked => "cracked disk",
            Self::Hacked => "hacked disk",
            Self::Modified => "modified disk",
        }
    }
}

/// Classify a canonical DAT / catalogue name by its TOSEC dump-flag groups
/// (`[t]`, `[t +2]`, `[cr]`, `[h]`, `[m]`). Only exact, recognised flags count;
/// anything else is ignored. Metadata classification only: it never decides that
/// a disk is original, and never turns a flagged disk into a cheat.
#[must_use]
pub fn classify_alternate_media(catalogue_name: &str) -> AlternateMedia {
    let mut found = AlternateMedia::NoFlag;
    let mut rest = catalogue_name;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else { break };
        let flag = after[..close].trim().to_ascii_lowercase();
        let head = flag.split_whitespace().next().unwrap_or("");
        let kind = match head {
            "t" => AlternateMedia::Trained,
            "cr" => AlternateMedia::Cracked,
            "h" => AlternateMedia::Hacked,
            "m" => AlternateMedia::Modified,
            _ => AlternateMedia::NoFlag,
        };
        found = found.max(kind);
        rest = &after[close + 1..];
    }
    found
}

/// A validated, hashed, read-only ADF image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdfMedia {
    pub path: PathBuf,
    pub len: u64,
    /// Lower-case hex SHA-256 of the complete image.
    pub sha256: String,
    pub dos_family: &'static str,
    /// Display only; a volume label is never an identity.
    pub volume_label: Option<String>,
    pub alternate: AlternateMedia,
}

impl AdfMedia {
    /// The canonical identity evidence this image supports: an exact-bytes
    /// whole-image hash and the platform established by structural validation.
    #[must_use]
    pub fn identity_facts(&self) -> Vec<IdentityEvidence> {
        let provenance = |method: &str| IdentityProvenance {
            archive_path: self.path.clone(),
            member_path: None,
            member_index: None,
            method: method.to_string(),
        };
        vec![
            IdentityEvidence {
                kind: IdentityKind::LooseRomSha256,
                status: IdentityStatus::Verified,
                value: Some(self.sha256.clone()),
                confidence: IdentityConfidence::ExactBytes,
                provenance: provenance("whole-image SHA-256 of a validated AmigaDOS floppy"),
                diagnostic: String::new(),
            },
            IdentityEvidence {
                kind: IdentityKind::Platform,
                status: IdentityStatus::Verified,
                value: Some("Amiga".to_string()),
                confidence: IdentityConfidence::StructuredMetadata,
                provenance: provenance("AmigaDOS boot and root block validated"),
                diagnostic: String::new(),
            },
        ]
    }
}

/// Inspect an ADF read-only. `catalogue_name` is the canonical DAT name when the
/// caller has one; it is used only to flag alternate media.
pub fn inspect_adf_media(
    path: &Path,
    catalogue_name: Option<&str>,
) -> Result<AdfMedia, AdfMediaError> {
    let link = fs::symlink_metadata(path).map_err(|e| AdfMediaError::Io(e.to_string()))?;
    if link.file_type().is_symlink() || !link.is_file() {
        return Err(AdfMediaError::NotARegularFile);
    }
    if link.len() > ADF_MAX_BYTES {
        return Err(AdfMediaError::TooLarge {
            len: link.len(),
            max: ADF_MAX_BYTES,
        });
    }
    // Structure first: an extension is never trusted.
    let floppy =
        inspect_amiga_floppy(path).map_err(|e| AdfMediaError::NotAnAmigaFloppy(e.to_string()))?;
    let mut file = fs::File::open(path).map_err(|e| AdfMediaError::Io(e.to_string()))?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(ADF_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| AdfMediaError::Io(e.to_string()))?;
    if bytes.len() as u64 > ADF_MAX_BYTES {
        return Err(AdfMediaError::TooLarge {
            len: bytes.len() as u64,
            max: ADF_MAX_BYTES,
        });
    }
    let after = fs::symlink_metadata(path).map_err(|e| AdfMediaError::Io(e.to_string()))?;
    if bytes.len() as u64 != link.len()
        || after.len() != link.len()
        || after.modified().ok() != link.modified().ok()
    {
        return Err(AdfMediaError::ChangedWhileReading);
    }
    let sha256: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(AdfMedia {
        path: path.to_path_buf(),
        len: bytes.len() as u64,
        sha256,
        dos_family: match floppy.filesystem.family {
            AmigaDosFamily::Ofs => "OFS",
            AmigaDosFamily::Ffs => "FFS",
        },
        volume_label: floppy
            .filesystem
            .volume_label
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty()),
        alternate: catalogue_name.map_or(AlternateMedia::NoFlag, classify_alternate_media),
    })
}
