//! shadPS4 facts for the shared AUTO/PREFERRED/FORCED resolver. Selection
//! remains entirely in the generic engine; the launch adapter owns safety.
use super::{EmulatorProfileAdapter, model::*};
use crate::emulator_inventory::InventoryEmulator;
use crate::launch::{
    installation::LaunchInstallation,
    installation_known::KnownInstallRoots,
    shadps4_profile::{
        ShadPs4DiscoveryCandidate, ShadPs4Profile, ShadPs4Refusal, ShadPs4RefusalKind,
        discover_shadps4, inspect_shadps4_profile,
    },
};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

pub struct ShadPs4Adapter {
    pub roots: KnownInstallRoots,
    pub configured: Vec<PathBuf>,
    pub path: Option<OsString>,
    /// Explicit consent for this exact AppImage/profile pair. Supplies facts,
    /// never overrides generic ranking, ambiguity or forced-refusal policy.
    pub trusted_appimage: Option<CandidateIdentity>,
}
impl ShadPs4Adapter {
    /// Bind exactly the resolver's chosen executable/profile pair. A pin or
    /// preference is explicit AppImage selection, never publisher verification.
    pub fn bind(
        &self,
        identity: &CandidateIdentity,
        explicitly_selected: bool,
    ) -> Result<ShadPs4Profile, ShadPs4Refusal> {
        let error = || ShadPs4Refusal {
            kind: ShadPs4RefusalKind::ConfigurationUnavailable,
            detail: "resolver pair is not the profile this shadPS4 executable uses".into(),
        };
        if identity.emulator != InventoryEmulator::ShadPs4 {
            return Err(error());
        }
        let executable = identity.executable.as_ref().ok_or_else(error)?;
        let profile = inspect_shadps4_profile(
            &ShadPs4DiscoveryCandidate {
                executable: executable.clone(),
                user_selected: explicitly_selected || self.configured.contains(executable),
            },
            executable.parent().ok_or_else(error)?,
            &self.roots.user_data,
        )?;
        if profile.user_directory != identity.profile_root {
            return Err(error());
        }
        Ok(profile)
    }
    fn facts(&self, identity: CandidateIdentity, explicit: bool) -> EmulatorProfileCandidate {
        let inspected = self.bind(
            &identity,
            explicit || self.trusted_appimage.as_ref() == Some(&identity),
        );
        let mut evidence = Vec::new();
        let mut warnings = Vec::new();
        let mut unusable = Vec::new();
        let root = identity.profile_root.clone();
        let mut config = root.join("config.json");
        let mut installation = LaunchInstallation::Native;
        let mut layout = ProfileLayout::DefaultUser;
        let mut folders = BTreeMap::new();
        match inspected {
            Ok(profile) => {
                installation = profile.installation;
                config = profile.config_path;
                if root == profile.working_directory.join("user") {
                    layout = ProfileLayout::Portable;
                    evidence.push(SelectionEvidence::supports(
                        EvidenceKind::PortableMarker,
                        "existing portable user directory owns this executable/profile pair",
                    ));
                }
                evidence.push(SelectionEvidence::neutral(
                    EvidenceKind::PairingProven,
                    "shadPS4 user-path precedence and read-only adapter inspection agree",
                ));
                folders.insert(
                    ProfileFolder::System,
                    ResolvedFolder {
                        exists: profile.sysmodules == crate::launch::shadps4_profile::ShadPs4SysmoduleState::PresentUnverified,
                        path: profile.settings.sysmodules_directory,
                        custom: false,
                    },
                );
            }
            Err(error) => {
                unusable.push(match error.kind {
                    ShadPs4RefusalKind::ExecutableMissing => UnusableReason::ExecutableMissing,
                    ShadPs4RefusalKind::ExecutableNotRunnable => {
                        UnusableReason::ExecutableNotExecutable
                    }
                    _ => UnusableReason::MainConfigUnreadable,
                });
                warnings.push(ConfigurationWarning::SettingsUnreadable {
                    reason: format!("{:?}: {}", error.kind, error.detail),
                });
                evidence.push(SelectionEvidence::disqualifies(
                    EvidenceKind::Unusable,
                    format!("{:?}: {}", error.kind, error.detail),
                ));
            }
        }
        if identity
            .executable
            .as_ref()
            .is_some_and(|p| self.configured.contains(p))
        {
            evidence.push(SelectionEvidence::supports(
                EvidenceKind::UserOverrideNamesIt,
                "explicit executable is discovery evidence; resolver mode still owns selection",
            ));
        }
        EmulatorProfileCandidate {
            identity,
            installation,
            layout,
            main_config: config.clone(),
            readiness: ProfileReadiness {
                profile_directory_present: root.is_dir(),
                main_config_present: config.is_file(),
                setup: SetupState::NotRecorded,
            },
            folders,
            evidence,
            unusable,
            warnings,
            details: None,
        }
    }
}
impl EmulatorProfileAdapter for ShadPs4Adapter {
    fn emulator(&self) -> InventoryEmulator {
        InventoryEmulator::ShadPs4
    }
    fn discover(&self) -> Vec<EmulatorProfileCandidate> {
        let mut found = discover_shadps4(&self.roots, &self.configured, self.path.as_deref());
        // Missing or unusable configured files still need an honest assessment.
        for path in self.configured.iter().take(16) {
            if !found.iter().any(|candidate| &candidate.executable == path) {
                found.push(ShadPs4DiscoveryCandidate {
                    executable: path.clone(),
                    user_selected: true,
                });
            }
        }
        found
            .into_iter()
            .map(|candidate| {
                let portable = candidate
                    .executable
                    .parent()
                    .unwrap_or(&candidate.executable)
                    .join("user");
                let root = if std::fs::symlink_metadata(&portable).is_ok() {
                    portable
                } else {
                    self.roots.user_data.join("shadPS4")
                };
                self.facts(
                    CandidateIdentity {
                        emulator: self.emulator(),
                        executable: Some(candidate.executable),
                        profile_root: root,
                    },
                    candidate.user_selected,
                )
            })
            .collect()
    }
    fn assess(&self, identity: &CandidateIdentity) -> EmulatorProfileCandidate {
        self.facts(identity.clone(), true)
    }
}
