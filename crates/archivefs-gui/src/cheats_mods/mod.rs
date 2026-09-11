//! Cheats & Mods GUI ownership, extracted out of `main.rs`.
//!
//! - `state`: `CheatWorkflowState` and every Cheats/Mods-specific
//!   supporting struct/enum (candidate selection, provider request keys,
//!   generated-install records, preview/transaction state, ...).
//! - `actions`: the typed action boundary the Cheats & Mods page emits
//!   (`CheatWorkflowAction`, `CheatArchivePickerAction`).
//! - `controller`: the `ArchiveFsApp` methods that own the Cheats/Mods
//!   workflow - discovery, fetch/import, worker start/poll, Cloudflare and
//!   offline-import handling, reconciliation, preview, apply, rollback.
//! - `render`: the Cheats/Mods rendering + presentation helper functions
//!   used by `cheats_mods_preview.rs` and the per-emulator workflow pages.
//!
//! `main.rs` still owns the `cheat_workflow: Option<CheatWorkflowState>`
//! field on `ArchiveFsApp` and routes into these methods; it no longer
//! owns the Cheats/Mods types or the bulk of the Cheats/Mods logic.

mod actions;
mod controller;
mod render;
mod state;

pub(crate) use actions::*;
pub(crate) use render::*;
pub(crate) use state::*;
