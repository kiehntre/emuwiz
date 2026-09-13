//! Pure, read-only release/media topology. A plan is data, never launch authority.
//! Existing parsers and identity producers supply evidence through adapters.
mod adapters;
mod engine;
mod inspect;
mod model;
mod naming;
mod plan;
pub use adapters::*;
pub use engine::{MediaIndex, index_media, resolve_index};
pub use inspect::{InspectionLimits, inspect_paths};
pub use model::*;
pub use naming::filename_evidence;
pub use plan::media_swap_plan;
#[cfg(test)]
mod tests;
