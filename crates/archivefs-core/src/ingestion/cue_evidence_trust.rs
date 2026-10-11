//! Typed observations of physical CUE/BIN bytes and layout, without source custody.
//!
//! This adapter compares already collected records. It performs no filesystem
//! access and cannot establish that any read stream was an atomic image, that
//! sources still contain those bytes, or that originals may be removed. Legacy
//! `Equivalent` means matching recorded facts here, never continuing equivalence.
//! No capture backend is implemented; even successful reads leave source
//! stability indeterminate. See `docs/design/cue_bin_evidence_trust.md`.

use std::convert::Infallible;

use super::cue_preservation::{
    CUE_PRESERVATION_SCHEMA, CueComparisonOutcome, CueLayoutDifference, CuePreservationError,
    DiscLayoutEvidence, compare_cue_evidence,
};
use crate::safe_read::SafeReadRefusal;

/// Relation of the complete observed physical bytes AND represented layout.
/// These are historical read-stream facts, not statements about current paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedRelation {
    MatchingBytesObserved,
    DifferentBytesObserved,
    NotCompared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandSide {
    Left,
    Right,
}

/// Support and source assessment remain separate, including mixed failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutSupport<'a> {
    SupportedLayout {
        contract: &'static str,
    },
    UnsupportedLayout(&'a CuePreservationError),
    /// Collection failed before a supported representation was established.
    NotEstablished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceUncertainty<'a> {
    /// Successful ordinary reads do not exclude any concurrent/future writer.
    ContinuousStabilityUnproven,
    CollectionFailed(&'a CuePreservationError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAssessment<'a> {
    ChangedSource(&'a CuePreservationError),
    IndeterminateSource(SourceUncertainty<'a>),
}

/// Reserved for a separately reviewed backend that enforces all-member custody.
/// This slice has no constructor, deserializer, cloning or upgrade capability.
/// Its private uninhabited field also prevents internal safe construction.
///
/// ```compile_fail
/// use archivefs_core::ingestion::cue_evidence_trust::CaptureGuard;
/// let fabricated = CaptureGuard {};
/// ```
///
/// ```compile_fail
/// use archivefs_core::ingestion::cue_evidence_trust::CaptureGuard;
/// let restored: CaptureGuard = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Debug)]
pub struct CaptureGuard {
    _unimplemented: Infallible,
}

/// Ordinary observations cannot select the reserved guarded state.
#[derive(Debug)]
pub enum RepresentationTrust {
    MutableReadObservation,
    VerifiedStableCapturedRepresentation(CaptureGuard),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StableCaptureRefusal {
    /// No filesystem, operating system or source type has a custody backend yet.
    CustodyBackendUnavailable,
}

/// Read-only access to original provenance, aliases, ranges and collection errors.
/// Borrowing retains the supplied record rather than replacing its earlier bytes
/// with a new read. This is not a retained CUE/BIN payload or a live source guard.
#[derive(Debug)]
pub struct OperandObservation<'a> {
    record: &'a Result<DiscLayoutEvidence, CuePreservationError>,
    layout: LayoutSupport<'a>,
    source: SourceAssessment<'a>,
    trust: RepresentationTrust,
}

impl<'a> OperandObservation<'a> {
    pub fn evidence(&self) -> Option<&'a DiscLayoutEvidence> {
        self.record.as_ref().ok()
    }

    pub fn collection_error(&self) -> Option<&'a CuePreservationError> {
        self.record.as_ref().err()
    }

    pub fn layout_support(&self) -> LayoutSupport<'a> {
        self.layout
    }

    pub fn source_assessment(&self) -> SourceAssessment<'a> {
        self.source
    }

    pub fn representation_trust(&self) -> &RepresentationTrust {
        &self.trust
    }

    /// Refuse explicitly; repeated observations cannot supply missing custody.
    pub fn require_verified_capture(&self) -> Result<&CaptureGuard, StableCaptureRefusal> {
        Err(StableCaptureRefusal::CustodyBackendUnavailable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperandRefusal<'a> {
    pub operand: OperandSide,
    pub reason: &'a CuePreservationError,
}

/// Opaque assessment, constructed only from original collector results.
/// No operation authority or stable-equivalence token is exposed.
///
/// A caller-fabricated legacy comparison is not an admissible input:
/// ```compile_fail
/// use archivefs_core::ingestion::cue_evidence_trust::assess_cue_observations;
/// use archivefs_core::ingestion::cue_preservation::{CuePreservationComparison, CueComparisonOutcome};
/// let fabricated = CuePreservationComparison {
///     outcome: CueComparisonOutcome::Equivalent, differences: vec![],
///     refusals: vec![], programs_match: Some(true),
/// };
/// let trusted = assess_cue_observations(&fabricated, &fabricated);
/// ```
///
/// No conversion from an observation assessment into a capture guard exists:
/// ```compile_fail
/// use archivefs_core::ingestion::cue_evidence_trust::{CaptureGuard, CueObservationAssessment};
/// fn upgrade(observation: CueObservationAssessment<'_>) -> CaptureGuard {
///     observation.try_into().unwrap()
/// }
/// ```
#[derive(Debug)]
pub struct CueObservationAssessment<'a> {
    relation: ObservedRelation,
    left: OperandObservation<'a>,
    right: OperandObservation<'a>,
    differences: Vec<CueLayoutDifference>,
    programs_match: Option<bool>,
}

impl<'a> CueObservationAssessment<'a> {
    pub fn relation(&self) -> ObservedRelation {
        self.relation
    }

    pub fn operand(&self, side: OperandSide) -> &OperandObservation<'a> {
        match side {
            OperandSide::Left => &self.left,
            OperandSide::Right => &self.right,
        }
    }

    pub fn differences(&self) -> &[CueLayoutDifference] {
        &self.differences
    }

    /// Physical program-byte/mode observations only; not layout equivalence,
    /// current-source identity, cooked-sector identity or conversion eligibility.
    /// Failed or unsupported comparisons never retain a positive program match.
    pub fn observed_programs_match(&self) -> Option<bool> {
        self.programs_match
    }

    /// Preserve operand attribution and both errors in deterministic left/right order.
    pub fn refusals(&self) -> impl Iterator<Item = OperandRefusal<'a>> + '_ {
        [OperandSide::Left, OperandSide::Right]
            .into_iter()
            .filter_map(|operand| {
                self.operand(operand)
                    .collection_error()
                    .map(|reason| OperandRefusal { operand, reason })
            })
    }
}

fn operand_observation(
    record: &Result<DiscLayoutEvidence, CuePreservationError>,
) -> OperandObservation<'_> {
    let (layout, source) = match record {
        Ok(_) => (
            LayoutSupport::SupportedLayout {
                contract: CUE_PRESERVATION_SCHEMA,
            },
            SourceAssessment::IndeterminateSource(SourceUncertainty::ContinuousStabilityUnproven),
        ),
        Err(error) => {
            // Reuse the engine's error classification without exposing its
            // private helper or duplicating a list that can drift. This call
            // only handles Err records; it never traverses successful evidence.
            let layout = if compare_cue_evidence(record, record).outcome
                == CueComparisonOutcome::Unsupported
            {
                LayoutSupport::UnsupportedLayout(error)
            } else {
                LayoutSupport::NotEstablished
            };
            let source = match error {
                CuePreservationError::ChangedSource { .. }
                | CuePreservationError::SafeRead {
                    refusal: SafeReadRefusal::ChangedWhileOpening,
                    ..
                } => SourceAssessment::ChangedSource(error),
                _ => SourceAssessment::IndeterminateSource(SourceUncertainty::CollectionFailed(
                    error,
                )),
            };
            (layout, source)
        }
    };
    OperandObservation {
        record,
        layout,
        source,
        trust: RepresentationTrust::MutableReadObservation,
    }
}

/// Adapt already collected observations without reopening or mutating sources.
/// All source paths may have changed, disappeared, or remained unverifiably
/// mutable since collection; a matching record never proves their freshness.
pub fn assess_cue_observations<'a>(
    left: &'a Result<DiscLayoutEvidence, CuePreservationError>,
    right: &'a Result<DiscLayoutEvidence, CuePreservationError>,
) -> CueObservationAssessment<'a> {
    let comparison = compare_cue_evidence(left, right);
    let relation = match comparison.outcome {
        CueComparisonOutcome::Equivalent => ObservedRelation::MatchingBytesObserved,
        CueComparisonOutcome::Different => ObservedRelation::DifferentBytesObserved,
        CueComparisonOutcome::Unsupported | CueComparisonOutcome::Indeterminate => {
            ObservedRelation::NotCompared
        }
    };
    let programs_match = if relation == ObservedRelation::NotCompared {
        None
    } else {
        comparison.programs_match
    };
    CueObservationAssessment {
        relation,
        left: operand_observation(left),
        right: operand_observation(right),
        differences: comparison.differences,
        programs_match,
    }
}
