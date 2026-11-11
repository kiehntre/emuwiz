//! Blue Flycast actions over core's existing Flycast planner: the stamped
//! GDI preview, plus a direct CHD/CDI request that re-verifies identity,
//! profile, BIOS and the exact file at click time (same pattern as the
//! SameBoy/mGBA offers).
use super::*;
use archivefs_core::launch::process_spawn::CapturedFileIdentity;
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
pub(crate) enum Request {
    Gdi {
        preview: Preview,
        roots: FlycastProfileDiscoveryRoots,
    },
    Direct(DirectRequest),
}
impl Request {
    pub(crate) fn key(&self) -> (PathBuf, String) {
        let path = match self {
            Self::Gdi { preview, .. } => preview.request.selected_content_path.clone(),
            Self::Direct(direct) => direct.path.clone(),
        };
        (path, "flycast".into())
    }
    pub(crate) fn execute(self) -> Result<archivefs_core::launch::LaunchedFlycastProcess, String> {
        match self {
            Self::Gdi { preview, roots } => preview.launch(&roots),
            Self::Direct(direct) => direct.execute(),
        }
    }
}

/// Extensions the direct (non-GDI) Flycast Play route accepts. GDI keeps its
/// companion-freshness preview; ZIP and other archives are never launched.
pub(crate) const DIRECT_EXTENSIONS: [&str; 2] = ["chd", "cdi"];

fn direct_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| DIRECT_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// A Ready standalone Flycast candidate for one direct `.chd`/`.cdi` file.
/// Building it reads only metadata; [`DirectRequest::execute`] repeats the
/// full identity, profile, BIOS and file checks before anything starts.
#[derive(Clone)]
pub(crate) struct DirectRequest {
    path: PathBuf,
    profile_id: String,
    platform_id: String,
    game_key: String,
    file: CapturedFileIdentity,
}

impl DirectRequest {
    fn offer(plan: &LaunchPlan, candidate: &LaunchCandidate) -> Result<Self, String> {
        let LaunchTarget::Standalone {
            adapter_id: "flycast",
            profile_id,
            ..
        } = &candidate.target
        else {
            return Err("The selected candidate is not standalone Flycast.".into());
        };
        if candidate.readiness != LaunchReadiness::Ready
            || !candidate.blockers.is_empty()
            || !candidate.warnings.is_empty()
            || candidate.content.requires_mount
            || candidate.content.container != Some(LaunchContainerKind::PlainFile)
        {
            return Err(
                "Flycast only starts from a clean check of a direct Dreamcast disc file. Resolve the listed blockers first."
                    .into(),
            );
        }
        if plan.platform_id.as_deref() != Some("Dreamcast") {
            return Err("Flycast requires a verified Dreamcast identity.".into());
        }
        let game_key = plan
            .game_key
            .clone()
            .ok_or("Game identity is unresolved. Recheck this game.")?;
        let path = candidate
            .content
            .resolved_path
            .clone()
            .ok_or("No direct disc path is available.")?;
        if !path.is_absolute() || !direct_extension(&path) {
            return Err(
                "Flycast Play needs a direct .chd or .cdi disc file here; archives are not opened."
                    .into(),
            );
        }
        Ok(Self {
            file: capture(&path)?,
            path,
            profile_id: profile_id.clone(),
            platform_id: "Dreamcast".into(),
            game_key,
        })
    }

    fn execute(self) -> Result<archivefs_core::launch::LaunchedFlycastProcess, String> {
        let roots = FlycastProfileDiscoveryRoots::from_environment()
            .map_err(|e| format!("Flycast profile roots are unavailable: {e}"))?;
        if capture(&self.path)? != self.file {
            return Err("The disc file changed since Play was offered. Recheck this game.".into());
        }
        let profile = discover_flycast_profiles(&roots)
            .profiles
            .into_iter()
            .find(|p| p.profile_id == self.profile_id)
            .ok_or("The selected Flycast profile was not rediscovered. Recheck emulator setup.")?;
        let binding = resolve_flycast_native_launch_binding(&profile)
            .map_err(|e| format!("Flycast cannot be used: {}", e.detail))?;
        let report = archivefs_core::game_identity::inspect_catalogued_game_identity(
            &self.path,
            Some("Dreamcast"),
        );
        let (identity, facts) =
            archivefs_core::launch::evidence_bridge::canonical_identity_from_game_report(&report);
        match &identity {
            CanonicalIdentityStatus::Resolved(id) if id.game_key == self.game_key => {}
            _ => {
                return Err(
                    "Fresh disc identity no longer matches the selected game. Recheck this game."
                        .into(),
                );
            }
        }
        let product = facts
            .iter()
            .find_map(|f| match f {
                archivefs_core::launch::VerifiedIdentityFact::DreamcastProductCode(code) => {
                    Some(code.clone())
                }
                _ => None,
            })
            .ok_or("No verified Dreamcast product code was read from the disc.")?;
        let request = FlycastLaunchRequest {
            selected_content_path: self.path.clone(),
            expected_platform_id: self.platform_id.clone(),
            expected_game_key: self.game_key.clone(),
            expected_dreamcast_product_code: product,
            profile_id: self.profile_id.clone(),
            expected_executable: binding.executable,
        };
        let command = archivefs_core::launch::preflight_flycast_launch(&request, &roots)
            .map_err(|e| format!("Flycast was not started: {}", e.detail))?;
        if capture(&self.path)? != self.file {
            return Err(
                "The disc file changed during the launch check. Nothing was started.".into(),
            );
        }
        archivefs_core::launch::spawn_flycast(command)
            .map_err(|e| format!("Flycast could not start: {e:?}"))
    }
}

fn capture(path: &Path) -> Result<CapturedFileIdentity, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("The Dreamcast disc is not a regular, non-symlink file.".into());
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

/// The direct CHD/CDI offer for one candidate, or `None` when this route
/// does not apply (not Flycast, or a GDI handled by the stamped preview).
pub(crate) fn direct_offer_for(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<Result<Request, String>> {
    if !matches!(
        candidate.target,
        LaunchTarget::Standalone {
            adapter_id: "flycast",
            ..
        }
    ) {
        return None;
    }
    let path = candidate.content.resolved_path.as_deref()?;
    direct_extension(path).then(|| DirectRequest::offer(plan, candidate).map(Request::Direct))
}

/// Dreamcast route preference. When a standalone Flycast profile exists and
/// nothing is remembered, standalone Flycast is the automatic choice: a
/// RetroArch Flycast core stays available only as an explicit emulator
/// choice, so Play never silently switches emulators. If standalone Flycast
/// is installed but blocked, Play reports Flycast's own reason.
///
/// `reasons` maps a Flycast profile id to the adapter's own reason it cannot
/// be used (not set up, executable missing/ambiguous); it replaces the
/// planner's generic "profile is not eligible" text.
pub(crate) fn prefer_standalone(plan: &mut LaunchPlan, reasons: &[(String, String)]) {
    for candidate in &mut plan.candidates {
        let LaunchTarget::Standalone {
            adapter_id: "flycast",
            profile_id,
            ..
        } = &candidate.target
        else {
            continue;
        };
        if let Some((_, reason)) = reasons.iter().find(|(id, _)| id == profile_id) {
            for blocker in &mut candidate.blockers {
                if blocker.kind == archivefs_core::launch::LaunchBlockerKind::ProfileIneligible {
                    blocker.detail = reason.clone();
                }
            }
        }
    }
    if plan.platform_id.as_deref() != Some("Dreamcast")
        || plan
            .candidates
            .iter()
            .any(|c| c.preference == CandidatePreference::Remembered)
    {
        return;
    }
    let flycast = |c: &LaunchCandidate| {
        matches!(
            c.target,
            LaunchTarget::Standalone {
                adapter_id: "flycast",
                ..
            }
        )
    };
    let Some(best) = plan
        .candidates
        .iter()
        .position(|c| flycast(c) && c.readiness == LaunchReadiness::Ready)
        .or_else(|| plan.candidates.iter().position(|c| flycast(c)))
    else {
        return;
    };
    for (index, candidate) in plan.candidates.iter_mut().enumerate() {
        if index == best {
            candidate.preference = CandidatePreference::SoleEligible;
        } else if candidate.preference == CandidatePreference::SoleEligible {
            candidate.preference = CandidatePreference::Undetermined;
        }
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
        Some(Request::Gdi {
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
        CandidatePreference, FirmwareReadiness, LaunchContentKind, ResolvedIdentity,
        StandaloneProfileInput, build_launch_plan,
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
    fn direct_plan(path: &Path) -> LaunchPlan {
        let roots = FlycastProfileDiscoveryRoots::from_environment().unwrap();
        let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Dreamcast".into(),
            game_key: "T-8109N".into(),
        });
        let content = LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(path.to_path_buf()),
            requires_mount: false,
            provenance: "synthetic planner input; no payload verification".into(),
        };
        let profiles = [StandaloneProfileInput {
            adapter_id: "flycast",
            profile_id: "flycast:native".into(),
            profile_path: Some(roots.xdg_config_home.join("flycast")),
            eligible: true,
            firmware: FirmwareReadiness::Verified,
        }];
        let env = archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![],
            diagnostics: vec![],
        };
        build_launch_plan(&identity, &content, &profiles, &env, &[])
    }
    #[test]
    fn direct_chd_and_cdi_get_a_click_time_offer_but_gdi_keeps_its_preview() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["game.chd", "game.CDI"] {
            let path = dir.path().join(name);
            std::fs::write(&path, b"disc").unwrap();
            let plan = direct_plan(&path);
            let offer = direct_offer_for(&plan, &plan.candidates[0]).expect("Flycast applies");
            assert!(matches!(offer, Ok(Request::Direct(_))), "{name}");
        }
        let gdi = dir.path().join("game.gdi");
        std::fs::write(&gdi, b"3").unwrap();
        let plan = direct_plan(&gdi);
        assert!(direct_offer_for(&plan, &plan.candidates[0]).is_none());
        let zip = dir.path().join("game.zip");
        std::fs::write(&zip, b"PK").unwrap();
        let plan = direct_plan(&zip);
        assert!(direct_offer_for(&plan, &plan.candidates[0]).is_none());
    }
    #[test]
    fn direct_offer_refuses_warnings_archives_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.chd");
        std::fs::write(&path, b"disc").unwrap();
        let plan = direct_plan(&path);
        let mut warned = plan.candidates[0].clone();
        warned.readiness = LaunchReadiness::ReadyWithWarnings;
        assert!(matches!(direct_offer_for(&plan, &warned), Some(Err(_))));
        let mut archived = plan.candidates[0].clone();
        archived.content.container = Some(LaunchContainerKind::Archive);
        assert!(matches!(direct_offer_for(&plan, &archived), Some(Err(_))));
        let link = dir.path().join("link.chd");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let plan = direct_plan(&link);
        assert!(matches!(
            direct_offer_for(&plan, &plan.candidates[0]),
            Some(Err(_))
        ));
    }
    #[test]
    fn standalone_flycast_outranks_a_hinted_route_and_explains_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.chd");
        std::fs::write(&path, b"disc").unwrap();
        let mut plan = direct_plan(&path);
        let mut other = plan.candidates[0].clone();
        other.target = LaunchTarget::Standalone {
            adapter_id: "another_adapter",
            profile_id: "other".into(),
            profile_path: None,
        };
        other.preference = CandidatePreference::SoleEligible;
        plan.candidates.push(other);
        plan.candidates[0].preference = CandidatePreference::Undetermined;
        prefer_standalone(&mut plan, &[]);
        assert_eq!(
            plan.candidates[0].preference,
            CandidatePreference::SoleEligible
        );
        assert_eq!(
            plan.candidates[1].preference,
            CandidatePreference::Undetermined
        );

        // A remembered explicit choice is never overridden.
        let mut remembered = direct_plan(&path);
        remembered.candidates[0].preference = CandidatePreference::Remembered;
        prefer_standalone(&mut remembered, &[]);
        assert_eq!(
            remembered.candidates[0].preference,
            CandidatePreference::Remembered
        );

        // An installed but never-opened Flycast says so, by name.
        let mut blocked = direct_plan(&path);
        blocked.candidates[0]
            .blockers
            .push(archivefs_core::launch::LaunchBlocker::new(
                archivefs_core::launch::LaunchBlockerKind::ProfileIneligible,
                "the discovered profile is not eligible",
            ));
        blocked.candidates[0].readiness = LaunchReadiness::Blocked;
        prefer_standalone(
            &mut blocked,
            &[(
                "flycast:native".into(),
                archivefs_core::patch_manager::FLYCAST_NOT_SET_UP.into(),
            )],
        );
        assert_eq!(
            blocked.candidates[0].preference,
            CandidatePreference::SoleEligible
        );
        assert!(
            blocked.candidates[0].blockers[0]
                .detail
                .contains("open Flycast once")
        );
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
