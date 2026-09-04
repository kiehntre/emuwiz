//! Safe native Hatari command planning.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{
    HatariGameInspection, HatariMachineModel, HatariProfile, HatariTosHealth,
};

pub const HATARI_SUPPORTED_PLATFORM_ID: &str = "AtariST";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HatariMediaFormat {
    St,
    Msa,
    Stx,
    Dim,
    Ipf,
}

pub fn hatari_media_format(path: &Path) -> Option<HatariMediaFormat> {
    match path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "st" => Some(HatariMediaFormat::St),
        "msa" => Some(HatariMediaFormat::Msa),
        "stx" => Some(HatariMediaFormat::Stx),
        "dim" => Some(HatariMediaFormat::Dim),
        "ipf" => Some(HatariMediaFormat::Ipf),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: HatariCommandSelection,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub machine_model: HatariMachineModel,
    pub content_path: PathBuf,
    pub media_format: HatariMediaFormat,
    pub tos_path: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariCommandPlan {
    pub command: Option<HatariCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

/// Builds an explicit-profile native Hatari invocation. The profile remains
/// read-only; the selected disk is passed as a separate argv component.
pub fn build_hatari_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    profile: &HatariProfile,
    inspection: &HatariGameInspection,
    selected_content: &Path,
    expected_machine: HatariMachineModel,
    disk_drive: char,
    ipf_backend_available: bool,
) -> HatariCommandPlan {
    let mut blockers = Vec::new();
    match identity {
        CanonicalIdentityStatus::Resolved(r) if r.platform_id == HATARI_SUPPORTED_PLATFORM_ID => {}
        CanonicalIdentityStatus::Resolved(r) => blockers.push(blocker(
            LaunchBlockerKind::HatariPlatformMismatch,
            format!("resolved platform is {}, not AtariST", r.platform_id),
        )),
        CanonicalIdentityStatus::Unknown => blockers.push(blocker(
            LaunchBlockerKind::IdentityUnresolved,
            "Atari ST identity is unresolved",
        )),
        CanonicalIdentityStatus::Conflicting => blockers.push(blocker(
            LaunchBlockerKind::IdentityConflict,
            "Atari ST identity evidence is conflicting",
        )),
    }
    let LaunchTarget::Standalone { adapter_id, .. } = &candidate.target else {
        blockers.push(blocker(
            LaunchBlockerKind::HatariCandidateRequired,
            "candidate is not a standalone Hatari target",
        ));
        return HatariCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "hatari" {
        blockers.push(blocker(
            LaunchBlockerKind::HatariCandidateRequired,
            format!("candidate targets `{adapter_id}`, not `hatari`"),
        ));
    }
    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
    }
    if !profile.eligible {
        blockers.push(blocker(
            LaunchBlockerKind::HatariProfileUnavailable,
            "selected Hatari profile is not eligible",
        ));
    }
    let Some(executable) = profile.executable_candidates.as_slice().first() else {
        blockers.push(blocker(
            LaunchBlockerKind::HatariEmulatorUnavailable,
            "no Hatari executable is bound to the selected profile",
        ));
        return HatariCommandPlan {
            command: None,
            blockers,
        };
    };
    if profile.executable_candidates.len() != 1 {
        blockers.push(blocker(
            LaunchBlockerKind::HatariBindingAmbiguous,
            "more than one Hatari executable remains possible",
        ));
    }
    if expected_machine == HatariMachineModel::Unknown
        || inspection.config.machine.model == HatariMachineModel::Unknown
        || inspection.config.machine.model != expected_machine
    {
        blockers.push(blocker(
            LaunchBlockerKind::HatariMachineMismatch,
            "Hatari machine model does not match the authorized model",
        ));
    }
    let Some(tos_path) = inspection.health.tos.path.clone() else {
        blockers.push(blocker(
            LaunchBlockerKind::HatariTosUnavailable,
            "selected Hatari profile has no TOS path",
        ));
        return HatariCommandPlan {
            command: None,
            blockers,
        };
    };
    match inspection.health.tos.health {
        HatariTosHealth::Verified | HatariTosHealth::PresentUnverified => {}
        HatariTosHealth::Missing => blockers.push(blocker(
            LaunchBlockerKind::HatariTosMissing,
            "configured Hatari TOS image is missing",
        )),
        HatariTosHealth::NotConfigured | HatariTosHealth::Unreadable => blockers.push(blocker(
            LaunchBlockerKind::HatariTosUnavailable,
            "Hatari TOS image is not safely available",
        )),
    }
    let Some(format) = hatari_media_format(selected_content) else {
        blockers.push(blocker(
            LaunchBlockerKind::HatariContentFormatUnsupported,
            "native Hatari supports .st, .msa, .stx, .dim, and conditional .ipf media",
        ));
        return HatariCommandPlan {
            command: None,
            blockers,
        };
    };
    if format == HatariMediaFormat::Ipf && !ipf_backend_available {
        blockers.push(blocker(
            LaunchBlockerKind::HatariIpfBackendUnavailable,
            "IPF media requires unavailable CAPS/SPS support",
        ));
    }
    if !selected_content.is_absolute() {
        blockers.push(blocker(
            LaunchBlockerKind::HatariContentUnavailable,
            "selected Hatari media path must be absolute",
        ));
    }
    if !matches!(disk_drive, 'A' | 'a' | 'B' | 'b') {
        blockers.push(blocker(
            LaunchBlockerKind::HatariDiskDriveInvalid,
            "disk selection must explicitly name drive A or B",
        ));
    }
    if !blockers.is_empty() {
        return HatariCommandPlan {
            command: None,
            blockers,
        };
    }
    let flag = if disk_drive.eq_ignore_ascii_case(&'A') {
        "--disk-a"
    } else {
        "--disk-b"
    };
    HatariCommandPlan {
        command: Some(HatariCommand {
            executable: executable.path.clone(),
            arguments: vec![
                "--configfile".into(),
                profile.config_path.clone().into_os_string(),
                flag.into(),
                selected_content.as_os_str().to_os_string(),
            ],
            working_directory: None,
            selection: HatariCommandSelection {
                profile_id: profile.profile_id.clone(),
                platform_id: HATARI_SUPPORTED_PLATFORM_ID.into(),
                machine_model: expected_machine,
                content_path: selected_content.into(),
                media_format: format,
                tos_path,
            },
        }),
        blockers,
    }
}

#[cfg(test)]
mod tests;
