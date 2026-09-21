//! Read-only Doctor projections for the existing DeSmuME and Mesen adapters.
//!
//! The adapter discovery and launch-preflight code remains authoritative. This
//! module only presents the already-supported profile facts; it never launches
//! an emulator, writes configuration, or persists a readiness result.

use crate::emulator_environment::EncodedPath;
use crate::launch::{
    DESMUME_SUPPORTED_PLATFORM_ID, DesmumeProfileDiscoveryRoots, MESEN_SUPPORTED_PLATFORM_IDS,
    discover_desmume_profiles, resolve_desmume_native_launch_binding,
};
use crate::patch_manager::{
    MesenProfileDiscoveryRoots, discover_mesen_profiles, resolve_mesen_native_launch_binding,
};

use super::{DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeMesenReadiness {
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

pub fn discover_desmume_mesen_readiness() -> Vec<DesmumeMesenReadiness> {
    let mut entries = Vec::new();
    let desmume_roots = DesmumeProfileDiscoveryRoots::from_environment();
    let desmume = discover_desmume_profiles(&desmume_roots);
    if desmume.profiles.is_empty() {
        entries.push(missing_desmume());
    } else {
        entries.extend(desmume.profiles.iter().map(desmume_entry));
    }

    if let Ok(mesen_roots) = MesenProfileDiscoveryRoots::from_environment() {
        let mesen = discover_mesen_profiles(&mesen_roots);
        if mesen.profiles.is_empty() {
            entries.push(missing_mesen());
        } else {
            entries.extend(mesen.profiles.iter().map(mesen_entry));
        }
    }
    entries
}

fn missing_desmume() -> DesmumeMesenReadiness {
    DesmumeMesenReadiness {
        adapter: "DeSmuME".into(),
        executable: None,
        version: None,
        profile: None,
        supported_systems: vec![DESMUME_SUPPORTED_PLATFORM_ID.into()],
        ready: false,
        blockers: vec!["DeSmuME executable was not found".into()],
        evidence: Vec::new(),
        remediation: "Install DeSmuME or select its executable in Emulator Setup.".into(),
    }
}

fn missing_mesen() -> DesmumeMesenReadiness {
    DesmumeMesenReadiness {
        adapter: "Mesen 2".into(),
        executable: None,
        version: None,
        profile: None,
        supported_systems: MESEN_SUPPORTED_PLATFORM_IDS
            .iter()
            .map(|platform| (*platform).into())
            .collect(),
        ready: false,
        blockers: vec!["Mesen executable/profile was not found".into()],
        evidence: Vec::new(),
        remediation: "Install Mesen 2 or select its executable/profile in Emulator Setup.".into(),
    }
}

fn desmume_entry(profile: &crate::launch::DesmumeProfile) -> DesmumeMesenReadiness {
    let executable = profile.executable.as_ref();
    let binding_error = resolve_desmume_native_launch_binding(profile)
        .err()
        .map(|error| error.detail);
    let blockers = profile
        .blocker
        .clone()
        .into_iter()
        .chain(binding_error.clone())
        .collect::<Vec<_>>();
    let ready = profile.eligible && binding_error.is_none();
    DesmumeMesenReadiness {
        adapter: "DeSmuME".into(),
        executable: executable.map(|item| EncodedPath::from_path(&item.path)),
        version: executable.and_then(|item| item.version.clone()),
        profile: profile
            .executable
            .as_ref()
            .map(|item| EncodedPath::from_path(&item.path)),
        supported_systems: vec![DESMUME_SUPPORTED_PLATFORM_ID.into()],
        ready,
        blockers,
        evidence: vec![
            "Configuration: not required by the current DeSmuME launch path".into(),
            "BIOS/firmware: no external requirement is modeled by this adapter".into(),
            format!("Supported system: {DESMUME_SUPPORTED_PLATFORM_ID}"),
        ],
        remediation: if ready {
            "DeSmuME is ready for Nintendo DS launch; selected content is still checked by preflight.".into()
        } else {
            "Fix the first reported DeSmuME blocker, then run Doctor again.".into()
        },
    }
}

fn mesen_entry(profile: &crate::patch_manager::MesenProfile) -> DesmumeMesenReadiness {
    let executable = profile.executable_candidates.first();
    let binding_error = resolve_mesen_native_launch_binding(profile)
        .err()
        .map(|error| error.detail);
    let blockers = profile
        .blocker
        .clone()
        .into_iter()
        .chain(binding_error.clone())
        .collect::<Vec<_>>();
    let ready = profile.eligible && binding_error.is_none();
    DesmumeMesenReadiness {
        adapter: "Mesen 2".into(),
        executable: executable.map(|item| EncodedPath::from_path(&item.path)),
        version: executable.and_then(|item| item.version.clone()),
        profile: Some(EncodedPath::from_path(&profile.configuration_path)),
        supported_systems: MESEN_SUPPORTED_PLATFORM_IDS
            .iter()
            .map(|platform| (*platform).into())
            .collect(),
        ready,
        blockers,
        evidence: vec![
            format!(
                "Configuration: {}",
                if profile.config.readable {
                    "settings.json readable"
                } else if profile.config.oversized {
                    "settings.json is too large"
                } else {
                    "settings.json missing or unreadable"
                }
            ),
            format!(
                "Supported systems: {}",
                MESEN_SUPPORTED_PLATFORM_IDS.join(", ")
            ),
            "Firmware: no external firmware requirement is modeled for the supported systems"
                .into(),
        ],
        remediation: if ready {
            "Mesen 2 is ready for its supported systems; selected content is still checked by preflight.".into()
        } else {
            "Fix the first reported Mesen 2 blocker, then run Doctor again.".into()
        },
    }
}

pub fn findings_from_desmume_mesen_readiness(entries: &[DesmumeMesenReadiness]) -> Vec<Finding> {
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
            finding = finding.with_evidence([format!(
                "Supported systems: {}",
                entry.supported_systems.join(", ")
            )]);
            finding
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desmume_optional_firmware_does_not_block_readiness() {
        let findings = findings_from_desmume_mesen_readiness(&[DesmumeMesenReadiness {
            adapter: "DeSmuME".into(),
            executable: None,
            version: None,
            profile: None,
            supported_systems: vec![DESMUME_SUPPORTED_PLATFORM_ID.into()],
            ready: true,
            blockers: Vec::new(),
            evidence: vec![
                "Configuration: not required by the current DeSmuME launch path".into(),
                "BIOS/firmware: no external requirement is modeled by this adapter".into(),
            ],
            remediation: "Ready.".into(),
        }]);
        assert_eq!(findings[0].severity, DoctorSeverity::Info);
        assert!(
            findings[0]
                .evidence
                .iter()
                .any(|line| line.contains("no external requirement"))
        );
    }

    #[test]
    fn mesen_unsupported_system_is_not_presented_as_a_supported_system() {
        let findings = findings_from_desmume_mesen_readiness(&[DesmumeMesenReadiness {
            adapter: "Mesen 2".into(),
            executable: None,
            version: None,
            profile: None,
            supported_systems: MESEN_SUPPORTED_PLATFORM_IDS
                .iter()
                .map(|platform| (*platform).into())
                .collect(),
            ready: false,
            blockers: vec!["resolved platform is not supported by Mesen 2".into()],
            evidence: Vec::new(),
            remediation: "Choose a supported system.".into(),
        }]);
        assert_eq!(findings[0].severity, DoctorSeverity::Warning);
        assert!(findings[0].explanation.contains("not supported"));
    }
}
