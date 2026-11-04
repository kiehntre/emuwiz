use super::*;
use crate::emulator_environment::retroarch::{
    ProfileKind, ProfileRef, ProfileScope, RetroArchEnvironmentReport,
};
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    RememberedPreference, ResolvedIdentity, StandaloneProfileInput, build_launch_plan,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchBlockerKind};

const APPIMAGE: &str = "dolphin:appimage:/home/player/Applications/Dolphin/User";
const NATIVE: &str = "dolphin:/home/player/.config/dolphin-emu";
const GAME: &str = "GZCE51";

fn option(profile_id: &str) -> String {
    format!("standalone/dolphin/{profile_id}")
}

fn content(path: &str, requires_mount: bool) -> LaunchContentRef {
    LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(if requires_mount {
            LaunchContainerKind::Archive
        } else {
            LaunchContainerKind::PlainFile
        }),
        resolved_path: (!requires_mount).then(|| path.into()),
        requires_mount,
        provenance: "synthetic fixture".into(),
    }
}

fn plan_with(
    platform: &str,
    content: LaunchContentRef,
    ids: &[&str],
    remembered: &[RememberedPreference],
) -> LaunchPlan {
    let inputs: Vec<_> = ids
        .iter()
        .map(|id| StandaloneProfileInput {
            adapter_id: "dolphin",
            profile_id: (*id).into(),
            profile_path: None,
            eligible: true,
            firmware: FirmwareReadiness::NotRequired,
        })
        .collect();
    build_launch_plan(
        &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: GAME.into(),
        }),
        &content,
        &inputs,
        &RetroArchEnvironmentReport {
            format_version: 1,
            profiles: Vec::new(),
            diagnostics: Vec::new(),
        },
        remembered,
    )
}

fn tie() -> LaunchPlan {
    plan_with(
        "GameCube",
        content("/games/ZooCube (USA).rvz", false),
        &[APPIMAGE, NATIVE],
        &[],
    )
}

fn saved(preferences: &[LaunchPreference]) -> LaunchChoiceInputs<'_> {
    LaunchChoiceInputs {
        once: None,
        preferences: Ok(preferences),
    }
}

fn candidate<'a>(plan: &'a LaunchPlan, profile_id: &str) -> &'a LaunchCandidate {
    plan.candidates
        .iter()
        .find(|c| matches!(&c.target, LaunchTarget::Standalone { profile_id: id, .. } if id == profile_id))
        .expect("candidate exists")
}

fn launchable(plan: &LaunchPlan) -> Vec<String> {
    plan.candidates
        .iter()
        .filter(|c| c.readiness == LaunchReadiness::Ready)
        .map(|c| launch_option_id(&c.target))
        .collect()
}

#[test]
fn an_ambiguous_tie_without_a_choice_asks_and_changes_nothing() {
    let plan = tie();
    assert!(plan.candidates.iter().all(|c| {
        c.readiness == LaunchReadiness::ReadyWithWarnings
            && c.warnings
                .iter()
                .any(|w| w.kind == LaunchWarningKind::MultipleEligibleProfiles)
    }));
    let mut unresolved = plan.clone();
    assert_eq!(
        apply(&mut unresolved, saved(&[])),
        LaunchChoiceOutcome::NoChoice
    );
    assert_eq!(unresolved, plan);
    assert!(plan.candidates.iter().all(|c| is_choosable(&plan, c)));
}

#[test]
fn a_system_default_makes_only_that_option_launchable() {
    let mut plan = tie();
    let preferences = [LaunchPreference::system("GameCube", &option(APPIMAGE))];
    assert_eq!(
        apply(&mut plan, saved(&preferences)),
        LaunchChoiceOutcome::Applied {
            source: LaunchChoiceSource::System,
            option_id: option(APPIMAGE)
        }
    );
    assert_eq!(launchable(&plan), vec![option(APPIMAGE)]);
    let chosen = candidate(&plan, APPIMAGE);
    assert_eq!(chosen.preference, CandidatePreference::Remembered);
    assert_eq!(
        chosen_source(&plan, chosen, saved(&preferences)),
        Some(LaunchChoiceSource::System)
    );
    let other = candidate(&plan, NATIVE);
    assert_eq!(other.readiness, LaunchReadiness::ReadyWithWarnings);
    assert!(
        is_choosable(&plan, other),
        "the other option can still be chosen"
    );
    assert_eq!(chosen_source(&plan, other, saved(&preferences)), None);
    assert_eq!(
        (plan.summary.ready, plan.summary.ready_with_warnings),
        (1, 1)
    );
}

#[test]
fn a_game_override_outranks_the_system_default_and_only_for_that_game() {
    let preferences = [
        LaunchPreference::system("GameCube", &option(APPIMAGE)),
        LaunchPreference::game("GameCube", GAME, &option(NATIVE)),
    ];
    let mut plan = tie();
    assert_eq!(
        apply(&mut plan, saved(&preferences)),
        LaunchChoiceOutcome::Applied {
            source: LaunchChoiceSource::Game,
            option_id: option(NATIVE)
        }
    );
    assert_eq!(launchable(&plan), vec![option(NATIVE)]);

    let mut other_game = tie();
    other_game.game_key = Some("GALE01".into());
    apply(&mut other_game, saved(&preferences));
    assert_eq!(launchable(&other_game), vec![option(APPIMAGE)]);
}

#[test]
fn a_one_time_choice_outranks_saved_choices_for_its_game_only() {
    let preferences = [LaunchPreference::game("GameCube", GAME, &option(NATIVE))];
    let once = LaunchOnceChoice {
        platform_id: "GameCube".into(),
        game_key: GAME.into(),
        option_id: option(APPIMAGE),
    };
    let inputs = LaunchChoiceInputs {
        once: Some(&once),
        preferences: Ok(&preferences),
    };
    let mut plan = tie();
    assert_eq!(
        apply(&mut plan, inputs),
        LaunchChoiceOutcome::Applied {
            source: LaunchChoiceSource::Once,
            option_id: option(APPIMAGE)
        }
    );
    assert_eq!(launchable(&plan), vec![option(APPIMAGE)]);

    let mut other_game = tie();
    other_game.game_key = Some("GALE01".into());
    assert_eq!(
        apply(&mut other_game, inputs),
        LaunchChoiceOutcome::NoChoice
    );
    assert!(launchable(&other_game).is_empty());
}

#[test]
fn a_stale_preference_never_falls_back_even_to_a_sole_valid_option() {
    // Only the AppImage remains; the saved default named a removed profile.
    let mut plan = plan_with(
        "GameCube",
        content("/games/ZooCube (USA).rvz", false),
        &[APPIMAGE],
        &[],
    );
    assert_eq!(launchable(&plan), vec![option(APPIMAGE)]);
    let stale = option("dolphin:appimage:/old/removed/User");
    let preferences = [LaunchPreference::system("GameCube", &stale)];
    assert_eq!(
        apply(&mut plan, saved(&preferences)),
        LaunchChoiceOutcome::PreferredUnavailable {
            source: LaunchChoiceSource::System,
            option_id: stale
        }
    );
    assert!(launchable(&plan).is_empty(), "no silent fallback");
    let only = candidate(&plan, APPIMAGE);
    assert!(is_choosable(&plan, only));
    assert!(only.warnings.iter().any(|w| {
        w.kind == LaunchWarningKind::PreferredLaunchChoiceUnavailable
            && w.detail.contains("is no longer available")
    }));

    // Choosing again explicitly recovers.
    let once = LaunchOnceChoice {
        platform_id: "GameCube".into(),
        game_key: GAME.into(),
        option_id: option(APPIMAGE),
    };
    let mut retried = plan_with(
        "GameCube",
        content("/games/ZooCube (USA).rvz", false),
        &[APPIMAGE],
        &[],
    );
    apply(
        &mut retried,
        LaunchChoiceInputs {
            once: Some(&once),
            preferences: Ok(&preferences),
        },
    );
    assert_eq!(launchable(&retried), vec![option(APPIMAGE)]);
}

#[test]
fn a_blocked_preferred_option_is_reported_and_never_unblocked() {
    let mut plan = tie();
    let native = plan
        .candidates
        .iter_mut()
        .find(|c| launch_option_id(&c.target) == option(NATIVE))
        .unwrap();
    native
        .blockers
        .push(crate::launch::readiness::LaunchBlocker::new(
            LaunchBlockerKind::DolphinBindingUnavailable,
            "No Dolphin program was found",
        ));
    native.readiness = LaunchReadiness::Blocked;
    let preferences = [LaunchPreference::system("GameCube", &option(NATIVE))];
    assert!(matches!(
        apply(&mut plan, saved(&preferences)),
        LaunchChoiceOutcome::PreferredUnavailable { .. }
    ));
    assert_eq!(candidate(&plan, NATIVE).readiness, LaunchReadiness::Blocked);
    assert!(launchable(&plan).is_empty());
    assert!(candidate(&plan, APPIMAGE).warnings.iter().any(|w| {
        w.kind == LaunchWarningKind::PreferredLaunchChoiceUnavailable
            && w.detail.contains("is blocked")
    }));
}

#[test]
fn a_choice_never_unblocks_zip_contained_content() {
    let mut plan = plan_with(
        "GameCube",
        content("/games/Animal Crossing (USA).zip", true),
        &[APPIMAGE, NATIVE],
        &[],
    );
    let once = LaunchOnceChoice {
        platform_id: "GameCube".into(),
        game_key: GAME.into(),
        option_id: option(APPIMAGE),
    };
    apply(
        &mut plan,
        LaunchChoiceInputs {
            once: Some(&once),
            preferences: Ok(&[]),
        },
    );
    assert!(plan.candidates.iter().all(|c| {
        c.readiness == LaunchReadiness::Blocked
            && c.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::ContentNotResolved)
    }));
    assert!(plan.candidates.iter().all(|c| !is_choosable(&plan, c)));
}

#[test]
fn unreadable_preferences_hold_every_option_but_a_one_time_choice_still_works() {
    let mut plan = tie();
    assert!(matches!(
        apply(
            &mut plan,
            LaunchChoiceInputs {
                once: None,
                preferences: Err("broken")
            }
        ),
        LaunchChoiceOutcome::PreferencesUnreadable(_)
    ));
    assert!(launchable(&plan).is_empty());

    let mut single = plan_with(
        "GameCube",
        content("/games/ZooCube (USA).rvz", false),
        &[APPIMAGE],
        &[],
    );
    apply(
        &mut single,
        LaunchChoiceInputs {
            once: None,
            preferences: Err("broken"),
        },
    );
    assert!(
        launchable(&single).is_empty(),
        "an unreadable default may name another emulator"
    );

    let once = LaunchOnceChoice {
        platform_id: "GameCube".into(),
        game_key: GAME.into(),
        option_id: option(NATIVE),
    };
    let mut chosen = tie();
    apply(
        &mut chosen,
        LaunchChoiceInputs {
            once: Some(&once),
            preferences: Err("broken"),
        },
    );
    assert_eq!(launchable(&chosen), vec![option(NATIVE)]);
}

#[test]
fn an_explicit_choice_outranks_an_unrelated_remembered_profile() {
    let mut plan = plan_with(
        "GameCube",
        content("/games/ZooCube (USA).rvz", false),
        &[APPIMAGE, NATIVE],
        &[RememberedPreference {
            adapter_id: "dolphin".into(),
            profile_id: NATIVE.into(),
        }],
    );
    assert_eq!(
        launchable(&plan).len(),
        2,
        "planner remembered NATIVE; both Ready"
    );
    apply(
        &mut plan,
        saved(&[LaunchPreference::system("GameCube", &option(APPIMAGE))]),
    );
    assert_eq!(launchable(&plan), vec![option(APPIMAGE)]);
    assert_eq!(
        candidate(&plan, NATIVE).preference,
        CandidatePreference::Undetermined
    );
}

#[test]
fn platforms_not_yet_adopted_are_untouched() {
    for platform in ["Saturn", "PS2", "NES"] {
        let mut plan = tie();
        plan.platform_id = Some(platform.into());
        let before = plan.clone();
        let preferences = [LaunchPreference::system(platform, &option(APPIMAGE))];
        assert_eq!(
            apply(&mut plan, saved(&preferences)),
            LaunchChoiceOutcome::NotAdopted
        );
        assert_eq!(plan, before);
        assert!(!is_choosable(&plan, &plan.candidates[0]));
    }
}

#[test]
fn option_ids_are_stable_and_distinguish_every_launch_identity_fact() {
    let standalone = LaunchTarget::Standalone {
        adapter_id: "dolphin",
        profile_id: APPIMAGE.into(),
        profile_path: Some("/display/only".into()),
    };
    assert_eq!(launch_option_id(&standalone), option(APPIMAGE));
    let retroarch = |kind, scope, core: &str| {
        launch_option_id(&LaunchTarget::RetroArchCore {
            profile: ProfileRef {
                profile_kind: kind,
                scope,
            },
            core_stem: core.into(),
            platform_id: "GameCube",
        })
    };
    assert_eq!(
        retroarch(ProfileKind::Native, ProfileScope::User, "dolphin"),
        "retroarch/native/user/dolphin"
    );
    let mut all = std::collections::BTreeSet::new();
    for id in [
        retroarch(ProfileKind::Native, ProfileScope::User, "dolphin"),
        retroarch(ProfileKind::Flatpak, ProfileScope::User, "dolphin"),
        retroarch(ProfileKind::Flatpak, ProfileScope::System, "dolphin"),
        retroarch(ProfileKind::AppImage, ProfileScope::User, "dolphin"),
        retroarch(ProfileKind::Native, ProfileScope::User, "dolphin_launcher"),
        launch_option_id(&standalone),
    ] {
        assert!(all.insert(id));
    }
}

#[test]
fn preferences_survive_reload_and_replace_only_their_own_slot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config").join(LAUNCH_PREFERENCES_FILE);
    assert_eq!(load_launch_preferences_at(&path), Ok(Vec::new()));
    set_launch_preference_at(
        &path,
        &LaunchPreference::system("GameCube", &option(NATIVE)),
    )
    .unwrap();
    set_launch_preference_at(
        &path,
        &LaunchPreference::game("GameCube", GAME, &option(NATIVE)),
    )
    .unwrap();
    set_launch_preference_at(&path, &LaunchPreference::system("Wii", &option(APPIMAGE))).unwrap();
    set_launch_preference_at(
        &path,
        &LaunchPreference::system("GameCube", &option(APPIMAGE)),
    )
    .unwrap();
    let reloaded = load_launch_preferences_at(&path).unwrap();
    assert_eq!(
        reloaded,
        vec![
            LaunchPreference::system("GameCube", &option(APPIMAGE)),
            LaunchPreference::game("GameCube", GAME, &option(NATIVE)),
            LaunchPreference::system("Wii", &option(APPIMAGE)),
        ]
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("\"version\": 1") && text.contains("\"scope\": \"game\""),
        "{text}"
    );

    clear_launch_preference_at(&path, LaunchPreferenceScope::Game, "GameCube", Some(GAME)).unwrap();
    assert_eq!(load_launch_preferences_at(&path).unwrap().len(), 2);
    clear_launch_preference_at(&path, LaunchPreferenceScope::Game, "GameCube", Some(GAME)).unwrap();
    let mut plan = tie();
    apply(
        &mut plan,
        saved(&load_launch_preferences_at(&path).unwrap()),
    );
    assert_eq!(launchable(&plan), vec![option(APPIMAGE)]);
}

#[test]
fn malformed_or_unsafe_preference_files_are_errors_and_are_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(LAUNCH_PREFERENCES_FILE);
    for bad in [
        "not json",
        r#"{"version":2,"preferences":[]}"#,
        r#"{"version":1,"preferences":[],"extra":true}"#,
        r#"{"version":1,"preferences":[{"scope":"system","platform_id":"GameCube","option_id":"a","name":"Dolphin"}]}"#,
        r#"{"version":1,"preferences":[{"scope":"game","platform_id":"GameCube","option_id":"a"}]}"#,
        r#"{"version":1,"preferences":[{"scope":"system","platform_id":"GameCube","game_key":"G","option_id":"a"}]}"#,
        r#"{"version":1,"preferences":[{"scope":"system","platform_id":"GameCube","option_id":"a"},{"scope":"system","platform_id":"GameCube","option_id":"b"}]}"#,
    ] {
        std::fs::write(&path, bad).unwrap();
        assert!(load_launch_preferences_at(&path).is_err(), "{bad}");
        assert!(
            set_launch_preference_at(&path, &LaunchPreference::system("GameCube", "x")).is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
    }
    std::fs::write(&path, vec![b' '; (MAX_FILE_BYTES + 1) as usize]).unwrap();
    assert!(load_launch_preferences_at(&path).is_err());
    #[cfg(unix)]
    {
        let target = dir.path().join("elsewhere.json");
        std::fs::write(&target, r#"{"version":1,"preferences":[]}"#).unwrap();
        let link = dir.path().join("linked.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(load_launch_preferences_at(&link).is_err());
    }
    let directory = dir.path().join("a-directory");
    std::fs::create_dir(&directory).unwrap();
    assert!(load_launch_preferences_at(&directory).is_err());
}

#[test]
fn incomplete_preferences_are_refused_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(LAUNCH_PREFERENCES_FILE);
    for bad in [
        LaunchPreference::system("", "x"),
        LaunchPreference::system("GameCube", ""),
        LaunchPreference::game("GameCube", "", "x"),
    ] {
        assert!(set_launch_preference_at(&path, &bad).is_err());
    }
    assert!(!path.exists());
}
