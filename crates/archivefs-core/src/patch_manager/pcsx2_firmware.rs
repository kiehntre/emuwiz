//! Local PS2 BIOS verification against already-loaded Redump firmware
//! evidence.
//!
//! # Acquisition is not this module's problem
//!
//! Every function here accepts firmware evidence as an already-parsed
//! `&[FirmwareIdentityRecord]` slice (see
//! [`crate::dat::firmware_evidence`]). This module never knows, and never
//! asks, whether that evidence came from a user-local DAT file loaded once
//! at startup or a future managed Redump-updater cache - both would call
//! [`crate::dat::parsers::parse_dat_file`] then
//! [`crate::dat::firmware_evidence::ps2_bios_evidence_from_dat`] and hand
//! the result here. No HTTP happens anywhere in this module.
//!
//! # What makes a BIOS `Verified`
//!
//! Never a filename, a directory name, an extension, or mere readability -
//! see [`Pcsx2BiosVerificationOutcome::Verified`]'s own doc comment. Only an
//! exact match of size, CRC32, MD5, *and* SHA-1 against one authoritative
//! [`FirmwareIdentityRecord`] produces `Verified`; anything else stays
//! `Unknown`, never a fabricated confidence.
//!
//! # Selection, never a filename guess
//!
//! [`resolve_pcsx2_bios`] prefers PCSX2's own `[Filenames] BIOS = ...`
//! configuration key when the profile's already-parsed global config names
//! one (real PCSX2 Qt behavior: that exact file, in the profile's `bios`
//! directory, is what PCSX2 itself will load - never "whichever `.bin` file
//! sorts first"). Only when no such key is configured does this module fall
//! back to enumerating every `.bin` candidate itself, and even then it never
//! silently picks one: a single candidate is used, and among several it
//! only reports `Verified` when *exactly one* of them matches the supplied
//! evidence - see [`Pcsx2BiosVerificationOutcome::Ambiguous`].

use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::dat::firmware_evidence::FirmwareIdentityRecord;
use crate::identity_source::hashing::Crc32;

use super::pcsx2_local::{
    Pcsx2BiosVerification, Pcsx2Config, Pcsx2GameInspection, Pcsx2GameRequest, Pcsx2Profile,
    inspect_pcsx2_game,
};

const HASH_CHUNK_BYTES: usize = 256 * 1024;
/// A real PS2 BIOS dump is a few MiB; this is a generous ceiling against a
/// misconfigured `bios` directory pointed at something enormous, not a
/// precise real-world bound.
const MAX_BIOS_HASH_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BIOS_CANDIDATES: usize = 64;

/// One local BIOS file whose bytes exactly match an authoritative
/// [`FirmwareIdentityRecord`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2VerifiedBios {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
    /// The exact matched authoritative record - name, description,
    /// provider, and publishing DAT version are all provenance carried
    /// through from here.
    pub record: FirmwareIdentityRecord,
}

/// The result of verifying (or attempting to verify) one PCSX2 profile's
/// selected BIOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pcsx2BiosVerificationOutcome {
    /// The selected BIOS file exists, is a regular non-symlinked file, was
    /// safely read, and its size/CRC32/MD5/SHA-1 all exactly match one
    /// authoritative record.
    Verified(Pcsx2VerifiedBios),
    /// The selected BIOS file exists, is safely readable, and was hashed -
    /// but no supplied record matches. Never called "corrupt" or "invalid"
    /// merely for being absent from the local evidence.
    Unknown { path: PathBuf },
    /// No BIOS file is selected/present at all (no configured filename
    /// resolves to a real file, and no candidate exists in the `bios`
    /// directory).
    Missing,
    /// The selected BIOS path could not be safely read (I/O failure other
    /// than "not found").
    Unreadable { detail: String },
    /// The selected path exists but is a symlink or not a regular file -
    /// refused before any byte is ever read from it.
    Unsafe { path: PathBuf, detail: String },
    /// More than one `.bin` candidate exists, PCSX2's own configuration
    /// does not select one, and more than one candidate matched a distinct
    /// authoritative record - which one PCSX2 will actually load is not
    /// resolvable from local evidence alone, so this never silently reports
    /// either as `Verified`.
    Ambiguous { detail: String },
}

impl Pcsx2BiosVerificationOutcome {
    /// Projects onto the existing, narrower [`Pcsx2BiosVerification`]
    /// status enum every other PCSX2 readiness/inspection path already
    /// reads. `Unsafe` and `Ambiguous` both map to `Unreadable` (in the
    /// codebase's existing sense of "honest uncertainty, not a proven
    /// failure" - see `resolve_pcsx2_native_launch_binding`'s own
    /// precedent) rather than to `Missing`, since a real file genuinely
    /// exists in both cases; `Unknown` maps to `PresentUnverified` for the
    /// same reason it always has.
    pub fn as_legacy_state(&self) -> Pcsx2BiosVerification {
        match self {
            Self::Verified(_) => Pcsx2BiosVerification::Verified,
            Self::Unknown { .. } => Pcsx2BiosVerification::PresentUnverified,
            Self::Missing => Pcsx2BiosVerification::Missing,
            Self::Unreadable { .. } | Self::Unsafe { .. } | Self::Ambiguous { .. } => {
                Pcsx2BiosVerification::Unreadable
            }
        }
    }
}

struct ComputedDigests {
    size_bytes: u64,
    crc32: String,
    md5: String,
    sha1: String,
}

fn hash_bios_file(path: &Path) -> Result<ComputedDigests, String> {
    use md5::Md5;
    use sha1::Sha1;
    use sha1::digest::Digest;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(path)
        .map_err(|error| format!("BIOS file could not be opened safely: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("BIOS file could not be inspected: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("BIOS file changed identity before it could be safely hashed".to_string());
    }
    let size_bytes = metadata.len();
    if size_bytes > MAX_BIOS_HASH_BYTES {
        return Err(format!(
            "BIOS file is {size_bytes} bytes, above the {MAX_BIOS_HASH_BYTES}-byte bound for a \
             PS2 BIOS dump"
        ));
    }

    let mut crc = Crc32::new();
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut buffer = vec![0_u8; HASH_CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("BIOS file could not be read: {error}"))?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        crc.update(chunk);
        md5.update(chunk);
        sha1.update(chunk);
        total += read as u64;
    }
    if total != size_bytes {
        return Err("BIOS file changed size while it was being read".to_string());
    }

    Ok(ComputedDigests {
        size_bytes: total,
        crc32: crc.finish_hex(),
        md5: hex(&md5.finalize()),
        sha1: hex(&sha1.finalize()),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Every evidence record whose size/CRC32/MD5/SHA-1 all agree with
/// `digests`, sorted deterministically by `(name, description)` so that if
/// more than one record in the evidence happens to be identical (the same
/// physical dump catalogued under more than one entry), the same one is
/// always chosen and reported - see the module's own "multiple identical
/// matching records" test.
fn matching_records<'a>(
    digests: &ComputedDigests,
    evidence: &'a [FirmwareIdentityRecord],
) -> Vec<&'a FirmwareIdentityRecord> {
    let mut matches: Vec<&FirmwareIdentityRecord> = evidence
        .iter()
        .filter(|record| {
            record.size_bytes == digests.size_bytes
                && record.crc32.eq_ignore_ascii_case(&digests.crc32)
                && record.md5.eq_ignore_ascii_case(&digests.md5)
                && record.sha1.eq_ignore_ascii_case(&digests.sha1)
        })
        .collect();
    matches.sort_by(|left, right| {
        (&left.name, &left.description).cmp(&(&right.name, &right.description))
    });
    matches
}

/// Verifies exactly one selected local BIOS path against `evidence`. Never
/// follows a symlink, never reads a non-regular file, never reports
/// `Verified` on a partial match.
fn verify_one_bios_path(
    path: &Path,
    evidence: &[FirmwareIdentityRecord],
) -> Pcsx2BiosVerificationOutcome {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Pcsx2BiosVerificationOutcome::Missing;
        }
        Err(error) => {
            return Pcsx2BiosVerificationOutcome::Unreadable {
                detail: error.to_string(),
            };
        }
    };
    if metadata.file_type().is_symlink() {
        return Pcsx2BiosVerificationOutcome::Unsafe {
            path: path.to_path_buf(),
            detail: "BIOS path is a symlink".to_string(),
        };
    }
    if !metadata.is_file() {
        return Pcsx2BiosVerificationOutcome::Unsafe {
            path: path.to_path_buf(),
            detail: "BIOS path is not a regular file".to_string(),
        };
    }
    let digests = match hash_bios_file(path) {
        Ok(digests) => digests,
        Err(detail) => return Pcsx2BiosVerificationOutcome::Unreadable { detail },
    };
    match matching_records(&digests, evidence).first() {
        Some(record) => Pcsx2BiosVerificationOutcome::Verified(Pcsx2VerifiedBios {
            path: path.to_path_buf(),
            size_bytes: digests.size_bytes,
            crc32: digests.crc32,
            md5: digests.md5,
            sha1: digests.sha1,
            record: (*record).clone(),
        }),
        None => Pcsx2BiosVerificationOutcome::Unknown {
            path: path.to_path_buf(),
        },
    }
}

/// Every regular, non-symlinked, `.bin`-named entry directly inside
/// `bios_root` (bounded, sorted for determinism) - candidates only, never a
/// selection.
fn discover_bios_candidates(bios_root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(bios_root) else {
        return Vec::new();
    };
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("bin"))
        })
        .filter(|path| {
            fs::symlink_metadata(path)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        })
        .take(MAX_BIOS_CANDIDATES)
        .collect();
    candidates.sort();
    candidates
}

/// PCSX2's own configured BIOS filename, from an already-parsed
/// `[Filenames] BIOS = ...` key - never guessed from what happens to exist
/// on disk. `Pcsx2Config`'s existing INI parser retains every section/key
/// pair it does not have a dedicated field for as `"section/key"` inside
/// `settings.unknown`, which is where this reads from.
fn configured_bios_filename(global_config: &Pcsx2Config) -> Option<&str> {
    global_config
        .settings
        .unknown
        .get("Filenames/BIOS")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// Resolves and verifies the BIOS a real PCSX2 launch of `profile` would
/// actually load, against `evidence`. Pure/read-only beyond the bounded
/// BIOS-file reads it performs itself; never writes anything, never
/// touches the network.
pub fn resolve_pcsx2_bios(
    bios_root: &Path,
    global_config: &Pcsx2Config,
    evidence: &[FirmwareIdentityRecord],
) -> Pcsx2BiosVerificationOutcome {
    if let Some(name) = configured_bios_filename(global_config) {
        return verify_one_bios_path(&bios_root.join(name), evidence);
    }

    let candidates = discover_bios_candidates(bios_root);
    match candidates.as_slice() {
        [] => Pcsx2BiosVerificationOutcome::Missing,
        [only] => verify_one_bios_path(only, evidence),
        many => {
            let mut verified = Vec::new();
            for candidate in many {
                if let Pcsx2BiosVerificationOutcome::Verified(verified_bios) =
                    verify_one_bios_path(candidate, evidence)
                {
                    verified.push(verified_bios);
                }
            }
            match verified.len() {
                0 => Pcsx2BiosVerificationOutcome::Unknown {
                    path: bios_root.to_path_buf(),
                },
                1 => Pcsx2BiosVerificationOutcome::Verified(
                    verified.into_iter().next().expect("length checked above"),
                ),
                count => Pcsx2BiosVerificationOutcome::Ambiguous {
                    detail: format!(
                        "{count} candidate BIOS files under {} each match a distinct \
                         authoritative record, and PCSX2's own configuration does not select \
                         one",
                        bios_root.display()
                    ),
                },
            }
        }
    }
}

/// [`super::pcsx2_local::inspect_pcsx2_game`], with the BIOS status
/// re-derived from real Redump evidence instead of presence-only detection.
/// The existing inspection function itself is never modified - this only
/// overwrites the two `pub` fields that report BIOS status, using facts
/// (`global_config`) that same inspection already computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2GameInspectionWithFirmware {
    pub inspection: Pcsx2GameInspection,
    pub bios_verification: Pcsx2BiosVerificationOutcome,
}

pub fn inspect_pcsx2_game_with_firmware_evidence(
    profile: &Pcsx2Profile,
    request: &Pcsx2GameRequest,
    firmware_evidence: &[FirmwareIdentityRecord],
) -> Pcsx2GameInspectionWithFirmware {
    let mut inspection = inspect_pcsx2_game(profile, request);
    let bios_root = profile.configuration_path.join("bios");
    let bios_verification =
        resolve_pcsx2_bios(&bios_root, &inspection.global_config, firmware_evidence);
    let legacy_state = bios_verification.as_legacy_state();
    inspection.bios.verification = legacy_state;
    inspection.health.bios = legacy_state;
    Pcsx2GameInspectionWithFirmware {
        inspection,
        bios_verification,
    }
}

#[cfg(test)]
mod tests;
