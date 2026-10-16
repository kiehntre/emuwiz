//! Metadata resolution for the existing MAME compatibility evaluator.
//! No collection IO, inventory inheritance, or compatibility verdicts here.
use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::dat::dependency::graph::{
    ChainFault, ChainGuard, DeclaredName, DependencyGraph, SetRef, declared_name,
};
use crate::dat::dependency::owner::{OwnerHop, RomOwner, rom_owner_with};
use crate::dat::dependency::{DependencyOutcome, DependencyRequirement, DependencyTarget};
use crate::dat::set::{MemberClass, classify_rom_member};

fn reference(
    declaring_set: &str,
    kind: DependencyKind,
    target: DependencyTarget,
    outcome: DependencyOutcome,
    via_member: Option<String>,
) -> MameDependencyReference {
    MameDependencyReference {
        declaring_set: declaring_set.into(),
        requirement: DependencyRequirement {
            kind,
            target,
            outcome,
            via_member,
        },
    }
}

fn named_target(value: &Option<String>) -> DependencyTarget {
    match declared_name(value) {
        DeclaredName::Named(name) => DependencyTarget::Set { name: name.into() },
        _ => DependencyTarget::Undeclared,
    }
}

/// A direct projection lacks the catalogue needed to prove external references.
pub(super) fn direct(expectation: &mut MameSetExpectation, game: &DatGameEntry) {
    for (kind, target) in edges(game) {
        expectation
            .dependencies
            .resolution
            .references
            .push(reference(
                &game.name,
                kind,
                named_target(&target),
                DependencyOutcome::EvidenceUnavailable,
                None,
            ));
    }
    expectation.roms.clear();
    expectation.disks.clear();
    append_members(expectation, game, None, &BTreeMap::new(), None);
}

fn edges(game: &DatGameEntry) -> Vec<(DependencyKind, Option<String>)> {
    let mut edges = Vec::new();
    if game.clone_of.is_some() {
        edges.push((DependencyKind::ParentSet, game.clone_of.clone()));
    }
    if game.rom_of.is_some() {
        edges.push((DependencyKind::RomSource, game.rom_of.clone()));
    }
    edges.extend(
        game.device_refs
            .iter()
            .map(|r| (DependencyKind::Device, r.name.clone())),
    );
    edges.sort();
    edges.dedup();
    edges
}

pub(super) fn resolve(
    dat: &ParsedDat,
    graph: &DependencyGraph<'_>,
    set_name: &str,
    layout: MameCollectionLayout,
    bios: &BTreeMap<String, String>,
) -> Option<MameSetExpectation> {
    let selected = match graph.resolve_set(set_name) {
        SetRef::Absent => return None,
        SetRef::Duplicate => {
            let mut expectation = MameSetExpectation::from_game(&DatGameEntry {
                name: set_name.into(),
                mame_listxml_schema_version: dat
                    .games
                    .iter()
                    .filter(|game| game.name == set_name)
                    .all(|game| {
                        game.mame_listxml_schema_version == Some(MAME_LISTXML_PARSER_SCHEMA_VERSION)
                    })
                    .then_some(MAME_LISTXML_PARSER_SCHEMA_VERSION),
                ..Default::default()
            });
            expectation
                .dependencies
                .resolution
                .references
                .push(reference(
                    set_name,
                    DependencyKind::ParentSet,
                    DependencyTarget::Set {
                        name: set_name.into(),
                    },
                    DependencyOutcome::Ambiguous,
                    None,
                ));
            return Some(expectation);
        }
        SetRef::Unique(index) => index,
    };
    let game = graph.game(selected);
    let mut expectation = MameSetExpectation::from_game(game);
    expectation.dependencies.resolution = MameDependencyResolution {
        layout,
        parser_schema_version: game.mame_listxml_schema_version,
        ..Default::default()
    };
    expectation.roms.clear();
    expectation.disks.clear();
    validate_graph(graph, selected, &mut expectation);

    // Only the subject, BIOS providers in its ROM-source chain and explicitly
    // referenced devices contribute requirements. Clone and ordinary ROM-source
    // inventories are never copied. A device/Bios's own declarations are support.
    let mut content = BTreeMap::from([(selected, None)]);
    let mut pending = vec![(selected, true)];
    let mut visited = BTreeSet::new();
    while let Some((index, include_devices)) = pending.pop() {
        if !visited.insert((index, include_devices)) {
            continue;
        }
        let current = graph.game(index);
        if current.mame_listxml_schema_version != Some(MAME_LISTXML_PARSER_SCHEMA_VERSION) {
            continue;
        }
        if let DeclaredName::Named(name) = declared_name(&current.rom_of)
            && let SetRef::Unique(provider) = graph.resolve_set(name)
        {
            let is_bios = status_is(graph.game(provider).is_bios.as_deref(), "yes");
            if is_bios {
                content
                    .entry(provider)
                    .or_insert(Some(DependencyKind::Bios));
            }
            pending.push((provider, is_bios));
        }
        if include_devices {
            for device in &current.device_refs {
                if let DeclaredName::Named(name) = declared_name(&device.name)
                    && let SetRef::Unique(device) = graph.resolve_set(name)
                {
                    content
                        .entry(device)
                        .or_insert(Some(DependencyKind::Device));
                    pending.push((device, true));
                }
            }
        }
    }
    let mut content: Vec<_> = content.into_iter().collect();
    content.sort_by_key(|(index, kind)| (kind.is_some(), graph.game(*index).name.clone()));
    let selected_bios = bios_selection(game, bios.get(&game.name)).ok().flatten();
    let mut effective_bios = bios.clone();
    for (index, kind) in content {
        let provider = graph.game(index);
        if kind == Some(DependencyKind::Bios)
            && let Some(selection) = &selected_bios
            && provider
                .bios_sets
                .iter()
                .any(|variant| variant.name.as_ref() == Some(selection))
        {
            if bios
                .get(&provider.name)
                .is_some_and(|explicit| explicit != selection)
            {
                expectation
                    .dependencies
                    .resolution
                    .references
                    .push(reference(
                        &game.name,
                        DependencyKind::Bios,
                        DependencyTarget::BiosSet {
                            set_name: provider.name.clone(),
                            bios_set: selection.clone(),
                        },
                        DependencyOutcome::Contradictory,
                        None,
                    ));
            }
            effective_bios.insert(provider.name.clone(), selection.clone());
        }
        append_members(
            &mut expectation,
            provider,
            kind,
            &effective_bios,
            Some((graph, index)),
        );
    }
    // Stable declaration order independent of machine order in the input XML.
    expectation
        .roms
        .sort_by_key(|rom| (rom.set_name.clone(), rom.name.clone()));
    expectation
        .disks
        .sort_by_key(|disk| (disk.set_name.clone(), disk.name.clone()));
    let resolution = &mut expectation.dependencies.resolution;
    resolution
        .roms
        .sort_by_key(|rom| (rom.declaring_set.clone(), rom.member_name.clone()));
    resolution.references.sort_by_key(|r| {
        (
            r.declaring_set.clone(),
            r.requirement.kind,
            r.requirement.target.clone(),
            r.requirement.via_member.clone(),
            r.requirement.outcome,
        )
    });
    resolution.references.dedup();
    resolution.stale_metadata_sets.sort();
    resolution.stale_metadata_sets.dedup();
    Some(expectation)
}

/// Iterative DFS uses an active path plus completed nodes. Shared subgraphs are
/// not cycles and are processed once, rather than expanding every diamond path.
fn validate_graph(
    graph: &DependencyGraph<'_>,
    selected: usize,
    expectation: &mut MameSetExpectation,
) {
    enum Step {
        Enter(usize, ChainGuard),
        Exit(usize),
    }
    let mut stack = vec![Step::Enter(selected, ChainGuard::default())];
    let mut done = BTreeSet::new();
    let mut names = BTreeSet::from([expectation.set_name.clone()]);
    while let Some(step) = stack.pop() {
        let (index, mut guard) = match step {
            Step::Exit(index) => {
                done.insert(index);
                continue;
            }
            Step::Enter(index, guard) => (index, guard),
        };
        if done.contains(&index) {
            continue;
        }
        if guard.visit(index).is_err() {
            continue;
        } // Checked on the declaring edge below.
        let game = graph.game(index);
        stack.push(Step::Exit(index));
        if game.mame_listxml_schema_version != Some(MAME_LISTXML_PARSER_SCHEMA_VERSION) {
            expectation
                .dependencies
                .resolution
                .stale_metadata_sets
                .push(game.name.clone());
            continue;
        }
        for (kind, target) in edges(game).into_iter().rev() {
            let mut outcome;
            match declared_name(&target) {
                DeclaredName::Absent | DeclaredName::Malformed => {
                    outcome = DependencyOutcome::Contradictory
                }
                DeclaredName::Named(name) => {
                    names.insert(name.into());
                    match graph.resolve_set(name) {
                        SetRef::Absent => outcome = DependencyOutcome::EvidenceUnavailable,
                        SetRef::Duplicate => outcome = DependencyOutcome::Ambiguous,
                        SetRef::Unique(next) => {
                            let mut next_guard = guard.clone();
                            outcome = match next_guard.visit(next) {
                                Err(ChainFault::Cycle) => DependencyOutcome::Cycle,
                                Err(ChainFault::DepthExceeded) => DependencyOutcome::Unsupported,
                                Ok(()) => {
                                    stack.push(Step::Enter(next, guard.clone()));
                                    DependencyOutcome::Satisfied
                                }
                            };
                            if kind == DependencyKind::Device
                                && !status_is(graph.game(next).is_device.as_deref(), "yes")
                            {
                                outcome = DependencyOutcome::Contradictory;
                            }
                            if kind == DependencyKind::RomSource
                                && status_is(graph.game(next).is_bios.as_deref(), "yes")
                            {
                                expectation
                                    .dependencies
                                    .resolution
                                    .references
                                    .push(reference(
                                        &game.name,
                                        DependencyKind::Bios,
                                        named_target(&target),
                                        outcome,
                                        None,
                                    ));
                            }
                        }
                    }
                }
            }
            expectation
                .dependencies
                .resolution
                .references
                .push(reference(
                    &game.name,
                    kind,
                    named_target(&target),
                    outcome,
                    None,
                ));
        }
        if game.unsupported_structure {
            expectation
                .dependencies
                .resolution
                .references
                .push(reference(
                    &game.name,
                    DependencyKind::RomSource,
                    DependencyTarget::Undeclared,
                    DependencyOutcome::Unsupported,
                    None,
                ));
        }
    }
    expectation.dependencies.closure_set_names = names.into_iter().collect();
}

fn bios_selection(
    game: &DatGameEntry,
    requested: Option<&String>,
) -> Result<Option<String>, DependencyOutcome> {
    let mut variants = BTreeSet::new();
    let mut defaults = Vec::new();
    for variant in &game.bios_sets {
        let DeclaredName::Named(name) = declared_name(&variant.name) else {
            return Err(DependencyOutcome::Contradictory);
        };
        if !variants.insert(name.to_string()) {
            return Err(DependencyOutcome::Ambiguous);
        }
        if status_is(variant.default.as_deref(), "yes") {
            defaults.push(name.to_string());
        } else if variant.default.is_some() && !status_is(variant.default.as_deref(), "no") {
            return Err(DependencyOutcome::Contradictory);
        }
    }
    for rom in &game.roms {
        if let Some(bios) = &rom.bios
            && !variants.contains(bios)
        {
            return Err(DependencyOutcome::Contradictory);
        }
    }
    if let Some(requested) = requested {
        return if variants.contains(requested) {
            Ok(Some(requested.clone()))
        } else {
            Err(DependencyOutcome::Contradictory)
        };
    }
    match defaults.len() {
        1 => Ok(defaults.pop()),
        0 if variants.len() <= 1 => Ok(variants.into_iter().next()),
        0 => Err(DependencyOutcome::EvidenceUnavailable),
        _ => Err(DependencyOutcome::Ambiguous),
    }
}

fn append_members<'a>(
    expectation: &mut MameSetExpectation,
    game: &'a DatGameEntry,
    kind: Option<DependencyKind>,
    bios: &BTreeMap<String, String>,
    graph: Option<(&DependencyGraph<'a>, usize)>,
) {
    if game.mame_listxml_schema_version != Some(MAME_LISTXML_PARSER_SCHEMA_VERSION) {
        return;
    }
    let selection = bios_selection(game, bios.get(&game.name));
    let selected_bios = match selection {
        Ok(selection) => {
            if let Some(name) = &selection {
                expectation
                    .dependencies
                    .resolution
                    .bios_selections
                    .insert(game.name.clone(), name.clone());
                expectation
                    .dependencies
                    .resolution
                    .references
                    .push(reference(
                        &game.name,
                        DependencyKind::Bios,
                        DependencyTarget::BiosSet {
                            set_name: game.name.clone(),
                            bios_set: name.clone(),
                        },
                        DependencyOutcome::Satisfied,
                        None,
                    ));
            }
            selection
        }
        Err(outcome) => {
            expectation
                .dependencies
                .resolution
                .references
                .push(reference(
                    &game.name,
                    DependencyKind::Bios,
                    DependencyTarget::Set {
                        name: game.name.clone(),
                    },
                    outcome,
                    None,
                ));
            None
        }
    };
    for rom in &game.roms {
        if rom.bios.is_some() && rom.bios != selected_bios {
            continue;
        }
        match classify_rom_member(rom) {
            MemberClass::NonFile | MemberClass::OptionalPhysical => continue,
            MemberClass::Contradictory | MemberClass::UnknownLoadflag => {
                expectation
                    .dependencies
                    .resolution
                    .references
                    .push(reference(
                        &game.name,
                        kind.unwrap_or(DependencyKind::RomSource),
                        DependencyTarget::Undeclared,
                        DependencyOutcome::Unsupported,
                        Some(rom.name.clone()),
                    ));
                continue;
            }
            _ => {}
        }
        let mut owner = Some(DependencyTarget::SetMember {
            set_name: game.name.clone(),
            member_name: rom.name.clone(),
        });
        let mut declarations = vec![expected_rom(&game.name, rom)];
        if rom.merge.is_some() {
            let resolved = graph.map_or(
                Err(DependencyOutcome::EvidenceUnavailable),
                |(graph, index)| merge_owner(graph, index, rom),
            );
            let outcome = match resolved {
                Ok((target, chain)) => {
                    owner = Some(target);
                    declarations = chain;
                    DependencyOutcome::Satisfied
                }
                Err(outcome) => {
                    owner = None;
                    outcome
                }
            };
            expectation
                .dependencies
                .resolution
                .references
                .push(reference(
                    &game.name,
                    DependencyKind::MergedRom,
                    owner
                        .clone()
                        .unwrap_or_else(|| DependencyTarget::SetMember {
                            set_name: game
                                .rom_of
                                .clone()
                                .or_else(|| game.clone_of.clone())
                                .unwrap_or_default(),
                            member_name: rom.merge.clone().unwrap_or_default(),
                        }),
                    outcome,
                    Some(rom.name.clone()),
                ));
        }
        // Dump status and optionality do not validate a declared merge marker.
        // Preserve the evaluator's existing NO_DUMP preservation-gap policy.
        if status_is(rom.optional.as_deref(), "yes") && !status_is(rom.status.as_deref(), "nodump")
        {
            continue;
        }
        let locations = locations(expectation, game, rom, owner.as_ref(), kind, graph);
        let mut required_rom = expected_rom(&game.name, rom);
        for declaration in &declarations {
            required_rom.size_bytes = required_rom.size_bytes.or(declaration.size_bytes);
            for checksum in &declaration.checksums {
                if !required_rom
                    .checksums
                    .iter()
                    .any(|fact| fact.algorithm == checksum.algorithm)
                {
                    required_rom.checksums.push(checksum.clone());
                }
            }
        }
        // A validated merge borrows the provider's content identity and its
        // known dump limitation. A clean/default borrower declaration cannot
        // upgrade BAD_DUMP bytes from any link in that explicit member chain.
        // Keep the original declarations intact as catalogue provenance.
        if (required_rom.status.is_none()
            || status_is(required_rom.status.as_deref().map(str::trim), "good"))
            && declarations.iter().any(|declaration| {
                status_is(declaration.status.as_deref().map(str::trim), "baddump")
            })
        {
            required_rom.status = Some("baddump".into());
        }
        required_rom.checksums.sort_by_key(|fact| fact.algorithm);
        expectation
            .dependencies
            .resolution
            .roms
            .push(MameRomRequirementEvidence {
                declaring_set: game.name.clone(),
                member_name: rom.name.clone(),
                supporting_kind: kind,
                owner,
                declarations,
                locations,
            });
        expectation.roms.push(required_rom);
    }
    for disk in &game.disks {
        if status_is(disk.optional.as_deref(), "yes") {
            continue;
        }
        if disk.merge.is_some() {
            expectation
                .dependencies
                .resolution
                .references
                .push(reference(
                    &game.name,
                    DependencyKind::MergedDisk,
                    DependencyTarget::SetMember {
                        set_name: game.rom_of.clone().unwrap_or_default(),
                        member_name: disk.merge.clone().unwrap_or_default(),
                    },
                    DependencyOutcome::Unsupported,
                    disk.name.clone(),
                ));
        }
        expectation.disks.push(MameExpectedDisk {
            set_name: game.name.clone(),
            name: disk.name.clone(),
            sha1: disk.sha1.clone(),
            status: disk.status.clone(),
            merge: disk.merge.clone(),
        });
    }
}

/// Resolves an explicit `merge=` through the shared catalogue owner resolver
/// (`romof`, or `cloneof` only when `romof` is absent; SHA-1 agreement
/// whenever either side publishes it). The evaluator adds only refusals:
/// every provider must carry current listxml provenance, and every borrowed
/// declaration must be an ordinary physical member. The chain of
/// declarations is returned as evidence.
fn merge_owner<'a>(
    graph: &DependencyGraph<'a>,
    index: usize,
    rom: &'a DatRomEntry,
) -> Result<(DependencyTarget, Vec<MameExpectedRom>), DependencyOutcome> {
    let mut declarations = vec![expected_rom(&graph.game(index).name, rom)];
    let owner = rom_owner_with(graph, index, rom, |hop| match hop {
        OwnerHop::Provider(provider) => {
            if graph.game(provider).mame_listxml_schema_version
                != Some(MAME_LISTXML_PARSER_SCHEMA_VERSION)
            {
                return Err(DependencyOutcome::EvidenceUnavailable);
            }
            Ok(())
        }
        OwnerHop::Declaration {
            provider,
            declaration,
        } => {
            if !matches!(
                classify_rom_member(declaration),
                MemberClass::PhysicalRequired
                    | MemberClass::OptionalPhysical
                    | MemberClass::Borrowed
                    | MemberClass::KnownBad
            ) {
                return Err(DependencyOutcome::Unsupported);
            }
            declarations.push(expected_rom(&graph.game(provider).name, declaration));
            Ok(())
        }
    })?;
    let (set_name, member_name) = match owner {
        RomOwner::Own => (graph.game(index).name.clone(), rom.name.clone()),
        RomOwner::Borrowed { owner, declaration } => {
            (graph.game(owner).name.clone(), declaration.name.clone())
        }
    };
    Ok((
        DependencyTarget::SetMember {
            set_name,
            member_name,
        },
        declarations,
    ))
}

fn locations(
    expectation: &MameSetExpectation,
    game: &DatGameEntry,
    rom: &DatRomEntry,
    owner: Option<&DependencyTarget>,
    kind: Option<DependencyKind>,
    graph: Option<(&DependencyGraph<'_>, usize)>,
) -> Vec<MameRomLocation> {
    let selected = &expectation.set_name;
    let layout = expectation.dependencies.resolution.layout;
    let mut locations = BTreeSet::new();
    let mut add = |set: &str, member: &str| {
        locations.insert(MameRomLocation {
            set_name: set.into(),
            member_name: member.into(),
        });
    };
    let merged_root = graph
        .map(|(graph, _)| {
            let mut name = selected.clone();
            let mut guard = ChainGuard::default();
            while let SetRef::Unique(index) = graph.resolve_set(&name) {
                if guard.visit(index).is_err() {
                    break;
                }
                if graph.game(index).mame_listxml_schema_version
                    != Some(MAME_LISTXML_PARSER_SCHEMA_VERSION)
                {
                    break;
                }
                match declared_name(&graph.game(index).clone_of) {
                    DeclaredName::Named(parent) => name = parent.into(),
                    _ => break,
                }
            }
            name
        })
        .unwrap_or_else(|| selected.clone());
    match layout {
        MameCollectionLayout::NonMerged => {
            add(selected, &rom.name);
            if kind.is_some() {
                add(&game.name, &rom.name);
                if let Some(DependencyTarget::SetMember {
                    set_name,
                    member_name,
                }) = owner
                {
                    add(set_name, member_name);
                }
            }
        }
        MameCollectionLayout::Split => {
            if let Some(DependencyTarget::SetMember {
                set_name,
                member_name,
            }) = owner
            {
                add(set_name, member_name);
            } else {
                add(&game.name, &rom.name);
            }
        }
        MameCollectionLayout::Merged => {
            add(&merged_root, &rom.name);
            add(&merged_root, &format!("{}/{}", game.name, rom.name));
            if kind.is_some() {
                add(&game.name, &rom.name);
            }
            if let Some(DependencyTarget::SetMember {
                set_name,
                member_name,
            }) = owner
            {
                if set_name != &game.name || kind.is_some() {
                    add(set_name, member_name);
                }
            }
        }
        MameCollectionLayout::RuntimeSearch => {
            add(selected, &rom.name);
            add(&game.name, &rom.name);
            add(&merged_root, &rom.name);
            if let Some(DependencyTarget::SetMember {
                set_name,
                member_name,
            }) = owner
            {
                add(set_name, member_name);
            }
        }
    }
    // Search locations express where MAME can look, not who owns a declaration.
    // A source/ancestor contributes no inventory: every candidate still has to
    // prove this specific required member's size and checksums in the evaluator.
    if let Some((graph, index)) = graph {
        if kind == Some(DependencyKind::Device) {
            // Device ROMs also search the system, its parents and corresponding
            // BIOS archives. These are locations, not owners or inventories.
            if let SetRef::Unique(system) = graph.resolve_set(selected) {
                for system_name in
                    std::iter::once(selected.clone()).chain(search_chain(graph, system, true))
                {
                    add(&system_name, &rom.name);
                    if let SetRef::Unique(system) = graph.resolve_set(&system_name) {
                        for source in search_chain(graph, system, false) {
                            if let SetRef::Unique(source_index) = graph.resolve_set(&source)
                                && graph.game(source_index).mame_listxml_schema_version
                                    == Some(MAME_LISTXML_PARSER_SCHEMA_VERSION)
                                && status_is(graph.game(source_index).is_bios.as_deref(), "yes")
                            {
                                add(&source, &rom.name);
                            }
                        }
                    }
                }
            }
        }
        if layout != MameCollectionLayout::NonMerged {
            add(&game.name, &rom.name);
            for name in search_chain(graph, index, true)
                .into_iter()
                .chain(search_chain(graph, index, false))
            {
                add(&name, &rom.name);
            }
        } else if kind.is_some() {
            for name in search_chain(graph, index, false) {
                add(&name, &rom.name);
            }
        }
    }
    locations.into_iter().collect()
}

fn search_chain(graph: &DependencyGraph<'_>, mut index: usize, lineage: bool) -> Vec<String> {
    let mut names = Vec::new();
    let mut guard = ChainGuard::default();
    while guard.visit(index).is_ok() {
        let game = graph.game(index);
        if game.mame_listxml_schema_version != Some(MAME_LISTXML_PARSER_SCHEMA_VERSION) {
            break;
        }
        let target = if lineage {
            &game.clone_of
        } else {
            &game.rom_of
        };
        let DeclaredName::Named(name) = declared_name(target) else {
            break;
        };
        let SetRef::Unique(next) = graph.resolve_set(name) else {
            break;
        };
        names.push(graph.game(next).name.clone());
        index = next;
    }
    names
}
