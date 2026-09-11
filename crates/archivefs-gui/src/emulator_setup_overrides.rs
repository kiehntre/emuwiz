//! GUI-only executable/configuration-folder override state for Emulator
//! Setup remediation controls.
//!
//! Every override here maps onto a real, already-existing typed field on
//! the adapter's own `*ProfileDiscoveryRoots` in `archivefs-core` (see
//! `archivefs_core::diagnostics::profiles::DiscoveredProfilesOverrides`,
//! which this module's [`EmulatorPathOverrides::as_core_overrides`]
//! projects into). No adapter here gains a setting it did not already
//! support - see that type's own doc comment for exactly which adapters
//! have an executable channel, a configuration-folder channel, both, or
//! neither (PCSX2: executable only; Dolphin: configuration folder only;
//! the rest: both).
//!
//! Persistence follows the exact convention `main.rs`'s
//! `retroarch_core_directory_override.txt` already established: one plain
//! path per file, under the EmuWiz config directory
//! (`archivefs_core::app_dirs::config_path`), read/written verbatim - not a
//! second config system, just one more small file beside it. A missing or
//! blank file means "automatic detection", exactly like RetroArch's.

use std::path::{Path, PathBuf};

use archivefs_core::diagnostics::profiles::DiscoveredProfilesOverrides;

/// The emulators this pass gives direct remediation controls to - every one
/// of them already has a proven `explicit_executables` and/or
/// `explicit_configuration_roots` channel in its own
/// `*ProfileDiscoveryRoots` (see the audit in the 0.8.2 UX pass task).
/// RetroArch is deliberately not here: it already has its own dedicated
/// core-folder override (`retroarch_core_setup`), untouched by this pass.
/// ScummVM is deliberately not here either: no typed override channel was
/// found for it in this audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum OverridableEmulator {
    Pcsx2,
    Rpcs3,
    Ppsspp,
    DuckStation,
    Xemu,
    Dolphin,
}

impl OverridableEmulator {
    pub(crate) const ALL: [Self; 6] = [
        Self::Pcsx2,
        Self::Rpcs3,
        Self::Ppsspp,
        Self::DuckStation,
        Self::Xemu,
        Self::Dolphin,
    ];

    /// The `LAUNCH_COMPATIBILITY`/`EmulatorSetupCandidate` adapter id this
    /// override applies to - the same string `emulator_setup_page` already
    /// uses for every candidate card.
    pub(crate) fn adapter_id(self) -> &'static str {
        match self {
            Self::Pcsx2 => "pcsx2",
            Self::Rpcs3 => "rpcs3",
            Self::Ppsspp => "ppsspp",
            Self::DuckStation => "duckstation",
            Self::Xemu => "xemu",
            Self::Dolphin => "dolphin",
        }
    }

    pub(crate) fn from_adapter_id(adapter_id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|emulator| emulator.adapter_id() == adapter_id)
    }

    /// Whether this adapter has a proven `explicit_executables` channel.
    /// Dolphin's only executable-shaped field
    /// (`DolphinProfileDiscoveryRoots::selected_executable`) merely breaks a
    /// tie among already-running processes - it never introduces a new
    /// executable candidate to discovery - so Dolphin has no real
    /// executable-override channel today.
    pub(crate) fn supports_executable_override(self) -> bool {
        !matches!(self, Self::Dolphin)
    }

    /// Whether this adapter has a proven `explicit_configuration_roots`
    /// channel. PCSX2 has none today (only `portable_configuration_roots`,
    /// which must come from an already-known configuration, never a blind
    /// user pick).
    pub(crate) fn supports_configuration_folder_override(self) -> bool {
        !matches!(self, Self::Pcsx2)
    }

    fn executable_override_file_name(self) -> Option<&'static str> {
        match self {
            Self::Pcsx2 => Some("pcsx2_executable_override.txt"),
            Self::Rpcs3 => Some("rpcs3_executable_override.txt"),
            Self::Ppsspp => Some("ppsspp_executable_override.txt"),
            Self::DuckStation => Some("duckstation_executable_override.txt"),
            Self::Xemu => Some("xemu_executable_override.txt"),
            Self::Dolphin => None,
        }
    }

    fn configuration_folder_override_file_name(self) -> Option<&'static str> {
        match self {
            Self::Pcsx2 => None,
            Self::Rpcs3 => Some("rpcs3_configuration_folder_override.txt"),
            Self::Ppsspp => Some("ppsspp_configuration_folder_override.txt"),
            Self::DuckStation => Some("duckstation_configuration_folder_override.txt"),
            Self::Xemu => Some("xemu_configuration_folder_override.txt"),
            Self::Dolphin => Some("dolphin_configuration_folder_override.txt"),
        }
    }
}

/// In-memory session state for every override this pass supports - a plain
/// projection of the on-disk files above, loaded once at startup.
#[derive(Debug, Clone, Default)]
pub(crate) struct EmulatorPathOverrides {
    pub(crate) pcsx2_executable: Option<PathBuf>,
    pub(crate) rpcs3_executable: Option<PathBuf>,
    pub(crate) rpcs3_configuration_folder: Option<PathBuf>,
    pub(crate) ppsspp_executable: Option<PathBuf>,
    pub(crate) ppsspp_configuration_folder: Option<PathBuf>,
    pub(crate) duckstation_executable: Option<PathBuf>,
    pub(crate) duckstation_configuration_folder: Option<PathBuf>,
    pub(crate) xemu_executable: Option<PathBuf>,
    pub(crate) xemu_configuration_folder: Option<PathBuf>,
    pub(crate) dolphin_configuration_folder: Option<PathBuf>,
}

impl EmulatorPathOverrides {
    pub(crate) fn load() -> Self {
        let mut overrides = Self::default();
        for emulator in OverridableEmulator::ALL {
            if let Some(path) = load_executable_override(emulator) {
                overrides.set_executable_in_memory(emulator, Some(path));
            }
            if let Some(path) = load_configuration_folder_override(emulator) {
                overrides.set_configuration_folder_in_memory(emulator, Some(path));
            }
        }
        overrides
    }

    pub(crate) fn executable(&self, emulator: OverridableEmulator) -> Option<&Path> {
        match emulator {
            OverridableEmulator::Pcsx2 => self.pcsx2_executable.as_deref(),
            OverridableEmulator::Rpcs3 => self.rpcs3_executable.as_deref(),
            OverridableEmulator::Ppsspp => self.ppsspp_executable.as_deref(),
            OverridableEmulator::DuckStation => self.duckstation_executable.as_deref(),
            OverridableEmulator::Xemu => self.xemu_executable.as_deref(),
            OverridableEmulator::Dolphin => None,
        }
    }

    pub(crate) fn configuration_folder(&self, emulator: OverridableEmulator) -> Option<&Path> {
        match emulator {
            OverridableEmulator::Pcsx2 => None,
            OverridableEmulator::Rpcs3 => self.rpcs3_configuration_folder.as_deref(),
            OverridableEmulator::Ppsspp => self.ppsspp_configuration_folder.as_deref(),
            OverridableEmulator::DuckStation => self.duckstation_configuration_folder.as_deref(),
            OverridableEmulator::Xemu => self.xemu_configuration_folder.as_deref(),
            OverridableEmulator::Dolphin => self.dolphin_configuration_folder.as_deref(),
        }
    }

    fn set_executable_in_memory(&mut self, emulator: OverridableEmulator, value: Option<PathBuf>) {
        match emulator {
            OverridableEmulator::Pcsx2 => self.pcsx2_executable = value,
            OverridableEmulator::Rpcs3 => self.rpcs3_executable = value,
            OverridableEmulator::Ppsspp => self.ppsspp_executable = value,
            OverridableEmulator::DuckStation => self.duckstation_executable = value,
            OverridableEmulator::Xemu => self.xemu_executable = value,
            OverridableEmulator::Dolphin => {}
        }
    }

    fn set_configuration_folder_in_memory(
        &mut self,
        emulator: OverridableEmulator,
        value: Option<PathBuf>,
    ) {
        match emulator {
            OverridableEmulator::Pcsx2 => {}
            OverridableEmulator::Rpcs3 => self.rpcs3_configuration_folder = value,
            OverridableEmulator::Ppsspp => self.ppsspp_configuration_folder = value,
            OverridableEmulator::DuckStation => self.duckstation_configuration_folder = value,
            OverridableEmulator::Xemu => self.xemu_configuration_folder = value,
            OverridableEmulator::Dolphin => self.dolphin_configuration_folder = value,
        }
    }

    /// Persists (or, for `None`, clears) an executable override, updating
    /// both the on-disk file and this in-memory copy. A no-op for an
    /// adapter [`OverridableEmulator::supports_executable_override`] does
    /// not cover.
    pub(crate) fn set_executable(&mut self, emulator: OverridableEmulator, path: Option<PathBuf>) {
        if !emulator.supports_executable_override() {
            return;
        }
        save_executable_override(emulator, path.as_deref());
        self.set_executable_in_memory(emulator, path);
    }

    /// Persists (or, for `None`, clears) a configuration-folder override.
    /// A no-op for an adapter
    /// [`OverridableEmulator::supports_configuration_folder_override`]
    /// does not cover.
    pub(crate) fn set_configuration_folder(
        &mut self,
        emulator: OverridableEmulator,
        path: Option<PathBuf>,
    ) {
        if !emulator.supports_configuration_folder_override() {
            return;
        }
        save_configuration_folder_override(emulator, path.as_deref());
        self.set_configuration_folder_in_memory(emulator, path);
    }

    /// Projects this GUI-only state into the typed core overrides
    /// `DiscoveredProfiles::from_environment_with_overrides` accepts - the
    /// one place these two shapes are translated, so they can never drift.
    pub(crate) fn as_core_overrides(&self) -> DiscoveredProfilesOverrides {
        DiscoveredProfilesOverrides {
            dolphin_configuration_root: self.dolphin_configuration_folder.clone(),
            pcsx2_executable: self.pcsx2_executable.clone(),
            ppsspp_executable: self.ppsspp_executable.clone(),
            ppsspp_configuration_root: self.ppsspp_configuration_folder.clone(),
            duckstation_executable: self.duckstation_executable.clone(),
            duckstation_configuration_root: self.duckstation_configuration_folder.clone(),
            xemu_executable: self.xemu_executable.clone(),
            xemu_configuration_root: self.xemu_configuration_folder.clone(),
            rpcs3_executable: self.rpcs3_executable.clone(),
            rpcs3_configuration_root: self.rpcs3_configuration_folder.clone(),
        }
    }
}

fn load_executable_override(emulator: OverridableEmulator) -> Option<PathBuf> {
    let file_name = emulator.executable_override_file_name()?;
    let path = archivefs_core::app_dirs::config_path(file_name).ok()?;
    load_override_at(&path)
}

fn load_configuration_folder_override(emulator: OverridableEmulator) -> Option<PathBuf> {
    let file_name = emulator.configuration_folder_override_file_name()?;
    let path = archivefs_core::app_dirs::config_path(file_name).ok()?;
    load_override_at(&path)
}

fn save_executable_override(emulator: OverridableEmulator, value: Option<&Path>) {
    let Some(file_name) = emulator.executable_override_file_name() else {
        return;
    };
    if let Ok(path) = archivefs_core::app_dirs::config_path(file_name) {
        save_override_at(&path, value);
    }
}

fn save_configuration_folder_override(emulator: OverridableEmulator, value: Option<&Path>) {
    let Some(file_name) = emulator.configuration_folder_override_file_name() else {
        return;
    };
    if let Ok(path) = archivefs_core::app_dirs::config_path(file_name) {
        save_override_at(&path, value);
    }
}

/// Reads an override from an explicit file path. A missing/unreadable
/// file, or one that is empty or only whitespace, is `None` (automatic
/// detection). The stored path is taken verbatim - it is the user's
/// explicit choice, never canonicalised here. Injectable (`_at`) for tests,
/// exactly like `load_retroarch_core_directory_override_at`.
fn load_override_at(path: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(path).ok()?;
    let trimmed = contents.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// Writes (or, for `None`, removes) the override at an explicit file path.
/// Best-effort: a persistence failure never blocks the in-memory value from
/// taking effect for the session.
fn save_override_at(path: &Path, value: Option<&Path>) {
    match value {
        Some(chosen) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, chosen.to_string_lossy().as_ref());
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Verdict for a file the user just picked as an executable override,
/// decided *before* it is persisted - never saved just because a native
/// dialog returned a path. Mirrors
/// `retroarch_core_setup::classify_picked_core_folder`'s exact shape for
/// folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickedExecutable {
    /// A real, regular file with at least one executable permission bit
    /// set (platforms without that concept accept any regular file).
    Usable,
    /// Missing, a directory, a broken symlink, or otherwise not a real
    /// file.
    NotAFile,
    /// A real file, but without an executable permission bit.
    NotExecutable,
}

pub(crate) fn classify_picked_executable(path: &Path) -> PickedExecutable {
    let Ok(metadata) = std::fs::metadata(path) else {
        return PickedExecutable::NotAFile;
    };
    if !metadata.is_file() {
        return PickedExecutable::NotAFile;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return PickedExecutable::NotExecutable;
        }
    }
    PickedExecutable::Usable
}

/// Verdict for a directory the user just picked as a configuration-folder
/// override. A real directory is the only value that leads to a save +
/// recheck; whether it actually contains a usable configuration is decided
/// later, by the real discovery pass, never here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickedConfigurationFolder {
    Directory,
    Unusable,
}

pub(crate) fn classify_picked_configuration_folder(path: &Path) -> PickedConfigurationFolder {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => PickedConfigurationFolder::Directory,
        _ => PickedConfigurationFolder::Unusable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_emulator_supports_at_least_one_override_kind() {
        for emulator in OverridableEmulator::ALL {
            assert!(
                emulator.supports_executable_override()
                    || emulator.supports_configuration_folder_override(),
                "{:?} supports neither override kind",
                emulator
            );
        }
    }

    #[test]
    fn pcsx2_has_no_configuration_folder_channel() {
        assert!(!OverridableEmulator::Pcsx2.supports_configuration_folder_override());
        assert_eq!(
            OverridableEmulator::Pcsx2.configuration_folder_override_file_name(),
            None
        );
    }

    #[test]
    fn dolphin_has_no_executable_channel() {
        assert!(!OverridableEmulator::Dolphin.supports_executable_override());
        assert_eq!(
            OverridableEmulator::Dolphin.executable_override_file_name(),
            None
        );
    }

    #[test]
    fn adapter_id_round_trips() {
        for emulator in OverridableEmulator::ALL {
            assert_eq!(
                OverridableEmulator::from_adapter_id(emulator.adapter_id()),
                Some(emulator)
            );
        }
        assert_eq!(OverridableEmulator::from_adapter_id("scummvm"), None);
        assert_eq!(OverridableEmulator::from_adapter_id("retroarch"), None);
    }

    #[test]
    fn missing_override_file_is_automatic_detection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pcsx2_executable_override.txt");
        assert_eq!(load_override_at(&path), None);
    }

    #[test]
    fn blank_override_file_is_automatic_detection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pcsx2_executable_override.txt");
        std::fs::write(&path, "   \n").unwrap();
        assert_eq!(load_override_at(&path), None);
    }

    #[test]
    fn saved_override_round_trips_and_reset_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/pcsx2_executable_override.txt");
        let chosen = PathBuf::from("/opt/PCSX2/pcsx2-qt");
        save_override_at(&path, Some(chosen.as_path()));
        assert_eq!(load_override_at(&path), Some(chosen));
        save_override_at(&path, None);
        assert_eq!(load_override_at(&path), None);
        assert!(!path.exists());
    }

    #[test]
    fn classify_picked_executable_rejects_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            classify_picked_executable(dir.path()),
            PickedExecutable::NotAFile
        );
    }

    #[test]
    fn classify_picked_executable_rejects_missing_paths() {
        assert_eq!(
            classify_picked_executable(Path::new("/definitely/not/a/real/path")),
            PickedExecutable::NotAFile
        );
    }

    #[test]
    #[cfg(unix)]
    fn classify_picked_executable_rejects_non_executable_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-executable");
        std::fs::write(&path, b"data").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert_eq!(
            classify_picked_executable(&path),
            PickedExecutable::NotExecutable
        );
    }

    #[test]
    #[cfg(unix)]
    fn classify_picked_executable_accepts_executable_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runnable");
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert_eq!(classify_picked_executable(&path), PickedExecutable::Usable);
    }

    #[test]
    fn classify_picked_configuration_folder_requires_a_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            classify_picked_configuration_folder(dir.path()),
            PickedConfigurationFolder::Directory
        );
        let file_path = dir.path().join("file.txt");
        std::fs::write(&file_path, b"x").unwrap();
        assert_eq!(
            classify_picked_configuration_folder(&file_path),
            PickedConfigurationFolder::Unusable
        );
        assert_eq!(
            classify_picked_configuration_folder(Path::new("/definitely/not/a/real/path")),
            PickedConfigurationFolder::Unusable
        );
    }

    #[test]
    fn as_core_overrides_only_carries_supported_fields() {
        let mut overrides = EmulatorPathOverrides::default();
        overrides.set_executable_in_memory(
            OverridableEmulator::Pcsx2,
            Some(PathBuf::from("/opt/pcsx2-qt")),
        );
        overrides.set_configuration_folder_in_memory(
            OverridableEmulator::Dolphin,
            Some(PathBuf::from("/home/user/.dolphin-emu")),
        );
        let core = overrides.as_core_overrides();
        assert_eq!(core.pcsx2_executable, Some(PathBuf::from("/opt/pcsx2-qt")));
        assert_eq!(
            core.dolphin_configuration_root,
            Some(PathBuf::from("/home/user/.dolphin-emu"))
        );
        assert_eq!(core.rpcs3_executable, None);
    }
}
