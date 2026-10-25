//! PPSSPP facts for the profile resolver - the second adapter, proving the
//! resolver is generic. It adapts PPSSPP's existing discovery and its
//! existing executable/profile pairing rule; PPSSPP's configuration
//! semantics are deliberately not forced into DuckStation's.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::EmulatorProfileAdapter;
use super::desktop::{default_desktop_directories, launcher_targets};
use super::fsfacts::{directory_has_entries, executable_problem, is_directory, is_regular_file};
use super::model::*;
use crate::emulator_inventory::InventoryEmulator;
use crate::patch_manager::ppsspp_local::{
    PpssppExecutable, PpssppInstallationType, PpssppProfile, PpssppProfileBlockerKind,
    PpssppProfileDiscoveryRoots, discover_ppsspp_profiles,
};

pub struct PpssppAdapter {
    pub roots: PpssppProfileDiscoveryRoots,
    pub desktop_directories: Vec<PathBuf>,
}

impl PpssppAdapter {
    pub fn new(roots: PpssppProfileDiscoveryRoots, desktop_directories: Vec<PathBuf>) -> Self {
        Self {
            roots,
            desktop_directories,
        }
    }

    pub fn from_environment() -> Result<Self, crate::patch_manager::PpssppDiscoveryError> {
        let roots = PpssppProfileDiscoveryRoots::from_environment()?;
        let desktop = default_desktop_directories(&roots.home, &roots.xdg_data_home);
        Ok(Self::new(roots, desktop))
    }
}

/// PPSSPP's existing pairing rule (see its launch binding): which executable
/// provenances may serve a profile of this installation type.
fn acceptable(profile: PpssppInstallationType) -> &'static [PpssppInstallationType] {
    match profile {
        PpssppInstallationType::Native => &[
            PpssppInstallationType::Native,
            PpssppInstallationType::Explicit,
        ],
        PpssppInstallationType::Explicit => &[PpssppInstallationType::Explicit],
        PpssppInstallationType::FlatpakUser => &[PpssppInstallationType::FlatpakUser],
        PpssppInstallationType::Portable => &[PpssppInstallationType::Portable],
    }
}

fn layout_of(profile: PpssppInstallationType) -> ProfileLayout {
    match profile {
        PpssppInstallationType::Native => ProfileLayout::DefaultUser,
        PpssppInstallationType::FlatpakUser => ProfileLayout::FlatpakSandbox,
        PpssppInstallationType::Portable => ProfileLayout::Portable,
        PpssppInstallationType::Explicit => ProfileLayout::Explicit,
    }
}

impl PpssppAdapter {
    fn build(
        &self,
        profile: &PpssppProfile,
        executable: Option<&PpssppExecutable>,
        launchers: &[PathBuf],
    ) -> EmulatorProfileCandidate {
        let root = profile.configuration_path.clone();
        let mut evidence = Vec::new();
        let mut unusable = Vec::new();

        match executable {
            Some(found) => unusable.extend(executable_problem(&found.path)),
            None if profile.installation_type == PpssppInstallationType::FlatpakUser => {
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
        for blocker in &profile.blockers {
            unusable.push(match blocker.kind {
                PpssppProfileBlockerKind::MissingConfiguration => {
                    UnusableReason::ProfileDirectoryMissing
                }
                PpssppProfileBlockerKind::MissingPpssppEvidence => {
                    UnusableReason::MainConfigMissing
                }
                PpssppProfileBlockerKind::NotDirectory => {
                    UnusableReason::ProfileDirectoryNotADirectory
                }
                _ => UnusableReason::MainConfigUnreadable,
            });
        }

        let config_present = is_regular_file(&profile.global_config_path);
        evidence.push(SelectionEvidence::neutral(
            EvidenceKind::PairingProven,
            "PPSSPP's own pairing rule allows this executable for this profile",
        ));
        let used = directory_has_entries(&profile.savedata_path)
            || directory_has_entries(&profile.state_path);
        evidence.push(if used {
            SelectionEvidence::supports(EvidenceKind::UseHistory, "save data or save states exist")
        } else {
            SelectionEvidence::against(
                EvidenceKind::NoUseHistory,
                "no save data or save states: apparently unused",
            )
        });
        if let Some(found) = executable {
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
        }
        for reason in &unusable {
            evidence.push(SelectionEvidence::disqualifies(
                EvidenceKind::Unusable,
                format!("{reason:?}"),
            ));
        }
        unusable.sort();
        unusable.dedup();

        let mut folders = BTreeMap::new();
        for (folder, path) in [
            (ProfileFolder::Cheats, &profile.cheats_path),
            (ProfileFolder::Textures, &profile.textures_path),
            (ProfileFolder::SaveData, &profile.savedata_path),
            (ProfileFolder::SaveStates, &profile.state_path),
            (ProfileFolder::GameData, &profile.game_path),
            (ProfileFolder::System, &profile.system_path),
        ] {
            folders.insert(
                folder,
                ResolvedFolder {
                    exists: is_directory(path),
                    path: path.clone(),
                    custom: false,
                },
            );
        }

        EmulatorProfileCandidate {
            identity: CandidateIdentity {
                emulator: InventoryEmulator::Ppsspp,
                executable: executable.map(|found| found.path.clone()),
                profile_root: root,
            },
            installation: executable
                .map(|found| found.launch.clone())
                .unwrap_or_default(),
            layout: layout_of(profile.installation_type),
            main_config: profile.global_config_path.clone(),
            folders,
            readiness: ProfileReadiness {
                profile_directory_present: is_directory(&profile.configuration_path),
                main_config_present: config_present,
                setup: SetupState::NotApplicable,
            },
            evidence,
            unusable,
            warnings: Vec::new(),
            details: Some(EmulatorDetails::Ppsspp(PpssppDetails {
                memstick: profile.memstick_path.clone(),
            })),
        }
    }
}

impl EmulatorProfileAdapter for PpssppAdapter {
    fn emulator(&self) -> InventoryEmulator {
        InventoryEmulator::Ppsspp
    }

    fn discover(&self) -> Vec<EmulatorProfileCandidate> {
        let launchers = launcher_targets(&self.desktop_directories, "ppsspp");
        let mut candidates = Vec::new();
        for profile in discover_ppsspp_profiles(&self.roots).profiles {
            let matching: Vec<&PpssppExecutable> = profile
                .executable_candidates
                .iter()
                .filter(|found| {
                    acceptable(profile.installation_type).contains(&found.installation_type)
                })
                .collect();
            if matching.is_empty() {
                candidates.push(self.build(&profile, None, &launchers));
            }
            for found in matching {
                candidates.push(self.build(&profile, Some(found), &launchers));
            }
        }
        candidates
    }

    fn assess(&self, identity: &CandidateIdentity) -> EmulatorProfileCandidate {
        let launchers = launcher_targets(&self.desktop_directories, "ppsspp");
        let mut roots = self.roots.clone();
        if !roots
            .explicit_configuration_roots
            .contains(&identity.profile_root)
        {
            roots
                .explicit_configuration_roots
                .push(identity.profile_root.clone());
        }
        let discovery = discover_ppsspp_profiles(&roots);
        let Some(profile) = discovery
            .profiles
            .iter()
            .find(|profile| profile.configuration_path == identity.profile_root)
        else {
            // Not inspectable (relative path, or past the profile limit).
            return EmulatorProfileCandidate {
                identity: identity.clone(),
                installation: Default::default(),
                layout: ProfileLayout::Explicit,
                main_config: identity.profile_root.join("PSP/SYSTEM/ppsspp.ini"),
                folders: BTreeMap::new(),
                readiness: ProfileReadiness {
                    profile_directory_present: false,
                    main_config_present: false,
                    setup: SetupState::NotApplicable,
                },
                evidence: Vec::new(),
                unusable: vec![UnusableReason::ProfileDirectoryMissing],
                warnings: Vec::new(),
                details: None,
            };
        };
        match &identity.executable {
            None => self.build(profile, None, &launchers),
            Some(path) => {
                let allowed = profile
                    .executable_candidates
                    .iter()
                    .find(|found| &found.path == path)
                    .filter(|found| {
                        acceptable(profile.installation_type).contains(&found.installation_type)
                    });
                match allowed {
                    Some(found) => self.build(profile, Some(found), &launchers),
                    None => {
                        let mut candidate = self.build(
                            profile,
                            Some(&PpssppExecutable {
                                path: path.clone(),
                                installation_type: PpssppInstallationType::Explicit,
                                launch: Default::default(),
                                version: None,
                            }),
                            &launchers,
                        );
                        candidate.unusable.push(UnusableReason::IncompatiblePairing);
                        candidate.unusable.sort();
                        candidate.unusable.dedup();
                        candidate
                    }
                }
            }
        }
    }
}
