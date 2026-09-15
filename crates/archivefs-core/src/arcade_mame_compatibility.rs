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

use crate::dat::archive::{ArchiveMemberEvidence, ArchivePassCompletion};
use crate::dat::dependency::DependencyKind;
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

/// One authoritative AND-group of requirements.  The expectation remains the
/// current MAME model; the typed dependency facts make the group's provenance
/// explicit without introducing another dependency vocabulary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameRequirementGroup {
    pub expectation: MameSetExpectation,
    pub dependency_kinds: Vec<DependencyKind>,
}

impl MameRequirementGroup {
    pub fn from_expectation(expectation: MameSetExpectation) -> Self {
        let mut dependency_kinds = Vec::new();
        if expectation.dependencies.parent.is_some() {
            dependency_kinds.push(DependencyKind::ParentSet);
        }
        if expectation.dependencies.rom_of.is_some() {
            dependency_kinds.push(DependencyKind::RomSource);
        }
        if !expectation.dependencies.device_refs.is_empty() {
            dependency_kinds.push(DependencyKind::Device);
        }
        if expectation.roms.iter().any(|rom| rom.merge.is_some()) {
            dependency_kinds.push(DependencyKind::MergedRom);
        }
        if expectation.disks.iter().any(|disk| disk.merge.is_some()) {
            dependency_kinds.push(DependencyKind::MergedDisk);
        }
        dependency_kinds.sort_unstable();
        dependency_kinds.dedup();
        Self {
            expectation,
            dependency_kinds,
        }
    }
}

/// Authoritative alternatives are OR-ed; requirements within one group are
/// AND-ed.  This is deliberately only a container: it does not infer a layout
/// from collection contents or select a preferred merge policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameCompletenessAlternativeSet {
    pub alternatives: Vec<MameRequirementGroup>,
}

impl MameCompletenessAlternativeSet {
    pub fn new(alternatives: impl IntoIterator<Item = MameRequirementGroup>) -> Self {
        let mut alternatives: Vec<_> = alternatives.into_iter().collect();
        alternatives.sort_by(|left, right| requirement_key(left).cmp(&requirement_key(right)));
        alternatives.dedup();
        Self { alternatives }
    }

    pub fn from_expectations(expectations: impl IntoIterator<Item = MameSetExpectation>) -> Self {
        Self::new(
            expectations
                .into_iter()
                .map(MameRequirementGroup::from_expectation),
        )
    }
}

fn requirement_key(group: &MameRequirementGroup) -> (String, String, Vec<DependencyKind>) {
    let expectation = &group.expectation;
    let members = expectation
        .roms
        .iter()
        .map(|rom| format!("rom:{}:{}:{:?}", rom.set_name, rom.name, rom.checksums))
        .chain(
            expectation
                .disks
                .iter()
                .map(|disk| format!("disk:{}:{:?}:{:?}", disk.set_name, disk.name, disk.sha1)),
        )
        .collect::<Vec<_>>()
        .join("|");
    (
        expectation.set_name.clone(),
        format!("{}:{:?}", members, expectation.dependencies),
        group.dependency_kinds.clone(),
    )
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameCompletenessEvaluation {
    pub set_name: String,
    pub state: MameSetCompatibilityState,
    pub alternatives: Vec<MameSetCompatibility>,
    pub mismatches: Vec<MameMismatch>,
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

/// Completeness of facts already gathered by an existing scan/archive pass.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObservedEvidenceCompleteness {
    Complete,
    Partial,
    NotGathered,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ObservedArcadeEvidenceSource {
    pub source_id: String,
    pub source_path: Option<PathBuf>,
    pub relative_path: Option<PathBuf>,
    pub listing_hash: Option<String>,
    pub parser_version: Option<String>,
    pub observed_size_bytes: Option<u64>,
    pub observed_mtime_ns: Option<i128>,
}

/// Existing catalogue/archive/CHD facts projected into the A1 input shape.
/// Constructing this value performs no filesystem access and no hashing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ObservedArcadeSetEvidence {
    pub set_name: String,
    pub observed_set_names: Vec<String>,
    pub observed_roms: Vec<MameObservedRom>,
    pub observed_disks: Vec<MameObservedDisk>,
    pub sources: Vec<ObservedArcadeEvidenceSource>,
    pub completeness: ObservedEvidenceCompleteness,
    pub collection_provenance_known: bool,
}

impl ObservedArcadeSetEvidence {
    pub fn new(set_name: impl Into<String>, completeness: ObservedEvidenceCompleteness) -> Self {
        Self {
            set_name: set_name.into(),
            observed_set_names: Vec::new(),
            observed_roms: Vec::new(),
            observed_disks: Vec::new(),
            sources: Vec::new(),
            completeness,
            collection_provenance_known: false,
        }
    }

    /// Adapt an existing archive-member hash pass. A complete archive pass
    /// with unhashable members remains partial; names and sizes are retained
    /// but never promoted to cryptographic proof.
    pub fn from_archive_pass(
        set_name: impl Into<String>,
        source_path: Option<PathBuf>,
        relative_path: Option<PathBuf>,
        listing_hash: Option<String>,
        members: &[ArchiveMemberEvidence],
        completion: &ArchivePassCompletion,
        resolved_set_names: impl IntoIterator<Item = String>,
    ) -> Self {
        let set_name = set_name.into();
        let completeness = match completion {
            ArchivePassCompletion::Complete
                if members.iter().all(ArchiveMemberEvidence::is_hash_complete) =>
            {
                ObservedEvidenceCompleteness::Complete
            }
            ArchivePassCompletion::Complete | ArchivePassCompletion::Incomplete { .. } => {
                ObservedEvidenceCompleteness::Partial
            }
        };
        let observed_roms = members
            .iter()
            .filter(|member| !member.is_nested_archive)
            .map(|member| {
                let hashes = member.hashes.as_ref();
                MameObservedRom {
                    set_name: set_name.clone(),
                    name: member.member_name_display.clone(),
                    size_bytes: Some(member.logical_size),
                    crc32: hashes.map(|hashes| hashes.crc32.clone()),
                    sha1: hashes.map(|hashes| hashes.sha1.clone()),
                }
            })
            .collect();
        let mut result = Self::new(set_name, completeness);
        result.observed_set_names = resolved_set_names.into_iter().collect();
        result.observed_roms = observed_roms;
        result.sources.push(ObservedArcadeEvidenceSource {
            source_id: "archive_member_pass".into(),
            source_path,
            relative_path,
            listing_hash,
            parser_version: None,
            observed_size_bytes: None,
            observed_mtime_ns: None,
        });
        result
    }

    /// Feed gathered observations into the existing A1 engine.
    pub fn audit(
        &self,
        mame: &InstalledMameEvidence,
        expectation: MameSetExpectation,
    ) -> MameSetCompatibility {
        if matches!(
            self.completeness,
            ObservedEvidenceCompleteness::NotGathered | ObservedEvidenceCompleteness::Unknown
        ) {
            return result(
                mame,
                expectation,
                MameSetCompatibilityState::Unknown,
                vec![MameMismatch {
                    reason: MameMismatchReason::EvidenceUnavailable,
                    set_name: self.set_name.clone(),
                    member_name: None,
                    expected_size: None,
                    observed_size: None,
                    detail: "Existing archive/catalogue evidence was not gathered for this set"
                        .into(),
                }],
                self.collection_provenance_known,
            );
        }
        let mut compatibility = audit_mame_set_with_completeness(
            mame,
            expectation,
            &self.observed_set_names,
            &self.observed_roms,
            &self.observed_disks,
            self.collection_provenance_known,
            self.completeness,
        );
        if self.completeness == ObservedEvidenceCompleteness::Partial
            && matches!(
                compatibility.state,
                MameSetCompatibilityState::Compatible
                    | MameSetCompatibilityState::CompatibleWithWarnings
            )
        {
            compatibility.state = MameSetCompatibilityState::Unknown;
            compatibility.ready_to_play_state = Some(ReadyToPlayState::Unknown);
            compatibility.mismatches.push(MameMismatch {
                reason: MameMismatchReason::EvidenceUnavailable,
                set_name: self.set_name.clone(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "Only partial cached evidence was available; compatibility is not proven"
                    .into(),
            });
        }
        compatibility.mismatches.sort_by_key(|item| {
            (
                item.reason,
                item.set_name.clone(),
                item.member_name.clone().unwrap_or_default(),
            )
        });
        compatibility
    }

    /// Evaluate authoritative requirement groups with OR semantics.  Each
    /// group is passed through the existing compatibility evaluator, so this
    /// projection cannot turn partial evidence into a missing-member claim.
    pub fn audit_alternatives(
        &self,
        mame: &InstalledMameEvidence,
        alternatives: &MameCompletenessAlternativeSet,
    ) -> MameCompletenessEvaluation {
        let mut results: Vec<_> = alternatives
            .alternatives
            .iter()
            .map(|group| self.audit(mame, group.expectation.clone()))
            .collect();
        results.sort_by(|left, right| {
            requirement_key(&MameRequirementGroup::from_expectation(
                left.expectation.clone(),
            ))
            .cmp(&requirement_key(&MameRequirementGroup::from_expectation(
                right.expectation.clone(),
            )))
        });

        let state = if results.iter().any(|result| {
            matches!(
                result.state,
                MameSetCompatibilityState::Compatible
                    | MameSetCompatibilityState::CompatibleWithWarnings
            )
        }) {
            if results
                .iter()
                .any(|result| result.state == MameSetCompatibilityState::Compatible)
            {
                MameSetCompatibilityState::Compatible
            } else {
                MameSetCompatibilityState::CompatibleWithWarnings
            }
        } else if results
            .iter()
            .any(|result| result.state == MameSetCompatibilityState::Unknown)
            || results.is_empty()
        {
            MameSetCompatibilityState::Unknown
        } else if results
            .iter()
            .all(|result| result.state == MameSetCompatibilityState::Unsupported)
        {
            MameSetCompatibilityState::Unsupported
        } else {
            MameSetCompatibilityState::Incompatible
        };
        let set_name = results
            .first()
            .map(|result| result.set_name.clone())
            .or_else(|| {
                alternatives
                    .alternatives
                    .first()
                    .map(|group| group.expectation.set_name.clone())
            })
            .unwrap_or_default();
        let mut mismatches = results
            .iter()
            .flat_map(|result| result.mismatches.iter().cloned())
            .collect::<Vec<_>>();
        mismatches.sort_by_key(|item| {
            (
                item.reason,
                item.set_name.clone(),
                item.member_name.clone().unwrap_or_default(),
                item.detail.clone(),
            )
        });
        mismatches.dedup();
        MameCompletenessEvaluation {
            set_name,
            state,
            alternatives: results,
            mismatches,
        }
    }
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

    /// Return the authoritative alternatives currently expressible by the
    /// imported DAT.  A normal set has one group.  A clone/ROM-source set also
    /// gets a conservative standalone group containing the authoritative
    /// closure's declared members under the selected set name; this models a
    /// duplicated standalone layout without inspecting the collection.
    pub fn alternatives_for(&self, set_name: &str) -> Option<MameCompletenessAlternativeSet> {
        let resolved = self.expectation_for(set_name)?;
        let selected = self.dat.games.iter().find(|game| game.name == set_name)?;
        if selected.clone_of.is_none() && selected.rom_of.is_none() {
            return Some(MameCompletenessAlternativeSet::from_expectations([
                resolved,
            ]));
        }
        let mut standalone = MameSetExpectation::from_game(selected);
        standalone.parent = None;
        standalone.rom_of = None;
        standalone.dependencies.parent = None;
        standalone.dependencies.rom_of = None;
        standalone.dependencies.closure_set_names = vec![set_name.to_string()];
        standalone.roms = resolved
            .roms
            .iter()
            .map(|rom| MameExpectedRom {
                set_name: set_name.to_string(),
                ..rom.clone()
            })
            .collect();
        standalone.disks = resolved
            .disks
            .iter()
            .map(|disk| MameExpectedDisk {
                set_name: set_name.to_string(),
                ..disk.clone()
            })
            .collect();
        Some(MameCompletenessAlternativeSet::new([
            MameRequirementGroup::from_expectation(resolved),
            MameRequirementGroup::from_expectation(standalone),
        ]))
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
    audit_mame_set_with_completeness(
        mame,
        expectation,
        observed_set_names,
        observed_roms,
        observed_disks,
        collection_provenance_known,
        ObservedEvidenceCompleteness::Complete,
    )
}

fn audit_mame_set_with_completeness(
    mame: &InstalledMameEvidence,
    expectation: MameSetExpectation,
    observed_set_names: &[String],
    observed_roms: &[MameObservedRom],
    observed_disks: &[MameObservedDisk],
    collection_provenance_known: bool,
    completeness: ObservedEvidenceCompleteness,
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
        if completeness == ObservedEvidenceCompleteness::Complete
            && !observed_set_names.iter().any(|name| name == dependency)
        {
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
            if completeness != ObservedEvidenceCompleteness::Complete {
                continue;
            }
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
            if completeness != ObservedEvidenceCompleteness::Complete {
                continue;
            }
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
    use crate::dat::model::{
        DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatRomEntry, DatSource,
    };

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
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::MAMEArcade,
                file_path: "synthetic-mame.xml".into(),
                name: Some("Synthetic MAME".into()),
                description: None,
                version: None,
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: 3,
                rom_count: 0,
                parse_warnings: Vec::new(),
                packing_policy: DatPackingPolicy::Standard,
            },
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

    #[test]
    fn archive_bridge_preserves_hashes_and_complete_evidence() {
        let member = ArchiveMemberEvidence {
            archive_path: PathBuf::from("/roms/pacman.zip"),
            member_name_raw: b"pacman.6e".to_vec(),
            member_name_display: "pacman.6e".into(),
            index: 0,
            logical_size: 4,
            is_nested_archive: false,
            status: crate::dat::archive::ArchiveMemberStatus::HashComplete,
            hashes: Some(crate::dat::archive::ArchiveMemberHashes {
                crc32: "aaaaaaaa".into(),
                md5: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                sha1: "1111111111111111111111111111111111111111".into(),
                sha256: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into(),
            }),
        };
        let bridge = ObservedArcadeSetEvidence::from_archive_pass(
            "pacman",
            Some(PathBuf::from("/roms/pacman.zip")),
            Some(PathBuf::from("pacman.zip")),
            Some("listing".into()),
            &[member],
            &ArchivePassCompletion::Complete,
            ["pacman".into()],
        );
        assert_eq!(bridge.completeness, ObservedEvidenceCompleteness::Complete);
        assert_eq!(
            bridge.observed_roms[0].sha1.as_deref(),
            Some("1111111111111111111111111111111111111111")
        );
        let result = bridge.audit(&mame(), MameSetExpectation::from_game(&game(None)));
        assert_eq!(result.state, MameSetCompatibilityState::Compatible);
    }

    #[test]
    fn partial_archive_evidence_cannot_be_promoted_to_compatible() {
        let mut bridge =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Partial);
        bridge.observed_set_names.push("pacman".into());
        bridge.observed_roms.push(observed());
        bridge.collection_provenance_known = true;
        let result = bridge.audit(&mame(), MameSetExpectation::from_game(&game(None)));
        assert_eq!(result.state, MameSetCompatibilityState::Unknown);
        assert_eq!(result.ready_to_play_state, Some(ReadyToPlayState::Unknown));
    }

    #[test]
    fn partial_absence_remains_unknown_instead_of_missing() {
        let bridge =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Partial);
        let result = bridge.audit(&mame(), MameSetExpectation::from_game(&game(None)));
        assert_eq!(result.state, MameSetCompatibilityState::Unknown);
        assert!(
            !result
                .mismatches
                .iter()
                .any(|item| item.reason == MameMismatchReason::RequiredFileMissing)
        );
        assert!(
            result
                .mismatches
                .iter()
                .any(|item| item.reason == MameMismatchReason::EvidenceUnavailable)
        );
    }

    #[test]
    fn partial_observed_contradiction_remains_incompatible() {
        let mut bridge =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Partial);
        let mut wrong = observed();
        wrong.size_bytes = Some(3);
        bridge.observed_set_names.push("pacman".into());
        bridge.observed_roms.push(wrong);
        let result = bridge.audit(&mame(), MameSetExpectation::from_game(&game(None)));
        assert_eq!(result.state, MameSetCompatibilityState::Incompatible);
        assert!(
            result
                .mismatches
                .iter()
                .any(|item| item.reason == MameMismatchReason::WrongSize)
        );
    }

    #[test]
    fn partial_dependency_absence_is_not_proven_missing() {
        let mut bridge =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Partial);
        bridge.observed_set_names.push("pacman".into());
        bridge.observed_roms.push(observed());
        let mut expectation = MameSetExpectation::from_game(&game(None));
        expectation.dependencies.closure_set_names = vec!["pacman".into(), "parent".into()];
        expectation.parent = Some("parent".into());
        expectation.dependencies.parent = Some("parent".into());
        let result = bridge.audit(&mame(), expectation);
        assert_eq!(result.state, MameSetCompatibilityState::Unknown);
        assert!(
            !result
                .mismatches
                .iter()
                .any(|item| item.reason == MameMismatchReason::ParentMissing)
        );
    }

    fn alternatives(
        expectations: impl IntoIterator<Item = MameSetExpectation>,
    ) -> MameCompletenessAlternativeSet {
        MameCompletenessAlternativeSet::from_expectations(expectations)
    }

    #[test]
    fn identical_single_layout_is_complete() {
        let result =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Complete);
        let mut result = result;
        result.observed_set_names.push("pacman".into());
        result.observed_roms.push(observed());
        let evaluation = result.audit_alternatives(
            &mame(),
            &alternatives([MameSetExpectation::from_game(&game(None))]),
        );
        assert_eq!(evaluation.state, MameSetCompatibilityState::Compatible);
        assert_eq!(evaluation.alternatives.len(), 1);
    }

    #[test]
    fn first_incomplete_second_complete_is_complete() {
        let mut incomplete = MameSetExpectation::from_game(&game(None));
        incomplete.roms[0].name = "missing".into();
        let mut evidence =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Complete);
        evidence.observed_set_names.push("pacman".into());
        evidence.observed_roms.push(observed());
        let evaluation = evidence.audit_alternatives(
            &mame(),
            &alternatives([incomplete, MameSetExpectation::from_game(&game(None))]),
        );
        assert_eq!(evaluation.state, MameSetCompatibilityState::Compatible);
    }

    #[test]
    fn unrelated_missing_unused_alternative_does_not_block() {
        let mut first = MameSetExpectation::from_game(&game(None));
        first.roms[0].name = "missing-a".into();
        let mut unused = first.clone();
        unused.roms[0].name = "missing-b".into();
        let evidence =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Complete);
        let mut evidence = evidence;
        evidence.observed_set_names.push("pacman".into());
        evidence.observed_roms.push(observed());
        let evaluation = evidence.audit_alternatives(
            &mame(),
            &alternatives([first, unused, MameSetExpectation::from_game(&game(None))]),
        );
        assert_eq!(evaluation.state, MameSetCompatibilityState::Compatible);
    }

    #[test]
    fn all_alternatives_proven_incomplete_are_incompatible() {
        let mut first = MameSetExpectation::from_game(&game(None));
        first.roms[0].name = "missing-a".into();
        let mut second = first.clone();
        second.roms[0].name = "missing-b".into();
        let evidence =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Complete);
        let evaluation = evidence.audit_alternatives(&mame(), &alternatives([first, second]));
        assert_eq!(evaluation.state, MameSetCompatibilityState::Incompatible);
    }

    #[test]
    fn incomplete_plus_unknown_is_unknown_and_partial_never_becomes_missing() {
        let mut first = MameSetExpectation::from_game(&game(None));
        first.roms[0].name = "missing".into();
        let mut evidence =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Partial);
        evidence.observed_set_names.push("pacman".into());
        let evaluation = evidence.audit_alternatives(
            &mame(),
            &alternatives([first, MameSetExpectation::from_game(&game(None))]),
        );
        assert_eq!(evaluation.state, MameSetCompatibilityState::Unknown);
        assert!(
            !evaluation
                .mismatches
                .iter()
                .any(|item| { item.reason == MameMismatchReason::RequiredFileMissing })
        );
        assert!(
            evaluation
                .mismatches
                .iter()
                .any(|item| { item.reason == MameMismatchReason::EvidenceUnavailable })
        );
    }

    #[test]
    fn dependency_kinds_are_preserved_for_bios_device_chd_and_parent_groups() {
        let mut expectation = MameSetExpectation::from_game(&game(None));
        expectation.dependencies.parent = Some("parent".into());
        expectation.dependencies.rom_of = Some("bios".into());
        expectation.dependencies.device_refs = vec!["sound".into()];
        expectation.roms[0].merge = Some("shared".into());
        expectation.disks.push(MameExpectedDisk {
            set_name: "pacman".into(),
            name: Some("disk".into()),
            sha1: Some("2222222222222222222222222222222222222222".into()),
            status: None,
            merge: Some("parent-disk".into()),
        });
        let group = MameRequirementGroup::from_expectation(expectation);
        assert_eq!(
            group.dependency_kinds,
            vec![
                DependencyKind::ParentSet,
                DependencyKind::RomSource,
                DependencyKind::MergedRom,
                DependencyKind::MergedDisk,
                DependencyKind::Device,
            ]
        );
    }

    #[test]
    fn duplicate_groups_and_result_order_are_deterministic() {
        let expectation = MameSetExpectation::from_game(&game(None));
        let set = MameCompletenessAlternativeSet::new([
            MameRequirementGroup::from_expectation(expectation.clone()),
            MameRequirementGroup::from_expectation(expectation.clone()),
        ]);
        assert_eq!(set.alternatives.len(), 1);
        let a = alternatives([expectation.clone()]);
        let b = alternatives([expectation]);
        let mut evidence =
            ObservedArcadeSetEvidence::new("pacman", ObservedEvidenceCompleteness::Complete);
        evidence.observed_set_names.push("pacman".into());
        evidence.observed_roms.push(observed());
        assert_eq!(
            evidence.audit_alternatives(&mame(), &a),
            evidence.audit_alternatives(&mame(), &b)
        );
    }

    #[test]
    fn authority_index_exposes_a_standalone_clone_alternative() {
        let selected = game(None);
        let parent = DatGameEntry {
            name: "parent".into(),
            roms: vec![DatRomEntry {
                name: "parent-rom".into(),
                size_bytes: Some(1),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut selected = selected;
        selected.name = "clone".into();
        selected.clone_of = Some("parent".into());
        let dat = ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::MAMEArcade,
                file_path: "synthetic.xml".into(),
                name: None,
                description: None,
                version: None,
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: 2,
                rom_count: 0,
                parse_warnings: Vec::new(),
                packing_policy: DatPackingPolicy::Standard,
            },
            games: vec![selected, parent],
        };
        let set = MameExpectationIndex::new(&dat)
            .alternatives_for("clone")
            .unwrap();
        assert_eq!(set.alternatives.len(), 2);
        assert!(
            set.alternatives
                .iter()
                .any(|group| { group.expectation.dependencies.parent.is_none() })
        );
    }
}
