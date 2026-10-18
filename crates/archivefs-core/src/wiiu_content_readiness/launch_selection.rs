//! The narrow Blue selection seam: one extracted base game's exact RPX anchor.
//! No conversion, decryption, installation or verified-release claim.
use super::*;
use crate::patch_manager::inspect_extracted_layout;
use crate::safe_read::{TrustedRoots, open_bounded_read};
use std::path::{Path, PathBuf};

/// A bounded Cafe ELF header/section-table probe, not payload verification.
/// Format offsets follow Cemu's RPLFileHeader and RPLLoader_ProcessHeaders:
/// https://github.com/cemu-project/Cemu/blob/main/src/Cafe/OS/RPL/rpl_structs.h
/// https://github.com/cemu-project/Cemu/blob/main/src/Cafe/OS/RPL/rpl.cpp
pub fn has_cafe_executable_format(path: &Path) -> bool {
    fn probe(path: &Path) -> Option<()> {
        crate::ensure_no_symlink_components(path).ok()?;
        let mut file = open_bounded_read(path, &TrustedRoots::none()).ok()?;
        let header = file.read_exact_at(0, 52, 52)?;
        if &header[..6] != b"\x7fELF\x01\x02"
            || header[6] > 1
            || &header[7..9] != b"\xca\xfe"
            || &header[18..24] != b"\0\x14\0\0\0\x01"
        {
            return None;
        }
        let offset = u32::from_be_bytes(header[32..36].try_into().ok()?) as u64;
        let size = u16::from_be_bytes(header[46..48].try_into().ok()?) as usize;
        let count = u16::from_be_bytes(header[48..50].try_into().ok()?) as usize;
        if size != 40 || !(2..=4096).contains(&count) || offset < 52 {
            return None;
        }
        let table = file.read_exact_at(offset, count.checked_mul(size)?, 4096 * 40)?;
        for section in table.chunks_exact(40) {
            let kind = u32::from_be_bytes(section[4..8].try_into().ok()?);
            let start = u32::from_be_bytes(section[16..20].try_into().ok()?) as u64;
            let bytes = u32::from_be_bytes(section[20..24].try_into().ok()?) as u64;
            if kind != 8 && start.checked_add(bytes)? > file.len() {
                return None;
            }
        }
        Some(())
    }
    probe(path).is_some()
}

/// A directory selection or its exact sole `code/*.rpx` may name the same
/// extracted base. Arbitrary loose RPXs, encrypted files and packages cannot.
pub fn extracted_root_for_selection(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || crate::ensure_no_symlink_components(path).is_err() {
        return None;
    }
    let root = if path.is_dir() {
        path
    } else {
        if !path.extension()?.eq_ignore_ascii_case("rpx") {
            return None;
        }
        let code = path.parent()?;
        if code.file_name()? != "code" {
            return None;
        }
        code.parent()?
    };
    let projected = project_wiiu_content_readiness(root, WiiUKeyStoreEvidence::NotChecked, None);
    if projected.representation != WiiUContentRepresentation::LoadiineExtracted
        || projected.readiness != WiiUContentReadiness::DirectlyLaunchable
        || projected.encryption_state != WiiUEncryptionState::Decrypted
        || projected.role != WiiUContentRole::BaseGame
        || !projected.issues.is_empty()
    {
        return None;
    }
    let layout = inspect_extracted_layout(root).ok()?;
    if (!path.is_dir() && layout.rpx_path != path) || !has_cafe_executable_format(&layout.rpx_path)
    {
        return None;
    }
    Some(root.into())
}
