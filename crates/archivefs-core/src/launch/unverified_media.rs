//! Launch-only identity for media with no header identity (ZX Spectrum tapes).
//!
//! Owner decision (2026-11-07): a recognised, structurally valid ZX `.tap` or
//! `.tzx` may launch through RetroArch with a visible "identity unverified"
//! warning, without DAT matching. The key is prefixed [`UNVERIFIED_KEY_PREFIX`]
//! so it can never be mistaken for a verified identity, is bound to the file's
//! content (SHA-256), and both readiness and the click-time preflight and spawn
//! step re-derive it from the file itself (the latter two bypass the cache).

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use sha2::{Digest, Sha256};

use crate::launch::planning::ResolvedIdentity;

/// Marks a launch-only identity that was never verified.
pub const UNVERIFIED_KEY_PREFIX: &str = "unverified:";

/// The only platform this path accepts.
pub const ZX_PLATFORM: &str = "ZX Spectrum";

/// Tape images are small; anything larger is not inspected here.
const MAX_TAPE_BYTES: u64 = 16 * 1024 * 1024;
const CACHE_LIMIT: usize = 256;

type CacheKey = (PathBuf, u64, u128);

fn cache() -> &'static Mutex<HashMap<CacheKey, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn is_unverified_key(game_key: &str) -> bool {
    game_key.starts_with(UNVERIFIED_KEY_PREFIX)
}

/// Readiness-time identity, cached by (path, size, mtime) so rebuilding the
/// plan never re-reads the file. Preflight and spawn use [`zx_tape_identity_fresh`].
pub fn zx_tape_identity(path: &Path) -> Option<ResolvedIdentity> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let key = (path.to_path_buf(), metadata.len(), mtime);
    if let Ok(guard) = cache().lock()
        && let Some(hit) = guard.get(&key)
    {
        return hit.clone().map(identity_for_key);
    }
    let derived = derive_key(path);
    if let Ok(mut guard) = cache().lock() {
        if guard.len() >= CACHE_LIMIT {
            guard.clear();
        }
        guard.insert(key, derived.clone());
    }
    derived.map(identity_for_key)
}

/// Always reads and validates the file now (click-time preflight and spawn).
pub fn zx_tape_identity_fresh(path: &Path) -> Option<ResolvedIdentity> {
    derive_key(path).map(identity_for_key)
}

fn identity_for_key(game_key: String) -> ResolvedIdentity {
    ResolvedIdentity {
        platform_id: ZX_PLATFORM.to_string(),
        game_key,
    }
}

fn derive_key(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let bytes = read_tape(path, &extension).ok()?;
    let stem: String = path
        .file_stem()?
        .to_str()?
        .chars()
        .take(48)
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let digest = Sha256::digest(&bytes);
    let short: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    Some(format!("{UNVERIFIED_KEY_PREFIX}{stem}:{short}"))
}

/// Opens without following a final symlink (and without blocking on a FIFO),
/// checks the *opened* handle is a regular file within the size cap, reads it
/// and validates the container structure.
pub fn read_tape(path: &Path, extension: &str) -> Result<Vec<u8>, String> {
    if !matches!(extension, "tap" | "tzx") {
        return Err("not a tape image extension".into());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("not a regular file".into());
    }
    if metadata.len() == 0 || metadata.len() > MAX_TAPE_BYTES {
        return Err("tape image size is outside the inspected range".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_TAPE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    match extension {
        "tzx" => validate_tzx(&bytes)?,
        _ => validate_tap(&bytes)?,
    }
    Ok(bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_le_bytes([
        *bytes.get(at)?,
        *bytes.get(at + 1)?,
    ])))
}

fn u24_at(bytes: &[u8], at: usize) -> Option<usize> {
    Some(
        usize::from(*bytes.get(at)?)
            | (usize::from(*bytes.get(at + 1)?) << 8)
            | (usize::from(*bytes.get(at + 2)?) << 16),
    )
}

fn u32_at(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes([
        *bytes.get(at)?,
        *bytes.get(at + 1)?,
        *bytes.get(at + 2)?,
        *bytes.get(at + 3)?,
    ]) as usize)
}

/// Total length (id byte included) of the standard TZX block starting at
/// `at`, or `None` for an unknown id or a truncated header.
fn tzx_block_len(bytes: &[u8], at: usize) -> Option<usize> {
    let id = *bytes.get(at)?;
    let p = at + 1;
    Some(match id {
        0x10 => 5 + u16_at(bytes, p + 2)?,
        0x11 => 19 + u24_at(bytes, p + 15)?,
        0x12 => 5,
        0x13 => 2 + 2 * usize::from(*bytes.get(p)?),
        0x14 => 11 + u24_at(bytes, p + 7)?,
        0x15 => 9 + u24_at(bytes, p + 5)?,
        0x18 | 0x19 => 5 + u32_at(bytes, p)?,
        0x20 | 0x23 | 0x24 => 3,
        0x21 | 0x30 => 2 + usize::from(*bytes.get(p)?),
        0x22 | 0x25 | 0x27 => 1,
        0x26 => 3 + 2 * u16_at(bytes, p)?,
        0x28 | 0x32 => 3 + u16_at(bytes, p)?,
        0x2A => 5,
        0x2B => 6,
        0x31 => 3 + usize::from(*bytes.get(p + 1)?),
        0x33 => 2 + 3 * usize::from(*bytes.get(p)?),
        0x35 => 21 + u32_at(bytes, p + 16)?,
        0x4B => 5 + u32_at(bytes, p)?,
        0x5A => 10,
        _ => return None,
    })
}

/// `ZXTape!` + 0x1A + major 1, then a chain of standard blocks that ends
/// exactly at the end of the file.
fn validate_tzx(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 11 || &bytes[..7] != b"ZXTape!" || bytes[7] != 0x1A {
        return Err("missing TZX signature".into());
    }
    if bytes[8] != 1 {
        return Err("unsupported TZX major version".into());
    }
    let mut position = 10;
    let mut blocks = 0usize;
    while position < bytes.len() {
        let length = tzx_block_len(bytes, position).ok_or("unknown or truncated TZX block")?;
        position += length;
        if position > bytes.len() {
            return Err("TZX block runs past the end of the file".into());
        }
        blocks += 1;
    }
    if blocks == 0 {
        return Err("no TZX blocks".into());
    }
    Ok(())
}

/// A TAP is a chain of `u16 length` + data blocks that must end exactly at
/// the end of the file, each block holding at least a flag and a checksum.
fn validate_tap(bytes: &[u8]) -> Result<(), String> {
    let mut position = 0usize;
    let mut blocks = 0usize;
    while position < bytes.len() {
        let length = u16_at(bytes, position).ok_or("truncated block header")?;
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

/// How many bytes from each end of a large file the content binding reads.
const EDGE_BYTES: u64 = 1024 * 1024;

/// Launch-only identity for a single-file game whose system is established
/// but whose exact game is not (owner, 2026-11-09, generalising the ZX tape
/// decision: catalogue identity must not gate a safe launch). `platform_id`
/// must be the canonical platform the library and any manual choice already
/// give the file; this function never infers one. It accepts the file only
/// when it is a non-empty regular file (opened without following a final
/// symlink or blocking on a FIFO) whose extension that platform's registry
/// entry lists as one of its formats. ZX tapes keep their stricter structural
/// validation. The key is `unverified:`-prefixed and bound to the content:
/// whole-file SHA-256 up to 16 MiB, otherwise size plus the first and last
/// MiB. Never a verified identity; every candidate built from it carries the
/// visible identity-unverified warning.
pub fn platform_media_identity(path: &Path, platform_id: &str) -> Option<ResolvedIdentity> {
    if platform_id == ZX_PLATFORM
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "tap" | "tzx"))
    {
        return zx_tape_identity(path);
    }
    cached(path, platform_id)
}

/// The same answer read from the file now (click-time preflight and spawn).
pub fn platform_media_identity_fresh(path: &Path, platform_id: &str) -> Option<ResolvedIdentity> {
    if platform_id == ZX_PLATFORM
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "tap" | "tzx"))
    {
        return zx_tape_identity_fresh(path);
    }
    platform_media_key(path, platform_id).map(|game_key| ResolvedIdentity {
        platform_id: platform_id.to_string(),
        game_key,
    })
}

fn cached(path: &Path, platform_id: &str) -> Option<ResolvedIdentity> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    // The platform is part of the key: the same file under another system
    // is a different question.
    let mut keyed = path.as_os_str().to_owned();
    keyed.push("\0");
    keyed.push(platform_id);
    let key = (PathBuf::from(keyed), metadata.len(), mtime);
    if let Ok(guard) = cache().lock()
        && let Some(hit) = guard.get(&key)
    {
        return hit.clone().map(|game_key| ResolvedIdentity {
            platform_id: platform_id.to_string(),
            game_key,
        });
    }
    let derived = platform_media_key(path, platform_id);
    if let Ok(mut guard) = cache().lock() {
        if guard.len() >= CACHE_LIMIT {
            guard.clear();
        }
        guard.insert(key, derived.clone());
    }
    derived.map(|game_key| ResolvedIdentity {
        platform_id: platform_id.to_string(),
        game_key,
    })
}

/// Whether `extension` is one of the formats the platform registry lists.
pub fn platform_lists_extension(platform_id: &str, extension: &str) -> bool {
    let Some(platform) = crate::platform::platform_by_id(platform_id) else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    platform
        .strong_extensions
        .iter()
        .chain(platform.weak_extensions.iter())
        .any(|listed| listed.eq_ignore_ascii_case(&extension))
}

fn platform_media_key(path: &Path, platform_id: &str) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let extension = path.extension()?.to_str()?;
    if !platform_lists_extension(platform_id, extension) {
        return None;
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return None;
    }
    let size = metadata.len();
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    if size <= MAX_TAPE_BYTES {
        let mut bytes = Vec::with_capacity(size as usize);
        (&mut file)
            .take(MAX_TAPE_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 != size {
            return None;
        }
        hasher.update(&bytes);
    } else {
        let mut head = vec![0u8; EDGE_BYTES as usize];
        file.read_exact(&mut head).ok()?;
        hasher.update(&head);
        file.seek(SeekFrom::Start(size - EDGE_BYTES)).ok()?;
        let mut tail = vec![0u8; EDGE_BYTES as usize];
        file.read_exact(&mut tail).ok()?;
        hasher.update(&tail);
    }
    let stem: String = path
        .file_stem()?
        .to_str()?
        .chars()
        .take(48)
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let digest = hasher.finalize();
    let short: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    Some(format!("{UNVERIFIED_KEY_PREFIX}{stem}:{short}"))
}

#[cfg(test)]
mod platform_media_tests {
    use super::*;

    #[test]
    fn a_listed_format_resolves_an_unverified_content_bound_key_for_that_platform_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bokosuka Wars (Japan).sfc");
        std::fs::write(&path, vec![7u8; 4096]).unwrap();
        let resolved = platform_media_identity_fresh(&path, "SNES").expect("listed format");
        assert_eq!(resolved.platform_id, "SNES");
        assert!(is_unverified_key(&resolved.game_key));
        // Not a format that platform lists.
        assert!(platform_media_identity_fresh(&path, "PS2").is_none());
        // Unknown platform id: never inferred.
        assert!(platform_media_identity_fresh(&path, "Not A System").is_none());
        // Cached and fresh answers agree.
        assert_eq!(
            platform_media_identity(&path, "SNES").unwrap().game_key,
            resolved.game_key
        );
    }

    #[test]
    fn the_key_follows_content_and_refuses_empty_files_directories_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.sfc");
        std::fs::write(&path, [1u8, 2, 3]).unwrap();
        let before = platform_media_identity_fresh(&path, "SNES")
            .unwrap()
            .game_key;
        std::fs::write(&path, [1u8, 2, 4]).unwrap();
        let after = platform_media_identity_fresh(&path, "SNES")
            .unwrap()
            .game_key;
        assert_ne!(before, after);

        let empty = dir.path().join("empty.sfc");
        std::fs::write(&empty, []).unwrap();
        assert!(platform_media_identity_fresh(&empty, "SNES").is_none());

        let folder = dir.path().join("folder.sfc");
        std::fs::create_dir(&folder).unwrap();
        assert!(platform_media_identity_fresh(&folder, "SNES").is_none());

        let link = dir.path().join("link.sfc");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(platform_media_identity_fresh(&link, "SNES").is_none());
    }

    #[test]
    fn a_large_file_is_bound_by_size_and_both_ends_without_reading_it_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disc.iso");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(40 * 1024 * 1024).unwrap();
        let resolved = platform_media_identity_fresh(&path, "PS2").expect("iso is a PS2 format");
        assert!(is_unverified_key(&resolved.game_key));
    }

    #[test]
    fn zx_tapes_keep_their_structural_validation() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.tap");
        std::fs::write(&bad, [0u8, 0, 0]).unwrap();
        assert!(platform_media_identity_fresh(&bad, ZX_PLATFORM).is_none());
    }
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

    fn tzx(blocks: &[&[u8]]) -> Vec<u8> {
        let mut out = b"ZXTape!\x1a\x01\x14".to_vec();
        for block in blocks {
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
    fn a_well_formed_tap_resolves_a_content_bound_unverified_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "Manic Miner (1983).tap",
            &tap(&[&[0, 1, 2, 3], &[0xFF, 9, 9]]),
        );
        let resolved = zx_tape_identity_fresh(&path).expect("valid tape resolves");
        assert_eq!(resolved.platform_id, "ZX Spectrum");
        assert!(
            resolved
                .game_key
                .starts_with("unverified:manic-miner--1983-:")
        );
        assert!(is_unverified_key(&resolved.game_key));
        assert!(!is_unverified_key("manic-miner"));
    }

    #[test]
    fn the_key_changes_when_the_file_content_changes_and_stems_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let first = write(&dir, "a.tap", &tap(&[&[0, 1, 2, 3]]));
        let key_before = zx_tape_identity_fresh(&first).unwrap().game_key;
        std::fs::write(&first, tap(&[&[0, 1, 2, 4]])).unwrap();
        let key_after = zx_tape_identity_fresh(&first).unwrap().game_key;
        assert_ne!(key_before, key_after);
        // Non-ASCII stems sanitise identically but stay distinct by content.
        let jp1 = write(&dir, "ゲーム.tap", &tap(&[&[0, 1, 2, 3]]));
        let jp2 = write(&dir, "ありがとう.tap", &tap(&[&[0, 7, 7, 7]]));
        assert_ne!(
            zx_tape_identity_fresh(&jp1).unwrap().game_key,
            zx_tape_identity_fresh(&jp2).unwrap().game_key
        );
    }

    #[test]
    fn malformed_or_unsupported_media_stays_blocked() {
        let dir = tempfile::tempdir().unwrap();
        assert!(zx_tape_identity_fresh(&write(&dir, "a.tap", &[0x10, 0x00, 1, 2, 3])).is_none());
        assert!(zx_tape_identity_fresh(&write(&dir, "b.tap", &[2, 0, 1, 2, 7])).is_none());
        assert!(zx_tape_identity_fresh(&write(&dir, "c.tap", &[])).is_none());
        assert!(zx_tape_identity_fresh(&write(&dir, "d.tap", &tap(&[&[1]]))).is_none());
        assert!(zx_tape_identity_fresh(&write(&dir, "e.z80", &tap(&[&[0, 1]]))).is_none());
        assert!(zx_tape_identity_fresh(&write(&dir, "f.tzx", b"not a tzx file at all")).is_none());
        let mut bad_major = b"ZXTape!\x1a".to_vec();
        bad_major.extend_from_slice(&[2, 0, 0x10]);
        assert!(zx_tape_identity_fresh(&write(&dir, "g.tzx", &bad_major)).is_none());
    }

    #[test]
    fn a_tzx_is_walked_block_by_block_not_just_signature_checked() {
        let dir = tempfile::tempdir().unwrap();
        // ID 10 standard block: pause(2) length(2)=3, 3 data bytes.
        let standard: &[u8] = &[0x10, 0, 0, 3, 0, 1, 2, 3];
        let pause: &[u8] = &[0x20, 0xE8, 0x03];
        let ok = write(&dir, "ok.tzx", &tzx(&[standard, pause]));
        assert!(zx_tape_identity_fresh(&ok).is_some());
        // Signature fine, garbage body (unknown block id) is refused.
        let garbage = write(&dir, "garbage.tzx", &tzx(&[&[0xEE, 1, 2, 3]]));
        assert!(zx_tape_identity_fresh(&garbage).is_none());
        // A block that claims more data than the file holds is refused.
        let truncated = write(&dir, "trunc.tzx", &tzx(&[&[0x10, 0, 0, 200, 0, 1, 2]]));
        assert!(zx_tape_identity_fresh(&truncated).is_none());
        // Trailing bytes that are not a whole block are refused.
        let trailing = write(&dir, "trail.tzx", &tzx(&[standard, &[0x20]]));
        assert!(zx_tape_identity_fresh(&trailing).is_none());
    }

    #[test]
    fn symlinks_and_fifos_never_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "Jetpac.tap", &tap(&[&[0, 1, 2]]));
        assert!(zx_tape_identity_fresh(&path).is_some());
        let link = dir.path().join("link.tap");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(zx_tape_identity_fresh(&link).is_none());
        let fifo = dir.path().join("pipe.tap");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(zx_tape_identity_fresh(&fifo).is_none());
    }

    #[test]
    fn the_cached_identity_tracks_the_file_it_was_read_from() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.tap", &tap(&[&[0, 1, 2]]));
        let first = zx_tape_identity(&path).unwrap().game_key;
        assert_eq!(zx_tape_identity(&path).unwrap().game_key, first);
        std::fs::write(&path, tap(&[&[0, 1, 2, 3, 4, 5]])).unwrap();
        assert_ne!(zx_tape_identity(&path).unwrap().game_key, first);
    }
}
