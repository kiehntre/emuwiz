use crate::*;

// --- Doctor Stage 1A: read-only diagnostic scan --------------------------

/// One completed read-only Doctor scan, plus when it finished.
pub(crate) struct DoctorScanOutcome {
    pub(crate) scan: DoctorScan,
    pub(crate) finished_at_unix_seconds: i64,
}

/// The inputs one Doctor run collects on a worker thread. Only the
/// path-based subsystems are gathered here; the preloaded, session-owned
/// ones (`LoadedData::doctor`, live health issues, discovered RetroArch
/// profiles) are borrowed on the main thread when the result arrives, so
/// nothing large has to be cloned into the worker.
pub(crate) struct DoctorGathered {
    pub(crate) mount_root_safety: Gathered<MountRootSafety>,
    pub(crate) stale_mount_directories: Gathered<Vec<PathBuf>>,
    pub(crate) index_freshness: Gathered<(archivefs_core::ArchiveIndexFreshness, PathBuf)>,
    pub(crate) database: Gathered<DatabaseHealthReport>,
    pub(crate) source_health: Gathered<Vec<SourceHealthIssue>>,
    pub(crate) transactions: Gathered<SharedHistoryReport>,
    pub(crate) storage: Gathered<StorageAssessment>,
    pub(crate) emulator_profiles: Gathered<ProfileAssessmentReport>,
    pub(crate) linux_emulator_installations: Gathered<Vec<LinuxEmulatorInstallationEvidence>>,
    pub(crate) arcade_dat_version:
        Gathered<Vec<archivefs_core::diagnostics::arcade_dat_version::ArcadeEmulatorDatReadiness>>,
    pub(crate) xemu_readiness: Gathered<Vec<XemuReadinessAssessment>>,
    pub(crate) xenia_readiness: Gathered<Vec<XeniaReadinessAssessment>>,
    pub(crate) ppsspp_readiness: Gathered<Vec<PpssppReadinessAssessment>>,
    pub(crate) rpcs3_readiness: Gathered<Vec<Rpcs3ReadinessAssessment>>,
    pub(crate) managed_entries: Gathered<ManagedEntryScan>,
}

/// The confirmation screen for one repair. Holding this is the *only* way a
/// repair can be executed: `show_doctor_page` never calls the executor, and
/// no repair runs from expanding a finding or from Run Doctor.
pub(crate) struct DoctorRepairReview {
    pub(crate) action: DoctorRepairAction,
    pub(crate) finding_id: String,
    /// The exact resource, when the finding names one. Used to resolve the
    /// finding unambiguously - never as a repair target in its own right.
    pub(crate) affected: Option<String>,
    pub(crate) finding_title: String,
    pub(crate) evidence: Vec<String>,
}

pub(crate) enum DoctorScanState {
    NotRun,
    Running {
        generation: RefreshGeneration,
        receiver: Receiver<(RefreshGeneration, DoctorGathered)>,
        /// The previous result stays on screen while a new run is in
        /// flight, so the page never blanks.
        previous: Option<Box<DoctorScanOutcome>>,
    },
    Ready(Box<DoctorScanOutcome>),
}

impl DoctorScanState {
    /// The result to display right now - the completed one, or the previous
    /// one while a new run is still gathering.
    pub(crate) fn displayed(&self) -> Option<&DoctorScanOutcome> {
        match self {
            Self::NotRun => None,
            Self::Running { previous, .. } => previous.as_deref(),
            Self::Ready(outcome) => Some(outcome),
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

