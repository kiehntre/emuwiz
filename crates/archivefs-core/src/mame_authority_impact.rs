//! Read-only impact projection for a refreshed MAME authority.
//!
//! This module compares already-parsed MAME `ParsedDat` snapshots. It does
//! not import, scan, hash, mutate, or recompute compatibility. The existing
//! DAT dependency resolver and MAME compatibility projection remain the
//! authorities for those questions; this module only identifies sets that
//! should be reconsidered and preserves why.

use crate::dat::dependency::DependencyKind;
use crate::dat::model::{DatGameEntry, ParsedDat};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameAuthorityIdentity {
    pub artifact_sha256: Option<String>,
    pub version: Option<String>,
    pub provenance: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MameImpactEvidenceState {
    Known,
    EvidenceUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MameAuthorityImpactReason {
    AuthorityIdentityChanged,
    ExpectedMemberDefinitionChanged,
    DependencyChanged(DependencyKind),
    EvidenceUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameAffectedSet {
    pub set_name: String,
    pub reasons: Vec<MameAuthorityImpactReason>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MameAuthorityImpact {
    pub affected_sets: Vec<MameAffectedSet>,
    pub evidence_state: MameImpactEvidenceState,
    pub authority_before: Option<MameAuthorityIdentity>,
    pub authority_after: Option<MameAuthorityIdentity>,
}

#[derive(Clone, Copy)]
pub struct MameAuthoritySnapshot<'a> {
    pub parsed: Option<&'a ParsedDat>,
    pub identity: Option<&'a MameAuthorityIdentity>,
    /// False means the caller does not have a complete, validated authority
    /// snapshot. It is never interpreted as proof that anything is missing.
    pub complete: bool,
}

/// Projects which authority-defined MAME sets may need re-evaluation.
///
/// Both snapshots must already be parsed and validated by the caller. No
/// filesystem or compatibility work occurs here. A missing/incomplete
/// snapshot produces `EvidenceUnavailable` reasons rather than missing or
/// incompatible conclusions.
pub fn project_mame_authority_impact(
    before: MameAuthoritySnapshot<'_>,
    after: MameAuthoritySnapshot<'_>,
) -> MameAuthorityImpact {
    let authority_before = before.identity.cloned();
    let authority_after = after.identity.cloned();
    let mut output = MameAuthorityImpact {
        affected_sets: Vec::new(),
        evidence_state: MameImpactEvidenceState::Known,
        authority_before,
        authority_after,
    };

    let Some(old) = before.parsed else {
        return unavailable_impact(&mut output, after.parsed, before.complete && after.complete);
    };
    let Some(new) = after.parsed else {
        return unavailable_impact(&mut output, Some(old), before.complete && after.complete);
    };
    if !before.complete || !after.complete {
        return unavailable_impact(&mut output, Some(new), false);
    }

    let old_by_name = games_by_name(&old.games);
    let new_by_name = games_by_name(&new.games);
    let all_names: BTreeSet<String> = old_by_name
        .keys()
        .chain(new_by_name.keys())
        .cloned()
        .collect();
    let identity_changed = before.identity != after.identity;
    let mut direct: BTreeMap<String, BTreeSet<MameAuthorityImpactReason>> = BTreeMap::new();

    for name in &all_names {
        let old_game = old_by_name.get(name).copied();
        let new_game = new_by_name.get(name).copied();
        let Some(reasons) = direct_reasons(old_game, new_game) else {
            continue;
        };
        let entry = direct.entry(name.clone()).or_default();
        entry.extend(reasons);
        if identity_changed {
            entry.insert(MameAuthorityImpactReason::AuthorityIdentityChanged);
        }
    }

    let mut affected = direct.clone();
    let mut queue: VecDeque<String> = direct.keys().cloned().collect();
    let mut seen = direct.keys().cloned().collect::<BTreeSet<_>>();
    while let Some(changed_name) = queue.pop_front() {
        let changed_is_bios = old_by_name
            .get(&changed_name)
            .or_else(|| new_by_name.get(&changed_name))
            .is_some_and(|game| is_yes(game.is_bios.as_deref()));
        let changed_is_device = old_by_name
            .get(&changed_name)
            .or_else(|| new_by_name.get(&changed_name))
            .is_some_and(|game| is_yes(game.is_device.as_deref()));
        let changed_has_chd = direct.get(&changed_name).is_some_and(|reasons| {
            reasons.contains(&MameAuthorityImpactReason::DependencyChanged(
                DependencyKind::ChdParent,
            ))
        });

        for (dependent_name, dependent) in old_by_name.iter().chain(new_by_name.iter()) {
            let mut propagated = BTreeSet::new();
            if references_parent(dependent, &changed_name) {
                propagated.insert(if changed_is_bios {
                    MameAuthorityImpactReason::DependencyChanged(DependencyKind::Bios)
                } else if changed_has_chd {
                    MameAuthorityImpactReason::DependencyChanged(DependencyKind::ChdParent)
                } else {
                    MameAuthorityImpactReason::DependencyChanged(DependencyKind::ParentSet)
                });
            }
            if references_device(dependent, &changed_name) && changed_is_device {
                propagated.insert(MameAuthorityImpactReason::DependencyChanged(
                    DependencyKind::Device,
                ));
            }
            if propagated.is_empty() {
                continue;
            }
            let entry = affected.entry((*dependent_name).clone()).or_default();
            let was_new = entry.is_empty();
            entry.extend(propagated);
            if was_new && seen.insert((*dependent_name).clone()) {
                queue.push_back((*dependent_name).clone());
            }
        }
    }

    output.affected_sets = affected
        .into_iter()
        .map(|(set_name, reasons)| MameAffectedSet {
            set_name,
            reasons: reasons.into_iter().collect(),
        })
        .collect();
    output
}

fn unavailable_impact(
    output: &mut MameAuthorityImpact,
    known_snapshot: Option<&ParsedDat>,
    complete: bool,
) -> MameAuthorityImpact {
    if complete {
        return output.clone();
    }
    output.evidence_state = MameImpactEvidenceState::EvidenceUnavailable;
    let mut names = BTreeSet::new();
    if let Some(parsed) = known_snapshot {
        names.extend(parsed.games.iter().map(|game| game.name.clone()));
    }
    output.affected_sets = names
        .into_iter()
        .map(|set_name| MameAffectedSet {
            set_name,
            reasons: vec![MameAuthorityImpactReason::EvidenceUnavailable],
        })
        .collect();
    output.clone()
}

fn games_by_name(games: &[DatGameEntry]) -> BTreeMap<String, &DatGameEntry> {
    let mut result = BTreeMap::new();
    for game in games {
        result.entry(game.name.clone()).or_insert(game);
    }
    result
}

fn direct_reasons(
    old: Option<&DatGameEntry>,
    new: Option<&DatGameEntry>,
) -> Option<BTreeSet<MameAuthorityImpactReason>> {
    let (Some(old), Some(new)) = (old, new) else {
        return Some(BTreeSet::from([
            MameAuthorityImpactReason::ExpectedMemberDefinitionChanged,
        ]));
    };
    let mut reasons = BTreeSet::new();
    if old.roms != new.roms {
        reasons.insert(MameAuthorityImpactReason::ExpectedMemberDefinitionChanged);
    }
    if old.disks != new.disks {
        reasons.insert(MameAuthorityImpactReason::ExpectedMemberDefinitionChanged);
        reasons.insert(MameAuthorityImpactReason::DependencyChanged(
            DependencyKind::ChdParent,
        ));
    }
    if old.clone_of != new.clone_of || old.rom_of != new.rom_of {
        reasons.insert(MameAuthorityImpactReason::DependencyChanged(
            DependencyKind::ParentSet,
        ));
    }
    if old.device_refs != new.device_refs {
        reasons.insert(MameAuthorityImpactReason::DependencyChanged(
            DependencyKind::Device,
        ));
    }
    if old.is_bios != new.is_bios
        || old.bios_sets != new.bios_sets
        || old.roms.iter().map(|rom| &rom.bios).collect::<Vec<_>>()
            != new.roms.iter().map(|rom| &rom.bios).collect::<Vec<_>>()
    {
        reasons.insert(MameAuthorityImpactReason::DependencyChanged(
            DependencyKind::Bios,
        ));
    }
    (!reasons.is_empty()).then_some(reasons)
}

fn references_parent(game: &DatGameEntry, target: &str) -> bool {
    game.clone_of.as_deref() == Some(target) || game.rom_of.as_deref() == Some(target)
}

fn references_device(game: &DatGameEntry, target: &str) -> bool {
    game.device_refs
        .iter()
        .any(|reference| reference.name.as_deref() == Some(target))
}

fn is_yes(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case("yes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::{
        DatDeviceRefEntry, DatDiskEntry, DatEcosystem, DatFormat, DatPackingPolicy, DatRomEntry,
        DatSource,
    };

    fn identity(hash: &str) -> MameAuthorityIdentity {
        MameAuthorityIdentity {
            artifact_sha256: Some(hash.into()),
            version: Some("test".into()),
            provenance: "synthetic imported MAME authority".into(),
        }
    }

    fn parsed(games: Vec<DatGameEntry>) -> ParsedDat {
        ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::MAMEArcade,
                file_path: "/tmp/synthetic-mame.xml".into(),
                name: Some("synthetic".into()),
                description: None,
                version: Some("test".into()),
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: games.len(),
                rom_count: games.iter().map(DatGameEntry::rom_count).sum(),
                parse_warnings: Vec::new(),
                packing_policy: DatPackingPolicy::Standard,
            },
            games,
        }
    }

    fn game(name: &str) -> DatGameEntry {
        DatGameEntry {
            name: name.into(),
            runnable: Some("yes".into()),
            ..Default::default()
        }
    }

    fn snapshot<'a>(
        parsed: &'a ParsedDat,
        identity: &'a MameAuthorityIdentity,
    ) -> MameAuthoritySnapshot<'a> {
        MameAuthoritySnapshot {
            parsed: Some(parsed),
            identity: Some(identity),
            complete: true,
        }
    }

    fn reason_set(impact: &MameAuthorityImpact, name: &str) -> BTreeSet<MameAuthorityImpactReason> {
        impact
            .affected_sets
            .iter()
            .find(|set| set.set_name == name)
            .map(|set| set.reasons.iter().copied().collect())
            .unwrap_or_default()
    }

    #[test]
    fn identical_authority_is_a_no_op_even_with_a_new_delivery_identity() {
        let old = parsed(vec![game("solo")]);
        let new = old.clone();
        let impact = project_mame_authority_impact(
            snapshot(&old, &identity("old")),
            snapshot(&new, &identity("new")),
        );
        assert_eq!(impact.evidence_state, MameImpactEvidenceState::Known);
        assert!(impact.affected_sets.is_empty());
    }

    #[test]
    fn changed_rom_definition_affects_only_that_set() {
        let old = parsed(vec![game("a"), game("unrelated")]);
        let mut changed = game("a");
        changed.roms.push(DatRomEntry {
            name: "a.bin".into(),
            size_bytes: Some(1),
            sha1: Some("a".repeat(40)),
            ..Default::default()
        });
        let new = parsed(vec![changed, game("unrelated")]);
        let impact = project_mame_authority_impact(
            snapshot(&old, &identity("same")),
            snapshot(&new, &identity("new")),
        );
        assert_eq!(impact.affected_sets.len(), 1);
        assert!(
            reason_set(&impact, "a")
                .contains(&MameAuthorityImpactReason::ExpectedMemberDefinitionChanged)
        );
        assert!(reason_set(&impact, "unrelated").is_empty());
    }

    #[test]
    fn bios_device_and_parent_changes_propagate_to_dependents() {
        let mut old_bios = game("bios");
        old_bios.is_bios = Some("yes".into());
        let mut old_device = game("device");
        old_device.is_device = Some("yes".into());
        let mut old_parent = game("parent");
        old_parent.roms.push(DatRomEntry::default());
        let mut clone = game("clone");
        clone.clone_of = Some("parent".into());
        clone.rom_of = Some("bios".into());
        clone.device_refs.push(DatDeviceRefEntry {
            name: Some("device".into()),
        });
        let old = parsed(vec![old_bios, old_device, old_parent, clone.clone()]);

        let mut new_bios = game("bios");
        new_bios.is_bios = Some("yes".into());
        new_bios.roms.push(DatRomEntry::default());
        let mut new_device = game("device");
        new_device.is_device = Some("yes".into());
        new_device.roms.push(DatRomEntry::default());
        let mut new_parent = game("parent");
        new_parent.roms.push(DatRomEntry {
            name: "new".into(),
            ..Default::default()
        });
        let new = parsed(vec![new_bios, new_device, new_parent, clone]);
        let impact = project_mame_authority_impact(
            snapshot(&old, &identity("old")),
            snapshot(&new, &identity("new")),
        );
        let reasons = reason_set(&impact, "clone");
        assert!(
            reasons.contains(&MameAuthorityImpactReason::DependencyChanged(
                DependencyKind::Bios,
            ))
        );
        assert!(
            reasons.contains(&MameAuthorityImpactReason::DependencyChanged(
                DependencyKind::Device,
            ))
        );
        assert!(
            reasons.contains(&MameAuthorityImpactReason::DependencyChanged(
                DependencyKind::ParentSet,
            ))
        );
    }

    #[test]
    fn changed_chd_definition_affects_the_set_and_parent_dependents() {
        let mut old = game("parent");
        old.disks.push(DatDiskEntry {
            name: Some("disc".into()),
            ..Default::default()
        });
        let mut child = game("child");
        child.rom_of = Some("parent".into());
        let before = parsed(vec![old, child]);
        let mut changed = game("parent");
        changed.disks.push(DatDiskEntry {
            name: Some("new-disc".into()),
            ..Default::default()
        });
        let after = parsed(vec![changed, game("child")]);
        let mut after = after;
        after.games[1].rom_of = Some("parent".into());
        let impact = project_mame_authority_impact(
            snapshot(&before, &identity("same")),
            snapshot(&after, &identity("new")),
        );
        assert!(reason_set(&impact, "parent").contains(
            &MameAuthorityImpactReason::DependencyChanged(DependencyKind::ChdParent),
        ));
        assert!(reason_set(&impact, "child").contains(
            &MameAuthorityImpactReason::DependencyChanged(DependencyKind::ChdParent),
        ));
    }

    #[test]
    fn unavailable_authority_is_typed_and_never_claims_missing_content() {
        let new = parsed(vec![game("known")]);
        let impact = project_mame_authority_impact(
            MameAuthoritySnapshot {
                parsed: None,
                identity: None,
                complete: false,
            },
            snapshot(&new, &identity("new")),
        );
        assert_eq!(
            impact.evidence_state,
            MameImpactEvidenceState::EvidenceUnavailable
        );
        assert_eq!(impact.affected_sets[0].set_name, "known");
        assert_eq!(
            impact.affected_sets[0].reasons,
            vec![MameAuthorityImpactReason::EvidenceUnavailable]
        );
    }

    #[test]
    fn ordering_and_reason_aggregation_are_deterministic() {
        let mut old = game("z");
        old.rom_of = Some("a".into());
        let before = parsed(vec![old, game("a")]);
        let mut changed_z = game("z");
        changed_z.rom_of = Some("a".into());
        changed_z.device_refs.push(DatDeviceRefEntry {
            name: Some("a".into()),
        });
        let mut changed_a = game("a");
        changed_a.is_device = Some("yes".into());
        changed_a.roms.push(DatRomEntry::default());
        let after = parsed(vec![changed_z, changed_a]);
        let first = project_mame_authority_impact(
            snapshot(&before, &identity("old")),
            snapshot(&after, &identity("new")),
        );
        let second = project_mame_authority_impact(
            snapshot(&before, &identity("old")),
            snapshot(&after, &identity("new")),
        );
        assert_eq!(first, second);
        assert_eq!(
            first
                .affected_sets
                .iter()
                .map(|set| set.set_name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        assert_eq!(
            first.authority_before.as_ref().unwrap().provenance,
            "synthetic imported MAME authority"
        );
    }
}
