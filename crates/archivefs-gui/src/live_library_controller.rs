//! Live archive-index loading and refresh state.
//!
//! This is deliberately separate from `database_load`: the live snapshot is
//! the authoritative in-memory archive view used for actions, while the
//! database controller owns the persisted catalogue cache.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui;

use crate::database_load::CachedLibrarySnapshot;

use super::{ArchiveRow, LoadedData, build_display_rows, load_read_only_snapshot_default};

pub(crate) type LoadResult = Result<LoadedData, String>;
pub(crate) type LoadMessage = (RefreshGeneration, LoadResult);

pub(crate) enum LoadState {
    Loading {
        generation: RefreshGeneration,
        receiver: Receiver<LoadMessage>,
        previous: Option<Box<LoadedData>>,
    },
    Ready(Box<LoadedData>),
    Error(String),
}

pub(crate) enum LiveLibraryPoll {
    Completed { merged_rows: Vec<ArchiveRow> },
    Failed { error: String, has_previous: bool },
}

pub(crate) fn poll_load(
    state: &mut LoadState,
    current_generation: RefreshGeneration,
    database_snapshot: Option<&CachedLibrarySnapshot>,
) -> Option<LiveLibraryPoll> {
    let result = match state {
        LoadState::Loading {
            generation,
            receiver,
            ..
        } => match receiver.try_recv() {
            Ok(message) => Some(message),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some((
                *generation,
                Err("background data loader stopped unexpectedly".to_string()),
            )),
        },
        LoadState::Ready(_) | LoadState::Error(_) => None,
    };

    let (generation, result) = result?;
    if generation != current_generation {
        return None;
    }
    let (state_generation, previous) =
        match std::mem::replace(state, LoadState::Error("load result pending".to_string())) {
            LoadState::Loading {
                generation,
                previous,
                ..
            } => (Some(generation), previous),
            LoadState::Ready(_) | LoadState::Error(_) => (None, None),
        };
    if state_generation != Some(generation) {
        return None;
    }

    match result {
        Ok(data) => {
            let merged_rows = build_display_rows(&data.records, &data.rows, database_snapshot);
            *state = LoadState::Ready(Box::new(data));
            Some(LiveLibraryPoll::Completed { merged_rows })
        }
        Err(error) => {
            let has_previous = previous.is_some();
            *state = previous.map_or_else(|| LoadState::Error(error.clone()), LoadState::Ready);
            Some(LiveLibraryPoll::Failed {
                error,
                has_previous,
            })
        }
    }
}

pub(crate) fn start_load(
    context: egui::Context,
    generation: RefreshGeneration,
    previous: Option<Box<LoadedData>>,
) -> LoadState {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = load_data();
        let _ = sender.send((generation, result));
        context.request_repaint();
    });
    LoadState::Loading {
        generation,
        receiver,
        previous,
    }
}

fn load_data() -> LoadResult {
    load_read_only_snapshot_default()
        .map(LoadedData::from_snapshot)
        .map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RefreshGeneration(pub(crate) u64);

impl RefreshGeneration {
    pub(crate) const INITIAL: Self = Self(0);

    pub(crate) fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}
