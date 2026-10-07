//! One shared answer to "which installation + profile does this emulator use?".
//!
//! Consumers (launching, cheats, saves, Doctor, BIOS checks, mods, settings)
//! ask this resolver instead of deciding for themselves.
//!
//! # Architecture
//!
//! * [`model`] - the typed vocabulary (candidates, evidence, modes, outcomes).
//! * [`resolve`] - the generic engine: ranking, modes, ambiguity. Owns
//!   selection.
//! * [`EmulatorProfileAdapter`] - the per-emulator seam. An adapter supplies
//!   *facts*: executable/profile pairs, readiness, resolved folders. It never
//!   ranks and never selects.
//! * [`duckstation`], [`ppsspp`] - adapters over the existing, mature
//!   discovery in `patch_manager`.
//! * [`persist`] - the remembered AUTO / PREFERRED / FORCED choice.
//!
//! # Modes
//!
//! * `Auto`: the strongest, healthiest candidate wins; ask only when evidence
//!   genuinely cannot separate usable candidates. An old executable override
//!   is evidence, never authority.
//! * `Preferred`: used when usable; otherwise bypassed *with a warning* if
//!   another candidate is clearly best. The preference is never erased.
//! * `Forced`: exactly that pair, or a typed refusal. Never substituted,
//!   never repaired.
//!
//! # Read-only
//!
//! Nothing here writes, repairs, migrates, initialises or deletes. A bad BIOS
//! folder is a [`ConfigurationWarning`], not something to fix.

pub mod desktop;
pub mod duckstation;
pub mod model;
pub mod persist;
pub mod ppsspp;
pub mod resolve;

#[cfg(test)]
mod tests;

pub use duckstation::DuckStationAdapter;
pub use model::*;
pub use persist::{
    StoredSelection, clear_selection_at, load_selection_from, remember_selection_to,
};
pub use ppsspp::PpssppAdapter;
pub use resolve::{refresh_emulator_profile, resolve_emulator_profile};

/// The per-emulator facts provider. Implementations must be read-only and
/// bounded; they must not rank, select, or repair.
pub trait EmulatorProfileAdapter {
    fn emulator(&self) -> crate::emulator_inventory::InventoryEmulator;

    /// Every executable/profile pair worth considering, found by the
    /// emulator's existing bounded discovery (no filesystem crawl).
    fn discover(&self) -> Vec<EmulatorProfileCandidate>;

    /// The facts for exactly this pair - used for PREFERRED and FORCED, which
    /// must judge the pinned pair itself and never a neighbour.
    fn assess(&self, identity: &CandidateIdentity) -> EmulatorProfileCandidate;
}

pub(crate) mod fsfacts {
    use std::fs;
    use std::path::Path;

    use super::model::UnusableReason;

    pub fn is_regular_file(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
    }

    pub fn is_directory(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
    }

    /// Executable safety shared by every adapter (same rules the launch
    /// bindings use: absolute, present, regular non-symlink, execute bit).
    pub fn executable_problem(path: &Path) -> Option<UnusableReason> {
        if !path.is_absolute() {
            return Some(UnusableReason::ExecutableNotAbsolute);
        }
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return Some(UnusableReason::ExecutableMissing);
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Some(UnusableReason::ExecutableNotARegularFile);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Some(UnusableReason::ExecutableNotExecutable);
            }
        }
        None
    }

    /// Whether the directory holds at least one entry (bounded to one read).
    pub fn directory_has_entries(path: &Path) -> bool {
        fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
    }

    /// Whether the directory holds a regular file ending in `.<extension>`.
    pub fn directory_has_extension(path: &Path, extension: &str) -> bool {
        let Ok(entries) = fs::read_dir(path) else {
            return false;
        };
        entries.take(2_000).flatten().any(|entry| {
            let candidate = entry.path();
            candidate
                .extension()
                .is_some_and(|found| found.eq_ignore_ascii_case(extension))
                && is_regular_file(&candidate)
        })
    }
}
