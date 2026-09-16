use archivefs_core::DatabaseRestorePlan;

use crate::doctor_repair::DoctorScanState;
use crate::onboarding::OnboardingState;
use crate::setup_controller::DiagnosticsState;
use crate::{DoctorRepairOutcome, DoctorRepairReview, RefreshGeneration};

/// UI/session state for setup diagnostics, Doctor, repair review, and database
/// restoration. Domain repair execution remains in the existing core and
/// doctor/repair controllers.
pub(crate) struct DoctorRepairState {
    pub(crate) database_restore_plan: Option<DatabaseRestorePlan>,
    pub(crate) database_restore_confirmation: String,
    pub(crate) database_restore_feedback: Option<String>,
    pub(crate) diagnostics: DiagnosticsState,
    pub(crate) config_previously_confirmed: bool,
    pub(crate) onboarding_state: OnboardingState,
    pub(crate) onboarding_auto_open_checked: bool,
    pub(crate) doctor_scan: DoctorScanState,
    pub(crate) doctor_scan_generation: RefreshGeneration,
    pub(crate) doctor_selected_finding: Option<String>,
    pub(crate) doctor_repair_review: Option<DoctorRepairReview>,
    pub(crate) doctor_repair_result: Option<Box<DoctorRepairOutcome>>,
    pub(crate) doctor_repair_finished_at_unix_seconds: Option<i64>,
}

impl Default for DoctorRepairState {
    fn default() -> Self {
        Self {
            database_restore_plan: None,
            database_restore_confirmation: String::new(),
            database_restore_feedback: None,
            diagnostics: DiagnosticsState::Error {
                generation: RefreshGeneration::INITIAL,
                message: String::new(),
            },
            config_previously_confirmed: false,
            onboarding_state: OnboardingState::NotStarted,
            onboarding_auto_open_checked: false,
            doctor_scan: DoctorScanState::NotRun,
            doctor_scan_generation: RefreshGeneration::INITIAL,
            doctor_selected_finding: None,
            doctor_repair_review: None,
            doctor_repair_result: None,
            doctor_repair_finished_at_unix_seconds: None,
        }
    }
}

impl DoctorRepairState {
    pub(crate) fn new(context: eframe::egui::Context, generation: RefreshGeneration) -> Self {
        Self {
            database_restore_plan: None,
            database_restore_confirmation: String::new(),
            database_restore_feedback: None,
            diagnostics: crate::start_diagnostics(context, generation),
            config_previously_confirmed: false,
            onboarding_state: crate::onboarding::load_onboarding_state(),
            onboarding_auto_open_checked: false,
            doctor_scan: DoctorScanState::NotRun,
            doctor_scan_generation: RefreshGeneration::INITIAL,
            doctor_selected_finding: None,
            doctor_repair_review: None,
            doctor_repair_result: None,
            doctor_repair_finished_at_unix_seconds: None,
        }
    }
}
