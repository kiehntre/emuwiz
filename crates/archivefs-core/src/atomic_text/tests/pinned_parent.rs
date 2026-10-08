//! Barriers after the final observations prove fd-relative syscall boundaries.

use super::{Event, FORCE_NAME, fixture, install_hook, snapshot, stages, write};
use std::cell::RefCell;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn directories(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let parent = root.join("parent");
    let parked = root.join("original-directory");
    fs::create_dir(&parent).unwrap();
    let target = parent.join("destination");
    fs::write(&target, "original destination").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    (parent, parked, target)
}

#[test]
fn parent_swap_after_last_check_publishes_only_original_and_reports_partial_outcome() {
    let root = fixture();
    let (parent, parked, target) = directories(root.path());
    let expected_stage = Rc::new(RefCell::new(None));
    let captured = expected_stage.clone();
    let (p, a) = (parent.clone(), parked.clone());
    let _reset = install_hook(move |event, stage| {
        if event == Event::BeforeRename {
            *captured.borrow_mut() = Some(snapshot(stage));
            fs::rename(&p, &a)?;
            fs::create_dir(&p)?;
            fs::write(p.join("destination"), "replacement destination")?;
            fs::write(p.join(stage.file_name().unwrap()), "other operation stage")?;
        }
        Ok(())
    });
    let error = write(&target, "new text").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("publication completed in the pinned original directory")
    );
    assert_eq!(
        snapshot(&parked.join("destination")),
        expected_stage.borrow_mut().take().unwrap()
    );
    assert_eq!(fs::read(&target).unwrap(), b"replacement destination");
    assert!(stages(&parked).is_empty());
    assert_eq!(
        fs::read(&stages(&parent)[0]).unwrap(),
        b"other operation stage"
    );
}

#[test]
fn symlink_rebinding_after_last_check_cannot_redirect_rename() {
    let root = fixture();
    let (original, _, _) = directories(root.path());
    let replacement = root.path().join("replacement");
    fs::create_dir(&replacement).unwrap();
    fs::write(replacement.join("destination"), "replacement destination").unwrap();
    let before = snapshot(&replacement.join("destination"));
    let link = root.path().join("link");
    symlink(&original, &link).unwrap();
    let (l, b) = (link.clone(), replacement.clone());
    let _reset = install_hook(move |event, _| {
        if event == Event::BeforeRename {
            fs::remove_file(&l)?;
            symlink(&b, &l)?;
        }
        Ok(())
    });
    let error = write(&link.join("destination"), "new text").unwrap_err();
    assert!(error.to_string().contains("publication completed"));
    assert_eq!(fs::read(original.join("destination")).unwrap(), b"new text");
    assert_eq!(snapshot(&replacement.join("destination")), before);
    assert!(stages(&original).is_empty());
}

#[test]
fn cleanup_parent_swap_after_identity_check_never_unlinks_replacement_entry() {
    let root = fixture();
    let (parent, parked, target) = directories(root.path());
    let old_destination = snapshot(&target);
    let foreign = Rc::new(RefCell::new(None));
    let captured = foreign.clone();
    let (p, a) = (parent.clone(), parked.clone());
    let _reset = install_hook(move |event, stage| {
        if event == Event::BeforeSync {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "synthetic interrupted sync",
            ));
        }
        if event == Event::BeforeUnlink {
            fs::rename(&p, &a)?;
            fs::create_dir(&p)?;
            fs::write(p.join("destination"), "replacement destination")?;
            fs::write(p.join(stage.file_name().unwrap()), "other operation stage")?;
            *captured.borrow_mut() = Some(snapshot(stage));
        }
        Ok(())
    });
    let error = write(&target, "new text").unwrap_err();
    assert!(error.to_string().contains("synthetic interrupted sync"));
    assert_eq!(snapshot(&parked.join("destination")), old_destination);
    assert!(stages(&parked).is_empty());
    assert_eq!(
        snapshot(&stages(&parent)[0]),
        foreign.borrow_mut().take().unwrap()
    );
    assert_eq!(fs::read(&target).unwrap(), b"replacement destination");
}

#[test]
fn stage_creation_is_anchored_after_parent_probe() {
    let root = fixture();
    let (parent, parked, target) = directories(root.path());
    let old_destination = snapshot(&target);
    let (p, a) = (parent.clone(), parked.clone());
    let _reset = install_hook(move |event, _| {
        if event == Event::BeforeStageCreate {
            fs::rename(&p, &a)?;
            fs::create_dir(&p)?;
            fs::write(p.join("destination"), "replacement destination")?;
            fs::write(
                p.join(".archivefs-config-write-forced.tmp"),
                "other operation stage",
            )?;
        }
        Ok(())
    });
    FORCE_NAME.set(true);
    let error = write(&target, "new text").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("parent directory binding changed")
    );
    assert_eq!(snapshot(&parked.join("destination")), old_destination);
    assert!(stages(&parked).is_empty());
    assert_eq!(
        fs::read(parent.join(".archivefs-config-write-forced.tmp")).unwrap(),
        b"other operation stage"
    );
    assert_eq!(fs::read(&target).unwrap(), b"replacement destination");
}

#[test]
fn unavailable_parent_refuses_and_cleans_pinned_stage() {
    let root = fixture();
    let (parent, parked, target) = directories(root.path());
    let before = snapshot(&target);
    let (p, a) = (parent.clone(), parked.clone());
    let _reset = install_hook(move |event, _| {
        if event == Event::BeforePublish {
            fs::rename(&p, &a)?;
        }
        Ok(())
    });
    let error = write(&target, "new text").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("parent directory binding unavailable")
    );
    assert_eq!(snapshot(&parked.join("destination")), before);
    assert!(stages(&parked).is_empty());
    assert!(!parent.exists());
}

#[test]
fn post_publication_binding_error_does_not_cleanup_a_new_stage_entry() {
    let root = fixture();
    let (parent, parked, target) = directories(root.path());
    let foreign = Rc::new(RefCell::new(None));
    let captured = foreign.clone();
    let (p, a) = (parent.clone(), parked.clone());
    let _reset = install_hook(move |event, stage| {
        if event == Event::AfterPublish {
            fs::write(stage, "next operation stage")?;
            *captured.borrow_mut() = Some(snapshot(stage));
            fs::rename(&p, &a)?;
            fs::create_dir(&p)?;
            fs::write(p.join("destination"), "replacement destination")?;
        }
        Ok(())
    });
    let error = write(&target, "new text").unwrap_err();
    assert!(error.to_string().contains("publication completed"));
    assert_eq!(fs::read(parked.join("destination")).unwrap(), b"new text");
    assert_eq!(
        snapshot(&stages(&parked)[0]),
        foreign.borrow_mut().take().unwrap()
    );
    assert_eq!(fs::read(&target).unwrap(), b"replacement destination");
}

#[test]
fn symlink_parent_and_alias_rebinding_to_same_object_remain_supported() {
    let root = fixture();
    let (parent, _, _) = directories(root.path());
    let alias = root.path().join("alias");
    let link = root.path().join("link");
    symlink(&parent, &alias).unwrap();
    symlink(&parent, &link).unwrap();
    let (l, a) = (link.clone(), alias.clone());
    let _reset = install_hook(move |event, _| {
        if event == Event::BeforePublish {
            fs::remove_file(&l)?;
            symlink(&a, &l)?;
        }
        Ok(())
    });
    write(&link.join("destination"), "new text").unwrap();
    assert_eq!(fs::read(parent.join("destination")).unwrap(), b"new text");
    assert!(stages(&parent).is_empty());
    assert_eq!(
        fs::metadata(parent.join("destination")).unwrap().mode() & 0o7777,
        0o600
    );
}

#[cfg(target_os = "linux")]
#[test]
fn write_search_only_directory_does_not_require_read_permission() {
    let root = fixture();
    let (parent, _, target) = directories(root.path());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o300)).unwrap();
    let result = write(&target, "new text");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    result.unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"new text");
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o600);
    assert!(stages(&parent).is_empty());
}

#[test]
fn non_utf8_destination_basename_is_preserved() {
    use std::os::unix::ffi::OsStrExt;
    let root = fixture();
    let target = root.path().join(std::ffi::OsStr::from_bytes(b"text-\xff"));
    write(&target, "new text").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"new text");
    assert!(stages(root.path()).is_empty());
}

#[test]
fn basename_validation_does_not_normalize_trailing_slash_or_dot_into_a_write() {
    let root = fixture();
    let target = root.path().join("destination");
    fs::write(&target, "original destination").unwrap();
    let before = snapshot(&target);
    for suffix in ["destination/", "destination/.", "destination/.."] {
        assert!(write(&root.path().join(suffix), "new text").is_err());
        assert_eq!(snapshot(&target), before);
    }
    assert!(stages(root.path()).is_empty());
}

#[test]
fn rename_executor_journal_refusal_is_outer_error_before_any_source_mutation() {
    use crate::dat::rename_apply::*;
    use crate::safe_read::TrustedRoots;
    use std::sync::atomic::AtomicBool;
    let root = fixture();
    let sources = root.path().join("sources");
    fs::create_dir(&sources).unwrap();
    let source = sources.join("synthetic.txt");
    let destination = sources.join("renamed.txt");
    fs::write(&source, "synthetic source bytes").unwrap();
    let before = snapshot(&source);
    let parent = root.path().join("journals");
    fs::create_dir(&parent).unwrap();
    let parked = root.path().join("original-journals");
    let mut transaction = RenameTransaction {
        transaction_id: "synthetic".into(),
        plan_generation: 7,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.into()),
        created_at_unix: 42,
        source_scan_root: sources.to_string_lossy().into_owned(),
        state: TransactionState::Planned,
        entries: vec![TransactionEntry {
            source_path: source.clone(),
            destination_path: destination.clone(),
            original_basename: "synthetic.txt".into(),
            proposed_basename: "renamed.txt".into(),
            identity: crate::dat::rename_apply::identity::capture_identity(&source).unwrap(),
            operation: TransactionOperation::RenameMove,
            preflight_passed: false,
            preflight_failures: vec![],
            state: EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: vec![],
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let (p, a) = (parent.clone(), parked.clone());
    let _reset = install_hook(move |event, _| {
        if event == Event::BeforePublish {
            fs::rename(&p, &a)?;
            fs::create_dir(&p)?;
        }
        Ok(())
    });
    let cancel = AtomicBool::new(false);
    let result = apply_transaction(&mut ApplyExecution {
        transaction: &mut transaction,
        approved_paths: [source.to_string_lossy().into_owned()]
            .into_iter()
            .collect(),
        current_generation: 7,
        trusted: TrustedRoots::from_paths([&sources]),
        journal_dir: parent,
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel: &cancel,
        directory_policy: preflight::DirectoryPolicy::SameDirectory,
        allow_symlink_source: false,
    });
    assert!(
        matches!(result, Err(ApplyError::Journal(ref detail)) if detail.contains("parent directory binding changed")),
        "{result:?}"
    );
    assert_eq!(snapshot(&source), before);
    assert!(!destination.exists());
    assert_eq!(transaction.state, TransactionState::Planned);
    assert_eq!(transaction.entries[0].state, EntryState::Planned);
    assert!(stages(&parked).is_empty());
}
