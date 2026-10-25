//! A bounded, hash-bound extracted PS4 executable layout. Metadata identity is
//! supplied by the existing PARAM.SFO inspector, never by the folder name.
use super::shadps4_profile::{
    BoundFile, MAX_BINARY_BYTES, ShadPs4Refusal, ShadPs4RefusalKind as Kind, open_file,
    real_directory, refuse,
};
use crate::game_identity::{IdentityPlatform, inspect_game_identity};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadPs4BootRepresentation {
    Ps4Elf,
    UnencryptedUncompressedSelf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadPs4GameInput {
    pub(crate) root: PathBuf,
    pub(crate) boot: BoundFile,
    pub(crate) sfo: BoundFile,
    pub(crate) title_id: String,
    pub(crate) content_id: Option<String>,
    pub(crate) representation: ShadPs4BootRepresentation,
}
impl ShadPs4GameInput {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn executable(&self) -> &Path {
        &self.boot.path
    }
    pub fn title_id(&self) -> &str {
        &self.title_id
    }
    pub fn content_id(&self) -> Option<&str> {
        self.content_id.as_deref()
    }
    pub fn representation(&self) -> ShadPs4BootRepresentation {
        self.representation
    }
    pub fn identity_provenance(&self) -> &Path {
        &self.sfo.path
    }
    pub fn executable_sha256(&self) -> [u8; 32] {
        self.boot.sha256
    }
    pub(crate) fn fresh(&self) -> bool {
        self.boot.unchanged()
            && self.sfo.unchanged()
            && inspect_shadps4_game(&self.root).is_ok_and(|now| now == *self)
    }
}
/// Root directory or its exact eboot.bin only. PKG/ISO/archives, arbitrary ELF
/// paths, patch roots and filename-only identities are not V1 launch inputs.
pub fn inspect_shadps4_game(selected: &Path) -> Result<ShadPs4GameInput, ShadPs4Refusal> {
    let root = if selected.file_name().is_some_and(|n| n == "eboot.bin") {
        selected.parent().unwrap_or(selected)
    } else {
        selected
    };
    if !real_directory(root) || !crate::game_identity::ps4_directory_paths_are_regular(root) {
        return Err(refuse(
            Kind::UnsupportedGameLayout,
            "V1 needs a real extracted PS4 root with eboot.bin and sce_sys/param.sfo; packages/images/archives are unsupported",
        ));
    }
    let sfo = BoundFile::capture(
        &root.join("sce_sys/param.sfo"),
        crate::param_sfo::MAX_SFO_BYTES as u64,
    )?;
    let report = inspect_game_identity(root, Some("PS4"));
    let title_id = report
        .verified_ps4_title_id()
        .filter(|_| report.platform == IdentityPlatform::PlayStation4)
        .ok_or_else(|| {
            refuse(
                Kind::IdentityUnverified,
                "PS4 TITLE_ID is missing, ambiguous or invalid; names are not authoritative",
            )
        })?
        .to_owned();
    let content_id = report.verified_ps4_content_id().map(str::to_owned);
    // CATEGORY is descriptive in the identity layer. Do not promote it to
    // identity; use it solely to refuse update/DLC layouts as base games.
    let sfo_bytes = sfo.bytes(crate::param_sfo::MAX_SFO_BYTES as u64)?;
    let observation = crate::param_sfo::parse_param_sfo(&sfo_bytes)
        .ok_or_else(|| refuse(Kind::UnsupportedGameLayout, "invalid SFO"))?;
    if observation.get_text("CATEGORY").is_some_and(|v| v != "gd") {
        return Err(refuse(
            Kind::UnsupportedGameLayout,
            "V1 supports base-game CATEGORY gd only, not updates/DLC",
        ));
    }
    let boot = BoundFile::capture(&root.join("eboot.bin"), MAX_BINARY_BYTES)
        .map_err(|e| refuse(Kind::UnsupportedGameLayout, e.detail))?;
    let mut prefix = Vec::new();
    open_file(&boot.path)?
        .take(65536)
        .read_to_end(&mut prefix)
        .map_err(|e| refuse(Kind::UnsupportedGameLayout, e.to_string()))?;
    let representation = inspect_boot_header(&prefix, boot.identity.size)?;
    if !boot.unchanged() || !sfo.unchanged() {
        return Err(refuse(
            Kind::ChangedAfterPreview,
            "PS4 input changed during inspection",
        ));
    }
    Ok(ShadPs4GameInput {
        root: root.into(),
        boot,
        sfo,
        title_id,
        content_id,
        representation,
    })
}
fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}
fn fits(offset: u64, size: u64, length: u64) -> bool {
    offset.checked_add(size).is_some_and(|end| end <= length)
}
fn inspect_boot_header(
    bytes: &[u8],
    length: u64,
) -> Result<ShadPs4BootRepresentation, ShadPs4Refusal> {
    let invalid = || {
        refuse(
            Kind::UnsupportedGameLayout,
            "truncated, encrypted, compressed or unsupported PS4 executable layout",
        )
    };
    if bytes.len() < 64 {
        return Err(invalid());
    }
    let (elf_offset, representation) = if u32_at(bytes, 0) == 0x1d3d154f {
        if bytes[4..10] != [0, 1, 1, 0x12, 1, 1] || u32_at(bytes, 16) as u64 != length {
            return Err(invalid());
        }
        let count = u16_at(bytes, 24) as usize;
        let elf_offset = 32 + count * 32;
        if count == 0
            || count > 512
            || bytes.len() < elf_offset + 64
            || (u16_at(bytes, 12) as usize) < elf_offset + 64
        {
            return Err(invalid());
        }
        for segment in bytes[32..elf_offset].chunks_exact(32) {
            if u64_at(segment, 0) & 0xa != 0
                || !fits(u64_at(segment, 8), u64_at(segment, 16), length)
            {
                return Err(invalid());
            }
        }
        (
            elf_offset,
            ShadPs4BootRepresentation::UnencryptedUncompressedSelf,
        )
    } else {
        (0, ShadPs4BootRepresentation::Ps4Elf)
    };
    let header = &bytes[elf_offset..elf_offset + 64];
    if &header[..9] != b"\x7fELF\x02\x01\x01\x09\x00"
        || !matches!(u16_at(header, 16), 0xfe00 | 0xfe10)
        || u16_at(header, 18) != 62
        || u32_at(header, 20) != 1
        || u16_at(header, 54) != 56
        || !matches!(u16_at(header, 58), 0 | 64)
    {
        return Err(invalid());
    }
    let count = u16_at(header, 56) as u64;
    let phoff = u64_at(header, 32);
    if count == 0
        || count > 512
        || phoff < 64
        || !fits(phoff, count * 56, (bytes.len() - elf_offset) as u64)
        || !fits(
            u64_at(header, 40),
            u16_at(header, 60) as u64 * u16_at(header, 58) as u64,
            length.saturating_sub(elf_offset as u64),
        )
    {
        return Err(invalid());
    }
    let start = elf_offset + phoff as usize;
    let mut load = false;
    for ph in bytes[start..start + count as usize * 56].chunks_exact(56) {
        if u32_at(ph, 0) == 1 {
            load = true;
        }
        // filesz <= memsz is a PT_LOAD invariant. PS4 metadata segments may
        // describe file bytes without an in-memory extent.
        if u32_at(ph, 0) == 1 && u64_at(ph, 32) > u64_at(ph, 40)
            || representation == ShadPs4BootRepresentation::Ps4Elf
                && !fits(u64_at(ph, 8), u64_at(ph, 32), length)
        {
            return Err(invalid());
        }
    }
    if !load {
        return Err(invalid());
    }
    Ok(representation)
}
