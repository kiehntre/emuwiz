//! Doctor projections for the existing ScummVM and DOSBox Staging evidence.
//!
//! These adapters deliberately remain thin: the backend assessments own
//! executable binding, detector/config inspection, and blocker semantics.
//! Doctor only turns their read-only results into the shared finding model.

use crate::emulator_environment::EncodedPath;
use crate::launch::dosbox_command::DosBoxReadinessEvidence;
use crate::scummvm_detection::{ScummVmReadinessBlockerKind, ScummVmReadinessEvidence};

use super::profiles::LinuxEmulatorInstallationEvidence;
use super::{DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmDoctorReadiness {
    pub executable: Option<EncodedPath>,
    pub version: Option<String>,
    pub detector_available: bool,
    pub game_folder: Option<EncodedPath>,
    pub detected_game: Option<String>,
    pub ready: bool,
    pub first_blocker: Option<String>,
    pub evidence: Vec<String>,
    pub remediation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DosBoxStagingDoctorReadiness {
    pub executable: Option<EncodedPath>,
    pub version: Option<String>,
    pub staging_identified: bool,
    pub config_path: Option<EncodedPath>,
    pub config_readable: bool,
    pub autoexec_command_lines: usize,
    pub ready: bool,
    pub first_blocker: Option<String>,
    pub evidence: Vec<String>,
    pub remediation: String,
}

fn path(path: Option<&std::path::Path>) -> Option<EncodedPath> {
    path.map(EncodedPath::from_path)
}

pub fn project_scummvm(evidence: &ScummVmReadinessEvidence) -> ScummVmDoctorReadiness {
    let detected_game = evidence.detected_game.as_ref().map(|game| {
        game.description
            .as_deref()
            .map(|description| format!("{} ({})", description, game.game_id))
            .unwrap_or_else(|| game.game_id.clone())
    });
    let mut facts = vec![format!(
        "Detector: {}",
        if evidence.detector_available {
            "available"
        } else {
            "unavailable"
        }
    )];
    facts.push(format!(
        "Game folder: {}",
        if evidence.game_folder.is_some() {
            "inspected"
        } else {
            "not selected"
        }
    ));
    if let Some(game) = &detected_game {
        facts.push(format!("Detected game: {game}"));
    }
    if let Some(blocker) = &evidence.first_blocker {
        facts.push(format!("First blocker: {blocker}"));
    }
    ScummVmDoctorReadiness {
        executable: path(evidence.executable.as_deref()),
        version: evidence.version.clone(),
        detector_available: evidence.detector_available,
        game_folder: path(evidence.game_folder.as_deref()),
        detected_game,
        ready: evidence.ready,
        first_blocker: evidence.first_blocker.clone(),
        evidence: facts,
        remediation: scummvm_remediation(evidence.blocker_kind.as_ref()),
    }
}

pub fn project_dosbox(evidence: &DosBoxReadinessEvidence) -> DosBoxStagingDoctorReadiness {
    let staging_identified = evidence.variant.is_some();
    let mut facts = vec![format!(
        "DOSBox implementation: {}",
        if staging_identified {
            "DOSBox Staging"
        } else {
            "unsupported or unknown variant"
        }
    )];
    facts.push(format!(
        "Configuration: {}",
        if evidence.config_readable {
            "readable"
        } else {
            "missing, unreadable, or not inspected"
        }
    ));
    facts.push(format!(
        "[autoexec] command lines: {} (structural inspection only)",
        evidence.autoexec_command_lines
    ));
    if let Some(blocker) = &evidence.first_blocker {
        facts.push(format!("First blocker: {blocker}"));
    }
    DosBoxStagingDoctorReadiness {
        executable: path(evidence.executable.as_deref()),
        version: evidence.version.clone(),
        staging_identified,
        config_path: path(evidence.config_path.as_deref()),
        config_readable: evidence.config_readable,
        autoexec_command_lines: evidence.autoexec_command_lines,
        ready: evidence.ready,
        first_blocker: evidence.first_blocker.clone(),
        evidence: facts,
        remediation: if evidence.ready {
            "DOSBox Staging is ready; final launch still revalidates the bound configuration."
                .into()
        } else {
            "Fix the first reported DOSBox Staging blocker, then run Doctor again.".into()
        },
    }
}

pub fn discover_scummvm_dosbox_readiness(
    installations: &[LinuxEmulatorInstallationEvidence],
) -> (ScummVmDoctorReadiness, DosBoxStagingDoctorReadiness) {
    let scummvm_path = installation_path(installations, "ScummVM");
    let scummvm = crate::scummvm_detection::assess_scummvm_readiness(scummvm_path.as_deref(), None);
    let dosbox_path = installation_path(installations, "DOSBox Staging");
    let dosbox = crate::launch::dosbox_command::assess_dosbox_readiness(
        dosbox_path.as_deref(),
        "dosbox-staging",
        None,
    );
    (project_scummvm(&scummvm), project_dosbox(&dosbox))
}

fn installation_path(
    installations: &[LinuxEmulatorInstallationEvidence],
    emulator: &str,
) -> Option<std::path::PathBuf> {
    installations
        .iter()
        .find(|entry| entry.emulator == emulator)
        .and_then(|entry| entry.executable.as_ref())
        .filter(|path| !path.lossy)
        .map(|path| std::path::PathBuf::from(&path.display))
}

fn scummvm_remediation(blocker: Option<&ScummVmReadinessBlockerKind>) -> String {
    match blocker {
        None => {
            "ScummVM is ready for the inspected profile; select a game folder to run detection."
                .into()
        }
        Some(ScummVmReadinessBlockerKind::ExecutableMissing) => {
            "Install ScummVM or select a safe executable in Emulator Setup.".into()
        }
        Some(ScummVmReadinessBlockerKind::DetectorUnavailable)
        | Some(ScummVmReadinessBlockerKind::DetectorFailed) => {
            "Repair the ScummVM detector executable or installation, then run Doctor again.".into()
        }
        Some(ScummVmReadinessBlockerKind::InvalidGameFolder) => {
            "Choose a readable, regular ScummVM game folder and run Doctor again.".into()
        }
        Some(ScummVmReadinessBlockerKind::IdentityUnresolved) => {
            "Choose a game folder ScummVM can identify unambiguously.".into()
        }
        Some(ScummVmReadinessBlockerKind::UnsupportedGame) => {
            "Choose a game folder supported by this ScummVM build.".into()
        }
    }
}

pub fn findings_from_scummvm_dosbox_readiness(
    scummvm: &ScummVmDoctorReadiness,
    dosbox: &DosBoxStagingDoctorReadiness,
) -> Vec<Finding> {
    vec![scummvm_finding(scummvm), dosbox_finding(dosbox)]
}

pub fn findings_from_scummvm_readiness(entry: &ScummVmDoctorReadiness) -> Vec<Finding> {
    vec![scummvm_finding(entry)]
}

pub fn findings_from_dosbox_staging_readiness(
    entry: &DosBoxStagingDoctorReadiness,
) -> Vec<Finding> {
    vec![dosbox_finding(entry)]
}

fn scummvm_finding(entry: &ScummVmDoctorReadiness) -> Finding {
    let mut finding = Finding::new(
        "emulator_profile.scummvm_readiness",
        DoctorCategory::Emulators,
        DoctorSubsystem::EmulatorReadiness,
        if entry.ready {
            DoctorSeverity::Info
        } else {
            DoctorSeverity::Warning
        },
        if entry.ready {
            "ScummVM ready"
        } else {
            "ScummVM needs setup"
        },
        entry
            .first_blocker
            .clone()
            .unwrap_or_else(|| "ScummVM readiness is established".into()),
    )
    .with_evidence(entry.evidence.clone())
    .with_guidance(
        "EmuWiz only reports ScummVM evidence and never creates persistent detector configuration.",
        entry.remediation.clone(),
    );
    if let Some(executable) = &entry.executable {
        finding = finding.with_evidence([format!("Executable: {}", executable.display)]);
    }
    if let Some(version) = &entry.version {
        finding = finding.with_evidence([format!("Version: {version}")]);
    }
    if let Some(folder) = &entry.game_folder {
        finding = finding.with_evidence([format!("Game folder: {}", folder.display)]);
    }
    finding
}

fn dosbox_finding(entry: &DosBoxStagingDoctorReadiness) -> Finding {
    let mut finding = Finding::new(
        "emulator_profile.dosbox_staging_readiness",
        DoctorCategory::Emulators,
        DoctorSubsystem::EmulatorReadiness,
        if entry.ready {
            DoctorSeverity::Info
        } else {
            DoctorSeverity::Warning
        },
        if entry.ready {
            "DOSBox Staging ready"
        } else {
            "DOSBox Staging needs setup"
        },
        entry
            .first_blocker
            .clone()
            .unwrap_or_else(|| "DOSBox Staging readiness is established".into()),
    )
    .with_evidence(entry.evidence.clone())
    .with_guidance(
        "EmuWiz inspects DOSBox Staging configuration structurally and never executes [autoexec] commands.",
        entry.remediation.clone(),
    );
    if let Some(executable) = &entry.executable {
        finding = finding.with_evidence([format!("Executable: {}", executable.display)]);
    }
    if let Some(version) = &entry.version {
        finding = finding.with_evidence([format!("Version: {version}")]);
    }
    if let Some(config) = &entry.config_path {
        finding = finding.with_evidence([format!("Config: {}", config.display)]);
    }
    finding
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::dosbox_command::DosBoxVariant;
    use crate::scummvm_detection::ScummVmDetectedGame;

    #[test]
    fn scummvm_projection_preserves_detected_identity_and_blocker() {
        let evidence = ScummVmReadinessEvidence {
            executable: Some("/usr/bin/scummvm".into()),
            version: Some("ScummVM 2.8.1".into()),
            detector_available: true,
            game_folder: Some("/roms/game".into()),
            detected_game: Some(ScummVmDetectedGame {
                game_id: "scumm:monkey".into(),
                engine_id: "scumm".into(),
                description: Some("Monkey Island".into()),
                platform: None,
                language: None,
                variant: None,
                demo: None,
            }),
            blocker_kind: None,
            first_blocker: None,
            ready: true,
        };
        let projection = project_scummvm(&evidence);
        assert!(projection.ready);
        assert_eq!(
            projection.detected_game.as_deref(),
            Some("Monkey Island (scumm:monkey)")
        );
    }

    #[test]
    fn dosbox_projection_exposes_structural_autoexec_without_execution() {
        let evidence = DosBoxReadinessEvidence {
            executable: Some("/usr/bin/dosbox-staging".into()),
            version: None,
            variant: Some(DosBoxVariant::Staging),
            config_path: Some("/games/dosbox.conf".into()),
            config_readable: true,
            autoexec_command_lines: 2,
            ready: true,
            first_blocker: None,
        };
        let projection = project_dosbox(&evidence);
        assert!(projection.ready);
        assert_eq!(projection.autoexec_command_lines, 2);
        assert!(
            projection
                .evidence
                .iter()
                .any(|line| line.contains("structural inspection only"))
        );
    }
}
