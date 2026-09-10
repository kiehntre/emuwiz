//! Background-owned ES-DE media provider state for selected-game lookups.
//!
//! XML parsing and media-reference indexing happen only on this worker. The UI
//! thread retains the immutable snapshot and performs exact map lookups.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::emulator_environment::es_de_metadata::{
    EsDeProviderCollection, discover_provider_snapshot,
};

#[derive(Debug)]
pub(crate) enum EsDeProviderState {
    NotStarted,
    Loading,
    Ready(EsDeProviderCollection),
    Error(String),
}

pub(crate) struct EsDeMediaState {
    state: EsDeProviderState,
    snapshot: Option<EsDeProviderCollection>,
    receiver: Option<Receiver<Result<EsDeProviderCollection, String>>>,
    generation: u64,
}

impl Default for EsDeMediaState {
    fn default() -> Self {
        Self {
            state: EsDeProviderState::NotStarted,
            snapshot: None,
            receiver: None,
            generation: 0,
        }
    }
}

impl EsDeMediaState {
    pub(crate) fn refresh(&mut self, repaint: eframe::egui::Context) {
        self.receiver = None;
        self.state = EsDeProviderState::NotStarted;
        self.start(repaint);
    }

    pub(crate) fn start(&mut self, repaint: eframe::egui::Context) {
        if !matches!(self.state, EsDeProviderState::NotStarted) {
            return;
        }
        let Some(home) = std::env::var_os("HOME") else {
            self.state = EsDeProviderState::Error("HOME is not set".to_string());
            return;
        };
        let root = PathBuf::from(home).join("ES-DE");
        let (sender, receiver) = mpsc::channel();
        self.state = EsDeProviderState::Loading;
        self.receiver = Some(receiver);
        let generation = self.generation.wrapping_add(1);
        self.generation = generation;
        std::thread::spawn(move || {
            let result = if root.is_dir() {
                Ok(discover_provider_snapshot(&root, generation))
            } else {
                Err(format!("ES-DE root is not configured: {}", root.display()))
            };
            let _ = sender.send(result);
            repaint.request_repaint();
        });
    }

    /// Polls once without blocking. Returns true when the active snapshot
    /// changed, allowing the caller to invalidate media generations.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(receiver) = self.receiver.as_ref() else {
            return false;
        };
        match receiver.try_recv() {
            Ok(Ok(snapshot)) => {
                self.snapshot = Some(snapshot.clone());
                self.state = EsDeProviderState::Ready(snapshot);
                self.receiver = None;
                true
            }
            Ok(Err(error)) => {
                self.state = EsDeProviderState::Error(error);
                self.receiver = None;
                false
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.state = EsDeProviderState::Error("ES-DE provider worker stopped".to_string());
                self.receiver = None;
                false
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Option<&EsDeProviderCollection> {
        self.snapshot.as_ref()
    }

    pub(crate) fn error(&self) -> Option<&str> {
        match &self.state {
            EsDeProviderState::Error(error) => Some(error),
            _ => None,
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn state(&self) -> &EsDeProviderState {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_snapshot(generation: u64) -> EsDeProviderCollection {
        EsDeProviderCollection {
            root: PathBuf::from("/fixture/ES-DE"),
            generation,
            indexes: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn provider_starts_absent_and_has_no_snapshot_before_background_load() {
        let state = EsDeMediaState::default();
        assert!(matches!(state.state(), EsDeProviderState::NotStarted));
        assert!(state.snapshot().is_none());
    }

    #[test]
    fn polling_without_a_started_provider_is_non_blocking() {
        let mut state = EsDeMediaState::default();
        assert!(!state.poll());
    }

    #[test]
    fn retained_snapshot_survives_refresh_state_and_error() {
        let snapshot = fixture_snapshot(7);
        let mut state = EsDeMediaState {
            state: EsDeProviderState::Ready(snapshot.clone()),
            snapshot: Some(snapshot),
            receiver: None,
            generation: 7,
        };
        state.state = EsDeProviderState::Loading;
        assert_eq!(state.snapshot().map(|value| value.generation), Some(7));
        state.state = EsDeProviderState::Error("fixture refresh failed".into());
        assert_eq!(state.snapshot().map(|value| value.generation), Some(7));
        assert_eq!(state.error(), Some("fixture refresh failed"));
    }
}
