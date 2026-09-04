//! The RomM adapter.
//!
//! RomM is a self-hosted library manager that has usually already scanned and
//! matched a collection. Stage 1 reads that work and nothing else: there is no
//! code path here that writes to RomM, triggers a scan, edits metadata or
//! touches a ROM.
//!
//! # Where the adapter boundary is
//!
//! [`config`] holds what a person configured, [`capability`] holds what the
//! instance said about itself, and [`client`] performs bounded read-only
//! requests. The identity model in [`super::model`] knows nothing about RomM, so
//! supporting a future RomM release means changing the mapping in this directory
//! rather than reshaping EmuWiz's identity.
//!
//! # Verified against a real instance
//!
//! The field and endpoint names here were read from the OpenAPI document a real
//! RomM 5.1.0 publishes at `/openapi.json`, not from documentation or memory.
//! What that instance actually exposes is recorded in [`capability`], along with
//! how the adapter behaves when a field or endpoint is missing.

pub mod capability;
pub mod client;
pub mod config;
pub mod duplicate_provider_report;
pub mod enrichment;
pub mod import;
pub mod linkage;
pub mod manual;
pub mod mapping_plan;
pub mod media_mapping;
pub mod normalise;
pub mod stale_report;

#[cfg(test)]
mod tests;
