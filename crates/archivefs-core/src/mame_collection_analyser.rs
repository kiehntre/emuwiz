//! Read-only MAME collection health and update-readiness analysis.
//!
//! This module deliberately does not run MAME, download a catalogue, rename a
//! file, or rebuild an archive.  A caller supplies the parsed current
//! `-listxml` catalogue and a bounded inventory of what it inspected.  Keeping
//! those two evidence sources separate makes an incomplete inspection visible
//! instead of turning it into a false verification result.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::dat::model::DatRomEntry;
use crate::dat::model::{DatGameEntry, ParsedDat};

mod storage;
mod versions;
pub use storage::{
    MameMemberEvidence, MameRequirementEvidence, MameRunnability, MameStorageEvidence,
    MameVerifiedChd,
};

pub const MAME_COLLECTION_ANALYSER_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameCollectionStyle {
    Merged,
    Split,
    NonMerged,
    ExplodedSelfContained,
    Mixed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameVersionCompatibility {
    CompatibleWithCurrentMame,
    /// Legacy serialized value; no longer emitted from collection statistics.
    MostlyCompatible,
    /// Legacy serialized value; no longer emitted from collection statistics.
    SignificantMismatch,
    CatalogueVersionMismatch,
    /// Parsed release tokens agree; this is not build identity or runnability.
    CatalogueVersionMatch,
    ExactSourceVersionUnknown,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameSetHealth {
    /// Required clean storage verified; never a claim of machine runnability.
    Good,
    Bad,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameObservedMember {
    pub name: String,
    pub size_bytes: Option<u64>,
    pub crc32: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameObservedSet {
    pub name: String,
    pub location: PathBuf,
    pub directory: bool,
    pub members: Vec<MameObservedMember>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameCollectionInventory {
    pub collection_root: PathBuf,
    pub sets: Vec<MameObservedSet>,
    pub inspected_completely: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameDependencyKind {
    GameSpecificRom,
    ParentSharedRom,
    BiosRom,
    DeviceRom,
    PldGalPal,
    Chd,
    NoDump,
    BadDump,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MamePreservationStatus {
    VerifiedDumpExpected,
    NoVerifiedDumpExists,
    NeedsRedump,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameMissingDependency {
    pub name: String,
    pub kind: MameDependencyKind,
    pub preservation_status: MamePreservationStatus,
    pub affected_sets: Vec<String>,
    pub affected_families: Vec<String>,
    pub occurrences: usize,
    pub potentially_repairs_sets: usize,
    pub obtainable_or_expected: bool,
    pub explanation: String,
}

#[derive(Default)]
struct MissingAccumulator {
    occurrences: usize,
    sets: BTreeSet<String>,
    families: BTreeSet<String>,
    kind: Option<MameDependencyKind>,
    preservation_status: Option<MamePreservationStatus>,
    repairable: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameParentCloneCoverage {
    pub parent_sets: usize,
    pub clone_sets: usize,
    pub clones_with_parent_present: usize,
    pub clones_with_parent_missing: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameChdCoverage {
    pub sets_requiring_chd: usize,
    /// Every required disk has explicit combined-digest verification evidence.
    pub sets_with_chd: usize,
    pub sets_missing_chd: usize,
    #[serde(default)]
    pub sets_unverified_chd: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameUpdateReadiness {
    /// False when duplicate/unnamed declarations prevent a safe comparison.
    #[serde(default)]
    pub comparison_available: bool,
    pub unchanged_hashes_reusable: usize,
    pub missing_required_hashes: usize,
    pub obsolete_hashes: usize,
    pub renamed_or_moved_canonical_entries: usize,
    pub new_sets: usize,
    pub removed_sets: usize,
    pub shared_dependency_opportunities: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameSetResult {
    pub name: String,
    pub health: MameSetHealth,
    pub expected_members: usize,
    pub matched_members: usize,
    pub missing_members: Vec<String>,
    pub parent: Option<String>,
    #[serde(default)]
    pub evidence: MameStorageEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameCollectionAnalysis {
    pub schema_version: u32,
    pub emulator_version: Option<String>,
    #[serde(default)]
    pub catalogue_version: Option<String>,
    pub collection_root: PathBuf,
    pub discovered_sets: usize,
    pub passing_sets: usize,
    pub failing_sets: usize,
    pub unknown_sets: usize,
    pub style: MameCollectionStyle,
    pub version_compatibility: MameVersionCompatibility,
    pub set_results: Vec<MameSetResult>,
    pub top_missing_dependencies: Vec<MameMissingDependency>,
    pub parent_clone_coverage: MameParentCloneCoverage,
    pub bios_device_failures: Vec<String>,
    pub chd_coverage: MameChdCoverage,
    pub update_readiness: Option<MameUpdateReadiness>,
    pub warnings: Vec<String>,
}

pub fn analyse_collection(
    current: &ParsedDat,
    inventory: &MameCollectionInventory,
    emulator_version: Option<String>,
    previous_catalogue: Option<&ParsedDat>,
) -> MameCollectionAnalysis {
    analyse_collection_with_chds(
        current,
        inventory,
        emulator_version,
        previous_catalogue,
        &[],
    )
}

/// Like `analyse_collection`, with explicitly fully verified CHD combined-digest evidence.
/// Generic member hashes are never interpreted as CHD verification.
pub fn analyse_collection_with_chds(
    current: &ParsedDat,
    inventory: &MameCollectionInventory,
    emulator_version: Option<String>,
    previous_catalogue: Option<&ParsedDat>,
    chds: &[MameVerifiedChd],
) -> MameCollectionAnalysis {
    let observed: BTreeSet<_> = inventory.sets.iter().map(|s| s.name.as_str()).collect();
    let index = crate::arcade_mame_compatibility::MameExpectationIndex::new(current);
    let mut results = Vec::new();
    let mut missing = BTreeMap::<String, MissingAccumulator>::new();
    let mut bios_device_failures = BTreeSet::new();
    let mut coverage = MameParentCloneCoverage::default();
    let mut chd = MameChdCoverage::default();
    let observed_index = storage::index_inventory(inventory);
    let mut catalogue_index = BTreeMap::<&str, Vec<&DatGameEntry>>::new();
    for game in &current.games {
        catalogue_index.entry(&game.name).or_default().push(game);
    }
    for games in catalogue_index.values() {
        let game = games[0];
        let mut evidence = storage::evaluate(&index, game, inventory, chds, &observed_index);
        if games.len() > 1 {
            results.push(MameSetResult {
                name: game.name.clone(),
                health: MameSetHealth::Unknown,
                expected_members: 0,
                matched_members: 0,
                missing_members: vec![],
                parent: None,
                evidence,
            });
            continue;
        }
        if game.unsupported_structure {
            evidence
                .unresolved_dependencies
                .push("Unsupported catalogue structure".into());
        }
        // An inspected collection is not a request to acquire the entire DAT.
        // Absent catalogue machines remain unowned/unknown unless their full
        // requirements are proven via a supported shared/merged layout.
        if !observed.contains(game.name.as_str()) && !evidence.verified_complete() {
            let mut result = storage::result(game, evidence);
            result.health = MameSetHealth::Unknown;
            results.push(result);
            continue;
        }
        for requirement in &evidence.requirements {
            if requirement.content == MameMemberEvidence::Verified
                && requirement.preservation == MamePreservationStatus::VerifiedDumpExpected
            {
                continue;
            }
            // Unknown/weak evidence is not a missing obtainable item.
            if !matches!(
                requirement.content,
                MameMemberEvidence::Missing | MameMemberEvidence::Contradictory
            ) && requirement.preservation == MamePreservationStatus::VerifiedDumpExpected
            {
                continue;
            }
            if !requirement.disk {
                if let Some(providers) = catalogue_index.get(requirement.declaring_set.as_str())
                    && providers.len() == 1
                    && let Some(provider) = providers.first()
                {
                    record_missing(
                        &mut missing,
                        &game.name,
                        provider,
                        requirement,
                        game.clone_of.as_deref(),
                        evidence.unresolved_dependencies.is_empty(),
                    );
                }
            } else {
                let a = missing.entry(requirement.member.clone()).or_default();
                a.occurrences += 1;
                a.sets.insert(game.name.clone());
                a.families
                    .insert(game.clone_of.clone().unwrap_or_else(|| game.name.clone()));
                merge_classification(
                    a,
                    MameDependencyKind::Chd,
                    if evidence.unresolved_dependencies.is_empty() {
                        requirement.preservation
                    } else if requirement.preservation
                        == MamePreservationStatus::VerifiedDumpExpected
                    {
                        MamePreservationStatus::Unknown
                    } else {
                        requirement.preservation
                    },
                    evidence.unresolved_dependencies.is_empty()
                        && requirement.preservation == MamePreservationStatus::VerifiedDumpExpected,
                );
            }
        }
        let has_disks = evidence.requirements.iter().any(|r| r.disk);
        if has_disks {
            chd.sets_requiring_chd += 1;
            if evidence
                .requirements
                .iter()
                .filter(|r| r.disk)
                .all(|r| r.content == MameMemberEvidence::Verified)
            {
                chd.sets_with_chd += 1;
            } else if evidence.requirements.iter().any(|r| {
                r.disk
                    && r.content == MameMemberEvidence::Missing
                    && r.preservation != MamePreservationStatus::NoVerifiedDumpExists
            }) {
                chd.sets_missing_chd += 1;
            } else {
                chd.sets_unverified_chd += 1;
            }
        }
        let result = storage::result(game, evidence);
        if (flag(&game.is_bios) || flag(&game.is_device)) && result.health == MameSetHealth::Bad {
            bios_device_failures.insert(game.name.clone());
        }
        if let Some(parent) = &game.clone_of {
            coverage.clone_sets += 1;
            if observed.contains(parent.as_str()) {
                coverage.clones_with_parent_present += 1;
            } else {
                coverage.clones_with_parent_missing += 1;
            }
        } else {
            coverage.parent_sets += 1;
        }
        results.push(result);
    }
    results.sort_by(|a, b| a.name.cmp(&b.name));
    let known_names: BTreeSet<_> = current.games.iter().map(|g| g.name.as_str()).collect();
    let unrecognised_observed_sets = inventory
        .sets
        .iter()
        .filter(|set| !known_names.contains(set.name.as_str()))
        .count();
    let unknown_sets = results
        .iter()
        .filter(|result| result.health == MameSetHealth::Unknown)
        .count()
        + unrecognised_observed_sets;
    let passing_sets = results
        .iter()
        .filter(|r| r.health == MameSetHealth::Good)
        .count();
    let failing_sets = results
        .iter()
        .filter(|r| r.health == MameSetHealth::Bad)
        .count();
    let mut top_missing_dependencies: Vec<_> = missing
        .into_iter()
        .map(|(name, accumulator)| {
            let kind = accumulator.kind.unwrap_or(MameDependencyKind::Unknown);
            let preservation_status = accumulator
                .preservation_status
                .unwrap_or(MamePreservationStatus::Unknown);
            let potentially_repairs_sets = if accumulator.repairable {
                accumulator.sets.len()
            } else {
                0
            };
            MameMissingDependency {
                name,
                kind,
                preservation_status,
                affected_sets: accumulator.sets.iter().cloned().collect(),
                affected_families: accumulator.families.iter().cloned().collect(),
                occurrences: accumulator.occurrences,
                potentially_repairs_sets,
                obtainable_or_expected: accumulator.repairable,
                explanation: dependency_explanation(
                    &kind,
                    &preservation_status,
                    potentially_repairs_sets,
                ),
            }
        })
        .collect();
    top_missing_dependencies.sort_by(|a, b| {
        b.occurrences
            .cmp(&a.occurrences)
            .then_with(|| a.name.cmp(&b.name))
    });
    top_missing_dependencies.truncate(20);
    let version_compatibility = versions::compare(current, emulator_version.as_deref());
    let update_readiness = previous_catalogue.map(|previous| compare_catalogues(previous, current));
    let mut warnings = inventory.warnings.clone();
    if update_readiness
        .as_ref()
        .is_some_and(|r| !r.comparison_available)
    {
        warnings.push("Update comparison unavailable: catalogue machine/member declarations are ambiguous or unnamed".into());
    }
    MameCollectionAnalysis {
        schema_version: MAME_COLLECTION_ANALYSER_SCHEMA_VERSION,
        emulator_version,
        catalogue_version: current.source.version.clone(),
        collection_root: inventory.collection_root.clone(),
        discovered_sets: inventory.sets.len(),
        passing_sets,
        failing_sets,
        unknown_sets,
        style: infer_style(current, inventory),
        version_compatibility,
        set_results: results,
        top_missing_dependencies,
        parent_clone_coverage: coverage,
        bios_device_failures: bios_device_failures.into_iter().collect(),
        chd_coverage: chd,
        update_readiness,
        warnings,
    }
}

fn flag(value: &Option<String>) -> bool {
    value
        .as_deref()
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("yes"))
}

fn merge_classification(
    a: &mut MissingAccumulator,
    kind: MameDependencyKind,
    status: MamePreservationStatus,
    repairable: bool,
) {
    if a.kind.is_some_and(|k| k != kind) || a.preservation_status.is_some_and(|s| s != status) {
        a.kind = Some(MameDependencyKind::Unknown);
        a.preservation_status = Some(MamePreservationStatus::Unknown);
        a.repairable = false;
    } else {
        a.kind = Some(kind);
        a.preservation_status = Some(status);
        a.repairable = repairable;
    }
}

fn record_missing(
    missing: &mut BTreeMap<String, MissingAccumulator>,
    set_name: &str,
    game: &DatGameEntry,
    requirement: &MameRequirementEvidence,
    family: Option<&str>,
    metadata_resolved: bool,
) {
    let key = requirement.member.clone();
    let accumulator = missing.entry(key).or_default();
    accumulator.occurrences += 1;
    accumulator.sets.insert(set_name.to_string());
    accumulator
        .families
        .insert(family.unwrap_or(set_name).to_string());
    let (kind, status, repairable) = if !metadata_resolved
        && requirement.preservation == MamePreservationStatus::VerifiedDumpExpected
    {
        (
            MameDependencyKind::Unknown,
            MamePreservationStatus::Unknown,
            false,
        )
    } else {
        classify_requirement(game, requirement)
    };
    merge_classification(accumulator, kind, status, repairable);
}

fn classify_requirement(
    game: &DatGameEntry,
    requirement: &MameRequirementEvidence,
) -> (MameDependencyKind, MamePreservationStatus, bool) {
    // Preservation belongs to this resolved requirement, never the first
    // same-named raw declaration in the catalogue.
    match requirement.preservation {
        MamePreservationStatus::NoVerifiedDumpExists => {
            return (MameDependencyKind::NoDump, requirement.preservation, false);
        }
        MamePreservationStatus::NeedsRedump => {
            return (MameDependencyKind::BadDump, requirement.preservation, false);
        }
        MamePreservationStatus::Unknown => {
            return (MameDependencyKind::Unknown, requirement.preservation, false);
        }
        MamePreservationStatus::VerifiedDumpExpected => {}
    }
    let declarations: Vec<_> = game
        .roms
        .iter()
        .filter(|r| r.name == requirement.member)
        .collect();
    let merged = !declarations.is_empty() && declarations.iter().all(|r| r.merge.is_some());
    let lower = requirement.member.to_ascii_lowercase();
    let kind = if flag(&game.is_bios) {
        MameDependencyKind::BiosRom
    } else if flag(&game.is_device) {
        MameDependencyKind::DeviceRom
    } else if ["gal", "pal", "pld"]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        MameDependencyKind::PldGalPal
    } else if merged {
        MameDependencyKind::ParentSharedRom
    } else {
        MameDependencyKind::GameSpecificRom
    };
    (kind, MamePreservationStatus::VerifiedDumpExpected, true)
}

fn dependency_explanation(
    kind: &MameDependencyKind,
    status: &MamePreservationStatus,
    potentially_repairs_sets: usize,
) -> String {
    if *status == MamePreservationStatus::Unknown {
        return "Catalogue evidence is unresolved; no acquisition or repair opportunity is established.".into();
    }
    if *status == MamePreservationStatus::NoVerifiedDumpExists {
        return "MAME has no verified good dump for this item.".to_string();
    }
    match kind {
        MameDependencyKind::ParentSharedRom => format!(
            "This is a parent/shared dependency, not a game-specific file; it could affect {potentially_repairs_sets} sets."
        ),
        MameDependencyKind::BiosRom | MameDependencyKind::DeviceRom => format!(
            "This is a shared device or BIOS dependency used by {potentially_repairs_sets} machines."
        ),
        MameDependencyKind::PldGalPal => {
            "This is a PLD/GAL/PAL device dump and may require specialist preservation evidence."
                .to_string()
        }
        MameDependencyKind::Chd => {
            format!("This is a required CHD and could affect {potentially_repairs_sets} sets.")
        }
        MameDependencyKind::BadDump => {
            "MAME knows a dump exists, but marks the available dump as bad and needing redump."
                .to_string()
        }
        _ => "This is an expected ROM requirement for the affected machine set.".to_string(),
    }
}

fn infer_style(current: &ParsedDat, inventory: &MameCollectionInventory) -> MameCollectionStyle {
    if inventory.sets.is_empty() {
        return MameCollectionStyle::Unknown;
    }
    let has_dirs = inventory.sets.iter().any(|set| set.directory);
    let has_archives = inventory.sets.iter().any(|set| !set.directory);
    if has_dirs && has_archives {
        return MameCollectionStyle::Mixed;
    }
    let _ = current;
    // More than one supported layout may satisfy the same inventory. Do not
    // infer physical packing from catalogue merge markers alone.
    MameCollectionStyle::Unknown
}

fn unique_declarations(dat: &ParsedDat) -> bool {
    let mut names = BTreeSet::new();
    dat.games.iter().all(|game| {
        let mut members = BTreeSet::new();
        !game.name.trim().is_empty()
            && names.insert(game.name.trim())
            && game
                .roms
                .iter()
                .all(|rom| !rom.name.trim().is_empty() && members.insert(rom.name.trim()))
    })
}

pub fn compare_catalogues(previous: &ParsedDat, current: &ParsedDat) -> MameUpdateReadiness {
    if !unique_declarations(previous) || !unique_declarations(current) {
        return MameUpdateReadiness::default();
    }
    let old: BTreeMap<_, _> = previous
        .games
        .iter()
        .map(|g| (g.name.as_str(), g))
        .collect();
    let new: BTreeMap<_, _> = current.games.iter().map(|g| (g.name.as_str(), g)).collect();
    let mut readiness = MameUpdateReadiness {
        comparison_available: true,
        new_sets: new.keys().filter(|name| !old.contains_key(**name)).count(),
        removed_sets: old.keys().filter(|name| !new.contains_key(**name)).count(),
        ..Default::default()
    };
    for (name, game) in &new {
        let Some(previous_game) = old.get(name) else {
            continue;
        };
        for rom in &game.roms {
            if let Some(old_rom) = previous_game
                .roms
                .iter()
                .find(|candidate| candidate.name == rom.name)
            {
                if rom.sha1.is_some() && rom.sha1 == old_rom.sha1 {
                    readiness.unchanged_hashes_reusable += 1;
                } else if rom.sha1.is_none() {
                    readiness.missing_required_hashes += 1;
                } else if rom.sha1 != old_rom.sha1 {
                    readiness.obsolete_hashes += 1;
                }
            } else if previous_game
                .roms
                .iter()
                .any(|candidate| candidate.sha1 == rom.sha1 && rom.sha1.is_some())
            {
                readiness.renamed_or_moved_canonical_entries += 1;
            } else {
                readiness.missing_required_hashes += 1;
            }
        }
        readiness.shared_dependency_opportunities +=
            usize::from(game.clone_of.is_some() || !game.device_refs.is_empty());
    }
    readiness
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::{
        DatDiskEntry, DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatRomEntry,
        DatSource,
    };

    pub(super) fn catalogue(games: Vec<DatGameEntry>) -> ParsedDat {
        ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::MAMEArcade,
                file_path: "listxml".into(),
                name: None,
                description: None,
                version: None,
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: games.len(),
                rom_count: games.iter().map(DatGameEntry::rom_count).sum(),
                parse_warnings: vec![],
                packing_policy: DatPackingPolicy::Standard,
            },
            games,
        }
    }
    fn game(name: &str, crc: &str) -> DatGameEntry {
        DatGameEntry {
            name: name.into(),
            mame_listxml_schema_version: Some(
                crate::dat::parsers::mame_listxml::MAME_LISTXML_PARSER_SCHEMA_VERSION,
            ),
            roms: vec![DatRomEntry {
                name: "main.bin".into(),
                size_bytes: Some(1),
                crc32: Some(crc.repeat(4)),
                sha1: Some(crc.repeat(20)),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn reports_good_bad_unknown_and_repeated_missing_dependency_deterministically() {
        let dat = catalogue(vec![game("good", "aa"), game("bad", "bb")]);
        let inventory = MameCollectionInventory {
            collection_root: "/roms".into(),
            sets: vec![
                MameObservedSet {
                    name: "good".into(),
                    location: "/roms/good.zip".into(),
                    directory: false,
                    members: vec![MameObservedMember {
                        name: "main.bin".into(),
                        size_bytes: Some(1),
                        crc32: Some("aaaaaaaa".into()),
                        md5: None,
                        sha1: Some("a".repeat(40)),
                    }],
                },
                MameObservedSet {
                    name: "bad".into(),
                    location: "/roms/bad.zip".into(),
                    directory: false,
                    members: vec![],
                },
                MameObservedSet {
                    name: "mystery".into(),
                    location: "/roms/mystery.zip".into(),
                    directory: false,
                    members: vec![],
                },
            ],
            inspected_completely: true,
            warnings: vec![],
        };
        let report = analyse_collection(&dat, &inventory, Some("0.264".into()), None);
        assert_eq!(
            (
                report.passing_sets,
                report.failing_sets,
                report.unknown_sets
            ),
            (1, 1, 1)
        );
        assert_eq!(report.top_missing_dependencies[0].name, "main.bin");
        assert_eq!(
            report.version_compatibility,
            MameVersionCompatibility::ExactSourceVersionUnknown
        );
    }

    #[test]
    fn update_readiness_reuses_hashes_and_detects_renames() {
        let old = catalogue(vec![game("set", "aa")]);
        let mut changed = game("set", "bb");
        changed.roms[0].name = "renamed.bin".into();
        let current = catalogue(vec![changed]);
        let readiness = compare_catalogues(&old, &current);
        assert_eq!(readiness.renamed_or_moved_canonical_entries, 0);
        assert_eq!(readiness.missing_required_hashes, 1);
    }

    #[test]
    fn explains_shared_statuses_without_calling_no_dump_repairable() {
        let mut bios_a = game("bios-a", "aa");
        bios_a.roms[0].name = "shared-bios.bin".into();
        bios_a.is_bios = Some("yes".into());
        let mut bios_b = game("bios-b", "aa");
        bios_b.roms[0].name = "shared-bios.bin".into();
        bios_b.is_bios = Some("yes".into());
        let mut no_dump = game("prototype", "bb");
        no_dump.roms[0].name = "undumped-gal.bin".into();
        no_dump.roms[0].status = Some("nodump".into());
        let mut bad_dump = game("bad", "cc");
        bad_dump.roms[0].status = Some("baddump".into());
        let mut chd = game("disc", "dd");
        chd.disks = vec![DatDiskEntry {
            name: Some("disc.chd".into()),
            sha1: Some("0123456789012345678901234567890123456789".into()),
            ..Default::default()
        }];
        let dat = catalogue(vec![bios_a, bios_b, no_dump, bad_dump, chd]);
        let inventory = MameCollectionInventory {
            collection_root: "/roms".into(),
            sets: ["bios-a", "bios-b", "prototype", "bad", "disc"]
                .into_iter()
                .map(|name| MameObservedSet {
                    name: name.into(),
                    location: format!("/roms/{name}.zip").into(),
                    directory: false,
                    members: Vec::new(),
                })
                .collect(),
            inspected_completely: true,
            warnings: Vec::new(),
        };
        let report = analyse_collection(&dat, &inventory, Some("0.264".into()), None);
        let bios = report
            .top_missing_dependencies
            .iter()
            .find(|item| item.name == "shared-bios.bin")
            .expect("shared BIOS should be explained");
        assert_eq!(bios.kind, MameDependencyKind::BiosRom);
        assert_eq!(bios.affected_sets.len(), 2);
        assert_eq!(bios.potentially_repairs_sets, 2);
        let nodump = report
            .top_missing_dependencies
            .iter()
            .find(|item| item.name == "undumped-gal.bin")
            .expect("NO_DUMP should be explained");
        assert_eq!(nodump.kind, MameDependencyKind::NoDump);
        assert!(!nodump.obtainable_or_expected);
        assert_eq!(nodump.potentially_repairs_sets, 0);
        assert!(nodump.explanation.contains("no verified good dump"));
        assert!(
            report
                .top_missing_dependencies
                .iter()
                .any(|item| item.kind == MameDependencyKind::Chd)
        );
    }
}

#[cfg(test)]
mod regressions;

#[cfg(test)]
mod corrections;
