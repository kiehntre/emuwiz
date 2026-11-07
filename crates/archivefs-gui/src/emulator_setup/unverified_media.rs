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

use std::io::Read;
use std::path::Path;

use archivefs_core::launch::{
    LaunchPlan, LaunchReadiness, LaunchWarning, LaunchWarningKind, ResolvedIdentity,
};

/// Tape images are small; anything larger is not inspected here.
const MAX_TAPE_BYTES: u64 = 32 * 1024 * 1024;

/// The only platform and extensions this path accepts.
const ZX_PLATFORM: &str = "ZX Spectrum";

pub(crate) const WARNING_TEXT: &str = "Game not identified: EmuWiz confirmed this is a valid ZX Spectrum tape image but could not confirm which game it is. It will still play; catalogue (DAT) verification is optional.";

/// Resolves an unverified-but-safe launch identity, or `None` (stay blocked).
pub(crate) fn resolve(path: &Path, platform_hint: &str) -> Option<ResolvedIdentity> {
    if archivefs_core::canonical_platform_for_alias(platform_hint) != Some(ZX_PLATFORM) {
        return None;
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    inspect_tape(path, &extension).ok()?;
    let stem = path.file_stem()?.to_str()?;
    let key: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    Some(ResolvedIdentity {
        platform_id: ZX_PLATFORM.to_string(),
        game_key: format!("unverified:{key}"),
    })
}

/// Adds the visible warning to every usable candidate (never removes a
/// blocker, never touches a blocked candidate) and refreshes the summary.
pub(crate) fn apply_warning(plan: &mut LaunchPlan) {
    for candidate in &mut plan.candidates {
        if candidate.readiness == LaunchReadiness::Blocked {
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

/// Reads the whole tape (bounded) and validates its container structure.
pub(crate) fn inspect_tape(path: &Path, extension: &str) -> Result<(), String> {
    if !matches!(extension, "tap" | "tzx") {
        return Err("not a tape image extension".into());
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("not a regular file".into());
    }
    if metadata.len() == 0 || metadata.len() > MAX_TAPE_BYTES {
        return Err("tape image size is outside the inspected range".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_TAPE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    match extension {
        "tzx" => validate_tzx(&bytes),
        _ => validate_tap(&bytes),
    }
}

/// `ZXTape!` + 0x1A + major 1, followed by at least one block byte.
fn validate_tzx(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 11 || &bytes[..7] != b"ZXTape!" || bytes[7] != 0x1A {
        return Err("missing TZX signature".into());
    }
    if bytes[8] != 1 {
        return Err("unsupported TZX major version".into());
    }
    Ok(())
}

/// A TAP is a chain of `u16 length` + data blocks that must end exactly at
/// the end of the file, each block holding at least a flag and a checksum.
fn validate_tap(bytes: &[u8]) -> Result<(), String> {
    let mut position = 0usize;
    let mut blocks = 0usize;
    while position < bytes.len() {
        let length = bytes
            .get(position..position + 2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
            .ok_or("truncated block header")?;
        if length < 2 {
            return Err("block too short".into());
        }
        position += 2;
        if position + length > bytes.len() {
            return Err("block runs past the end of the file".into());
        }
        position += length;
        blocks += 1;
    }
    if blocks == 0 {
        return Err("no blocks".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(blocks: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for block in blocks {
            out.extend_from_slice(&(block.len() as u16).to_le_bytes());
            out.extend_from_slice(block);
        }
        out
    }

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_well_formed_tap_resolves_an_unverified_key_for_zx_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "Manic Miner (1983).tap",
            &tap(&[&[0, 1, 2, 3], &[0xFF, 9, 9]]),
        );
        let resolved = resolve(&path, "ZX Spectrum").expect("valid tape resolves");
        assert_eq!(resolved.platform_id, "ZX Spectrum");
        assert_eq!(resolved.game_key, "unverified:manic-miner--1983-");
        // Never for another platform, whatever the extension.
        assert!(resolve(&path, "Commodore 64").is_none());
    }

    #[test]
    fn malformed_or_unsupported_media_stays_blocked() {
        let dir = tempfile::tempdir().unwrap();
        // Block length points past the end of the file.
        let truncated = write(&dir, "a.tap", &[0x10, 0x00, 1, 2, 3]);
        assert!(resolve(&truncated, "ZX Spectrum").is_none());
        // Trailing garbage that is not a whole block.
        let trailing = write(&dir, "b.tap", &[2, 0, 1, 2, 7]);
        assert!(resolve(&trailing, "ZX Spectrum").is_none());
        // Empty and one-byte blocks.
        assert!(resolve(&write(&dir, "c.tap", &[]), "ZX Spectrum").is_none());
        assert!(resolve(&write(&dir, "d.tap", &tap(&[&[1]])), "ZX Spectrum").is_none());
        // Wrong extension, even if the bytes are a valid tape.
        let wrong = write(&dir, "e.z80", &tap(&[&[0, 1]]));
        assert!(resolve(&wrong, "ZX Spectrum").is_none());
        // TZX without its signature or with the wrong major version.
        assert!(
            resolve(
                &write(&dir, "f.tzx", b"not a tzx file at all"),
                "ZX Spectrum"
            )
            .is_none()
        );
        let mut bad_major = b"ZXTape!\x1a".to_vec();
        bad_major.extend_from_slice(&[2, 0, 0x10]);
        assert!(resolve(&write(&dir, "g.tzx", &bad_major), "ZX Spectrum").is_none());
    }

    #[test]
    fn a_valid_tzx_resolves_and_a_symlink_never_does() {
        let dir = tempfile::tempdir().unwrap();
        let mut tzx = b"ZXTape!\x1a".to_vec();
        tzx.extend_from_slice(&[1, 20, 0x10, 0, 0]);
        let path = write(&dir, "Jetpac.tzx", &tzx);
        assert!(resolve(&path, "ZX Spectrum").is_some());
        let link = dir.path().join("link.tzx");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(resolve(&link, "ZX Spectrum").is_none());
    }
}
