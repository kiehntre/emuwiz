use super::*;
use crate::patch_manager::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyStatus, SharedRollbackConfirmation,
    SharedRollbackOptions,
};
use std::fs;
use std::io::Write;
use tempfile::tempdir;

const TITLE: &str = "00050000101010ED";

fn pack(root: &std::path::Path) {
    fs::create_dir_all(root.join("content/shaders/nested")).unwrap();
    fs::write(
        root.join("rules.txt"),
        format!("[Definition]\nname = Safe Pack\ntitleIds = {TITLE}\n"),
    )
    .unwrap();
    fs::write(root.join("content/main.bin"), b"content").unwrap();
    fs::write(root.join("content/shaders/nested/effect_ps.txt"), b"shader").unwrap();
}

#[test]
fn valid_nested_pack_is_inspected_without_flattening() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("pack");
    pack(&root);
    let inspection = inspect_cemu_graphic_pack(&root).unwrap();
    assert_eq!(inspection.pack_name, "Safe Pack");
    assert_eq!(inspection.title_ids, vec![TITLE]);
    assert_eq!(
        inspection.files,
        vec![
            std::path::PathBuf::from("content/main.bin"),
            std::path::PathBuf::from("content/shaders/nested/effect_ps.txt"),
            std::path::PathBuf::from("rules.txt"),
        ]
    );
}

#[test]
fn zip_package_is_staged_and_inspected_without_touching_the_source() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("pack.zip");
    {
        let file = fs::File::create(&source).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::FileOptions::<()>::default();
        archive.start_file("Safe Pack/rules.txt", options).unwrap();
        archive
            .write_all(format!("[Definition]\nname = Safe Pack\ntitleIds = {TITLE}\n").as_bytes())
            .unwrap();
        archive
            .start_file("Safe Pack/content/main.bin", options)
            .unwrap();
        archive.write_all(b"content").unwrap();
        archive.finish().unwrap();
    }
    let before = fs::read(&source).unwrap();
    let preview = inspect_cemu_graphic_pack_zip(&source).unwrap();
    assert_eq!(preview.inspection.pack_name, "Safe Pack");
    assert_eq!(fs::read(&source).unwrap(), before);
    let _ = fs::remove_dir_all(preview.staging_root);
}

#[test]
fn missing_or_malformed_rules_are_rejected() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("pack");
    fs::create_dir_all(&root).unwrap();
    assert_eq!(
        inspect_cemu_graphic_pack(&root).unwrap_err().kind,
        CemuGraphicPackErrorKind::MissingRules
    );
    fs::write(root.join("rules.txt"), "[Definition]\nname = no id\n").unwrap();
    assert_eq!(
        inspect_cemu_graphic_pack(&root).unwrap_err().kind,
        CemuGraphicPackErrorKind::TargetIdentityMissing
    );
}

#[test]
fn traversal_and_symlink_members_are_rejected() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("pack");
    pack(&root);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("rules.txt"), root.join("content/link.txt")).unwrap();
        assert_eq!(
            inspect_cemu_graphic_pack(&root).unwrap_err().kind,
            CemuGraphicPackErrorKind::UnsafeMember
        );
    }
}

#[test]
fn conflicting_target_identity_is_refused() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("pack");
    pack(&root);
    let inspection = inspect_cemu_graphic_pack(&root).unwrap();
    let error = build_cemu_graphic_pack_plan(
        &inspection,
        "0005000012345678",
        &dir.path().join("game"),
        &dir.path().join("graphicPacks"),
    )
    .unwrap_err();
    assert_eq!(error.kind, CemuGraphicPackErrorKind::TargetNotDeclared);
}

#[test]
fn apply_rollback_and_conflict_preserve_unrelated_files() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("pack");
    pack(&root);
    let destination = dir.path().join("graphicPacks");
    fs::create_dir_all(destination.join("Other Pack")).unwrap();
    fs::write(destination.join("Other Pack/unrelated.txt"), b"keep").unwrap();
    let inspection = inspect_cemu_graphic_pack(&root).unwrap();
    let plan =
        build_cemu_graphic_pack_plan(&inspection, TITLE, &dir.path().join("game"), &destination)
            .unwrap();
    assert_eq!(plan.report.summary.install_new, 3);
    let transaction = build_cemu_graphic_pack_transaction_plan(&plan, "cemu:test").unwrap();
    let history = dir.path().join("history");
    let backups = dir.path().join("backups");
    let applied = apply_cemu_graphic_pack(
        &transaction,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: transaction.plan_id.clone(),
                general_approved: true,
                replacement_approved: true,
            }),
            operation_id: "cemu-test-apply".into(),
            timestamp_unix_seconds: 1,
            current_context: transaction.context.clone(),
            history_root: history.clone(),
            backup_root: backups.clone(),
        },
    );
    assert_eq!(applied.apply.journal.status, SharedApplyStatus::Success);
    assert_eq!(
        fs::read(destination.join("Other Pack/unrelated.txt")).unwrap(),
        b"keep"
    );
    let journal = applied.apply.journal_path.unwrap();
    let preview = preview_cemu_graphic_pack_rollback(&journal, &destination, &backups);
    assert!(preview.available);
    let rollback = rollback_cemu_graphic_pack(
        &preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: "cemu-test-rollback".into(),
            timestamp_unix_seconds: 2,
            history_root: history,
            backup_root: backups,
        },
    );
    assert_eq!(rollback.status, SharedApplyStatus::Success);
    assert!(!destination.join("Safe Pack/rules.txt").exists());
    assert!(destination.join("Other Pack/unrelated.txt").exists());
}

#[test]
fn identical_existing_files_are_skipped_and_different_files_are_replacements() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("pack");
    pack(&root);
    let destination = dir.path().join("graphicPacks");
    fs::create_dir_all(destination.join("Safe Pack/content")).unwrap();
    fs::write(destination.join("Safe Pack/content/main.bin"), b"content").unwrap();
    let inspection = inspect_cemu_graphic_pack(&root).unwrap();
    let plan =
        build_cemu_graphic_pack_plan(&inspection, TITLE, &dir.path().join("game"), &destination)
            .unwrap();
    assert_eq!(plan.report.summary.already_installed, 1);
    assert_eq!(plan.report.summary.install_new, 2);
    fs::create_dir_all(destination.join("Safe Pack/content/shaders/nested")).unwrap();
    fs::write(
        destination.join("Safe Pack/content/shaders/nested/effect_ps.txt"),
        b"different",
    )
    .unwrap();
    let changed =
        build_cemu_graphic_pack_plan(&inspection, TITLE, &dir.path().join("game"), &destination)
            .unwrap();
    assert_eq!(changed.report.summary.replace_different, 1);
    assert_eq!(changed.report.summary.install_new, 1);
}
