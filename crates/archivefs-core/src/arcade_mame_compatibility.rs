//! Read-only per-set compatibility against one pinned installed MAME build.
//!
//! This module deliberately consumes evidence gathered by the caller.  It
//! does not execute MAME, walk a rompath, hash files, or mutate a catalogue.
//! The existing [`crate::identity_source::mame_listxml`] importer remains the
//! authoritative parser for local `-listxml` output; this layer gives that
//! parsed expectation a small, provenance-bearing compatibility projection.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::dat::model::{DatChecksum, DatGameEntry, DatRomEntry, ParsedDat};
use crate::identity_source::mame_listxml::ImportedMameListxmlSource;
use crate::ready_to_play::ReadyToPlayState;

pub const MAME_COMPATIBILITY_SCHEMA_VERSION: u32 = 1;

/// Identity of the MAME expectation source used by an audit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstalledMameEvidence {
    pub executable_path: PathBuf,
    pub reported_version: String,
    pub metadata_source: PathBuf,
    pub evidence_timestamp_unix: Option<u64>,
    /// Hash of the exact listxml artifact, when it was retained by the
    /// existing importer.
    pub metadata_build_identity: Option<String>,
    pub effective_rompath: Vec<PathBuf>,
    pub parser_schema_version: u32,
    pub usable: bool,
}

impl InstalledMameEvidence {
    pub fn new(
        executable_path: impl Into<PathBuf>,
        reported_version: impl Into<String>,
        metadata_source: impl Into<PathBuf>,
        effective_rompath: impl IntoIterator<Item = PathBuf>,
        metadata_build_identity: Option<String>,
        evidence_timestamp_unix: Option<u64>,
    ) -> Self {
        let reported_version = reported_version.into();
        Self {
            executable_path: executable_path.into(),
            usable: !reported_version.trim().is_empty(),
            reported_version,
            metadata_source: metadata_source.into(),
            evidence_timestamp_unix,
            metadata_build_identity,
            effective_rompath: effective_rompath.into_iter().collect(),
            parser_schema_version: MAME_COMPATIBILITY_SCHEMA_VERSION,
        }
    }

    /// Builds pinned evidence from an already imported listxml artifact.
    /// Version capture remains explicit because the parser intentionally does
    /// not infer release provenance from the delivered filename.
    pub fn from_imported(
        executable_path: impl Into<PathBuf>,
        reported_version: impl Into<String>,
        metadata_source: &Path,
        effective_rompath: impl IntoIterator<Item = PathBuf>,
        imported: &ImportedMameListxmlSource,
        evidence_timestamp_unix: Option<u64>,
    ) -> Self {
        Self::new(
            executable_path,
            reported_version,
            metadata_source.to_path_buf(),
            effective_rompath,
            Some(imported.artifact_sha256.clone()),
            evidence_timestamp_unix,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MameSetCompatibilityState {
    Compatible,
    CompatibleWithWarnings,
    Incompatible,
    Unknown,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MameMismatchReason {
    RequiredFileMissing,
    WrongSize,
    CrcMismatch,
    Sha1Mismatch,
    ParentMissing,
    BiosMissing,
    DeviceDependencyMissing,
    ChdMissing,
    ChdHashMismatch,
    NeedsRedump,
    BadDump,
    NoDump,
    BestAvailableImperfect,
    SetUnknownToInstalledMame,
    VersionProvenanceUnknown,
    EvidenceUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameMismatch {
    pub reason: MameMismatchReason,
    pub set_name: String,
    pub member_name: Option<String>,
    pub expected_size: Option<u64>,
    pub observed_size: Option<u64>,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameExpectedRom {
    pub set_name: String,
    pub name: String,
    pub size_bytes: Option<u64>,
    pub checksums: Vec<DatChecksum>,
    pub status: Option<String>,
    pub merge: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameExpectedDisk {
    pub set_name: String,
    pub name: Option<String>,
    pub sha1: Option<String>,
    pub status: Option<String>,
    pub merge: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameDependencyEvidence {
    pub set_name: String,
    pub parent: Option<String>,
    pub rom_of: Option<String>,
    pub device_refs: Vec<String>,
    pub closure_set_names: Vec<String>,
    pub runnable: Option<String>,
    pub is_bios: Option<String>,
    pub is_device: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameSetExpectation {
    pub set_name: String,
    pub parent: Option<String>,
    pub rom_of: Option<String>,
    pub runnable: Option<String>,
    pub is_bios: Option<String>,
    pub is_device: Option<String>,
    pub driver_status: Option<String>,
    pub roms: Vec<MameExpectedRom>,
    pub disks: Vec<MameExpectedDisk>,
    pub dependencies: MameDependencyEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameObservedRom {
    pub set_name: String,
    pub name: String,
    pub size_bytes: Option<u64>,
    pub crc32: Option<String>,
    pub sha1: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameObservedDisk {
    pub set_name: String,
    pub name: Option<String>,
    pub sha1: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameSetCompatibility {
    pub set_name: String,
    pub state: MameSetCompatibilityState,
    pub mame: InstalledMameEvidence,
    pub expectation: MameSetExpectation,
    pub mismatches: Vec<MameMismatch>,
    pub driver_status: Option<String>,
    pub collection_provenance_known: bool,
    pub ready_to_play_state: Option<ReadyToPlayState>,
}

impl MameSetExpectation {
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
                .map(|disk| MameExpectedDisk {
                    set_name: game.name.clone(),
                    name: disk.name.clone(),
                    sha1: disk.sha1.clone(),
                    status: disk.status.clone(),
                    merge: disk.merge.clone(),
                })
                .collect(),
            dependencies: MameDependencyEvidence {
                set_name: game.name.clone(),
                parent: game.clone_of.clone(),
                rom_of: game.rom_of.clone(),
                device_refs: game
                    .device_refs
                    .iter()
                    .filter_map(|reference| reference.name.clone())
                    .collect(),
                closure_set_names: vec![game.name.clone()],
                runnable: game.runnable.clone(),
                is_bios: game.is_bios.clone(),
                is_device: game.is_device.clone(),
            },
        }
    }
}

/// Indexed expectations from one imported MAME listxml artifact.
#[derive(Clone, Debug)]
pub struct MameExpectationIndex<'a> {
    dat: &'a ParsedDat,
}

impl<'a> MameExpectationIndex<'a> {
    pub fn new(dat: &'a ParsedDat) -> Self {
        Self { dat }
    }

    /// Returns the selected machine and its explicitly declared parent/ROM
    /// and device closure. Missing closure nodes remain visible as dependency
    /// evidence and are reported by the compatibility audit.
    pub fn expectation_for(&self, set_name: &str) -> Option<MameSetExpectation> {
        let mut names = BTreeSet::new();
        let mut queue = vec![set_name.to_string()];
        while let Some(name) = queue.pop() {
            if !names.insert(name.clone()) {
                continue;
            }
            let Some(game) = self.dat.games.iter().find(|game| game.name == name) else {
                continue;
            };
            if let Some(parent) = game.clone_of.as_ref().or(game.rom_of.as_ref()) {
                queue.push(parent.clone());
            }
            queue.extend(
                game.device_refs
                    .iter()
                    .filter_map(|reference| reference.name.clone()),
            );
        }
        let selected = self.dat.games.iter().find(|game| game.name == set_name)?;
        let mut expectation = MameSetExpectation::from_game(selected);
        expectation.dependencies.closure_set_names = names.iter().cloned().collect();
        for name in names.into_iter().filter(|name| name != set_name) {
            if let Some(game) = self.dat.games.iter().find(|game| game.name == name) {
                expectation
                    .roms
                    .extend(game.roms.iter().map(|rom| expected_rom(&game.name, rom)));
                expectation
                    .disks
                    .extend(game.disks.iter().map(|disk| MameExpectedDisk {
                        set_name: game.name.clone(),
                        name: disk.name.clone(),
                        sha1: disk.sha1.clone(),
                        status: disk.status.clone(),
                        merge: disk.merge.clone(),
                    }));
            }
        }
        Some(expectation)
    }
}

fn expected_rom(set_name: &str, rom: &DatRomEntry) -> MameExpectedRom {
    MameExpectedRom {
        set_name: set_name.to_string(),
        name: rom.name.clone(),
        size_bytes: rom.size_bytes,
        checksums: rom.checksums(),
        status: rom.status.clone(),
        merge: rom.merge.clone(),
    }
}

fn checksum<'a>(rom: &'a MameExpectedRom, algorithm: &str) -> Option<&'a str> {
    rom.checksums
        .iter()
        .find(|checksum| checksum.algorithm.label() == algorithm)
        .map(|checksum| checksum.value.as_str())
}

fn status_is(status: Option<&str>, expected: &str) -> bool {
    status.is_some_and(|status| status.eq_ignore_ascii_case(expected))
}

fn status_is_redump(status: Option<&str>) -> bool {
    status.is_some_and(|status| {
        status.eq_ignore_ascii_case("needs_redump") || status.eq_ignore_ascii_case("needs redump")
    })
}

fn mismatch(
    reason: MameMismatchReason,
    expected: &MameExpectedRom,
    observed: Option<&MameObservedRom>,
    detail: impl Into<String>,
) -> MameMismatch {
    MameMismatch {
        reason,
        set_name: expected.set_name.clone(),
        member_name: Some(expected.name.clone()),
        expected_size: expected.size_bytes,
        observed_size: observed.and_then(|item| item.size_bytes),
        detail: detail.into(),
    }
}

/// Compare one selected set and its supplied resolved content evidence.
///
/// `observed_roms` should contain the result of MAME's configured rompath
/// resolution, including members found through split/merged parent and BIOS
/// paths. This function intentionally does not assume an archive layout.
pub fn audit_mame_set(
    mame: &InstalledMameEvidence,
    expectation: MameSetExpectation,
    observed_set_names: &[String],
    observed_roms: &[MameObservedRom],
    observed_disks: &[MameObservedDisk],
    collection_provenance_known: bool,
) -> MameSetCompatibility {
    if !mame.usable {
        return result(
            mame,
            expectation,
            MameSetCompatibilityState::Unknown,
            vec![MameMismatch {
                reason: MameMismatchReason::VersionProvenanceUnknown,
                set_name: String::new(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "Installed MAME version evidence is not usable".into(),
            }],
            collection_provenance_known,
        );
    }

    let mut mismatches = Vec::new();
    for dependency in &expectation.dependencies.closure_set_names {
        if !observed_set_names.iter().any(|name| name == dependency) {
            let reason = if dependency == &expectation.set_name {
                MameMismatchReason::RequiredFileMissing
            } else if expectation
                .dependencies
                .device_refs
                .iter()
                .any(|name| name == dependency)
            {
                MameMismatchReason::DeviceDependencyMissing
            } else if expectation.dependencies.parent.as_deref() == Some(dependency)
                || expectation.dependencies.rom_of.as_deref() == Some(dependency)
            {
                MameMismatchReason::ParentMissing
            } else {
                MameMismatchReason::BiosMissing
            };
            mismatches.push(MameMismatch {
                reason,
                set_name: dependency.clone(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "Required MAME dependency set was not resolved through the supplied rompath evidence".into(),
            });
        }
    }
    for expected in &expectation.roms {
        let observed = observed_roms
            .iter()
            .find(|item| item.set_name == expected.set_name && item.name == expected.name);
        if status_is(expected.status.as_deref(), "nodump") {
            mismatches.push(mismatch(
                MameMismatchReason::NoDump,
                expected,
                observed,
                "MAME declares no known dump for this member",
            ));
            continue;
        }
        let Some(observed) = observed else {
            mismatches.push(mismatch(
                MameMismatchReason::RequiredFileMissing,
                expected,
                None,
                "Required ROM member was not resolved through the supplied rompath evidence",
            ));
            continue;
        };
        if expected.size_bytes != observed.size_bytes {
            mismatches.push(mismatch(
                MameMismatchReason::WrongSize,
                expected,
                Some(observed),
                "Observed ROM size does not match the pinned MAME expectation",
            ));
            continue;
        }
        if let Some(expected_crc) = checksum(expected, "CRC32") {
            let Some(observed_crc) = observed.crc32.as_deref() else {
                mismatches.push(mismatch(
                    MameMismatchReason::EvidenceUnavailable,
                    expected,
                    Some(observed),
                    "Observed CRC32 evidence was not gathered",
                ));
                continue;
            };
            if observed_crc != expected_crc {
                mismatches.push(mismatch(
                    MameMismatchReason::CrcMismatch,
                    expected,
                    Some(observed),
                    "Observed CRC32 does not match the pinned MAME expectation",
                ));
                continue;
            }
        }
        if let Some(expected_sha1) = checksum(expected, "SHA-1") {
            let Some(observed_sha1) = observed.sha1.as_deref() else {
                mismatches.push(mismatch(
                    MameMismatchReason::EvidenceUnavailable,
                    expected,
                    Some(observed),
                    "Observed SHA-1 evidence was not gathered",
                ));
                continue;
            };
            if observed_sha1 != expected_sha1 {
                mismatches.push(mismatch(
                    MameMismatchReason::Sha1Mismatch,
                    expected,
                    Some(observed),
                    "Observed SHA-1 does not match the pinned MAME expectation",
                ));
                continue;
            }
        }
        if status_is(expected.status.as_deref(), "baddump") {
            mismatches.push(mismatch(
                MameMismatchReason::BadDump,
                expected,
                Some(observed),
                "MAME accepts this member but marks the dump as imperfect",
            ));
        } else if status_is_redump(expected.status.as_deref()) {
            mismatches.push(mismatch(
                MameMismatchReason::NeedsRedump,
                expected,
                Some(observed),
                "MAME accepts this member but records a redump requirement",
            ));
        }
    }

    for expected in &expectation.disks {
        let observed = observed_disks
            .iter()
            .find(|item| item.set_name == expected.set_name && item.name == expected.name);
        if expected.sha1.is_none() {
            mismatches.push(MameMismatch {
                reason: MameMismatchReason::ChdMissing,
                set_name: expected.set_name.clone(),
                member_name: expected.name.clone(),
                expected_size: None,
                observed_size: None,
                detail: "CHD identity is unavailable in the pinned expectation".into(),
            });
        } else if observed.is_none() {
            mismatches.push(MameMismatch {
                reason: MameMismatchReason::ChdMissing,
                set_name: expected.set_name.clone(),
                member_name: expected.name.clone(),
                expected_size: None,
                observed_size: None,
                detail: "Required CHD was not resolved through the supplied rompath evidence"
                    .into(),
            });
        } else if observed.and_then(|item| item.sha1.as_deref()) != expected.sha1.as_deref() {
            mismatches.push(MameMismatch {
                reason: MameMismatchReason::ChdHashMismatch,
                set_name: expected.set_name.clone(),
                member_name: expected.name.clone(),
                expected_size: None,
                observed_size: None,
                detail: "Observed CHD identity does not match the pinned MAME expectation".into(),
            });
        }
    }

    let state = if mismatches.iter().any(|item| {
        matches!(
            item.reason,
            MameMismatchReason::RequiredFileMissing
                | MameMismatchReason::WrongSize
                | MameMismatchReason::CrcMismatch
                | MameMismatchReason::Sha1Mismatch
                | MameMismatchReason::ParentMissing
                | MameMismatchReason::BiosMissing
                | MameMismatchReason::DeviceDependencyMissing
                | MameMismatchReason::ChdMissing
                | MameMismatchReason::ChdHashMismatch
        )
    }) {
        MameSetCompatibilityState::Incompatible
    } else if mismatches
        .iter()
        .any(|item| matches!(item.reason, MameMismatchReason::NoDump))
    {
        MameSetCompatibilityState::Unsupported
    } else if mismatches
        .iter()
        .any(|item| item.reason == MameMismatchReason::EvidenceUnavailable)
    {
        MameSetCompatibilityState::Unknown
    } else if mismatches.is_empty() {
        MameSetCompatibilityState::Compatible
    } else {
        MameSetCompatibilityState::CompatibleWithWarnings
    };
    result(
        mame,
        expectation,
        state,
        mismatches,
        collection_provenance_known,
    )
}

fn result(
    mame: &InstalledMameEvidence,
    expectation: MameSetExpectation,
    state: MameSetCompatibilityState,
    mut mismatches: Vec<MameMismatch>,
    collection_provenance_known: bool,
) -> MameSetCompatibility {
    mismatches.sort_by_key(|item| {
        (
            item.reason,
            item.set_name.clone(),
            item.member_name.clone().unwrap_or_default(),
        )
    });
    let driver_status = expectation.driver_status.clone();
    let driver_warning = expectation
        .driver_status
        .as_deref()
        .is_some_and(|status| !status.eq_ignore_ascii_case("good"));
    let ready_to_play_state = match state {
        MameSetCompatibilityState::Compatible if driver_warning => {
            Some(ReadyToPlayState::ReadyWithWarnings)
        }
        MameSetCompatibilityState::Compatible => Some(ReadyToPlayState::Ready),
        MameSetCompatibilityState::CompatibleWithWarnings => {
            Some(ReadyToPlayState::ReadyWithWarnings)
        }
        MameSetCompatibilityState::Incompatible => Some(ReadyToPlayState::Blocked),
        MameSetCompatibilityState::Unsupported => Some(ReadyToPlayState::Unsupported),
        MameSetCompatibilityState::Unknown => Some(ReadyToPlayState::Unknown),
    };
    MameSetCompatibility {
        set_name: expectation.set_name.clone(),
        state,
        mame: mame.clone(),
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

    fn mame() -> InstalledMameEvidence {
        InstalledMameEvidence::new(
            "/usr/games/mame",
            "0.264",
            "/tmp/mame-listxml",
            [PathBuf::from("/roms")],
            Some("xml-sha".into()),
            None,
        )
    }

    fn game(status: Option<&str>) -> DatGameEntry {
        DatGameEntry {
            name: "pacman".into(),
            roms: vec![DatRomEntry {
                name: "pacman.6e".into(),
                size_bytes: Some(4),
                crc32: Some("aaaaaaaa".into()),
                sha1: Some("1111111111111111111111111111111111111111".into()),
                status: status.map(str::to_string),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn observed() -> MameObservedRom {
        MameObservedRom {
            set_name: "pacman".into(),
            name: "pacman.6e".into(),
            size_bytes: Some(4),
            crc32: Some("aaaaaaaa".into()),
            sha1: Some("1111111111111111111111111111111111111111".into()),
        }
    }

    #[test]
    fn clean_set_is_compatible_even_when_collection_provenance_is_unknown() {
        let result = audit_mame_set(
            &mame(),
            MameSetExpectation::from_game(&game(None)),
            &["pacman".into()],
            &[observed()],
            &[],
            false,
        );
        assert_eq!(result.state, MameSetCompatibilityState::Compatible);
        assert_eq!(result.ready_to_play_state, Some(ReadyToPlayState::Ready));
        assert!(!result.collection_provenance_known);
    }

    #[test]
    fn wrong_size_is_incompatible() {
        let mut item = observed();
        item.size_bytes = Some(3);
        let result = audit_mame_set(
            &mame(),
            MameSetExpectation::from_game(&game(None)),
            &["pacman".into()],
            &[item],
            &[],
            true,
        );
        assert_eq!(result.state, MameSetCompatibilityState::Incompatible);
        assert_eq!(result.mismatches[0].reason, MameMismatchReason::WrongSize);
    }

    #[test]
    fn needs_redump_is_warning_not_missing() {
        let result = audit_mame_set(
            &mame(),
            MameSetExpectation::from_game(&game(Some("baddump"))),
            &["pacman".into()],
            &[observed()],
            &[],
            true,
        );
        assert_eq!(
            result.state,
            MameSetCompatibilityState::CompatibleWithWarnings
        );
        assert_eq!(result.mismatches[0].reason, MameMismatchReason::BadDump);
    }

    #[test]
    fn missing_hash_evidence_remains_unknown() {
        let mut item = observed();
        item.sha1 = None;
        let result = audit_mame_set(
            &mame(),
            MameSetExpectation::from_game(&game(None)),
            &["pacman".into()],
            &[item],
            &[],
            true,
        );
        assert_eq!(result.state, MameSetCompatibilityState::Unknown);
        assert_eq!(
            result.mismatches[0].reason,
            MameMismatchReason::EvidenceUnavailable
        );
    }

    #[test]
    fn imperfect_driver_is_separate_but_projects_a_warning() {
        let mut imperfect = game(None);
        imperfect
            .original_metadata
            .fields
            .insert("driver.status".into(), "imperfect".into());
        let result = audit_mame_set(
            &mame(),
            MameSetExpectation::from_game(&imperfect),
            &["pacman".into()],
            &[observed()],
            &[],
            true,
        );
        assert_eq!(result.state, MameSetCompatibilityState::Compatible);
        assert_eq!(
            result.ready_to_play_state,
            Some(ReadyToPlayState::ReadyWithWarnings)
        );
        assert!(result.mismatches.is_empty());
    }

    #[test]
    fn missing_member_is_not_unknown() {
        let result = audit_mame_set(
            &mame(),
            MameSetExpectation::from_game(&game(None)),
            &["pacman".into()],
            &[],
            &[],
            true,
        );
        assert_eq!(result.state, MameSetCompatibilityState::Incompatible);
        assert_eq!(
            result.mismatches[0].reason,
            MameMismatchReason::RequiredFileMissing
        );
    }

    #[test]
    fn expectation_index_includes_parent_and_device_closure_names() {
        let child = DatGameEntry {
            name: "child".into(),
            clone_of: Some("parent".into()),
            device_refs: vec![crate::dat::model::DatDeviceRefEntry {
                name: Some("qsound".into()),
            }],
            ..Default::default()
        };
        let parent = DatGameEntry {
            name: "parent".into(),
            ..Default::default()
        };
        let device = DatGameEntry {
            name: "qsound".into(),
            is_device: Some("yes".into()),
            runnable: Some("no".into()),
            ..Default::default()
        };
        let dat = ParsedDat {
            source: Default::default(),
            games: vec![child, parent, device],
        };
        let expectation = MameExpectationIndex::new(&dat)
            .expectation_for("child")
            .unwrap();
        assert_eq!(
            expectation.dependencies.closure_set_names,
            vec!["child", "parent", "qsound"]
        );
    }
}
