//! Read-only Doctor projections for the existing melonDS and mGBA adapters.
//!
//! These projections consume the same discovered profiles and firmware/config
//! evidence used by launch planning and native preflight. They do not probe by
//! launching a process, write configuration, or persist a readiness snapshot.

use crate::emulator_environment::EncodedPath;
use crate::launch::readiness::FirmwareReadiness;
use crate::patch_manager::{
    MelonDsFirmwareMode, MelonDsFirmwareState, MelonDsProfileDiscoveryRoots,
    MgbaBiosState, MgbaProfileDiscoveryRoots, discover_melonds_profiles,
    discover_mgba_profiles, resolve_melonds_native_launch_binding,
    resolve_mgba_native_launch_binding,
};

use super::{DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsMgbaReadiness {
    pub adapter: String,
    pub executable: Option<EncodedPath>,
    pub version: Option<String>,
    pub profile: Option<EncodedPath>,
    pub ready: bool,
    pub blockers: Vec<String>,
    pub evidence: Vec<String>,
    pub remediation: String,
}

pub fn discover_melonds_mgba_readiness() -> Vec<MelonDsMgbaReadiness> {
    let mut entries = Vec::new();
    if let Ok(roots) = MelonDsProfileDiscoveryRoots::from_environment() {
        let discovery = discover_melonds_profiles(&roots);
        if discovery.profiles.is_empty() {
            entries.push(melonds_missing());
        } else {
            entries.extend(discovery.profiles.iter().map(melonds_entry));
        }
    }
    if let Ok(roots) = MgbaProfileDiscoveryRoots::from_environment() {
        let discovery = discover_mgba_profiles(&roots);
        if discovery.profiles.is_empty() {
            entries.push(mgba_missing());
        } else {
            entries.extend(discovery.profiles.iter().map(mgba_entry));
        }
    }
    entries
}

fn melonds_missing() -> MelonDsMgbaReadiness {
    MelonDsMgbaReadiness {
        adapter: "melonDS".into(),
        executable: None,
        version: None,
        profile: None,
        ready: false,
        blockers: vec!["melonDS executable/profile was not found".into()],
        evidence: Vec::new(),
        remediation: "Install melonDS or select its executable/profile in Emulator Setup.".into(),
    }
}

fn mgba_missing() -> MelonDsMgbaReadiness {
    MelonDsMgbaReadiness {
        adapter: "mGBA".into(),
        executable: None,
        version: None,
        profile: None,
        ready: false,
        blockers: vec!["mGBA executable/profile was not found".into()],
        evidence: Vec::new(),
        remediation: "Install mGBA or select its executable/profile in Emulator Setup.".into(),
    }
}

fn melonds_entry(
    profile: &crate::patch_manager::MelonDsProfile,
) -> MelonDsMgbaReadiness {
    let executable = profile.executable_candidates.first();
    let firmware = &profile.firmware;
    let firmware_readiness = melonds_firmware_readiness(firmware);
    let binding_error = resolve_melonds_native_launch_binding(profile)
        .err()
        .map(|error| error.detail);
    let mut blockers = profile.blocker.clone().into_iter().collect::<Vec<_>>();
    if firmware_readiness == FirmwareReadiness::Missing {
        blockers.push(melonds_missing_firmware(firmware));
    } else if firmware_readiness == FirmwareReadiness::Unknown {
        blockers.push("melonDS firmware mode or required images are unknown".into());
    }
    if let Some(error) = &binding_error {
        blockers.push(error.clone());
    }
    let ready = profile.eligible
        && firmware_readiness != FirmwareReadiness::Missing
        && firmware_readiness != FirmwareReadiness::Unknown
        && binding_error.is_none();
    let mut evidence = vec![format!(
        "Configuration: {}",
        if profile.config_path.is_some() {
            "readable"
        } else {
            "not found"
        }
    )];
    evidence.push(format!("DS readiness: {}", melonds_ds_label(ready, firmware)));
    evidence.push(
        "DSi readiness: not separately established; the current native launch path supports Nintendo DS only"
            .into(),
    );
    evidence.push(format!("BIOS7: {}", melonds_state_label(firmware.bios7)));
    evidence.push(format!("BIOS9: {}", melonds_state_label(firmware.bios9)));
    evidence.push(format!("Firmware: {}", melonds_state_label(firmware.firmware)));
    MelonDsMgbaReadiness {
        adapter: "melonDS".into(),
        executable: executable.map(|item| EncodedPath::from_path(&item.path)),
        version: executable.and_then(|item| item.version.clone()),
        profile: Some(EncodedPath::from_path(&profile.configuration_path)),
        ready,
        blockers,
        evidence,
        remediation: if ready {
            "melonDS is ready for Nintendo DS launch. DSi readiness is not established by this profile."
                .into()
        } else {
            "Fix the first reported melonDS blocker, then run Doctor again.".into()
        },
    }
}

fn mgba_entry(profile: &crate::patch_manager::MgbaProfile) -> MelonDsMgbaReadiness {
    let executable = profile.executable_candidates.first();
    let binding_error = resolve_mgba_native_launch_binding(profile)
        .err()
        .map(|error| error.detail);
    let blockers = profile
        .blocker
        .clone()
        .into_iter()
        .chain(binding_error.clone())
        .collect::<Vec<_>>();
    let ready = profile.eligible && binding_error.is_none();
    let bios = profile
        .config
        .as_ref()
        .map(|config| mgba_bios_label(config.bios))
        .unwrap_or("not configured (optional)");
    let evidence = vec![
        format!(
            "Configuration: {}",
            if profile.config_path.is_some() {
                "readable"
            } else {
                "not configured; mGBA can launch without a config"
            }
        ),
        format!("BIOS: {bios}"),
        "BIOS is optional for ordinary Game Boy, Game Boy Color, and Game Boy Advance launch".into(),
    ];
    MelonDsMgbaReadiness {
        adapter: "mGBA".into(),
        executable: executable.map(|item| EncodedPath::from_path(&item.path)),
        version: executable.and_then(|item| item.version.clone()),
        profile: Some(EncodedPath::from_path(&profile.configuration_path)),
        ready,
        blockers,
        evidence,
        remediation: if ready {
            "mGBA is ready. A BIOS is optional for ordinary launch.".into()
        } else {
            "Fix the first reported mGBA blocker, then run Doctor again.".into()
        },
    }
}

fn melonds_firmware_readiness(
    firmware: &crate::patch_manager::MelonDsFirmwareEvidence,
) -> FirmwareReadiness {
    match firmware.mode {
        MelonDsFirmwareMode::DirectBoot => FirmwareReadiness::NotRequired,
        MelonDsFirmwareMode::ExternalFirmwareBoot
            if [firmware.bios7, firmware.bios9, firmware.firmware]
                .iter()
                .all(|state| matches!(state, MelonDsFirmwareState::PresentUnverified)) =>
        {
            FirmwareReadiness::PresentUnverified
        }
        MelonDsFirmwareMode::ExternalFirmwareBoot => FirmwareReadiness::Missing,
        MelonDsFirmwareMode::Unknown => FirmwareReadiness::Unknown,
    }
}

fn melonds_ds_label(
    ready: bool,
    firmware: &crate::patch_manager::MelonDsFirmwareEvidence,
) -> &'static str {
    if ready {
        return "ready";
    }
    match firmware.mode {
        MelonDsFirmwareMode::DirectBoot => "blocked by profile or executable",
        MelonDsFirmwareMode::ExternalFirmwareBoot => "missing or incomplete firmware",
        MelonDsFirmwareMode::Unknown => "not established",
    }
}

fn melonds_state_label(state: MelonDsFirmwareState) -> &'static str {
    match state {
        MelonDsFirmwareState::NotRequiredForDirectBoot => "not required for direct boot",
        MelonDsFirmwareState::PresentUnverified => "present (unverified)",
        MelonDsFirmwareState::Missing => "missing",
        MelonDsFirmwareState::Unknown => "unknown",
    }
}

fn melonds_missing_firmware(
    firmware: &crate::patch_manager::MelonDsFirmwareEvidence,
) -> String {
    let missing = [
        ("BIOS7", firmware.bios7),
        ("BIOS9", firmware.bios9),
        ("firmware", firmware.firmware),
    ]
    .into_iter()
    .filter_map(|(name, state)| (state == MelonDsFirmwareState::Missing).then_some(name))
    .collect::<Vec<_>>();
    format!("melonDS requires {}", missing.join(", "))
}

fn mgba_bios_label(state: MgbaBiosState) -> &'static str {
    match state {
        MgbaBiosState::NotConfigured => "not configured (optional)",
        MgbaBiosState::PresentUnverified => "present (unverified)",
        MgbaBiosState::Missing => "configured path is missing (optional)",
        MgbaBiosState::Unknown => "unknown (optional)",
    }
}

pub fn findings_from_melonds_mgba_readiness(
    entries: &[MelonDsMgbaReadiness],
) -> Vec<Finding> {
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
                    entry.adapter.to_ascii_lowercase()
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
            finding
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ds_ready_does_not_imply_dsi_ready() {
        let entries = findings_from_melonds_mgba_readiness(&[MelonDsMgbaReadiness {
            adapter: "melonDS".into(),
            executable: None,
            version: None,
            profile: None,
            ready: true,
            blockers: Vec::new(),
            evidence: vec![
                "DS readiness: ready".into(),
                "DSi readiness: not separately established; the current native launch path supports Nintendo DS only".into(),
            ],
            remediation: "No action.".into(),
        }]);
        assert!(entries[0].explanation.contains("selected-game launch preflight"));
        assert!(entries[0].evidence.iter().any(|line| line.contains("DSi readiness: not separately established")));
    }

    #[test]
    fn mgba_optional_bios_is_not_a_false_blocker() {
        let entries = findings_from_melonds_mgba_readiness(&[MelonDsMgbaReadiness {
            adapter: "mGBA".into(),
            executable: None,
            version: None,
            profile: None,
            ready: true,
            blockers: Vec::new(),
            evidence: vec![
                "BIOS: not configured (optional)".into(),
                "BIOS is optional for ordinary Game Boy, Game Boy Color, and Game Boy Advance launch".into(),
            ],
            remediation: "mGBA is ready. A BIOS is optional for ordinary launch.".into(),
        }]);
        assert_eq!(entries[0].severity, DoctorSeverity::Info);
        assert!(entries[0].title.contains("ready"));
        assert!(entries[0].evidence.iter().any(|line| line.contains("optional")));
    }
}
