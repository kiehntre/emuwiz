//! Controlled "identity unverified" launch for systems with no header identity.
//!
//! ZX Spectrum tape images (`.tap`, `.tzx`) carry no game identity of their
//! own, and optional DAT matching is not a launch requirement. Owner decision
//! (2026-11-07): such media may launch through RetroArch with a visible
//! [`LaunchWarningKind::IdentityUnverified`] warning, but only when the file is
//! a recognised, structurally valid tape image that was safely inspected here.
//!
//! This never produces a *verified* identity: the key is prefixed
//! `unverified:` and every candidate carries the warning. Any file that fails
//! inspection falls back to the existing "identity needs review" block.

use std::path::Path;

use archivefs_core::launch::{
    LaunchPlan, LaunchReadiness, LaunchWarning, LaunchWarningKind, ResolvedIdentity,
};

use archivefs_core::launch::unverified_media::ZX_PLATFORM;

pub(crate) const WARNING_TEXT: &str = "Game not identified: EmuWiz confirmed this is a valid ZX Spectrum tape image but could not confirm which game it is. It will still play; catalogue (DAT) verification is optional.";

/// Resolves an unverified-but-safe launch identity, or `None` (stay blocked).
pub(crate) fn resolve(path: &Path, platform_hint: &str) -> Option<ResolvedIdentity> {
    if archivefs_core::canonical_platform_for_alias(platform_hint) != Some(ZX_PLATFORM) {
        return None;
    }
    archivefs_core::launch::unverified_media::zx_tape_identity(path)
}

/// Adds the visible warning to every usable candidate (never removes a
/// blocker, never touches a blocked candidate) and refreshes the summary.
pub(crate) fn apply_warning(plan: &mut LaunchPlan) {
    for candidate in &mut plan.candidates {
        // Only the RetroArch route was approved; standalone routes keep their
        // own (stricter) rules and never receive the warning.
        if candidate.readiness == LaunchReadiness::Blocked
            || !matches!(
                candidate.target,
                archivefs_core::launch::LaunchTarget::RetroArchCore { .. }
            )
            || candidate
                .warnings
                .iter()
                .any(|w| w.kind == LaunchWarningKind::IdentityUnverified)
        {
            continue;
        }
        candidate.warnings.push(LaunchWarning::new(
            LaunchWarningKind::IdentityUnverified,
            WARNING_TEXT,
        ));
        if candidate.readiness == LaunchReadiness::Ready {
            candidate.readiness = LaunchReadiness::ReadyWithWarnings;
        }
    }
    let count = |r: LaunchReadiness| plan.candidates.iter().filter(|c| c.readiness == r).count();
    plan.summary.ready = count(LaunchReadiness::Ready);
    plan.summary.ready_with_warnings = count(LaunchReadiness::ReadyWithWarnings);
    plan.summary.blocked = count(LaunchReadiness::Blocked);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_zx_media_resolves_through_the_platform_gate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Jetpac.tap");
        std::fs::write(&path, [2u8, 0, 0, 1]).unwrap();
        assert!(resolve(&path, "ZX Spectrum").is_some());
        assert!(resolve(&path, "Commodore 64").is_none());
    }
}
