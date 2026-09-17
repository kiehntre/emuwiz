//! Shared Doctor projections for the native MAME and FBNeo launch plans.
//!
//! The launch command plans remain authoritative for set identity, platform,
//! completeness, dependency, content, and search-path decisions. Doctor only
//! presents those decisions; when no selected-game plan is available it says
//! so instead of treating an executable as launch-ready.

use crate::emulator_environment::EncodedPath;
use crate::launch::fbneo_command::{FbneoCommandPlan, FbneoReadiness, classify_fbneo_readiness};
use crate::launch::mame_command::{MameCommandPlan, MameReadiness, classify_mame_readiness};

use super::arcade_dat_version::{
    ArcadeDatVersionCompatibility, ArcadeEmulator, ArcadeEmulatorDatReadiness,
};
use super::{DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArcadeReadiness {
    pub adapter: String,
    pub executable: Option<EncodedPath>,
    pub version: Option<String>,
    pub dat_revision: Option<String>,
    pub dat_compatibility: Option<ArcadeDatVersionCompatibility>,
    pub launch_state: String,
    pub selected_set: Option<String>,
    pub set_complete: Option<bool>,
    pub dependencies_ready: Option<bool>,
    pub content_ready: Option<bool>,
    pub ready: bool,
    pub blockers: Vec<String>,
    pub evidence: Vec<String>,
    pub remediation: String,
}

fn dat_fields(
    dat: Option<&ArcadeEmulatorDatReadiness>,
) -> (
    Option<String>,
    Option<String>,
    Option<ArcadeDatVersionCompatibility>,
    Vec<String>,
) {
    let Some(dat) = dat else {
        return (None, None, None, vec!["DAT evidence: not loaded".into()]);
    };
    (
        dat.emulator_version.clone(),
        dat.dat_revision.clone(),
        Some(dat.compatibility),
        vec![
            format!("DAT ecosystem: {}", dat.dat_ecosystem.label()),
            format!(
                "DAT revision: {}",
                dat.dat_revision.as_deref().unwrap_or("unknown")
            ),
            format!("DAT compatibility: {}", dat.compatibility.label()),
        ],
    )
}

fn dat_advisory(evidence: &mut Vec<String>, dat: Option<&ArcadeEmulatorDatReadiness>) {
    if let Some(dat) = dat
        && !matches!(
            dat.compatibility,
            ArcadeDatVersionCompatibility::Matching | ArcadeDatVersionCompatibility::NotApplicable
        )
    {
        evidence.push(
            "DAT compatibility is advisory; set completeness remains a separate launch check"
                .into(),
        );
    }
}

pub fn from_mame_plan(
    plan: Option<&MameCommandPlan>,
    dat: Option<&ArcadeEmulatorDatReadiness>,
    executable: Option<EncodedPath>,
) -> ArcadeReadiness {
    let (version, dat_revision, dat_compatibility, mut evidence) = dat_fields(dat);
    dat_advisory(&mut evidence, dat);
    match plan {
        Some(plan) => {
            let state = classify_mame_readiness(plan);
            let blockers = plan
                .blockers
                .iter()
                .map(|blocker| format!("{}: {}", format_debug(blocker.kind), blocker.detail))
                .collect::<Vec<_>>();
            let selected_set = plan
                .command
                .as_ref()
                .map(|command| command.set_name.clone());
            if let Some(command) = &plan.command {
                evidence.push(format!("Selected set: {}", command.set_name));
                evidence.push(format!(
                    "Selected content: {}",
                    command.selected_content.display()
                ));
            }
            evidence.extend(
                plan.blockers
                    .iter()
                    .map(|blocker| format!("Launch evidence: {}", blocker.detail)),
            );
            let ready = matches!(state, MameReadiness::Ready);
            ArcadeReadiness {
                adapter: "MAME".into(),
                executable,
                version,
                dat_revision,
                dat_compatibility,
                launch_state: mame_state_label(state).into(),
                selected_set,
                set_complete: ready.then_some(true),
                dependencies_ready: ready.then_some(true),
                content_ready: ready.then_some(true),
                ready,
                blockers,
                evidence,
                remediation: if ready {
                    "MAME launch evidence is ready; final launch still revalidates the selected set.".into()
                } else {
                    "Fix the first reported MAME launch blocker, then run Doctor again.".into()
                },
            }
        }
        None => {
            let executable_found = executable.is_some();
            let mut blockers = Vec::new();
            if !executable_found {
                blockers.push("MAME executable was not detected".into());
            } else {
                blockers.push("selected MAME set evidence was not inspected".into());
            }
            evidence.push("Selected set: not inspected".into());
            evidence.push("Set completeness/dependencies/content: not inspected".into());
            ArcadeReadiness {
                adapter: "MAME".into(),
                executable,
                version,
                dat_revision,
                dat_compatibility,
                launch_state: mame_state_label(if executable_found {
                    MameReadiness::NeedsSetup
                } else {
                    MameReadiness::Blocked
                })
                .into(),
                selected_set: None,
                set_complete: None,
                dependencies_ready: None,
                content_ready: None,
                ready: false,
                blockers,
                evidence,
                remediation:
                    "Select content and run its MAME launch preflight before treating it as ready."
                        .into(),
            }
        }
    }
}

pub fn from_fbneo_plan(
    plan: Option<&FbneoCommandPlan>,
    dat: Option<&ArcadeEmulatorDatReadiness>,
    executable: Option<EncodedPath>,
) -> ArcadeReadiness {
    let (version, dat_revision, dat_compatibility, mut evidence) = dat_fields(dat);
    dat_advisory(&mut evidence, dat);
    match plan {
        Some(plan) => {
            let state = classify_fbneo_readiness(plan);
            let blockers = plan
                .blockers
                .iter()
                .map(|blocker| format!("{}: {}", format_debug(blocker.kind), blocker.detail))
                .collect::<Vec<_>>();
            let selected_set = plan
                .command
                .as_ref()
                .map(|command| command.driver_name.clone());
            if let Some(command) = &plan.command {
                evidence.push(format!("Verified FBNeo set: {}", command.driver_name));
                evidence.push(format!(
                    "Selected content: {}",
                    command.selected_content.display()
                ));
            }
            evidence.extend(
                plan.blockers
                    .iter()
                    .map(|blocker| format!("Launch evidence: {}", blocker.detail)),
            );
            let ready = matches!(state, FbneoReadiness::Ready);
            ArcadeReadiness {
                adapter: "FBNeo".into(),
                executable,
                version,
                dat_revision,
                dat_compatibility,
                launch_state: fbneo_state_label(state).into(),
                selected_set,
                set_complete: ready.then_some(true),
                dependencies_ready: ready.then_some(true),
                content_ready: ready.then_some(true),
                ready,
                blockers,
                evidence,
                remediation: if ready {
                    "FBNeo launch evidence is ready; final launch still revalidates the selected set.".into()
                } else {
                    "Fix the first reported FBNeo launch blocker, then run Doctor again.".into()
                },
            }
        }
        None => {
            let executable_found = executable.is_some();
            let mut blockers = Vec::new();
            if !executable_found {
                blockers.push("FBNeo executable/core was not detected".into());
            } else {
                blockers.push("verified FBNeo set evidence was not inspected".into());
            }
            evidence.push("Verified set: not inspected".into());
            evidence.push("Set completeness/dependencies/content: not inspected".into());
            evidence.push("Global BIOS requirement: none modeled by FBNeo".into());
            ArcadeReadiness {
                adapter: "FBNeo".into(),
                executable,
                version,
                dat_revision,
                dat_compatibility,
                launch_state: fbneo_state_label(if executable_found {
                    FbneoReadiness::NeedsSetup
                } else {
                    FbneoReadiness::Blocked
                })
                .into(),
                selected_set: None,
                set_complete: None,
                dependencies_ready: None,
                content_ready: None,
                ready: false,
                blockers,
                evidence,
                remediation:
                    "Select content and run its FBNeo launch preflight before treating it as ready."
                        .into(),
            }
        }
    }
}

pub fn discover_arcade_readiness(
    installations: &[super::profiles::LinuxEmulatorInstallationEvidence],
    dat_readiness: &[ArcadeEmulatorDatReadiness],
) -> Vec<ArcadeReadiness> {
    [ArcadeEmulator::Mame, ArcadeEmulator::Fbneo]
        .into_iter()
        .map(|adapter| {
            let name = adapter.label();
            let executable = installations
                .iter()
                .find(|entry| {
                    entry.emulator == name
                        || (adapter == ArcadeEmulator::Fbneo && entry.emulator == "FinalBurn Neo")
                })
                .and_then(|entry| entry.executable.clone());
            let dat = dat_readiness.iter().find(|entry| entry.emulator == adapter);
            match adapter {
                ArcadeEmulator::Mame => from_mame_plan(None, dat, executable),
                ArcadeEmulator::Fbneo => from_fbneo_plan(None, dat, executable),
            }
        })
        .collect()
}

fn mame_state_label(state: MameReadiness) -> &'static str {
    match state {
        MameReadiness::Ready => "Ready",
        MameReadiness::NeedsSetup => "Needs setup",
        MameReadiness::Blocked => "Blocked",
    }
}

fn fbneo_state_label(state: FbneoReadiness) -> &'static str {
    match state {
        FbneoReadiness::Ready => "Ready",
        FbneoReadiness::NeedsSetup => "Needs setup",
        FbneoReadiness::Blocked => "Blocked",
    }
}

fn format_debug<T: std::fmt::Debug>(value: T) -> String {
    format!("{value:?}")
}

pub fn findings_from_arcade_readiness(entries: &[ArcadeReadiness]) -> Vec<Finding> {
    entries
        .iter()
        .map(|entry| {
            let severity = if entry.ready {
                DoctorSeverity::Info
            } else {
                DoctorSeverity::Warning
            };
            let title = if entry.ready {
                format!("{} ready", entry.adapter)
            } else {
                format!("{} needs setup", entry.adapter)
            };
            let explanation = entry
                .blockers
                .first()
                .cloned()
                .unwrap_or_else(|| format!("{} launch readiness is established", entry.adapter));
            let mut finding = Finding::new(
                format!("emulator_profile.{}_readiness", entry.adapter.to_ascii_lowercase()),
                DoctorCategory::Emulators,
                DoctorSubsystem::EmulatorReadiness,
                severity,
                title,
                explanation,
            )
            .with_evidence(entry.evidence.clone())
            .with_evidence([format!("Launch readiness: {}", entry.launch_state)])
            .with_guidance(
                "EmuWiz only reports existing arcade evidence and does not change emulator or ROM files.",
                entry.remediation.clone(),
            );
            if let Some(path) = &entry.executable {
                finding = finding.with_evidence([format!("Executable: {}", path.display)]);
            }
            if let Some(version) = &entry.version {
                finding = finding.with_evidence([format!("Version: {version}")]);
            }
            if let Some(set) = &entry.selected_set {
                finding = finding.with_evidence([format!("Selected set: {set}")]);
            }
            finding
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::dependency::{DependencyState, SetDependencyReport};
    use crate::dat::model::DatEcosystem;
    use crate::dat::set::{SetIdentity, SetResolution, SetState};
    use crate::launch::fbneo_command::{
        FbneoIdentityEvidence, FbneoSetEvidence, build_fbneo_command_plan,
    };
    use crate::launch::mame_command::build_mame_command_plan;
    use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
    use std::path::Path;

    fn identity(platform: &str, key: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: key.into(),
        })
    }

    fn resolution(name: &str, state: SetState) -> SetResolution {
        SetResolution {
            identity: SetIdentity {
                source_id: "test".into(),
                game_name: name.into(),
            },
            archive_path: "/roms/set.zip".into(),
            state,
            members_required: Vec::new(),
            members_verified: Vec::new(),
            members_bad: Vec::new(),
            members_optional: Vec::new(),
            members_borrowed: Vec::new(),
            disks_required: Vec::new(),
            disks_verified: Vec::new(),
            disks_parent_required: Vec::new(),
            dependencies: SetDependencyReport {
                state: DependencyState::NotApplicable,
                requirements: Vec::new(),
            },
        }
    }

    #[test]
    fn mame_projection_reuses_plan_and_first_blocker() {
        let plan = build_mame_command_plan(
            &identity("Arcade", "pacman"),
            &[resolution("pacman", SetState::Incomplete)],
            Some(Path::new("/usr/bin/mame")),
            true,
        );
        let projection = from_mame_plan(
            Some(&plan),
            None,
            Some(EncodedPath::from_path(Path::new("/usr/bin/mame"))),
        );
        assert!(!projection.ready);
        assert!(projection.blockers[0].contains("MameSetIncomplete"));
    }

    #[test]
    fn fbneo_projection_never_invents_a_bios_blocker() {
        let set = FbneoSetEvidence {
            driver_name: "sf2".into(),
            resolution: resolution("sf2", SetState::Complete),
            identity_evidence: FbneoIdentityEvidence::VerifiedDat {
                source_id: "fbneo".into(),
                ecosystem: DatEcosystem::FBNeo,
            },
        };
        let plan = build_fbneo_command_plan(
            &identity("Arcade", "sf2"),
            &set,
            Some(Path::new("/usr/bin/fbneo")),
        );
        let projection = from_fbneo_plan(
            Some(&plan),
            None,
            Some(EncodedPath::from_path(Path::new("/usr/bin/fbneo"))),
        );
        assert!(projection.ready);
        assert!(
            projection
                .evidence
                .iter()
                .all(|item| !item.to_ascii_lowercase().contains("bios"))
        );
    }
}
