//! RomM GUI orchestration owned by `main.rs`'s `ArchiveFsApp`, extracted
//! out of `main.rs` itself.
//!
//! - `state`: `RunningRommOperation`, the in-flight-operation record kept
//!   on `ArchiveFsApp` (generation, cancellation flag, result/progress
//!   receivers).
//! - `controller`: the `ArchiveFsApp` methods that start/cancel/poll a
//!   RomM operation, load/render the configuration dialog, open/render
//!   the browse window and dispatch its requests, and render/dispatch the
//!   RomM game panel.
//!
//! `RommOperation`/`RommOperationOutcome`/`RommProgress`/
//! `RommProgressEvent` already lived in `romm_source.rs` before this
//! extraction (not in `main.rs`), as did the browse/config/game page
//! implementations in `romm_browse.rs`/`romm_config.rs`/`romm_game.rs` -
//! this module does not touch or duplicate any of that; it only owns the
//! `ArchiveFsApp`-side orchestration that used to live directly in
//! `main.rs`.
//!
//! `main.rs` still owns the `romm_operation: Option<RunningRommOperation>`
//! field on `ArchiveFsApp` (same pattern as Parts 1 and 2) and the small
//! bridge call sites that route into these methods.

mod controller;
mod state;

pub(crate) use state::*;
