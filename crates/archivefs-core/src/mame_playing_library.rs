//! Read-only curation of a practical MAME playing library.
//!
//! This is deliberately a planner, not a ROM manager.  It consumes the
//! already parsed MAME catalogue, collection inventory, and health analysis;
//! it never scans, hashes, copies, links, renames, or deletes anything.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::dat::model::{DatEcosystem, ParsedDat};
use crate::mame_collection_analyser::{
    MameCollectionAnalysis, MameCollectionInventory, MameSetHealth,
};
use crate::playing_library::evidence::dat_release_evidence;

pub const MAME_PLAYING_LIBRARY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MamePlayingLibraryCategory {
    Bootleg,
    Prototype,
    GamblingFruit,
    Mechanical,
    NonVideo,
    NonWorking,
    Imperfect,
}

impl MamePlayingLibraryCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bootleg => "bootleg",
            Self::Prototype => "prototype",
            Self::GamblingFruit => "gambling/fruit machine",
            Self::Mechanical => "mechanical",
            Self::NonVideo => "non-video",
            Self::NonWorking => "non-working",
            Self::Imperfect => "imperfect/unknown working status",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MamePlayingLibraryPreferences {
    pub preferred_regions: Vec<String>,
    pub prefer_parent: bool,
    pub prefer_newest_revision: bool,
    pub prefer_english: bool,
    pub include_bootlegs: bool,
    pub include_prototypes: bool,
    pub include_gambling_fruit: bool,
    pub include_mechanical: bool,
    pub include_non_video: bool,
    pub include_non_working: bool,
    pub include_imperfect: bool,
    pub keep_distinct_multiplayer: bool,
    pub keep_notable_regional_differences: bool,
}

impl Default for MamePlayingLibraryPreferences {
    fn default() -> Self {
        Self {
            preferred_regions: vec![
                "World".into(),
                "USA".into(),
                "Europe".into(),
                "Japan".into(),
            ],
            prefer_parent: true,
            prefer_newest_revision: true,
            prefer_english: true,
            include_bootlegs: false,
            include_prototypes: false,
            include_gambling_fruit: false,
            include_mechanical: false,
            include_non_video: false,
            include_non_working: false,
            include_imperfect: false,
            keep_distinct_multiplayer: true,
            keep_notable_regional_differences: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MamePlayingLibraryRequest<'a> {
    pub catalogue: &'a ParsedDat,
    pub inventory: &'a MameCollectionInventory,
    pub analysis: &'a MameCollectionAnalysis,
    pub preferences: MamePlayingLibraryPreferences,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameSelectedSet {
    pub name: String,
    pub parent: Option<String>,
    pub clones: Vec<String>,
    pub regional_alternatives: Vec<String>,
    pub required_dependencies: Vec<String>,
    pub complete: bool,
    pub meaningfully_distinct_clone: bool,
    pub estimated_storage_bytes: Option<u64>,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameExcludedSet {
    pub name: String,
    pub family_root: String,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameRequiredSupportSet {
    pub name: String,
    pub kind: String,
    pub available: bool,
    pub estimated_storage_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameUnresolvedCase {
    pub family_root: String,
    pub candidates: Vec<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MamePlayingLibraryPlan {
    pub schema_version: u32,
    pub collection_root: PathBuf,
    pub archival_set_count: usize,
    pub archival_storage_bytes: Option<u64>,
    pub selected_sets: Vec<MameSelectedSet>,
    pub excluded_sets: Vec<MameExcludedSet>,
    pub required_support_sets: Vec<MameRequiredSupportSet>,
    pub projected_set_count: usize,
    pub projected_storage_bytes: Option<u64>,
    pub projected_savings_bytes: Option<u64>,
    pub unresolved_cases: Vec<MameUnresolvedCase>,
    /// Storage is a conservative logical member-byte estimate when all
    /// member sizes are known; compressed archive overhead is not invented.
    pub storage_estimate_is_logical_member_sum: bool,
    pub preferences: MamePlayingLibraryPreferences,
}

pub fn build_mame_playing_library_plan(
    request: &MamePlayingLibraryRequest<'_>,
) -> Result<MamePlayingLibraryPlan, String> {
    if request.catalogue.source.ecosystem != DatEcosystem::MAMEArcade {
        return Err("MAME playing library requires a MAME arcade catalogue".into());
    }
    let names: BTreeMap<&str, usize> = request
        .catalogue
        .games
        .iter()
        .enumerate()
        .map(|(index, game)| (game.name.as_str(), index))
        .collect();
    let observed: BTreeMap<&str, _> = request
        .inventory
        .sets
        .iter()
        .map(|set| (set.name.as_str(), set))
        .collect();
    let health: BTreeMap<&str, MameSetHealth> = request
        .analysis
        .set_results
        .iter()
        .map(|result| (result.name.as_str(), result.health))
        .collect();

    let mut families: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, game) in request.catalogue.games.iter().enumerate() {
        if game.is_bios.is_some()
            || game.is_device.is_some()
            || !observed.contains_key(game.name.as_str())
        {
            continue;
        }
        let root = family_root(&request.catalogue.games, index, &names);
        families.entry(root).or_default().push(index);
    }

    let mut plan = MamePlayingLibraryPlan {
        schema_version: MAME_PLAYING_LIBRARY_SCHEMA_VERSION,
        collection_root: request.inventory.collection_root.clone(),
        archival_set_count: request.inventory.sets.len(),
        archival_storage_bytes: total_storage(request.inventory),
        selected_sets: Vec::new(),
        excluded_sets: Vec::new(),
        required_support_sets: Vec::new(),
        projected_set_count: 0,
        projected_storage_bytes: Some(0),
        projected_savings_bytes: None,
        unresolved_cases: Vec::new(),
        storage_estimate_is_logical_member_sum: true,
        preferences: request.preferences.clone(),
    };

    let mut selected_indices = BTreeSet::new();
    for (root, mut members) in families {
        members.sort_by_key(|index| request.catalogue.games[*index].name.clone());
        let mut eligible = Vec::new();
        for index in members.iter().copied() {
            let game = &request.catalogue.games[index];
            let categories = categories(game);
            let mut reasons = Vec::new();
            if health.get(game.name.as_str()) != Some(&MameSetHealth::Good) {
                reasons.push("collection evidence is incomplete or unknown".into());
            }
            for category in &categories {
                if !category_allowed(*category, &request.preferences) {
                    reasons.push(format!("excluded category: {}", category.label()));
                }
            }
            if reasons.is_empty() {
                eligible.push(index);
            } else {
                plan.excluded_sets.push(MameExcludedSet {
                    name: game.name.clone(),
                    family_root: root.clone(),
                    reasons,
                });
            }
        }
        if eligible.is_empty() {
            if !members.is_empty() {
                plan.unresolved_cases.push(MameUnresolvedCase {
                    family_root: root,
                    candidates: members
                        .iter()
                        .map(|i| request.catalogue.games[*i].name.clone())
                        .collect(),
                    reason: "no complete, preference-eligible representative is available".into(),
                });
            }
            continue;
        }
        eligible.sort_by_key(|index| {
            election_key(
                &request.catalogue.games[*index],
                *index,
                &request.preferences,
            )
        });
        let winner = eligible[0];
        let winner_game = &request.catalogue.games[winner];
        selected_indices.insert(winner);
        let mut retained = vec![winner];
        for index in eligible.iter().copied().skip(1) {
            if meaningful_distinct(
                winner_game,
                &request.catalogue.games[index],
                &request.preferences,
            ) {
                retained.push(index);
                selected_indices.insert(index);
            } else {
                plan.excluded_sets.push(MameExcludedSet {
                    name: request.catalogue.games[index].name.clone(),
                    family_root: root.clone(),
                    reasons: vec![format!(
                        "redundant clone of selected representative {}",
                        winner_game.name
                    )],
                });
            }
        }
        let alternatives: Vec<String> = eligible
            .iter()
            .copied()
            .filter(|index| {
                *index != winner
                    && same_region(
                        &request.catalogue.games[winner],
                        &request.catalogue.games[*index],
                    )
            })
            .map(|index| request.catalogue.games[index].name.clone())
            .collect();
        for index in retained {
            let game = &request.catalogue.games[index];
            let dependencies = declared_dependencies(game);
            plan.selected_sets.push(MameSelectedSet {
                name: game.name.clone(),
                parent: game.clone_of.clone(),
                clones: eligible.iter().filter(|other| request.catalogue.games[**other].clone_of.as_deref() == Some(game.name.as_str())).map(|other| request.catalogue.games[*other].name.clone()).collect(),
                regional_alternatives: alternatives.clone(),
                required_dependencies: dependencies,
                complete: health.get(game.name.as_str()) == Some(&MameSetHealth::Good),
                meaningfully_distinct_clone: index != winner,
                estimated_storage_bytes: observed.get(game.name.as_str()).and_then(|set| set_storage(set)),
                reason: if index == winner { "preferred family representative".into() } else { "retained because metadata shows a meaningful regional/control-panel distinction".into() },
            });
        }
    }

    let mut support_names = BTreeSet::new();
    for index in selected_indices {
        let game = &request.catalogue.games[index];
        if let Some(parent) = game.clone_of.as_deref() {
            support_names.insert((parent.to_string(), "parent".to_string()));
        }
        for dependency in declared_dependencies(game) {
            support_names.insert((dependency, "BIOS/device dependency".into()));
        }
    }
    for (name, kind) in support_names {
        if plan.selected_sets.iter().any(|set| set.name == name) {
            continue;
        }
        let storage = observed.get(name.as_str()).and_then(|set| set_storage(set));
        let available = storage.is_some() || observed.contains_key(name.as_str());
        plan.required_support_sets.push(MameRequiredSupportSet {
            name,
            kind,
            available,
            estimated_storage_bytes: storage,
        });
    }
    plan.selected_sets.sort_by(|a, b| a.name.cmp(&b.name));
    plan.required_support_sets
        .sort_by(|a, b| a.name.cmp(&b.name));
    plan.projected_set_count = plan.selected_sets.len() + plan.required_support_sets.len();
    plan.projected_storage_bytes = plan
        .selected_sets
        .iter()
        .map(|set| set.estimated_storage_bytes)
        .sum::<Option<u64>>();
    let support_storage = plan
        .required_support_sets
        .iter()
        .map(|set| set.estimated_storage_bytes)
        .sum::<Option<u64>>();
    plan.projected_storage_bytes = match (plan.projected_storage_bytes, support_storage) {
        (Some(selected), Some(support)) => Some(selected + support),
        _ => None,
    };
    plan.projected_savings_bytes = match (plan.archival_storage_bytes, plan.projected_storage_bytes)
    {
        (Some(archival), Some(projected)) if archival >= projected => Some(archival - projected),
        _ => None,
    };
    Ok(plan)
}

fn family_root(
    games: &[crate::dat::model::DatGameEntry],
    start: usize,
    names: &BTreeMap<&str, usize>,
) -> String {
    let mut current = start;
    let mut visited = BTreeSet::new();
    while visited.insert(current) {
        let Some(parent) = games[current].clone_of.as_deref() else {
            break;
        };
        let Some(next) = names.get(parent).copied() else {
            break;
        };
        current = next;
    }
    games[current].name.clone()
}

fn categories(game: &crate::dat::model::DatGameEntry) -> BTreeSet<MamePlayingLibraryCategory> {
    let evidence = dat_release_evidence(&game.name);
    let tokens = parenthesized_tokens(&game.name);
    let mut result = BTreeSet::new();
    if tokens.iter().any(|token| token == "bootleg") {
        result.insert(MamePlayingLibraryCategory::Bootleg);
    }
    if evidence
        .release_classes
        .iter()
        .any(|class| matches!(class, crate::playing_library::ReleaseClass::Proto))
    {
        result.insert(MamePlayingLibraryCategory::Prototype);
    }
    if tokens
        .iter()
        .any(|token| matches!(token.as_str(), "gambling" | "fruit machine" | "casino"))
    {
        result.insert(MamePlayingLibraryCategory::GamblingFruit);
    }
    if tokens.iter().any(|token| token == "mechanical") {
        result.insert(MamePlayingLibraryCategory::Mechanical);
    }
    if tokens
        .iter()
        .any(|token| matches!(token.as_str(), "non-video" | "non video"))
    {
        result.insert(MamePlayingLibraryCategory::NonVideo);
    }
    match game
        .runnable
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("no") => {
            result.insert(MamePlayingLibraryCategory::NonWorking);
        }
        Some("partial") => {
            result.insert(MamePlayingLibraryCategory::Imperfect);
        }
        _ => {}
    }
    result
}

fn category_allowed(
    category: MamePlayingLibraryCategory,
    preferences: &MamePlayingLibraryPreferences,
) -> bool {
    match category {
        MamePlayingLibraryCategory::Bootleg => preferences.include_bootlegs,
        MamePlayingLibraryCategory::Prototype => preferences.include_prototypes,
        MamePlayingLibraryCategory::GamblingFruit => preferences.include_gambling_fruit,
        MamePlayingLibraryCategory::Mechanical => preferences.include_mechanical,
        MamePlayingLibraryCategory::NonVideo => preferences.include_non_video,
        MamePlayingLibraryCategory::NonWorking => preferences.include_non_working,
        MamePlayingLibraryCategory::Imperfect => preferences.include_imperfect,
    }
}

fn election_key(
    game: &crate::dat::model::DatGameEntry,
    index: usize,
    preferences: &MamePlayingLibraryPreferences,
) -> (usize, usize, u16, u16, char, usize, String) {
    let evidence = dat_release_evidence(&game.name);
    let region_rank = evidence
        .regions
        .iter()
        .filter_map(|region| {
            preferences
                .preferred_regions
                .iter()
                .position(|preferred| preferred.eq_ignore_ascii_case(region))
        })
        .min()
        .unwrap_or(preferences.preferred_regions.len());
    let english_rank = if preferences.prefer_english
        && evidence
            .languages
            .iter()
            .any(|language| language.eq_ignore_ascii_case("en"))
    {
        0
    } else {
        1
    };
    let revision = evidence.revision.unwrap_or_default();
    let revision_key = if preferences.prefer_newest_revision {
        u16::MAX - revision.major
    } else {
        0
    };
    let parent_rank = if preferences.prefer_parent && game.clone_of.is_none() {
        0
    } else {
        1
    };
    (
        region_rank,
        english_rank,
        revision_key,
        u16::MAX - revision.minor,
        revision.letter,
        parent_rank + index / usize::MAX,
        game.name.clone(),
    )
}

fn meaningful_distinct(
    parent: &crate::dat::model::DatGameEntry,
    candidate: &crate::dat::model::DatGameEntry,
    preferences: &MamePlayingLibraryPreferences,
) -> bool {
    if preferences.keep_distinct_multiplayer && parent.mame_input != candidate.mame_input {
        return true;
    }
    preferences.keep_notable_regional_differences
        && !same_region(parent, candidate)
        && parent.description != candidate.description
}

fn same_region(
    left: &crate::dat::model::DatGameEntry,
    right: &crate::dat::model::DatGameEntry,
) -> bool {
    dat_release_evidence(&left.name).regions == dat_release_evidence(&right.name).regions
}

fn declared_dependencies(game: &crate::dat::model::DatGameEntry) -> Vec<String> {
    let mut dependencies: BTreeSet<String> = game
        .device_refs
        .iter()
        .filter_map(|item| item.name.clone())
        .collect();
    dependencies.extend(game.bios_sets.iter().filter_map(|item| item.name.clone()));
    dependencies.into_iter().collect()
}

fn parenthesized_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = name;
    while let Some(close) = rest.rfind(')') {
        let Some(open) = rest[..close].rfind('(') else {
            break;
        };
        tokens.extend(
            rest[open + 1..close]
                .split(',')
                .map(|token| token.trim().to_ascii_lowercase()),
        );
        rest = &rest[..open];
    }
    tokens
}

fn set_storage(set: &crate::mame_collection_analyser::MameObservedSet) -> Option<u64> {
    set.members.iter().map(|member| member.size_bytes).sum()
}

fn total_storage(inventory: &MameCollectionInventory) -> Option<u64> {
    inventory.sets.iter().map(set_storage).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::{
        DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatRomEntry, DatSource,
    };
    use crate::mame_collection_analyser::{MameObservedMember, MameObservedSet, MameSetResult};

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
    fn game(name: &str, parent: Option<&str>, size: u64) -> DatGameEntry {
        DatGameEntry {
            name: name.into(),
            clone_of: parent.map(str::to_string),
            roms: vec![DatRomEntry {
                name: "rom.bin".into(),
                size_bytes: Some(size),
                crc32: Some("aa".into()),
                ..Default::default()
            }],
            runnable: Some("yes".into()),
            ..Default::default()
        }
    }
    fn request<'a>(
        catalogue: &'a ParsedDat,
        inventory: &'a MameCollectionInventory,
        analysis: &'a MameCollectionAnalysis,
    ) -> MamePlayingLibraryRequest<'a> {
        MamePlayingLibraryRequest {
            catalogue,
            inventory,
            analysis,
            preferences: MamePlayingLibraryPreferences::default(),
        }
    }
    fn facts(catalogue: &ParsedDat, inventory: &MameCollectionInventory) -> MameCollectionAnalysis {
        MameCollectionAnalysis {
            schema_version: 1,
            emulator_version: Some("0.264".into()),
            collection_root: inventory.collection_root.clone(),
            discovered_sets: inventory.sets.len(),
            passing_sets: inventory.sets.len(),
            failing_sets: 0,
            unknown_sets: 0,
            style: crate::mame_collection_analyser::MameCollectionStyle::Split,
            version_compatibility:
                crate::mame_collection_analyser::MameVersionCompatibility::CompatibleWithCurrentMame,
            set_results: catalogue
                .games
                .iter()
                .map(|game| MameSetResult {
                    name: game.name.clone(),
                    health: MameSetHealth::Good,
                    expected_members: 1,
                    matched_members: 1,
                    missing_members: vec![],
                    parent: game.clone_of.clone(),
                })
                .collect(),
            top_missing_dependencies: vec![],
            parent_clone_coverage: Default::default(),
            bios_device_failures: vec![],
            chd_coverage: Default::default(),
            update_readiness: None,
            warnings: vec![],
        }
    }
    fn inventory(names: &[(&str, u64)]) -> MameCollectionInventory {
        MameCollectionInventory {
            collection_root: "/archive".into(),
            sets: names
                .iter()
                .map(|(name, size)| MameObservedSet {
                    name: (*name).into(),
                    location: format!("/archive/{name}.zip").into(),
                    directory: false,
                    members: vec![MameObservedMember {
                        name: "rom.bin".into(),
                        size_bytes: Some(*size),
                        crc32: Some("aa".into()),
                        md5: None,
                        sha1: None,
                    }],
                })
                .collect(),
            inspected_completely: true,
            warnings: vec![],
        }
    }

    #[test]
    fn parent_clone_family_elects_one_and_reports_savings() {
        let catalogue = catalogue(vec![
            game("pacman", None, 10),
            game("pacmanj", Some("pacman"), 11),
        ]);
        let inventory = inventory(&[("pacman", 10), ("pacmanj", 11)]);
        let analysis = facts(&catalogue, &inventory);
        let plan =
            build_mame_playing_library_plan(&request(&catalogue, &inventory, &analysis)).unwrap();
        assert_eq!(plan.selected_sets.len(), 1);
        assert_eq!(plan.projected_set_count, 1);
        assert_eq!(plan.projected_savings_bytes, Some(11));
    }

    #[test]
    fn source_inventory_is_not_mutated_and_bad_set_is_excluded() {
        let catalogue = catalogue(vec![game("pacman", None, 10)]);
        let inventory = inventory(&[("pacman", 10)]);
        let before = inventory.clone();
        let mut analysis = facts(&catalogue, &inventory);
        analysis.set_results[0].health = MameSetHealth::Bad;
        let plan =
            build_mame_playing_library_plan(&request(&catalogue, &inventory, &analysis)).unwrap();
        assert!(plan.selected_sets.is_empty());
        assert_eq!(inventory, before);
    }

    #[test]
    fn distinct_control_panel_variant_is_retained() {
        let mut parent = game("game", None, 10);
        parent.mame_input = Some(Default::default());
        let mut clone = game("game2", Some("game"), 10);
        clone.mame_input = Some(crate::dat::mame_input_metadata::MameInputMetadata {
            players: Some("2".into()),
            ..Default::default()
        });
        let catalogue = catalogue(vec![parent, clone]);
        let inventory = inventory(&[("game", 10), ("game2", 10)]);
        let analysis = facts(&catalogue, &inventory);
        let plan =
            build_mame_playing_library_plan(&request(&catalogue, &inventory, &analysis)).unwrap();
        assert_eq!(plan.selected_sets.len(), 2);
        assert!(
            plan.selected_sets
                .iter()
                .any(|set| set.meaningfully_distinct_clone)
        );
    }
}
