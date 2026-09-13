//! Deterministic, read-only reconciliation of identity, topology and readiness
//! evidence.
//!
//! This module is deliberately a derived layer.  It does not inspect files,
//! choose a DAT, write a catalogue, launch an emulator, or replace any of the
//! existing evidence producers.  Callers adapt their already-observed facts
//! into [`EvidenceClaim`] values and receive an explainable result.
//!
//! Claims retain their provenance and derivation root.  Two parsers fed by the
//! same filename therefore do not count as independent agreement, while a
//! native header and an authoritative DAT can strengthen one another.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The properties this resolver understands.  Values are typed separately in
/// [`EvidenceValue`], so callers cannot accidentally use a media ordinal as a
/// platform claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimProperty {
    Platform,
    GameIdentity,
    Region,
    Revision,
    Language,
    MediaSetMembership,
    MediaOrdinal,
    MediaSide,
    MediaRole,
    ExpectedMediaCount,
    BiosDependency,
    LaunchTarget,
    EmulatorProfileCompatibility,
}

impl ClaimProperty {
    pub fn label(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::GameIdentity => "game/release identity",
            Self::Region => "region",
            Self::Revision => "revision/version",
            Self::Language => "language",
            Self::MediaSetMembership => "media-set membership",
            Self::MediaOrdinal => "media ordinal",
            Self::MediaSide => "media side",
            Self::MediaRole => "media role",
            Self::ExpectedMediaCount => "expected media count",
            Self::BiosDependency => "BIOS dependency",
            Self::LaunchTarget => "launch target",
            Self::EmulatorProfileCompatibility => "emulator/profile compatibility",
        }
    }
}

/// Typed claim values.  Text is used only for domains whose vocabulary is
/// intentionally open (for example a provider's release namespace).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum EvidenceValue {
    Platform(String),
    GameIdentity { namespace: String, value: String },
    Region(String),
    Revision(String),
    Language(String),
    MediaSetMembership(String),
    MediaOrdinal(u16),
    MediaSide(u8),
    MediaRole(String),
    ExpectedMediaCount(u16),
    BiosDependency(String),
    LaunchTarget(String),
    EmulatorProfileCompatibility(String),
}

impl EvidenceValue {
    fn canonical(&self) -> String {
        serde_json::to_string(self).expect("EvidenceValue is always serializable")
    }
}

/// Broad source class.  The resolver applies a domain-specific order to these
/// classes; this enum is not itself a universal truth hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceSource {
    NativeVerified,
    AuthorityVerified,
    ContentVerified,
    UserConfirmed,
    StructuredMetadata,
    FilenameStrong,
    DirectoryContext,
    FuzzyInference,
}

impl EvidenceSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::NativeVerified => "native verified",
            Self::AuthorityVerified => "authority verified",
            Self::ContentVerified => "content verified",
            Self::UserConfirmed => "human confirmed",
            Self::StructuredMetadata => "structured metadata",
            Self::FilenameStrong => "filename/path",
            Self::DirectoryContext => "directory context",
            Self::FuzzyInference => "fuzzy inference",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStrength {
    Weak,
    Corroborated,
    Strong,
    Verified,
}

/// The polarity distinction is important: a provider returning no result is
/// not evidence that a title is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClaimPolarity {
    Supports,
    NoSupport,
    Contradicts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityState {
    Current,
    Partial,
    Stale,
    Unknown,
}

/// Scope/generation metadata copied from the existing source-of-truth
/// projection.  Unknown fields stay unknown rather than being fabricated.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EvidenceScope {
    pub ecosystem: Option<String>,
    pub platform: Option<String>,
    pub authority_digest: Option<String>,
    pub authority_version: Option<String>,
    pub coverage_complete: Option<bool>,
    pub authority_state: Option<AuthorityState>,
    pub scan_generation: Option<String>,
    pub source_identity: Option<String>,
    pub parser_schema: Option<String>,
    pub emulator_generation: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EvidenceProvenance {
    pub source_detail: String,
    pub observed_at: Option<String>,
    pub source_identity: Option<String>,
    pub derivation_chain: Vec<String>,
    /// Stable identity of the underlying observation.  Claims with the same
    /// root are never counted as independent agreement.
    pub derivation_root: String,
}

impl EvidenceProvenance {
    pub fn new(source_detail: impl Into<String>, derivation_root: impl Into<String>) -> Self {
        Self {
            source_detail: source_detail.into(),
            derivation_root: derivation_root.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceClaim {
    pub claim_id: String,
    pub subject: String,
    pub property: ClaimProperty,
    pub value: EvidenceValue,
    pub source: EvidenceSource,
    pub strength: EvidenceStrength,
    pub polarity: ClaimPolarity,
    pub provenance: EvidenceProvenance,
    pub scope: EvidenceScope,
}

impl EvidenceClaim {
    pub fn new(
        claim_id: impl Into<String>,
        subject: impl Into<String>,
        property: ClaimProperty,
        value: EvidenceValue,
        source: EvidenceSource,
        strength: EvidenceStrength,
        provenance: EvidenceProvenance,
    ) -> Self {
        Self {
            claim_id: claim_id.into(),
            subject: subject.into(),
            property,
            value,
            source,
            strength,
            polarity: ClaimPolarity::Supports,
            provenance,
            scope: EvidenceScope::default(),
        }
    }

    pub fn no_support(mut self) -> Self {
        self.polarity = ClaimPolarity::NoSupport;
        self
    }

    pub fn contradicts(mut self) -> Self {
        self.polarity = ClaimPolarity::Contradicts;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResolutionState {
    Verified,
    Probable,
    Ambiguous,
    Conflicting,
    Unverified,
    Unsupported,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ActionSafety {
    SafeToAct,
    ReviewRequired,
    Blocked,
}

/// Stable machine-readable reasons behind a result.  Human-readable detail is
/// kept alongside it in `ResolutionResult::reasoning`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResolutionReason {
    StrongestDomainClaim,
    IndependentAgreement,
    WeakClaimPreserved,
    IncompatibleClaims,
    NoPositiveEvidence,
    NoSupportIsNotContradiction,
    ExplicitContradiction,
    StaleEvidence,
    UserNativeConflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceConflict {
    pub property: ClaimProperty,
    pub values: Vec<EvidenceValue>,
    pub claims: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissingEvidenceRequirement {
    NativeIdentity,
    MatchingDatAuthority,
    UserConfirmation,
    VerifiedBiosReadiness,
    MediaSetMember,
    RevisionEvidence,
    FreshEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionResult {
    pub subject: String,
    pub property: ClaimProperty,
    pub resolved_value: Option<EvidenceValue>,
    pub state: ResolutionState,
    pub supporting_claims: Vec<String>,
    pub conflicting_claims: Vec<String>,
    pub ignored_claims: Vec<String>,
    pub no_support_claims: Vec<String>,
    pub conflicts: Vec<EvidenceConflict>,
    pub reasons: Vec<ResolutionReason>,
    pub reasoning: Vec<String>,
    pub missing_evidence: Vec<MissingEvidenceRequirement>,
    pub action_safety: ActionSafety,
}

/// Indexed claims.  The subject/property index keeps resolution bounded by
/// the relevant evidence rather than comparing every claim with every other
/// claim.
#[derive(Debug, Clone, Default)]
pub struct EvidenceIndex {
    claims: BTreeMap<(String, ClaimProperty), Vec<EvidenceClaim>>,
}

impl EvidenceIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ingest(&mut self, claim: EvidenceClaim) {
        self.claims
            .entry((claim.subject.clone(), claim.property))
            .or_default()
            .push(claim);
    }

    pub fn from_claims(claims: impl IntoIterator<Item = EvidenceClaim>) -> Self {
        let mut index = Self::new();
        for claim in claims {
            index.ingest(claim);
        }
        index
    }

    pub fn len(&self) -> usize {
        self.claims.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }

    pub fn resolve(&self, subject: &str, property: ClaimProperty) -> ResolutionResult {
        self.claims
            .get(&(subject.to_owned(), property))
            .map_or_else(
                || resolve_empty(subject, property),
                |claims| resolve_claims(subject, property, claims),
            )
    }

    pub fn resolve_all(&self) -> Vec<ResolutionResult> {
        self.claims
            .keys()
            .map(|(subject, property)| self.resolve(subject, *property))
            .collect()
    }
}

/// Resolve one claim domain/property.  Claims for other subjects or
/// properties are ignored, which makes this safe for callers assembling a
/// shared evidence batch.
pub fn resolve_claim(claims: &[EvidenceClaim]) -> ResolutionResult {
    let Some(first) = claims.first() else {
        return resolve_empty("", ClaimProperty::Platform);
    };
    resolve_claims(&first.subject, first.property, claims)
}

/// Resolve every subject/property represented in `claims`.
pub fn resolve_identity(claims: &[EvidenceClaim]) -> Vec<ResolutionResult> {
    EvidenceIndex::from_claims(claims.iter().cloned()).resolve_all()
}

pub fn explain_resolution(result: &ResolutionResult) -> String {
    let value = result
        .resolved_value
        .as_ref()
        .map_or_else(|| "no resolved value".to_string(), EvidenceValue::canonical);
    let mut lines = vec![
        format!("Resolved {}: {value}", result.property.label()),
        format!("State: {:?}", result.state),
        format!("Action: {:?}", result.action_safety),
    ];
    lines.extend(result.reasoning.iter().cloned());
    if !result.conflicts.is_empty() {
        lines.push("Conflict: preserved incompatible claims".to_string());
    }
    if !result.missing_evidence.is_empty() {
        lines.push(format!("Missing evidence: {:?}", result.missing_evidence));
    }
    lines.join("\n")
}

/// Stable digest of a result set, useful for cache invalidation and
/// determinism tests.  It contains no file contents or secrets.
pub fn resolution_digest(results: &[ResolutionResult]) -> String {
    let mut ordered = results.to_vec();
    ordered.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.property.cmp(&b.property)));
    let bytes = serde_json::to_vec(&ordered).expect("ResolutionResult is always serializable");
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn resolve_empty(subject: &str, property: ClaimProperty) -> ResolutionResult {
    ResolutionResult {
        subject: subject.to_string(),
        property,
        resolved_value: None,
        state: ResolutionState::Unverified,
        supporting_claims: Vec::new(),
        conflicting_claims: Vec::new(),
        ignored_claims: Vec::new(),
        no_support_claims: Vec::new(),
        conflicts: Vec::new(),
        reasons: vec![ResolutionReason::NoPositiveEvidence],
        reasoning: vec![format!("No {} evidence was supplied", property.label())],
        missing_evidence: vec![requirement_for(property)],
        action_safety: ActionSafety::ReviewRequired,
    }
}

#[derive(Debug, Clone)]
struct Candidate<'a> {
    value: EvidenceValue,
    claims: Vec<&'a EvidenceClaim>,
    rank: u16,
    independent_roots: usize,
    stale: bool,
}

fn resolve_claims<'a>(
    subject: &str,
    property: ClaimProperty,
    claims: &'a [EvidenceClaim],
) -> ResolutionResult {
    let mut ordered = claims
        .iter()
        .filter(|claim| claim.subject == subject && claim.property == property)
        .collect::<Vec<_>>();
    ordered.sort_by(|a, b| claim_order(a, b));
    let mut ignored_claims = Vec::new();
    let mut no_support_claims = Vec::new();
    let mut contradiction_claims = Vec::new();
    let mut groups: BTreeMap<String, Candidate> = BTreeMap::new();

    for claim in ordered {
        if claim.property != property || claim.subject != subject {
            continue;
        }
        if claim.polarity == ClaimPolarity::NoSupport {
            no_support_claims.push(claim.claim_id.clone());
            continue;
        }
        if claim.polarity == ClaimPolarity::Contradicts {
            contradiction_claims.push(claim.claim_id.clone());
            continue;
        }
        let rank = rank_for(property, claim.source, claim.strength);
        let key = claim.value.canonical();
        let candidate = groups.entry(key).or_insert_with(|| Candidate {
            value: claim.value.clone(),
            claims: Vec::new(),
            rank,
            independent_roots: 0,
            stale: false,
        });
        candidate.rank = candidate.rank.max(rank);
        candidate.stale |= claim_is_stale(&claim);
        if !candidate
            .claims
            .iter()
            .any(|existing| existing.provenance.derivation_root == claim.provenance.derivation_root)
        {
            candidate.independent_roots += 1;
        }
        candidate.claims.push(claim);
    }

    let mut candidates = groups.into_values().collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.rank
            .cmp(&a.rank)
            .then(b.independent_roots.cmp(&a.independent_roots))
            .then(a.value.canonical().cmp(&b.value.canonical()))
    });

    let explicit_contradiction = !contradiction_claims.is_empty();
    let all_stale = !candidates.is_empty() && candidates.iter().all(|candidate| candidate.stale);
    let mut conflicts = Vec::new();
    let mut conflicting_claims = Vec::new();
    let (resolved_value, state, action_safety, supporting_claims, reasoning) = if candidates
        .is_empty()
    {
        let state = if explicit_contradiction {
            ResolutionState::Conflicting
        } else if all_stale {
            ResolutionState::Stale
        } else if no_support_claims.is_empty() {
            ResolutionState::Unverified
        } else {
            ResolutionState::Unsupported
        };
        let mut reasoning = if no_support_claims.is_empty() {
            vec![format!(
                "No positive {} claim is available",
                property.label()
            )]
        } else {
            vec!["A provider/source reported NO_SUPPORT; absence is not contradiction".to_string()]
        };
        if explicit_contradiction {
            reasoning.push("An explicit contradictory claim requires review".to_string());
            conflicts.push(EvidenceConflict {
                property,
                values: Vec::new(),
                claims: contradiction_claims.clone(),
                reason: "explicit contradiction has no positive value to resolve".to_string(),
            });
        } else if state == ResolutionState::Unsupported {
            reasoning
                .push("The available authority does not prove a negative identity".to_string());
        }
        (None, state, safety_for(state), Vec::new(), reasoning)
    } else {
        let best = &candidates[0];
        let tied = candidates
            .iter()
            .skip(1)
            .any(|candidate| candidate.rank == best.rank);
        let user_native_conflict = candidates.iter().skip(1).any(|candidate| {
            candidate.rank != best.rank
                && candidate
                    .claims
                    .iter()
                    .any(|claim| claim.source == EvidenceSource::UserConfirmed)
                && best
                    .claims
                    .iter()
                    .any(|claim| claim.source == EvidenceSource::NativeVerified)
        }) || (best
            .claims
            .iter()
            .any(|claim| claim.source == EvidenceSource::UserConfirmed)
            && candidates.iter().skip(1).any(|candidate| {
                candidate
                    .claims
                    .iter()
                    .any(|claim| claim.source == EvidenceSource::NativeVerified)
            }));
        let conflicting = tied || explicit_contradiction || user_native_conflict;
        for candidate in candidates.iter().skip(1) {
            conflicting_claims.extend(candidate.claims.iter().map(|claim| claim.claim_id.clone()));
        }
        conflicting_claims.extend(contradiction_claims.iter().cloned());
        conflicting_claims.sort();
        conflicting_claims.dedup();
        if candidates.len() > 1 || explicit_contradiction {
            let mut values = candidates
                .iter()
                .map(|candidate| candidate.value.clone())
                .collect::<Vec<_>>();
            values.sort();
            values.dedup();
            conflicts.push(EvidenceConflict {
                property,
                values,
                claims: conflicting_claims.clone(),
                reason: if conflicting {
                    "incompatible claims require review".to_string()
                } else {
                    "a weaker claim conflicts with the selected stronger claim".to_string()
                },
            });
        }
        let ambiguous = tied && best.rank < 70 && !explicit_contradiction;
        let state = if all_stale {
            ResolutionState::Stale
        } else if conflicting {
            if ambiguous {
                ResolutionState::Ambiguous
            } else {
                ResolutionState::Conflicting
            }
        } else if best.independent_roots > 1
            || best.claims.iter().any(|claim| {
                matches!(
                    claim.source,
                    EvidenceSource::NativeVerified
                        | EvidenceSource::AuthorityVerified
                        | EvidenceSource::ContentVerified
                ) && matches!(
                    claim.strength,
                    EvidenceStrength::Strong | EvidenceStrength::Verified
                )
            })
        {
            ResolutionState::Verified
        } else {
            ResolutionState::Probable
        };
        let mut reasoning = vec![format!(
            "Selected the strongest {} claim using domain-specific precedence",
            property.label()
        )];
        if best.independent_roots > 1 {
            reasoning.push(format!(
                "{} independent observation roots agree",
                best.independent_roots
            ));
        }
        if !conflicts.is_empty() {
            reasoning.push(
                "Weaker or incompatible claims remain visible in the conflict record".to_string(),
            );
        }
        (
            Some(best.value.clone()),
            state,
            safety_for(state),
            best.claims
                .iter()
                .map(|claim| claim.claim_id.clone())
                .collect(),
            reasoning,
        )
    };

    let all_ids = claims
        .iter()
        .map(|claim| claim.claim_id.clone())
        .collect::<BTreeSet<_>>();
    let used = supporting_claims
        .iter()
        .chain(conflicting_claims.iter())
        .chain(no_support_claims.iter())
        .chain(contradiction_claims.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    ignored_claims.extend(all_ids.difference(&used).cloned());
    ignored_claims.sort();
    let mut result = ResolutionResult {
        subject: subject.to_string(),
        property,
        resolved_value,
        state,
        supporting_claims,
        conflicting_claims,
        ignored_claims,
        no_support_claims,
        conflicts,
        reasons: Vec::new(),
        reasoning,
        missing_evidence: Vec::new(),
        action_safety,
    };
    if result.supporting_claims.is_empty() {
        result.reasons.push(if result.no_support_claims.is_empty() {
            ResolutionReason::NoPositiveEvidence
        } else {
            ResolutionReason::NoSupportIsNotContradiction
        });
    } else {
        result.reasons.push(ResolutionReason::StrongestDomainClaim);
        if result
            .reasoning
            .iter()
            .any(|line| line.contains("independent observation"))
        {
            result.reasons.push(ResolutionReason::IndependentAgreement);
        }
        if !result.conflicts.is_empty() {
            result.reasons.push(ResolutionReason::WeakClaimPreserved);
        }
    }
    if result.state == ResolutionState::Ambiguous {
        result.reasons.push(ResolutionReason::IncompatibleClaims);
    }
    if result.state == ResolutionState::Conflicting {
        result.reasons.push(
            if result.conflicting_claims.iter().any(|claim_id| {
                claims.iter().any(|claim| {
                    claim.claim_id == *claim_id && claim.source == EvidenceSource::UserConfirmed
                })
            }) {
                ResolutionReason::UserNativeConflict
            } else {
                ResolutionReason::IncompatibleClaims
            },
        );
    }
    if result.state == ResolutionState::Stale {
        result.reasons.push(ResolutionReason::StaleEvidence);
    }
    if !contradiction_claims.is_empty() {
        result.reasons.push(ResolutionReason::ExplicitContradiction);
    }
    if matches!(
        result.state,
        ResolutionState::Unverified | ResolutionState::Unsupported | ResolutionState::Stale
    ) {
        result.missing_evidence.push(requirement_for(property));
    }
    result
}

fn claim_order(a: &EvidenceClaim, b: &EvidenceClaim) -> std::cmp::Ordering {
    a.subject
        .cmp(&b.subject)
        .then(a.property.cmp(&b.property))
        .then(a.value.canonical().cmp(&b.value.canonical()))
        .then(
            a.provenance
                .derivation_root
                .cmp(&b.provenance.derivation_root),
        )
        .then(a.claim_id.cmp(&b.claim_id))
}

fn claim_is_stale(claim: &EvidenceClaim) -> bool {
    matches!(claim.scope.authority_state, Some(AuthorityState::Stale))
}

fn rank_for(property: ClaimProperty, source: EvidenceSource, strength: EvidenceStrength) -> u16 {
    let source_rank = match property {
        ClaimProperty::Region | ClaimProperty::Revision => match source {
            EvidenceSource::NativeVerified => 100,
            EvidenceSource::ContentVerified => 95,
            EvidenceSource::AuthorityVerified => 85,
            EvidenceSource::UserConfirmed => 80,
            EvidenceSource::StructuredMetadata => 60,
            EvidenceSource::FilenameStrong => 40,
            EvidenceSource::DirectoryContext => 30,
            EvidenceSource::FuzzyInference => 10,
        },
        ClaimProperty::Platform => match source {
            EvidenceSource::NativeVerified => 100,
            EvidenceSource::ContentVerified => 95,
            EvidenceSource::AuthorityVerified => 90,
            EvidenceSource::UserConfirmed => 80,
            EvidenceSource::StructuredMetadata => 65,
            EvidenceSource::FilenameStrong => 45,
            EvidenceSource::DirectoryContext => 30,
            EvidenceSource::FuzzyInference => 10,
        },
        ClaimProperty::GameIdentity => match source {
            EvidenceSource::NativeVerified => 100,
            EvidenceSource::AuthorityVerified => 95,
            EvidenceSource::ContentVerified => 90,
            EvidenceSource::UserConfirmed => 80,
            EvidenceSource::StructuredMetadata => 65,
            EvidenceSource::FilenameStrong => 45,
            EvidenceSource::DirectoryContext => 25,
            EvidenceSource::FuzzyInference => 10,
        },
        _ => match source {
            EvidenceSource::NativeVerified => 100,
            EvidenceSource::AuthorityVerified => 95,
            EvidenceSource::ContentVerified => 90,
            EvidenceSource::UserConfirmed => 85,
            EvidenceSource::StructuredMetadata => 70,
            EvidenceSource::FilenameStrong => 50,
            EvidenceSource::DirectoryContext => 30,
            EvidenceSource::FuzzyInference => 10,
        },
    };
    source_rank
        + match strength {
            EvidenceStrength::Verified => 4,
            EvidenceStrength::Strong => 3,
            EvidenceStrength::Corroborated => 2,
            EvidenceStrength::Weak => 1,
        }
}

fn safety_for(state: ResolutionState) -> ActionSafety {
    match state {
        ResolutionState::Verified => ActionSafety::SafeToAct,
        ResolutionState::Conflicting => ActionSafety::Blocked,
        ResolutionState::Probable
        | ResolutionState::Ambiguous
        | ResolutionState::Unverified
        | ResolutionState::Unsupported
        | ResolutionState::Stale => ActionSafety::ReviewRequired,
    }
}

fn requirement_for(property: ClaimProperty) -> MissingEvidenceRequirement {
    match property {
        ClaimProperty::Platform | ClaimProperty::GameIdentity => {
            MissingEvidenceRequirement::NativeIdentity
        }
        ClaimProperty::Region | ClaimProperty::Revision => {
            MissingEvidenceRequirement::RevisionEvidence
        }
        ClaimProperty::MediaSetMembership
        | ClaimProperty::MediaOrdinal
        | ClaimProperty::MediaSide
        | ClaimProperty::MediaRole
        | ClaimProperty::ExpectedMediaCount => MissingEvidenceRequirement::MediaSetMember,
        ClaimProperty::BiosDependency | ClaimProperty::EmulatorProfileCompatibility => {
            MissingEvidenceRequirement::VerifiedBiosReadiness
        }
        ClaimProperty::Language | ClaimProperty::LaunchTarget => {
            MissingEvidenceRequirement::UserConfirmation
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(
        id: &str,
        property: ClaimProperty,
        value: EvidenceValue,
        source: EvidenceSource,
        root: &str,
    ) -> EvidenceClaim {
        EvidenceClaim::new(
            id,
            "game-1",
            property,
            value,
            source,
            EvidenceStrength::Verified,
            EvidenceProvenance::new(source.label(), root),
        )
    }

    #[test]
    fn independent_agreement_is_verified_and_explained() {
        let result = resolve_claim(&[
            claim(
                "native",
                ClaimProperty::Platform,
                EvidenceValue::Platform("PSX".into()),
                EvidenceSource::NativeVerified,
                "header",
            ),
            claim(
                "dat",
                ClaimProperty::Platform,
                EvidenceValue::Platform("PSX".into()),
                EvidenceSource::AuthorityVerified,
                "dat-entry",
            ),
        ]);
        assert_eq!(result.state, ResolutionState::Verified);
        assert_eq!(result.action_safety, ActionSafety::SafeToAct);
        assert!(
            result
                .reasoning
                .iter()
                .any(|line| line.contains("2 independent"))
        );
    }

    #[test]
    fn derived_evidence_is_not_double_counted() {
        let result = resolve_claim(&[
            claim(
                "filename",
                ClaimProperty::Region,
                EvidenceValue::Region("USA".into()),
                EvidenceSource::FilenameStrong,
                "same-file",
            ),
            claim(
                "tosec",
                ClaimProperty::Region,
                EvidenceValue::Region("USA".into()),
                EvidenceSource::StructuredMetadata,
                "same-file",
            ),
        ]);
        assert_eq!(result.state, ResolutionState::Probable);
        assert!(
            result
                .reasoning
                .iter()
                .all(|line| !line.contains("2 independent"))
        );
    }

    #[test]
    fn native_beats_weak_filename_but_conflict_is_preserved() {
        let result = resolve_claim(&[
            claim(
                "native",
                ClaimProperty::Region,
                EvidenceValue::Region("Europe".into()),
                EvidenceSource::NativeVerified,
                "header",
            ),
            claim(
                "filename",
                ClaimProperty::Region,
                EvidenceValue::Region("USA".into()),
                EvidenceSource::FilenameStrong,
                "name",
            ),
        ]);
        assert_eq!(
            result.resolved_value,
            Some(EvidenceValue::Region("Europe".into()))
        );
        assert_eq!(result.state, ResolutionState::Verified);
        assert_eq!(result.action_safety, ActionSafety::SafeToAct);
        assert_eq!(result.conflicts.len(), 1);
    }

    #[test]
    fn equal_strength_is_conflicting_and_fails_closed() {
        let result = resolve_claim(&[
            claim(
                "a",
                ClaimProperty::Platform,
                EvidenceValue::Platform("PSX".into()),
                EvidenceSource::NativeVerified,
                "a",
            ),
            claim(
                "b",
                ClaimProperty::Platform,
                EvidenceValue::Platform("Saturn".into()),
                EvidenceSource::NativeVerified,
                "b",
            ),
        ]);
        assert_eq!(result.state, ResolutionState::Conflicting);
        assert_eq!(result.action_safety, ActionSafety::Blocked);
    }

    #[test]
    fn no_support_is_not_contradiction() {
        let result = resolve_claim(&[claim(
            "tpdb-miss",
            ClaimProperty::GameIdentity,
            EvidenceValue::GameIdentity {
                namespace: "tpdb".into(),
                value: "none".into(),
            },
            EvidenceSource::StructuredMetadata,
            "tpdb-query",
        )
        .no_support()]);
        assert_eq!(result.state, ResolutionState::Unsupported);
        assert!(result.conflicts.is_empty());
        assert!(
            result
                .reasoning
                .iter()
                .any(|line| line.contains("not contradiction"))
        );
    }

    #[test]
    fn stale_authority_does_not_produce_safe_result() {
        let mut stale = claim(
            "old-dat",
            ClaimProperty::GameIdentity,
            EvidenceValue::GameIdentity {
                namespace: "dat".into(),
                value: "rev-a".into(),
            },
            EvidenceSource::AuthorityVerified,
            "old-dat",
        );
        stale.scope.authority_state = Some(AuthorityState::Stale);
        let result = resolve_claim(&[stale]);
        assert_eq!(result.state, ResolutionState::Stale);
        assert_eq!(result.action_safety, ActionSafety::ReviewRequired);
    }

    #[test]
    fn user_confirmation_vs_native_requires_review() {
        let result = resolve_claim(&[
            claim(
                "human",
                ClaimProperty::GameIdentity,
                EvidenceValue::GameIdentity {
                    namespace: "user".into(),
                    value: "A".into(),
                },
                EvidenceSource::UserConfirmed,
                "user",
            ),
            claim(
                "native",
                ClaimProperty::GameIdentity,
                EvidenceValue::GameIdentity {
                    namespace: "native".into(),
                    value: "B".into(),
                },
                EvidenceSource::NativeVerified,
                "native",
            ),
        ]);
        assert_eq!(result.state, ResolutionState::Conflicting);
        assert_eq!(result.action_safety, ActionSafety::Blocked);
    }

    #[test]
    fn media_topology_and_readiness_are_separate_domains() {
        let results = resolve_identity(&[
            claim(
                "topology",
                ClaimProperty::MediaOrdinal,
                EvidenceValue::MediaOrdinal(2),
                EvidenceSource::StructuredMetadata,
                "media-set",
            ),
            claim(
                "bios",
                ClaimProperty::BiosDependency,
                EvidenceValue::BiosDependency("scph5502.bin".into()),
                EvidenceSource::StructuredMetadata,
                "duckstation",
            ),
        ]);
        assert_eq!(results.len(), 2);
        assert!(
            results
                .iter()
                .all(|result| result.property != ClaimProperty::Platform)
        );
    }

    #[test]
    fn insertion_order_does_not_change_digest_or_result() {
        let a = vec![
            claim(
                "b",
                ClaimProperty::Platform,
                EvidenceValue::Platform("PSX".into()),
                EvidenceSource::DirectoryContext,
                "dir",
            ),
            claim(
                "a",
                ClaimProperty::Platform,
                EvidenceValue::Platform("PSX".into()),
                EvidenceSource::FilenameStrong,
                "file",
            ),
        ];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(
            resolution_digest(&resolve_identity(&a)),
            resolution_digest(&resolve_identity(&b))
        );
        assert_eq!(resolve_claim(&a), resolve_claim(&b));
    }

    #[test]
    fn explicit_contradiction_is_not_treated_as_absence() {
        let result = resolve_claim(&[claim(
            "bad",
            ClaimProperty::Platform,
            EvidenceValue::Platform("Saturn".into()),
            EvidenceSource::NativeVerified,
            "header",
        )
        .contradicts()]);
        assert_eq!(result.state, ResolutionState::Conflicting);
        assert_eq!(result.action_safety, ActionSafety::Blocked);
        assert!(!result.conflicts.is_empty());
    }

    #[test]
    fn weak_equal_claims_are_ambiguous_not_authoritative_conflict() {
        let result = resolve_claim(&[
            claim(
                "a",
                ClaimProperty::Platform,
                EvidenceValue::Platform("PSX".into()),
                EvidenceSource::FilenameStrong,
                "a",
            ),
            claim(
                "b",
                ClaimProperty::Platform,
                EvidenceValue::Platform("Saturn".into()),
                EvidenceSource::FilenameStrong,
                "b",
            ),
        ]);
        assert_eq!(result.state, ResolutionState::Ambiguous);
        assert_eq!(result.action_safety, ActionSafety::ReviewRequired);
    }
}
