//! Launch-only identity for media with no header identity (ZX Spectrum tapes).
//!
//! Owner decision (2026-11-07): a recognised, structurally valid ZX `.tap` or
//! `.tzx` may launch through RetroArch with a visible "identity unverified"
//! warning, without DAT matching. The key is prefixed [`UNVERIFIED_KEY_PREFIX`]
//! so it can never be mistaken for a verified identity, and both readiness and
//! the click-time preflight re-derive it from the file itself.

use std::io::Read;
use std::path::Path;

use crate::launch::planning::ResolvedIdentity;

/// Marks a launch-only identity that was never verified.
pub const UNVERIFIED_KEY_PREFIX: &str = "unverified:";

/// Tape images are small; anything larger is not inspected here.
const MAX_TAPE_BYTES: u64 = 32 * 1024 * 1024;

const ZX_PLATFORM: &str = "ZX Spectrum";

pub fn is_unverified_key(game_key: &str) -> bool {
    game_key.starts_with(UNVERIFIED_KEY_PREFIX)
}

/// Resolves the unverified launch identity for a valid ZX tape at `path`, or
/// `None` (stay blocked). The platform must already have been confirmed as
/// ZX Spectrum by the caller.
pub fn zx_tape_identity(path: &Path) -> Option<ResolvedIdentity> {
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
        game_key: format!("{UNVERIFIED_KEY_PREFIX}{key}"),
    })
}

/// Reads the whole tape (bounded) and validates its container structure.
pub fn inspect_tape(path: &Path, extension: &str) -> Result<(), String> {
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
    fn a_well_formed_tap_resolves_an_unverified_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "Manic Miner (1983).tap",
            &tap(&[&[0, 1, 2, 3], &[0xFF, 9, 9]]),
        );
        let resolved = zx_tape_identity(&path).expect("valid tape resolves");
        assert_eq!(resolved.platform_id, "ZX Spectrum");
        assert_eq!(resolved.game_key, "unverified:manic-miner--1983-");
        assert!(is_unverified_key(&resolved.game_key));
        assert!(!is_unverified_key("manic-miner"));
    }

    #[test]
    fn malformed_or_unsupported_media_stays_blocked() {
        let dir = tempfile::tempdir().unwrap();
        assert!(zx_tape_identity(&write(&dir, "a.tap", &[0x10, 0x00, 1, 2, 3])).is_none());
        assert!(zx_tape_identity(&write(&dir, "b.tap", &[2, 0, 1, 2, 7])).is_none());
        assert!(zx_tape_identity(&write(&dir, "c.tap", &[])).is_none());
        assert!(zx_tape_identity(&write(&dir, "d.tap", &tap(&[&[1]]))).is_none());
        assert!(zx_tape_identity(&write(&dir, "e.z80", &tap(&[&[0, 1]]))).is_none());
        assert!(zx_tape_identity(&write(&dir, "f.tzx", b"not a tzx file at all")).is_none());
        let mut bad_major = b"ZXTape!\x1a".to_vec();
        bad_major.extend_from_slice(&[2, 0, 0x10]);
        assert!(zx_tape_identity(&write(&dir, "g.tzx", &bad_major)).is_none());
    }

    #[test]
    fn a_valid_tzx_resolves_and_a_symlink_never_does() {
        let dir = tempfile::tempdir().unwrap();
        let mut tzx = b"ZXTape!\x1a".to_vec();
        tzx.extend_from_slice(&[1, 20, 0x10, 0, 0]);
        let path = write(&dir, "Jetpac.tzx", &tzx);
        assert!(zx_tape_identity(&path).is_some());
        let link = dir.path().join("link.tzx");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(zx_tape_identity(&link).is_none());
    }
}
