//! Bounded readiness reuse only. Cached evidence is never spawn authority:
//! canonical preflight and spawn always reload source-aware Gold evidence.
use archivefs_core::classic_trainer_evidence::snapshot::SnapshotEvidence;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};
type Stamp = Vec<Option<(u64, SystemTime)>>;
struct Cached {
    path: PathBuf,
    stamp: Stamp,
    at: Instant,
    proof: Option<SnapshotEvidence>,
}
thread_local! {static CACHE:RefCell<Option<Cached>>=const{RefCell::new(None)};}
pub(crate) fn readiness(path: &Path) -> Option<SnapshotEvidence> {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("z80"))
    {
        return None;
    }
    let stores = [
        Some(path.to_path_buf()),
        archivefs_core::default_database_path().ok(),
        archivefs_core::default_config_path().ok(),
        archivefs_core::dat::sources::default_dat_sources_config_path().ok(),
    ];
    let stamp = stores
        .iter()
        .map(|p| {
            p.as_ref()
                .and_then(|p| std::fs::symlink_metadata(p).ok())
                .and_then(|m| Some((m.len(), m.modified().ok()?)))
        })
        .collect::<Stamp>();
    CACHE.with(|cache| {
        if let Some(cached) = cache.borrow().as_ref() {
            if cached.path == path
                && cached.stamp == stamp
                && cached.at.elapsed() < Duration::from_secs(10)
                && cached
                    .proof
                    .as_ref()
                    .is_none_or(|p| p.verify_read_guards().is_ok())
            {
                return cached.proof.clone();
            }
        }
        let proof = SnapshotEvidence::current(path).ok();
        *cache.borrow_mut() = Some(Cached {
            path: path.into(),
            stamp,
            at: Instant::now(),
            proof: proof.clone(),
        });
        proof
    })
}
