//! Conversion of a valid local `.slave` artifact into attributed evidence.

use super::slave::SlaveArtifact;
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope, LineageRelation,
    Provenance, Representation, SourceArtifactIdentity, SourceFamily,
};

fn provenance(slave: &SlaveArtifact) -> Provenance {
    Provenance {
        channel: EvidenceChannel::LocalWHDLoad,
        upstream_source: SourceFamily::WHDLoad,
        // `ws_Version` is the required WHDLoad runtime version, not a
        // catalogue, slave-release, or upstream-source version.
        upstream_version: None,
        source_artifact: Some(SourceArtifactIdentity {
            source_family: SourceFamily::WHDLoad,
            upstream_version: None,
            artifact_sha256: Some(slave.hashes.sha256.clone()),
            artifact_name: Some(slave.name.clone()),
        }),
        imported_at_unix: None,
        retrieved_at_unix: None,
        generator_version: None,
        lineage: LineageRelation::Independent,
        representation: Representation::WHDLoadSlave,
    }
}

/// A valid WHDLoad slave is strong structural evidence of Amiga software.
/// It is not an exact game identity and does not derive identity from a path.
pub fn structural_slave_observation(slave: &SlaveArtifact) -> EvidenceObservation {
    EvidenceObservation {
        provenance: provenance(slave),
        claim: ClaimType::PlatformCandidate,
        claim_strength: ClaimStrength::Strong,
        identity_scope: IdentityScope::PlatformIdentity,
        hash_or_value: None,
        platform_candidate: Some("Amiga".to_string()),
        release_candidate: None,
        notes: Some(format!(
            "validated WHDLoad slave runtime v{}",
            slave.parsed.runtime_version
        )),
    }
}

/// Identity of the DAT catalogue whose own hash index matched a `.slave`'s
/// whole-file SHA-1, so an [`exact_slave_match_observation`] can be
/// attributed to the ecosystem/source/revision it actually came from
/// instead of only to the local slave bytes. Built from the same generic
/// lineage vocabulary every other DAT source uses - there is no
/// Amiga-specific provenance type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedSlaveDatSource {
    /// The DAT's preservation corpus (`NoIntro`, `TOSEC`, `Retroplay`, ...).
    pub source_family: SourceFamily,
    /// The catalogue revision/version, verbatim, when the DAT records one.
    pub upstream_version: Option<String>,
    /// The delivered DAT artifact's own identity (name + sha256), when known.
    pub artifact: Option<SourceArtifactIdentity>,
}

/// For a caller that has already matched the whole `.slave` SHA-1 against a
/// known slave catalogue. Package/HDF/LHA hashes must never call this helper.
///
/// `dat_source` is `Some` when the match came from a real DAT catalogue's
/// own hash index: the observation is then attributed to that catalogue
/// (channel [`EvidenceChannel::LocalDat`], its source family, and its
/// recorded revision/artifact) rather than to the local slave bytes, while
/// the matched hash value stays on `hash_or_value` as the evidence used.
/// `None` keeps the slave-attributed provenance (a caller that matched
/// against something other than an imported DAT).
pub fn exact_slave_match_observation(
    slave: &SlaveArtifact,
    release_candidate: Option<String>,
    dat_source: Option<&MatchedSlaveDatSource>,
) -> EvidenceObservation {
    let provenance = match dat_source {
        None => provenance(slave),
        Some(source) => Provenance {
            channel: EvidenceChannel::LocalDat,
            upstream_source: source.source_family,
            upstream_version: source.upstream_version.clone(),
            source_artifact: source.artifact.clone(),
            imported_at_unix: None,
            retrieved_at_unix: None,
            generator_version: None,
            // A DAT catalogue is a genuinely separate preservation lane
            // from the local slave's own structure.
            lineage: LineageRelation::Independent,
            representation: Representation::WHDLoadSlave,
        },
    };
    let notes = match dat_source.and_then(|source| {
        source
            .artifact
            .as_ref()
            .and_then(|artifact| artifact.artifact_name.clone())
    }) {
        Some(name) => format!("exact whole-.slave SHA-1 match in {name}"),
        None => "exact whole-.slave SHA-1 match".to_string(),
    };
    EvidenceObservation {
        provenance,
        claim: ClaimType::ExactSlaveMatch,
        claim_strength: ClaimStrength::Strong,
        identity_scope: IdentityScope::DumpIdentity,
        // The whole-file SHA-1 that produced the match - the hash value and
        // (implicitly) the algorithm actually used.
        hash_or_value: Some(slave.hashes.sha1.clone()),
        platform_candidate: Some("Amiga".to_string()),
        release_candidate,
        notes: Some(notes),
    }
}
