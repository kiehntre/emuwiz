//! Phase 2A: the evidence adapters. Authority first: a canonical "unknown",
//! "candidate", "blocked" or "unsupported" must never come out stronger.

use std::collections::BTreeSet;

use archivefs_core::launch::{CanonicalIdentityStatus, FirmwareReadiness};

use super::evidence::*;
use super::evidence_coverage::{ALL_KINDS, Coverage, coverage_of, report};
use super::model::{FactKind as K, GuidanceFact as F, GuidancePage};
use super::select::select;
use crate::gamer_artwork::{CoverAnswer, NoCover};
use crate::gui_v2::launch_readiness_summary::{
    FirmwareSummary, GameReadinessSummary, IdentitySummary, ReadinessFreshness,
    ReadinessPresentationState as State, SourceSummary,
};

fn kinds(facts: &[F]) -> BTreeSet<K> {
    facts.iter().map(F::kind).collect()
}

fn summary(status: State, freshness: ReadinessFreshness) -> GameReadinessSummary {
    GameReadinessSummary {
        status,
        headline: String::new(),
        explanation: String::new(),
        emulator: None,
        firmware: FirmwareSummary::Unknown,
        identity: IdentitySummary::NeedsReview,
        source: SourceSummary::Available,
        warnings: Vec::new(),
        primary_action: None,
        details: Vec::new(),
        freshness,
        findings: Vec::new(),
        attempt: None,
    }
}

fn launch(status: State) -> Vec<F> {
    LaunchGuidanceEvidence {
        readiness: Some(summary(status, ReadinessFreshness::Current)),
        ..Default::default()
    }
    .facts()
}

fn identity(status: CanonicalIdentityStatus, title: Option<&str>, strict: bool) -> Vec<F> {
    GameGuidanceEvidence {
        identity: Some(status),
        candidate_title: title.map(str::to_string),
        operation_needs_verified_identity: Some(strict),
        ..Default::default()
    }
    .facts()
}

fn resolved() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(archivefs_core::launch::ResolvedIdentity {
        platform_id: "psx".into(),
        game_key: "SLUS-00000".into(),
    })
}

#[test]
fn unknown_candidate_and_conflicting_identity_never_become_verified() {
    // (canonical status, candidate title, expected kinds)
    let table: Vec<(CanonicalIdentityStatus, Option<&str>, Vec<K>)> = vec![
        (
            CanonicalIdentityStatus::Unknown,
            None,
            vec![K::IdentityUnknown],
        ),
        (
            CanonicalIdentityStatus::Unknown,
            Some("Maybe Game"),
            vec![K::IdentityUnknown, K::IdentityCandidateOnly],
        ),
        // Conflict stays a conflict, and a candidate never softens it.
        (
            CanonicalIdentityStatus::Conflicting,
            Some("Maybe Game"),
            vec![K::IdentityConflicting],
        ),
        // Resolved produces no design fact: Mr Wiz does not announce verification.
        (resolved(), Some("Maybe Game"), vec![]),
    ];
    for (status, title, expected) in table {
        let got = kinds(&identity(status.clone(), title, true));
        let expected_set: BTreeSet<K> = expected.iter().copied().collect();
        let got_without_strict: BTreeSet<K> = got
            .iter()
            .copied()
            .filter(|k| *k != K::OperationNeedsStrongerIdentity)
            .collect();
        assert_eq!(got_without_strict, expected_set, "{status:?}");
        // The strict-operation flag only ever rides on an unknown identity.
        assert_eq!(
            got.contains(&K::OperationNeedsStrongerIdentity),
            status == CanonicalIdentityStatus::Unknown
        );
    }
}

#[test]
fn nothing_loaded_yields_nothing() {
    assert!(PageGuidanceEvidence::default().facts().is_empty());
    assert!(identity_none().is_empty());
}

fn identity_none() -> Vec<F> {
    GameGuidanceEvidence::default().facts()
}

#[test]
fn blocked_unsupported_and_partial_launch_states_are_not_called_ready() {
    for status in [
        State::Checking,
        State::ReadyWithWarnings,
        State::NeedsIdentityReview,
        State::SourceUnavailable,
        State::MediaUnreadable,
        State::Blocked,
        State::Stale,
    ] {
        assert!(
            !kinds(&launch(status)).contains(&K::LaunchReady),
            "{status:?}"
        );
    }
    assert_eq!(
        kinds(&launch(State::Ready)),
        BTreeSet::from([K::LaunchReady])
    );
    assert_eq!(
        kinds(&launch(State::NeedsEmulator)),
        BTreeSet::from([K::LaunchNoCompatibleEmulator])
    );
    assert_eq!(
        kinds(&launch(State::NeedsFirmware)),
        BTreeSet::from([K::FirmwareMissing])
    );
}

#[test]
fn a_ready_summary_that_is_stale_or_carries_warnings_is_not_ready() {
    let stale = LaunchGuidanceEvidence {
        readiness: Some(summary(State::Ready, ReadinessFreshness::Stale)),
        ..Default::default()
    };
    assert!(stale.facts().is_empty());
    let mut with_warning = summary(State::Ready, ReadinessFreshness::Current);
    with_warning.warnings.push("check".into());
    let warned = LaunchGuidanceEvidence {
        readiness: Some(with_warning),
        ..Default::default()
    };
    assert!(warned.facts().is_empty());
}

#[test]
fn only_confirmed_missing_firmware_is_missing_firmware() {
    for (readiness, missing) in [
        (FirmwareReadiness::Missing, true),
        (FirmwareReadiness::Unknown, false),
        (FirmwareReadiness::PresentUnverified, false),
        (FirmwareReadiness::Verified, false),
        (FirmwareReadiness::NotRequired, false),
    ] {
        let facts = LaunchGuidanceEvidence {
            firmware: Some(readiness),
            ..Default::default()
        }
        .facts();
        assert_eq!(
            kinds(&facts).contains(&K::FirmwareMissing),
            missing,
            "{readiness:?}"
        );
    }
}

#[test]
fn library_and_selection_states_map_one_to_one() {
    let lib = |e: LibraryGuidanceEvidence| kinds(&e.facts());
    assert_eq!(
        lib(LibraryGuidanceEvidence {
            games_loaded: Some(0),
            ..Default::default()
        }),
        BTreeSet::from([K::LibraryLoadedEmpty])
    );
    // Loading (None) or a populated library is not "empty".
    assert!(
        lib(LibraryGuidanceEvidence {
            games_loaded: Some(3),
            ..Default::default()
        })
        .is_empty()
    );
    assert!(lib(LibraryGuidanceEvidence::default()).is_empty());
    assert_eq!(
        lib(LibraryGuidanceEvidence {
            sources_configured: Some(false),
            ..Default::default()
        }),
        BTreeSet::from([K::SourcesNoneConfigured])
    );
    assert!(
        lib(LibraryGuidanceEvidence {
            sources_configured: Some(true),
            ..Default::default()
        })
        .is_empty()
    );
    // A partial scan stays partial; zero unreadable folders is not a failure.
    assert!(
        lib(LibraryGuidanceEvidence {
            scan_unreadable_folders: Some(0),
            ..Default::default()
        })
        .is_empty()
    );
    assert_eq!(
        lib(LibraryGuidanceEvidence {
            scan_unreadable_folders: Some(2),
            ..Default::default()
        }),
        BTreeSet::from([K::ScanPartialFailure])
    );
    let sel = |s| {
        kinds(
            &GameGuidanceEvidence {
                game_selected: s,
                ..Default::default()
            }
            .facts(),
        )
    };
    assert_eq!(sel(Some(false)), BTreeSet::from([K::NoGameSelected]));
    assert!(sel(Some(true)).is_empty());
    assert!(sel(None).is_empty());
}

#[test]
fn source_states() {
    let s = |availability, review| {
        kinds(
            &SourceGuidanceEvidence {
                availability,
                changed_since_preview: review,
                ..Default::default()
            }
            .facts(),
        )
    };
    assert_eq!(
        s(Some(SourceAvailability::Unavailable), None),
        BTreeSet::from([K::SourceUnavailable])
    );
    assert!(s(Some(SourceAvailability::Available), None).is_empty());
    assert_eq!(
        s(None, Some(true)),
        BTreeSet::from([K::SourceChangedSincePreview])
    );
    assert!(s(None, Some(false)).is_empty());
}

#[test]
fn artwork_answers_are_not_reinterpreted() {
    let cover = |answer: CoverAnswer| {
        ArtworkGuidanceEvidence {
            cover: CoverEvidence::from_answer(&answer),
        }
        .facts()
    };
    assert_eq!(
        kinds(&cover(CoverAnswer::Unchanged { key: "k".into() })),
        BTreeSet::from([K::ArtworkCachedUsable])
    );
    assert_eq!(
        kinds(&cover(CoverAnswer::None(NoCover::NoArtwork))),
        BTreeSet::from([K::ArtworkCoverMissing])
    );
    assert_eq!(
        kinds(&cover(CoverAnswer::None(NoCover::Unavailable))),
        BTreeSet::from([K::ArtworkSourceUnavailable])
    );
    // A failure, a public-only reference and a missing RomM identity say nothing.
    for no in [
        NoCover::Failed,
        NoCover::PublicOnly,
        NoCover::NoRommIdentity,
    ] {
        assert!(cover(CoverAnswer::None(no)).is_empty(), "{no:?}");
    }
}

#[test]
fn operation_success_and_failure_keep_their_own_verification() {
    let repair = |verification| {
        kinds(
            &OperationGuidanceEvidence {
                outcome: Some(OperationOutcome::RepairCompleted {
                    operation_id: 7,
                    verification,
                }),
                ..Default::default()
            }
            .facts(),
        )
    };
    assert_eq!(
        repair(RepairVerification::Verified),
        BTreeSet::from([K::RepairCompletedVerified])
    );
    // Pending/unavailable verification is never promoted to verified.
    for v in [RepairVerification::Pending, RepairVerification::Unavailable] {
        assert!(!repair(v).contains(&K::RepairCompletedVerified), "{v:?}");
        assert!(repair(v).contains(&K::RepairCompletedVerificationPending));
    }
    assert!(repair(RepairVerification::Unavailable).contains(&K::VerificationUnavailable));
    let failed = OperationGuidanceEvidence {
        outcome: Some(OperationOutcome::LaunchFailed {
            operation_id: 3,
            plain_failure_reason: "The emulator stopped.".into(),
        }),
        ..Default::default()
    }
    .facts();
    assert_eq!(kinds(&failed), BTreeSet::from([K::LaunchFailed]));
    // An empty reason is not evidence (the fact is dropped as invalid).
    let blank = PageGuidanceEvidence {
        operation: OperationGuidanceEvidence {
            outcome: Some(OperationOutcome::LaunchFailed {
                operation_id: 3,
                plain_failure_reason: " ".into(),
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(blank.facts().is_empty());
}

#[test]
fn unsupported_and_read_only_pass_through_and_nothing_more() {
    let facts = OperationGuidanceEvidence {
        unsupported: Some(("CHD".into(), "repair".into())),
        destination_read_only: Some(true),
        ..Default::default()
    }
    .facts();
    assert_eq!(
        kinds(&facts),
        BTreeSet::from([K::FormatOperationUnsupported, K::DestinationReadOnly])
    );
    assert!(
        OperationGuidanceEvidence {
            destination_read_only: Some(false),
            ..Default::default()
        }
        .facts()
        .is_empty()
    );
}

#[test]
fn cheats_none_known_only_after_a_finished_lookup() {
    let c = |n| CheatGuidanceEvidence { known_cheats: n }.facts();
    assert_eq!(kinds(&c(Some(0))), BTreeSet::from([K::CheatsNoneKnown]));
    assert!(c(Some(4)).is_empty());
    assert!(c(None).is_empty());
}

#[test]
fn multiple_installations_needs_a_real_count() {
    let e = |count| PageGuidanceEvidence {
        launch: LaunchGuidanceEvidence {
            installations: Some(EmulatorInstallations {
                emulator: "Dolphin".into(),
                count,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(e(1).facts().is_empty());
    assert_eq!(
        kinds(&e(2).facts()),
        BTreeSet::from([K::EmulatorMultipleInstallations])
    );
}

fn busy_page() -> PageGuidanceEvidence {
    PageGuidanceEvidence {
        library: LibraryGuidanceEvidence {
            sources_configured: Some(true),
            games_loaded: Some(0),
            scan_running: Some(false),
            scan_unreadable_folders: Some(2),
            dats_available: Some(0),
        },
        source: SourceGuidanceEvidence {
            availability: Some(SourceAvailability::Unavailable),
            optional_provider_unreachable: Some(true),
            changed_since_preview: Some(true),
        },
        game: GameGuidanceEvidence {
            game_selected: Some(false),
            identity: Some(CanonicalIdentityStatus::Unknown),
            candidate_title: Some("Maybe".into()),
            operation_needs_verified_identity: Some(true),
        },
        launch: LaunchGuidanceEvidence {
            readiness: Some(summary(State::NeedsEmulator, ReadinessFreshness::Current)),
            firmware: Some(FirmwareReadiness::Missing),
            installations: Some(EmulatorInstallations {
                emulator: "Dolphin".into(),
                count: 2,
            }),
            bios_folder_chooser_available: Some(true),
        },
        artwork: ArtworkGuidanceEvidence {
            cover: Some(CoverEvidence::NoneRecorded),
        },
        cheats: CheatGuidanceEvidence {
            known_cheats: Some(0),
        },
        operation: OperationGuidanceEvidence {
            outcome: Some(OperationOutcome::RepairCompleted {
                operation_id: 1,
                verification: RepairVerification::Pending,
            }),
            queued: Some(2),
            running: Some(1),
            destination_read_only: Some(true),
            unsupported: Some(("CHD".into(), "repair".into())),
        },
    }
}

#[test]
fn the_same_state_gives_the_same_facts_and_fact_order_changes_no_selection() {
    let page = busy_page();
    assert_eq!(page.facts(), page.clone().facts());
    let facts = page.facts();
    let mut reversed = facts.clone();
    reversed.reverse();
    for target in [
        GuidancePage::Games,
        GuidancePage::Launch,
        GuidancePage::Sources,
    ] {
        let a = select(target, &facts).map(|s| s.semantic_key());
        let b = select(target, &reversed).map(|s| s.semantic_key());
        assert_eq!(a, b, "{target:?}");
    }
}

#[test]
fn adapters_emit_valid_facts_with_one_per_kind() {
    let facts = busy_page().facts();
    assert!(facts.iter().all(F::is_valid));
    assert_eq!(kinds(&facts).len(), facts.len());
}

#[test]
fn adapted_facts_flow_through_the_existing_selector() {
    let page = PageGuidanceEvidence {
        game: GameGuidanceEvidence {
            game_selected: Some(false),
            ..Default::default()
        },
        ..Default::default()
    };
    let chosen = select(GuidancePage::CheatsMods, &page.facts()).expect("selects");
    assert_eq!(chosen.script.id, "cheats.no_game_selected");
}

/// Every kind the coverage table says an adapter emits is emitted by one, and no
/// adapter emits a kind the table does not list.
#[test]
fn the_coverage_table_matches_what_the_adapters_emit() {
    let mut emitted = BTreeSet::new();
    emitted.extend(kinds(&busy_page().facts()));
    // Branches that exclude one another in `busy_page`.
    for ident in [CanonicalIdentityStatus::Conflicting] {
        emitted.extend(kinds(&identity(ident, None, false)));
    }
    for outcome in [
        OperationOutcome::LaunchFailed {
            operation_id: 1,
            plain_failure_reason: "x".into(),
        },
        OperationOutcome::RepairCompleted {
            operation_id: 1,
            verification: RepairVerification::Verified,
        },
        OperationOutcome::RepairCompleted {
            operation_id: 1,
            verification: RepairVerification::Unavailable,
        },
    ] {
        emitted.extend(kinds(
            &OperationGuidanceEvidence {
                outcome: Some(outcome),
                ..Default::default()
            }
            .facts(),
        ));
    }
    emitted.extend(kinds(&launch(State::Ready)));
    emitted.extend(kinds(
        &ArtworkGuidanceEvidence {
            cover: Some(CoverEvidence::Usable),
        }
        .facts(),
    ));
    emitted.extend(kinds(
        &ArtworkGuidanceEvidence {
            cover: Some(CoverEvidence::SourceUnavailable),
        }
        .facts(),
    ));
    emitted.extend(kinds(
        &LibraryGuidanceEvidence {
            scan_running: Some(true),
            sources_configured: Some(false),
            ..Default::default()
        }
        .facts(),
    ));

    let listed: BTreeSet<K> = ALL_KINDS
        .iter()
        .copied()
        .filter(|k| !coverage_of(*k).adapter.is_empty() && !format!("{k:?}").starts_with("Legacy"))
        .collect();
    assert_eq!(emitted, listed);
}

#[test]
fn coverage_report_is_complete_and_honest() {
    let report = report(super::catalogue::CATALOGUE);
    assert_eq!(report.scripts.len(), 81);
    // Unsupported claims: nothing marked available/simple without a source.
    for kind in ALL_KINDS {
        let row = coverage_of(*kind);
        assert!(
            !row.source.is_empty(),
            "{kind:?} has no recorded canonical source"
        );
        if row.coverage >= Coverage::NeedsNewProductState {
            assert!(
                row.adapter.is_empty(),
                "{kind:?} is blocked but has an adapter"
            );
        }
    }
    // Every catalogue kind is classified.
    for script in super::catalogue::CATALOGUE {
        for kind in script.requires.iter().chain(script.excludes) {
            assert!(ALL_KINDS.contains(kind), "{kind:?}");
        }
    }
    assert!(report.receivable_now().len() + report.without_adapter().len() == 81);
}

#[test]
#[ignore = "prints the Phase 2A wiring checklist"]
fn print_evidence_coverage() {
    println!("{}", report(super::catalogue::CATALOGUE));
}
