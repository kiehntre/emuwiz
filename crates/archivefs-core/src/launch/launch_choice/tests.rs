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
            && w.detail.contains("dolphin at /old/removed/User")
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
fn every_platform_can_resolve_a_tie_by_choice_and_a_plan_without_one_is_untouched() {
    // Owner screenshot 2026-11-09: PS2 with three PCSX2 installs showed
    // "Choice needed" and no way to choose. Every platform now resolves a tie
    // the same way GameCube and Wii always did.
    for platform in ["Saturn", "PS2", "NES"] {
        let mut plan = tie();
        plan.platform_id = Some(platform.into());
        let preferences = [LaunchPreference::system(platform, &option(APPIMAGE))];
        assert!(matches!(
            apply(&mut plan, saved(&preferences)),
            LaunchChoiceOutcome::Applied { .. }
        ));
        assert!(is_choosable(&plan, &plan.candidates[0]));
    }
    let mut plan = tie();
    plan.platform_id = None;
    let before = plan.clone();
    assert_eq!(
        apply(&mut plan, saved(&[])),
        LaunchChoiceOutcome::NotAdopted
    );
    assert_eq!(plan, before);
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
fn local_amiga_media_keys_are_never_saved_as_a_game_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(LAUNCH_PREFERENCES_FILE);
    let key = format!("{}{}", crate::launch::LOCAL_ADF_KEY_PREFIX, "ab".repeat(32));
    let error = set_launch_preference_at(
        &path,
        &LaunchPreference::game("Amiga", &key, "standalone/fsuae/x"),
    )
    .unwrap_err();
    assert!(error.contains("launch target only"), "{error}");
    assert!(!path.exists());
}

#[test]
fn launch_only_homebrew_keys_are_never_saved_as_a_game_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config").join(LAUNCH_PREFERENCES_FILE);
    let key = format!(
        "{}{}",
        crate::launch::AZAHAR_HOMEBREW_KEY_PREFIX,
        "ab".repeat(32)
    );
    let error = set_launch_preference_at(
        &path,
        &LaunchPreference::game("Nintendo 3DS", &key, "standalone/azahar/x"),
    )
    .unwrap_err();
    assert!(error.contains("launch target only"), "{error}");
    assert!(!path.exists(), "nothing may be written");
    // System-wide defaults and ordinary game defaults are unaffected.
    set_launch_preference_at(
        &path,
        &LaunchPreference::system("Nintendo 3DS", "standalone/azahar/x"),
    )
    .unwrap();
    set_launch_preference_at(
        &path,
        &LaunchPreference::game("GameCube", GAME, &option(NATIVE)),
    )
    .unwrap();
    let saved = load_launch_preferences_at(&path).unwrap();
    assert_eq!(saved.len(), 2);
    assert!(saved.iter().all(|p| {
        !p.game_key
            .as_deref()
            .is_some_and(|k| k.starts_with("3dsx-sha256:"))
    }));
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

#[test]
fn an_explicit_choice_also_outranks_a_reviewed_hint_preference() {
    let mut plan = tie();
    // A reviewed hint makes the planner prefer one option without a tie.
    for c in &mut plan.candidates {
        if matches!(&c.target, LaunchTarget::Standalone { profile_id, .. } if profile_id == NATIVE)
        {
            c.preference = CandidatePreference::SoleEligible;
        }
    }
    apply(
        &mut plan,
        saved(&[LaunchPreference::system("GameCube", &option(APPIMAGE))]),
    );
    assert_eq!(launchable(&plan), vec![option(APPIMAGE)]);
    assert_eq!(
        candidate(&plan, NATIVE).preference,
        CandidatePreference::Undetermined
    );
    assert_eq!(
        candidate(&plan, APPIMAGE).preference,
        CandidatePreference::Remembered
    );
}

#[test]
fn a_save_that_would_exceed_the_read_limit_is_refused_and_keeps_the_old_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(LAUNCH_PREFERENCES_FILE);
    let mut saved_count = 0;
    let refusal = loop {
        let key = format!("GAME-{saved_count:06}-{}", "k".repeat(64));
        match set_launch_preference_at(
            &path,
            &LaunchPreference::game("GameCube", &key, &option(APPIMAGE)),
        ) {
            Ok(()) => saved_count += 1,
            Err(error) => break error,
        }
        assert!(saved_count < 10_000, "the size limit was never enforced");
    };
    assert!(refusal.contains("size limit"), "{refusal}");
    assert!(std::fs::metadata(&path).unwrap().len() <= MAX_FILE_BYTES);
    let reloaded = load_launch_preferences_at(&path).expect("the old file stays readable");
    assert_eq!(reloaded.len(), saved_count);
}

fn core_tied(mut plan: LaunchPlan, extra: Option<LaunchBlockerKind>) -> LaunchPlan {
    for c in &mut plan.candidates {
        c.warnings.clear();
        c.blockers
            .push(crate::launch::readiness::LaunchBlocker::new(
                LaunchBlockerKind::AmbiguousCore,
                "more than one installed RetroArch core resolves to this system",
            ));
        if let Some(kind) = extra {
            c.blockers
                .push(crate::launch::readiness::LaunchBlocker::new(kind, "other"));
        }
        c.readiness = LaunchReadiness::Blocked;
    }
    plan
}

fn once(platform: &str, profile: &str) -> LaunchOnceChoice {
    LaunchOnceChoice {
        platform_id: platform.into(),
        game_key: GAME.into(),
        option_id: option(profile),
    }
}

/// Census 2026-11-10 (Atari 2600 zip, Neo Geo Pocket Color): several
/// installed cores and no reviewed preference left every option Blocked with
/// "Choice needed" and no way to choose. The person's choice answers exactly
/// that tie, on the option it names only.
#[test]
fn a_core_tie_is_answered_by_an_explicit_choice_and_nothing_else_is_unblocked() {
    let mut plan = core_tied(tie(), None);
    assert!(plan.candidates.iter().all(|c| is_choosable(&plan, c)));
    let choice = once("GameCube", APPIMAGE);
    apply(
        &mut plan,
        LaunchChoiceInputs {
            once: Some(&choice),
            preferences: Ok(&[]),
        },
    );
    assert_eq!(launchable(&plan), vec![option(APPIMAGE)]);
    assert_eq!(candidate(&plan, NATIVE).readiness, LaunchReadiness::Blocked);

    // Any other blocker beside the tie still refuses, chosen or not.
    let mut plan = core_tied(tie(), Some(LaunchBlockerKind::RequiredFirmwareMissing));
    assert!(plan.candidates.iter().all(|c| !is_choosable(&plan, c)));
    apply(
        &mut plan,
        LaunchChoiceInputs {
            once: Some(&choice),
            preferences: Ok(&[]),
        },
    );
    assert!(
        plan.candidates
            .iter()
            .all(|c| c.readiness == LaunchReadiness::Blocked)
    );

    // Saturn keeps its own validated native core selection.
    let mut saturn = core_tied(
        plan_with(
            "Saturn",
            content("/games/Saturn.chd", false),
            &[APPIMAGE, NATIVE],
            &[],
        ),
        None,
    );
    assert!(saturn.candidates.iter().all(|c| !is_choosable(&saturn, c)));
    let choice = once("Saturn", APPIMAGE);
    apply(
        &mut saturn,
        LaunchChoiceInputs {
            once: Some(&choice),
            preferences: Ok(&[]),
        },
    );
    assert!(
        saturn
            .candidates
            .iter()
            .all(|c| c.readiness == LaunchReadiness::Blocked)
    );
}

/// Real-planner fixtures for the core-tie choice (owner review 2026-11-10).
mod core_tie_review {
    use super::*;
    use crate::emulator_environment::retroarch::{
        ConfigFileFinding, ConfigReadOutcome, CoreFinding, CoreInfoFinding, DirectoryProbeFinding,
        Evidence, RetroArchPlaylistInventory, RetroArchProfile,
    };
    use crate::emulator_environment::{EncodedPath, FsProbe};
    use std::path::PathBuf;

    fn core(stem: &str, database: &str) -> CoreFinding {
        CoreFinding {
            file_name: EncodedPath::from_path(&PathBuf::from(format!("{stem}_libretro.so"))),
            full_path: EncodedPath::from_path(&PathBuf::from(format!(
                "/retroarch/cores/{stem}_libretro.so"
            ))),
            core_stem: stem.to_string(),
            info: CoreInfoFinding::Found {
                display_name: None,
                display_version: None,
                system_name: None,
                supported_extensions: Vec::new(),
                core_name: Some(stem.to_string()),
                manufacturer: None,
                categories: None,
                database: Some(database.to_string()),
                firmware: Vec::new(),
            },
        }
    }

    fn environment(kind: ProfileKind, cores: Vec<CoreFinding>) -> RetroArchEnvironmentReport {
        RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![RetroArchProfile {
                profile_kind: kind,
                scope: ProfileScope::User,
                evidence: Evidence {
                    executables: vec![EncodedPath::from_path(&PathBuf::from("/usr/bin/retroarch"))],
                    flatpak_metadata_found: false,
                    config_directory_found: true,
                    config_file_found: true,
                },
                config_directory: DirectoryProbeFinding {
                    path: EncodedPath::from_path(&PathBuf::from("/retroarch")),
                    probe: FsProbe::PresentDirectory,
                },
                config_file: ConfigFileFinding {
                    path: EncodedPath::from_path(&PathBuf::from("/retroarch/retroarch.cfg")),
                    probe: FsProbe::PresentFile,
                    read: ConfigReadOutcome::NotAttempted,
                },
                paths: Vec::new(),
                cores,
                playlists: RetroArchPlaylistInventory {
                    directory: None,
                    playlists: Vec::new(),
                    diagnostics: Vec::new(),
                    complete: true,
                },
                app_images: Vec::new(),
                save_directories: Vec::new(),
                diagnostics: Vec::new(),
            }],
            diagnostics: Vec::new(),
        }
    }

    fn plan(platform: &str, file: &str, environment: &RetroArchEnvironmentReport) -> LaunchPlan {
        build_launch_plan(
            &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: platform.into(),
                game_key: GAME.into(),
            }),
            &LaunchContentRef {
                kind: Some(LaunchContentKind::Cartridge),
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(file.into()),
                requires_mount: false,
                provenance: "synthetic fixture".into(),
            },
            &[],
            environment,
            &[],
        )
    }

    fn ra(kind: ProfileKind, stem: &str) -> String {
        retroarch_option_id(
            ProfileRef {
                profile_kind: kind,
                scope: ProfileScope::User,
            },
            stem,
        )
    }

    fn usable(plan: &LaunchPlan) -> Vec<String> {
        plan.candidates
            .iter()
            .filter(|c| c.readiness != LaunchReadiness::Blocked)
            .map(|c| launch_option_id(&c.target))
            .collect()
    }

    fn choose_once(plan: &mut LaunchPlan, platform: &str, option: &str) -> LaunchChoiceOutcome {
        let once = LaunchOnceChoice {
            platform_id: platform.into(),
            game_key: GAME.into(),
            option_id: option.into(),
        };
        apply(
            plan,
            LaunchChoiceInputs {
                once: Some(&once),
                preferences: Ok(&[]),
            },
        )
    }

    fn all_tied(plan: &LaunchPlan) -> bool {
        plan.candidates.len() >= 2
            && plan.candidates.iter().all(|c| {
                c.readiness == LaunchReadiness::Blocked
                    && c.blockers
                        .iter()
                        .all(|b| b.kind == LaunchBlockerKind::AmbiguousCore)
            })
    }

    #[test]
    fn atari_2600_and_neo_geo_pocket_color_core_ties_are_answered_by_a_choice() {
        for (platform, file, a, b, database) in [
            (
                "Atari2600",
                "/games/Moonsweeper (USA).a26",
                "core_a2600_x",
                "core_a2600_y",
                "Atari - 2600",
            ),
            (
                "Neo Geo Pocket Color",
                "/games/Big Bang Pro Wrestling (Japan).ngc",
                "core_ngpc_x",
                "core_ngpc_y",
                "SNK - Neo Geo Pocket Color",
            ),
        ] {
            let env = environment(
                ProfileKind::Native,
                vec![core(a, database), core(b, database)],
            );
            let mut tied = plan(platform, file, &env);
            assert!(all_tied(&tied), "{platform}: {:?}", tied.candidates);
            assert!(
                usable(&tied).is_empty(),
                "{platform}: nothing is picked automatically"
            );
            assert!(tied.candidates.iter().all(|c| is_choosable(&tied, c)));
            assert!(matches!(
                choose_once(&mut tied, platform, &ra(ProfileKind::Native, b)),
                LaunchChoiceOutcome::Applied { .. }
            ));
            assert_eq!(
                usable(&tied),
                vec![ra(ProfileKind::Native, b)],
                "{platform}"
            );
        }
    }

    #[test]
    fn a_saved_choice_never_authorises_a_removed_or_changed_core() {
        let database = "Atari - 2600";
        let file = "/games/Moonsweeper (USA).a26";
        let saved_choice = [LaunchPreference::system(
            "Atari2600",
            &ra(ProfileKind::Native, "core_a2600_x"),
        )];
        // The chosen core was uninstalled; two others still tie.
        let env = environment(
            ProfileKind::Native,
            vec![
                core("core_a2600_y", database),
                core("core_a2600_z", database),
            ],
        );
        let mut removed = plan("Atari2600", file, &env);
        assert!(matches!(
            apply(&mut removed, saved(&saved_choice)),
            LaunchChoiceOutcome::PreferredUnavailable { .. }
        ));
        assert!(usable(&removed).is_empty(), "no other core is substituted");
        // The same core stem under a different installation (Flatpak instead
        // of native) is a different option: not authorised by the old choice.
        let env = environment(
            ProfileKind::Flatpak,
            vec![
                core("core_a2600_x", database),
                core("core_a2600_y", database),
            ],
        );
        let mut moved = plan("Atari2600", file, &env);
        assert!(matches!(
            apply(&mut moved, saved(&saved_choice)),
            LaunchChoiceOutcome::PreferredUnavailable { .. }
        ));
        assert!(usable(&moved).is_empty());
    }

    #[test]
    fn missing_bios_and_duplicate_installs_still_refuse_after_a_choice() {
        let database = "SNK - Neo Geo Pocket Color";
        let file = "/games/Big Bang Pro Wrestling (Japan).ngc";
        let env = environment(
            ProfileKind::Native,
            vec![core("core_ngpc_x", database), core("core_ngpc_y", database)],
        );
        // Required firmware missing on the chosen core: still refused.
        let mut bios = plan("Neo Geo Pocket Color", file, &env);
        for c in &mut bios.candidates {
            c.blockers
                .push(crate::launch::readiness::LaunchBlocker::new(
                    LaunchBlockerKind::RequiredFirmwareMissing,
                    "required BIOS is missing",
                ));
        }
        assert!(bios.candidates.iter().all(|c| !is_choosable(&bios, c)));
        choose_once(
            &mut bios,
            "Neo Geo Pocket Color",
            &ra(ProfileKind::Native, "core_ngpc_x"),
        );
        assert!(usable(&bios).is_empty());
        // The same core installed twice under one stem is not a tie a choice
        // answers: the planner sees one stem, and the click-time command
        // builder refuses the duplicate (retroarch_command
        // `ambiguous_core`), whatever was chosen.
        let env = environment(
            ProfileKind::Native,
            vec![core("core_ngpc_x", database), core("core_ngpc_x", database)],
        );
        let duplicate = plan("Neo Geo Pocket Color", file, &env);
        assert!(!all_tied(&duplicate));
        let candidate = duplicate.candidates.first().expect("a candidate");
        let command = crate::launch::retroarch_command::build_retroarch_command_plan(
            &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "Neo Geo Pocket Color".into(),
                game_key: GAME.into(),
            }),
            candidate,
            &env,
        );
        assert!(
            command
                .blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::AmbiguousCore)
        );
    }
}

#[test]
fn a_saved_option_id_is_described_by_the_emulator_it_names() {
    assert_eq!(
        describe_option_id(
            "standalone/duckstation/duckstation:/home/user/Applications/DuckStation"
        ),
        "duckstation at /home/user/Applications/DuckStation"
    );
    assert_eq!(
        describe_option_id("standalone/mesen/default"),
        "mesen (default)"
    );
    assert_eq!(
        describe_option_id("retroarch/native/user/swanstation"),
        "RetroArch core swanstation (native, user)"
    );
    assert_eq!(describe_option_id("something-else"), "something-else");
}
