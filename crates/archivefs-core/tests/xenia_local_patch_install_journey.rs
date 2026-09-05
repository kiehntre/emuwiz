use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use archivefs_core::patch_manager::{
    LocalXeniaFileError, LocalXeniaInstallState, SharedApplyConfirmation, SharedApplyOptions,
    SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions,
    XeniaInstallPreviewRequest, build_shared_transaction_plan, build_xenia_install_preview,
    check_local_xenia_install_state, discover_local_xenia_patch_file, execute_shared_apply,
    execute_shared_rollback, load_local_xenia_destination, preview_shared_rollback,
    stage_local_xenia_patch_file,
};

const PATCH: &str = r#"title_name = "Test Game"
title_id = "415607D2"
hash = "4768B579A3C5F134"

[[patch]]
name = "Infinite health"
desc = "test"
author = "local"
is_enabled = false
[[patch.be32]]
address = 0x82000000
value = 0x1
"#;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "archivefs-xenia-local-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.path(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn prepared(
    f: &Fixture,
) -> (
    archivefs_core::patch_manager::LocalXeniaDiscovery,
    archivefs_core::patch_manager::LoadedXeniaDestination,
    PathBuf,
) {
    let source = f.write("incoming/Test.patch.toml", PATCH);
    let config = f.path("xenia");
    fs::create_dir_all(&config).unwrap();
    let discovery = discover_local_xenia_patch_file(&source, Some("415607D2")).unwrap();
    let destination = load_local_xenia_destination(&config, "Test.patch.toml").unwrap();
    (discovery, destination, config)
}

#[test]
fn valid_local_patch_previews_applies_and_undoes_atomically() {
    let f = Fixture::new();
    let (discovery, destination, config) = prepared(&f);
    let staging = f.path("generated");
    let staged = stage_local_xenia_patch_file(
        &staging,
        "Test.patch.toml",
        &discovery,
        destination.document.as_ref(),
    )
    .unwrap();
    let preview = build_xenia_install_preview(&XeniaInstallPreviewRequest {
        selected_archive: discovery.source_path.clone(),
        configuration_path: config.clone(),
        title_id: discovery.candidate.title_id.clone(),
        compatibility: discovery.candidate.compatibility,
        staged,
    })
    .unwrap();
    assert_eq!(preview.report.entries.len(), 1);
    assert!(
        !config.join("patches/Test.patch.toml").exists(),
        "preview must not mutate"
    );
    let plan =
        build_shared_transaction_plan(&preview.report, "profile", "xenia-local-file", &staging)
            .unwrap();
    let result = execute_shared_apply(
        &plan,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: true,
                replacement_approved: true,
            }),
            operation_id: "xenia-local-test".into(),
            timestamp_unix_seconds: 1,
            current_context: plan.context.clone(),
            history_root: f.path("history"),
            backup_root: f.path("backup"),
        },
    );
    assert_eq!(result.journal.status, SharedApplyStatus::Success);
    assert!(result.journal_path.is_some());
    assert_eq!(
        fs::read_to_string(config.join("patches/Test.patch.toml"))
            .unwrap()
            .matches("Infinite health")
            .count(),
        1
    );
    let after = load_local_xenia_destination(&config, "Test.patch.toml").unwrap();
    assert_eq!(
        check_local_xenia_install_state(&after, &discovery),
        LocalXeniaInstallState::AlreadyInstalled
    );
    let journal = result.journal_path.unwrap();
    let rollback_preview = preview_shared_rollback(&journal, &config, &f.path("backup"));
    let rollback = execute_shared_rollback(
        &rollback_preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: rollback_preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: "xenia-local-undo".into(),
            timestamp_unix_seconds: 2,
            history_root: f.path("history"),
            backup_root: f.path("backup"),
        },
    );
    assert_eq!(rollback.status, SharedApplyStatus::Success);
    assert!(!config.join("patches/Test.patch.toml").exists());
}

#[test]
fn local_xenia_safety_and_identity_blocks_are_fail_closed() {
    let f = Fixture::new();
    let source = f.write("Test.patch.toml", PATCH);
    assert!(matches!(
        discover_local_xenia_patch_file(&source, None),
        Err(LocalXeniaFileError::IdentityUnavailable { .. })
    ));
    assert!(matches!(
        discover_local_xenia_patch_file(&source, Some("DEADBEEF")),
        Err(LocalXeniaFileError::IdentityConflict { .. })
    ));
    assert!(matches!(
        discover_local_xenia_patch_file(&f.write("bad.txt", PATCH), Some("415607D2")),
        Err(LocalXeniaFileError::UnsupportedExtension { .. })
    ));
    assert!(matches!(
        discover_local_xenia_patch_file(
            &f.write("bad.patch.toml", "not = [valid"),
            Some("415607D2")
        ),
        Err(LocalXeniaFileError::Malformed { .. })
    ));
    assert!(matches!(
        discover_local_xenia_patch_file(
            &f.write(
                "empty.patch.toml",
                "title_id=\"415607D2\"\ntitle_name=\"x\""
            ),
            Some("415607D2")
        ),
        Err(LocalXeniaFileError::NoPatchesFound { .. })
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&source, f.path("link.patch.toml")).unwrap();
        assert!(matches!(
            discover_local_xenia_patch_file(&f.path("link.patch.toml"), Some("415607D2")),
            Err(LocalXeniaFileError::IsSymlink { .. })
        ));
    }
}

#[test]
fn unrelated_existing_patch_is_preserved_and_oversized_source_is_rejected() {
    let f = Fixture::new();
    let (discovery, _, config) = prepared(&f);
    let unrelated = "title_name=\"Other\"\ntitle_id=\"415607D2\"\n";
    f.write("xenia/patches/Other.patch.toml", unrelated);
    let huge = f.write("huge.patch.toml", &"x".repeat(256 * 1024 + 1));
    assert!(matches!(
        discover_local_xenia_patch_file(&huge, Some("415607D2")),
        Err(LocalXeniaFileError::TooLarge { .. })
    ));
    let destination = load_local_xenia_destination(&config, "Test.patch.toml").unwrap();
    let staged = stage_local_xenia_patch_file(
        &f.path("generated"),
        "Test.patch.toml",
        &discovery,
        destination.document.as_ref(),
    )
    .unwrap();
    assert!(staged.contents.contains("Infinite health"));
    assert_eq!(
        fs::read_to_string(f.path("xenia/patches/Other.patch.toml")).unwrap(),
        unrelated
    );
}
