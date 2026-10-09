//! History/Recovery must warn when discovery is incomplete and must never
//! present an incomplete list as the whole inventory. Synthetic data only.
use super::*;
use archivefs_core::memory_card_inventory::restore_guard::{FileIdentity, Ps2RestoreJournal};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;

const WARNING: &str = "INCOMPLETE";

fn texts(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

fn render(ctx: &egui::Context, dir: &Path) -> Vec<String> {
    texts(&ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show_in(ui, true, dir));
    }))
}

fn has(texts: &[String], needle: &str) -> bool {
    texts.iter().any(|text| text.contains(needle))
}

fn write_journal(dir: &Path, file: &str, phase: Ps2RestorePhase) {
    let card = dir.join("synthetic-card.ps2");
    let journal = Ps2RestoreJournal {
        version: 1,
        operation_id: file.into(),
        phase,
        binding: ps2_card_binding(&card, "synthetic save"),
        card_path: card.clone(),
        psu_path: dir.join("synthetic.psu"),
        psu_sha256: "synthetic".into(),
        backup_path: dir.join("synthetic-backup.ps2"),
        backup_sha256: Some("synthetic".into()),
        original_sha256: "synthetic".into(),
        original_size: 0,
        original_identity: FileIdentity {
            device: 0,
            inode: 0,
            size: 0,
            mtime_seconds: 0,
            mtime_nanoseconds: 0,
        },
        staged_path: None,
        staged_sha256: None,
        post_sha256: Some("synthetic".into()),
        post_identity: None,
        undo_identity: None,
        save_display_name: "synthetic save".into(),
        file_count: 1,
        detail: None,
        history: vec![(phase, 1)],
    };
    let body = serde_json::to_vec(&journal).unwrap();
    let mut bytes = format!(
        "EMUWIZ-PS2-RESTORE-JOURNAL v1 sha256={}\n",
        Sha256::digest(&body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
    .into_bytes();
    bytes.extend(body);
    std::fs::write(dir.join(file), bytes).unwrap();
}

fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .is_ok_and(|s| s.lines().any(|l| l.starts_with("Uid:\t0\t")))
}

#[test]
fn a_complete_empty_inventory_shows_no_warning() {
    let dir = tempfile::tempdir().unwrap();
    let shown = render(&egui::Context::default(), dir.path());
    assert!(!has(&shown, WARNING) && !has(&shown, "Check interrupted restores"));
}

#[test]
fn an_unreadable_journal_folder_warns_and_never_looks_empty() {
    if running_as_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let journals = dir.path().join("journals");
    std::fs::create_dir(&journals).unwrap();
    write_journal(&journals, "ps2-psu-restore-a.json", Ps2RestorePhase::Staged);
    std::fs::set_permissions(&journals, std::fs::Permissions::from_mode(0o000)).unwrap();
    let ctx = egui::Context::default();
    let shown = render(&ctx, &journals);
    std::fs::set_permissions(&journals, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(has(&shown, WARNING), "{shown:?}");
    assert!(has(&shown, "cannot be listed") && has(&shown, "os error 13"));
    assert!(!has(&shown, "Review Undo") && !has(&shown, "Check interrupted restores"));
    // Refresh after the cause is fixed clears the warning and shows the record.
    ctx.data_mut(|data| data.remove::<History>(history_id()));
    let fixed = render(&ctx, &journals);
    assert!(!has(&fixed, WARNING) && has(&fixed, "Check interrupted restores"));
}

/// `count` records named 00000..: corrupt ones older, then a Staged and a
/// Published record as the two newest names.
fn newest_valid_dir(count: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // Quiet filler (a finished, valid record draws no row), so the two newest
    // records are not pushed out of the visible area by hundreds of rows.
    for i in 0..count - 2 {
        write_journal(
            dir.path(),
            &format!("ps2-psu-restore-{i:05}.json"),
            Ps2RestorePhase::Undone,
        );
    }
    write_journal(
        dir.path(),
        &format!("ps2-psu-restore-{:05}.json", count - 2),
        Ps2RestorePhase::Staged,
    );
    write_journal(
        dir.path(),
        &format!("ps2-psu-restore-{:05}.json", count - 1),
        Ps2RestorePhase::Published,
    );
    dir
}

#[test]
fn truncation_warns_and_withholds_review_undo_and_the_recovery_check() {
    let dir = newest_valid_dir(1025);
    let ctx = egui::Context::default();
    let shown = render(&ctx, dir.path());
    assert!(has(&shown, WARNING), "{shown:?}");
    assert!(has(&shown, "only the newest 1024"), "{shown:?}");
    assert!(has(&shown, "1 older records are not shown"), "{shown:?}");
    assert!(has(&shown, "withheld"), "{shown:?}");
    // The newest records are listed (so the operator can see them) ...
    let history = ctx
        .data_mut(|data| data.get_temp::<History>(history_id()))
        .unwrap();
    assert_eq!(history.rows.len(), 1024);
    assert!(history.rows.iter().any(|row| row.undo_available));
    // ... but no list-dependent action is offered, under any control.
    assert!(!has(&shown, "Review Undo"), "{shown:?}");
    assert!(!has(&shown, "Check interrupted restores"), "{shown:?}");
    assert!(!has(&shown, "Confirm Undo"), "{shown:?}");
}

#[test]
fn an_already_open_undo_review_is_withheld_when_discovery_is_incomplete() {
    let dir = newest_valid_dir(1025);
    let journal = dir.path().join("ps2-psu-restore-01024.json");
    let ctx = egui::Context::default();
    // Complete discovery: the open review is allowed.
    let complete_dir = newest_valid_dir(3);
    let complete_journal = complete_dir.path().join("ps2-psu-restore-00002.json");
    render(&ctx, complete_dir.path());
    let open = |path: &Path| {
        texts(&ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = confirm_undo(ui, path, true);
            });
        }))
    };
    let shown = open(&complete_journal);
    assert!(has(&shown, "Review Undo from restore record"));
    assert!(!has(&shown, "Undo is withheld"), "{shown:?}");
    // The listing becomes truncated while that review is open (Refresh records).
    ctx.data_mut(|data| data.remove::<History>(history_id()));
    render(&ctx, dir.path());
    let shown = open(&journal);
    assert!(has(&shown, "Undo is withheld"), "{shown:?}");
    assert!(has(&shown, "INCOMPLETE"), "{shown:?}");
}

#[test]
fn actions_return_after_refresh_produces_complete_discovery() {
    let dir = newest_valid_dir(1025);
    let ctx = egui::Context::default();
    let shown = render(&ctx, dir.path());
    assert!(has(&shown, WARNING) && !has(&shown, "Review Undo"));
    // The user moves the oldest record out of the folder (EmuWiz deletes nothing).
    let aside = tempfile::tempdir().unwrap();
    std::fs::rename(
        dir.path().join("ps2-psu-restore-00000.json"),
        aside.path().join("ps2-psu-restore-00000.json"),
    )
    .unwrap();
    // Refresh records == re-discovery replacing the cached result.
    ctx.data_mut(|data| data.remove::<History>(history_id()));
    let fixed = render(&ctx, dir.path());
    assert!(!has(&fixed, WARNING), "{fixed:?}");
    assert!(has(&fixed, "Review Undo"), "{fixed:?}");
    assert!(has(&fixed, "Check interrupted restores"), "{fixed:?}");
}

#[test]
fn exactly_1024_records_show_no_warning_and_keep_actions() {
    let dir = newest_valid_dir(1024);
    let shown = render(&egui::Context::default(), dir.path());
    assert!(!has(&shown, WARNING));
    assert!(has(&shown, "Review Undo") && has(&shown, "Check interrupted restores"));
}

#[test]
fn a_failed_listing_blocks_the_batch_recovery_claim_of_nothing_to_do() {
    // The batch run reports its discovery problems instead of an empty success.
    if running_as_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let journals = dir.path().join("journals");
    std::fs::create_dir(&journals).unwrap();
    std::fs::set_permissions(&journals, std::fs::Permissions::from_mode(0o000)).unwrap();
    let run = recover_all_interrupted_ps2_restores(&journals, 1);
    std::fs::set_permissions(&journals, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(run.outcomes.is_empty() && !run.problems.is_empty());
}

// ---- unknown discovery state (no cached result) fails closed ----

/// Draws the Undo review for `journal` and, when `click`, presses and releases the
/// pointer on "Confirm Undo". Returns what the review asked the dialog to do on the
/// release frame, and the texts of the first frame. Deterministic: fixed frames.
fn review_undo(
    ctx: &egui::Context,
    journal: &Path,
    click: bool,
) -> (Option<PsuRestoreDialogState>, Vec<String>) {
    let frame = |input: egui::RawInput| {
        let mut asked = None;
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                asked = confirm_undo(ui, journal, true);
            });
        });
        (asked, out)
    };
    let (_, first) = frame(egui::RawInput::default());
    let shown = texts(&first);
    if !click {
        return (None, shown);
    }
    let pos = first
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "Confirm Undo" => Some(text.pos),
            _ => None,
        })
        .expect("Confirm Undo is drawn");
    let at = pos + egui::vec2(6.0, 4.0);
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let mut press = egui::RawInput::default();
    press.events = vec![egui::Event::PointerMoved(at), button(true)];
    frame(press);
    let mut release = egui::RawInput::default();
    release.events = vec![button(false)];
    let (asked, _) = frame(release);
    (asked, shown)
}

fn one_published_journal() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    write_journal(
        dir.path(),
        "ps2-psu-restore-00001.json",
        Ps2RestorePhase::Published,
    );
    let path = dir.path().join("ps2-psu-restore-00001.json");
    (dir, path)
}

#[test]
fn no_cached_discovery_is_unknown_and_withholds_undo_with_honest_wording() {
    let (_dir, journal) = one_published_journal();
    let ctx = egui::Context::default();
    assert!(
        ctx.data_mut(|d| d.get_temp::<History>(history_id()))
            .is_none()
    );
    let (asked, shown) = review_undo(&ctx, &journal, true);
    assert!(has(&shown, "Review Undo from restore record"), "{shown:?}");
    assert!(
        has(
            &shown,
            "Undo is unavailable until the restore records have been checked"
        ),
        "{shown:?}"
    );
    // Never claims a state that was not established.
    assert!(!has(&shown, "INCOMPLETE"), "{shown:?}");
    assert!(!has(&shown, "cannot be listed"), "{shown:?}");
    // A real click on Confirm Undo asks for nothing, and the journal is untouched.
    assert!(asked.is_none(), "Undo asked for a dialog transition");
    assert_eq!(
        load_ps2_restore_journal(&journal).unwrap().phase,
        Ps2RestorePhase::Published
    );
}

#[test]
fn the_click_handler_refuses_on_its_own_when_the_cache_is_absent() {
    // Even with the button forced to look enabled, the handler re-reads the state.
    let (_dir, journal) = one_published_journal();
    let ctx = egui::Context::default();
    for _ in 0..2 {
        let (asked, _) = review_undo(&ctx, &journal, true);
        assert!(asked.is_none(), "Undo asked for a dialog transition");
    }
    // The state was never populated by merely drawing the review.
    assert!(
        ctx.data_mut(|d| d.get_temp::<History>(history_id()))
            .is_none()
    );
}

#[test]
fn first_complete_discovery_restores_normal_eligibility() {
    let (dir, journal) = one_published_journal();
    let ctx = egui::Context::default();
    assert!(review_undo(&ctx, &journal, true).0.is_none());
    // The page's own discovery populates the cache (complete listing).
    let shown = render(&ctx, dir.path());
    assert!(!has(&shown, WARNING), "{shown:?}");
    let (asked, shown) = review_undo(&ctx, &journal, true);
    assert!(!has(&shown, "Undo is unavailable"), "{shown:?}");
    assert!(!has(&shown, "Undo is withheld"), "{shown:?}");
    // Normal existing behaviour: the click reaches the guarded Undo path (which
    // then applies its own per-record checks and may refuse).
    assert!(asked.is_some(), "a click must reach the guarded Undo path");
}

#[test]
fn known_incomplete_discovery_still_withholds_the_click() {
    let dir = newest_valid_dir(1025);
    let journal = dir.path().join("ps2-psu-restore-01024.json");
    let ctx = egui::Context::default();
    render(&ctx, dir.path());
    let (asked, shown) = review_undo(&ctx, &journal, true);
    assert!(has(&shown, "INCOMPLETE"), "{shown:?}");
    assert!(!has(&shown, "Undo is unavailable until"), "{shown:?}");
    assert!(asked.is_none(), "Undo asked for a dialog transition");
}

#[test]
fn refresh_from_unknown_or_incomplete_to_complete_restores_eligible_actions() {
    // unknown -> complete
    let (dir, journal) = one_published_journal();
    let ctx = egui::Context::default();
    assert!(review_undo(&ctx, &journal, true).0.is_none());
    render(&ctx, dir.path());
    assert!(review_undo(&ctx, &journal, true).0.is_some());
    // incomplete -> complete (the user moves the oldest record aside, then Refresh)
    let big = newest_valid_dir(1025);
    let newest = big.path().join("ps2-psu-restore-01024.json");
    let ctx = egui::Context::default();
    render(&ctx, big.path());
    assert!(review_undo(&ctx, &newest, true).0.is_none());
    let aside = tempfile::tempdir().unwrap();
    std::fs::rename(
        big.path().join("ps2-psu-restore-00000.json"),
        aside.path().join("ps2-psu-restore-00000.json"),
    )
    .unwrap();
    ctx.data_mut(|d| d.remove::<History>(history_id())); // what Refresh records replaces
    render(&ctx, big.path());
    assert!(review_undo(&ctx, &newest, true).0.is_some());
}
