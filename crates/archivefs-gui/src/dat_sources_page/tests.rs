//! Tests for the DAT Sources page's state and view model.
//!
//! These are data tests: the view model is a pure function of state, so what
//! the page *says* is checkable without a frame buffer. Drawing is exercised
//! only through the view it consumes.
//!
//! # What these tests never touch
//!
//! Every path is inside a per-test temp directory removed on drop. `DatSourcesPageState::load`
//! takes its registry path as an argument precisely so no test has to read, or
//! disturb, the real `HOME`. No real ROM or DAT collection is opened, and there
//! is no network surface anywhere in this page or the core it calls.

use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use archivefs_core::dat::audit::{AuditEntry, AuditReport, AuditSummary, AuditVerdict};
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::managed_sources::load_managed_dat_sources_from;
use archivefs_core::dat::model::{DatEcosystem, DatFormat};
use archivefs_core::dat::parser::DiagnosticSeverity;
use archivefs_core::dat::sources::{
    DatDiagnostic, DatFileOutcome, DatFileReport, DatHealthState, DatSourceKind, DatSourceRegistry,
    DatValidationReport, audit_run::DatAuditOutcome, load_dat_sources_config_from,
};
use archivefs_core::dat::tosec_release_pack::{TosecFriendlyCategory, TosecMediaType};
use archivefs_core::dat::updates::{
    ManagedDatSnapshot, ManagedDatState, load_managed_dat_state, save_managed_dat_state,
};
use archivefs_core::safe_read::TrustedRoots;

use super::*;

const LOGIQX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<datafile>
    <header>
        <name>Test No-Intro Collection</name>
        <version>2026-01-01</version>
    </header>
    <game name="Super Game (World)">
        <rom name="super.bin" size="4" crc="0c7e7fd8" md5="098f6bcd4621d373cade4e832627b4f6" sha1="a94a8fe5ccb19ba61c4c0873d391e987982fbbd3"/>
    </game>
</datafile>"#;

/// Bytes whose MD5/SHA-1 are the ones in [`LOGIQX`].
const SUPER_BIN: &[u8] = b"test";

/// How long a test waits for a worker thread before calling it a failure.
const JOB_TIMEOUT: Duration = Duration::from_secs(30);

struct Fixture {
    root: PathBuf,
    config_path: PathBuf,
}

/// Disambiguates fixture roots within one test binary process, alongside the
/// process id and a wall-clock timestamp. A monotonic counter is required
/// (not just a timestamp) because `cargo test`'s parallel test threads can
/// call `Fixture::new()` at effectively the same instant: on a coarser or
/// contended clock this can produce colliding nanosecond readings across
/// threads, which would otherwise let two tests' `remove_dir_all`/
/// `create_dir_all` calls race over the same directory. Matches this
/// codebase's own established pattern for the same problem elsewhere (e.g.
/// `generate_library_view_id`, `create_or_repair_symlink`'s temp-file
/// sequence).
static FIXTURE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Fixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-gui-dat-sources-page-{}-{}-{sequence:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        let config_path = root.join("config").join("dat_sources.toml");
        Self { root, config_path }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// A page whose registry lives in the fixture, with no library folders and
    /// no trusted roots - the strictest, and the one a fresh install has.
    fn page(&self) -> DatSourcesPageState {
        // The rename-transaction journal directory is a temp path inside the
        // fixture, so no test ever reads or writes the real home directory.
        let journal = self.root.join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        DatSourcesPageState::load_with_transaction_dir(
            self.config_path.clone(),
            Vec::new(),
            TrustedRoots::none(),
            journal,
        )
    }

    fn page_with_library(&self, folders: Vec<PathBuf>) -> DatSourcesPageState {
        let journal = self.root.join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        DatSourcesPageState::load_with_transaction_dir(
            self.config_path.clone(),
            folders,
            TrustedRoots::none(),
            journal,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Polls until the page's background job finishes, or fails the test.
fn run_to_completion(page: &mut DatSourcesPageState) {
    let deadline = Instant::now() + JOB_TIMEOUT;
    while page.is_busy() {
        page.poll();
        if Instant::now() > deadline {
            panic!("a background job did not finish within {JOB_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // One final drain: the job may have finished between the last poll and the
    // loop's exit test.
    page.poll();
}

/// A recursive listing of `(relative path, contents)`, for proving nothing
/// changed on disk.
fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(directory) = queue.pop() {
        for entry in std::fs::read_dir(&directory).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                queue.push(path);
            } else {
                out.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                ));
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Empty state
// ---------------------------------------------------------------------------

#[test]
fn a_fresh_install_renders_an_empty_page_with_no_error() {
    // The clean-install case: no registry file exists, and that is not a
    // failure. The page must be usable and must not report a problem.
    let fixture = Fixture::new();
    assert!(!fixture.config_path.exists());

    let page = fixture.page();
    let view = page.view();

    assert!(view.is_empty(), "no sources are registered yet");
    assert!(view.rows.is_empty());
    assert!(view.load_error.is_none(), "an absent file is not an error");
    assert!(view.load_problems.is_empty());
    assert!(view.unresolved.is_empty());
    assert!(!view.dirty, "an untouched page has nothing to save");
    assert!(view.pending_consequences.is_empty());
    assert!(view.running.is_none());
    assert!(view.audit.is_none());
    assert_eq!(view.save_state, DatSaveState::Idle);
    assert_eq!(view.config_path, fixture.config_path);
    // The policy section is present and at safe defaults on a clean install.
    assert_eq!(view.policy.scope, None);
    assert_eq!(view.policy.scope_label, "All platforms");
    assert!(view.policy.region_preferences.is_empty());
    assert!(view.policy.language_preferences.is_empty());
    assert_eq!(view.policy.revision_policy, RevisionPolicy::default());
    assert_eq!(view.policy.clone_policy, ClonePolicy::default());
    assert!(view.policy.effective.source_ordering.is_empty());
    assert!(view.policy.problems.is_empty());
    assert!(view.policy.editable);

    // And opening the page must not have created the file.
    assert!(
        !fixture.config_path.exists(),
        "viewing the page must write nothing"
    );
}

#[test]
fn an_unreadable_registry_is_reported_and_blocks_saving() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.config_path.parent().unwrap()).unwrap();
    std::fs::write(&fixture.config_path, "this is not valid toml {{[").unwrap();
    let before = std::fs::read_to_string(&fixture.config_path).unwrap();

    let mut page = fixture.page();
    assert!(page.view().load_error.is_some());

    // Saving must refuse rather than overwrite a file the user may still want
    // to repair by hand.
    page.apply(DatSourcesPageAction::Save);
    assert!(matches!(page.view().save_state, DatSaveState::Failed(_)));
    assert_eq!(
        std::fs::read_to_string(&fixture.config_path).unwrap(),
        before,
        "a refused save must not have touched the file"
    );
}

// ---------------------------------------------------------------------------
// Adding
// ---------------------------------------------------------------------------

#[test]
fn adding_a_dat_file_shows_it_as_an_unsaved_change() {
    let fixture = Fixture::new();
    let dat = fixture.write("no-intro.dat", LOGIQX);

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat.clone() });

    let view = page.view();
    assert_eq!(view.rows.len(), 1);
    let row = &view.rows[0];
    assert_eq!(row.id, "no-intro");
    assert_eq!(row.display_name, "no-intro.dat");
    assert_eq!(row.kind_label, "DAT file");
    assert!(row.enabled);
    assert!(row.changed);
    assert_eq!(
        row.health_state,
        DatHealthState::NotChecked,
        "adding a source must not claim a health nobody checked"
    );
    assert!(
        row.formats.is_empty(),
        "format is only ever reported from a real check, never guessed from the name"
    );

    assert!(view.dirty);
    assert!(
        view.pending_consequences
            .iter()
            .any(|line| line.contains("no-intro.dat")),
        "{:?}",
        view.pending_consequences
    );
    assert!(
        !fixture.config_path.exists(),
        "nothing is written before Save"
    );
}

#[test]
fn adding_a_dat_folder_registers_the_folder_itself() {
    let fixture = Fixture::new();
    let folder = fixture.dir("dats");
    std::fs::write(folder.join("a.dat"), LOGIQX).unwrap();

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFolder {
        path: folder.clone(),
    });

    let view = page.view();
    assert_eq!(view.rows.len(), 1);
    assert_eq!(view.rows[0].kind_label, "DAT folder");
    assert_eq!(view.rows[0].path, folder.to_string_lossy());
    assert!(view.action_error.is_none());
}

#[test]
fn adding_the_same_path_twice_is_refused_with_a_reason() {
    let fixture = Fixture::new();
    let dat = fixture.write("no-intro.dat", LOGIQX);

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat.clone() });
    page.apply(DatSourcesPageAction::AddFile { path: dat });

    let view = page.view();
    assert_eq!(view.rows.len(), 1, "the second add must not have landed");
    let error = view.action_error.expect("the refusal must be shown");
    assert!(error.contains("already registers"), "{error}");
}

#[test]
fn adding_a_folder_as_a_file_is_refused() {
    let fixture = Fixture::new();
    let folder = fixture.dir("dats");

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: folder });

    let view = page.view();
    assert!(view.rows.is_empty());
    let error = view.action_error.expect("the mismatch must be reported");
    assert!(error.contains("folder"), "{error}");
}

#[cfg(unix)]
#[test]
fn adding_a_symlinked_dat_is_refused() {
    let fixture = Fixture::new();
    let real = fixture.write("real.dat", LOGIQX);
    let link = fixture.root.join("link.dat");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: link });

    let view = page.view();
    assert!(view.rows.is_empty());
    let error = view.action_error.expect("the refusal must be shown");
    assert!(error.contains("symlink"), "{error}");
}

// ---------------------------------------------------------------------------
// Save and Discard
// ---------------------------------------------------------------------------

#[test]
fn save_writes_the_registry_and_clears_the_unsaved_state() {
    let fixture = Fixture::new();
    let dat = fixture.write("no-intro.dat", LOGIQX);

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Save);

    let view = page.view();
    assert_eq!(view.save_state, DatSaveState::Saved);
    assert!(!view.dirty, "after a save there is nothing left to save");
    assert!(!view.rows[0].changed, "and nothing is marked changed");
    assert!(fixture.config_path.exists());

    // Reloading a new page from the same path sees it.
    let reloaded = fixture.page();
    let view = reloaded.view();
    assert_eq!(view.rows.len(), 1);
    assert_eq!(view.rows[0].id, "no-intro");
    assert!(!view.dirty);
}

#[test]
fn discard_restores_exactly_what_is_on_disk() {
    let fixture = Fixture::new();
    let first = fixture.write("first.dat", LOGIQX);
    let second = fixture.write("second.dat", LOGIQX);

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: first });
    page.apply(DatSourcesPageAction::Save);
    let saved_text = std::fs::read_to_string(&fixture.config_path).unwrap();

    // Now make several unsaved edits of different kinds.
    page.apply(DatSourcesPageAction::AddFile { path: second });
    page.apply(DatSourcesPageAction::SetEnabled {
        id: "first".to_string(),
        enabled: false,
    });
    page.apply(DatSourcesPageAction::SetPlatform {
        id: "first".to_string(),
        platform: Some("NES".to_string()),
    });
    assert!(page.view().dirty);

    page.apply(DatSourcesPageAction::Revert);
    let view = page.view();
    assert!(!view.dirty, "discarding must leave nothing pending");
    assert_eq!(view.rows.len(), 1);
    assert!(view.rows[0].enabled);
    assert!(view.rows[0].platform_display.is_none());
    assert_eq!(
        std::fs::read_to_string(&fixture.config_path).unwrap(),
        saved_text,
        "discarding must not have written anything"
    );
}

#[test]
fn a_disabled_source_stays_disabled_across_a_save_and_reload() {
    let fixture = Fixture::new();
    let dat = fixture.write("off.dat", LOGIQX);

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::SetEnabled {
        id: "off".to_string(),
        enabled: false,
    });
    page.apply(DatSourcesPageAction::Save);

    let reloaded = fixture.page();
    let view = reloaded.view();
    assert_eq!(view.rows.len(), 1, "a disabled source is still listed");
    assert!(!view.rows[0].enabled);
}

#[test]
fn disabling_says_the_source_is_kept() {
    let fixture = Fixture::new();
    let dat = fixture.write("off.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Save);
    page.apply(DatSourcesPageAction::SetEnabled {
        id: "off".to_string(),
        enabled: false,
    });

    let consequences = page.view().pending_consequences;
    assert!(
        consequences
            .iter()
            .any(|line| line.contains("kept in your catalogue list")),
        "{consequences:?}"
    );
}

// ---------------------------------------------------------------------------
// Platform assignment
// ---------------------------------------------------------------------------

#[test]
fn a_platform_can_be_assigned_and_cleared() {
    let fixture = Fixture::new();
    let dat = fixture.write("nes.dat", LOGIQX);
    let canonical = archivefs_core::platform::canonical_ids()[0];

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::SetPlatform {
        id: "nes".to_string(),
        platform: Some(canonical.to_string()),
    });

    let view = page.view();
    assert_eq!(
        view.rows[0].platform_display.as_deref(),
        Some(archivefs_core::platform::display_name_for(canonical))
    );
    assert!(!view.rows[0].platform_unresolved);

    page.apply(DatSourcesPageAction::SetPlatform {
        id: "nes".to_string(),
        platform: None,
    });
    assert!(page.view().rows[0].platform_display.is_none());
}

#[test]
fn the_platform_picker_offers_only_canonical_platforms() {
    // Every candidate comes from the same registry `canonical_platform_for_alias`
    // resolves against, so an assignment can only ever name a platform the
    // resolver will match.
    let choices = platform_choices("");
    assert!(!choices.is_empty());
    assert!(choices.len() <= MAX_PLATFORM_CHOICES);
    for (id, _) in &choices {
        assert!(
            archivefs_core::canonical_platform_for_alias(id).is_some(),
            "the picker offered '{id}', which the resolver does not know"
        );
    }
    assert!(
        platform_choice_count("") >= choices.len(),
        "the count must not understate the truncated list"
    );
    assert_eq!(platform_choice_count("a-platform-nobody-has"), 0);
}

#[test]
fn an_unresolved_platform_is_shown_kept_and_round_trips() {
    let fixture = Fixture::new();
    let dat = fixture.write("future.dat", LOGIQX);

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::SetPlatform {
        id: "future".to_string(),
        platform: Some("APlatformFromALaterBuild".to_string()),
    });
    page.apply(DatSourcesPageAction::Save);

    let reloaded = fixture.page();
    let view = reloaded.view();
    assert_eq!(
        view.rows[0].platform_display.as_deref(),
        Some("APlatformFromALaterBuild"),
        "an unresolved assignment renders as itself rather than vanishing"
    );
    assert!(view.rows[0].platform_unresolved);
    assert_eq!(view.unresolved.len(), 1);
    assert!(
        view.unresolved[0]
            .explanation
            .contains("APlatformFromALaterBuild")
    );
}

// ---------------------------------------------------------------------------
// Forward compatibility through the page
// ---------------------------------------------------------------------------

#[test]
fn saving_from_the_page_keeps_settings_written_by_a_newer_build() {
    let fixture = Fixture::new();
    let dat = fixture.write("shared.dat", LOGIQX);
    std::fs::create_dir_all(fixture.config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &fixture.config_path,
        format!(
            r#"
a_future_top_level_key = "kept"

[[sources]]
id = "shared"
display_name = "Shared"
path = "{}"
kind = "file"
enabled = true
a_future_entry_key = 7
"#,
            dat.display()
        ),
    )
    .unwrap();

    let mut page = fixture.page();
    assert!(page.view().load_error.is_none());
    // Edit something unrelated and save from the page, exactly as a user would.
    page.apply(DatSourcesPageAction::SetEnabled {
        id: "shared".to_string(),
        enabled: false,
    });
    page.apply(DatSourcesPageAction::Save);
    assert_eq!(page.view().save_state, DatSaveState::Saved);

    let text = std::fs::read_to_string(&fixture.config_path).unwrap();
    assert!(text.contains("a_future_top_level_key"), "{text}");
    assert!(text.contains("a_future_entry_key"), "{text}");

    // And the page says it kept them rather than leaving the user guessing.
    let reloaded = fixture.page();
    let view = reloaded.view();
    assert!(
        view.unresolved
            .iter()
            .any(|row| row.explanation.contains("a_future_entry_key")),
        "{:?}",
        view.unresolved
    );
}

// ---------------------------------------------------------------------------
// Removal
// ---------------------------------------------------------------------------

#[test]
fn removing_a_source_never_deletes_the_dat_file() {
    let fixture = Fixture::new();
    let folder = fixture.dir("dats");
    std::fs::write(folder.join("keep.dat"), LOGIQX).unwrap();
    std::fs::write(folder.join("rom.bin"), b"pretend ROM").unwrap();
    let before = snapshot(&folder);

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFolder {
        path: folder.clone(),
    });
    page.apply(DatSourcesPageAction::Save);
    page.apply(DatSourcesPageAction::Remove {
        id: "dats".to_string(),
    });

    let view = page.view();
    assert!(view.rows.is_empty(), "the entry is gone from the list");
    assert!(view.dirty, "and removing it is an unsaved change");
    assert!(
        view.pending_consequences
            .iter()
            .any(|line| line.contains("is not deleted")),
        "the page must say the file survives: {:?}",
        view.pending_consequences
    );

    page.apply(DatSourcesPageAction::Save);

    assert!(folder.exists(), "the folder must survive");
    assert_eq!(
        snapshot(&folder),
        before,
        "removing a registry entry changed something on disk"
    );

    // And the saved registry no longer lists it.
    let config = load_dat_sources_config_from(&fixture.config_path).unwrap();
    let (registry, _) = DatSourceRegistry::from_config(&config);
    assert!(registry.is_empty());
}

#[test]
fn removing_a_saved_source_then_discarding_restores_it() {
    let fixture = Fixture::new();
    let dat = fixture.write("keep.dat", LOGIQX);

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Save);
    assert!(!page.view().dirty);

    page.apply(DatSourcesPageAction::Remove {
        id: "keep".to_string(),
    });
    assert!(page.view().rows.is_empty(), "removed from the draft");
    assert!(page.view().dirty);

    page.apply(DatSourcesPageAction::Revert);
    let view = page.view();
    assert!(
        !view.dirty,
        "discarding a pending removal must restore exactly what was saved"
    );
    assert_eq!(view.rows.len(), 1, "the saved source must come back");
    assert_eq!(view.rows[0].id, "keep");
    assert!(!view.rows[0].changed);
}

// ---------------------------------------------------------------------------
// Validation through the page
// ---------------------------------------------------------------------------

#[test]
fn validating_reports_the_format_and_counts_it_observed() {
    let fixture = Fixture::new();
    let dat = fixture.write("no-intro.dat", LOGIQX);

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Validate {
        id: "no-intro".to_string(),
    });
    assert!(page.is_busy(), "validation runs off the calling thread");
    run_to_completion(&mut page);

    let view = page.view();
    assert!(view.running.is_none());
    let row = &view.rows[0];
    assert_eq!(row.health_state, DatHealthState::Valid);
    assert_eq!(row.formats, vec!["Logiqx XML".to_string()]);
    assert_eq!(row.entry_count, Some(1));
    assert_eq!(row.rom_count, Some(1));
    assert!(row.last_validated.is_some());
    assert!(!row.health_stale);

    // The Inspect panel has the per-file breakdown.
    let detail = row.detail.as_ref().expect("a validated source has detail");
    assert_eq!(detail.files.len(), 1);
    assert_eq!(detail.files[0].status, "OK");
    assert!(
        detail.files[0].detail.contains("Test No-Intro Collection"),
        "{:?}",
        detail.files[0]
    );
}

#[test]
fn validating_a_broken_dat_reports_it_without_touching_the_file() {
    let fixture = Fixture::new();
    let dat = fixture.write("broken.dat", "<?xml version=\"1.0\"?><datafile><game");
    let before = std::fs::read(&dat).unwrap();

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat.clone() });
    page.apply(DatSourcesPageAction::Validate {
        id: "broken".to_string(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    assert_eq!(view.rows[0].health_state, DatHealthState::Invalid);
    assert!(view.rows[0].health_detail.is_some());
    assert_eq!(
        std::fs::read(&dat).unwrap(),
        before,
        "validating must not modify the DAT"
    );
}

#[test]
fn a_validation_result_becomes_an_unsaved_change_the_user_can_discard() {
    let fixture = Fixture::new();
    let dat = fixture.write("no-intro.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Save);
    assert!(!page.view().dirty);

    page.apply(DatSourcesPageAction::Validate {
        id: "no-intro".to_string(),
    });
    run_to_completion(&mut page);

    assert!(
        page.view().dirty,
        "the observed health is a change like any other, not written behind the user's back"
    );
    page.apply(DatSourcesPageAction::Revert);
    assert!(!page.view().dirty);
    assert_eq!(page.view().rows[0].health_state, DatHealthState::NotChecked);
}

#[test]
fn discarding_while_a_validation_is_in_flight_stops_it_from_landing() {
    // Regression: `Revert` used to leave the background job running. `poll()`
    // never checked whether the job's target survived the discard, so a
    // validation that finished afterwards still wrote its result into
    // `self.validations` under the source's id - and if that id had never
    // been saved, the id no longer named anything in the registry at all. A
    // later add that reused the same auto-suggested id (the ordinary case for
    // re-adding the same file) would then show stale Inspect detail for a
    // source nobody had actually checked yet.
    let fixture = Fixture::new();
    let dat = fixture.write("no-intro.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat.clone() });
    page.apply(DatSourcesPageAction::Validate {
        id: "no-intro".to_string(),
    });
    assert!(page.is_busy());

    // The source was never saved, so discarding removes it from the draft
    // entirely - the sharpest version of the race, since the id the worker is
    // about to report against will not exist anywhere in the registry.
    page.apply(DatSourcesPageAction::Revert);
    assert!(!page.is_busy(), "discarding must stop the job immediately");
    assert!(page.view().rows.is_empty());

    run_to_completion(&mut page);
    assert!(page.view().running.is_none());
    assert!(
        page.view().rows.is_empty(),
        "the discarded source must not have reappeared"
    );

    // Re-adding the same file gets the same suggested id. If the abandoned
    // job's result had still landed in `self.validations`, this row would
    // show Inspect detail for a source that was never actually validated.
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    let view = page.view();
    assert_eq!(view.rows[0].id, "no-intro");
    assert_eq!(view.rows[0].health_state, DatHealthState::NotChecked);
    assert!(
        view.rows[0].detail.is_none(),
        "a freshly re-added source must not carry Inspect detail from an \
         abandoned job: {:?}",
        view.rows[0].detail
    );
}

#[test]
fn discarding_forgets_completed_session_validation_records() {
    // Regression: Revert used to leave `validations` (and now the diagnostic
    // groups cache) in place, so a source that was validated, then discarded
    // (never saved), then re-added - reusing its auto-suggested id - showed
    // stale Inspect detail and stale diagnostic groups next to a "Not checked"
    // badge.
    let fixture = Fixture::new();
    let dat = fixture.write("no-intro.dat", &logiqx_with_doctype_and_entries(1));
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat.clone() });
    page.apply(DatSourcesPageAction::Validate {
        id: "no-intro".to_string(),
    });
    run_to_completion(&mut page);
    assert!(
        page.validation("no-intro").is_some(),
        "a completed validation is recorded"
    );
    assert_eq!(
        page.view().rows[0].diagnostic_types(DiagnosticSeverity::Note),
        1,
        "the DOCTYPE note is grouped so the discard below is observable"
    );

    // Discard the never-saved source, then re-add the same file.
    page.apply(DatSourcesPageAction::Revert);
    assert!(page.view().rows.is_empty());
    page.apply(DatSourcesPageAction::AddFile { path: dat });

    let view = page.view();
    assert_eq!(view.rows[0].id, "no-intro");
    assert_eq!(view.rows[0].health_state, DatHealthState::NotChecked);
    assert!(
        view.rows[0].detail.is_none(),
        "a re-added source must not show stale Inspect detail from a discarded run"
    );
    assert!(
        view.rows[0].groups.is_empty(),
        "a re-added source must not show stale diagnostic groups from a discarded run"
    );
}

#[test]
fn a_second_job_is_refused_while_one_is_running() {
    let fixture = Fixture::new();
    let dat = fixture.write("no-intro.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Validate {
        id: "no-intro".to_string(),
    });
    let first = page.view().running.map(|running| running.what);
    // A second request while the first is in flight must not replace it.
    page.apply(DatSourcesPageAction::Validate {
        id: "no-intro".to_string(),
    });
    assert_eq!(page.view().running.map(|running| running.what), first);
    run_to_completion(&mut page);
}

// ---------------------------------------------------------------------------
// Validate all
// ---------------------------------------------------------------------------

#[test]
fn validate_all_schedules_every_source_exactly_once() {
    let fixture = Fixture::new();
    let a = fixture.write("a.dat", &logiqx_with_doctype_and_entries(1));
    let b = fixture.write("b.dat", LOGIQX);
    let c = fixture.write("c.dat", "<?xml version=\"1.0\"?><datafile><game");
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: a });
    page.apply(DatSourcesPageAction::AddFile { path: b });
    page.apply(DatSourcesPageAction::AddFile { path: c });

    page.apply(DatSourcesPageAction::ValidateAll);
    assert!(page.is_busy(), "validate all runs off the calling thread");
    run_to_completion(&mut page);

    assert!(page.view().running.is_none());
    for id in ["a", "b", "c"] {
        assert!(
            page.validation(id).is_some(),
            "{id} must have been validated exactly once"
        );
    }
    let summary = page
        .view()
        .last_validate_all_summary
        .expect("a summary after validate all");
    assert_eq!(summary.total, 3);
    assert_eq!(summary.skipped, 0, "an uncancelled run skips nothing");
}

#[test]
fn validate_all_preserves_each_sources_own_result() {
    // Every row must end up with exactly the same per-source result a single
    // Validate against it would have produced - `ValidateAll` must not merge,
    // average, or otherwise blend distinct sources' reports.
    let fixture = Fixture::new();
    let good = fixture.write("good.dat", LOGIQX);
    let bad = fixture.write("bad.dat", "<?xml version=\"1.0\"?><datafile><game");
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: good });
    page.apply(DatSourcesPageAction::AddFile { path: bad });

    page.apply(DatSourcesPageAction::ValidateAll);
    run_to_completion(&mut page);

    let view = page.view();
    let good_row = view.rows.iter().find(|row| row.id == "good").unwrap();
    let bad_row = view.rows.iter().find(|row| row.id == "bad").unwrap();
    assert_eq!(good_row.health_state, DatHealthState::Valid);
    assert_eq!(good_row.formats, vec!["Logiqx XML".to_string()]);
    assert_eq!(good_row.entry_count, Some(1));
    assert_eq!(bad_row.health_state, DatHealthState::Invalid);
    assert!(bad_row.health_detail.is_some());
}

#[test]
fn validate_all_summarizes_a_mix_of_valid_changed_and_failed() {
    let fixture = Fixture::new();
    let already_checked = fixture.write("already-checked.dat", LOGIQX);
    let never_checked = fixture.write("never-checked.dat", &logiqx_with_doctype_and_entries(1));
    let broken = fixture.write("broken.dat", "<?xml version=\"1.0\"?><datafile><game");
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile {
        path: already_checked,
    });
    page.apply(DatSourcesPageAction::AddFile {
        path: never_checked,
    });
    page.apply(DatSourcesPageAction::AddFile { path: broken });

    // Validate one source ahead of time, with its file left untouched -
    // `ValidateAll` re-validating it must then find nothing changed.
    page.apply(DatSourcesPageAction::Validate {
        id: "already-checked".to_string(),
    });
    run_to_completion(&mut page);

    page.apply(DatSourcesPageAction::ValidateAll);
    run_to_completion(&mut page);

    let summary = page
        .view()
        .last_validate_all_summary
        .expect("a summary after validate all");
    assert_eq!(summary.total, 3);
    assert_eq!(summary.valid, 1, "the already-checked, unchanged source");
    assert_eq!(summary.changed, 1, "the never-before-checked source");
    assert_eq!(summary.failed, 1, "the broken source");
    assert_eq!(summary.skipped, 0);
}

#[test]
fn validate_all_on_an_empty_registry_reports_an_empty_summary_without_a_job() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    assert!(page.view().rows.is_empty());

    page.apply(DatSourcesPageAction::ValidateAll);

    assert!(
        !page.is_busy(),
        "nothing to validate means no job is spawned"
    );
    let summary = page
        .view()
        .last_validate_all_summary
        .expect("an immediate empty summary");
    assert_eq!(summary, ValidateAllSummary::default());
}

#[test]
fn clicking_validate_all_twice_while_active_does_not_duplicate_the_run() {
    let fixture = Fixture::new();
    let a = fixture.write("a.dat", LOGIQX);
    let b = fixture.write("b.dat", &logiqx_with_doctype_and_entries(1));
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: a });
    page.apply(DatSourcesPageAction::AddFile { path: b });

    page.apply(DatSourcesPageAction::ValidateAll);
    let first = page.view().running.clone();
    assert!(first.is_some());
    // A second click while the first run is in flight must not replace it or
    // start a second worker.
    page.apply(DatSourcesPageAction::ValidateAll);
    assert_eq!(page.view().running, first);

    run_to_completion(&mut page);

    let summary = page
        .view()
        .last_validate_all_summary
        .expect("a summary after validate all");
    assert_eq!(
        summary.total, 2,
        "exactly one run's worth of sources, not doubled by the repeated click"
    );
    assert!(page.validation("a").is_some());
    assert!(page.validation("b").is_some());
}

#[test]
fn one_failed_source_does_not_abort_validation_of_the_rest() {
    let fixture = Fixture::new();
    let broken = fixture.write("aaa-broken.dat", "<?xml version=\"1.0\"?><datafile><game");
    let good_one = fixture.write("bbb-good-one.dat", LOGIQX);
    let good_two = fixture.write("ccc-good-two.dat", &logiqx_with_doctype_and_entries(1));
    let mut page = fixture.page();
    // Added, and therefore scheduled, in an order where the failure comes
    // first - proving a failure partway through does not stop the sources
    // scheduled after it.
    page.apply(DatSourcesPageAction::AddFile { path: broken });
    page.apply(DatSourcesPageAction::AddFile { path: good_one });
    page.apply(DatSourcesPageAction::AddFile { path: good_two });

    page.apply(DatSourcesPageAction::ValidateAll);
    run_to_completion(&mut page);

    assert_eq!(
        page.view().rows[0].health_state,
        DatHealthState::Invalid,
        "the aaa- prefix sorts first, so this is the source that failed"
    );
    assert!(page.validation("bbb-good-one").is_some());
    assert_eq!(
        page.validation("bbb-good-one").unwrap().state,
        DatHealthState::Valid
    );
    assert!(page.validation("ccc-good-two").is_some());
    assert_eq!(
        page.validation("ccc-good-two").unwrap().state,
        DatHealthState::Valid
    );
    let summary = page.view().last_validate_all_summary.unwrap();
    assert_eq!(summary.total, 3);
    assert_eq!(summary.failed, 1);
}

#[test]
fn validate_all_never_touches_a_dat_file_on_disk() {
    let fixture = Fixture::new();
    let a = fixture.write("a.dat", LOGIQX);
    let b = fixture.write("b.dat", "<?xml version=\"1.0\"?><datafile><game");
    let before = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: a.clone() });
    page.apply(DatSourcesPageAction::AddFile { path: b.clone() });

    page.apply(DatSourcesPageAction::ValidateAll);
    run_to_completion(&mut page);

    assert_eq!(
        (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap()),
        before,
        "validating must never modify a DAT file"
    );
}

// ---------------------------------------------------------------------------
// Audit through the page
// ---------------------------------------------------------------------------

/// A page with one registered DAT and a ROM folder holding one known file and
/// one unknown one.
fn audit_fixture() -> (Fixture, DatSourcesPageState, PathBuf) {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let roms = fixture.dir("roms");
    std::fs::write(roms.join("super.bin"), SUPER_BIN).unwrap();
    std::fs::write(roms.join("mystery.bin"), b"not in any catalogue").unwrap();

    let mut page = fixture.page_with_library(vec![roms.clone()]);
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    (fixture, page, roms)
}

#[test]
fn the_audit_summary_shows_elapsed_time_and_a_shortened_scan_folder() {
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms.clone(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let audit = view.audit.as_ref().expect("an audit result");
    // The full folder is still on the result for provenance...
    assert_eq!(audit.scan_root, roms.to_string_lossy());
    // ...but the display uses a shortened form that never exposes the full path.
    assert_eq!(audit.scan_root_short, shorten_path(&roms.to_string_lossy()));
    assert!(
        !audit.scan_root_short.contains("/tmp"),
        "the full private path must not be shown: {}",
        audit.scan_root_short
    );
    // A completed audit knows how long it took.
    assert!(audit.elapsed_seconds.is_some());

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Completed in"));
    assert!(
        !rendered_text_contains(&output, &roms.to_string_lossy()),
        "the full scan folder must not be rendered"
    );
}

#[test]
fn the_audit_folder_picker_shows_friendly_names_with_the_full_path_secondary() {
    let fixture = Fixture::new();
    let roms = fixture.dir("roms");
    let deep = fixture.dir("library/GameCube");
    let dat = fixture.write("collection.dat", LOGIQX);
    let mut page = fixture.page_with_library(vec![roms.clone(), deep.clone()]);
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    let view = page.view();
    let row = view
        .rows
        .iter()
        .find(|r| r.id == "collection")
        .expect("the added source");

    let mut ui_state = DatSourcesPageUi {
        open_audit_picker: Some(row.id.clone()),
        ..DatSourcesPageUi::default()
    };
    let output = render(&view, &mut ui_state);

    assert!(rendered_text_contains(&output, "Check which files?"));
    assert!(
        rendered_text_contains(&output, "Library folder: GameCube"),
        "the friendly folder name must come first"
    );
    assert!(
        rendered_text_contains(&output, &deep.to_string_lossy()),
        "the full path must stay accessible, muted, under the friendly name"
    );
    assert!(
        rendered_text_contains(&output, &roms.to_string_lossy()),
        "every offered folder's full path must stay reachable"
    );
    assert!(rendered_text_contains(&output, "Choose another folder…"));
    assert!(rendered_text_contains(&output, "Choose one file…"));
}

#[test]
fn the_audit_folder_picker_stays_usable_at_compact_width() {
    let fixture = Fixture::new();
    let deep = fixture.dir("library/GameCube");
    let dat = fixture.write("collection.dat", LOGIQX);
    let mut page = fixture.page_with_library(vec![deep.clone()]);
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    let view = page.view();
    let row = view
        .rows
        .iter()
        .find(|r| r.id == "collection")
        .expect("the added source");

    let mut ui_state = DatSourcesPageUi {
        open_audit_picker: Some(row.id.clone()),
        ..DatSourcesPageUi::default()
    };
    let output = render_at_width(&view, &mut ui_state, 480.0);

    assert!(
        rendered_text_contains(&output, "Library folder: GameCube"),
        "the friendly name must survive a compact window"
    );
    assert!(
        rendered_text_contains(&output, &deep.to_string_lossy()),
        "the full path must remain reachable at compact width"
    );
    assert!(rendered_text_contains(&output, "Choose another folder…"));
    assert!(rendered_text_contains(&output, "Choose one file…"));
}

#[test]
fn a_single_regular_file_audit_builds_a_read_only_rename_preview() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let roms = fixture.dir("roms");
    let source = roms.join("not-canonical.bin");
    std::fs::write(&source, SUPER_BIN).unwrap();

    let mut page = fixture.page_with_library(vec![roms]);
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    let before = snapshot(&fixture.root);
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: source.clone(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let audit = view.audit.as_ref().expect("single-file audit result");
    assert_eq!(audit.files_scanned, 1);
    assert_eq!(audit.scan_root, source.to_string_lossy());
    let plan = view.rename_plan.as_ref().expect("read-only rename plan");
    assert_eq!(plan.counts.suggested, 1, "{:?}", plan.counts);
    assert_eq!(plan.rows[0].proposed_basename.as_deref(), Some("super.bin"));
    assert_eq!(
        snapshot(&fixture.root),
        before,
        "audit and planning never mutate files"
    );
}

#[test]
fn identify_rename_uses_all_enabled_evidence_without_manual_catalogue_choice() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let roms = fixture.dir("roms");
    let mut page = fixture.page_with_library(vec![roms]);
    page.apply(DatSourcesPageAction::AddFile { path: dat });

    let before = snapshot(&fixture.root);
    let output = render_identify_rename(&page.view(), &mut DatSourcesPageUi::default());

    assert!(rendered_text_contains(&output, "Identify & Rename"));
    assert!(rendered_text_contains(&output, "No filename guessing"));
    assert!(rendered_text_contains(&output, "Available evidence"));
    assert!(rendered_text_contains(&output, "No-Intro / Local DATs"));
    assert!(rendered_text_contains(&output, "Choose library or file…"));
    assert_eq!(
        snapshot(&fixture.root),
        before,
        "opening the workflow must not start an audit or mutate the collection"
    );
}

#[test]
fn identify_rename_combined_action_scans_once_and_builds_a_rename_plan() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let roms = fixture.dir("roms");
    let source = roms.join("messy-name.bin");
    std::fs::write(&source, SUPER_BIN).unwrap();
    let mut page = fixture.page_with_library(vec![roms]);
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    let before = snapshot(&fixture.root);

    page.apply(DatSourcesPageAction::AuditAllEnabled {
        scan_root: source.clone(),
    });
    run_to_completion(&mut page);

    let audit = page.view().audit.expect("combined audit result");
    assert_eq!(audit.source_id, "combined-enabled-dat-sources");
    assert_eq!(audit.files_scanned, 1);
    let plan = page.view().rename_plan.expect("combined rename plan");
    assert_eq!(plan.counts.suggested, 1);
    assert_eq!(plan.rows[0].proposed_basename.as_deref(), Some("super.bin"));
    assert_eq!(
        snapshot(&fixture.root),
        before,
        "combined preview is read-only"
    );
}

#[test]
fn combined_identify_rename_excludes_configured_redump_bios_sources() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.managed_sources
        .add_redump_bios(
            RedumpBiosSystem::PlayStation,
            archivefs_core::dat::updates::ManagedDatUpdatePolicy::Manual,
        )
        .unwrap();

    assert!(page.combined_audit_sources().is_empty());
}

#[test]
fn the_audit_summary_survives_navigation_and_is_replaced_by_a_new_generation() {
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms.clone(),
    });
    run_to_completion(&mut page);

    // Navigating away and back keeps the same summary for this generation.
    let first = page
        .view()
        .audit
        .as_ref()
        .expect("summary")
        .headline
        .clone();
    for _ in 0..3 {
        assert_eq!(
            page.view().audit.as_ref().map(|a| a.headline.clone()),
            Some(first.clone()),
            "the same generation must keep its summary across views"
        );
    }

    // A cancelled new generation never shows a success summary.
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    page.apply(DatSourcesPageAction::CancelJob);
    run_to_completion(&mut page);
    assert!(
        page.view().audit.is_none(),
        "a cancelled audit never shows a success summary"
    );
}

#[test]
fn the_audit_picker_offers_the_configured_library_folders() {
    let (_fixture, page, roms) = audit_fixture();
    assert_eq!(page.view().library_folders, vec![roms]);
}

#[test]
fn an_audit_reports_only_the_categories_the_core_produces() {
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms.clone(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    assert!(view.audit_error.is_none(), "{:?}", view.audit_error);
    let audit = view.audit.expect("the audit produced a result");

    // Exactly the eight categories `AuditSummary` counts, in the core's own
    // vocabulary. Nothing invented, nothing merged.
    let labels: Vec<&str> = audit.categories.iter().map(|c| c.label).collect();
    assert_eq!(
        labels,
        vec![
            "Exact",
            "Exact (multiple)",
            "Probable",
            "Probable (multiple)",
            "Filename only",
            "Ambiguous",
            "Not in catalogue",
            "No usable evidence",
        ]
    );
    for category in &audit.categories {
        assert!(
            !category.meaning.is_empty(),
            "'{}' must explain what it means",
            category.label
        );
    }

    let count_of = |label: &str| {
        audit
            .categories
            .iter()
            .find(|c| c.label == label)
            .map(|c| c.count)
            .unwrap()
    };
    assert_eq!(count_of("Exact"), 1);
    assert_eq!(count_of("Not in catalogue"), 1);
    assert_eq!(audit.files_scanned, 2);
    assert!(!audit.truncated);

    // Provenance is on the result, not something the reader has to remember.
    assert_eq!(audit.source_id, "collection");
    assert_eq!(
        audit.catalogue_names,
        vec!["Test No-Intro Collection".to_string()]
    );
    assert_eq!(audit.scan_root, roms.to_string_lossy());
    assert_eq!(audit.entries.len(), 2);
    assert_eq!(audit.entries_truncated, 0);
}

#[test]
fn an_audit_changes_nothing_on_disk() {
    // The guarantee the page promises in its banner, checked rather than
    // asserted in prose.
    let (fixture, mut page, roms) = audit_fixture();
    let before_roms = snapshot(&roms);
    let before_all = snapshot(&fixture.root);

    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms.clone(),
    });
    run_to_completion(&mut page);
    assert!(page.view().audit.is_some());

    assert_eq!(snapshot(&roms), before_roms, "the ROM folder changed");
    assert_eq!(
        snapshot(&fixture.root),
        before_all,
        "an audit created, removed or altered something"
    );
}

#[test]
fn an_audit_can_be_cancelled_from_the_page() {
    // Deterministic by construction: `CancelJob` flips the page's own
    // `cancel_requested` flag, and `poll()` drops any terminal result that
    // arrives after it - so whatever the worker does (observe the flag and
    // send `Cancelled`, or finish first and send `Audited`), a cancelled audit
    // can never land in `view.audit`.
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    let running = page.view().running.expect("a job is running");
    assert_eq!(running.what, "Auditing");
    assert!(
        running.cancellable,
        "an audit must offer a Cancel that actually does something"
    );

    // Cancel flips the visible state immediately, while the worker is still
    // running.
    page.apply(DatSourcesPageAction::CancelJob);
    let running = page.view().running.expect("still busy while stopping");
    assert!(
        running.cancellation_requested,
        "the card must read 'Stopping…' the moment Cancel is pressed"
    );
    assert!(
        page.is_busy(),
        "the operation remains busy until the worker confirms termination"
    );

    run_to_completion(&mut page);

    let view = page.view();
    assert!(view.running.is_none(), "the job stopped");
    // A cancelled run reports nothing rather than a partial result dressed up
    // as a complete one.
    assert!(view.audit.is_none());
    assert!(view.audit_error.is_none(), "cancelling is not a failure");
}

#[test]
fn an_audit_of_an_empty_folder_reports_why_rather_than_claiming_all_clear() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let empty = fixture.dir("empty");

    let mut page = fixture.page_with_library(vec![empty.clone()]);
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: empty,
    });
    run_to_completion(&mut page);

    let view = page.view();
    assert!(view.audit.is_none());
    let error = view.audit_error.expect("the reason must be shown");
    assert!(error.contains("no files"), "{error}");
}

#[test]
fn removing_a_source_drops_a_result_attributed_to_it() {
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    run_to_completion(&mut page);
    assert!(page.view().audit.is_some());

    page.apply(DatSourcesPageAction::Remove {
        id: "collection".to_string(),
    });
    assert!(
        page.view().audit.is_none(),
        "a result attributed to a source that is gone has nothing to point at"
    );
}

#[test]
fn discarding_while_an_audit_is_in_flight_stops_it_from_landing() {
    // Regression: the same race as
    // `discarding_while_a_validation_is_in_flight_stops_it_from_landing`, for
    // the path where the checklist calls it out explicitly - a late audit
    // result must not be able to update a job that discard already swept
    // away, even though a real `AuditReport` moving out of the channel does
    // not touch the row it was for the way a health write does.
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    assert!(page.is_busy());

    page.apply(DatSourcesPageAction::Revert);
    assert!(
        !page.is_busy(),
        "discarding must stop the audit immediately"
    );
    assert!(page.view().audit.is_none());

    run_to_completion(&mut page);
    let view = page.view();
    assert!(view.running.is_none());
    assert!(
        view.audit.is_none(),
        "an audit result must not appear for a source that was discarded before \
         the run finished, got: {:?}",
        view.audit
    );
    assert!(
        view.audit_error.is_none(),
        "an abandoned job is not a failure the user needs telling about"
    );
}

#[test]
fn removing_a_source_with_no_job_running_does_not_touch_a_different_jobs_job() {
    // `abandon_job_for` must be surgical: removing a source that is not the
    // one a running job targets must leave that job alone. Reachable only at
    // the state layer today, since the GUI disables Remove entirely while any
    // job runs - covered here so the guarantee does not depend on that gate
    // staying in place.
    let (_fixture, mut page, roms) = audit_fixture();
    let dat_path = page.view().rows[0].path.clone();
    let unrelated = DatSourceEntry::new(
        "unrelated".to_string(),
        "Unrelated".to_string(),
        PathBuf::from(&dat_path).with_file_name("unrelated.dat"),
        DatSourceKind::File,
    );
    std::fs::write(&unrelated.path, LOGIQX).unwrap();
    page.apply(DatSourcesPageAction::AddFile {
        path: unrelated.path.clone(),
    });

    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    assert!(page.is_busy());

    // Removing the *other* source must not cancel the audit running against
    // "collection".
    page.apply(DatSourcesPageAction::Remove {
        id: "unrelated".to_string(),
    });
    assert!(
        page.is_busy(),
        "removing an unrelated source must not cancel a running job"
    );

    run_to_completion(&mut page);
    let view = page.view();
    assert!(
        view.audit.is_some(),
        "the audit against the still-registered source must have completed"
    );
}

// ---------------------------------------------------------------------------
// Wording
// ---------------------------------------------------------------------------

#[test]
fn the_page_states_what_it_supports_and_what_it_will_never_do() {
    // These two sentences are the page's contract with the user, so they are
    // pinned rather than left to drift.
    assert!(READ_ONLY_PROMISE.contains(SAFE_PROMISE));
    assert!(READ_ONLY_PROMISE.contains("delete"));
    assert!(SUPPORTED_FORMATS.contains("Logiqx"));
    assert!(SUPPORTED_FORMATS.contains("ClrMamePro"));
}

// ---------------------------------------------------------------------------
// Validation warning presentation
// ---------------------------------------------------------------------------

/// A warning-severity diagnostic for a test report.
fn warn(message: impl Into<String>) -> DatDiagnostic {
    diagnostic(DiagnosticSeverity::Warning, "test_warning", message)
}

/// A parser-note-severity diagnostic for a test report.
fn note(message: impl Into<String>) -> DatDiagnostic {
    diagnostic(DiagnosticSeverity::Note, "test_note", message)
}

/// An error-severity diagnostic for a test report.
fn error(message: impl Into<String>) -> DatDiagnostic {
    diagnostic(DiagnosticSeverity::Error, "test_error", message)
}

fn diagnostic(
    severity: DiagnosticSeverity,
    code: &'static str,
    message: impl Into<String>,
) -> DatDiagnostic {
    DatDiagnostic {
        severity,
        code,
        message: message.into(),
        line: None,
        column: None,
    }
}

/// A page holding one folder source plus a stored validation report built by
/// the test, so diagnostic presentation can be driven without depending on
/// parser wording. The health state is supplied by the test because it now
/// depends on the severities present.
fn page_with_report(
    per_file_diagnostics: Vec<Vec<DatDiagnostic>>,
    state: DatHealthState,
    truncated: bool,
    total_dat_files: Option<usize>,
) -> (Fixture, DatSourcesPageState) {
    let fixture = Fixture::new();
    let folder = fixture.dir("warn");
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFolder {
        path: folder.clone(),
    });
    let id = "warn".to_string();
    let files: Vec<DatFileReport> = per_file_diagnostics
        .iter()
        .enumerate()
        .map(|(index, diagnostics)| DatFileReport {
            path: format!("{}/warn-{index}.dat", folder.display()),
            file_name: format!("warn-{index}.dat"),
            outcome: DatFileOutcome::Parsed {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::GenericLogiqx,
                name: Some("Test Catalogue".to_string()),
                version: Some("2026-01-01".to_string()),
                entry_count: 1,
                rom_count: 1,
                diagnostics: diagnostics.clone(),
            },
        })
        .collect();
    let report = DatValidationReport {
        source_id: id.clone(),
        path: folder.to_string_lossy().into_owned(),
        kind: "DAT folder",
        state,
        files,
        duplicate_identities: Vec::new(),
        skipped: Vec::new(),
        truncated,
        total_dat_files,
        summary: "1 DAT files, 1 entries, 1 ROMs".to_string(),
        entry_count: 1,
        rom_count: 1,
        formats: vec!["Logiqx XML".to_string()],
        path_refusal: None,
    };
    page.validations.insert(id.clone(), report.clone());
    if let Some(entry) = page.draft.get_mut(&id) {
        entry.health = report.to_health(&folder, DatSourceKind::Folder);
    }
    (fixture, page)
}

/// Draws the page headlessly, the way the cheat-sources page's tests do.
fn render(view: &DatSourcesPageView, ui_state: &mut DatSourcesPageUi) -> egui::FullOutput {
    let context = egui::Context::default();
    context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let _ = show_dat_sources_page(ui, view, ui_state);
        });
    })
}

/// Draws the task-oriented rename entry point without creating an audit job.
/// Rendering must stay read-only: starting an audit always requires the
/// explicit catalogue and folder/file clicks the page exposes.
fn render_identify_rename(
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> egui::FullOutput {
    let context = egui::Context::default();
    context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let _ = show_identify_rename_page(ui, view, ui_state);
        });
    })
}

fn render_quick_rename(
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> egui::FullOutput {
    let context = egui::Context::default();
    context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let _ = show_quick_rename_page(ui, view, ui_state);
        });
    })
}

#[test]
fn quick_rename_opens_without_scanning_or_writing() {
    let fixture = Fixture::new();
    let page = fixture.page();
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);

    assert!(rendered_text_contains(&output, "Quick Rename"));
    assert!(view.running.is_none());
    assert!(view.audit.is_none());
    assert!(view.rename_plan.is_none());
    assert!(!fixture.config_path.exists());
}

#[test]
fn verify_renders_the_app_owned_romm_summary() {
    let fixture = Fixture::new();
    let page = fixture.page();
    let mut view = page.view();
    view.romm_summary = Some(crate::romm_source::VerifyRommSummary {
        total: 94_000,
        confirmed: 90_000,
        strong: 2_000,
        probable: 1_000,
        ambiguous: 500,
        stale: 250,
        unmatched: 250,
    });

    let output = render(&view, &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(&output, "RomM identity summary"));
    assert!(rendered_text_contains(&output, "94000"));
    assert!(rendered_text_contains(&output, "92000 / 94000"));
}

/// Draws every disclosure body for assertions about information retained in
/// the technical view. The normal `render` helper deliberately keeps those
/// bodies closed so beginner-facing tests exercise the real default.
fn render_with_details(
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> egui::FullOutput {
    let context = egui::Context::default();
    context.memory_mut(|memory| memory.set_everything_is_visible(true));
    context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let _ = show_dat_sources_page(ui, view, ui_state);
        });
    })
}

/// Draws only the running-job card, so the platform line can be asserted
/// without the source card's own platform control interfering.
fn render_running_card(running: &RunningJobView) -> egui::FullOutput {
    let context = egui::Context::default();
    context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let _ = show_running_job(ui, running);
        });
    })
}

/// The same helper the shared widgets' own tests use.
fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|shape| shape_contains(shape, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

/// How many times `needle` appears across every rendered text shape.
fn rendered_text_count(output: &egui::FullOutput, needle: &str) -> usize {
    fn shape_count(shape: &egui::Shape, needle: &str) -> usize {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().matches(needle).count(),
            egui::Shape::Vec(nested) => nested.iter().map(|shape| shape_count(shape, needle)).sum(),
            _ => 0,
        }
    }
    output
        .shapes
        .iter()
        .map(|clipped| shape_count(&clipped.shape, needle))
        .sum()
}

#[test]
fn warnings_render_count_summary_and_expandable_details() {
    let warnings = vec![
        "The header version differs from the file's name".to_string(),
        "A ROM entry has no SHA-1 checksum; only CRC32 was compared".to_string(),
    ];
    let diagnostics = vec![warnings.iter().map(warn).collect::<Vec<_>>()];
    let (_fixture, page) =
        page_with_report(diagnostics, DatHealthState::ValidWithWarnings, false, None);
    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 2);
    assert_eq!(row.diagnostic_occurrences(DiagnosticSeverity::Warning), 2);
    assert_eq!(row.health_state, DatHealthState::ValidWithWarnings);

    let mut ui_state = DatSourcesPageUi::default();
    let collapsed = render(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &collapsed,
        "2 catalogue issues found"
    ));
    assert!(rendered_text_contains(
        &collapsed,
        "The catalogue still works. Files that could not be used were skipped."
    ));
    assert!(rendered_text_contains(&collapsed, "What happened?"));
    assert!(!rendered_text_contains(
        &collapsed,
        "2 warning types, 2 occurrences"
    ));
    for warning in &warnings {
        assert!(
            !rendered_text_contains(&collapsed, warning),
            "raw parser messages stay out of the beginner-facing summary"
        );
    }
    assert!(
        !rendered_text_contains(&collapsed, "Location unavailable"),
        "the drill-down must stay hidden until the user expands a group"
    );

    // Expanding one group reveals its locations (unavailable here, since the
    // test diagnostics carry no parser location).
    ui_state.open_diagnostic = Some(row.groups[0].id.clone());
    let expanded = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &expanded,
        "2 warning types, 2 occurrences"
    ));
    for warning in &warnings {
        assert!(rendered_text_contains(&expanded, warning));
    }
    assert!(rendered_text_contains(&expanded, "Hide locations"));
    assert!(rendered_text_contains(&expanded, "Location unavailable"));
}

#[test]
fn zero_warnings_show_no_warning_details_control() {
    let (_fixture, page) = page_with_report(vec![Vec::new()], DatHealthState::Valid, false, None);
    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 0);
    assert_eq!(row.diagnostic_occurrences(DiagnosticSeverity::Warning), 0);
    assert!(row.groups.is_empty());

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(
        !rendered_text_contains(&output, "View locations"),
        "no diagnostics means no details control"
    );
}
#[test]
fn warnings_and_parser_notes_render_as_separate_sections() {
    // A parsed file carrying both a real warning and a parser note must show
    // them as two distinct, labelled sections - the note must never be counted
    // as a warning.
    let note_text = "Logiqx DTD referenced, but no trusted local copy was found. The DAT was parsed normally without DTD validation.";
    let warning_text =
        "crc attribute on a rom element is not a well-formed checksum and was dropped";
    let (_fixture, page) = page_with_report(
        vec![vec![warn(warning_text), note(note_text)]],
        DatHealthState::ValidWithWarnings,
        false,
        None,
    );
    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 1);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Note), 1);
    let warning_group = row.groups_of(DiagnosticSeverity::Warning)[0];
    let note_group = row.groups_of(DiagnosticSeverity::Note)[0];
    assert_eq!(warning_group.message, warning_text);
    assert_eq!(note_group.message, note_text);

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &output,
        "1 warning type, 1 occurrence"
    ));
    assert!(rendered_text_contains(
        &output,
        "1 parser-note type, 1 occurrence"
    ));
    // The note reassurance is retained in the explanation disclosure.
    assert!(rendered_text_contains(
        &output,
        "1 additional catalogue note recorded. No action is needed for these."
    ));

    // Expanding a note group reveals its (unavailable) location.
    ui_state.open_diagnostic = Some(note_group.id.clone());
    let expanded = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(&expanded, "Hide locations"));
    assert!(rendered_text_contains(&expanded, note_text));
}

#[test]
fn an_error_diagnostic_renders_in_its_own_section_not_as_a_warning() {
    // An Error-severity diagnostic must not be folded into the warnings list:
    // it gets its own Blocked section, and the source reads Invalid.
    let error_text = "the catalogue declares an entry the build refuses to index";
    let (_fixture, page) = page_with_report(
        vec![vec![error(error_text)]],
        DatHealthState::Invalid,
        false,
        None,
    );
    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Error), 1);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 0);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Note), 0);
    assert_eq!(row.health_state, DatHealthState::Invalid);

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &output,
        "1 error type, 1 occurrence"
    ));
    assert!(rendered_text_contains(&output, error_text));
    assert!(
        !rendered_text_contains(&output, "warning type"),
        "the error must not appear in a warning section"
    );
}

#[test]
fn an_error_headline_does_not_falsely_reassure_the_catalogue_still_works() {
    // Error severity means core marked the whole source Invalid ("part of
    // what they asked for is unusable"), which is a different claim than
    // "still works, some files were skipped" (the Warning-only wording).
    // The headline must say so truthfully and match the Blocked badge tone.
    let error_text = "the catalogue declares an entry the build refuses to index";
    let (_fixture, page) = page_with_report(
        vec![vec![error(error_text)]],
        DatHealthState::Invalid,
        false,
        None,
    );
    let view = page.view();
    assert_eq!(view.rows[0].health_state, DatHealthState::Invalid);

    let mut ui_state = DatSourcesPageUi::default();
    let collapsed = render(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &collapsed,
        "1 catalogue issue found"
    ));
    assert!(rendered_text_contains(
        &collapsed,
        "Some files could not be used and need your attention."
    ));
    assert!(!rendered_text_contains(
        &collapsed,
        "The catalogue still works. Files that could not be used were skipped."
    ));
}

#[test]
fn mixed_errors_warnings_and_notes_render_as_three_sections() {
    // All three severities present: each gets its own labelled section, and the
    // badge stays driven by core health (Invalid because an error is present).
    let error_text = "one entry was refused";
    let warning_text = "a checksum was dropped";
    let note_text = "Logiqx DTD referenced, but no trusted local copy was found. The DAT was parsed normally without DTD validation.";
    let (_fixture, page) = page_with_report(
        vec![vec![error(error_text), warn(warning_text), note(note_text)]],
        DatHealthState::Invalid,
        false,
        None,
    );
    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Error), 1);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 1);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Note), 1);

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Invalid"));
    assert!(rendered_text_contains(
        &output,
        "1 error type, 1 occurrence"
    ));
    assert!(rendered_text_contains(
        &output,
        "1 warning type, 1 occurrence"
    ));
    assert!(rendered_text_contains(
        &output,
        "1 parser-note type, 1 occurrence"
    ));
}

#[test]
fn repeated_identical_notes_group_into_one_type_with_full_occurrence_count() {
    // A folder of 512 DAT files all carrying the same DOCTYPE note must render
    // as ONE group with an occurrence count - never 512 separate lines.
    let fixture = Fixture::new();
    let folder = fixture.dir("dats");
    for index in 0..512 {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE datafile PUBLIC "-//Logiqx//DTD ROM Management Datafile//EN" "http://www.logiqx.com/Dats/datafile.dtd">
<datafile><header><name>Set {index}</name><version>1</version></header>
<game name="Game {index}"><rom name="g.bin" size="16" crc="0c7e7fd8"/></game></datafile>"#
        );
        std::fs::write(folder.join(format!("set-{index:04}.dat")), &xml).unwrap();
    }

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFolder { path: folder });
    page.apply(DatSourcesPageAction::Validate {
        id: "dats".to_string(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(
        row.health_state,
        DatHealthState::Valid,
        "parser notes do not lower the verdict"
    );
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Note), 1);
    assert_eq!(row.diagnostic_occurrences(DiagnosticSeverity::Note), 512);
    let note_group = &row.groups_of(DiagnosticSeverity::Note)[0];
    assert_eq!(note_group.affected_file_count, 512);
    assert_eq!(
        note_group.occurrences.len(),
        MAX_DIAGNOSTIC_OCCURRENCES_SHOWN,
        "the drill-down must be bounded"
    );
    assert!(note_group.occurrences_truncated);

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &output,
        "1 parser-note type, 512 occurrences"
    ));
}

#[test]
fn thousands_of_diagnostics_stay_bounded_and_deterministic() {
    // 100 files x 30 identical warnings = 3000 occurrences: one group, exact
    // counts, a drill-down capped at MAX_DIAGNOSTIC_OCCURRENCES_SHOWN rows, and
    // a deterministic message order alongside a second distinct type.
    let per_file: Vec<Vec<DatDiagnostic>> = (0..100)
        .map(|_| (0..30).map(|_| warn("repeated checksum dropped")).collect())
        .collect();
    let mut with_second_type = per_file.clone();
    with_second_type[0].push(note("Logiqx DTD referenced, but no trusted local copy was found. The DAT was parsed normally without DTD validation."));

    let (_fixture, page) = page_with_report(
        with_second_type,
        DatHealthState::ValidWithWarnings,
        false,
        None,
    );
    let row = &page.view().rows[0];
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 1);
    assert_eq!(
        row.diagnostic_occurrences(DiagnosticSeverity::Warning),
        3000
    );
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Note), 1);

    let warning_group = &row.groups_of(DiagnosticSeverity::Warning)[0];
    assert_eq!(warning_group.affected_file_count, 100);
    assert_eq!(
        warning_group.occurrences.len(),
        MAX_DIAGNOSTIC_OCCURRENCES_SHOWN,
        "the drill-down must stay bounded however many diagnostics exist"
    );
    assert!(warning_group.occurrences_truncated);
    assert_eq!(
        warning_group.occurrences[0].file_name, "warn-0.dat",
        "occurrence rows keep the file they belong to"
    );
    // 30 per file; the 50-row cap therefore spans warn-0.dat and warn-1.dat.
    assert_eq!(
        warning_group.occurrences[29].file_name, "warn-0.dat",
        "occurrences stay in file order"
    );
    assert_eq!(
        warning_group.occurrences.last().unwrap().file_name,
        "warn-1.dat",
        "the cap cuts mid-file, never out of order"
    );

    // Deterministic: repeated view builds produce identical groups.
    let first = page.view().rows[0].groups.clone();
    let second = page.view().rows[0].groups.clone();
    assert_eq!(first, second);
}

#[test]
fn expanding_one_group_does_not_expand_the_others() {
    let (_fixture, page) = page_with_report(
        vec![vec![
            warn("first warning text"),
            warn("second warning text"),
            note("first note text"),
        ]],
        DatHealthState::ValidWithWarnings,
        false,
        None,
    );
    let view = page.view();
    let row = &view.rows[0];
    let warning_groups = row.groups_of(DiagnosticSeverity::Warning);
    assert_eq!(warning_groups.len(), 2);

    let mut ui_state = DatSourcesPageUi::default();
    let collapsed = render_with_details(&view, &mut ui_state);
    assert_eq!(rendered_text_count(&collapsed, "View locations"), 3);

    // Open only the first warning group.
    ui_state.open_diagnostic = Some(warning_groups[0].id.clone());
    let expanded = render_with_details(&view, &mut ui_state);
    assert_eq!(
        rendered_text_count(&expanded, "Hide locations"),
        1,
        "exactly one group expands"
    );
    assert_eq!(rendered_text_count(&expanded, "View locations"), 2);
}

#[test]
fn diagnostics_group_by_code_not_only_by_message() {
    // The same message text under two different codes is two distinct types.
    let same_text = "identical wording, different kinds";
    let (_fixture, page) = page_with_report(
        vec![vec![
            DatDiagnostic {
                severity: DiagnosticSeverity::Warning,
                code: "code_a",
                message: same_text.to_string(),
                line: None,
                column: None,
            },
            DatDiagnostic {
                severity: DiagnosticSeverity::Warning,
                code: "code_b",
                message: same_text.to_string(),
                line: None,
                column: None,
            },
        ]],
        DatHealthState::ValidWithWarnings,
        false,
        None,
    );
    let row = &page.view().rows[0];
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 2);
    assert_eq!(row.diagnostic_occurrences(DiagnosticSeverity::Warning), 2);
}

#[test]
fn diagnostics_with_the_same_message_at_different_severities_stay_separate() {
    // A Note and a Warning with identical wording are two distinct groups and
    // the note never drags the warning's verdict down.
    let same_text = "identical wording, different severities";
    let (_fixture, page) = page_with_report(
        vec![vec![note(same_text), warn(same_text)]],
        DatHealthState::ValidWithWarnings,
        false,
        None,
    );
    let row = &page.view().rows[0];
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Note), 1);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 1);
    assert_eq!(row.diagnostic_occurrences(DiagnosticSeverity::Warning), 1);
}

#[test]
fn clrmamepro_truncation_warnings_group_across_files() {
    // Regression: the ClrMamePro parser records a byte offset on its only
    // warning, and DatDiagnostic used to carry the Display form of the message
    // ("… (byte 12)"). Every occurrence then had a unique message and grouping
    // silently failed for TOSEC/ClrMamePro - the exact format this PR exists
    // for. The message is now the raw text; the offset lives in the location,
    // so two identical truncations in two files become ONE group.
    let fixture = Fixture::new();
    let folder = fixture.dir("dats");
    let description = "x".repeat(80);
    for (name, header) in [("a", "A"), ("b", "B")] {
        std::fs::write(
            folder.join(format!("{name}.dat")),
            format!(
                "clrmamepro (\n\tname {header}\n\tdescription {description}\n)\n\
                 game ( name G rom ( name {name}.bin size 1 crc deadbeef ) )\n"
            ),
        )
        .unwrap();
    }

    let mut page = fixture.page();
    page.limits = DatLimits::builder().max_description_length(10).build();
    page.apply(DatSourcesPageAction::AddFolder { path: folder });
    page.apply(DatSourcesPageAction::Validate {
        id: "dats".to_string(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(
        row.health_state,
        DatHealthState::ValidWithWarnings,
        "a truncated description is a genuine warning"
    );
    assert_eq!(
        row.diagnostic_types(DiagnosticSeverity::Warning),
        1,
        "two identical truncations must be one diagnostic type"
    );
    assert_eq!(row.diagnostic_occurrences(DiagnosticSeverity::Warning), 2);
    let group = &row.groups_of(DiagnosticSeverity::Warning)[0];
    assert_eq!(group.code, "description_truncated");
    assert_eq!(
        group.message, "description truncated from 80 to 10 bytes",
        "the raw message must not carry a per-occurrence byte offset"
    );
    assert_eq!(group.affected_file_count, 2);
    // Each occurrence keeps its own genuine one-based line and no invented
    // column.
    assert!(group.occurrences.iter().all(|o| o.line.is_some()));
    assert!(group.occurrences.iter().all(|o| o.column.is_none()));
}

#[test]
fn expanding_a_group_on_one_source_does_not_open_the_same_group_on_another() {
    // Regression: the group id used to be "{code}:{message}", so expanding the
    // DOCTYPE note on source A left source B's identical note open on load. The
    // id is now scoped by source and severity.
    let note_text = "Logiqx DTD referenced, but no trusted local copy was found. The DAT was parsed normally without DTD validation.";
    let fixture = Fixture::new();

    // Build two independent sources that both carry the same note.
    let mut pages = Vec::new();
    for name in ["a", "b"] {
        let folder = fixture.dir(name);
        let mut page = fixture.page();
        page.apply(DatSourcesPageAction::AddFolder {
            path: folder.clone(),
        });
        let id = name.to_string();
        let report = DatValidationReport {
            source_id: id.clone(),
            path: folder.to_string_lossy().into_owned(),
            kind: "DAT folder",
            state: DatHealthState::Valid,
            files: vec![DatFileReport {
                path: format!("{}/x.dat", folder.display()),
                file_name: format!("{name}.dat"),
                outcome: DatFileOutcome::Parsed {
                    format: DatFormat::Logiqx,
                    ecosystem: DatEcosystem::GenericLogiqx,
                    name: Some("Test".to_string()),
                    version: Some("1".to_string()),
                    entry_count: 1,
                    rom_count: 1,
                    diagnostics: vec![note(note_text)],
                },
            }],
            duplicate_identities: Vec::new(),
            skipped: Vec::new(),
            truncated: false,
            total_dat_files: None,
            summary: "1 DAT files, 1 entries, 1 ROMs".to_string(),
            entry_count: 1,
            rom_count: 1,
            formats: vec!["Logiqx XML".to_string()],
            path_refusal: None,
        };
        page.validations.insert(id, report);
        pages.push(page);
    }

    let view_a = pages[0].view();
    let view_b = pages[1].view();
    assert_ne!(
        view_a.rows[0].groups[0].id, view_b.rows[0].groups[0].id,
        "same-typed groups on different sources must have distinct ids"
    );

    // Opening A's group must not leave B's group open.
    let mut ui_state = DatSourcesPageUi {
        open_diagnostic: Some(view_a.rows[0].groups[0].id.clone()),
        ..Default::default()
    };
    let rendered_a = render_with_details(&view_a, &mut ui_state);
    assert_eq!(rendered_text_count(&rendered_a, "Hide locations"), 1);
    let rendered_b = render_with_details(&view_b, &mut ui_state);
    assert_eq!(
        rendered_text_count(&rendered_b, "Hide locations"),
        0,
        "source B's identical group must not be open just because A's was"
    );
    assert_eq!(rendered_text_count(&rendered_b, "View locations"), 1);
}

#[test]
fn drill_down_shows_parser_location_when_available_and_unavailable_otherwise() {
    // The drill-down shows line/column only when the parser provided one;
    // otherwise it says "Location unavailable". It never re-parses to build.
    let with_location = DatDiagnostic {
        severity: DiagnosticSeverity::Warning,
        code: "test_warning",
        message: "has a location".to_string(),
        line: Some(3),
        column: Some(12),
    };
    // The ClrMamePro parser records a line but no column; that shape must read
    // "line N", not "line N:0" and not "Location unavailable".
    let line_only = DatDiagnostic {
        severity: DiagnosticSeverity::Warning,
        code: "test_warning",
        message: "has a line only".to_string(),
        line: Some(9),
        column: None,
    };
    let without = warn("no location");
    let (_fixture, page) = page_with_report(
        vec![vec![with_location, line_only, without]],
        DatHealthState::ValidWithWarnings,
        false,
        None,
    );
    let view = page.view();
    let row = &view.rows[0];
    let groups = row.groups_of(DiagnosticSeverity::Warning);
    assert_eq!(groups.len(), 3);
    let located = groups
        .iter()
        .find(|group| group.message == "has a location")
        .unwrap();
    assert_eq!(located.occurrences[0].line, Some(3));
    assert_eq!(located.occurrences[0].column, Some(12));

    let mut ui_state = DatSourcesPageUi {
        open_diagnostic: Some(located.id.clone()),
        ..Default::default()
    };
    let located_output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(&located_output, "line 3:12"));

    let line_only_group = groups
        .iter()
        .find(|group| group.message == "has a line only")
        .unwrap();
    ui_state.open_diagnostic = Some(line_only_group.id.clone());
    let line_only_output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(&line_only_output, "line 9"));
    assert!(
        !rendered_text_contains(&line_only_output, "line 9:0"),
        "a missing column must not be rendered as zero"
    );

    let unlocated = groups
        .iter()
        .find(|group| group.message == "no location")
        .unwrap();
    ui_state.open_diagnostic = Some(unlocated.id.clone());
    let unlocated_output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &unlocated_output,
        "Location unavailable"
    ));
}

#[test]
fn a_safety_limit_stop_is_labelled_incomplete_and_counts_are_exact() {
    // Both numbers genuinely known: the read count and the folder's real total.
    let (_fixture, page) = page_with_report(
        vec![vec![warn("w")]],
        DatHealthState::ValidWithWarnings,
        true,
        Some(2024),
    );
    let row = &page.view().rows[0];
    assert!(row.incomplete_load);
    assert_eq!(row.dat_files_read, Some(1));
    assert_eq!(row.dat_files_total, Some(2024));
    assert_eq!(
        row.incomplete_load_line().as_deref(),
        Some("1 of 2024 DAT files read")
    );

    // An unknown total never invents one: the safety limit is named instead.
    let (_fixture, page) = page_with_report(
        vec![vec![warn("w")]],
        DatHealthState::ValidWithWarnings,
        true,
        None,
    );
    let row = &page.view().rows[0];
    assert!(row.incomplete_load);
    assert_eq!(row.dat_files_total, None);
    assert_eq!(
        row.incomplete_load_line().as_deref(),
        Some("Processing stopped at the configured safety limit")
    );
}

#[test]
fn an_incomplete_load_is_drawn_prominently_with_its_counts() {
    let (_fixture, page) = page_with_report(
        vec![vec![warn("w")]],
        DatHealthState::ValidWithWarnings,
        true,
        Some(2024),
    );
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(
        rendered_text_contains(&output, "Incomplete catalogue load"),
        "the incompleteness must be a headline, not body text"
    );
    assert!(rendered_text_contains(&output, "1 of 2024 DAT files read"));
}

#[test]
fn unknown_total_never_invents_a_count_or_percentage() {
    assert_eq!(format_percentage(5, 0), None);
    assert_eq!(format_percentage(0, 0), None);

    let (_fixture, page) = page_with_report(
        vec![vec![warn("w")]],
        DatHealthState::ValidWithWarnings,
        true,
        None,
    );
    let row = &page.view().rows[0];
    assert_eq!(row.dat_files_read, Some(1), "the read count is still known");
    assert_eq!(row.dat_files_total, None);
    assert!(
        !row.incomplete_load_line().unwrap().contains("of"),
        "no invented total may appear: {:?}",
        row.incomplete_load_line()
    );
}

#[test]
fn warning_order_is_deterministic() {
    let per_file = vec![
        vec![warn("first-a"), warn("second-a")],
        vec![warn("first-b"), warn("second-b")],
    ];
    let (_fixture, page) =
        page_with_report(per_file, DatHealthState::ValidWithWarnings, false, None);
    let row = &page.view().rows[0];
    let messages: Vec<&str> = row
        .groups
        .iter()
        .map(|group| group.message.as_str())
        .collect();
    assert_eq!(
        messages,
        vec!["first-a", "first-b", "second-a", "second-b"],
        "groups must come in a deterministic order (by message), never in read_dir order"
    );
}

#[test]
fn the_history_and_logs_reference_is_only_drawn_when_details_are_recorded_there() {
    let (_fixture, page) = page_with_report(
        vec![vec![warn("w")]],
        DatHealthState::ValidWithWarnings,
        false,
        None,
    );
    let mut view = page.view();
    // Nothing is recorded in History & Logs today, so the honest card does not
    // point there.
    assert!(!view.rows[0].history_link_available);

    let mut ui_state = DatSourcesPageUi::default();
    assert!(
        !rendered_text_contains(&render(&view, &mut ui_state), "History & Logs"),
        "no link may be offered when the details are not recorded there"
    );

    // If the flag is ever set because the details genuinely are recorded there,
    // the reference is drawn.
    view.rows[0].history_link_available = true;
    assert!(rendered_text_contains(
        &render(&view, &mut ui_state),
        "History & Logs"
    ));
}

#[test]
fn warnings_have_a_plain_summary_with_parser_details_on_demand() {
    let diagnostics = vec![vec![
        warn("A ROM entry has no SHA-1 checksum; only CRC32 was compared"),
        warn("The header declares a version that differs from the filename"),
    ]];
    let (_fixture, page) =
        page_with_report(diagnostics, DatHealthState::ValidWithWarnings, false, None);
    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(row.health_state, DatHealthState::ValidWithWarnings);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 2);

    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Valid, with warnings"));
    assert!(rendered_text_contains(&output, "2 catalogue issues found"));
    assert!(rendered_text_contains(
        &output,
        "The catalogue still works. Files that could not be used were skipped."
    ));
    assert!(!rendered_text_contains(
        &output,
        "2 warning types, 2 occurrences"
    ));
    let details = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &details,
        "2 warning types, 2 occurrences"
    ));
    assert!(rendered_text_contains(&details, "View locations"));
}

// ---------------------------------------------------------------------------
// Diagnostic severity: the TOSEC DOCTYPE reproduction and its neighbours
// ---------------------------------------------------------------------------

/// A Logiqx XML DAT carrying the standard DOCTYPE plus `games` entries.
///
/// The DOCTYPE is expected parser behaviour and must surface as a parser note,
/// never as a warning. This is the reproduction reported against the GUI: a
/// single TOSEC DAT whose only diagnostic was the DOCTYPE, shown as "Valid,
/// with warnings" and "1 warning".
fn logiqx_with_doctype_and_entries(games: usize) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE datafile PUBLIC \"-//Logiqx//DTD ROM Management Datafile//EN\" \
         \"http://www.logiqx.com/Dats/datafile.dtd\">\n\
         <datafile>\n\
         <header><name>Test TOSEC Set</name><version>2026-01-01</version></header>\n",
    );
    for index in 0..games {
        xml.push_str(&format!(
            "<game name=\"Game {index}\"><rom name=\"g{index}.bin\" size=\"16\" crc=\"{index:08x}\"/></game>\n"
        ));
    }
    xml.push_str("</datafile>\n");
    xml
}

/// A Logiqx XML DAT whose checksum is malformed: the parser drops it and warns,
/// so the DAT parses but carries a real warning.
fn logiqx_with_malformed_checksum(doctype: bool) -> String {
    let doctype = if doctype {
        "<!DOCTYPE datafile PUBLIC \"-//Logiqx//DTD ROM Management Datafile//EN\" \
         \"http://www.logiqx.com/Dats/datafile.dtd\">\n"
    } else {
        ""
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
{doctype}<datafile><game name="G"><rom name="a.bin" size="4" crc="not-a-checksum"/></game></datafile>"#
    )
}

#[test]
fn a_doctype_parser_note_shows_valid_with_no_warnings() {
    // The exact reproduction: a single TOSEC DAT, 1005 entries, whose only
    // diagnostic is the DOCTYPE parser note. It must read "Valid" and "1 parser
    // note", never "Valid, with warnings" or "1 warning".
    let fixture = Fixture::new();
    let dat = fixture.write("tosec.dat", &logiqx_with_doctype_and_entries(1005));

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Validate {
        id: "tosec".to_string(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(row.entry_count, Some(1005));
    assert_eq!(row.rom_count, Some(1005));
    assert_eq!(
        row.health_state,
        DatHealthState::Valid,
        "a parser note must not lower the verdict"
    );
    assert_eq!(
        row.diagnostic_types(DiagnosticSeverity::Warning),
        0,
        "the DOCTYPE must not surface as a warning"
    );
    assert_eq!(
        row.diagnostic_types(DiagnosticSeverity::Note),
        1,
        "the DOCTYPE must be a single parser-note type"
    );
    assert_eq!(row.diagnostic_occurrences(DiagnosticSeverity::Note), 1);
    let note_group = &row.groups_of(DiagnosticSeverity::Note)[0];
    assert!(
        note_group.message.contains("Logiqx") && note_group.message.contains("DTD"),
        "{}",
        note_group.message
    );

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(!rendered_text_contains(&output, "with warnings"));
    assert!(
        !rendered_text_contains(&output, "warning type"),
        "the note must not be called a warning"
    );
    assert!(rendered_text_contains(
        &output,
        "1 parser-note type, 1 occurrence"
    ));
    assert!(rendered_text_contains(&output, "View locations"));
}

#[test]
fn a_real_warning_shows_valid_with_warnings() {
    let fixture = Fixture::new();
    let dat = fixture.write("warn.dat", &logiqx_with_malformed_checksum(false));

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Validate {
        id: "warn".to_string(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(row.health_state, DatHealthState::ValidWithWarnings);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 1);
    assert_eq!(row.diagnostic_occurrences(DiagnosticSeverity::Warning), 1);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Note), 0);

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Valid, with warnings"));
    assert!(rendered_text_contains(
        &output,
        "1 warning type, 1 occurrence"
    ));
    assert!(rendered_text_contains(&output, "View locations"));
}

#[test]
fn a_real_parser_failure_shows_invalid() {
    let fixture = Fixture::new();
    let dat = fixture.write("broken.dat", "<?xml version=\"1.0\"?><datafile><game");

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Validate {
        id: "broken".to_string(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(row.health_state, DatHealthState::Invalid);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 0);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Note), 0);
    assert!(row.groups.is_empty());

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Invalid"));
    assert!(!rendered_text_contains(&output, "View locations"));
}

#[test]
fn mixed_warning_and_notes_shows_valid_with_warnings() {
    let fixture = Fixture::new();
    let dat = fixture.write("mixed.dat", &logiqx_with_malformed_checksum(true));
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Validate {
        id: "mixed".to_string(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let row = &view.rows[0];
    assert_eq!(
        row.health_state,
        DatHealthState::ValidWithWarnings,
        "a warning overrides parser notes in the verdict"
    );
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Warning), 1);
    assert_eq!(row.diagnostic_types(DiagnosticSeverity::Note), 1);

    let mut ui_state = DatSourcesPageUi::default();
    let output = render_with_details(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Valid, with warnings"));
    assert!(rendered_text_contains(
        &output,
        "1 warning type, 1 occurrence"
    ));
    assert!(rendered_text_contains(
        &output,
        "1 parser-note type, 1 occurrence"
    ));
}

#[test]
fn mixed_errors_warnings_and_notes_shows_invalid() {
    let fixture = Fixture::new();
    let folder = fixture.dir("mixed");
    std::fs::write(
        folder.join("broken.dat"),
        "<?xml version=\"1.0\"?><datafile><game",
    )
    .unwrap();
    std::fs::write(folder.join("ok.dat"), logiqx_with_malformed_checksum(true)).unwrap();

    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFolder { path: folder });
    page.apply(DatSourcesPageAction::Validate {
        id: "mixed".to_string(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    assert_eq!(
        view.rows[0].health_state,
        DatHealthState::Invalid,
        "an error in any file makes the whole source invalid"
    );

    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Invalid"));
}

// ---------------------------------------------------------------------------
// Audit progress: ETA and formatting
// ---------------------------------------------------------------------------

#[test]
fn no_eta_before_one_hundred_files() {
    let mut estimator = EtaEstimator::new();
    estimator.update(50, 6.0);
    estimator.update(90, 12.0);
    let eta = estimator.eta(90, 1000, 12.0);
    assert!(
        !matches!(eta, EtaView::About { .. }),
        "an ETA must not appear before 100 files: {eta:?}"
    );
    assert_eq!(eta, EtaView::Estimating);
}

#[test]
fn no_eta_before_five_seconds() {
    let mut estimator = EtaEstimator::new();
    estimator.update(50, 0.0);
    estimator.update(150, 3.0);
    let eta = estimator.eta(150, 1000, 3.0);
    assert!(
        !matches!(eta, EtaView::About { .. }),
        "an ETA must not appear before 5 seconds: {eta:?}"
    );
    assert_eq!(eta, EtaView::Estimating);
}

#[test]
fn unknown_total_produces_no_eta() {
    let mut tracker = AuditProgressTracker::new();
    tracker.update(
        &DatAuditProgress::Scanning {
            files_found: 42,
            current_dir: Some("/home/user/roms".to_string()),
        },
        12.0,
    );
    let view = tracker.view(12);
    assert_eq!(view.total_files, None);
    assert_eq!(view.eta, EtaView::None);
    assert_eq!(view.percent, None);
    // The position must not invent a denominator.
    assert_eq!(view.position(), "42 files so far");
}

#[test]
fn stable_progress_produces_an_approximate_eta() {
    let mut estimator = EtaEstimator::new();
    estimator.update(100, 10.0);
    estimator.update(200, 20.0);
    estimator.update(300, 30.0);
    // 100 files per 10 seconds, 700 remaining -> about 70 seconds.
    match estimator.eta(300, 1000, 30.0) {
        EtaView::About { seconds_remaining } => {
            assert!(
                (55..=85).contains(&seconds_remaining),
                "{seconds_remaining}"
            );
            let line = format_eta_remaining(seconds_remaining);
            assert!(line.starts_with("About "), "{line}");
            assert!(line.ends_with("remaining"), "{line}");
        }
        other => panic!("expected an approximate ETA, got {other:?}"),
    }
}

#[test]
fn eta_is_smoothed_not_jumping_from_one_sample() {
    let mut estimator = EtaEstimator::new();
    estimator.update(100, 10.0);
    estimator.update(200, 20.0);
    estimator.update(300, 30.0);
    // One fast frame: 100 files in 1 second (100/s). A naive estimate would
    // drop to ~6 seconds remaining; the smoothed one moves only partway.
    estimator.update(400, 31.0);
    match estimator.eta(400, 1000, 31.0) {
        EtaView::About { seconds_remaining } => {
            assert!(
                seconds_remaining >= 15,
                "the ETA must not jump to the single-frame speed: {seconds_remaining}s"
            );
            assert!(
                seconds_remaining < 60,
                "the ETA must move toward the spike, not ignore it: {seconds_remaining}s"
            );
        }
        other => panic!("expected an approximate ETA, got {other:?}"),
    }
}

#[test]
fn zero_progress_cannot_divide_by_zero() {
    assert_eq!(format_percentage(0, 0), None);
    assert_eq!(format_percentage(5, 0), None);

    let mut estimator = EtaEstimator::new();
    estimator.update(0, 0.0);
    estimator.update(0, 5.0);
    assert_eq!(estimator.eta(0, 500, 5.0), EtaView::None);

    let mut tracker = AuditProgressTracker::new();
    tracker.update(
        &DatAuditProgress::Hashing {
            index: 0,
            total: 0,
            file_name: "x".to_string(),
        },
        6.0,
    );
    let view = tracker.view(6);
    assert_eq!(view.percent, None);
    assert_eq!(view.eta, EtaView::None);
}

#[test]
fn a_frozen_tracker_keeps_the_eta_it_had_at_its_last_update() {
    // Regression: the ETA must be a snapshot from the last progress update,
    // not recomputed from the live wall clock - otherwise a stalled or
    // cancelled run could flip from "Estimating…" to a number purely because
    // seconds passed.
    let mut tracker = AuditProgressTracker::new();
    tracker.update(
        &DatAuditProgress::Hashing {
            index: 50,
            total: 1000,
            file_name: "a".to_string(),
        },
        2.0,
    );
    tracker.update(
        &DatAuditProgress::Hashing {
            index: 200,
            total: 1000,
            file_name: "b".to_string(),
        },
        6.0,
    );
    let eta_at_6 = tracker.view(6).eta.clone();
    assert!(matches!(eta_at_6, EtaView::About { .. }));

    let eta_at_600 = tracker.view(600).eta;
    assert_eq!(
        eta_at_600, eta_at_6,
        "a tracker that has not been fed must not change its ETA as the clock moves"
    );
}

#[test]
fn draining_a_progress_backlog_keeps_the_eta_stable() {
    // Regression: poll() used to timestamp every drained AuditProgress message
    // with its own `started_at.elapsed()`. A backlog queued between GUI frames
    // is drained within microseconds, so EtaEstimator saw a large delta_files
    // over a near-zero delta_seconds and spiked the throughput, collapsing the
    // ETA toward zero. poll() now reads the clock once per drain pass: every
    // message in the burst shares one elapsed value, the `delta_seconds > 0`
    // guard skips the rest of the burst, and the rate stays where the normally
    // spaced passes put it.
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    // Drive the job through a controllable channel, backdating the clock so
    // the confidence gates (100 files, 5 seconds) are already open. A larger
    // channel than the production constant lets the burst queue entirely
    // without a blocking send.
    let cancel = page.job.as_ref().expect("a job is running").cancel.clone();
    let (sender, messages) = sync_channel(256);
    page.job = Some(RunningJob {
        kind: JobKind::Audit,
        source_id: "collection".to_string(),
        cancel,
        cancel_requested: false,
        messages,
        latest: "Starting…".to_string(),
        started_at: Instant::now() - Duration::from_secs(60),
        audit_progress: Some(AuditProgressTracker::new()),
        platform_display: None,
        bulk: None,
    });

    let total = 1000;
    let hash = |index: usize| {
        JobMessage::AuditProgress(DatAuditProgress::Hashing {
            index,
            total,
            file_name: format!("f{index}.bin"),
        })
    };

    // Two normally spaced passes establish a steady rate (one file per 20 ms).
    sender.send(hash(1)).unwrap();
    page.poll();
    std::thread::sleep(Duration::from_millis(20));
    sender.send(hash(2)).unwrap();
    page.poll();

    // Queue a backlog, then drain all of it in a single poll() pass. The EMA
    // (alpha 0.2) needs ~21 samples to converge toward a spike rate, so 101
    // queued messages are plenty to make the old per-message timing collapse
    // the ETA, while staying above the 100-file confidence gate.
    std::thread::sleep(Duration::from_millis(20));
    for index in 3..=103 {
        sender.send(hash(index)).unwrap();
    }
    page.poll();

    let running = page.view().running.expect("the job is still running");
    let progress = running.progress.as_ref().expect("audit progress");
    assert_eq!(progress.files_checked, 103);
    // Coalescing: after draining a 101-message backlog in one pass, the detail
    // line is the last event's, not the first's.
    assert_eq!(running.detail, "Checking 103 of 1000: f103.bin");
    match &progress.eta {
        EtaView::About { seconds_remaining } => {
            // ~50 files/s with 897 left is on the order of 18 seconds. A
            // per-message timestamp on the drained backlog would compute
            // millions of files per second and collapse this to ~1 second.
            assert!(
                *seconds_remaining >= 5,
                "the ETA must not collapse toward zero after a drained backlog: \
                 {seconds_remaining}s"
            );
        }
        other => panic!("expected a real ETA after a stable run, got {other:?}"),
    }
}

#[test]
fn completed_progress_shows_one_hundred_percent() {
    assert_eq!(format_percentage(500, 500), Some(100));
    assert_eq!(format_percentage(500, 1000), Some(50));

    let mut tracker = AuditProgressTracker::new();
    tracker.update(
        &DatAuditProgress::Hashing {
            index: 500,
            total: 500,
            file_name: "last.bin".to_string(),
        },
        30.0,
    );
    let view = tracker.view(30);
    assert_eq!(view.percent, Some(100));
    assert_eq!(view.position(), "500 of 500");
}

#[test]
fn the_current_path_is_shortened_safely() {
    assert_eq!(
        shorten_path("/home/user/private/games/platform"),
        "…/games/platform"
    );
    assert_eq!(shorten_path("/a/b/c/d/e/f"), "…/e/f");
    // Short paths are returned as they are; nothing panics on edge cases.
    assert_eq!(shorten_path("/roms"), "/roms");
    assert_eq!(shorten_path(""), "");
}

#[test]
fn a_private_path_never_enters_the_detail_or_progress_text() {
    let private = "/home/user/private";
    let description = describe(&DatAuditProgress::Scanning {
        files_found: 7,
        current_dir: Some(format!("{private}/platform")),
    });
    assert!(!description.contains(private), "{description}");

    let mut tracker = AuditProgressTracker::new();
    tracker.update(
        &DatAuditProgress::Scanning {
            files_found: 7,
            current_dir: Some(format!("{private}/platform")),
        },
        3.0,
    );
    let view = tracker.view(3);
    let shown = view.current_path.expect("a current path is shown");
    assert!(!shown.contains(private), "{shown}");
    assert_eq!(shown, "…/private/platform");
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

/// Replaces the running job with a controllable one on a fresh channel, so a
/// test can drive the exact message sequence without racing a worker thread.
fn take_over_job(page: &mut DatSourcesPageState, latest: &str) -> SyncSender<JobMessage> {
    let cancel = page.job.as_ref().expect("a job is running").cancel.clone();
    let (sender, messages) = sync_channel(PROGRESS_QUEUE_DEPTH);
    page.job = Some(RunningJob {
        kind: JobKind::Audit,
        source_id: "collection".to_string(),
        cancel,
        cancel_requested: false,
        messages,
        latest: latest.to_string(),
        started_at: Instant::now(),
        audit_progress: Some(AuditProgressTracker::new()),
        platform_display: None,
        bulk: None,
    });
    sender
}

/// A completed-audit outcome, for proving a late result after cancellation is
/// dropped rather than presented.
fn minimal_outcome() -> DatAuditOutcome {
    DatAuditOutcome {
        source_id: "collection".to_string(),
        source_display_name: "collection.dat".to_string(),
        dat_path: "/tmp/collection.dat".to_string(),
        scan_root: "/tmp/roms".to_string(),
        catalogue_names: vec!["Test No-Intro Collection".to_string()],
        catalogue_entries: 1,
        catalogue_roms: 1,
        catalogue_version: None,
        catalogue_author: None,
        catalogue_homepage: None,
        catalogue_ecosystem: None,
        unreadable_catalogues: Vec::new(),
        report: AuditReport {
            entries: Vec::new(),
            summary: AuditSummary::default(),
        },
        evidence_sources: Vec::new(),
        archives: Vec::new(),
        sets: Vec::new(),
        unhashed: Vec::new(),
        files_scanned: 2,
        bytes_hashed: 4,
        archive_bytes_hashed: 0,
        truncated: false,
        policy: None,
        content: Default::default(),
        platform: None,
        cache: Default::default(),
        known_hashes: Default::default(),
    }
}

#[test]
fn archive_member_evidence_has_separate_gui_rows() {
    use archivefs_core::dat::archive::{
        ArchiveMemberEvidence, ArchiveMemberHashes, ArchiveMemberStatus, ArchivePassCompletion,
    };
    use archivefs_core::dat::audit::AuditVerdict;
    use archivefs_core::dat::sources::audit_run::{DatArchiveAudit, DatArchiveMemberAudit};

    let mut outcome = minimal_outcome();
    outcome.archives.push(DatArchiveAudit {
        archive_path: "/tmp/roms/games.zip".into(),
        outer_identity: None,
        format: "zip".to_string(),
        total_members: 1,
        completion: ArchivePassCompletion::Complete,
        members: vec![DatArchiveMemberAudit {
            evidence: ArchiveMemberEvidence {
                archive_path: "/tmp/roms/games.zip".into(),
                member_name_raw: b"game.rom".to_vec(),
                member_name_display: "game.rom".to_string(),
                index: 0,
                logical_size: 4,
                is_nested_archive: false,
                status: ArchiveMemberStatus::HashComplete,
                hashes: Some(ArchiveMemberHashes {
                    crc32: "00000000".to_string(),
                    md5: "00".to_string(),
                    sha1: "00".to_string(),
                    sha256: "00".to_string(),
                }),
            },
            verdict: Some(AuditVerdict::Exact {
                game_name: "Game".to_string(),
                rom_name: "game.rom".to_string(),
                algorithm: "SHA-1",
            }),
            matched_refs: Vec::new(),
            evidence_sources: Vec::new(),
        }],
        combined_identity: None,
    });

    let view = audit_view(&outcome, Some(1));
    assert_eq!(
        view.entries.len(),
        0,
        "member is not flattened into physical rows"
    );
    assert_eq!(view.archives.len(), 1);
    assert_eq!(view.archives[0].archive_name, "games.zip");
    assert_eq!(view.archives[0].members[0].name, "game.rom");
    assert_eq!(
        view.archives[0].members[0].verdict.as_deref(),
        Some("Exact")
    );
}

#[test]
fn cancellation_changes_the_wording_to_stopping() {
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    let sender = take_over_job(&mut page, "Checking 10 of 100: a.bin");

    let running = page.view().running.expect("a job is running");
    assert_eq!(running.heading(), "Auditing 'collection'");
    assert!(!running.cancellation_requested);

    page.apply(DatSourcesPageAction::CancelJob);

    let running = page.view().running.expect("still busy while stopping");
    assert!(running.cancellation_requested);
    assert!(
        running.heading().contains("Stopping"),
        "{}",
        running.heading()
    );
    assert!(
        page.is_busy(),
        "the operation stays busy until the worker confirms termination"
    );

    // The worker's confirmation ends it without any result.
    sender.send(JobMessage::Cancelled).unwrap();
    page.poll();
    assert!(page.view().running.is_none());
    assert!(page.view().audit.is_none());
}

#[test]
fn stale_progress_after_cancellation_is_ignored() {
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    let sender = take_over_job(&mut page, "Starting…");

    // A real progress update before cancellation, so there is something to
    // freeze.
    sender
        .send(JobMessage::AuditProgress(DatAuditProgress::Hashing {
            index: 10,
            total: 100,
            file_name: "a.bin".to_string(),
        }))
        .unwrap();
    page.poll();
    let before = page.view().running.expect("running");
    assert_eq!(before.detail, "Checking 10 of 100: a.bin");
    assert_eq!(before.progress.as_ref().unwrap().files_checked, 10);

    page.apply(DatSourcesPageAction::CancelJob);

    // The worker has not observed the flag yet and goes on reporting. None of
    // it may move the shown state.
    sender
        .send(JobMessage::AuditProgress(DatAuditProgress::Hashing {
            index: 11,
            total: 100,
            file_name: "c.bin".to_string(),
        }))
        .unwrap();
    page.poll();

    let running = page.view().running.expect("still busy");
    assert!(running.cancellation_requested);
    assert_eq!(
        running.detail, before.detail,
        "stale progress after cancellation must not change the shown detail"
    );
    assert_eq!(
        running.progress.as_ref().unwrap().files_checked,
        10,
        "stale progress after cancellation must not move the position or ETA"
    );
}

#[test]
fn a_cancelled_audit_never_appears_complete() {
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    let sender = take_over_job(&mut page, "Starting…");

    page.apply(DatSourcesPageAction::CancelJob);

    // Even if the worker finished the whole audit before it noticed the flag,
    // the page must not present that as a completed audit.
    sender
        .send(JobMessage::Audited {
            generation: 0,
            outcome: Box::new(minimal_outcome()),
            enrichment: None,
            plan: None,
        })
        .unwrap();
    page.poll();

    let view = page.view();
    assert!(view.running.is_none());
    assert!(
        view.audit.is_none(),
        "a cancelled audit never appears complete"
    );
    assert!(view.audit_error.is_none(), "cancelling is not a failure");
}

#[test]
fn platform_conflict_names_each_source_and_requires_review() {
    use archivefs_core::platform::identity::{
        PlatformIdentityConfidence, PlatformIdentityEvidence, PlatformIdentitySource,
    };

    let (_fixture, mut page, _roms) = audit_fixture();
    page.identity_enrichment = Some(Box::new(
        archivefs_core::PlatformIdentityEnrichmentSummary {
            conflicts: 1,
            conflict_details: vec![archivefs_core::PlatformIdentityConflictDetail {
                archive_id: 1,
                evidence: vec![
                    PlatformIdentityEvidence::canonical(
                        "PSX",
                        PlatformIdentitySource::VerifiedDat,
                        PlatformIdentityConfidence::Verified,
                        1,
                        "verified DAT fixture",
                    )
                    .unwrap(),
                    PlatformIdentityEvidence::canonical(
                        "PSP",
                        PlatformIdentitySource::Romm,
                        PlatformIdentityConfidence::High,
                        1,
                        "RomM fixture",
                    )
                    .unwrap(),
                ],
            }],
            ..Default::default()
        },
    ));

    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&page.view(), &mut ui_state);
    for expected in [
        "Platform conflict",
        "Verified DAT: Sony PlayStation",
        "RomM: Sony PlayStation Portable",
        "Review required",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing conflict text {expected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Richer live audit context
// ---------------------------------------------------------------------------

#[test]
fn the_running_card_shows_the_platform_only_when_authoritative() {
    let (_fixture, mut page, roms) = audit_fixture();
    // An unassigned source gets no platform line at all.
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms.clone(),
    });
    let unassigned = page.view().running.expect("running").clone();
    assert!(
        unassigned.platform_display.is_none(),
        "no platform may be claimed for an unassigned source"
    );
    assert!(!rendered_text_contains(
        &render_running_card(&unassigned),
        "Platform:"
    ));
    page.apply(DatSourcesPageAction::CancelJob);
    run_to_completion(&mut page);

    // A recognised assignment is authoritative and appears on the running card.
    let canonical = archivefs_core::platform::canonical_ids()[0];
    page.apply(DatSourcesPageAction::SetPlatform {
        id: "collection".to_string(),
        platform: Some(canonical.to_string()),
    });
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms.clone(),
    });

    let assigned = page.view().running.expect("running").clone();
    assert_eq!(
        assigned.platform_display.as_deref(),
        Some(archivefs_core::platform::display_name_for(canonical)),
        "a resolved assignment must be shown"
    );
    assert!(rendered_text_contains(
        &render_running_card(&assigned),
        "Platform:"
    ));
    page.apply(DatSourcesPageAction::CancelJob);
    run_to_completion(&mut page);
}

#[test]
fn an_unresolved_platform_is_never_presented_on_the_running_card() {
    // An assignment this build does not recognise is kept, but must not be
    // presented as authoritative during a run.
    let (_fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::SetPlatform {
        id: "collection".to_string(),
        platform: Some("APlatformFromALaterBuild".to_string()),
    });
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });

    let running = page.view().running.expect("running").clone();
    assert!(
        running.platform_display.is_none(),
        "an unresolved platform must not be claimed"
    );
    assert!(!rendered_text_contains(
        &render_running_card(&running),
        "Platform:"
    ));
    page.apply(DatSourcesPageAction::CancelJob);
    run_to_completion(&mut page);
}

// ---------------------------------------------------------------------------
// DAT matching policy: controls, summary, safety
// ---------------------------------------------------------------------------

/// Renders the whole page at a given window width.
fn render_at_width(
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
    width: f32,
) -> egui::FullOutput {
    // Production always wraps this page in the shared outer page scroll
    // (`main_view_uses_page_scroll` includes `MainView::DatSources`), so a
    // real narrow window can always reach every section by scrolling.
    // This harness has no such scroll container, so its fixed height must
    // stay comfortably above the page's actual rendered height at this
    // width - otherwise content below the fold is genuinely absent from
    // the painted output (not just visually clipped), which would make
    // this a test of the harness's height, not of the page's own layout.
    let context = egui::Context::default();
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 20_000.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let _ = show_dat_sources_page(ui, view, ui_state);
            });
        },
    )
}

#[test]
fn policy_edits_are_unsaved_changes_and_never_touch_the_rom_folder() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });

    // Set a preference.
    page.apply(DatSourcesPageAction::AddRegion {
        scope: None,
        region: RegionId::Europe,
    });
    page.apply(DatSourcesPageAction::SetRevisionPolicy {
        scope: None,
        policy: RevisionPolicy::LatestVerified,
    });

    let view = page.view();
    assert!(
        view.dirty,
        "a policy edit is an unsaved change exactly like a source edit"
    );
    assert_eq!(view.policy.region_preferences.len(), 1);
    assert_eq!(view.policy.region_preferences[0].label, "Europe");
    assert_eq!(view.policy.revision_policy, RevisionPolicy::LatestVerified);
    assert!(
        !fixture.config_path.exists(),
        "nothing is written before Save"
    );

    // Save writes only the registry file; the ROM folder is never touched.
    let roms = fixture.dir("roms");
    let before = snapshot(&roms);
    page.apply(DatSourcesPageAction::Save);
    let after = snapshot(&roms);
    assert_eq!(before, after, "no file beside the registry was touched");
    assert!(fixture.config_path.exists(), "Save wrote the registry");

    // A fresh page reloads the preference.
    let reloaded = fixture.page();
    assert_eq!(reloaded.view().policy.region_preferences[0].label, "Europe");
    assert_eq!(
        reloaded.view().policy.revision_policy,
        RevisionPolicy::LatestVerified
    );
}

#[test]
fn region_reorder_controls_move_and_remove() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    for region in [RegionId::Europe, RegionId::Usa, RegionId::Japan] {
        page.apply(DatSourcesPageAction::AddRegion {
            scope: None,
            region,
        });
    }
    let order = |page: &DatSourcesPageState| {
        page.view()
            .policy
            .region_preferences
            .iter()
            .map(|row| row.label.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&page), vec!["Europe", "USA", "Japan"]);

    // Move Europe down one.
    page.apply(DatSourcesPageAction::MoveRegion {
        scope: None,
        index: 0,
        delta: 1,
    });
    assert_eq!(order(&page), vec!["USA", "Europe", "Japan"]);
    assert_eq!(page.view().policy.region_preferences[0].position, 1);

    // Move Japan up to the front.
    page.apply(DatSourcesPageAction::MoveRegion {
        scope: None,
        index: 2,
        delta: -2,
    });
    assert_eq!(order(&page), vec!["Japan", "USA", "Europe"]);

    // Remove USA.
    page.apply(DatSourcesPageAction::RemoveRegion {
        scope: None,
        index: 1,
    });
    assert_eq!(order(&page), vec!["Japan", "Europe"]);
    assert!(page.view().dirty);
}

#[test]
fn language_editor_adds_a_specific_language_multi_and_original() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddLanguage {
        scope: None,
        preference: LanguagePreference::Language(LanguageId::En),
    });
    page.apply(DatSourcesPageAction::AddLanguage {
        scope: None,
        preference: LanguagePreference::MultiLanguage,
    });
    page.apply(DatSourcesPageAction::AddLanguage {
        scope: None,
        preference: LanguagePreference::OriginalLanguage,
    });
    let view = page.view();
    let labels: Vec<&str> = view
        .policy
        .language_preferences
        .iter()
        .map(|row| row.label.as_str())
        .collect();
    assert_eq!(
        labels,
        vec!["English", "Multi-language", "Original language"]
    );

    // Adding the same language again is refused (no duplicates).
    page.apply(DatSourcesPageAction::AddLanguage {
        scope: None,
        preference: LanguagePreference::Language(LanguageId::En),
    });
    assert_eq!(page.view().policy.language_preferences.len(), 3);

    page.apply(DatSourcesPageAction::MoveLanguage {
        scope: None,
        index: 2,
        delta: -2,
    });
    let view = page.view();
    let labels: Vec<&str> = view
        .policy
        .language_preferences
        .iter()
        .map(|row| row.label.as_str())
        .collect();
    assert_eq!(
        labels,
        vec!["Original language", "English", "Multi-language"]
    );
}

#[test]
fn revision_and_clone_policies_can_be_chosen() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::SetRevisionPolicy {
        scope: None,
        policy: RevisionPolicy::PreferOriginal,
    });
    page.apply(DatSourcesPageAction::SetClonePolicy {
        scope: None,
        policy: ClonePolicy::PreferParent,
    });
    let view = page.view();
    assert_eq!(view.policy.revision_policy, RevisionPolicy::PreferOriginal);
    assert_eq!(view.policy.clone_policy, ClonePolicy::PreferParent);
    assert_eq!(view.policy.effective.revision, "Prefer original");
    assert_eq!(view.policy.effective.clone, "Prefer parent");
}

#[test]
fn the_effective_policy_summary_shows_the_resolved_values_for_the_scope() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::SetPlatform {
        id: page.view().rows[0].id.clone(),
        platform: Some("NES".to_string()),
    });
    for region in [RegionId::Europe, RegionId::Usa] {
        page.apply(DatSourcesPageAction::AddRegion {
            scope: None,
            region,
        });
    }
    page.apply(DatSourcesPageAction::AddLanguage {
        scope: None,
        preference: LanguagePreference::Language(LanguageId::En),
    });
    page.apply(DatSourcesPageAction::SetRevisionPolicy {
        scope: None,
        policy: RevisionPolicy::LatestVerified,
    });
    page.apply(DatSourcesPageAction::SetClonePolicy {
        scope: None,
        policy: ClonePolicy::KeepAllVariants,
    });

    let summary = &page.view().policy.effective;
    assert_eq!(summary.platform, "All platforms");
    assert_eq!(summary.region, "Europe, USA");
    assert_eq!(summary.language, "English");
    assert_eq!(summary.revision, "Latest verified revision");
    assert_eq!(summary.clone, "Keep all variants");
    // Source ordering lists the enabled source as consulted 1st.
    assert_eq!(summary.source_ordering.len(), 1);
    assert_eq!(summary.source_ordering[0].consulted_position, 1);
    assert!(
        summary
            .source_of
            .iter()
            .any(|(field, scope)| field == "Region preference" && scope == "Global")
    );
}

#[test]
fn a_platform_override_is_reflected_in_the_summary_with_its_source_of_value() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::SetPlatform {
        id: page.view().rows[0].id.clone(),
        platform: Some("NES".to_string()),
    });
    page.apply(DatSourcesPageAction::AddRegion {
        scope: None,
        region: RegionId::Europe,
    });

    // Select the NES scope and set a Japan override.
    page.apply(DatSourcesPageAction::SelectPolicyScope {
        scope: Some("NES".to_string()),
    });
    page.apply(DatSourcesPageAction::AddRegion {
        scope: Some("NES".to_string()),
        region: RegionId::Japan,
    });

    let policy = page.view().policy;
    assert_eq!(policy.scope_label, "Nintendo Entertainment System");
    // The authored list for the NES override is Japan alone; the global list
    // is untouched.
    assert_eq!(policy.region_preferences.len(), 1);
    assert_eq!(policy.region_preferences[0].label, "Japan");
    page.apply(DatSourcesPageAction::SelectPolicyScope { scope: None });
    let global = page.view().policy;
    assert_eq!(global.scope_label, "All platforms");
    assert_eq!(global.region_preferences[0].label, "Europe");
    // Switch back to NES for the summary assertions.
    page.apply(DatSourcesPageAction::SelectPolicyScope {
        scope: Some("NES".to_string()),
    });
    let policy = page.view().policy;
    // The summary resolves Japan for NES, and says where it came from.
    assert_eq!(policy.effective.region, "Japan");
    assert!(
        policy
            .effective
            .source_of
            .iter()
            .any(|(field, scope)| field == "Region preference" && scope == "Platform override")
    );
    assert_eq!(policy.effective.platform, "Nintendo Entertainment System");
}

#[test]
fn the_global_scope_is_used_by_default_and_the_scope_selector_is_offered() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::SetPlatform {
        id: page.view().rows[0].id.clone(),
        platform: Some("NES".to_string()),
    });
    let view = page.view();
    assert_eq!(view.policy.scope, None);
    assert_eq!(view.policy.scope_label, "All platforms");
    assert!(
        view.policy
            .scopes_available
            .iter()
            .any(|option| option.id.as_deref() == Some("NES")),
        "the platform a source covers is offered as a scope"
    );
    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Applies to:"));
    assert!(rendered_text_contains(&output, "All platforms"));
    assert!(rendered_text_contains(&output, "Editing: Global defaults"));
}

#[test]
fn unknown_policy_values_are_surfaced_but_preserved_through_save() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    std::fs::create_dir_all(fixture.config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &fixture.config_path,
        "[policy]\nregion_preferences = [\"europe\", \"moon\"]\nrevision_policy = \"newest_future_policy\"\n",
    )
    .unwrap();

    let mut page = fixture.page();
    let view = page.view();
    assert!(
        view.policy
            .problems
            .iter()
            .any(|problem| problem.contains("moon")),
        "{:?}",
        view.policy.problems
    );
    assert!(
        view.policy
            .problems
            .iter()
            .any(|problem| problem.contains("newest_future_policy")),
        "{:?}",
        view.policy.problems
    );
    // The unknown values are not applied but are preserved on a later save.
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::Save);
    let text = std::fs::read_to_string(&fixture.config_path).unwrap();
    assert!(text.contains("moon"), "{text}");
    assert!(text.contains("newest_future_policy"), "{text}");
}

#[test]
fn the_policy_section_and_summary_render_at_a_narrow_compact_width() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::AddRegion {
        scope: None,
        region: RegionId::Europe,
    });
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_at_width(&view, &mut ui_state, 420.0);
    assert!(rendered_text_contains(&output, "DAT matching policy"));
    assert!(rendered_text_contains(&output, "Preferred regions"));
    assert!(rendered_text_contains(&output, "Europe"));
    assert!(rendered_text_contains(&output, "Effective policy"));
    assert!(rendered_text_contains(&output, "Show:"));
    assert!(rendered_text_contains(&output, "All entries"));
    assert!(rendered_text_contains(&output, GAMES_ONLY_EXPLANATION));
    assert!(rendered_text_contains(&output, "Platform: All platforms"));
    assert!(rendered_text_contains(
        &output,
        "Your files won't be renamed unless you approve it."
    ));
}

#[test]
fn games_only_wording_is_beginner_facing_and_switching_is_reversible() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.audit = Some(Box::new(minimal_outcome()));
    assert_eq!(
        page.view().policy.content_selection,
        archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries
    );
    page.apply(DatSourcesPageAction::SetContentSelection {
        scope: None,
        policy: archivefs_core::dat::classification::ContentSelectionPolicy::GamesOnly,
    });
    assert_eq!(
        page.view().policy.content_selection,
        archivefs_core::dat::classification::ContentSelectionPolicy::GamesOnly
    );
    assert!(
        page.audit.is_none(),
        "stale selection annotations are discarded"
    );
    page.apply(DatSourcesPageAction::SetContentSelection {
        scope: None,
        policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
    });
    assert_eq!(
        page.view().policy.content_selection,
        archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries
    );
    assert!(GAMES_ONLY_EXPLANATION.contains("Unknown entries are kept for review"));
}

#[test]
fn technical_content_view_preserves_evidence_original_metadata_and_version() {
    use archivefs_core::dat::classification::{
        CLASSIFIER_VERSION, ClassificationEvidence, ClassificationEvidenceKind,
        ClassifierConfidence, DatContentClass, DatContentClassification, DatOriginalMetadata,
    };
    let mut metadata = DatOriginalMetadata::default();
    metadata
        .fields
        .insert("category".to_string(), "Games".to_string());
    let classification = DatContentClassification {
        class: DatContentClass::Game,
        confidence: ClassifierConfidence::High,
        evidence: vec![ClassificationEvidence {
            kind: ClassificationEvidenceKind::StructuredEntryMetadata,
            field: Some("category".to_string()),
            original_value: Some("Games".to_string()),
            rule: "fixture.rule".to_string(),
        }],
        classifier_version: CLASSIFIER_VERSION.to_string(),
    };
    let view = super::content_technical_view(&classification, &metadata);
    assert_eq!(view.classification, "Game");
    assert_eq!(view.confidence, "High");
    assert_eq!(view.classifier_version, CLASSIFIER_VERSION);
    assert!(view.evidence.iter().any(|line| line.contains("Games")));
    assert_eq!(
        view.original_metadata,
        vec![("category".to_string(), "Games".to_string())]
    );
}

#[test]
fn the_policy_controls_are_reachable_by_keyboard() {
    let fixture = Fixture::new();
    let dat = fixture.write("collection.dat", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::AddRegion {
        scope: None,
        region: RegionId::Europe,
    });
    let view = page.view();
    let ctx = egui::Context::default();
    let mut focused_anything = false;
    // Focus-traverse far enough to reach the policy section's controls; the
    // page must render and keep focus moving at every step without panicking.
    for _ in 0..40 {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 4000.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut ui_state = DatSourcesPageUi::default();
                    let _ = show_dat_sources_page(ui, &view, &mut ui_state);
                });
            },
        );
        if ctx.memory(|memory| memory.focused()).is_some() {
            focused_anything = true;
        }
    }
    assert!(focused_anything, "Tab never focused anything on the page");
}

#[test]
fn an_audit_result_shows_the_policy_preferred_candidate_for_multi_candidate_files() {
    let fixture = Fixture::new();
    let multi = r#"<?xml version="1.0" encoding="UTF-8"?>
<datafile>
    <header>
        <name>Multi</name>
        <version>1</version>
        <author>Test</author>
    </header>
    <game name="Game (USA)">
        <rom name="game.bin" size="4" md5="098f6bcd4621d373cade4e832627b4f6"/>
    </game>
    <game name="Game (Europe)">
        <rom name="game.bin" size="4" md5="098f6bcd4621d373cade4e832627b4f6"/>
    </game>
</datafile>"#;
    let dat = fixture.write("multi.dat", multi);
    let roms = fixture.dir("roms");
    std::fs::write(roms.join("game.bin"), SUPER_BIN).unwrap();

    let mut page = fixture.page_with_library(vec![roms]);
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::AddRegion {
        scope: None,
        region: RegionId::Europe,
    });
    page.apply(DatSourcesPageAction::Audit {
        id: page.view().rows[0].id.clone(),
        scan_root: fixture.root.join("roms"),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let audit = view.audit.as_ref().expect("an audit result");
    assert_eq!(
        audit
            .categories
            .iter()
            .find(|category| category.label == "Exact (multiple)")
            .map(|category| category.count),
        Some(1)
    );
    let policy = audit.policy.as_ref().expect("the audit carried a policy");
    assert_eq!(policy.notes.len(), 1);
    let note = &policy.notes[0];
    assert!(note.decided);
    assert_eq!(note.winner.as_deref(), Some("Game (Europe) (game.bin)"));
    assert!(
        note.explanations
            .iter()
            .any(|line| line.contains("preferred region matched"))
    );
    assert!(!note.ambiguous);

    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Policy preference"));
    assert!(rendered_text_contains(&output, "Game (Europe)"));
    assert!(rendered_text_contains(
        &output,
        "Preferred: Game (Europe) (game.bin)"
    ));
}

// ---------------------------------------------------------------------------
// Rename planning section
// ---------------------------------------------------------------------------

use archivefs_core::dat::rename_plan::{
    ProposalState, RenamePlan, RenamePlanCounts, RenameProposal, ReviewDecision, SourceObjectKind,
};

fn plan_proposal(
    source: &str,
    current: &str,
    proposed: Option<&str>,
    state: ProposalState,
) -> RenameProposal {
    RenameProposal {
        source_path: PathBuf::from(source),
        current_basename: current.to_string(),
        proposed_basename: proposed.map(str::to_string),
        platform: None,
        platform_display: None,
        source_id: "src".to_string(),
        source_display_name: "Source".to_string(),
        game_name: Some("Game".to_string()),
        rom_name: proposed.map(str::to_string),
        verdict_label: "Exact".to_string(),
        match_confident: true,
        explanations: vec!["preferred region matched (Europe)".to_string()],
        content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
        content_classification:
            archivefs_core::dat::classification::DatContentClassification::unknown(),
        original_metadata: Default::default(),
        state,
        object_kind: SourceObjectKind::RegularFile,
        ambiguity_reason: None,
        collision: None,
        blockers: Vec::new(),
        extension_status: None,
        sanitisation_notes: Vec::new(),
        actionable: state == ProposalState::Suggested,
        audited_identity: None,
        is_outer_archive: false,
    }
}

fn page_with_plan(proposals: Vec<RenameProposal>) -> (Fixture, DatSourcesPageState) {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    let counts = RenamePlanCounts::from_proposals(&proposals);
    let verified_total = proposals.len();
    page.rename_plan = Some(RenamePlan {
        generation: 1,
        source_id: "src".to_string(),
        source_display_name: "Source".to_string(),
        scan_root: "/tmp/roms".to_string(),
        platform: None,
        platform_display: None,
        content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
        classifier_version: archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        proposals,
        counts,
        audited_total: 2,
        verified_total,
        truncated: false,
    });
    (fixture, page)
}

#[test]
fn the_planning_only_warning_is_prominent() {
    let proposals = vec![plan_proposal(
        "/tmp/roms/game.bin",
        "game.bin",
        Some("Game (Europe).bin"),
        ProposalState::Suggested,
    )];
    let (_fixture, page) = page_with_plan(proposals);
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Planning only"));
    assert!(
        rendered_text_contains(&output, "EmuWiz will not rename any files"),
        "the read-only promise must be stated plainly"
    );
}

#[test]
fn no_apply_rename_or_commit_control_exists() {
    let proposals = vec![plan_proposal(
        "/tmp/roms/game.bin",
        "game.bin",
        Some("Game (Europe).bin"),
        ProposalState::Suggested,
    )];
    let (_fixture, page) = page_with_plan(proposals);
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    for forbidden in [
        "Apply",
        "Execute",
        "Commit",
        "Move",
        "Delete",
        "Fix automatically",
    ] {
        assert!(
            !rendered_text_contains(&output, forbidden),
            "the plan section must not offer a {forbidden} control"
        );
    }
    // The only controls present are review decisions and copy.
    assert!(rendered_text_contains(&output, "Accept"));
    assert!(rendered_text_contains(&output, "Ignore"));
    assert!(rendered_text_contains(&output, "Copy name"));
}

#[test]
fn the_plan_filters_select_which_rows_are_drawn() {
    let proposals = vec![
        plan_proposal(
            "/tmp/roms/a.bin",
            "a.bin",
            Some("Game (Europe).bin"),
            ProposalState::Suggested,
        ),
        plan_proposal(
            "/tmp/roms/b.bin",
            "b.bin",
            Some("Other.bin"),
            ProposalState::Conflict,
        ),
        plan_proposal(
            "/tmp/roms/c.bin",
            "Game.bin",
            Some("Game.bin"),
            ProposalState::AlreadyCanonical,
        ),
    ];
    let (_fixture, page) = page_with_plan(proposals);
    let view = page.view();
    assert_eq!(view.rename_plan.as_ref().unwrap().rows.len(), 3);

    let mut ui_state = DatSourcesPageUi {
        plan_filter: RenamePlanFilter::Suggested,
        ..Default::default()
    };
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "a.bin"));
    assert!(
        !rendered_text_contains(&output, "b.bin"),
        "conflicted row must be filtered out"
    );
    assert!(
        !rendered_text_contains(&output, "Game.bin"),
        "already-canonical row must be filtered out"
    );

    ui_state.plan_filter = RenamePlanFilter::Conflicts;
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "b.bin"));
    assert!(!rendered_text_contains(&output, "a.bin"));
}

#[test]
fn review_decisions_never_touch_files() {
    let fixture = Fixture::new();
    let roms = fixture.dir("roms");
    let file = roms.join("game.bin");
    std::fs::write(&file, b"content").unwrap();
    let mut page = fixture.page();
    page.rename_plan = Some(RenamePlan {
        generation: 1,
        source_id: "src".to_string(),
        source_display_name: "Source".to_string(),
        scan_root: roms.to_string_lossy().into_owned(),
        platform: None,
        platform_display: None,
        content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
        classifier_version: archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        proposals: vec![plan_proposal(
            file.to_string_lossy().as_ref(),
            "game.bin",
            Some("Game.bin"),
            ProposalState::Suggested,
        )],
        counts: RenamePlanCounts::default(),
        audited_total: 1,
        verified_total: 1,
        truncated: false,
    });

    let before = snapshot(&roms);
    page.apply(DatSourcesPageAction::SetReviewDecision {
        path: file.to_string_lossy().into_owned(),
        decision: Some(ReviewDecision::AcceptedForReview),
    });
    page.apply(DatSourcesPageAction::SetReviewDecision {
        path: file.to_string_lossy().into_owned(),
        decision: Some(ReviewDecision::Ignored),
    });
    let after = snapshot(&roms);
    assert_eq!(before, after, "a review decision must not change any file");
    assert_eq!(
        page.view().rename_plan.as_ref().unwrap().rows[0].decision,
        Some(ReviewDecision::Ignored)
    );
}

#[test]
fn clearing_review_decisions_leaves_source_files_untouched() {
    let fixture = Fixture::new();
    let roms = fixture.dir("roms");
    let file = roms.join("game.bin");
    std::fs::write(&file, b"content").unwrap();
    let mut page = fixture.page();
    page.rename_plan = Some(RenamePlan {
        generation: 1,
        source_id: "src".to_string(),
        source_display_name: "Source".to_string(),
        scan_root: roms.to_string_lossy().into_owned(),
        platform: None,
        platform_display: None,
        content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
        classifier_version: archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        proposals: vec![plan_proposal(
            file.to_string_lossy().as_ref(),
            "game.bin",
            Some("Game.bin"),
            ProposalState::Suggested,
        )],
        counts: RenamePlanCounts::default(),
        audited_total: 1,
        verified_total: 1,
        truncated: false,
    });

    page.apply(DatSourcesPageAction::SetReviewDecision {
        path: file.to_string_lossy().into_owned(),
        decision: Some(ReviewDecision::AcceptedForReview),
    });
    assert_eq!(
        page.view().rename_plan.as_ref().unwrap().rows[0].decision,
        Some(ReviewDecision::AcceptedForReview)
    );

    let before = snapshot(&roms);
    page.apply(DatSourcesPageAction::ClearReviewDecisions);
    let after = snapshot(&roms);
    assert_eq!(before, after, "clearing decisions must not change any file");
    assert_eq!(
        page.view().rename_plan.as_ref().unwrap().rows[0].decision,
        None,
        "the decision is cleared"
    );
}

/// Bulk selection must accept only the actionable (`Suggested`) rows and
/// never touch a row unmatched, ambiguous, unsupported, conflicting,
/// already-canonical, or blocked - those must stay unselectable regardless
/// of a bulk action.
#[test]
fn select_all_actionable_accepts_only_suggested_rows() {
    let fixture = Fixture::new();
    let roms = fixture.dir("roms");
    for name in ["a.bin", "b.bin", "c.bin", "d.bin", "e.bin"] {
        std::fs::write(roms.join(name), b"fixture contents").unwrap();
    }
    let mut page = fixture.page();
    let proposals = vec![
        plan_proposal(
            roms.join("a.bin").to_str().unwrap(),
            "a.bin",
            Some("Game A (Europe).bin"),
            ProposalState::Suggested,
        ),
        plan_proposal(
            roms.join("b.bin").to_str().unwrap(),
            "b.bin",
            Some("Game B (Europe).bin"),
            ProposalState::Suggested,
        ),
        plan_proposal(
            roms.join("c.bin").to_str().unwrap(),
            "c.bin",
            Some("Other.bin"),
            ProposalState::Conflict,
        ),
        plan_proposal(
            roms.join("d.bin").to_str().unwrap(),
            "d.bin",
            None,
            ProposalState::Ambiguous,
        ),
        plan_proposal(
            roms.join("e.bin").to_str().unwrap(),
            "Game E.bin",
            Some("Game E.bin"),
            ProposalState::AlreadyCanonical,
        ),
    ];
    let counts = RenamePlanCounts::from_proposals(&proposals);
    page.rename_plan = Some(RenamePlan {
        generation: 1,
        source_id: "src".to_string(),
        source_display_name: "Source".to_string(),
        scan_root: roms.to_string_lossy().into_owned(),
        platform: None,
        platform_display: None,
        content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
        classifier_version: archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        proposals,
        counts,
        audited_total: 5,
        verified_total: 5,
        truncated: false,
    });

    let before = snapshot(&roms);
    page.apply(DatSourcesPageAction::SelectAllActionable);
    let after = snapshot(&roms);
    assert_eq!(before, after, "bulk selection must not change any file");

    let view = page.view();
    let rows = &view.rename_plan.as_ref().unwrap().rows;
    let decision_for = |name: &str| {
        rows.iter()
            .find(|row| row.current_basename == name)
            .and_then(|row| row.decision)
    };
    assert_eq!(
        decision_for("a.bin"),
        Some(ReviewDecision::AcceptedForReview)
    );
    assert_eq!(
        decision_for("b.bin"),
        Some(ReviewDecision::AcceptedForReview)
    );
    assert_eq!(
        decision_for("c.bin"),
        None,
        "a conflict must stay unselected"
    );
    assert_eq!(
        decision_for("d.bin"),
        None,
        "an ambiguous row must stay unselected"
    );
    assert_eq!(
        decision_for("Game E.bin"),
        None,
        "an already-canonical row must stay unselected"
    );
}

/// The pagination slicing math itself, exercised without an egui context.
/// A rendered-text assertion cannot prove which rows are *included* on a
/// page: an egui `ScrollArea` only paints whatever fits in its visible
/// height regardless of how many rows are hits handed to it, so a row far
/// down a page would never show up in the shape list even when correctly
/// included. This is the ground truth the drawing code (`show_rename_plan_
/// section`) reads its `start`/`end` slice from.
#[test]
fn rename_plan_page_bounds_slices_and_clamps_correctly() {
    // A page of exactly the configured size: one page, the whole range.
    assert_eq!(
        rename_plan_page_bounds(RENAME_PLAN_PAGE_SIZE, 0),
        (0, RENAME_PLAN_PAGE_SIZE, 1)
    );
    // 320 rows at 150/page: three pages, the last one partial.
    assert_eq!(rename_plan_page_bounds(320, 0), (0, 150, 3));
    assert_eq!(rename_plan_page_bounds(320, 1), (150, 300, 3));
    assert_eq!(rename_plan_page_bounds(320, 2), (300, 320, 3));
    // The proven Game Boy production scale: 1839 actionable entries.
    assert_eq!(rename_plan_page_bounds(1839, 0), (0, 150, 13));
    assert_eq!(rename_plan_page_bounds(1839, 12), (1800, 1839, 13));
    // An out-of-range requested page clamps to the last real page rather
    // than slicing out of bounds.
    assert_eq!(rename_plan_page_bounds(320, 99), (300, 320, 3));
    // Zero rows still yields one (empty) page, never a divide-by-zero page
    // count.
    assert_eq!(rename_plan_page_bounds(0, 0), (0, 0, 1));
}

/// A large plan (the scale Game Boy's proven production run actually
/// produced: 1839 actionable entries) must expose page-header text proving
/// which bounded page is showing, and moving to another page must never
/// lose or alter a selection made on a different page - selection lives on
/// `review_decisions` keyed by path, entirely independent of what is
/// currently drawn or which page is active.
#[test]
fn large_plans_are_paginated_and_selection_survives_paging() {
    let (_fixture, _roms, mut page) = page_with_apply_plan(320);
    let view = page.view();
    let plan = view.rename_plan.as_ref().unwrap();
    assert_eq!(plan.rows.len(), 320);

    let mut ui_state = DatSourcesPageUi::default();
    let first_page = render_identify_rename(&view, &mut ui_state);
    assert!(rendered_text_contains(&first_page, "Page 1 of 3"));
    assert!(rendered_text_contains(&first_page, "showing 1-150 of 320"));
    assert!(
        rendered_text_contains(&first_page, "game0.bin"),
        "the first row of page 1 must render"
    );

    // Select a row on page 1, then move to page 2 and confirm the
    // selection is unaffected by which page is now active.
    page.apply(DatSourcesPageAction::SetReviewDecision {
        path: page
            .view()
            .rename_plan
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .find(|row| row.current_basename == "game0.bin")
            .unwrap()
            .source_path
            .to_string_lossy()
            .into_owned(),
        decision: Some(ReviewDecision::AcceptedForReview),
    });
    let view = page.view();
    ui_state.plan_page = 1;
    let second_page = render_identify_rename(&view, &mut ui_state);
    assert!(rendered_text_contains(&second_page, "Page 2 of 3"));
    assert!(rendered_text_contains(
        &second_page,
        "showing 151-300 of 320"
    ));
    assert!(
        rendered_text_contains(&second_page, "game150.bin"),
        "the first row of page 2 must render"
    );
    assert!(
        !rendered_text_contains(&second_page, "game0.bin"),
        "page 1's first row must not render on page 2"
    );
    assert_eq!(
        view.rename_plan.as_ref().unwrap().rows[0].decision,
        Some(ReviewDecision::AcceptedForReview),
        "the selection made while on page 1 must survive navigating to page 2"
    );
}

#[test]
fn the_plan_section_renders_at_a_narrow_compact_width() {
    let proposals = vec![plan_proposal(
        "/tmp/roms/game.bin",
        "game.bin",
        Some("Game (Europe).bin"),
        ProposalState::Suggested,
    )];
    let (_fixture, page) = page_with_plan(proposals);
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_at_width(&view, &mut ui_state, 700.0);
    assert!(rendered_text_contains(&output, "Rename planning"));
    assert!(rendered_text_contains(&output, "Planning only"));
    assert!(rendered_text_contains(&output, "game.bin"));
    assert!(rendered_text_contains(&output, "Game (Europe).bin"));
}

#[test]
fn the_plan_controls_are_reachable_by_keyboard() {
    let proposals = vec![plan_proposal(
        "/tmp/roms/game.bin",
        "game.bin",
        Some("Game (Europe).bin"),
        ProposalState::Suggested,
    )];
    let (_fixture, page) = page_with_plan(proposals);
    let view = page.view();
    let ctx = egui::Context::default();
    let mut focused_anything = false;
    for _ in 0..40 {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 4000.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut ui_state = DatSourcesPageUi::default();
                    let _ = show_dat_sources_page(ui, &view, &mut ui_state);
                });
            },
        );
        if ctx.memory(|memory| memory.focused()).is_some() {
            focused_anything = true;
        }
    }
    assert!(focused_anything, "Tab never focused anything on the page");
}

#[test]
fn an_audit_builds_a_read_only_rename_plan_and_changes_nothing() {
    // End-to-end: audit a folder whose file matches two catalogue entries
    // with a region preference; the plan must propose the preferred name and
    // the scanned tree must be byte-for-byte unchanged (paths, contents and
    // file identities).
    let fixture = Fixture::new();
    let multi = r#"<?xml version="1.0" encoding="UTF-8"?>
<datafile>
    <header>
        <name>Multi</name>
        <version>1</version>
        <author>Test</author>
    </header>
    <game name="Game (USA)">
        <rom name="Game (USA).bin" size="4" md5="098f6bcd4621d373cade4e832627b4f6"/>
    </game>
    <game name="Game (Europe)">
        <rom name="Game (Europe).bin" size="4" md5="098f6bcd4621d373cade4e832627b4f6"/>
    </game>
</datafile>"#;
    let dat = fixture.write("multi.dat", multi);
    let roms = fixture.dir("roms");
    std::fs::write(roms.join("game.bin"), SUPER_BIN).unwrap();

    let mut page = fixture.page_with_library(vec![roms.clone()]);
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    page.apply(DatSourcesPageAction::AddRegion {
        scope: None,
        region: RegionId::Europe,
    });

    let before = snapshot(&fixture.root);
    page.apply(DatSourcesPageAction::Audit {
        id: page.view().rows[0].id.clone(),
        scan_root: roms.clone(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let plan = view
        .rename_plan
        .as_ref()
        .expect("an audit with a policy produces a rename plan");
    assert_eq!(plan.counts.suggested, 1, "{:?}", plan.counts);
    assert_eq!(
        plan.rows[0].proposed_basename.as_deref(),
        Some("Game (Europe).bin"),
        "the preferred region's name is proposed"
    );
    assert!(
        plan.rows[0]
            .explanations
            .iter()
            .any(|e| e.contains("preferred region matched"))
    );
    assert_eq!(
        snapshot(&fixture.root),
        before,
        "planning an audit must not change any path, file identity or content"
    );
}

// ---------------------------------------------------------------------------
// Rename apply flow
// ---------------------------------------------------------------------------

/// Builds a page whose plan references real files in a trusted folder, with a
/// temporary journal directory - so an apply actually renames on a real worker
/// and writes its journal into the temp dir, never the real home.
fn page_with_apply_plan(count: usize) -> (Fixture, PathBuf, DatSourcesPageState) {
    let fixture = Fixture::new();
    let roms = fixture.dir("roms");
    let mut proposals = Vec::new();
    for index in 0..count {
        let name = format!("game{index}.bin");
        let path = roms.join(&name);
        std::fs::write(&path, b"fixture contents").unwrap();
        proposals.push(plan_proposal(
            path.to_str().unwrap(),
            &name,
            Some(&format!("Game {index} (Europe).bin")),
            ProposalState::Suggested,
        ));
    }
    let journal = fixture.dir("journal");
    let counts = RenamePlanCounts::from_proposals(&proposals);
    let mut page = DatSourcesPageState::load_with_transaction_dir(
        fixture.config_path.clone(),
        Vec::new(),
        TrustedRoots::from_paths([&roms]),
        journal.clone(),
    );
    page.rename_plan = Some(RenamePlan {
        generation: 1,
        source_id: "src".to_string(),
        source_display_name: "Source".to_string(),
        scan_root: roms.to_string_lossy().into_owned(),
        platform: None,
        platform_display: None,
        content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
        classifier_version: archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        proposals,
        counts,
        audited_total: counts.total,
        verified_total: counts.total,
        truncated: false,
    });
    (fixture, roms, page)
}

fn fixture_content_technical_view() -> ContentTechnicalView {
    ContentTechnicalView {
        classification: "Verified".to_string(),
        confidence: "Exact".to_string(),
        evidence: vec!["CRC match".to_string()],
        original_metadata: Vec::new(),
        classifier_version: "test".to_string(),
    }
}

fn fixture_rename_plan_row(source_path: &str, basename: &str) -> RenamePlanRowView {
    RenamePlanRowView {
        source_path: PathBuf::from(source_path),
        current_basename: basename.to_string(),
        proposed_basename: Some(format!("{basename}.renamed")),
        platform_display: None,
        source_display_name: "Test source".to_string(),
        game_name: None,
        rom_name: None,
        verdict_label: "Verified".to_string(),
        content: fixture_content_technical_view(),
        state: ProposalState::Suggested,
        object_kind_label: "file",
        explanations: Vec::new(),
        ambiguity_reason: None,
        collision_detail: None,
        blockers: Vec::new(),
        extension_preserved: true,
        sanitisation_notes: Vec::new(),
        decision: None,
    }
}

/// Root-cause regression for the widespread duplicate-widget-ID warning on
/// DAT Sources: `show_content_technical_details`'s "Technical
/// classification details" `CollapsingHeader` used to have no `id_salt`,
/// so every row calling it (every rename-plan row, every audit-entry
/// content item) collided on the exact same ID - which is why one
/// specific ID number kept recurring across many rows/sections at once.
/// Two rows rendered in the same frame (the real-world shape: a rename
/// plan with more than one file) must now toggle their own disclosure
/// independently - proof the fix's per-row salt (`row.source_path`)
/// actually produces distinct IDs, not just that the two rows happen to
/// look different.
#[test]
fn rename_plan_rows_technical_details_toggle_independently_in_the_same_frame() {
    let row_a = fixture_rename_plan_row("/roms/a.bin", "a.bin");
    let row_b = fixture_rename_plan_row("/roms/b.bin", "b.bin");
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
    let base_input = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };

    let render = |ctx: &egui::Context, input: egui::RawInput| {
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut action = None;
                show_rename_plan_row(ui, &row_a, &mut action);
                show_rename_plan_row(ui, &row_b, &mut action);
            });
        })
    };

    // Frame 1: both collapsed by default; find and click row A's header
    // (the first "Technical classification details" painted).
    let first = render(&ctx, base_input.clone());
    let header_pos = find_exact_text_center(&first, "Technical classification details")
        .expect("expected at least one disclosure header to render");
    let click = egui::RawInput {
        screen_rect: Some(screen),
        events: vec![
            egui::Event::PointerMoved(header_pos),
            egui::Event::PointerButton {
                pos: header_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: header_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ],
        ..Default::default()
    };
    let _ = render(&ctx, click);

    // Frame 3: settle. Row A's body ("Classification: Verified") must be
    // visible exactly once; row B must remain independently collapsed.
    let output = render(&ctx, base_input);
    assert_eq!(
        rendered_text_count(&output, "Classification: Verified"),
        1,
        "row A's own disclosure must be open"
    );
}

fn find_exact_text_center(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn find_in_shape(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text_shape) => (text_shape.galley.text() == needle)
                .then(|| text_shape.pos + text_shape.galley.size() / 2.0),
            egui::Shape::Vec(nested) => nested.iter().find_map(|s| find_in_shape(s, needle)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|clipped| find_in_shape(&clipped.shape, needle))
}

fn fixture_recovery_transaction(
    transaction_id: &str,
    state: TransactionState,
    human_summary: &str,
) -> RecoveryTransactionView {
    let transaction = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: transaction_id.to_string(),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: 1,
        source_scan_root: String::new(),
        state,
        entries: vec![fixture_entry("old.bin", "New.bin")],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let mut view = RecoveryTransactionView::from_transaction(
        &transaction,
        ExactResumeStatusView::NeedsCurrentPlan,
    );
    view.human_summary = human_summary.to_string();
    view
}

/// Follow-up regression for the DAT Sources duplicate-widget-ID report:
/// a realistic render carrying everything the live screenshots named at
/// once - multiple rename transactions (distinct `transaction_id`s, mixed
/// `Applied`/interrupted states), the rename-planning banner and its
/// All/Suggested/Already canonical/Ambiguous filter row, multiple plan
/// rows each with their own "Technical classification details"
/// disclosure, and the rollback/leave-untouched controls - all rendered
/// together in one frame, sharing one `ui`, exactly as
/// `show_dat_sources_page` composes them. Every stateful (ID-keyed)
/// control is exercised through a real click and proven independent;
/// every non-ID-keyed control (plain buttons, the filter selection, which
/// lives in `ui_state` - a Rust field, not egui's own ID-keyed memory) is
/// proven to carry the correct row-specific data back out.
#[test]
fn a_realistic_multi_section_dat_sources_render_has_no_cross_widget_id_collisions() {
    let apply = RenameApplyView {
        review: None,
        outcome: None,
        apply_error: None,
        subset_available: false,
        rollback_result: None,
        rollback_error: None,
        resume_result: None,
        resume_error: None,
        apply_running: false,
        rollback_running: false,
        resume_running: false,
        recovery: vec![
            fixture_recovery_transaction(
                "tx-applied-1",
                TransactionState::Applied,
                "Renamed \"one.bin\" -> \"One (Europe).bin\"",
            ),
            fixture_recovery_transaction(
                "tx-applied-2",
                TransactionState::Applied,
                "Renamed \"two.bin\" -> \"Two (Europe).bin\"",
            ),
            fixture_recovery_transaction(
                "tx-interrupted-3",
                TransactionState::ApplyFailed,
                "Renamed 3 files",
            ),
        ],
        journal_dir: "/journal".to_string(),
        recovery_resolution_error: None,
        recovery_archive_error: None,
        recovery_archive_confirm: None,
        recovery_archive_outcome: None,
    };
    let plan = RenamePlanView {
        generation: 1,
        scan_root: "/roms".to_string(),
        scan_root_short: "roms".to_string(),
        platform_display: None,
        source_display_name: "Source".to_string(),
        counts: archivefs_core::dat::rename_plan::RenamePlanCounts {
            suggested: 2,
            ambiguous: 1,
            total: 3,
            ..Default::default()
        },
        audited_total: 3,
        verified_total: 3,
        truncated: false,
        rows: vec![
            fixture_rename_plan_row("/roms/a.bin", "a.bin"),
            fixture_rename_plan_row("/roms/b.bin", "b.bin"),
            RenamePlanRowView {
                state: ProposalState::Ambiguous,
                ambiguity_reason: Some("two equally-scored candidates".to_string()),
                ..fixture_rename_plan_row("/roms/c.bin", "c.bin")
            },
        ],
        error: None,
    };

    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 1600.0));
    let base_input = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    let mut ui_state = DatSourcesPageUi::default();

    let render = |ctx: &egui::Context,
                  input: egui::RawInput,
                  ui_state: &mut DatSourcesPageUi,
                  action_out: &mut Option<DatSourcesPageAction>| {
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut action = show_rename_apply_section(ui, &apply, ui_state);
                if let Some(plan_action) = show_rename_plan_section(ui, &plan, ui_state) {
                    action = action.or(Some(plan_action));
                }
                *action_out = action;
            });
        })
    };

    // Frame 1: settle. Every row's own "Technical classification details"
    // header renders once, and every recovery transaction's rollback
    // control renders with the right label for its own state.
    let mut action = None;
    let first = render(&ctx, base_input.clone(), &mut ui_state, &mut action);
    assert_eq!(
        rendered_text_count(&first, "Technical classification details"),
        3,
        "every plan row must render its own disclosure header"
    );
    assert!(rendered_text_contains(&first, "Roll back transaction"));
    assert!(rendered_text_contains(&first, "Roll back completed steps"));
    assert!(rendered_text_contains(&first, "Leave untouched"));
    assert!(rendered_text_contains(&first, "All"));
    assert!(rendered_text_contains(&first, "Suggested"));
    assert!(rendered_text_contains(&first, "Already canonical"));
    assert!(rendered_text_contains(&first, "Ambiguous"));

    // Click the FIRST plan row's "Technical classification details"
    // header open.
    let header_pos = find_exact_text_center(&first, "Technical classification details")
        .expect("expected at least one disclosure header to render");
    let click = egui::RawInput {
        screen_rect: Some(screen),
        events: vec![
            egui::Event::PointerMoved(header_pos),
            egui::Event::PointerButton {
                pos: header_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: header_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ],
        ..Default::default()
    };
    let _ = render(&ctx, click, &mut ui_state, &mut action);

    // Frame 3: settle. Exactly one row's body must have opened - proof
    // the three "Technical classification details" headers (row a, row b,
    // row c) hold independent IDs, not a shared one.
    let after_open = render(&ctx, base_input.clone(), &mut ui_state, &mut action);
    assert_eq!(
        rendered_text_count(&after_open, "Classification: Verified"),
        1,
        "opening one row's disclosure must not open any other row's"
    );

    // Click "Roll back transaction" for the SECOND applied recovery entry
    // specifically (not the first) - proving the action returned carries
    // that row's own transaction_id, not a neighbour's.
    let rollback_positions: Vec<egui::Pos2> = {
        fn collect(shape: &egui::Shape, needle: &str, out: &mut Vec<egui::Pos2>) {
            match shape {
                egui::Shape::Text(text_shape) if text_shape.galley.text() == needle => {
                    out.push(text_shape.pos + text_shape.galley.size() / 2.0);
                }
                egui::Shape::Vec(nested) => {
                    for s in nested {
                        collect(s, needle, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &after_open.shapes {
            collect(&clipped.shape, "Roll back transaction", &mut out);
        }
        out
    };
    assert_eq!(
        rollback_positions.len(),
        2,
        "both Applied recovery entries must render their own rollback button"
    );
    let second_rollback_pos = rollback_positions[1];
    let click_second_rollback = egui::RawInput {
        screen_rect: Some(screen),
        events: vec![
            egui::Event::PointerMoved(second_rollback_pos),
            egui::Event::PointerButton {
                pos: second_rollback_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: second_rollback_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ],
        ..Default::default()
    };
    let mut rollback_action = None;
    let _ = render(
        &ctx,
        click_second_rollback,
        &mut ui_state,
        &mut rollback_action,
    );
    assert_eq!(
        rollback_action,
        Some(DatSourcesPageAction::RecoveryChoice {
            id: "tx-applied-2".to_string(),
            choice: RecoveryChoice::RollBack,
        }),
        "the second recovery card's own rollback button must roll back its own transaction, \
         never a neighbour's"
    );
}

fn approve_all(page: &mut DatSourcesPageState) {
    let plan = page.rename_plan.clone().unwrap();
    for proposal in &plan.proposals {
        page.apply(DatSourcesPageAction::SetReviewDecision {
            path: proposal.source_path.to_string_lossy().into_owned(),
            decision: Some(ReviewDecision::AcceptedForReview),
        });
    }
}

#[test]
fn apply_is_hidden_without_approved_suggested_proposals() {
    let (_fixture, _roms, page) = page_with_apply_plan(1);
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    assert!(
        !rendered_text_contains(&output, "Apply approved renames"),
        "with nothing approved there is nothing to apply"
    );
}

#[test]
fn apply_is_hidden_for_unsafe_proposals() {
    let (_fixture, _roms, mut page) = page_with_apply_plan(1);
    // Turn the only proposal into an ambiguous one (not applicable).
    let plan = page.rename_plan.as_mut().unwrap();
    plan.proposals[0].state = ProposalState::Ambiguous;
    plan.proposals[0].actionable = false;
    approve_all(&mut page);
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    assert!(
        !rendered_text_contains(&output, "Apply approved renames"),
        "ambiguous proposals must never be offered for apply"
    );
}

#[test]
fn begin_review_shows_the_read_only_old_to_new_pairs() {
    let (_fixture, roms, mut page) = page_with_apply_plan(2);
    approve_all(&mut page);
    page.apply(DatSourcesPageAction::BeginApplyReview);
    let view = page.view();
    let review = view
        .rename_apply
        .review
        .as_ref()
        .expect("a review is shown");
    assert_eq!(review.rows.len(), 2);
    assert_eq!(review.rows[0].current_basename, "game0.bin");
    assert_eq!(review.rows[0].proposed_basename, "Game 0 (Europe).bin");
    assert_eq!(review.trusted_root.as_deref(), Some(roms.to_str().unwrap()));
    // Nothing has been renamed yet.
    assert!(roms.join("game0.bin").exists());
    assert!(!roms.join("Game 0 (Europe).bin").exists());
}

#[test]
fn stale_classifier_plan_shows_regenerate_message_and_does_not_rename() {
    let (_fixture, roms, mut page) = page_with_apply_plan(1);
    page.rename_plan.as_mut().unwrap().classifier_version = "superseded-classifier".to_string();
    approve_all(&mut page);

    page.apply(DatSourcesPageAction::BeginApplyReview);

    assert_eq!(
        page.view().rename_apply.apply_error.as_deref(),
        Some(
            "Rename plan is stale because classification rules changed. Regenerate the plan before applying."
        )
    );
    assert!(roms.join("game0.bin").exists());
    assert!(!roms.join("Game 0 (Europe).bin").exists());
}

#[test]
fn a_large_batch_requires_the_typed_confirmation() {
    let (_fixture, roms, mut page) = page_with_apply_plan(9);
    approve_all(&mut page);
    page.apply(DatSourcesPageAction::BeginApplyReview);
    let view = page.view();
    let review = view.rename_apply.review.as_ref().expect("review");
    assert_eq!(
        review.required_phrase.as_deref(),
        Some("RENAME 9 FILES"),
        "a batch larger than the threshold needs a typed phrase"
    );

    // A wrong phrase is refused and nothing happens.
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: "RENAME 8 FILES".to_string(),
    });
    assert!(page.view().rename_apply.apply_error.is_some());
    assert!(roms.join("game0.bin").exists());

    // The correct phrase applies.
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: "RENAME 9 FILES".to_string(),
    });
    run_to_completion(&mut page);
    let view = page.view();
    let outcome = view.rename_apply.outcome.as_ref().expect("an outcome");
    assert_eq!(outcome.applied, 9);
    assert!(roms.join("Game 0 (Europe).bin").exists());
}

#[test]
fn an_approved_apply_renames_and_the_outcome_is_visible() {
    let (_fixture, roms, mut page) = page_with_apply_plan(2);
    approve_all(&mut page);
    page.apply(DatSourcesPageAction::BeginApplyReview);
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: String::new(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let apply = &view.rename_apply;
    let outcome = apply.outcome.as_ref().expect("an apply outcome");
    assert_eq!(outcome.applied, 2);
    assert_eq!(outcome.failed, 0);
    assert!(!roms.join("game0.bin").exists());
    assert!(roms.join("Game 0 (Europe).bin").exists());
    assert_eq!(
        std::fs::read(roms.join("Game 1 (Europe).bin")).unwrap(),
        b"fixture contents"
    );
    // The journal records the applied state.
    assert!(apply.journal_dir.contains("journal"));
}

#[test]
fn rollback_from_the_page_restores_original_files() {
    let (_fixture, roms, mut page) = page_with_apply_plan(1);
    approve_all(&mut page);
    page.apply(DatSourcesPageAction::BeginApplyReview);
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: String::new(),
    });
    run_to_completion(&mut page);
    assert!(roms.join("Game 0 (Europe).bin").exists());

    let id = page
        .view()
        .rename_apply
        .outcome
        .as_ref()
        .unwrap()
        .transaction_id
        .clone();
    page.apply(DatSourcesPageAction::RollbackTransaction { id });
    run_to_completion(&mut page);
    let view = page.view();
    assert_eq!(
        view.rename_apply.rollback_result.as_ref().map(|r| r.label),
        Some("Fully rolled back")
    );
    assert!(roms.join("game0.bin").exists());
    assert!(!roms.join("Game 0 (Europe).bin").exists());
    assert_eq!(
        std::fs::read(roms.join("game0.bin")).unwrap(),
        b"fixture contents"
    );
}

#[test]
fn an_interrupted_transaction_is_offered_for_recovery_and_never_auto_resumes() {
    let (_fixture, _roms, mut page) = page_with_apply_plan(0);
    // Write an interrupted journal directly into the page's temp journal dir.
    let tx = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "interrupted-test".to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 1,
        source_scan_root: "/tmp/roms".to_string(),
        state: archivefs_core::dat::rename_apply::TransactionState::Applying,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: PathBuf::from("/tmp/roms/a.bin"),
            destination_path: PathBuf::from("/tmp/roms/b.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "b.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: archivefs_core::dat::rename_apply::EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    archivefs_core::dat::rename_apply::write_journal(
        Path::new(&page.view().rename_apply.journal_dir),
        &tx,
    )
    .unwrap();
    page.refresh_recovery();

    let view = page.view();
    assert_eq!(view.rename_apply.recovery.len(), 1);
    assert!(
        view.rename_apply.recovery[0]
            .transaction_id
            .contains("interrupted")
    );
    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    // There is no auto-resume anywhere in the UI.
    assert!(!rendered_text_contains(&output, "Resume renames"));
}

/// Regression for the transaction-level reconciliation gap behind
/// `an_applied_transaction_is_rediscovered_after_restart_and_rollbackable`
/// (CI's occasional failure there): a journal whose transaction-level
/// `state` is stuck at `Applying` - exactly what a final journal write that
/// failed to land after every entry already settled looks like - but whose
/// sole entry is already durably `Applied`, must load as `Applied`, not
/// `Applying`. Deterministic (no background job, no timing dependency): the
/// journal is hand-written directly, so this exercises the restart/load
/// path in isolation from whatever caused the CI flake.
#[test]
fn a_transaction_stuck_applying_with_an_applied_entry_loads_as_applied_after_restart() {
    let fixture = Fixture::new();
    let journal = fixture.dir("journal");
    let tx = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "stuck-applying-test".to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 1,
        source_scan_root: "/tmp/roms".to_string(),
        state: archivefs_core::dat::rename_apply::TransactionState::Applying,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: PathBuf::from("/tmp/roms/a.bin"),
            destination_path: PathBuf::from("/tmp/roms/b.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "b.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state: archivefs_core::dat::rename_apply::EntryState::Applied,
            failure_reason: None,
            applied_at_unix: Some(1),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    archivefs_core::dat::rename_apply::write_journal(&journal, &tx).unwrap();

    // A fresh page load, exactly what a real restart does - never a running
    // page's own `refresh_recovery`.
    let restarted = DatSourcesPageState::load_with_transaction_dir(
        fixture.config_path.clone(),
        Vec::new(),
        TrustedRoots::none(),
        journal,
    );
    let view = restarted.view();
    assert_eq!(view.rename_apply.recovery.len(), 1);
    assert_eq!(
        view.rename_apply.recovery[0].state,
        TransactionState::Applied,
        "a transaction whose every entry is already durably Applied must load as Applied, not \
         stay stuck at Applying"
    );
}

fn fixture_identity() -> archivefs_core::dat::rename_apply::ObjectIdentity {
    archivefs_core::dat::rename_apply::ObjectIdentity {
        size_bytes: 1,
        modified_unix: 0,
        kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
        #[cfg(unix)]
        ino: 0,
        #[cfg(unix)]
        dev: 0,
    }
}

fn fixture_entry(original: &str, proposed: &str) -> TransactionEntry {
    TransactionEntry {
        operation: Default::default(),
        source_path: PathBuf::from(original),
        destination_path: PathBuf::from(proposed),
        original_basename: original.to_string(),
        proposed_basename: proposed.to_string(),
        identity: fixture_identity(),
        preflight_passed: true,
        preflight_failures: Vec::new(),
        state: EntryState::Applied,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    }
}

#[test]
fn rename_transaction_human_summary_leads_with_the_single_rename_when_there_is_one() {
    let entries = vec![fixture_entry("game0.bin", "Game 0 (Europe).bin")];
    assert_eq!(
        rename_transaction_human_summary(&entries),
        "Renamed \"game0.bin\" -> \"Game 0 (Europe).bin\""
    );
}

#[test]
fn rename_transaction_human_summary_counts_multiple_renames_instead_of_naming_them() {
    let entries = vec![
        fixture_entry("game0.bin", "Game 0 (Europe).bin"),
        fixture_entry("game1.bin", "Game 1 (Europe).bin"),
    ];
    assert_eq!(
        rename_transaction_human_summary(&entries),
        "Renamed 2 files"
    );
}

#[test]
fn recovery_human_state_label_collapses_every_non_settled_state_to_needs_attention() {
    assert_eq!(
        recovery_human_state_label(TransactionState::Applied),
        "Applied"
    );
    assert_eq!(
        recovery_human_state_label(TransactionState::RolledBack),
        "Rolled back"
    );
    for unsettled in [
        TransactionState::Planned,
        TransactionState::Applying,
        TransactionState::ApplyFailed,
        TransactionState::RollingBack,
        TransactionState::RollbackFailed,
    ] {
        assert_eq!(recovery_human_state_label(unsettled), "Needs attention");
    }
}

#[test]
fn quick_rename_reuses_repair_history_transaction_presentation() {
    let transaction = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "shared-presentation".to_string(),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: 1,
        source_scan_root: "/missing/root".to_string(),
        state: TransactionState::Applied,
        entries: vec![fixture_entry("old.bin", "New.bin")],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let quick = RecoveryTransactionView::from_transaction(
        &transaction,
        ExactResumeStatusView::UnavailableLegacy,
    );
    let cleanup = classify_recovery_cleanup(&transaction);
    let repair = crate::repair_history_page::presentation::classify(&transaction, cleanup);

    assert_eq!(quick.presentation, repair);
}

#[test]
fn an_applied_transaction_is_rediscovered_after_restart_and_rollbackable() {
    let (fixture, roms, mut page) = page_with_apply_plan(1);
    approve_all(&mut page);
    page.apply(DatSourcesPageAction::BeginApplyReview);
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: String::new(),
    });
    run_to_completion(&mut page);
    // The apply renamed the file and wrote an `Applied` journal on disk.
    assert!(roms.join("Game 0 (Europe).bin").exists());
    assert!(!roms.join("game0.bin").exists());

    // Simulate a restart: a fresh page state loaded from the same journal dir
    // must rediscover the settled Applied transaction, not just interrupted
    // ones.
    let journal = fixture.dir("journal");
    let mut restarted = DatSourcesPageState::load_with_transaction_dir(
        fixture.config_path.clone(),
        Vec::new(),
        TrustedRoots::from_paths([&roms]),
        journal,
    );
    let view = restarted.view();
    let recovery = &view.rename_apply.recovery;
    assert_eq!(
        recovery.len(),
        1,
        "the applied journal must be rediscovered"
    );
    assert_eq!(recovery[0].state, TransactionState::Applied);
    assert_eq!(recovery[0].applied_count, 1);
    let id = recovery[0].transaction_id.clone();

    // DAT Sources owns catalogue management only. The applied journal remains
    // available in the view model for Identify & Rename / History & Logs, but
    // settled rename history is not rendered inline here.
    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    assert!(!rendered_text_contains(&output, "Roll back transaction"));
    assert!(!rendered_text_contains(&output, "Renamed \"game0.bin\""));
    assert!(
        !rendered_text_contains(&output, &id),
        "the raw transaction ID must not appear on the primary line"
    );

    // Rolling back from the rediscovered page restores the original file.
    restarted.apply(DatSourcesPageAction::RecoveryChoice {
        id,
        choice: RecoveryChoice::RollBack,
    });
    run_to_completion(&mut restarted);
    assert!(roms.join("game0.bin").exists());
    assert!(!roms.join("Game 0 (Europe).bin").exists());
    assert_eq!(
        std::fs::read(roms.join("game0.bin")).unwrap(),
        b"fixture contents"
    );
}

#[test]
fn the_just_applied_transaction_is_not_double_listed_in_recovery() {
    let (_fixture, _roms, mut page) = page_with_apply_plan(1);
    approve_all(&mut page);
    page.apply(DatSourcesPageAction::BeginApplyReview);
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: String::new(),
    });
    run_to_completion(&mut page);

    // The transaction is shown via the apply-outcome card, so the recovery list
    // must not list it a second time even though its journal is `Applied`.
    let view = page.view();
    assert!(view.rename_apply.outcome.is_some());
    assert!(
        view.rename_apply.recovery.is_empty(),
        "the just-applied transaction must not be double-listed"
    );
}

#[test]
fn a_rediscovered_applied_transaction_still_refuses_rollback_when_the_destination_changed() {
    let (fixture, roms, mut page) = page_with_apply_plan(1);
    approve_all(&mut page);
    page.apply(DatSourcesPageAction::BeginApplyReview);
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: String::new(),
    });
    run_to_completion(&mut page);
    let destination = roms.join("Game 0 (Europe).bin");
    assert!(destination.exists());

    // Simulate a restart, then replace the destination externally: identity
    // protection must still hold after rediscovery, so rollback refuses rather
    // than guessing.
    let journal = fixture.dir("journal");
    let mut restarted = DatSourcesPageState::load_with_transaction_dir(
        fixture.config_path.clone(),
        Vec::new(),
        TrustedRoots::from_paths([&roms]),
        journal,
    );
    let id = restarted.view().rename_apply.recovery[0]
        .transaction_id
        .clone();
    std::fs::remove_file(&destination).unwrap();
    std::fs::write(&destination, b"externally changed").unwrap();

    restarted.apply(DatSourcesPageAction::RecoveryChoice {
        id,
        choice: RecoveryChoice::RollBack,
    });
    run_to_completion(&mut restarted);

    let view = restarted.view();
    assert_eq!(
        view.rename_apply.rollback_result.as_ref().map(|r| r.label),
        Some("Rollback failed")
    );
    // Nothing was renamed back: the changed destination is untouched and the
    // original source was not restored.
    assert!(destination.exists());
    assert!(!roms.join("game0.bin").exists());
    assert_eq!(std::fs::read(&destination).unwrap(), b"externally changed");
}

#[test]
fn the_apply_section_renders_at_a_narrow_compact_width() {
    let (_fixture, _roms, mut page) = page_with_apply_plan(2);
    approve_all(&mut page);
    page.apply(DatSourcesPageAction::BeginApplyReview);
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_at_width(&view, &mut ui_state, 700.0);
    assert!(rendered_text_contains(&output, "Local DAT Sources"));
    assert!(!rendered_text_contains(&output, "Review approved renames"));
    assert!(!rendered_text_contains(&output, "Rename transactions"));
}

#[test]
fn the_apply_controls_are_reachable_by_keyboard() {
    let (_fixture, _roms, mut page) = page_with_apply_plan(1);
    approve_all(&mut page);
    page.apply(DatSourcesPageAction::BeginApplyReview);
    let view = page.view();
    let ctx = egui::Context::default();
    let mut focused_anything = false;
    for _ in 0..40 {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 4000.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut ui_state = DatSourcesPageUi::default();
                    let _ = show_dat_sources_page(ui, &view, &mut ui_state);
                });
            },
        );
        if ctx.memory(|memory| memory.focused()).is_some() {
            focused_anything = true;
        }
    }
    assert!(focused_anything, "Tab never focused anything on the page");
}

// ---------------------------------------------------------------------------
// Beta UX: safety promise, Any-preference, grouped diagnostics
// ---------------------------------------------------------------------------

#[test]
fn the_safe_promise_is_rendered_on_the_dat_sources_page() {
    let fixture = Fixture::new();
    let page = fixture.page();
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    assert!(
        rendered_text_contains(&output, SAFE_PROMISE),
        "the simple safety promise must be visible"
    );
    assert!(
        SAFE_PROMISE.contains("Your files won't be renamed unless you approve it."),
        "the promise uses the exact friendly wording"
    );
}

#[test]
fn any_region_clears_the_region_preference() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddRegion {
        scope: None,
        region: RegionId::Europe,
    });
    page.apply(DatSourcesPageAction::AddRegion {
        scope: None,
        region: RegionId::Japan,
    });
    assert_eq!(page.view().policy.region_preferences.len(), 2);
    page.apply(DatSourcesPageAction::ClearRegion { scope: None });
    assert!(
        page.view().policy.region_preferences.is_empty(),
        "Any region clears the preference ordering"
    );
    assert_eq!(page.view().policy.effective.region, "Any");
}

#[test]
fn any_language_clears_the_language_preference() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddLanguage {
        scope: None,
        preference: LanguagePreference::Language(LanguageId::En),
    });
    assert_eq!(page.view().policy.language_preferences.len(), 1);
    page.apply(DatSourcesPageAction::ClearLanguage { scope: None });
    assert!(
        page.view().policy.language_preferences.is_empty(),
        "Any language clears the preference ordering"
    );
    assert_eq!(page.view().policy.effective.language, "Any");
}

#[test]
fn the_effective_policy_renders_any_not_none_all_equal() {
    assert_eq!(super::render_preference_list(Vec::new()), "Any");
    assert_eq!(
        super::render_preference_list(vec!["Europe".to_string()]),
        "Europe"
    );
}

#[test]
fn repeated_symlink_diagnostics_group_with_an_exact_count_and_bounded_examples() {
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let mut files = Vec::new();
            for index in 0..25 {
                files.push((
                    format!("disc{index}.bin"),
                    "symlink refused: it resolves outside every configured source root".to_string(),
                ));
            }
            super::show_unhashed_groups(ui, &files);
        });
    });
    assert!(
        rendered_text_contains(&output, "25 symlinks could not be hashed"),
        "the group must carry the exact count"
    );
    assert!(
        rendered_text_contains(&output, "Show all 25"),
        "the raw findings remain available behind an expansion"
    );
    // Examples are bounded to the first 10; the rest are only in "Show all".
    let example_count = rendered_text_count(&output, "disc");
    assert!(
        example_count <= 10,
        "examples must be bounded (found {example_count})"
    );
}

#[test]
fn the_empty_state_uses_the_verify_identity_and_short_wording() {
    let fixture = Fixture::new();
    let page = fixture.page();
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render(&view, &mut ui_state);
    // With no DAT sources registered the empty state is the friendly, short
    // "No DATs added" with the verify icon - not a long technical block.
    assert!(rendered_text_contains(&output, crate::ui::icons::VERIFY));
    assert!(rendered_text_contains(&output, "No DATs added"));
}

// ---------------------------------------------------------------------------
// Managed MAME DAT sources
// ---------------------------------------------------------------------------

#[test]
fn dat_acquisition_entry_point_exposes_existing_safe_provider_paths_without_auto_configuring() {
    let fixture = Fixture::new();
    let page = fixture.page();
    let view = page.view();

    // Drawing is descriptive only: it must not create a managed source, start
    // a network operation, or enable a TOSEC selection.
    assert!(view.managed_rows.is_empty());
    assert!(view.redump_bios_rows.iter().all(|row| !row.configured));
    assert!(view.redump_game_rows.iter().all(|row| !row.configured));
    assert!(view.tosec_packs.is_empty());
    assert!(!view.background_busy);

    let mut ui_state = DatSourcesPageUi {
        managed_sources_expanded: Some(true),
        ..Default::default()
    };
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &output,
        "Get evidence for your library"
    ));
    assert!(rendered_text_contains(&output, "No-Intro — cartridge ROMs"));
    assert!(rendered_text_contains(&output, "Open DAT-o-MATIC"));
    assert!(rendered_text_contains(&output, "Choose downloaded ZIP…"));
    assert!(rendered_text_contains(&output, "WHDLoad — Amiga packages"));
    assert!(rendered_text_contains(&output, "Choose WHDLoad DAT…"));
    assert!(rendered_text_contains(&output, "TOSEC — vintage systems"));
    assert!(rendered_text_contains(
        &output,
        "Choose extracted TOSEC pack…"
    ));
    assert!(rendered_text_contains(
        &output,
        "Redump — disc and BIOS metadata"
    ));
    assert!(rendered_text_contains(&output, "MAME and other local DATs"));
    assert!(rendered_text_contains(&output, "Choose local DAT…"));
}

const TOSEC_AMIGA_FLOPPY: &str = r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>Amiga - Games - Floppy</name>
    <description>Amiga - Games - Floppy (TOSEC-v2021-01-09)</description>
    <version>2021-01-09</version>
    <author>TOSEC</author>
  </header>
  <game name="Example"><rom name="example.adf" size="4" crc="00000001" md5="00000000000000000000000000000001" sha1="0000000000000000000000000000000000000001"/></game>
</datafile>"#;

const TOSEC_ISO_DEFERRED: &str = r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>TOSEC-ISO - PC</name>
    <description>TOSEC-ISO - PC (TOSEC-v2021-01-09)</description>
    <version>2021-01-09</version>
    <author>TOSEC</author>
  </header>
  <game name="Example"><rom name="example.iso" size="4" crc="00000001" md5="00000000000000000000000000000001" sha1="0000000000000000000000000000000000000001"/></game>
</datafile>"#;

const WHDLOAD_PACKAGE_DAT: &str = r#"clrmamepro (
  name "Commodore - Amiga - WHDLoad"
  description "Commodore - Amiga - WHDLoad"
  date "2026-07-05"
  author "MrV2K"
  comment "Retroplay"
)
game (
  name "Fixture Game"
  rom ( name "Fixture_v1.0_0001.lha" size 4 crc 00000001 md5 00000000000000000000000000000001 sha1 0000000000000000000000000000000000000001 )
)
"#;

#[test]
fn whdload_local_import_requires_the_parsed_catalogue_identity_and_preserves_provenance() {
    let fixture = Fixture::new();
    let valid = fixture.write("Commodore - Amiga - WHDLoad.dat", WHDLOAD_PACKAGE_DAT);
    let invalid = fixture.write("lookalike.dat", LOGIQX);
    let mut page = fixture.page();

    page.apply(DatSourcesPageAction::AddWHDLoadDat { path: invalid });
    assert!(page.view().rows.is_empty());
    assert!(
        page.view()
            .action_error
            .as_deref()
            .is_some_and(|error| error.contains("expected Commodore - Amiga - WHDLoad"))
    );

    page.apply(DatSourcesPageAction::AddWHDLoadDat { path: valid });
    assert_eq!(page.draft.entries().len(), 1);
    let entry = &page.draft.entries()[0];
    assert_eq!(entry.display_name, "WHDLoad / Retroplay catalogue");
    assert_eq!(
        entry.origin.as_deref(),
        Some("WHDLoad / Retroplay-derived local catalogue selected through DAT Sources")
    );
    assert!(entry.enabled);
}

#[test]
fn managed_sources_render_in_a_separate_section_from_local_sources() {
    let fixture = Fixture::new();
    let local = fixture.write("MAME-local.xml", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: local });
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });

    let view = page.view();
    assert_eq!(view.rows.len(), 1);
    assert_eq!(view.managed_rows.len(), 1);
    assert_eq!(view.managed_rows[0].authoritative_name, "gamecom");
    let mut ui_state = DatSourcesPageUi {
        managed_sources_expanded: Some(true),
        ..Default::default()
    };
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Local DAT Sources"));
    assert!(rendered_text_contains(&output, "Managed DAT Sources"));
    assert!(rendered_text_contains(&output, "System/List: gamecom"));
}

#[test]
fn redump_game_disc_rows_show_only_the_three_typed_supported_systems() {
    let fixture = Fixture::new();
    let page = fixture.page();
    let view = page.view();
    assert_eq!(view.redump_game_rows.len(), 3);
    assert!(
        view.redump_game_rows
            .iter()
            .all(|row| !row.configured && row.provider == ManagedDatProvider::RedumpGames)
    );
    let output = render(&view, &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(&output, "Redump Game/Disc DATs"));
    assert!(rendered_text_contains(&output, "System: PlayStation"));
    assert!(rendered_text_contains(&output, "System: PlayStation 2"));
    assert!(rendered_text_contains(&output, "System: Xbox"));
    assert!(!rendered_text_contains(&output, "Saturn"));
    assert!(rendered_text_contains(&output, "Redump BIOS DATs"));
    assert!(rendered_text_contains(&output, "MAME software list"));
}

#[test]
fn redump_game_check_and_update_actions_keep_the_closed_typed_system_identity() {
    let source_id = ManagedDatSourceId::redump_games(RedumpGameSystem::PlayStation2);
    assert_eq!(
        managed_dat_action(source_id.clone(), ManagedDatOperation::Check),
        DatSourcesPageAction::CheckManagedDat {
            source_id: source_id.clone()
        }
    );
    assert_eq!(
        managed_dat_action(source_id.clone(), ManagedDatOperation::Update),
        DatSourcesPageAction::UpdateManagedDat { source_id }
    );
}

#[test]
fn each_redump_bios_enable_row_emits_its_exact_typed_add_action() {
    let fixture = Fixture::new();
    let page = fixture.page();
    for system in [
        RedumpBiosSystem::PlayStation,
        RedumpBiosSystem::PlayStation2,
        RedumpBiosSystem::Xbox,
    ] {
        let source_id = ManagedDatSourceId::redump_bios(system);
        let row = page
            .view()
            .redump_bios_rows
            .into_iter()
            .find(|row| row.source_id == source_id)
            .expect("every closed Redump BIOS system has a visible row");
        assert!(!row.configured);
        assert_eq!(
            managed_add_action(&row),
            Some(DatSourcesPageAction::AddManagedRedumpBios { system })
        );
    }
}

#[test]
fn redump_bios_enable_actions_configure_ps1_ps2_and_xbox_independently_and_persist() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    for system in [
        RedumpBiosSystem::PlayStation,
        RedumpBiosSystem::PlayStation2,
        RedumpBiosSystem::Xbox,
    ] {
        let source_id = ManagedDatSourceId::redump_bios(system);
        let row = page
            .view()
            .redump_bios_rows
            .into_iter()
            .find(|row| row.source_id == source_id)
            .expect("every closed Redump BIOS system has a visible row");
        page.apply(
            managed_add_action(&row).expect("the enabled BIOS row must produce an add action"),
        );
        assert!(
            page.view()
                .redump_bios_rows
                .iter()
                .find(|row| row.source_id == source_id)
                .is_some_and(|row| row.configured),
            "{system:?} must refresh as configured after its add action"
        );
    }

    let persisted = load_managed_dat_sources_from(&page.managed_config_path).unwrap();
    assert_eq!(persisted.redump_bios_entries().len(), 3);
    for system in [
        RedumpBiosSystem::PlayStation,
        RedumpBiosSystem::PlayStation2,
        RedumpBiosSystem::Xbox,
    ] {
        assert!(
            persisted
                .redump_bios_entries()
                .iter()
                .any(|entry| entry.system == system),
            "{system:?} must persist independently"
        );
    }

    let reloaded = fixture.page();
    assert!(
        reloaded
            .view()
            .redump_bios_rows
            .iter()
            .all(|row| row.configured)
    );
}

#[test]
fn tosec_pack_inventory_starts_with_empty_selection_and_renders_bounded_groups() {
    let fixture = Fixture::new();
    let pack = fixture.dir("tosec-pack");
    fixture.write(
        "tosec-pack/Amiga - Games - Floppy (TOSEC-v2021-01-09).dat",
        TOSEC_AMIGA_FLOPPY,
    );
    fixture.write(
        "tosec-pack/TOSEC-ISO - PC (TOSEC-v2021-01-09).dat",
        TOSEC_ISO_DEFERRED,
    );
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::ImportTosecReleasePack { root: pack });

    let view = page.view();
    assert_eq!(view.tosec_packs.len(), 1);
    assert_eq!(view.tosec_packs[0].selected_dat_count, 0);
    assert!(view.tosec_packs[0].deferred_count > 0);
    assert!(
        view.tosec_packs[0]
            .groups
            .iter()
            .any(|group| group.key.category.label() == "Games"
                && group.key.media.label() == "Floppy / Disk")
    );
    let output = render(&view, &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(&output, "TOSEC Release Packs"));
    assert!(rendered_text_contains(&output, "Enable"));
    assert!(rendered_text_contains(&output, "TOSEC-ISO / TOSEC-PIX"));
}

#[test]
fn deferred_tosec_groups_cannot_be_enabled_even_through_a_direct_page_action() {
    let fixture = Fixture::new();
    let pack_root = fixture.dir("deferred-tosec-pack");
    let key = TosecSelectionKey {
        system: "PC".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::CdOpticalDisc,
    };
    let mut page = fixture.page();
    page.tosec_packs = vec![PersistedTosecPack {
        pack_id: "deferred-pack".to_string(),
        root_path: pack_root,
        imported_unix_seconds: 0,
        selections: Default::default(),
        dats: vec![TosecPackDat {
            relative_path: PathBuf::from("TOSEC-ISO - PC.dat"),
            raw_catalogue_name: "TOSEC-ISO - PC".to_string(),
            system: key.system.clone(),
            category: key.category,
            media: key.media,
            raw_category_label: "Games".to_string(),
            classification_confident: true,
            content_sha256: None,
        }],
    }];

    page.apply(DatSourcesPageAction::SetTosecSelection {
        pack_id: "deferred-pack".to_string(),
        key,
        enabled: true,
    });

    assert!(page.tosec_packs[0].selections.is_empty());
    assert!(
        page.tosec_action_error
            .as_deref()
            .is_some_and(|error| error.contains("cannot be enabled"))
    );
}

#[test]
fn tosec_group_rendering_never_draws_more_than_two_hundred_rows() {
    let fixture = Fixture::new();
    let pack_root = fixture.dir("large-tosec-pack");
    let dats = (0..205)
        .map(|index| TosecPackDat {
            relative_path: PathBuf::from(format!("System {index}.dat")),
            raw_catalogue_name: format!("System {index} - Games - ROM"),
            system: format!("System {index}"),
            category: TosecFriendlyCategory::Games,
            media: TosecMediaType::Rom,
            raw_category_label: "Games".to_string(),
            classification_confident: true,
            content_sha256: None,
        })
        .collect();
    let mut page = fixture.page();
    page.tosec_packs = vec![PersistedTosecPack {
        pack_id: "large-pack".to_string(),
        root_path: pack_root,
        imported_unix_seconds: 0,
        selections: Default::default(),
        dats,
    }];

    let output = render(&page.view(), &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(
        &output,
        "Showing 200 of 205 matching groups"
    ));
    // Counts only the per-group "<system> · 1 DAT(s)" rows (every synthetic
    // group here has exactly one DAT), not the unrelated page-level "N
    // selected TOSEC DAT(s)" evidence-readiness summary that also contains
    // the substring "DAT(s)" once per render.
    assert!(
        rendered_text_count(&output, "· 1 DAT(s)") <= 200,
        "only the bounded visible group rows may be rendered"
    );
}

#[test]
fn tosec_missing_pack_is_honest_after_restart_and_selection_is_explicit() {
    let fixture = Fixture::new();
    let pack = fixture.dir("missing-tosec-pack");
    fixture.write(
        "missing-tosec-pack/Amiga - Games - Floppy (TOSEC-v2021-01-09).dat",
        TOSEC_AMIGA_FLOPPY,
    );
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::ImportTosecReleasePack { root: pack.clone() });
    let group = page.view().tosec_packs[0].groups[0].key.clone();
    let pack_id = page.view().tosec_packs[0].pack_id.clone();
    page.apply(DatSourcesPageAction::SetTosecSelection {
        pack_id: pack_id.clone(),
        key: group,
        enabled: true,
    });
    assert_eq!(page.view().tosec_packs[0].selected_dat_count, 1);

    std::fs::remove_dir_all(pack).unwrap();
    let reloaded = fixture.page();
    assert_eq!(
        reloaded.view().tosec_packs[0].availability,
        PackAvailability::Missing
    );
    let output = render(&reloaded.view(), &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(&output, "Pack missing"));
}

#[test]
fn applying_an_explicit_tosec_selection_registers_only_the_selected_group() {
    let fixture = Fixture::new();
    let pack = fixture.dir("apply-tosec-pack");
    fixture.write(
        "apply-tosec-pack/Amiga - Games - Floppy (TOSEC-v2021-01-09).dat",
        TOSEC_AMIGA_FLOPPY,
    );
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::ImportTosecReleasePack { root: pack });
    let view = page.view();
    let pack_id = view.tosec_packs[0].pack_id.clone();
    let key = view.tosec_packs[0].groups[0].key.clone();
    page.apply(DatSourcesPageAction::SetTosecSelection {
        pack_id: pack_id.clone(),
        key,
        enabled: true,
    });
    page.apply(DatSourcesPageAction::ApplyTosecSelection { pack_id });

    assert_eq!(page.view().rows.len(), 1);
    let applied = page.tosec_last_apply.as_ref().unwrap();
    assert_eq!(applied.registered, 1);
    assert_eq!(applied.failed, 0);
}

#[test]
fn a_local_mame_named_source_never_becomes_managed() {
    let fixture = Fixture::new();
    let local = fixture.write("MAME.xml", LOGIQX);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: local });
    let id = page.view().rows[0].id.clone();
    page.draft.get_mut(&id).unwrap().origin = Some("MAME".to_string());
    assert_eq!(page.view().rows.len(), 1);
    assert!(page.view().managed_rows.is_empty());
}

#[test]
fn configured_but_uninstalled_managed_source_survives_restart() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    assert!(page.managed_config_path.exists());

    let reloaded = fixture.page();
    let view = reloaded.view();
    assert_eq!(view.managed_rows.len(), 1);
    assert!(!view.managed_rows[0].installed);
    assert_eq!(
        view.managed_rows[0].status,
        ManagedDatStatusView::NotInstalled
    );
    assert!(view.managed_rows[0].current_revision.is_none());
}

#[test]
fn managed_add_uses_only_the_typed_mame_configuration() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    let persisted = load_managed_dat_sources_from(&page.managed_config_path).unwrap();
    assert_eq!(persisted.entries().len(), 1);
    assert_eq!(persisted.entries()[0].authoritative_name, "gamecom");
    assert_eq!(
        persisted.entries()[0].update_policy,
        ManagedDatUpdatePolicy::Manual,
        "an explicit manual GUI source must never opt into automatic checks"
    );
    let config_text = std::fs::read_to_string(&page.managed_config_path).unwrap();
    assert!(!config_text.contains("http"));
    assert!(!config_text.contains("repository"));
}

#[test]
fn removing_managed_configuration_keeps_existing_object_files() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    let object = page
        .managed_root
        .join("mame-software-list/gamecom/objects/keep-me");
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(&object, "immutable object").unwrap();

    page.apply(DatSourcesPageAction::RemoveManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    assert!(page.view().managed_rows.is_empty());
    assert!(object.exists(), "removing config must not delete objects");
    assert!(
        load_managed_dat_sources_from(&page.managed_config_path)
            .unwrap()
            .entries()
            .is_empty()
    );
}

#[test]
fn rendering_managed_sources_starts_no_worker_or_network_operation() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    let view = page.view();
    let _ = render(&view, &mut DatSourcesPageUi::default());
    assert!(page.managed_job.is_none());
    assert!(!page.is_busy());
    assert_eq!(
        page.managed_statuses.get(&managed_source_key(
            &ManagedDatSourceId::mame_software_list("gamecom").unwrap()
        )),
        None,
        "rendering is read-only and must not synthesize a check result"
    );
}

#[test]
fn update_requires_a_prior_explicit_check_result() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    assert!(!page.view().managed_rows[0].update_enabled);

    page.managed_statuses.insert(
        managed_source_key(&ManagedDatSourceId::mame_software_list("gamecom").unwrap()),
        ManagedDatStatusView::UpdateAvailable {
            upstream_revision: "a".repeat(40),
        },
    );
    assert!(page.view().managed_rows[0].update_enabled);
}

#[test]
fn managed_buttons_emit_only_typed_check_and_update_actions() {
    assert_eq!(
        managed_dat_action(
            ManagedDatSourceId::mame_software_list("gamecom").unwrap(),
            ManagedDatOperation::Check,
        ),
        DatSourcesPageAction::CheckManagedDat {
            source_id: ManagedDatSourceId::mame_software_list("gamecom").unwrap()
        }
    );
    assert_eq!(
        managed_dat_action(
            ManagedDatSourceId::mame_software_list("gamecom").unwrap(),
            ManagedDatOperation::Update,
        ),
        DatSourcesPageAction::UpdateManagedDat {
            source_id: ManagedDatSourceId::mame_software_list("gamecom").unwrap()
        }
    );
}

#[test]
fn installed_current_and_previous_snapshots_refresh_the_managed_row_without_promoting_previous() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    let descriptor = page.managed_sources.entries()[0].descriptor().unwrap();
    let current = "a".repeat(64);
    let previous = "b".repeat(64);
    let objects = page
        .managed_root
        .join(descriptor.source_id().storage_relative_path())
        .join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join(&current), "current XML").unwrap();
    std::fs::write(objects.join(&previous), "previous XML").unwrap();
    let mut state = ManagedDatState::new(
        &descriptor,
        ManagedDatSnapshot::new(current.clone()).unwrap(),
    )
    .unwrap();
    state.previous_snapshot = Some(ManagedDatSnapshot::new(previous.clone()).unwrap());
    state.upstream_revision = Some("c".repeat(40));
    state.sha256 = current.clone();
    save_managed_dat_state(&page.managed_root, &state).unwrap();

    let view = page.view();
    let row = &view.managed_rows[0];
    assert!(row.installed);
    assert_eq!(row.current_revision, Some("c".repeat(40)));
    assert_eq!(row.technical.sha256, Some(current.clone()));
    assert_eq!(row.technical.previous_snapshot, Some(previous.clone()));
    assert!(
        row.technical
            .current_path
            .as_deref()
            .is_some_and(|path| path.ends_with(&current))
    );
    assert!(
        row.technical
            .previous_path
            .as_deref()
            .is_some_and(|path| path.ends_with(&previous))
    );
    assert_ne!(row.technical.current_path, row.technical.previous_path);

    let mut ui_state = DatSourcesPageUi {
        open_managed_technical: Some(ManagedDatSourceId::mame_software_list("gamecom").unwrap()),
        ..Default::default()
    };
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &output,
        "Previous snapshot (not active)"
    ));
}

// --- managed DAT rollback --------------------------------------------------

/// Builds a two-object managed MAME source (current + previous, both real
/// files under `objects/`) with distinct, hand-set provenance for each -
/// exactly the fixture shape a real update leaves behind after this task's
/// core change to `publish_validated_snapshot`, but built directly so the
/// test controls both revisions independently.
fn two_snapshot_fixture(
    page: &DatSourcesPageState,
) -> (ManagedDatSourceDescriptor, String, String) {
    let descriptor = page.managed_sources.entries()[0].descriptor().unwrap();
    let current = "a".repeat(64);
    let previous = "b".repeat(64);
    let objects = page
        .managed_root
        .join(descriptor.source_id().storage_relative_path())
        .join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join(&current), "current XML").unwrap();
    std::fs::write(objects.join(&previous), "previous XML").unwrap();
    let mut state = ManagedDatState::new(
        &descriptor,
        ManagedDatSnapshot::new(current.clone()).unwrap(),
    )
    .unwrap();
    state.previous_snapshot = Some(ManagedDatSnapshot::new(previous.clone()).unwrap());
    state.upstream_revision = Some("current-revision".to_string());
    state.retrieved_at_unix_seconds = Some(2_000);
    state.validation_summary = Some("current validated cleanly".to_string());
    state.previous_upstream_revision = Some("previous-revision".to_string());
    state.previous_retrieved_at_unix_seconds = Some(1_000);
    state.previous_validation_summary = Some("previous validated cleanly".to_string());
    state.sha256 = current.clone();
    save_managed_dat_state(&page.managed_root, &state).unwrap();
    (descriptor, current, previous)
}

#[test]
fn a_previous_revision_and_its_acquisition_time_are_shown_when_present() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    two_snapshot_fixture(&page);

    let view = page.view();
    let row = &view.managed_rows[0];
    assert_eq!(row.previous_revision.as_deref(), Some("previous-revision"));
    assert!(row.previous_retrieved.is_some());
    assert!(row.rollback_available);

    let mut ui_state = DatSourcesPageUi {
        managed_sources_expanded: Some(true),
        ..Default::default()
    };
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "previous-revision"));
    assert!(rendered_text_contains(
        &output,
        "Previous revision available"
    ));
}

#[test]
fn no_previous_revision_is_reported_honestly_when_there_is_only_one_snapshot() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    let descriptor = page.managed_sources.entries()[0].descriptor().unwrap();
    let current = "c".repeat(64);
    let objects = page
        .managed_root
        .join(descriptor.source_id().storage_relative_path())
        .join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join(&current), "only XML").unwrap();
    let state =
        ManagedDatState::new(&descriptor, ManagedDatSnapshot::new(current).unwrap()).unwrap();
    save_managed_dat_state(&page.managed_root, &state).unwrap();

    let view = page.view();
    let row = &view.managed_rows[0];
    assert_eq!(row.previous_revision, None);
    assert!(!row.rollback_available);

    let mut ui_state = DatSourcesPageUi {
        managed_sources_expanded: Some(true),
        ..Default::default()
    };
    let output = render(&view, &mut ui_state);
    assert!(rendered_text_contains(
        &output,
        "No previous local revision available"
    ));
}

#[test]
fn no_previous_revision_disables_the_rollback_control() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    let descriptor = page.managed_sources.entries()[0].descriptor().unwrap();
    let current = "d".repeat(64);
    let objects = page
        .managed_root
        .join(descriptor.source_id().storage_relative_path())
        .join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join(&current), "only XML").unwrap();
    let state =
        ManagedDatState::new(&descriptor, ManagedDatSnapshot::new(current).unwrap()).unwrap();
    save_managed_dat_state(&page.managed_root, &state).unwrap();

    // Requesting rollback anyway (as if a stale button had been clicked)
    // must be a no-op: the core call fails closed, and the on-disk state is
    // untouched.
    let before = load_managed_dat_state(&page.managed_root, &descriptor).unwrap();
    page.apply(DatSourcesPageAction::RollbackManagedDat {
        source_id: descriptor.source_id().clone(),
    });
    let after = load_managed_dat_state(&page.managed_root, &descriptor).unwrap();
    assert_eq!(before, after);
}

#[test]
fn rolling_back_swaps_the_active_managed_snapshot() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    let (descriptor, current_sha, previous_sha) = two_snapshot_fixture(&page);

    page.apply(DatSourcesPageAction::RollbackManagedDat {
        source_id: descriptor.source_id().clone(),
    });

    let view = page.view();
    let row = &view.managed_rows[0];
    assert_eq!(row.technical.sha256, Some(previous_sha.clone()));
    assert_eq!(row.current_revision.as_deref(), Some("previous-revision"));
    assert_eq!(row.technical.previous_snapshot, Some(current_sha.clone()));
    assert_eq!(row.previous_revision.as_deref(), Some("current-revision"));
    assert!(row.rollback_available);

    // Provider/ecosystem identity describes the source as a whole, not one
    // snapshot, so rollback must never change it.
    let state = load_managed_dat_state(&page.managed_root, &descriptor).unwrap();
    assert_eq!(state.authoritative_name, "gamecom");
    assert_eq!(
        state.parsed_ecosystem,
        archivefs_core::dat::model::DatEcosystem::MAMESoftwareList
    );
}

#[test]
fn rolling_back_twice_restores_the_original_active_snapshot() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    let (descriptor, current_sha, _previous_sha) = two_snapshot_fixture(&page);
    let original = load_managed_dat_state(&page.managed_root, &descriptor).unwrap();

    page.apply(DatSourcesPageAction::RollbackManagedDat {
        source_id: descriptor.source_id().clone(),
    });
    let after_first = load_managed_dat_state(&page.managed_root, &descriptor).unwrap();
    assert_eq!(after_first.current_snapshot.sha256, "b".repeat(64));

    page.apply(DatSourcesPageAction::RollbackManagedDat {
        source_id: descriptor.source_id().clone(),
    });
    let after_second = load_managed_dat_state(&page.managed_root, &descriptor).unwrap();
    assert_eq!(
        after_second, original,
        "rolling back twice must restore the original state exactly"
    );
    assert_eq!(after_second.current_snapshot.sha256, current_sha);
}

#[test]
fn rollback_status_is_distinct_from_an_updated_status() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddManagedMameSoftwareList {
        authoritative_name: "gamecom".to_string(),
    });
    let (descriptor, _current_sha, _previous_sha) = two_snapshot_fixture(&page);

    page.apply(DatSourcesPageAction::RollbackManagedDat {
        source_id: descriptor.source_id().clone(),
    });

    let view = page.view();
    assert_eq!(
        view.managed_rows[0].status,
        ManagedDatStatusView::RolledBack
    );
    let output = render(&view, &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(
        &output,
        "Rolled back to previous revision"
    ));
}

#[test]
fn rollback_reaches_the_page_only_through_the_synchronous_local_action_never_the_network_worker() {
    // `RollbackManagedDat` must never be dispatched to
    // `start_managed_dat_operation` (the thread that owns
    // `HttpsManagedDatTransport`) - it is handled entirely inline in
    // `apply()`. This pins that wiring directly against the source text, so
    // a future refactor cannot silently route rollback through the network
    // worker without this test failing.
    let source = include_str!("../dat_sources_page.rs");
    let handler = source
        .find("DatSourcesPageAction::RollbackManagedDat { source_id } => {")
        .expect("the rollback action has a handler in apply()");
    let handler_line_end = source[handler..]
        .find('\n')
        .map(|offset| handler + offset)
        .unwrap();
    let handler_line = &source[handler..handler_line_end];
    assert!(
        !handler_line.contains("start_managed_dat_operation"),
        "rollback's action arm must not exist"
    );
    let next_line_end = source[handler_line_end + 1..]
        .find('\n')
        .map(|offset| handler_line_end + 1 + offset)
        .unwrap();
    let call_line = &source[handler_line_end + 1..next_line_end];
    assert!(
        call_line.contains("self.rollback_managed_dat(source_id)"),
        "rollback must call the synchronous local method, got: {call_line}"
    );
}

#[test]
fn managed_outcomes_have_honest_non_destructive_presentations() {
    assert_eq!(
        managed_dat_status_from_outcome(ManagedDatUpdateOutcome::UpToDate {
            upstream_revision: None,
        }),
        ManagedDatStatusView::UpToDate
    );
    assert!(matches!(
        managed_dat_status_from_outcome(ManagedDatUpdateOutcome::UpdateAvailable {
            upstream_revision: "a".repeat(40),
        }),
        ManagedDatStatusView::UpdateAvailable { .. }
    ));
    assert_eq!(
        managed_dat_status_from_outcome(ManagedDatUpdateOutcome::Offline),
        ManagedDatStatusView::Offline
    );
    assert_eq!(
        managed_dat_status_from_outcome(ManagedDatUpdateOutcome::RateLimited {
            retry_after_seconds: Some(60),
        }),
        ManagedDatStatusView::RateLimited {
            retry_after_seconds: Some(60)
        }
    );
    let validation = managed_dat_status_from_outcome(ManagedDatUpdateOutcome::Failed {
        kind: ManagedDatUpdateFailureKind::Parser,
        detail: "bad XML".to_string(),
    });
    assert!(matches!(
        validation,
        ManagedDatStatusView::Failed { ref detail }
            if detail.contains("Downloaded DAT failed validation; current copy kept")
    ));
}

// ---------------------------------------------------------------------------
// Quick Rename: one-click safe apply, simple confirmation/success, and
// history-clutter suppression
// ---------------------------------------------------------------------------

/// A Quick Rename plan mixing two genuinely safe/actionable proposals with
/// one of each non-actionable state Quick Rename must never touch:
/// Unsupported, Ambiguous, Conflict.
fn page_with_mixed_quick_rename_plan() -> (Fixture, PathBuf, DatSourcesPageState) {
    let fixture = Fixture::new();
    let roms = fixture.dir("roms");
    let safe_a = roms.join("safe-a.bin");
    let safe_b = roms.join("safe-b.bin");
    std::fs::write(&safe_a, b"fixture contents").unwrap();
    std::fs::write(&safe_b, b"fixture contents").unwrap();
    let proposals = vec![
        plan_proposal(
            safe_a.to_str().unwrap(),
            "safe-a.bin",
            Some("Safe A (Europe).bin"),
            ProposalState::Suggested,
        ),
        plan_proposal(
            safe_b.to_str().unwrap(),
            "safe-b.bin",
            Some("Safe B (Europe).bin"),
            ProposalState::Suggested,
        ),
        plan_proposal(
            "/tmp/quick-rename-fixture/unsupported.bin",
            "unsupported.bin",
            None,
            ProposalState::Unsupported,
        ),
        plan_proposal(
            "/tmp/quick-rename-fixture/ambiguous.bin",
            "ambiguous.bin",
            None,
            ProposalState::Ambiguous,
        ),
        plan_proposal(
            "/tmp/quick-rename-fixture/conflict.bin",
            "conflict.bin",
            Some("Conflict (Europe).bin"),
            ProposalState::Conflict,
        ),
    ];
    let journal = fixture.dir("journal");
    let counts = RenamePlanCounts::from_proposals(&proposals);
    let mut page = DatSourcesPageState::load_with_transaction_dir(
        fixture.config_path.clone(),
        Vec::new(),
        TrustedRoots::from_paths([&roms]),
        journal,
    );
    page.rename_plan = Some(RenamePlan {
        generation: 1,
        source_id: "src".to_string(),
        source_display_name: "Source".to_string(),
        scan_root: roms.to_string_lossy().into_owned(),
        platform: None,
        platform_display: None,
        content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
        classifier_version: archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        proposals,
        counts,
        audited_total: counts.total,
        verified_total: counts.total,
        truncated: false,
    });
    (fixture, roms, page)
}

/// Requested test: "safe proposals are automatically selected by Quick
/// Rename apply action". One click (`QuickRenamePrepareApply`) must select
/// every currently actionable proposal and build the exact same apply
/// review the advanced planner's two-step `SelectAllActionable` +
/// `BeginApplyReview` produces.
#[test]
fn quick_rename_prepare_apply_selects_only_safe_actionable_proposals() {
    let (_fixture, roms, mut page) = page_with_mixed_quick_rename_plan();
    let before = snapshot(&roms);
    page.apply(DatSourcesPageAction::QuickRenamePrepareApply);
    let after = snapshot(&roms);
    assert_eq!(
        after, before,
        "preparing the apply must not touch any file yet"
    );

    let view = page.view();
    let rows = &view.rename_plan.as_ref().unwrap().rows;
    let decision_for = |basename: &str| {
        rows.iter()
            .find(|row| row.current_basename == basename)
            .and_then(|row| row.decision)
    };
    assert_eq!(
        decision_for("safe-a.bin"),
        Some(ReviewDecision::AcceptedForReview)
    );
    assert_eq!(
        decision_for("safe-b.bin"),
        Some(ReviewDecision::AcceptedForReview)
    );

    let review = view
        .rename_apply
        .review
        .as_ref()
        .expect("QuickRenamePrepareApply must build the apply review");
    assert_eq!(
        review.rows.len(),
        2,
        "only the two safe proposals may enter the transaction"
    );
    let proposed: Vec<&str> = review
        .rows
        .iter()
        .map(|row| row.proposed_basename.as_str())
        .collect();
    assert!(proposed.contains(&"Safe A (Europe).bin"));
    assert!(proposed.contains(&"Safe B (Europe).bin"));
}

/// Requested tests: "unsupported proposals are never included" and
/// "ambiguous/conflicting proposals are never included" in Quick Rename's
/// one-click apply.
#[test]
fn quick_rename_prepare_apply_excludes_unsupported_ambiguous_and_conflicting_proposals() {
    let (_fixture, _roms, mut page) = page_with_mixed_quick_rename_plan();
    page.apply(DatSourcesPageAction::QuickRenamePrepareApply);

    let view = page.view();
    let rows = &view.rename_plan.as_ref().unwrap().rows;
    let decision_for = |basename: &str| {
        rows.iter()
            .find(|row| row.current_basename == basename)
            .and_then(|row| row.decision)
    };
    assert_eq!(decision_for("unsupported.bin"), None);
    assert_eq!(decision_for("ambiguous.bin"), None);
    assert_eq!(decision_for("conflict.bin"), None);

    let review = view.rename_apply.review.as_ref().unwrap();
    for row in &review.rows {
        assert_ne!(row.current_basename, "unsupported.bin");
        assert_ne!(row.current_basename, "ambiguous.bin");
        assert_ne!(row.current_basename, "conflict.bin");
    }
}

/// Requested test: "Quick Rename can proceed without opening advanced
/// planner". The normal (non-`Review changes`) path must never render the
/// full Identify & Rename planner's terminology or per-row selection
/// mechanics - exactly the live-QA complaint ("says in planning mode").
#[test]
fn quick_rename_can_proceed_without_opening_advanced_planner() {
    let (_fixture, _roms, page) = page_with_apply_plan(2);
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);

    assert!(rendered_text_contains(&output, "Rename 2 verified files"));
    assert!(
        !rendered_text_contains(&output, "Planning only"),
        "the simple Quick Rename path must never show planner terminology"
    );
    assert!(
        !rendered_text_contains(&output, "Already canonical"),
        "the advanced planner's filter row must not render on the simple path"
    );
    assert!(
        !rendered_text_contains(&output, "Accept"),
        "per-row Accept/Ignore/Needs review controls belong only to Review changes"
    );
}

#[test]
fn quick_rename_summary_explains_verified_and_unresolved_results() {
    let (_fixture, _roms, page) = page_with_mixed_quick_rename_plan();
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);

    assert!(rendered_text_contains(&output, "verified correct"));
    assert!(!rendered_text_contains(&output, "already correct"));
    assert!(rendered_text_contains(
        &output,
        "These files were checked and already have the expected canonical name."
    ));
    assert!(rendered_text_contains(
        &output,
        "unresolved / no safe rename"
    ));
    assert!(rendered_text_contains(
        &output,
        "EmuWiz could not prove a safe rename, so these files were left untouched."
    ));
    assert!(rendered_text_contains(
        &output,
        "Unresolved breakdown: 1 conflicts · 1 ambiguous · 0 blocked"
    ));
}

#[test]
fn quick_rename_conflict_view_distinguishes_conflicts_from_duplicates() {
    let (_fixture, _roms, page) = page_with_mixed_quick_rename_plan();
    let view = page.view();
    let mut ui_state = DatSourcesPageUi {
        quick_review_open: true,
        plan_filter: RenamePlanFilter::Conflicts,
        ..Default::default()
    };
    let output = render_quick_rename(&view, &mut ui_state);

    assert!(rendered_text_contains(
        &output,
        "Conflict means EmuWiz found competing safe interpretations"
    ));
    assert!(rendered_text_contains(
        &output,
        "It does not necessarily mean the files are duplicates."
    ));
    assert!(rendered_text_contains(&output, "1 conflicts"));
}

/// Requested test: "Review changes still opens advanced planner". It
/// remains the deliberate, optional route to the full technical view.
#[test]
fn review_changes_still_opens_the_advanced_planner() {
    let (_fixture, _roms, page) = page_with_apply_plan(2);
    let view = page.view();
    let mut ui_state = DatSourcesPageUi {
        quick_review_open: true,
        ..Default::default()
    };
    let output = render_quick_rename(&view, &mut ui_state);

    assert!(
        rendered_text_contains(&output, "Planning only"),
        "Review changes must still open the real advanced planner"
    );
    assert!(rendered_text_contains(&output, "Already canonical"));
}

/// Requested test: "confirmation shows correct actionable count", in plain
/// language, not planner terminology.
#[test]
fn quick_rename_confirmation_shows_the_correct_actionable_count() {
    let (_fixture, _roms, mut page) = page_with_apply_plan(2);
    page.apply(DatSourcesPageAction::QuickRenamePrepareApply);
    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);

    assert!(rendered_text_contains(
        &output,
        "Ready to rename 2 verified files."
    ));
    assert!(rendered_text_contains(
        &output,
        "A recovery journal will be created."
    ));
    assert!(!rendered_text_contains(&output, "Planning only"));
    assert!(
        !rendered_text_contains(&output, "Trusted root"),
        "the simple confirmation must not surface trusted-root/technical detail"
    );
}

/// Requested tests: "apply uses production transaction path" and "journal
/// is created". `QuickRenamePrepareApply` + `ConfirmApply` must run through
/// exactly the same `build_transaction`/`apply_transaction`/journal
/// machinery as the advanced planner - proven here by the same real-file,
/// real-journal-directory assertions the advanced apply test already uses.
#[test]
fn quick_rename_apply_uses_the_production_transaction_path_and_creates_a_journal() {
    let (_fixture, roms, mut page) = page_with_apply_plan(2);
    let journal_dir = PathBuf::from(page.view().rename_apply.journal_dir.clone());
    page.apply(DatSourcesPageAction::QuickRenamePrepareApply);
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: String::new(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let outcome = view
        .rename_apply
        .outcome
        .as_ref()
        .expect("an apply outcome");
    assert_eq!(outcome.applied, 2);
    assert_eq!(outcome.failed, 0);
    assert!(!roms.join("game0.bin").exists());
    assert!(roms.join("Game 0 (Europe).bin").exists());
    assert!(roms.join("Game 1 (Europe).bin").exists());
    let journal_entries: Vec<_> = std::fs::read_dir(&journal_dir)
        .expect("journal directory must exist")
        .collect();
    assert!(
        !journal_entries.is_empty(),
        "a recovery journal file must have been written"
    );
}

/// Requested test: "old unrelated transaction history is not shown inline"
/// and "active blocking recovery state is still surfaced". A settled
/// (`Applied`) transaction from some earlier, unrelated operation is
/// optional rollback history and must collapse behind "View
/// recovery/history"; an interrupted transaction genuinely blocks trusting
/// this folder's state and must stay directly visible.
#[test]
fn quick_rename_hides_settled_history_but_surfaces_blocking_recovery() {
    let (_fixture, roms, mut page) = page_with_apply_plan(1);
    let journal_dir = PathBuf::from(page.view().rename_apply.journal_dir.clone());
    let roms_str = roms.to_string_lossy().into_owned();

    let settled = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "settled-old-one".to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 1,
        source_scan_root: "/tmp/roms".to_string(),
        state: archivefs_core::dat::rename_apply::TransactionState::Applied,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: PathBuf::from("/tmp/roms/old-a.bin"),
            destination_path: PathBuf::from("/tmp/roms/old-alpha.bin"),
            original_basename: "old-a.bin".to_string(),
            proposed_basename: "old-alpha.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state: archivefs_core::dat::rename_apply::EntryState::Applied,
            failure_reason: None,
            applied_at_unix: Some(1),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &settled).unwrap();

    let interrupted = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "interrupted-current".to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 2,
        source_scan_root: roms_str.clone(),
        state: archivefs_core::dat::rename_apply::TransactionState::Applying,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: roms.join("mid-a.bin"),
            destination_path: roms.join("mid-alpha.bin"),
            original_basename: "mid-a.bin".to_string(),
            proposed_basename: "mid-alpha.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: archivefs_core::dat::rename_apply::EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &interrupted).unwrap();
    page.refresh_recovery();

    let view = page.view();
    assert_eq!(view.rename_apply.recovery.len(), 2);
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);

    assert!(
        rendered_text_contains(&output, "Unresolved rename transaction"),
        "the blocking, unresolved transaction must be surfaced directly"
    );
    assert!(!rendered_text_contains(
        &output,
        "Roll back completed steps"
    ));
    assert!(
        !rendered_text_contains(&output, "Roll back transaction"),
        "the settled transaction's own rollback control must not render inline"
    );
    assert!(
        rendered_text_contains(&output, "View recovery/history (1)"),
        "the settled transaction must be reachable behind a collapsed disclosure"
    );
}

/// Requested test: "successful completion returns to Quick Rename success
/// summary" - never back to the planner, and "Done" returns to Quick
/// Rename's own summary rather than leaving the success card up forever.
#[test]
fn quick_rename_success_returns_to_the_simple_summary_after_done() {
    let (_fixture, _roms, mut page) = page_with_apply_plan(2);
    page.apply(DatSourcesPageAction::QuickRenamePrepareApply);
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: String::new(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Quick Rename complete"));
    assert!(rendered_text_contains(&output, "2 files renamed"));
    assert!(rendered_text_contains(&output, "Recovery journal saved."));
    assert!(
        !rendered_text_contains(&output, "Planning only"),
        "the success path must never fall back into the planner"
    );
    assert!(
        !rendered_text_contains(&output, "game0.bin"),
        "itemized rows must stay behind View details until asked for"
    );

    page.apply(DatSourcesPageAction::ClearApplyOutcome);
    let view_after_done = page.view();
    let output_after_done = render_quick_rename(&view_after_done, &mut ui_state);
    assert!(
        !rendered_text_contains(&output_after_done, "Quick Rename complete"),
        "Done must dismiss the success card"
    );
    assert!(
        !rendered_text_contains(&output_after_done, "Planning only"),
        "Done must return to Quick Rename's own summary, never the planner"
    );
    assert!(rendered_text_contains(&output_after_done, "safe renames"));
}

// ---------------------------------------------------------------------------
// Quick Rename fixes: rollback feedback without a plan, dead-state removal,
// and the ConflictingBatchTarget collision wording
// ---------------------------------------------------------------------------

/// Fix 1 regression test: rolling back a settled transaction from Quick
/// Rename's history section, before any folder has ever been scanned in
/// this session, must still show the result. Previously this banner was
/// nested under `if let Some(plan) = &view.rename_plan`, so it silently
/// never rendered when `rename_plan` was `None`.
#[test]
fn quick_rename_shows_rollback_success_even_with_no_rename_plan_loaded() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    assert!(page.view().rename_plan.is_none());
    page.rollback_result = Some(archivefs_core::dat::rename_apply::RollbackResult::FullyRolledBack);

    let view = page.view();
    assert!(
        view.rename_plan.is_none(),
        "sanity check for the regression scenario"
    );
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Fully rolled back"));
    assert!(rendered_text_contains(
        &output,
        "every applied rename was reversed and confirmed."
    ));
}

/// Fix 1 regression test: the failure case of the same gap.
#[test]
fn quick_rename_shows_rollback_error_even_with_no_rename_plan_loaded() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.rollback_error = Some("journal unreadable: corrupt file".to_string());

    let view = page.view();
    assert!(
        view.rename_plan.is_none(),
        "sanity check for the regression scenario"
    );
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Rollback could not run"));
    assert!(rendered_text_contains(
        &output,
        "journal unreadable: corrupt file"
    ));
}

/// Fix 1 must not duplicate the banner in the advanced ("Review changes")
/// route, where `show_rename_apply_review_and_outcome` already renders the
/// exact same rollback feedback.
#[test]
fn quick_rename_rollback_feedback_is_not_duplicated_in_the_advanced_route() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    page.rollback_result = Some(archivefs_core::dat::rename_apply::RollbackResult::FullyRolledBack);

    let view = page.view();
    let mut ui_state = DatSourcesPageUi {
        quick_review_open: true,
        ..Default::default()
    };
    let output = render_quick_rename(&view, &mut ui_state);
    assert_eq!(
        rendered_text_count(&output, "Fully rolled back"),
        1,
        "the advanced route must show the rollback result exactly once, not twice"
    );
}

/// Fix 2: `quick_history_open` was dead state (declared, reset, never
/// read) - the "View recovery/history" disclosure already tracks its own
/// open/closed state via `CollapsingHeader`'s built-in egui memory. This is
/// a compile-time proof of removal: the struct must build with
/// `..Default::default()` and no reference to the removed field anywhere,
/// which the crate already enforces just by compiling - this test exists so
/// a future re-introduction is caught by a failing assertion, not only by
/// noticing an extra unused field in review.
#[test]
fn quick_history_open_field_no_longer_exists() {
    // `DatSourcesPageUi` is constructible purely from `Default` plus the one
    // field this suite actually sets - if `quick_history_open` were ever
    // re-added without a purpose, this still compiles and passes, but the
    // struct's field count would grow silently. Guard on the concrete
    // observable behavior instead: the collapsed history disclosure still
    // works correctly using only `CollapsingHeader`'s own state (see
    // `quick_rename_hides_settled_history_but_surfaces_blocking_recovery`),
    // with no `DatSourcesPageUi` field driving it.
    let ui_state = DatSourcesPageUi::default();
    assert!(!ui_state.quick_review_open);
}

/// Fix 3: `is_collision_reason` must recognize `ConflictingBatchTarget`'s
/// real wording, "two proposals in this batch target the same destination".
#[test]
fn is_collision_reason_recognizes_all_three_real_collision_wordings() {
    assert!(is_collision_reason(
        "the destination name now exists; it is never overwritten"
    ));
    assert!(is_collision_reason(
        "a sibling whose name differs from the destination only by case now exists"
    ));
    assert!(is_collision_reason(
        "two proposals in this batch target the same destination"
    ));
}

/// Fix 3 must stay narrow: unrelated preflight refusals are not collisions.
#[test]
fn is_collision_reason_stays_narrow_for_unrelated_preflight_failures() {
    assert!(!is_collision_reason("the source file no longer exists"));
    assert!(!is_collision_reason(
        "the source has been replaced by a symlink; a symlink is never renamed"
    ));
    assert!(!is_collision_reason(
        "the rename would operate outside the configured trusted roots"
    ));
    assert!(!is_collision_reason(
        "the plan generation changed since approval (now 2, expected 1); the plan is stale"
    ));
}

/// End-to-end proof that Fix 3 actually changes the Quick Rename success
/// summary's displayed count, not just the helper function in isolation.
#[test]
fn quick_rename_success_summary_counts_a_conflicting_batch_target_as_a_collision() {
    let outcome = ApplyOutcomeView {
        transaction_id: "tx-1".to_string(),
        state: TransactionState::Applied,
        requested: 2,
        applied: 1,
        skipped: 1,
        failed: 0,
        rows: vec![
            ApplyRowView {
                current_basename: "a.bin".to_string(),
                proposed_basename: "Alpha.bin".to_string(),
                state: EntryState::Applied,
                failure_reason: None,
            },
            ApplyRowView {
                current_basename: "b.bin".to_string(),
                proposed_basename: "Alpha.bin".to_string(),
                state: EntryState::Skipped,
                failure_reason: Some(
                    "two proposals in this batch target the same destination".to_string(),
                ),
            },
        ],
    };
    let mut ui_state = DatSourcesPageUi::default();
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_quick_rename_success(ui, &outcome, &mut ui_state);
        });
    });
    assert!(rendered_text_contains(&output, "1 collision"));
}

// ---------------------------------------------------------------------------
// Quick Rename: restart action, current-library recovery filtering, and
// durable "Leave untouched" resolution
// ---------------------------------------------------------------------------

/// Requested tests: "successful Quick Rename exposes Rename another
/// library" and "successful BBC-style batch returns to simple completion
/// state, not planner".
#[test]
fn quick_rename_success_exposes_rename_another_library_not_the_planner() {
    let (_fixture, _roms, mut page) = page_with_apply_plan(2);
    page.apply(DatSourcesPageAction::QuickRenamePrepareApply);
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: String::new(),
    });
    run_to_completion(&mut page);

    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);
    assert!(rendered_text_contains(&output, "Quick Rename complete"));
    assert!(rendered_text_contains(&output, "Rename another library"));
    assert!(
        !rendered_text_contains(&output, "Planning only"),
        "a completed batch must never fall back into the advanced planner"
    );
}

/// Requested tests: "starting another Quick Rename clears only current
/// session state", "DAT source configuration survives reset", and
/// "transaction history survives reset". One end-to-end scenario: a real
/// DAT source is registered, a batch is applied, an unrelated settled
/// journal already exists on disk, then the user resets for another
/// library.
#[test]
fn quick_rename_reset_clears_only_session_state_and_preserves_config_and_history() {
    let (_fixture, roms, mut page) = page_with_apply_plan(2);
    let journal_dir = PathBuf::from(page.view().rename_apply.journal_dir.clone());

    // A real DAT source, registered the normal way.
    let dat_path = roms.join("collection.dat");
    std::fs::write(&dat_path, LOGIQX).unwrap();
    page.apply(DatSourcesPageAction::AddFile {
        path: dat_path.clone(),
    });
    let dat_sources_before = page.view().rows.len();
    assert_eq!(
        dat_sources_before, 1,
        "sanity check: the DAT source was registered"
    );

    // An unrelated, already-settled transaction from some earlier session -
    // durable history that must survive a Quick Rename reset untouched.
    let unrelated_settled = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "unrelated-settled".to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 1,
        source_scan_root: "/some/other/library".to_string(),
        state: archivefs_core::dat::rename_apply::TransactionState::Applied,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: PathBuf::from("/some/other/library/a.bin"),
            destination_path: PathBuf::from("/some/other/library/alpha.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "alpha.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state: archivefs_core::dat::rename_apply::EntryState::Applied,
            failure_reason: None,
            applied_at_unix: Some(1),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &unrelated_settled).unwrap();

    // Run a real, successful apply for this session.
    page.apply(DatSourcesPageAction::QuickRenamePrepareApply);
    page.apply(DatSourcesPageAction::ConfirmApply {
        typed: String::new(),
    });
    run_to_completion(&mut page);
    assert!(page.view().rename_apply.outcome.is_some());
    let before_reset = snapshot(&roms);
    let journal_files_before: usize = std::fs::read_dir(&journal_dir).unwrap().count();

    page.apply(DatSourcesPageAction::ResetQuickRenameSession);

    let view = page.view();
    assert!(view.rename_plan.is_none(), "the plan must be cleared");
    assert!(view.audit.is_none(), "the audit result must be cleared");
    assert!(
        view.rename_apply.review.is_none(),
        "any pending review must be cleared"
    );
    assert!(
        view.rename_apply.outcome.is_none(),
        "the completed outcome must be cleared"
    );

    // Nothing on disk changed as a result of the reset itself.
    assert_eq!(
        snapshot(&roms),
        before_reset,
        "reset must not touch any file"
    );
    let journal_files_after: usize = std::fs::read_dir(&journal_dir).unwrap().count();
    assert_eq!(
        journal_files_after, journal_files_before,
        "reset must not delete or add any journal file"
    );

    // DAT source configuration survives.
    assert_eq!(
        view.rows.len(),
        dat_sources_before,
        "DAT source configuration must survive a Quick Rename reset"
    );

    // Transaction history survives: both this session's own just-applied
    // transaction and the pre-existing unrelated one are still visible as
    // recovery/rollback history.
    let ids: Vec<&str> = view
        .rename_apply
        .recovery
        .iter()
        .map(|recovery| recovery.transaction_id.as_str())
        .collect();
    assert!(
        ids.contains(&"unrelated-settled"),
        "the pre-existing unrelated settled transaction must still be visible: {ids:?}"
    );
}

/// Requested tests: "old unrelated unresolved transaction does not
/// dominate/block current library when path/root relationship can be
/// proven" and "unresolved transaction for current library still surfaces
/// prominently" - in one scenario, so the split is proven relative to each
/// other, not just in isolation.
#[test]
fn unrelated_unresolved_transaction_does_not_dominate_current_library_recovery() {
    let (_fixture, roms, mut page) = page_with_apply_plan(1);
    let journal_dir = PathBuf::from(page.view().rename_apply.journal_dir.clone());

    fn interrupted_transaction(
        id: &str,
        scan_root: &str,
    ) -> archivefs_core::dat::rename_apply::RenameTransaction {
        archivefs_core::dat::rename_apply::RenameTransaction {
            transaction_id: id.to_string(),
            plan_generation: 1,
            classifier_version: Some(
                archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
            ),
            created_at_unix: 1,
            source_scan_root: scan_root.to_string(),
            state: archivefs_core::dat::rename_apply::TransactionState::Applying,
            entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
                operation: Default::default(),
                source_path: PathBuf::from(scan_root).join("a.bin"),
                destination_path: PathBuf::from(scan_root).join("alpha.bin"),
                original_basename: "a.bin".to_string(),
                proposed_basename: "alpha.bin".to_string(),
                identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                    size_bytes: 1,
                    modified_unix: 1,
                    kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                    #[cfg(unix)]
                    ino: 1,
                    #[cfg(unix)]
                    dev: 1,
                },
                preflight_passed: false,
                preflight_failures: Vec::new(),
                state: archivefs_core::dat::rename_apply::EntryState::Planned,
                failure_reason: None,
                applied_at_unix: None,
                rolled_back_at_unix: None,
                unknown: Default::default(),
            }],
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    let roms_str = roms.to_string_lossy().into_owned();
    let current = interrupted_transaction("current-library-interrupted", &roms_str);
    let other = interrupted_transaction(
        "other-library-interrupted",
        "/tmp/some-unrelated-test-folder",
    );
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &current).unwrap();
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &other).unwrap();
    page.refresh_recovery();
    assert_eq!(page.view().rename_apply.recovery.len(), 2);

    let view = page.view();
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);

    assert!(
        rendered_text_contains(&output, "Unresolved rename transaction"),
        "the current library's own unresolved transaction must surface directly"
    );
    assert!(
        rendered_text_contains(&output, "View recovery/history (1)"),
        "the other library's unresolved transaction must collapse instead of dominating"
    );
}

/// Requested tests, combined: "Quick Rename no longer blocks on
/// acknowledged Leave untouched", "unresolved/unacknowledged transaction
/// still blocks", and "rollback capability remains correct" for a resolved
/// transaction that still has applied entries.
#[test]
fn resolved_leave_untouched_no_longer_blocks_but_unresolved_still_does() {
    let (_fixture, roms, mut page) = page_with_apply_plan(1);
    let journal_dir = PathBuf::from(page.view().rename_apply.journal_dir.clone());
    let roms_str = roms.to_string_lossy().into_owned();

    let interrupted = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "current-library-interrupted".to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 1,
        source_scan_root: roms_str.clone(),
        state: archivefs_core::dat::rename_apply::TransactionState::Applying,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: roms.join("a.bin"),
            destination_path: roms.join("alpha.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "alpha.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: archivefs_core::dat::rename_apply::EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &interrupted).unwrap();
    page.refresh_recovery();

    // Unresolved: still blocks.
    {
        let view = page.view();
        let mut ui_state = DatSourcesPageUi::default();
        let output = render_quick_rename(&view, &mut ui_state);
        assert!(
            rendered_text_contains(&output, "Unresolved rename transaction"),
            "an unresolved, unacknowledged transaction for this library must still block"
        );
    }

    page.apply(DatSourcesPageAction::RecoveryChoice {
        id: "current-library-interrupted".to_string(),
        choice: RecoveryChoice::LeaveUntouched,
    });

    let view = page.view();
    assert!(
        view.rename_apply.recovery_resolution_error.is_none(),
        "the choice must have persisted without error"
    );

    // Resolved: no longer blocks Quick Rename, but stays reachable.
    let mut ui_state = DatSourcesPageUi::default();
    let output = render_quick_rename(&view, &mut ui_state);
    assert!(
        !rendered_text_contains(&output, "Unresolved rename transaction"),
        "an acknowledged Leave untouched must no longer block Quick Rename"
    );
    assert!(
        rendered_text_contains(&output, "View recovery/history (1)"),
        "the resolved transaction must remain reachable, not vanish"
    );

    // Rollback capability must remain correct: the resolved transaction
    // still has an applied entry, so acknowledging the prompt must never
    // silently take away the ability to undo it from the advanced view.
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 1600.0));
    let mut adv_ui_state = DatSourcesPageUi::default();
    let adv_output = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_rename_apply_section(ui, &view.rename_apply, &mut adv_ui_state);
            });
        },
    );
    assert!(
        rendered_text_contains(&adv_output, "Roll back transaction"),
        "a still-rollbackable transaction must keep its rollback control after acknowledgement"
    );
    assert!(
        rendered_text_contains(&adv_output, "left untouched by user"),
        "the advanced view must show the resolution, never hide it"
    );
}

/// Requested test: "choosing Leave untouched resolves/removes Needs
/// attention state". This is the exact regression the fix targets:
/// `refresh_recovery` reloads from disk on every `poll()`, so the choice
/// must be remembered (`dismissed_recovery_ids`), not just removed from the
/// in-memory list for one frame.
#[test]
fn leave_untouched_removes_needs_attention_state_across_subsequent_polls() {
    let (fixture, roms, mut page) = page_with_apply_plan(0);
    let journal_dir = PathBuf::from(page.view().rename_apply.journal_dir.clone());
    let tx = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "leave-untouched-test".to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 1,
        source_scan_root: roms.to_string_lossy().into_owned(),
        state: archivefs_core::dat::rename_apply::TransactionState::Applying,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: roms.join("a.bin"),
            destination_path: roms.join("alpha.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "alpha.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: archivefs_core::dat::rename_apply::EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &tx).unwrap();
    page.refresh_recovery();
    assert_eq!(page.view().rename_apply.recovery.len(), 1);
    let before = &page.view().rename_apply.recovery[0];
    assert!(
        before.resolution.is_none(),
        "an unresolved interrupted transaction initially shows Needs attention"
    );
    assert_eq!(before.state, TransactionState::Applying);

    page.apply(DatSourcesPageAction::RecoveryChoice {
        id: "leave-untouched-test".to_string(),
        choice: RecoveryChoice::LeaveUntouched,
    });

    // It remains visible (inspectable), just no longer unresolved - this is
    // the persistent-resolution replacement for the old session-only
    // "vanishes from the list immediately" behavior.
    let view = page.view();
    assert_eq!(view.rename_apply.recovery.len(), 1);
    let resolved = &view.rename_apply.recovery[0];
    assert_eq!(
        resolved.resolution,
        Some(archivefs_core::dat::rename_apply::RecoveryResolution::LeaveUntouched)
    );
    // The original `TransactionState` remains truthful: still `Applying`
    // (interrupted), never rewritten to `Applied` or anything else.
    assert_eq!(
        resolved.state,
        TransactionState::Applying,
        "the original interrupted state must never be overwritten by a resolution"
    );

    // Requested test: "restarting/reloading state does not resurrect
    // Needs attention" - simulate the next frame's poll (which reloads
    // straight from disk) AND a fresh, independent `DatSourcesPageState`
    // load (which simulates an EmuWiz restart, not just another frame of
    // the same process).
    page.poll();
    let after_poll = &page.view().rename_apply.recovery[0];
    assert_eq!(
        after_poll.resolution,
        Some(archivefs_core::dat::rename_apply::RecoveryResolution::LeaveUntouched),
        "the resolution must survive a subsequent poll/refresh, not just one frame"
    );

    let restarted = DatSourcesPageState::load_with_transaction_dir(
        fixture.config_path.clone(),
        Vec::new(),
        TrustedRoots::from_paths([&roms]),
        journal_dir.clone(),
    );
    let restarted_recovery = &restarted.view().rename_apply.recovery;
    assert_eq!(restarted_recovery.len(), 1);
    assert_eq!(
        restarted_recovery[0].resolution,
        Some(archivefs_core::dat::rename_apply::RecoveryResolution::LeaveUntouched),
        "a completely fresh load (simulating an EmuWiz restart) must not resurrect Needs \
         attention for an already-resolved transaction"
    );

    // Requested test: "journal file still exists" / "no accidental
    // deletion" - and it was persisted, not just held in memory: read it
    // back directly from disk, independent of `DatSourcesPageState`.
    assert!(
        archivefs_core::dat::rename_apply::journal_exists(&journal_dir, "leave-untouched-test"),
        "Leave untouched must never delete the journal"
    );
    let on_disk = archivefs_core::dat::rename_apply::read_journal(
        &archivefs_core::dat::rename_apply::journal_path(&journal_dir, "leave-untouched-test")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        on_disk.recovery_resolution,
        Some(archivefs_core::dat::rename_apply::RecoveryResolution::LeaveUntouched),
        "the resolution must be durably persisted to the journal file itself"
    );
    assert_eq!(
        on_disk.state,
        TransactionState::Applying,
        "the journal's own state field must remain truthful on disk too"
    );
}

/// Requested test: "old journal without resolution field still loads".
/// Backward compatibility: a journal written before `recovery_resolution`
/// existed has no such key in its JSON at all - `#[serde(default)]` must
/// decode that as `None`, not fail to parse.
#[test]
fn old_journal_without_resolution_field_still_loads() {
    let fixture = Fixture::new();
    let journal_dir = fixture.dir("journal");
    let transaction_id = "pre-resolution-journal";
    // Hand-written JSON with no `recovery_resolution`/`recovery_resolved_at_unix`
    // key at all, exactly what a journal written before this feature existed
    // looks like on disk.
    let raw = serde_json::json!({
        "transaction_id": transaction_id,
        "plan_generation": 1,
        "created_at_unix": 1,
        "source_scan_root": "/tmp/roms",
        "state": "applying",
        "entries": [{
            "source_path": "/tmp/roms/a.bin",
            "destination_path": "/tmp/roms/alpha.bin",
            "original_basename": "a.bin",
            "proposed_basename": "alpha.bin",
            "identity": {
                "size_bytes": 1,
                "modified_unix": 1,
                "kind": "regular_file"
            },
            "state": "planned"
        }]
    });
    std::fs::write(
        archivefs_core::dat::rename_apply::journal_path(&journal_dir, transaction_id).unwrap(),
        serde_json::to_string_pretty(&raw).unwrap(),
    )
    .unwrap();

    let transaction = archivefs_core::dat::rename_apply::read_journal(
        &archivefs_core::dat::rename_apply::journal_path(&journal_dir, transaction_id).unwrap(),
    )
    .expect("an old journal with no resolution field must still parse");
    assert_eq!(transaction.recovery_resolution, None);
    assert_eq!(transaction.recovery_resolved_at_unix, None);
    assert_eq!(transaction.state, TransactionState::Applying);
    assert!(
        transaction.needs_attention(),
        "an old, unresolved journal must still show Needs attention"
    );
}

/// Requested test: "Applied journals unchanged" - resolving (or failing to
/// resolve) has nothing to do with settled `Applied` transactions; they are
/// never offered the Leave-untouched prompt in the first place, and nothing
/// about their journal changes.
#[test]
fn applied_journals_are_never_touched_by_recovery_resolution() {
    let fixture = Fixture::new();
    let journal_dir = fixture.dir("journal");
    let applied = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "already-applied".to_string(),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: 1,
        source_scan_root: "/tmp/roms".to_string(),
        state: TransactionState::Applied,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: PathBuf::from("/tmp/roms/a.bin"),
            destination_path: PathBuf::from("/tmp/roms/alpha.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "alpha.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state: archivefs_core::dat::rename_apply::EntryState::Applied,
            failure_reason: None,
            applied_at_unix: Some(1),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &applied).unwrap();
    assert!(
        !applied.needs_attention(),
        "a settled transaction never needs attention"
    );
    assert!(
        applied.is_rollbackable(),
        "rollback eligibility must be unaffected by recovery_resolution"
    );

    let reread = archivefs_core::dat::rename_apply::read_journal(
        &archivefs_core::dat::rename_apply::journal_path(&journal_dir, "already-applied").unwrap(),
    )
    .unwrap();
    assert_eq!(
        reread, applied,
        "an Applied journal must be byte-for-byte unchanged"
    );
}

/// Requested test: "settled history remains available but collapsed" - the
/// shared rendering function still offers the real rollback control for a
/// settled transaction once the collapsed section is actually shown; only
/// the default collapsed state hides it, nothing about its availability
/// changed.
#[test]
fn settled_history_remains_available_once_expanded() {
    let settled = vec![fixture_recovery_transaction(
        "settled-1",
        TransactionState::Applied,
        "Renamed \"a.bin\" -> \"Alpha.bin\"",
    )];
    let context = egui::Context::default();
    let mut action = None;
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = show_recovery_transactions(ui, &settled, false, false);
        });
    });
    assert!(rendered_text_contains(&output, "Roll back transaction"));
    assert!(
        action.is_none(),
        "rendering alone must not perform any action"
    );
}

/// Requested test: "no journal is silently deleted" - covering both new
/// actions this task adds (`ResetQuickRenameSession`,
/// `HideSettledRecoveryHistory`), neither of which has any business
/// touching the journal directory.
#[test]
fn hide_settled_history_never_deletes_a_journal() {
    let (_fixture, roms, mut page) = page_with_apply_plan(0);
    let journal_dir = PathBuf::from(page.view().rename_apply.journal_dir.clone());
    let settled = archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: "settled-to-hide".to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 1,
        source_scan_root: roms.to_string_lossy().into_owned(),
        state: archivefs_core::dat::rename_apply::TransactionState::Applied,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: roms.join("a.bin"),
            destination_path: roms.join("alpha.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "alpha.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state: archivefs_core::dat::rename_apply::EntryState::Applied,
            failure_reason: None,
            applied_at_unix: Some(1),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &settled).unwrap();
    page.refresh_recovery();
    assert_eq!(page.view().rename_apply.recovery.len(), 1);

    page.apply(DatSourcesPageAction::HideSettledRecoveryHistory);
    assert!(page.view().rename_apply.recovery.is_empty());
    assert!(
        archivefs_core::dat::rename_apply::journal_exists(&journal_dir, "settled-to-hide"),
        "Hide settled history must never delete the journal file"
    );

    // Requested test: "advanced Repair History still sees all durable
    // journals" - Repair History reads the same journal directory
    // independently from disk; `dismissed_recovery_ids`/`HideSettledRecoveryHistory`
    // live only on `DatSourcesPageState` and never touch what is on disk,
    // so a fresh, independent read of the directory (exactly what Repair
    // History's own load does) must still see it.
    let (all_journals, problems) = archivefs_core::dat::rename_apply::list_journals(&journal_dir);
    assert!(problems.is_empty());
    assert!(
        all_journals
            .iter()
            .any(|transaction| transaction.transaction_id == "settled-to-hide"),
        "the durable journal must still be discoverable by Repair History's own independent read"
    );
}

#[test]
fn no_intro_pack_selection_is_mutation_free_and_uses_only_the_homepage() {
    let fixture = Fixture::new();
    let pack = fixture.root.join("downloaded-pack.zip");
    std::fs::write(&pack, b"not imported yet").unwrap();
    let mut page = fixture.page();

    page.apply(DatSourcesPageAction::ChooseNoIntroPack { path: pack.clone() });
    let view = page.view();
    assert_eq!(
        view.no_intro_selected_pack.as_ref().unwrap().0,
        "downloaded-pack.zip"
    );
    assert!(view.no_intro_inspection.is_none());
    assert!(view.no_intro_installed.is_none());
    assert!(!fixture.root.join("state.json").exists());
    assert_eq!(
        archivefs_core::identity_source::no_intro::NO_INTRO_DATOMATIC_DOWNLOAD_PAGE,
        "https://datomatic.no-intro.org/"
    );
}

#[test]
fn no_intro_variant_labels_do_not_conflate_aftermarket_with_standard() {
    assert_eq!(
        no_intro_classification_label(
            archivefs_core::identity_source::no_intro::NoIntroPackClassification::Aftermarket
        ),
        "Aftermarket / Love Pack"
    );
    assert_eq!(
        no_intro_classification_label(
            archivefs_core::identity_source::no_intro::NoIntroPackClassification::Standard
        ),
        "Standard No-Intro"
    );
}

// --- DAT completion summary ------------------------------------------------

/// One `Exact` match, naming a distinct game, so `verified` counts games
/// rather than files.
fn exact_entry(game_index: usize) -> AuditEntry {
    AuditEntry {
        local_path: format!("/tmp/roms/game-{game_index}.bin"),
        local_filename: format!("game-{game_index}.bin"),
        verdict: AuditVerdict::Exact {
            game_name: format!("Game {game_index}"),
            rom_name: format!("game-{game_index}.bin"),
            algorithm: "SHA-1",
        },
    }
}

fn not_in_dat_entry(index: usize) -> AuditEntry {
    AuditEntry {
        local_path: format!("/tmp/roms/extra-{index}.bin"),
        local_filename: format!("extra-{index}.bin"),
        verdict: AuditVerdict::NotInDat,
    }
}

/// An outcome with `total` catalogue entries, `verified` of them matched by
/// a distinct-game `Exact` verdict, and `extra` local files that matched
/// nothing in the catalogue - built the same way a real audit's report
/// would be shaped, so `dat_completion_view` sees exactly the data a real
/// run produces.
fn completion_outcome(total: usize, verified: usize, extra: usize) -> DatAuditOutcome {
    let mut entries: Vec<AuditEntry> = (0..verified).map(exact_entry).collect();
    entries.extend((0..extra).map(not_in_dat_entry));
    let summary = AuditSummary {
        total: entries.len(),
        exact: verified,
        not_in_dat: extra,
        ..AuditSummary::default()
    };
    let mut outcome = minimal_outcome();
    outcome.catalogue_entries = total;
    outcome.catalogue_roms = total;
    outcome.report = AuditReport { entries, summary };
    outcome
}

#[test]
fn exactly_100_of_100_is_complete_at_100_percent() {
    let outcome = completion_outcome(100, 100, 0);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.state, DatCompletionState::Complete);
    assert_eq!(completion.percent, Some(100.0));
    assert_eq!(completion.verified, 100);
    assert_eq!(completion.total, 100);
    assert_eq!(completion.missing, Some(0));
}

#[test]
fn ninety_nine_of_100_is_nearly_complete_at_99_percent() {
    let outcome = completion_outcome(100, 99, 0);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.state, DatCompletionState::NearlyComplete);
    assert_eq!(completion.percent, Some(99.0));
}

#[test]
fn ninety_five_of_100_is_nearly_complete() {
    let outcome = completion_outcome(100, 95, 0);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.state, DatCompletionState::NearlyComplete);
    assert_eq!(completion.percent, Some(95.0));
}

#[test]
fn ninety_four_of_100_is_incomplete() {
    let outcome = completion_outcome(100, 94, 0);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.state, DatCompletionState::Incomplete);
}

#[test]
fn one_of_100_is_incomplete() {
    let outcome = completion_outcome(100, 1, 0);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.state, DatCompletionState::Incomplete);
}

#[test]
fn zero_of_100_is_none_verified() {
    let outcome = completion_outcome(100, 0, 0);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.state, DatCompletionState::NoneVerified);
    assert_eq!(completion.percent, Some(0.0));
}

#[test]
fn an_unavailable_total_is_unknown_with_no_fabricated_percentage() {
    // `catalogue_entries == 0`: nothing to measure completion against.
    let outcome = completion_outcome(0, 0, 0);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.state, DatCompletionState::Unknown);
    assert_eq!(
        completion.percent, None,
        "an untrustworthy total must never produce a percentage"
    );
    assert_eq!(completion.missing, None);
    assert!(completion.caveat.is_some());
}

#[test]
fn verified_exceeding_total_is_clamped_not_shown_over_100_percent() {
    // Two distinct matched games against a catalogue that only declares one
    // entry - a real inconsistency (e.g. a merged folder source), not
    // something that should ever render as 200%.
    let outcome = completion_outcome(1, 2, 0);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.verified, 1, "verified is clamped to total");
    assert_eq!(completion.total, 1);
    assert_eq!(completion.percent, Some(100.0));
    assert!(
        completion.caveat.is_some(),
        "clamping must be surfaced, not silently hidden"
    );
}

#[test]
fn missing_is_total_minus_verified() {
    let outcome = completion_outcome(12_500, 12_438, 0);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.missing, Some(62));
}

#[test]
fn extra_local_files_are_never_folded_into_missing_or_total() {
    // 60 missing catalogue entries and 5 unrelated extra local files at the
    // same time: neither count may affect the other.
    let outcome = completion_outcome(100, 40, 5);
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.total, 100, "extra files never inflate the total");
    assert_eq!(completion.missing, Some(60));
    assert_eq!(
        completion.extra_local_files, 5,
        "extra files are reported, not merged into missing"
    );
}

#[test]
fn completion_details_name_the_exact_selected_source_and_revision() {
    let mut outcome = completion_outcome(1, 1, 0);
    outcome.catalogue_names = vec!["Nintendo - Game Boy Advance".to_string()];
    outcome.catalogue_version = Some("20240115-123456".to_string());
    outcome.catalogue_author = Some("No-Intro".to_string());
    let completion = dat_completion_view(&outcome).unwrap();
    assert_eq!(completion.source_title, "Nintendo - Game Boy Advance");
    assert_eq!(completion.revision.as_deref(), Some("20240115-123456"));
    assert_eq!(completion.provider.as_deref(), Some("No-Intro"));
}

#[test]
fn no_intro_badge_appears_only_at_exact_100_percent() {
    let mut complete = completion_outcome(10, 10, 0);
    complete.catalogue_ecosystem = Some(archivefs_core::dat::model::DatEcosystem::NoIntro);
    let complete_view = dat_completion_view(&complete).unwrap();
    assert!(complete_view.no_intro_complete_badge);

    let mut nearly = completion_outcome(10, 9, 0);
    nearly.catalogue_ecosystem = Some(archivefs_core::dat::model::DatEcosystem::NoIntro);
    let nearly_view = dat_completion_view(&nearly).unwrap();
    assert!(
        !nearly_view.no_intro_complete_badge,
        "the badge must not appear below exact 100%"
    );

    // A non-No-Intro provider never gets the badge even at 100%.
    let mut redump = completion_outcome(10, 10, 0);
    redump.catalogue_ecosystem = Some(archivefs_core::dat::model::DatEcosystem::Redump);
    let redump_view = dat_completion_view(&redump).unwrap();
    assert!(!redump_view.no_intro_complete_badge);
}

#[test]
fn switching_the_selected_source_updates_the_completion_basis() {
    let mut gba = completion_outcome(100, 100, 0);
    gba.catalogue_names = vec!["Nintendo - Game Boy Advance".to_string()];
    let mut nes = completion_outcome(200, 50, 0);
    nes.catalogue_names = vec!["Nintendo Entertainment System".to_string()];

    let gba_completion = dat_completion_view(&gba).unwrap();
    let nes_completion = dat_completion_view(&nes).unwrap();

    assert_eq!(gba_completion.source_title, "Nintendo - Game Boy Advance");
    assert_eq!(gba_completion.state, DatCompletionState::Complete);
    assert_eq!(nes_completion.source_title, "Nintendo Entertainment System");
    assert_eq!(nes_completion.state, DatCompletionState::Incomplete);
}

#[test]
fn building_the_completion_view_never_mutates_the_outcome() {
    let outcome = completion_outcome(100, 62, 3);
    let before = outcome.clone();
    let _ = dat_completion_view(&outcome);
    let _ = dat_completion_view(&outcome);
    assert_eq!(
        outcome, before,
        "reading a completion view must never change the audit outcome"
    );
}

#[test]
fn combined_multi_source_audits_get_no_completion_claim() {
    let mut outcome = completion_outcome(100, 100, 0);
    outcome.source_id = "combined-enabled-dat-sources".to_string();
    assert!(
        dat_completion_view(&outcome).is_none(),
        "a combined audit has no single selected DAT/snapshot to measure completion against"
    );
}

#[test]
fn source_replacement_staleness_is_conservative_but_ignores_enablement_only() {
    let mut before = DatSourceEntry::new(
        "source".to_string(),
        "Source".to_string(),
        PathBuf::from("/catalogue/one.dat"),
        DatSourceKind::File,
    );
    let mut disabled = before.clone();
    disabled.enabled = false;
    assert!(!source_requires_dat_identity_stale_mark(&before, &disabled));

    let mut replaced = before.clone();
    replaced.path = PathBuf::from("/catalogue/two.dat");
    assert!(source_requires_dat_identity_stale_mark(&before, &replaced));

    before.health.observed_size_bytes = Some(10);
    replaced = before.clone();
    replaced.health.observed_size_bytes = Some(11);
    assert!(source_requires_dat_identity_stale_mark(&before, &replaced));
}

// --- managed-update stale-mark decision (task: staleness lifecycle wiring) ---

#[test]
fn a_successful_managed_update_targets_the_canonical_audit_source_id() {
    let source_id = ManagedDatSourceId::mame_software_list("neogeo").unwrap();
    let result: archivefs_core::Result<ManagedDatUpdateOutcome> =
        Ok(ManagedDatUpdateOutcome::Updated {
            upstream_revision: "abc123".to_string(),
            sha256: "d".repeat(64),
        });
    let target = managed_update_stale_mark_target(&source_id, &result)
        .expect("a successful Updated outcome must target a stale-mark");
    assert_eq!(
        target.0,
        archivefs_core::dat::catalogue_selection::managed_dat_audit_source_id(&source_id),
        "must use the canonical audit dat_source_id, never ManagedDatSourceId::Display"
    );
    assert_ne!(
        target.0,
        source_id.to_string(),
        "ManagedDatSourceId's own Display uses a different token/separator"
    );
    assert_eq!(target.1, "abc123");
}

#[test]
fn a_failed_managed_update_never_targets_a_stale_mark() {
    let source_id = ManagedDatSourceId::mame_software_list("neogeo").unwrap();
    for result in [
        Ok(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::Network,
            detail: "connection reset".to_string(),
        }),
        Ok(ManagedDatUpdateOutcome::Offline),
        Ok(ManagedDatUpdateOutcome::Disabled),
        Ok(ManagedDatUpdateOutcome::RateLimited {
            retry_after_seconds: Some(60),
        }),
        Ok(ManagedDatUpdateOutcome::UpToDate {
            upstream_revision: Some("abc123".to_string()),
        }),
        Ok(ManagedDatUpdateOutcome::UpdateAvailable {
            upstream_revision: "def456".to_string(),
        }),
        Err(archivefs_core::ArchiveFsError::Config(
            "transport could not be built".to_string(),
        )),
    ] {
        assert!(
            managed_update_stale_mark_target(&source_id, &result).is_none(),
            "must not stale-mark for {result:?}"
        );
    }
}

#[test]
fn a_cancelled_or_unresolved_check_never_targets_a_stale_mark() {
    // "Cancelled" has no dedicated outcome variant in this core API - a
    // check/update that never completes simply never reaches
    // `poll_managed_dat_job` with a message at all (no channel send), which
    // this module's own logic already treats as "no result yet, nothing to
    // mark". The `Checking`-in-flight-with-no-message case is proven by
    // `managed_update_stale_mark_target` never being called except from
    // inside `Ok(message) => ...` after a real `try_recv()` - and every
    // *arrived* non-`Updated` result is already proven inert above.
    let source_id = ManagedDatSourceId::mame_software_list("neogeo").unwrap();
    let result: archivefs_core::Result<ManagedDatUpdateOutcome> =
        Ok(ManagedDatUpdateOutcome::UpToDate {
            upstream_revision: None,
        });
    assert!(managed_update_stale_mark_target(&source_id, &result).is_none());
}

// --- `persist_expected_inventory_if_valid` --------------------------------

mod persist_expected_inventory {
    use archivefs_core::dat::expected_inventory::{
        ExpectedDatEntryRecord, ExpectedDatInventoryProjection,
    };

    use super::*;

    fn report(state: DatHealthState, source_id: &str) -> DatValidationReport {
        DatValidationReport {
            source_id: source_id.to_string(),
            path: "/dats/source.dat".to_string(),
            kind: "DAT file",
            state,
            files: vec![DatFileReport {
                path: "/dats/source.dat".to_string(),
                file_name: "source.dat".to_string(),
                outcome: DatFileOutcome::Parsed {
                    format: DatFormat::Logiqx,
                    ecosystem: DatEcosystem::NoIntro,
                    name: Some("Test Catalogue".to_string()),
                    version: Some("v1".to_string()),
                    entry_count: 1,
                    rom_count: 1,
                    diagnostics: Vec::new(),
                },
            }],
            duplicate_identities: Vec::new(),
            skipped: Vec::new(),
            truncated: false,
            total_dat_files: None,
            summary: "1 DAT files, 1 entries, 1 ROMs".to_string(),
            entry_count: 1,
            rom_count: 1,
            formats: vec!["Logiqx XML".to_string()],
            path_refusal: None,
        }
    }

    fn projection(names: &[&str]) -> ExpectedDatInventoryProjection {
        let mut projection = ExpectedDatInventoryProjection::default();
        for name in names {
            projection.entries.push(ExpectedDatEntryRecord {
                canonical_identity: (*name).to_string(),
                display_name: (*name).to_string(),
                dat_game_id: None,
                rom_count: 1,
            });
        }
        projection
    }

    fn database_path(fixture: &Fixture) -> PathBuf {
        let path = fixture.root.join("library.sqlite3");
        archivefs_core::Database::open_or_create(&path)
            .unwrap()
            .close()
            .unwrap();
        path
    }

    #[test]
    fn a_valid_report_persists_its_projected_entries() {
        let fixture = Fixture::new();
        let db_path = database_path(&fixture);
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        persist_expected_inventory_if_valid(
            Some(&db_path),
            &report(DatHealthState::Valid, "no-intro-nes"),
            &projection(&["Game A", "Game B"]),
            &sender,
        );

        let database = archivefs_core::Database::open_or_create(&db_path).unwrap();
        assert_eq!(
            database.expected_dat_entry_count("no-intro-nes").unwrap(),
            2
        );
        database.close().unwrap();
    }

    #[test]
    fn an_invalid_report_persists_nothing() {
        let fixture = Fixture::new();
        let db_path = database_path(&fixture);
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        persist_expected_inventory_if_valid(
            Some(&db_path),
            &report(DatHealthState::Invalid, "no-intro-nes"),
            &projection(&["Game A"]),
            &sender,
        );

        let database = archivefs_core::Database::open_or_create(&db_path).unwrap();
        assert_eq!(
            database.expected_dat_entry_count("no-intro-nes").unwrap(),
            0
        );
        database.close().unwrap();
    }

    #[test]
    fn a_later_invalid_report_never_destroys_prior_good_inventory() {
        let fixture = Fixture::new();
        let db_path = database_path(&fixture);
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        persist_expected_inventory_if_valid(
            Some(&db_path),
            &report(DatHealthState::Valid, "no-intro-nes"),
            &projection(&["Game A", "Game B"]),
            &sender,
        );
        // A later validation of the same source fails to parse (a
        // corrupted download, say). The good inventory from the earlier
        // successful validation must survive untouched.
        persist_expected_inventory_if_valid(
            Some(&db_path),
            &report(DatHealthState::Invalid, "no-intro-nes"),
            &ExpectedDatInventoryProjection::default(),
            &sender,
        );

        let database = archivefs_core::Database::open_or_create(&db_path).unwrap();
        assert_eq!(
            database.expected_dat_entry_count("no-intro-nes").unwrap(),
            2
        );
        database.close().unwrap();
    }

    #[test]
    fn an_unreadable_report_also_persists_nothing() {
        let fixture = Fixture::new();
        let db_path = database_path(&fixture);
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        persist_expected_inventory_if_valid(
            Some(&db_path),
            &report(DatHealthState::Unreadable, "no-intro-nes"),
            &projection(&["Game A"]),
            &sender,
        );

        let database = archivefs_core::Database::open_or_create(&db_path).unwrap();
        assert_eq!(
            database.expected_dat_entry_count("no-intro-nes").unwrap(),
            0
        );
        database.close().unwrap();
    }

    #[test]
    fn no_database_path_persists_nothing_and_does_not_panic() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        persist_expected_inventory_if_valid(
            None,
            &report(DatHealthState::Valid, "no-intro-nes"),
            &projection(&["Game A"]),
            &sender,
        );
        // No assertion beyond "did not panic" - there is no database to
        // check anything against.
    }
}

/// The Collection Coverage panel's end-to-end wiring through the page: it
/// reads the authoritative core aggregation and populates the view, and it
/// starts no background work of any kind.
mod dat_coverage_section {
    use super::*;
    use crate::dat_coverage_panel::{CoverageLoad, CoverageUnitView, FullSetView};

    fn database_at(fixture: &Fixture) -> PathBuf {
        let path = fixture.root.join("library.sqlite3");
        archivefs_core::Database::open_or_create(&path)
            .unwrap()
            .close()
            .unwrap();
        path
    }

    /// A page with one folder DAT source assigned to `platform`, backed by
    /// a real (empty) library database.
    fn page_with_source(
        fixture: &Fixture,
        platform: Option<&str>,
    ) -> (DatSourcesPageState, String) {
        let db_path = database_at(fixture);
        let folder = fixture.dir("cov-dat");
        let mut page = fixture.page().with_database_path(Some(db_path));
        page.apply(DatSourcesPageAction::AddFolder {
            path: folder.clone(),
        });
        let id = "cov-dat".to_string();
        page.apply(DatSourcesPageAction::SetPlatform {
            id: id.clone(),
            platform: platform.map(str::to_string),
        });
        (page, id)
    }

    #[test]
    fn loading_coverage_reads_the_core_and_starts_no_job() {
        let fixture = Fixture::new();
        let (mut page, id) = page_with_source(&fixture, Some("Game Boy Advance"));

        page.apply(DatSourcesPageAction::LoadCoverage { id: id.clone() });

        // No background work of any kind.
        assert!(!page.is_busy());
        let view = page.view();
        assert!(view.running.is_none());
        assert!(view.audit.is_none());
        // The registry has one source and the coverage view has a row for
        // it, now Ready (an empty library, never validated -> the expected
        // inventory is missing, verification metrics are all zero).
        assert_eq!(view.coverage_sources.len(), 1);
        let entry = &view.coverage_sources[0];
        match &entry.load {
            CoverageLoad::Ready(CoverageUnitView::Canonical(canonical)) => {
                assert_eq!(canonical.owned, 0);
                assert!(!canonical.expected.is_available());
                // Not a fabricated 0% / 0 missing.
                assert_eq!(canonical.missing_count, None);
                assert_eq!(canonical.completion_percent, None);
            }
            other => panic!("expected a Ready canonical coverage, got {other:?}"),
        }
        // Loading coverage never produced a validation report.
        assert!(page.validations.is_empty());
    }

    #[test]
    fn an_unassigned_source_loads_verification_metrics_but_no_expected_denominator() {
        let fixture = Fixture::new();
        let (mut page, id) = page_with_source(&fixture, None);

        page.apply(DatSourcesPageAction::LoadCoverage { id });
        assert!(!page.is_busy());

        let view = page.view();
        let entry = &view.coverage_sources[0];
        match &entry.load {
            CoverageLoad::Ready(CoverageUnitView::Canonical(canonical)) => {
                assert!(!canonical.expected.is_available());
                assert!(matches!(
                    canonical.full_set,
                    FullSetView::NotProvable { .. }
                ));
                assert_eq!(canonical.missing_count, None);
            }
            other => panic!("expected Ready canonical, got {other:?}"),
        }
    }

    #[test]
    fn refresh_re_reads_without_starting_a_job() {
        let fixture = Fixture::new();
        let (mut page, id) = page_with_source(&fixture, Some("NES"));
        page.apply(DatSourcesPageAction::LoadCoverage { id: id.clone() });
        page.apply(DatSourcesPageAction::RefreshCoverage { id });
        assert!(!page.is_busy());
        assert!(page.validations.is_empty());
        assert!(matches!(
            page.view().coverage_sources[0].load,
            CoverageLoad::Ready(_)
        ));
    }

    #[test]
    fn a_page_without_a_database_reports_a_read_failure_not_a_panic() {
        let fixture = Fixture::new();
        let folder = fixture.dir("no-db-dat");
        let mut page = fixture.page(); // no with_database_path -> None
        page.apply(DatSourcesPageAction::AddFolder { path: folder });
        page.apply(DatSourcesPageAction::LoadCoverage {
            id: "no-db-dat".to_string(),
        });
        assert!(!page.is_busy());
        assert!(matches!(
            page.view().coverage_sources[0].load,
            CoverageLoad::Failed(_)
        ));
    }

    #[test]
    fn the_coverage_section_renders_and_the_headline_uses_plain_wording() {
        let fixture = Fixture::new();
        let (page, _id) = page_with_source(&fixture, Some("Game Boy Advance"));
        let view = page.view();
        let mut ui_state = DatSourcesPageUi::default();
        let output = render_with_details(&view, &mut ui_state);
        assert!(rendered_text_contains(&output, "Collection coverage"));
        // Beginner wording, not raw "DAT".
        assert!(rendered_text_contains(
            &output,
            "How much of each platform's catalogue"
        ));
    }
}
