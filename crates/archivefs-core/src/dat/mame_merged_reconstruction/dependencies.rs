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

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::super::dependency::{
    DependencyOutcome,
    graph::{DependencyGraph, SetRef},
    owner::{OwnerHop, RomOwner, rom_owner_with},
};
use super::super::model::{DatGameEntry, DatRomEntry, ParsedDat};
use super::is_non_physical;

/// Shown with every dependency report.
pub const RECONSTRUCTION_RUNNABILITY_NOTE: &str = "A rebuilt merged set contains only the parent's and clones' own ROMs. It is not self-contained: BIOS, device, disk (CHD) and sample sets stay separate and are not checked by this rebuild. Meeting storage requirements does not prove any machine runs.";

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

/// Who supplies one ROM a referenced device requires. Device dependency is
/// kept apart from clone lineage and from ROM-source (`romof`) inheritance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceMemberSupply {
    /// Stored in the device's own set.
    DeviceOwn,
    /// Declared in the device but stored in this other set by explicit merge.
    BorrowedFrom(String),
}

/// One ROM a referenced device requires, with the device that declares it and
/// the family sets that depend on that device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceMember {
    pub device: String,
    pub member_name: String,
    pub supply: DeviceMemberSupply,
    pub needed_by: Vec<String>,
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
    #[serde(default)]
    pub unresolved_relationships: Vec<String>,
    /// ROMs required through `<device_ref>`, resolved to their terminal owner.
    #[serde(default)]
    pub device_members: Vec<DeviceMember>,
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
    family: BTreeSet<&'a str>,
    graph: DependencyGraph<'a>,
}

impl<'a> FamilyCatalogue<'a> {
    pub(super) fn new(dat: &'a ParsedDat, family: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            graph: DependencyGraph::build(&dat.games),
            family: family.into_iter().collect(),
        }
    }

    fn reference(&self, name: &str) -> (CatalogueReference, Option<&'a DatGameEntry>) {
        match self.graph.resolve_set(name) {
            SetRef::Absent => (CatalogueReference::Absent, None),
            SetRef::Unique(index) => (CatalogueReference::Unique, Some(self.graph.game(index))),
            SetRef::Duplicate => (CatalogueReference::Duplicate, None),
        }
    }

    /// Resolve every declaration through the canonical content-owner rules.
    /// MAME reconstruction requires explicit ROM sourcing, never clone lineage.
    pub(super) fn placement(&self, game: &DatGameEntry, rom: &DatRomEntry) -> MemberPlacement {
        let SetRef::Unique(index) = self.graph.resolve_set(&game.name) else {
            return MemberPlacement::Unresolved(format!(
                "{} has ambiguous catalogue identity",
                game.name
            ));
        };
        if rom.merge.is_some() && game.rom_of.is_none() {
            return MemberPlacement::Unresolved(format!(
                "{} declares merge without romof",
                game.name
            ));
        }
        let result = rom_owner_with(&self.graph, index, rom, |hop| {
            if let OwnerHop::Declaration {
                provider,
                declaration,
            } = hop
            {
                if declaration.merge.is_some() && self.graph.game(provider).rom_of.is_none() {
                    return Err(DependencyOutcome::Contradictory);
                }
            }
            Ok(())
        });
        match result {
            Ok(RomOwner::Own) => MemberPlacement::Family,
            Ok(RomOwner::Borrowed { owner, .. }) => {
                let provider = &self.graph.game(owner).name;
                if self.family.contains(provider.as_str()) {
                    MemberPlacement::Family
                } else {
                    MemberPlacement::External {
                        provider: provider.clone(),
                    }
                }
            }
            Err(outcome) => MemberPlacement::Unresolved(format!(
                "{}:{} merge {:?} via romof {:?}: {outcome:?}",
                game.name, rom.name, rom.merge, game.rom_of
            )),
        }
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
    let parent_game = dat.games.iter().find(|game| game.name == parent).unwrap();
    if parent_game.clone_of.is_some()
        || selected
            .clone_of
            .as_ref()
            .is_some_and(|v| v.trim().is_empty())
    {
        return Ok(ReconstructionFamilyShape::IncompleteIdentity {
            reason:
                "cyclic, malformed or nested clone lineage is not a supported reconstruction family"
                    .into(),
        });
    }
    let clones = dat
        .games
        .iter()
        .filter(|game| game.name != parent && non_empty(&game.clone_of) == Some(parent))
        .map(|game| game.name.clone())
        .collect::<Vec<_>>();
    let unique: BTreeSet<_> = clones.iter().map(String::as_str).collect();
    if unique.len() != clones.len()
        || dat
            .games
            .iter()
            .filter(|g| unique.contains(g.name.as_str()))
            .count()
            != clones.len()
        || dat
            .games
            .iter()
            .any(|g| non_empty(&g.clone_of).is_some_and(|p| unique.contains(p)))
    {
        return Ok(ReconstructionFamilyShape::IncompleteIdentity {
            reason: "duplicate or nested clone lineage is not a supported reconstruction family"
                .into(),
        });
    }
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
    let parent = family_names[0].clone();
    let catalogue = FamilyCatalogue::new(dat, family_names.iter().map(String::as_str));
    let family_games = dat
        .games
        .iter()
        .filter(|game| catalogue.family.contains(game.name.as_str()))
        .collect::<Vec<_>>();

    let external: RefCell<BTreeMap<(String, u8), ExternalSupportSet>> =
        RefCell::new(BTreeMap::new());
    let note_external = |name: &str, kind: ExternalSetKind, needed_by: &str| {
        let (reference, game) = catalogue.reference(name);
        let mut external = external.borrow_mut();
        let entry = external
            .entry((name.to_string(), kind.cmp_rank()))
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
    let mut unresolved_relationships = BTreeSet::new();
    for game in &family_games {
        collect_support(
            &catalogue,
            game,
            &note_external,
            &mut unresolved_relationships,
        );
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
                        let (_, source) = catalogue.reference(&provider);
                        let kind = if source.is_some_and(|g| status_is_yes(&g.is_bios)) {
                            ExternalSetKind::Bios
                        } else {
                            ExternalSetKind::RomSource
                        };
                        note_external(&provider, kind, &game.name);
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
    let device_members = resolve_device_members(
        &catalogue,
        &external,
        &note_external,
        &mut unresolved_relationships,
    );
    let mut external_sets = external.into_inner().into_values().collect::<Vec<_>>();
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
        unresolved_relationships: unresolved_relationships.into_iter().collect(),
        device_members,
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

/// Resolves the ROMs of every referenced device through the canonical owner
/// rules. A device that only aliases another set's ROMs stores nothing itself;
/// the terminal owner is reported instead. Absent, duplicate, cyclic, merge-less
/// or conflicting declarations stay unresolved, never guessed from file names.
fn resolve_device_members(
    catalogue: &FamilyCatalogue<'_>,
    external: &RefCell<BTreeMap<(String, u8), ExternalSupportSet>>,
    note: &impl Fn(&str, ExternalSetKind, &str),
    unresolved: &mut BTreeSet<String>,
) -> Vec<DeviceMember> {
    let devices = external
        .borrow()
        .values()
        .filter(|set| set.kind == ExternalSetKind::Device)
        .map(|set| (set.name.clone(), set.needed_by.clone()))
        .collect::<Vec<_>>();
    let mut members = Vec::new();
    for (name, needed_by) in devices {
        // Absent or duplicate devices are already reported by the support traversal.
        let (_, Some(device)) = catalogue.reference(&name) else {
            continue;
        };
        if !status_is_yes(&device.is_device) {
            continue;
        }
        let mut own = 0;
        for rom in device.roms.iter().filter(|rom| !is_non_physical(rom)) {
            let supply = match catalogue.placement(device, rom) {
                MemberPlacement::Family if rom.merge.is_some() => {
                    // Merged into a family set: the device would depend on the
                    // family's own ROMs. Device ownership is never family lineage.
                    unresolved.insert(format!(
                        "device {name}: member {} merges into a family set, not a device or ROM-source owner",
                        rom.name
                    ));
                    continue;
                }
                MemberPlacement::Family => {
                    own += 1;
                    DeviceMemberSupply::DeviceOwn
                }
                MemberPlacement::External { provider } => {
                    let (_, source) = catalogue.reference(&provider);
                    let kind = if source.is_some_and(|g| status_is_yes(&g.is_bios)) {
                        ExternalSetKind::Bios
                    } else {
                        ExternalSetKind::RomSource
                    };
                    for dependent in &needed_by {
                        note(&provider, kind, dependent);
                    }
                    DeviceMemberSupply::BorrowedFrom(provider)
                }
                MemberPlacement::Unresolved(reason) => {
                    unresolved.insert(format!(
                        "device {name}: member {} is unresolved: {reason}",
                        rom.name
                    ));
                    continue;
                }
            };
            members.push(DeviceMember {
                device: name.clone(),
                member_name: rom.name.clone(),
                supply,
                needed_by: needed_by.clone(),
            });
        }
        // Only ROMs the device owns are stored in its own set.
        if let Some(set) = external
            .borrow_mut()
            .get_mut(&(name.clone(), ExternalSetKind::Device.cmp_rank()))
        {
            set.declared_roms = own;
        }
    }
    members.sort_by(|a, b| (&a.device, &a.member_name).cmp(&(&b.device, &b.member_name)));
    members
}

/// Bounded metadata traversal. ROM sourcing carries BIOS requirements, but an
/// ordinary source's device dependencies do not become the subject's devices.
fn collect_support(
    catalogue: &FamilyCatalogue<'_>,
    subject: &DatGameEntry,
    note: &impl Fn(&str, ExternalSetKind, &str),
    unresolved: &mut BTreeSet<String>,
) {
    let mut queue = vec![(subject.name.clone(), true, Vec::<String>::new())];
    let mut seen = BTreeSet::new();
    while let Some((name, include_devices, mut path)) = queue.pop() {
        if path.len() >= 64 || path.contains(&name) {
            unresolved.insert(format!(
                "{}: cyclic or over-deep support relationship at {name}",
                subject.name
            ));
            continue;
        }
        if !seen.insert((name.clone(), include_devices)) {
            continue;
        }
        path.push(name.clone());
        let (reference, game) = catalogue.reference(&name);
        let Some(game) = game else {
            unresolved.insert(format!(
                "{}: support {name} is {}",
                subject.name,
                describe_reference(reference)
            ));
            continue;
        };
        if let Some(raw) = &game.rom_of {
            let provider = raw.trim();
            if provider.is_empty() {
                unresolved.insert(format!("{name}: unnamed romof relationship"));
            } else {
                let (reference, target) = catalogue.reference(provider);
                let bios = target.is_some_and(|g| status_is_yes(&g.is_bios));
                if bios {
                    note(provider, ExternalSetKind::Bios, &subject.name);
                }
                if reference != CatalogueReference::Unique {
                    // Preserve the declared unresolved reference without guessing BIOS identity.
                    note(provider, ExternalSetKind::RomSource, &subject.name);
                    unresolved.insert(format!(
                        "{name}: romof {provider} is {}",
                        describe_reference(reference)
                    ));
                }
                queue.push((provider.to_string(), bios, path.clone()));
            }
        }
        if include_devices {
            for device in &game.device_refs {
                let Some(device) = non_empty(&device.name) else {
                    unresolved.insert(format!("{name}: unnamed device relationship"));
                    continue;
                };
                note(device, ExternalSetKind::Device, &subject.name);
                let (_, target) = catalogue.reference(device);
                if target.is_some_and(|g| !status_is_yes(&g.is_device)) {
                    unresolved.insert(format!("{name}: {device} is not confirmed as a device"));
                }
                queue.push((device.to_string(), true, path.clone()));
            }
        }
    }
}
