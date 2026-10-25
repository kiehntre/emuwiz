//! Read-only native Amiberry launch planning for explicitly configured Amiga media.
//!
//! Phase 1 never guesses a machine, Kickstart, disk order, WHDLoad entry point,
//! or mount command. The selected media must already be represented by a
//! readable Amiberry profile/configuration; argv contains only the executable
//! and the documented configuration option.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{AmigaKickstartState, AmigaMachineProfile};

pub const AMIBERRY_SUPPORTED_PLATFORM_ID: &str = "Amiga";
pub const AMIBERRY_CONFIG_FLAG: &str = "--config";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmiberryMediaFormat {
    Adf,
    Adz,
    Dms,
    Ipf,
    Hdf,
    Hdz,
    Lha,
    Cue,
    Iso,
}

impl AmiberryMediaFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "adf" => Some(Self::Adf),
            "adz" => Some(Self::Adz),
            "dms" => Some(Self::Dms),
            "ipf" => Some(Self::Ipf),
            "hdf" => Some(Self::Hdf),
            "hdz" => Some(Self::Hdz),
            "lha" => Some(Self::Lha),
            "cue" => Some(Self::Cue),
            "iso" => Some(Self::Iso),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryKickstartEvidence {
    pub path: Option<PathBuf>,
    pub state: AmigaKickstartState,
    pub hash_verified: bool,
    /// Identity captured when this evidence was authorized.  A configured
    /// Kickstart without this binding cannot be safely revalidated at spawn.
    pub identity: Option<CapturedFileIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryLaunchRequest {
    pub executable: PathBuf,
    pub profile: PathBuf,
    pub canonical_platform: String,
    pub machine_model: String,
    pub selected_content: PathBuf,
    pub media_format: AmiberryMediaFormat,
    pub kickstart_evidence: AmiberryKickstartEvidence,
    pub identity_evidence: String,
    pub content_identity: CapturedFileIdentity,
    pub profile_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
    pub ipf_backend_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: AmiberryLaunchSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryLaunchSelection {
    pub platform_id: String,
    pub machine_model: String,
    pub profile: PathBuf,
    pub content: PathBuf,
    pub media_format: AmiberryMediaFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCommandPlan {
    pub command: Option<AmiberryCommand>,
    pub blockers: Vec<LaunchBlocker>,
    pub readiness: LaunchReadiness,
}

fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

fn profile_mentions_media(
    machine: &AmigaMachineProfile,
    content: &Path,
    format: AmiberryMediaFormat,
) -> bool {
    match format {
        AmiberryMediaFormat::Adf
        | AmiberryMediaFormat::Adz
        | AmiberryMediaFormat::Dms
        | AmiberryMediaFormat::Ipf => machine.floppy_mounts.iter().any(|path| path == content),
        AmiberryMediaFormat::Hdf | AmiberryMediaFormat::Hdz => {
            machine.hdf_mounts.iter().any(|path| path == content)
        }
        AmiberryMediaFormat::Lha | AmiberryMediaFormat::Cue | AmiberryMediaFormat::Iso => false,
    }
}

pub fn build_amiberry_command_plan(
    identity: &CanonicalIdentityStatus,
    request: &AmiberryLaunchRequest,
    machine: &AmigaMachineProfile,
) -> AmiberryCommandPlan {
    let mut blockers = Vec::new();
    match identity {
        CanonicalIdentityStatus::Resolved(value)
            if value.platform_id == AMIBERRY_SUPPORTED_PLATFORM_ID => {}
        CanonicalIdentityStatus::Resolved(value) => blockers.push(block(
            LaunchBlockerKind::AmiberryPlatformMismatch,
            format!("resolved platform is {}, not Amiga", value.platform_id),
        )),
        CanonicalIdentityStatus::Unknown => blockers.push(block(
            LaunchBlockerKind::IdentityUnresolved,
            "Amiga identity is unresolved",
        )),
        CanonicalIdentityStatus::Conflicting => blockers.push(block(
            LaunchBlockerKind::IdentityConflict,
            "Amiga identity evidence conflicts",
        )),
    }
    if request.canonical_platform != AMIBERRY_SUPPORTED_PLATFORM_ID
        || request.identity_evidence.trim().is_empty()
    {
        blockers.push(block(
            LaunchBlockerKind::AmiberryPlatformMismatch,
            "request lacks an independently verified Amiga identity",
        ));
    }
    if request.machine_model.trim().is_empty()
        || machine.machine_model.as_deref().is_none_or(str::is_empty)
        || machine.machine_model.as_deref() != Some(request.machine_model.as_str())
    {
        blockers.push(block(
            LaunchBlockerKind::AmiberryMachineAmbiguous,
            "machine model must come from explicit profile evidence",
        ));
    }
    if matches!(
        request.kickstart_evidence.state,
        AmigaKickstartState::Missing
            | AmigaKickstartState::Unreadable
            | AmigaKickstartState::NotConfigured
    ) {
        blockers.push(block(
            LaunchBlockerKind::AmiberryKickstartUnavailable,
            "required Kickstart is not available",
        ));
    }
    if request.media_format == AmiberryMediaFormat::Ipf && !request.ipf_backend_available {
        blockers.push(block(
            LaunchBlockerKind::AmiberryIpfBackendUnavailable,
            "IPF requires proven CAPS/SPS backend evidence",
        ));
    }
    if !profile_mentions_media(machine, &request.selected_content, request.media_format) {
        blockers.push(block(
            LaunchBlockerKind::AmiberryMediaNotConfigured,
            "selected media is not explicitly mounted by the Amiberry profile",
        ));
    }
    if !blockers.is_empty() {
        return AmiberryCommandPlan {
            command: None,
            readiness: LaunchReadiness::Blocked,
            blockers,
        };
    }
    let warnings = !request.kickstart_evidence.hash_verified;
    AmiberryCommandPlan {
        command: Some(AmiberryCommand {
            executable: request.executable.clone(),
            arguments: vec![
                OsString::from(AMIBERRY_CONFIG_FLAG),
                request.profile.clone().into_os_string(),
            ],
            working_directory: request.selected_content.parent().map(Path::to_path_buf),
            selection: AmiberryLaunchSelection {
                platform_id: AMIBERRY_SUPPORTED_PLATFORM_ID.into(),
                machine_model: request.machine_model.clone(),
                profile: request.profile.clone(),
                content: request.selected_content.clone(),
                media_format: request.media_format,
            },
        }),
        readiness: if warnings {
            LaunchReadiness::ReadyWithWarnings
        } else {
            LaunchReadiness::Ready
        },
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::ResolvedIdentity;
    use std::fs;

    fn request(path: &str) -> (AmiberryLaunchRequest, AmigaMachineProfile) {
        let content = PathBuf::from(path);
        let mut machine = AmigaMachineProfile {
            machine_model: Some("A500".into()),
            ..Default::default()
        };
        machine.floppy_mounts.push(content.clone());
        let metadata = fs::metadata("/dev/null").unwrap();
        let identity = CapturedFileIdentity::capture(&metadata);
        (
            AmiberryLaunchRequest {
                executable: "/usr/bin/amiberry".into(),
                profile: "/games/game.uae".into(),
                canonical_platform: "Amiga".into(),
                machine_model: "A500".into(),
                selected_content: content,
                media_format: AmiberryMediaFormat::Adf,
                kickstart_evidence: AmiberryKickstartEvidence {
                    path: Some("/roms/kick.rom".into()),
                    state: AmigaKickstartState::PresentUnverified,
                    hash_verified: false,
                    identity: None,
                },
                identity_evidence: "verified-amiga".into(),
                content_identity: identity,
                profile_identity: identity,
                executable_identity: identity,
                ipf_backend_available: false,
            },
            machine,
        )
    }
    #[test]
    fn explicit_configured_adf_plan_is_typed_and_shell_free() {
        let (request, machine) = request("/games/My Game/disk 1.adf");
        let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Amiga".into(),
            game_key: "A".into(),
        });
        let plan = build_amiberry_command_plan(&identity, &request, &machine);
        let command = plan.command.unwrap();
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("--config"),
                OsString::from("/games/game.uae")
            ]
        );
        assert_eq!(plan.readiness, LaunchReadiness::ReadyWithWarnings);
    }
    #[test]
    fn formats_and_unsafe_automatic_launches_fail_closed() {
        assert_eq!(
            AmiberryMediaFormat::from_path(Path::new("x.ADZ")),
            Some(AmiberryMediaFormat::Adz)
        );
        assert_eq!(AmiberryMediaFormat::from_path(Path::new("x.zip")), None);
        let (mut request, machine) = request("/games/game.adf");
        request.canonical_platform = "DOS".into();
        let plan =
            build_amiberry_command_plan(&CanonicalIdentityStatus::Unknown, &request, &machine);
        assert!(plan.command.is_none());
    }
}
