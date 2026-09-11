//! Background-owned ES-DE media provider state for selected-game lookups.
//!
//! XML parsing and media-reference indexing happen only on this worker. The UI
//! thread retains the immutable snapshot and performs exact map lookups.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::emulator_environment::es_de_metadata::EsDeProviderCollection;

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
        let canonical_rom_root = archivefs_core::Config::load_default()
            .ok()
            .and_then(|config| config.master_rom_root);
        let (sender, receiver) = mpsc::channel();
        self.state = EsDeProviderState::Loading;
        self.receiver = Some(receiver);
        let generation = self.generation.wrapping_add(1);
        self.generation = generation;
        std::thread::spawn(move || {
            let result = if root.is_dir() {
                Ok(archivefs_core::emulator_environment::es_de_metadata::
                    discover_provider_snapshot_with_rom_root(
                        &root,
                        generation,
                        canonical_rom_root.as_deref(),
                    ))
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

    #[test]
    fn late_generation_cannot_replace_the_current_refresh_snapshot() {
        let (old_sender, old_receiver) = mpsc::channel::<Result<EsDeProviderCollection, String>>();
        let (current_sender, current_receiver) =
            mpsc::channel::<Result<EsDeProviderCollection, String>>();
        let old = fixture_snapshot(1);
        let current = fixture_snapshot(2);
        let mut state = EsDeMediaState {
            state: EsDeProviderState::Loading,
            snapshot: None,
            receiver: Some(old_receiver),
            generation: 2,
        };
        state.receiver = Some(current_receiver);
        assert!(old_sender.send(Ok(old)).is_err());
        current_sender.send(Ok(current)).unwrap();
        assert!(state.poll());
        assert_eq!(state.snapshot().map(|value| value.generation), Some(2));
    }
}
