//! The page is rendered headlessly from a hand-built report, so no library,
//! folder or database is needed and nothing outside the test is read.

use super::*;
use archivefs_core::collection_intelligence::{
    ContentCheckSummary, InventorySummary, LinkCause, LinkGroup, LinkSummary, Measures,
    REPORT_SCHEMA_VERSION, TierCount,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn record(
    id: i64,
    set: RecordSet,
    tier: EvidenceTier,
    old: &str,
    target: Option<&str>,
) -> RecordFinding {
    RecordFinding {
        set,
        archive_id: id,
        old_path: PathBuf::from(old),
        platform: Some("Example System".into()),
        archive_kind: "direct_game_image".into(),
        size_bytes: Some(32_768),
        modified_unix_seconds: Some(800_000_000),
        first_seen_at: Some("2030-01-01T00:00:00Z".into()),
        last_seen_at: Some("2030-01-01T00:00:00Z".into()),
        flagged_missing_at: Some("2030-01-02T00:00:00Z".into()),
        tier,
        class: tier.class(),
        code: tier.code(),
        target: target.map(PathBuf::from),
        target_archive_id: None,
        evidence: vec!["one file has this name and size; its date is the same".into()],
    }
}

fn report() -> ReconciliationReport {
    let records = vec![
        record(
            1,
            RecordSet::FlaggedMissing,
            EvidenceTier::ConfirmedObject,
            "/library/carts/Example Cart (USA).zip",
            Some("/library/carts/Example Cart.zip"),
        ),
        record(
            2,
            RecordSet::FlaggedMissing,
            EvidenceTier::MetadataMatch,
            "/library/old-name/Example Game (World).rom",
            Some("/library/new-name/Example Game (World).rom"),
        ),
        record(
            3,
            RecordSet::AbsentNeverFlagged,
            EvidenceTier::NameOnlyLead,
            "/old/system-a/Shared Title (Japan).zip",
            Some("/library/system-b/Shared Title (Japan).zip"),
        ),
        record(
            4,
            RecordSet::Forgotten,
            EvidenceTier::NoTrace,
            "/library/system-c/Lost Game (USA).rom",
            None,
        ),
    ];
    let mut record_sets: BTreeMap<RecordSet, BTreeMap<&'static str, TierCount>> = BTreeMap::new();
    let mut by_class = BTreeMap::new();
    for record in &records {
        record_sets
            .entry(record.set)
            .or_default()
            .entry(record.code)
            .or_default()
            .records += 1;
        *by_class.entry(record.class).or_default() += 1;
    }
    ReconciliationReport {
        schema_version: REPORT_SCHEMA_VERSION,
        database_path: "/library.sqlite3".into(),
        generated_at_unix: 0,
        inventory: InventorySummary::default(),
        measures: Measures {
            rows_in_library: 1_200,
            rows_present_as_recorded: 1_000,
            rows_flagged_missing: 1_120,
            rows_absent_never_flagged: 1_040,
            records_removed_by_forget_missing: 1_030,
            records_examined: 4,
            records_tied_to_a_file: 2,
            distinct_files_reached: 2,
            ..Measures::default()
        },
        record_sets,
        by_class,
        receipts: Vec::new(),
        content_check: ContentCheckSummary::default(),
        links: LinkSummary {
            total: 3_300,
            resolve: 2_200,
            broken: 1_100,
            target_identified: 1_060,
            plausible_candidate: 1_030,
            unresolved: 10,
            storage_unavailable: 1_085,
            target_missing: 15,
            groups: vec![LinkGroup {
                link_folder: "/library/collection".into(),
                target_root: "/storage/incoming/staging".into(),
                grade: LinkGrade::TargetIdentified,
                cause: Some(LinkCause::StorageEmpty),
                links: 1_055,
                example_link: "/library/collection/Game.img".into(),
                example_target: "/storage/incoming/staging/Collection/Game.img".into(),
            }],
        },
        records,
        link_findings: vec![LinkFinding {
            link: "/library/collection/Game.img".into(),
            stored_target: "/storage/incoming/staging/Collection/Game.img".into(),
            resolved_target: "/storage/incoming/staging/Collection/Game.img".into(),
            grade: LinkGrade::TargetIdentified,
            cause: Some(LinkCause::StorageEmpty),
            deepest_existing_ancestor: None,
            found: Some("/storage/Collection/Game.img".into()),
            found_inode: Some(7),
            evidence: vec!["that file was created before the link was made".into()],
        }],
        notes: vec!["file contents were not read".into()],
    }
}

fn render(state: &mut State) -> String {
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 1600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, state));
        },
    );
    fn text(shape: &egui::epaint::Shape, output: &mut String) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                output.push_str(text.galley.text());
                output.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|shape| text(shape, output)),
            _ => {}
        }
    }
    let mut visible = String::new();
    for clipped in &output.shapes {
        text(&clipped.shape, &mut visible);
    }
    visible
}

#[test]
fn before_a_check_the_page_says_what_it_will_do_and_starts_nothing() {
    let mut state = State::default();
    let visible = render(&mut state);
    assert!(
        visible.contains("It only looks: nothing is changed."),
        "{visible}"
    );
    assert!(visible.contains("Check now"));
    assert!(visible.contains("Not checked yet"));
    assert!(
        !state.busy(),
        "opening the page must not start listing the disks"
    );
}

#[test]
fn overview_keeps_the_counts_apart_and_says_what_a_metadata_match_does_not_show() {
    let mut state = State::with_report(report());
    let visible = render(&mut state);
    assert!(visible.contains("These are separate counts"), "{visible}");
    assert!(visible.contains("1,200") && visible.contains("1,120") && visible.contains("1,040"));
    assert!(
        visible.contains("1,030"),
        "forgotten entries are shown on their own"
    );
    for level in [
        "Confirmed move",
        "Strong match",
        "Metadata match",
        "Weak match",
        "Stale missing mark",
        "Possible match",
        "Unresolved",
    ] {
        assert!(visible.contains(level), "missing level {level}: {visible}");
    }
    assert!(visible.contains("does not show that two files hold the same data"));
    assert!(visible.contains("Links are not library entries"));
    assert!(visible.contains("3,300") && visible.contains("1,060"));
    // A page that only looks offers nothing that changes anything.
    for forbidden in ["Repair", "Delete", "Restore", "Undo", "Relink", "Forget"] {
        let offered = visible
            .lines()
            .any(|line| line.trim() == forbidden || line.starts_with(&format!("{forbidden} ")));
        assert!(!offered, "the page must not offer {forbidden}");
    }
}

#[test]
fn the_review_list_starts_with_what_needs_a_person_and_can_be_searched() {
    let mut state = State::with_report(report());
    state.tab = Tab::Review;
    let visible = render(&mut state);
    // Confirmed and metadata matches are not in the default list; leads and no-trace are.
    assert_eq!(state.filtered.len(), 2);
    assert!(
        visible.contains("Shared Title (Japan).zip") && visible.contains("Lost Game (USA).rom"),
        "{visible}"
    );
    assert!(!visible.contains("Example Cart (USA).zip"));
    state.show = Show::All;
    state.query = "EXAMPLE old-name".into();
    render(&mut state);
    assert_eq!(state.filtered.len(), 1);
    state.query.clear();
    state.set = Some(RecordSet::Forgotten);
    render(&mut state);
    assert_eq!(state.filtered.len(), 1);
    // Selecting the name-only lead shows it as a possible match, never as the file.
    state.set = None;
    state.show = Show::Only(EvidenceClass::Candidate);
    render(&mut state);
    state.selected = Some(state.filtered[0]);
    let visible = render(&mut state);
    assert!(
        visible.contains("Possible match (not accepted)"),
        "{visible}"
    );
    assert!(!visible.contains("The file now\n/library/system-b"));
}

#[test]
fn the_links_tab_groups_links_by_cause_and_shows_the_found_target() {
    let mut state = State::with_report(report());
    state.tab = Tab::Links;
    state.selected_link = Some(0);
    let visible = render(&mut state);
    assert!(visible.contains("1,055"), "{visible}");
    assert!(visible.contains("the folder it points into is empty"));
    // Working links, broken links and storage that is not there are told apart.
    assert!(
        visible.contains("Working") && visible.contains("2,200"),
        "{visible}"
    );
    assert!(
        visible.contains("the storage they point at is not there") && visible.contains("1,085")
    );
    assert!(visible.contains("the file is gone from a folder that is there"));
    assert!(visible.contains("never as games"));
    assert!(visible.contains("Its target was probably moved to"));
    assert!(visible.contains("/storage/Collection/Game.img"));
}

#[test]
fn a_library_that_cannot_be_read_is_reported_plainly() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = State::for_database(temp.path().join("no-library.sqlite3"));
    let ctx = egui::Context::default();
    state.start(&ctx);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while state.busy() && std::time::Instant::now() < deadline {
        state.poll();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!state.busy());
    let visible = render(&mut state);
    assert!(visible.contains("Could not complete"), "{visible}");
    assert!(
        !temp.path().join("no-library.sqlite3").exists(),
        "the check must not create a library"
    );
}
