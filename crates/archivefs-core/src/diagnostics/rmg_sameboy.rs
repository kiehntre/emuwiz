//! Read-only Doctor projections for the existing RMG and SameBoy adapters.
//!
//! Discovery and launch binding remain authoritative. This module only turns
//! those already-inspected facts into the shared Doctor finding model.

use crate::emulator_environment::EncodedPath;
use crate::launch::RMG_SUPPORTED_PLATFORM_ID;
use crate::launch::sameboy_command::SAMEBOY_SUPPORTED_PLATFORM_IDS;
use crate::patch_manager::{
    RmgProfileDiscoveryRoots, SameBoyProfileDiscoveryRoots, discover_rmg_profiles,
    discover_sameboy_profiles, resolve_rmg_native_launch_binding,
    resolve_sameboy_native_launch_binding,
};

use super::{DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgSameBoyReadiness {
    pub adapter: String,
    pub executable: Option<EncodedPath>,
    pub version: Option<String>,
    pub profile: Option<EncodedPath>,
    pub supported_systems: Vec<String>,
    pub ready: bool,
    pub blockers: Vec<String>,
    pub evidence: Vec<String>,
    pub remediation: String,
}

pub fn discover_rmg_sameboy_readiness() -> Vec<RmgSameBoyReadiness> {
    let mut entries = Vec::new();

    let rmg = discover_rmg_profiles(&RmgProfileDiscoveryRoots::from_environment());
    if rmg.profiles.is_empty() {
        entries.push(missing_rmg());
    } else {
        entries.extend(rmg.profiles.iter().map(rmg_entry));
    }

    if let Ok(roots) = SameBoyProfileDiscoveryRoots::from_environment() {
        let sameboy = discover_sameboy_profiles(&roots);
        if sameboy.profiles.is_empty() {
            entries.push(missing_sameboy());
        } else {
            entries.extend(sameboy.profiles.iter().map(sameboy_entry));
        }
    } else {
        entries.push(RmgSameBoyReadiness {
            adapter: "SameBoy".into(),
            executable: None,
            version: None,
            profile: None,
            supported_systems: sameboy_systems(),
            ready: false,
            blockers: vec![
                "SameBoy could not inspect its profile because HOME is unavailable".into(),
            ],
            evidence: Vec::new(),
            remediation: "Set a usable HOME directory, then run Doctor again.".into(),
        });
    }

    entries
}

fn sameboy_systems() -> Vec<String> {
    SAMEBOY_SUPPORTED_PLATFORM_IDS
        .iter()
        .map(|s| (*s).into())
        .collect()
}

fn missing_rmg() -> RmgSameBoyReadiness {
    RmgSameBoyReadiness {
        adapter: "RMG".into(),
        executable: None,
        version: None,
        profile: None,
        supported_systems: vec![RMG_SUPPORTED_PLATFORM_ID.into()],
        ready: false,
        blockers: vec!["RMG executable was not found".into()],
        evidence: Vec::new(),
        remediation: "Install RMG or select its executable in Emulator Setup.".into(),
    }
}

fn missing_sameboy() -> RmgSameBoyReadiness {
    RmgSameBoyReadiness {
        adapter: "SameBoy".into(),
        executable: None,
        version: None,
        profile: None,
        supported_systems: sameboy_systems(),
        ready: false,
        blockers: vec!["SameBoy executable/profile was not found".into()],
        evidence: Vec::new(),
        remediation: "Install SameBoy or select its executable/profile in Emulator Setup.".into(),
    }
}

fn rmg_entry(profile: &crate::patch_manager::RmgProfile) -> RmgSameBoyReadiness {
    let executable = profile.executable_candidates.first();
    let binding_error = resolve_rmg_native_launch_binding(profile)
        .err()
        .map(|error| error.detail);
    let blockers = profile
        .blocker
        .clone()
        .into_iter()
        .chain(binding_error.clone())
        .collect::<Vec<_>>();
    let ready = profile.eligible && binding_error.is_none();
    RmgSameBoyReadiness {
        adapter: "RMG".into(),
        executable: executable.map(|item| EncodedPath::from_path(&item.path)),
        version: executable.and_then(|item| item.version.clone()),
        profile: None,
        supported_systems: vec![RMG_SUPPORTED_PLATFORM_ID.into()],
        ready,
        blockers,
        evidence: vec![
            "Configuration: not required by the current RMG launch path".into(),
            "Firmware/BIOS: no external requirement is modeled for Nintendo 64 launch".into(),
            format!("Supported system: {RMG_SUPPORTED_PLATFORM_ID}"),
        ],
        remediation: if ready {
            "RMG is ready for Nintendo 64 launch; selected content is still checked by preflight."
                .into()
        } else {
            "Fix the first reported RMG blocker, then run Doctor again.".into()
        },
    }
}

fn sameboy_entry(profile: &crate::patch_manager::SameBoyProfile) -> RmgSameBoyReadiness {
    let executable = profile.executable_candidates.first();
    let binding_error = resolve_sameboy_native_launch_binding(profile)
        .err()
        .map(|error| error.detail);
    let blockers = profile
        .blocker
        .clone()
        .into_iter()
        .chain(binding_error.clone())
        .collect::<Vec<_>>();
    let ready = profile.eligible && binding_error.is_none();
    let config = if profile.config.exists && profile.config.readable {
        "preferences readable"
    } else if profile.config.oversized {
        "preferences file is too large"
    } else {
        "preferences not written yet (SameBoy defaults will be used)"
    };
    let boot_rom = match profile.boot_rom.state {
        crate::patch_manager::SameBoyBootRomState::NotConfigured => {
            "custom boot ROM: not configured; built-in boot ROMs are optional and will be used"
        }
        crate::patch_manager::SameBoyBootRomState::PresentUnverified => {
            "custom boot ROM: configured (presence observed, not verified)"
        }
        crate::patch_manager::SameBoyBootRomState::Missing => {
            "custom boot ROM: configured path is missing; built-in boot ROMs remain available"
        }
        crate::patch_manager::SameBoyBootRomState::Unknown => {
            "custom boot ROM: configured directory has no recognized boot ROM; built-in boot ROMs remain available"
        }
    };
    RmgSameBoyReadiness {
        adapter: "SameBoy".into(),
        executable: executable.map(|item| EncodedPath::from_path(&item.path)),
        version: executable.and_then(|item| item.version.clone()),
        profile: Some(EncodedPath::from_path(&profile.configuration_path)),
        supported_systems: sameboy_systems(),
        ready,
        blockers,
        evidence: vec![
            format!("Configuration: {config}"),
            format!("{boot_rom}"),
            "Firmware: external boot ROM is optional; SameBoy built-ins are used when needed"
                .into(),
            format!(
                "Supported systems: {}",
                SAMEBOY_SUPPORTED_PLATFORM_IDS.join(", ")
            ),
        ],
        remediation: if ready {
            "SameBoy is ready for Game Boy and Game Boy Color launch; selected content is still checked by preflight.".into()
        } else {
            "Fix the first reported SameBoy blocker, then run Doctor again.".into()
        },
    }
}

pub fn findings_from_rmg_sameboy_readiness(entries: &[RmgSameBoyReadiness]) -> Vec<Finding> {
    entries
        .iter()
        .map(|entry| {
            let (severity, title, explanation) = if entry.ready {
                (
                    DoctorSeverity::Info,
                    format!("{} ready", entry.adapter),
                    format!(
                        "{} has a usable discovered profile; selected-game launch preflight still runs before launch.",
                        entry.adapter
                    ),
                )
            } else {
                (
                    DoctorSeverity::Warning,
                    format!("{} needs setup", entry.adapter),
                    entry
                        .blockers
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "readiness is not established".into()),
                )
            };
            let mut finding = Finding::new(
                format!(
                    "emulator_profile.{}_readiness",
                    entry.adapter.to_ascii_lowercase().replace(' ', "_")
                ),
                DoctorCategory::EmulatorProfiles,
                DoctorSubsystem::EmulatorReadiness,
                severity,
                title,
                explanation,
            )
            .with_evidence(entry.evidence.clone())
            .with_guidance(
                "EmuWiz only reports inspected evidence and does not change emulator files.",
                entry.remediation.clone(),
            );
            if let Some(executable) = &entry.executable {
                finding = finding.with_evidence([format!("Executable: {}", executable.display)]);
            }
            if let Some(version) = &entry.version {
                finding = finding.with_evidence([format!("Version: {version}")]);
            }
            if let Some(profile) = &entry.profile {
                finding = finding.with_evidence([format!("Profile/config: {}", profile.display)]);
            }
            finding.with_evidence([format!("Supported systems: {}", entry.supported_systems.join(", "))])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::{
        RmgInstallationType, RmgProfile, SameBoyBootRomEvidence, SameBoyBootRomState,
        SameBoyConfigInspection, SameBoyInstallationType, SameBoyProfile,
    };
    use std::fs;
    use std::path::PathBuf;

    #[cfg(unix)]
    fn mark_exec(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn rmg_ready_projection_states_n64_and_no_firmware_requirement() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("RMG");
        fs::write(&executable, b"rmg").unwrap();
        #[cfg(unix)]
        mark_exec(&executable);
        let entry = rmg_entry(&RmgProfile {
            profile_id: "rmg:native".into(),
            installation_type: RmgInstallationType::Native,
            eligible: true,
            blocker: None,
            executable_candidates: vec![crate::patch_manager::RmgExecutable {
                path: executable,
                installation_type: RmgInstallationType::Native,
                version: Some("0.5.0".into()),
            }],
        });
        assert!(entry.ready);
        assert_eq!(entry.version.as_deref(), Some("0.5.0"));
        assert!(
            entry
                .evidence
                .iter()
                .any(|line| line.contains("no external requirement"))
        );
    }

    #[test]
    fn sameboy_optional_boot_rom_does_not_block_ready_profile() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("sameboy");
        fs::write(&executable, b"sameboy").unwrap();
        #[cfg(unix)]
        mark_exec(&executable);
        let entry = sameboy_entry(&SameBoyProfile {
            profile_id: "sameboy:/profile".into(),
            installation_type: SameBoyInstallationType::Explicit,
            configuration_path: PathBuf::from("/profile"),
            config: SameBoyConfigInspection {
                path: PathBuf::from("/profile/prefs.bin"),
                exists: false,
                readable: false,
                oversized: false,
            },
            eligible: true,
            blocker: None,
            executable_candidates: vec![crate::patch_manager::SameBoyExecutable {
                path: executable,
                installation_type: SameBoyInstallationType::Explicit,
                version: Some("1.0.3".into()),
            }],
            boot_rom: SameBoyBootRomEvidence {
                directory: None,
                state: SameBoyBootRomState::NotConfigured,
            },
        });
        assert!(entry.ready);
        assert!(entry.evidence.iter().any(|line| line.contains("optional")));
        assert_eq!(entry.supported_systems, vec!["Game Boy", "Game Boy Color"]);
    }
}
