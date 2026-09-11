//! Platform / Alias / Source health action GUI orchestration, extracted
//! out of `main.rs`.
//!
//! - `state`: `PlatformAction`/`RunningPlatformAction`,
//!   `BulkPlatformActionKind`/`BulkPlatformActionOutcome`/
//!   `RunningBulkPlatformAction`, `CUSTOM_PLATFORM_CHOICE`, `AliasAction`/
//!   `RunningAliasAction`, `SourceAction`/`SourceActionOutcome`/
//!   `RunningSourceAction`, and the Sources page's own scan-result echo
//!   (`SourcesScanScope`/`SourcesLastScan`, set by `poll_source_action`).
//! - `controller`: the `ArchiveFsApp` methods that check eligibility for
//!   and start/poll each of the four action kinds (single platform
//!   assignment, bulk platform assignment, custom-platform alias, and
//!   Sources-page source management). Every variant calls straight into
//!   the existing `archivefs_core` functions (the same ones the CLI
//!   uses) - nothing here reimplements validation, canonicalization,
//!   scanning, or persistence.
//!
//! `main.rs` still owns the `platform_action`/`bulk_platform_action`/
//! `alias_action`/`source_action`/`confirm_bulk_platform_action`/
//! `sources_last_scan` fields on `ArchiveFsApp` (same pattern as Parts
//! 1-6) and the small bridge call sites that route into these methods.

mod controller;
mod state;

pub(crate) use state::*;
