//! End-to-end checks that the consolidated pieces agree with each other:
//! hardened parser -> canonical projection -> reconciliation -> saved review ->
//! applicability -> launch gate. Each earlier layer was built and tested on its
//! own; these tests exist so they cannot silently drift apart.

use std::collections::BTreeMap;

use super::*;
use crate::launch::{ApplicabilityVerdict, CheatCandidate, applicability_verdict};

const GAME: &str = "verified:GAME01";

fn project(file: &str, text: &str) -> Vec<CheatReconciliationEntry> {
    parse_cht_text(text).unwrap().reconciliation_entries(
        GAME,
        true,
        CheatPlatform::GameCube,
        "pack",
        file,
    )
}

fn reconcile(entries: Vec<CheatReconciliationEntry>) -> CheatReconciliationResult {
    match reconcile_cheats_for_game(entries) {
        CheatReconciliationOutcome::Ready(result) => result,
        other => panic!("{other:?}"),
    }
}

fn request() -> ResolvedCheatPlanRequest {
    ResolvedCheatPlanRequest {
        source_report_digest: "digest".into(),
        emulator: "RetroArch".into(),
        profile: "test".into(),
        target_file: None,
        existing_file_digest: None,
        destination_changed: false,
    }
}

const CHT_A: &str = "cheats = 1\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\n";
const CHT_B: &str = "cheats = 1\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000BB00\"\n";

/// A decodable (Gecko Write8) entry carrying the canonical typed provenance, so
/// the apply-plan stage can actually select it.
fn decodable(name: &str, file: &str, value: u8) -> CheatReconciliationEntry {
    let record = CheatRecordProvenance::local_with_sha256(
        std::path::Path::new(file),
        &"ab".repeat(32),
        "gecko",
    );
    CheatReconciliationEntry {
        game_identity: GAME.into(),
        identity_verified: true,
        applicability: Default::default(),
        source_path: Some(file.into()),
        source_index: Some(0),
        source_fields: vec![],
        title: name.into(),
        source: file.into(),
        source_format: CheatSourceFormat::Gecko,
        document: CheatDocument {
            source_evidence: vec![record],
            title: name.into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::Gecko,
            operations: vec![CheatOperation::Write8 {
                address: 100,
                value,
            }],
            issues: vec![],
            provenance: vec![file.into()],
        },
        raw_code: None,
        provenance: vec![file.into()],
    }
}

#[test]
fn retroarch_conflicts_are_detected_and_opaque_codes_never_enter_an_apply_plan() {
    let mut entries = project("a.cht", CHT_A);
    entries.extend(project("b.cht", CHT_B));
    let result = reconcile(entries);
    assert_eq!(result.groups.len(), 1, "one logical cheat, two variants");
    assert!(CheatCandidate::requires_choice(&result.groups[0]));
    assert!(result.auto_winner.is_none());
    // Even with an explicit choice, an opaque engine code is reported as
    // unsupported for the generic apply plan - never silently dropped and
    // never auto-applied. RetroArch installs use their own selection path.
    let choices = BTreeMap::from([(0usize, CheatReviewChoice::KeepA)]);
    let plan = resolve_reviewed_cheat_plan(&result, &choices, &request());
    assert!(plan.selected_entries.is_empty());
    assert!(!plan.unsupported_entries.is_empty());
}

#[test]
fn conflicting_variants_stay_distinct_until_a_choice_is_made() {
    let result = reconcile(vec![
        decodable("Infinite Lives", "a.gct", 1),
        decodable("Infinite Lives", "b.gct", 2),
    ]);
    assert!(CheatCandidate::requires_choice(&result.groups[0]));
    assert!(result.auto_winner.is_none());

    // No saved choice: nothing is selected for anyone.
    let none = resolve_reviewed_cheat_plan(&result, &BTreeMap::new(), &request());
    assert!(none.selected_entries.is_empty());
    assert_eq!(none.unresolved_conflicts.len(), 1);

    // An explicit choice selects exactly one variant and keeps its provenance.
    let choices = BTreeMap::from([(0usize, CheatReviewChoice::KeepA)]);
    let chosen = resolve_reviewed_cheat_plan(&result, &choices, &request());
    assert_eq!(chosen.selected_entries.len(), 1);
    let evidence = &chosen.selected_entries[0].document.source_evidence;
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].display_filename(), Some("a.gct"));
}

#[test]
fn identical_duplicates_dedupe_but_keep_every_source() {
    let result = reconcile(vec![
        decodable("Infinite Lives", "a.gct", 1),
        decodable("Infinite Lives", "copy/a.gct", 1),
    ]);
    assert_eq!(result.groups.len(), 1);
    assert!(!CheatCandidate::requires_choice(&result.groups[0]));
    let plan = resolve_reviewed_cheat_plan(&result, &BTreeMap::new(), &request());
    assert_eq!(plan.selected_entries.len(), 1);
    assert_eq!(plan.selected_entries[0].duplicate_entry_indices.len(), 2);
    let provenance = plan.selected_entries[0].provenance.join("|");
    assert!(
        provenance.contains("a.gct") && provenance.contains("copy/a.gct"),
        "{provenance}"
    );
}

#[test]
fn reconciliation_report_round_trips_with_provenance_and_conflicts_intact() {
    let mut entries = project("a.cht", CHT_A);
    entries.extend(project("b.cht", CHT_B));
    let result = reconcile(entries);
    let json = serde_json::to_string(&result).unwrap();
    let restored: CheatReconciliationResult = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, result);
    assert!(
        restored
            .entries
            .iter()
            .all(|entry| !entry.document.source_evidence.is_empty()
                && !entry.source_fields.is_empty())
    );
}

#[test]
fn hostile_input_is_bounded_and_never_panics() {
    // Sparse, enormous index: one entry, no allocation driven by the number.
    let document =
        parse_cht_text("cheats = 4294967295\ncheat4294967295_desc = X\ncheat4294967295_code = A\n")
            .unwrap();
    assert_eq!(document.entries.len(), 1);
    assert_eq!(document.entries[0].index, u32::MAX);

    // More distinct entries than the bound: capped, with an explicit warning.
    let many: String = (0..(MAX_CHT_ENTRIES + 50))
        .map(|index| format!("cheat{index}_code = A\n"))
        .collect();
    let capped = parse_cht_text(&many).unwrap();
    assert_eq!(capped.entries.len(), MAX_CHT_ENTRIES);
    assert!(
        capped
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::LimitReached)
    );

    // Control bytes and an unterminated quote make an entry unselectable.
    for bad in [
        "cheat0_code = \"AB\u{0}CD\"\n",
        "cheat0_code = \"unfinished\n",
    ] {
        let document = parse_cht_text(bad).unwrap();
        assert!(!document.entries[0].is_selectable(), "{bad:?}");
    }
}

fn applicability(
    game: CheatSelectedGame,
    association: CheatGameAssociation,
) -> CheatApplicabilityReport {
    let parsed = parse_cht_text(CHT_A).unwrap();
    let entry = parsed.entries[0].clone();
    let projected = project("a.cht", CHT_A).remove(0);
    let input = CheatApplicabilityInput {
        game,
        association,
        document: projected.document,
        parsing: CheatParseEvidence::from_cht_entry(&entry),
        native_cht: Some(entry),
        route: None,
        reconciliation: None,
    };
    assess_cheat_applicability(&input)
}

#[test]
fn similar_titles_or_bare_claims_never_make_a_cheat_launchable() {
    let projected = project("a.cht", CHT_A).remove(0);
    let association = CheatGameAssociation::from_entry(&projected);

    // Nothing is known about the selected game.
    let unknown = applicability(CheatSelectedGame::default(), association.clone());
    assert_ne!(
        applicability_verdict(&unknown),
        ApplicabilityVerdict::Allowed,
        "{unknown:?}"
    );

    // The title merely looks the same.
    let titled = CheatSelectedGame {
        title: Some("Infinite Lives Adventure".into()),
        ..CheatSelectedGame::default()
    };
    let mut claimed = association;
    claimed.title = Some("Infinite Lives Adventure".into());
    let state = applicability(titled, claimed);
    assert_ne!(
        applicability_verdict(&state),
        ApplicabilityVerdict::Allowed,
        "{state:?}"
    );
}
