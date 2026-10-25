//! Read-only, per-platform BIOS / firmware information view.
//!
//! Pure projection over evidence the caller already holds: EmuWiz's built-in
//! requirement table (via [`crate::bios_projection::plan_for_emulator`]), the
//! last explicitly inspected BIOS folder, an optional caller-supplied
//! RetroBIOS metadata snapshot, and the installed-emulator inventory. Nothing
//! here reads the filesystem, fetches metadata, or changes a file or setting.
//!
//! Authority is preserved: built-in rows keep the planner's own match result,
//! RetroBIOS rows go through [`crate::retrobios_provider::join_bios_evidence`]
//! (which never overrides authoritative `Verified`, `Missing` or
//! `NotRequired`), and a filename match is never reported as verified.

use crate::bios_projection::{
    self, BiosContentClass, BiosEvidence, BiosMasterInventory, BiosMatchStatus,
    BiosProjectionMethod, BiosProjectionTarget, BiosRequirement, BiosTargetState,
};
use crate::emulator_inventory::EmulatorInventory;
use crate::launch::readiness::FirmwareReadiness;
use crate::retrobios_provider::{
    RetroBiosCanonicalTarget, RetroBiosHashes, RetroBiosMetadataItem, RetroBiosMetadataSnapshot,
    RetroBiosRequiredness, RetroBiosSnapshotProvenance, join_bios_evidence,
};

/// One selectable platform. The emulator and RetroBIOS system lists are a
/// closed, reviewed table: nothing is inferred from a platform name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwarePlatform {
    pub key: &'static str,
    pub label: &'static str,
    /// Emulators from [`bios_projection::SUPPORTED_ADAPTERS`] whose built-in
    /// requirements belong to this platform alone.
    pub adapters: &'static [&'static str],
    /// Exact RetroBIOS `system` slugs for this platform.
    pub retrobios_systems: &'static [&'static str],
}

pub const FIRMWARE_PLATFORMS: &[FirmwarePlatform] = &[
    FirmwarePlatform {
        key: "ps1",
        label: "Sony PlayStation",
        adapters: &["DuckStation"],
        retrobios_systems: &["sony-playstation"],
    },
    FirmwarePlatform {
        key: "ps2",
        label: "Sony PlayStation 2",
        adapters: &["PCSX2"],
        retrobios_systems: &["sony-playstation-2"],
    },
    FirmwarePlatform {
        key: "ps3",
        label: "Sony PlayStation 3",
        adapters: &["RPCS3"],
        retrobios_systems: &["sony-playstation-3"],
    },
    FirmwarePlatform {
        key: "psp",
        label: "Sony PSP",
        adapters: &["PPSSPP"],
        retrobios_systems: &["sony-psp"],
    },
    FirmwarePlatform {
        key: "gamecube",
        label: "Nintendo GameCube",
        adapters: &["Dolphin"],
        retrobios_systems: &["nintendo-gamecube"],
    },
    FirmwarePlatform {
        key: "3ds",
        label: "Nintendo 3DS",
        adapters: &["Azahar"],
        retrobios_systems: &[],
    },
    FirmwarePlatform {
        key: "switch",
        label: "Nintendo Switch",
        adapters: &["Ryubing"],
        retrobios_systems: &[],
    },
    FirmwarePlatform {
        key: "saturn",
        label: "Sega Saturn",
        adapters: &[],
        retrobios_systems: &["sega-saturn"],
    },
    FirmwarePlatform {
        key: "dreamcast",
        label: "Sega Dreamcast",
        adapters: &[],
        retrobios_systems: &["sega-dreamcast"],
    },
    FirmwarePlatform {
        key: "xbox",
        label: "Microsoft Xbox",
        adapters: &["xemu"],
        retrobios_systems: &["microsoft-xbox"],
    },
    FirmwarePlatform {
        key: "amiga",
        label: "Commodore Amiga",
        adapters: &["FS-UAE"],
        retrobios_systems: &["amiga"],
    },
    FirmwarePlatform {
        key: "atari-st",
        label: "Atari ST",
        adapters: &["Hatari"],
        retrobios_systems: &["atari-st"],
    },
    FirmwarePlatform {
        key: "atari-8bit",
        label: "Atari 8-bit / 5200",
        adapters: &["Atari800"],
        retrobios_systems: &[],
    },
    FirmwarePlatform {
        key: "cpc",
        label: "Amstrad CPC",
        adapters: &["Caprice32"],
        retrobios_systems: &[],
    },
    FirmwarePlatform {
        key: "arcade",
        label: "Arcade (MAME)",
        adapters: &["MAME"],
        retrobios_systems: &["arcade"],
    },
];

pub fn firmware_platform(key: &str) -> Option<&'static FirmwarePlatform> {
    FIRMWARE_PLATFORMS
        .iter()
        .find(|platform| platform.key == key)
}

/// Where a row's requirement came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirmwareRowSource {
    /// EmuWiz's built-in emulator requirement table (authoritative).
    BuiltIn,
    /// A supplementary RetroBIOS snapshot record.
    RetroBios { source_path: String },
}

/// Requiredness as actually stated by the row's source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareRequiredness {
    Required,
    Optional,
    HleFallback,
    NotRequired,
    /// The source does not say. EmuWiz's built-in table lists expected files
    /// but does not encode required versus optional.
    NotStated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareRowState {
    /// Exact content match against an expected hash.
    Verified,
    /// A file with an accepted name exists; its content is not proven.
    FilenameOnly,
    /// A file with an accepted name exists but its hash or size differs.
    Incompatible,
    /// More than one local candidate; the planner will not choose.
    MultipleCandidates,
    /// Not found in the inspected BIOS folder.
    Missing,
    NotRequired,
    /// Installed through the emulator itself or held as external system
    /// data/keys; not a loose BIOS file EmuWiz can inspect.
    ExternalInstall,
    /// No BIOS folder has been inspected in this session.
    NotInspected,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareCandidate {
    pub relative_path: std::path::PathBuf,
    pub filename: String,
    pub size_bytes: u64,
    /// Present only when the inspection hashed the file.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareRow {
    pub source: FirmwareRowSource,
    /// Emulator (and core, for RetroBIOS rows) exactly as the source names it.
    pub emulator: String,
    pub name: String,
    pub accepted_filenames: Vec<String>,
    pub requiredness: FirmwareRequiredness,
    pub content_class: BiosContentClass,
    pub region: Option<String>,
    pub revision: Option<String>,
    pub version: Option<String>,
    pub expected_size: Option<u64>,
    pub expected_hashes: RetroBiosHashes,
    pub candidates: Vec<FirmwareCandidate>,
    pub state: FirmwareRowState,
    pub explanation: String,
    /// The EmuWiz emulator this row's evidence applies to, from an explicit
    /// mapping only. `None` means the mapping is not reviewed.
    pub mapped_emulator: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmulatorInstallation {
    /// The emulator inventory found at least one installation.
    Installed { versions: Vec<String> },
    /// The inventory ran and found none.
    NotFound,
    /// The inventory has not been scanned in this session.
    NotScanned,
    /// The emulator inventory does not track this emulator.
    NotTracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorFirmwareUse {
    pub emulator: &'static str,
    pub installation: EmulatorInstallation,
    /// Filenames whose content is verified for this emulator.
    pub verified: Vec<String>,
    /// Required rows that are missing or incompatible.
    pub blocking: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformFirmwareView {
    pub platform: &'static FirmwarePlatform,
    pub inventory_root: Option<std::path::PathBuf>,
    pub provenance: Option<RetroBiosSnapshotProvenance>,
    pub rows: Vec<FirmwareRow>,
    pub emulators: Vec<EmulatorFirmwareUse>,
}

/// Builds the view. `inventory` is the last explicit BIOS folder inspection
/// (or `None`), `snapshot` an already parsed RetroBIOS snapshot, and
/// `installed` the emulator inventory if one was scanned.
pub fn platform_firmware_view(
    platform: &'static FirmwarePlatform,
    inventory: Option<&BiosMasterInventory>,
    snapshot: Option<&RetroBiosMetadataSnapshot>,
    installed: Option<&EmulatorInventory>,
) -> PlatformFirmwareView {
    let empty = BiosMasterInventory {
        root: std::path::PathBuf::new(),
        entries: Vec::new(),
        warnings: Vec::new(),
    };
    let mut rows = Vec::new();
    let mut built_in = Vec::new();
    for emulator in platform.adapters {
        let plan = bios_projection::plan_for_emulator(inventory.unwrap_or(&empty), emulator);
        for requirement in &plan.requirements {
            let candidates: Vec<FirmwareCandidate> = inventory
                .map(|inventory| {
                    inventory
                        .entries
                        .iter()
                        .filter(|entry| {
                            requirement
                                .expected_filenames
                                .iter()
                                .any(|name| entry.filename.eq_ignore_ascii_case(name))
                        })
                        .map(candidate)
                        .collect()
                })
                .unwrap_or_default();
            let match_status = planner_status(requirement, &candidates);
            let (state, explanation) = built_in_state(requirement, match_status, inventory);
            built_in.push((requirement.clone(), state));
            rows.push(FirmwareRow {
                source: FirmwareRowSource::BuiltIn,
                emulator: (*emulator).to_string(),
                name: requirement.name.clone(),
                accepted_filenames: requirement.expected_filenames.clone(),
                requiredness: if requirement.method == BiosProjectionMethod::NoBiosRequired {
                    FirmwareRequiredness::NotRequired
                } else {
                    FirmwareRequiredness::NotStated
                },
                content_class: requirement.content_class,
                region: None,
                revision: None,
                version: None,
                expected_size: None,
                expected_hashes: RetroBiosHashes {
                    sha256: requirement.expected_sha256.clone(),
                    ..RetroBiosHashes::default()
                },
                candidates,
                state,
                explanation,
                mapped_emulator: bios_projection::SUPPORTED_ADAPTERS
                    .iter()
                    .copied()
                    .find(|name| name == emulator),
            });
        }
    }
    if let Some(snapshot) = snapshot {
        for item in snapshot
            .items
            .iter()
            .filter(|item| platform.retrobios_systems.contains(&item.system.as_str()))
        {
            rows.push(retrobios_row(item, snapshot, inventory, &built_in));
        }
    }
    let emulators = emulator_uses(&rows, installed);
    PlatformFirmwareView {
        platform,
        inventory_root: inventory.map(|inventory| inventory.root.clone()),
        provenance: snapshot.map(|snapshot| snapshot.provenance.clone()),
        rows,
        emulators,
    }
}

fn candidate(entry: &BiosEvidence) -> FirmwareCandidate {
    FirmwareCandidate {
        relative_path: entry.relative_path.clone(),
        filename: entry.filename.clone(),
        size_bytes: entry.size_bytes,
        sha256: entry.sha256.clone(),
    }
}

/// The planner keeps the inventory's own `match_status` on the evidence it
/// records, so its per-requirement decision is mirrored here exactly.
fn planner_status(
    requirement: &BiosRequirement,
    candidates: &[FirmwareCandidate],
) -> BiosMatchStatus {
    if requirement.method == BiosProjectionMethod::NoBiosRequired
        || (candidates.is_empty() && requirement.expected_filenames.is_empty())
    {
        BiosMatchStatus::Unknown
    } else if candidates.is_empty() {
        BiosMatchStatus::Missing
    } else if candidates.len() > 1 {
        BiosMatchStatus::Ambiguous
    } else if let Some(expected) = &requirement.expected_sha256 {
        if candidates[0].sha256.as_deref() == Some(expected.as_str()) {
            BiosMatchStatus::VerifiedMatch
        } else {
            BiosMatchStatus::HashMismatch
        }
    } else {
        BiosMatchStatus::FilenameOnly
    }
}

fn built_in_state(
    requirement: &BiosRequirement,
    status: BiosMatchStatus,
    inventory: Option<&BiosMasterInventory>,
) -> (FirmwareRowState, String) {
    match requirement.method {
        BiosProjectionMethod::NoBiosRequired => {
            return (
                FirmwareRowState::NotRequired,
                "EmuWiz's emulator table records that no conventional BIOS is required.".into(),
            );
        }
        BiosProjectionMethod::FirmwareInstallRequired
        | BiosProjectionMethod::ExternalSystemData => {
            return (
                FirmwareRowState::ExternalInstall,
                "Installed through the emulator itself or held as its own system data; EmuWiz does not inspect it as a loose BIOS file.".into(),
            );
        }
        BiosProjectionMethod::RomsetDependency => {
            return (
                FirmwareRowState::Unknown,
                "BIOS and device ROMs are governed by MAME's ROM-set dependency model, not a BIOS folder.".into(),
            );
        }
        _ => {}
    }
    if inventory.is_none() {
        return (
            FirmwareRowState::NotInspected,
            "No BIOS folder has been inspected yet, so EmuWiz cannot say whether this file is present.".into(),
        );
    }
    match status {
        BiosMatchStatus::VerifiedMatch => (
            FirmwareRowState::Verified,
            "The local file matches the expected hash.".into(),
        ),
        BiosMatchStatus::FilenameOnly => (
            FirmwareRowState::FilenameOnly,
            "A file with an accepted name was found. EmuWiz's table has no expected hash for it, so its content is not verified.".into(),
        ),
        BiosMatchStatus::HashMismatch => (
            FirmwareRowState::Incompatible,
            "A file with an accepted name was found, but its hash does not match.".into(),
        ),
        BiosMatchStatus::Ambiguous => (
            FirmwareRowState::MultipleCandidates,
            "Several accepted files were found. EmuWiz will not choose one for you.".into(),
        ),
        BiosMatchStatus::Missing => (
            FirmwareRowState::Missing,
            "No accepted filename was found in the inspected BIOS folder.".into(),
        ),
        BiosMatchStatus::Unknown => (
            FirmwareRowState::Unknown,
            "EmuWiz has no evidence either way.".into(),
        ),
    }
}

/// Explicit target-to-emulator mapping. Saturn and unmapped records carry no
/// reviewed EmuWiz emulator, so they never claim one.
fn mapped_emulator(item: &RetroBiosMetadataItem) -> Option<&'static str> {
    use RetroBiosCanonicalTarget as T;
    match item.canonical_target {
        T::Ps1DuckStation => Some("DuckStation"),
        T::Ps2Pcsx2 => Some("PCSX2"),
        T::Ps1RetroArchCore | T::Ps2RetroArchCore | T::DreamcastRetroArchCore => Some("RetroArch"),
        T::DreamcastFlycast => Some("Flycast"),
        T::AmigaKickstart => match item.emulator.as_str() {
            "fs-uae" => Some("FS-UAE"),
            _ => None,
        },
        T::AtariStTos => Some("Hatari"),
        T::ArcadeDependency => Some("MAME"),
        T::XboxXemu => Some("xemu"),
        T::Rpcs3Firmware => Some("RPCS3"),
        T::PpssppNotRequired => Some("PPSSPP"),
        T::DolphinOptionalIpl => Some("Dolphin"),
        T::Saturn | T::Unmapped => None,
    }
}

fn retrobios_row(
    item: &RetroBiosMetadataItem,
    snapshot: &RetroBiosMetadataSnapshot,
    inventory: Option<&BiosMasterInventory>,
    built_in: &[(BiosRequirement, FirmwareRowState)],
) -> FirmwareRow {
    let mapped = mapped_emulator(item);
    let accepted: Vec<String> = std::iter::once(item.canonical_filename.clone())
        .chain(item.aliases.iter().cloned())
        .collect();
    let local: Vec<&BiosEvidence> = inventory
        .map(|inventory| {
            inventory
                .entries
                .iter()
                .filter(|entry| accepted.contains(&entry.filename))
                .collect()
        })
        .unwrap_or_default();
    // The built-in requirement for the same emulator that names this file
    // supplies the authoritative readiness the join must not override.
    let authority = built_in.iter().find(|(requirement, _)| {
        Some(requirement.emulator.as_str()) == mapped
            && requirement
                .expected_filenames
                .iter()
                .any(|name| accepted.iter().any(|a| a.eq_ignore_ascii_case(name)))
    });
    let requirement = authority
        .map(|(requirement, _)| requirement.clone())
        .unwrap_or_else(|| BiosRequirement {
            name: item.canonical_filename.clone(),
            emulator: item.emulator.clone(),
            expected_filenames: Vec::new(),
            expected_sha256: None,
            target: BiosProjectionTarget {
                description: String::new(),
                path: None,
                current_state: BiosTargetState::NotInspected,
            },
            content_class: BiosContentClass::ImmutableFirmware,
            method: BiosProjectionMethod::Unknown,
        });
    let authoritative = match authority {
        Some((_, FirmwareRowState::Verified)) => FirmwareReadiness::Verified,
        Some((_, FirmwareRowState::NotRequired)) => FirmwareReadiness::NotRequired,
        Some((_, FirmwareRowState::Missing)) => FirmwareReadiness::Missing,
        _ if inventory.is_some() && local.is_empty() => FirmwareReadiness::Missing,
        _ => FirmwareReadiness::Unknown,
    };
    let (state, explanation) = if inventory.is_none() {
        (
            FirmwareRowState::NotInspected,
            "No BIOS folder has been inspected yet, so EmuWiz cannot compare local files with this record.".to_string(),
        )
    } else if local.len() > 1 {
        (
            FirmwareRowState::MultipleCandidates,
            "Several files use this record's filename or aliases. EmuWiz will not choose one for you.".to_string(),
        )
    } else {
        let joined = join_bios_evidence(
            &requirement,
            local.first().copied(),
            authoritative,
            Some(item),
            Some(&snapshot.provenance),
        );
        let state = match (joined.readiness, joined.match_status) {
            (FirmwareReadiness::Verified, _) => FirmwareRowState::Verified,
            (FirmwareReadiness::NotRequired, _) => FirmwareRowState::NotRequired,
            (_, BiosMatchStatus::HashMismatch) => FirmwareRowState::Incompatible,
            (FirmwareReadiness::Missing, _) => FirmwareRowState::Missing,
            (FirmwareReadiness::PresentUnverified, _) => FirmwareRowState::FilenameOnly,
            _ => FirmwareRowState::Unknown,
        };
        let explanation = match state {
            FirmwareRowState::Missing => {
                "No file with this record's filename or aliases was found in the inspected BIOS folder.".to_string()
            }
            _ => sentence(&joined.explanation),
        };
        (state, explanation)
    };
    FirmwareRow {
        source: FirmwareRowSource::RetroBios {
            source_path: item.source_path.clone(),
        },
        emulator: match &item.core {
            Some(core) => format!("{} · {core}", item.emulator),
            None => item.emulator.clone(),
        },
        name: item.canonical_filename.clone(),
        accepted_filenames: accepted,
        requiredness: match item.requiredness {
            RetroBiosRequiredness::Required => FirmwareRequiredness::Required,
            RetroBiosRequiredness::Optional => FirmwareRequiredness::Optional,
            RetroBiosRequiredness::HleFallback => FirmwareRequiredness::HleFallback,
            RetroBiosRequiredness::NotRequired => FirmwareRequiredness::NotRequired,
            RetroBiosRequiredness::Unknown => FirmwareRequiredness::NotStated,
        },
        content_class: BiosContentClass::ImmutableFirmware,
        region: item.region.clone(),
        revision: item.revision.clone(),
        version: item.version.clone(),
        expected_size: item.size_bytes,
        expected_hashes: item.hashes.clone(),
        candidates: local.into_iter().map(candidate).collect(),
        state,
        explanation,
        mapped_emulator: mapped,
    }
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let mut out = chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>())
        .unwrap_or_default();
    out.push_str(chars.as_str());
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

fn emulator_uses(
    rows: &[FirmwareRow],
    installed: Option<&EmulatorInventory>,
) -> Vec<EmulatorFirmwareUse> {
    let mut uses: Vec<EmulatorFirmwareUse> = Vec::new();
    for row in rows {
        let Some(emulator) = row.mapped_emulator else {
            continue;
        };
        let index = match uses.iter().position(|item| item.emulator == emulator) {
            Some(index) => index,
            None => {
                uses.push(EmulatorFirmwareUse {
                    emulator,
                    installation: installation(emulator, installed),
                    verified: Vec::new(),
                    blocking: Vec::new(),
                });
                uses.len() - 1
            }
        };
        let entry = &mut uses[index];
        if row.state == FirmwareRowState::Verified {
            for candidate in &row.candidates {
                if !entry.verified.contains(&candidate.filename) {
                    entry.verified.push(candidate.filename.clone());
                }
            }
        }
        if row.requiredness == FirmwareRequiredness::Required
            && matches!(
                row.state,
                FirmwareRowState::Missing | FirmwareRowState::Incompatible
            )
            && !entry.blocking.contains(&row.name)
        {
            entry.blocking.push(row.name.clone());
        }
    }
    uses
}

fn installation(emulator: &str, installed: Option<&EmulatorInventory>) -> EmulatorInstallation {
    use crate::emulator_inventory::InventoryEmulator as E;
    let tracked = [
        E::Mame,
        E::Dolphin,
        E::Rpcs3,
        E::Pcsx2,
        E::Ppsspp,
        E::DuckStation,
        E::Xemu,
        E::ShadPs4,
        E::Flycast,
    ];
    if !tracked.iter().any(|item| item.label() == emulator) {
        return EmulatorInstallation::NotTracked;
    }
    let Some(inventory) = installed else {
        return EmulatorInstallation::NotScanned;
    };
    let versions: Vec<String> = inventory
        .installations
        .iter()
        .filter(|installation| installation.emulator.label() == emulator)
        .map(|installation| {
            installation
                .version
                .clone()
                .unwrap_or_else(|| "unknown version".into())
        })
        .collect();
    if versions.is_empty() {
        EmulatorInstallation::NotFound
    } else {
        EmulatorInstallation::Installed { versions }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bios_projection::BiosEvidenceSource;
    use crate::retrobios_provider::{RETROBIOS_UPSTREAM_URL, parse_snapshot};
    use serde_json::{Value, json};

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn item(filename: &str, region: &str, revision: &str) -> Value {
        json!({
            "filename": filename,
            "aliases": [],
            "emulator": "duckstation",
            "platform": "RetroArch",
            "system": "sony-playstation",
            "required": true,
            "region": region,
            "revision": revision,
            "size": 524288,
            "hashes": {"sha256": DIGEST},
            "verification_mode": "exact_hash",
            "source_path": format!("emulators/duckstation.yml#{filename}"),
        })
    }

    fn snapshot(items: Vec<Value>) -> RetroBiosMetadataSnapshot {
        let bytes = serde_json::to_vec(&json!({
            "schema_version": 1,
            "provider": "retrobios",
            "upstream": {
                "url": RETROBIOS_UPSTREAM_URL,
                "ref": "v2026.09.04",
                "snapshot_digest": DIGEST,
                "timestamp": "2026-09-20T00:00:00Z",
                "signature_state": "verified",
                "mirror_used": false
            },
            "items": items
        }))
        .unwrap();
        parse_snapshot(&bytes, "test").unwrap()
    }

    fn inventory(files: &[(&str, Option<&str>)]) -> BiosMasterInventory {
        BiosMasterInventory {
            root: "/bios".into(),
            entries: files
                .iter()
                .map(|(name, digest)| BiosEvidence {
                    relative_path: (*name).into(),
                    filename: (*name).to_string(),
                    size_bytes: 524288,
                    sha256: digest.map(ToOwned::to_owned),
                    source: BiosEvidenceSource::Hash,
                    match_status: BiosMatchStatus::Unknown,
                    platform: None,
                })
                .collect(),
            warnings: Vec::new(),
        }
    }

    fn ps1() -> &'static FirmwarePlatform {
        firmware_platform("ps1").unwrap()
    }

    fn retro_rows(view: &PlatformFirmwareView) -> Vec<&FirmwareRow> {
        view.rows
            .iter()
            .filter(|row| matches!(row.source, FirmwareRowSource::RetroBios { .. }))
            .collect()
    }

    #[test]
    fn known_firmware_is_verified_only_by_exact_retrobios_hash() {
        let snapshot = snapshot(vec![item("scph5501.bin", "US", "5501")]);
        let inventory = inventory(&[("scph5501.bin", Some(DIGEST))]);
        let view = platform_firmware_view(ps1(), Some(&inventory), Some(&snapshot), None);
        let built_in = &view.rows[0];
        assert_eq!(built_in.source, FirmwareRowSource::BuiltIn);
        assert_eq!(built_in.state, FirmwareRowState::FilenameOnly);
        assert_eq!(built_in.requiredness, FirmwareRequiredness::NotStated);
        let retro = retro_rows(&view)[0];
        assert_eq!(retro.state, FirmwareRowState::Verified);
        assert_eq!(retro.region.as_deref(), Some("US"));
        assert_eq!(retro.revision.as_deref(), Some("5501"));
        assert_eq!(retro.mapped_emulator, Some("DuckStation"));
        assert_eq!(view.emulators[0].verified, vec!["scph5501.bin".to_string()]);
        assert_eq!(
            view.emulators[0].installation,
            EmulatorInstallation::NotScanned
        );
        assert_eq!(
            view.provenance.as_ref().unwrap().upstream_ref,
            "v2026.09.04"
        );
    }

    #[test]
    fn missing_and_incompatible_are_distinct_and_block_required_use() {
        let snapshot = snapshot(vec![
            item("scph5501.bin", "US", "5501"),
            item("scph5502.bin", "EU", "5502"),
        ]);
        let inventory = inventory(&[("scph5501.bin", Some(OTHER))]);
        let view = platform_firmware_view(ps1(), Some(&inventory), Some(&snapshot), None);
        let retro = retro_rows(&view);
        assert_eq!(retro[0].state, FirmwareRowState::Incompatible);
        assert_eq!(retro[1].state, FirmwareRowState::Missing);
        assert!(view.emulators[0].verified.is_empty());
        assert_eq!(
            view.emulators[0].blocking,
            vec!["scph5501.bin".to_string(), "scph5502.bin".to_string()]
        );
    }

    #[test]
    fn multiple_region_candidates_are_listed_not_chosen() {
        let snapshot = snapshot(vec![
            item("scph5501.bin", "US", "5501"),
            item("scph5502.bin", "EU", "5502"),
        ]);
        let inventory = inventory(&[("scph5501.bin", Some(DIGEST)), ("scph5502.bin", None)]);
        let view = platform_firmware_view(ps1(), Some(&inventory), Some(&snapshot), None);
        let built_in = &view.rows[0];
        assert_eq!(built_in.state, FirmwareRowState::MultipleCandidates);
        assert_eq!(built_in.candidates.len(), 2);
        let retro = retro_rows(&view);
        assert_eq!(retro[0].state, FirmwareRowState::Verified);
        // An unhashed file is never verified, whatever its name.
        assert_eq!(retro[1].state, FirmwareRowState::FilenameOnly);
        assert_eq!(retro[1].region.as_deref(), Some("EU"));
    }

    #[test]
    fn filename_only_and_unmapped_records_stay_unverified() {
        let mut presence = item("scph5501.bin", "US", "5501");
        presence["verification_mode"] = json!("presence_only");
        let mut unmapped = item("mystery.bin", "JP", "1");
        unmapped["emulator"] = json!("someemu");
        let snapshot = snapshot(vec![presence, unmapped]);
        let inventory = inventory(&[
            ("scph5501.bin", Some(DIGEST)),
            ("mystery.bin", Some(DIGEST)),
        ]);
        let view = platform_firmware_view(ps1(), Some(&inventory), Some(&snapshot), None);
        let retro = retro_rows(&view);
        assert_eq!(retro[0].state, FirmwareRowState::FilenameOnly);
        assert_eq!(retro[1].state, FirmwareRowState::Verified);
        assert_eq!(retro[1].mapped_emulator, None);
        assert!(view.emulators.iter().all(|use_| use_.verified.is_empty()));
    }

    #[test]
    fn uninspected_folder_is_never_reported_missing() {
        let snapshot = snapshot(vec![item("scph5501.bin", "US", "5501")]);
        let view = platform_firmware_view(ps1(), None, Some(&snapshot), None);
        assert!(
            view.rows
                .iter()
                .all(|row| row.state == FirmwareRowState::NotInspected)
        );
        assert!(view.emulators.iter().all(|use_| use_.blocking.is_empty()));
    }

    #[test]
    fn platform_switch_uses_only_that_platforms_records_and_adapters() {
        let snapshot = snapshot(vec![item("scph5501.bin", "US", "5501")]);
        let inventory = inventory(&[("scph5501.bin", Some(DIGEST))]);
        let view = platform_firmware_view(
            firmware_platform("psp").unwrap(),
            Some(&inventory),
            Some(&snapshot),
            None,
        );
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.rows[0].emulator, "PPSSPP");
        assert_eq!(view.rows[0].state, FirmwareRowState::NotRequired);
        let saturn = platform_firmware_view(
            firmware_platform("saturn").unwrap(),
            Some(&inventory),
            None,
            None,
        );
        assert!(saturn.rows.is_empty());
    }

    #[test]
    fn installation_comes_from_the_inventory_only() {
        let snapshot = snapshot(vec![item("scph5501.bin", "US", "5501")]);
        let inventory = inventory(&[("scph5501.bin", Some(DIGEST))]);
        let empty = EmulatorInventory::default();
        let view = platform_firmware_view(ps1(), Some(&inventory), Some(&snapshot), Some(&empty));
        assert_eq!(
            view.emulators[0].installation,
            EmulatorInstallation::NotFound
        );
        let amiga = platform_firmware_view(
            firmware_platform("amiga").unwrap(),
            Some(&inventory),
            None,
            Some(&empty),
        );
        assert_eq!(
            amiga.emulators[0].installation,
            EmulatorInstallation::NotTracked
        );
    }
}
