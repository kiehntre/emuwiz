//! Blue GDI action over core's existing Flycast planner and stamped preview.
use super::*;
use archivefs_core::{
    launch::{FlycastLaunchRequest, LaunchContentRef, flycast_gdi_readiness::Preview},
    patch_manager::{
        FlycastProfileDiscoveryRoots, discover_flycast_profiles,
        resolve_flycast_native_launch_binding,
    },
};
#[derive(Clone)]
pub(crate) struct Context {
    pub selected: PathBuf,
    pub identity: CanonicalIdentityStatus,
    pub roots: FlycastProfileDiscoveryRoots,
    pub previews: Vec<(String, Result<Preview, String>)>,
}
#[derive(Clone)]
pub(crate) struct Request {
    preview: Preview,
    roots: FlycastProfileDiscoveryRoots,
}
impl Request {
    pub(crate) fn key(&self) -> (PathBuf, String) {
        (
            self.preview.request.selected_content_path.clone(),
            "flycast".into(),
        )
    }
    pub(crate) fn execute(self) -> Result<archivefs_core::launch::LaunchedFlycastProcess, String> {
        self.preview.launch(&self.roots)
    }
}
impl Context {
    /// Worker-only filesystem inspection; no game launch or payload verification.
    pub(crate) fn inspect(selected: PathBuf) -> Result<Self, String> {
        let report = archivefs_core::game_identity::inspect_catalogued_game_identity(
            &selected,
            Some("Dreamcast"),
        );
        let (identity, facts) =
            archivefs_core::launch::evidence_bridge::canonical_identity_from_game_report(&report);
        let CanonicalIdentityStatus::Resolved(id) = &identity else {
            return Err("Dreamcast identity is unresolved or conflicting".into());
        };
        let product = facts
            .iter()
            .find_map(|f| {
                if let archivefs_core::launch::VerifiedIdentityFact::DreamcastProductCode(code) = f
                {
                    Some(code.clone())
                } else {
                    None
                }
            })
            .ok_or("No verified Dreamcast product-code evidence")?;
        let roots = FlycastProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?;
        let discovery = discover_flycast_profiles(&roots);
        let previews = discovery
            .profiles
            .iter()
            .map(|p| {
                let result = resolve_flycast_native_launch_binding(p)
                    .map_err(|e| e.detail)
                    .and_then(|binding| {
                        Preview::inspect(
                            FlycastLaunchRequest {
                                selected_content_path: selected.clone(),
                                expected_platform_id: id.platform_id.clone(),
                                expected_game_key: id.game_key.clone(),
                                expected_dreamcast_product_code: product.clone(),
                                profile_id: p.profile_id.clone(),
                                expected_executable: binding.executable,
                            },
                            &roots,
                        )
                    });
                (p.profile_id.clone(), result)
            })
            .collect();
        Ok(Self {
            selected,
            identity,
            roots,
            previews,
        })
    }
    pub(crate) fn inputs(
        &self,
        prior: &CanonicalIdentityStatus,
    ) -> Option<(CanonicalIdentityStatus, LaunchContentRef)> {
        if prior != &self.identity || !matches!(prior, CanonicalIdentityStatus::Resolved(_)) {
            return None;
        }
        if !self
            .previews
            .iter()
            .any(|(_, p)| p.as_ref().is_ok_and(|p| !p.expired()))
        {
            return None;
        }
        Some((self.identity.clone(),LaunchContentRef {kind:Some(archivefs_core::launch::LaunchContentKind::OpticalDisc),container:Some(LaunchContainerKind::PlainFile),resolved_path:Some(self.selected.clone()),requires_mount:false,provenance:"GDI structural inspection and SHA256 freshness; no trusted catalogue verification".into()}))
    }
    pub(crate) fn apply(&self, plan: &mut LaunchPlan) {
        for candidate in &mut plan.candidates {
            let LaunchTarget::Standalone {
                adapter_id: "flycast",
                profile_id,
                ..
            } = &candidate.target
            else {
                // This new content projection is authorized only by the stamped
                // Flycast action. Do not make another adapter runnable with it.
                if candidate.content.resolved_path.as_ref() == Some(&self.selected)
                    && candidate.content.kind
                        == Some(archivefs_core::launch::LaunchContentKind::OpticalDisc)
                {
                    candidate.blockers.push(archivefs_core::launch::LaunchBlocker::new(archivefs_core::launch::LaunchBlockerKind::CandidateBlocked,"This GDI preview authorizes native Flycast only; other adapters need companion freshness support"));
                    candidate.readiness = LaunchReadiness::Blocked;
                }
                continue;
            };
            let reason = match self.previews.iter().find(|(id, _)| id == profile_id) {
                Some((_, Ok(p))) if !p.expired() => None,
                Some((_, Err(e))) => Some(e.clone()),
                _ => Some("Refresh Dreamcast readiness before launch".into()),
            };
            if let Some(reason) = reason {
                candidate
                    .blockers
                    .push(archivefs_core::launch::LaunchBlocker::new(
                        archivefs_core::launch::LaunchBlockerKind::FlycastBindingUnavailable,
                        reason,
                    ));
                candidate.readiness = LaunchReadiness::Blocked;
            }
        }
        plan.summary.ready = plan
            .candidates
            .iter()
            .filter(|c| c.readiness == LaunchReadiness::Ready)
            .count();
        plan.summary.ready_with_warnings = plan
            .candidates
            .iter()
            .filter(|c| c.readiness == LaunchReadiness::ReadyWithWarnings)
            .count();
        plan.summary.blocked = plan
            .candidates
            .iter()
            .filter(|c| c.readiness == LaunchReadiness::Blocked)
            .count();
    }
    pub(crate) fn request(
        &self,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
    ) -> Option<Request> {
        let LaunchTarget::Standalone {
            adapter_id: "flycast",
            profile_id,
            ..
        } = &candidate.target
        else {
            return None;
        };
        let preview = self
            .previews
            .iter()
            .find(|(id, _)| id == profile_id)?
            .1
            .as_ref()
            .ok()?;
        if preview.expired()
            || !selection_matches(
                plan,
                candidate,
                &self.selected,
                &preview.request,
                &self.roots,
            )
        {
            return None;
        }
        Some(Request {
            preview: preview.clone(),
            roots: self.roots.clone(),
        })
    }
    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
        state: &mut StandaloneLaunchState,
    ) {
        if state.is_active()
            && !state
                .tracked
                .as_ref()
                .is_some_and(|(path, adapter, _)| path == &self.selected && adapter == "flycast")
        {
            ui.label("Another standalone launch is active; wait before starting Flycast.");
            return;
        }
        if let Some(request) = self.request(plan, candidate) {
            show_standalone_launch_action(ui, state, StandaloneLaunchRequest::Flycast(request));
        }
    }
}

fn selection_matches(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    selected: &Path,
    request: &FlycastLaunchRequest,
    roots: &FlycastProfileDiscoveryRoots,
) -> bool {
    candidate.readiness == LaunchReadiness::Ready
        && candidate.blockers.is_empty()
        && candidate.warnings.is_empty()
        && !candidate.content.requires_mount
        && candidate.content.resolved_path.as_deref() == Some(selected)
        && request.selected_content_path == selected
        && candidate.content.kind == Some(archivefs_core::launch::LaunchContentKind::OpticalDisc)
        && candidate.content.container == Some(LaunchContainerKind::PlainFile)
        && plan.platform_id.as_deref() == Some(request.expected_platform_id.as_str())
        && plan.game_key.as_deref() == Some(request.expected_game_key.as_str())
        && matches!(&candidate.target,LaunchTarget::Standalone {adapter_id:"flycast",profile_id,profile_path}
            if profile_id==&request.profile_id && profile_path.as_ref()==Some(&roots.xdg_config_home.join("flycast")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::{
        FirmwareReadiness, LaunchContentKind, ResolvedIdentity, StandaloneProfileInput,
        build_launch_plan,
    };
    fn fixture() -> (Context, LaunchPlan) {
        let selected = PathBuf::from("/synthetic/game.gdi");
        let roots = FlycastProfileDiscoveryRoots::from_environment().unwrap();
        let profile_id = format!(
            "flycast:{}",
            roots.xdg_config_home.join("flycast").display()
        );
        let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Dreamcast".into(),
            game_key: "T-8109N".into(),
        });
        let content = LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(selected.clone()),
            requires_mount: false,
            provenance: "synthetic planner input; no payload verification".into(),
        };
        let profiles = [StandaloneProfileInput {
            adapter_id: "flycast",
            profile_id: profile_id.clone(),
            profile_path: Some(roots.xdg_config_home.join("flycast")),
            eligible: true,
            firmware: FirmwareReadiness::NotRequired,
        }];
        let env = archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![],
            diagnostics: vec![],
        };
        let plan = build_launch_plan(&identity, &content, &profiles, &env, &[]);
        (
            Context {
                selected,
                identity,
                roots,
                previews: vec![(
                    profile_id,
                    Err("companion changed; refresh required".into()),
                )],
            },
            plan,
        )
    }
    #[test]
    fn failed_companion_preflight_blocks_the_existing_blue_candidate() {
        let (context, mut plan) = fixture();
        assert!(
            plan.candidates
                .iter()
                .any(|c| c.readiness == LaunchReadiness::Ready)
        );
        context.apply(&mut plan);
        assert_eq!(plan.summary.ready, 0);
        assert!(
            plan.candidates
                .iter()
                .all(|c| c.readiness == LaunchReadiness::Blocked)
        );
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|b| b.detail.contains("companion changed"))
        );
        assert!(context.request(&plan, &plan.candidates[0]).is_none());
    }
    #[test]
    fn no_preflight_cannot_upgrade_unknown_or_conflicting_identity() {
        let (context, plan) = fixture();
        assert!(context.inputs(&CanonicalIdentityStatus::Unknown).is_none());
        assert!(
            context
                .inputs(&CanonicalIdentityStatus::Conflicting)
                .is_none()
        );
        assert!(context.request(&plan, &plan.candidates[0]).is_none());
    }
    #[test]
    fn gdi_projection_cannot_unlock_an_unstamped_adapter() {
        let (context, mut plan) = fixture();
        let mut other = plan.candidates[0].clone();
        other.target = LaunchTarget::Standalone {
            adapter_id: "another_adapter",
            profile_id: "other".into(),
            profile_path: None,
        };
        plan.candidates.push(other);
        context.apply(&mut plan);
        assert!(
            plan.candidates
                .iter()
                .all(|c| c.readiness == LaunchReadiness::Blocked)
        );
        assert!(
            plan.candidates[1]
                .blockers
                .iter()
                .any(|b| b.detail.contains("native Flycast only"))
        );
    }
    #[test]
    fn blue_selection_binds_exact_game_profile_and_media() {
        let (context, plan) = fixture();
        let candidate = &plan.candidates[0];
        let request = FlycastLaunchRequest {
            selected_content_path: context.selected.clone(),
            expected_platform_id: "Dreamcast".into(),
            expected_game_key: "T-8109N".into(),
            expected_dreamcast_product_code: "T-8109N".into(),
            profile_id: context.previews[0].0.clone(),
            expected_executable: "/synthetic/flycast".into(),
        };
        let matches = |c: &LaunchCandidate, r: &FlycastLaunchRequest| {
            selection_matches(&plan, c, &context.selected, r, &context.roots)
        };
        assert!(matches(candidate, &request));
        let mut wrong = request.clone();
        wrong.profile_id = "other".into();
        assert!(!matches(candidate, &wrong));
        let mut wrong = request.clone();
        wrong.expected_game_key = "other".into();
        assert!(!matches(candidate, &wrong));
        let mut wrong = request.clone();
        wrong.selected_content_path = "/synthetic/other.gdi".into();
        assert!(!matches(candidate, &wrong));
        let mut wrong = candidate.clone();
        wrong.content.requires_mount = true;
        assert!(!matches(&wrong, &request));
        let mut wrong = candidate.clone();
        wrong.content.container = Some(LaunchContainerKind::Archive);
        assert!(!matches(&wrong, &request));
        let mut wrong = candidate.clone();
        wrong.readiness = LaunchReadiness::ReadyWithWarnings;
        assert!(!matches(&wrong, &request));
    }
}
