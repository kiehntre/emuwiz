//! DuckStation facts for the profile resolver. Read-only.
//!
//! Pairing follows DuckStation's own `Core::SetDataRoot()` rule (documented
//! beside the native launch binding): `portable.txt` or `settings.ini` beside
//! the executable makes that directory the profile; otherwise the per-user
//! default; a Flatpak keeps its own sandbox. An executable is therefore only
//! ever paired with the profile it would really use.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::EmulatorProfileAdapter;
use super::desktop::{default_desktop_directories, launcher_targets};
use super::fsfacts::{
    directory_has_entries, directory_has_extension, executable_problem, is_directory,
    is_regular_file,
};
use super::model::*;
use crate::emulator_inventory::InventoryEmulator;
use crate::launch::installation::LaunchInstallation;
use crate::patch_manager::duckstation_local::{
    DuckStationExecutable, DuckStationInstallationType, DuckStationProfileDiscoveryRoots,
    discover_duckstation_executables, discover_duckstation_profiles,
    expected_duckstation_native_root,
};
use crate::patch_manager::duckstation_native::{ini_value, resolve_duckstation_folders};

const FLATPAK_APP_ID: &str = "org.duckstation.DuckStation";
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const MAX_PLAYTIME_BYTES: u64 = 64 * 1024;

pub struct DuckStationAdapter {
    pub roots: DuckStationProfileDiscoveryRoots,
    pub desktop_directories: Vec<PathBuf>,
}

impl DuckStationAdapter {
    pub fn new(roots: DuckStationProfileDiscoveryRoots, desktop_directories: Vec<PathBuf>) -> Self {
        Self {
            roots,
            desktop_directories,
        }
    }

    pub fn from_environment() -> Result<Self, crate::patch_manager::DuckStationDiscoveryError> {
        let roots = DuckStationProfileDiscoveryRoots::from_environment()?;
        let desktop = default_desktop_directories(&roots.home, &roots.xdg_data_home);
        Ok(Self::new(roots, desktop))
    }

    fn flatpak_root(&self) -> PathBuf {
        let base = self.roots.home.join(".var/app").join(FLATPAK_APP_ID);
        let data = base.join("data/duckstation");
        if is_regular_file(&data.join("settings.ini")) {
            data
        } else {
            base.join("config/duckstation")
        }
    }

    /// The profile this executable would really use, and how.
    fn pairing(&self, executable: &DuckStationExecutable) -> (PathBuf, ProfileLayout) {
        if executable.installation_type == DuckStationInstallationType::FlatpakUser {
            return (self.flatpak_root(), ProfileLayout::FlatpakSandbox);
        }
        if let Some(directory) = executable.path.parent()
            && (is_regular_file(&directory.join("portable.txt"))
                || is_regular_file(&directory.join("settings.ini")))
        {
            return (directory.to_path_buf(), ProfileLayout::Portable);
        }
        (
            expected_duckstation_native_root(&self.roots),
            ProfileLayout::DefaultUser,
        )
    }

    fn layout_for_named_root(&self, root: &Path) -> ProfileLayout {
        if root.starts_with(self.roots.home.join(".var/app").join(FLATPAK_APP_ID)) {
            ProfileLayout::FlatpakSandbox
        } else if root == expected_duckstation_native_root(&self.roots) {
            ProfileLayout::DefaultUser
        } else if is_regular_file(&root.join("portable.txt")) {
            ProfileLayout::Portable
        } else {
            ProfileLayout::Explicit
        }
    }

    fn build(
        &self,
        executable: Option<&DuckStationExecutable>,
        root: PathBuf,
        layout: ProfileLayout,
        launchers: &[PathBuf],
    ) -> EmulatorProfileCandidate {
        let mut evidence = Vec::new();
        let mut unusable = Vec::new();
        let mut warnings = Vec::new();

        // --- executable half of the pair
        let installation = executable
            .map(|found| found.launch.clone())
            .unwrap_or_default();
        match executable {
            Some(found) => {
                if let Some(problem) = executable_problem(&found.path) {
                    unusable.push(problem);
                }
            }
            None if layout == ProfileLayout::FlatpakSandbox => {
                unusable.push(UnusableReason::FlatpakNotInstalled);
                evidence.push(SelectionEvidence::disqualifies(
                    EvidenceKind::StaleProfileData,
                    "Flatpak profile data exists but the Flatpak is not installed",
                ));
            }
            None => {
                unusable.push(UnusableReason::ExecutableMissing);
                evidence.push(SelectionEvidence::disqualifies(
                    EvidenceKind::StaleProfileData,
                    "profile data exists but no installed executable uses it",
                ));
            }
        }

        // --- profile half of the pair
        let main_config = root.join("settings.ini");
        let directory_present = is_directory(&root);
        if !directory_present {
            unusable.push(if fs::symlink_metadata(&root).is_ok() {
                UnusableReason::ProfileDirectoryNotADirectory
            } else {
                UnusableReason::ProfileDirectoryMissing
            });
        }
        let config_present = is_regular_file(&main_config);
        if directory_present && !config_present {
            unusable.push(UnusableReason::MainConfigMissing);
        }
        let settings_text = if config_present {
            read_bounded(&main_config, MAX_SETTINGS_BYTES)
        } else {
            None
        };
        if config_present && settings_text.is_none() {
            unusable.push(UnusableReason::MainConfigUnreadable);
            warnings.push(ConfigurationWarning::SettingsUnreadable {
                reason: "settings.ini is too large or not readable text".into(),
            });
        }
        let ini = |section: &str, key: &str, warnings: &mut Vec<ConfigurationWarning>| {
            settings_text
                .as_deref()
                .and_then(|text| match ini_value(text, section, key) {
                    Ok(value) => value,
                    Err(reason) => {
                        warnings.push(ConfigurationWarning::SettingsUnreadable { reason });
                        None
                    }
                })
        };

        // --- setup state
        let setup = if !config_present {
            SetupState::NotRecorded
        } else {
            match ini("Main", "SetupWizardIncomplete", &mut warnings)
                .map(|value| value.trim().to_ascii_lowercase())
                .as_deref()
            {
                Some("false" | "0" | "no" | "off") => SetupState::Complete,
                Some("true" | "1" | "yes" | "on") => SetupState::Incomplete,
                _ => SetupState::NotRecorded,
            }
        };
        match setup {
            SetupState::Complete => evidence.push(SelectionEvidence::supports(
                EvidenceKind::SetupComplete,
                "settings.ini records SetupWizardIncomplete = false",
            )),
            SetupState::Incomplete => {
                evidence.push(SelectionEvidence::against(
                    EvidenceKind::SetupIncomplete,
                    "settings.ini records SetupWizardIncomplete = true; DuckStation would show its setup wizard",
                ));
                warnings.push(ConfigurationWarning::SetupWizardIncomplete);
            }
            _ => evidence.push(SelectionEvidence::against(
                EvidenceKind::SetupNotRecorded,
                "nothing records that setup was completed",
            )),
        }

        // --- folders (read-only; [Folders] overrides respected)
        let mut folders = BTreeMap::new();
        let mut details_folders = None;
        if directory_present {
            match resolve_duckstation_folders(&root) {
                Ok(resolved) => {
                    for (folder, path, custom) in [
                        (
                            ProfileFolder::Cheats,
                            &resolved.cheats,
                            resolved.cheats_custom,
                        ),
                        (
                            ProfileFolder::GameSettings,
                            &resolved.game_settings,
                            resolved.game_settings_custom,
                        ),
                    ] {
                        let exists = is_directory(path);
                        if custom && !exists {
                            warnings.push(ConfigurationWarning::CustomFolderMissing {
                                folder,
                                path: path.clone(),
                            });
                        }
                        folders.insert(
                            folder,
                            ResolvedFolder {
                                path: path.clone(),
                                exists,
                                custom,
                            },
                        );
                    }
                    details_folders = Some(resolved);
                }
                Err(refusal) => warnings.push(ConfigurationWarning::FolderUnsafe {
                    folder: ProfileFolder::Cheats,
                    reason: format!("{refusal:?}"),
                }),
            }
        }
        for (folder, name) in [
            (ProfileFolder::Patches, "patches"),
            (ProfileFolder::Textures, "textures"),
            (ProfileFolder::MemoryCards, "memcards"),
            (ProfileFolder::SaveStates, "savestates"),
        ] {
            let path = root.join(name);
            folders.insert(
                folder,
                ResolvedFolder {
                    exists: is_directory(&path),
                    path,
                    custom: false,
                },
            );
        }

        // --- BIOS search directory (relative values resolve against the data
        // root, never the process working directory)
        let bios_setting = ini("BIOS", "SearchDirectory", &mut warnings)
            .or_else(|| ini("BIOS", "BIOSDirectory", &mut warnings))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let bios_dir = match bios_setting.as_deref() {
            Some(value) if Path::new(value).is_absolute() => PathBuf::from(value),
            Some(value) => root.join(value),
            None => root.join("bios"),
        };
        let bios_exists = is_directory(&bios_dir);
        folders.insert(
            ProfileFolder::BiosSearch,
            ResolvedFolder {
                path: bios_dir.clone(),
                exists: bios_exists,
                custom: bios_setting.is_some(),
            },
        );
        if bios_exists && directory_has_entries(&bios_dir) {
            evidence.push(SelectionEvidence::supports(
                EvidenceKind::ValidBiosConfiguration,
                format!(
                    "BIOS directory {} exists and is not empty",
                    bios_dir.display()
                ),
            ));
        } else if !bios_exists && config_present {
            evidence.push(SelectionEvidence::against(
                EvidenceKind::BiosLocationBroken,
                format!("BIOS directory {} does not exist", bios_dir.display()),
            ));
            warnings.push(ConfigurationWarning::BiosDirectoryMissing {
                configured: bios_setting.clone().unwrap_or_else(|| "bios".into()),
                resolved: bios_dir.clone(),
            });
        }

        // --- usage and per-game state
        let playtime = read_bounded(&root.join("playtime.dat"), MAX_PLAYTIME_BYTES)
            .is_some_and(|text| text.lines().any(|line| !line.trim().is_empty()));
        let used = playtime
            || directory_has_entries(&root.join("memcards"))
            || directory_has_entries(&root.join("savestates"));
        evidence.push(if used {
            SelectionEvidence::supports(
                EvidenceKind::UseHistory,
                "play history, memory cards or save states exist",
            )
        } else {
            SelectionEvidence::against(
                EvidenceKind::NoUseHistory,
                "no play history, memory cards or save states: apparently unused",
            )
        });
        let game_settings = folders
            .get(&ProfileFolder::GameSettings)
            .map(|folder| folder.path.clone())
            .unwrap_or_else(|| root.join("gamesettings"));
        if directory_has_extension(&game_settings, "ini") {
            evidence.push(SelectionEvidence::supports(
                EvidenceKind::GameSpecificConfig,
                "per-game settings exist",
            ));
        }

        // --- relationships
        let portable_marker = executable
            .and_then(|found| found.path.parent())
            .map(|directory| directory.join("portable.txt"))
            .filter(|marker| is_regular_file(marker) && marker.parent() == Some(root.as_path()));
        if let Some(marker) = &portable_marker {
            evidence.push(SelectionEvidence::supports(
                EvidenceKind::PortableMarker,
                format!("{} sits beside the executable", marker.display()),
            ));
        }
        if let Some(found) = executable {
            let (expected_root, _) = self.pairing(found);
            if expected_root == root {
                evidence.push(SelectionEvidence::supports(
                    EvidenceKind::PairingProven,
                    "this executable uses this profile by DuckStation's own data-root rule",
                ));
            }
            if launchers.contains(&found.path) {
                evidence.push(SelectionEvidence::supports(
                    EvidenceKind::DesktopLauncher,
                    "a desktop launcher starts this executable",
                ));
            }
            if self.roots.explicit_executables.contains(&found.path)
                || self.roots.explicit_configuration_roots.contains(&root)
            {
                evidence.push(SelectionEvidence::supports(
                    EvidenceKind::UserOverrideNamesIt,
                    "named by an existing EmuWiz override",
                ));
            }
        } else if self.roots.explicit_configuration_roots.contains(&root) {
            evidence.push(SelectionEvidence::neutral(
                EvidenceKind::UserOverrideNamesIt,
                "named by an existing EmuWiz override, but nothing installed uses it",
            ));
        }
        for reason in &unusable {
            evidence.push(SelectionEvidence::disqualifies(
                EvidenceKind::Unusable,
                format!("{reason:?}"),
            ));
        }
        unusable.sort();
        unusable.dedup();

        EmulatorProfileCandidate {
            identity: CandidateIdentity {
                emulator: InventoryEmulator::DuckStation,
                executable: executable.map(|found| found.path.clone()),
                profile_root: root,
            },
            installation,
            layout,
            main_config,
            folders,
            readiness: ProfileReadiness {
                profile_directory_present: directory_present,
                main_config_present: config_present,
                setup,
            },
            evidence,
            unusable,
            warnings,
            details: Some(EmulatorDetails::DuckStation(DuckStationDetails {
                folders: details_folders,
                bios_search_directory_setting: bios_setting,
                portable_marker,
            })),
        }
    }
}

impl EmulatorProfileAdapter for DuckStationAdapter {
    fn emulator(&self) -> InventoryEmulator {
        InventoryEmulator::DuckStation
    }

    fn discover(&self) -> Vec<EmulatorProfileCandidate> {
        let launchers = launcher_targets(&self.desktop_directories, "duckstation");
        let mut candidates = Vec::new();
        for executable in discover_duckstation_executables(&self.roots) {
            let (root, layout) = self.pairing(&executable);
            candidates.push(self.build(Some(&executable), root, layout, &launchers));
        }
        // Profile data no executable pairs with (stale Flatpak data, an old
        // directory, an explicit root): kept so it can be reported, never
        // selected.
        for profile in discover_duckstation_profiles(&self.roots).profiles {
            let root = profile.configuration_path;
            if candidates
                .iter()
                .any(|candidate| candidate.identity.profile_root == root)
            {
                continue;
            }
            let layout = self.layout_for_named_root(&root);
            candidates.push(self.build(None, root, layout, &launchers));
        }
        candidates
    }

    fn assess(&self, identity: &CandidateIdentity) -> EmulatorProfileCandidate {
        let launchers = launcher_targets(&self.desktop_directories, "duckstation");
        let Some(path) = &identity.executable else {
            let layout = self.layout_for_named_root(&identity.profile_root);
            return self.build(None, identity.profile_root.clone(), layout, &launchers);
        };
        let known = discover_duckstation_executables(&self.roots)
            .into_iter()
            .find(|found| &found.path == path);
        let executable = known.unwrap_or_else(|| {
            let appimage = path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("AppImage"));
            DuckStationExecutable {
                path: path.clone(),
                installation_type: DuckStationInstallationType::Explicit,
                launch: if appimage {
                    LaunchInstallation::AppImage {
                        extract_and_run: false,
                    }
                } else {
                    LaunchInstallation::Native
                },
                version: None,
            }
        });
        let (expected_root, _) = self.pairing(&executable);
        let layout = self.layout_for_named_root(&identity.profile_root);
        let mut candidate = self.build(
            Some(&executable),
            identity.profile_root.clone(),
            layout,
            &launchers,
        );
        if expected_root != identity.profile_root {
            candidate.unusable.push(UnusableReason::IncompatiblePairing);
            candidate.unusable.sort();
            candidate.unusable.dedup();
            candidate.evidence.push(SelectionEvidence::disqualifies(
                EvidenceKind::PinnedIdentityMismatch,
                format!(
                    "this executable would use {}, not {}",
                    expected_root.display(),
                    identity.profile_root.display()
                ),
            ));
        }
        candidate
    }
}

fn read_bounded(path: &Path, limit: u64) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(limit)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}
