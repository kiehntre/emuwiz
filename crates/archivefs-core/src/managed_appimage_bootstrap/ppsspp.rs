//! PPSSPP-only managed bootstrap policy.

use std::path::PathBuf;

use crate::diagnostics::profiles::assess_ppsspp_readiness;
use crate::patch_manager::{
    discover_ppsspp_profiles, parse_ppsspp_version, PpssppProfileDiscoveryRoots,
};

use super::{BootstrapContext, BootstrapError, BootstrapTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppInspection {
    pub profile_ready: bool,
    pub firmware_required: bool,
    pub configuration_paths: Vec<PathBuf>,
    pub policy_fingerprint: String,
    pub checks: Vec<String>,
}

impl PpssppInspection {
    pub const FIRMWARE_REQUIRED: bool = false;

    pub fn ready(&self) -> bool {
        self.profile_ready && !self.firmware_required
    }
}

fn target_check(context: &BootstrapContext) -> Result<(), BootstrapError> {
    if context.target != BootstrapTarget::Ppsspp {
        return Err(BootstrapError::Policy(
            "PPSSPP policy received a different target".into(),
        ));
    }
    Ok(())
}

pub fn inspect(
    context: &BootstrapContext,
    roots: &PpssppProfileDiscoveryRoots,
) -> Result<PpssppInspection, BootstrapError> {
    target_check(context)?;
    let mut roots = roots.clone();
    if let Some(executable) = &context.executable {
        super::safety::safe_path(executable)?;
        roots.explicit_executables.push(executable.clone());
    }
    let discovery = discover_ppsspp_profiles(&roots);
    let assessments = assess_ppsspp_readiness(Some(&discovery));
    let profile_ready = assessments
        .iter()
        .any(|assessment| assessment.executable.is_some());
    let configuration_paths = discovery
        .profiles
        .iter()
        .map(|profile| profile.configuration_path.clone())
        .collect::<Vec<_>>();
    let mut checks = discovery
        .warnings
        .iter()
        .map(|warning| warning.detail.clone())
        .collect::<Vec<_>>();
    checks.extend(
        assessments
            .iter()
            .filter_map(|assessment| assessment.binding_problem.clone()),
    );
    checks.push("PPSSPP firmware is not required; launch checks remain separate".into());
    Ok(PpssppInspection {
        profile_ready,
        firmware_required: false,
        configuration_paths,
        policy_fingerprint: format!("profiles:{discovery:?};readiness:{assessments:?}"),
        checks,
    })
}

pub fn configuration_paths(roots: &PpssppProfileDiscoveryRoots) -> Vec<PathBuf> {
    vec![
        roots.xdg_config_home.join("ppsspp/PSP/SYSTEM/ppsspp.ini"),
        roots.xdg_data_home.join("ppsspp/PSP/SYSTEM/ppsspp.ini"),
    ]
}

pub fn first_run_required(roots: &PpssppProfileDiscoveryRoots) -> bool {
    configuration_paths(roots)
        .iter()
        .all(|path| !path.is_file())
}

pub fn verify_version(output: &str, release: &str) -> Result<String, BootstrapError> {
    let version = parse_ppsspp_version(output)
        .ok_or_else(|| BootstrapError::Process("unrecognized PPSSPP version output".into()))?;
    let expected = release.strip_prefix('v').unwrap_or(release);
    if version != expected {
        return Err(BootstrapError::StalePlan(
            "PPSSPP version differs from reviewed release",
        ));
    }
    Ok(version)
}

pub fn reject_stale_evidence(
    reviewed: &PpssppInspection,
    fresh: &PpssppInspection,
) -> Result<(), BootstrapError> {
    if reviewed != fresh {
        return Err(BootstrapError::StalePlan(
            "PPSSPP profile or configuration evidence changed since review",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_output_must_match_reviewed_release() {
        assert_eq!(
            verify_version("PPSSPP v1.17.1-42\n", "v1.17.1").unwrap(),
            "1.17.1"
        );
        assert!(verify_version("PPSSPP v1.17.0\n", "v1.17.1").is_err());
        assert!(verify_version("not a version", "v1.17.1").is_err());
    }

    #[test]
    fn changed_configuration_evidence_is_stale() {
        let reviewed = PpssppInspection {
            profile_ready: true,
            firmware_required: false,
            configuration_paths: vec![PathBuf::from("/tmp/ppsspp.ini")],
            policy_fingerprint: "a".into(),
            checks: vec![],
        };
        let mut fresh = reviewed.clone();
        assert!(reject_stale_evidence(&reviewed, &fresh).is_ok());
        fresh.policy_fingerprint = "b".into();
        assert!(reject_stale_evidence(&reviewed, &fresh).is_err());
    }

    #[test]
    fn firmware_is_never_required() {
        assert!(!PpssppInspection::FIRMWARE_REQUIRED);
        let inspection = PpssppInspection {
            profile_ready: true,
            firmware_required: false,
            configuration_paths: vec![],
            policy_fingerprint: "test".into(),
            checks: vec![],
        };
        assert!(inspection.ready());
    }

    #[test]
    fn first_run_requires_both_known_configuration_locations_to_be_absent() {
        let roots = PpssppProfileDiscoveryRoots {
            home: PathBuf::from("/tmp/home"),
            xdg_config_home: PathBuf::from("/tmp/config"),
            xdg_data_home: PathBuf::from("/tmp/data"),
            explicit_configuration_roots: vec![],
            portable_configuration_roots: vec![],
            explicit_executables: vec![],
            known_version_outputs: Default::default(),
            appimage_directory: None,
        };
        assert!(first_run_required(&roots));
    }

    #[test]
    fn policy_rejects_pcsx2_context() {
        let context = BootstrapContext {
            target: BootstrapTarget::Pcsx2,
            root: PathBuf::from("/tmp/emuwiz"),
            destination: PathBuf::from("/tmp/emuwiz/pcsx2.AppImage"),
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
        let roots = PpssppProfileDiscoveryRoots {
            home: PathBuf::from("/tmp/home"),
            xdg_config_home: PathBuf::from("/tmp/config"),
            xdg_data_home: PathBuf::from("/tmp/data"),
            explicit_configuration_roots: vec![],
            portable_configuration_roots: vec![],
            explicit_executables: vec![],
            known_version_outputs: Default::default(),
            appimage_directory: None,
        };
        assert!(inspect(&context, &roots).is_err());
    }
}
