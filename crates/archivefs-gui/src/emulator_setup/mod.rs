//! Emulator Setup / Profile Discovery GUI ownership, extracted out of
//! `main.rs`.
//!
//! - `state`: the profile-discovery state enums (RetroArch, PCSX2,
//!   Dolphin, Dolphin-local, PCSX2-launch, Flycast, PCSX2-firmware,
//!   Xenia) plus their small presentation-label helpers.
//! - `controller`: the `ArchiveFsApp` methods that own profile discovery
//!   (scan/poll for every adapter), RPCS3/PCSX2 launch-readiness status
//!   loading, ScummVM detection/readiness, the RetroArch core-folder
//!   override, and the Emulator Setup page's action dispatch + the
//!   Launch Readiness input builder.
//!
//! `main.rs` still owns the `retroarch_profiles`/`pcsx2_profiles`/
//! `dolphin_profiles`/... fields on `ArchiveFsApp` (as before Part 1's
//! precedent for `cheat_workflow`) and routes into these methods; it no
//! longer owns the profile-discovery types or the bulk of the Emulator
//! Setup logic.
//!
//! Managed install/update GUI state for PCSX2/PPSSPP/DuckStation/RPCS3/
//! xemu already lived entirely in `emulator_download_page.rs`, not in
//! `main.rs`, so Part 2 does not need to move it.

mod controller;
mod state;

pub(crate) use state::*;
#[allow(unused_imports)]
pub(crate) use controller::*;
