//! Read-only per-set compatibility against one pinned FinalBurn Neo source.
//!
//! FBNeo definitions are emulator-specific truth.  This module intentionally
//! does not reuse MAME expectations and does not discover, hash, or mutate
//! files.  Callers provide an explicitly imported local FBNeo DAT and the
//! existing observed arcade evidence bridge.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;

use crate::arcade_mame_compatibility::{
    MameObservedRom, ObservedArcadeSetEvidence, ObservedEvidenceCompleteness,
};
use crate::dat::dependency::graph::{
    ChainFault, ChainGuard, DeclaredName, DependencyGraph, Flag, SetRef, declared_name, parse_flag,
};
use crate::dat::dependency::owner::{RomOwner, rom_owner};
use crate::dat::model::{DatChecksum, DatEcosystem, DatGameEntry, DatRomEntry, ParsedDat};
use crate::identity_source::fbneo::ImportedFBNeoSource;
use crate::ready_to_play::ReadyToPlayState;

pub const FBNEO_COMPATIBILITY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FbNeoInstallationState {
    Installed,
    NotInstalled,
    Unknown,
}

/// Identity of the exact FBNeo/core and expectation source used for an audit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstalledFbNeoEvidence {
    pub installation_state: FbNeoInstallationState,
    pub core_path: Option<PathBuf>,
    pub executable_path: Option<PathBuf>,
    pub reported_version: Option<String>,
    pub core_revision: Option<String>,
    pub metadata_source: Option<PathBuf>,
    pub dat_source: Option<PathBuf>,
    pub dat_version: Option<String>,
    pub dat_sha256: Option<String>,
    pub evidence_timestamp_unix: Option<u64>,
    pub parser_schema_version: u32,
    pub usable: bool,
}

impl InstalledFbNeoEvidence {
    pub fn new(
        core_path: impl Into<PathBuf>,
        reported_version: Option<String>,
        core_revision: Option<String>,
        dat_source: impl Into<PathBuf>,
        dat_version: Option<String>,
        dat_sha256: Option<String>,
        evidence_timestamp_unix: Option<u64>,
    ) -> Self {
        Self {
            installation_state: FbNeoInstallationState::Installed,
            core_path: Some(core_path.into()),
            executable_path: None,
            reported_version,
            core_revision,
            metadata_source: None,
            dat_source: Some(dat_source.into()),
            dat_version,
            dat_sha256,
            evidence_timestamp_unix,
            parser_schema_version: FBNEO_COMPATIBILITY_SCHEMA_VERSION,
            usable: true,
        }
    }

    pub fn not_installed() -> Self {
        Self {
            installation_state: FbNeoInstallationState::NotInstalled,
            core_path: None,
            executable_path: None,
            reported_version: None,
            core_revision: None,
            metadata_source: None,
            dat_source: None,
            dat_version: None,
            dat_sha256: None,
            evidence_timestamp_unix: None,
            parser_schema_version: FBNEO_COMPATIBILITY_SCHEMA_VERSION,
            usable: false,
        }
    }

    pub fn unknown() -> Self {
        Self {
            installation_state: FbNeoInstallationState::Unknown,
            core_path: None,
            executable_path: None,
            reported_version: None,
            core_revision: None,
            metadata_source: None,
            dat_source: None,
            dat_version: None,
            dat_sha256: None,
            evidence_timestamp_unix: None,
            parser_schema_version: FBNEO_COMPATIBILITY_SCHEMA_VERSION,
            usable: false,
        }
    }

    pub fn from_imported(
        core_path: impl Into<PathBuf>,
        reported_version: Option<String>,
        core_revision: Option<String>,
        imported: &ImportedFBNeoSource,
        evidence_timestamp_unix: Option<u64>,
    ) -> Self {
        Self::new(
            core_path,
            reported_version,
            core_revision,
            imported.artifact_path.clone(),
            imported.upstream_version.clone(),
            Some(imported.artifact_sha256.clone()),
            evidence_timestamp_unix,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FbNeoSetCompatibilityState {
    Compatible,
    CompatibleWithWarnings,
    Incompatible,
    Unknown,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FbNeoMismatchReason {
    RequiredFileMissing,
    WrongSize,
    CrcMismatch,
    Sha1Mismatch,
    ParentMissing,
    BiosMissing,
    DeviceDependencyMissing,
    ChdMissing,
    ChdHashMismatch,
    BadDump,
    NoDump,
    UnsupportedSet,
    EvidenceUnavailable,
    VersionProvenanceUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FbNeoMismatch {
    pub reason: FbNeoMismatchReason,
    pub set_name: String,
    pub member_name: Option<String>,
    pub expected_size: Option<u64>,
    pub observed_size: Option<u64>,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FbNeoExpectedRom {
    pub set_name: String,
    pub name: String,
    pub size_bytes: Option<u64>,
    pub checksums: Vec<DatChecksum>,
    pub status: Option<String>,
    pub merge: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FbNeoExpectedDisk {
    pub set_name: String,
    pub name: Option<String>,
    pub sha1: Option<String>,
    pub status: Option<String>,
    pub merge: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FbNeoDependencyEvidence {
    pub set_name: String,
    pub parent: Option<String>,
    pub rom_of: Option<String>,
    pub bios_sets: Vec<String>,
    pub device_refs: Vec<String>,
    pub closure_set_names: Vec<String>,
    /// Dependencies the catalogue does not prove (missing, duplicated or
    /// cyclic sets, contradictory or unverifiable `merge=` borrows). Any entry
    /// keeps the set from being called compatible.
    pub unresolved: Vec<String>,
    pub runnable: Option<String>,
    pub is_bios: Option<String>,
    pub is_device: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FbNeoSetExpectation {
    pub set_name: String,
    pub parent: Option<String>,
    pub rom_of: Option<String>,
    pub runnable: Option<String>,
    pub is_bios: Option<String>,
    pub is_device: Option<String>,
    pub driver_status: Option<String>,
    pub roms: Vec<FbNeoExpectedRom>,
    pub disks: Vec<FbNeoExpectedDisk>,
    pub dependencies: FbNeoDependencyEvidence,
}

impl FbNeoSetExpectation {
    pub fn from_game(game: &DatGameEntry) -> Self {
        Self {
            set_name: game.name.clone(),
            parent: game.clone_of.clone(),
            rom_of: game.rom_of.clone(),
            runnable: game.runnable.clone(),
            is_bios: game.is_bios.clone(),
            is_device: game.is_device.clone(),
            driver_status: game.original_metadata.fields.get("driver.status").cloned(),
            roms: game
                .roms
                .iter()
                .map(|rom| expected_rom(&game.name, rom))
                .collect(),
            disks: game
                .disks
                .iter()
                .map(|disk| FbNeoExpectedDisk {
                    set_name: game.name.clone(),
                    name: disk.name.clone(),
                    sha1: disk.sha1.clone(),
                    status: disk.status.clone(),
                    merge: disk.merge.clone(),
                })
                .collect(),
            dependencies: FbNeoDependencyEvidence {
                set_name: game.name.clone(),
                parent: game.clone_of.clone(),
                rom_of: game.rom_of.clone(),
                // BIOS provider *sets* are found through the catalogue in
                // `expectation_for`; `<biosset>` names are variants, not sets.
                bios_sets: Vec::new(),
                device_refs: game
                    .device_refs
                    .iter()
                    .filter_map(|reference| reference.name.clone())
                    .collect(),
                closure_set_names: vec![game.name.clone()],
                unresolved: Vec::new(),
                runnable: game.runnable.clone(),
                is_bios: game.is_bios.clone(),
                is_device: game.is_device.clone(),
            },
        }
    }
}

/// Expectations indexed from exactly one explicitly imported FBNeo DAT.
#[derive(Clone, Debug)]
pub struct FbNeoExpectationIndex<'a> {
    dat: &'a ParsedDat,
}

impl<'a> FbNeoExpectationIndex<'a> {
    pub fn new(dat: &'a ParsedDat) -> Self {
        Self { dat }
    }

    /// Builds the expectation for one set from catalogue facts only.
    ///
    /// Clone ancestry (`cloneof`) is recorded, never inherited: a parent's
    /// inventory is not a clone's requirement. The set's own declarations are
    /// required where they are stored: its own members in the set itself, and
    /// each `merge=` borrow at the declaration the catalogue proves owns it
    /// (following `romof`, possibly into a BIOS). BIOS providers come from the
    /// `romof` chain and devices from `device_ref`s, transitively. Whatever the
    /// catalogue does not prove is recorded as unresolved instead of guessed.
    pub fn expectation_for(&self, set_name: &str) -> Option<FbNeoSetExpectation> {
        if self.dat.source.ecosystem != DatEcosystem::FBNeo {
            return None;
        }
        let graph = DependencyGraph::build(&self.dat.games);
        let mut unresolved = Vec::new();
        let subject = match graph.resolve_set(set_name) {
            SetRef::Unique(index) => index,
            SetRef::Absent => return None,
            SetRef::Duplicate => {
                unresolved.push(format!("{set_name} is declared more than once"));
                self.dat
                    .games
                    .iter()
                    .position(|game| game.name == set_name)?
            }
        };
        let selected = graph.game(subject);
        let mut expectation = FbNeoSetExpectation::from_game(selected);
        let mut closure = BTreeSet::from([selected.name.clone()]);
        let mut bios_sets = BTreeSet::new();

        expectation.roms = required_roms(&graph, subject, &mut closure, &mut unresolved);

        // ROM sources: the `romof` chain only. A BIOS among them is a BIOS
        // dependency; `cloneof` never enters this chain.
        let mut guard = ChainGuard::starting_at(subject);
        let mut current = subject;
        loop {
            let declaring = graph.game(current);
            let provider = match declared_name(&declaring.rom_of) {
                DeclaredName::Absent => break,
                DeclaredName::Malformed => {
                    unresolved.push(format!("{} declares an empty romof", declaring.name));
                    break;
                }
                DeclaredName::Named(name) => name,
            };
            let next = match graph.resolve_set(provider) {
                SetRef::Unique(next) => next,
                SetRef::Absent => {
                    unresolved.push(format!(
                        "ROM source {provider} is absent from the catalogue"
                    ));
                    break;
                }
                SetRef::Duplicate => {
                    unresolved.push(format!("ROM source {provider} is declared more than once"));
                    break;
                }
            };
            if let Err(fault) = guard.visit(next) {
                unresolved.push(match fault {
                    ChainFault::Cycle => format!("romof chain from {} loops", selected.name),
                    ChainFault::DepthExceeded => {
                        format!("romof chain from {} is too deep", selected.name)
                    }
                });
                break;
            }
            let provider_game = graph.game(next);
            closure.insert(provider_game.name.clone());
            if parse_flag(&provider_game.is_bios) == Flag::Yes {
                bios_sets.insert(provider_game.name.clone());
            }
            current = next;
        }

        // Devices, transitively. A device's own ROMs are required; a game
        // referencing a device does not make that device a parent.
        let mut devices = BTreeSet::new();
        let mut pending = selected
            .device_refs
            .iter()
            .map(|reference| reference.name.clone())
            .collect::<Vec<_>>();
        while let Some(reference) = pending.pop() {
            let name = match declared_name(&reference) {
                DeclaredName::Named(name) => name.to_string(),
                _ => {
                    unresolved.push(format!("{} declares an empty device_ref", selected.name));
                    continue;
                }
            };
            if !devices.insert(name.clone()) {
                continue;
            }
            let device = match graph.resolve_set(&name) {
                SetRef::Unique(device) => device,
                SetRef::Absent => {
                    unresolved.push(format!("device {name} is absent from the catalogue"));
                    continue;
                }
                SetRef::Duplicate => {
                    unresolved.push(format!("device {name} is declared more than once"));
                    continue;
                }
            };
            closure.insert(name);
            expectation
                .roms
                .extend(required_roms(&graph, device, &mut closure, &mut unresolved));
            pending.extend(
                graph
                    .game(device)
                    .device_refs
                    .iter()
                    .map(|reference| reference.name.clone()),
            );
        }

        expectation.roms.sort_by(|a, b| {
            (a.set_name.as_str(), a.name.as_str()).cmp(&(b.set_name.as_str(), b.name.as_str()))
        });
        expectation.roms.dedup();
        unresolved.sort();
        unresolved.dedup();
        expectation.dependencies.bios_sets = bios_sets.into_iter().collect();
        expectation.dependencies.device_refs = devices.into_iter().collect();
        expectation.dependencies.closure_set_names = closure.into_iter().collect();
        expectation.dependencies.unresolved = unresolved;
        Some(expectation)
    }
}

/// The ROMs set `index` declares, each required where the catalogue proves
/// its content is stored. Sets that own borrowed members join `closure`;
/// borrows the catalogue does not prove are recorded, not required.
fn required_roms(
    graph: &DependencyGraph<'_>,
    index: usize,
    closure: &mut BTreeSet<String>,
    unresolved: &mut Vec<String>,
) -> Vec<FbNeoExpectedRom> {
    let game = graph.game(index);
    let mut roms = Vec::new();
    for rom in &game.roms {
        match rom_owner(graph, index, rom) {
            Ok(RomOwner::Own) => roms.push(expected_rom(&game.name, rom)),
            Ok(RomOwner::Borrowed { owner, declaration }) => {
                let owner = graph.game(owner);
                closure.insert(owner.name.clone());
                let mut expected = expected_rom(&owner.name, declaration);
                expected.merge = rom.merge.clone();
                roms.push(expected);
            }
            Err(outcome) => unresolved.push(format!(
                "{}:{} borrows {} without a proven owner ({outcome:?})",
                game.name,
                rom.name,
                rom.merge.as_deref().unwrap_or_default()
            )),
        }
    }
    roms
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FbNeoSetCompatibility {
    pub set_name: String,
    pub state: FbNeoSetCompatibilityState,
    pub fbneo: InstalledFbNeoEvidence,
    pub expectation: Option<FbNeoSetExpectation>,
    pub mismatches: Vec<FbNeoMismatch>,
    pub driver_status: Option<String>,
    pub collection_provenance_known: bool,
    pub ready_to_play_state: Option<ReadyToPlayState>,
}

/// Compare one set using only existing observed evidence.  A partial listing
/// can prove a mismatch, but can never be promoted to a clean compatibility
/// claim.
pub fn audit_fbneo_set(
    fbneo: &InstalledFbNeoEvidence,
    expectation: Option<FbNeoSetExpectation>,
    observed: &ObservedArcadeSetEvidence,
) -> FbNeoSetCompatibility {
    let set_name = observed.set_name.clone();
    let collection_provenance_known = observed.collection_provenance_known;
    let Some(expectation) = expectation else {
        let state = if fbneo.usable {
            FbNeoSetCompatibilityState::Unsupported
        } else {
            FbNeoSetCompatibilityState::Unknown
        };
        return result(
            set_name,
            fbneo,
            None,
            state,
            vec![FbNeoMismatch {
                reason: if fbneo.usable {
                    FbNeoMismatchReason::UnsupportedSet
                } else {
                    FbNeoMismatchReason::VersionProvenanceUnknown
                },
                set_name: observed.set_name.clone(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "No usable FBNeo expectation is available for this set".into(),
            }],
            collection_provenance_known,
        );
    };
    let driver_status = expectation.driver_status.clone();
    if !fbneo.usable {
        return result(
            set_name,
            fbneo,
            Some(expectation),
            FbNeoSetCompatibilityState::Unknown,
            vec![FbNeoMismatch {
                reason: FbNeoMismatchReason::VersionProvenanceUnknown,
                set_name: observed.set_name.clone(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "Installed FBNeo/core evidence is not usable".into(),
            }],
            collection_provenance_known,
        );
    }
    if matches!(
        observed.completeness,
        ObservedEvidenceCompleteness::NotGathered | ObservedEvidenceCompleteness::Unknown
    ) {
        return result(
            set_name,
            fbneo,
            Some(expectation),
            FbNeoSetCompatibilityState::Unknown,
            vec![FbNeoMismatch {
                reason: FbNeoMismatchReason::EvidenceUnavailable,
                set_name: observed.set_name.clone(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "Observed FBNeo evidence was not gathered".into(),
            }],
            collection_provenance_known,
        );
    }
    let mut mismatches = Vec::new();
    for unresolved in &expectation.dependencies.unresolved {
        mismatches.push(FbNeoMismatch {
            reason: FbNeoMismatchReason::EvidenceUnavailable,
            set_name: observed.set_name.clone(),
            member_name: None,
            expected_size: None,
            observed_size: None,
            detail: format!("FBNeo dependency is not proven by the catalogue: {unresolved}"),
        });
    }
    for dependency in &expectation.dependencies.closure_set_names {
        if !observed
            .observed_set_names
            .iter()
            .any(|name| name == dependency)
        {
            if observed.completeness != ObservedEvidenceCompleteness::Complete {
                continue;
            }
            let reason = if dependency == &expectation.set_name {
                FbNeoMismatchReason::RequiredFileMissing
            } else if expectation
                .dependencies
                .device_refs
                .iter()
                .any(|name| name == dependency)
            {
                FbNeoMismatchReason::DeviceDependencyMissing
            } else if expectation
                .dependencies
                .bios_sets
                .iter()
                .any(|name| name == dependency)
            {
                FbNeoMismatchReason::BiosMissing
            } else {
                FbNeoMismatchReason::ParentMissing
            };
            mismatches.push(FbNeoMismatch {
                reason,
                set_name: dependency.clone(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "Required FBNeo dependency was not resolved by supplied evidence".into(),
            });
        }
    }
    for expected in &expectation.roms {
        let observed_rom = observed
            .observed_roms
            .iter()
            .find(|item| item.set_name == expected.set_name && item.name == expected.name);
        if status_is(expected.status.as_deref(), "nodump") {
            mismatches.push(mismatch(
                FbNeoMismatchReason::NoDump,
                expected,
                observed_rom,
                "FBNeo declares no known dump for this member",
            ));
            continue;
        }
        let Some(observed_rom) = observed_rom else {
            if observed.completeness != ObservedEvidenceCompleteness::Complete {
                continue;
            }
            mismatches.push(mismatch(
                FbNeoMismatchReason::RequiredFileMissing,
                expected,
                None,
                "Required FBNeo ROM member was not present in supplied evidence",
            ));
            continue;
        };
        if expected.size_bytes != observed_rom.size_bytes {
            mismatches.push(mismatch(
                FbNeoMismatchReason::WrongSize,
                expected,
                Some(observed_rom),
                "Observed ROM size does not match the FBNeo expectation",
            ));
            continue;
        }
        let mut compared_checksum = false;
        for checksum in &expected.checksums {
            let observed_value = match checksum.algorithm.label() {
                "CRC32" => observed_rom.crc32.as_deref(),
                "SHA-1" => observed_rom.sha1.as_deref(),
                _ => None,
            };
            let Some(observed_value) = observed_value else {
                mismatches.push(mismatch(
                    FbNeoMismatchReason::EvidenceUnavailable,
                    expected,
                    Some(observed_rom),
                    format!(
                        "Observed {} evidence was not gathered",
                        checksum.algorithm.label()
                    ),
                ));
                continue;
            };
            compared_checksum = true;
            if observed_value != checksum.value {
                mismatches.push(mismatch(
                    match checksum.algorithm.label() {
                        "CRC32" => FbNeoMismatchReason::CrcMismatch,
                        "SHA-1" => FbNeoMismatchReason::Sha1Mismatch,
                        _ => FbNeoMismatchReason::EvidenceUnavailable,
                    },
                    expected,
                    Some(observed_rom),
                    format!(
                        "Observed {} does not match FBNeo",
                        checksum.algorithm.label()
                    ),
                ));
            }
        }
        if expected.checksums.is_empty() || !compared_checksum {
            mismatches.push(mismatch(
                FbNeoMismatchReason::EvidenceUnavailable,
                expected,
                Some(observed_rom),
                "Filename and size alone do not prove FBNeo compatibility",
            ));
        }
        if status_is(expected.status.as_deref(), "baddump") {
            mismatches.push(mismatch(
                FbNeoMismatchReason::BadDump,
                expected,
                Some(observed_rom),
                "FBNeo marks this dump as imperfect",
            ));
        }
    }
    for expected in &expectation.disks {
        let observed_disk = observed
            .observed_disks
            .iter()
            .find(|item| item.set_name == expected.set_name && item.name == expected.name);
        if observed_disk.is_none() {
            if observed.completeness != ObservedEvidenceCompleteness::Complete {
                continue;
            }
            mismatches.push(FbNeoMismatch {
                reason: FbNeoMismatchReason::ChdMissing,
                set_name: expected.set_name.clone(),
                member_name: expected.name.clone(),
                expected_size: None,
                observed_size: None,
                detail: "Required FBNeo CHD was not present in supplied evidence".into(),
            });
        } else if observed_disk.and_then(|item| item.sha1.as_deref()) != expected.sha1.as_deref() {
            mismatches.push(FbNeoMismatch {
                reason: if observed_disk
                    .and_then(|item| item.sha1.as_deref())
                    .is_some()
                {
                    FbNeoMismatchReason::ChdHashMismatch
                } else {
                    FbNeoMismatchReason::EvidenceUnavailable
                },
                set_name: expected.set_name.clone(),
                member_name: expected.name.clone(),
                expected_size: None,
                observed_size: None,
                detail: "Observed FBNeo CHD identity is missing or mismatched".into(),
            });
        }
    }
    if observed.completeness == ObservedEvidenceCompleteness::Partial {
        mismatches.push(FbNeoMismatch {
            reason: FbNeoMismatchReason::EvidenceUnavailable,
            set_name: observed.set_name.clone(),
            member_name: None,
            expected_size: None,
            observed_size: None,
            detail: "Only partial cached evidence was available; compatibility is not proven"
                .into(),
        });
    } else if matches!(
        observed.completeness,
        ObservedEvidenceCompleteness::NotGathered | ObservedEvidenceCompleteness::Unknown
    ) {
        mismatches.push(FbNeoMismatch {
            reason: FbNeoMismatchReason::EvidenceUnavailable,
            set_name: observed.set_name.clone(),
            member_name: None,
            expected_size: None,
            observed_size: None,
            detail: "Observed FBNeo evidence was not gathered".into(),
        });
    }
    let state = if mismatches.iter().any(|item| {
        matches!(
            item.reason,
            FbNeoMismatchReason::RequiredFileMissing
                | FbNeoMismatchReason::WrongSize
                | FbNeoMismatchReason::CrcMismatch
                | FbNeoMismatchReason::Sha1Mismatch
                | FbNeoMismatchReason::ParentMissing
                | FbNeoMismatchReason::BiosMissing
                | FbNeoMismatchReason::DeviceDependencyMissing
                | FbNeoMismatchReason::ChdMissing
                | FbNeoMismatchReason::ChdHashMismatch
        )
    }) {
        FbNeoSetCompatibilityState::Incompatible
    } else if mismatches.iter().any(|item| {
        matches!(
            item.reason,
            FbNeoMismatchReason::NoDump | FbNeoMismatchReason::UnsupportedSet
        )
    }) {
        FbNeoSetCompatibilityState::Unsupported
    } else if mismatches
        .iter()
        .any(|item| item.reason == FbNeoMismatchReason::EvidenceUnavailable)
    {
        FbNeoSetCompatibilityState::Unknown
    } else if mismatches.is_empty() {
        if driver_status
            .as_deref()
            .is_some_and(|status| !status.eq_ignore_ascii_case("good"))
        {
            FbNeoSetCompatibilityState::CompatibleWithWarnings
        } else {
            FbNeoSetCompatibilityState::Compatible
        }
    } else {
        FbNeoSetCompatibilityState::CompatibleWithWarnings
    };
    result(
        set_name,
        fbneo,
        Some(expectation),
        state,
        mismatches,
        collection_provenance_known,
    )
}

fn expected_rom(set_name: &str, rom: &DatRomEntry) -> FbNeoExpectedRom {
    FbNeoExpectedRom {
        set_name: set_name.into(),
        name: rom.name.clone(),
        size_bytes: rom.size_bytes,
        checksums: rom.checksums(),
        status: rom.status.clone(),
        merge: rom.merge.clone(),
    }
}

fn status_is(status: Option<&str>, expected: &str) -> bool {
    status.is_some_and(|status| status.eq_ignore_ascii_case(expected))
}

fn mismatch(
    reason: FbNeoMismatchReason,
    expected: &FbNeoExpectedRom,
    observed: Option<&MameObservedRom>,
    detail: impl Into<String>,
) -> FbNeoMismatch {
    FbNeoMismatch {
        reason,
        set_name: expected.set_name.clone(),
        member_name: Some(expected.name.clone()),
        expected_size: expected.size_bytes,
        observed_size: observed.and_then(|item| item.size_bytes),
        detail: detail.into(),
    }
}

fn result(
    set_name: String,
    fbneo: &InstalledFbNeoEvidence,
    expectation: Option<FbNeoSetExpectation>,
    state: FbNeoSetCompatibilityState,
    mut mismatches: Vec<FbNeoMismatch>,
    collection_provenance_known: bool,
) -> FbNeoSetCompatibility {
    mismatches.sort_by_key(|item| {
        (
            item.reason,
            item.set_name.clone(),
            item.member_name.clone().unwrap_or_default(),
        )
    });
    let driver_status = expectation
        .as_ref()
        .and_then(|item| item.driver_status.clone());
    let ready_to_play_state = match state {
        FbNeoSetCompatibilityState::Compatible => Some(ReadyToPlayState::Ready),
        FbNeoSetCompatibilityState::CompatibleWithWarnings => {
            Some(ReadyToPlayState::ReadyWithWarnings)
        }
        FbNeoSetCompatibilityState::Incompatible => Some(ReadyToPlayState::Blocked),
        FbNeoSetCompatibilityState::Unsupported => Some(ReadyToPlayState::Unsupported),
        FbNeoSetCompatibilityState::Unknown => Some(ReadyToPlayState::Unknown),
    };
    FbNeoSetCompatibility {
        set_name,
        state,
        fbneo: fbneo.clone(),
        expectation,
        mismatches,
        driver_status,
        collection_provenance_known,
        ready_to_play_state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::{DatGameEntry, DatRomEntry};

    fn fbneo() -> InstalledFbNeoEvidence {
        InstalledFbNeoEvidence::new(
            "/cores/fbneo_libretro.so",
            Some("1.0".into()),
            Some("commit".into()),
            "/tmp/fbneo.dat",
            Some("2026".into()),
            Some("dat-sha".into()),
            None,
        )
    }

    fn expectation() -> FbNeoSetExpectation {
        FbNeoSetExpectation::from_game(&DatGameEntry {
            name: "game".into(),
            roms: vec![DatRomEntry {
                name: "game.bin".into(),
                size_bytes: Some(4),
                crc32: Some("aaaaaaaa".into()),
                ..Default::default()
            }],
            ..Default::default()
        })
    }

    fn observed() -> ObservedArcadeSetEvidence {
        let mut observed =
            ObservedArcadeSetEvidence::new("game", ObservedEvidenceCompleteness::Complete);
        observed.observed_set_names.push("game".into());
        observed.observed_roms.push(MameObservedRom {
            set_name: "game".into(),
            name: "game.bin".into(),
            size_bytes: Some(4),
            crc32: Some("aaaaaaaa".into()),
            sha1: None,
        });
        observed
    }

    #[test]
    fn clean_fbneo_set_is_compatible_independently_of_mame() {
        let result = audit_fbneo_set(&fbneo(), Some(expectation()), &observed());
        assert_eq!(result.state, FbNeoSetCompatibilityState::Compatible);
        assert_eq!(result.ready_to_play_state, Some(ReadyToPlayState::Ready));
    }

    #[test]
    fn wrong_size_is_incompatible() {
        let mut evidence = observed();
        evidence.observed_roms[0].size_bytes = Some(3);
        let result = audit_fbneo_set(&fbneo(), Some(expectation()), &evidence);
        assert_eq!(result.state, FbNeoSetCompatibilityState::Incompatible);
        assert!(
            result
                .mismatches
                .iter()
                .any(|item| item.reason == FbNeoMismatchReason::WrongSize)
        );
    }

    #[test]
    fn partial_evidence_cannot_be_clean_compatibility() {
        let mut evidence = observed();
        evidence.completeness = ObservedEvidenceCompleteness::Partial;
        let result = audit_fbneo_set(&fbneo(), Some(expectation()), &evidence);
        assert_eq!(result.state, FbNeoSetCompatibilityState::Unknown);
        assert_eq!(result.ready_to_play_state, Some(ReadyToPlayState::Unknown));
    }

    #[test]
    fn partial_absence_remains_unknown_instead_of_missing() {
        let mut evidence = observed();
        evidence.completeness = ObservedEvidenceCompleteness::Partial;
        evidence.observed_roms.clear();
        let result = audit_fbneo_set(&fbneo(), Some(expectation()), &evidence);
        assert_eq!(result.state, FbNeoSetCompatibilityState::Unknown);
        assert!(
            !result
                .mismatches
                .iter()
                .any(|item| item.reason == FbNeoMismatchReason::RequiredFileMissing)
        );
        assert!(
            result
                .mismatches
                .iter()
                .any(|item| item.reason == FbNeoMismatchReason::EvidenceUnavailable)
        );
    }

    #[test]
    fn partial_observed_contradiction_remains_incompatible() {
        let mut evidence = observed();
        evidence.completeness = ObservedEvidenceCompleteness::Partial;
        evidence.observed_roms[0].size_bytes = Some(3);
        let result = audit_fbneo_set(&fbneo(), Some(expectation()), &evidence);
        assert_eq!(result.state, FbNeoSetCompatibilityState::Incompatible);
        assert!(
            result
                .mismatches
                .iter()
                .any(|item| item.reason == FbNeoMismatchReason::WrongSize)
        );
    }

    #[test]
    fn complete_absence_is_proven_missing() {
        let mut evidence = observed();
        evidence.observed_roms.clear();
        let result = audit_fbneo_set(&fbneo(), Some(expectation()), &evidence);
        assert_eq!(result.state, FbNeoSetCompatibilityState::Incompatible);
        assert!(
            result
                .mismatches
                .iter()
                .any(|item| item.reason == FbNeoMismatchReason::RequiredFileMissing)
        );
    }

    #[test]
    fn partial_dependency_absence_is_not_proven_missing() {
        let mut evidence = observed();
        evidence.completeness = ObservedEvidenceCompleteness::Partial;
        let mut exp = expectation();
        exp.dependencies.closure_set_names = vec!["game".into(), "parent".into()];
        exp.dependencies.parent = Some("parent".into());
        let result = audit_fbneo_set(&fbneo(), Some(exp), &evidence);
        assert_eq!(result.state, FbNeoSetCompatibilityState::Unknown);
        assert!(
            !result
                .mismatches
                .iter()
                .any(|item| item.reason == FbNeoMismatchReason::ParentMissing)
        );
    }

    #[test]
    fn not_gathered_evidence_does_not_infer_missing_members() {
        let mut evidence = observed();
        evidence.completeness = ObservedEvidenceCompleteness::NotGathered;
        evidence.observed_roms.clear();
        evidence.observed_set_names.clear();
        let result = audit_fbneo_set(&fbneo(), Some(expectation()), &evidence);
        assert_eq!(result.state, FbNeoSetCompatibilityState::Unknown);
        assert!(
            !result
                .mismatches
                .iter()
                .any(|item| item.reason == FbNeoMismatchReason::RequiredFileMissing)
        );
    }

    #[test]
    fn missing_dependency_is_blocking_and_device_is_not_required_to_be_runnable() {
        let mut exp = expectation();
        exp.dependencies.closure_set_names = vec!["game".into(), "qsound".into()];
        exp.dependencies.device_refs = vec!["qsound".into()];
        let result = audit_fbneo_set(&fbneo(), Some(exp), &observed());
        assert_eq!(result.state, FbNeoSetCompatibilityState::Incompatible);
        assert!(
            result
                .mismatches
                .iter()
                .any(|item| item.reason == FbNeoMismatchReason::DeviceDependencyMissing)
        );
    }

    #[test]
    fn absent_expectation_is_unsupported_without_using_mame_truth() {
        let result = audit_fbneo_set(&fbneo(), None, &observed());
        assert_eq!(result.state, FbNeoSetCompatibilityState::Unsupported);
        assert_eq!(
            result.ready_to_play_state,
            Some(ReadyToPlayState::Unsupported)
        );
    }

    #[test]
    fn absent_installation_is_unknown() {
        let result = audit_fbneo_set(
            &InstalledFbNeoEvidence::not_installed(),
            Some(expectation()),
            &observed(),
        );
        assert_eq!(result.state, FbNeoSetCompatibilityState::Unknown);
    }

    // --- Dependency model regressions -------------------------------------

    const SHA_P: &str = "1111111111111111111111111111111111111111";
    const SHA_C: &str = "2222222222222222222222222222222222222222";
    const SHA_B: &str = "3333333333333333333333333333333333333333";
    const SHA_U: &str = "4444444444444444444444444444444444444444";

    fn dat_rom(name: &str, sha1: &str, merge: Option<&str>) -> DatRomEntry {
        DatRomEntry {
            name: name.into(),
            size_bytes: Some(4),
            sha1: Some(sha1.into()),
            merge: merge.map(str::to_owned),
            ..Default::default()
        }
    }

    fn fbneo_dat(games: Vec<DatGameEntry>) -> ParsedDat {
        ParsedDat {
            source: crate::dat::model::DatSource {
                format: crate::dat::model::DatFormat::Logiqx,
                ecosystem: DatEcosystem::FBNeo,
                file_path: "fbneo-fixture".into(),
                name: None,
                description: None,
                version: Some("test".into()),
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: games.len(),
                rom_count: games.iter().map(|g| g.roms.len()).sum(),
                parse_warnings: vec![],
                packing_policy: crate::dat::model::DatPackingPolicy::Standard,
            },
            games,
        }
    }

    /// neogeo-style: BIOS <- parent (romof bios) <- clone (cloneof/romof parent).
    fn neogeo_family() -> ParsedDat {
        let mut bios = DatGameEntry {
            name: "bios".into(),
            roms: vec![dat_rom("sys.rom", SHA_B, None)],
            bios_sets: vec![crate::dat::model::DatBiosSetEntry {
                name: Some("euro".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        bios.is_bios = Some("yes".into());
        let parent = DatGameEntry {
            name: "parent".into(),
            rom_of: Some("bios".into()),
            roms: vec![
                dat_rom("sys.rom", SHA_B, Some("sys.rom")),
                dat_rom("p.rom", SHA_P, None),
                dat_rom("unique-to-parent.rom", SHA_U, None),
            ],
            ..Default::default()
        };
        let clone = DatGameEntry {
            name: "clone".into(),
            clone_of: Some("parent".into()),
            rom_of: Some("parent".into()),
            roms: vec![
                dat_rom("sys.rom", SHA_B, Some("sys.rom")),
                dat_rom("p.rom", SHA_P, Some("p.rom")),
                dat_rom("c.rom", SHA_C, None),
            ],
            ..Default::default()
        };
        fbneo_dat(vec![bios, parent, clone])
    }

    fn required(expectation: &FbNeoSetExpectation) -> Vec<(String, String)> {
        expectation
            .roms
            .iter()
            .map(|rom| (rom.set_name.clone(), rom.name.clone()))
            .collect()
    }

    #[test]
    fn a_clone_requires_its_own_declarations_where_stored_never_the_parent_inventory() {
        let dat = neogeo_family();
        let expectation = FbNeoExpectationIndex::new(&dat)
            .expectation_for("clone")
            .unwrap();
        // Before: the parent's whole inventory (including unique-to-parent.rom)
        // and the BIOS inventory were copied in as requirements.
        assert_eq!(
            required(&expectation),
            vec![
                ("bios".into(), "sys.rom".into()),
                ("clone".into(), "c.rom".into()),
                ("parent".into(), "p.rom".into()),
            ]
        );
        assert_eq!(expectation.parent.as_deref(), Some("parent"));
        assert_eq!(expectation.dependencies.bios_sets, vec!["bios".to_string()]);
        assert_eq!(
            expectation.dependencies.closure_set_names,
            vec!["bios".to_string(), "clone".into(), "parent".into()]
        );
        // `<biosset>` variant names are never treated as sets.
        assert!(
            !expectation
                .dependencies
                .closure_set_names
                .contains(&"euro".to_string())
        );
        assert!(expectation.dependencies.unresolved.is_empty());
    }

    #[test]
    fn romof_is_followed_even_when_cloneof_names_a_different_set() {
        // Before: `clone_of.or(rom_of)` followed "lineage" and never reached
        // the real ROM source.
        let dat = fbneo_dat(vec![
            DatGameEntry {
                name: "lineage".into(),
                roms: vec![dat_rom("l.rom", SHA_U, None)],
                ..Default::default()
            },
            DatGameEntry {
                name: "source".into(),
                roms: vec![dat_rom("p.rom", SHA_P, None)],
                ..Default::default()
            },
            DatGameEntry {
                name: "child".into(),
                clone_of: Some("lineage".into()),
                rom_of: Some("source".into()),
                roms: vec![dat_rom("p.rom", SHA_P, Some("p.rom"))],
                ..Default::default()
            },
        ]);
        let expectation = FbNeoExpectationIndex::new(&dat)
            .expectation_for("child")
            .unwrap();
        assert_eq!(
            required(&expectation),
            vec![("source".into(), "p.rom".into())]
        );
        assert!(
            !expectation
                .dependencies
                .closure_set_names
                .contains(&"lineage".to_string())
        );
    }

    #[test]
    fn clone_with_complete_own_evidence_is_compatible_without_parent_only_roms() {
        let dat = neogeo_family();
        let expectation = FbNeoExpectationIndex::new(&dat)
            .expectation_for("clone")
            .unwrap();
        let mut evidence =
            ObservedArcadeSetEvidence::new("clone", ObservedEvidenceCompleteness::Complete);
        for (set, name, sha1) in [
            ("bios", "sys.rom", SHA_B),
            ("parent", "p.rom", SHA_P),
            ("clone", "c.rom", SHA_C),
        ] {
            if !evidence.observed_set_names.iter().any(|s| s == set) {
                evidence.observed_set_names.push(set.into());
            }
            evidence.observed_roms.push(MameObservedRom {
                set_name: set.into(),
                name: name.into(),
                size_bytes: Some(4),
                crc32: None,
                sha1: Some(sha1.into()),
            });
        }
        // unique-to-parent.rom is absent and must not matter to the clone.
        let result = audit_fbneo_set(&fbneo(), Some(expectation), &evidence);
        assert_eq!(
            result.state,
            FbNeoSetCompatibilityState::Compatible,
            "{result:#?}"
        );
    }

    #[test]
    fn a_missing_bios_is_reported_as_a_bios_dependency() {
        let dat = neogeo_family();
        let expectation = FbNeoExpectationIndex::new(&dat)
            .expectation_for("parent")
            .unwrap();
        let mut evidence =
            ObservedArcadeSetEvidence::new("parent", ObservedEvidenceCompleteness::Complete);
        evidence.observed_set_names.push("parent".into());
        let result = audit_fbneo_set(&fbneo(), Some(expectation), &evidence);
        assert_eq!(result.state, FbNeoSetCompatibilityState::Incompatible);
        assert!(
            result
                .mismatches
                .iter()
                .any(|m| m.reason == FbNeoMismatchReason::BiosMissing && m.set_name == "bios")
        );
        assert!(
            !result
                .mismatches
                .iter()
                .any(|m| m.reason == FbNeoMismatchReason::ParentMissing)
        );
    }

    #[test]
    fn unproven_dependencies_are_explicit_and_never_compatible() {
        let dat = fbneo_dat(vec![
            DatGameEntry {
                name: "parent".into(),
                roms: vec![dat_rom("p.rom", SHA_U, None)],
                ..Default::default()
            },
            DatGameEntry {
                name: "clone".into(),
                clone_of: Some("parent".into()),
                rom_of: Some("parent".into()),
                // Wrong parent content under the merged name.
                roms: vec![dat_rom("p.rom", SHA_P, Some("p.rom"))],
                ..Default::default()
            },
            DatGameEntry {
                name: "orphan".into(),
                rom_of: Some("missing".into()),
                roms: vec![dat_rom("o.rom", SHA_C, None)],
                ..Default::default()
            },
        ]);
        let index = FbNeoExpectationIndex::new(&dat);
        let clone = index.expectation_for("clone").unwrap();
        assert!(
            clone.roms.is_empty(),
            "an unproven borrow is never required"
        );
        assert_eq!(clone.dependencies.unresolved.len(), 1);
        assert!(clone.dependencies.unresolved[0].contains("Contradictory"));
        let orphan = index.expectation_for("orphan").unwrap();
        assert!(orphan.dependencies.unresolved[0].contains("missing is absent"));

        let mut evidence =
            ObservedArcadeSetEvidence::new("orphan", ObservedEvidenceCompleteness::Complete);
        evidence.observed_set_names.push("orphan".into());
        evidence.observed_roms.push(MameObservedRom {
            set_name: "orphan".into(),
            name: "o.rom".into(),
            size_bytes: Some(4),
            crc32: None,
            sha1: Some(SHA_C.into()),
        });
        let result = audit_fbneo_set(&fbneo(), Some(orphan), &evidence);
        assert_eq!(result.state, FbNeoSetCompatibilityState::Unknown);
    }

    #[test]
    fn device_roms_are_required_transitively_and_cycles_are_refused() {
        let device = |name: &str, refs: &[&str], roms: Vec<DatRomEntry>| DatGameEntry {
            name: name.into(),
            is_device: Some("yes".into()),
            device_refs: refs
                .iter()
                .map(|r| crate::dat::model::DatDeviceRefEntry {
                    name: Some((*r).into()),
                })
                .collect(),
            roms,
            ..Default::default()
        };
        let dat = fbneo_dat(vec![
            device("cpu", &["sound"], vec![]),
            device("sound", &[], vec![dat_rom("snd.rom", SHA_B, None)]),
            DatGameEntry {
                name: "game".into(),
                device_refs: vec![crate::dat::model::DatDeviceRefEntry {
                    name: Some("cpu".into()),
                }],
                roms: vec![dat_rom("g.rom", SHA_P, None)],
                ..Default::default()
            },
            DatGameEntry {
                name: "loop-a".into(),
                rom_of: Some("loop-b".into()),
                ..Default::default()
            },
            DatGameEntry {
                name: "loop-b".into(),
                rom_of: Some("loop-a".into()),
                ..Default::default()
            },
        ]);
        let index = FbNeoExpectationIndex::new(&dat);
        let game = index.expectation_for("game").unwrap();
        assert_eq!(
            required(&game),
            vec![
                ("game".into(), "g.rom".into()),
                ("sound".into(), "snd.rom".into()),
            ]
        );
        assert_eq!(
            game.dependencies.device_refs,
            vec!["cpu".to_string(), "sound".into()]
        );
        let looped = index.expectation_for("loop-a").unwrap();
        assert!(looped.dependencies.unresolved[0].contains("loops"));
    }
}
