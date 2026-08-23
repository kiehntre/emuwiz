//! Conservative MAMERedump CHD evidence, without a fabricated Redump track
//! crosswalk.
//!
//! A Redump track SHA-1 and a CHD combined SHA-1 describe different byte
//! domains.  This module therefore has two deliberately separate jobs:
//!
//! - turn an *already trusted* MAMERedump DAT `<disk sha1>` hit for a CHD
//!   header's `combined_sha1` into `LogicalChd` evidence derived from Redump;
//! - explicitly refuse a Redump-track-set -> CHD-combined-SHA1 correlation
//!   until a future caller supplies a reviewed mapping artifact.
//!
//! It never receives a filename or cue path, and it never reads/decompresses
//! CHD content.  The only lookup is `DatDiskIndex::lookup_disk_sha1` using the
//! bounded-header combined SHA-1.  Raw SHA-1, parent SHA-1, and a physical CHD
//! file hash consequently have no route into this bridge's identity lookup.

use crate::chd_identity::ChdIdentityObservation;
use crate::dat::index::{DatDiskIndex, parse_disk_sha1};
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope, LineageRelation,
    Provenance, Representation, SourceArtifactIdentity, SourceFamily,
};

/// A checked source declaration for MAMERedump disk metadata.  Construction
/// requires an explicit, externally verified family classification; artifact
/// names are retained for provenance only and have no classification or
/// crosswalk authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MameRedumpSource {
    source_artifact: SourceArtifactIdentity,
}

/// Refusal from [`MameRedumpSource::from_explicit_classification`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MameRedumpSourceError {
    NotMameRedump { found: SourceFamily },
}

impl MameRedumpSource {
    /// Creates a MAMERedump source only from an explicit trusted source-family
    /// classification.  This bridge intentionally does not inspect an
    /// artifact filename, a CHD filename, a shortname, or description text.
    pub fn from_explicit_classification(
        source_artifact: SourceArtifactIdentity,
    ) -> Result<Self, MameRedumpSourceError> {
        if source_artifact.source_family != SourceFamily::MAMERedump {
            return Err(MameRedumpSourceError::NotMameRedump {
                found: source_artifact.source_family,
            });
        }
        Ok(Self { source_artifact })
    }

    pub fn source_artifact(&self) -> &SourceArtifactIdentity {
        &self.source_artifact
    }
}

/// The known relationship for MAMERedump.  This narrow helper keeps callers
/// from restating the lineage rule as an ad-hoc source-name comparison.
pub fn classify_mame_redump_lineage(source: SourceFamily) -> Option<LineageRelation> {
    (source == SourceFamily::MAMERedump).then_some(LineageRelation::DerivedFrom)
}

fn provenance(source: &MameRedumpSource) -> Provenance {
    Provenance {
        channel: EvidenceChannel::LocalMame,
        upstream_source: SourceFamily::MAMERedump,
        upstream_version: source.source_artifact.upstream_version.clone(),
        source_artifact: Some(source.source_artifact.clone()),
        imported_at_unix: None,
        retrieved_at_unix: None,
        generator_version: None,
        lineage: LineageRelation::DerivedFrom,
        representation: Representation::LogicalChd,
    }
}

/// Matches only `identity.combined_sha1` against a trusted MAMERedump disk
/// index and returns collision-preserving logical-CHD observations.
///
/// A child/differencing CHD stays a `LogicalChd` observation in the same
/// MAMERedump-derived lineage; its `parent_sha1` is deliberately not queried
/// and cannot create a second physical-dump identity.
pub fn observations_from_mame_redump_chd_identity(
    source: &MameRedumpSource,
    disk_index: &DatDiskIndex,
    identity: &ChdIdentityObservation,
) -> Vec<EvidenceObservation> {
    let combined_sha1 = identity.combined_sha1_hex();
    let Some(combined_sha1) = parse_disk_sha1(&combined_sha1) else {
        return Vec::new();
    };

    disk_index
        .lookup_disk_sha1(&combined_sha1)
        .iter()
        .map(|disk| EvidenceObservation {
            provenance: provenance(source),
            claim: ClaimType::ExactLogicalDiscMatch,
            claim_strength: ClaimStrength::Strong,
            identity_scope: IdentityScope::DumpIdentity,
            hash_or_value: Some(combined_sha1.clone()),
            platform_candidate: None,
            release_candidate: Some(disk.game_name.clone()),
            notes: Some("MAMERedump logical CHD match via indexed disk SHA-1".to_string()),
        })
        .collect()
}

/// Explicit fail-closed seam for the absent Redump-track-set -> CHD-combined
/// mapping.  Track hashes are intentionally not accepted as equality evidence
/// for `combined_sha1`; filename and cue metadata are not part of this API.
///
/// A future implementation may return a vetted crosswalk only when supplied
/// a reviewed mapping artifact that directly links the *entire ordered track
/// set* to a CHD combined SHA-1.  Decoding sectors or hashing full CHDs is out
/// of scope for this bounded-header bridge.
pub fn correlate_redump_track_set_to_chd_combined_sha1(
    _redump_track_sha1s: &[String],
    _chd_combined_sha1: &str,
) -> Option<()> {
    None
}

#[cfg(test)]
mod tests;
