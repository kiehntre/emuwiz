use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn metadata_tick() {
    std::thread::sleep(std::time::Duration::from_millis(10));
}
fn disc(root: &Path) -> PathBuf {
    let source = root.join("disc.cue");
    std::fs::write(&source, "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n").unwrap();
    std::fs::write(root.join("data.bin"), [0; 2048]).unwrap();
    std::fs::write(root.join("audio.bin"), [0; 2352]).unwrap();
    source
}
fn populate(cache: &mut Cache<PathBuf, usize>, source: &Path, calls: &Arc<AtomicUsize>) {
    cache.update(&source.to_owned());
    let path = source.to_owned();
    let counter = calls.clone();
    assert!(cache.start(source.to_owned(), Default::default(), move || {
        Observed::inspect(&path, None, || Ok(counter.fetch_add(1, Ordering::SeqCst)))
    }));
    cache.finish_for_test();
}
#[test]
fn unchanged_frames_reuse_one_worker_inspection_without_content_reads() {
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut cache = Cache::default();
    populate(&mut cache, &source, &calls);
    for _ in 0..20 {
        cache.update(&source);
        assert_eq!(cache.value(), Some(&Ok(0)));
        assert!(!cache.start(source.clone(), Default::default(), || panic!(
            "cached evidence must be reused"
        )));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[test]
fn descriptor_data_audio_and_removed_companions_invalidate_before_display() {
    for name in ["disc.cue", "data.bin", "audio.bin"] {
        let root = tempfile::tempdir().unwrap();
        let source = disc(root.path());
        let mut cache = Cache::default();
        let calls = Arc::new(AtomicUsize::new(0));
        populate(&mut cache, &source, &calls);
        metadata_tick();
        let file = root.path().join(name);
        // Same size rewrite: metadata identity, not just length, must change.
        let mut bytes = std::fs::read(&file).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&file, bytes).unwrap();
        cache.update(&source);
        assert!(cache.value().is_none(), "{name}");
    }
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    let mut cache = Cache::default();
    let calls = Arc::new(AtomicUsize::new(0));
    populate(&mut cache, &source, &calls);
    metadata_tick();
    std::fs::remove_file(root.path().join("audio.bin")).unwrap();
    cache.update(&source);
    assert!(cache.value().is_none());
}
#[test]
fn missing_and_case_ambiguous_companions_invalidate_cached_refusals() {
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    let audio = root.path().join("audio.bin");
    std::fs::remove_file(&audio).unwrap();
    let mut cache = Cache::<PathBuf, ()>::default();
    cache.update(&source);
    cache.entry = Some(Observed::inspect(&source, None, || {
        Err("missing companion".into())
    }));
    cache.update(&source);
    assert!(cache.value().is_some());
    metadata_tick();
    std::fs::write(root.path().join("AUDIO.BIN"), [0; 2352]).unwrap();
    cache.update(&source);
    assert!(cache.value().is_none());
    cache.entry = Some(Observed::inspect(&source, None, || {
        Err("case-ambiguous companion".into())
    }));
    metadata_tick();
    std::fs::write(&audio, [0; 2352]).unwrap();
    cache.update(&source);
    assert!(cache.value().is_none());
}
#[test]
fn changed_patch_or_manifest_selection_cannot_reuse_preflight() {
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    let patch = root.path().join("patch.ips");
    std::fs::write(&patch, b"PATCHEOF").unwrap();
    // Exact manifest identity and relevant patch selection are part of a
    // caller's key. The cache never substitutes evidence across selections.
    let key = (source.clone(), "manifest-a", patch.clone());
    let mut cache = Cache::<_, ()>::default();
    cache.update(&key);
    cache.entry = Some(Observed::inspect(&source, Some(&patch), || Ok(())));
    cache.update(&(source.clone(), "manifest-b", patch.clone()));
    assert!(cache.value().is_none());
    cache.entry = Some(Observed::inspect(&source, Some(&patch), || Ok(())));
    cache.update(&(source.clone(), "manifest-b", root.path().join("other.ips")));
    assert!(cache.value().is_none());
    cache.update(&key);
    cache.entry = Some(Observed::inspect(&source, Some(&patch), || Ok(())));
    metadata_tick();
    std::fs::write(&patch, b"PATCHXYZ").unwrap();
    cache.update(&key);
    assert!(cache.value().is_none());
}
#[test]
fn late_completion_after_selection_or_manual_refresh_is_discarded() {
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    let mut cache = Cache::<PathBuf, ()>::default();
    cache.update(&source);
    let observed = Observed::inspect(&source, None, || Ok(()));
    let revision = cache.revision;
    cache.invalidate();
    cache.accept(source.clone(), revision, observed);
    assert!(cache.value().is_none());
    let observed = Observed::inspect(&source, None, || Ok(()));
    let revision = cache.revision;
    cache.update(&root.path().join("other.cue"));
    cache.accept(source, revision, observed);
    assert!(cache.value().is_none());
}
#[test]
fn changes_during_worker_inspection_never_become_current_evidence() {
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    let mut cache = Cache::<PathBuf, ()>::default();
    cache.update(&source);
    let observed = Observed::inspect(&source, None, || {
        metadata_tick();
        std::fs::write(root.path().join("audio.bin"), [1; 2352]).unwrap();
        Ok(())
    });
    cache.accept(source, cache.revision, observed);
    assert!(cache.value().is_none());
}

#[cfg(unix)]
#[test]
fn nested_alias_retarget_and_nonregular_companion_recovery_invalidate() {
    for nonregular in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let source = disc(root.path());
        let links = root.path().join("links");
        std::fs::create_dir(&links).unwrap();
        let payloads = root.path().join("payloads");
        std::fs::create_dir(&payloads).unwrap();
        std::fs::write(payloads.join("a.bin"), [0; 2352]).unwrap();
        std::fs::write(payloads.join("b.bin"), [1; 2352]).unwrap();
        let alias = links.join("audio.bin");
        if nonregular {
            std::fs::create_dir(&alias).unwrap();
        } else {
            std::os::unix::fs::symlink("../payloads/a.bin", &alias).unwrap();
        }
        let text = std::fs::read_to_string(&source)
            .unwrap()
            .replace("audio.bin", "links/audio.bin");
        std::fs::write(&source, text).unwrap();
        let mut cache = Cache::<PathBuf, ()>::default();
        cache.update(&source);
        cache.entry = Some(Observed::inspect(&source, None, || {
            if nonregular {
                Err("unsafe companion".into())
            } else {
                Ok(())
            }
        }));
        assert!(cache.entry.as_ref().unwrap().sources.current());
        metadata_tick();
        if nonregular {
            std::fs::remove_dir(&alias).unwrap();
            std::fs::write(&alias, [0; 2352]).unwrap();
        } else {
            std::fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink("../payloads/b.bin", &alias).unwrap();
        }
        cache.update(&source);
        assert!(cache.value().is_none(), "nonregular={nonregular}");
    }
}

#[test]
fn restored_mtime_and_same_bytes_replacement_invalidate_identity() {
    for replace in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let source = disc(root.path());
        let mut cache = Cache::default();
        let calls = Arc::new(AtomicUsize::new(0));
        populate(&mut cache, &source, &calls);
        let file = root.path().join("audio.bin");
        let old = std::fs::metadata(&file).unwrap().modified().unwrap();
        metadata_tick();
        if replace {
            let replacement = root.path().join("replacement.bin");
            std::fs::write(&replacement, [0; 2352]).unwrap();
            std::fs::rename(replacement, &file).unwrap();
        } else {
            std::fs::write(&file, [1; 2352]).unwrap();
        }
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        cache.update(&source);
        assert!(cache.value().is_none());
    }
}
#[cfg(unix)]
#[test]
fn intermediate_directory_alias_retarget_invalidates() {
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    for name in ["links", "payloads", "first", "second"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    for name in ["first", "second"] {
        std::fs::write(root.path().join(name).join("audio.bin"), [0; 2352]).unwrap();
    }
    std::os::unix::fs::symlink("../payloads", root.path().join("links/folder")).unwrap();
    std::os::unix::fs::symlink("../first", root.path().join("payloads/inner")).unwrap();
    let text = std::fs::read_to_string(&source)
        .unwrap()
        .replace("audio.bin", "links/folder/inner/audio.bin");
    std::fs::write(&source, text).unwrap();
    let mut cache = Cache::<PathBuf, ()>::default();
    cache.update(&source);
    cache.entry = Some(Observed::inspect(&source, None, || Ok(())));
    metadata_tick();
    std::fs::remove_file(root.path().join("payloads/inner")).unwrap();
    std::os::unix::fs::symlink("../second", root.path().join("payloads/inner")).unwrap();
    cache.update(&source);
    assert!(cache.value().is_none());
}
#[test]
fn malformed_deep_references_have_a_bounded_refusal() {
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    let deep = "dir/".repeat(100);
    std::fs::write(&source, format!("FILE \"{deep}audio.bin\" BINARY\n")).unwrap();
    let observed = Observed::<()>::inspect(&source, None, || {
        panic!("unbounded watch sets cannot authorize inspection")
    });
    assert!(
        observed
            .value
            .unwrap_err()
            .contains("bounded cache watch set")
    );
    assert!(observed.sources.stamps.len() <= MAX_WATCHED_PATHS);
}

#[cfg(unix)]
#[test]
fn intermediate_file_alias_retarget_invalidates() {
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    for name in ["links", "hops", "payloads"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    for name in ["a.bin", "b.bin"] {
        std::fs::write(root.path().join("payloads").join(name), [0; 2352]).unwrap();
    }
    std::os::unix::fs::symlink("../hops/audio.bin", root.path().join("links/audio.bin")).unwrap();
    std::os::unix::fs::symlink("../payloads/a.bin", root.path().join("hops/audio.bin")).unwrap();
    let text = std::fs::read_to_string(&source)
        .unwrap()
        .replace("audio.bin", "links/audio.bin");
    std::fs::write(&source, text).unwrap();
    let mut cache = Cache::<PathBuf, ()>::default();
    cache.update(&source);
    cache.entry = Some(Observed::inspect(&source, None, || Ok(())));
    assert!(cache.entry.as_ref().unwrap().sources.current());
    // Link target identity catches this even within one filesystem timestamp tick.
    std::fs::remove_file(root.path().join("hops/audio.bin")).unwrap();
    std::os::unix::fs::symlink("../payloads/b.bin", root.path().join("hops/audio.bin")).unwrap();
    cache.update(&source);
    assert!(cache.value().is_none());
}
#[test]
fn disconnected_old_worker_cannot_block_a_new_selection() {
    let root = tempfile::tempdir().unwrap();
    let source = disc(root.path());
    let mut cache = Cache::<PathBuf, ()>::default();
    cache.update(&source);
    let (sender, receiver) = mpsc::channel();
    cache.pending = Some(Pending {
        key: source.clone(),
        revision: cache.revision,
        receiver,
    });
    drop(sender);
    cache.update(&root.path().join("other.cue"));
    assert!(cache.error.is_none());
    assert!(!cache.busy());
    assert!(cache.value().is_none());
}
