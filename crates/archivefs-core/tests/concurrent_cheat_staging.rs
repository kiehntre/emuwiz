//! Concurrent cheat-install staging: independent jobs must never share a
//! staged path, and same-destination applies must serialize or refuse.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use archivefs_core::patch_manager::{ChtInstallEntry, stage_generated_cheat_file};

fn entries() -> Vec<ChtInstallEntry> {
    vec![ChtInstallEntry {
        description: "Infinite Lives".to_string(),
        code: "0001-0002".to_string(),
        enabled: false,
        extra_fields: Vec::new(),
    }]
}

/// Stages identical content under `root_for(job)` from `jobs` threads at once.
fn stage_concurrently(
    jobs: usize,
    root_for: impl Fn(usize) -> PathBuf + Send + Sync + 'static,
) -> Vec<Result<PathBuf, String>> {
    let barrier = Arc::new(Barrier::new(jobs));
    let root_for = Arc::new(root_for);
    let handles: Vec<_> = (0..jobs)
        .map(|job| {
            let barrier = Arc::clone(&barrier);
            let root_for = Arc::clone(&root_for);
            std::thread::spawn(move || {
                barrier.wait();
                stage_generated_cheat_file(&root_for(job), "Chrono Quest (USA)", &entries(), &[])
                    .map(|staged| staged.path)
                    .map_err(|failure| failure.to_string())
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

fn assert_all_intact(results: &[Result<PathBuf, String>]) {
    let mut seen = std::collections::BTreeSet::new();
    for result in results {
        let path = result
            .as_ref()
            .unwrap_or_else(|e| panic!("staging failed: {e}"));
        assert!(
            seen.insert(path.clone()),
            "two jobs share {}",
            path.display()
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("Infinite Lives"), "{text}");
    }
}

#[test]
fn repro_shared_root_collides() {
    // Documents the original defect: a single shared root hands every job the
    // same staged path.
    let dir = tempfile::tempdir().unwrap();
    let shared: PathBuf = dir.path().join("generated-cheats");
    let mut collided = false;
    for _ in 0..200 {
        let results = stage_concurrently(8, {
            let shared = shared.clone();
            move |_| shared.clone()
        });
        let paths: std::collections::BTreeSet<_> =
            results.iter().filter_map(|r| r.as_ref().ok()).collect();
        if results.iter().any(Result::is_err) || paths.len() < results.len() {
            collided = true;
            break;
        }
    }
    assert!(collided, "shared root unexpectedly never collided");
}

#[test]
fn per_job_roots_never_collide() {
    use archivefs_core::patch_manager::new_staging_job_root;
    let dir = tempfile::tempdir().unwrap();
    let base: PathBuf = dir.path().join("generated-cheats");
    for _ in 0..50 {
        let base = base.clone();
        let results = stage_concurrently(8, move |_| new_staging_job_root(&base));
        assert_all_intact(&results);
    }
}

#[allow(dead_code)]
fn _unused(_: &Path) {}
