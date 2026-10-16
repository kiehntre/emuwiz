//! Descriptor binding in `hash_file_reporting`. Kept outside `hashing.rs`,
//! which `stage1b_tests` requires to contain no filesystem writes.

use super::*;
use std::cell::Cell;
use std::fs;

/// The path is replaced by another object between the fingerprint and the
/// open. The after-read fingerprint check would also refuse, but only
/// after hashing the imposter; the descriptor binding must refuse before a
/// single byte of it is read.
#[test]
fn an_object_swapped_in_before_open_is_refused_before_any_byte_is_hashed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.bin");
    let original = temp.path().join("original-aside");
    fs::write(&path, b"original").unwrap();
    let trusted = TrustedRoots::from_paths([temp.path()]);

    let (hook_path, hook_original) = (path.clone(), original.clone());
    BEFORE_OPEN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            fs::rename(&hook_path, &hook_original).unwrap();
            fs::write(&hook_path, b"imposter").unwrap();
        }))
    });
    let most_read = Cell::new(0_u64);
    let result = hash_file_reporting(&path, &trusted, None, &|progress| {
        most_read.set(most_read.get().max(progress.bytes_read));
    });
    BEFORE_OPEN.with(|hook| *hook.borrow_mut() = None);

    assert!(
        matches!(result, Err(HashRefusal::ChangedWhileReading)),
        "{result:?}"
    );
    assert_eq!(
        most_read.get(),
        0,
        "the swapped-in object was hashed before being refused"
    );
}
