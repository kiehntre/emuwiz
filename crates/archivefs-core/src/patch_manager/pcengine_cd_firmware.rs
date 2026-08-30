//! Local PC Engine CD / TurboGrafx-CD **System Card** firmware
//! verification and readiness.
//!
//! This is firmware *readiness*, deliberately kept separate from media
//! identity: `crate::pcengine_cd_boot_evidence` already decides whether a
//! disc is a PC Engine CD from its IPL boot-record. Nothing in this module
//! ever feeds back into that. It answers only: do we have a *verified*
//! System Card, which class, and is it enough for the selected
//! game/emulator.
//!
//! # What a System Card is
//!
//! A PC Engine CD console needs the CD-ROM² System ROM to boot a disc. It
//! ships as a HuCard ("System Card"), so it is a raw, header-less HuCard
//! ROM image, not a disc image. Revisions:
//!
//! * **CD-ROM² System Card v1.x / v2.x** - the original 64 KiB-buffer BIOS
//!   (Japan) and its US TurboGrafx-CD equivalent. 262144 bytes.
//! * **Super System Card v3.0** - 256 KiB buffer (hence "Super"). Sold as a
//!   HuCard upgrade and built into the PC Engine Duo / TurboDuo. It is
//!   **backward-compatible** with every v1/v2 disc, so a verified v3.0 card
//!   is sufficient for any disc class the emulators below actually run.
//!   262144 bytes.
//! * **Arcade Card Pro / Arcade Card Duo** - RAM add-ons. They add DRAM,
//!   **not** a new BIOS: the Arcade Card Pro's BIOS stays at v3.00. There
//!   is therefore no separate "Arcade Card BIOS" to hash - an Arcade Card
//!   game needs the Super System Card v3.0 BIOS plus RAM the emulator
//!   provides internally.
//! * **Games Express CD Card** - a third-party card for a small set of
//!   unlicensed titles. 32768 bytes.
//!
//! # Hashes verified against two independent sources
//!
//! Every record in [`KNOWN_SYSTEM_CARDS`] carries a size + CRC32 + SHA-1
//! that appears, identically, in **both**:
//!
//! 1. **MAME's CC0-licensed software lists** `hash/pce.xml` and
//!    `hash/tg16.xml` (`<software name="cdsys*"/"scdsys"/"gecd">`), which
//!    give the exact size, CRC32 and SHA-1.
//! 2. The **Beetle PCE / Mednafen PCE** firmware list, as republished by
//!    RetroBIOS and by libretro's `mednafen_pce_libretro.info`
//!    (`firmware*_path = "syscard3.pce"` ... plus the `notes` md5
//!    `38179df8f4ac870017db21ebcbf53114` for `syscard3.pce`), which gives
//!    the same SHA-1 prefixes and the emulator-usage class (which card is
//!    the required one).
//!
//! Filenames (`syscard3.pce`, `syscard2.pce`, ...) are display hints only.
//! Verification is byte-content only - a file called `syscard3.pce` whose
//! bytes do not hash to a known record is **not** verified, and a
//! correctly-hashing file under any other name **is**.
//!
//! # No per-title requirement source, on purpose
//!
//! MAME's `hash/pcecd.xml` does carry a structured
//! `<sharedfeat name="requirement" value="scdsys"/pce:acardpro"/>` per
//! title - but EmuWiz does not ingest that catalogue, and no other
//! trustworthy structured "Arcade Card required / Super CD-ROM² required /
//! plain CD-ROM²" table exists in this build. So this module never guesses
//! a title's requirement: when the requirement is not independently known,
//! readiness stays [`PceCdFirmwareReadiness::VerifiedButRequirementUnknown`]
//! / [`PceCdFirmwareReadiness::EmulatorRequirementUnknown`] rather than a
//! fabricated `Ready`.

use std::path::{Path, PathBuf};

use crate::dat::firmware_evidence::{ComputedFirmwareDigests, hash_firmware_file};

const HASH_CHUNK_BYTES: usize = 256 * 1024;

/// A System Card is 262144 bytes (32768 for the Games Express card). This
/// ceiling is ~4x the largest real card - comfortably above a
/// hypothetically padded dump, far below anything worth parsing. There is
/// no standard copier header for `.pce` HuCard images, so no normalisation
/// is attempted here.
pub const MAX_SYSTEM_CARD_BYTES: u64 = 1024 * 1024;

/// The most directory entries [`inventory_system_cards`] will look at.
pub const MAX_SYSTEM_CARD_CANDIDATES: usize = 64;

/// Region a System Card dump belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemCardRegion {
    Japan,
    UsTurboGrafx,
}

/// The capability class of a verified System Card. Ordered by capability
/// via [`SystemCardClass::rank`]; `SuperSystemCardV3` is the superset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemCardClass {
    /// CD-ROM² System Card v1.x.
    CdRom2V1,
    /// CD-ROM² System Card v2.x.
    CdRom2V2,
    /// Super System Card v3.0 - backward compatible with every v1/v2 disc,
    /// and the BIOS Arcade Card titles use.
    SuperSystemCardV3,
    /// Games Express CD Card - a separate third-party card, not a superset
    /// or subset of the Hudson cards.
    GamesExpress,
}

impl SystemCardClass {
    /// A capability rank for "best card present" selection. Higher is more
    /// capable. `GamesExpress` ranks below the Hudson cards because it does
    /// not run licensed CD-ROM² titles.
    pub fn rank(self) -> u8 {
        match self {
            Self::SuperSystemCardV3 => 3,
            Self::CdRom2V2 => 2,
            Self::CdRom2V1 => 1,
            Self::GamesExpress => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CdRom2V1 => "CD-ROM\u{b2} System Card v1",
            Self::CdRom2V2 => "CD-ROM\u{b2} System Card v2",
            Self::SuperSystemCardV3 => "Super System Card v3.0",
            Self::GamesExpress => "Games Express CD Card",
        }
    }
}

/// One authoritative System Card dump: exactly the fields both sources
/// agree on, plus a stable EmuWiz-local id. Never hashed against anything
/// itself - carried as reference evidence for [`classify_system_card_file`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownSystemCard {
    /// Stable EmuWiz-local identifier - not a filename, not a hash.
    pub canonical_id: &'static str,
    pub label: &'static str,
    pub class: SystemCardClass,
    pub region: SystemCardRegion,
    pub size_bytes: u64,
    /// Lowercase hex, 8 chars.
    pub crc32: &'static str,
    /// Lowercase hex, 40 chars.
    pub sha1: &'static str,
}

/// The verified known-good System Card table. Every SHA-1 here is
/// corroborated by MAME `hash/pce.xml` / `hash/tg16.xml` **and** the
/// Beetle PCE / Mednafen PCE firmware list (RetroBIOS / libretro
/// `mednafen_pce_libretro.info`). See the module docs.
pub const KNOWN_SYSTEM_CARDS: &[KnownSystemCard] = &[
    KnownSystemCard {
        canonical_id: "pce-cdrom2-system-card-v1-jp",
        label: "CD-ROM\u{b2} System Card v1.0 (Japan)",
        class: SystemCardClass::CdRom2V1,
        region: SystemCardRegion::Japan,
        size_bytes: 262_144,
        crc32: "3f9f95a4",
        sha1: "a39a66da7de6ba94ab84d04eef7afeec7d4ee66a",
    },
    KnownSystemCard {
        canonical_id: "pce-cdrom2-system-card-v2-jp",
        label: "CD-ROM\u{b2} System Card v2.1 (Japan)",
        class: SystemCardClass::CdRom2V2,
        region: SystemCardRegion::Japan,
        size_bytes: 262_144,
        crc32: "283b74e0",
        sha1: "88da02e2503f7c32810f5d93a34849d470742b6d",
    },
    KnownSystemCard {
        canonical_id: "tg16cd-system-card-v2-us",
        label: "TurboGrafx-CD System Card v2.0 (USA)",
        class: SystemCardClass::CdRom2V2,
        region: SystemCardRegion::UsTurboGrafx,
        size_bytes: 262_144,
        crc32: "ff2a5ec3",
        sha1: "2bea3dac98f84b2f2f469fa77ea720b8770d598d",
    },
    KnownSystemCard {
        canonical_id: "pce-super-system-card-v3-jp",
        label: "CD-ROM\u{b2} Super System Card v3.0 (Japan)",
        class: SystemCardClass::SuperSystemCardV3,
        region: SystemCardRegion::Japan,
        size_bytes: 262_144,
        crc32: "6d9a73ef",
        sha1: "79f5ff55dd10187c7fd7b8daab0b3ffbd1f56a2c",
    },
    KnownSystemCard {
        canonical_id: "tg16cd-super-system-card-v3-us",
        label: "TurboGrafx-CD Super System Card v3.0 (USA)",
        class: SystemCardClass::SuperSystemCardV3,
        region: SystemCardRegion::UsTurboGrafx,
        size_bytes: 262_144,
        crc32: "2b5b75fe",
        sha1: "d02611d99921986147c753df14c7349b31d71950",
    },
    KnownSystemCard {
        canonical_id: "pce-games-express-cd-card-jp",
        label: "Games Express CD Card (Japan)",
        class: SystemCardClass::GamesExpress,
        region: SystemCardRegion::Japan,
        size_bytes: 32_768,
        crc32: "51a12d90",
        sha1: "014881a959e045e00f4db8f52955200865d40280",
    },
];

/// One local file whose bytes exactly match a [`KnownSystemCard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSystemCard {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub crc32: String,
    pub sha1: String,
    /// The matched authoritative record - id/label/class/region are all
    /// provenance carried through from here.
    pub card: KnownSystemCard,
}

/// The result of classifying (or attempting to classify) one candidate
/// System Card file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PceCdFirmwareOutcome {
    /// The file exists, is a regular non-symlinked file, was hashed, and
    /// its size + CRC32 + SHA-1 all match exactly one [`KnownSystemCard`].
    Verified(VerifiedSystemCard),
    /// The file exists, was safely read and hashed, but its bytes match no
    /// known record. Never called corrupt or invalid merely for being
    /// absent from this table.
    Unknown { path: PathBuf },
    /// No file exists at the given path.
    Missing,
    /// The path could not be safely read (I/O failure other than
    /// not-found, or the file was above [`MAX_SYSTEM_CARD_BYTES`]).
    Unreadable { detail: String },
    /// The path exists but is a symlink or not a regular file - refused
    /// before any byte is read.
    Unsafe { path: PathBuf, detail: String },
}

/// Hashes one candidate file (bounded, refusing symlinks and non-regular
/// files) and classifies it against [`KNOWN_SYSTEM_CARDS`]. Reuses
/// [`crate::dat::firmware_evidence::hash_firmware_file`] - no new hashing
/// or normalisation is added here.
pub fn classify_system_card_file(path: &Path) -> PceCdFirmwareOutcome {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PceCdFirmwareOutcome::Missing;
        }
        Err(error) => {
            return PceCdFirmwareOutcome::Unreadable {
                detail: error.to_string(),
            };
        }
    };
    if metadata.file_type().is_symlink() {
        return PceCdFirmwareOutcome::Unsafe {
            path: path.to_path_buf(),
            detail: "System Card path is a symlink".to_string(),
        };
    }
    if !metadata.is_file() {
        return PceCdFirmwareOutcome::Unsafe {
            path: path.to_path_buf(),
            detail: "System Card path is not a regular file".to_string(),
        };
    }
    let digests = match hash_firmware_file(path, MAX_SYSTEM_CARD_BYTES, HASH_CHUNK_BYTES) {
        Ok(digests) => digests,
        Err(detail) => return PceCdFirmwareOutcome::Unreadable { detail },
    };
    match match_known_system_card(&digests) {
        Some(card) => PceCdFirmwareOutcome::Verified(VerifiedSystemCard {
            path: path.to_path_buf(),
            size_bytes: digests.size_bytes,
            crc32: digests.crc32,
            sha1: digests.sha1,
            card: *card,
        }),
        None => PceCdFirmwareOutcome::Unknown {
            path: path.to_path_buf(),
        },
    }
}

/// The [`KnownSystemCard`] whose size, CRC32 **and** SHA-1 all agree with
/// `digests`, if any. All three must match - a size/CRC32 coincidence
/// without the SHA-1 is never accepted.
pub fn match_known_system_card(
    digests: &ComputedFirmwareDigests,
) -> Option<&'static KnownSystemCard> {
    KNOWN_SYSTEM_CARDS.iter().find(|card| {
        card.size_bytes == digests.size_bytes
            && card.crc32.eq_ignore_ascii_case(&digests.crc32)
            && card.sha1.eq_ignore_ascii_case(&digests.sha1)
    })
}

/// Extensions a raw System Card HuCard image is plausibly stored under.
const SYSTEM_CARD_EXTENSIONS: &[&str] = &["pce", "bin", "rom"];

/// Scans one directory for candidate System Card files (bounded entry
/// count, plausible extensions only) and classifies each. Directory order
/// is not relied on: results are sorted by path so a caller's "best card"
/// pick is deterministic.
pub fn inventory_system_cards(directory: &Path) -> Vec<(PathBuf, PceCdFirmwareOutcome)> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .take(MAX_SYSTEM_CARD_CANDIDATES)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| {
                    SYSTEM_CARD_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
                })
                .unwrap_or(false)
        })
        .collect();
    candidates.sort();
    candidates
        .into_iter()
        .map(|path| {
            let outcome = classify_system_card_file(&path);
            (path, outcome)
        })
        .collect()
}

/// The most capable verified System Card class across a set of classified
/// files, or `None` if none verified.
pub fn best_verified_card_class(
    outcomes: &[(PathBuf, PceCdFirmwareOutcome)],
) -> Option<SystemCardClass> {
    outcomes
        .iter()
        .filter_map(|(_, outcome)| match outcome {
            PceCdFirmwareOutcome::Verified(verified) => Some(verified.card.class),
            _ => None,
        })
        .max_by_key(|class| class.rank())
}

/// Whether any candidate file was present and hashed but matched no known
/// record - a "present but unverified" signal.
pub fn any_unverified_candidate_present(outcomes: &[(PathBuf, PceCdFirmwareOutcome)]) -> bool {
    outcomes
        .iter()
        .any(|(_, outcome)| matches!(outcome, PceCdFirmwareOutcome::Unknown { .. }))
}

/// How the selected emulator handles the PC Engine CD System Card. Supplied
/// by a caller that knows its target - this module never guesses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PceCdEmulatorFirmwarePolicy {
    /// The emulator loads an external System Card ROM (Mednafen / Beetle
    /// PCE / MiSTer): a verified card of a sufficient class is required.
    RequiresExternalSystemCard,
    /// The emulator ships or embeds its own System Card - no external file
    /// is a launch blocker.
    EmbedsSystemCard,
    /// Whether the selected emulator needs an external System Card has not
    /// been established.
    Unknown,
}

/// A title's System Card requirement, only ever from a source the caller
/// independently trusts. This build has no per-title requirement table, so
/// callers pass [`Self::Unknown`] unless they have their own evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PceCdTitleRequirement {
    /// The title needs at least the Super System Card v3.0 (a Super
    /// CD-ROM² or Arcade Card game).
    SuperSystemCard,
    /// The title is a plain CD-ROM² game - any Hudson System Card runs it.
    AnyCdRom2,
    /// Not independently known.
    Unknown,
}

/// The compact, factual firmware-readiness verdict - the vocabulary Doctor
/// / launch surface. Deliberately distinct from
/// [`crate::launch::readiness::FirmwareReadiness`], which it projects onto
/// via [`crate::launch::readiness::pcengine_cd_firmware_readiness`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PceCdFirmwareReadiness {
    /// The selected emulator ships its own System Card; no external file is
    /// needed.
    EmulatorProvidesFirmware,
    /// A verified System Card is present and its class is provably enough
    /// for the resolved title requirement.
    VerifiedSufficient { class: SystemCardClass },
    /// A verified System Card is present, but its class cannot be proven
    /// sufficient because this build has no per-title requirement source.
    VerifiedButRequirementUnknown { class: SystemCardClass },
    /// A candidate firmware file is present but its bytes match no verified
    /// record.
    CandidatePresentHashUnknown,
    /// No verified System Card was found and the selected emulator needs
    /// one.
    NoVerifiedFirmware,
    /// Whether the selected emulator needs an external System Card was not
    /// established, and no verified card is present.
    EmulatorRequirementUnknown,
}

impl PceCdFirmwareReadiness {
    /// A one-line, factual status string. Never tells the user to rename a
    /// file to a magic name.
    pub fn summary(self) -> String {
        match self {
            Self::EmulatorProvidesFirmware => {
                "Selected emulator does not require an external System Card".to_string()
            }
            Self::VerifiedSufficient { class } => {
                format!("Verified {} present", class.label())
            }
            Self::VerifiedButRequirementUnknown { class } => format!(
                "Verified {} present, but this title's System Card requirement is unknown",
                class.label()
            ),
            Self::CandidatePresentHashUnknown => {
                "A candidate System Card file is present but its hash is not recognised".to_string()
            }
            Self::NoVerifiedFirmware => "No verified PC Engine CD System Card found".to_string(),
            Self::EmulatorRequirementUnknown => {
                "Firmware requirement for the selected emulator is unknown".to_string()
            }
        }
    }
}

fn class_is_sufficient_for(class: SystemCardClass, title: PceCdTitleRequirement) -> bool {
    match class {
        // v3.0 is backward compatible with every disc class the supported
        // emulators run, so it is always sufficient once present.
        SystemCardClass::SuperSystemCardV3 => true,
        // A v1/v2/GamesExpress card is only provably enough when the title
        // is independently known to be a plain CD-ROM² game.
        SystemCardClass::CdRom2V1 | SystemCardClass::CdRom2V2 | SystemCardClass::GamesExpress => {
            matches!(title, PceCdTitleRequirement::AnyCdRom2)
        }
    }
}

/// Resolves the firmware-readiness verdict from the classified inventory
/// plus the caller's emulator policy and (usually `Unknown`) title
/// requirement. Never returns a "ready" verdict from a guessed per-title
/// requirement - `Unknown` beats a guessed pass.
pub fn resolve_pcengine_cd_firmware(
    inventory: &[(PathBuf, PceCdFirmwareOutcome)],
    emulator: PceCdEmulatorFirmwarePolicy,
    title: PceCdTitleRequirement,
) -> PceCdFirmwareReadiness {
    if emulator == PceCdEmulatorFirmwarePolicy::EmbedsSystemCard {
        return PceCdFirmwareReadiness::EmulatorProvidesFirmware;
    }

    if let Some(class) = best_verified_card_class(inventory) {
        return if class_is_sufficient_for(class, title) {
            PceCdFirmwareReadiness::VerifiedSufficient { class }
        } else {
            PceCdFirmwareReadiness::VerifiedButRequirementUnknown { class }
        };
    }

    if any_unverified_candidate_present(inventory) {
        return PceCdFirmwareReadiness::CandidatePresentHashUnknown;
    }

    match emulator {
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard => {
            PceCdFirmwareReadiness::NoVerifiedFirmware
        }
        // Unknown emulator policy + nothing verified: do not call firmware
        // "missing" when we cannot prove the emulator needs it.
        PceCdEmulatorFirmwarePolicy::EmbedsSystemCard | PceCdEmulatorFirmwarePolicy::Unknown => {
            PceCdFirmwareReadiness::EmulatorRequirementUnknown
        }
    }
}

#[cfg(test)]
mod tests;
