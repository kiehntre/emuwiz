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

/// For a caller that has already matched the whole `.slave` SHA-1 against a
/// known slave catalogue. Package/HDF/LHA hashes must never call this helper.
pub fn exact_slave_match_observation(
    slave: &SlaveArtifact,
    release_candidate: Option<String>,
) -> EvidenceObservation {
    EvidenceObservation {
        provenance: provenance(slave),
        claim: ClaimType::ExactSlaveMatch,
        claim_strength: ClaimStrength::Strong,
        identity_scope: IdentityScope::DumpIdentity,
        hash_or_value: Some(slave.hashes.sha1.clone()),
        platform_candidate: Some("Amiga".to_string()),
        release_candidate,
        notes: Some("exact whole-.slave SHA-1 match".to_string()),
    }
}
