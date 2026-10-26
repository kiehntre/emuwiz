//! Required material support for Playing Library election.
//! Reuses verified collection evidence; does not infer machine runnability.

use std::collections::{BTreeMap, BTreeSet};

use crate::dat::dependency::graph::{
    ChainGuard, DeclaredName, DependencyGraph, Flag, MemberRef, SetRef, declared_name, parse_flag,
};
use crate::dat::dependency::owner::{OwnerHop, rom_owner_with};
use crate::dat::model::DatGameEntry;
use crate::dat::set::{
    MemberClass, classify_disk_member, classify_rom_member, declared_disks, declared_roms,
};
use crate::mame_collection_analyser::{MameSetHealth, MameStorageEvidence};

#[derive(Default)]
pub(super) struct RequiredEvidence {
    pub reasons: Vec<String>,
    pub support_sets: BTreeMap<String, &'static str>,
}

pub(super) struct RequiredSupport<'a> {
    graph: DependencyGraph<'a>,
    health: BTreeMap<&'a str, MameSetHealth>,
}

impl<'a> RequiredSupport<'a> {
    pub fn new(games: &'a [DatGameEntry], health: BTreeMap<&'a str, MameSetHealth>) -> Self {
        Self {
            graph: DependencyGraph::build(games),
            health,
        }
    }

    pub fn assess(
        &self,
        subject: usize,
        evidence: Option<&MameStorageEvidence>,
    ) -> RequiredEvidence {
        // The current evaluator includes active BIOS/device declarations together,
        // with the subject's effective BIOS choice and supported storage layout.
        // Preserve that evidence instead of substituting a provider's default BIOS.
        let covered: BTreeSet<&str> = evidence
            .filter(|e| e.verified_complete())
            .into_iter()
            .flat_map(|e| e.requirements.iter().map(|r| r.declaring_set.as_str()))
            .collect();
        // A verified child alias can discharge its exact parent declaration in
        // a non-merged layout. Reuse the validated borrow chain, never a global
        // hash/name match or an assumption that the whole provider is healthy.
        let mut borrowed_declarations = BTreeSet::new();
        for requirement in evidence
            .filter(|e| e.verified_complete())
            .into_iter()
            .flat_map(|e| &e.requirements)
            .filter(|r| !r.disk)
        {
            let SetRef::Unique(declaring) = self.graph.resolve_set(&requirement.declaring_set)
            else {
                continue;
            };
            let MemberRef::Unique((_, rom)) =
                self.graph.rom_declared_as(declaring, &requirement.member)
            else {
                continue;
            };
            let mut chain = Vec::new();
            if rom_owner_with(&self.graph, declaring, rom, |hop| {
                if let OwnerHop::Declaration {
                    provider,
                    declaration,
                } = hop
                {
                    chain.push((provider, declaration.name.as_str()));
                }
                Ok(())
            })
            .is_ok()
            {
                borrowed_declarations.extend(chain);
            }
        }
        let mut result = RequiredEvidence::default();
        let mut reasons = BTreeSet::new();
        // Exit markers distinguish a shared dependency (a diamond) from a
        // cycle. Each branch also uses the existing bounded path guard.
        let mut pending = vec![(subject, ChainGuard::default(), "machine", false)];
        let mut finished = BTreeSet::new();
        while let Some((index, mut guard, kind, exiting)) = pending.pop() {
            if exiting {
                finished.insert(index);
                continue;
            }
            let game = self.graph.game(index);
            if let Err(fault) = guard.visit(index) {
                reasons.insert(format!(
                    "required dependency closure at {} is unresolved: {fault:?}",
                    game.name
                ));
                continue;
            }
            if finished.contains(&index) {
                continue;
            }
            pending.push((index, guard.clone(), kind, true));
            if self.graph.resolve_set(&game.name) != SetRef::Unique(index) {
                reasons.insert(format!("catalogue identity for {} is ambiguous", game.name));
            }
            if game.unsupported_structure {
                reasons.insert(format!(
                    "dependency information for {} is unsupported",
                    game.name
                ));
            }

            let active_bios = evidence
                .filter(|e| e.verified_complete())
                .and_then(|e| e.bios_selections.get(&game.name));
            let active_rom = |rom: &crate::dat::model::DatRomEntry| {
                active_bios.is_none_or(|selected| rom.bios.as_ref().is_none_or(|b| b == selected))
            };
            // NO_DUMP and unsupported declarations remain material uncertainty.
            // Only optional/non-file material can be omitted from this gate.
            let material = declared_roms(index, game)
                .filter(|(_, rom)| active_rom(rom))
                .any(|(_, rom)| {
                    !matches!(
                        classify_rom_member(rom),
                        MemberClass::OptionalPhysical | MemberClass::NonFile
                    )
                })
                || declared_disks(index, game).any(|(_, disk)| {
                    !matches!(
                        classify_disk_member(disk),
                        MemberClass::OptionalPhysical | MemberClass::NonFile
                    )
                });
            let uncovered = declared_roms(index, game)
                .filter(|(_, rom)| active_rom(rom))
                .any(|(_, rom)| {
                    !matches!(
                        classify_rom_member(rom),
                        MemberClass::OptionalPhysical | MemberClass::NonFile
                    ) && !borrowed_declarations.contains(&(index, rom.name.as_str()))
                })
                || declared_disks(index, game).any(|(_, disk)| {
                    !matches!(
                        classify_disk_member(disk),
                        MemberClass::OptionalPhysical | MemberClass::NonFile
                    )
                });
            if index != subject && material {
                result.support_sets.insert(game.name.clone(), kind);
                if !covered.contains(game.name.as_str())
                    && uncovered
                    && self.health.get(game.name.as_str()) != Some(&MameSetHealth::Good)
                {
                    reasons.insert(format!(
                        "required support set {} has incomplete or unknown collection evidence",
                        game.name
                    ));
                }
            }

            // Hierarchy and ROM-source declarations must each be well formed.
            // cloneof alone does not demand the parent's storage (Stage 2d's
            // ParentSet semantics), so it is validated separately from the
            // required storage closure below.
            self.validate_chain(index, |game| &game.clone_of, "parent", &mut reasons);
            self.validate_chain(index, |game| &game.rom_of, "ROM source", &mut reasons);

            let borrowed = declared_roms(index, game)
                .filter(|(_, rom)| active_rom(rom))
                .any(|(_, rom)| classify_rom_member(rom) == MemberClass::Borrowed)
                || declared_disks(index, game)
                    .any(|(_, disk)| classify_disk_member(disk) == MemberClass::Borrowed);
            let provider = match declared_name(&game.rom_of) {
                DeclaredName::Absent if borrowed => Some(&game.clone_of),
                DeclaredName::Absent => None,
                _ => Some(&game.rom_of),
            };
            if let Some(provider) = provider {
                match self.resolve(provider, "ROM source", &mut reasons) {
                    Some(next) => pending.push((next, guard.clone(), "ROM source/parent", false)),
                    None if matches!(declared_name(provider), DeclaredName::Absent) => {
                        reasons.insert(format!(
                            "required borrowed member in {} has no provider",
                            game.name
                        ));
                    }
                    None => {}
                }
            }
            // BIOS roots can also be reached through cloneof-only ancestry.
            // biosset names are local alternatives, never external set names.
            if let Some(bios) = self.bios_root(index, &mut reasons) {
                pending.push((bios, guard.clone(), "BIOS", false));
            }
            // device_ref is required; optional slotoption devices are not
            // represented by these edges and must never be swept into them.
            for device in &game.device_refs {
                let Some(next) = self.resolve(&device.name, "device", &mut reasons) else {
                    if matches!(declared_name(&device.name), DeclaredName::Absent) {
                        reasons.insert(format!(
                            "required device reference in {} has no name",
                            game.name
                        ));
                    }
                    continue;
                };
                let target = self.graph.game(next);
                if parse_flag(&target.is_device) != Flag::Yes
                    || matches!(parse_flag(&target.runnable), Flag::Yes | Flag::Malformed)
                {
                    reasons.insert(format!(
                        "required device {} has unconfirmed or contradictory device metadata",
                        target.name
                    ));
                }
                pending.push((next, guard.clone(), "device", false));
            }
        }
        result.reasons = reasons.into_iter().collect();
        result
    }

    fn resolve(
        &self,
        reference: &Option<String>,
        kind: &str,
        reasons: &mut BTreeSet<String>,
    ) -> Option<usize> {
        match declared_name(reference) {
            DeclaredName::Absent => None,
            DeclaredName::Malformed => {
                reasons.insert(format!("required {kind} reference is empty or malformed"));
                None
            }
            DeclaredName::Named(name) => match self.graph.resolve_set(name) {
                SetRef::Unique(index) => Some(index),
                fault => {
                    reasons.insert(format!(
                        "required {kind} reference {name} is unresolved: {fault:?}"
                    ));
                    None
                }
            },
        }
    }

    fn validate_chain(
        &self,
        start: usize,
        field: fn(&DatGameEntry) -> &Option<String>,
        kind: &str,
        reasons: &mut BTreeSet<String>,
    ) {
        let mut guard = ChainGuard::starting_at(start);
        let mut current = start;
        while let Some(next) = self.resolve(field(self.graph.game(current)), kind, reasons) {
            if let Err(fault) = guard.visit(next) {
                reasons.insert(format!("required {kind} chain is unresolved: {fault:?}"));
                return;
            }
            current = next;
        }
    }

    fn bios_root(&self, start: usize, reasons: &mut BTreeSet<String>) -> Option<usize> {
        // ROM-source ancestry is traversed by assess. Along lineage alone,
        // inspect only cloneof and BIOS identity: an ancestor's unrelated
        // romof/device failures must not undo #159's lineage separation.
        if !matches!(
            declared_name(&self.graph.game(start).rom_of),
            DeclaredName::Absent
        ) {
            return None;
        }
        let mut current = start;
        let mut guard = ChainGuard::starting_at(start);
        loop {
            let game = self.graph.game(current);
            let next = self.resolve(&game.clone_of, "BIOS ancestry", reasons)?;
            if let Err(fault) = guard.visit(next) {
                reasons.insert(format!("required BIOS ancestry is unresolved: {fault:?}"));
                return None;
            }
            match parse_flag(&self.graph.game(next).is_bios) {
                Flag::Yes => return Some(next),
                Flag::Malformed => {
                    reasons.insert("required BIOS ancestry has malformed BIOS metadata".into());
                    return None;
                }
                Flag::No | Flag::Absent => current = next,
            }
        }
    }
}
