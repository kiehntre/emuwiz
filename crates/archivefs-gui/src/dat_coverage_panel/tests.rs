//! Tests for the Collection Coverage panel: pure projections and headless
//! rendering. No database, no DAT parse, no ROM access - the projections
//! take core structs built inline.

use std::collections::BTreeSet;

use archivefs_core::dat::coverage::{
    ArcadeDatSetCoverage, CompleteSetVerdict, DatCoverageUnit, ExpectedInventoryStatus,
    PlatformDatCoverage,
};
use archivefs_core::dat::model::DatEcosystem;

use super::*;

fn base_canonical() -> PlatformDatCoverage {
    PlatformDatCoverage {
        platform: "Game Boy Advance".to_string(),
        dat_source_id: "no-intro-gba".to_string(),
        source_name: Some("No-Intro - Game Boy Advance".to_string()),
        ecosystem: Some(DatEcosystem::NoIntro),
        source_revision: Some("2026-01-01".to_string()),
        unit: DatCoverageUnit::CanonicalDatEntry,
        owned_applicable: 1201,
        checked: 1195,
        verified_current: 1187,
        verified_stale: 3,
        probable: 0,
        unmatched: 4,
        ambiguous: 14,
        unknown: 0,
        duplicate_canonical_identities: 5,
        duplicate_extra_archives: 7,
        expected_inventory: ExpectedInventoryStatus::Available {
            entry_count: 1243,
            duplicate_names_skipped: 0,
        },
        expected_unique_count: Some(1243),
        represented_unique_count: Some(1201),
        missing_count: Some(42),
        completion_percent: Some(96.62),
        complete_set: CompleteSetVerdict::Incomplete { missing_count: 42 },
    }
}

fn base_arcade() -> ArcadeDatSetCoverage {
    ArcadeDatSetCoverage {
        platform: "Arcade".to_string(),
        dat_source_id: "mame".to_string(),
        source_platform_of_row: Some("Arcade".to_string()),
        ecosystem: Some(DatEcosystem::MAMEArcade),
        source_revision: Some("0.270".to_string()),
        unit: DatCoverageUnit::ArcadeSet,
        checked_sets: 40,
        complete_sets: 30,
        incomplete_sets: 8,
        bad_metadata_sets: 0,
        needs_review_sets: 2,
        stale_sets: 1,
        expected_inventory: ExpectedInventoryStatus::Available {
            entry_count: 100,
            duplicate_names_skipped: 0,
        },
        expected_sets: Some(100),
        represented_complete_sets: Some(30),
        missing_sets: Some(50),
        completion_percent: Some(30.0),
        complete_set: CompleteSetVerdict::Incomplete { missing_count: 70 },
    }
}

// --- projection: full set / expected status ------------------------------

#[test]
fn a_complete_verdict_projects_to_complete_with_extras() {
    let mut coverage = base_canonical();
    coverage.complete_set = CompleteSetVerdict::Complete {
        extra_duplicate_archives: 7,
    };
    coverage.missing_count = Some(0);
    let view = project_canonical(&coverage, MissingListView::default());
    assert_eq!(
        view.full_set,
        FullSetView::Complete {
            extra_duplicate_archives: 7
        }
    );
}

#[test]
fn an_incomplete_verdict_keeps_its_missing_count() {
    let view = project_canonical(&base_canonical(), MissingListView::default());
    assert_eq!(view.full_set, FullSetView::Incomplete { missing_count: 42 });
    assert_eq!(view.missing_count, Some(42));
}

#[test]
fn a_notprovable_verdict_never_becomes_incomplete() {
    let mut coverage = base_canonical();
    coverage.complete_set = CompleteSetVerdict::NotProvable {
        reason: "duplicate <game> names; not one-to-one".to_string(),
    };
    let view = project_canonical(&coverage, MissingListView::default());
    match view.full_set {
        FullSetView::NotProvable { reason } => assert!(reason.contains("duplicate")),
        other => panic!("expected NotProvable, got {other:?}"),
    }
}

#[test]
fn an_unassigned_source_projects_to_unavailable_with_a_reason_not_a_zero() {
    let mut coverage = base_canonical();
    coverage.expected_inventory = ExpectedInventoryStatus::PlatformUnassigned;
    coverage.expected_unique_count = None;
    coverage.missing_count = None;
    coverage.completion_percent = None;
    coverage.complete_set = CompleteSetVerdict::NotProvable {
        reason: "no explicit platform assignment".to_string(),
    };
    let view = project_canonical(&coverage, MissingListView::default());
    assert!(!view.expected.is_available());
    match &view.expected {
        ExpectedStatusView::Unavailable {
            headline,
            offer_validate,
            ..
        } => {
            assert!(headline.contains("Not assigned"));
            assert!(!offer_validate);
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
    assert_eq!(view.missing_count, None);
    assert_eq!(view.completion_percent, None);
}

#[test]
fn a_missing_inventory_offers_validation() {
    let mut coverage = base_canonical();
    coverage.expected_inventory = ExpectedInventoryStatus::InventoryMissing;
    coverage.expected_unique_count = None;
    let view = project_canonical(&coverage, MissingListView::default());
    match &view.expected {
        ExpectedStatusView::Unavailable { offer_validate, .. } => assert!(offer_validate),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[test]
fn a_stale_inventory_offers_re_validation_and_no_zero_percent() {
    let mut coverage = base_canonical();
    coverage.expected_inventory = ExpectedInventoryStatus::InventoryStale {
        reason: "revised since recorded".to_string(),
    };
    coverage.expected_unique_count = None;
    coverage.missing_count = None;
    coverage.completion_percent = None;
    let view = project_canonical(&coverage, MissingListView::default());
    assert!(!view.expected.is_available());
    assert_eq!(view.completion_percent, None);
    match &view.expected {
        ExpectedStatusView::Unavailable { offer_validate, .. } => assert!(offer_validate),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

// --- rendering ---------------------------------------------------------------

fn render(
    entries: &[SourceCoverageEntry],
    open: &mut Option<String>,
    missing_open: &mut BTreeSet<String>,
) -> (egui::FullOutput, Option<CoveragePanelRequest>) {
    let context = egui::Context::default();
    context.memory_mut(|memory| memory.set_everything_is_visible(true));
    let mut captured = None;
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            captured = show_coverage_section(ui, entries, open, missing_open);
        });
    });
    (output, captured)
}

fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

fn entry(load: CoverageLoad) -> SourceCoverageEntry {
    SourceCoverageEntry {
        source_id: "no-intro-gba".to_string(),
        source_label: "No-Intro - Game Boy Advance".to_string(),
        platform: Some("Game Boy Advance".to_string()),
        enabled: true,
        load,
    }
}

#[test]
fn a_complete_set_renders_a_positive_badge() {
    let mut coverage = base_canonical();
    coverage.complete_set = CompleteSetVerdict::Complete {
        extra_duplicate_archives: 7,
    };
    coverage.missing_count = Some(0);
    coverage.completion_percent = Some(100.0);
    let view = project_canonical(&coverage, MissingListView::default());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let (output, _) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    assert!(rendered_text_contains(&output, "Full set ✓"));
    assert!(rendered_text_contains(&output, "100.0% complete"));
}

#[test]
fn a_missing_count_and_completion_come_from_the_core_percentage() {
    let view = project_canonical(&base_canonical(), MissingListView::default());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let (output, _) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    // 96.62 from the core, not owned/expected (1201/1243 = 96.6%) or
    // verified/expected.
    assert!(rendered_text_contains(&output, "96.6% complete"));
    assert!(rendered_text_contains(&output, "Incomplete — 42 missing"));
    assert!(rendered_text_contains(&output, "42"));
}

#[test]
fn owned_over_expected_never_shows_over_one_hundred_percent() {
    let mut coverage = base_canonical();
    coverage.owned_applicable = 5000; // far more owned than expected
    coverage.completion_percent = Some(100.0); // core caps it
    coverage.missing_count = Some(0);
    coverage.complete_set = CompleteSetVerdict::Complete {
        extra_duplicate_archives: 3800,
    };
    let view = project_canonical(&coverage, MissingListView::default());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let (output, _) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    assert!(rendered_text_contains(&output, "100.0% complete"));
    assert!(!rendered_text_contains(&output, "105"));
    assert!(!rendered_text_contains(&output, "400.0%"));
}

#[test]
fn an_unassigned_source_still_shows_verification_metrics_but_dashes_for_expected() {
    let mut coverage = base_canonical();
    coverage.expected_inventory = ExpectedInventoryStatus::PlatformUnassigned;
    coverage.expected_unique_count = None;
    coverage.missing_count = None;
    coverage.completion_percent = None;
    coverage.complete_set = CompleteSetVerdict::NotProvable {
        reason: "no explicit platform assignment".to_string(),
    };
    let view = project_canonical(&coverage, MissingListView::default());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let (output, _) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    // verification metrics present
    assert!(rendered_text_contains(&output, "Owned"));
    assert!(rendered_text_contains(&output, "1,201"));
    assert!(rendered_text_contains(&output, "Verified"));
    // gated ones dashed, not zeroed
    assert!(rendered_text_contains(&output, "—"));
    assert!(rendered_text_contains(
        &output,
        "Full set cannot be determined"
    ));
    assert!(rendered_text_contains(
        &output,
        "Assign this catalogue to a platform"
    ));
    assert!(!rendered_text_contains(&output, "0% complete"));
    assert!(!rendered_text_contains(&output, "0 missing"));
}

#[test]
fn a_missing_inventory_shows_a_validate_action_and_returns_a_validate_request() {
    let mut coverage = base_canonical();
    coverage.expected_inventory = ExpectedInventoryStatus::InventoryMissing;
    coverage.expected_unique_count = None;
    coverage.missing_count = None;
    coverage.completion_percent = None;
    coverage.complete_set = CompleteSetVerdict::NotProvable {
        reason: "never validated".to_string(),
    };
    let view = project_canonical(&coverage, MissingListView::default());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let (output, _) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    assert!(rendered_text_contains(&output, "Validate catalogue"));
    assert!(rendered_text_contains(&output, "Validate this catalogue"));
}

#[test]
fn duplicate_names_inventory_shows_counts_but_full_set_not_provable() {
    let mut coverage = base_canonical();
    coverage.expected_inventory = ExpectedInventoryStatus::Available {
        entry_count: 1243,
        duplicate_names_skipped: 4,
    };
    coverage.missing_count = Some(0);
    coverage.completion_percent = Some(100.0);
    coverage.complete_set = CompleteSetVerdict::NotProvable {
        reason: "this DAT declared 4 duplicate <game> name(s)".to_string(),
    };
    let view = project_canonical(&coverage, MissingListView::default());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let (output, _) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    assert!(rendered_text_contains(&output, "1,243")); // expected still shown
    assert!(rendered_text_contains(
        &output,
        "Full set cannot be determined"
    ));
    assert!(!rendered_text_contains(&output, "Full set ✓"));
}

#[test]
fn the_missing_list_action_only_appears_when_missing_is_positive() {
    // missing == 0
    let mut coverage = base_canonical();
    coverage.missing_count = Some(0);
    coverage.completion_percent = Some(100.0);
    coverage.complete_set = CompleteSetVerdict::Complete {
        extra_duplicate_archives: 0,
    };
    let view = project_canonical(&coverage, MissingListView::default());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let (output, _) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    assert!(!rendered_text_contains(&output, "View missing games"));
}

#[test]
fn the_missing_list_is_bounded_and_pages() {
    let list = MissingListView {
        names: (0..MISSING_PAGE_SIZE)
            .map(|index| format!("Missing Game {index}"))
            .collect(),
        has_more: true,
    };
    assert_eq!(list.names.len(), MISSING_PAGE_SIZE as usize);
    let view = project_canonical(&base_canonical(), list);
    let mut missing_open = BTreeSet::new();
    missing_open.insert("no-intro-gba".to_string());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let (output, request) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut missing_open,
    );
    assert!(rendered_text_contains(&output, "Missing Game 0"));
    assert!(rendered_text_contains(&output, "Load more"));
    // No request unless a button was actually clicked this frame.
    assert!(request.is_none());
}

#[test]
fn arcade_uses_set_specific_labels_not_canonical_ones() {
    let view = project_arcade(&base_arcade());
    let entries = [SourceCoverageEntry {
        source_id: "mame".to_string(),
        source_label: "MAME".to_string(),
        platform: Some("Arcade".to_string()),
        enabled: true,
        load: CoverageLoad::Ready(CoverageUnitView::Arcade(Box::new(view))),
    }];
    let (output, _) = render(
        &entries,
        &mut Some("mame".to_string()),
        &mut BTreeSet::new(),
    );
    assert!(rendered_text_contains(&output, "Expected sets"));
    assert!(rendered_text_contains(&output, "Complete sets"));
    assert!(rendered_text_contains(&output, "Incomplete sets"));
    assert!(rendered_text_contains(&output, "Needs review"));
    assert!(rendered_text_contains(&output, "Missing sets"));
    assert!(rendered_text_contains(&output, "Arcade sets"));
    // Canonical vocabulary must not appear.
    assert!(!rendered_text_contains(&output, "Not in catalogue"));
}

#[test]
fn an_arcade_dependency_incomplete_set_is_never_shown_as_complete() {
    let mut coverage = base_arcade();
    // 40 checked, only 30 dependency-aware Complete; the other 10 include
    // incomplete/needs-review. Full set follows the core verdict.
    coverage.complete_set = CompleteSetVerdict::Incomplete { missing_count: 70 };
    let view = project_arcade(&coverage);
    assert_eq!(view.complete_sets, 30);
    assert_eq!(view.incomplete_sets, 8);
    assert_eq!(view.full_set, FullSetView::Incomplete { missing_count: 70 });
}

#[test]
fn opening_an_unloaded_source_returns_a_load_request_once() {
    let entries = [entry(CoverageLoad::NotOpened)];
    let mut open = None;
    let (_output, request) = render(&entries, &mut open, &mut BTreeSet::new());
    // Not opened yet, not clicked -> no request, no state change.
    assert_eq!(request, None);
    assert_eq!(open, None);

    // Now simulate it being open but still unloaded -> panel asks to load.
    let (_output, request) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    assert_eq!(
        request,
        Some(CoveragePanelRequest::Load {
            source_id: "no-intro-gba".to_string()
        })
    );
}

#[test]
fn a_failed_read_shows_an_error_and_never_a_fallback_number() {
    let entries = [entry(CoverageLoad::Failed(
        "database is locked".to_string(),
    ))];
    let (output, _) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    assert!(rendered_text_contains(
        &output,
        "Coverage could not be read"
    ));
    assert!(rendered_text_contains(&output, "database is locked"));
    assert!(!rendered_text_contains(&output, "% complete"));
}

#[test]
fn two_sources_render_as_two_separate_cards() {
    let a = SourceCoverageEntry {
        source_id: "no-intro-gba".to_string(),
        source_label: "No-Intro - GBA".to_string(),
        platform: Some("Game Boy Advance".to_string()),
        enabled: true,
        load: CoverageLoad::NotOpened,
    };
    let b = SourceCoverageEntry {
        source_id: "redump-psx".to_string(),
        source_label: "Redump - PlayStation".to_string(),
        platform: Some("PlayStation".to_string()),
        enabled: true,
        load: CoverageLoad::NotOpened,
    };
    let (output, _) = render(&[a, b], &mut None, &mut BTreeSet::new());
    assert!(rendered_text_contains(&output, "No-Intro - GBA"));
    assert!(rendered_text_contains(&output, "Redump - PlayStation"));
}

#[test]
fn a_narrow_viewport_render_does_not_panic() {
    let view = project_canonical(&base_canonical(), MissingListView::default());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let context = egui::Context::default();
    context.memory_mut(|memory| memory.set_everything_is_visible(true));
    let mut open = Some("no-intro-gba".to_string());
    let mut missing_open = BTreeSet::new();
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(260.0, 900.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let _ = show_coverage_section(ui, &entries, &mut open, &mut missing_open);
            });
        },
    );
}

#[test]
fn no_raw_enum_or_debug_text_reaches_the_user() {
    let mut coverage = base_canonical();
    coverage.expected_inventory = ExpectedInventoryStatus::PlatformMismatch {
        source_platform: "SNES".to_string(),
    };
    coverage.expected_unique_count = None;
    coverage.missing_count = None;
    coverage.completion_percent = None;
    coverage.complete_set = CompleteSetVerdict::NotProvable {
        reason: "assigned to SNES, not the requested platform".to_string(),
    };
    let view = project_canonical(&coverage, MissingListView::default());
    let entries = [entry(CoverageLoad::Ready(CoverageUnitView::Canonical(
        Box::new(view),
    )))];
    let (output, _) = render(
        &entries,
        &mut Some("no-intro-gba".to_string()),
        &mut BTreeSet::new(),
    );
    for forbidden in [
        "PlatformMismatch",
        "ExpectedInventoryStatus",
        "CompleteSetVerdict",
        "NotProvable",
        "Some(",
        "None,",
        "{ ",
    ] {
        assert!(
            !rendered_text_contains(&output, forbidden),
            "raw token {forbidden:?} leaked to the UI"
        );
    }
    assert!(rendered_text_contains(&output, "Assigned to SNES"));
}
