use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::*;
use archivefs_core::emulator_environment::retroarch::{
    ProfileKind, ProfileRef, ProfileScope, RetroArchEnvironmentReport,
};
use archivefs_core::launch::launch_choice::LAUNCH_PREFERENCES_FILE;
use archivefs_core::launch::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    LaunchReadiness, LaunchTarget, ResolvedIdentity, build_launch_plan,
};
use archivefs_core::patch_manager::{DolphinLocalDiscoveryRoots, discover_dolphin_local_profiles};

const GAME: &str = "GZCE51";

fn executable(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"#!/bin/false\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Two Dolphin AppImages, each with its own `User/` root, so both profiles
/// have a real launch binding and genuinely tie.
fn two_dolphins(home: &Path) -> super::super::DolphinLaunchContext {
    for name in ["Dolphin", "Dolphin-Dev"] {
        let image = home.join(format!("Applications/{name}/Dolphin.AppImage"));
        executable(&image);
        fs::create_dir_all(image.parent().unwrap().join("User/Config")).unwrap();
        fs::write(
            image.parent().unwrap().join("User/Config/Dolphin.ini"),
            b"[Core]\n",
        )
        .unwrap();
    }
    let roots = DolphinLocalDiscoveryRoots {
        home: home.to_path_buf(),
        xdg_config_home: home.join(".config"),
        xdg_data_home: home.join(".local/share"),
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
        appimage_directory: None,
        dolphin_emu_userpath_override: None,
        path_override: Some(Vec::new()),
    };
    super::super::DolphinLaunchContext {
        discovery: discover_dolphin_local_profiles(&roots),
        roots,
    }
}

fn plan_for(context: &super::super::DolphinLaunchContext, state: &LaunchChoiceState) -> LaunchPlan {
    let profiles = archivefs_core::launch::dolphin_binding_readiness::dolphin_launch_profiles(
        &context.discovery,
        &context.roots,
    );
    let mut plan = build_launch_plan(
        &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "GameCube".into(),
            game_key: GAME.into(),
        }),
        &LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some("/games/ZooCube (USA).rvz".into()),
            requires_mount: false,
            provenance: "synthetic fixture".into(),
        },
        &profiles.inputs,
        &RetroArchEnvironmentReport {
            format_version: 1,
            profiles: Vec::new(),
            diagnostics: Vec::new(),
        },
        &[],
    );
    archivefs_core::launch::dolphin_binding_readiness::apply(&mut plan, &profiles);
    archivefs_core::launch::launch_choice::apply(&mut plan, state.inputs());
    plan
}

fn dolphin_requests(
    plan: &LaunchPlan,
    context: &super::super::DolphinLaunchContext,
) -> Vec<String> {
    plan.candidates
        .iter()
        .filter_map(|candidate| super::super::dolphin_launch_request(plan, candidate, context))
        .map(|request| request.profile_id)
        .collect()
}

fn appimage_option(plan: &LaunchPlan, folder: &str) -> String {
    plan.candidates
        .iter()
        .map(|c| core_choice::launch_option_id(&c.target))
        .find(|id| id.contains(&format!("/Applications/{folder}/User")))
        .expect("option exists")
}

#[test]
fn a_tie_offers_no_launch_until_the_user_chooses_and_then_exactly_one() {
    let home = tempfile::tempdir().unwrap();
    let context = two_dolphins(home.path());
    let mut state = LaunchChoiceState::load_at(home.path().join(LAUNCH_PREFERENCES_FILE));

    let tied = plan_for(&context, &state);
    assert!(
        dolphin_requests(&tied, &context).is_empty(),
        "the tie hides Launch"
    );
    let choosable: Vec<_> = tied
        .candidates
        .iter()
        .filter(|c| core_choice::is_choosable(&tied, c))
        .collect();
    assert_eq!(
        choosable.len(),
        2,
        "each tied option is offered as a choice"
    );
    for candidate in &choosable {
        let chosen = plan_if_chosen(&tied, candidate).expect("a usable option can be chosen");
        let simulated = chosen
            .candidates
            .iter()
            .find(|c| c.target == candidate.target)
            .unwrap();
        assert!(super::super::dolphin_launch_request(&chosen, simulated, &context).is_some());
    }

    let dev = appimage_option(&tied, "Dolphin-Dev");
    state.handle(LaunchChoiceAction::SetDefault(LaunchPreference::system(
        "GameCube", &dev,
    )));
    assert_eq!(state.save_error, None);
    let chosen = plan_for(&context, &state);
    let requests = dolphin_requests(&chosen, &context);
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("Dolphin-Dev"), "{requests:?}");

    // Durable: a fresh state read from disk makes the same choice.
    let reloaded = LaunchChoiceState::load_at(state.path().unwrap().to_path_buf());
    assert_eq!(
        dolphin_requests(&plan_for(&context, &reloaded), &context),
        requests
    );
}

#[test]
fn a_game_default_and_a_one_time_choice_follow_precedence() {
    let home = tempfile::tempdir().unwrap();
    let context = two_dolphins(home.path());
    let mut state = LaunchChoiceState::load_at(home.path().join(LAUNCH_PREFERENCES_FILE));
    let tied = plan_for(&context, &state);
    let stable = appimage_option(&tied, "Dolphin");
    let dev = appimage_option(&tied, "Dolphin-Dev");

    state.handle(LaunchChoiceAction::SetDefault(LaunchPreference::system(
        "GameCube", &stable,
    )));
    state.handle(LaunchChoiceAction::SetDefault(LaunchPreference::game(
        "GameCube", GAME, &dev,
    )));
    let requests = dolphin_requests(&plan_for(&context, &state), &context);
    assert!(requests.len() == 1 && requests[0].contains("Dolphin-Dev"));

    let path = state.path().unwrap().to_path_buf();
    let saved = fs::read_to_string(&path).unwrap();
    state.handle(LaunchChoiceAction::UseOnce(LaunchOnceChoice {
        platform_id: "GameCube".into(),
        game_key: GAME.into(),
        option_id: stable.clone(),
    }));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        saved,
        "once is never saved"
    );
    let requests = dolphin_requests(&plan_for(&context, &state), &context);
    assert!(requests.len() == 1 && !requests[0].contains("Dolphin-Dev"));
    assert!(
        LaunchChoiceState::load_at(path.clone()).once.is_none(),
        "a one-time choice does not survive a restart"
    );

    // Forgetting the game default returns to the system default and drops
    // the one-time choice for that game.
    state.handle(LaunchChoiceAction::ClearDefault {
        scope: LaunchPreferenceScope::Game,
        platform_id: "GameCube".into(),
        game_key: Some(GAME.into()),
    });
    assert!(state.once.is_none());
    let requests = dolphin_requests(&plan_for(&context, &state), &context);
    assert!(requests.len() == 1 && !requests[0].contains("Dolphin-Dev"));
}

#[test]
fn a_removed_preferred_install_shows_a_reason_and_offers_a_new_choice() {
    let home = tempfile::tempdir().unwrap();
    let context = two_dolphins(home.path());
    let mut state = LaunchChoiceState::load_at(home.path().join(LAUNCH_PREFERENCES_FILE));
    let dev = appimage_option(&plan_for(&context, &state), "Dolphin-Dev");
    state.handle(LaunchChoiceAction::SetDefault(LaunchPreference::system(
        "GameCube", &dev,
    )));
    fs::remove_dir_all(home.path().join("Applications/Dolphin-Dev")).unwrap();
    let context = super::super::DolphinLaunchContext {
        discovery: discover_dolphin_local_profiles(&context.roots),
        roots: context.roots.clone(),
    };
    let plan = plan_for(&context, &state);
    assert!(
        dolphin_requests(&plan, &context).is_empty(),
        "no silent fallback"
    );
    let remaining: Vec<_> = plan
        .candidates
        .iter()
        .filter(|c| c.readiness != LaunchReadiness::Blocked)
        .collect();
    assert_eq!(remaining.len(), 1);
    assert!(core_choice::is_choosable(&plan, remaining[0]));
    assert!(remaining[0].warnings.iter().any(|w| {
        w.kind == archivefs_core::launch::LaunchWarningKind::PreferredLaunchChoiceUnavailable
    }));
    assert!(plan_if_chosen(&plan, remaining[0]).is_some());
}

#[test]
fn an_unreadable_preferences_file_is_reported_and_never_overwritten() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(LAUNCH_PREFERENCES_FILE);
    fs::write(&path, "{broken").unwrap();
    let mut state = LaunchChoiceState::load_at(path.clone());
    assert!(state.preferences.is_err());
    state.handle(LaunchChoiceAction::SetDefault(LaunchPreference::system(
        "GameCube",
        "standalone/dolphin/x",
    )));
    assert!(state.save_error.is_some());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");

    let mut detached = LaunchChoiceState::default();
    detached.handle(LaunchChoiceAction::SetDefault(LaunchPreference::system(
        "GameCube",
        "standalone/dolphin/x",
    )));
    assert!(detached.save_error.is_some(), "default state never writes");
}

#[test]
fn a_chosen_retroarch_candidate_carries_its_choice_into_the_launch_request() {
    let candidate = archivefs_core::launch::LaunchCandidate {
        target: LaunchTarget::RetroArchCore {
            profile: ProfileRef {
                profile_kind: ProfileKind::Native,
                scope: ProfileScope::User,
            },
            core_stem: "dolphin".into(),
            platform_id: "GameCube",
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some("/games/ZooCube (USA).iso".into()),
            requires_mount: false,
            provenance: "synthetic fixture".into(),
        },
        firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: archivefs_core::launch::CandidatePreference::Remembered,
    };
    let mut plan = LaunchPlan {
        platform_id: Some("GameCube".into()),
        game_key: Some(GAME.into()),
        candidates: vec![candidate.clone()],
        summary: Default::default(),
        media_topology: None,
    };
    let request = super::super::retroarch_launch_request(&plan, &candidate, None).unwrap();
    assert_eq!(
        request.launch_choice.as_deref(),
        Some("retroarch/native/user/dolphin")
    );

    let mut sole = candidate.clone();
    sole.preference = archivefs_core::launch::CandidatePreference::SoleEligible;
    plan.candidates = vec![sole.clone()];
    assert_eq!(
        super::super::retroarch_launch_request(&plan, &sole, None)
            .unwrap()
            .launch_choice,
        None,
        "an unambiguous candidate needs no choice authority"
    );

    plan.platform_id = Some("MegaDrive".into());
    assert_eq!(
        super::super::retroarch_launch_request(&plan, &candidate, None)
            .unwrap()
            .launch_choice,
        None,
        "platforms not yet adopted never carry a choice"
    );
}

#[test]
fn the_gamer_view_asks_for_a_choice_when_a_preference_is_held() {
    let home = tempfile::tempdir().unwrap();
    let context = two_dolphins(home.path());
    let mut state = LaunchChoiceState::load_at(home.path().join(LAUNCH_PREFERENCES_FILE));
    let tied = plan_for(&context, &state);
    let blocker = super::super::gamer_blocker_for_plan(&tied, true, true);
    assert_eq!(
        blocker.kind,
        super::super::GamerBlockerKind::MultipleChoices
    );

    state.handle(LaunchChoiceAction::SetDefault(LaunchPreference::system(
        "GameCube",
        "standalone/dolphin/dolphin:appimage:/removed/User",
    )));
    let held = plan_for(&context, &state);
    let blocker = super::super::gamer_blocker_for_plan(&held, true, true);
    assert_eq!(
        blocker.kind,
        super::super::GamerBlockerKind::MultipleChoices
    );
    assert!(
        blocker.detail.contains("is no longer available"),
        "{}",
        blocker.detail
    );
}

#[test]
fn a_one_time_choice_is_used_up_by_the_next_launch_that_starts() {
    let home = tempfile::tempdir().unwrap();
    let context = two_dolphins(home.path());
    let mut state = LaunchChoiceState::load_at(home.path().join(LAUNCH_PREFERENCES_FILE));
    let stable = appimage_option(&plan_for(&context, &state), "Dolphin");
    let once = LaunchChoiceAction::UseOnce(LaunchOnceChoice {
        platform_id: "GameCube".into(),
        game_key: GAME.into(),
        option_id: stable,
    });

    // Choosing while an earlier launch is still running keeps the choice
    // until that launch ends and the next one starts.
    state.observe_launches(true);
    state.handle(once.clone());
    state.observe_launches(true);
    assert!(state.once.is_some());
    state.observe_launches(false);
    assert_eq!(
        dolphin_requests(&plan_for(&context, &state), &context).len(),
        1
    );

    state.observe_launches(true);
    assert!(state.once.is_none(), "the launch used the one-time choice");
    assert!(
        dolphin_requests(&plan_for(&context, &state), &context).is_empty(),
        "the tie asks again after the one-time launch"
    );
}
