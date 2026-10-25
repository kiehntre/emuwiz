//! Assessment only: no restore mechanism, state writes, configuration writes or execution.
use crate::launch::readiness::LaunchBlocker;
use crate::save_state_compatibility::{SaveStateAssessment, SaveStateCompatibility};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStateResumeVerdict {
    ReadyForManualReview,
    NeedsEvidence,
    Incompatible,
    LaunchBlocked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveStateResumeReadiness {
    pub assessment: SaveStateAssessment,
    pub verdict: SaveStateResumeVerdict,
    pub launch_blockers: Vec<LaunchBlocker>,
    pub resume_available: bool,
}

pub fn plan_save_state_resume(
    assessment: SaveStateAssessment,
    launch_blockers: Vec<LaunchBlocker>,
) -> SaveStateResumeReadiness {
    let verdict = if !launch_blockers.is_empty() {
        SaveStateResumeVerdict::LaunchBlocked
    } else {
        match assessment.compatibility {
            SaveStateCompatibility::Supported => SaveStateResumeVerdict::ReadyForManualReview,
            SaveStateCompatibility::Incompatible => SaveStateResumeVerdict::Incompatible,
            SaveStateCompatibility::Unknown => SaveStateResumeVerdict::NeedsEvidence,
        }
    };
    SaveStateResumeReadiness {
        assessment,
        verdict,
        launch_blockers,
        resume_available: false,
    }
}
