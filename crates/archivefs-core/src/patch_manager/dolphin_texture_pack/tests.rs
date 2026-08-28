use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::*;
use crate::game_identity::IdentityPlatform;
use crate::patch_manager::shared_transaction::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyStatus, SharedRollbackConfirmation,
    SharedRollbackOptions, execute_shared_apply, execute_shared_rollback,
    generate_shared_operation_id, preview_shared_rollback,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "archivefs-dolphin-texture-pack-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn identity(game_id: &str) -> DolphinTextureModIdentity {
    DolphinTextureModIdentity {
        game_id: game_id.to_string(),
        platform: IdentityPlatform::GameCube,
    }
}

fn file(root: &Path, name: &str, destination: &str, bytes: &[u8]) -> DolphinTexturePackFile {
    let source_path = root.join(name);
    std::fs::write(&source_path, bytes).unwrap();
    DolphinTexturePackFile {
        source_path,
        destination_filename: destination.to_string(),
        size_bytes: bytes.len() as u64,
        sha256: sha256(bytes),
    }
}

fn manifest(root: &Path, files: Vec<DolphinTexturePackFile>) -> DolphinTexturePackManifest {
    DolphinTexturePackManifest {
        format: DOLPHIN_TEXTURE_PACK_MANIFEST_FORMAT.to_string(),
        name: "Example HD Pack".to_string(),
        version: Some("1.0".to_string()),
        target_game_id: "GALE01".to_string(),
        source_root: root.to_path_buf(),
        files,
    }
}

fn request(
    root: &Path,
    destination_root: &Path,
    manifest: DolphinTexturePackManifest,
) -> DolphinTexturePackPreviewRequest {
    DolphinTexturePackPreviewRequest {
        selected_archive: PathBuf::from("/library/game.iso"),
        identity: identity("GALE01"),
        destination_root: destination_root.to_path_buf(),
        source_root: root.to_path_buf(),
        manifest,
    }
}

fn apply_options(
    plan: &crate::patch_manager::SharedTransactionPlan,
    history_root: PathBuf,
    backup_root: PathBuf,
    replacement_approved: bool,
) -> SharedApplyOptions {
    SharedApplyOptions {
        dry_run: false,
        confirmation: Some(SharedApplyConfirmation {
            plan_id: plan.plan_id.clone(),
            general_approved: true,
            replacement_approved,
        }),
        operation_id: generate_shared_operation_id(),
        timestamp_unix_seconds: 1_700_000_000,
        current_context: plan.context.clone(),
        history_root,
        backup_root,
    }
}

#[test]
fn valid_multi_file_manifest_builds_deterministic_applyable_preview() {
    let dir = TestDir::new("valid");
    let source = dir.0.join("expanded pack");
    let destination = dir.0.join("Load/Textures");
    std::fs::create_dir_all(&source).unwrap();
    let request = request(
        &source,
        &destination,
        manifest(
            &source,
            vec![
                file(&source, "z.png", "z.png", b"z"),
                file(&source, "a.png", "a.png", b"a"),
            ],
        ),
    );
    let first = build_dolphin_texture_pack_preview(&request).unwrap();
    let second = build_dolphin_texture_pack_preview(&request).unwrap();
    assert!(first.is_applyable(), "{:#?}", first.report);
    assert_eq!(first.report, second.report);
    assert_eq!(first.install_count(), 2);
    assert_eq!(
        first.report.entries[0].destination_relative_path,
        Some(PathBuf::from("GALE01/a.png"))
    );
}

#[test]
fn versioned_manifest_json_round_trips_without_filename_authority() {
    let dir = TestDir::new("json");
    let source = dir.0.join("source");
    std::fs::create_dir(&source).unwrap();
    let original = manifest(&source, vec![file(&source, "a.png", "a.png", b"a")]);
    let json = serde_json::to_string(&original).unwrap();
    let restored: DolphinTexturePackManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, original);
    assert_eq!(restored.format, DOLPHIN_TEXTURE_PACK_MANIFEST_FORMAT);
}

#[test]
fn target_game_id_must_match_verified_identity() {
    let dir = TestDir::new("wrong-game");
    let source = dir.0.join("source");
    std::fs::create_dir(&source).unwrap();
    let mut pack = manifest(&source, vec![file(&source, "a.png", "a.png", b"a")]);
    pack.target_game_id = "GM8E01".to_string();
    let error =
        validate_dolphin_texture_pack_manifest(&pack, &identity("GALE01"), &source).unwrap_err();
    assert_eq!(error.kind, DolphinTextureModErrorKind::IdentityUnverified);
}

#[test]
fn unsafe_duplicate_and_outside_sources_are_refused() {
    let dir = TestDir::new("unsafe");
    let source = dir.0.join("source");
    std::fs::create_dir(&source).unwrap();
    let mut pack = manifest(
        &source,
        vec![
            file(&source, "a.png", "../escape.png", b"a"),
            file(&source, "b.png", "b.png", b"b"),
        ],
    );
    assert_eq!(
        validate_dolphin_texture_pack_manifest(&pack, &identity("GALE01"), &source)
            .unwrap_err()
            .kind,
        DolphinTextureModErrorKind::SourceUnsafeFilename
    );
    pack.files[0].destination_filename = "b.png".to_string();
    assert_eq!(
        validate_dolphin_texture_pack_manifest(&pack, &identity("GALE01"), &source)
            .unwrap_err()
            .kind,
        DolphinTextureModErrorKind::PreviewFailed
    );
    let outside = dir.0.join("outside.png");
    std::fs::write(&outside, b"outside").unwrap();
    pack.files[0].destination_filename = "a.png".to_string();
    pack.files[0].source_path = outside;
    assert_eq!(
        validate_dolphin_texture_pack_manifest(&pack, &identity("GALE01"), &source)
            .unwrap_err()
            .kind,
        DolphinTextureModErrorKind::SourceOutsideApprovedScope
    );
}

#[cfg(unix)]
#[test]
fn symlink_source_is_refused() {
    use std::os::unix::fs::symlink;
    let dir = TestDir::new("symlink");
    let source = dir.0.join("source");
    std::fs::create_dir(&source).unwrap();
    let real = dir.0.join("real.png");
    std::fs::write(&real, b"png").unwrap();
    symlink(&real, source.join("link.png")).unwrap();
    let pack = manifest(
        &source,
        vec![file(&source, "unused.png", "unused.png", b"x")],
    );
    let mut pack = pack;
    pack.files[0].source_path = source.join("link.png");
    assert_eq!(
        validate_dolphin_texture_pack_manifest(&pack, &identity("GALE01"), &source)
            .unwrap_err()
            .kind,
        DolphinTextureModErrorKind::SourceSymlink
    );
}

#[test]
fn existing_identical_is_skipped_and_different_content_is_replacement() {
    let dir = TestDir::new("existing");
    let source = dir.0.join("source");
    let destination = dir.0.join("textures");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(destination.join("GALE01")).unwrap();
    std::fs::write(destination.join("GALE01/same.png"), b"same").unwrap();
    std::fs::write(destination.join("GALE01/replace.png"), b"old").unwrap();
    let pack = manifest(
        &source,
        vec![
            file(&source, "same.png", "same.png", b"same"),
            file(&source, "replace.png", "replace.png", b"new"),
        ],
    );
    let preview =
        build_dolphin_texture_pack_preview(&request(&source, &destination, pack)).unwrap();
    assert_eq!(preview.already_installed_count(), 1);
    assert_eq!(preview.replacement_count(), 1);
    assert!(preview.is_applyable());
}

#[test]
fn complete_pack_applies_through_shared_transaction_and_rolls_back_byte_exact_replacement() {
    let dir = TestDir::new("apply-rollback");
    let source = dir.0.join("source with spaces");
    let destination = dir.0.join("textures/ユニコード");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(destination.join("GALE01")).unwrap();
    std::fs::write(destination.join("GALE01/old.png"), b"before").unwrap();
    let pack = manifest(
        &source,
        vec![
            file(&source, "new.png", "new.png", b"new"),
            file(&source, "old.png", "old.png", b"after"),
        ],
    );
    let preview =
        build_dolphin_texture_pack_preview(&request(&source, &destination, pack)).unwrap();
    let transaction =
        build_dolphin_texture_pack_transaction_plan(&preview, "dolphin", &source).unwrap();
    let history = dir.0.join("history");
    let backups = dir.0.join("backups");
    let applied = execute_shared_apply(
        &transaction,
        &apply_options(&transaction, history.clone(), backups.clone(), true),
    );
    assert_eq!(applied.journal.status, SharedApplyStatus::Success);
    assert_eq!(
        std::fs::read(destination.join("GALE01/old.png")).unwrap(),
        b"after"
    );
    assert_eq!(
        std::fs::read(destination.join("GALE01/new.png")).unwrap(),
        b"new"
    );

    let journal = applied.journal_path.unwrap();
    let rollback = preview_shared_rollback(&journal, &destination, &backups);
    let result = execute_shared_rollback(
        &rollback,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: rollback.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 1_700_000_001,
            history_root: history,
            backup_root: backups,
        },
    );
    assert_eq!(result.status, SharedApplyStatus::Success);
    assert!(!destination.join("GALE01/new.png").exists());
    assert_eq!(
        std::fs::read(destination.join("GALE01/old.png")).unwrap(),
        b"before"
    );
}

#[test]
fn mid_pack_source_change_fails_without_leaving_earlier_file_installed() {
    let dir = TestDir::new("mid-failure");
    let source = dir.0.join("source");
    let destination = dir.0.join("textures");
    std::fs::create_dir_all(&source).unwrap();
    let first = file(&source, "a.png", "a.png", b"a");
    let second = file(&source, "b.png", "b.png", b"b");
    let preview = build_dolphin_texture_pack_preview(&request(
        &source,
        &destination,
        manifest(&source, vec![first, second.clone()]),
    ))
    .unwrap();
    let transaction =
        build_dolphin_texture_pack_transaction_plan(&preview, "dolphin", &source).unwrap();
    std::fs::write(&second.source_path, b"changed after preview").unwrap();
    let result = execute_dolphin_texture_pack_apply(
        &transaction,
        &apply_options(
            &transaction,
            dir.0.join("history"),
            dir.0.join("backups"),
            false,
        ),
    );
    assert_eq!(
        result.apply.journal.status,
        SharedApplyStatus::PartialFailure
    );
    assert_eq!(
        result.rollback.as_ref().map(|rollback| rollback.status),
        Some(SharedApplyStatus::Success)
    );
    assert!(!destination.join("GALE01/a.png").exists());
}

#[test]
fn unknown_identity_cannot_become_a_pack_target() {
    let dir = TestDir::new("unknown");
    let source = dir.0.join("source");
    std::fs::create_dir(&source).unwrap();
    let pack = manifest(&source, vec![file(&source, "a.png", "a.png", b"a")]);
    let mut unknown = identity("GALE01");
    unknown.game_id.clear();
    assert_eq!(
        validate_dolphin_texture_pack_manifest(&pack, &unknown, &source)
            .unwrap_err()
            .kind,
        DolphinTextureModErrorKind::IdentityUnverified
    );
}
