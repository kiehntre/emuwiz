//! Doctor / Repair / Problems GUI orchestration, extracted out of
//! `main.rs`.
//!
//! - `state`: `DoctorScanOutcome`, `DoctorGathered` (the worker-thread scan
//!   payload), `DoctorRepairReview` (the confirmation-screen record that is
//!   the *only* way a repair can execute), and `DoctorScanState`.
//! - `controller`: the `ArchiveFsApp` methods that start/poll a Doctor
//!   scan, review/confirm/cancel a repair, and the thin page-bridge
//!   methods for the Doctor page body, the Problems & Repair tab router,
//!   Repair Review, Repair History, and Exact Duplicate Review. None of
//!   these re-implement diagnosis or repair - they call
//!   `doctor_page`/`problems_repair_page`/`repair_review_page`/
//!   `repair_history_page`/`exact_duplicate_review_page`, which already
//!   own their real state and rendering.
//!
//! Deliberately NOT moved (left in `main.rs`, cross-feature or
//! out-of-scope):
//! - `navigate_to_problems_repair_tab`/`reconcile_problems_repair_tab`:
//!   genuinely global navigation (`self.view`, `MainView`).
//! - `refresh_diagnostics`/`poll_diagnostics`/`cached_health_issues`/
//!   `SetupAction`/`start_setup_action`/`poll_setup_action`: app-wide
//!   startup diagnostics and first-run setup, shared with the general
//!   Diagnostics overlay and Sources' mount-root picker - not Doctor-page
//!   specific.
//! - `show_optical_conversion_page` (Disc Conversion): a pure global-route
//!   bridge with zero Doctor/Repair-owned state - the real feature lives
//!   entirely in `optical_conversion_page.rs`. Moving it here would only
//!   reduce a line count, not clarify ownership, so it stays put.
//! - `show_rom_organisation_page`: not part of this block's scope; same
//!   "first-class standalone destination" shape as Disc Conversion.
//!
//! `main.rs` still owns the `doctor_scan`/`doctor_repair_review`/
//! `doctor_repair_result`/`doctor_scan_history`/`doctor_selected_finding`
//! fields on `ArchiveFsApp` (same pattern as Parts 1-3) and the small
//! bridge call sites that route into these methods.

mod controller;
mod state;

pub(crate) use state::*;
#[allow(unused_imports)]
pub(crate) use controller::*;
