use super::*;
use archivefs_core::game_identity::IdentityPlatform;
use archivefs_core::patch_manager::*;

#[test]
fn refused_texture_undo_preserves_receipt_and_reports_failure() {
    texture_undo(true);
}

#[test]
fn successful_texture_undo_clears_receipt_and_error() {
    texture_undo(false);
}

fn texture_undo(interfere: bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("source");
    let destination = root.join("PSP/TEXTURES");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(source.join("new.png"), b"texture").unwrap();
    let identity = PpssppTextureIdentity {
        disc_id: "ULUS-12345".into(),
        platform: IdentityPlatform::PlayStation,
    };
    let build = build_ppsspp_texture_pack_manifest(&PpssppTexturePackBuildRequest {
        source_root: source.clone(),
        identity: identity.clone(),
        name: "Fixture".into(),
    })
    .unwrap();
    let plan = build_ppsspp_texture_pack_preview(&PpssppTexturePackPreviewRequest {
        selected_archive: root.join("game.iso"),
        identity,
        destination_root: destination.clone(),
        source_root: source.clone(),
        manifest: build.manifest,
    })
    .unwrap();
    let transaction = build_ppsspp_texture_pack_transaction_plan(&plan, "ppsspp", &source).unwrap();
    let history = root.join("history");
    let backups = root.join("backups");
    let applied = execute_ppsspp_texture_pack_apply(
        &transaction,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: transaction.plan_id.clone(),
                general_approved: true,
                replacement_approved: true,
            }),
            operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 1_700_000_000,
            current_context: transaction.context.clone(),
            history_root: history.clone(),
            backup_root: backups.clone(),
        },
    )
    .apply;
    assert_eq!(applied.journal.status, SharedApplyStatus::Success);
    let preview = preview_shared_rollback(
        applied.journal_path.as_ref().unwrap(),
        &destination,
        &backups,
    );
    assert!(preview.available);
    // A change after preview must not erase the installed receipt in the GUI.
    let output = destination.join("ULUS-12345/new.png");
    if interfere {
        std::fs::write(&output, b"foreign texture").unwrap();
    }
    let result = execute_shared_rollback(
        &preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 1_700_000_001,
            history_root: history,
            backup_root: backups,
        },
    );
    assert_eq!(
        result.status,
        if interfere {
            SharedApplyStatus::Failed
        } else {
            SharedApplyStatus::Success
        }
    );
    let mut state = PpssppTextureModPageState {
        applied: Some(applied),
        error: Some("previous operation error".into()),
        ..Default::default()
    };
    state.finish_rollback(result);
    if interfere {
        assert!(
            state.applied.is_some(),
            "failed Undo must retain the installed receipt"
        );
        assert!(
            state
                .error
                .as_deref()
                .unwrap()
                .contains("Undo did not complete")
        );
        assert_eq!(std::fs::read(output).unwrap(), b"foreign texture");
    } else {
        assert!(state.applied.is_none());
        assert!(state.error.is_none());
        assert!(!output.exists());
    }
    assert_eq!(std::fs::read(source.join("new.png")).unwrap(), b"texture");
}

#[test]
fn profile_discovery_is_reused_within_the_ttl_and_refreshed_after_it() {
    let mut state = PpssppTextureModPageState::default();
    let start = Instant::now();
    let probes = std::cell::Cell::new(0);
    let probe = |state: &mut PpssppTextureModPageState, at: Instant| {
        state.eligible_profile(at, || {
            probes.set(probes.get() + 1);
            None
        })
    };
    assert!(probe(&mut state, start).is_none());
    assert!(probe(&mut state, start + Duration::from_secs(1)).is_none());
    assert_eq!(probes.get(), 1, "a frame inside the TTL must not re-probe");
    probe(&mut state, start + PROFILE_DISCOVERY_TTL);
    assert_eq!(probes.get(), 2, "the TTL expiring forces a fresh probe");
}
