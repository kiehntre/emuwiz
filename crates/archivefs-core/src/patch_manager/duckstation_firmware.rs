//! Local PS1 BIOS verification against already-loaded Redump firmware
//! evidence, plus DuckStation's own region-specific BIOS configuration.
//!
//! # Acquisition is not this module's problem
//!
//! Every function here accepts firmware evidence as an already-parsed
//! `&[FirmwareIdentityRecord]` slice (see [`crate::dat::firmware_evidence`]).
//! This module never knows, and never asks, whether that evidence came from
//! a user-local DAT file or the managed Redump BIOS provider
//! (`crate::dat::updates`) - both would call
//! [`crate::dat::firmware_evidence::redump_bios_evidence_from_dat`] with
//! [`crate::dat::firmware_evidence::FirmwareSystem::PlayStation`] and hand
//! the result here. No HTTP happens anywhere in this module.
//!
//! # What makes a BIOS `Verified`
//!
//! Never a filename, a directory name, an extension, mere readability, or a
//! model string alone - see [`DuckStationBiosVerificationOutcome::Verified`]'s
//! own doc comment. Only an exact match of size, CRC32, MD5, *and* SHA-1
//! against one authoritative [`FirmwareIdentityRecord`] produces `Verified`;
//! anything else stays `Unknown`, never a fabricated confidence. Hashing
//! itself reuses [`crate::dat::firmware_evidence::hash_firmware_file`]/
//! [`crate::dat::firmware_evidence::matching_firmware_records`] - the exact
//! same primitives PCSX2 BIOS verification uses, not a second hashing
//! framework.
//!
//! # DuckStation's own configured BIOS, never a filename guess
//!
//! [`resolve_duckstation_bios`] reads the BIOS DuckStation is actually
//! configured to load: DuckStation supports both a single generic BIOS
//! (`[BIOS] BIOSFilename = ...`, already a first-class field on
//! [`DuckStationSettings`]) and, in its documented `settings.ini` format,
//! per-region overrides (`[BIOS] PathNTSCU/PathNTSCJ/PathPAL = ...`) -
//! DuckStation's existing INI parser already retains any key it has no
//! dedicated field for verbatim in `settings.unknown` as
//! `"section.key"` (lowercased), which is where the region-specific keys
//! are read from here, exactly the way `patch_manager::pcsx2_firmware`
//! reads PCSX2's `Filenames/BIOS` key. This module adds no INI parsing of
//! its own and does not modify `duckstation_local.rs`'s parser.
//!
//! # Region selection, never guessed
//!
//! The required region is derived only from an already-verified PS1 serial
//! (`DuckStationSerialMapping::VerifiedPs1Serial` - never
//! `EmulatorMetadataOnly`/`ConflictingEmulatorMetadata`, matching every
//! other adapter's "verified evidence only" convention) via the documented
//! Sony region-prefix scheme. When the required region is unknown and more
//! than one region-specific BIOS path is configured, or when the global and
//! per-game configuration disagree about the effective single BIOS file,
//! this honestly reports [`DuckStationBiosVerificationOutcome::Ambiguous`]/
//! [`DuckStationBiosVerificationOutcome::Conflict`] rather than guessing.

use std::path::{Path, PathBuf};

use crate::dat::firmware_evidence::{
    ComputedFirmwareDigests, FirmwareIdentityRecord, hash_firmware_file, matching_firmware_records,
};

use super::duckstation_local::{
    DuckStationBiosInventory, DuckStationBiosState, DuckStationConfigInspection,
    DuckStationGameInspection, DuckStationGameRequest, DuckStationProfile,
    DuckStationSerialMapping, DuckStationSettings, inspect_duckstation_game,
};

const HASH_CHUNK_BYTES: usize = 256 * 1024;
/// A real PS1 BIOS dump is 512 KiB; this is a generous ceiling against a
/// misconfigured BIOS directory pointed at something enormous, not a
/// precise real-world bound.
const MAX_BIOS_HASH_BYTES: u64 = 16 * 1024 * 1024;

/// A PS1 console region, derived only from a verified disc serial - never
/// from a filename, a BIOS model string, or an unverified emulator-reported
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckStationRegion {
    NtscU,
    NtscJ,
    Pal,
}

impl DuckStationRegion {
    /// The lowercased `settings.unknown` key DuckStation's own
    /// `[BIOS] Path<Region>` setting is retained under for this region.
    fn config_key(self) -> &'static str {
        match self {
            Self::NtscU => "bios.pathntscu",
            Self::NtscJ => "bios.pathntscj",
            Self::Pal => "bios.pathpal",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NtscU => "NTSC-U",
            Self::NtscJ => "NTSC-J",
            Self::Pal => "PAL",
        }
    }
}

/// Documented Sony PS1 serial-prefix region scheme - the same mapping
/// `patch_manager::pcsx2_identity`'s own (private) PS2 equivalent uses,
/// re-derived here rather than imported since that function is private to
/// an unrelated adapter module and this is a small, fixed, publicly
/// documented table, not a subsystem worth sharing a dependency edge for.
fn duckstation_region_for_serial(serial: &str) -> Option<DuckStationRegion> {
    let prefix: String = serial
        .chars()
        .filter(char::is_ascii_alphabetic)
        .take(4)
        .collect::<String>()
        .to_ascii_uppercase();
    match prefix.as_str() {
        "SLUS" | "SCUS" => Some(DuckStationRegion::NtscU),
        "SLES" | "SCES" => Some(DuckStationRegion::Pal),
        "SLPS" | "SCPS" | "SLPM" | "SCPM" => Some(DuckStationRegion::NtscJ),
        _ => None,
    }
}

/// The required region for `inspection`'s game, only when it comes from a
/// genuinely verified PS1 serial - `EmulatorMetadataOnly`/
/// `ConflictingEmulatorMetadata` never produce a required region here,
/// exactly the same "verified evidence only" rule every other identity
/// consumer in this codebase already follows.
fn required_region(inspection: &DuckStationGameInspection) -> Option<DuckStationRegion> {
    if inspection.serial_mapping != DuckStationSerialMapping::VerifiedPs1Serial {
        return None;
    }
    inspection
        .serial
        .as_deref()
        .and_then(duckstation_region_for_serial)
}

/// One local BIOS file whose bytes exactly match an authoritative
/// [`FirmwareIdentityRecord`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationVerifiedBios {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
    /// The exact matched authoritative record - name, description,
    /// provider, and publishing DAT version are all provenance carried
    /// through from here.
    pub record: FirmwareIdentityRecord,
    /// The region this verification was performed for, when one was
    /// determined from a verified PS1 serial. `None` when no region was
    /// required to resolve a single configured BIOS (e.g. only one generic
    /// BIOS is configured at all).
    pub required_region: Option<&'static str>,
}

/// The result of verifying (or attempting to verify) DuckStation's
/// configured PS1 BIOS for one profile/game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuckStationBiosVerificationOutcome {
    /// The selected BIOS file exists, is a regular non-symlinked file, was
    /// safely read, and its size/CRC32/MD5/SHA-1 all exactly match one
    /// authoritative record.
    Verified(DuckStationVerifiedBios),
    /// The selected BIOS file exists, is safely readable, and was hashed -
    /// but no supplied record matches. Never called "corrupt" or "invalid"
    /// merely for being absent from the local evidence.
    Unknown { path: PathBuf },
    /// No BIOS file is selected/present at all for the resolved
    /// requirement (no configured filename resolves to a real file).
    Missing,
    /// The selected BIOS path could not be safely read (I/O failure other
    /// than "not found").
    Unreadable { detail: String },
    /// The selected path exists but is a symlink or not a regular file -
    /// refused before any byte is ever read from it.
    Unsafe { path: PathBuf, detail: String },
    /// More than one region-specific BIOS path is configured and the
    /// required region could not be determined from a verified PS1 serial
    /// - which file DuckStation will actually load is not resolvable from
    /// local evidence alone, so this never silently picks one.
    Ambiguous { detail: String },
    /// DuckStation's global configuration and the per-game configuration
    /// override name two different BIOS files for the same resolved slot.
    /// A per-game `.ini` override genuinely changes which file DuckStation
    /// loads for this specific title, so silently trusting the global
    /// value would be wrong exactly as often as trusting the override -
    /// this is reported honestly instead of guessed.
    Conflict { detail: String },
}

impl DuckStationBiosVerificationOutcome {
    /// Projects onto the existing, narrower [`DuckStationBiosState`] status
    /// enum every other DuckStation readiness/inspection path already
    /// reads. `Unsafe`, `Ambiguous`, and `Conflict` all map to `Unknown`
    /// (honest uncertainty, not a proven absence - a real file or a real
    /// configuration genuinely exists in every one of these cases, so
    /// `Missing` would be dishonest); `Unreadable` maps to `Unknown` for
    /// the same reason `Pcsx2BiosVerificationOutcome` maps its own
    /// `Unreadable`/`Unsafe`/`Ambiguous` to `Unreadable` rather than
    /// `Missing`.
    pub fn as_legacy_state(&self) -> DuckStationBiosState {
        match self {
            Self::Verified(_) => DuckStationBiosState::Verified,
            Self::Unknown { .. } => DuckStationBiosState::PresentUnverified,
            Self::Missing => DuckStationBiosState::Missing,
            Self::Unreadable { .. }
            | Self::Unsafe { .. }
            | Self::Ambiguous { .. }
            | Self::Conflict { .. } => DuckStationBiosState::Unknown,
        }
    }
}

/// Resolves a raw configured BIOS value (a bare filename or a full path,
/// DuckStation accepts either) against the profile's BIOS directory.
fn resolve_candidate_path(value: &str, directory: &Path) -> PathBuf {
    let candidate = Path::new(value);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        directory.join(candidate)
    }
}

/// DuckStation's own configured BIOS search directory - `[BIOS]
/// BIOSDirectory`/`bios_directory` (already a dedicated field), then the
/// documented `SearchDirectory` key (retained generically in
/// `settings.unknown` since `duckstation_local.rs`'s parser has no
/// dedicated field for it), then the profile's own default `bios/`
/// directory - never fabricated.
fn configured_bios_directory(
    profile: &DuckStationProfile,
    settings: &DuckStationSettings,
) -> PathBuf {
    settings
        .bios_directory
        .as_deref()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            settings
                .unknown
                .get("bios.searchdirectory")
                .map(String::as_str)
                .filter(|value| !value.is_empty())
        })
        .map(PathBuf::from)
        .unwrap_or_else(|| profile.bios_path.clone())
}

/// One config's generic single-BIOS selection (`[BIOS] BIOSFilename`),
/// resolved to a full path - `None` when that config does not set one.
fn generic_bios_path(config: &DuckStationConfigInspection, directory: &Path) -> Option<PathBuf> {
    config
        .settings
        .bios_filename
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_candidate_path(value, directory))
}

/// What DuckStation's configuration resolves to for the (region-aware)
/// required BIOS slot, before any file is opened or hashed. Selection
/// policy only - see [`resolve_duckstation_bios`] for what happens next.
enum ConfiguredBiosSelection {
    Missing,
    One {
        path: PathBuf,
        required_region: Option<DuckStationRegion>,
    },
    Ambiguous(String),
    Conflict(String),
}

fn resolve_configured_bios_selection(
    profile: &DuckStationProfile,
    inspection: &DuckStationGameInspection,
) -> ConfiguredBiosSelection {
    let global = &inspection.global_config.settings;
    let directory = configured_bios_directory(profile, global);
    let region = required_region(inspection);

    let region_candidates: Vec<(DuckStationRegion, PathBuf)> = [
        DuckStationRegion::NtscU,
        DuckStationRegion::NtscJ,
        DuckStationRegion::Pal,
    ]
    .into_iter()
    .filter_map(|candidate_region| {
        global
            .unknown
            .get(candidate_region.config_key())
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| (candidate_region, resolve_candidate_path(value, &directory)))
    })
    .collect();

    let global_generic = generic_bios_path(&inspection.global_config, &directory);
    let per_game_generic = inspection
        .per_game_config
        .as_ref()
        .and_then(|config| generic_bios_path(config, &directory));

    if let (Some(global_path), Some(per_game_path)) = (&global_generic, &per_game_generic)
        && global_path != per_game_path
    {
        return ConfiguredBiosSelection::Conflict(format!(
            "global configuration selects {} but the per-game configuration overrides it to \
             {} - which file DuckStation will actually load for this title cannot be resolved \
             from local evidence alone",
            global_path.display(),
            per_game_path.display()
        ));
    }
    // A per-game override is what DuckStation genuinely loads for this
    // title when it disagrees only by being the *only* one set; when both
    // are set and equal, either is fine to use.
    let effective_generic = per_game_generic.or(global_generic);

    if !region_candidates.is_empty() {
        return match region {
            Some(required) => {
                if let Some((_, path)) = region_candidates
                    .iter()
                    .find(|(candidate_region, _)| *candidate_region == required)
                {
                    ConfiguredBiosSelection::One {
                        path: path.clone(),
                        required_region: Some(required),
                    }
                } else if let Some(path) = effective_generic {
                    // No region-specific override for the required region -
                    // DuckStation itself falls back to the generic BIOS
                    // file when a region-specific path is unset.
                    ConfiguredBiosSelection::One {
                        path,
                        required_region: Some(required),
                    }
                } else {
                    ConfiguredBiosSelection::Missing
                }
            }
            None => ConfiguredBiosSelection::Ambiguous(format!(
                "{} region-specific BIOS path(s) are configured but the required region could \
                 not be determined from a verified PS1 serial",
                region_candidates.len()
            )),
        };
    }

    match effective_generic {
        Some(path) => ConfiguredBiosSelection::One {
            path,
            required_region: region,
        },
        None => ConfiguredBiosSelection::Missing,
    }
}

/// Verifies exactly one selected local BIOS path against `evidence`. Never
/// follows a symlink, never reads a non-regular file, never reports
/// `Verified` on a partial match.
fn verify_one_bios_path(
    path: &Path,
    evidence: &[FirmwareIdentityRecord],
    required_region: Option<DuckStationRegion>,
) -> DuckStationBiosVerificationOutcome {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return DuckStationBiosVerificationOutcome::Missing;
        }
        Err(error) => {
            return DuckStationBiosVerificationOutcome::Unreadable {
                detail: error.to_string(),
            };
        }
    };
    if metadata.file_type().is_symlink() {
        return DuckStationBiosVerificationOutcome::Unsafe {
            path: path.to_path_buf(),
            detail: "BIOS path is a symlink".to_string(),
        };
    }
    if !metadata.is_file() {
        return DuckStationBiosVerificationOutcome::Unsafe {
            path: path.to_path_buf(),
            detail: "BIOS path is not a regular file".to_string(),
        };
    }
    let digests: ComputedFirmwareDigests =
        match hash_firmware_file(path, MAX_BIOS_HASH_BYTES, HASH_CHUNK_BYTES) {
            Ok(digests) => digests,
            Err(detail) => return DuckStationBiosVerificationOutcome::Unreadable { detail },
        };
    match matching_firmware_records(&digests, evidence).first() {
        Some(record) => DuckStationBiosVerificationOutcome::Verified(DuckStationVerifiedBios {
            path: path.to_path_buf(),
            size_bytes: digests.size_bytes,
            crc32: digests.crc32,
            md5: digests.md5,
            sha1: digests.sha1,
            record: (*record).clone(),
            required_region: required_region.map(DuckStationRegion::label),
        }),
        None => DuckStationBiosVerificationOutcome::Unknown {
            path: path.to_path_buf(),
        },
    }
}

/// Resolves and verifies the BIOS a real DuckStation launch of `profile`
/// for `inspection`'s game would actually load, against `evidence`. Pure/
/// read-only beyond the bounded BIOS-file read it performs itself; never
/// writes anything, never touches the network.
pub fn resolve_duckstation_bios(
    profile: &DuckStationProfile,
    inspection: &DuckStationGameInspection,
    evidence: &[FirmwareIdentityRecord],
) -> DuckStationBiosVerificationOutcome {
    match resolve_configured_bios_selection(profile, inspection) {
        ConfiguredBiosSelection::Missing => DuckStationBiosVerificationOutcome::Missing,
        ConfiguredBiosSelection::Ambiguous(detail) => {
            DuckStationBiosVerificationOutcome::Ambiguous { detail }
        }
        ConfiguredBiosSelection::Conflict(detail) => {
            DuckStationBiosVerificationOutcome::Conflict { detail }
        }
        ConfiguredBiosSelection::One {
            path,
            required_region,
        } => verify_one_bios_path(&path, evidence, required_region),
    }
}

/// [`super::duckstation_local::inspect_duckstation_game`], with the BIOS
/// status re-derived from real Redump evidence instead of presence-only
/// detection. The existing inspection function itself is never modified -
/// this only overwrites the two `pub` fields that report BIOS status,
/// using facts (`global_config`/`per_game_config`) that same inspection
/// already computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationGameInspectionWithFirmware {
    pub inspection: DuckStationGameInspection,
    pub bios_verification: DuckStationBiosVerificationOutcome,
}

pub fn inspect_duckstation_game_with_firmware_evidence(
    profile: &DuckStationProfile,
    request: &DuckStationGameRequest,
    firmware_evidence: &[FirmwareIdentityRecord],
) -> DuckStationGameInspectionWithFirmware {
    let mut inspection = inspect_duckstation_game(profile, request);
    let bios_verification = resolve_duckstation_bios(profile, &inspection, firmware_evidence);
    let legacy_state = bios_verification.as_legacy_state();
    inspection.bios = DuckStationBiosInventory {
        configured_path: inspection.bios.configured_path.clone(),
        state: legacy_state,
    };
    inspection.health.bios = legacy_state;
    DuckStationGameInspectionWithFirmware {
        inspection,
        bios_verification,
    }
}

#[cfg(test)]
mod tests;
