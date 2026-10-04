use super::*;
use crate::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use crate::launch::retroarch_command::RetroArchCommandSelection;
use crate::launch::retroarch_resource_projection::approved_retroarch_launch_root;
use crate::patch_manager::parse_cht_text;

fn source(id: &str) -> CheatSourceReference {
    CheatSourceReference {
        provider: "local".into(),
        source_id: id.into(),
        source_path: Some(PathBuf::from(format!("/library/cheats/{id}.cht"))),
        source_sha256: Some("cd".repeat(32)),
        entry_index: None,
    }
}

fn entries() -> Vec<ChtEntry> {
    parse_cht_text(
        "cheats = 4\n\
         cheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\ncheat0_enable = false\n\
         cheat1_desc = \"Max Ammo\"\ncheat1_code = \"8000BB00\"\ncheat1_enable = false\n\
         cheat2_desc = \"Max Ammo Alt\"\ncheat2_code = \"8000CC00\"\ncheat2_enable = false\n\
         cheat3_desc = \"No Code\"\ncheat3_enable = false\n",
    )
    .unwrap()
    .entries
}

fn variant(
    id: &str,
    src: &str,
    entry: Option<ChtEntry>,
    state: CheatApplicabilityState,
) -> CheatVariant {
    CheatVariant {
        variant_id: id.into(),
        source: source(src),
        format: CheatLaunchFormat::RetroArchCht,
        applicability: assessment(state),
        entry,
    }
}

fn candidate(logical: &str, variants: Vec<CheatVariant>) -> CheatCandidate {
    CheatCandidate {
        logical_id: logical.into(),
        title: logical.into(),
        variants,
        unresolved_conflict: false,
    }
}

fn facts() -> RetroArchLaunchFacts {
    RetroArchLaunchFacts {
        launch_root: Some(approved_retroarch_launch_root().join("launch-1")),
        real_config_path: Some(PathBuf::from("/home/u/.config/retroarch/retroarch.cfg")),
        real_save_directory: Some(PathBuf::from("/home/u/saves")),
        real_state_directory: Some(PathBuf::from("/home/u/states")),
        content_path: PathBuf::from("/library/roms/Game.sfc"),
        core_library_name: "Snes9x".into(),
        system_directory: None,
        profile_isolation: RetroArchProfileIsolation::DisposableProfile,
        effective_overrides: vec![],
    }
}

const READY: CheatApplicabilityState = CheatApplicabilityState::ExactGameMatch;

fn base_request(selections: Vec<CheatLaunchSelection>) -> CheatLaunchRequest {
    let parsed = entries();
    CheatLaunchRequest {
        launch_id: "launch-1".into(),
        target: CheatLaunchTarget {
            adapter_id: "retroarch".into(),
            game_identity: "snes:game".into(),
            identity_verified: true,
        },
        candidates: vec![
            candidate(
                "lives",
                vec![variant("v1", "a", Some(parsed[0].clone()), READY)],
            ),
            candidate(
                "ammo",
                vec![variant("v1", "b", Some(parsed[1].clone()), READY)],
            ),
            candidate(
                "bomb",
                vec![variant("v1", "c", Some(parsed[0].clone()), READY)],
            ),
        ],
        selections,
        retroarch: Some(facts()),
    }
}

fn select(id: &str) -> CheatLaunchSelection {
    CheatLaunchSelection {
        logical_id: id.into(),
        variant_id: None,
        review_acknowledged: false,
    }
}

fn blocked_reason(plan: &CheatLaunchPlan) -> Vec<CheatLaunchBlockReason> {
    plan.blocked
        .iter()
        .map(|blocked| blocked.reason.clone())
        .chain(plan.plan_blocks.iter().cloned())
        .collect()
}

fn cht(plan: &CheatLaunchPlan) -> String {
    String::from_utf8(plan.derivative.as_ref().unwrap().bytes.clone()).unwrap()
}

#[test]
fn no_cheats_selected_changes_nothing() {
    let plan = plan_cheat_launch(&base_request(vec![]));
    assert_eq!(plan.status, CheatLaunchPlanStatus::NoCheatsSelected);
    assert!(plan.selected.is_empty() && plan.derivative.is_none() && plan.retroarch.is_none());
    assert!(plan.grants.grants.is_empty() && plan.expectations.is_empty());
    assert!(plan.persistent_writes.is_empty() && plan.global_config_mutations.is_empty());
}

#[test]
fn no_cheats_leaves_the_command_unchanged_even_for_unsupported_targets() {
    let command = RetroArchCommand {
        executable: PathBuf::from("/usr/bin/retroarch"),
        arguments: vec![
            "-L".into(),
            "/cores/x.so".into(),
            "/library/roms/Game.sfc".into(),
        ],
        working_directory: None,
        selection: RetroArchCommandSelection {
            profile: ProfileRef {
                profile_kind: ProfileKind::Native,
                scope: ProfileScope::User,
            },
            core_stem: "x".into(),
            platform_id: "SNES".into(),
            core_library: PathBuf::from("/cores/x.so"),
            content_path: PathBuf::from("/library/roms/Game.sfc"),
        },
    };
    let mut request = base_request(vec![]);
    request.target.adapter_id = "pcsx2".into();
    let plan = plan_cheat_launch(&request);
    assert_eq!(
        command_with_cheat_launch_plan(&command, &plan).unwrap(),
        command
    );

    let ready = plan_cheat_launch(&base_request(vec![select("lives")]));
    let composed = command_with_cheat_launch_plan(&command, &ready).unwrap();
    assert_eq!(composed.arguments[..3], command.arguments[..3]);
    assert_eq!(composed.arguments[3], OsString::from("--config"));
    assert!(!composed.arguments.iter().any(|arg| arg == "--appendconfig"));

    let blocked = plan_cheat_launch(&base_request(vec![select("missing")]));
    assert_eq!(
        command_with_cheat_launch_plan(&command, &blocked),
        Err(CheatLaunchCommandError::PlanBlocked)
    );
}

#[test]
fn one_explicit_cheat_is_composed_and_available_ones_are_excluded() {
    let plan = plan_cheat_launch(&base_request(vec![select("ammo")]));
    assert!(plan.is_ready(), "{:?}", blocked_reason(&plan));
    let text = cht(&plan);
    assert!(text.contains("Max Ammo") && text.contains("cheats = 1\n"));
    assert!(
        !text.contains("Infinite Lives"),
        "available but unselected cheat leaked"
    );
    assert_eq!(plan.selected.len(), 1);
}

#[test]
fn several_selected_cheats_are_ordered_deterministically_and_repeatably() {
    let one = plan_cheat_launch(&base_request(vec![select("lives"), select("ammo")]));
    let two = plan_cheat_launch(&base_request(vec![select("ammo"), select("lives")]));
    assert!(one.is_ready());
    assert_eq!(one, two, "selection order must not matter");
    assert_eq!(
        one,
        plan_cheat_launch(&base_request(vec![select("lives"), select("ammo")]))
    );
    let text = cht(&one);
    assert!(text.contains("cheats = 2\n"));
    assert!(text.find("Max Ammo").unwrap() < text.find("Infinite Lives").unwrap());
    assert!(!text.contains("cheat2_"));
}

#[test]
fn unresolved_conflict_blocks_and_never_picks_a_variant() {
    let parsed = entries();
    let mut request = base_request(vec![select("ammo")]);
    request.candidates[1] = candidate(
        "ammo",
        vec![
            variant("v1", "b", Some(parsed[1].clone()), READY),
            variant("v2", "c", Some(parsed[2].clone()), READY),
        ],
    );
    let plan = plan_cheat_launch(&request);
    assert_eq!(plan.status, CheatLaunchPlanStatus::Blocked);
    assert_eq!(
        blocked_reason(&plan),
        vec![CheatLaunchBlockReason::UnresolvedConflict {
            variants: vec!["v1".into(), "v2".into()]
        }]
    );
    assert!(plan.derivative.is_none() && plan.grants.grants.is_empty());

    // Reconciliation-reported conflict on a single listed variant also blocks.
    let mut flagged = base_request(vec![select("ammo")]);
    flagged.candidates[1].unresolved_conflict = true;
    assert!(!plan_cheat_launch(&flagged).is_ready());
}

#[test]
fn explicit_variant_choice_composes_only_that_variant() {
    let parsed = entries();
    let mut request = base_request(vec![CheatLaunchSelection {
        logical_id: "ammo".into(),
        variant_id: Some("v2".into()),
        review_acknowledged: false,
    }]);
    request.candidates[1] = candidate(
        "ammo",
        vec![
            variant("v1", "b", Some(parsed[1].clone()), READY),
            variant("v2", "c", Some(parsed[2].clone()), READY),
        ],
    );
    request.candidates[1].unresolved_conflict = true;
    let plan = plan_cheat_launch(&request);
    assert!(plan.is_ready(), "{:?}", blocked_reason(&plan));
    let text = cht(&plan);
    assert!(text.contains("8000CC00") && !text.contains("8000BB00"));
    assert_eq!(plan.selected[0].variant_id, "v2");

    let mut bad = request.clone();
    bad.selections[0].variant_id = Some("nope".into());
    assert!(matches!(
        blocked_reason(&plan_cheat_launch(&bad))[0],
        CheatLaunchBlockReason::UnknownVariant { .. }
    ));
}

#[test]
fn malformed_selected_cheat_blocks_the_whole_plan() {
    let parsed = entries();
    let mut request = base_request(vec![select("lives"), select("bad")]);
    request.candidates.push(candidate(
        "bad",
        vec![variant("v1", "d", Some(parsed[3].clone()), READY)],
    ));
    request
        .candidates
        .push(candidate("gone", vec![variant("v1", "d", None, READY)]));
    let plan = plan_cheat_launch(&request);
    assert_eq!(plan.status, CheatLaunchPlanStatus::Blocked);
    assert!(matches!(
        blocked_reason(&plan)[0],
        CheatLaunchBlockReason::Malformed { .. }
    ));
    // A missing entry is malformed too, and nothing is composed.
    request.selections = vec![select("gone")];
    let plan = plan_cheat_launch(&request);
    assert!(matches!(
        blocked_reason(&plan)[0],
        CheatLaunchBlockReason::Malformed { .. }
    ));
    assert!(plan.derivative.is_none());
}

#[test]
fn unsupported_targets_block_and_are_not_confused_with_persistent_install() {
    for (adapter, expected_mode) in [
        ("pcsx2", CheatLaunchMode::PersistentInstallOnly),
        ("nonexistent-emulator", CheatLaunchMode::Unsupported),
        ("", CheatLaunchMode::Unknown),
    ] {
        let mut request = base_request(vec![select("lives")]);
        request.target.adapter_id = adapter.into();
        let plan = plan_cheat_launch(&request);
        assert_eq!(plan.capability.mode, expected_mode, "{adapter}");
        assert_eq!(plan.status, CheatLaunchPlanStatus::Blocked);
        assert!(matches!(
            blocked_reason(&plan)[0],
            CheatLaunchBlockReason::UnsupportedTarget { .. }
        ));
        assert!(plan.derivative.is_none() && plan.persistent_writes.is_empty());
    }
    // ScummVM has a launch-scoped mechanism but no composer here: not ready.
    let mut request = base_request(vec![select("lives")]);
    request.target.adapter_id = "scummvm".into();
    assert!(matches!(
        blocked_reason(&plan_cheat_launch(&request))[0],
        CheatLaunchBlockReason::NoComposer { .. }
    ));
}

#[test]
fn every_launch_mode_is_representable() {
    let modes = [
        CheatLaunchMode::Unsupported,
        CheatLaunchMode::PersistentInstallOnly,
        CheatLaunchMode::LaunchScopedConfig,
        CheatLaunchMode::LaunchScopedScript,
        CheatLaunchMode::LaunchScopedMemoryCommands,
        CheatLaunchMode::GuiOnly,
        CheatLaunchMode::Unknown,
    ];
    let distinct: BTreeSet<String> = modes
        .iter()
        .map(|mode| serde_json::to_string(mode).unwrap())
        .collect();
    assert_eq!(distinct.len(), modes.len());
    assert!(cheat_launch_capability("retroarch").requires_state_fencing);
}

#[test]
fn applicability_evidence_blocks_or_requires_review() {
    use CheatApplicabilityState as S;
    for state in [
        S::WrongRegion,
        S::WrongRevision,
        S::DifferentGame,
        S::UnsupportedFormat,
        S::UnsupportedEmulator,
        S::Malformed,
    ] {
        let mut request = base_request(vec![select("lives")]);
        request.candidates[0].variants[0].applicability = assessment(state);
        // Even an acknowledgement cannot unblock wrong/unsafe evidence.
        request.selections[0].review_acknowledged = true;
        let plan = plan_cheat_launch(&request);
        assert_eq!(
            blocked_reason(&plan),
            vec![CheatLaunchBlockReason::Applicability { state }]
        );
    }
    for state in [
        S::StrongMatch,
        S::PossibleMatch,
        S::NeedsReview,
        S::MissingRequiredEvidence,
        S::ConflictingVariants,
    ] {
        let mut request = base_request(vec![select("lives")]);
        request.candidates[0].variants[0].applicability = assessment(state);
        assert_eq!(
            blocked_reason(&plan_cheat_launch(&request)),
            vec![CheatLaunchBlockReason::ReviewRequired { state }]
        );
        request.selections[0].review_acknowledged = true;
        let plan = plan_cheat_launch(&request);
        assert!(plan.is_ready());
        // Acknowledged, but never promoted to an exact match.
        assert_eq!(plan.selected[0].applicability, state);
        assert!(plan.selected[0].review_acknowledged);
    }
    // Only an exact/ready assessment launches without review; similar-looking
    // titles are never enough on their own.
    for state in [S::Ready, S::ExactGameMatch] {
        assert_eq!(
            applicability_verdict(&assessment(state)),
            ApplicabilityVerdict::Allowed
        );
    }
    assert_eq!(
        applicability_verdict(&assessment(S::StrongMatch)),
        ApplicabilityVerdict::ReviewRequired
    );
}

#[test]
fn sources_are_never_granted_or_writable_and_derivative_lives_in_scratch() {
    let plan = plan_cheat_launch(&base_request(vec![select("lives"), select("ammo")]));
    assert!(plan.is_ready());
    let facts = facts();
    let root = facts.launch_root.clone().unwrap();
    let writable = |grant: &&LaunchResourceGrant| grant.access != LaunchResourceAccess::ReadOnly;
    for grant in plan.grants.grants.iter().filter(writable) {
        let path = grant.presented_path.as_ref().unwrap();
        let is_scratch = path.starts_with(&root);
        let is_user_state = grant.role == LaunchResourceRole::SaveData
            && grant.projection == LaunchProjectionMethod::DirectPath
            && grant.lifetime == LaunchResourceLifetime::Persistent;
        assert!(
            is_scratch || is_user_state,
            "unexpected writable grant: {grant:?}"
        );
        assert_ne!(path, &facts.content_path);
        assert!(!path.starts_with("/library/cheats"));
    }
    // No grant names the ROM as writable or any source cheat file at all.
    let media: Vec<_> = plan
        .grants
        .grants
        .iter()
        .filter(|grant| grant.role == LaunchResourceRole::GameMedia)
        .collect();
    assert_eq!(media.len(), 1);
    assert_eq!(media[0].access, LaunchResourceAccess::ReadOnly);
    assert!(!plan.grants.grants.iter().any(|grant| {
        grant
            .presented_path
            .as_ref()
            .is_some_and(|p| p.starts_with("/library/cheats"))
    }));
    let derivative = plan.derivative.as_ref().unwrap();
    assert!(derivative.destination.starts_with(&root));
    assert!(derivative.destination.ends_with("Snes9x/Game.cht"));
    let cheat_grant = plan
        .grants
        .grants
        .iter()
        .find(|grant| grant.presented_path.as_ref() == Some(&derivative.destination))
        .unwrap();
    assert_eq!(cheat_grant.role, LaunchResourceRole::CheatMaterial);
    assert_eq!(
        cheat_grant.projection,
        LaunchProjectionMethod::GeneratedFile
    );
    assert_eq!(cheat_grant.lifetime, LaunchResourceLifetime::LaunchOnly);
    assert!(cheat_grant.source_path.is_none());
}

#[test]
fn provenance_survives_into_the_plan() {
    let plan = plan_cheat_launch(&base_request(vec![select("ammo")]));
    let derivative = plan.derivative.as_ref().unwrap();
    assert_eq!(derivative.entries[0].source, source("b"));
    assert_eq!(derivative.entries[0].logical_id, "ammo");
    assert_eq!(plan.selected[0].source, source("b"));
    assert!(cht(&plan).contains("Source: local:b"));
}

#[test]
fn real_saves_are_passthrough_and_real_config_is_protected() {
    let plan = plan_cheat_launch(&base_request(vec![select("lives")]));
    let saves: Vec<_> = plan
        .grants
        .grants
        .iter()
        .filter(|grant| grant.role == LaunchResourceRole::SaveData)
        .collect();
    assert_eq!(saves.len(), 2);
    for grant in saves {
        assert_eq!(grant.projection, LaunchProjectionMethod::DirectPath);
        assert_eq!(grant.source_path, grant.presented_path);
        assert_eq!(grant.access, LaunchResourceAccess::ReadWrite);
        assert_eq!(grant.lifetime, LaunchResourceLifetime::Persistent);
        for copy in [
            LaunchProjectionMethod::ScratchCopy,
            LaunchProjectionMethod::TempCopy,
            LaunchProjectionMethod::Reflink,
        ] {
            assert_ne!(grant.projection, copy);
        }
    }
    let settings = plan.retroarch.as_ref().unwrap();
    assert!(
        settings
            .base_config_contents
            .contains("savefile_directory = \"/home/u/saves\"")
    );
    assert!(
        settings
            .base_config_contents
            .contains("savestate_directory = \"/home/u/states\"")
    );
    // The real config is neither granted nor the base, only a protected reference.
    let real = PathBuf::from("/home/u/.config/retroarch/retroarch.cfg");
    assert!(
        !plan
            .grants
            .grants
            .iter()
            .any(|grant| grant.presented_path.as_ref() == Some(&real))
    );
    assert!(
        !settings
            .extra_arguments
            .contains(&real.to_string_lossy().into_owned())
    );
    let protected: Vec<_> = plan
        .expectations
        .iter()
        .filter(|expectation| expectation.path == real)
        .collect();
    assert_eq!(protected.len(), 2);
    for expectation in protected {
        assert_eq!(expectation.class, LaunchStateClass::ProtectedConfig);
        assert_eq!(
            expectation.expectation,
            StateExpectation::MustRemainUnchanged
        );
    }
    assert!(plan.global_config_mutations.is_empty() && plan.persistent_writes.is_empty());
}

#[test]
fn retroarch_plan_disables_save_on_exit_and_uses_a_scratch_profile() {
    let plan = plan_cheat_launch(&base_request(vec![select("lives")]));
    let settings = plan.retroarch.as_ref().unwrap();
    let root = facts().launch_root.unwrap();
    let text = &settings.base_config_contents;
    for line in [
        "config_save_on_exit = \"false\"",
        "auto_overrides_enable = \"false\"",
        "apply_cheats_after_load = \"true\"",
    ] {
        assert!(text.contains(line), "{line}");
    }
    assert!(settings.base_config_path.starts_with(&root));
    assert!(
        settings.extra_arguments
            == vec![
                "--config".to_string(),
                settings.base_config_path.to_string_lossy().into_owned()
            ]
    );
    // Auxiliary writable state is redirected into scratch.
    for key in [
        "core_options_path",
        "rgui_config_directory",
        "cache_directory",
        "log_dir",
    ] {
        let line = text
            .lines()
            .find(|line| line.starts_with(key))
            .unwrap_or_else(|| panic!("{key}"));
        assert!(
            line.contains(&root.to_string_lossy().into_owned()),
            "{line}"
        );
    }
    assert!(!text.contains("retroarch.cfg"));
    assert!(
        text.lines()
            .find(|line| line.starts_with("cheat_database_path"))
            .unwrap()
            .contains(&root.to_string_lossy().into_owned())
    );
    // Every mandatory key is present exactly once.
    for key in RETROARCH_MANDATORY_KEYS {
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with(&format!("{key} =")))
                .count(),
            1,
            "{key}"
        );
    }
}

#[test]
fn unsafe_or_unknown_retroarch_ownership_fails_closed() {
    let cases: Vec<(
        &str,
        Box<dyn Fn(&mut RetroArchLaunchFacts)>,
        CheatLaunchBlockReason,
    )> = vec![
        (
            "no scratch",
            Box::new(|f| f.launch_root = None),
            CheatLaunchBlockReason::ScratchUnavailable,
        ),
        (
            "scratch outside approved root",
            Box::new(|f| f.launch_root = Some(PathBuf::from("/tmp/elsewhere"))),
            CheatLaunchBlockReason::ScratchUnavailable,
        ),
        (
            "scratch is the approved root itself",
            Box::new(|f| f.launch_root = Some(approved_retroarch_launch_root())),
            CheatLaunchBlockReason::ScratchUnavailable,
        ),
        (
            "no protected config",
            Box::new(|f| f.real_config_path = None),
            CheatLaunchBlockReason::ProtectedConfigUnclear,
        ),
        (
            "relative protected config",
            Box::new(|f| f.real_config_path = Some(PathBuf::from("retroarch.cfg"))),
            CheatLaunchBlockReason::ProtectedConfigUnclear,
        ),
        (
            "no save dir",
            Box::new(|f| f.real_save_directory = None),
            CheatLaunchBlockReason::SavePathInvalid {
                which: "save".into(),
            },
        ),
        (
            "save dir inside scratch would fork saves",
            Box::new(|f| {
                f.real_save_directory = Some(f.launch_root.clone().unwrap().join("saves"))
            }),
            CheatLaunchBlockReason::SavePathInvalid {
                which: "save".into(),
            },
        ),
        (
            "traversal in state dir",
            Box::new(|f| f.real_state_directory = Some(PathBuf::from("/home/u/../x"))),
            CheatLaunchBlockReason::SavePathInvalid {
                which: "state".into(),
            },
        ),
        (
            "relative content",
            Box::new(|f| f.content_path = PathBuf::from("Game.sfc")),
            CheatLaunchBlockReason::ContentPathInvalid,
        ),
        (
            "config-file-only isolation is insufficient",
            Box::new(|f| f.profile_isolation = RetroArchProfileIsolation::ConfigFileOnly),
            CheatLaunchBlockReason::ProfileIsolationInsufficient {
                provided: RetroArchProfileIsolation::ConfigFileOnly,
            },
        ),
        (
            "no isolation",
            Box::new(|f| f.profile_isolation = RetroArchProfileIsolation::None),
            CheatLaunchBlockReason::ProfileIsolationInsufficient {
                provided: RetroArchProfileIsolation::None,
            },
        ),
    ];
    for (name, mutate, expected) in cases {
        let mut request = base_request(vec![select("lives")]);
        mutate(request.retroarch.as_mut().unwrap());
        let plan = plan_cheat_launch(&request);
        assert_eq!(plan.status, CheatLaunchPlanStatus::Blocked, "{name}");
        assert!(
            plan.plan_blocks.contains(&expected),
            "{name}: {:?}",
            plan.plan_blocks
        );
        assert!(
            plan.derivative.is_none() && plan.retroarch.is_none() && plan.grants.grants.is_empty(),
            "{name}"
        );
        assert!(
            plan.persistent_writes.is_empty(),
            "{name}: never degrades to persistent writes"
        );
    }
    let mut missing = base_request(vec![select("lives")]);
    missing.retroarch = None;
    assert_eq!(
        plan_cheat_launch(&missing).plan_blocks,
        vec![CheatLaunchBlockReason::FactsMissing]
    );
}

#[test]
fn an_override_that_defeats_a_mandatory_key_blocks() {
    let mut request = base_request(vec![select("lives")]);
    request.retroarch.as_mut().unwrap().effective_overrides = vec![
        RetroArchOverrideFinding {
            path: PathBuf::from("/home/u/.config/retroarch/config/Snes9x/Game.cfg"),
            keys: vec!["video_scale".into(), "config_save_on_exit".into()],
        },
        RetroArchOverrideFinding {
            path: PathBuf::from("/home/u/.config/retroarch/config/Snes9x/Snes9x.cfg"),
            keys: vec!["video_scale".into()],
        },
    ];
    let plan = plan_cheat_launch(&request);
    assert_eq!(plan.status, CheatLaunchPlanStatus::Blocked);
    assert_eq!(
        plan.plan_blocks,
        vec![CheatLaunchBlockReason::OverrideDefeatsSetting {
            path: PathBuf::from("/home/u/.config/retroarch/config/Snes9x/Game.cfg"),
            key: "config_save_on_exit".into(),
        }]
    );
}

#[test]
fn duplicate_and_unknown_selections_are_blocked() {
    let plan = plan_cheat_launch(&base_request(vec![
        select("lives"),
        select("lives"),
        select("ghost"),
    ]));
    assert_eq!(plan.status, CheatLaunchPlanStatus::Blocked);
    let reasons = blocked_reason(&plan);
    assert!(reasons.contains(&CheatLaunchBlockReason::DuplicateSelection));
    assert!(reasons.contains(&CheatLaunchBlockReason::UnknownCheat));
    let mut request = base_request(vec![select("lives")]);
    request.launch_id = " ".into();
    assert_eq!(
        plan_cheat_launch(&request).plan_blocks,
        vec![CheatLaunchBlockReason::EmptyLaunchId]
    );
}

#[test]
fn plan_round_trips_with_cleanup_and_verification_expectations() {
    let plan = plan_cheat_launch(&base_request(vec![select("lives"), select("ammo")]));
    let json = serde_json::to_string(&plan).unwrap();
    let back: CheatLaunchPlan = serde_json::from_str(&json).unwrap();
    assert_eq!(back, plan);
    let root = facts().launch_root.unwrap();
    let classes: BTreeSet<_> = back
        .expectations
        .iter()
        .map(|expectation| (expectation.class, expectation.expectation))
        .collect();
    for wanted in [
        (
            LaunchStateClass::ReadOnlySource,
            StateExpectation::MustRemainUnchanged,
        ),
        (
            LaunchStateClass::ProtectedConfig,
            StateExpectation::MustRemainUnchanged,
        ),
        (LaunchStateClass::RealUserState, StateExpectation::MayChange),
        (
            LaunchStateClass::EphemeralConfig,
            StateExpectation::MustNotExistAfter,
        ),
        (
            LaunchStateClass::EphemeralRuntime,
            StateExpectation::MustNotExistAfter,
        ),
    ] {
        assert!(classes.contains(&wanted), "{wanted:?}");
    }
    assert!(
        back.expectations
            .iter()
            .any(|e| e.path == root && e.class == LaunchStateClass::EphemeralRuntime)
    );
    // Each source cheat file and the ROM are must-remain-unchanged.
    for path in [
        "/library/cheats/a.cht",
        "/library/cheats/b.cht",
        "/library/roms/Game.sfc",
    ] {
        assert!(
            back.expectations.iter().any(|e| {
                e.path == PathBuf::from(path)
                    && e.class == LaunchStateClass::ReadOnlySource
                    && e.expectation == StateExpectation::MustRemainUnchanged
            }),
            "{path}"
        );
    }
}

#[test]
fn baseline_capture_detects_source_and_protected_config_changes_but_allows_saves() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("Game.sfc");
    let cht_source = dir.path().join("a.cht");
    let config = dir.path().join("retroarch.cfg");
    let saves = dir.path().join("saves");
    std::fs::create_dir(&saves).unwrap();
    std::fs::write(saves.join("game.srm"), b"progress").unwrap();
    std::fs::write(&rom, b"rom bytes").unwrap();
    std::fs::write(&cht_source, b"cheats = 0\n").unwrap();
    std::fs::write(
        &config,
        "savefile_directory = \"/real\"\nvideo_scale = \"3\"\n",
    )
    .unwrap();

    let expect = |path: &Path, class, expectation, fingerprint| LaunchStateExpectation {
        path: path.to_path_buf(),
        class,
        expectation,
        fingerprint,
        severity: ViolationSeverity::Warning,
    };
    let source_exp = expect(
        &cht_source,
        LaunchStateClass::ReadOnlySource,
        StateExpectation::MustRemainUnchanged,
        FingerprintKind::Sha256,
    );
    let rom_exp = expect(
        &rom,
        LaunchStateClass::ReadOnlySource,
        StateExpectation::MustRemainUnchanged,
        FingerprintKind::FileIdentity,
    );
    let key_exp = expect(
        &config,
        LaunchStateClass::ProtectedConfig,
        StateExpectation::MustRemainUnchanged,
        FingerprintKind::KeyProbe {
            keys: vec!["savefile_directory".into(), "config_save_on_exit".into()],
        },
    );
    let save_exp = expect(
        &saves.join("game.srm"),
        LaunchStateClass::RealUserState,
        StateExpectation::MayChange,
        FingerprintKind::ExistsNotTruncated,
    );
    let scratch = dir.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let scratch_exp = expect(
        &scratch,
        LaunchStateClass::EphemeralRuntime,
        StateExpectation::MustNotExistAfter,
        FingerprintKind::NotPresent,
    );

    let all = [&source_exp, &rom_exp, &key_exp, &save_exp, &scratch_exp];
    let baselines: Vec<_> = all.iter().map(|exp| capture_baseline(exp)).collect();
    // Nothing changed except that scratch is still present.
    for (exp, baseline) in all.iter().zip(&baselines).take(4) {
        assert_eq!(
            verify_expectation(exp, baseline),
            StateOutcome::Unchanged,
            "{:?}",
            exp.path
        );
    }
    assert!(matches!(
        verify_expectation(&scratch_exp, &baselines[4]),
        StateOutcome::Violation { .. }
    ));

    // A leaked key in the protected config is caught; unrelated churn is not.
    std::fs::write(
        &config,
        "savefile_directory = \"/real\"\nvideo_scale = \"4\"\n",
    )
    .unwrap();
    assert_eq!(
        verify_expectation(&key_exp, &baselines[2]),
        StateOutcome::Unchanged
    );
    std::fs::write(&config, "savefile_directory = \"/tmp/scratch\"\n").unwrap();
    assert!(matches!(
        verify_expectation(&key_exp, &baselines[2]),
        StateOutcome::Violation { .. }
    ));
    // A modified source cheat file and a deleted ROM are violations.
    std::fs::write(&cht_source, b"cheats = 1\n").unwrap();
    assert!(matches!(
        verify_expectation(&source_exp, &baselines[0]),
        StateOutcome::Violation { .. }
    ));
    std::fs::remove_file(&rom).unwrap();
    assert!(matches!(
        verify_expectation(&rom_exp, &baselines[1]),
        StateOutcome::Violation { .. }
    ));
    // Saves may change; being emptied or removed is not acceptable.
    std::fs::write(saves.join("game.srm"), b"more progress").unwrap();
    assert!(matches!(
        verify_expectation(&save_exp, &baselines[3]),
        StateOutcome::Changed { .. }
    ));
    std::fs::write(saves.join("game.srm"), b"").unwrap();
    assert!(matches!(
        verify_expectation(&save_exp, &baselines[3]),
        StateOutcome::Violation { .. }
    ));
}

#[test]
fn persistent_install_stays_distinct_from_launch_scoped_composition() {
    // The route table still reports persistent-install support for RetroArch
    // and other adapters; launch composition neither changes nor uses it.
    assert_eq!(
        crate::patch_manager::cheat_apply_support(&CheatRouteTarget::retroarch(None)),
        CheatApplySupport::Supported
    );
    assert_eq!(
        cheat_launch_capability("pcsx2").mode,
        CheatLaunchMode::PersistentInstallOnly
    );
    assert_eq!(
        cheat_launch_capability("retroarch").mode,
        CheatLaunchMode::LaunchScopedConfig
    );
    let plan = plan_cheat_launch(&base_request(vec![select("lives")]));
    assert!(plan.persistent_writes.is_empty() && plan.global_config_mutations.is_empty());
    assert!(matches!(
        plan.capability.proof,
        PersistenceProof::ProvenByHarness { .. }
    ));
}

#[test]
fn canonical_reconciliation_group_decides_whether_a_choice_is_required() {
    use crate::patch_manager::{
        CheatDuplicateKind as K, CheatReconciliationGroup, CheatRelationship as R,
    };
    let group = |relationship, classifications| CheatReconciliationGroup {
        relationship,
        classifications,
        entry_indices: vec![0, 1],
        normalized_title: "lives".into(),
        semantic_fingerprint: None,
        raw_fingerprint: None,
        differences: vec![],
        quality: vec![],
    };
    for kinds in [vec![K::ExactDuplicate], vec![K::CorroboratingObservation]] {
        assert!(!CheatCandidate::requires_choice(&group(
            R::ExactRawDuplicate,
            kinds
        )));
    }
    for kind in [
        K::CodeConflict,
        K::RegionVariant,
        K::VersionVariant,
        K::SourceIndexConflict,
    ] {
        assert!(CheatCandidate::requires_choice(&group(
            R::ExactRawDuplicate,
            vec![kind]
        )));
    }
    assert!(CheatCandidate::requires_choice(&group(
        R::SameTitleDifferentCode,
        vec![]
    )));
}

fn assessment(state: CheatApplicabilityState) -> CheatApplicabilityReport {
    use crate::patch_manager::*;
    let parsed = parse_cht_text("cheat0_desc=Lives\ncheat0_code=A\n").unwrap();
    let mut report = assess_cheat_applicability(&CheatApplicabilityInput {
        game: Default::default(),
        association: Default::default(),
        document: parsed.reconciliation_entries(
            "game",
            true,
            CheatPlatform::GameCube,
            "local",
            "a.cht",
        )[0]
        .document
        .clone(),
        parsing: CheatParseEvidence::Valid,
        native_cht: Some(parsed.entries[0].clone()),
        route: None,
        reconciliation: None,
    });
    // Unit fixtures for presentation-state policy; end-to-end tests below
    // retain all assessed findings without changing the report.
    report.state = state;
    report.blockers.clear();
    report
}

#[test]
fn assessed_hard_findings_survive_variant_choice_and_acknowledgement() {
    use crate::game_identity::IdentityStatus;
    use crate::patch_manager::*;
    for mismatch in ["region", "revision", "identity", "none"] {
        let parsed = parse_cht_text("cheat0_desc=Lives\ncheat0_code=A\n").unwrap();
        let mut entries =
            parsed.reconciliation_entries("game", true, CheatPlatform::GameCube, "local", "a.cht");
        entries[0].applicability.region = Some("Europe".into());
        entries[0].applicability.revision = Some("1".into());
        let association = CheatGameAssociation::from_entry(&entries[0]);
        let document = entries[0].document.clone();
        let mut second = entries[0].clone();
        second.raw_code = Some("B".into());
        second.document.operations = vec![CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::RetroArch,
            raw: "B".into(),
            reason: "opaque".into(),
        }];
        second.source_path = Some("b.cht".into());
        entries.push(second);
        let CheatReconciliationOutcome::Ready(reconciliation) = reconcile_cheats_for_game(entries)
        else {
            panic!()
        };
        let report = assess_cheat_applicability(&CheatApplicabilityInput {
            game: CheatSelectedGame {
                region: Some(CheatReleaseEvidence {
                    value: if mismatch == "region" {
                        "USA"
                    } else {
                        "Europe"
                    }
                    .into(),
                    status: if mismatch == "identity" {
                        IdentityStatus::Ambiguous
                    } else {
                        IdentityStatus::Verified
                    },
                }),
                revision: Some(CheatReleaseEvidence {
                    value: if mismatch == "revision" { "2" } else { "1" }.into(),
                    status: IdentityStatus::Verified,
                }),
                ..Default::default()
            },
            association,
            document,
            parsing: CheatParseEvidence::Valid,
            native_cht: Some(parsed.entries[0].clone()),
            route: None,
            reconciliation: Some(reconciliation),
        });
        assert_eq!(report.state, CheatApplicabilityState::ConflictingVariants);
        let expected = match mismatch {
            "region" => Some(CheatApplicabilityIssue::WrongRegion),
            "revision" => Some(CheatApplicabilityIssue::WrongRevision),
            "identity" => Some(CheatApplicabilityIssue::ConflictingIdentity),
            _ => None,
        };
        if let Some(issue) = expected {
            assert!(report.blockers.contains(&issue));
        }
        for acknowledge in [false, true] {
            for choose in [false, true] {
                let mut request = base_request(vec![select("lives")]);
                request.candidates[0].unresolved_conflict = true;
                request.candidates[0].variants[0].applicability = report.clone();
                request.selections[0].variant_id = choose.then(|| "v1".into());
                request.selections[0].review_acknowledged = acknowledge;
                assert_eq!(
                    plan_cheat_launch(&request).is_ready(),
                    mismatch == "none" && acknowledge && choose,
                    "{mismatch}, acknowledged={acknowledge}, chosen={choose}"
                );
            }
        }
    }
}

#[test]
fn the_planner_accepts_the_emuwiz_data_root_and_rejects_everything_else() {
    use crate::launch::retroarch_resource_projection::approved_retroarch_data_launch_root;
    let Some(data_root) = approved_retroarch_data_launch_root() else {
        return; // no resolvable data directory on this host
    };
    let plan_with = |root: PathBuf| {
        let mut request = base_request(vec![select("lives")]);
        request.retroarch.as_mut().unwrap().launch_root = Some(root);
        plan_cheat_launch(&request)
    };
    assert_eq!(
        plan_with(data_root.join("cheats-launch-1")).status,
        CheatLaunchPlanStatus::Ready
    );
    for root in [
        data_root.clone(),
        data_root.parent().unwrap().join("elsewhere"),
        PathBuf::from("/var/tmp/emuwiz-x"),
    ] {
        assert_eq!(plan_with(root).status, CheatLaunchPlanStatus::Blocked);
    }
}
