//! Scan scope policy for reusing current, positively classified ancillary files.
//!
//! This module is deliberately pure: callers provide the already persisted
//! classification/freshness facts and receive a decision. It never reads or
//! writes files and never decides that an unknown or game-like item is safe to
//! skip.

use serde::{Deserialize, Serialize};

use super::side_file_classification::SideFileRole;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanMode {
    Normal,
    Full,
    Targeted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AncillaryScanDecision {
    ReuseCurrentClassification,
    DeepAnalyse,
}

/// Facts persisted by a prior scan. A role is not enough on its own: the
/// producer/version and file freshness must also agree, and relationships may
/// require the normal game/set path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassificationFreshness {
    pub role: SideFileRole,
    pub file_unchanged: bool,
    pub producer_current: bool,
    pub dependency_required: bool,
    pub conflict_or_ambiguity: bool,
}

impl ClassificationFreshness {
    pub fn decision(self, mode: ScanMode, targeted: bool) -> AncillaryScanDecision {
        if mode != ScanMode::Normal
            || targeted
            || !self.file_unchanged
            || !self.producer_current
            || self.dependency_required
            || self.conflict_or_ambiguity
            || !is_fast_path_role(self.role)
        {
            AncillaryScanDecision::DeepAnalyse
        } else {
            AncillaryScanDecision::ReuseCurrentClassification
        }
    }
}

pub fn is_fast_path_role(role: SideFileRole) -> bool {
    matches!(
        role,
        SideFileRole::Artwork
            | SideFileRole::Manual
            | SideFileRole::Readme
            | SideFileRole::Metadata
            | SideFileRole::SaveOrState
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanScopeCounts {
    pub files_checked: usize,
    pub deeply_analysed: usize,
    pub reused_classifications: usize,
}

impl ScanScopeCounts {
    pub fn record(&mut self, decision: AncillaryScanDecision) {
        self.files_checked += 1;
        match decision {
            AncillaryScanDecision::ReuseCurrentClassification => {
                self.reused_classifications += 1
            }
            AncillaryScanDecision::DeepAnalyse => self.deeply_analysed += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(role: SideFileRole) -> ClassificationFreshness {
        ClassificationFreshness {
            role,
            file_unchanged: true,
            producer_current: true,
            dependency_required: false,
            conflict_or_ambiguity: false,
        }
    }

    #[test]
    fn only_current_unchanged_ancillary_roles_reuse() {
        assert_eq!(facts(SideFileRole::Artwork).decision(ScanMode::Normal, false), AncillaryScanDecision::ReuseCurrentClassification);
        assert_eq!(facts(SideFileRole::Manual).decision(ScanMode::Normal, false), AncillaryScanDecision::ReuseCurrentClassification);
        assert_eq!(facts(SideFileRole::CueSheet).decision(ScanMode::Normal, false), AncillaryScanDecision::DeepAnalyse);
        assert_eq!(facts(SideFileRole::PrimaryContent).decision(ScanMode::Normal, false), AncillaryScanDecision::DeepAnalyse);
    }

    #[test]
    fn full_and_targeted_always_bypass_fast_path() {
        let f = facts(SideFileRole::Artwork);
        assert_eq!(f.decision(ScanMode::Full, false), AncillaryScanDecision::DeepAnalyse);
        assert_eq!(f.decision(ScanMode::Targeted, false), AncillaryScanDecision::DeepAnalyse);
        assert_eq!(f.decision(ScanMode::Normal, true), AncillaryScanDecision::DeepAnalyse);
    }

    #[test]
    fn stale_changed_or_related_files_reanalyse() {
        let mut f = facts(SideFileRole::Artwork);
        f.file_unchanged = false;
        assert_eq!(f.decision(ScanMode::Normal, false), AncillaryScanDecision::DeepAnalyse);
        let mut f = facts(SideFileRole::Artwork);
        f.producer_current = false;
        assert_eq!(f.decision(ScanMode::Normal, false), AncillaryScanDecision::DeepAnalyse);
        let mut f = facts(SideFileRole::Artwork);
        f.dependency_required = true;
        assert_eq!(f.decision(ScanMode::Normal, false), AncillaryScanDecision::DeepAnalyse);
    }

    #[test]
    fn counts_reconcile_without_removing_files() {
        let mut c = ScanScopeCounts::default();
        c.record(AncillaryScanDecision::ReuseCurrentClassification);
        c.record(AncillaryScanDecision::DeepAnalyse);
        assert_eq!(c.files_checked, 2);
        assert_eq!(c.reused_classifications + c.deeply_analysed, c.files_checked);
    }
}
