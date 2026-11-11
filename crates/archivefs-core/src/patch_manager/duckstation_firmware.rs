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
    ComputedFirmwareDigests, FirmwareIdentityRecord, hash_firmware_file, hash_open_firmware_file,
    matching_firmware_records,
};
use crate::safe_read::TrustedRoots;

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
// One-shot value returned by value from a single verification call and
// immediately matched; it is never held in a collection, so the ~280-byte
// `Verified` payload is not a memory-footprint concern and boxing it would
// only add indirection to every match arm.
#[allow(clippy::large_enum_variant)]
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
    /// required region could not be determined from a verified PS1 serial -
    /// which file DuckStation will actually load is not resolvable from
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
    let configured = settings
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
        .map(PathBuf::from);
    match configured {
        // DuckStation joins a relative folder to its data root, never to the
        // process working directory.
        Some(directory) if directory.is_relative() => profile.configuration_path.join(directory),
        Some(directory) => directory,
        None => profile.bios_path.clone(),
    }
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
    /// No BIOS file is named for the required slot, so DuckStation searches
    /// its BIOS folder itself (upstream `BIOS::FindBIOSImageInDirectory`).
    AutoSearch {
        directory: PathBuf,
        required_region: Option<DuckStationRegion>,
    },
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
                    // An empty Path<Region> makes DuckStation search its
                    // BIOS folder for this region.
                    ConfiguredBiosSelection::AutoSearch {
                        directory,
                        required_region: Some(required),
                    }
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
        // Nothing names a file: DuckStation's default is to pick one from
        // its BIOS folder, which is not the same as "no BIOS".
        None => ConfiguredBiosSelection::AutoSearch {
            directory,
            required_region: region,
        },
    }
}

/// The trusted BIOS tree for DuckStation's configured BIOS folder, chosen by
/// [`trusted_bios_root`] (usually the folder above it: a `bios/psx` folder of
/// links to `bios/*.bin` is a common layout). Canonical, so it names real
/// directories. Empty - every BIOS link refused - when the folder cannot be
/// resolved.
fn trusted_bios_tree(directory: &Path) -> TrustedRoots {
    trusted_bios_tree_for_home(directory, std::env::var_os("HOME"))
}

/// [`trusted_bios_tree`] with the `HOME` value passed in, so the fail-closed
/// branch is testable without changing the process environment.
fn trusted_bios_tree_for_home(directory: &Path, home: Option<std::ffi::OsString>) -> TrustedRoots {
    let Ok(canonical) = std::fs::canonicalize(directory) else {
        return TrustedRoots::none();
    };
    // Without a resolvable home folder the home guard cannot be applied, so
    // no tree is trusted and every BIOS link is refused (fail closed).
    let Some(home) = home
        .filter(|home| !home.is_empty())
        .and_then(|home| std::fs::canonicalize(home).ok())
    else {
        return TrustedRoots::none();
    };
    match trusted_bios_root(&canonical, Some(&home)) {
        Some(root) => TrustedRoots::from_paths([root]),
        None => TrustedRoots::none(),
    }
}

/// The folder above is trusted only when it is itself at least
/// [`MIN_TRUSTED_PARENT_DEPTH`] levels deep, so a BIOS folder such as `/mnt/x`
/// or `~/bios` never trusts `/mnt` or `/home`; such a folder trusts only
/// itself. A parent that is the user's home folder (or holds it, as
/// `/var/home/user` does on image-based systems) or a removable-media folder
/// such as `/run/media/user` is never trusted either. A BIOS folder at the
/// filesystem root, or one level below it, trusts nothing, so every link
/// there is refused.
fn trusted_bios_root<'a>(canonical: &'a Path, home: Option<&Path>) -> Option<&'a Path> {
    let depth = |path: &Path| {
        path.components()
            .filter(|component| matches!(component, std::path::Component::Normal(_)))
            .count()
    };
    if depth(canonical) < 2 {
        return None;
    }
    // A folder is broad when it is the home folder or holds it, or when it is
    // a mount root or above one: `/mnt/<drive>` (`/var/mnt/<drive>` once
    // canonical on image-based systems, where `/mnt` links there), or
    // `/media/<user>/<drive>` and `/run/media/<user>/<drive>` (per user).
    let broad = |folder: &Path| {
        home.is_some_and(|home| home.starts_with(folder))
            || [
                ("/run/media", 2),
                ("/media", 2),
                ("/mnt", 1),
                ("/var/mnt", 1),
            ]
            .iter()
            .any(|(media, levels)| {
                folder.starts_with(media) && depth(folder) <= depth(Path::new(media)) + levels
            })
    };
    match canonical.parent() {
        Some(parent) if depth(parent) >= MIN_TRUSTED_PARENT_DEPTH && !broad(parent) => Some(parent),
        // The BIOS folder itself is never trusted when it is broad either.
        _ if broad(canonical) => None,
        _ => Some(canonical),
    }
}

/// See [`trusted_bios_root`]: `/mnt/usbdrive/[BIOS]/bios` qualifies (four),
/// `/home/user` does not (two).
const MIN_TRUSTED_PARENT_DEPTH: usize = 3;

/// Opens a BIOS symlink only when it is a relative link whose resolved target
/// is a regular file inside `trusted`. Only the link itself must be relative:
/// a further link in a chain may be absolute, but the fully resolved target
/// is still checked against `trusted`, so no chain leaves the tree. Via
/// [`crate::safe_read::open_bounded_read`]: broken links and loops fail to
/// resolve, special files and targets outside the tree are refused, and the
/// target is opened by a no-follow descriptor walk so a component swapped for
/// a link after the check is refused rather than followed. The caller hashes
/// the returned descriptor, never the pathname again.
fn open_contained_bios_link(path: &Path, trusted: &TrustedRoots) -> Result<std::fs::File, String> {
    let target = std::fs::read_link(path).map_err(|error| error.to_string())?;
    if target.is_absolute() {
        return Err(
            "BIOS link is absolute; only relative links inside the BIOS folder are followed"
                .to_string(),
        );
    }
    // The link's own directory must be named without `..` for the safe open.
    let parent = path.parent().ok_or("BIOS link has no parent folder")?;
    let name = path.file_name().ok_or("BIOS link has no file name")?;
    let parent = std::fs::canonicalize(parent).map_err(|error| error.to_string())?;
    crate::safe_read::open_bounded_read(&parent.join(name), trusted)
        .map(crate::safe_read::SafeFile::into_file)
        .map_err(|refusal| format!("BIOS link refused: {}", refusal.detail()))
}

/// DuckStation's BIOS image size; upstream also accepts PS2/PS3 BIOS sizes,
/// which Redump PS1 evidence can never verify.
const PS1_BIOS_SIZE: u64 = 512 * 1024;
/// Bounds on the folder search: entries listed, and candidate files hashed.
const MAX_AUTO_SEARCH_ENTRIES: usize = 4096;
const MAX_AUTO_SEARCH_HASHED: usize = 128;

/// The console region a Redump PS1 BIOS record is for, from its machine name
/// (`ps-41a` USA, `ps-41e` Europe, `ps-22j` / `psone-43j` Japan; a trailing
/// `(w)`/`(v)` variant tag is ignored). Any other suffix gives `None`.
fn redump_ps1_bios_region(record: &FirmwareIdentityRecord) -> Option<DuckStationRegion> {
    if record.system != crate::dat::firmware_evidence::FirmwareSystem::PlayStation {
        return None;
    }
    let name = record.name.trim();
    let base = name.split('(').next().unwrap_or(name).trim();
    if !(base.starts_with("ps-") || base.starts_with("psone-")) {
        return None;
    }
    match base.chars().last()?.to_ascii_lowercase() {
        'a' => Some(DuckStationRegion::NtscU),
        'e' => Some(DuckStationRegion::Pal),
        'j' => Some(DuckStationRegion::NtscJ),
        _ => None,
    }
}

/// Models DuckStation's automatic BIOS pick for a folder without guessing a
/// file: `Verified` only when the folder holds a regular 512 KiB file (or a
/// relative link to one inside the trusted BIOS tree; see
/// [`open_contained_bios_link`]) whose bytes exactly match a Redump PS1 BIOS record for the
/// game's required region. DuckStation prefers a known BIOS of the matching
/// region over any unknown or other-region image, so with such a file present
/// it loads a known BIOS for this region (the reported path is the verified
/// file EmuWiz found). Image-sized files that do not verify give `Unknown`
/// (DuckStation may still boot, possibly with the wrong BIOS); none at all
/// gives `Missing`, which is what DuckStation itself reports.
fn auto_search_bios(
    directory: &Path,
    evidence: &[FirmwareIdentityRecord],
    required_region: Option<DuckStationRegion>,
) -> DuckStationBiosVerificationOutcome {
    match std::fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return DuckStationBiosVerificationOutcome::Unsafe {
                path: directory.to_path_buf(),
                detail: "DuckStation's BIOS search folder is not a real folder".to_string(),
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return DuckStationBiosVerificationOutcome::Missing;
        }
        Err(error) => {
            return DuckStationBiosVerificationOutcome::Unreadable {
                detail: error.to_string(),
            };
        }
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            return DuckStationBiosVerificationOutcome::Unreadable {
                detail: error.to_string(),
            };
        }
    };
    let mut candidates: Vec<PathBuf> = entries
        .take(MAX_AUTO_SEARCH_ENTRIES)
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            std::fs::symlink_metadata(path).is_ok_and(|metadata| {
                (metadata.is_file() && metadata.len() == PS1_BIOS_SIZE)
                    // Only a pre-filter so broken, oversized and special links
                    // never use the hash budget; the size and type that count
                    // are re-checked on the safely opened descriptor.
                    // Absolute link targets are refused later anyway, so they
                    // are dropped here without a stat. A relative link (or a
                    // chain) is still stat-ed before containment is checked.
                    || (metadata.file_type().is_symlink()
                        && std::fs::read_link(path).is_ok_and(|target| target.is_relative())
                        && std::fs::metadata(path).is_ok_and(|target| {
                            target.is_file() && target.len() == PS1_BIOS_SIZE
                        }))
            })
        })
        .collect();
    if candidates.is_empty() {
        return DuckStationBiosVerificationOutcome::Missing;
    }
    candidates.sort();
    let trusted = trusted_bios_tree(directory);
    if let Some(required) = required_region {
        for path in candidates.iter().take(MAX_AUTO_SEARCH_HASHED) {
            let outcome = verify_one_bios_path(path, evidence, Some(required), &trusted);
            if let DuckStationBiosVerificationOutcome::Verified(verified) = &outcome
                && redump_ps1_bios_region(&verified.record) == Some(required)
            {
                return outcome;
            }
        }
    }
    DuckStationBiosVerificationOutcome::Unknown {
        path: directory.to_path_buf(),
    }
}

/// Verifies exactly one selected local BIOS path against `evidence`. Follows
/// only a contained relative link ([`open_contained_bios_link`]) whose target
/// is exactly a PS1 BIOS image in size, never reads a non-regular file, never
/// reports `Verified` on a partial match.
fn verify_one_bios_path(
    path: &Path,
    evidence: &[FirmwareIdentityRecord],
    required_region: Option<DuckStationRegion>,
    trusted: &TrustedRoots,
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
    let digests: ComputedFirmwareDigests = if metadata.file_type().is_symlink() {
        match open_contained_bios_link(path, trusted) {
            Ok(file)
                if file.metadata().map(|metadata| metadata.len()).ok() != Some(PS1_BIOS_SIZE) =>
            {
                return DuckStationBiosVerificationOutcome::Unknown {
                    path: path.to_path_buf(),
                };
            }
            Ok(file) => match hash_open_firmware_file(file, PS1_BIOS_SIZE, HASH_CHUNK_BYTES) {
                Ok(digests) => digests,
                Err(detail) => {
                    return DuckStationBiosVerificationOutcome::Unreadable { detail };
                }
            },
            Err(detail) => {
                return DuckStationBiosVerificationOutcome::Unsafe {
                    path: path.to_path_buf(),
                    detail,
                };
            }
        }
    } else if !metadata.is_file() {
        return DuckStationBiosVerificationOutcome::Unsafe {
            path: path.to_path_buf(),
            detail: "BIOS path is not a regular file".to_string(),
        };
    } else {
        match hash_firmware_file(path, MAX_BIOS_HASH_BYTES, HASH_CHUNK_BYTES) {
            Ok(digests) => digests,
            Err(detail) => return DuckStationBiosVerificationOutcome::Unreadable { detail },
        }
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
        ConfiguredBiosSelection::Ambiguous(detail) => {
            DuckStationBiosVerificationOutcome::Ambiguous { detail }
        }
        ConfiguredBiosSelection::Conflict(detail) => {
            DuckStationBiosVerificationOutcome::Conflict { detail }
        }
        ConfiguredBiosSelection::One {
            path,
            required_region,
        } => {
            let trusted = path
                .parent()
                .map_or_else(TrustedRoots::none, trusted_bios_tree);
            verify_one_bios_path(&path, evidence, required_region, &trusted)
        }
        ConfiguredBiosSelection::AutoSearch {
            directory,
            required_region,
        } => auto_search_bios(&directory, evidence, required_region),
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
