//! Typed selected-file DAT evidence. Matching happens once in the worker;
//! fusion and downstream handoff use the captured result without I/O.

use archivefs_core::dat::audit::AuditVerdict;
use archivefs_core::dat::identity::{
    DatPlatformIdentity, identify_dat_source, resolve_dat_platform_identity,
};
use archivefs_core::dat::index::DatRomRef;
use archivefs_core::dat::model::ChecksumAlgorithm;
use archivefs_core::identity_source::hashing::LocalHashes;
use archivefs_core::identity_source::no_intro::convert::{
    lookup_no_intro, observations_from_no_intro_matches,
};
use archivefs_core::identity_source::no_intro::{ImportedNoIntroSource, NoIntroSourceLabel};
use archivefs_core::platform_evidence_fusion::combined_identity::combine_identity;
use archivefs_core::platform_evidence_fusion::dat_hash_representation::RepresentationMatchOutcome;
use archivefs_core::platform_evidence_fusion::evidence_lineage::{
    EvidenceObservation, Representation, SourceArtifactIdentity, SourceFamily,
};
use archivefs_core::platform_evidence_fusion::identity_orchestrator::IdentityResult;

/// A source's exact SHA-1 hit. Catalogue identity alone is never file authority.
/// Fields are private so display strings cannot be promoted into verified facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedSelectedDat {
    pub(super) registry_id: Option<String>,
    pub(super) artifact: SourceArtifactIdentity,
    pub(super) catalogue: DatPlatformIdentity,
    sha1: String,
    matches: Vec<DatRomRef>,
    observations: Vec<EvidenceObservation>,
}

impl VerifiedSelectedDat {
    pub(super) fn lookup(
        label: Option<&NoIntroSourceLabel>,
        source: &ImportedNoIntroSource,
        hashes: &LocalHashes,
    ) -> Option<Self> {
        let matches = lookup_no_intro(&source.index, ChecksumAlgorithm::Sha1, &hashes.sha1);
        if matches.is_empty() {
            return None;
        }
        Some(Self {
            registry_id: label.map(|label| label.source_id.clone()),
            artifact: SourceArtifactIdentity {
                source_family: SourceFamily::NoIntro,
                upstream_version: source.upstream_version.clone(),
                artifact_sha256: Some(source.artifact_sha256.clone()),
                artifact_name: Some(source.artifact_name.clone()),
            },
            catalogue: identify_dat_source(&source.dat),
            sha1: hashes.sha1.clone(),
            matches: matches.to_vec(),
            observations: observations_from_no_intro_matches(
                source,
                ChecksumAlgorithm::Sha1,
                &hashes.sha1,
                Representation::PhysicalFile,
            ),
        })
    }

    pub(super) fn observations(&self) -> &[EvidenceObservation] {
        &self.observations
    }

    pub(super) fn matches_hashes(&self, hashes: &LocalHashes) -> bool {
        self.sha1 == hashes.sha1
    }
}

/// Original identity retained verbatim, separately from DAT-derived facts and
/// the authoritative fused `SelectedEvidenceReport::identity_result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedDatIdentity {
    pub(crate) base: IdentityResult,
    pub(crate) sources: Vec<VerifiedSelectedDat>,
}

pub(super) fn has_verified_dat_identity(identity: &IdentityResult) -> bool {
    matches!(identity.representation_match.as_ref(),
        Some(RepresentationMatchOutcome::PhysicalOnly { verdict }
            | RepresentationMatchOutcome::NormalizedOnly { verdict }
            | RepresentationMatchOutcome::BothAgree { verdict, .. }) if verdict.is_confident())
}

/// Clone the existing identity and change only the DAT/combined lanes. Content,
/// archive-set evidence, representation provenance and pre-existing caveats stay
/// intact. Strong catalogue disagreement is never resolved by source priority.
pub(super) fn fuse(base: &IdentityResult, sources: &[VerifiedSelectedDat]) -> IdentityResult {
    if sources.is_empty() {
        return base.clone();
    }
    let mut result = base.clone();
    let identities = base
        .dat
        .iter()
        .chain(sources.iter().map(|source| &source.catalogue));
    let mut evidence = Vec::new();
    for identity in identities {
        match identity {
            DatPlatformIdentity::Resolved {
                evidence: facts, ..
            }
            | DatPlatformIdentity::Ambiguous { candidates: facts } => {
                evidence.extend(facts.clone())
            }
            DatPlatformIdentity::Unknown => {}
        }
    }
    let dat = resolve_dat_platform_identity(evidence);
    let combined = combine_identity(&base.content, &dat);
    if combined.relationship.is_conflict() {
        const CAVEAT: &str = "content and DAT-source identity disagree on the platform";
        if !result.caveats.contains(&CAVEAT) {
            result.caveats.push(CAVEAT);
        }
    }
    result.dat = Some(dat);
    result.combined = Some(combined);

    // Retain an existing normalized/physical verdict, including conflicts. A
    // new physical observation must not erase an already verified axis.
    if !has_verified_dat_identity(base)
        && matches!(
            base.representation_match,
            None | Some(RepresentationMatchOutcome::NoMatch)
        )
    {
        let rows: Vec<&DatRomRef> = sources.iter().flat_map(|source| &source.matches).collect();
        let verdict = match rows.as_slice() {
            [row] => AuditVerdict::Exact {
                game_name: row.game_name.clone(),
                rom_name: row.rom_name.clone(),
                algorithm: "SHA-1",
            },
            _ => AuditVerdict::ExactMultipleCandidates {
                algorithm: "SHA-1",
                count: rows.len(),
                game_names: {
                    let mut names: Vec<_> = rows.iter().map(|row| row.game_name.clone()).collect();
                    names.sort();
                    names
                },
            },
        };
        result.representation_match = Some(RepresentationMatchOutcome::PhysicalOnly { verdict });
    }
    result
}

#[cfg(test)]
mod tests;
