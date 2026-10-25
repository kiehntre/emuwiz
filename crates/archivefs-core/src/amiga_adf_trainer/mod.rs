//! Foundation for trainers on ordinary (non-WHDLoad) Amiga disk images.
//!
//! WHDLoad trainers are launch options and live in
//! `patch_manager::whdload_trainer`; they do not apply here. For a loose ADF the
//! only evidenced trainer mechanism is writing memory in a *running* emulator,
//! so this module stops at a verified, immutable-source **plan**: what would be
//! written, to exactly which image, with which provenance and conflicts. It
//! never modifies the image, spawns a process, or claims a cheat works.

mod import;
mod media;
mod model;
mod plan;

#[cfg(test)]
mod tests;

pub use import::*;
pub use media::*;
pub use model::*;
pub use plan::*;
