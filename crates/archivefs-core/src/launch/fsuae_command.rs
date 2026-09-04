//! Read-only native FS-UAE command planning.
//!
//! FS-UAE is a distinct Amiga emulator target, not an Amiberry alias. This
//! first slice launches one exact loose floppy or hard-disk image through an
//! explicit FS-UAE profile/configuration. It consumes upstream Amiga identity
//! and Kickstart evidence; filenames do not create identity.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{
    AmigaEmulatorKind, AmigaExecutable, AmigaGameInspection, AmigaInstallationType,
    AmigaKickstartState, AmigaProfile, AmigaProfileScope,
};

pub const FSUAE_SUPPORTED_PLATFORM_ID: &str = "Amiga";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsUaeMediaFormat {
    Adf,
    Adz,
    Dms,
    Ipf,
    Hdf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUaeCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: FsUaeCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUaeCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_amiga_identity: String,
    pub media_format: FsUaeMediaFormat,
    pub content_path: PathBuf,
    pub profile_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUaeCommandPlan {
    pub command: Option<FsUaeCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsUaeLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    ExecutableUnsafe,
    ExecutableNotExecutable,
    AmbiguousExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUaeLaunchBlocker {
    pub kind: FsUaeLaunchBlockerKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUaeNativeLaunchBinding {
    pub executable: PathBuf,
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

fn media_format(path: &Path) -> Option<FsUaeMediaFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "adf" => Some(FsUaeMediaFormat::Adf),
        "adz" => Some(FsUaeMediaFormat::Adz),
        "dms" => Some(FsUaeMediaFormat::Dms),
        "ipf" => Some(FsUaeMediaFormat::Ipf),
        "hdf" => Some(FsUaeMediaFormat::Hdf),
        _ => None,
    }
}

fn profile_config(profile: &AmigaProfile) -> Option<PathBuf> {
    profile
        .global_config_path
        .clone()
        .or_else(|| profile.profile_paths.first().cloned())
}

/// Resolves exactly one safe FS-UAE executable associated with the selected
/// profile. No Amiberry or RetroArch executable can satisfy this binding.
pub fn resolve_fsuae_native_launch_binding(
    profile: &AmigaProfile,
) -> Result<FsUaeNativeLaunchBinding, FsUaeLaunchBlocker> {
    if profile.emulator != AmigaEmulatorKind::FsUae || !profile.eligible {
        return Err(FsUaeLaunchBlocker {
            kind: FsUaeLaunchBlockerKind::ProfileIneligible,
            detail: "profile is not an eligible FS-UAE profile".into(),
        });
    }
    let candidates: Vec<&AmigaExecutable> = profile
        .executable_candidates
        .iter()
        .filter(|e| {
            e.installation_type == profile.installation_type
                || (profile.scope == AmigaProfileScope::Explicit
                    && e.installation_type == AmigaInstallationType::Explicit)
        })
        .collect();
    let valid: Vec<_> = candidates
        .into_iter()
        .filter(|e| {
            let Ok(m) = fs::symlink_metadata(&e.path) else {
                return false;
            };
            if m.file_type().is_symlink() || !m.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
        .collect();
    match valid.as_slice() {
        [] => Err(FsUaeLaunchBlocker {
            kind: FsUaeLaunchBlockerKind::ExecutableMissing,
            detail: "no safe FS-UAE executable is associated with this profile".into(),
        }),
        [one] => Ok(FsUaeNativeLaunchBinding {
            executable: one.path.clone(),
        }),
        _ => Err(FsUaeLaunchBlocker {
            kind: FsUaeLaunchBlockerKind::AmbiguousExecutable,
            detail: "more than one safe FS-UAE executable matches this profile".into(),
        }),
    }
}

fn kickstart_ready(inspection: &AmigaGameInspection) -> bool {
    matches!(
        inspection.health.kickstart.state,
        AmigaKickstartState::PresentUnverified | AmigaKickstartState::Unknown
    )
}

/// Builds a deterministic native FS-UAE argv plan. `caps_backend_available`
/// is explicit evidence for IPF/CAPS; the planner never assumes it exists.
pub fn build_fsuae_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_identity: Option<&str>,
    candidate: &LaunchCandidate,
    profile: &AmigaProfile,
    inspection: &AmigaGameInspection,
    binding: &Result<FsUaeNativeLaunchBinding, FsUaeLaunchBlocker>,
    caps_backend_available: bool,
) -> FsUaeCommandPlan {
    let mut blockers = Vec::new();
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(r) => Some(r),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical Amiga identity is unresolved",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityConflict,
                "canonical Amiga identity is conflicting",
            ));
            None
        }
    };
    if let Some(r) = resolved
        && r.platform_id != FSUAE_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::FsUaePlatformMismatch,
            format!("resolved platform is {}, not ordinary Amiga", r.platform_id),
        ));
    }
    let identity_value = verified_identity.filter(|v| !v.trim().is_empty());
    if identity_value.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::FsUaeIdentityMissing,
            "no verified Amiga identity is available",
        ));
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::FsUaeCandidateRequired,
            "candidate is not a standalone target",
        ));
        return FsUaeCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "fsuae" {
        blockers.push(blocker(
            LaunchBlockerKind::FsUaeCandidateRequired,
            format!("candidate targets `{adapter_id}`, not `fsuae`"),
        ));
    }
    if candidate.readiness == LaunchReadiness::Blocked {
        blockers.extend(candidate.blockers.iter().cloned());
    }
    let Some(content) = candidate
        .content
        .resolved_path
        .as_ref()
        .filter(|_| !candidate.content.requires_mount)
    else {
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "no direct FS-UAE content path is available",
        ));
        return FsUaeCommandPlan {
            command: None,
            blockers,
        };
    };
    let Some(format) = media_format(content) else {
        blockers.push(blocker(
            LaunchBlockerKind::FsUaeContentFormatUnsupported,
            "FS-UAE Phase 1 supports direct ADF, ADZ, DMS, IPF, or HDF only",
        ));
        return FsUaeCommandPlan {
            command: None,
            blockers,
        };
    };
    if matches!(format, FsUaeMediaFormat::Ipf) && !caps_backend_available {
        blockers.push(blocker(
            LaunchBlockerKind::FsUaeIpfBackendUnavailable,
            "IPF requires explicit CAPS/SPS backend evidence",
        ));
    }
    if matches!(format, FsUaeMediaFormat::Hdf) && profile_config(profile).is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::FsUaeProfileRequired,
            "direct HDF launch requires an explicit FS-UAE profile/configuration",
        ));
    }
    let config = profile_config(profile);
    if config.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::FsUaeProfileRequired,
            "FS-UAE requires a discovered profile/configuration for this launch",
        ));
    }
    if !kickstart_ready(inspection) {
        blockers.push(blocker(
            LaunchBlockerKind::FsUaeKickstartUnavailable,
            "configured FS-UAE profile has no launch-ready Kickstart evidence",
        ));
    }
    let binding = match binding {
        Ok(b) => Some(b),
        Err(e) => {
            blockers.push(blocker(
                LaunchBlockerKind::FsUaeBindingUnavailable,
                format!("{:?}: {}", e.kind, e.detail),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return FsUaeCommandPlan {
            command: None,
            blockers,
        };
    }
    let resolved = resolved.expect("identity resolved when unblocked");
    let identity_value = identity_value.expect("identity when unblocked");
    let config = config.expect("profile config when unblocked");
    let binding = binding.expect("binding when unblocked");
    let media_flag = match format {
        FsUaeMediaFormat::Hdf => "--hard-drive-0=",
        _ => "--floppy-drive-0=",
    };
    FsUaeCommandPlan {
        command: Some(FsUaeCommand {
            executable: binding.executable.clone(),
            arguments: vec![
                OsString::from(format!("--config={}", config.display())),
                OsString::from(format!("{media_flag}{}", content.display())),
            ],
            working_directory: None,
            selection: FsUaeCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: resolved.platform_id.clone(),
                verified_amiga_identity: identity_value.to_string(),
                media_format: format,
                content_path: content.clone(),
                profile_path: config,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::{
        CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
        ResolvedIdentity,
    };
    use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
    use crate::patch_manager::{
        AmigaConfig, AmigaGameMapping, AmigaHealth, AmigaKickstart, AmigaMachineProfile,
    };

    fn profile() -> AmigaProfile {
        AmigaProfile {
            profile_id: "fsuae-test".into(),
            emulator: AmigaEmulatorKind::FsUae,
            installation_type: AmigaInstallationType::Native,
            scope: AmigaProfileScope::User,
            configuration_root: "/tmp/fs-uae".into(),
            global_config_path: Some("/tmp/fs-uae/fs-uae.conf".into()),
            profile_paths: vec![],
            executable_candidates: vec![],
            eligible: true,
            warnings: vec![],
        }
    }

    fn inspection() -> AmigaGameInspection {
        AmigaGameInspection {
            game_mapping: AmigaGameMapping::VerifiedIdentity,
            verified_identity: Some("amiga-test".into()),
            emulator_metadata: None,
            config: AmigaConfig {
                path: "/tmp/fs-uae/fs-uae.conf".into(),
                exists: true,
                readable: true,
                machine: AmigaMachineProfile::default(),
                warnings: vec![],
            },
            per_game_config: None,
            slave_candidates: vec![],
            hdf_inspections: vec![],
            save_state_candidates: vec![],
            save_state_complete: false,
            health: AmigaHealth {
                detected: true,
                config_readable: true,
                kickstart: AmigaKickstart {
                    path: None,
                    state: AmigaKickstartState::PresentUnverified,
                    hash_verified: false,
                },
                whdload_support_present: false,
                controller_configured: false,
                game_mapping: AmigaGameMapping::VerifiedIdentity,
                warnings: vec![],
            },
        }
    }

    fn candidate(path: &str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "fsuae",
                profile_id: "fsuae-test".into(),
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::Executable),
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(path.into()),
                requires_mount: false,
                provenance: "test".into(),
            },
            firmware: FirmwareReadiness::PresentUnverified,
            blockers: vec![],
            warnings: vec![],
            readiness: LaunchReadiness::ReadyWithWarnings,
            preference: CandidatePreference::SoleEligible,
        }
    }

    fn binding() -> Result<FsUaeNativeLaunchBinding, FsUaeLaunchBlocker> {
        Ok(FsUaeNativeLaunchBinding {
            executable: "/usr/bin/fs-uae".into(),
        })
    }

    #[test]
    fn fs_uae_recognizes_only_proven_direct_media_formats() {
        assert_eq!(
            media_format(Path::new("game.adf")),
            Some(FsUaeMediaFormat::Adf)
        );
        assert_eq!(
            media_format(Path::new("game.ADZ")),
            Some(FsUaeMediaFormat::Adz)
        );
        assert_eq!(
            media_format(Path::new("game.dms")),
            Some(FsUaeMediaFormat::Dms)
        );
        assert_eq!(
            media_format(Path::new("game.ipf")),
            Some(FsUaeMediaFormat::Ipf)
        );
        assert_eq!(
            media_format(Path::new("game.hdf")),
            Some(FsUaeMediaFormat::Hdf)
        );
        assert_eq!(media_format(Path::new("game.zip")), None);
        assert_eq!(media_format(Path::new("game.whd")), None);
    }

    #[test]
    fn fs_uae_builds_deterministic_native_floppy_command_without_fallback() {
        let plan = build_fsuae_command_plan(
            &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: FSUAE_SUPPORTED_PLATFORM_ID.into(),
                game_key: "amiga-test".into(),
            }),
            Some("amiga-test"),
            &candidate("/games/Amiga Games/game.adf"),
            &AmigaProfile {
                executable_candidates: vec![AmigaExecutable {
                    path: "/usr/bin/fs-uae".into(),
                    installation_type: AmigaInstallationType::Native,
                    version: None,
                }],
                ..profile()
            },
            &inspection(),
            &binding(),
            false,
        );
        let command = plan.command.expect("valid FS-UAE plan");
        assert_eq!(command.executable, PathBuf::from("/usr/bin/fs-uae"));
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("--config=/tmp/fs-uae/fs-uae.conf"),
                OsString::from("--floppy-drive-0=/games/Amiga Games/game.adf"),
            ]
        );
    }

    #[test]
    fn fs_uae_ipf_requires_explicit_caps_backend_evidence() {
        let plan = build_fsuae_command_plan(
            &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "Amiga".into(),
                game_key: "amiga-test".into(),
            }),
            Some("amiga-test"),
            &candidate("/games/game.ipf"),
            &profile(),
            &inspection(),
            &binding(),
            false,
        );
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::FsUaeIpfBackendUnavailable)
        );
    }

    #[test]
    fn fs_uae_distinct_cd_platforms_are_not_collapsed_into_amiga() {
        let plan = build_fsuae_command_plan(
            &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "AmigaCD32".into(),
                game_key: "cd32-test".into(),
            }),
            Some("cd32-test"),
            &candidate("/games/game.adf"),
            &profile(),
            &inspection(),
            &binding(),
            false,
        );
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::FsUaePlatformMismatch)
        );
    }
}
