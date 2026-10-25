use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::*;
use crate::game_identity::{
    GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat, IdentityKind,
    IdentityPlatform, IdentityProvenance, IdentityStatus,
};
use crate::patch_manager::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyStatus, SharedRollbackConfirmation,
    SharedRollbackOptions,
};
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

const TITLE: &str = "BLUS30000";

fn sfo(title_id: &str) -> Vec<u8> {
    let key = b"TITLE_ID\0";
    let mut bytes = vec![0u8; 20 + 16 + key.len() + title_id.len() + 1];
    bytes[0..4].copy_from_slice(b"\0PSF");
    bytes[8..12].copy_from_slice(&(36u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&((36 + key.len()) as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
    bytes[20..22].copy_from_slice(&0u16.to_le_bytes());
    bytes[22..24].copy_from_slice(&0x0204u16.to_le_bytes());
    bytes[24..28].copy_from_slice(&((title_id.len() + 1) as u32).to_le_bytes());
    bytes[28..32].copy_from_slice(&((title_id.len() + 1) as u32).to_le_bytes());
    bytes[32..36].copy_from_slice(&0u32.to_le_bytes());
    bytes[36..36 + key.len()].copy_from_slice(key);
    bytes[36 + key.len()..36 + key.len() + title_id.len()].copy_from_slice(title_id.as_bytes());
    bytes
}

fn package(root: &Path) {
    fs::create_dir_all(root.join("PS3_GAME/USRDIR/mod/nested")).unwrap();
    fs::write(root.join("PS3_GAME/PARAM.SFO"), sfo(TITLE)).unwrap();
    fs::write(root.join("PS3_GAME/USRDIR/mod/nested/file.bin"), b"new mod").unwrap();
}

fn report(title_id: &str, archive_path: &Path) -> GameIdentityReport {
    GameIdentityReport {
        archive_path: archive_path.to_path_buf(),
        platform: IdentityPlatform::PlayStation3,
        format: IdentityImageFormat::Iso,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::Ps3TitleId,
            status: IdentityStatus::Verified,
            value: Some(title_id.to_string()),
            confidence: IdentityConfidence::ExactBytes,
            provenance: IdentityProvenance {
                archive_path: archive_path.to_path_buf(),
                member_path: None,
                member_index: None,
                method: "test fixture".into(),
            },
            diagnostic: "verified test identity".into(),
        }],
        warnings: Vec::new(),
        bytes_read: 1,
        archive_members_inspected: 0,
        metadata_paths_inspected: 1,
        nested_container_depth: 0,
        complete: true,
    }
}

#[test]
fn valid_wrapper_preserves_nested_layout_and_requires_package_identity() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("wrapper/mod");
    package(&root);
    let inspection = inspect_rpcs3_ordinary_mod(&dir.path().join("wrapper")).unwrap();
    assert_eq!(inspection.declared_title_id, TITLE);
    assert_eq!(
        inspection.files,
        vec![PathBuf::from("PS3_GAME/USRDIR/mod/nested/file.bin")]
    );
    let selected = dir.path().join("game.iso");
    let identity = report(TITLE, &selected);
    let plan = build_rpcs3_ordinary_mod_plan(&inspection, &identity, &dir.path().join("game-root"))
        .unwrap();
    assert_eq!(
        plan.report.entries[0]
            .destination_relative_path
            .as_deref()
            .unwrap(),
        Path::new("USRDIR/mod/nested/file.bin")
    );
}

#[test]
fn missing_malformed_and_empty_packages_fail_closed() {
    let dir = tempdir().unwrap();
    let empty = dir.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        inspect_rpcs3_ordinary_mod(&empty).unwrap_err().kind,
        Rpcs3OrdinaryModErrorKind::MissingIdentity
    );
    let malformed = dir.path().join("malformed");
    fs::create_dir_all(malformed.join("PS3_GAME")).unwrap();
    fs::write(malformed.join("PS3_GAME/PARAM.SFO"), b"bad").unwrap();
    assert_eq!(
        inspect_rpcs3_ordinary_mod(&malformed).unwrap_err().kind,
        Rpcs3OrdinaryModErrorKind::MalformedIdentity
    );
    let identity_only = dir.path().join("identity-only");
    fs::create_dir_all(identity_only.join("PS3_GAME")).unwrap();
    fs::write(identity_only.join("PS3_GAME/PARAM.SFO"), sfo(TITLE)).unwrap();
    assert_eq!(
        inspect_rpcs3_ordinary_mod(&identity_only).unwrap_err().kind,
        Rpcs3OrdinaryModErrorKind::EmptyPackage
    );
}

#[test]
fn conflicting_identity_and_selected_identity_are_refused() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("conflict");
    package(&root);
    fs::write(root.join("PARAM.SFO"), sfo("BLES00001")).unwrap();
    assert_eq!(
        inspect_rpcs3_ordinary_mod(&root).unwrap_err().kind,
        Rpcs3OrdinaryModErrorKind::ConflictingIdentity
    );

    let other = dir.path().join("other");
    package(&other);
    let inspection = inspect_rpcs3_ordinary_mod(&other).unwrap();
    let selected = dir.path().join("game.iso");
    let identity = report("BLES00001", &selected);
    assert_eq!(
        build_rpcs3_ordinary_mod_plan(&inspection, &identity, &dir.path().join("game-root"))
            .unwrap_err()
            .kind,
        Rpcs3OrdinaryModErrorKind::IdentityConflict
    );
}

#[test]
fn zip_traversal_and_symlink_members_are_rejected() {
    let dir = tempdir().unwrap();
    let archive = dir.path().join("traversal.zip");
    let file = fs::File::create(&archive).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::<()>::default();
    zip.start_file("../escape", options).unwrap();
    zip.write_all(b"escape").unwrap();
    zip.finish().unwrap();
    assert_eq!(
        inspect_rpcs3_ordinary_mod_zip(&archive).unwrap_err().kind,
        Rpcs3OrdinaryModErrorKind::ArchiveRejected
    );

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("missing"), dir.path().join("link")).unwrap();
        assert_eq!(
            inspect_rpcs3_ordinary_mod(&dir.path().join("link"))
                .unwrap_err()
                .kind,
            Rpcs3OrdinaryModErrorKind::SourceSymlink
        );

        let hardlink_root = dir.path().join("hardlink");
        package(&hardlink_root);
        fs::hard_link(
            hardlink_root.join("PS3_GAME/USRDIR/mod/nested/file.bin"),
            hardlink_root.join("PS3_GAME/USRDIR/mod/nested/second.bin"),
        )
        .unwrap();
        assert_eq!(
            inspect_rpcs3_ordinary_mod(&hardlink_root).unwrap_err().kind,
            Rpcs3OrdinaryModErrorKind::UnsafeMember
        );

        let fifo_root = dir.path().join("fifo");
        package(&fifo_root);
        let fifo = fifo_root.join("PS3_GAME/USRDIR/mod/nested/fifo");
        let fifo_bytes = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // A FIFO is created solely as a hostile test fixture; inspection must
        // reject it without opening or reading from it.
        assert_eq!(unsafe { libc::mkfifo(fifo_bytes.as_ptr(), 0o600) }, 0);
        assert_eq!(
            inspect_rpcs3_ordinary_mod(&fifo_root).unwrap_err().kind,
            Rpcs3OrdinaryModErrorKind::UnsafeMember
        );
    }
}

#[test]
fn identical_files_skip_and_apply_rollback_restores_replaced_files() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("mod");
    package(&root);
    let selected = dir.path().join("game.iso");
    let identity = report(TITLE, &selected);
    let destination = dir.path().join("game-root");
    let target = destination.join("USRDIR/mod/nested/file.bin");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"old content").unwrap();
    let inspection = inspect_rpcs3_ordinary_mod(&root).unwrap();
    let plan = build_rpcs3_ordinary_mod_plan(&inspection, &identity, &destination).unwrap();
    assert_eq!(plan.report.summary.replace_different, 1);
    let transaction = build_rpcs3_ordinary_mod_transaction_plan(&plan, "rpcs3:test").unwrap();
    let history = dir.path().join("history");
    let backups = dir.path().join("backups");
    let applied = apply_rpcs3_ordinary_mod(
        &transaction,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: transaction.plan_id.clone(),
                general_approved: true,
                replacement_approved: true,
            }),
            operation_id: "rpcs3-mod-apply".into(),
            timestamp_unix_seconds: 1,
            current_context: transaction.context.clone(),
            history_root: history.clone(),
            backup_root: backups.clone(),
        },
    );
    assert_eq!(applied.apply.journal.status, SharedApplyStatus::Success);
    assert_eq!(fs::read(&target).unwrap(), b"new mod");
    let unrelated = destination.join("USRDIR/unrelated.bin");
    fs::write(&unrelated, b"keep").unwrap();
    let journal = applied.apply.journal_path.unwrap();
    let preview = preview_rpcs3_ordinary_mod_rollback(&journal, &destination, &backups);
    assert!(preview.available);
    let rollback = rollback_rpcs3_ordinary_mod(
        &preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: "rpcs3-mod-rollback".into(),
            timestamp_unix_seconds: 2,
            history_root: history,
            backup_root: backups,
        },
    );
    assert_eq!(rollback.status, SharedApplyStatus::Success);
    assert_eq!(fs::read(&target).unwrap(), b"old content");
    assert_eq!(fs::read(unrelated).unwrap(), b"keep");
}

#[test]
fn stale_source_is_rejected_before_write() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("mod");
    package(&root);
    let selected = dir.path().join("game.iso");
    let identity = report(TITLE, &selected);
    let destination = dir.path().join("game-root");
    let inspection = inspect_rpcs3_ordinary_mod(&root).unwrap();
    let plan = build_rpcs3_ordinary_mod_plan(&inspection, &identity, &destination).unwrap();
    let transaction = build_rpcs3_ordinary_mod_transaction_plan(&plan, "rpcs3:test").unwrap();
    fs::write(
        root.join("PS3_GAME/USRDIR/mod/nested/file.bin"),
        b"changed after review",
    )
    .unwrap();
    let result = apply_rpcs3_ordinary_mod(
        &transaction,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: transaction.plan_id.clone(),
                general_approved: true,
                replacement_approved: true,
            }),
            operation_id: "rpcs3-stale-source".into(),
            timestamp_unix_seconds: 1,
            current_context: transaction.context.clone(),
            history_root: dir.path().join("history"),
            backup_root: dir.path().join("backups"),
        },
    );
    assert_ne!(result.apply.journal.status, SharedApplyStatus::Success);
    assert!(!destination.join("USRDIR/mod/nested/file.bin").exists());
}

#[test]
fn identical_destination_is_reported_without_rewriting() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("mod");
    package(&root);
    let target = dir.path().join("game-root/USRDIR/mod/nested/file.bin");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"new mod").unwrap();
    let selected = dir.path().join("game.iso");
    let identity = report(TITLE, &selected);
    let inspection = inspect_rpcs3_ordinary_mod(&root).unwrap();
    let plan = build_rpcs3_ordinary_mod_plan(&inspection, &identity, &dir.path().join("game-root"))
        .unwrap();
    assert_eq!(plan.report.summary.already_installed, 1);
    assert_eq!(fs::read(&target).unwrap(), b"new mod");
}
