//! Deterministic, read-only resolution of a reviewed cheat reconciliation.

//!
//! This is deliberately the seam between review and an emulator-specific
//! installer. It consumes the existing reconciliation report and the exact
//! persisted review choices, keeps conflicts fail-closed, collapses only
//! proven duplicates, and performs no filesystem or emulator writes.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::cheat_ir::{
    CheatDocument, CheatIssue, CheatOperation, CheatPlatform, CheatReconciliationEntry,
    CheatReconciliationGroup, CheatReconciliationResult, CheatRelationship,
};

/// The persisted review vocabulary. GUI persistence may use its own private
/// representation, but conversion to this enum must preserve these semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatReviewChoice {
    KeepA,
    KeepB,
    KeepBoth,
    Skip,
    IgnoreConflict,
}

/// A resolved entry, with every source entry that was collapsed into it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCheatEntry {
    pub canonical_entry_index: usize,
    pub duplicate_entry_indices: Vec<usize>,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub title: String,
    pub platform: CheatPlatform,
    pub source_format: super::cheat_ir::CheatSourceFormat,
    pub document: CheatDocument,
    pub provenance: Vec<String>,
    pub selected_by_review: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCheatDiagnostic {
    #[serde(default)]
    pub source_evidence: Vec<super::cheat_provenance::CheatRecordProvenance>,
    pub entry_indices: Vec<usize>,
    pub title: String,
    pub reason: String,
    #[serde(default)]
    pub classifications: Vec<super::cheat_ir::CheatDuplicateKind>,
    pub provenance: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCheatPlanDestination {
    pub emulator: String,
    pub profile: String,
    pub target_file: Option<String>,
    pub existing_file_digest: Option<String>,
    pub destination_changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedCheatApplyEligibility {
    PreviewOnly { reason: String },
    Blocked { reasons: Vec<String> },
}

/// The complete reviewed result. It is an approval input, not an install
/// command; emulator-specific journeys remain responsible for staging,
/// backup, transaction and rollback.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCheatPlan {
    /// Audit snapshot retains disagreement even when a review chooses one representation.
    #[serde(default)]
    pub source_report: Option<Box<CheatReconciliationResult>>,
    pub source_report_digest: String,
    pub review_choices_digest: String,
    pub game_identity: String,
    pub identity_verified: bool,
    pub platform: CheatPlatform,
    pub emulator: String,
    pub profile: String,
    pub selected_entries: Vec<ResolvedCheatEntry>,
    pub skipped_entries: Vec<ResolvedCheatDiagnostic>,
    pub ignored_entries: Vec<ResolvedCheatDiagnostic>,
    pub unresolved_conflicts: Vec<ResolvedCheatDiagnostic>,
    pub malformed_entries: Vec<ResolvedCheatDiagnostic>,
    pub unsupported_entries: Vec<ResolvedCheatDiagnostic>,
    pub destination: ResolvedCheatPlanDestination,
    pub warnings: Vec<String>,
    pub apply_eligibility: ResolvedCheatApplyEligibility,
}

/// Inputs captured by the preview. Callers must invalidate this request when
/// the report digest, review choices, game identity, profile, or destination
/// changes; no path-only authority is created here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCheatPlanRequest {
    pub source_report_digest: String,
    pub emulator: String,
    pub profile: String,
    pub target_file: Option<String>,
    pub existing_file_digest: Option<String>,
    pub destination_changed: bool,
}

impl ResolvedCheatPlan {
    /// Resolves the report in stable group/entry order. Missing choices on a
    /// conflict never imply a winner. Unique entries are included when safe;
    /// malformed and unsupported entries are retained diagnostically only.
    pub fn build(
        report: &CheatReconciliationResult,
        choices: &BTreeMap<usize, CheatReviewChoice>,
        request: &ResolvedCheatPlanRequest,
    ) -> Self {
        let mut selected = Vec::new();
        let mut skipped = Vec::new();
        let mut ignored = Vec::new();
        let mut unresolved = Vec::new();
        let mut malformed = Vec::new();
        let mut unsupported = Vec::new();
        let mut consumed = BTreeSet::new();
        let mut warnings = Vec::new();
        // A filtered or stale projection must not turn omitted conflicts
        // into automatically selected fallback entries.
        let current = super::cheat_ir::reconcile_cheats_for_game(report.entries.clone());
        let current_groups = match &current {
            super::cheat_ir::CheatReconciliationOutcome::Ready(current) => {
                current.groups.as_slice()
            }
            _ => &[],
        };
        let needs_review = |group: &CheatReconciliationGroup| {
            matches!(
                group.relationship,
                CheatRelationship::SameTitleDifferentCode | CheatRelationship::RelatedUnproven
            )
        };
        let review_by_entry: BTreeMap<_, _> = current_groups
            .iter()
            .filter(|group| needs_review(group))
            .flat_map(|group| group.entry_indices.iter().map(move |index| (*index, group)))
            .collect();

        for (group_index, group) in report.groups.iter().enumerate() {
            let indices = valid_indices(report, group);
            if indices.is_empty() {
                continue;
            }
            if indices.iter().any(|index| consumed.contains(index)) {
                continue;
            }
            if let Some(conflict) = indices
                .iter()
                .filter_map(|index| review_by_entry.get(index))
                .find(|current| {
                    !needs_review(group)
                        || current
                            .entry_indices
                            .iter()
                            .copied()
                            .collect::<BTreeSet<_>>()
                            != indices.iter().copied().collect()
                })
            {
                consumed.extend(conflict.entry_indices.iter().copied());
                let mut item = diagnostic(
                    report,
                    &conflict.entry_indices,
                    "source evidence contains a conflict absent from this group projection",
                );
                item.classifications = conflict.classifications.clone();
                unresolved.push(item);
                continue;
            }
            consumed.extend(indices.iter().copied());
            let choice = choices.get(&group_index).copied();
            let selected_indices: Vec<usize> = match group.relationship {
                CheatRelationship::SameTitleDifferentCode | CheatRelationship::RelatedUnproven => {
                    match choice {
                        Some(CheatReviewChoice::KeepA) if indices.len() == 2 => {
                            indices.first().copied().into_iter().collect()
                        }
                        Some(CheatReviewChoice::KeepB) if indices.len() == 2 => {
                            indices.get(1).copied().into_iter().collect()
                        }
                        Some(CheatReviewChoice::KeepBoth) => indices.clone(),
                        Some(CheatReviewChoice::Skip) => {
                            skipped.push(diagnostic(
                                report,
                                &indices,
                                "excluded by the saved Skip review choice",
                            ));
                            Vec::new()
                        }
                        Some(CheatReviewChoice::IgnoreConflict) => {
                            ignored.push(diagnostic(
                                report,
                                &indices,
                                "left unresolved by the saved Ignore review choice",
                            ));
                            Vec::new()
                        }
                        None | Some(CheatReviewChoice::KeepA | CheatReviewChoice::KeepB) => {
                            unresolved.push(diagnostic(
                                report,
                                &indices,
                                "conflicting entries have no saved review choice",
                            ));
                            Vec::new()
                        }
                    }
                }
                CheatRelationship::ExactSemanticDuplicate
                | CheatRelationship::ExactRawDuplicate => {
                    indices.first().copied().into_iter().collect()
                }
                CheatRelationship::Unique => indices.clone(),
            };
            for index in selected_indices {
                if let Some(entry) = report.entries.get(index) {
                    if let Some(reason) = unsafe_reason(entry) {
                        let item = diagnostic(report, &[index], reason);
                        if is_malformed(entry) {
                            malformed.push(item);
                        } else {
                            unsupported.push(item);
                        }
                    } else {
                        let provenance_indices = match group.relationship {
                            CheatRelationship::ExactSemanticDuplicate
                            | CheatRelationship::ExactRawDuplicate => &indices,
                            _ => std::slice::from_ref(&index),
                        };
                        selected.push(resolved_entry(report, index, provenance_indices));
                    }
                }
            }
        }

        for index in 0..report.entries.len() {
            if !consumed.contains(&index) {
                if let Some(conflict) = review_by_entry.get(&index) {
                    let unseen: Vec<_> = conflict
                        .entry_indices
                        .iter()
                        .copied()
                        .filter(|index| !consumed.contains(index))
                        .collect();
                    consumed.extend(unseen.iter().copied());
                    let mut item = diagnostic(report, &unseen, "omitted conflict requires review");
                    item.classifications = conflict.classifications.clone();
                    unresolved.push(item);
                    continue;
                }
                let entry = &report.entries[index];
                if let Some(reason) = unsafe_reason(entry) {
                    let item = diagnostic(report, &[index], reason);
                    if is_malformed(entry) {
                        malformed.push(item);
                    } else {
                        unsupported.push(item);
                    }
                } else {
                    selected.push(resolved_entry(report, index, &[index]));
                }
            }
        }
        // Review state must not hide the malformed/unsupported evidence of
        // a source entry, including entries omitted from a filtered report.
        let mut diagnosed: BTreeSet<_> = malformed
            .iter()
            .chain(unsupported.iter())
            .flat_map(|item| item.entry_indices.iter().copied())
            .collect();
        for (index, entry) in report.entries.iter().enumerate() {
            if !diagnosed.contains(&index) {
                if let Some(reason) = unsafe_reason(entry) {
                    let item = diagnostic(report, &[index], reason);
                    if is_malformed(entry) {
                        malformed.push(item);
                    } else {
                        unsupported.push(item);
                    }
                    diagnosed.insert(index);
                }
            }
        }
        selected.sort_by_cached_key(|entry| {
            (
                entry
                    .title
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_ascii_lowercase(),
                entry.provenance.clone(),
                serde_json::to_string(&entry.document)
                    .expect("resolved cheat document is serializable"),
            )
        });
        warnings.extend(
            malformed
                .iter()
                .chain(unsupported.iter())
                .map(|item| item.reason.clone()),
        );
        if !unresolved.is_empty() {
            warnings.push("unresolved cheat conflicts require review".into());
        }
        if !report.entries.iter().all(|entry| entry.identity_verified) {
            warnings.push("one or more source entries lack verified game identity".into());
        }
        if request.destination_changed {
            warnings.push("destination changed since preview inputs were captured".into());
        }

        let mut blocked = Vec::new();
        if matches!(
            current,
            super::cheat_ir::CheatReconciliationOutcome::Unavailable { .. }
        ) || report.entries.iter().any(|entry| {
            entry.game_identity != report.game_identity
                || entry.document.platform != report.platform
        }) {
            blocked.push("entries do not share the reported verified game identity".into());
            selected.clear();
        }
        if report.auto_winner.is_some() {
            blocked.push("reconciliation report supplied an automatic winner".into());
        }
        if !report.entries.iter().all(|entry| entry.identity_verified) {
            blocked.push("game identity is not fully verified".into());
        }
        if !unresolved.is_empty() || !ignored.is_empty() {
            blocked.push("unresolved conflicts remain".into());
        }
        if request.destination_changed {
            blocked.push("destination freshness check failed".into());
        }
        let apply_eligibility = if blocked.is_empty() {
            ResolvedCheatApplyEligibility::PreviewOnly { reason: "reconciliation entries are format-neutral; an emulator-specific reviewed installer must perform any apply".into() }
        } else {
            ResolvedCheatApplyEligibility::Blocked { reasons: blocked }
        };

        Self {
            source_report: Some(Box::new(report.clone())),
            source_report_digest: request.source_report_digest.clone(),
            review_choices_digest: choices_digest(choices),
            game_identity: report.game_identity.clone(),
            identity_verified: report.entries.iter().all(|entry| entry.identity_verified),
            platform: report.platform.clone(),
            emulator: request.emulator.clone(),
            profile: request.profile.clone(),
            selected_entries: selected,
            skipped_entries: skipped,
            ignored_entries: ignored,
            unresolved_conflicts: unresolved,
            malformed_entries: malformed,
            unsupported_entries: unsupported,
            destination: ResolvedCheatPlanDestination {
                emulator: request.emulator.clone(),
                profile: request.profile.clone(),
                target_file: request.target_file.clone(),
                existing_file_digest: request.existing_file_digest.clone(),
                destination_changed: request.destination_changed,
            },
            warnings,
            apply_eligibility,
        }
    }

    pub fn is_fresh(
        &self,
        source_report_digest: &str,
        choices: &BTreeMap<usize, CheatReviewChoice>,
        game_identity: &str,
        emulator: &str,
        profile: &str,
        destination_changed: bool,
    ) -> bool {
        !destination_changed
            && self.source_report_digest == source_report_digest
            && self.review_choices_digest == choices_digest(choices)
            && self.game_identity == game_identity
            && self.emulator == emulator
            && self.profile == profile
    }
}

pub fn resolve_reviewed_cheat_plan(
    report: &CheatReconciliationResult,
    choices: &BTreeMap<usize, CheatReviewChoice>,
    request: &ResolvedCheatPlanRequest,
) -> ResolvedCheatPlan {
    ResolvedCheatPlan::build(report, choices, request)
}

fn valid_indices(
    report: &CheatReconciliationResult,
    group: &CheatReconciliationGroup,
) -> Vec<usize> {
    group
        .entry_indices
        .iter()
        .copied()
        .filter(|index| *index < report.entries.len())
        .collect()
}

fn choices_digest(choices: &BTreeMap<usize, CheatReviewChoice>) -> String {
    let bytes = serde_json::to_vec(choices).expect("review choices are serializable");
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_malformed(entry: &CheatReconciliationEntry) -> bool {
    entry.document.issues.iter().any(|issue| {
        matches!(
            issue,
            CheatIssue::UnsupportedOperation(_) | CheatIssue::UnknownWidth
        )
    })
}

fn unsafe_reason(entry: &CheatReconciliationEntry) -> Option<String> {
    if !entry.document.issues.is_empty() {
        return Some(format!(
            "entry has parser issues: {:?}",
            entry.document.issues
        ));
    }
    if entry
        .document
        .operations
        .iter()
        .any(|operation| matches!(operation, CheatOperation::UnsupportedRaw { .. }))
    {
        return Some("entry contains an unsupported or raw operation".into());
    }
    if entry.document.operations.is_empty() {
        return Some("entry contains no executable operations".into());
    }
    None
}

fn resolved_entry(
    report: &CheatReconciliationResult,
    index: usize,
    duplicate_indices: &[usize],
) -> ResolvedCheatEntry {
    let entry = &report.entries[index];
    let mut provenance = Vec::new();
    for source_index in duplicate_indices {
        if let Some(source) = report.entries.get(*source_index) {
            provenance.push(source.source.clone());
            if let Some(path) = &source.source_path {
                provenance.push(format!("{}: {path}", source.source));
            }
            provenance.extend(
                source
                    .document
                    .provenance
                    .iter()
                    .map(|value| format!("{}: {value}", source.source)),
            );
            provenance.extend(
                source
                    .provenance
                    .iter()
                    .map(|value| format!("{}: {value}", source.source)),
            );
        }
    }
    provenance.sort();
    provenance.dedup();
    let mut document = entry.document.clone();
    document.source_evidence = duplicate_indices
        .iter()
        .filter_map(|i| report.entries.get(*i))
        .flat_map(|e| e.audit_evidence())
        .collect();
    super::cheat_provenance::order_cheat_provenance(&mut document.source_evidence);
    ResolvedCheatEntry {
        canonical_entry_index: index,
        duplicate_entry_indices: duplicate_indices.to_vec(),
        aliases: duplicate_indices
            .iter()
            .filter_map(|index| report.entries.get(*index).map(|entry| entry.title.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        title: entry.title.clone(),
        platform: entry.document.platform.clone(),
        source_format: entry.source_format.clone(),
        document,
        provenance,
        selected_by_review: true,
    }
}

fn diagnostic(
    report: &CheatReconciliationResult,
    indices: &[usize],
    reason: impl Into<String>,
) -> ResolvedCheatDiagnostic {
    let mut provenance = Vec::new();
    let title = indices
        .first()
        .and_then(|index| report.entries.get(*index))
        .map(|entry| entry.title.clone())
        .unwrap_or_default();
    for index in indices {
        if let Some(entry) = report.entries.get(*index) {
            provenance.push(entry.source.clone());
            provenance.extend(
                entry
                    .document
                    .provenance
                    .iter()
                    .map(|value| format!("{}: {value}", entry.source)),
            );
            provenance.extend(
                entry
                    .provenance
                    .iter()
                    .map(|value| format!("{}: {value}", entry.source)),
            );
        }
    }
    provenance.sort();
    provenance.dedup();
    let mut source_evidence = indices
        .iter()
        .filter_map(|i| report.entries.get(*i))
        .flat_map(|e| e.audit_evidence())
        .collect::<Vec<_>>();
    super::cheat_provenance::order_cheat_provenance(&mut source_evidence);
    ResolvedCheatDiagnostic {
        source_evidence,
        entry_indices: indices.to_vec(),
        title,
        reason: reason.into(),
        classifications: report
            .groups
            .iter()
            .filter(|group| {
                group
                    .entry_indices
                    .iter()
                    .any(|index| indices.contains(index))
            })
            .flat_map(|group| group.classifications.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        provenance,
    }
}

#[cfg(test)]
mod tests;
