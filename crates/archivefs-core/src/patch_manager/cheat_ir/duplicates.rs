//! Deterministic evidence grouping. Connected name/code/index buckets ensure
//! a duplicate subset cannot hide another implementation of the same cheat.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

fn explicit_variant(left: &Option<String>, right: &Option<String>) -> bool {
    matches!((left, right), (Some(a), Some(b)) if a != b)
}

fn classify(
    a: &CheatReconciliationEntry,
    b: &CheatReconciliationEntry,
    same_title: bool,
    same_semantic: bool,
    same_raw: bool,
) -> BTreeSet<CheatDuplicateKind> {
    use CheatDuplicateKind::*;
    let mut kinds = BTreeSet::new();
    let same_index = a.source == b.source
        && a.source_path == b.source_path
        && a.source_index.is_some()
        && a.source_index == b.source_index;
    if same_index
        && (!same_title
            || ((a.raw_code != b.raw_code || a.document.operations != b.document.operations)
                && !same_raw
                && !same_semantic)
            || a.source_format != b.source_format
            || a.applicability != b.applicability)
    {
        kinds.insert(SourceIndexConflict);
    }
    if explicit_variant(&a.applicability.region, &b.applicability.region) {
        kinds.insert(RegionVariant);
    }
    if explicit_variant(&a.applicability.revision, &b.applicability.revision)
        || explicit_variant(
            &a.applicability.verified_binary_identity,
            &b.applicability.verified_binary_identity,
        )
    {
        kinds.insert(VersionVariant);
    }
    if explicit_variant(&a.applicability.engine, &b.applicability.engine) {
        kinds.insert(SyntaxVariant);
    }
    if !kinds.is_empty() {
        return kinds;
    }
    if a.applicability != b.applicability
        || a.document
            .issues
            .iter()
            .chain(b.document.issues.iter())
            .any(|issue| !matches!(issue, CheatIssue::RawPreserved))
        || a.document.issues != b.document.issues
        || a.source_format != a.document.source_format
        || b.source_format != b.document.source_format
    {
        kinds.insert(AmbiguousPossibleDuplicate);
    } else if same_raw
        && !same_semantic
        && (operation_semantics_known(&a.document) && operation_semantics_known(&b.document)
            || a.document
                .operations
                .iter()
                .filter(|operation| !matches!(operation, CheatOperation::UnsupportedRaw { .. }))
                .collect::<Vec<_>>()
                != b.document
                    .operations
                    .iter()
                    .filter(|operation| !matches!(operation, CheatOperation::UnsupportedRaw { .. }))
                    .collect::<Vec<_>>())
    {
        kinds.insert(
            if operation_semantics_known(&a.document) && operation_semantics_known(&b.document) {
                CodeConflict
            } else {
                AmbiguousPossibleDuplicate
            },
        );
    } else if same_semantic || (same_raw && a.source_format == b.source_format) {
        kinds.insert(if same_title && a.source_format == b.source_format {
            ExactDuplicate
        } else {
            EquivalentDuplicate
        });
    } else if same_title {
        if a.source_format != b.source_format {
            kinds.insert(SyntaxVariant);
        } else if (operation_semantics_known(&a.document) && operation_semantics_known(&b.document))
            || (raw_fingerprint(a).is_some() && raw_fingerprint(b).is_some())
        {
            kinds.insert(NameConflict);
        } else {
            kinds.insert(AmbiguousPossibleDuplicate);
        }
    } else {
        kinds.insert(AmbiguousPossibleDuplicate);
    }
    kinds
}

pub(super) fn reconcile(entries: Vec<CheatReconciliationEntry>) -> CheatReconciliationOutcome {
    let Some(first) = entries.first() else {
        return CheatReconciliationOutcome::Unavailable {
            reason: "no cheat entries were supplied".into(),
        };
    };
    if !first.identity_verified
        || first.game_identity.trim().is_empty()
        || entries.iter().any(|entry| {
            !entry.identity_verified
                || entry.game_identity != first.game_identity
                || entry.document.platform != first.document.platform
        })
    {
        return CheatReconciliationOutcome::Unavailable {
            reason: "entries do not share one verified game and platform identity".into(),
        };
    }
    CheatReconciliationOutcome::Ready(group(entries))
}

/// Shared read-only grouping also accepts uncertain observations. It never
/// grants them verified identity or an automatic duplicate/merge decision.
pub(super) fn group(entries: Vec<CheatReconciliationEntry>) -> CheatReconciliationResult {
    let first = &entries[0];
    let semantics: Vec<_> = entries
        .iter()
        .map(|entry| {
            entry
                .document
                .issues
                .iter()
                .all(|issue| matches!(issue, CheatIssue::RawPreserved))
                .then(|| semantic_fingerprint(&entry.document))
                .flatten()
        })
        .collect();
    let raws: Vec<_> = entries.iter().map(raw_fingerprint).collect();
    let titles: Vec<_> = entries
        .iter()
        .map(|entry| reconciliation_title(&entry.title))
        .collect();
    // Comparison keys are tuples, never delimiter-concatenated source strings.
    let mut buckets = BTreeMap::<(u8, String), Vec<usize>>::new();
    for (index, entry) in entries.iter().enumerate() {
        if !titles[index].is_empty() {
            buckets
                .entry((0, titles[index].clone()))
                .or_default()
                .push(index);
        }
        if let Some(key) = &semantics[index] {
            buckets.entry((1, key.clone())).or_default().push(index);
        }
        if let Some(key) = &raws[index] {
            buckets.entry((2, key.clone())).or_default().push(index);
        }
        if let Some(key) = possible_raw_fingerprint(entry) {
            buckets.entry((4, key)).or_default().push(index);
        }
        if let Some(source_index) = entry.source_index {
            let key = serde_json::to_string(&(&entry.source, &entry.source_path, source_index))
                .expect("source key is serializable");
            buckets.entry((3, key)).or_default().push(index);
        }
    }
    let stable_keys: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                titles[index].clone(),
                entry.source.clone(),
                entry.source_path.clone(),
                entry.source_index,
                serde_json::to_string(entry).expect("cheat evidence is serializable"),
            )
        })
        .collect();
    let mut parents: Vec<_> = (0..entries.len()).collect();
    let mut evidence = vec![BTreeSet::new(); entries.len()];
    for indices in buckets.values_mut() {
        indices.sort_by(|a, b| stable_keys[*a].cmp(&stable_keys[*b]));
        let anchor = indices[0];
        // A star plus adjacent edges catches differences without quadratic
        // work on packs containing thousands of copies of the same record.
        for pair in indices
            .windows(2)
            .map(|p| (p[0], p[1]))
            .chain(indices.iter().skip(1).map(|b| (anchor, *b)))
        {
            let (a, b) = pair;
            let left = root(&mut parents, a);
            let right = root(&mut parents, b);
            parents[right] = left;
            let kinds = if !entries[a].identity_verified || !entries[b].identity_verified {
                BTreeSet::from([CheatDuplicateKind::AmbiguousPossibleDuplicate])
            } else {
                classify(
                    &entries[a],
                    &entries[b],
                    titles[a] == titles[b],
                    semantics[a].is_some() && semantics[a] == semantics[b],
                    raws[a].is_some() && raws[a] == raws[b],
                )
            };
            evidence[a].extend(kinds.iter().copied());
            evidence[b].extend(kinds);
        }
    }
    let mut components = BTreeMap::<usize, Vec<usize>>::new();
    for index in 0..entries.len() {
        components
            .entry(root(&mut parents, index))
            .or_default()
            .push(index);
    }
    let mut groups = Vec::new();
    for mut indices in components.into_values() {
        indices.sort_by(|a, b| stable_keys[*a].cmp(&stable_keys[*b]));
        let mut kinds = BTreeSet::new();
        for index in &indices {
            kinds.extend(evidence[*index].iter().copied());
            if entries[*index]
                .document
                .issues
                .contains(&CheatIssue::SourceIndexConflict)
            {
                kinds.insert(CheatDuplicateKind::SourceIndexConflict);
            }
        }
        if indices.iter().any(|index| {
            entries[*index]
                .document
                .issues
                .contains(&CheatIssue::SourceMetadataConflict)
        }) {
            kinds.insert(CheatDuplicateKind::SourceMetadataConflict);
        }
        if kinds.is_empty() {
            kinds.insert(CheatDuplicateKind::Unique);
        }
        let review = kinds.iter().any(|kind| kind.requires_review());
        let semantic = semantics[indices[0]]
            .clone()
            .filter(|key| indices.iter().all(|i| semantics[*i].as_ref() == Some(key)));
        let raw = raws[indices[0]]
            .clone()
            .filter(|key| indices.iter().all(|i| raws[*i].as_ref() == Some(key)));
        let relationship = if kinds.iter().any(|kind| {
            matches!(
                kind,
                CheatDuplicateKind::NameConflict
                    | CheatDuplicateKind::CodeConflict
                    | CheatDuplicateKind::SourceIndexConflict
                    | CheatDuplicateKind::SourceMetadataConflict
            )
        }) {
            CheatRelationship::SameTitleDifferentCode
        } else if review {
            CheatRelationship::RelatedUnproven
        } else if indices.len() == 1 {
            CheatRelationship::Unique
        } else if semantic.is_some() {
            CheatRelationship::ExactSemanticDuplicate
        } else {
            CheatRelationship::ExactRawDuplicate
        };
        let mut differences = Vec::new();
        if review {
            differences.extend(
                kinds
                    .iter()
                    .filter(|kind| kind.requires_review())
                    .map(|kind| format!("{kind:?}")),
            );
            for pair in indices.windows(2) {
                differences.extend(difference_summary(
                    &entries[pair[0]].document,
                    &entries[pair[1]].document,
                ));
            }
        }
        groups.push(CheatReconciliationGroup {
            relationship,
            classifications: kinds.into_iter().collect(),
            normalized_title: titles[indices[0]].clone(),
            quality: indices
                .iter()
                .map(|i| entry_quality(&entries[*i]))
                .collect(),
            entry_indices: indices,
            semantic_fingerprint: semantic,
            raw_fingerprint: raw,
            differences,
        });
    }
    groups.sort_by(|a, b| stable_keys[a.entry_indices[0]].cmp(&stable_keys[b.entry_indices[0]]));
    CheatReconciliationResult {
        game_identity: first.game_identity.clone(),
        platform: first.document.platform.clone(),
        groups,
        entries,
        auto_winner: None,
    }
}

#[cfg(test)]
mod tests;
