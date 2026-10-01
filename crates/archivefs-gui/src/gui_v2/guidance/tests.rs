//! Behaviour, truth and catalogue tests for the Mr Wiz guidance engine.

use super::audit::{all_categories, report};
use super::catalogue::CATALOGUE;
use super::exposure::{Exposure, ExposureEvent, ExposureState, OPTIONAL_TIP_COOLDOWN_SECS};
use super::model::{
    FactKind, GuidanceAction, GuidanceCategory, GuidanceContext, GuidanceEvidence, GuidanceFact,
    GuidanceLevel, GuidancePage, GuidanceTopic, MascotState,
};
use super::script::{PRIORITY_BANDS, RepeatPolicy, ScriptOrigin, placeholders};
use super::select::{GuidanceSelection, eligible, select, select_in};
use super::{GuidanceState, GuidanceTip};

use GuidanceFact as F;

fn pick(page: GuidancePage, facts: &[GuidanceFact]) -> GuidanceSelection {
    select(page, facts).unwrap_or_else(|| panic!("nothing selected for {page:?} with {facts:?}"))
}

fn id(page: GuidancePage, facts: &[GuidanceFact]) -> &'static str {
    pick(page, facts).script.id
}

fn script(id: &str) -> &'static super::script::GuidanceScript {
    CATALOGUE
        .iter()
        .find(|script| script.id == id)
        .unwrap_or_else(|| panic!("no script {id}"))
}

// --- Determinism ----------------------------------------------------------------------

#[test]
fn the_same_evidence_always_selects_the_same_guidance() {
    let facts = [
        F::LaunchNoCompatibleEmulator,
        F::FirmwareMissing,
        F::ScanRunning,
        F::LaunchReady,
    ];
    let first = pick(GuidancePage::Launch, &facts);
    for _ in 0..50 {
        let again = pick(GuidancePage::Launch, &facts);
        assert_eq!(again.script.id, first.script.id);
        assert_eq!(
            again.item(GuidanceLevel::Quick),
            first.item(GuidanceLevel::Quick)
        );
        assert_eq!(again.provenance, first.provenance);
    }
}

#[test]
fn the_order_the_caller_supplies_facts_in_cannot_change_the_answer() {
    let facts = [
        F::FirmwareMissing,
        F::LaunchNoCompatibleEmulator,
        F::ScanRunning,
        F::ArtworkAlternatives {
            provider: "Provider".into(),
            alternative_count: 2,
        },
    ];
    let forward = pick(GuidancePage::Launch, &facts);
    let mut reversed = facts.to_vec();
    reversed.reverse();
    let backward = pick(GuidancePage::Launch, &reversed);
    assert_eq!(forward.script.id, backward.script.id);
    assert_eq!(forward.provenance, backward.provenance);
}

#[test]
fn selection_does_not_depend_on_what_was_selected_before() {
    let mut state = GuidanceState::default();
    let home = GuidanceContext::new(GuidancePage::Home);
    let first = state.select(&home);
    // Navigating elsewhere and back is irrelevant: there is no rotation.
    for page in [
        GuidancePage::Games,
        GuidancePage::Sources,
        GuidancePage::Settings,
    ] {
        state.select(&GuidanceContext::new(page));
    }
    assert_eq!(state.select(&home), first);
    assert_eq!(GuidanceState::default().select(&home), first);
}

// --- Precedence -----------------------------------------------------------------------

#[test]
fn a_blocker_beats_a_tip_and_the_tip_is_reported_as_outranked() {
    // Launch has an orientation message of its own; a refusal reason must win.
    let selection = pick(GuidancePage::Launch, &[F::LaunchNoCompatibleEmulator]);
    assert_eq!(selection.script.id, "launch.no_compatible_emulator");
    assert_eq!(selection.script.category, GuidanceCategory::WhyBlocked);
    assert!(selection.provenance.outranked.contains(&"launch-checks"));
    // And a narrow, cosmetic game-scoped tip cannot hide a collection-wide refusal.
    let selection = pick(
        GuidancePage::Games,
        &[
            F::ArtworkAlternatives {
                provider: "Provider".into(),
                alternative_count: 2,
            },
            F::SourceChangedSincePreview,
        ],
    );
    assert_eq!(selection.script.id, "recovery.source_changed_since_preview");
    assert!(
        selection
            .provenance
            .outranked
            .contains(&"artwork.alternatives_available")
    );
}

#[test]
fn an_informational_message_never_hides_a_refusal_reason() {
    // Every informational (non-blocking) script against every refusal (band 100-90).
    let refusals: Vec<_> = CATALOGUE
        .iter()
        .filter(|s| s.priority >= 90 && s.category == GuidanceCategory::WhyBlocked)
        .collect();
    let informational: Vec<_> = CATALOGUE
        .iter()
        .filter(|s| {
            matches!(
                s.category,
                GuidanceCategory::Tip | GuidanceCategory::Explain | GuidanceCategory::Success
            ) && s.origin == ScriptOrigin::Design
        })
        .collect();
    for blocker in &refusals {
        for note in &informational {
            let page = common_page(blocker.pages, note.pages);
            let Some(page) = page else { continue };
            let facts: Vec<GuidanceFact> = blocker
                .requires
                .iter()
                .chain(note.requires)
                .map(|kind| kind.sample())
                .collect();
            let facts = valid(facts);
            let winner = select(page, &facts).unwrap();
            assert!(
                winner.script.priority >= blocker.priority
                    || winner.script.category == GuidanceCategory::WhyBlocked
                    || !is_eligible(blocker, page, &facts),
                "{} hid {} on {page:?}",
                winner.script.id,
                blocker.id
            );
        }
    }
}

#[test]
fn a_warning_beats_a_success_where_both_apply() {
    let selection = pick(
        GuidancePage::Games,
        &[F::LaunchReady, F::IdentityConflicting],
    );
    assert_eq!(selection.script.id, "identity.conflicting_evidence");
    assert_eq!(selection.script.category, GuidanceCategory::Warning);
}

#[test]
fn no_success_message_outranks_a_blocker_or_warning_it_shares_a_page_with() {
    let loud: Vec<_> = CATALOGUE
        .iter()
        .filter(|s| {
            s.priority >= 70
                && matches!(
                    s.category,
                    GuidanceCategory::WhyBlocked | GuidanceCategory::Warning
                )
        })
        .collect();
    let successes: Vec<_> = CATALOGUE
        .iter()
        .filter(|s| s.category == GuidanceCategory::Success)
        .collect();
    let mut compared = 0;
    for blocker in &loud {
        for success in &successes {
            let Some(page) = common_page(blocker.pages, success.pages) else {
                continue;
            };
            let facts = valid(
                blocker
                    .requires
                    .iter()
                    .chain(success.requires)
                    .map(|kind| kind.sample())
                    .collect(),
            );
            if !is_eligible(blocker, page, &facts) || !is_eligible(success, page, &facts) {
                continue;
            }
            compared += 1;
            let winner = select(page, &facts).unwrap();
            assert_ne!(
                winner.script.category,
                GuidanceCategory::Success,
                "{} was shown over {} on {page:?}",
                winner.script.id,
                blocker.id
            );
        }
    }
    assert!(
        compared > 20,
        "the property compared too few pairs ({compared})"
    );
}

#[test]
fn equal_priority_ties_prefer_the_blocker_then_the_id_never_table_order() {
    // Two band-90 blockers about the same game: the order is by ID, so reordering
    // the table could never change the answer.
    let both = pick(
        GuidancePage::Launch,
        &[F::LaunchNoCompatibleEmulator, F::FirmwareMissing],
    );
    assert_eq!(both.script.id, "firmware.missing");
    assert_eq!(
        both.provenance.other_blockers,
        ["launch.no_compatible_emulator"]
    );
    assert_eq!(
        id(GuidancePage::Launch, &[F::FirmwareMissing]),
        "firmware.missing"
    );
}

#[test]
fn a_blocker_in_a_higher_band_wins_whatever_its_scope() {
    // Operation-scoped band 50 (scan running) versus game-scoped band 90.
    let selection = pick(GuidancePage::Launch, &[F::ScanRunning, F::FirmwareMissing]);
    assert_eq!(selection.script.id, "firmware.missing");
}

// --- Empty states, success, and no evidence ----------------------------------------------

#[test]
fn an_empty_state_is_selected_from_a_completed_check_only() {
    assert_eq!(
        id(GuidancePage::Home, &[F::LibraryLoadedEmpty]),
        "library.loaded_empty"
    );
    // While a scan is running the empty list is not the story.
    assert_eq!(
        id(GuidancePage::Home, &[F::LibraryLoadedEmpty, F::ScanRunning]),
        "scan.running"
    );
    // With no folders configured the folder empty state wins, not "list is empty".
    assert_eq!(
        id(
            GuidancePage::Home,
            &[F::LibraryLoadedEmpty, F::SourcesNoneConfigured]
        ),
        "sources.none_configured"
    );
}

#[test]
fn success_is_selected_only_for_proven_success_and_names_its_scope() {
    let ready = pick(GuidancePage::Launch, &[F::LaunchReady]);
    assert_eq!(ready.script.id, "launch.ready");
    assert_eq!(ready.script.category, GuidanceCategory::Success);
    assert_eq!(ready.script.mascot, MascotState::Success);
    let message = ready.item(GuidanceLevel::Quick).message;
    assert!(message.contains("launch checks"), "{message}");
    // Unacknowledged launch warnings withdraw the success claim entirely.
    let withdrawn = pick(
        GuidancePage::Launch,
        &[F::LaunchReady, F::LaunchWarningsUnacknowledged],
    );
    assert_ne!(withdrawn.script.category, GuidanceCategory::Success);
    // A repair that completed but is not verified is not a success.
    let pending = pick(
        GuidancePage::Home,
        &[F::RepairCompletedVerificationPending { operation_id: 1 }],
    );
    assert_ne!(pending.script.category, GuidanceCategory::Success);
    assert!(
        pending
            .item(GuidanceLevel::Quick)
            .message
            .contains("not been fully verified")
    );
}

#[test]
fn no_evidence_selects_no_design_script() {
    for page in all_pages() {
        let selected = select(page, &[]);
        if let Some(selection) = selected {
            assert_eq!(
                selection.script.origin,
                ScriptOrigin::Legacy,
                "{page:?} selected design script {} with no evidence",
                selection.script.id
            );
        }
    }
}

#[test]
fn a_page_with_no_applicable_script_returns_an_explicit_none() {
    // Duplicates has no orientation message and no facts: nothing, not a filler.
    assert!(select(GuidancePage::Duplicates, &[]).is_none());
    assert!(
        GuidanceState::default()
            .select(&GuidanceContext::new(GuidancePage::Duplicates))
            .is_none()
    );
    // An unrelated fact on a page it does not apply to does not conjure guidance.
    assert!(select(GuidancePage::Duplicates, &[F::FirmwareMissing]).is_none());
}

// --- Levels ---------------------------------------------------------------------------

#[test]
fn quick_explain_technical_and_minimal_are_distinct_authored_text() {
    let selection = pick(
        GuidancePage::Games,
        &[F::IdentityUnknown, F::OperationNeedsStrongerIdentity],
    );
    assert_eq!(selection.script.id, "identity.unknown");
    let quick = selection.item(GuidanceLevel::Quick);
    let explain = selection.item(GuidanceLevel::Explain);
    let technical = selection.item(GuidanceLevel::Technical);
    let minimal = selection.item(GuidanceLevel::Minimal);
    assert_eq!(
        [quick.level, explain.level, technical.level, minimal.level],
        [
            GuidanceLevel::Quick,
            GuidanceLevel::Explain,
            GuidanceLevel::Technical,
            GuidanceLevel::Minimal
        ]
    );
    let texts = [
        &quick.message,
        &explain.message,
        &technical.message,
        &minimal.message,
    ];
    for (index, a) in texts.iter().enumerate() {
        for b in &texts[index + 1..] {
            assert_ne!(a, b);
        }
    }
    assert!(technical.message.starts_with("Technical details"));
    assert!(minimal.message.len() < quick.message.len());
    // The same facts, the same selection, at every level.
    assert!(
        [&explain, &technical, &minimal]
            .iter()
            .all(|item| item.id == quick.id)
    );
    assert_eq!(
        quick.available_levels,
        vec![
            GuidanceLevel::Minimal,
            GuidanceLevel::Quick,
            GuidanceLevel::Explain,
            GuidanceLevel::Technical
        ]
    );
}

#[test]
fn a_level_a_script_does_not_author_resolves_to_quick_and_says_so() {
    let legacy = pick(GuidancePage::Home, &[]);
    assert_eq!(legacy.script.origin, ScriptOrigin::Legacy);
    for level in [
        GuidanceLevel::Minimal,
        GuidanceLevel::Explain,
        GuidanceLevel::Technical,
    ] {
        let item = legacy.item(level);
        assert_eq!(item.requested_level, level);
        assert_eq!(item.level, GuidanceLevel::Quick);
        assert_eq!(item.message, legacy.item(GuidanceLevel::Quick).message);
    }
}

#[test]
fn technical_text_is_authored_never_assembled_from_values() {
    for script in CATALOGUE
        .iter()
        .filter(|s| s.origin == ScriptOrigin::Design)
    {
        let technical = script.technical.expect("design scripts author Technical");
        assert!(technical.starts_with("Technical details"), "{}", script.id);
        // It describes what the owning page can show; it embeds no runtime value.
        assert!(
            !technical.contains('{'),
            "{} interpolates technical text",
            script.id
        );
        assert!(
            !technical.contains("0x") && !technical.contains("Some("),
            "{}",
            script.id
        );
    }
}

// --- Next actions and mascot ------------------------------------------------------------

#[test]
fn the_offered_action_is_a_typed_identifier_projected_from_the_selection() {
    let findings = pick(
        GuidancePage::ProblemsRepair,
        &[F::ProblemsActionable { finding_count: 3 }],
    );
    assert_eq!(findings.action, Some(GuidanceAction::ReviewFirstFinding));
    assert_eq!(findings.item(GuidanceLevel::Quick).action, findings.action);
    // The owner has no first finding to open: the alternate action replaces it.
    let none_to_open = pick(
        GuidancePage::ProblemsRepair,
        &[
            F::ProblemsActionable { finding_count: 3 },
            F::NoFirstFindingTarget,
        ],
    );
    assert_eq!(none_to_open.action, Some(GuidanceAction::ReviewProblems));
    assert_eq!(GuidanceAction::ReviewProblems.label(), "Review problems");
}

#[test]
fn conditional_alternate_actions_replace_the_primary_and_can_omit_it() {
    let cover = |extra: &[GuidanceFact]| {
        let mut facts = vec![F::ArtworkCoverMissing];
        facts.extend_from_slice(extra);
        pick(GuidancePage::Artwork, &facts).action
    };
    assert_eq!(cover(&[]), Some(GuidanceAction::ReviewArtworkSources));
    assert_eq!(
        cover(&[F::ArtworkRefreshAvailable]),
        Some(GuidanceAction::RefreshArtwork)
    );
    assert_eq!(
        cover(&[F::IdentityPreventsArtworkMatch]),
        Some(GuidanceAction::ReviewGameEvidence)
    );
    // When both apply, uncertain identity wins: a refresh cannot help.
    assert_eq!(
        cover(&[F::ArtworkRefreshAvailable, F::IdentityPreventsArtworkMatch]),
        Some(GuidanceAction::ReviewGameEvidence)
    );
    assert_eq!(
        pick(GuidancePage::BiosFirmware, &[F::FirmwareMissing]).action,
        Some(GuidanceAction::ReviewBiosFolder)
    );
    assert_eq!(
        pick(
            GuidancePage::BiosFirmware,
            &[F::FirmwareMissing, F::BiosFolderChooserAvailable]
        )
        .action,
        Some(GuidanceAction::ChooseBiosFolder)
    );
    // Omit the action entirely when no capability view exists to open.
    let unsupported = |extra: &[GuidanceFact]| {
        let mut facts = vec![F::FormatOperationUnsupported {
            format: "CHD".into(),
            operation: "repair".into(),
        }];
        facts.extend_from_slice(extra);
        pick(GuidancePage::Converter, &facts).action
    };
    assert_eq!(
        unsupported(&[]),
        Some(GuidanceAction::ReviewSupportedOptions)
    );
    assert_eq!(unsupported(&[F::NoCapabilityView]), None);
}

#[test]
fn an_offer_to_navigate_to_the_page_you_are_on_is_dropped() {
    let queued = F::QueuedWork {
        queued_count: 2,
        running_count: 1,
    };
    assert_eq!(
        pick(GuidancePage::Home, std::slice::from_ref(&queued)).action,
        Some(GuidanceAction::ViewActivity)
    );
    assert_eq!(pick(GuidancePage::Activity, &[queued]).action, None);
    // A control that lives on its page is not navigation and is kept.
    assert_eq!(
        pick(GuidancePage::Sources, &[F::SourcesNoneConfigured]).action,
        Some(GuidanceAction::AddGameFolder)
    );
}

#[test]
fn legacy_scripts_offer_no_action() {
    for script in CATALOGUE
        .iter()
        .filter(|s| s.origin == ScriptOrigin::Legacy)
    {
        assert!(
            script.action.is_none() && script.action_alternates.is_empty(),
            "{}",
            script.id
        );
    }
}

#[test]
fn mascot_state_follows_the_same_selection_and_is_consistent_with_category() {
    for script in CATALOGUE {
        match script.category {
            GuidanceCategory::Warning => {
                assert_eq!(script.mascot, MascotState::Warning, "{}", script.id)
            }
            GuidanceCategory::Success => {
                assert_eq!(script.mascot, MascotState::Success, "{}", script.id)
            }
            _ => {
                assert_ne!(
                    script.mascot,
                    MascotState::Warning,
                    "{} uses the warning mascot",
                    script.id
                );
                assert_ne!(
                    script.mascot,
                    MascotState::Success,
                    "{} uses the success mascot",
                    script.id
                );
            }
        }
        if script.mascot == MascotState::Concerned {
            assert_eq!(
                script.category,
                GuidanceCategory::WhyBlocked,
                "{}",
                script.id
            );
        }
    }
    // Ordinary missing firmware is a helpful blocker, not an alarm.
    assert_eq!(
        pick(GuidancePage::Launch, &[F::FirmwareMissing])
            .script
            .mascot,
        MascotState::Helpful
    );
    // A failed launch needs recovery; unknown identity is still being weighed.
    let failed = pick(
        GuidancePage::Launch,
        &[F::LaunchFailed {
            plain_failure_reason: "the emulator exited".into(),
            operation_id: 1,
        }],
    );
    assert_eq!(failed.script.mascot, MascotState::Concerned);
    assert_eq!(
        pick(
            GuidancePage::Games,
            &[F::IdentityUnknown, F::OperationNeedsStrongerIdentity]
        )
        .script
        .mascot,
        MascotState::Thinking
    );
    // The item carries the mascot of the script it came from.
    let item = failed.item(GuidanceLevel::Quick);
    assert_eq!(item.mascot, failed.script.mascot);
}

// --- No false authority ---------------------------------------------------------------

#[test]
fn unknown_candidate_and_unsupported_never_read_as_verified_or_ready() {
    let unknown = pick(GuidancePage::Games, &[F::IdentityUnknown]);
    let unknown_text = unknown.item(GuidanceLevel::Quick).message;
    assert!(
        unknown_text.contains("cannot safely identify"),
        "{unknown_text}"
    );
    assert_ne!(unknown.script.category, GuidanceCategory::Success);

    let candidate = pick(
        GuidancePage::Games,
        &[F::IdentityCandidateOnly {
            title: "Pac-Man".into(),
        }],
    );
    let text = candidate.item(GuidanceLevel::Quick).message;
    assert!(
        text.contains("may be Pac-Man") && text.contains("has not been verified"),
        "{text}"
    );
    assert_ne!(candidate.script.category, GuidanceCategory::Success);
    assert_eq!(candidate.script.mascot, MascotState::Thinking);

    let unsupported = pick(
        GuidancePage::Converter,
        &[F::FormatOperationUnsupported {
            format: "CHD".into(),
            operation: "repair".into(),
        }],
    );
    assert_eq!(unsupported.script.category, GuidanceCategory::WhyBlocked);
    assert!(
        unsupported
            .item(GuidanceLevel::Quick)
            .message
            .contains("cannot perform repair")
    );
}

#[test]
fn blocked_stays_blocked_even_beside_contradictory_good_news() {
    let selection = pick(
        GuidancePage::Launch,
        &[
            F::LaunchReady,
            F::LaunchNoCompatibleEmulator,
            F::RepairCompletedVerified { operation_id: 1 },
        ],
    );
    assert_eq!(selection.script.id, "launch.no_compatible_emulator");
    assert_eq!(selection.script.category, GuidanceCategory::WhyBlocked);
}

#[test]
fn no_success_script_requires_a_negative_fact_and_none_is_reachable_without_its_proof() {
    const NEGATIVE: [FactKind; 18] = [
        FactKind::IdentityUnknown,
        FactKind::IdentityCandidateOnly,
        FactKind::IdentityConflicting,
        FactKind::OperationNeedsStrongerIdentity,
        FactKind::LaunchFailed,
        FactKind::LaunchNoCompatibleEmulator,
        FactKind::FirmwareMissing,
        FactKind::UndoRefused,
        FactKind::SourceChangedSincePreview,
        FactKind::DestinationReadOnly,
        FactKind::FormatOperationUnsupported,
        FactKind::ConversionPreservationUnknown,
        FactKind::RepairCompletedVerificationPending,
        FactKind::VerificationUnavailable,
        FactKind::PatchBaseMismatch,
        FactKind::DatNoneAvailable,
        FactKind::MultiDiscRequiredMissing,
        FactKind::SourceUnavailable,
    ];
    for script in CATALOGUE
        .iter()
        .filter(|s| s.category == GuidanceCategory::Success)
    {
        for kind in script.requires {
            assert!(
                !NEGATIVE.contains(kind),
                "{} requires negative fact {kind:?}",
                script.id
            );
        }
    }
    // Success exists only where its proof is required.
    assert_eq!(
        script("repair.completed_verified").requires,
        [FactKind::RepairCompletedVerified]
    );
    assert_eq!(script("launch.ready").requires, [FactKind::LaunchReady]);
    assert!(
        script("launch.ready")
            .excludes
            .contains(&FactKind::LaunchWarningsUnacknowledged)
    );
}

#[test]
fn missing_or_empty_evidence_never_produces_an_invented_specific() {
    // A fact that promises a specific but carries none is ignored entirely.
    let empty: [(GuidancePage, GuidanceFact); 7] = [
        (
            GuidancePage::Games,
            F::IdentityCandidateOnly { title: "  ".into() },
        ),
        (
            GuidancePage::Sources,
            F::ScanPartialFailure { folder_count: 0 },
        ),
        (
            GuidancePage::Launch,
            F::LaunchFailed {
                plain_failure_reason: String::new(),
                operation_id: 1,
            },
        ),
        (
            GuidancePage::Organisation,
            F::MameMissingMembers { missing_count: 0 },
        ),
        (
            GuidancePage::History,
            F::UndoRefused {
                plain_undo_reason: String::new(),
            },
        ),
        (
            GuidancePage::Activity,
            F::QueuedWork {
                queued_count: 0,
                running_count: 3,
            },
        ),
        (
            GuidancePage::Launch,
            F::EmulatorMultipleInstallations {
                emulator: "Dolphin".into(),
                count: 1,
            },
        ),
    ];
    for (page, fact) in empty {
        assert!(!fact.is_valid(), "{fact:?}");
        let evidence = GuidanceEvidence {
            facts: vec![fact.clone()],
            ..Default::default()
        };
        assert!(
            !evidence.facts().contains(&fact),
            "an invalid fact survived into selection: {fact:?}"
        );
        if let Some(selection) = select(page, &evidence.facts()) {
            assert_eq!(selection.script.origin, ScriptOrigin::Legacy, "{fact:?}");
        }
    }
    // The same page with no such fact falls back to nothing specific.
    assert_eq!(id(GuidancePage::Games, &[]), "games-browse");
}

#[test]
fn a_script_whose_placeholder_has_no_value_is_not_eligible() {
    // Selecting with the *kind* but a fact that lacks the param cannot happen via
    // the public path; verify the guard by eligibility on a hand-built script.
    use super::script::{BASE, GuidanceScript};
    static NEEDS_TITLE: &[GuidanceScript] = &[GuidanceScript {
        id: "test.needs_title",
        requires: &[FactKind::IdentityUnknown],
        quick: "This may be {title}.",
        ..BASE
    }];
    assert!(select_in(NEEDS_TITLE, GuidancePage::Home, &[F::IdentityUnknown]).is_none());
}

// --- Repeat and suppression -------------------------------------------------------------

fn first_use() -> GuidanceSelection {
    pick(GuidancePage::Home, &[F::HomeFirstUseTipEligible])
}

#[test]
fn first_use_help_shows_once_then_a_compact_link() {
    let mut state = ExposureState::default();
    let tip = first_use();
    assert_eq!(tip.script.repeat, RepeatPolicy::FirstUse);
    assert_eq!(state.decide(&tip, 0), Exposure::Expanded);
    // Constructing the widget is not seeing it: until a Seen event, it stays full.
    assert_eq!(state.decide(&tip, 5), Exposure::Expanded);
    state.record(&tip, ExposureEvent::Seen, 10);
    assert_eq!(state.decide(&tip, 20), Exposure::Compact);
}

#[test]
fn optional_tips_are_rate_limited_by_topic_and_by_a_cooldown() {
    let mut state = ExposureState::default();
    let home = first_use();
    state.record(&home, ExposureEvent::Seen, 100);
    // A second unsolicited tip on the same topic is held back.
    let artwork = pick(
        GuidancePage::Artwork,
        &[F::ArtworkAlternatives {
            provider: "Provider".into(),
            alternative_count: 2,
        }],
    );
    assert_eq!(artwork.script.repeat, RepeatPolicy::FirstUse);
    assert_eq!(
        state.decide(&artwork, 100 + 60),
        Exposure::Suppressed,
        "inside the cooldown"
    );
    assert_eq!(
        state.decide(&artwork, 100 + OPTIONAL_TIP_COOLDOWN_SECS),
        Exposure::Expanded,
        "after the cooldown a tip about a different topic may appear"
    );
    // The cooldown is a function of the injected clock, never the wall clock.
    assert_eq!(
        state.decide(&artwork, 100 + OPTIONAL_TIP_COOLDOWN_SECS - 1),
        Exposure::Suppressed
    );
}

#[test]
fn a_blocker_can_be_collapsed_but_is_never_suppressed() {
    let mut state = ExposureState::default();
    let blocker = pick(GuidancePage::Launch, &[F::FirmwareMissing]);
    assert_eq!(blocker.script.repeat, RepeatPolicy::Blocker);
    assert_eq!(state.decide(&blocker, 0), Exposure::Expanded);
    state.record(&blocker, ExposureEvent::Collapsed, 1);
    assert_eq!(state.decide(&blocker, 2), Exposure::Compact);
    state.record(&blocker, ExposureEvent::Acknowledged, 3);
    for now in [4, 1_000, 1_000_000] {
        assert_ne!(state.decide(&blocker, now), Exposure::Suppressed, "t={now}");
    }
}

#[test]
fn a_success_is_suppressed_for_that_operation_but_a_new_operation_is_new() {
    let mut state = ExposureState::default();
    let success = |operation_id| {
        pick(
            GuidancePage::Home,
            &[F::RepairCompletedVerified { operation_id }],
        )
    };
    let first = success(1);
    assert_eq!(first.script.repeat, RepeatPolicy::PerOperation);
    assert_eq!(state.decide(&first, 0), Exposure::Expanded);
    state.record(&first, ExposureEvent::Acknowledged, 1);
    assert_eq!(
        state.decide(&first, 2),
        Exposure::Suppressed,
        "no spam forever"
    );
    assert_eq!(
        state.decide(&success(2), 2),
        Exposure::Expanded,
        "a different operation"
    );
}

#[test]
fn changed_relevant_evidence_reselects_and_resets_suppression_but_a_count_does_not() {
    let mut state = ExposureState::default();
    let failed = |reason: &str| {
        pick(
            GuidancePage::Launch,
            &[F::LaunchFailed {
                plain_failure_reason: reason.into(),
                operation_id: 7,
            }],
        )
    };
    let a = failed("the emulator exited");
    state.record(&a, ExposureEvent::Collapsed, 0);
    assert_eq!(state.decide(&a, 1), Exposure::Compact);
    // A materially different reason is a new event: show it in full again.
    let b = failed("the firmware is missing");
    assert_ne!(a.semantic_key(), b.semantic_key());
    assert_eq!(state.decide(&b, 1), Exposure::Expanded);
    // Ordinary counts and progress are not a new event.
    let partial = |folders| {
        pick(
            GuidancePage::Sources,
            &[F::ScanPartialFailure {
                folder_count: folders,
            }],
        )
    };
    let two = partial(2);
    state.record(&two, ExposureEvent::Collapsed, 0);
    assert_eq!(two.semantic_key(), partial(3).semantic_key());
    assert_eq!(state.decide(&partial(3), 1), Exposure::Compact);
}

#[test]
fn an_explicit_request_always_opens_and_reset_forgets_the_session() {
    let mut state = ExposureState::default();
    let tip = first_use();
    state.record(&tip, ExposureEvent::Seen, 0);
    assert_eq!(state.decide(&tip, 1), Exposure::Compact);
    assert_eq!(state.requested(&tip), Exposure::Expanded);
    state.reset();
    assert_eq!(state.decide(&tip, 1), Exposure::Expanded);
}

#[test]
fn selection_is_unaffected_by_what_exposure_has_suppressed() {
    let mut state = ExposureState::default();
    let tip = first_use();
    state.record(&tip, ExposureEvent::Seen, 0);
    // Compact for presentation, but still the selected script.
    assert_eq!(state.decide(&tip, 1), Exposure::Compact);
    assert_eq!(
        id(GuidancePage::Home, &[F::HomeFirstUseTipEligible]),
        tip.script.id
    );
}

// --- Semantic keys and provenance ------------------------------------------------------------

#[test]
fn provenance_explains_the_choice() {
    let selection = pick(
        GuidancePage::Launch,
        &[
            F::FirmwareMissing,
            F::LaunchNoCompatibleEmulator,
            F::ScanRunning,
        ],
    );
    let why = &selection.provenance;
    assert_eq!(why.priority, 90);
    assert!(why.page_specific);
    assert_eq!(why.matched_kinds, [FactKind::FirmwareMissing]);
    assert_eq!(why.outranked[0], "launch.no_compatible_emulator");
    assert!(why.outranked.contains(&"scan.running"));
    assert_eq!(why.other_blockers, ["launch.no_compatible_emulator"]);
}

// --- Compatibility with the pre-engine guidance ---------------------------------------------------

fn old(page: GuidancePage, edit: impl FnOnce(&mut GuidanceEvidence)) -> GuidanceTip {
    let mut context = GuidanceContext::new(page);
    edit(&mut context.evidence);
    GuidanceState::default()
        .select(&context)
        .expect("legacy pages always answer")
}

#[test]
fn launch_blocker_uses_only_explicit_identity_evidence() {
    let tip = old(GuidancePage::Launch, |e| {
        e.launch_identity_verified = Some(false)
    });
    assert_eq!(tip.category, GuidanceCategory::WhyBlocked);
    assert!(tip.message.contains("identity has not been verified"));
    assert_eq!(tip.key, "launch-identity-blocked");
}

#[test]
fn source_tip_does_not_invent_a_scan_time() {
    let tip = old(GuidancePage::Sources, |_| {});
    assert!(!tip.message.contains("last scanned"));
    assert!(!tip.message.contains("202"));
}

#[test]
fn tape_tip_uses_real_block_count_and_format() {
    let tip = old(GuidancePage::TapeInspector, |e| {
        e.tape_format = Some("TZX".into());
        e.tape_blocks = Some(12);
    });
    assert!(tip.message.contains("TZX contains 12 blocks"));
}

#[test]
fn problems_guidance_is_neutral_helpful_and_evidence_driven() {
    let problems = |actionable, attention| {
        old(GuidancePage::ProblemsRepair, |e| {
            e.problems_actionable = actionable;
            e.problems_needing_attention = attention;
        })
    };
    let checking = problems(None, None);
    assert_eq!(checking.mascot, MascotState::Thinking);
    assert!(checking.message.contains("checking"));
    let clear = problems(Some(0), Some(0));
    assert_eq!(clear.category, GuidanceCategory::Success);
    assert_eq!(clear.mascot, MascotState::Success);
    let busy = problems(Some(3), Some(2));
    assert_eq!(busy.category, GuidanceCategory::Explain);
    assert_eq!(busy.mascot, MascotState::Helpful);
    assert!(busy.message.contains("3 finding(s)"));
    assert!(busy.message.contains("2 needing attention"));
    assert!(busy.message.contains("Nothing changes until you confirm"));
    for tip in [&checking, &clear, &busy] {
        assert_ne!(tip.category, GuidanceCategory::Warning);
        assert_ne!(tip.mascot, MascotState::Warning);
    }
    let blocked = old(GuidancePage::ProblemsRepair, |e| {
        e.blocker = Some("Repair is blocked because the archive changed.".into());
        e.problems_actionable = Some(5);
    });
    assert_eq!(blocked.category, GuidanceCategory::WhyBlocked);
    assert_eq!(
        blocked.message,
        "Repair is blocked because the archive changed."
    );
    assert_ne!(blocked.mascot, MascotState::Warning);
}

#[test]
fn every_existing_page_still_answers_and_none_repeat_browse_first() {
    for page in all_pages()
        .into_iter()
        .filter(|p| *p != GuidancePage::Duplicates)
    {
        let tip = old(page, |_| {});
        if page != GuidancePage::Home {
            assert!(!tip.message.starts_with("Browse first"), "{page:?}");
        }
        assert!(tip.message.len() > 30, "{page:?} is a bare label");
    }
}

#[test]
fn activity_guidance_uses_the_real_running_count() {
    let idle = old(GuidancePage::Activity, |e| e.jobs_running = Some(0));
    assert!(idle.message.contains("Nothing is running"));
    let unknown = old(GuidancePage::Activity, |_| {});
    assert!(unknown.message.contains("Nothing is running"));
    let busy = old(GuidancePage::Activity, |e| e.jobs_running = Some(2));
    assert!(busy.message.contains("2 task(s) active"));
}

#[test]
fn home_guidance_distinguishes_an_empty_list_from_a_ready_one() {
    assert_eq!(
        old(GuidancePage::Home, |e| e.has_games = Some(false)).key,
        "home-empty"
    );
    assert_eq!(
        old(GuidancePage::Home, |e| e.has_games = Some(true)).key,
        "home-browse"
    );
    assert_eq!(old(GuidancePage::Home, |_| {}).key, "home-browse");
}

#[test]
fn exactly_one_legacy_message_applies_to_every_page_and_evidence_combination() {
    let bools = [None, Some(true), Some(false)];
    let counts = [None, Some(0usize), Some(3)];
    for page in all_pages() {
        for has_games in bools {
            for source_available in bools {
                for scan in [None, Some("a time".to_string())] {
                    for identity in bools {
                        for blocker in [None, Some("blocked".to_string())] {
                            for actionable in counts {
                                for jobs in counts {
                                    let evidence = GuidanceEvidence {
                                        has_games,
                                        source_available,
                                        source_last_scan: scan.clone(),
                                        launch_identity_verified: identity,
                                        blocker: blocker.clone(),
                                        tape_format: Some("TAP".into()),
                                        tape_blocks: has_games.map(|_| 4),
                                        dat_name: scan.clone(),
                                        problems_actionable: actionable,
                                        problems_needing_attention: actionable,
                                        jobs_running: jobs,
                                        ..Default::default()
                                    };
                                    let facts = evidence.facts();
                                    let legacy = eligible(CATALOGUE, page, &facts)
                                        .into_iter()
                                        .filter(|c| c.script.origin == ScriptOrigin::Legacy)
                                        .count();
                                    // Exactly one pre-engine message applies to every page
                                    // that has one, whatever the evidence: no ties, no gaps.
                                    let expected = usize::from(page != GuidancePage::Duplicates);
                                    assert_eq!(legacy, expected, "{page:?} {evidence:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn legacy_pages_never_tie_for_the_winning_message() {
    for page in all_pages() {
        for facts in [
            vec![],
            vec![F::LegacyLibraryEmpty],
            vec![
                F::LegacySourceUnavailable,
                F::LegacySourceLastScan { scan: "t".into() },
            ],
        ] {
            let found = eligible(CATALOGUE, page, &facts);
            if let [first, second, ..] = found.as_slice() {
                let a = first.script;
                let b = second.script;
                assert!(
                    (
                        a.priority,
                        a.scope,
                        !a.pages.is_empty(),
                        a.category.precedence()
                    ) != (
                        b.priority,
                        b.scope,
                        !b.pages.is_empty(),
                        b.category.precedence()
                    ) || a.id < b.id,
                    "{} and {} tie on {page:?}",
                    a.id,
                    b.id
                );
            }
        }
    }
}

// --- Catalogue lints -----------------------------------------------------------------------

const LEGACY_KEYS: [&str; 35] = [
    "home-empty",
    "home-browse",
    "source-unavailable-history",
    "source-unavailable",
    "source-last-scan",
    "source-review",
    "launch-identity-blocked",
    "launch-verified",
    "launch-checks",
    "problem-blocker",
    "problem-checking",
    "problem-none",
    "problem-review",
    "organisation-preview",
    "cheats-mods-review",
    "museum-browse",
    "tape-structure",
    "tape-review",
    "archive-review",
    "dat-provenance",
    "dat-review",
    "firmware-review",
    "emulator-readiness",
    "setup-fix-first",
    "check-platform",
    "activity-idle",
    "activity-busy",
    "history-undo",
    "saves-kinds",
    "converter-preview",
    "artwork-game",
    "romm-readonly",
    "advanced-inspect",
    "settings-hints",
    "games-browse",
];

#[test]
fn the_catalogue_has_the_42_designed_scripts_their_variants_and_the_35_legacy_keys() {
    let r = report(CATALOGUE);
    assert_eq!(r.design_numbered, 42, "{r}");
    assert!(r.design_numbers_missing.is_empty(), "{r}");
    assert!(r.design_numbers_duplicated.is_empty(), "{r}");
    assert_eq!(r.design_variants, 4, "{r}");
    assert_eq!(r.legacy, 35, "{r}");
    assert_eq!(r.total, 42 + 4 + 35);
    // Every pre-engine message key survives with the same identity.
    for key in LEGACY_KEYS {
        let legacy = script(key);
        assert_eq!(legacy.origin, ScriptOrigin::Legacy, "{key}");
    }
    assert_eq!(
        CATALOGUE
            .iter()
            .filter(|s| s.origin == ScriptOrigin::Legacy)
            .count(),
        LEGACY_KEYS.len()
    );
}

#[test]
fn every_stable_script_id_is_unique_and_a_variant_names_a_real_script() {
    let r = report(CATALOGUE);
    assert!(
        r.duplicate_ids.is_empty(),
        "duplicate IDs: {:?}",
        r.duplicate_ids
    );
    for script in CATALOGUE {
        assert!(!script.id.is_empty());
        if let Some(base) = script.variant_of {
            let parent = self::script(base);
            assert!(
                parent.design_number.is_some(),
                "{} varies a non-numbered script",
                script.id
            );
            assert_eq!(script.origin, ScriptOrigin::Design);
        }
        for replacement in script.replaced_by {
            assert!(
                CATALOGUE
                    .iter()
                    .any(|s| s.id == *replacement && s.origin == ScriptOrigin::Design),
                "{} is replaced by unknown {replacement}",
                script.id
            );
        }
    }
}

#[test]
fn there_are_no_unreachable_scripts_and_no_duplicate_selectors() {
    let r = report(CATALOGUE);
    assert!(
        r.unreachable.is_empty(),
        "unreachable under the selector rules: {:?}",
        r.unreachable
    );
    assert!(
        r.duplicate_selectors.is_empty(),
        "duplicate selectors: {:?}",
        r.duplicate_selectors
    );
}

#[test]
fn every_design_topic_and_the_activity_supplement_has_a_script() {
    let r = report(CATALOGUE);
    assert!(
        r.topics_without_scripts.is_empty(),
        "{:?}",
        r.topics_without_scripts
    );
    assert_eq!(GuidanceTopic::DESIGN.len(), 29);
    let categories: Vec<_> = all_categories()
        .into_iter()
        .filter(|c| !CATALOGUE.iter().any(|s| s.category == *c))
        .collect();
    assert!(categories.is_empty(), "unused categories: {categories:?}");
}

#[test]
fn priorities_use_only_the_fixed_bands_and_categories_sit_in_sensible_bands() {
    for script in CATALOGUE {
        assert!(
            PRIORITY_BANDS.contains(&script.priority),
            "{} priority {}",
            script.id,
            script.priority
        );
        match script.category {
            GuidanceCategory::WhyBlocked => assert!(script.priority >= 80, "{}", script.id),
            GuidanceCategory::Warning => assert!(script.priority >= 70, "{}", script.id),
            GuidanceCategory::Success => assert!(script.priority <= 60, "{}", script.id),
            GuidanceCategory::Tip => assert!(script.priority <= 10, "{}", script.id),
            GuidanceCategory::Explain | GuidanceCategory::EmptyState => {}
        }
        assert!(
            (script.repeat == RepeatPolicy::Blocker)
                == (script.category == GuidanceCategory::WhyBlocked)
                || script.origin == ScriptOrigin::Legacy
                || script.variant_of.is_some(),
            "{} repeat policy disagrees with its category",
            script.id
        );
    }
}

#[test]
fn design_scripts_author_every_level_and_never_lengthen_the_minimal_form() {
    for script in CATALOGUE
        .iter()
        .filter(|s| s.origin == ScriptOrigin::Design)
    {
        let minimal = script.minimal.expect(script.id);
        assert!(
            script.explain.is_some() && script.technical.is_some(),
            "{}",
            script.id
        );
        assert!(
            !script.requires.is_empty(),
            "{} can appear with no evidence",
            script.id
        );
        assert!(
            minimal.len() <= script.quick.len() + 12,
            "{} minimal is not shorter",
            script.id
        );
    }
    for script in CATALOGUE {
        assert!(!script.quick.trim().is_empty(), "{}", script.id);
    }
}

#[test]
fn every_placeholder_is_supplied_by_a_required_fact_with_the_right_kind() {
    for script in CATALOGUE {
        let supplied: Vec<_> = script
            .requires
            .iter()
            .flat_map(|k| k.params().iter())
            .collect();
        for text in script.texts() {
            for (name, needs_count) in
                placeholders(text).unwrap_or_else(|e| panic!("{}: {e}", script.id))
            {
                let found = supplied.iter().find(|(n, _)| *n == name);
                let (_, kind) = found.unwrap_or_else(|| {
                    panic!("{} uses {{{name}}} no required fact supplies", script.id)
                });
                if needs_count {
                    assert_eq!(
                        *kind,
                        super::model::ParamKind::Count,
                        "{} plural on text {name}",
                        script.id
                    );
                }
            }
        }
    }
}

#[test]
fn every_script_renders_with_representative_values_and_leaves_no_braces() {
    for script in CATALOGUE {
        let facts: Vec<_> = script.requires.iter().map(|k| k.sample()).collect();
        let page = script.pages.first().copied().unwrap_or(GuidancePage::Home);
        let selection = select_in(CATALOGUE, page, &facts).expect(script.id);
        // Render from this script's own facts, whichever script won.
        for level in [
            GuidanceLevel::Minimal,
            GuidanceLevel::Quick,
            GuidanceLevel::Explain,
            GuidanceLevel::Technical,
        ] {
            let message = selection.item(level).message;
            assert!(
                !message.is_empty() && !message.contains('{') && !message.contains('}'),
                "{}: {message}",
                script.id
            );
        }
    }
}

#[test]
fn singular_and_plural_forms_are_explicit_and_never_use_s_in_brackets() {
    let one = pick(
        GuidancePage::Sources,
        &[F::ScanPartialFailure { folder_count: 1 }],
    )
    .item(GuidanceLevel::Quick)
    .message;
    let many = pick(
        GuidancePage::Sources,
        &[F::ScanPartialFailure { folder_count: 3 }],
    )
    .item(GuidanceLevel::Quick)
    .message;
    assert!(
        one.contains("1 folder could not") && one.contains("that folder"),
        "{one}"
    );
    assert!(
        many.contains("3 folders could not") && many.contains("those folders"),
        "{many}"
    );
    let queued = |q, r| {
        pick(
            GuidancePage::Home,
            &[F::QueuedWork {
                queued_count: q,
                running_count: r,
            }],
        )
        .item(GuidanceLevel::Quick)
        .message
    };
    assert!(
        queued(1, 1).starts_with("1 task is waiting to start; 1 is running."),
        "{}",
        queued(1, 1)
    );
    assert!(
        queued(4, 2).starts_with("4 tasks are waiting to start; 2 are running."),
        "{}",
        queued(4, 2)
    );
    for script in CATALOGUE
        .iter()
        .filter(|s| s.origin == ScriptOrigin::Design)
    {
        for text in script.texts() {
            assert!(
                !text.contains("(s)"),
                "{} uses a bracketed plural",
                script.id
            );
        }
    }
    assert!(
        pick(
            GuidancePage::Launch,
            &[F::EmulatorMultipleInstallations {
                emulator: "Dolphin".into(),
                count: 2
            }]
        )
        .item(GuidanceLevel::Quick)
        .message
        .starts_with("Two Dolphin installations were found.")
    );
}

#[test]
fn the_voice_guide_is_respected() {
    // Retired name, filler and exclamation marks (design section 2).
    for script in CATALOGUE
        .iter()
        .filter(|s| s.origin == ScriptOrigin::Design)
    {
        for text in script.texts() {
            assert!(!text.contains('!'), "{} shouts: {text}", script.id);
            let lower = text.to_lowercase();
            for banned in ["wizzy", "obviously", "baby"] {
                assert!(!lower.contains(banned), "{} contains {banned}", script.id);
            }
            // "just" as a minimiser ("just do X") is discouraged; "just because" is not.
            let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).collect();
            for (index, word) in words.iter().enumerate() {
                let minimising_just = *word == "just" && words.get(index + 1) != Some(&"because");
                assert!(
                    !minimising_just && !matches!(*word, "easy" | "obviously"),
                    "{} uses '{word}': {text}",
                    script.id
                );
            }
        }
    }
}

// --- Helpers ----------------------------------------------------------------------------------

fn all_pages() -> [GuidancePage; 24] {
    use GuidancePage as P;
    [
        P::Home,
        P::Sources,
        P::Games,
        P::Launch,
        P::ProblemsRepair,
        P::Organisation,
        P::CheatsMods,
        P::Museum,
        P::TapeInspector,
        P::ArchiveInspector,
        P::DatManagement,
        P::BiosFirmware,
        P::EmulatorSetup,
        P::Setup,
        P::CheckGames,
        P::Activity,
        P::History,
        P::Saves,
        P::Converter,
        P::Artwork,
        P::Romm,
        P::Advanced,
        P::Settings,
        P::Duplicates,
    ]
}

/// A page both selectors allow (an empty list means any page).
fn common_page(a: &[GuidancePage], b: &[GuidancePage]) -> Option<GuidancePage> {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Some(GuidancePage::Home),
        (true, false) => b.first().copied(),
        (false, true) => a.first().copied(),
        (false, false) => a.iter().find(|page| b.contains(page)).copied(),
    }
}

fn valid(facts: Vec<GuidanceFact>) -> Vec<GuidanceFact> {
    facts.into_iter().filter(GuidanceFact::is_valid).collect()
}

fn is_eligible(
    script: &super::script::GuidanceScript,
    page: GuidancePage,
    facts: &[GuidanceFact],
) -> bool {
    eligible(CATALOGUE, page, facts)
        .iter()
        .any(|c| c.script.id == script.id)
}

/// Prints the catalogue report: `cargo test -p archivefs-gui --lib
/// print_catalogue_report -- --ignored --nocapture`.
#[test]
#[ignore = "development report, prints to stdout"]
fn print_catalogue_report() {
    println!("{}", report(CATALOGUE));
}
