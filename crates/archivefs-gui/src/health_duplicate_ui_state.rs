use std::collections::HashSet;
use std::path::PathBuf;

use archivefs_core::{CatalogueDuplicateGroup, HealthCategory};

use crate::{ArchiveFsApp, HealthIssue, LoadState, RefreshGeneration, build_health_issues};

/// UI/session state for health and duplicate review surfaces.
///
/// Health and duplicate computation remains in the existing page/core helpers;
/// this bundle only owns the filters, selections, sort state, and cached report
/// retained by the GUI between frames.
pub(crate) struct HealthDuplicateUiState {
    pub(crate) duplicate_filters: DuplicateReviewFilters,
    pub(crate) duplicate_sort_field: DuplicateSortField,
    pub(crate) duplicate_sort_ascending: bool,
    pub(crate) selected_duplicate_group: Option<DuplicateGroupIdentity>,
    pub(crate) selected_duplicate_archive: Option<PathBuf>,
    pub(crate) health_filters: HealthDashboardFilters,
    pub(crate) health_sort_field: HealthSortField,
    pub(crate) health_sort_ascending: bool,
    pub(crate) selected_health_issue: Option<PathBuf>,
    pub(crate) diagnostics_refresh_generation: RefreshGeneration,
    pub(crate) health_report_cache: Option<HealthReportCache>,
}

impl Default for HealthDuplicateUiState {
    fn default() -> Self {
        Self {
            duplicate_filters: DuplicateReviewFilters::initial(),
            duplicate_sort_field: DuplicateSortField::Title,
            duplicate_sort_ascending: true,
            selected_duplicate_group: None,
            selected_duplicate_archive: None,
            health_filters: HealthDashboardFilters::default(),
            health_sort_field: HealthSortField::default(),
            health_sort_ascending: true,
            selected_health_issue: None,
            diagnostics_refresh_generation: RefreshGeneration::INITIAL,
            health_report_cache: None,
        }
    }
}

impl ArchiveFsApp {
    /// The Health Dashboard's report, rebuilt only when the underlying
    /// live snapshot, database snapshot, diagnostics refresh, or recovery
    /// offers have actually changed since the last call - see
    /// `HealthReportCacheKey`'s doc comment for why pointer identity
    /// rather than a raw generation comparison. Never called unless the
    /// dashboard is actually open (see its one call site), so this adds
    /// no cost to the ordinary library view.
    pub(crate) fn cached_health_issues(&mut self) -> &[HealthIssue] {
        let key = HealthReportCacheKey {
            live_data_ptr: match &self.state {
                LoadState::Ready(data) => Some(std::ptr::from_ref(data.as_ref()) as usize),
                LoadState::Loading { .. } | LoadState::Error(_) => None,
            },
            database_snapshot_ptr: self
                .database_state
                .snapshot()
                .map(|snapshot| std::ptr::from_ref(snapshot) as usize),
            diagnostics_generation: self.health_duplicate_ui.diagnostics_refresh_generation,
        };

        let cache_is_fresh = self
            .health_duplicate_ui
            .health_report_cache
            .as_ref()
            .is_some_and(|cache| {
                cache.key == key
                    && cache.lazy_unmount_offers == self.mount_ui.lazy_unmount_offers
                    && cache.remount_offers == self.mount_ui.remount_offers
            });

        if !cache_is_fresh {
            let live_records = match &self.state {
                LoadState::Ready(data) => Some(&data.records),
                LoadState::Loading { .. } | LoadState::Error(_) => None,
            };
            let issues = match (live_records, self.database_state.snapshot()) {
                (Some(records), Some(snapshot)) => build_health_issues(
                    records,
                    snapshot,
                    &self.mount_ui.lazy_unmount_offers,
                    &self.mount_ui.remount_offers,
                ),
                _ => Vec::new(),
            };
            self.health_duplicate_ui.health_report_cache = Some(HealthReportCache {
                key,
                lazy_unmount_offers: self.mount_ui.lazy_unmount_offers.clone(),
                remount_offers: self.mount_ui.remount_offers.clone(),
                issues,
            });
        }

        &self
            .health_duplicate_ui
            .health_report_cache
            .as_ref()
            .unwrap()
            .issues
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DuplicateReviewFilters {
    pub(crate) search: String,
    pub(crate) platform: Option<String>,
    pub(crate) include_missing: bool,
    pub(crate) more_than_two: bool,
}

impl DuplicateReviewFilters {
    pub(crate) fn initial() -> Self {
        Self {
            include_missing: true,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DuplicateSortField {
    #[default]
    Title,
    Platform,
    Entries,
    KnownSize,
}

impl std::fmt::Display for DuplicateSortField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Title => "Title",
            Self::Platform => "Platform",
            Self::Entries => "Number of entries",
            Self::KnownSize => "Total known size",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DuplicateGroupIdentity {
    pub(crate) normalized_title: String,
    pub(crate) platform: String,
}

impl From<&CatalogueDuplicateGroup> for DuplicateGroupIdentity {
    fn from(group: &CatalogueDuplicateGroup) -> Self {
        Self {
            normalized_title: group.normalized_title.clone(),
            platform: group.platform.clone(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HealthIssueFilter {
    #[default]
    All,
    Missing,
    MountFailures,
    Retryable,
    Terminal,
    Historical,
    NoMountRequired,
    NeedsContext,
    AwaitingValidation,
    CachedOnly,
    RecoveryAvailable,
    UnknownPlatform,
}

impl HealthIssueFilter {
    pub(crate) const ALL: [Self; 12] = [
        Self::All,
        Self::Missing,
        Self::MountFailures,
        Self::Retryable,
        Self::Terminal,
        Self::Historical,
        Self::NoMountRequired,
        Self::NeedsContext,
        Self::AwaitingValidation,
        Self::CachedOnly,
        Self::RecoveryAvailable,
        Self::UnknownPlatform,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "All issues",
            Self::Missing => "Missing",
            Self::MountFailures => "Mount failures",
            Self::Retryable => "Retryable",
            Self::Terminal => "Terminal",
            Self::Historical => "Historical mount failures",
            Self::NoMountRequired => "No mount required",
            Self::NeedsContext => "Needs context",
            Self::AwaitingValidation => "Awaiting validation",
            Self::CachedOnly => "Cached-only",
            Self::RecoveryAvailable => "Recovery available",
            Self::UnknownPlatform => "Unknown platform",
        }
    }

    pub(crate) fn matches(self, category: HealthCategory) -> bool {
        match self {
            Self::All => true,
            Self::Missing => category == HealthCategory::Missing,
            Self::MountFailures => matches!(
                category,
                HealthCategory::TerminalFailure | HealthCategory::RetryableFailure
            ),
            Self::Retryable => category == HealthCategory::RetryableFailure,
            Self::Terminal => category == HealthCategory::TerminalFailure,
            Self::Historical => category == HealthCategory::HistoricalMountFailure,
            Self::NoMountRequired => category == HealthCategory::MountNotRequired,
            Self::NeedsContext => category == HealthCategory::MountFailureEvidenceInsufficient,
            Self::AwaitingValidation => category == HealthCategory::AwaitingValidation,
            Self::CachedOnly => category == HealthCategory::CachedOnly,
            Self::RecoveryAvailable => category == HealthCategory::RecoveryAvailable,
            Self::UnknownPlatform => category == HealthCategory::UnknownPlatform,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HealthDashboardFilters {
    pub(crate) search: String,
    pub(crate) platform: Option<String>,
    pub(crate) category: HealthIssueFilter,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HealthSortField {
    #[default]
    Severity,
    Path,
    Platform,
    State,
    Reason,
}

impl std::fmt::Display for HealthSortField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Severity => "Severity",
            Self::Path => "Archive path",
            Self::Platform => "Platform",
            Self::State => "State",
            Self::Reason => "Reason",
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HealthReportCacheKey {
    pub(crate) live_data_ptr: Option<usize>,
    pub(crate) database_snapshot_ptr: Option<usize>,
    pub(crate) diagnostics_generation: RefreshGeneration,
}

/// The Health Dashboard's cached report - see `cached_health_issues`.
/// Recovery offers are compared by content (`HashSet::eq`), not identity:
/// `lazy_unmount_offers`/`remount_offers` are mutated in place across
/// several scattered call sites (individual and batch mount/unmount/
/// remount/lazy-unmount completion), so no single pointer or generation
/// bump could reliably cover all of them without being easy to miss one.
/// Both sets are always small (bounded by archives with an active
/// recovery offer this session), so cloning and comparing them each frame
/// is negligible next to the cost `build_health_issues` would pay to
/// actually rebuild.
pub(crate) struct HealthReportCache {
    pub(crate) key: HealthReportCacheKey,
    pub(crate) lazy_unmount_offers: HashSet<PathBuf>,
    pub(crate) remount_offers: HashSet<PathBuf>,
    pub(crate) issues: Vec<HealthIssue>,
}
