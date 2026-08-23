//! Tests for the Library View History page.
//!
//! Every fixture is a fresh per-test temp directory holding disposable
//! `.json` history record files, written directly (there is no public way to
//! reach the core crate's own `write_record_atomically` from here - it is
//! private to `archivefs_core::library_view_history`). Any `.json` file in
//! the directory is a valid fixture: `list_library_view_history_at` only
//! requires the `.json` extension and sorts by filename, so the exact name
//! never matters for these tests.

use std::path::{Path, PathBuf};

use archivefs_core::{FrontendProfileKind, LibraryViewHistoryOperation, LibraryViewHistoryRecord};

use super::*;

/// A per-test temp directory under the system temp dir, removed on drop.
struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "archivefs-gui-library-view-history-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds a record with every field explicit, so each test only overrides
/// what it actually cares about.
fn sample_record(
    operation: LibraryViewHistoryOperation,
    view_name: &str,
    created: usize,
    failed: usize,
    skipped_or_collision: Option<usize>,
    warnings: Vec<String>,
) -> LibraryViewHistoryRecord {
    LibraryViewHistoryRecord {
        schema_version: 1,
        timestamp: "2026-08-23T09:00:00Z".to_string(),
        operation,
        view_id: "view-1".to_string(),
        view_name: view_name.to_string(),
        profile_kind: FrontendProfileKind::Romm,
        destination_root: "/data/library-views/view-1/dest".to_string(),
        manifest_path: "/data/library-views/view-1.manifest.json".to_string(),
        planned_count: created + failed,
        created,
        repaired: 0,
        removed: 0,
        unchanged: 0,
        failed,
        skipped_or_collision,
        success: failed == 0,
        warnings,
    }
}

/// Writes `record` as a fresh `.json` file under `dir`, named uniquely so
/// several writes in one test never collide.
fn write_fixture_record(dir: &Path, name: &str, record: &LibraryViewHistoryRecord) {
    let bytes = serde_json::to_vec_pretty(record).expect("record serializes");
    std::fs::write(dir.join(format!("{name}.json")), bytes).expect("fixture write");
}

fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

fn render(state: &mut LibraryViewHistoryPageState) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_library_view_history_page(ui, state);
        });
    })
}

// --- valid Apply record renders ---------------------------------------------

#[test]
fn valid_apply_record_renders() {
    let dir = TestDir::new("apply");
    let record = sample_record(
        LibraryViewHistoryOperation::Apply,
        "My NES View",
        3,
        0,
        Some(1),
        Vec::new(),
    );
    write_fixture_record(dir.path(), "apply", &record);

    let mut state = LibraryViewHistoryPageState::load_with_history_dir(dir.path().to_path_buf());
    assert_eq!(state.entries.len(), 1);

    let output = render(&mut state);
    assert!(rendered_text_contains(&output, "Apply"));
    assert!(rendered_text_contains(&output, "My NES View"));
    assert!(rendered_text_contains(&output, "Succeeded"));
    assert!(rendered_text_contains(&output, "RomM profile"));
}

// --- valid Remove record renders ---------------------------------------------

#[test]
fn valid_remove_record_renders() {
    let dir = TestDir::new("remove");
    let record = sample_record(
        LibraryViewHistoryOperation::Remove,
        "Old SNES View",
        0,
        0,
        None,
        Vec::new(),
    );
    write_fixture_record(dir.path(), "remove", &record);

    let mut state = LibraryViewHistoryPageState::load_with_history_dir(dir.path().to_path_buf());
    assert_eq!(state.entries.len(), 1);

    let output = render(&mut state);
    assert!(rendered_text_contains(&output, "Remove"));
    assert!(rendered_text_contains(&output, "Old SNES View"));
    assert!(rendered_text_contains(&output, "Succeeded"));
}

// --- counts render correctly -------------------------------------------------

#[test]
fn counts_render_correctly_including_skipped_or_collision() {
    let dir = TestDir::new("counts");
    let mut record = sample_record(
        LibraryViewHistoryOperation::Apply,
        "Counted View",
        4,
        2,
        Some(3),
        Vec::new(),
    );
    record.repaired = 1;
    record.removed = 5;
    record.unchanged = 6;
    write_fixture_record(dir.path(), "counts", &record);

    let mut state = LibraryViewHistoryPageState::load_with_history_dir(dir.path().to_path_buf());
    let output = render(&mut state);
    assert!(rendered_text_contains(
        &output,
        "Created 4 · Repaired 1 · Removed 5 · Unchanged 6 · Failed 2 · Skipped/collision 3"
    ));
    // failed > 0, so this record is a failure, not a success.
    assert!(rendered_text_contains(&output, "Failed"));
}

// --- malformed record does not hide valid records ---------------------------

#[test]
fn malformed_record_does_not_hide_valid_records() {
    let dir = TestDir::new("malformed-tolerant");
    let record = sample_record(
        LibraryViewHistoryOperation::Apply,
        "Still Readable View",
        1,
        0,
        None,
        Vec::new(),
    );
    write_fixture_record(dir.path(), "valid", &record);
    std::fs::write(dir.path().join("0-corrupt.json"), b"{ not valid json").unwrap();

    let mut state = LibraryViewHistoryPageState::load_with_history_dir(dir.path().to_path_buf());
    assert_eq!(state.entries.len(), 2);
    let valid_count = state
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                archivefs_core::LibraryViewHistoryEntry::Record { .. }
            )
        })
        .count();
    let malformed_count = state
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                archivefs_core::LibraryViewHistoryEntry::Malformed { .. }
            )
        })
        .count();
    assert_eq!(valid_count, 1);
    assert_eq!(malformed_count, 1);

    let output = render(&mut state);
    assert!(
        rendered_text_contains(&output, "Still Readable View"),
        "the valid record must still render"
    );
    assert!(
        rendered_text_contains(&output, "A history record could not be read"),
        "the malformed record must be surfaced honestly, not silently dropped"
    );
}

// --- empty state renders -----------------------------------------------------

#[test]
fn empty_history_shows_a_clear_message() {
    let dir = TestDir::new("empty");
    let mut state = LibraryViewHistoryPageState::load_with_history_dir(dir.path().to_path_buf());
    assert!(state.entries.is_empty());

    let output = render(&mut state);
    assert!(rendered_text_contains(
        &output,
        "No Library View operations recorded yet"
    ));
}

#[test]
fn empty_history_shows_a_clear_message_when_the_directory_does_not_exist_yet() {
    let dir = TestDir::new("missing-dir");
    let missing = dir.path().join("does-not-exist");
    let mut state = LibraryViewHistoryPageState::load_with_history_dir(missing);
    assert!(state.entries.is_empty());

    let output = render(&mut state);
    assert!(rendered_text_contains(
        &output,
        "No Library View operations recorded yet"
    ));
}

// --- reachable from existing History & Logs navigation ----------------------

#[test]
fn library_view_history_is_reachable_from_the_history_and_journals_nav_group() {
    let group = crate::navigation::ADVANCED_NAV_GROUPS
        .iter()
        .find(|group| group.heading == Some("HISTORY & JOURNALS"))
        .expect("the History & Journals group must still exist");
    let has_entry = group.entries.iter().any(|entry| {
        matches!(
            entry.click,
            crate::navigation::NavClick::View(crate::MainView::LibraryViewHistory)
        )
    });
    assert!(
        has_entry,
        "Library View History must be reachable from the same group as History & Logs / \
         Repair History"
    );
    let has_history_logs = group.entries.iter().any(|entry| {
        matches!(
            entry.click,
            crate::navigation::NavClick::View(crate::MainView::HistoryLogs)
        )
    });
    assert!(
        has_history_logs,
        "the existing History & Logs entry must remain in the same group"
    );
}
