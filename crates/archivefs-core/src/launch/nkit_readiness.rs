//! Existing NKit marker evidence limits the supported GameCube/Wii launch contract.
//! This neither verifies a disc nor asserts that Dolphin itself cannot read NKit.
use super::{LaunchBlocker, LaunchBlockerKind, LaunchPlan, LaunchPlanSummary, LaunchReadiness};
use crate::gamecube_wii_boot_evidence::nkit::{NkitMarkerProbe, probe_marker};
use std::path::Path;

pub const NKIT_LAUNCH_REFUSAL: &str = "NKit launch is unavailable in EmuWiz: header recognition does not establish supported Dolphin compatibility, a complete disc or recoverability. No conversion or recovery action is implemented.";

/// Only canonical GameCube/Wii content is probed. No title, DAT or game-key
/// evidence is upgraded or erased by this representation check.
pub fn check_raw_nkit_launch(platform: &str, path: &Path) -> Result<(), String> {
    if !matches!(platform, "GameCube" | "Wii") {
        return Ok(());
    }
    match probe_marker(path) {
        Ok(NkitMarkerProbe::RawMarker(true)) => Err(NKIT_LAUNCH_REFUSAL.into()),
        Ok(NkitMarkerProbe::RawMarker(false)) => Ok(()),
        // Preserve existing wrapper identity/extension/preflight policy. This
        // says nothing about inner NKit presence, compatibility or preservation.
        Ok(NkitMarkerProbe::WrappedInnerUninspected(_)) => Ok(()),
        Err(error) => Err(format!(
            "Disc representation could not be safely checked for NKit; launch remains unavailable. {error}"
        )),
    }
}

/// Post-inspection composition seam, outside the pure launch planner. Each
/// candidate's own selected path is checked; native and libretro stay aligned.
pub fn inspect_and_apply(plan: &mut LaunchPlan) {
    let Some(platform) = plan
        .platform_id
        .as_deref()
        .filter(|id| matches!(*id, "GameCube" | "Wii"))
    else {
        return;
    };
    for candidate in &mut plan.candidates {
        let Some(path) = candidate.content.resolved_path.as_deref() else {
            continue;
        };
        if let Err(detail) = check_raw_nkit_launch(platform, path) {
            let blocker =
                LaunchBlocker::new(LaunchBlockerKind::DolphinContentFormatUnsupported, detail);
            if !candidate.blockers.contains(&blocker) {
                candidate.blockers.insert(0, blocker);
            }
            candidate.readiness = LaunchReadiness::Blocked;
        }
    }
    plan.summary = LaunchPlanSummary {
        candidates: plan.candidates.len(),
        ready: plan
            .candidates
            .iter()
            .filter(|c| c.readiness == LaunchReadiness::Ready)
            .count(),
        ready_with_warnings: plan
            .candidates
            .iter()
            .filter(|c| c.readiness == LaunchReadiness::ReadyWithWarnings)
            .count(),
        blocked: plan
            .candidates
            .iter()
            .filter(|c| c.readiness == LaunchReadiness::Blocked)
            .count(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
    use crate::launch::{
        CanonicalIdentityStatus, FirmwareReadiness, LaunchContainerKind, LaunchContentRef,
        ResolvedIdentity, StandaloneProfileInput, build_launch_plan,
    };
    use std::fs;

    fn plan(path: &Path, platform: &str) -> LaunchPlan {
        build_launch_plan(
            &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: platform.into(),
                game_key: "GTSE01".into(),
            }),
            &LaunchContentRef {
                kind: None,
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(path.into()),
                requires_mount: false,
                provenance: "synthetic".into(),
            },
            &[StandaloneProfileInput {
                adapter_id: "dolphin",
                profile_id: "fixture".into(),
                profile_path: None,
                eligible: true,
                firmware: FirmwareReadiness::NotRequired,
            }],
            &RetroArchEnvironmentReport {
                format_version: 1,
                profiles: Vec::new(),
                diagnostics: Vec::new(),
            },
            &[],
        )
    }
    #[test]
    fn marked_versions_block_readiness_without_erasing_identity_or_changing_source() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ordinary.iso");
        for version in [b" v01", b" v99"] {
            let mut bytes = vec![0u8; 0x440];
            bytes[0x200..0x204].copy_from_slice(b"NKIT");
            bytes[0x204..0x208].copy_from_slice(version);
            fs::write(&path, &bytes).unwrap();
            for platform in ["GameCube", "Wii"] {
                let mut plan = plan(&path, platform);
                assert_eq!(plan.summary.ready, 1);
                inspect_and_apply(&mut plan);
                assert_eq!(plan.summary.ready, 0);
                assert_eq!(plan.summary.blocked, 1);
                assert_eq!(plan.game_key.as_deref(), Some("GTSE01"));
                assert!(plan.candidates[0].blockers[0].detail.contains("NKit"));
                inspect_and_apply(&mut plan);
                assert_eq!(plan.candidates[0].blockers.len(), 1);
            }
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn filenames_and_unrelated_platforms_never_establish_nkit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ordinary.nkit.iso");
        fs::write(&path, vec![0u8; 0x440]).unwrap();
        let mut ordinary = plan(&path, "GameCube");
        inspect_and_apply(&mut ordinary);
        assert_eq!(ordinary.summary.ready, 1);
        let mut bytes = vec![0u8; 0x440];
        bytes[0x200..0x204].copy_from_slice(b"NKIT");
        fs::write(&path, bytes).unwrap();
        assert!(check_raw_nkit_launch("NES", &path).is_ok());
        assert!(check_raw_nkit_launch("GameCube", &path).is_err());
    }
    #[test]
    fn unavailable_short_and_replaced_raw_sources_cannot_reuse_ready() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disc.iso");
        assert!(check_raw_nkit_launch("GameCube", &path).is_err());
        fs::write(&path, b"short").unwrap();
        assert!(check_raw_nkit_launch("GameCube", &path).is_err());
        fs::write(&path, [1, 0xc0, 0x0b, 0xb1]).unwrap();
        assert!(matches!(
            probe_marker(&path),
            Ok(NkitMarkerProbe::WrappedInnerUninspected(_))
        ));
        fs::write(&path, vec![0u8; 0x440]).unwrap();
        assert!(check_raw_nkit_launch("GameCube", &path).is_ok());
        let mut bytes = vec![0u8; 0x440];
        bytes[0x200..0x204].copy_from_slice(b"NKIT");
        fs::write(&path, bytes).unwrap();
        assert!(check_raw_nkit_launch("GameCube", &path).is_err());
    }
}
