//! Projects the existing typed dependency resolver into collection storage facts.
//! No graph traversal, filesystem access, emulator execution or repair policy.
use super::*;
use crate::arcade_mame_compatibility::{
    MameCollectionLayout, MameExpectationIndex, MameSetExpectation,
};
use crate::dat::model::{ChecksumAlgorithm, DatChecksum};
use crate::dat::parsers::mame_listxml::MAME_LISTXML_PARSER_SCHEMA_VERSION;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameMemberEvidence {
    Verified,
    WeakHash,
    PartialHash,
    NameOrSizeOnly,
    Contradictory,
    Missing,
    Uninspectable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameRequirementEvidence {
    pub declaring_set: String,
    pub member: String,
    pub disk: bool,
    pub content: MameMemberEvidence,
    pub preservation: MamePreservationStatus,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameRunnability {
    /// Storage, dump status and version equality do not establish execution.
    #[default]
    NotAssessed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameStorageEvidence {
    /// False for old serialized reports or incomplete inventories.
    /// Metadata uncertainty is recorded in unresolved_dependencies.
    pub inspection_complete: bool,
    /// One evaluated alternative, not a unique inference of physical packing.
    pub evaluated_layout: Option<MameCollectionStyle>,
    /// Default BIOS selections made by the existing metadata resolver.
    pub bios_selections: BTreeMap<String, String>,
    pub unresolved_dependencies: Vec<String>,
    pub requirements: Vec<MameRequirementEvidence>,
    pub runnability: MameRunnability,
}

impl MameStorageEvidence {
    pub fn verified_complete(&self) -> bool {
        self.inspection_complete
            && self.unresolved_dependencies.is_empty()
            && !self.requirements.is_empty()
            && self.requirements.iter().all(|r| {
                r.content == MameMemberEvidence::Verified
                    && r.preservation == MamePreservationStatus::VerifiedDumpExpected
            })
    }
    fn health(&self) -> MameSetHealth {
        if self.requirements.iter().any(|r| {
            r.preservation != MamePreservationStatus::NoVerifiedDumpExists
                && matches!(
                    r.content,
                    MameMemberEvidence::Missing | MameMemberEvidence::Contradictory
                )
        }) {
            MameSetHealth::Bad
        } else if self.verified_complete() {
            MameSetHealth::Good
        } else {
            MameSetHealth::Unknown
        }
    }
}

/// A fully verified CHD combined raw+metadata digest, including any required
/// parent chain, supplied by a CHD-aware inspector.
/// Raw-data SHA-1, compressed-file SHA-1 and an unverified header are NOT this fact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameVerifiedChd {
    pub set_name: String,
    /// Catalogue disk name (without adding a .chd extension).
    pub name: String,
    pub verified_overall_sha1: String,
}

fn preservation(status: Option<&str>) -> MamePreservationStatus {
    let Some(status) = status else {
        return MamePreservationStatus::VerifiedDumpExpected;
    };
    match status.to_ascii_lowercase().as_str() {
        "nodump" | "no_dump" => MamePreservationStatus::NoVerifiedDumpExists,
        "baddump" | "bad_dump" => MamePreservationStatus::NeedsRedump,
        "good" => MamePreservationStatus::VerifiedDumpExpected,
        _ => MamePreservationStatus::Unknown,
    }
}

pub(super) fn member_evidence(
    size: Option<u64>,
    hashes: &[DatChecksum],
    member: &MameObservedMember,
) -> MameMemberEvidence {
    let mut strong = false;
    let mut weak = false;
    let mut unavailable = matches!((size, member.size_bytes), (Some(_), None));
    if matches!((size,member.size_bytes),(Some(a),Some(b)) if a!=b) {
        return MameMemberEvidence::Contradictory;
    }
    for hash in hashes {
        let observed = match hash.algorithm {
            ChecksumAlgorithm::Sha1 => member.sha1.as_deref(),
            ChecksumAlgorithm::Md5 => member.md5.as_deref(),
            ChecksumAlgorithm::Crc32 => member.crc32.as_deref(),
            ChecksumAlgorithm::Sha256 => None,
        };
        let Some(expected) = DatChecksum::parse(hash.algorithm, &hash.value) else {
            unavailable = true;
            continue;
        };
        let Some(raw) = observed else {
            unavailable = true;
            continue;
        };
        let Some(actual) = DatChecksum::parse(hash.algorithm, raw) else {
            unavailable = true;
            continue;
        };
        if expected != actual {
            return MameMemberEvidence::Contradictory;
        }
        if hash.algorithm == ChecksumAlgorithm::Crc32 {
            weak = true;
        } else {
            strong = true;
        }
    }
    if unavailable && (strong || weak) {
        MameMemberEvidence::PartialHash
    } else if unavailable {
        MameMemberEvidence::Uninspectable
    } else if strong {
        MameMemberEvidence::Verified
    } else if weak {
        MameMemberEvidence::WeakHash
    } else {
        MameMemberEvidence::NameOrSizeOnly
    }
}

fn absent(inventory: &MameCollectionInventory) -> MameMemberEvidence {
    if inventory.inspected_completely {
        MameMemberEvidence::Missing
    } else {
        MameMemberEvidence::Uninspectable
    }
}

pub(super) type ObservedIndex<'a> = BTreeMap<&'a str, Vec<&'a MameObservedSet>>;

pub(super) fn index_inventory(inventory: &MameCollectionInventory) -> ObservedIndex<'_> {
    let mut sets = BTreeMap::<&str, Vec<&MameObservedSet>>::new();
    for set in &inventory.sets {
        sets.entry(&set.name).or_default().push(set);
    }
    sets
}

pub(super) fn evaluate(
    index: &MameExpectationIndex<'_>,
    game: &DatGameEntry,
    inventory: &MameCollectionInventory,
    chds: &[MameVerifiedChd],
    sets: &ObservedIndex<'_>,
) -> MameStorageEvidence {
    // Try the existing supported layouts, never an invented union of requirements.
    let mut alternatives: Vec<_> = [
        MameCollectionLayout::Split,
        MameCollectionLayout::Merged,
        MameCollectionLayout::NonMerged,
    ]
    .into_iter()
    .filter_map(|layout| index.expectation_for_layout(&game.name, layout))
    .map(|e| evaluate_expectation(&e, inventory, chds, sets))
    .collect();
    alternatives.sort_by_key(|e| {
        (
            match e.health() {
                MameSetHealth::Good => 0,
                MameSetHealth::Unknown => 1,
                MameSetHealth::Bad => 2,
            },
            std::cmp::Reverse(
                e.requirements
                    .iter()
                    .filter(|r| r.content == MameMemberEvidence::Verified)
                    .count(),
            ),
        )
    });
    alternatives.into_iter().next().unwrap_or_default()
}

/// A same-named declaration is not authority for another declaration. Only
/// equivalent resolved identities, statuses and bindings can share a slot.
fn conflicting_roms(e: &MameSetExpectation) -> BTreeSet<(String, String)> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for rom in &e.roms {
        groups
            .entry((&rom.set_name, &rom.name))
            .or_default()
            .push(rom);
    }
    groups
        .into_iter()
        .filter_map(|((set, name), declarations)| {
            let first = declarations[0];
            let status = preservation(first.status.as_deref());
            let equivalent = status != MamePreservationStatus::Unknown
                && declarations.iter().all(|r| {
                    r.size_bytes == first.size_bytes
                        && r.checksums == first.checksums
                        && r.merge == first.merge
                        && preservation(r.status.as_deref()) == status
                });
            let bindings: Vec<_> = e
                .dependencies
                .resolution
                .roms
                .iter()
                .filter(|r| r.declaring_set == *set && r.member_name == *name)
                .collect();
            let same_binding = bindings.first().is_none_or(|first| {
                bindings.iter().all(|r| {
                    r.owner == first.owner
                        && r.locations == first.locations
                        && r.supporting_kind == first.supporting_kind
                })
            });
            (!equivalent || !same_binding).then(|| (set.clone(), name.clone()))
        })
        .collect()
}

fn evaluate_expectation(
    e: &MameSetExpectation,
    inventory: &MameCollectionInventory,
    chds: &[MameVerifiedChd],
    sets: &ObservedIndex<'_>,
) -> MameStorageEvidence {
    let resolution = &e.dependencies.resolution;
    let mut facts = MameStorageEvidence {
        inspection_complete: inventory.inspected_completely,
        evaluated_layout: Some(match resolution.layout {
            MameCollectionLayout::Split => MameCollectionStyle::Split,
            MameCollectionLayout::Merged => MameCollectionStyle::Merged,
            MameCollectionLayout::NonMerged => MameCollectionStyle::NonMerged,
            MameCollectionLayout::RuntimeSearch => MameCollectionStyle::Unknown,
        }),
        bios_selections: resolution.bios_selections.clone(),
        ..Default::default()
    };
    if resolution.parser_schema_version != Some(MAME_LISTXML_PARSER_SCHEMA_VERSION) {
        facts
            .unresolved_dependencies
            .push("MAME catalogue parser provenance unavailable; reimport required".into());
    }
    facts.unresolved_dependencies.extend(
        resolution
            .stale_metadata_sets
            .iter()
            .map(|s| format!("Stale dependency metadata: {s}")),
    );
    facts.unresolved_dependencies.extend(
        resolution
            .references
            .iter()
            .filter(|r| !r.requirement.outcome.permits_complete())
            .map(|r| {
                format!(
                    "{}: {:?} {:?}: {:?}",
                    r.declaring_set,
                    r.requirement.kind,
                    r.requirement.target,
                    r.requirement.outcome
                )
            }),
    );
    let conflicts = conflicting_roms(e);
    facts.unresolved_dependencies.extend(
        conflicts.iter().map(|(set, name)| {
            format!("Conflicting or unsupported ROM declarations: {set}/{name}")
        }),
    );
    for rom in &e.roms {
        let locations = resolution
            .roms
            .iter()
            .find(|r| r.declaring_set == rom.set_name && r.member_name == rom.name);
        let mut states = Vec::new();
        if let Some(locations) = locations {
            for location in &locations.locations {
                if let Some(containers) = sets.get(location.set_name.as_str()) {
                    if containers.len() != 1 {
                        states.push(MameMemberEvidence::Uninspectable);
                        continue;
                    }
                    let members: Vec<_> = containers[0]
                        .members
                        .iter()
                        .filter(|m| m.name == location.member_name)
                        .collect();
                    if members.len() > 1 {
                        states.push(MameMemberEvidence::Uninspectable);
                        continue;
                    }
                    if let Some(member) = members.first() {
                        states.push(member_evidence(rom.size_bytes, &rom.checksums, member));
                    }
                }
            }
        }
        // Alternative locations can contain different members with the same
        // name. A strong match proves this requirement, not all other copies.
        let content = if conflicts.contains(&(rom.set_name.clone(), rom.name.clone())) {
            MameMemberEvidence::Uninspectable
        } else if states.contains(&MameMemberEvidence::Uninspectable) {
            MameMemberEvidence::Uninspectable
        } else if states.contains(&MameMemberEvidence::Verified) {
            MameMemberEvidence::Verified
        } else if states.contains(&MameMemberEvidence::Contradictory) {
            MameMemberEvidence::Contradictory
        } else if states.contains(&MameMemberEvidence::PartialHash) {
            MameMemberEvidence::PartialHash
        } else if states.contains(&MameMemberEvidence::WeakHash) {
            MameMemberEvidence::WeakHash
        } else if states.contains(&MameMemberEvidence::NameOrSizeOnly) {
            MameMemberEvidence::NameOrSizeOnly
        } else {
            absent(inventory)
        };
        facts.requirements.push(MameRequirementEvidence {
            declaring_set: rom.set_name.clone(),
            member: rom.name.clone(),
            disk: false,
            content,
            preservation: preservation(rom.status.as_deref()),
        });
    }
    for disk in &e.disks {
        let name = disk.name.as_deref().unwrap_or("(unnamed CHD)");
        let verified: Vec<_> = chds
            .iter()
            .filter(|d| {
                disk.search_locations.iter().any(|location| {
                    d.set_name == location.set_name && d.name == location.member_name
                })
            })
            .collect();
        let ambiguous = disk.search_locations.iter().any(|location| {
            sets.get(location.set_name.as_str())
                .is_some_and(|ss| ss.len() > 1)
        });
        let present: Vec<_> = disk
            .search_locations
            .iter()
            .filter(|location| {
                sets.get(location.set_name.as_str()).is_some_and(|ss| {
                    ss.iter().any(|s| {
                        s.members.iter().any(|m| {
                            m.name == location.member_name
                                || m.name == format!("{}.chd", location.member_name)
                        })
                    })
                })
            })
            .collect();
        let unverified_alternative = present.iter().any(|location| {
            !verified
                .iter()
                .any(|d| d.set_name == location.set_name && d.name == location.member_name)
        });
        let content = if verified.len() > 1 || ambiguous {
            MameMemberEvidence::Uninspectable
        } else if let Some(d) = verified.first() {
            match (
                disk.sha1
                    .as_deref()
                    .and_then(|v| crate::dat::index::parse_disk_sha1(v)),
                crate::dat::index::parse_disk_sha1(&d.verified_overall_sha1),
            ) {
                (Some(a), Some(b)) if a == b => MameMemberEvidence::Verified,
                // A wrong copy at one search location does not disprove a
                // different copy whose CHD content has not been verified.
                (Some(_), Some(_)) if !unverified_alternative => MameMemberEvidence::Contradictory,
                _ => MameMemberEvidence::Uninspectable,
            }
        } else if !present.is_empty() || disk.merge.is_some() {
            // Merged disk ownership/aliases remain unsupported. Neither the
            // filename nor an unresolved storage binding proves absence.
            MameMemberEvidence::Uninspectable
        } else {
            absent(inventory)
        };
        facts.requirements.push(MameRequirementEvidence {
            declaring_set: disk.set_name.clone(),
            member: name.into(),
            disk: true,
            content,
            preservation: preservation(disk.status.as_deref()),
        });
    }
    facts.unresolved_dependencies.sort();
    facts.unresolved_dependencies.dedup();
    facts.requirements.sort_by(|a, b| {
        (
            &a.declaring_set,
            &a.member,
            a.disk,
            a.preservation as u8,
            a.content as u8,
        )
            .cmp(&(
                &b.declaring_set,
                &b.member,
                b.disk,
                b.preservation as u8,
                b.content as u8,
            ))
    });
    facts.requirements.dedup();
    facts
}

pub(super) fn result(game: &DatGameEntry, evidence: MameStorageEvidence) -> MameSetResult {
    MameSetResult {
        name: game.name.clone(),
        health: evidence.health(),
        expected_members: evidence.requirements.len(),
        matched_members: evidence
            .requirements
            .iter()
            .filter(|r| r.content == MameMemberEvidence::Verified)
            .count(),
        missing_members: evidence
            .requirements
            .iter()
            .filter(|r| {
                r.content == MameMemberEvidence::Missing
                    && r.preservation != MamePreservationStatus::NoVerifiedDumpExists
            })
            .map(|r| format!("{}/{}", r.declaring_set, r.member))
            .collect(),
        parent: game.clone_of.clone(),
        evidence,
    }
}
