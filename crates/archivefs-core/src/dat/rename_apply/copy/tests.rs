use super::super::model::{EntryState, TransactionOperation};
use super::*;
fn fixture() -> (tempfile::TempDir, TransactionEntry) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.bin");
    fs::write(&source, b"synthetic exact bytes").unwrap();
    let entry = TransactionEntry {
        source_path: source.clone(),
        destination_path: root.path().join("target.bin"),
        original_basename: "source.bin".into(),
        proposed_basename: "target.bin".into(),
        identity: capture_identity(&source).unwrap(),
        operation: TransactionOperation::CreateCopy {
            expected_source: source,
            destination_root: root.path().to_path_buf(),
        },
        preflight_passed: false,
        preflight_failures: vec![],
        state: EntryState::Planned,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    };
    (root, entry)
}
fn hook(at: Edge, f: impl FnOnce(&Path) -> io::Result<()> + 'static) {
    HOOK.with(|h| *h.borrow_mut() = Some((at, Box::new(f))));
}
#[test]
fn copy_temporary_symlink_cannot_clobber_unrelated_file() {
    let (root, mut entry) = fixture();
    let sentinel = root.path().join("unrelated");
    fs::write(&sentinel, b"must survive").unwrap();
    let target = sentinel.clone();
    hook(Edge::BeforeCreate, move |path| {
        std::os::unix::fs::symlink(target, path)
    });
    assert!(prepare(&mut entry).is_err());
    assert_eq!(fs::read(sentinel).unwrap(), b"must survive");
    assert!(!entry.destination_path.exists());
}
#[test]
fn held_copy_cannot_adopt_identical_foreign_staging_inode() {
    let (_root, mut entry) = fixture();
    hook(Edge::BeforeCapture, |path| {
        fs::rename(path, path.with_extension("retained"))?;
        fs::write(path, b"synthetic exact bytes")
    });
    assert!(prepare(&mut entry).is_err());
    assert!(!entry.destination_path.exists());
}
#[test]
fn foreign_stage_before_publication_is_retained_and_refused() {
    let (root, mut entry) = fixture();
    let prepared = prepare(&mut entry).unwrap().unwrap();
    hook(Edge::BeforePublish, |path| {
        fs::rename(path, path.with_extension("retained"))?;
        fs::write(path, b"synthetic exact bytes")
    });
    assert!(prepared.publish().is_err());
    assert!(!entry.destination_path.exists());
    assert_eq!(
        fs::read(receipt(&entry).unwrap().staging).unwrap(),
        b"synthetic exact bytes"
    );
    assert_eq!(
        fs::read(root.path().join("source.bin")).unwrap(),
        b"synthetic exact bytes"
    );
}
#[test]
fn publication_and_undo_sync_failures_keep_exact_recoverable_slots() {
    let (root, mut entry) = fixture();
    let prepared = prepare(&mut entry).unwrap().unwrap();
    hook(Edge::AfterPublish, |_| {
        Err(io::Error::other("injected publication sync failure"))
    });
    let failure = prepared.publish().unwrap_err();
    assert_eq!(failure.0, EntryState::Applying);
    let bound = recovery_binding(&entry).unwrap();
    assert!(classify(&entry, &bound).unwrap());
    hook(Edge::AfterUndoMove, |_| {
        Err(io::Error::other("injected Undo sync failure"))
    });
    let failure = undo(&entry, root.path()).unwrap_err();
    assert!(failure.starts_with(super::super::rollback::IN_FLIGHT_MARK));
    let bound = recovery_binding(&entry).unwrap();
    assert!(!classify(&entry, &bound).unwrap());
    assert!(!entry.destination_path.exists());
}
