//! Selected Evidence / Identity pipeline GUI orchestration, extracted out
//! of `main.rs`.
//!
//! - `state`: `SelectedEvidenceEnrichmentState`, the deferred whole-file-
//!   checksum + No-Intro-lookup pass, kept independent of
//!   `selected_evidence_page::SelectedEvidenceState` so the fast
//!   structural/verified-identity report is never held back by it.
//! - `controller`: the `ArchiveFsApp` methods that start/poll/cancel the
//!   evidence-gather worker, reconcile it against the current selection,
//!   start/poll the deferred enrichment pass, start/poll the Hasheous
//!   check, and start/poll identity-source discovery. All of these call
//!   into `selected_evidence_page`/`identity_sources_page` (which already
//!   own their real state, rendering, and stale-generation guards) rather
//!   than reimplementing anything.
//!
//! `selected_evidence_page::SelectedEvidenceState` and
//! `selected_evidence_page::HasheousState` already lived in
//! `selected_evidence_page.rs` before this extraction, not in `main.rs` -
//! consistent with the "types already externalized, only orchestration
//! methods remained" pattern from Parts 3 and 5.
//!
//! Deliberately NOT moved (left in `main.rs`):
//! - `start_game_identity_inspection`: despite its name, this is a
//!   Cheats & Mods method - its entire body operates on
//!   `self.cheat_workflow` (the cheat-matching identity lookup, not the
//!   Selected page's evidence pipeline). Flagged as an additional Cheats
//!   leftover for the queued cleanup pass, alongside the seven already
//!   identified in Parts 2-5.
//! - `start_plan_preview_load`/`handle_plan_preview_action`/
//!   `poll_plan_preview`: a separate feature (the DAT rename plan
//!   preview) that *consumes* the evidence report as an input but is not
//!   owned by this pipeline.
//! - `review_identity`/`open_emulator_setup_for`: genuinely global
//!   navigation bridges (select archive + switch `MainView`).
//! - `show_game_details`: the Selected page's composition root - it
//!   renders the RomM panel, evidence panel, launch readiness, identity
//!   sources, ScummVM detection, plan preview, RPCS3, and PCSX2 panels
//!   together. It calls into the methods this module now owns, but it is
//!   itself cross-feature page composition, not Selected-Evidence-owned
//!   logic - moving it here would misrepresent it as owned by one
//!   feature among the many it coordinates.
//!
//! `main.rs` still owns the `selected_evidence`/`selected_evidence_generation`/
//! `selected_evidence_enrichment`/`identity_sources`/`game_identity` fields
//! on `ArchiveFsApp` (same pattern as Parts 1-5) and the small bridge call
//! sites (inside `show_game_details`) that route into these methods.

mod controller;
mod state;

pub(crate) use state::*;
