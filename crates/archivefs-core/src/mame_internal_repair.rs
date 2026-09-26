//! Read-only planning for repairing failing MAME sets from exact local bytes.
//!
//! This module is intentionally a planner, not a copier.  It consumes a
//! pinned MAME catalogue and an already-built inventory.  SHA-1 is the
//! identity relation; filenames are only placement labels.  In particular,
//! a differently named local member is valid evidence when its SHA-1 is the
//! exact catalogue SHA-1.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::dat::model::{DatGameEntry, DatRomEntry, ParsedDat};
use crate::mame_collection_analyser::{
    MameCollectionAnalysis, MameCollectionInventory, MameObservedMember, MameObservedSet,
};

pub const MAME_INTERNAL_REPAIR_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameRepairRequirementClass {
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

impl MameRepairRequirementClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::GameSpecificRom => "game-specific ROM",
            Self::ParentSharedRom => "parent/shared ROM",
            Self::BiosRom => "BIOS ROM",
            Self::DeviceRom => "device ROM",
            Self::PldGalPal => "PLD/GAL/PAL",
            Self::Chd => "CHD",
            Self::NoDump => "NO_DUMP",
            Self::BadDump => "BAD_DUMP",
            Self::Unknown => "unknown requirement",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameRepairRelationshipKind {
    GameSpecific,
    ParentShared,
    Bios,
    Device,
    PldGalPal,
    Chd,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameRepairRelationship {
    pub kind: MameRepairRelationshipKind,
    pub set: String,
    pub parent: Option<String>,
    pub merge_member: Option<String>,
    pub device_refs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameRepairSource {
    pub container: PathBuf,
    pub member: String,
    #[serde(default)]
    pub container_is_directory: bool,
    pub sha1: String,
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameRepairDisposition {
    SafeInternalRepair,
    PresentButBadDump,
    NoDump,
    GenuinelyAbsent,
    Ambiguous,
    WrongContentSameName,
}

impl MameRepairDisposition {
    pub fn label(self) -> &'static str {
        match self {
            Self::SafeInternalRepair => "safe internal repair",
            Self::PresentButBadDump => "present but BAD_DUMP",
            Self::NoDump => "NO_DUMP / preservation only",
            Self::GenuinelyAbsent => "genuinely absent",
            Self::Ambiguous => "ambiguous",
            Self::WrongContentSameName => "wrong content, same name",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameRepairConfidence {
    ExactSha1Deterministic,
    ExactSha1MultipleCopies,
    Refused,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameInternalRepairRequirement {
    pub affected_set: String,
    pub required_filename: String,
    pub required_size: Option<u64>,
    pub crc: Option<String>,
    pub sha1: Option<String>,
    pub requirement_class: MameRepairRequirementClass,
    pub relationship: MameRepairRelationship,
    pub expected_destination_container: Option<PathBuf>,
    #[serde(default)]
    pub expected_destination_is_directory: bool,
    pub expected_destination_member: String,
    pub exact_matching_sources: Vec<MameRepairSource>,
    pub source_sha1_verified: bool,
    pub ambiguity: Option<String>,
    pub preservation_status: String,
    pub proposed_operation: String,
    pub repair_confidence: MameRepairConfidence,
    pub disposition: MameRepairDisposition,
    pub refusal_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameRepairImpact {
    pub set_name: String,
    pub repairable_requirements: usize,
    pub source_identities: Vec<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameInternalRepairPlan {
    pub schema_version: u32,
    pub collection_root: PathBuf,
    pub catalogue_version: Option<String>,
    pub sets_currently_failing: usize,
    pub affected_sets: Vec<String>,
    pub requirements: Vec<MameInternalRepairRequirement>,
    pub safe_internal_repair_count: usize,
    pub no_download_needed_count: usize,
    pub genuinely_absent_count: usize,
    pub preservation_only_no_dump_count: usize,
    pub bad_dump_count: usize,
    pub ambiguous_count: usize,
    pub wrong_content_same_name_count: usize,
    pub unique_source_identities_needed: usize,
    pub filesystem_operations_required: usize,
    pub projected_sets_repairable: usize,
    pub top_repairs_by_impact: Vec<MameRepairImpact>,
    pub warnings: Vec<String>,
}

pub struct MameInternalRepairRequest<'a> {
    pub catalogue: &'a ParsedDat,
    pub inventory: &'a MameCollectionInventory,
    pub analysis: &'a MameCollectionAnalysis,
}

#[derive(Clone)]
struct Expected<'a> {
    set: &'a DatGameEntry,
    filename: String,
    size: Option<u64>,
    crc: Option<String>,
    sha1: Option<String>,
    class: MameRepairRequirementClass,
    relationship: MameRepairRelationship,
    destination_set: String,
}

#[derive(Clone)]
struct Candidate<'a> {
    set: &'a MameObservedSet,
    member: &'a MameObservedMember,
}

pub fn build_mame_internal_repair_plan(
    request: &MameInternalRepairRequest<'_>,
) -> Result<MameInternalRepairPlan, String> {
    if request.catalogue.source.ecosystem != crate::dat::model::DatEcosystem::MAMEArcade {
        return Err("MAME internal repair requires a MAME arcade catalogue".into());
    }

    let observed: BTreeMap<&str, &MameObservedSet> = request
        .inventory
        .sets
        .iter()
        .map(|set| (set.name.as_str(), set))
        .collect();
    let mut by_sha1: BTreeMap<String, Vec<Candidate<'_>>> = BTreeMap::new();
    for set in &request.inventory.sets {
        for member in &set.members {
            if let Some(sha1) = normalized_sha1(member.sha1.as_deref()) {
                by_sha1
                    .entry(sha1)
                    .or_default()
                    .push(Candidate { set, member });
            }
        }
    }
    for candidates in by_sha1.values_mut() {
        candidates.sort_by(|left, right| {
            left.set
                .location
                .cmp(&right.set.location)
                .then_with(|| left.set.name.cmp(&right.set.name))
                .then_with(|| left.member.name.cmp(&right.member.name))
        });
    }

    let mut requirements = Vec::new();
    let mut affected_sets = BTreeSet::new();
    for game in &request.catalogue.games {
        let Some(game_set) = observed.get(game.name.as_str()).copied() else {
            continue;
        };
        let mut expected = Vec::new();
        for rom in &game.roms {
            expected.push(expected_rom(game, rom));
        }
        for disk in &game.disks {
            let Some(filename) = disk.name.clone() else {
                continue;
            };
            expected.push(Expected {
                set: game,
                filename: filename.clone(),
                size: None,
                crc: None,
                sha1: normalized_sha1(disk.sha1.as_deref()),
                class: MameRepairRequirementClass::Chd,
                relationship: MameRepairRelationship {
                    kind: MameRepairRelationshipKind::Chd,
                    set: game.name.clone(),
                    parent: game.rom_of.clone().or_else(|| game.clone_of.clone()),
                    merge_member: disk.merge.clone(),
                    device_refs: game
                        .device_refs
                        .iter()
                        .filter_map(|item| item.name.clone())
                        .collect(),
                },
                destination_set: game.name.clone(),
            });
        }

        for item in expected {
            let destination = observed.get(item.destination_set.as_str()).copied();
            if destination
                .is_some_and(|set| destination_matches(set, &item.filename, item.sha1.as_deref()))
                && !is_bad_dump(&item)
                && !is_no_dump(&item)
            {
                continue;
            }
            let requirement = classify_requirement(&item, destination, &by_sha1);
            affected_sets.insert(item.set.name.clone());
            requirements.push(requirement);
        }
        let _ = game_set;
    }

    requirements.sort_by(|left, right| {
        left.affected_set
            .cmp(&right.affected_set)
            .then_with(|| left.required_filename.cmp(&right.required_filename))
            .then_with(|| left.sha1.cmp(&right.sha1))
    });

    let safe_internal_repair_count = requirements
        .iter()
        .filter(|item| item.disposition == MameRepairDisposition::SafeInternalRepair)
        .count();
    let genuinely_absent_count = requirements
        .iter()
        .filter(|item| item.disposition == MameRepairDisposition::GenuinelyAbsent)
        .count();
    let preservation_only_no_dump_count = requirements
        .iter()
        .filter(|item| item.disposition == MameRepairDisposition::NoDump)
        .count();
    let bad_dump_count = requirements
        .iter()
        .filter(|item| item.disposition == MameRepairDisposition::PresentButBadDump)
        .count();
    let ambiguous_count = requirements
        .iter()
        .filter(|item| item.disposition == MameRepairDisposition::Ambiguous)
        .count();
    let wrong_content_same_name_count = requirements
        .iter()
        .filter(|item| item.disposition == MameRepairDisposition::WrongContentSameName)
        .count();
    let unique_source_identities_needed: BTreeSet<_> = requirements
        .iter()
        .filter(|item| item.disposition == MameRepairDisposition::SafeInternalRepair)
        .filter_map(|item| item.sha1.clone())
        .collect();

    let mut by_set: BTreeMap<String, Vec<&MameInternalRepairRequirement>> = BTreeMap::new();
    for item in &requirements {
        by_set
            .entry(item.affected_set.clone())
            .or_default()
            .push(item);
    }
    let projected_sets_repairable = by_set
        .values()
        .filter(|items| {
            !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.disposition == MameRepairDisposition::SafeInternalRepair)
        })
        .count();
    let mut top_repairs_by_impact = Vec::new();
    for (set_name, items) in by_set {
        let safe: Vec<_> = items
            .iter()
            .filter(|item| item.disposition == MameRepairDisposition::SafeInternalRepair)
            .filter_map(|item| item.sha1.clone())
            .collect();
        if !safe.is_empty() {
            top_repairs_by_impact.push(MameRepairImpact {
                set_name,
                repairable_requirements: safe.len(),
                source_identities: safe,
                reason:
                    "exact SHA-1 bytes are already present locally; placement remains preview-only"
                        .into(),
            });
        }
    }
    top_repairs_by_impact.sort_by(|left, right| {
        right
            .repairable_requirements
            .cmp(&left.repairable_requirements)
            .then_with(|| left.set_name.cmp(&right.set_name))
    });
    top_repairs_by_impact.truncate(20);

    let mut warnings = request.inventory.warnings.clone();
    if !request.inventory.inspected_completely {
        warnings
            .push("The inventory was not marked complete; absent results are provisional.".into());
    }
    if request.inventory.sets.is_empty() {
        warnings.push(
            "No observed MAME sets were supplied; no repair destination can be proven.".into(),
        );
    }

    Ok(MameInternalRepairPlan {
        schema_version: MAME_INTERNAL_REPAIR_SCHEMA_VERSION,
        collection_root: request.inventory.collection_root.clone(),
        catalogue_version: request.catalogue.source.version.clone(),
        sets_currently_failing: request.analysis.failing_sets,
        affected_sets: affected_sets.into_iter().collect(),
        requirements,
        safe_internal_repair_count,
        no_download_needed_count: safe_internal_repair_count,
        genuinely_absent_count,
        preservation_only_no_dump_count,
        bad_dump_count,
        ambiguous_count,
        wrong_content_same_name_count,
        unique_source_identities_needed: unique_source_identities_needed.len(),
        filesystem_operations_required: safe_internal_repair_count,
        projected_sets_repairable,
        top_repairs_by_impact,
        warnings,
    })
}

fn expected_rom<'a>(game: &'a DatGameEntry, rom: &DatRomEntry) -> Expected<'a> {
    let (class, relationship_kind) = requirement_class(game, rom);
    let parent = game.rom_of.clone().or_else(|| game.clone_of.clone());
    let merged_destination = rom.merge.clone().zip(parent.clone());
    let (destination_set, filename) = merged_destination
        .map(|(member, parent)| (parent, member))
        .unwrap_or_else(|| (game.name.clone(), rom.name.clone()));
    Expected {
        set: game,
        filename,
        size: rom.size_bytes,
        crc: rom.crc32.clone(),
        sha1: normalized_sha1(rom.sha1.as_deref()),
        class,
        relationship: MameRepairRelationship {
            kind: relationship_kind,
            set: game.name.clone(),
            parent,
            merge_member: rom.merge.clone(),
            device_refs: game
                .device_refs
                .iter()
                .filter_map(|item| item.name.clone())
                .collect(),
        },
        destination_set,
    }
}

fn requirement_class(
    game: &DatGameEntry,
    rom: &DatRomEntry,
) -> (MameRepairRequirementClass, MameRepairRelationshipKind) {
    let status = rom.status.as_deref().unwrap_or_default();
    if status.eq_ignore_ascii_case("nodump") || status.eq_ignore_ascii_case("no_dump") {
        return (
            MameRepairRequirementClass::NoDump,
            MameRepairRelationshipKind::Unknown,
        );
    }
    if status.eq_ignore_ascii_case("baddump") || status.eq_ignore_ascii_case("bad_dump") {
        return (
            MameRepairRequirementClass::BadDump,
            MameRepairRelationshipKind::Unknown,
        );
    }
    let name = rom.name.to_ascii_lowercase();
    if game.is_bios.is_some() {
        return (
            MameRepairRequirementClass::BiosRom,
            MameRepairRelationshipKind::Bios,
        );
    }
    if game.is_device.is_some() || !game.device_refs.is_empty() {
        return (
            MameRepairRequirementClass::DeviceRom,
            MameRepairRelationshipKind::Device,
        );
    }
    if ["gal", "pal", "pld"]
        .iter()
        .any(|marker| name.contains(marker))
    {
        return (
            MameRepairRequirementClass::PldGalPal,
            MameRepairRelationshipKind::PldGalPal,
        );
    }
    if rom.merge.is_some() || game.clone_of.is_some() || game.rom_of.is_some() {
        return (
            MameRepairRequirementClass::ParentSharedRom,
            MameRepairRelationshipKind::ParentShared,
        );
    }
    (
        MameRepairRequirementClass::GameSpecificRom,
        MameRepairRelationshipKind::GameSpecific,
    )
}

fn classify_requirement(
    expected: &Expected<'_>,
    destination: Option<&MameObservedSet>,
    by_sha1: &BTreeMap<String, Vec<Candidate<'_>>>,
) -> MameInternalRepairRequirement {
    let expected_sha1 = expected.sha1.clone();
    let candidates = expected_sha1
        .as_deref()
        .and_then(|sha1| by_sha1.get(sha1))
        .cloned()
        .unwrap_or_default();
    let exact_matching_sources: Vec<_> = candidates
        .iter()
        .map(|candidate| MameRepairSource {
            container: candidate.set.location.clone(),
            member: candidate.member.name.clone(),
            container_is_directory: candidate.set.directory,
            sha1: normalized_sha1(candidate.member.sha1.as_deref()).unwrap_or_default(),
            size_bytes: candidate.member.size_bytes,
        })
        .collect();
    let destination_has_same_name = destination.and_then(|set| {
        set.members
            .iter()
            .find(|member| member.name == expected.filename)
    });
    let destination_wrong = destination_has_same_name.is_some_and(|member| {
        expected_sha1.as_deref() != normalized_sha1(member.sha1.as_deref()).as_deref()
    });
    let is_no_dump = expected.class == MameRepairRequirementClass::NoDump;
    let is_bad_dump = expected.class == MameRepairRequirementClass::BadDump;
    let (disposition, confidence, refusal_reason, ambiguity, operation, preservation_status) =
        if is_no_dump {
            (
                MameRepairDisposition::NoDump,
                MameRepairConfidence::Refused,
                Some("MAME declares NO_DUMP; there is no verified ordinary repair target.".into()),
                None,
                "No operation: retain as a preservation gap".into(),
                "No verified dump exists".into(),
            )
        } else if is_bad_dump {
            (
                MameRepairDisposition::PresentButBadDump,
                MameRepairConfidence::Refused,
                Some(
                    "MAME marks the expected dump BAD_DUMP; exact bytes do not make it healthy."
                        .into(),
                ),
                None,
                "No operation: retain and flag for redump".into(),
                "Dump exists but needs redump".into(),
            )
        } else if expected_sha1.is_none() {
            (
                MameRepairDisposition::Ambiguous,
                MameRepairConfidence::Refused,
                Some("The catalogue does not provide an exact SHA-1 identity.".into()),
                Some("Filename/size/CRC evidence is insufficient for safe internal repair.".into()),
                "No operation: exact content identity is missing".into(),
                "Expected verified dump, but identity is incomplete".into(),
            )
        } else if candidates.is_empty() {
            (
                if destination_wrong {
                    MameRepairDisposition::WrongContentSameName
                } else {
                    MameRepairDisposition::GenuinelyAbsent
                },
                MameRepairConfidence::Refused,
                Some(if destination_wrong {
                    "The required filename exists, but its SHA-1 is not the expected content and no exact source exists locally."
                } else {
                    "The exact expected SHA-1 was not found anywhere in the supplied library inventory."
                }.into()),
                None,
                "No operation: download or external acquisition would be required".into(),
                "Verified dump expected".into(),
            )
        } else if destination.is_none() {
            (
                MameRepairDisposition::Ambiguous,
                MameRepairConfidence::Refused,
                Some("The exact bytes exist, but the expected destination container was not observed.".into()),
                Some("A source is known but destination ownership cannot be proven from the inventory.".into()),
                "No operation: destination container is not proven".into(),
                "Verified dump expected".into(),
            )
        } else {
            (
                MameRepairDisposition::SafeInternalRepair,
                if candidates.len() == 1 {
                    MameRepairConfidence::ExactSha1Deterministic
                } else {
                    MameRepairConfidence::ExactSha1MultipleCopies
                },
                None,
                (candidates.len() > 1).then(|| "Multiple identical SHA-1 source copies exist; the first deterministic source is only a preview choice.".into()),
                format!("Preview copy exact SHA-1 bytes into {}::{}, without mutating sources", expected.destination_set, expected.filename),
                "Verified dump expected".into(),
            )
        };
    MameInternalRepairRequirement {
        affected_set: expected.set.name.clone(),
        required_filename: expected.filename.clone(),
        required_size: expected.size,
        crc: expected.crc.clone(),
        sha1: expected_sha1,
        requirement_class: expected.class,
        relationship: expected.relationship.clone(),
        expected_destination_container: destination.map(|set| set.location.clone()),
        expected_destination_is_directory: destination.is_some_and(|set| set.directory),
        expected_destination_member: expected.filename.clone(),
        exact_matching_sources,
        source_sha1_verified: !candidates.is_empty(),
        ambiguity,
        preservation_status,
        proposed_operation: operation,
        repair_confidence: confidence,
        disposition,
        refusal_reason,
    }
}

fn destination_matches(set: &MameObservedSet, filename: &str, expected_sha1: Option<&str>) -> bool {
    let Some(member) = set.members.iter().find(|member| member.name == filename) else {
        return false;
    };
    expected_sha1.is_some_and(|expected| {
        normalized_sha1(member.sha1.as_deref()).as_deref() == Some(expected)
    })
}

fn normalized_sha1(value: Option<&str>) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    (value.len() == 40 && value.chars().all(|character| character.is_ascii_hexdigit()))
        .then_some(value)
}

fn is_no_dump(expected: &Expected<'_>) -> bool {
    expected.class == MameRepairRequirementClass::NoDump
}

fn is_bad_dump(expected: &Expected<'_>) -> bool {
    expected.class == MameRepairRequirementClass::BadDump
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::{DatEcosystem, DatFormat, DatGameEntry, DatRomEntry, DatSource};

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SHA_C: &str = "cccccccccccccccccccccccccccccccccccccccc";

    fn catalogue(games: Vec<DatGameEntry>) -> ParsedDat {
        ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::MAMEArcade,
                file_path: "synthetic-listxml".into(),
                version: Some("0.264".into()),
                name: None,
                description: None,
                author: None,
                homepage: None,
                clrmamepro_header: None,
                parse_warnings: Vec::new(),
                packing_policy: Default::default(),
                entry_count: games.len(),
                rom_count: games.iter().map(DatGameEntry::rom_count).sum(),
            },
            games,
        }
    }

    fn game(name: &str, rom: DatRomEntry) -> DatGameEntry {
        DatGameEntry {
            name: name.into(),
            roms: vec![rom],
            ..Default::default()
        }
    }

    fn rom(name: &str, sha1: &str) -> DatRomEntry {
        DatRomEntry {
            name: name.into(),
            size_bytes: Some(4),
            crc32: Some("1234abcd".into()),
            sha1: Some(sha1.into()),
            ..Default::default()
        }
    }

    fn request<'a>(
        catalogue: &'a ParsedDat,
        inventory: &'a MameCollectionInventory,
        analysis: &'a MameCollectionAnalysis,
    ) -> MameInternalRepairRequest<'a> {
        MameInternalRepairRequest {
            catalogue,
            inventory,
            analysis,
        }
    }

    fn analysis_for(
        catalogue: &ParsedDat,
        inventory: &MameCollectionInventory,
    ) -> MameCollectionAnalysis {
        crate::mame_collection_analyser::analyse_collection(
            catalogue,
            inventory,
            Some("0.264".into()),
            None,
        )
    }

    fn set(name: &str, member_name: &str, sha1: &str) -> MameObservedSet {
        MameObservedSet {
            name: name.into(),
            location: format!("/roms/{name}.zip").into(),
            directory: false,
            members: vec![MameObservedMember {
                name: member_name.into(),
                size_bytes: Some(4),
                crc32: None,
                md5: None,
                sha1: Some(sha1.into()),
            }],
        }
    }

    #[test]
    fn same_hash_different_filename_is_safe_and_deterministic() {
        let dat = catalogue(vec![
            game("target", rom("315-5298.b9", SHA_A)),
            game("source", rom("pls153.bin", SHA_A)),
        ]);
        let inventory = MameCollectionInventory {
            collection_root: "/roms".into(),
            sets: vec![
                set("target", "other.bin", SHA_B),
                set("source", "pls153.bin", SHA_A),
            ],
            inspected_completely: true,
            ..Default::default()
        };
        let inventory_before = inventory.clone();
        let analysis = analysis_for(&dat, &inventory);
        let plan = build_mame_internal_repair_plan(&request(&dat, &inventory, &analysis)).unwrap();
        let item = plan
            .requirements
            .iter()
            .find(|item| item.affected_set == "target")
            .unwrap();
        assert_eq!(item.disposition, MameRepairDisposition::SafeInternalRepair);
        assert_eq!(item.exact_matching_sources[0].member, "pls153.bin");
        assert_eq!(plan.projected_sets_repairable, 1);
        assert_eq!(inventory, inventory_before);
    }

    #[test]
    fn same_filename_wrong_hash_is_not_identity() {
        let dat = catalogue(vec![game("target", rom("main.bin", SHA_A))]);
        let inventory = MameCollectionInventory {
            collection_root: "/roms".into(),
            sets: vec![set("target", "main.bin", SHA_B)],
            inspected_completely: true,
            ..Default::default()
        };
        let analysis = analysis_for(&dat, &inventory);
        let plan = build_mame_internal_repair_plan(&request(&dat, &inventory, &analysis)).unwrap();
        assert_eq!(
            plan.requirements[0].disposition,
            MameRepairDisposition::WrongContentSameName
        );
    }

    #[test]
    fn merged_clone_resolves_parent_destination() {
        let mut clone = game("clone", rom("clone.bin", SHA_A));
        clone.clone_of = Some("parent".into());
        clone.rom_of = Some("parent".into());
        clone.roms[0].merge = Some("parent.bin".into());
        let dat = catalogue(vec![clone, game("parent", rom("parent.bin", SHA_A))]);
        let inventory = MameCollectionInventory {
            collection_root: "/roms".into(),
            sets: vec![
                set("clone", "clone.bin", SHA_B),
                set("parent", "parent.bin", SHA_A),
            ],
            inspected_completely: true,
            ..Default::default()
        };
        let analysis = analysis_for(&dat, &inventory);
        let plan = build_mame_internal_repair_plan(&request(&dat, &inventory, &analysis)).unwrap();
        assert!(
            plan.requirements
                .iter()
                .all(|item| item.affected_set != "clone")
        );
    }

    #[test]
    fn bad_dump_and_no_dump_are_never_safe_repairs() {
        let mut bad = rom("bad.bin", SHA_A);
        bad.status = Some("baddump".into());
        let mut nodump = rom("unknown.bin", SHA_B);
        nodump.status = Some("nodump".into());
        let dat = catalogue(vec![game("bad", bad), game("nodump", nodump)]);
        let inventory = MameCollectionInventory {
            collection_root: "/roms".into(),
            sets: vec![
                set("bad", "bad.bin", SHA_A),
                set("nodump", "other.bin", SHA_B),
            ],
            inspected_completely: true,
            ..Default::default()
        };
        let analysis = analysis_for(&dat, &inventory);
        let plan = build_mame_internal_repair_plan(&request(&dat, &inventory, &analysis)).unwrap();
        assert_eq!(plan.bad_dump_count, 1);
        assert_eq!(plan.preservation_only_no_dump_count, 1);
        assert_eq!(plan.safe_internal_repair_count, 0);
    }

    #[test]
    fn multiple_copies_are_deterministic_but_visible() {
        let dat = catalogue(vec![game("target", rom("main.bin", SHA_A))]);
        let inventory = MameCollectionInventory {
            collection_root: "/roms".into(),
            sets: vec![
                set("z-source", "z.bin", SHA_A),
                set("a-source", "a.bin", SHA_A),
                set("target", "wrong.bin", SHA_B),
            ],
            inspected_completely: true,
            ..Default::default()
        };
        let analysis = analysis_for(&dat, &inventory);
        let plan = build_mame_internal_repair_plan(&request(&dat, &inventory, &analysis)).unwrap();
        let item = &plan.requirements[0];
        assert_eq!(
            item.repair_confidence,
            MameRepairConfidence::ExactSha1MultipleCopies
        );
        assert_eq!(
            item.exact_matching_sources[0].container,
            PathBuf::from("/roms/a-source.zip")
        );
    }

    #[test]
    fn bios_device_and_pld_relationships_are_preserved() {
        let mut bios = game("bios", rom("bios.bin", SHA_A));
        bios.is_bios = Some("yes".into());
        let mut device = game("device", rom("device.bin", SHA_A));
        device.is_device = Some("yes".into());
        let pld = game("board", rom("security.pld", SHA_A));
        let dat = catalogue(vec![
            bios,
            device,
            pld,
            game("donor", rom("donor.bin", SHA_A)),
        ]);
        let inventory = MameCollectionInventory {
            collection_root: "/roms".into(),
            sets: vec![
                set("bios", "wrong.bin", SHA_B),
                set("device", "wrong.bin", SHA_B),
                set("board", "wrong.bin", SHA_B),
                set("donor", "donor.bin", SHA_A),
            ],
            inspected_completely: true,
            ..Default::default()
        };
        let analysis = analysis_for(&dat, &inventory);
        let plan = build_mame_internal_repair_plan(&request(&dat, &inventory, &analysis)).unwrap();
        assert_eq!(
            plan.requirements
                .iter()
                .find(|item| item.affected_set == "bios")
                .unwrap()
                .requirement_class,
            MameRepairRequirementClass::BiosRom
        );
        assert_eq!(
            plan.requirements
                .iter()
                .find(|item| item.affected_set == "device")
                .unwrap()
                .requirement_class,
            MameRepairRequirementClass::DeviceRom
        );
        assert_eq!(
            plan.requirements
                .iter()
                .find(|item| item.affected_set == "board")
                .unwrap()
                .requirement_class,
            MameRepairRequirementClass::PldGalPal
        );
    }

    #[test]
    fn absent_identity_and_unproven_parent_destination_are_distinct() {
        let dat = catalogue(vec![
            game("absent", rom("missing.bin", SHA_C)),
            {
                let mut clone = game("clone", rom("shared.bin", SHA_A));
                clone.clone_of = Some("missing-parent".into());
                clone.rom_of = Some("missing-parent".into());
                clone.roms[0].merge = Some("shared.bin".into());
                clone
            },
            game("donor", rom("donor.bin", SHA_A)),
        ]);
        let inventory = MameCollectionInventory {
            collection_root: "/roms".into(),
            sets: vec![
                set("absent", "other.bin", SHA_B),
                set("clone", "other.bin", SHA_B),
                set("donor", "donor.bin", SHA_A),
            ],
            inspected_completely: true,
            ..Default::default()
        };
        let analysis = analysis_for(&dat, &inventory);
        let plan = build_mame_internal_repair_plan(&request(&dat, &inventory, &analysis)).unwrap();
        assert_eq!(
            plan.requirements
                .iter()
                .find(|item| item.affected_set == "absent")
                .unwrap()
                .disposition,
            MameRepairDisposition::GenuinelyAbsent
        );
        let clone = plan
            .requirements
            .iter()
            .find(|item| item.affected_set == "clone")
            .unwrap();
        assert_eq!(clone.disposition, MameRepairDisposition::Ambiguous);
        assert!(
            clone
                .refusal_reason
                .as_deref()
                .unwrap()
                .contains("destination container")
        );
    }
}
