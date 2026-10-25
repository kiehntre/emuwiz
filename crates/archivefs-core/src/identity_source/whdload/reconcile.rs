//! Production join: reconcile already-parsed WHDLoad `.slave` artifacts
//! against EmuWiz's existing generic DAT hash index.
//!
//! This is the one place a discovered WHDLoad install's verified slave
//! evidence enters the ordinary identity pipeline. It adds no parser, no
//! Amiga-specific catalogue, and no second matcher: every lookup goes
//! through the shared, collision-preserving [`DatIndex::lookup_sha1`], and
//! every output is an ordinary lineage [`EvidenceObservation`] built by the
//! existing [`structural_slave_observation`] / [`exact_slave_match_observation`]
//! helpers, ready for [`crate::platform_evidence_fusion::evidence_lineage::merge_evidence`].
//!
//! # Authority
//!
//! - A structurally valid slave is *always* strong structural Amiga
//!   evidence ([`structural_slave_observation`]).
//! - An exact whole-`.slave` SHA-1 hit in a DAT is exact dump identity,
//!   attributed to that DAT ([`exact_slave_match_observation`] with a
//!   [`MatchedSlaveDatSource`]).
//! - A filename such as `Game_v1.2_0017.slave` never becomes identity: the
//!   release name only ever comes from a matched DAT entry.
//!
//! # Fail-closed
//!
//! - no DAT context (unavailable / not imported) -> structural evidence only;
//! - valid slave, no hash hit -> structural evidence only;
//! - one slave whose hash hits more than one distinct release -> no exact
//!   observation for it, and the whole result is marked ambiguous;
//! - several slaves that resolve to different releases -> every exact
//!   observation is kept for review, and the result is marked ambiguous;
//! - several slaves that agree on one release -> corroborating exact
//!   observations, not a conflict;
//! - an unmatched extra slave alongside a matched one -> structural only for
//!   the extra; it never removes the exact observation.

use std::collections::BTreeSet;

use super::convert::{
    MatchedSlaveDatSource, exact_slave_match_observation, structural_slave_observation,
};
use super::slave::SlaveArtifact;
use crate::dat::index::DatIndex;
use crate::platform_evidence_fusion::evidence_lineage::{
    EvidenceObservation, SourceArtifactIdentity, SourceFamily,
};

/// The generic DAT context a slave's whole-file SHA-1 is reconciled
/// against. `index` is the existing shared hash index for one imported DAT
/// source; the remaining fields carry that source's own identity so an
/// exact match can be attributed to it (see [`MatchedSlaveDatSource`]).
#[derive(Debug, Clone, Copy)]
pub struct WhdloadDatContext<'a> {
    pub index: &'a DatIndex,
    pub source_family: SourceFamily,
    pub upstream_version: Option<&'a str>,
    pub source_artifact: Option<&'a SourceArtifactIdentity>,
}

impl WhdloadDatContext<'_> {
    fn matched_source(&self) -> MatchedSlaveDatSource {
        MatchedSlaveDatSource {
            source_family: self.source_family,
            upstream_version: self.upstream_version.map(str::to_string),
            artifact: self.source_artifact.cloned(),
        }
    }
}

/// How one parsed slave resolved against the DAT context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhdloadSlaveMatch {
    /// Structurally valid, but no exact catalogue identity (no context, or
    /// no hash hit).
    StructuralOnly,
    /// Exactly one distinct release matched this slave's SHA-1.
    Exact,
    /// This slave's SHA-1 hit more than one distinct release - fail-closed,
    /// no exact identity is asserted for it.
    ExactAmbiguous,
}

/// One parsed slave's reconciliation outcome, in plain terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhdloadSlaveIdentity {
    pub slave_name: String,
    /// Lower-cased whole-file SHA-1 that was looked up.
    pub sha1: String,
    /// The matched release name - always from the DAT entry, never the
    /// filename. `None` unless `outcome` is [`WhdloadSlaveMatch::Exact`].
    pub matched_release: Option<String>,
    pub outcome: WhdloadSlaveMatch,
}

/// The result of reconciling a WHDLoad install's slaves against one DAT
/// context.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WhdloadReconciliation {
    /// Lineage observations for the whole install: one structural
    /// observation per valid slave, plus one exact observation per slave
    /// that resolved to exactly one release. Ready for `merge_evidence`.
    pub observations: Vec<EvidenceObservation>,
    /// `true` when a slave hit multiple releases, or the install's slaves
    /// disagree on the release - a consumer must treat identity as
    /// Ambiguous / NeedsReview.
    pub ambiguous: bool,
    /// Per-slave outcome, in input order.
    pub slaves: Vec<WhdloadSlaveIdentity>,
}

impl WhdloadReconciliation {
    /// The single agreed exact release, when the install resolves to one
    /// (one or more slaves, all agreeing, none ambiguous). `None` for a
    /// structural-only or ambiguous install.
    pub fn agreed_release(&self) -> Option<&str> {
        if self.ambiguous {
            return None;
        }
        let mut releases = self
            .slaves
            .iter()
            .filter_map(|slave| slave.matched_release.as_deref());
        let first = releases.next()?;
        releases.all(|other| other == first).then_some(first)
    }
}

/// Reconcile `slaves` (already parsed and whole-file hashed - this never
/// reads or re-hashes bytes) against `dat`.
///
/// `dat` is `None` when no catalogue is available; every slave is then
/// structural-only, which is a legitimate fail-closed state, not an error.
pub fn reconcile_whdload_slaves(
    slaves: &[SlaveArtifact],
    dat: Option<&WhdloadDatContext<'_>>,
) -> WhdloadReconciliation {
    let mut result = WhdloadReconciliation::default();
    let mut agreed_releases: BTreeSet<String> = BTreeSet::new();

    for slave in slaves {
        // A structurally valid slave is always strong structural evidence.
        result
            .observations
            .push(structural_slave_observation(slave));

        let sha1 = slave.hashes.sha1.to_ascii_lowercase();
        let mut entry = WhdloadSlaveIdentity {
            slave_name: slave.name.clone(),
            sha1: sha1.clone(),
            matched_release: None,
            outcome: WhdloadSlaveMatch::StructuralOnly,
        };

        if let Some(context) = dat {
            let distinct_releases: BTreeSet<&str> = context
                .index
                .lookup_sha1(&sha1)
                .iter()
                .map(|rom| rom.game_name.as_str())
                .collect();
            match distinct_releases.len() {
                0 => {}
                1 => {
                    let release = (*distinct_releases.iter().next().expect("len == 1")).to_string();
                    result.observations.push(exact_slave_match_observation(
                        slave,
                        Some(release.clone()),
                        Some(&context.matched_source()),
                    ));
                    agreed_releases.insert(release.clone());
                    entry.matched_release = Some(release);
                    entry.outcome = WhdloadSlaveMatch::Exact;
                }
                _ => {
                    // One slave, several distinct releases: never pick one.
                    result.ambiguous = true;
                    entry.outcome = WhdloadSlaveMatch::ExactAmbiguous;
                }
            }
        }

        result.slaves.push(entry);
    }

    // Several slaves that each matched cleanly but to different releases:
    // keep every exact observation for review, but the install as a whole
    // is ambiguous.
    if agreed_releases.len() > 1 {
        result.ambiguous = true;
    }

    result
}
