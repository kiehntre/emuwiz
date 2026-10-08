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

#[test]
fn truncation_warns_but_verified_listed_records_stay_actionable() {
    let dir = tempfile::tempdir().unwrap();
    write_journal(
        dir.path(),
        "ps2-psu-restore-00000.json",
        Ps2RestorePhase::Published,
    );
    write_journal(
        dir.path(),
        "ps2-psu-restore-00001.json",
        Ps2RestorePhase::Staged,
    );
    for i in 2..=1024 {
        std::fs::write(
            dir.path().join(format!("ps2-psu-restore-{i:05}.json")),
            b"corrupt synthetic journal",
        )
        .unwrap();
    }
    let ctx = egui::Context::default();
    let shown = render(&ctx, dir.path());
    assert!(
        has(&shown, WARNING) && has(&shown, "1 more exist"),
        "{shown:?}"
    );
    assert!(
        has(&shown, "Review Undo"),
        "a verified listed record keeps its action"
    );
    assert!(has(&shown, "Check interrupted restores"));
    let history = ctx
        .data_mut(|data| data.get_temp::<History>(history_id()))
        .unwrap();
    assert_eq!(history.rows.len(), 1024);
    assert_eq!(history.problems.len(), 1);
}

#[test]
fn exactly_1024_records_show_no_warning() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..1024 {
        std::fs::write(
            dir.path().join(format!("ps2-psu-restore-{i:05}.json")),
            b"corrupt synthetic journal",
        )
        .unwrap();
    }
    let shown = render(&egui::Context::default(), dir.path());
    assert!(!has(&shown, WARNING));
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
