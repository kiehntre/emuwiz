//! Read-only, bounded projections. Resolution always belongs to the originating
//! workflow: this module has no database table, executor, probe, or resolved flag.

use std::collections::BTreeMap;

use crate::operation::{
    ActionAvailability, OperationKind, OperationRecord, OperationState, RecoveryClassification,
};

pub const ATTENTION_PAGE_SIZE: usize = 50;
pub const ATTENTION_GROUP_LIMIT: usize = 1024;

/// Inverse of the repository's UTC receipt formatter. Unknown/invalid times
/// stay unknown; observation time is never replaced with page-load time.
pub(crate) fn receipt_utc_seconds(value: &str) -> Option<i64> {
    if value.len() != 20 || !value.is_ascii() {
        return None;
    }
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: i64 = value.get(5..7)?.parse().ok()?;
    let day: i64 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * shifted_month + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    let seconds = days * 86400 + hour * 3600 + minute * 60 + second;
    (crate::format_unix_timestamp_utc(seconds) == value).then_some(seconds)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttentionSeverity {
    Blocking,
    ActionNeeded,
    Warning,
    Info,
}
impl AttentionSeverity {
    pub const ALL: [Self; 4] = [
        Self::Blocking,
        Self::ActionNeeded,
        Self::Warning,
        Self::Info,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Blocking => "Blocking",
            Self::ActionNeeded => "Action needed",
            Self::Warning => "Warning",
            Self::Info => "Info",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttentionCategory {
    Sources,
    Identity,
    Dat,
    Duplicates,
    Repair,
    Emulator,
    Launch,
    Publication,
    CheatsMods,
    Recovery,
    Conversion,
    Unsupported,
    Operations,
}
impl AttentionCategory {
    pub const ALL: [Self; 13] = [
        Self::Sources,
        Self::Identity,
        Self::Dat,
        Self::Duplicates,
        Self::Repair,
        Self::Emulator,
        Self::Launch,
        Self::Publication,
        Self::CheatsMods,
        Self::Recovery,
        Self::Conversion,
        Self::Unsupported,
        Self::Operations,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Sources => "Sources",
            Self::Identity => "Identity",
            Self::Dat => "DAT authority",
            Self::Duplicates => "Duplicate review",
            Self::Repair => "Repair",
            Self::Emulator => "Emulator readiness",
            Self::Launch => "Launch readiness",
            Self::Publication => "Publication",
            Self::CheatsMods => "Cheats & Mods",
            Self::Recovery => "Database recovery",
            Self::Conversion => "Disc conversion",
            Self::Unsupported => "Unsupported files",
            Self::Operations => "Operations",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttentionDestination {
    Sources,
    Discovery,
    DatReview,
    Duplicates,
    Problems,
    EmulatorSetup,
    LaunchReadiness,
    History,
    Romm,
    EsDe,
    LibraryOrganisation,
    CheatsMods,
    DiscConversion,
    ExactDuplicates,
}
impl AttentionDestination {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sources => "Sources",
            Self::Discovery => "Discovery review",
            Self::DatReview => "DAT review",
            Self::Duplicates => "Library duplicate review",
            Self::Problems => "Problems & Repair",
            Self::EmulatorSetup => "Emulator Setup",
            Self::LaunchReadiness => "Launch readiness",
            Self::History => "History & Logs / recovery",
            Self::Romm => "Library Organisation / RomM",
            Self::EsDe => "Library Organisation / ES-DE",
            Self::LibraryOrganisation => "Library Organisation",
            Self::CheatsMods => "Cheats & Mods",
            Self::DiscConversion => "Disc Conversion",
            Self::ExactDuplicates => "Duplicate Finder",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttentionState {
    #[default]
    Unresolved,
    Resolved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttentionItem {
    /// Source-owned identity, not a hash of display labels.
    pub id: String,
    pub category: AttentionCategory,
    pub severity: AttentionSeverity,
    pub title: String,
    pub summary: String,
    pub affected: Option<String>,
    pub platform: Option<String>,
    pub source_workflow: String,
    pub source_records: Vec<String>,
    pub first_detected: Option<i64>,
    pub last_observed: Option<i64>,
    pub state: AttentionState,
    pub recommended_action: String,
    pub destination: AttentionDestination,
    pub recoverability: String,
    pub provenance: String,
    /// A summary card can represent many objects, without loading those objects.
    pub affected_count: u64,
}
impl AttentionItem {
    pub fn new(
        id: String,
        category: AttentionCategory,
        severity: AttentionSeverity,
        title: String,
        destination: AttentionDestination,
    ) -> Self {
        Self {
            id,
            category,
            severity,
            title,
            summary: String::new(),
            affected: None,
            platform: None,
            source_workflow: category.label().into(),
            source_records: Vec::new(),
            first_detected: None,
            last_observed: None,
            state: AttentionState::Unresolved,
            recommended_action: format!("Review in {}", destination.label()),
            destination,
            recoverability: "Review the current evidence in the originating workflow".into(),
            provenance: String::new(),
            affected_count: 1,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttentionFilters {
    pub search: String,
    pub severity: Option<AttentionSeverity>,
    pub category: Option<AttentionCategory>,
    pub platform: Option<String>,
    pub workflow: Option<String>,
    /// None means both; default is unresolved only.
    pub state: AttentionStateFilter,
    pub newest_first: bool,
    pub page: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttentionStateFilter {
    #[default]
    Unresolved,
    Resolved,
    All,
}

#[derive(Clone, Debug, Default)]
pub struct AttentionSnapshot {
    items: BTreeMap<String, AttentionItem>,
    pub coverage_notes: Vec<String>,
    pub source_rows: u64,
    pub query_count: usize,
    pub query_millis: u128,
    pub limited: bool,
}
impl AttentionSnapshot {
    pub fn items(&self) -> impl Iterator<Item = &AttentionItem> {
        self.items.values()
    }
    pub fn insert(&mut self, mut item: AttentionItem) {
        if let Some(previous) = self.items.get(&item.id) {
            // Same receipt/root: most recent source state wins, including resolution.
            if previous.last_observed > item.last_observed {
                return;
            }
            item.first_detected = previous.first_detected.or(item.first_detected);
            for reference in &previous.source_records {
                if item.source_records.len() < 8 && !item.source_records.contains(reference) {
                    item.source_records.push(reference.clone());
                }
            }
        } else if self.items.len() >= ATTENTION_GROUP_LIMIT {
            self.limited = true;
            // Historical receipts must not crowd out a newly observed blocker.
            let worst = self
                .items
                .values()
                .max_by_key(|i| (i.severity, std::cmp::Reverse(i.last_observed), &i.id));
            if let Some(worst) = worst.filter(|i| i.severity > item.severity) {
                let id = worst.id.clone();
                self.items.remove(&id);
            } else {
                return;
            }
        }
        item.source_records.truncate(8);
        self.items.insert(item.id.clone(), item);
    }
    pub fn merge(&mut self, other: Self) {
        self.source_rows += other.source_rows;
        self.query_count += other.query_count;
        self.query_millis += other.query_millis;
        self.limited |= other.limited;
        self.coverage_notes.extend(other.coverage_notes);
        for item in other.items.into_values() {
            self.insert(item);
        }
    }
    /// SQL adapters combine related verdicts into one platform review card.
    /// Counts are evidence records, not a fabricated distinct-file count.
    pub(crate) fn insert_dat_summary(&mut self, mut item: AttentionItem) {
        item.id = format!(
            "dat-review:{}",
            item.platform.as_deref().unwrap_or("unassigned")
        );
        item.title = "DAT authority and set evidence need review".into();
        if let Some(previous) = self.items.remove(&item.id) {
            item.summary = format!("{} {}", previous.summary, item.summary);
            item.affected_count += previous.affected_count;
            item.last_observed = item.last_observed.max(previous.last_observed);
            item.source_records.extend(previous.source_records);
        }
        item.provenance = "Saved DAT identity/set records grouped for one review action. Several records can concern the same file; no collection percentage is inferred.".into();
        self.insert(item);
    }
    pub fn counts(&self) -> [usize; 4] {
        let mut counts = [0; 4];
        for item in self
            .items()
            .filter(|item| item.state == AttentionState::Unresolved)
        {
            counts[item.severity as usize] += 1;
        }
        counts
    }
    pub fn page(&self, filters: &AttentionFilters) -> AttentionPage<'_> {
        let needle = filters.search.trim().to_lowercase();
        // Bounded summary groups, never a Vec of catalogue objects.
        let mut matching: Vec<_> = self
            .items()
            .filter(|item| {
                filters.severity.is_none_or(|v| v == item.severity)
                    && filters.category.is_none_or(|v| v == item.category)
                    && filters
                        .platform
                        .as_ref()
                        .is_none_or(|v| item.platform.as_ref() == Some(v))
                    && filters
                        .workflow
                        .as_ref()
                        .is_none_or(|v| &item.source_workflow == v)
                    && match filters.state {
                        AttentionStateFilter::All => true,
                        AttentionStateFilter::Unresolved => {
                            item.state == AttentionState::Unresolved
                        }
                        AttentionStateFilter::Resolved => item.state == AttentionState::Resolved,
                    }
                    && (needle.is_empty()
                        || item.title.to_lowercase().contains(&needle)
                        || item.summary.to_lowercase().contains(&needle)
                        || item
                            .affected
                            .as_ref()
                            .is_some_and(|v| v.to_lowercase().contains(&needle)))
            })
            .collect();
        matching.sort_by(|a, b| {
            if filters.newest_first {
                b.last_observed
                    .cmp(&a.last_observed)
                    .then(a.severity.cmp(&b.severity))
            } else {
                a.severity
                    .cmp(&b.severity)
                    .then(b.last_observed.cmp(&a.last_observed))
            }
            .then(a.id.cmp(&b.id))
        });
        let total = matching.len();
        let page = filters
            .page
            .min(total.saturating_sub(1) / ATTENTION_PAGE_SIZE);
        AttentionPage {
            total,
            page,
            items: matching
                .into_iter()
                .skip(page * ATTENTION_PAGE_SIZE)
                .take(ATTENTION_PAGE_SIZE)
                .collect(),
        }
    }
}
pub struct AttentionPage<'a> {
    pub total: usize,
    pub page: usize,
    pub items: Vec<&'a AttentionItem>,
}

/// Uses OperationRegistry's interpreted states/capabilities, not journal or UI
/// label heuristics. Completed rollback availability alone is not a problem.
pub fn operation_attention(record: &OperationRecord) -> Option<AttentionItem> {
    use AttentionCategory as C;
    use AttentionDestination as D;
    let resolved = matches!(
        record.state,
        OperationState::Completed | OperationState::RolledBack
    );
    let problem = matches!(
        record.state,
        OperationState::Failed
            | OperationState::Partial
            | OperationState::Interrupted
            | OperationState::Stale
            | OperationState::RollbackBlocked
    ) || (!resolved
        && record.recovery.classification == RecoveryClassification::RequiresReview
        && record.recovery.actions.review == ActionAvailability::Available);
    if !resolved && !problem {
        return None;
    }
    let (category, destination) = match record.kind {
        OperationKind::DatRename => (C::Dat, D::DatReview),
        OperationKind::DuplicateQuarantine => (C::Duplicates, D::Problems),
        OperationKind::RepairApply => (C::Repair, D::Problems),
        OperationKind::CheatApply | OperationKind::ModApply => (C::CheatsMods, D::CheatsMods),
        OperationKind::RommPublish => (C::Publication, D::Romm),
        OperationKind::EsDePublish => (C::Publication, D::EsDe),
        OperationKind::PlayingLibrary | OperationKind::LibraryViewPublish => {
            (C::Publication, D::LibraryOrganisation)
        }
        OperationKind::DatabaseRecovery | OperationKind::DatabaseBackup => {
            (C::Recovery, D::History)
        }
        OperationKind::DiscConversion => (C::Conversion, D::DiscConversion),
    };
    let severity = if resolved {
        AttentionSeverity::Info
    } else if record.kind == OperationKind::DatabaseRecovery
        || record.state == OperationState::RollbackBlocked
        || (record.state == OperationState::Failed
            && record.recovery.classification == RecoveryClassification::Stale)
    {
        AttentionSeverity::Blocking
    } else {
        AttentionSeverity::ActionNeeded
    };
    let mut item = AttentionItem::new(
        format!("operation:{}", record.operation_id),
        category,
        severity,
        format!(
            "{} {}",
            record.kind.label(),
            if resolved {
                "has completed"
            } else {
                "needs review"
            }
        ),
        destination,
    );
    item.summary = record
        .error
        .as_ref()
        .unwrap_or(&record.output.summary)
        .clone();
    item.affected = record.destination.clone();
    item.source_workflow = record.kind.label().into();
    item.source_records.push(record.operation_id.clone());
    if let Some(reference) = &record.output.journal_reference {
        item.source_records.push(reference.clone());
    }
    item.first_detected = (record.created_at_unix > 0).then_some(record.created_at_unix as i64);
    item.last_observed = record
        .completed_at_unix
        .map(|v| v as i64)
        .or(item.first_detected);
    item.state = if resolved {
        AttentionState::Resolved
    } else {
        AttentionState::Unresolved
    };
    item.recoverability = record.recovery.explanation.clone();
    item.provenance = "OperationRegistry projection of the saved receipt; recovery revalidates evidence before acting".into();
    Some(item)
}

/// Already-computed Doctor evidence only. Caller replaces this entire snapshot
/// after a new diagnostic run, so repaired findings disappear automatically.
pub fn doctor_attention(scan: &crate::diagnostics::DoctorScan, observed: i64) -> AttentionSnapshot {
    use crate::diagnostics::{DoctorCategory as C, DoctorSeverity as S};
    let mut snapshot = AttentionSnapshot::default();
    for finding in &scan.findings {
        snapshot.source_rows += 1;
        let (category, destination) = match finding.category {
            // Indexed catalogue and OperationRegistry evidence are stronger.
            C::Sources | C::Transactions => continue,
            C::Library
                if matches!(
                    finding.id.as_str(),
                    "library.archive_missing" | "library.unknown_platform"
                ) =>
            {
                continue;
            }
            C::Emulators | C::EmulatorProfiles => (
                AttentionCategory::Emulator,
                AttentionDestination::EmulatorSetup,
            ),
            C::ManagedEntries => (
                AttentionCategory::CheatsMods,
                AttentionDestination::CheatsMods,
            ),
            C::Database => (AttentionCategory::Recovery, AttentionDestination::History),
            _ => (AttentionCategory::Repair, AttentionDestination::Problems),
        };
        let severity = match finding.severity {
            S::Healthy => continue,
            S::Critical | S::Error => AttentionSeverity::Blocking,
            S::Warning => AttentionSeverity::Warning,
            S::Info => AttentionSeverity::Info,
        };
        let mut item = AttentionItem::new(
            format!("doctor:{}:{:?}", finding.id, finding.affected),
            category,
            severity,
            finding.title.clone(),
            destination,
        );
        item.summary = finding.explanation.clone();
        item.affected = finding.affected.as_ref().map(|p| p.display.clone());
        item.source_records.push(finding.id.clone());
        item.source_workflow = "Diagnostics".into();
        item.last_observed = Some(observed);
        if let Some(action) = &finding.next_step {
            item.recommended_action = action.clone();
        }
        item.provenance = "Last completed Doctor check; not a live probe".into();
        snapshot.insert(item);
    }
    if !scan.not_checked.is_empty() {
        snapshot.coverage_notes.push(format!("{} diagnostic checks were not performed. Open Problems & Repair / Diagnostics for coverage.", scan.not_checked.len()));
    }
    snapshot
}

#[cfg(test)]
mod tests;
