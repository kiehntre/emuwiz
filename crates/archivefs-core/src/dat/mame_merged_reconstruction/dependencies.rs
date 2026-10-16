//! Read-only dependency facts for one merged reconstruction family.
//!
//! A merged set holds the parent's and its clones' own ROMs. Members a family
//! set borrows from a set outside the family (normally a BIOS through
//! `romof` and `<rom merge>`) live in that provider's own set, as do device
//! ROMs and CHDs. This module decides, from catalogue declarations only, which
//! family members belong in the merged output and reports every dependency the
//! output does not contain. It performs no collection IO and gives no
//! runnability verdict: storage requirements being met never proves a machine
//! runs.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::super::model::{DatGameEntry, DatRomEntry, ParsedDat};
use super::is_non_physical;

/// Shown with every dependency report.
pub const RECONSTRUCTION_RUNNABILITY_NOTE: &str = "A rebuilt merged set contains only the parent's and clones' own ROMs. It is not self-contained: BIOS, device, disk (CHD) and sample sets stay separate and are not checked by this rebuild. Meeting storage requirements does not prove any machine runs.";

/// Longest `romof`/`merge` chain followed before a member is called unresolved.
const MAX_MERGE_DEPTH: usize = 16;

/// Where one declared family ROM belongs in a merged layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MemberPlacement {
    /// Stored in the merged output.
    Family,
    /// Stored in a set outside the family; not part of this output.
    External { provider: String },
    /// The catalogue does not prove where the member belongs.
    Unresolved(String),
}

/// Whether a named set resolves to exactly one catalogue entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogueReference {
    Unique,
    Absent,
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSetKind {
    /// A `romof` provider the catalogue marks `isbios="yes"`.
    Bios,
    /// A `romof` provider outside the family that is not marked as a BIOS.
    RomSource,
    /// A `<device_ref>` target, directly or through another device.
    Device,
}

impl ExternalSetKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bios => "BIOS",
            Self::RomSource => "ROM source set",
            Self::Device => "device",
        }
    }
}

/// A set the merged output depends on but does not contain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSupportSet {
    pub name: String,
    pub kind: ExternalSetKind,
    pub catalogue: CatalogueReference,
    /// Physical ROMs the catalogue declares for that set. Zero means nothing
    /// has to be stored for it.
    pub declared_roms: usize,
    /// Family sets that declare the dependency.
    pub needed_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExcludedMemberReason {
    /// Stored in this provider set, outside the merged output.
    ProvidedBy(String),
    /// The catalogue records no dump for this member.
    NoDump,
    /// Declared optional.
    Optional,
    /// A load instruction rather than a physical ROM file.
    LoadInstruction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedMember {
    pub declaring_set: String,
    pub member_name: String,
    pub reason: ExcludedMemberReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedMember {
    pub declaring_set: String,
    pub member_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredDisk {
    pub declaring_set: String,
    pub name: String,
}

/// Catalogue-only dependency facts for a reconstruction family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconstructionDependencies {
    pub dat_version: String,
    pub requested_set: String,
    pub parent: String,
    pub clones: Vec<String>,
    pub external_sets: Vec<ExternalSupportSet>,
    pub excluded_members: Vec<ExcludedMember>,
    pub unresolved_members: Vec<UnresolvedMember>,
    /// CHDs are never read, copied or verified by this rebuild.
    pub disks: Vec<DeclaredDisk>,
    pub samples: Vec<String>,
}

impl ReconstructionDependencies {
    /// External sets that must be stored separately because they declare ROMs.
    pub fn stored_separately(&self) -> impl Iterator<Item = &ExternalSupportSet> {
        self.external_sets
            .iter()
            .filter(|set| set.declared_roms > 0 || set.catalogue != CatalogueReference::Unique)
    }
}

/// Catalogue lookup shared by the planner and the dependency report.
pub(super) struct FamilyCatalogue<'a> {
    by_name: BTreeMap<&'a str, Vec<&'a DatGameEntry>>,
    family: BTreeSet<&'a str>,
}

impl<'a> FamilyCatalogue<'a> {
    pub(super) fn new(dat: &'a ParsedDat, family: impl IntoIterator<Item = &'a str>) -> Self {
        let mut by_name: BTreeMap<&str, Vec<&DatGameEntry>> = BTreeMap::new();
        for game in &dat.games {
            by_name.entry(game.name.as_str()).or_default().push(game);
        }
        Self {
            by_name,
            family: family.into_iter().collect(),
        }
    }

    fn reference(&self, name: &str) -> (CatalogueReference, Option<&'a DatGameEntry>) {
        match self.by_name.get(name).map(Vec::as_slice) {
            None | Some([]) => (CatalogueReference::Absent, None),
            Some([game]) => (CatalogueReference::Unique, Some(game)),
            Some(_) => (CatalogueReference::Duplicate, None),
        }
    }

    /// Follows `<rom merge>` through the declaring set's `romof` chain. A
    /// member with no `merge` is the declaring set's own. The first provider
    /// outside the family owns the member externally; every provider inside it
    /// must declare the merged name with an agreeing checksum.
    pub(super) fn placement(&self, game: &DatGameEntry, rom: &DatRomEntry) -> MemberPlacement {
        let mut current = game;
        let mut borrowed = rom;
        for _ in 0..MAX_MERGE_DEPTH {
            let Some(merge) = non_empty(&borrowed.merge) else {
                return MemberPlacement::Family;
            };
            let Some(provider_name) = non_empty(&current.rom_of) else {
                return MemberPlacement::Unresolved(format!(
                    "{} merges {merge} but {} declares no romof provider",
                    borrowed.name, current.name
                ));
            };
            let (reference, provider) = self.reference(provider_name);
            let Some(provider) = provider else {
                return MemberPlacement::Unresolved(format!(
                    "romof provider {provider_name} is {} in the catalogue",
                    describe_reference(reference)
                ));
            };
            let declared = provider
                .roms
                .iter()
                .filter(|candidate| candidate.name == merge)
                .collect::<Vec<_>>();
            let [target] = declared.as_slice() else {
                return MemberPlacement::Unresolved(format!(
                    "{provider_name} declares {} member(s) named {merge}",
                    declared.len()
                ));
            };
            if checksums_disagree(borrowed, target) {
                return MemberPlacement::Unresolved(format!(
                    "{provider_name}:{merge} has a different checksum from {}:{}",
                    current.name, borrowed.name
                ));
            }
            if !self.family.contains(provider.name.as_str()) {
                return MemberPlacement::External {
                    provider: provider.name.clone(),
                };
            }
            current = provider;
            borrowed = target;
        }
        MemberPlacement::Unresolved(format!(
            "{} has a merge chain longer than {MAX_MERGE_DEPTH} sets",
            rom.name
        ))
    }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

fn describe_reference(reference: CatalogueReference) -> &'static str {
    match reference {
        CatalogueReference::Unique => "present",
        CatalogueReference::Absent => "absent",
        CatalogueReference::Duplicate => "declared more than once",
    }
}

fn checksums_disagree(a: &DatRomEntry, b: &DatRomEntry) -> bool {
    let differs = |x: &Option<String>, y: &Option<String>| match (x, y) {
        (Some(x), Some(y)) => !x.eq_ignore_ascii_case(y),
        _ => false,
    };
    differs(&a.sha1, &b.sha1)
        || differs(&a.crc32, &b.crc32)
        || matches!((a.size_bytes, b.size_bytes), (Some(x), Some(y)) if x != y)
}

fn status_is_yes(value: &Option<String>) -> bool {
    value
        .as_deref()
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("yes"))
}

/// What kind of family `requested_set` belongs to, from catalogue names only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconstructionFamilyShape {
    /// A parent with at least one clone: a merged reconstruction applies.
    ParentWithClones { parent: String, clones: Vec<String> },
    /// A valid parent with no clones. Its merged, split and non-merged sets
    /// hold the same own ROMs, so there is nothing to merge.
    StandaloneParent { parent: String },
    /// The catalogue does not prove a single parent for this set.
    IncompleteIdentity { reason: String },
}

/// Classifies the family of `requested_set` without building a plan. An
/// absent set is an error; a clone-free parent is a valid shape, not a fault.
pub fn reconstruction_family_shape(
    dat: &ParsedDat,
    requested_set: &str,
) -> Result<ReconstructionFamilyShape, String> {
    let selected = dat
        .games
        .iter()
        .filter(|game| game.name == requested_set)
        .collect::<Vec<_>>();
    let selected = match selected.as_slice() {
        [] => {
            return Err(format!(
                "MAME set is absent from the selected catalogue: {requested_set}"
            ));
        }
        [game] => *game,
        _ => {
            return Ok(ReconstructionFamilyShape::IncompleteIdentity {
                reason: format!("{requested_set} is declared more than once in the catalogue"),
            });
        }
    };
    let parent = non_empty(&selected.clone_of).unwrap_or(selected.name.as_str());
    let parents = dat.games.iter().filter(|game| game.name == parent).count();
    if parents != 1 {
        return Ok(ReconstructionFamilyShape::IncompleteIdentity {
            reason: if parents == 0 {
                format!("parent set {parent} is absent from the catalogue")
            } else {
                format!("parent set {parent} is declared more than once in the catalogue")
            },
        });
    }
    let clones = dat
        .games
        .iter()
        .filter(|game| game.name != parent && game.clone_of.as_deref() == Some(parent))
        .map(|game| game.name.clone())
        .collect::<Vec<_>>();
    Ok(if clones.is_empty() {
        ReconstructionFamilyShape::StandaloneParent {
            parent: parent.to_string(),
        }
    } else {
        ReconstructionFamilyShape::ParentWithClones {
            parent: parent.to_string(),
            clones,
        }
    })
}

/// Builds the dependency report for the family containing `requested_set`.
/// Pure catalogue reading: no file, database or collection access.
pub fn reconstruction_dependencies(
    dat: &ParsedDat,
    requested_set: &str,
) -> Result<ReconstructionDependencies, String> {
    let family_names = super::reconstruction_family_names(dat, requested_set)?;
    let selected = dat
        .games
        .iter()
        .find(|game| game.name == requested_set)
        .ok_or_else(|| {
            format!("MAME set is absent from the selected catalogue: {requested_set}")
        })?;
    let parent = selected
        .clone_of
        .clone()
        .unwrap_or_else(|| selected.name.clone());
    let catalogue = FamilyCatalogue::new(dat, family_names.iter().map(String::as_str));
    let family_games = dat
        .games
        .iter()
        .filter(|game| catalogue.family.contains(game.name.as_str()))
        .collect::<Vec<_>>();

    let mut external: BTreeMap<String, ExternalSupportSet> = BTreeMap::new();
    let mut note_external = |name: &str, kind: ExternalSetKind, needed_by: &str| {
        let (reference, game) = catalogue.reference(name);
        let entry = external
            .entry(name.to_string())
            .or_insert_with(|| ExternalSupportSet {
                name: name.to_string(),
                kind,
                catalogue: reference,
                declared_roms: game.map_or(0, |game| {
                    game.roms.iter().filter(|rom| !is_non_physical(rom)).count()
                }),
                needed_by: Vec::new(),
            });
        if !entry.needed_by.iter().any(|set| set == needed_by) {
            entry.needed_by.push(needed_by.to_string());
        }
    };

    let mut excluded = Vec::new();
    let mut unresolved = Vec::new();
    let mut disks = Vec::new();
    let mut samples = BTreeSet::new();
    let mut device_queue = Vec::new();
    for game in &family_games {
        if let Some(provider) = non_empty(&game.rom_of)
            && !catalogue.family.contains(provider)
        {
            let (_, provider_game) = catalogue.reference(provider);
            let kind = if provider_game.is_some_and(|p| status_is_yes(&p.is_bios)) {
                ExternalSetKind::Bios
            } else {
                ExternalSetKind::RomSource
            };
            note_external(provider, kind, &game.name);
        }
        for device in game.device_refs.iter().filter_map(|d| non_empty(&d.name)) {
            device_queue.push((device.to_string(), game.name.clone()));
        }
        if let Some(sample) = non_empty(&game.sample_of) {
            samples.insert(sample.to_string());
        }
        for disk in &game.disks {
            disks.push(DeclaredDisk {
                declaring_set: game.name.clone(),
                name: disk.name.clone().unwrap_or_default(),
            });
        }
        for rom in &game.roms {
            let reason = if rom
                .status
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case("nodump"))
            {
                Some(ExcludedMemberReason::NoDump)
            } else if rom
                .optional
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case("yes"))
            {
                Some(ExcludedMemberReason::Optional)
            } else if rom.loadflag.is_some() {
                Some(ExcludedMemberReason::LoadInstruction)
            } else {
                match catalogue.placement(game, rom) {
                    MemberPlacement::Family => None,
                    MemberPlacement::External { provider } => {
                        Some(ExcludedMemberReason::ProvidedBy(provider))
                    }
                    MemberPlacement::Unresolved(reason) => {
                        unresolved.push(UnresolvedMember {
                            declaring_set: game.name.clone(),
                            member_name: rom.name.clone(),
                            reason,
                        });
                        None
                    }
                }
            };
            if let Some(reason) = reason {
                excluded.push(ExcludedMember {
                    declaring_set: game.name.clone(),
                    member_name: rom.name.clone(),
                    reason,
                });
            }
        }
    }
    // Devices are resolved transitively; each is visited once.
    let mut seen_devices = BTreeSet::new();
    while let Some((device, needed_by)) = device_queue.pop() {
        note_external(&device, ExternalSetKind::Device, &needed_by);
        if !seen_devices.insert(device.clone()) {
            continue;
        }
        if let (_, Some(game)) = catalogue.reference(&device) {
            for nested in game.device_refs.iter().filter_map(|d| non_empty(&d.name)) {
                device_queue.push((nested.to_string(), needed_by.clone()));
            }
        }
    }
    let mut external_sets = external.into_values().collect::<Vec<_>>();
    for set in &mut external_sets {
        set.needed_by.sort();
    }
    external_sets.sort_by(|a, b| {
        a.kind
            .cmp_rank()
            .cmp(&b.kind.cmp_rank())
            .then(a.name.cmp(&b.name))
    });
    Ok(ReconstructionDependencies {
        dat_version: dat.source.version.clone().unwrap_or_default(),
        requested_set: requested_set.to_string(),
        clones: family_names
            .iter()
            .filter(|name| **name != parent)
            .cloned()
            .collect(),
        parent,
        external_sets,
        excluded_members: excluded,
        unresolved_members: unresolved,
        disks,
        samples: samples.into_iter().collect(),
    })
}

impl ExternalSetKind {
    fn cmp_rank(self) -> u8 {
        match self {
            Self::Bios => 0,
            Self::RomSource => 1,
            Self::Device => 2,
        }
    }
}
