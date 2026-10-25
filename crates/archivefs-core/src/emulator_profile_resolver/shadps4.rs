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

enum BindError {
    /// The resolver's executable/profile pair is not one shadPS4 would use.
    Pairing,
    Refused(ShadPs4Refusal),
}

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
        self.bind_checked(identity, explicitly_selected)
            .map_err(|error| match error {
                BindError::Pairing => ShadPs4Refusal {
                    kind: ShadPs4RefusalKind::ConfigurationUnavailable,
                    detail: "resolver pair is not the profile this shadPS4 executable uses".into(),
                },
                BindError::Refused(refusal) => refusal,
            })
    }
    /// [`Self::bind`], keeping a pairing mismatch distinct from an adapter
    /// refusal so the resolver can name the right unusable reason.
    fn bind_checked(
        &self,
        identity: &CandidateIdentity,
        explicitly_selected: bool,
    ) -> Result<ShadPs4Profile, BindError> {
        if identity.emulator != InventoryEmulator::ShadPs4 {
            return Err(BindError::Pairing);
        }
        let executable = identity.executable.as_ref().ok_or(BindError::Pairing)?;
        let profile = inspect_shadps4_profile(
            &ShadPs4DiscoveryCandidate {
                executable: executable.clone(),
                user_selected: explicitly_selected || self.configured.contains(executable),
            },
            executable.parent().ok_or(BindError::Pairing)?,
            &self.roots.user_data,
        )
        .map_err(BindError::Refused)?;
        if profile.user_directory != identity.profile_root {
            return Err(BindError::Pairing);
        }
        Ok(profile)
    }
    fn facts(&self, identity: CandidateIdentity, explicit: bool) -> EmulatorProfileCandidate {
        let inspected = self.bind_checked(
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
            Err(BindError::Pairing) => {
                unusable.push(UnusableReason::IncompatiblePairing);
                evidence.push(SelectionEvidence::disqualifies(
                    EvidenceKind::Unusable,
                    "this shadPS4 executable would use a different user directory",
                ));
            }
            Err(BindError::Refused(error)) => {
                unusable.push(match error.kind {
                    ShadPs4RefusalKind::ExecutableMissing => UnusableReason::ExecutableMissing,
                    ShadPs4RefusalKind::ExecutableNotRunnable => {
                        UnusableReason::ExecutableNotExecutable
                    }
                    ShadPs4RefusalKind::ExecutableIdentityUnconfirmed => {
                        UnusableReason::ExecutableIdentityUnconfirmed
                    }
                    // Profile refusals are named by what is actually on disk.
                    _ if !root.exists() => UnusableReason::ProfileDirectoryMissing,
                    _ if !root.is_dir() => UnusableReason::ProfileDirectoryNotADirectory,
                    _ if !config.exists() => UnusableReason::MainConfigMissing,
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
        // The fixed native location the installation diagnostics also report
        // (diagnostics::profiles). Recognition still requires the CLI marker;
        // the Qt launcher AppImage beside it is not a core executable.
        let native = self.roots.home.join("Applications/shadPS4/shadps4");
        if std::fs::symlink_metadata(&native).is_ok()
            && !found.iter().any(|candidate| candidate.executable == native)
        {
            found.push(ShadPs4DiscoveryCandidate {
                executable: native,
                user_selected: false,
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_profile_resolver::resolve_emulator_profile;
    use std::os::unix::fs::PermissionsExt;

    /// A synthetic native executable carrying the shadPS4 CLI marker, or an
    /// AppImage-flagged one whose identity the file cannot confirm.
    fn host_elf(appimage: bool) -> Vec<u8> {
        let mut bytes = vec![0u8; 180];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        if appimage {
            bytes[8..11].copy_from_slice(b"AI\x02");
        }
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x400078u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
        bytes[68..72].copy_from_slice(&5u32.to_le_bytes());
        bytes[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
        bytes[96..104].copy_from_slice(&180u64.to_le_bytes());
        bytes[104..112].copy_from_slice(&180u64.to_le_bytes());
        bytes[112..120].copy_from_slice(&4096u64.to_le_bytes());
        bytes[120..132].copy_from_slice(&[0xb8, 60, 0, 0, 0, 0xbf, 7, 0, 0, 0, 0x0f, 0x05]);
        bytes[140..160].copy_from_slice(b"shadPS4 Emulator CLI");
        bytes
    }

    struct Lab {
        dir: tempfile::TempDir,
    }
    impl Lab {
        fn new() -> Self {
            let lab = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            std::fs::create_dir_all(lab.data().join("shadPS4")).unwrap();
            std::fs::write(lab.data().join("shadPS4/config.json"), "{}").unwrap();
            lab
        }
        fn data(&self) -> PathBuf {
            self.dir.path().join("data")
        }
        fn executable(&self, relative: &str, appimage: bool) -> PathBuf {
            let path = self.dir.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, host_elf(appimage)).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
        fn adapter(&self, configured: Vec<PathBuf>) -> ShadPs4Adapter {
            ShadPs4Adapter {
                roots: KnownInstallRoots {
                    home: self.dir.path().into(),
                    user_data: self.data(),
                    system_data: self.dir.path().join("system"),
                    path_dirs: vec![],
                },
                configured,
                path: None,
                trusted_appimage: None,
            }
        }
    }

    #[test]
    fn the_native_location_diagnostics_report_is_discovered() {
        let lab = Lab::new();
        let native = lab.executable("Applications/shadPS4/shadps4", false);
        let candidates = lab.adapter(vec![]).discover();
        let found = candidates
            .iter()
            .find(|c| c.identity.executable.as_ref() == Some(&native))
            .expect("the native location is a candidate");
        assert!(found.unusable.is_empty(), "{:?}", found.unusable);
        assert!(matches!(
            resolve_emulator_profile(&lab.adapter(vec![]), &EmulatorSelectionMode::Auto),
            ResolutionOutcome::Resolved(_)
        ));
    }

    #[test]
    fn a_pairing_mismatch_and_an_unconfirmed_appimage_name_their_real_reasons() {
        let lab = Lab::new();
        let exe = lab.executable("bin/shadps4", false);
        let adapter = lab.adapter(vec![exe.clone()]);
        let wrong_root = adapter.assess(&CandidateIdentity {
            emulator: InventoryEmulator::ShadPs4,
            executable: Some(exe.clone()),
            profile_root: lab.dir.path().join("somewhere-else"),
        });
        assert_eq!(
            wrong_root.unusable,
            vec![UnusableReason::IncompatiblePairing]
        );

        let appimage = lab.executable("Applications/shadPS4-qt.AppImage", true);
        let unconfirmed = lab.adapter(vec![]).facts(
            CandidateIdentity {
                emulator: InventoryEmulator::ShadPs4,
                executable: Some(appimage),
                profile_root: lab.data().join("shadPS4"),
            },
            false,
        );
        assert_eq!(
            unconfirmed.unusable,
            vec![UnusableReason::ExecutableIdentityUnconfirmed]
        );
    }

    #[test]
    fn two_equally_ranked_portable_installations_are_ambiguous_in_auto() {
        let lab = Lab::new();
        let mut configured = Vec::new();
        for name in ["one", "two"] {
            let exe = lab.executable(&format!("{name}/shadps4"), false);
            std::fs::create_dir_all(exe.parent().unwrap().join("user")).unwrap();
            std::fs::write(exe.parent().unwrap().join("user/config.json"), "{}").unwrap();
            configured.push(exe);
        }
        let outcome =
            resolve_emulator_profile(&lab.adapter(configured), &EmulatorSelectionMode::Auto);
        assert!(
            matches!(outcome, ResolutionOutcome::Ambiguous { .. }),
            "{outcome:?}"
        );
    }
}
