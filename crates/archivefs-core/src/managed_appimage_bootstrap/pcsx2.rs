//! PCSX2-only managed bootstrap policy.

use std::path::{Path, PathBuf};

use crate::dat::firmware_evidence::FirmwareIdentityRecord;
use crate::launch::readiness::{pcsx2_firmware_readiness, FirmwareReadiness};
use crate::patch_manager::{
    discover_pcsx2_profiles, inspect_pcsx2_game_with_firmware_evidence, parse_pcsx2_version,
    resolve_pcsx2_native_launch_binding, Pcsx2BiosVerificationOutcome, Pcsx2GameRequest,
    Pcsx2ProfileDiscoveryRoots,
};

use super::{BootstrapContext, BootstrapError, BootstrapTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2Inspection {
    pub profile_ready: bool,
    pub bios: FirmwareReadiness,
    pub configuration_paths: Vec<PathBuf>,
    pub bios_fingerprint: Option<String>,
    pub policy_fingerprint: String,
    pub checks: Vec<String>,
}

impl Pcsx2Inspection {
    pub fn bios_ready(&self) -> bool {
        self.bios == FirmwareReadiness::Verified
    }
    pub fn ready(&self) -> bool {
        self.profile_ready && self.bios_ready()
    }
}

fn target_check(context: &BootstrapContext) -> Result<(), BootstrapError> {
    if context.target != BootstrapTarget::Pcsx2 {
        return Err(BootstrapError::Policy(
            "PCSX2 policy received a different target".into(),
        ));
    }
    Ok(())
}

pub fn inspect(
    context: &BootstrapContext,
    roots: &Pcsx2ProfileDiscoveryRoots,
    evidence: &[FirmwareIdentityRecord],
) -> Result<Pcsx2Inspection, BootstrapError> {
    target_check(context)?;
    let mut roots = roots.clone();
    if let Some(executable) = &context.executable {
        let portable = executable
            .parent()
            .ok_or_else(|| BootstrapError::Policy("PCSX2 executable has no parent".into()))?
            .join("portable.ini");
        super::safety::safe_path(&portable)?;
        if portable.exists() {
            return Err(BootstrapError::Policy(
                "PCSX2 portable configuration requires manual setup".into(),
            ));
        }
        roots.explicit_executables.push(executable.clone());
    }
    let discovery = discover_pcsx2_profiles(&roots)
        .map_err(|error| BootstrapError::Policy(error.to_string()))?;
    let expected = roots.xdg_config_home.join("PCSX2");
    super::safety::safe_path(&expected)?;
    let profiles: Vec<_> = discovery
        .profiles
        .iter()
        .filter(|profile| profile.configuration_path == expected)
        .collect();
    let mut checks = Vec::new();
    if profiles.len() != 1 {
        checks.push("PCSX2 profile is missing or ambiguous".into());
        return Ok(Pcsx2Inspection {
            profile_ready: false,
            bios: FirmwareReadiness::Missing,
            configuration_paths: vec![expected],
            bios_fingerprint: None,
            policy_fingerprint: format!("profiles:{discovery:?}"),
            checks,
        });
    }
    let profile = profiles[0];
    checks.extend(
        profile
            .blockers
            .iter()
            .map(|blocker| blocker.detail.clone()),
    );
    let inspected =
        inspect_pcsx2_game_with_firmware_evidence(profile, &Pcsx2GameRequest::default(), evidence);
    let config = &inspected.inspection.global_config;
    let setup_complete = config
        .settings
        .unknown
        .get("UI/SetupWizardIncomplete")
        .is_some_and(|value| value.eq_ignore_ascii_case("false"));
    let binding = resolve_pcsx2_native_launch_binding(profile, &roots);
    let executable_bound = binding
        .map(|binding| {
            context
                .executable
                .as_ref()
                .is_none_or(|path| path == &binding.executable)
        })
        .unwrap_or(false);
    let bios = pcsx2_firmware_readiness(inspected.bios_verification.as_legacy_state());
    let bios_path = match &inspected.bios_verification {
        Pcsx2BiosVerificationOutcome::Verified(record) => Some(&record.path),
        Pcsx2BiosVerificationOutcome::Unknown { path } => Some(path),
        _ => None,
    };
    let bios_fingerprint = bios_path
        .map(|path| super::safety::file_hash(path, super::safety::MAX_HASH_BYTES))
        .transpose()?
        .flatten();
    checks.push(format!("BIOS evidence: {bios:?}"));
    checks.push("PCSX2 BIOS is evidence only and is never downloaded or modified".into());
    Ok(Pcsx2Inspection {
        profile_ready: profile.eligible && config.readable && setup_complete && executable_bound,
        bios,
        configuration_paths: vec![expected],
        bios_fingerprint,
        policy_fingerprint: format!("profile:{profile:?};bios:{bios:?}"),
        checks,
    })
}

pub fn prepare_destination(executable: &Path) -> Result<(), BootstrapError> {
    super::safety::safe_path(executable)?;
    let directory = executable
        .parent()
        .ok_or_else(|| BootstrapError::UnsafePath(executable.display().to_string()))?;
    std::fs::create_dir_all(directory)
        .map_err(|error| BootstrapError::Policy(error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(directory)
            .map_err(|error| BootstrapError::Policy(error.to_string()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(BootstrapError::Policy(
                "PCSX2 destination is not private".into(),
            ));
        }
    }
    Ok(())
}

pub fn supported_release(release: &str) -> Result<String, BootstrapError> {
    let version = release.strip_prefix('v').unwrap_or(release);
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(BootstrapError::Policy(
            "unsupported PCSX2 release format".into(),
        ));
    }
    if parts[0] != "2" {
        return Err(BootstrapError::Policy(
            "PCSX2 release is outside the supported Qt 2.x contract".into(),
        ));
    }
    Ok(version.into())
}

pub fn verify_version(output: &str, release: &str) -> Result<String, BootstrapError> {
    let expected = supported_release(release)?;
    let version = parse_pcsx2_version(output)
        .ok_or_else(|| BootstrapError::Process("unrecognized PCSX2 version output".into()))?;
    if version != expected {
        return Err(BootstrapError::StalePlan(
            "PCSX2 version differs from reviewed release",
        ));
    }
    Ok(version)
}

pub fn reject_stale_evidence(
    reviewed: &Pcsx2Inspection,
    fresh: &Pcsx2Inspection,
) -> Result<(), BootstrapError> {
    if reviewed != fresh {
        return Err(BootstrapError::StalePlan(
            "PCSX2 profile or BIOS evidence changed since review",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_release_accepts_qt_two_versions_only() {
        assert_eq!(supported_release("v2.8.2").unwrap(), "2.8.2");
        assert!(supported_release("v1.7.5").is_err());
        assert!(supported_release("latest").is_err());
    }

    #[test]
    fn version_output_must_match_reviewed_release() {
        assert_eq!(verify_version("PCSX2 2.8.2\n", "v2.8.2").unwrap(), "2.8.2");
        assert!(verify_version("PCSX2 2.8.1\n", "v2.8.2").is_err());
        assert!(verify_version("PCSX2 2.8.2\n", "v1.7.5").is_err());
    }

    #[test]
    fn changed_bios_evidence_is_stale() {
        let mut reviewed = Pcsx2Inspection {
            profile_ready: true,
            bios: FirmwareReadiness::Verified,
            configuration_paths: vec![],
            bios_fingerprint: Some("a".into()),
            policy_fingerprint: "p".into(),
            checks: vec![],
        };
        let fresh = reviewed.clone();
        assert!(reject_stale_evidence(&reviewed, &fresh).is_ok());
        reviewed.bios_fingerprint = Some("b".into());
        assert!(reject_stale_evidence(&reviewed, &fresh).is_err());
    }

    #[test]
    fn policy_rejects_ppsspp_context() {
        let context = BootstrapContext {
            target: BootstrapTarget::Ppsspp,
            root: PathBuf::from("/tmp/emuwiz"),
            destination: PathBuf::from("/tmp/emuwiz/ppsspp.AppImage"),
            executable: None,
            release: None,
            asset_url: None,
            expected_digest: None,
            source: "official".into(),
            host: "linux".into(),
            arch: "x86_64".into(),
            environment: Default::default(),
            filesystem: Default::default(),
            policy_fingerprint: "test".into(),
        };
        let roots = Pcsx2ProfileDiscoveryRoots {
            home: PathBuf::from("/tmp/home"),
            xdg_config_home: PathBuf::from("/tmp/config"),
            xdg_data_home: PathBuf::from("/tmp/data"),
            documents_home: PathBuf::from("/tmp/Documents"),
            flatpak_system_root: PathBuf::from("/tmp/flatpak"),
            appimage_directory: None,
            portable_configuration_roots: vec![],
            explicit_executables: vec![],
        };
        assert!(inspect(&context, &roots, &[]).is_err());
    }
}
