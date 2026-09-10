//! App-owned lifecycle for the read-only LaunchBoxLocal provider snapshot.
//!
//! XML and media indexing happen once on a worker.  Selected-game rendering
//! receives only the immutable snapshot and performs map lookups in memory.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::identity_source::launchbox_local::LaunchBoxLocalProviderIndex;

#[derive(Debug)]
pub(crate) enum LaunchBoxLocalState {
    NotConfigured,
    Loading,
    Ready(LaunchBoxLocalProviderIndex),
    Error(String),
}

pub(crate) struct LaunchBoxLocalMediaState {
    state: LaunchBoxLocalState,
    snapshot: Option<LaunchBoxLocalProviderIndex>,
    receiver: Option<Receiver<Result<LaunchBoxLocalProviderIndex, String>>>,
    generation: u64,
}

impl Default for LaunchBoxLocalMediaState {
    fn default() -> Self {
        Self {
            state: LaunchBoxLocalState::NotConfigured,
            snapshot: None,
            receiver: None,
            generation: 0,
        }
    }
}

impl LaunchBoxLocalMediaState {
    pub(crate) fn start(&mut self, repaint: eframe::egui::Context) {
        if !matches!(self.state, LaunchBoxLocalState::NotConfigured) {
            return;
        }
        let Some(root) = discover_root() else {
            return;
        };
        self.start_root(root, repaint);
    }

    #[cfg(test)]
    pub(crate) fn start_root_for_test(&mut self, root: PathBuf, repaint: eframe::egui::Context) {
        self.start_root(root, repaint);
    }

    fn start_root(&mut self, root: PathBuf, repaint: eframe::egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.state = LaunchBoxLocalState::Loading;
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = archivefs_core::identity_source::launchbox_local::discover_launchbox_local(
                &root, generation,
            );
            let _ = sender.send(result);
            repaint.request_repaint();
        });
    }

    pub(crate) fn refresh(&mut self, repaint: eframe::egui::Context) {
        let root = match &self.snapshot {
            Some(snapshot) => snapshot.root.clone(),
            None => discover_root().unwrap_or_default(),
        };
        self.receiver = None;
        if root.as_os_str().is_empty() || !root.is_dir() {
            self.state = LaunchBoxLocalState::NotConfigured;
            return;
        }
        self.start_root(root, repaint);
    }

    pub(crate) fn poll(&mut self) -> bool {
        let Some(receiver) = self.receiver.as_ref() else {
            return false;
        };
        match receiver.try_recv() {
            Ok(Ok(snapshot)) => {
                self.snapshot = Some(snapshot.clone());
                self.state = LaunchBoxLocalState::Ready(snapshot);
                self.receiver = None;
                true
            }
            Ok(Err(error)) => {
                self.state = LaunchBoxLocalState::Error(error);
                self.receiver = None;
                false
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.state = LaunchBoxLocalState::Error(
                    "LaunchBoxLocal provider worker stopped".to_string(),
                );
                self.receiver = None;
                false
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Option<&LaunchBoxLocalProviderIndex> {
        self.snapshot.as_ref()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn error(&self) -> Option<&str> {
        match &self.state {
            LaunchBoxLocalState::Error(error) => Some(error),
            _ => None,
        }
    }

    pub(crate) fn state(&self) -> &LaunchBoxLocalState {
        &self.state
    }
}

fn discover_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("LAUNCHBOX_ROOT") {
        let root = PathBuf::from(root);
        return root.is_dir().then_some(root);
    }
    let home = std::env::var_os("HOME")?;
    let user = std::env::var_os("USER").unwrap_or_else(|| "user".into());
    let root = PathBuf::from(home)
        .join(".wine/drive_c/users")
        .join(user)
        .join("LaunchBox");
    root.is_dir().then_some(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_root_is_not_configured_without_starting_io() {
        let state = LaunchBoxLocalMediaState::default();
        assert!(matches!(state.state(), LaunchBoxLocalState::NotConfigured));
        assert!(state.snapshot().is_none());
    }

    #[test]
    fn polling_before_worker_is_non_blocking() {
        let mut state = LaunchBoxLocalMediaState::default();
        assert!(!state.poll());
    }

    #[test]
    fn background_index_becomes_ready_and_keeps_generation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("Data/Platforms")).unwrap();
        std::fs::write(
            root.path().join("Data/Platforms/Test.xml"),
            br#"<LaunchBox><Game><ID>lb-1</ID><DatabaseID>1</DatabaseID><Platform>Test</Platform><Title>Example</Title></Game></LaunchBox>"#,
        )
        .unwrap();
        let mut state = LaunchBoxLocalMediaState::default();
        state.start_root_for_test(root.path().to_path_buf(), eframe::egui::Context::default());
        assert!(matches!(state.state(), LaunchBoxLocalState::Loading));
        for _ in 0..1000 {
            if state.poll() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            matches!(state.state(), LaunchBoxLocalState::Ready(_)),
            "state: {:?}",
            state.state()
        );
        assert_eq!(
            state.snapshot().map(|snapshot| snapshot.games.len()),
            Some(1)
        );
        assert_eq!(state.generation(), 1);
    }

    #[test]
    fn empty_state_has_no_error_or_snapshot_to_present() {
        let state = LaunchBoxLocalMediaState::default();
        assert!(state.snapshot().is_none());
        assert_eq!(state.error(), None);
    }
}
