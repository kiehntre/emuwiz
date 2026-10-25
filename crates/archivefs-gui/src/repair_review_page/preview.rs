//! Shared read-only validation snapshot, prepared by the load/scan worker.
use super::*;

pub(super) struct PreparedPreview {
    pub(super) plan: LibraryRepairPlan,
    pub(super) snapshot: PreviewSnapshot,
}

impl PreparedPreview {
    pub(super) fn new(plan: LibraryRepairPlan) -> Self {
        let snapshot = PreviewSnapshot::new(&plan);
        Self { plan, snapshot }
    }
}

pub(super) struct PreviewSnapshot {
    rows: Vec<RepairReviewRow>,
    pub(super) counts: ReportCounts,
    pub(super) safe_ids: BTreeSet<RepairProposalId>,
}

impl PreviewSnapshot {
    pub(super) fn new(plan: &LibraryRepairPlan) -> Self {
        // build_rows performs Core conflict validation exactly once.
        let rows = build_rows(plan, None);
        let safe_ids: BTreeSet<_> = rows
            .iter()
            .filter_map(|row| row.proposal_id.clone())
            .collect();
        let mut counts = plan.report.counts;
        counts.safe_repairs = plan
            .repair_plan
            .proposals
            .iter()
            .filter(|proposal| {
                safe_ids.contains(&proposal.id)
                    && matches!(
                        proposal.action,
                        archivefs_core::repair::proposal::RepairAction::RenamePath { .. }
                    )
            })
            .count();
        counts.needs_review = rows
            .iter()
            .filter(|row| row.kind == RepairRowKind::NeedsReview)
            .count();
        counts.blocked_repair = rows
            .iter()
            .filter(|row| row.kind == RepairRowKind::Blocked)
            .count();
        Self {
            rows,
            counts,
            safe_ids,
        }
    }

    pub(super) fn filtered_rows(&self, filter: Option<RepairFilter>) -> Vec<RepairReviewRow> {
        self.rows
            .iter()
            .filter(|row| {
                filter.is_none_or(|filter| {
                    matches!(
                        (filter, row.kind),
                        (RepairFilter::Safe, RepairRowKind::Safe)
                            | (RepairFilter::NeedsReview, RepairRowKind::NeedsReview)
                            | (RepairFilter::Blocked, RepairRowKind::Blocked)
                    )
                })
            })
            .cloned()
            .collect()
    }
}
