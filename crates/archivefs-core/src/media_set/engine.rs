use super::{model::*, naming::normalize_title};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SET_RECORDS: usize = 4096;
const MAX_COUNT: u16 = 4096;
type GroupKey = (
    Option<String>,
    Option<MediaFamily>,
    IdentityKey,
    ReleaseVariant,
);
pub struct MediaIndex {
    groups: BTreeMap<GroupKey, Vec<MediaRepresentation>>,
    separated: BTreeSet<(Option<String>, Option<MediaFamily>, IdentityKey)>,
    stats: TopologyStats,
}
pub(super) fn issue(
    kind: ConflictKind,
    detail: impl Into<String>,
    blocking: bool,
) -> MediaSetConflict {
    MediaSetConflict {
        kind,
        detail: detail.into(),
        blocking,
    }
}
fn choose<T: Clone + Eq>(
    evidence: &[MediaEvidence],
    get: impl Fn(&MediaEvidence) -> Option<T>,
    kind: ConflictKind,
    conflicts: &mut Vec<MediaSetConflict>,
) -> Option<(T, Provenance)> {
    let mut selected: Option<(T, Provenance)> = None;
    for e in evidence {
        if let Some(value) = get(e) {
            if let Some((prior, source)) = &selected {
                if *prior != value {
                    conflicts.push(issue(
                        kind,
                        format!(
                            "{} disagrees with {}; the stronger claim is retained",
                            e.provenance.source, source.source
                        ),
                        true,
                    ));
                }
            } else {
                selected = Some((value, e.provenance.clone()));
            }
        }
    }
    selected
}
fn identity(
    evidence: &[MediaEvidence],
    release: bool,
    conflicts: &mut Vec<MediaSetConflict>,
) -> Option<(IdentityKey, Provenance, Equivalence)> {
    let mut seen = BTreeMap::new();
    let mut selected = None;
    for e in evidence {
        let value = if release {
            e.release.as_ref()
        } else {
            e.medium.as_ref()
        };
        if let Some(value) = value {
            if value.namespace.is_empty() || value.value.is_empty() {
                conflicts.push(issue(
                    ConflictKind::InvalidEvidence,
                    "An identity claim has an empty namespace/value",
                    true,
                ));
                continue;
            }
            if !e.provenance.trusted() {
                continue;
            }
            if seen
                .insert(value.namespace.clone(), value.value.clone())
                .is_some_and(|v| v != value.value)
            {
                conflicts.push(issue(
                    if release {
                        ConflictKind::ReleaseConflict
                    } else {
                        ConflictKind::IdentityConflict
                    },
                    format!("Conflicting identities in {}", value.namespace),
                    true,
                ));
            }
            if selected.is_none() {
                selected = Some((value.clone(), e.provenance.clone(), e.equivalence));
            }
        }
    }
    selected
}
fn canonical_ordinal(mut o: MediaOrdinal, family: Option<MediaFamily>) -> MediaOrdinal {
    if matches!(
        o.unit,
        OrdinalUnit::Medium | OrdinalUnit::Disc | OrdinalUnit::Disk | OrdinalUnit::Tape
    ) {
        o.unit = match family {
            Some(MediaFamily::Optical) => OrdinalUnit::Disc,
            Some(MediaFamily::Floppy) => OrdinalUnit::Disk,
            Some(MediaFamily::Tape) => OrdinalUnit::Tape,
            None => o.unit,
        };
    }
    o
}
fn normalize(mut record: MediaRecord) -> MediaRepresentation {
    record.format = record.format.to_ascii_lowercase();
    record
        .evidence
        .sort_by(|a, b| b.provenance.kind.cmp(&a.provenance.kind).then(a.cmp(b)));
    record.evidence.dedup();
    let mut conflicts = Vec::new();
    if super::adapters::family_for_format(&record.format) != record.family {
        conflicts.push(issue(
            ConflictKind::UnsupportedFormat,
            "Declared media family does not match the supported representation format",
            true,
        ));
    }
    let canonical = record.platform.as_deref().and_then(|s| {
        crate::platform::platform_by_id(s).or_else(|| crate::platform::platform_for_alias(s))
    });
    if record.platform.is_some() && canonical.is_none() {
        conflicts.push(issue(
            ConflictKind::PlatformConflict,
            "Platform is not in the canonical registry",
            true,
        ));
    }
    record.platform = canonical.map(|p| p.id.to_owned());
    if record.platform.is_none() {
        conflicts.push(issue(
            ConflictKind::UnprovenGrouping,
            "Platform is unresolved; cross-record grouping is disabled",
            true,
        ));
    }
    if record.evidence.len() > 128 {
        conflicts.push(issue(
            ConflictKind::InvalidEvidence,
            "Evidence count exceeds 128 claims per record",
            true,
        ));
        record.evidence.truncate(128);
    }
    let e = &record.evidence;
    for claim in e {
        for note in &claim.notes {
            if note.starts_with("Conflicting")
                || note.starts_with("Multiple filename")
                || note.contains("exceeds topology")
            {
                conflicts.push(issue(ConflictKind::InvalidEvidence, note.clone(), true));
            }
        }
    }
    let release = identity(e, true, &mut conflicts).map(|(key, p, _)| MediaSetIdentity {
        key,
        provenance: vec![p],
        verified: true,
    });
    let medium =
        identity(e, false, &mut conflicts).map(|(key, provenance, equivalence)| MediaIdentity {
            key,
            provenance,
            equivalence,
        });
    let ordinal = choose(
        e,
        |e| {
            e.ordinal
                .clone()
                .map(|o| canonical_ordinal(o, record.family))
        },
        ConflictKind::OrdinalConflict,
        &mut conflicts,
    )
    .map(|x| x.0);
    let side = choose(
        e,
        |e| e.side.clone(),
        ConflictKind::SideConflict,
        &mut conflicts,
    )
    .map(|x| x.0);
    let role = choose(e, |e| e.role, ConflictKind::RoleConflict, &mut conflicts)
        .map_or(MediaRole::UnknownMedia, |x| x.0);
    let count = choose(
        e,
        |e| {
            e.expected_count.clone().map(|mut c| {
                c.unit = canonical_ordinal(
                    MediaOrdinal {
                        number: 1,
                        unit: c.unit,
                    },
                    record.family,
                )
                .unit;
                c
            })
        },
        ConflictKind::CountConflict,
        &mut conflicts,
    );
    if ordinal
        .as_ref()
        .is_some_and(|o| o.number == 0 || o.number > MAX_COUNT)
        || count
            .as_ref()
            .is_some_and(|(c, _)| c.count == 0 || c.count > MAX_COUNT)
        || side.as_ref().is_some_and(|s| s.number == 0 || s.number > 2)
    {
        conflicts.push(issue(
            ConflictKind::InvalidEvidence,
            "Ordinal/count/side is outside the supported bounds",
            true,
        ));
    }
    let variant = ReleaseVariant {
        region: choose(
            e,
            |e| e.variant.region.as_deref().map(normalize_title),
            ConflictKind::VariantConflict,
            &mut conflicts,
        )
        .map(|x| x.0),
        revision: choose(
            e,
            |e| e.variant.revision.as_deref().map(normalize_title),
            ConflictKind::VariantConflict,
            &mut conflicts,
        )
        .map(|x| x.0),
        language: choose(
            e,
            |e| e.variant.language.as_deref().map(normalize_title),
            ConflictKind::VariantConflict,
            &mut conflicts,
        )
        .map(|x| x.0),
        video_standard: choose(
            e,
            |e| e.variant.video_standard.as_deref().map(normalize_title),
            ConflictKind::VariantConflict,
            &mut conflicts,
        )
        .map(|x| x.0),
        edition: choose(
            e,
            |e| e.variant.edition.as_deref().map(normalize_title),
            ConflictKind::VariantConflict,
            &mut conflicts,
        )
        .map(|x| x.0),
    };
    let side_layout = choose(
        e,
        |e| {
            e.side_layout
                .filter(|layout| *layout != SideLayout::Unknown)
        },
        ConflictKind::SideConflict,
        &mut conflicts,
    )
    .map_or(SideLayout::Unknown, |x| x.0);
    let expected_sides = e
        .iter()
        .find(|e| !e.expected_sides.is_empty())
        .map(|e| e.expected_sides.clone())
        .unwrap_or_default();
    let confidence = if release.is_some() {
        MediaSetConfidence::Proven
    } else {
        MediaSetConfidence::Likely
    };
    MediaRepresentation {
        record,
        release_identity: release,
        media_identity: medium,
        ordinal,
        side,
        role,
        variant,
        expected_count: count,
        side_layout,
        expected_sides,
        confidence,
        conflicts,
    }
}
fn source_key(source: &MediaSource) -> String {
    // Serialization is solely an opaque in-memory identity, never an output path.
    format!("{:?}:{:?}", source.path, source.archive_member)
}
/// Catalogue-wide indexing: exact keys, no all-pairs or directory-neighbour scans.
pub fn index_media(records: Vec<MediaRecord>) -> MediaIndex {
    let mut groups: BTreeMap<GroupKey, Vec<MediaRepresentation>> = BTreeMap::new();
    let mut variants: BTreeMap<_, BTreeSet<ReleaseVariant>> = BTreeMap::new();
    let input_records = records.len();
    for record in records {
        let r = normalize(record);
        let key = r
            .release_identity
            .as_ref()
            .map(|x| x.key.clone())
            .or_else(|| {
                r.record
                    .evidence
                    .iter()
                    .find(|e| {
                        e.provenance.kind >= EvidenceKind::Filename
                            && e.title.as_ref().is_some_and(|t| !t.trim().is_empty())
                    })
                    .map(|e| {
                        IdentityKey::new(
                            "provisional-title",
                            normalize_title(e.title.as_deref().unwrap_or_default()),
                        )
                    })
            });
        let key = match (r.record.platform.as_ref(), key) {
            (Some(_), Some(key)) => key,
            _ => IdentityKey::new("unresolved-source", source_key(&r.record.source)),
        };
        let base = (r.record.platform.clone(), r.record.family, key.clone());
        variants.entry(base).or_default().insert(r.variant.clone());
        groups
            .entry((
                r.record.platform.clone(),
                r.record.family,
                key,
                r.variant.clone(),
            ))
            .or_default()
            .push(r);
    }
    let separated = variants
        .into_iter()
        .filter_map(|(key, variants)| (variants.len() > 1).then_some(key))
        .collect();
    let stats = TopologyStats {
        input_records,
        grouping_buckets: groups.len(),
        candidate_comparisons: 0,
    };
    MediaIndex {
        groups,
        separated,
        stats,
    }
}
fn equivalence(r: &MediaRepresentation) -> Option<IdentityKey> {
    r.record
        .evidence
        .iter()
        .find(|e| {
            e.provenance.trusted() && e.equivalence != Equivalence::None && e.medium.is_some()
        })
        .and_then(|e| e.medium.clone())
}
pub(super) fn auxiliary(role: MediaRole) -> bool {
    matches!(
        role,
        MediaRole::BonusMedia
            | MediaRole::ExtrasMedia
            | MediaRole::SaveMedia
            | MediaRole::DemoMedia
            | MediaRole::AudioMedia
    )
}
fn member_sort(a: &MediaSetMember, b: &MediaSetMember) -> std::cmp::Ordering {
    a.ordinal
        .is_none()
        .cmp(&b.ordinal.is_none())
        .then(a.ordinal.cmp(&b.ordinal))
        .then(role_order(a.role).cmp(&role_order(b.role)))
        .then(a.id.cmp(&b.id))
}
pub(super) fn role_order(role: MediaRole) -> u8 {
    match role {
        MediaRole::BootMedia => 0,
        MediaRole::InstallMedia => 1,
        MediaRole::GameMedia | MediaRole::PlayMedia => 2,
        MediaRole::DataMedia => 3,
        MediaRole::SystemMedia | MediaRole::UtilityMedia => 4,
        MediaRole::UnknownMedia => 5,
        _ => 6,
    }
}
fn available(m: &MediaSetMember, side: Option<&MediaSide>) -> bool {
    m.representations.iter().any(|r| {
        r.record.availability == MediaAvailability::Observed
            && (side.is_none()
                || r.side.as_ref() == side
                || r.side_layout == SideLayout::WholeMedium
                    && r.expected_sides.contains(side.unwrap()))
    })
}
fn build_set(
    key: GroupKey,
    mut records: Vec<MediaRepresentation>,
    separated: bool,
    stats: &mut TopologyStats,
) -> MediaSet {
    records.sort_by(|a, b| a.record.source.cmp(&b.record.source));
    let mut conflicts = records
        .iter()
        .flat_map(|r| r.conflicts.clone())
        .collect::<Vec<_>>();
    let mut warnings = records
        .iter()
        .flat_map(|r| r.record.warnings.clone())
        .collect::<Vec<_>>();
    if separated {
        conflicts.push(issue(ConflictKind::VariantConflict,"Records with this release/title key have incompatible or unknown region/revision/language/video/edition variants and were kept in separate sets",false));
    }
    if records.len() > MAX_SET_RECORDS {
        conflicts.push(issue(
            ConflictKind::InvalidEvidence,
            "Set exceeds 4096 input representations; no completeness proof is possible",
            true,
        ));
    }
    let proven = records
        .iter()
        .all(|r| r.release_identity.as_ref().is_some_and(|x| x.verified));
    let mut provenance = records
        .iter()
        .filter_map(|r| r.release_identity.as_ref())
        .flat_map(|i| i.provenance.clone())
        .collect::<Vec<_>>();
    if provenance.is_empty() {
        provenance = records
            .iter()
            .flat_map(|r| {
                r.record
                    .evidence
                    .iter()
                    .filter(|e| e.title.is_some())
                    .map(|e| e.provenance.clone())
            })
            .collect();
    }
    provenance.sort();
    provenance.dedup();
    let identity = MediaSetIdentity {
        key: key.2,
        provenance,
        verified: proven,
    };
    let mut count_claims = records
        .iter()
        .filter_map(|r| r.expected_count.clone())
        .collect::<Vec<_>>();
    count_claims.sort_by(|a, b| b.1.kind.cmp(&a.1.kind).then(a.cmp(b)));
    count_claims.dedup();
    let expected_count = count_claims.first().cloned();
    if count_claims
        .iter()
        .any(|(c, _)| Some(c) != expected_count.as_ref().map(|(c, _)| c))
    {
        conflicts.push(issue(
            ConflictKind::CountConflict,
            "Expected media count declarations disagree",
            true,
        ));
    }
    let requirements = records
        .iter()
        .flat_map(|r| r.record.evidence.iter())
        .filter(|e| e.provenance.kind >= EvidenceKind::Embedded)
        .flat_map(|e| {
            e.requirements
                .iter()
                .cloned()
                .map(|r| (r, e.provenance.clone()))
        })
        .fold(
            BTreeMap::<MediumRequirement, Provenance>::new(),
            |mut claims, (requirement, provenance)| {
                claims
                    .entry(requirement)
                    .and_modify(|prior| {
                        if provenance.kind > prior.kind
                            || (provenance.kind == prior.kind && provenance < *prior)
                        {
                            *prior = provenance.clone();
                        }
                    })
                    .or_insert(provenance);
                claims
            },
        );
    let mut buckets: BTreeMap<String, Vec<MediaRepresentation>> = BTreeMap::new();
    for r in records {
        stats.candidate_comparisons += 1;
        // Sides share a physical slot only when explicitly numbered, or when
        // a separate-side layout has been supplied. Unknown sides never become disks 1/2.
        let physical_side = r.side.is_some()
            && (r.ordinal.is_some() || r.side_layout == SideLayout::SeparateSideImages);
        let bucket = if physical_side {
            format!("side-slot:{:?}", r.ordinal)
        } else if let Some(id) = equivalence(&r) {
            format!("equivalent:{id:?}")
        } else {
            format!("source:{}", source_key(&r.record.source))
        };
        buckets.entry(bucket).or_default().push(r);
    }
    let mut members = Vec::new();
    for (id, representations) in buckets {
        let first = &representations[0];
        let mut by_side: BTreeMap<Option<MediaSide>, Vec<&MediaRepresentation>> = BTreeMap::new();
        for r in &representations {
            by_side.entry(r.side.clone()).or_default().push(r);
            if r.ordinal != first.ordinal {
                conflicts.push(issue(
                    ConflictKind::OrdinalConflict,
                    "Equivalent representations disagree about their ordinal",
                    true,
                ));
            }
            if r.side == first.side && r.role != first.role {
                conflicts.push(issue(
                    ConflictKind::RoleConflict,
                    "Equivalent representations disagree about their role",
                    true,
                ));
            }
        }
        for (side, reps) in by_side {
            if reps.len() > 1 {
                let keys = reps.iter().map(|r| equivalence(r)).collect::<BTreeSet<_>>();
                if keys.contains(&None) || keys.len() != 1 {
                    conflicts.push(issue(
                        ConflictKind::CompetingMedia,
                        format!(
                            "Competing representations for side {side:?} lack proven equivalence"
                        ),
                        true,
                    ));
                }
            }
        }
        let sides = representations
            .iter()
            .filter_map(|r| r.side.clone())
            .chain(
                representations
                    .iter()
                    .filter(|r| r.side_layout == SideLayout::WholeMedium)
                    .flat_map(|r| r.expected_sides.clone()),
            )
            .collect();
        let side_layout = if representations
            .iter()
            .any(|r| r.side_layout == SideLayout::WholeMedium)
        {
            SideLayout::WholeMedium
        } else if representations
            .iter()
            .all(|r| r.side_layout == SideLayout::SeparateSideImages)
        {
            SideLayout::SeparateSideImages
        } else {
            SideLayout::Unknown
        };
        members.push(MediaSetMember {
            id,
            ordinal: first.ordinal.clone(),
            role: if representations.iter().all(|r| r.role == first.role) {
                first.role
            } else {
                MediaRole::UnknownMedia
            },
            sides,
            side_layout,
            representations,
        });
    }
    members.sort_by(member_sort);
    let mut ordinal_index: BTreeMap<MediaOrdinal, Vec<usize>> = BTreeMap::new();
    let mut role_index: BTreeMap<MediaRole, Vec<usize>> = BTreeMap::new();
    let mut identity_index: BTreeMap<IdentityKey, Vec<usize>> = BTreeMap::new();
    for (i, m) in members.iter().enumerate() {
        if !auxiliary(m.role)
            && let Some(o) = &m.ordinal
        {
            ordinal_index.entry(o.clone()).or_default().push(i);
        }
        for role in m
            .representations
            .iter()
            .map(|r| r.role)
            .collect::<BTreeSet<_>>()
        {
            role_index.entry(role).or_default().push(i);
        }
        for r in &m.representations {
            for e in &r.record.evidence {
                if let Some(id) = &e.medium
                    && e.provenance.trusted()
                {
                    identity_index.entry(id.clone()).or_default().push(i);
                }
            }
        }
        let expected_sides = m
            .representations
            .iter()
            .flat_map(|r| r.expected_sides.iter())
            .collect::<BTreeSet<_>>();
        for side in expected_sides {
            if !available(m, Some(side)) {
                conflicts.push(issue(
                    if m.representations
                        .iter()
                        .any(|r| r.record.availability == MediaAvailability::Unverified)
                    {
                        ConflictKind::UnprovenGrouping
                    } else {
                        ConflictKind::MissingSide
                    },
                    format!("Missing side {} for {:?}", side.number, m.ordinal),
                    true,
                ));
            }
        }
        if !available(m, None) {
            conflicts.push(issue(
                if m.representations
                    .iter()
                    .all(|r| r.record.availability == MediaAvailability::Missing)
                {
                    ConflictKind::UnavailableRepresentation
                } else {
                    ConflictKind::UnprovenGrouping
                },
                format!("No observed representation for {}", m.id),
                true,
            ));
        }
        if !m.sides.is_empty() && m.side_layout == SideLayout::Unknown {
            conflicts.push(issue(
                ConflictKind::UnprovenGrouping,
                "Side labels do not prove whether images contain one side or a whole medium",
                true,
            ));
        }
    }
    for (ordinal, indices) in &ordinal_index {
        if indices.len() > 1 {
            conflicts.push(issue(
                ConflictKind::CompetingMedia,
                format!(
                    "{} distinct media compete for {:?} {}",
                    indices.len(),
                    ordinal.unit,
                    ordinal.number
                ),
                true,
            ));
        }
    }
    let unresolved_position = members
        .iter()
        .any(|m| m.ordinal.is_none() && !auxiliary(m.role));
    let unverified_source = members.iter().any(|m| {
        m.representations
            .iter()
            .any(|r| r.record.availability == MediaAvailability::Unverified)
    });
    if let Some((count, _)) = &expected_count
        && count.count > 0
        && count.count <= MAX_COUNT
    {
        for number in 1..=count.count {
            let o = MediaOrdinal {
                number,
                unit: count.unit,
            };
            if !ordinal_index
                .get(&o)
                .is_some_and(|ids| ids.iter().any(|i| available(&members[*i], None)))
            {
                // Role-based manifests are evaluated separately; an unnumbered
                // install/play pair is not assigned invented disc numbers.
                if !unresolved_position && !unverified_source {
                    conflicts.push(issue(
                        ConflictKind::MissingMedium,
                        format!(
                            "Missing {:?} {number} of {} (declared count provenance retained)",
                            count.unit, count.count
                        ),
                        true,
                    ));
                }
            }
        }
        if ordinal_index
            .keys()
            .any(|o| o.unit == count.unit && o.number > count.count)
        {
            conflicts.push(issue(
                ConflictKind::CountConflict,
                "A medium ordinal exceeds the expected total",
                true,
            ));
        }
    }
    let mut requirement_proven = true;
    for (required, prov) in &requirements {
        if required.optional {
            continue;
        }
        requirement_proven &= prov.trusted();
        let canonical = required
            .ordinal
            .clone()
            .map(|o| canonical_ordinal(o, key.1));
        let candidates = if let Some(id) = &required.medium {
            identity_index.get(id)
        } else if let Some(o) = &canonical {
            ordinal_index.get(o)
        } else if let Some(role) = required.role {
            role_index.get(&role)
        } else {
            None
        };
        let mut position_proven = false;
        if required.medium.is_none()
            && canonical.is_none()
            && candidates.is_some_and(|ids| {
                ids.iter()
                    .filter(|i| {
                        stats.candidate_comparisons += 1;
                        available(&members[**i], required.side.as_ref())
                    })
                    .take(2)
                    .count()
                    > 1
            })
        {
            conflicts.push(issue(ConflictKind::CompetingMedia,
                "Multiple distinct media satisfy one role-only requirement; their order or equivalence is unresolved", true));
        }
        let found = candidates.is_some_and(|ids| {
            ids.iter().any(|i| {
                stats.candidate_comparisons += 1;
                let m = &members[*i];
                m.representations.iter().any(|r| {
                    let matches = (canonical.is_none() || r.ordinal == canonical)
                        && (required.role.is_none() || required.role == Some(r.role))
                        && (required.side.is_none()
                            || r.side == required.side
                            || r.side_layout == SideLayout::WholeMedium
                                && r.expected_sides.contains(required.side.as_ref().unwrap()))
                        && r.record.availability == MediaAvailability::Observed;
                    if matches {
                        position_proven = [
                            (
                                canonical.is_some(),
                                r.record.evidence.iter().any(|e| {
                                    e.provenance.kind >= EvidenceKind::Embedded
                                        && e.ordinal.is_some()
                                }),
                            ),
                            (
                                required.role.is_some(),
                                r.record.evidence.iter().any(|e| {
                                    e.provenance.kind >= EvidenceKind::Embedded && e.role.is_some()
                                }),
                            ),
                            (
                                required.side.is_some(),
                                r.record.evidence.iter().any(|e| {
                                    e.provenance.kind >= EvidenceKind::Embedded
                                        && (e.side.is_some() || !e.expected_sides.is_empty())
                                }),
                            ),
                        ]
                        .iter()
                        .all(|(needed, proven)| !needed || *proven);
                    }
                    matches
                })
            })
        });
        requirement_proven &= position_proven;
        if !found {
            conflicts.push(issue(
                if unverified_source {
                    ConflictKind::UnprovenGrouping
                } else {
                    ConflictKind::MissingMedium
                },
                format!("Missing declared medium/side requirement {required:?}"),
                true,
            ));
        }
    }
    let trusted_positions = members.iter().filter(|m| !auxiliary(m.role)).all(|m| {
        m.representations.iter().all(|r| {
            r.record
                .evidence
                .iter()
                .any(|e| e.provenance.kind >= EvidenceKind::Embedded && e.ordinal.is_some())
        })
    });
    let known_scope = expected_count
        .as_ref()
        .is_some_and(|(_, p)| p.kind >= EvidenceKind::Embedded && trusted_positions)
        || (expected_count.is_none() && !requirements.is_empty() && requirement_proven);
    if !known_scope && expected_count.is_some() {
        conflicts.push(issue(
            ConflictKind::UnprovenGrouping,
            "Count or media positions rely on supporting evidence; completeness is unverified",
            true,
        ));
    }
    if expected_count.is_none() && requirements.is_empty() {
        conflicts.push(issue(
            ConflictKind::UnknownCount,
            "No expected media inventory/count is known; highest observed ordinal is not a total",
            true,
        ));
    }
    if !proven {
        conflicts.push(issue(
            ConflictKind::UnprovenGrouping,
            "Release grouping has supporting evidence only",
            true,
        ));
    }
    if members.iter().any(|m| {
        m.ordinal.is_none()
            && !auxiliary(m.role)
            && (requirements.is_empty() || m.role == MediaRole::UnknownMedia)
    }) {
        conflicts.push(issue(
            ConflictKind::UnknownOrdinal,
            "At least one required medium has no proven position",
            true,
        ));
    }
    if key.1.is_none()
        || members
            .iter()
            .flat_map(|m| &m.representations)
            .any(|r| r.record.family.is_none())
    {
        conflicts.push(issue(
            ConflictKind::UnsupportedFormat,
            "Unsupported media family/format",
            true,
        ));
    }
    // Explicit load relationships must stay within this set and be acyclic.
    let mut graph: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    let mut indegree = vec![0usize; members.len()];
    for (i, m) in members.iter().enumerate() {
        for e in m.representations.iter().flat_map(|r| &r.record.evidence) {
            for rel in &e.relationships {
                let targets = identity_index
                    .get(&rel.target)
                    .map(|v| v.iter().copied().collect::<BTreeSet<_>>())
                    .unwrap_or_default();
                if targets.len() != 1 {
                    conflicts.push(issue(
                        ConflictKind::RelationshipConflict,
                        "Load relationship target is absent or ambiguous",
                        true,
                    ));
                    continue;
                }
                let j = *targets.first().unwrap();
                if graph.entry(i).or_default().insert(j) {
                    indegree[j] += 1;
                }
                if members[i]
                    .ordinal
                    .as_ref()
                    .zip(members[j].ordinal.as_ref())
                    .is_some_and(|(a, b)| a.unit == b.unit && a.number >= b.number)
                {
                    conflicts.push(issue(
                        ConflictKind::RelationshipConflict,
                        "Load relationship contradicts the media order",
                        true,
                    ));
                }
            }
        }
    }
    let mut ready = (0..members.len())
        .filter(|i| indegree[*i] == 0)
        .collect::<BTreeSet<_>>();
    let mut order = Vec::new();
    while let Some(i) = ready.pop_first() {
        order.push(i);
        if let Some(next) = graph.get(&i) {
            for j in next {
                indegree[*j] -= 1;
                if indegree[*j] == 0 {
                    ready.insert(*j);
                }
            }
        }
    }
    if order.len() != members.len() {
        conflicts.push(issue(
            ConflictKind::RelationshipConflict,
            "Cyclic load relationships",
            true,
        ));
    } else {
        let mut slots = members.into_iter().map(Some).collect::<Vec<_>>();
        members = order
            .into_iter()
            .map(|i| slots[i].take().unwrap())
            .collect();
    }
    conflicts.sort();
    conflicts.dedup();
    warnings.sort();
    warnings.dedup();
    let has = |k| conflicts.iter().any(|c| c.blocking && c.kind == k);
    let state = if has(ConflictKind::UnsupportedFormat) || records_exceeded(&members) {
        MediaSetState::UnsupportedSet
    } else if [
        ConflictKind::PlatformConflict,
        ConflictKind::ReleaseConflict,
        ConflictKind::VariantConflict,
        ConflictKind::OrdinalConflict,
        ConflictKind::SideConflict,
        ConflictKind::RoleConflict,
        ConflictKind::CountConflict,
        ConflictKind::IdentityConflict,
        ConflictKind::InvalidEvidence,
        ConflictKind::RelationshipConflict,
    ]
    .iter()
    .any(|k| has(*k))
    {
        MediaSetState::ConflictingSet
    } else if has(ConflictKind::CompetingMedia) {
        MediaSetState::AmbiguousSet
    } else if has(ConflictKind::MissingMedium)
        || has(ConflictKind::MissingSide)
        || has(ConflictKind::UnavailableRepresentation)
    {
        MediaSetState::IncompleteSet
    } else if !known_scope
        || has(ConflictKind::UnknownOrdinal)
        || has(ConflictKind::UnprovenGrouping)
        || has(ConflictKind::UnknownCount)
    {
        MediaSetState::UnverifiedSet
    } else {
        MediaSetState::CompleteSet
    };
    let confidence = match state {
        MediaSetState::ConflictingSet => MediaSetConfidence::Conflicting,
        MediaSetState::AmbiguousSet => MediaSetConfidence::Ambiguous,
        _ if proven => MediaSetConfidence::Proven,
        _ if identity.key.namespace == "provisional-title" => MediaSetConfidence::Likely,
        _ => MediaSetConfidence::Unverified,
    };
    MediaSet {
        identity,
        platform: key.0,
        family: key.1,
        variant: key.3,
        members,
        expected_count,
        state,
        confidence,
        conflicts,
        warnings,
    }
}
fn records_exceeded(members: &[MediaSetMember]) -> bool {
    members
        .iter()
        .map(|m| m.representations.len())
        .sum::<usize>()
        > MAX_SET_RECORDS
}
pub fn resolve_index(index: MediaIndex) -> TopologyReport {
    let MediaIndex {
        groups,
        separated,
        mut stats,
    } = index;
    let sets = groups
        .into_iter()
        .map(|(key, records)| {
            let split = separated.contains(&(key.0.clone(), key.1, key.2.clone()));
            build_set(key, records, split, &mut stats)
        })
        .collect();
    TopologyReport { sets, stats }
}
