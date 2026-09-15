use std::sync::{Arc, Mutex, atomic::AtomicBool};

use crate::identity_sources_page;
use crate::plan_preview_page;
use crate::selected_evidence_no_intro;
use crate::selected_evidence_page;
use crate::selected_evidence_pipeline::SelectedEvidenceEnrichmentState;

/// UI/session state retained by the selected-game evidence surfaces.
///
/// Evidence gathering, readiness projection, and launch planning remain owned
/// by their existing modules. This bundle only consolidates their GUI state,
/// worker generations, and caches.
pub(crate) struct SelectedEvidenceUiState {
    pub(crate) selected_evidence: selected_evidence_page::SelectedEvidenceState,
    pub(crate) selected_evidence_generation: u64,
    pub(crate) selected_evidence_cancel: Option<Arc<AtomicBool>>,
    pub(crate) selected_evidence_enrichment: SelectedEvidenceEnrichmentState,
    pub(crate) no_intro_source_cache: Arc<Mutex<selected_evidence_no_intro::NoIntroSourceCache>>,
    pub(crate) identity_sources: identity_sources_page::IdentitySourcesState,
    pub(crate) identity_sources_generation: u64,
    pub(crate) scummvm_readiness: identity_sources_page::ScummVmReadinessState,
    pub(crate) scummvm_check: identity_sources_page::ScummVmCheckState,
    pub(crate) scummvm_check_generation: u64,
    pub(crate) plan_preview: plan_preview_page::PlanPreviewState,
    pub(crate) plan_preview_generation: u64,
}

impl Default for SelectedEvidenceUiState {
    fn default() -> Self {
        Self {
            selected_evidence: selected_evidence_page::SelectedEvidenceState::Idle,
            selected_evidence_generation: 0,
            selected_evidence_cancel: None,
            selected_evidence_enrichment: SelectedEvidenceEnrichmentState::Idle,
            no_intro_source_cache: Arc::new(Mutex::new(
                selected_evidence_no_intro::NoIntroSourceCache::new(),
            )),
            identity_sources: identity_sources_page::IdentitySourcesState::Idle,
            identity_sources_generation: 0,
            scummvm_readiness: identity_sources_page::ScummVmReadinessState::NotChecked,
            scummvm_check: identity_sources_page::ScummVmCheckState::Idle,
            scummvm_check_generation: 0,
            plan_preview: plan_preview_page::PlanPreviewState::Idle,
            plan_preview_generation: 0,
        }
    }
}
