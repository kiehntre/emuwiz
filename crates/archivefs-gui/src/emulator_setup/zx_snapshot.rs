//! One bounded background readiness worker. Cached proofs never replace
//! canonical preflight or final spawn revalidation.
use archivefs_core::classic_trainer_evidence::snapshot::SnapshotEvidence;
use eframe::egui;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant, SystemTime},
};
type Stamp = Vec<Option<(u64, SystemTime)>>;
struct Cached {
    path: PathBuf,
    stamp: Stamp,
    at: Instant,
    proof: Option<SnapshotEvidence>,
}
struct Pending {
    path: PathBuf,
    stamp: Stamp,
    result: Receiver<Option<SnapshotEvidence>>,
}
#[derive(Default)]
struct Cache {
    cached: Option<Cached>,
    pending: Option<Pending>,
    context: Option<egui::Context>,
}
thread_local! {static CACHE:RefCell<Cache>=RefCell::new(Cache::default());}

/// Register the live UI's repaint target. This performs no evidence I/O.
pub(crate) fn poll(context: &egui::Context) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.collect();
        cache.context = Some(context.clone());
        if cache.pending.is_some() {
            context.request_repaint_after(Duration::from_millis(50));
        }
    });
}
impl Cache {
    fn collect(&mut self) {
        let Some(pending) = &self.pending else {
            return;
        };
        let proof = match pending.result.try_recv() {
            Ok(proof) => proof,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        let pending = self.pending.take().unwrap();
        self.cached = Some(Cached {
            path: pending.path,
            stamp: pending.stamp,
            at: Instant::now(),
            proof,
        });
    }
    fn lookup(
        &mut self,
        path: &Path,
        stamp: Stamp,
        resolve: impl FnOnce() -> Option<SnapshotEvidence> + Send + 'static,
    ) -> Option<SnapshotEvidence> {
        self.collect();
        let current = self.cached.as_ref().filter(|c| {
            c.path == path
                && c.stamp == stamp
                && c.proof
                    .as_ref()
                    .is_none_or(|p| p.verify_read_guards().is_ok())
        });
        let proof = current.and_then(|c| c.proof.clone());
        let refresh = current.is_none_or(|c| c.at.elapsed() >= Duration::from_secs(10));
        // Only one resolver may hash catalogues at a time. A selection change
        // discards the old result by key and schedules its replacement afterward.
        if refresh && self.pending.is_none() {
            let (sender, result) = mpsc::channel();
            let context = self.context.clone();
            std::thread::spawn(move || {
                let proof = resolve();
                let _ = sender.send(proof);
                if let Some(context) = context {
                    context.request_repaint();
                }
            });
            self.pending = Some(Pending {
                path: path.into(),
                stamp,
                result,
            });
        }
        proof
    }
}
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
    let selected = path.to_path_buf();
    CACHE.with(|cache| {
        cache.borrow_mut().lookup(path, stamp, move || {
            SnapshotEvidence::current(&selected).ok()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolver_never_runs_on_render_thread_and_selection_churn_stays_bounded() {
        let mut cache = Cache::default();
        let caller = std::thread::current().id();
        let (started, start) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        assert!(
            cache
                .lookup(Path::new("/first.z80"), vec![], move || {
                    started.send(std::thread::current().id()).unwrap();
                    gate.recv().unwrap();
                    None
                })
                .is_none()
        );
        assert_ne!(start.recv_timeout(Duration::from_secs(2)).unwrap(), caller);
        assert!(
            cache
                .lookup(Path::new("/second.z80"), vec![], || panic!(
                    "a second worker must not be spawned while the first is running"
                ))
                .is_none()
        );
        assert_eq!(
            cache.pending.as_ref().unwrap().path,
            Path::new("/first.z80")
        );
        release.send(()).unwrap();
        // Consume the completed message deterministically, then put it back for
        // the production collect path. No catalogue or runtime sleeps are needed.
        let old = cache.pending.take().unwrap();
        let proof = old.result.recv_timeout(Duration::from_secs(2)).unwrap();
        let (sender, result) = mpsc::channel();
        sender.send(proof).unwrap();
        cache.pending = Some(Pending {
            path: old.path,
            stamp: old.stamp,
            result,
        });
        cache.collect();
        assert_eq!(cache.cached.as_ref().unwrap().path, Path::new("/first.z80"));
        assert!(
            cache
                .lookup(Path::new("/second.z80"), vec![], || None)
                .is_none()
        );
        assert_eq!(
            cache.pending.as_ref().unwrap().path,
            Path::new("/second.z80")
        );
    }
    #[test]
    fn failed_worker_is_cached_without_granting_authority() {
        let mut cache = Cache::default();
        let (sender, result) = mpsc::channel();
        sender.send(None).unwrap();
        cache.pending = Some(Pending {
            path: "/missing.z80".into(),
            stamp: vec![],
            result,
        });
        assert!(
            cache
                .lookup(Path::new("/missing.z80"), vec![], || panic!(
                    "negative cache should avoid retrying every frame"
                ))
                .is_none()
        );
        assert!(cache.pending.is_none());
    }
}
