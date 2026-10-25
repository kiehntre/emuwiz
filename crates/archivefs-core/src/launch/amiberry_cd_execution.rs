//! Fresh, read-only preflight for Amiberry CD32/CDTV launches.

use std::fs;
use std::path::Path;

use crate::amiga_cd_evidence::AmigaCdMachineReadiness;
use crate::launch::amiberry_cd_command::{
    AmiberryCdCommand, AmiberryCdCommandPlan, AmiberryCdFileBinding, AmiberryCdLaunchRequest,
    build_amiberry_cd_command_plan,
};
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::patch_manager::AmigaMachineProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmiberryCdLaunchPreflightErrorKind {
    BindingUnavailable,
    BindingDrift,
    ProfileDrift,
    ContentUnavailable,
    ContentDrift,
    FirmwareUnavailable,
    FirmwareDrift,
    DependencyUnavailable,
    DependencyDrift,
    MachineDrift,
    EvidenceDrift,
    CommandBlocked,
    CommandMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCdLaunchPreflightError {
    pub kind: AmiberryCdLaunchPreflightErrorKind,
    pub detail: String,
}

fn error(
    kind: AmiberryCdLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> AmiberryCdLaunchPreflightError {
    AmiberryCdLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn checked(
    path: &Path,
    expected: CapturedFileIdentity,
    kind: AmiberryCdLaunchPreflightErrorKind,
    label: &str,
) -> Result<(), AmiberryCdLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|cause| error(kind, format!("{label} unavailable: {cause}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(kind, format!("{label} is not a safe regular file")));
    }
    if CapturedFileIdentity::capture(&metadata) != expected {
        return Err(error(kind, format!("{label} changed since authorization")));
    }
    Ok(())
}

fn checked_binding(
    binding: &AmiberryCdFileBinding,
    kind: AmiberryCdLaunchPreflightErrorKind,
    label: &str,
) -> Result<(), AmiberryCdLaunchPreflightError> {
    checked(&binding.path, binding.identity, kind, label)
}

fn prepared(command: AmiberryCdCommand) -> PreparedProcessCommand {
    PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    }
}

/// Revalidates every captured CD32/CDTV binding immediately before spawn.
/// This never rewrites profiles/media/firmware and does not provide a fallback.
pub fn preflight_amiberry_cd_launch(
    request: &AmiberryCdLaunchRequest,
    current_readiness: &AmigaCdMachineReadiness,
    current_machine: &AmigaMachineProfile,
    current_profile_media_configured: bool,
) -> Result<PreparedProcessCommand, AmiberryCdLaunchPreflightError> {
    checked(
        &request.executable,
        request.executable_identity,
        AmiberryCdLaunchPreflightErrorKind::BindingDrift,
        "Amiberry executable",
    )?;
    checked(
        &request.profile,
        request.profile_identity,
        AmiberryCdLaunchPreflightErrorKind::ProfileDrift,
        "Amiberry profile",
    )?;
    checked_binding(
        &request.selected_content,
        AmiberryCdLaunchPreflightErrorKind::ContentDrift,
        "selected CD media",
    )?;
    for dependency in &request.media_dependencies {
        checked_binding(
            dependency,
            AmiberryCdLaunchPreflightErrorKind::DependencyDrift,
            "CD media dependency",
        )?;
    }
    checked_binding(
        &request.firmware_main,
        AmiberryCdLaunchPreflightErrorKind::FirmwareDrift,
        "main Kickstart",
    )?;
    checked_binding(
        &request.firmware_extended,
        AmiberryCdLaunchPreflightErrorKind::FirmwareDrift,
        "extended ROM",
    )?;
    if request.readiness != *current_readiness {
        return Err(error(
            AmiberryCdLaunchPreflightErrorKind::EvidenceDrift,
            "CD readiness evidence changed since authorization",
        ));
    }
    if request.profile_media_configured != current_profile_media_configured {
        return Err(error(
            AmiberryCdLaunchPreflightErrorKind::EvidenceDrift,
            "profile media evidence changed since authorization",
        ));
    }
    if current_machine.machine_model.as_deref() != Some(request.machine_model.as_str()) {
        return Err(error(
            AmiberryCdLaunchPreflightErrorKind::MachineDrift,
            "machine profile changed since authorization",
        ));
    }
    let plan: AmiberryCdCommandPlan = build_amiberry_cd_command_plan(request, current_machine);
    if let Some(blocker) = plan.blockers.into_iter().next() {
        return Err(error(
            AmiberryCdLaunchPreflightErrorKind::CommandBlocked,
            format!("Amiberry CD plan blocked: {blocker:?}"),
        ));
    }
    plan.command.map(prepared).ok_or_else(|| {
        error(
            AmiberryCdLaunchPreflightErrorKind::CommandMissing,
            "Amiberry CD plan produced no command",
        )
    })
}

pub use crate::launch::process_spawn::ProcessExitReport as AmiberryCdLaunchExitReport;

pub struct LaunchedAmiberryCdProcess {
    pub pid: u32,
    pub command: PreparedProcessCommand,
    watched: WatchedProcess,
}
impl LaunchedAmiberryCdProcess {
    pub fn poll(&mut self) -> Option<&AmiberryCdLaunchExitReport> {
        self.watched.poll()
    }
    pub fn is_running(&self) -> bool {
        self.watched.is_running()
    }
}

#[derive(Debug)]
pub enum AmiberryCdLaunchSpawnError {
    Spawn(std::io::Error),
}
pub fn spawn_amiberry_cd(
    command: PreparedProcessCommand,
) -> Result<LaunchedAmiberryCdProcess, AmiberryCdLaunchSpawnError> {
    let watched = process_spawn::spawn_watched_process(&command)
        .map_err(AmiberryCdLaunchSpawnError::Spawn)?;
    Ok(LaunchedAmiberryCdProcess {
        pid: watched.pid,
        command,
        watched,
    })
}

#[derive(Debug)]
pub enum AmiberryCdLaunchExecutionError {
    Preflight(AmiberryCdLaunchPreflightError),
    Spawn(AmiberryCdLaunchSpawnError),
}
impl From<AmiberryCdLaunchPreflightError> for AmiberryCdLaunchExecutionError {
    fn from(value: AmiberryCdLaunchPreflightError) -> Self {
        Self::Preflight(value)
    }
}
impl From<AmiberryCdLaunchSpawnError> for AmiberryCdLaunchExecutionError {
    fn from(value: AmiberryCdLaunchSpawnError) -> Self {
        Self::Spawn(value)
    }
}

pub fn preflight_and_launch_amiberry_cd(
    request: &AmiberryCdLaunchRequest,
    current_readiness: &AmigaCdMachineReadiness,
    current_machine: &AmigaMachineProfile,
    current_profile_media_configured: bool,
) -> Result<LaunchedAmiberryCdProcess, AmiberryCdLaunchExecutionError> {
    Ok(spawn_amiberry_cd(preflight_amiberry_cd_launch(
        request,
        current_readiness,
        current_machine,
        current_profile_media_configured,
    )?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amiga_cd_evidence::*;
    use crate::launch::amiberry_cd_command::AmiberryCdFileBinding;
    use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
    use std::fs::File;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: std::path::PathBuf,
        request: AmiberryCdLaunchRequest,
        readiness: AmigaCdMachineReadiness,
        machine: AmigaMachineProfile,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "archivefs-amiberry-cd-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            let make = |name: &str, bytes: &[u8]| {
                let p = root.join(name);
                File::create(&p).unwrap().write_all(bytes).unwrap();
                p
            };
            let exe = make("amiberry", b"x");
            let profile = make("profile.uae", b"x");
            let media = make("disc.iso", b"x");
            let main = make("kick.rom", b"x");
            let ext = make("extended.rom", b"x");
            let bind = |p: &std::path::Path| AmiberryCdFileBinding {
                path: p.to_path_buf(),
                identity: CapturedFileIdentity::capture(&fs::metadata(p).unwrap()),
            };
            let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: AMIGA_CD32_PLATFORM_ID.into(),
                game_key: "disc".into(),
            });
            let evidence = AmigaCdIdentityEvidence {
                claims: vec![AmigaCdPlatformClaim {
                    machine: AmigaCdMachine::Cd32,
                    source: AmigaCdEvidenceSource::ProviderDat,
                }],
            };
            let readiness = assess_amiga_cd_readiness(
                &identity,
                &evidence,
                AmigaCdMachine::Cd32,
                AmigaCdFirmwareEvidence {
                    main_kickstart: AmigaCdFirmwareState::Verified,
                    extended_rom: AmigaCdFirmwareState::Verified,
                },
                AmigaCdMediaEvidence {
                    format: AmigaCdMediaFormat::Iso,
                    complete: true,
                    identified_platform: Some(AmigaCdMachine::Cd32),
                },
            );
            let machine = AmigaMachineProfile {
                machine_model: Some("CD32".into()),
                ..Default::default()
            };
            let request = AmiberryCdLaunchRequest {
                executable: bind(&exe).path,
                profile: bind(&profile).path,
                canonical_platform: AMIGA_CD32_PLATFORM_ID.into(),
                machine: AmigaCdMachine::Cd32,
                machine_model: "CD32".into(),
                selected_content: bind(&media),
                media_format: AmigaCdMediaFormat::Iso,
                media_dependencies: vec![],
                firmware_main: bind(&main),
                firmware_extended: bind(&ext),
                readiness: readiness.clone(),
                identity,
                profile_identity: bind(&profile).identity,
                executable_identity: bind(&exe).identity,
                profile_media_configured: true,
            };
            Self {
                root,
                request,
                readiness,
                machine,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn preflight_accepts_exact_cd32_and_is_shell_free() {
        let f = Fixture::new();
        let p = preflight_amiberry_cd_launch(&f.request, &f.readiness, &f.machine, true).unwrap();
        assert_eq!(p.arguments.len(), 2);
        assert_eq!(p.arguments[0], "--config");
    }
    #[test]
    fn media_drift_and_machine_drift_fail_closed() {
        let f = Fixture::new();
        fs::write(&f.request.selected_content.path, b"replacement").unwrap();
        assert_eq!(
            preflight_amiberry_cd_launch(&f.request, &f.readiness, &f.machine, true)
                .unwrap_err()
                .kind,
            AmiberryCdLaunchPreflightErrorKind::ContentDrift
        );
    }
    #[test]
    fn symlinked_media_is_rejected() {
        let f = Fixture::new();
        let other = f.root.join("other");
        File::create(&other).unwrap();
        #[cfg(unix)]
        fs::remove_file(&f.request.selected_content.path).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&other, &f.request.selected_content.path).unwrap();
        #[cfg(unix)]
        assert_eq!(
            preflight_amiberry_cd_launch(&f.request, &f.readiness, &f.machine, true)
                .unwrap_err()
                .kind,
            AmiberryCdLaunchPreflightErrorKind::ContentDrift
        );
    }
}
