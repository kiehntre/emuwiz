//! GUI-owned inputs and retained results for Dolphin OnFrame installation.
//!
//! This is deliberately separate from the existing Gecko/Action Replay local
//! install context. It owns only typed state and backend results; discovery,
//! binding, planning, previewing, applying, and rollback remain core-owned.

use std::path::{Path, PathBuf};

use archivefs_core::patch_manager::{
    CheatDocument, CheatPlatform, DolphinOnFrameBinding, DolphinOnFrameCandidate,
    DolphinOnFrameInstallPlan, DolphinOnFrameInstallPreview, DolphinOnFrameInstallStatus,
    DolphinOnFrameSourceError, SharedApplyResult, bind_dolphin_onframe_candidate,
    discover_dolphin_onframe_candidates,
};

use crate::onframe_install_state::OnFrameInstallState;

#[derive(Debug, Clone)]
pub(crate) struct OnFrameGuiSource {
    pub candidate: DolphinOnFrameCandidate,
    pub document: CheatDocument,
    pub provenance: PathBuf,
    pub platform: CheatPlatform,
}

impl From<DolphinOnFrameCandidate> for OnFrameGuiSource {
    fn from(candidate: DolphinOnFrameCandidate) -> Self {
        Self {
            document: candidate.document.clone(),
            provenance: candidate.source_path.clone(),
            platform: candidate.platform.clone(),
            candidate,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct OnFrameCandidateCollection {
    pub candidates: Vec<OnFrameGuiSource>,
    pub selected: Option<usize>,
    pub discovery_error: Option<String>,
    pub discovering: bool,
}

impl OnFrameCandidateCollection {
    fn clear_selection(&mut self) {
        self.selected = None;
    }

    fn selected(&self) -> Option<&OnFrameGuiSource> {
        self.selected.and_then(|index| self.candidates.get(index))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OnFrameGuiBinding {
    pub candidate: OnFrameGuiSource,
    pub platform: CheatPlatform,
    pub verified_game_id: String,
    pub profile_root: PathBuf,
    pub gamesettings_destination: PathBuf,
    pub can_install: bool,
    pub refusal_reasons: Vec<String>,
    pub backend: DolphinOnFrameBinding,
}

impl OnFrameGuiBinding {
    fn from_backend(
        candidate: OnFrameGuiSource,
        profile_root: PathBuf,
        backend: DolphinOnFrameBinding,
    ) -> Self {
        Self {
            platform: backend.platform.clone(),
            verified_game_id: backend.game_id.clone(),
            gamesettings_destination: backend.gamesettings_path.clone(),
            can_install: backend.can_install,
            refusal_reasons: backend.reasons.clone(),
            candidate,
            profile_root,
            backend,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct OnFrameInstallViewModel {
    pub source: Option<String>,
    pub game: Option<String>,
    pub game_id: Option<String>,
    pub profile: Option<String>,
    pub destination: Option<String>,
    pub patch: Option<String>,
    pub exact_operations: usize,
    pub unsupported_operations: usize,
    pub status: Option<String>,
    pub can_install: bool,
    pub rollback_available: bool,
}

#[derive(Debug, Default)]
pub(crate) struct OnFrameInstallSession {
    pub workflow_state: OnFrameInstallState,
    pub source_candidates: OnFrameCandidateCollection,
    pub selected_candidate: Option<OnFrameGuiSource>,
    pub binding: Option<OnFrameGuiBinding>,
    pub plan: Option<DolphinOnFrameInstallPlan>,
    pub preview: Option<DolphinOnFrameInstallPreview>,
    pub transaction: Option<SharedApplyResult>,
    pub rollback_available: bool,
    pub recovery_required: Option<String>,
    pub error: Option<String>,
}

impl OnFrameInstallSession {
    pub(crate) fn begin_discovery(&mut self, source: PathBuf) {
        self.clear_install_state();
        self.selected_candidate = None;
        self.source_candidates = OnFrameCandidateCollection {
            candidates: Vec::new(),
            selected: None,
            discovery_error: None,
            discovering: true,
        };
        self.workflow_state.begin_discovery(source);
    }

    pub(crate) fn complete_discovery(
        &mut self,
        result: Result<Vec<DolphinOnFrameCandidate>, DolphinOnFrameSourceError>,
    ) {
        self.clear_install_state();
        self.selected_candidate = None;
        self.source_candidates.clear_selection();
        match result {
            Ok(candidates) => {
                self.source_candidates.candidates =
                    candidates.into_iter().map(OnFrameGuiSource::from).collect();
                self.source_candidates.discovering = false;
                self.source_candidates.discovery_error = None;
                self.workflow_state = OnFrameInstallState::Idle;
            }
            Err(error) => {
                self.source_candidates.discovering = false;
                self.source_candidates.discovery_error = Some(error.to_string());
                self.error = Some(error.to_string());
                self.workflow_state = OnFrameInstallState::Failed {
                    message: error.to_string(),
                };
            }
        }
    }

    pub(crate) fn discover(&mut self, source: PathBuf, platform: CheatPlatform) {
        self.begin_discovery(source.clone());
        let result = discover_dolphin_onframe_candidates(&source, platform);
        self.complete_discovery(result);
    }

    pub(crate) fn select_candidate(&mut self, index: usize) -> bool {
        let Some(source) = self.source_candidates.candidates.get(index).cloned() else {
            self.error = Some("OnFrame candidate selection is out of range".into());
            return false;
        };
        self.clear_install_state();
        self.source_candidates.selected = Some(index);
        self.selected_candidate = Some(source.clone());
        self.workflow_state
            .select_candidate(source.candidate.clone());
        true
    }

    pub(crate) fn bind_selected(
        &mut self,
        verified_game_id: Option<&str>,
        profile_root: Option<&Path>,
        conflicting_identity: bool,
    ) -> bool {
        let Some(source) = self.source_candidates.selected().cloned() else {
            self.set_error("Select an OnFrame candidate before binding it");
            return false;
        };
        let Some(profile_root) = profile_root else {
            self.set_error("Select a Dolphin profile before binding this OnFrame patch");
            return false;
        };
        if source.candidate.document.source_format
            != archivefs_core::patch_manager::CheatSourceFormat::DolphinOnFrame
        {
            self.set_error("only Dolphin OnFrame candidates can enter this workflow");
            return false;
        }
        let backend = match bind_dolphin_onframe_candidate(
            &source.candidate,
            verified_game_id,
            profile_root,
            conflicting_identity,
        ) {
            Ok(binding) => binding,
            Err(error) => {
                self.set_error(error.to_string());
                return false;
            }
        };
        self.clear_plan_and_apply_state();
        self.binding = Some(OnFrameGuiBinding::from_backend(
            source.clone(),
            profile_root.to_path_buf(),
            backend.clone(),
        ));
        self.workflow_state = OnFrameInstallState::Bound { binding: backend };
        true
    }

    pub(crate) fn retain_plan_and_preview(
        &mut self,
        plan: DolphinOnFrameInstallPlan,
        preview: DolphinOnFrameInstallPreview,
    ) -> bool {
        if self.binding.is_none() {
            self.set_error("OnFrame preview requires a verified binding");
            return false;
        }
        let status = plan.status;
        self.plan = Some(plan);
        self.preview = Some(preview);
        self.workflow_state.preview(status);
        true
    }

    pub(crate) fn request_confirmation(&mut self) {
        if self
            .binding
            .as_ref()
            .is_some_and(|binding| binding.can_install)
            && self.plan.as_ref().is_some_and(|plan| plan.can_apply)
        {
            self.workflow_state.confirm();
        } else {
            self.set_error("OnFrame confirmation requires an installable preview");
        }
    }

    pub(crate) fn begin_apply(&mut self) {
        self.workflow_state.approve();
    }

    pub(crate) fn retain_apply_result(&mut self, result: SharedApplyResult) {
        let transaction_id = result.journal.operation_id.clone();
        let needs_recovery = matches!(
            result.journal.status,
            archivefs_core::patch_manager::SharedApplyStatus::PartialFailure
                | archivefs_core::patch_manager::SharedApplyStatus::Failed
        );
        self.rollback_available = result.journal.rollback_operation_id.is_some();
        self.transaction = Some(result);
        if needs_recovery {
            self.recovery_required = Some("Dolphin OnFrame apply requires recovery review".into());
            self.workflow_state.recovery_required(
                self.recovery_required
                    .as_deref()
                    .expect("recovery detail was just stored"),
            );
            return;
        }
        let status = self
            .plan
            .as_ref()
            .map(|plan| plan.status)
            .unwrap_or(DolphinOnFrameInstallStatus::Refused);
        self.workflow_state.complete(status, Some(transaction_id));
    }

    pub(crate) fn reset_for_game_identity_change(&mut self) {
        self.clear_binding_and_apply_state();
    }

    pub(crate) fn reset_for_profile_change(&mut self) {
        self.clear_binding_and_apply_state();
    }

    pub(crate) fn reset_for_candidate_rediscovery(&mut self) {
        self.clear_install_state();
        self.source_candidates.clear_selection();
        self.selected_candidate = None;
        self.workflow_state = OnFrameInstallState::Idle;
    }

    pub(crate) fn view_model(&self, selected_game: Option<&str>) -> OnFrameInstallViewModel {
        let Some(binding) = self.binding.as_ref() else {
            return OnFrameInstallViewModel {
                game: selected_game.map(str::to_owned),
                ..Default::default()
            };
        };
        let plan = self.plan.as_ref();
        OnFrameInstallViewModel {
            source: Some(binding.candidate.provenance.display().to_string()),
            game: selected_game.map(str::to_owned),
            game_id: Some(binding.verified_game_id.clone()),
            profile: Some(binding.profile_root.display().to_string()),
            destination: Some(binding.gamesettings_destination.display().to_string()),
            patch: plan.map(|plan| plan.patch_name.clone()),
            exact_operations: plan.map_or(0, |plan| plan.lines.len()),
            unsupported_operations: binding.candidate.document.issues.len(),
            status: plan.map(|plan| format!("{:?}", plan.status)),
            can_install: binding.can_install && plan.is_some_and(|plan| plan.can_apply),
            rollback_available: self.rollback_available,
        }
    }

    fn set_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.error = Some(message.clone());
        self.workflow_state = OnFrameInstallState::Failed { message };
    }

    fn clear_install_state(&mut self) {
        self.clear_binding_and_apply_state();
        self.error = None;
    }

    fn clear_binding_and_apply_state(&mut self) {
        self.binding = None;
        self.clear_plan_and_apply_state();
        self.workflow_state = if let Some(candidate) = self.selected_candidate.as_ref() {
            OnFrameInstallState::CandidateSelected {
                candidate: candidate.candidate.clone(),
            }
        } else {
            OnFrameInstallState::Idle
        };
    }

    fn clear_plan_and_apply_state(&mut self) {
        self.plan = None;
        self.preview = None;
        self.transaction = None;
        self.rollback_available = false;
        self.recovery_required = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::patch_manager::{
        CheatOperation, CheatSourceFormat, SharedApplyContext, SharedApplyJournal,
        SharedTransactionPath, StagedDolphinOnFrameIni,
    };

    fn candidate(platform: CheatPlatform, title: &str) -> DolphinOnFrameCandidate {
        DolphinOnFrameCandidate {
            title: title.into(),
            source_path: PathBuf::from("source.ini"),
            platform: platform.clone(),
            document: CheatDocument {
                title: title.into(),
                platform,
                source_format: CheatSourceFormat::DolphinOnFrame,
                operations: vec![CheatOperation::OnFrameWrite32 {
                    address: 1,
                    value: 2,
                }],
                issues: Vec::new(),
                provenance: vec!["source.ini".into()],
            },
            warnings: Vec::new(),
        }
    }

    fn selected_session(platform: CheatPlatform) -> OnFrameInstallSession {
        let mut session = OnFrameInstallSession::default();
        session.complete_discovery(Ok(vec![candidate(platform, "60 FPS")]));
        assert!(session.select_candidate(0));
        session
    }

    #[test]
    fn discovery_and_selection_are_typed_and_stored() {
        let mut session = OnFrameInstallSession::default();
        session.complete_discovery(Ok(vec![candidate(CheatPlatform::GameCube, "60 FPS")]));
        assert_eq!(session.source_candidates.candidates.len(), 1);
        assert!(session.select_candidate(0));
        assert_eq!(session.source_candidates.selected, Some(0));
        assert!(matches!(
            session.workflow_state,
            OnFrameInstallState::CandidateSelected { .. }
        ));
    }

    #[test]
    fn binding_requires_verified_identity_and_keeps_platform_profile_and_destination() {
        let mut session = selected_session(CheatPlatform::GameCube);
        assert!(!session.bind_selected(None, Some(Path::new("/dolphin")), false));
        assert!(session.bind_selected(Some("GMSE01"), Some(Path::new("/dolphin")), false));
        let binding = session.binding.as_ref().unwrap();
        assert_eq!(binding.platform, CheatPlatform::GameCube);
        assert_eq!(binding.verified_game_id, "GMSE01");
        assert_eq!(binding.profile_root, PathBuf::from("/dolphin"));
        assert_eq!(
            binding.gamesettings_destination,
            PathBuf::from("/dolphin/GameSettings/GMSE01.ini")
        );
    }

    #[test]
    fn conflicting_identity_is_refused_and_other_formats_cannot_be_selected() {
        let mut session = selected_session(CheatPlatform::Wii);
        assert!(!session.bind_selected(Some("RMGE01"), Some(Path::new("/dolphin")), true));
        let mut wrong = OnFrameInstallSession::default();
        let mut ar = candidate(CheatPlatform::GameCube, "AR");
        ar.document.source_format = CheatSourceFormat::DolphinActionReplay;
        wrong.complete_discovery(Ok(vec![ar]));
        assert!(wrong.select_candidate(0));
        assert!(!wrong.bind_selected(Some("GMSE01"), Some(Path::new("/dolphin")), false));
    }

    #[test]
    fn identity_profile_candidate_and_rediscovery_resets_clear_stale_install_state() {
        let mut session = selected_session(CheatPlatform::GameCube);
        assert!(session.bind_selected(Some("GMSE01"), Some(Path::new("/dolphin")), false));
        session.reset_for_game_identity_change();
        assert!(session.binding.is_none());
        assert!(session.preview.is_none());
        assert!(session.transaction.is_none());
        session.reset_for_profile_change();
        session.reset_for_candidate_rediscovery();
        assert!(session.selected_candidate.is_none());
        assert!(session.source_candidates.selected.is_none());
    }

    #[test]
    fn preview_and_view_model_are_retained_before_confirmation() {
        let mut session = selected_session(CheatPlatform::GameCube);
        assert!(session.bind_selected(Some("GMSE01"), Some(Path::new("/dolphin")), false));
        let plan = DolphinOnFrameInstallPlan {
            game_id: "GMSE01".into(),
            revision: None,
            destination_file_name: "GMSE01.ini".into(),
            patch_name: "60 FPS".into(),
            lines: vec!["04000000 00000001 00000002".into()],
            new_contents: "$60 FPS\n04000000 00000001 00000002\n".into(),
            status: DolphinOnFrameInstallStatus::Conflict,
            can_apply: false,
            conflicts: vec!["existing patch differs".into()],
            warnings: vec!["review required".into()],
        };
        let preview = DolphinOnFrameInstallPreview {
            report: archivefs_core::patch_manager::SharedPreviewReport {
                request_archive: PathBuf::from("source.ini"),
                adapter: archivefs_core::patch_manager::PreviewAdapter::Dolphin,
                entries: Vec::new(),
                conflicts: Vec::new(),
                warnings: Vec::new(),
                summary: Default::default(),
                complete: true,
            },
            staged: StagedDolphinOnFrameIni {
                staging_root: PathBuf::from("/tmp/stage"),
                path: PathBuf::from("/tmp/stage/GMSE01.ini"),
                digest: "a".repeat(64),
                contents: "$60 FPS\n".into(),
                destination_existed: true,
                destination_file_name: "GMSE01.ini".into(),
            },
        };
        assert!(session.retain_plan_and_preview(plan, preview));
        assert!(session.preview.is_some());
        assert!(matches!(
            session.workflow_state,
            OnFrameInstallState::PreviewReady {
                status: DolphinOnFrameInstallStatus::Conflict,
                ..
            }
        ));
        session.request_confirmation();
        assert!(matches!(
            session.workflow_state,
            OnFrameInstallState::Failed { .. }
        ));
        let view = session.view_model(Some("Example Game"));
        assert_eq!(view.game_id.as_deref(), Some("GMSE01"));
        assert_eq!(view.patch.as_deref(), Some("60 FPS"));
        assert!(!view.can_install);
    }

    #[test]
    fn apply_result_retains_transaction_rollback_and_recovery_state() {
        let mut session = selected_session(CheatPlatform::Wii);
        assert!(session.bind_selected(Some("RMGE01"), Some(Path::new("/dolphin")), false));
        let path = SharedTransactionPath::from_path(Path::new("/dolphin/GameSettings"));
        let journal = SharedApplyJournal {
            schema_version: 1,
            operation_id: "onframe-op".into(),
            plan_id: "onframe-plan".into(),
            timestamp_unix_seconds: 1,
            context: SharedApplyContext {
                adapter: archivefs_core::patch_manager::PreviewAdapter::Dolphin,
                selected_archive: path.clone(),
                verified_game_identity: "RMGE01".into(),
                profile_id: "profile".into(),
                source_mode: "Dolphin OnFrame".into(),
            },
            approved_source_root: path.clone(),
            destination_root: path,
            created_root_directories: Vec::new(),
            dry_run: false,
            entries: Vec::new(),
            status: archivefs_core::patch_manager::SharedApplyStatus::PartialFailure,
            rollback_operation_id: Some("rollback-op".into()),
        };
        session.workflow_state = OnFrameInstallState::Applying {
            binding: session.binding.as_ref().unwrap().backend.clone(),
        };
        session.retain_apply_result(SharedApplyResult {
            journal,
            journal_path: None,
            journal_failure: None,
        });
        assert_eq!(
            session.transaction.as_ref().unwrap().journal.operation_id,
            "onframe-op"
        );
        assert!(session.rollback_available);
        assert!(session.recovery_required.is_some());
        assert!(matches!(
            session.workflow_state,
            OnFrameInstallState::RecoveryRequired { .. }
        ));
    }
}
