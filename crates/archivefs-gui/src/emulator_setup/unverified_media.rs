//! Controlled "identity unverified" launch for games whose system is known
//! but whose exact game is not.
//!
//! Owner decisions: ZX Spectrum tapes (2026-11-07), generalised on 2026-11-09
//! to every single-file game whose system is already established (the
//! library's stored platform, which includes the person's own system choice),
//! because catalogue (DAT) identity must not gate a safe launch. The file must
//! be a format that system lists, and nothing in the file's own content may
//! point to another system. Media, archive, firmware and click-time safety
//! checks are unchanged.
//!
//! This never produces a *verified* identity: the key is prefixed
//! `unverified:` and every candidate carries the visible warning. Anything
//! that fails falls back to the existing "identity needs review" block.

use std::path::Path;

use archivefs_core::game_identity::IdentityPlatform;
use archivefs_core::launch::{
    LaunchPlan, LaunchReadiness, LaunchWarning, LaunchWarningKind, ResolvedIdentity,
};

/// The warning every candidate of an unverified launch carries.
pub(crate) fn warning_text(platform_id: &str) -> String {
    let name = archivefs_core::platform::platform_by_id(platform_id)
        .map_or(platform_id, |p| p.display_name);
    format!(
        "Game not identified: EmuWiz confirmed this is a {name} file it can start, but could not confirm which game it is. It will still play; catalogue (DAT) verification is optional."
    )
}

/// Resolves an unverified-but-safe launch identity, or `None` (stay blocked).
/// `content_platform` is what the file's own content says, if anything; a
/// different system there is a conflict and is never overridden.
pub(crate) fn resolve(
    path: &Path,
    platform_hint: &str,
    content_platform: Option<IdentityPlatform>,
) -> Option<ResolvedIdentity> {
    let platform = archivefs_core::canonical_platform_for_alias(platform_hint)?;
    if let Some(content) = content_platform
        && content != IdentityPlatform::Other
        && archivefs_core::launch::evidence_bridge::launch_platform_id(content)
            .is_none_or(|c| c != platform)
    {
        return None;
    }
    archivefs_core::launch::unverified_media::platform_media_identity(path, platform)
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
        let text = warning_text(plan.platform_id.as_deref().unwrap_or_default());
        candidate.warnings.push(LaunchWarning::new(
            LaunchWarningKind::IdentityUnverified,
            text,
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
    fn only_media_of_the_established_platform_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Jetpac.tap");
        std::fs::write(&path, [2u8, 0, 0, 1]).unwrap();
        assert!(resolve(&path, "ZX Spectrum", None).is_some());
        let tape = dir.path().join("Jetpac.tzx");
        std::fs::write(&tape, b"ZXTape!\x1a\x01\x14\x20\x00\x00").unwrap();
        assert!(resolve(&tape, "ZX Spectrum", None).is_some());
        assert!(resolve(&tape, "Commodore 64", None).is_none());
    }

    #[test]
    fn content_pointing_to_another_system_is_never_overridden() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Game.sfc");
        std::fs::write(&path, [1u8; 64]).unwrap();
        assert!(resolve(&path, "SNES", None).is_some());
        assert!(resolve(&path, "SNES", Some(IdentityPlatform::Other)).is_some());
        assert!(resolve(&path, "SNES", Some(IdentityPlatform::PlayStation2)).is_none());
        assert!(resolve(&path, "a folder name that is no system", None).is_none());
    }
}
