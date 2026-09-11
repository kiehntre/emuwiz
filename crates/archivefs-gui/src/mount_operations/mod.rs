//! Bulk mount/unmount ("Mount All" / "Unmount All") GUI orchestration
//! owned by `main.rs`'s `ArchiveFsApp`, extracted out of `main.rs` itself.
//!
//! - `controller`: the `ArchiveFsApp` methods that start/poll/request-stop
//!   for both the Mount All and Unmount All workers.
//!
//! All of the actual state types (`RunningMountAll`, `RunningUnmountAll`,
//! `MountAllItem`/`Confirmation`/`Progress`/`Result`/`Failure`/`Skipped`,
//! `UnmountAllItem`/`Confirmation`/`Progress`/`Result`/`Failure`/`Skip`/
//! `CleanupFailure`, and the `BatchMountAttempt`/`BatchUnmountAttempt`
//! outcome enums) already lived in `mount_batch.rs` before this
//! extraction, not in `main.rs` - so there was nothing to move there.
//! This module owns only the `ArchiveFsApp`-side worker lifecycle:
//! spawn, progress polling, stop-request handling, completion, and
//! result/error recording, previously implemented directly in `main.rs`.
//!
//! The generic single-archive operation queue (`AppOperationRequest`,
//! `start_operation`/`poll_operation`) is genuinely shared worker
//! infrastructure used by mount-all, unmount-all, and ordinary
//! single-archive actions alike - it was not duplicated or moved here,
//! consistent with Part 4's treatment of similar shared infrastructure.
//!
//! `main.rs` still owns the `mount_all`/`unmount_all`/`confirm_mount_all`/
//! `confirm_unmount_all` fields on `ArchiveFsApp` (same pattern as Parts
//! 1-4) and the small bridge call sites that route into these methods.

mod controller;
