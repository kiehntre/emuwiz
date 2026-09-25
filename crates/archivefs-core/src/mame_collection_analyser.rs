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

use crate::dat::model::{DatGameEntry, DatRomEntry, ParsedDat};

pub const MAME_COLLECTION_ANALYSER_SCHEMA_VERSION: u32 = 1;

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
    MostlyCompatible,
    SignificantMismatch,
    ExactSourceVersionUnknown,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameSetHealth {
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
    pub sets_with_chd: usize,
    pub sets_missing_chd: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameUpdateReadiness {
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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameCollectionAnalysis {
    pub schema_version: u32,
    pub emulator_version: Option<String>,
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
    let observed: BTreeMap<_, _> = inventory
        .sets
        .iter()
        .map(|set| (set.name.as_str(), set))
        .collect();
    let mut results = Vec::new();
    let mut missing = BTreeMap::<String, MissingAccumulator>::new();
    let mut bios_device_failures = BTreeSet::new();
    let mut coverage = MameParentCloneCoverage::default();
    let mut chd = MameChdCoverage::default();

    for game in &current.games {
        let Some(set) = observed.get(game.name.as_str()) else {
            results.push(MameSetResult {
                name: game.name.clone(),
                health: MameSetHealth::Unknown,
                expected_members: game.roms.len(),
                matched_members: 0,
                missing_members: Vec::new(),
                parent: game.clone_of.clone(),
            });
            continue;
        };
        let members: BTreeMap<_, _> = set
            .members
            .iter()
            .map(|member| (member.name.as_str(), member))
            .collect();
        let mut missing_members = Vec::new();
        let mut matched = 0;
        for rom in &game.roms {
            if rom
                .status
                .as_deref()
                .is_some_and(|status| status.eq_ignore_ascii_case("nodump"))
            {
                record_missing(&mut missing, &game.name, game, rom, None);
                continue;
            }
            if let Some(member) = members.get(rom.name.as_str()) {
                if member_matches(rom, member) {
                    if rom
                        .status
                        .as_deref()
                        .is_some_and(|status| status.eq_ignore_ascii_case("baddump"))
                    {
                        record_missing(&mut missing, &game.name, game, rom, None);
                    }
                    matched += 1;
                    continue;
                }
            }
            missing_members.push(rom.name.clone());
            record_missing(&mut missing, &game.name, game, rom, None);
        }
        for disk in &game.disks {
            let name = disk.name.as_deref().unwrap_or("(unnamed CHD)");
            if !set.members.iter().any(|member| member.name == name) {
                let accumulator = missing.entry(name.to_string()).or_default();
                accumulator.occurrences += 1;
                accumulator.sets.insert(game.name.clone());
                accumulator
                    .families
                    .insert(game.clone_of.clone().unwrap_or_else(|| game.name.clone()));
                accumulator.kind = Some(MameDependencyKind::Chd);
                accumulator.preservation_status =
                    Some(MamePreservationStatus::VerifiedDumpExpected);
                accumulator.repairable = true;
            }
        }
        let health = if game.roms.is_empty() || missing_members.is_empty() {
            MameSetHealth::Good
        } else {
            MameSetHealth::Bad
        };
        if game.is_bios.is_some() && health == MameSetHealth::Bad {
            bios_device_failures.insert(game.name.clone());
        }
        if game.is_device.is_some() && health == MameSetHealth::Bad {
            bios_device_failures.insert(game.name.clone());
        }
        if game.clone_of.is_some() {
            coverage.clone_sets += 1;
            if observed.contains_key(game.clone_of.as_deref().unwrap_or_default()) {
                coverage.clones_with_parent_present += 1;
            } else {
                coverage.clones_with_parent_missing += 1;
            }
        } else {
            coverage.parent_sets += 1;
        }
        if !game.disks.is_empty() {
            chd.sets_requiring_chd += 1;
            let has_chd = set
                .members
                .iter()
                .any(|member| member.name.to_ascii_lowercase().ends_with(".chd"));
            if has_chd {
                chd.sets_with_chd += 1;
            } else {
                chd.sets_missing_chd += 1;
            }
        }
        results.push(MameSetResult {
            name: game.name.clone(),
            health,
            expected_members: game.roms.len(),
            matched_members: matched,
            missing_members,
            parent: game.clone_of.clone(),
        });
    }
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
    let version_compatibility = if failing_sets == 0 && unknown_sets == 0 {
        MameVersionCompatibility::CompatibleWithCurrentMame
    } else if passing_sets >= failing_sets && passing_sets > 0 {
        MameVersionCompatibility::MostlyCompatible
    } else {
        MameVersionCompatibility::SignificantMismatch
    };
    MameCollectionAnalysis {
        schema_version: MAME_COLLECTION_ANALYSER_SCHEMA_VERSION,
        emulator_version,
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
        update_readiness: previous_catalogue.map(|previous| compare_catalogues(previous, current)),
        warnings: inventory.warnings.clone(),
    }
}

fn member_matches(rom: &crate::dat::model::DatRomEntry, member: &MameObservedMember) -> bool {
    if rom.size_bytes.is_some() && rom.size_bytes != member.size_bytes {
        return false;
    }
    rom.sha1
        .as_deref()
        .is_some_and(|hash| member.sha1.as_deref() == Some(hash))
        || rom
            .md5
            .as_deref()
            .is_some_and(|hash| member.md5.as_deref() == Some(hash))
        || rom
            .crc32
            .as_deref()
            .is_some_and(|hash| member.crc32.as_deref() == Some(hash))
        || (rom.sha1.is_none() && rom.md5.is_none() && rom.crc32.is_none())
}

fn record_missing(
    missing: &mut BTreeMap<String, MissingAccumulator>,
    set_name: &str,
    game: &DatGameEntry,
    rom: &DatRomEntry,
    _reason: Option<&str>,
) {
    let key = rom.name.clone();
    let accumulator = missing.entry(key).or_default();
    accumulator.occurrences += 1;
    accumulator.sets.insert(set_name.to_string());
    accumulator.families.insert(
        game.clone_of
            .clone()
            .unwrap_or_else(|| set_name.to_string()),
    );
    let (kind, status, repairable) = classify_rom(game, rom);
    if accumulator.kind.is_some_and(|existing| existing != kind) {
        accumulator.kind = Some(MameDependencyKind::Unknown);
        accumulator.preservation_status = Some(MamePreservationStatus::Unknown);
        accumulator.repairable = false;
    } else {
        accumulator.kind = Some(kind);
        accumulator.preservation_status = Some(status);
        accumulator.repairable |= repairable;
    }
}

fn classify_rom(
    game: &DatGameEntry,
    rom: &DatRomEntry,
) -> (MameDependencyKind, MamePreservationStatus, bool) {
    let status = rom
        .status
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if status == "nodump" || status == "no_dump" {
        return (
            MameDependencyKind::NoDump,
            MamePreservationStatus::NoVerifiedDumpExists,
            false,
        );
    }
    if status == "baddump" || status == "bad_dump" {
        return (
            MameDependencyKind::BadDump,
            MamePreservationStatus::NeedsRedump,
            false,
        );
    }
    let lower = rom.name.to_ascii_lowercase();
    let kind = if game.is_bios.is_some() {
        MameDependencyKind::BiosRom
    } else if game.is_device.is_some() || !game.device_refs.is_empty() {
        MameDependencyKind::DeviceRom
    } else if ["gal", "pal", "pld"]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        MameDependencyKind::PldGalPal
    } else if rom.merge.is_some() || game.clone_of.is_some() {
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
    if has_dirs {
        return MameCollectionStyle::ExplodedSelfContained;
    }
    let merged = current
        .games
        .iter()
        .flat_map(|g| g.roms.iter())
        .any(|rom| rom.merge.is_some());
    let duplicated_parent_members = current
        .games
        .iter()
        .filter(|g| g.clone_of.is_some())
        .any(|g| g.roms.iter().any(|rom| rom.merge.is_none()));
    if merged {
        MameCollectionStyle::Merged
    } else if duplicated_parent_members {
        MameCollectionStyle::NonMerged
    } else {
        MameCollectionStyle::Split
    }
}

pub fn compare_catalogues(previous: &ParsedDat, current: &ParsedDat) -> MameUpdateReadiness {
    let old: BTreeMap<_, _> = previous
        .games
        .iter()
        .map(|g| (g.name.as_str(), g))
        .collect();
    let new: BTreeMap<_, _> = current.games.iter().map(|g| (g.name.as_str(), g)).collect();
    let mut readiness = MameUpdateReadiness {
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

    fn catalogue(games: Vec<DatGameEntry>) -> ParsedDat {
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
            roms: vec![DatRomEntry {
                name: "main.bin".into(),
                size_bytes: Some(1),
                crc32: Some(crc.into()),
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
                        crc32: Some("aa".into()),
                        md5: None,
                        sha1: None,
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
            MameVersionCompatibility::MostlyCompatible
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
