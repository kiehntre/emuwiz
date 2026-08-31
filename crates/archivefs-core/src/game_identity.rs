//! Strictly read-only, bounded game identity inspection.
//!
//! Identity is evidence: only values obtained from reviewed on-disc structures
//! are `Verified`. Archive and member names can only produce `Candidate` values.

use std::fmt;
use std::fs::File;

use crate::safe_read::TrustedRoots;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::atari7800_header_evidence::{A78_HEADER_BYTES, parse_a78_header};
use crate::disc_evidence_collector::{
    DiscCollectionRefusal, chd_needs_specialist_optical_backend, open_chd_iso9660,
    open_chd_raw_track, read_bounded_chd_bytes,
};
use crate::dreamcast_boot_evidence::{IP_BIN_META_BYTES, parse_ip_bin_meta};
use crate::executable_signatures::{
    XBE_CERTIFICATE_READ_BYTES, XBE_HEADER_PREFIX_BYTES, looks_like_self, looks_like_xbe,
    parse_xbe_header, xbe_certificate_file_offset,
};
use crate::gb_header_evidence::{GB_HEADER_BYTES, GbColorSupport, parse_gb_header};
use crate::header_normalization::{
    HeaderNormalizationKind, recognize_snes_copier_candidate, strip_known_header,
};
use crate::ingestion::cue_bin::{CueDataTrackMode, resolve_data_track};
use crate::ingestion::gdi::{GdiDataTrackMode, resolve_gdi_data_track};
use crate::iso9660::find_path;
use crate::logical_media::LogicalMedia as _;
use crate::lynx_header_evidence::{LYNX_HEADER_BYTES, parse_lynx_header};
use crate::n64_byte_order::{detect_n64_byte_order, normalize_to_z64};
use crate::n64_cic_evidence::{cic_lookup, validate_crc1_crc2};
use crate::n64_header_evidence::parse_n64_header;
use crate::neogeocd_boot_evidence::{MAX_IPL_TXT_BYTES, parse_ipl_txt};
use crate::nes_header_evidence::{INES_HEADER_BYTES, InesHeaderFact, parse_ines_header};
use crate::ngp_header_evidence::{NGP_HEADER_BYTES, NgpSystemFlag, parse_ngp_header};
use crate::param_sfo::parse_param_sfo;
use crate::pcengine_cd_boot_evidence::{
    PCE_CD_IPL_HEADER_BYTES, PCE_CD_IPL_SECTOR_OFFSET, parse_pce_cd_ipl,
};
use crate::pcfx_boot_evidence::{
    PCFX_BOOT_SECTOR_BYTES, PCFX_VOLUME_HEADER_BYTES, parse_pcfx_boot_sector,
    parse_pcfx_volume_header, pcfx_disc_hash,
};
use crate::playstation_boot_evidence::{
    PSX_EXECUTABLE_HEADER_BYTES, looks_like_psx_exe, parse_system_cnf_boot,
};
use crate::ps3_disc_evidence::observe_ps3_directory;
use crate::psp_pbp_evidence::{
    PBP_HEADER_BYTES, observe_pbp_evidence, parse_pbp_header, read_pbp_param_sfo,
    validate_pbp_offsets,
};
use crate::raw_cd_logical_media::{
    open_cooked_cd_file_logical_media, open_raw_cd_file_logical_media,
};
use crate::saturn_boot_evidence::{SATURN_SYSTEM_ID_BYTES, parse_saturn_system_id};
use crate::segacd_boot_evidence::{SEGA_CD_DISC_ID_BYTES, parse_segacd_product_code};
use crate::snes_header_evidence::{SnesHeaderFact, SnesMapMode, parse_snes_header_candidate};
use crate::threedo_boot_evidence::{OPERA_HEADER_BYTES, parse_opera_volume_header};

pub const MAX_BYTES_READ: u64 = 64 * 1024 * 1024;
pub const MAX_ARCHIVE_MEMBERS: usize = 4_096;
pub const MAX_METADATA_PATHS: usize = 32;
pub const MAX_ISO_DIRECTORY_ENTRIES: usize = 4_096;
pub const MAX_ISO_DESCRIPTORS: usize = 32;
pub const MAX_PATH_BYTES: usize = 512;
pub const MAX_SYSTEM_CNF_BYTES: u64 = 64 * 1024;
pub const MAX_EXECUTABLE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_DIRECTORY_BYTES: u64 = 1024 * 1024;
pub const MAX_NESTED_CONTAINER_DEPTH: usize = 1;
pub const MAX_RETAINED_WARNINGS: usize = 64;
pub const MAX_LOOSE_ROM_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_LOOSE_ROM_FILES: usize = 1;
pub const MAX_LOOSE_ROM_WARNINGS: usize = 16;
pub const MAX_LOOSE_ROM_METADATA_TOKENS: usize = 16;

const ISO_SECTOR_SIZE: u64 = 2_048;
const DOLPHIN_HEADER_BYTES: usize = 0x20;
const WII_MAGIC_OFFSET: usize = 0x18;
const GAMECUBE_MAGIC_OFFSET: usize = 0x1c;
const WII_MAGIC: [u8; 4] = [0x5d, 0x1c, 0x9e, 0xa3];
const GAMECUBE_MAGIC: [u8; 4] = [0xc2, 0x33, 0x9f, 0x3d];

/// WIA/RVZ file format, per `docs/WiaAndRvz.md` in the Dolphin repository:
/// `wia_file_head_t` (0x48 bytes, offset 0x0) is followed immediately by
/// `wia_disc_t` (offset 0x48), whose first four `u32` fields
/// (`disc_type`, `compression`, `compr_level`, `chunk_size`) precede an
/// uncompressed `u8 dhead[0x80]` field holding the first 0x80 bytes of the
/// original disc image - the same bytes `inspect_dolphin_header` already
/// reads from offset 0 of a direct ISO. All WIA/RVZ integers are big
/// endian. Only these bounded, always-uncompressed header bytes are ever
/// read; the compressed disc body is never touched.
const RVZ_MAGIC: [u8; 4] = *b"RVZ\x01";
const WIA_DISC_TYPE_OFFSET: u64 = 0x48;
const WIA_DHEAD_OFFSET: u64 = 0x58;
const WIA_DISC_TYPE_GAMECUBE: u32 = 1;
const WIA_DISC_TYPE_WII: u32 = 2;

/// The uncompressed, sparse-block GameCube/Wii `.ciso` format used by
/// several backup/dumping tools (distinct from the unrelated PSP "CISO"
/// compressed format). Layout confirmed against Dolphin's `CISOBlob.cpp`
/// reader: a fixed 0x8000-byte header (4-byte magic, `u32` little-endian
/// block size, then a `0x7FF8`-byte block-presence map, one byte per
/// possible block), followed by only the *present* blocks stored back to
/// back in original order - no compression, so the disc header block, if
/// present, can be located and read directly.
const CISO_MAGIC: [u8; 4] = *b"CISO";
const CISO_HEADER_SIZE: u64 = 0x8000;
const CISO_MAP_OFFSET: u64 = 8;

/// WBFS container format, per `libwbfs.c`/`libwbfs.h` (Kwiirk/WiiThemer,
/// as also read by Dolphin's `WbfsBlob.cpp`). The first HD sector holds
/// `wbfs_head_t`: a 4-byte magic, a big-endian `u32 n_hd_sec`, then two
/// `u8` power-of-two shifts (`hd_sec_sz_s`, `wbfs_sec_sz_s`), followed by
/// the disc table - one byte per disc slot, non-zero when occupied. Each
/// occupied slot begins with `wbfs_disc_info_t`, whose first 0x100 bytes
/// are a verbatim copy of the original disc header, so the same bytes
/// `inspect_dolphin_header` reads from offset 0 of a plain ISO are stored
/// uncompressed and can be read directly. Only the head, the disc table,
/// the small `wlba` mapping table, and that one bounded disc header are
/// read; mapped disc-data sectors are never read.
const WBFS_MAGIC: [u8; 4] = *b"WBFS";
const WBFS_N_HD_SEC_OFFSET: usize = 0x04;
const WBFS_HD_SEC_SZ_S_OFFSET: u64 = 0x08;
const WBFS_WBFS_SEC_SZ_S_OFFSET: u64 = 0x09;
const WBFS_DISC_TABLE_OFFSET: u64 = 0x0c;
/// `wbfs_disc_info_t.disc_header_copy` is a fixed 0x100-byte prefix.
const WBFS_DISC_INFO_HEADER_BYTES: u64 = 0x100;
/// A full, unscrubbed Wii DVD image (`WII_MAX_DISC_SIZE` in libwbfs);
/// used only to size the per-slot `wlba` table so slot offsets can be
/// computed. Never used as a read length.
const WBFS_WII_DISC_SIZE: u64 = 0x1_1824_0000;
/// HD sector shifts outside 512 B..64 KiB are not produced by any real
/// formatter and would make the slot arithmetic meaningless.
const WBFS_MIN_HD_SECTOR_SHIFT: u32 = 9;
const WBFS_MAX_HD_SECTOR_SHIFT: u32 = 16;
/// WBFS sector shifts outside 64 KiB..64 MiB likewise indicate a corrupt
/// or hostile header.
const WBFS_MIN_SECTOR_SHIFT: u32 = 16;
const WBFS_MAX_SECTOR_SHIFT: u32 = 26;

/// Xbox 360 XEX2 module header - see `xex2_header` in Xenia's
/// `xex2_info.h`. Only the unencrypted, uncompressed header is ever read;
/// the compressed/encrypted module body is never touched.
const XEX_MAGIC: [u8; 4] = *b"XEX2";
const XEX_BASE_HEADER_BYTES: usize = 0x18;
const XEX_HEADER_COUNT_OFFSET: usize = 0x14;
const XEX_OPT_HEADER_TABLE_OFFSET: u64 = 0x18;
const XEX_OPT_HEADER_ENTRY_BYTES: u64 = 8;
/// `XEX_HEADER_EXECUTION_INFO` in Xenia's `xex2_header_keys`.
const XEX_EXECUTION_INFO_KEY: u32 = 0x0004_0006;
/// `sizeof(xex2_opt_execution_info)` (`static_assert_size(..., 0x18)`).
const XEX_EXECUTION_INFO_BYTES: usize = 0x18;
/// Bounds the optional-header table read (real XEX files carry a few
/// dozen entries at most); prevents a corrupt `header_count` from
/// requesting an unreasonable allocation.
const MAX_XEX_OPT_HEADERS: u32 = 512;
/// Bounds how much of a ZIP-contained XEX member is buffered before
/// parsing. Real Xenia headers are a few KiB; this is generous enough for
/// any legitimate header while remaining a small, fixed, safe read.
const XEX_HEADER_PREFIX_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityStatus {
    Verified,
    Candidate,
    Missing,
    Unsupported,
    Deferred,
    Invalid,
    Ambiguous,
    ResourceLimitReached,
}

impl fmt::Display for IdentityStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Verified => "Verified",
            Self::Candidate => "Candidate",
            Self::Missing => "Missing",
            Self::Unsupported => "Unsupported",
            Self::Deferred => "Not available yet",
            Self::Invalid => "Invalid",
            Self::Ambiguous => "Ambiguous",
            Self::ResourceLimitReached => "Resource limit reached",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    Platform,
    Ps1Serial,
    Ps2Serial,
    PspDiscId,
    Ps3TitleId,
    SaturnProductNumber,
    DreamcastProductCode,
    SegaCdProductCode,
    Pcsx2ExecutableCrc,
    DolphinGameId,
    DolphinRevision,
    DolphinDiscNumber,
    DolphinRegion,
    LooseRomSha256,
    /// SHA-256 of the byte-order-normalized (canonical `Z64`) image - only
    /// ever emitted for a platform with a tested, reversible representation
    /// normalization ([`crate::n64_byte_order`] and
    /// [`crate::header_normalization`]).
    /// Distinct from [`Self::LooseRomSha256`], which always covers the exact
    /// physical on-disk bytes regardless of byte order.
    LooseRomCanonicalSha256,
    LooseRomFormat,
    LooseRomTitle,
    /// A verified original-Xbox title ID, read from a `default.xbe`-style
    /// executable's own certificate. Distinct from [`Self::XexTitleId`]
    /// (Xbox 360) - the two platforms are never conflated.
    XbeTitleId,
    XexTitleId,
    XexMediaId,
    /// A game ID returned by the locally installed ScummVM detector for an
    /// extracted game folder. The ID is never derived from its folder name.
    ScummVmGameId,
    /// Composite structured identity from a 3DO Opera volume header:
    /// volume identifier, root unique identifier, and declared block count.
    /// These are on-disc fields, not a filename or title-database lookup.
    ThreeDoDiscId,
    /// CIC-NUS bootcode security metadata; this never identifies a title,
    /// release, region, or exact game.
    N64Cic,
    /// CIC-specific validation result for the N64 header's CRC1/CRC2 fields.
    N64CrcValidation,
    /// The PC Engine CD-ROM² / TurboGrafx-CD IPL boot-record signature was
    /// present and structurally valid. Platform/media evidence only - it
    /// carries no serial, title or release identity (the IPL header has
    /// none). Exact game identity stays DAT/hash-driven.
    PceCdBootStructure,
    /// A structurally valid Neo Geo CD `IPL.TXT` load manifest was present
    /// on the disc (bounded entry list, terminator byte present). Platform/
    /// media evidence only - the IPL manifest carries no serial, title or
    /// product code, so exact game identity stays DAT/hash-driven. A file
    /// merely named `IPL.TXT` that does not parse never produces this.
    NeoGeoCdBootStructure,
    /// Parsed iNES/NES 2.0 header metadata; exact release identity remains
    /// authoritative only when established by DAT/hash evidence.
    NesHeader,
    /// Parsed, checksum/complement-validated SNES internal-header metadata;
    /// exact release identity remains authoritative only via DAT/hash.
    SnesHeader,
    /// The established PC-FX custom disc-identification hash. It is derived
    /// from PC-FX sector/header/boot content, never from a filename.
    PcfxDiscHash,
}

impl fmt::Display for IdentityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Platform => "Platform",
            Self::Ps1Serial => "PS1 serial",
            Self::Ps2Serial => "PS2 serial",
            Self::PspDiscId => "PSP disc ID",
            Self::Ps3TitleId => "PS3 title ID",
            Self::SaturnProductNumber => "Saturn product number",
            Self::DreamcastProductCode => "Dreamcast product code",
            Self::SegaCdProductCode => "Sega CD product code",
            Self::Pcsx2ExecutableCrc => "PCSX2 executable CRC",
            Self::DolphinGameId => "Dolphin Game ID",
            Self::DolphinRevision => "Dolphin revision",
            Self::DolphinDiscNumber => "Dolphin disc number",
            Self::DolphinRegion => "Dolphin region code",
            Self::LooseRomSha256 => "Local ROM SHA-256",
            Self::LooseRomCanonicalSha256 => "Canonical byte-order-normalized ROM SHA-256",
            Self::LooseRomFormat => "Loose ROM format",
            Self::LooseRomTitle => "Normalized ROM title",
            Self::XbeTitleId => "Xbox Title ID",
            Self::XexTitleId => "Xbox 360 Title ID",
            Self::XexMediaId => "Xbox 360 Media ID",
            Self::ScummVmGameId => "ScummVM game ID",
            Self::ThreeDoDiscId => "3DO disc identity",
            Self::N64Cic => "N64 CIC",
            Self::N64CrcValidation => "N64 CRC1/CRC2 validation",
            Self::PceCdBootStructure => "PC Engine CD boot structure",
            Self::NeoGeoCdBootStructure => "Neo Geo CD boot structure",
            Self::NesHeader => "NES header",
            Self::SnesHeader => "SNES header",
            Self::PcfxDiscHash => "PC-FX disc hash",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityConfidence {
    ExactBytes,
    StructuredMetadata,
    CatalogueContext,
    FilenameOnly,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityPlatform {
    PlayStation,
    PlayStation2,
    Psp,
    PlayStation3,
    Saturn,
    Dreamcast,
    SegaCd,
    GameCube,
    Wii,
    WiiU,
    ThreeDS,
    Switch,
    MegaDrive,
    Snes,
    Nes,
    GameBoy,
    GameBoyColor,
    GameBoyAdvance,
    N64,
    Xbox,
    Xbox360,
    ScummVM,
    ThreeDo,
    Pcfx,
    PcEngineCd,
    NeoGeoCd,
    Ngp,
    Ngpc,
    Atari2600,
    Atari5200,
    Atari7800,
    Atari8Bit,
    AtariLynx,
    AtariJaguar,
    AtariST,
    Other,
}

impl IdentityPlatform {
    pub fn from_catalogue(value: Option<&str>) -> Self {
        let value = value.unwrap_or_default().trim().to_ascii_lowercase();
        match value.as_str() {
            "playstation" | "playstation 1" | "playstation1" | "psx" | "ps1"
            | "sony playstation" => Self::PlayStation,
            "playstation 2" | "playstation2" | "ps2" | "sony playstation 2" => Self::PlayStation2,
            "psp" | "playstation portable" | "sony playstation portable" => Self::Psp,
            "playstation 3" | "playstation3" | "ps3" | "sony playstation 3" => Self::PlayStation3,
            "saturn" | "sega saturn" | "sega saturn console" => Self::Saturn,
            "dreamcast" | "sega dreamcast" => Self::Dreamcast,
            "sega cd" | "sega-cd" | "segacd" | "mega cd" | "mega-cd" | "megacd" => Self::SegaCd,
            "gamecube" | "nintendo gamecube" | "gc" | "gcn" => Self::GameCube,
            "wii" | "nintendo wii" => Self::Wii,
            "wiiu" | "wii u" | "nintendo wii u" => Self::WiiU,
            "3ds" | "nintendo 3ds" | "new 3ds" | "new3ds" => Self::ThreeDS,
            "switch" | "nintendo switch" => Self::Switch,
            "megadrive" | "mega drive" | "genesis" | "sega mega drive" | "sega genesis" => {
                Self::MegaDrive
            }
            "snes"
            | "super nintendo"
            | "super nintendo entertainment system"
            | "nintendo super nintendo entertainment system"
            | "super famicom" => Self::Snes,
            "nes" | "nintendo entertainment system" | "famicom" | "nintendo famicom" => Self::Nes,
            "game boy" | "gb" | "nintendo game boy" => Self::GameBoy,
            "game boy color" | "gbc" | "nintendo game boy color" => Self::GameBoyColor,
            "game boy advance" | "gba" | "nintendo game boy advance" => Self::GameBoyAdvance,
            "n64" | "nintendo 64" | "nintendo64" => Self::N64,
            "xbox" | "original xbox" | "microsoft xbox" => Self::Xbox,
            "xbox360" | "xbox 360" | "microsoft xbox 360" => Self::Xbox360,
            "scummvm" | "scumm vm" => Self::ScummVM,
            "3do" | "panasonic 3do" | "3do interactive multiplayer" => Self::ThreeDo,
            "pc-fx" | "pcfx" | "nec pc-fx" | "nec pcfx" => Self::Pcfx,
            "pc engine cd"
            | "pcenginecd"
            | "pc-engine cd"
            | "nec pc engine cd"
            | "turbografx-cd"
            | "turbografx cd"
            | "turbografxcd"
            | "turbografx-cd-rom"
            | "tgcd"
            | "tg-cd"
            | "pc engine cd-rom\u{b2}"
            | "pc engine cd-rom2"
            | "cd-rom\u{b2}"
            | "cd-rom2"
            | "cdrom2"
            | "super cd-rom\u{b2}"
            | "super cd-rom2"
            | "turbo duo"
            | "turboduo" => Self::PcEngineCd,
            "neo geo cd" | "neogeocd" | "neo-geo cd" | "neo geo cd-rom" | "snk neo geo cd"
            | "ngcd" | "neocd" | "neo cd" | "neocdz" => Self::NeoGeoCd,
            "neo geo pocket" | "neogeopocket" | "ngp" => Self::Ngp,
            "neo geo pocket color" | "neogeopocketcolor" | "ngpc" => Self::Ngpc,
            "atari 2600" | "atari2600" | "a2600" | "atari vcs" | "atarivcs" => Self::Atari2600,
            "atari 5200" | "atari5200" | "a5200" => Self::Atari5200,
            "atari 7800" | "atari7800" | "a7800" => Self::Atari7800,
            "atari 8-bit" | "atari8bit" | "atari 8 bit" | "atari 800" | "atari800"
            | "atari 400" | "atari400" | "atari xe" | "atarixe" | "atari xl" | "atarixl"
            | "atari xegs" | "atarixegs" | "atari 130xe" | "atari130xe" => Self::Atari8Bit,
            "atari lynx" | "atarilynx" | "lynx" | "lynx ii" | "lynxii" | "atarilynxlynx" => {
                Self::AtariLynx
            }
            "atari jaguar" | "atarijaguar" | "jaguar" | "jaguar64" | "atarijag" => {
                Self::AtariJaguar
            }
            "atari st" | "atarist" | "atari ste" | "atariste" | "atari tt" | "atarittu"
            | "atari falcon" | "atarifalcon" => Self::AtariST,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PlayStation => "PlayStation",
            Self::PlayStation2 => "PlayStation 2",
            Self::Psp => "PlayStation Portable",
            Self::PlayStation3 => "PlayStation 3",
            Self::Saturn => "Sega Saturn",
            Self::Dreamcast => "Sega Dreamcast",
            Self::SegaCd => "Sega Mega-CD / Sega CD",
            Self::GameCube => "GameCube",
            Self::Wii => "Wii",
            Self::WiiU => "Wii U",
            Self::ThreeDS => "Nintendo 3DS",
            Self::Switch => "Nintendo Switch",
            Self::MegaDrive => "Mega Drive / Genesis",
            Self::Snes => "SNES",
            Self::Nes => "NES",
            Self::GameBoy => "Game Boy",
            Self::GameBoyColor => "Game Boy Color",
            Self::GameBoyAdvance => "Game Boy Advance",
            Self::N64 => "Nintendo 64",
            Self::Xbox => "Xbox",
            Self::Xbox360 => "Xbox 360",
            Self::ScummVM => "ScummVM",
            Self::ThreeDo => "3DO",
            Self::Pcfx => "PC-FX",
            Self::PcEngineCd => "PC Engine CD / TurboGrafx-CD",
            Self::NeoGeoCd => "Neo Geo CD",
            Self::Ngp => "Neo Geo Pocket",
            Self::Ngpc => "Neo Geo Pocket Color",
            Self::Atari2600 => "Atari 2600",
            Self::Atari5200 => "Atari 5200",
            Self::Atari7800 => "Atari 7800",
            Self::Atari8Bit => "Atari 8-bit",
            Self::AtariLynx => "Atari Lynx",
            Self::AtariJaguar => "Atari Jaguar",
            Self::AtariST => "Atari ST",
            Self::Other => "Unsupported platform",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityImageFormat {
    Iso,
    ZipContainingIso,
    LooseCartridgeRom,
    Xex,
    ZipContainingXex,
    /// A direct original-Xbox `.xbe` executable - identity is read from its
    /// own header/certificate, exactly like [`Self::Xex`] for Xbox 360.
    Xbe,
    /// A ZIP containing exactly one `.xbe` member - see [`Self::Xbe`].
    ZipContainingXbe,
    /// A whole original-Xbox disc image (`.iso`/`.xiso`) - identity is read
    /// from `/default.xbe` inside its XDVDFS filesystem, via a bounded,
    /// random-access reader that never materializes the whole image (see
    /// [`crate::xdvdfs_traversal`]'s file-backed API). Unlike [`Self::Xbe`],
    /// this format's own path is genuinely runnable disc content, not just
    /// verified identity.
    XboxDiscImage,
    /// WIA/RVZ (`.rvz`) - identity is read from the documented uncompressed
    /// `wia_disc_t.dhead` header field, never from the compressed disc body.
    Rvz,
    /// The uncompressed, sparse-block GameCube/Wii `.ciso` format (distinct
    /// from PSP CISO) - identity is read from the first stored block only.
    Ciso,
    /// The WBFS container format - identity is read from the plain, stored
    /// Wii disc header at the start of the single occupied disc slot. Its
    /// bounded mapping table is validated; mapped disc data is never read.
    Wbfs,
    /// A `.chd` decoded through the existing bounded pure-Rust track reader
    /// (see [`crate::disc_evidence_collector::open_chd_iso9660`]) and
    /// recognised as an ISO 9660 PS1 disc - currently the only CHD case
    /// this module resolves authoritatively rather than deferring; every
    /// other CHD (other platforms, specialist-optical-backend media,
    /// non-ISO9660 content) still reports [`Self::Deferred`].
    Chd,
    /// A Dreamcast `.gdi` descriptor's own high-density data track,
    /// resolved via [`crate::ingestion::gdi::resolve_gdi_data_track`] and
    /// read through the existing bounded raw/cooked CD logical-media
    /// readers - the same standard [`Self::Iso`]/CUE Dreamcast identity
    /// already meets. GDI is Dreamcast-only: PS1/Saturn never used GD-ROM.
    Gdi,
    /// A Dreamcast DiscJuggler `.cdi` image's own selected data track
    /// (see [`crate::dreamcast_cdi::open_dreamcast_cdi_logical_media`]),
    /// read through the same standard [`Self::Iso`]/CUE/GDI Dreamcast
    /// identity path. CDI is Dreamcast-only, exactly like [`Self::Gdi`].
    /// Requires the `dreamcast-cdi` build feature (default-on); without
    /// it, this reports [`Self::Unsupported`] instead, never a guess.
    Cdi,
    /// A bounded PSP/PS1 `EBOOT.PBP` container. The container and any
    /// embedded PSP product code are inspected, but PSAR payloads are not
    /// decompressed or traversed.
    Pbp,
    /// An extracted ScummVM game folder verified by the installed ScummVM
    /// detector. No archive or folder name is used as identity evidence.
    ScummVmDirectory,
    /// A PS3 digital package whose fixed header was structurally validated.
    /// This observes package identity only; it is not an installed or
    /// directly runnable PS3 title.
    Pkg,
    Deferred,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityProvenance {
    #[serde(with = "path_bytes_serde")]
    pub archive_path: PathBuf,
    pub member_path: Option<Vec<u8>>,
    pub member_index: Option<usize>,
    pub method: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityEvidence {
    pub kind: IdentityKind,
    pub status: IdentityStatus,
    pub value: Option<String>,
    pub confidence: IdentityConfidence,
    pub provenance: IdentityProvenance,
    pub diagnostic: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameIdentityReport {
    #[serde(with = "path_bytes_serde")]
    pub archive_path: PathBuf,
    pub platform: IdentityPlatform,
    pub format: IdentityImageFormat,
    pub evidence: Vec<IdentityEvidence>,
    pub warnings: Vec<String>,
    pub bytes_read: u64,
    pub archive_members_inspected: usize,
    pub metadata_paths_inspected: usize,
    pub nested_container_depth: usize,
    pub complete: bool,
}

#[cfg(unix)]
mod path_bytes_serde {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(path.as_os_str().as_bytes())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        Ok(PathBuf::from(OsString::from_vec(bytes)))
    }
}

#[cfg(not(unix))]
mod path_bytes_serde {
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        path.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        PathBuf::deserialize(deserializer)
    }
}

impl GameIdentityReport {
    pub fn verified_value(&self, kind: IdentityKind) -> Option<&str> {
        self.evidence.iter().find_map(|evidence| {
            (evidence.kind == kind && evidence.status == IdentityStatus::Verified)
                .then_some(evidence.value.as_deref())
                .flatten()
        })
    }

    pub fn verified_dolphin_game_id(&self) -> Option<&str> {
        self.verified_value(IdentityKind::DolphinGameId)
    }

    pub fn verified_dolphin_revision(&self) -> Option<u16> {
        self.verified_value(IdentityKind::DolphinRevision)?
            .parse()
            .ok()
    }

    pub fn verified_pcsx2_crc(&self) -> Option<&str> {
        self.verified_value(IdentityKind::Pcsx2ExecutableCrc)
    }

    pub fn verified_ps2_serial(&self) -> Option<&str> {
        self.verified_value(IdentityKind::Ps2Serial)
    }

    pub fn verified_psp_disc_id(&self) -> Option<&str> {
        self.verified_value(IdentityKind::PspDiscId)
    }

    pub fn verified_ps3_title_id(&self) -> Option<&str> {
        self.verified_value(IdentityKind::Ps3TitleId)
    }

    pub fn verified_pcfx_disc_hash(&self) -> Option<&str> {
        self.verified_value(IdentityKind::PcfxDiscHash)
    }

    /// The validated SNES internal-header map mode. Header metadata is
    /// structural evidence only; exact release identity remains DAT/hash-
    /// driven.
    pub fn verified_snes_header(&self) -> Option<&str> {
        self.verified_value(IdentityKind::SnesHeader)
    }

    /// The verified CIC-NUS bootcode family, reported as hardware/security
    /// metadata only - never a title, release, region, or exact game ID.
    pub fn verified_n64_cic(&self) -> Option<&str> {
        self.verified_value(IdentityKind::N64Cic)
    }

    pub fn verified_ps1_serial(&self) -> Option<&str> {
        self.verified_value(IdentityKind::Ps1Serial)
    }

    pub fn verified_saturn_product_number(&self) -> Option<&str> {
        self.verified_value(IdentityKind::SaturnProductNumber)
    }

    pub fn verified_dreamcast_product_code(&self) -> Option<&str> {
        self.verified_value(IdentityKind::DreamcastProductCode)
    }

    pub fn verified_sega_cd_product_code(&self) -> Option<&str> {
        self.verified_value(IdentityKind::SegaCdProductCode)
    }

    pub fn verified_loose_rom_sha256(&self) -> Option<&str> {
        self.verified_value(IdentityKind::LooseRomSha256)
    }

    /// The verified byte-order-normalized (canonical `Z64`) SHA-256 -
    /// currently only ever populated for N64. `None` whenever no reversible
    /// byte-order normalization exists for the platform, or the ROM's own
    /// header/length couldn't be normalized (see [`push_n64_canonical_evidence`]).
    pub fn verified_loose_rom_canonical_sha256(&self) -> Option<&str> {
        self.verified_value(IdentityKind::LooseRomCanonicalSha256)
    }

    pub fn is_verified_loose_rom(&self) -> bool {
        self.format == IdentityImageFormat::LooseCartridgeRom
            && self.verified_loose_rom_sha256().is_some()
    }

    /// The verified original-Xbox Title ID, formatted as eight uppercase hex
    /// characters, read directly from a `default.xbe`-style executable's
    /// own certificate. Distinct from [`Self::verified_xex_title_id`]
    /// (Xbox 360) - the two platforms are never conflated.
    pub fn verified_xbox_title_id(&self) -> Option<&str> {
        self.verified_value(IdentityKind::XbeTitleId)
    }

    /// The verified Xbox 360 Title ID, formatted as eight uppercase hex
    /// characters (matching the `xenia-canary/game-patches` filename and
    /// `title_id` field convention).
    pub fn verified_xex_title_id(&self) -> Option<&str> {
        self.verified_value(IdentityKind::XexTitleId)
    }

    /// The verified Xbox 360 Media ID, formatted as eight uppercase hex
    /// characters. Read directly from the XEX execution-info header;
    /// never derived from a title or filename.
    pub fn verified_xex_media_id(&self) -> Option<&str> {
        self.verified_value(IdentityKind::XexMediaId)
    }

    pub fn verified_scummvm_game_id(&self) -> Option<&str> {
        self.verified_value(IdentityKind::ScummVmGameId)
    }

    /// The verified structured 3DO Opera disc identity. This preserves the
    /// volume/root identifiers and declared block count from the disc header;
    /// it is not a publisher serial or a filename-derived title.
    pub fn verified_threedo_disc_id(&self) -> Option<&str> {
        self.verified_value(IdentityKind::ThreeDoDiscId)
    }

    /// The PC Engine CD-ROM² / TurboGrafx-CD IPL boot-record was found and
    /// structurally valid on the disc's first data track. The returned
    /// value is the fixed signature string, not a serial or title - PC
    /// Engine CD carries no release identity in its boot record, so exact
    /// game identity still comes from DAT/hash matching.
    pub fn verified_pcengine_cd_boot_structure(&self) -> Option<&str> {
        self.verified_value(IdentityKind::PceCdBootStructure)
    }

    /// A structurally valid Neo Geo CD `IPL.TXT` load manifest was found in
    /// the disc's ISO 9660 root. The returned value is the fixed `"IPL.TXT"`
    /// marker, not a serial or title - the Neo Geo CD IPL manifest carries
    /// no release identity, so exact game identity still comes from DAT/hash
    /// matching.
    pub fn verified_neogeocd_boot_structure(&self) -> Option<&str> {
        self.verified_value(IdentityKind::NeoGeoCdBootStructure)
    }
}

pub fn inspect_game_identity(path: &Path, platform_hint: Option<&str>) -> GameIdentityReport {
    inspect_game_identity_with_platform_trust(path, platform_hint, false, &TrustedRoots::none())
}

/// Like [`inspect_game_identity`], but permitted to follow a symlink whose link
/// and canonical target both lie inside one of `trusted`'s configured source
/// roots. Without this, a symlinked library can never be identified - see
/// [`crate::safe_read`].
pub fn inspect_game_identity_in_roots(
    path: &Path,
    platform_hint: Option<&str>,
    trusted: &TrustedRoots,
) -> GameIdentityReport {
    inspect_game_identity_with_platform_trust(path, platform_hint, false, trusted)
}

/// Inspect identity using platform evidence already validated by the library
/// scanner or an explicit manual assignment. The boolean is deliberately not
/// inferred from a filename: callers must opt in at the catalogue boundary.
pub fn inspect_catalogued_game_identity(
    path: &Path,
    platform_hint: Option<&str>,
) -> GameIdentityReport {
    inspect_game_identity_with_platform_trust(path, platform_hint, true, &TrustedRoots::none())
}

/// Like [`inspect_catalogued_game_identity`], but permitted to follow a symlink
/// contained by `trusted` - see [`crate::safe_read`]. `trusted` governs only
/// *reading*: it is never used as a write destination.
pub fn inspect_catalogued_game_identity_in_roots(
    path: &Path,
    platform_hint: Option<&str>,
    trusted: &TrustedRoots,
) -> GameIdentityReport {
    inspect_game_identity_with_platform_trust(path, platform_hint, true, trusted)
}

fn inspect_game_identity_with_platform_trust(
    path: &Path,
    platform_hint: Option<&str>,
    trusted_platform: bool,
    trusted: &TrustedRoots,
) -> GameIdentityReport {
    let platform = IdentityPlatform::from_catalogue(platform_hint);
    let mut report = GameIdentityReport {
        archive_path: path.to_path_buf(),
        platform,
        format: IdentityImageFormat::Unsupported,
        evidence: Vec::new(),
        warnings: Vec::new(),
        bytes_read: 0,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: false,
    };
    report.evidence.push(evidence(
        &report,
        IdentityKind::Platform,
        if trusted_platform {
            IdentityStatus::Verified
        } else {
            IdentityStatus::Candidate
        },
        platform_hint.map(str::to_owned),
        IdentityConfidence::CatalogueContext,
        if trusted_platform {
            "exact platform supplied by scanner or manual assignment; not derived from ROM bytes"
        } else {
            "catalogue platform context; not derived from disc bytes"
        },
        if trusted_platform {
            "trusted EmuWiz library platform context"
        } else {
            "EmuWiz catalogue context"
        },
    ));
    add_filename_candidate(&mut report);

    if matches!(
        platform,
        IdentityPlatform::MegaDrive
            | IdentityPlatform::Snes
            | IdentityPlatform::Nes
            | IdentityPlatform::GameBoy
            | IdentityPlatform::GameBoyColor
            | IdentityPlatform::GameBoyAdvance
            | IdentityPlatform::N64
            | IdentityPlatform::Atari2600
            | IdentityPlatform::Atari5200
            | IdentityPlatform::Atari7800
            | IdentityPlatform::Atari8Bit
            | IdentityPlatform::AtariLynx
            | IdentityPlatform::AtariJaguar
            | IdentityPlatform::Ngp
            | IdentityPlatform::Ngpc
    ) {
        inspect_loose_rom(&mut report, trusted_platform, trusted);
        return report;
    }

    // Original-Xbox identity is gated on trusted platform evidence even
    // though the structural XBE/XDVDFS family shares filesystem signatures
    // with Xbox 360 - an untrusted/scanner-guessed "Xbox" hint must never
    // authorize verified identity, unlike the existing (unchanged) Xbox 360
    // XEX path, which has no equivalent collision risk to guard against.
    if platform == IdentityPlatform::Xbox && !trusted_platform {
        add_unavailable(
            &mut report,
            IdentityStatus::Ambiguous,
            "original-Xbox identity requires exact scanner or manual platform evidence",
        );
        return report;
    }

    if platform == IdentityPlatform::Other {
        report.evidence.push(evidence(
            &report,
            IdentityKind::DolphinGameId,
            IdentityStatus::Unsupported,
            None,
            IdentityConfidence::Unavailable,
            "shared identity inspection currently supports PS2, GameCube, and Wii",
            "platform eligibility",
        ));
        return report;
    }

    if platform == IdentityPlatform::PlayStation3
        && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
    {
        inspect_ps3_directory_identity(&mut report);
        return report;
    }

    if platform == IdentityPlatform::PlayStation3
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("pkg"))
    {
        inspect_direct_pkg(&mut report, trusted);
        return report;
    }

    if platform == IdentityPlatform::ScummVM
        && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
    {
        inspect_scummvm_directory(&mut report);
        return report;
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "gdi" if platform == IdentityPlatform::Dreamcast => {
            inspect_gdi(&mut report, trusted);
        }
        "cdi" if platform == IdentityPlatform::Dreamcast => {
            inspect_disc_cdi(&mut report);
        }
        "cue"
            if matches!(
                platform,
                IdentityPlatform::PlayStation
                    | IdentityPlatform::Psp
                    | IdentityPlatform::Saturn
                    | IdentityPlatform::Dreamcast
                    | IdentityPlatform::SegaCd
                    | IdentityPlatform::ThreeDo
                    | IdentityPlatform::Pcfx
                    | IdentityPlatform::PcEngineCd
                    | IdentityPlatform::NeoGeoCd
            ) =>
        {
            inspect_cue(&mut report, trusted);
        }
        "iso" | "gcm"
            if platform != IdentityPlatform::Xbox360 && platform != IdentityPlatform::Xbox =>
        {
            inspect_direct_iso(&mut report, trusted)
        }
        "pbp"
            if matches!(
                platform,
                IdentityPlatform::PlayStation | IdentityPlatform::Psp
            ) =>
        {
            inspect_pbp(&mut report, trusted)
        }
        "xex" if platform == IdentityPlatform::Xbox360 => inspect_direct_xex(&mut report, trusted),
        "zip" if platform == IdentityPlatform::Xbox360 => inspect_zip_xex(&mut report, trusted),
        "xbe" if platform == IdentityPlatform::Xbox => inspect_direct_xbe(&mut report, trusted),
        "zip" if platform == IdentityPlatform::Xbox => inspect_zip_xbe(&mut report, trusted),
        "iso" | "xiso" if platform == IdentityPlatform::Xbox => {
            inspect_direct_xbox_disc(&mut report, trusted);
        }
        "zip" => inspect_zip_iso(&mut report, trusted),
        "rvz" if matches!(platform, IdentityPlatform::GameCube | IdentityPlatform::Wii) => {
            inspect_rvz(&mut report, trusted);
        }
        "ciso" if matches!(platform, IdentityPlatform::GameCube | IdentityPlatform::Wii) => {
            inspect_ciso(&mut report, trusted);
        }
        "wbfs" if platform == IdentityPlatform::Wii => {
            inspect_wbfs(&mut report, trusted);
        }
        "chd"
            if matches!(
                platform,
                IdentityPlatform::PlayStation
                    | IdentityPlatform::PlayStation2
                    | IdentityPlatform::Saturn
                    | IdentityPlatform::Dreamcast
                    | IdentityPlatform::SegaCd
                    | IdentityPlatform::ThreeDo
                    | IdentityPlatform::Pcfx
                    | IdentityPlatform::PcEngineCd
                    | IdentityPlatform::NeoGeoCd
            ) =>
        {
            inspect_disc_chd(&mut report, trusted);
        }
        "chd" | "cso" | "rvz" | "wbfs" | "ciso" | "gcz" | "7z" | "rar" => {
            report.format = IdentityImageFormat::Deferred;
            add_unavailable(
                &mut report,
                IdentityStatus::Deferred,
                "format has no existing safe bounded reader in EmuWiz",
            );
        }
        _ => add_unavailable(
            &mut report,
            IdentityStatus::Unsupported,
            "only direct ISO/GCM, RVZ and CISO for GameCube/Wii, a single ISO inside ZIP, direct XEX, a single XEX inside ZIP, direct XBE, and a single XBE inside ZIP are supported",
        ),
    }
    report
}

pub fn supported_loose_rom_format(path: &Path, platform: IdentityPlatform) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match (platform, extension.as_str()) {
        (IdentityPlatform::MegaDrive, "md") => Some("md"),
        (IdentityPlatform::MegaDrive, "gen") => Some("gen"),
        (IdentityPlatform::MegaDrive, "smd") => Some("smd"),
        (IdentityPlatform::MegaDrive, "bin") => Some("bin"),
        (IdentityPlatform::Snes, "sfc") => Some("sfc"),
        (IdentityPlatform::Snes, "smc") => Some("smc"),
        (IdentityPlatform::Nes, "nes") => Some("nes"),
        (IdentityPlatform::Nes, "fds") => Some("fds"),
        (IdentityPlatform::Nes, "unf") => Some("unf"),
        (IdentityPlatform::GameBoy, "gb") => Some("gb"),
        (IdentityPlatform::GameBoyColor, "gbc") => Some("gbc"),
        (IdentityPlatform::GameBoyAdvance, "gba") => Some("gba"),
        (IdentityPlatform::N64, "z64") => Some("z64"),
        (IdentityPlatform::N64, "v64") => Some("v64"),
        (IdentityPlatform::N64, "n64") => Some("n64"),
        (IdentityPlatform::Atari2600, "a26") => Some("a26"),
        (IdentityPlatform::Atari5200, "a52") => Some("a52"),
        (IdentityPlatform::Atari7800, "a78") => Some("a78"),
        (IdentityPlatform::Atari8Bit, "atr") => Some("atr"),
        (IdentityPlatform::Atari8Bit, "atx") => Some("atx"),
        (IdentityPlatform::Atari8Bit, "xex") => Some("xex"),
        (IdentityPlatform::Atari8Bit, "xfd") => Some("xfd"),
        (IdentityPlatform::AtariLynx, "lnx") => Some("lnx"),
        (IdentityPlatform::AtariLynx, "lyx") => Some("lyx"),
        (IdentityPlatform::AtariJaguar, "j64") => Some("j64"),
        (IdentityPlatform::AtariJaguar, "jag") => Some("jag"),
        (IdentityPlatform::Ngp, "ngp" | "ngc") => Some("ngp"),
        (IdentityPlatform::Ngpc, "ngp" | "ngc") => Some("ngc"),
        _ => None,
    }
}

/// Parses the existing iNES/NES 2.0 header detector from a loose `.nes`
/// file, but only promotes it when the declared trainer/PRG/CHR payload fits
/// within the physical file. The parser remains responsible for header
/// decoding; this small file-bound check prevents truncated images from
/// gaining production header evidence.
fn inspect_nes_header(file: &mut File, file_len: u64) -> Option<InesHeaderFact> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut header = [0u8; INES_HEADER_BYTES];
    file.read_exact(&mut header).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    let fact = parse_ines_header(&header)?;
    let trainer_bytes = if fact.trainer { 512_u64 } else { 0 };
    let prg_bytes = u64::from(fact.prg_rom_16k_units).checked_mul(16 * 1024)?;
    let chr_bytes = u64::from(fact.chr_rom_8k_units).checked_mul(8 * 1024)?;
    let required = u64::try_from(INES_HEADER_BYTES)
        .ok()?
        .checked_add(trainer_bytes)?
        .checked_add(prg_bytes)?
        .checked_add(chr_bytes)?;
    (required <= file_len).then_some(fact)
}

/// Returns the existing validated SNES header and whether a 512-byte copier
/// header was removed before parsing. The size rule only selects the already-
/// supported reversible candidate; checksum/complement validation is the proof.
fn inspect_snes_header(bytes: &[u8]) -> Option<(SnesHeaderFact, bool)> {
    if recognize_snes_copier_candidate(bytes.len())
        && let Ok(normalized) = strip_known_header(bytes, HeaderNormalizationKind::SnesCopier512)
        && let Some(fact) = unique_validated_snes_header(&normalized.bytes)
    {
        return Some((fact, true));
    }
    unique_validated_snes_header(bytes).map(|fact| (fact, false))
}

fn unique_validated_snes_header(bytes: &[u8]) -> Option<SnesHeaderFact> {
    let mut candidates = SnesMapMode::ALL
        .into_iter()
        .filter_map(|mode| parse_snes_header_candidate(bytes, mode))
        .filter(|fact| fact.checksum_valid() && fact.map_mode_matches())
        .collect::<Vec<_>>();
    (candidates.len() == 1).then(|| candidates.remove(0))
}

/// Whether a `.gb`-hinted cartridge's own header proves it cannot actually
/// run on original Game Boy hardware - a genuine, structural
/// platform/content contradiction, never a guess.
///
/// Reads only the fixed, tiny [`GB_HEADER_BYTES`] header (never the whole
/// file) through the same safe-open primitive every other loose-ROM read in
/// this module uses - a second, independent read from the one
/// [`inspect_loose_rom`] performs for the SHA-256 hash, since the header
/// this checks and the identity that hash proves are two different
/// concerns answered from two different (tiny vs. bounded-full) reads.
///
/// `None` for everything that is not a *proven* contradiction: an
/// unreadable/too-short file, a header whose Nintendo logo does not match
/// (no structural claim to contradict), or a header that validates as
/// `DmgOnly`/`CgbEnhanced` (both genuinely run on original Game Boy
/// hardware). This never fails closed on mere absence of evidence - only a
/// `cgb_flag == 0xC0` ("CGB-only") header, which the hardware itself
/// enforces, counts.
fn gameboy_extension_conflict(path: &Path, trusted: &TrustedRoots) -> Option<String> {
    let mut file = open_read_only_regular(path, trusted).ok()?;
    let mut header = vec![0_u8; GB_HEADER_BYTES];
    file.read_exact(&mut header).ok()?;
    let fact = parse_gb_header(&header)?;
    if fact.logo_valid && fact.color_support == GbColorSupport::CgbOnly {
        return Some(
            "the cartridge header's own cgb_flag (0xC0) proves this title is Game Boy \
             Color-exclusive; it cannot run on original Game Boy hardware, so its \
             platform/content evidence conflicts with a .gb Game Boy assignment"
                .to_string(),
        );
    }
    None
}

fn inspect_loose_rom(
    report: &mut GameIdentityReport,
    trusted_platform: bool,
    trusted: &TrustedRoots,
) {
    report.format = IdentityImageFormat::LooseCartridgeRom;
    let Some(format) = supported_loose_rom_format(&report.archive_path, report.platform) else {
        add_loose_rom_unavailable(
            report,
            IdentityStatus::Unsupported,
            "file extension is not supported for the exact cartridge platform",
        );
        return;
    };
    if !trusted_platform {
        add_loose_rom_unavailable(
            report,
            IdentityStatus::Ambiguous,
            "loose ROM identity requires exact scanner or manual platform evidence",
        );
        return;
    }
    if report.platform == IdentityPlatform::GameBoy
        && let Some(reason) = gameboy_extension_conflict(&report.archive_path, trusted)
    {
        add_loose_rom_unavailable(report, IdentityStatus::Invalid, &reason);
        return;
    }
    let mut file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_loose_rom_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let before = match StableFileMetadata::from_file(&file) {
        Ok(metadata) => metadata,
        Err(error) => {
            add_loose_rom_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    if before.len > MAX_LOOSE_ROM_BYTES {
        add_loose_rom_unavailable(
            report,
            IdentityStatus::ResourceLimitReached,
            &format!(
                "loose ROM is {} bytes; maximum supported size is {} bytes",
                before.len, MAX_LOOSE_ROM_BYTES
            ),
        );
        return;
    }
    let mut nes_header = None;
    let mut atari7800_header = None;
    let mut lynx_header = None;
    let mut ngp_header = None;
    if report.platform == IdentityPlatform::Nes && format == "nes" {
        nes_header = inspect_nes_header(&mut file, before.len);
    }
    if report.platform == IdentityPlatform::Atari7800 && format == "a78" {
        file.seek(SeekFrom::Start(0)).ok();
        let mut header = vec![0_u8; A78_HEADER_BYTES];
        if file.read_exact(&mut header).is_ok()
            && let Some(fact) = parse_a78_header(&header)
            && u64::from(fact.rom_size)
                .checked_add(A78_HEADER_BYTES as u64)
                .is_some_and(|required| required <= before.len)
        {
            atari7800_header = Some(fact);
        }
    }
    if report.platform == IdentityPlatform::AtariLynx && format == "lnx" {
        file.seek(SeekFrom::Start(0)).ok();
        let mut header = vec![0_u8; LYNX_HEADER_BYTES];
        if file.read_exact(&mut header).is_ok() {
            lynx_header = parse_lynx_header(&header);
        }
    }
    if matches!(
        report.platform,
        IdentityPlatform::Ngp | IdentityPlatform::Ngpc
    ) {
        file.seek(SeekFrom::Start(0)).ok();
        let mut header = [0_u8; NGP_HEADER_BYTES];
        let valid = if file.read_exact(&mut header).is_ok() {
            parse_ngp_header(&header).filter(|fact| fact.copyright_recognized)
        } else {
            None
        };
        file.seek(SeekFrom::Start(0)).ok();
        let Some(fact) = valid else {
            add_loose_rom_unavailable(
                report,
                IdentityStatus::Invalid,
                "the NGP/NGPC header did not validate against the file",
            );
            return;
        };
        let platform = match fact.system_flag {
            NgpSystemFlag::Monochrome => IdentityPlatform::Ngp,
            NgpSystemFlag::Color => IdentityPlatform::Ngpc,
            NgpSystemFlag::Unknown(_) => {
                add_loose_rom_unavailable(
                    report,
                    IdentityStatus::Invalid,
                    "the NGP/NGPC header contains an unknown system flag",
                );
                return;
            }
        };
        report.platform = platform;
        ngp_header = Some(fact);
    }
    if (format == "a78" && atari7800_header.is_none()) || (format == "lnx" && lynx_header.is_none())
    {
        add_loose_rom_unavailable(
            report,
            IdentityStatus::Invalid,
            "the Atari cartridge header did not validate against the file",
        );
        return;
    }
    let is_n64 = report.platform == IdentityPlatform::N64;
    let is_snes = report.platform == IdentityPlatform::Snes;
    let needs_whole_file = is_n64 || is_snes || atari7800_header.is_some() || lynx_header.is_some();
    let mut whole_file_bytes: Option<Vec<u8>> = None;
    let digest = if needs_whole_file {
        // N64 needs the raw bytes afterward for byte-order detection and
        // canonical normalization, not just a hash - read once into memory
        // (already bounded by the `before.len <= MAX_LOOSE_ROM_BYTES` check
        // above) and hash that buffer the exact same way
        // [`hash_bounded_file`] would, rather than reading the file twice.
        if file.seek(SeekFrom::Start(0)).is_err() {
            add_loose_rom_unavailable(
                report,
                IdentityStatus::Invalid,
                "could not rewind the bounded Atari cartridge read",
            );
            return;
        }
        let mut bytes = Vec::with_capacity(before.len as usize);
        match file
            .by_ref()
            .take(MAX_LOOSE_ROM_BYTES)
            .read_to_end(&mut bytes)
        {
            Ok(_) => {}
            Err(error) => {
                add_loose_rom_unavailable(report, source_error_status(&error), &error.to_string());
                return;
            }
        }
        report.bytes_read = bytes.len() as u64;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        whole_file_bytes = Some(bytes);
        digest
    } else {
        match hash_bounded_file(&mut file, MAX_LOOSE_ROM_BYTES) {
            Ok((digest, bytes_read)) => {
                report.bytes_read = bytes_read;
                digest
            }
            Err(error) => {
                add_loose_rom_unavailable(report, source_error_status(&error), &error.to_string());
                return;
            }
        }
    };
    let after = match StableFileMetadata::from_file(&file) {
        Ok(metadata) => metadata,
        Err(error) => {
            add_loose_rom_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    if !loose_rom_read_was_stable(&before, &after, report.bytes_read) {
        add_loose_rom_unavailable(
            report,
            IdentityStatus::Invalid,
            "loose ROM changed while its identity was being read",
        );
        return;
    }

    report.evidence.push(evidence(
        report,
        IdentityKind::LooseRomSha256,
        IdentityStatus::Verified,
        Some(digest),
        IdentityConfidence::ExactBytes,
        "SHA-256 covers the exact on-disk bytes; it is not a known-good dump claim",
        "bounded full-file SHA-256",
    ));
    if let Some(fact) = nes_header {
        let header_format = if fact.is_nes20 { "NES 2.0" } else { "iNES" };
        let diagnostic = format!(
            "{header_format} header: mapper {}, submapper {}, PRG {} x16KiB, CHR {} x8KiB, trainer {}, battery {}, mirroring {:?}",
            fact.mapper,
            fact.submapper
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            fact.prg_rom_16k_units,
            fact.chr_rom_8k_units,
            fact.trainer,
            fact.battery,
            fact.mirroring,
        );
        report.evidence.push(evidence(
            report,
            IdentityKind::NesHeader,
            IdentityStatus::Verified,
            Some(header_format.to_string()),
            IdentityConfidence::StructuredMetadata,
            &diagnostic,
            "nes_header_evidence::parse_ines_header",
        ));
    }
    if let Some(fact) = atari7800_header {
        report.evidence.push(evidence(
            report,
            IdentityKind::Platform,
            IdentityStatus::Verified,
            Some(IdentityPlatform::Atari7800.label().to_string()),
            IdentityConfidence::StructuredMetadata,
            &format!(
                "ATARI7800 header validated: version {}, declared ROM payload {} bytes, title {:?}",
                fact.header_version, fact.rom_size, fact.cart_title
            ),
            "atari7800_header_evidence::parse_a78_header",
        ));
    }
    if let Some(fact) = lynx_header {
        report.evidence.push(evidence(
            report,
            IdentityKind::Platform,
            IdentityStatus::Verified,
            Some(IdentityPlatform::AtariLynx.label().to_string()),
            IdentityConfidence::StructuredMetadata,
            &format!(
                "LYNX header validated: version {}, bank page sizes {} and {}, name {:?}",
                fact.version, fact.bank0_page_size, fact.bank1_page_size, fact.cart_name
            ),
            "lynx_header_evidence::parse_lynx_header",
        ));
    }
    if let Some(fact) = ngp_header {
        report.evidence.push(evidence(
            report,
            IdentityKind::Platform,
            IdentityStatus::Verified,
            Some(report.platform.label().to_string()),
            IdentityConfidence::StructuredMetadata,
            &format!(
                "NGP cartridge header validated: system flag {:?}, software ID {:#06x}, version {}, title {:?}",
                fact.system_flag, fact.software_id, fact.version, fact.title
            ),
            "ngp_header_evidence::parse_ngp_header",
        ));
    }
    if let Some(bytes) = whole_file_bytes.as_deref() {
        if is_n64 {
            push_n64_canonical_evidence(report, bytes);
        } else if !is_snes {
            push_header_canonical_evidence(report, bytes);
        }
        if is_snes {
            push_snes_header_evidence(report, bytes);
        }
    }
    report.evidence.push(evidence(
        report,
        IdentityKind::LooseRomFormat,
        IdentityStatus::Verified,
        Some(format.to_string()),
        IdentityConfidence::StructuredMetadata,
        if format == "smd" {
            "format is recorded from the exact extension; bytes were not header-stripped or deinterleaved"
        } else {
            "format is recorded from the exact extension and trusted platform context"
        },
        "exact file extension",
    ));
    if let Some(title) = normalized_loose_rom_title(&report.archive_path) {
        report.evidence.push(evidence(
            report,
            IdentityKind::LooseRomTitle,
            IdentityStatus::Verified,
            Some(title),
            IdentityConfidence::CatalogueContext,
            "deterministic display title derived from the exact filename; not content identity",
            "filename stem normalization",
        ));
    } else {
        retain_warning(
            report,
            "ROM title contains unsupported path encoding; exact path bytes remain preserved",
        );
    }
    report.complete = true;
}

/// Emits the canonical SHA-256 for a verified Atari 7800 or Lynx headered
/// cartridge by reusing the already-reviewed, reversible header-normalization
/// transforms. The physical hash remains separate and is always retained.
fn push_header_canonical_evidence(report: &mut GameIdentityReport, bytes: &[u8]) {
    use crate::header_normalization::{recognize_header_normalization, strip_known_header};

    let Some(kind) = recognize_header_normalization(bytes)
        .into_iter()
        .find(|kind| {
            matches!(
                (report.platform, kind),
                (
                    IdentityPlatform::Atari7800,
                    crate::header_normalization::HeaderNormalizationKind::Atari7800_128
                ) | (
                    IdentityPlatform::AtariLynx,
                    crate::header_normalization::HeaderNormalizationKind::Lynx64
                )
            )
        })
    else {
        retain_warning(
            report,
            "verified Atari header was not available to the canonical normalization pass",
        );
        return;
    };
    let Ok(normalized) = strip_known_header(bytes, kind) else {
        retain_warning(
            report,
            "verified Atari header could not be reversibly stripped for canonical hashing",
        );
        return;
    };
    let mut hasher = Sha256::new();
    hasher.update(&normalized.bytes);
    let canonical_sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    report.evidence.push(evidence(
        report,
        IdentityKind::LooseRomCanonicalSha256,
        IdentityStatus::Verified,
        Some(canonical_sha256),
        IdentityConfidence::ExactBytes,
        "SHA-256 of the reversibly header-stripped Atari cartridge representation; distinct from the physical-file SHA-256",
        normalized.transform_id,
    ));
}

/// Promotes the existing checksum/complement-validated SNES header parser into
/// the production report. Internal title and header fields remain metadata,
/// not exact release identity.
fn push_snes_header_evidence(report: &mut GameIdentityReport, bytes: &[u8]) {
    let Some((fact, copier_header)) = inspect_snes_header(bytes) else {
        return;
    };
    let diagnostic = format!(
        "SNES {} header: title {:?}, map mode {:#04x}, cartridge type {:#04x}, ROM size code {:#04x}, RAM size code {:#04x}, destination {:#04x}, developer {:#04x}, version {:#04x}, checksum {:#06x}, complement {:#06x}, copier header {}",
        fact.mode.label(),
        fact.title,
        fact.map_mode_low_nibble,
        fact.cartridge_type,
        fact.rom_size_code,
        fact.ram_size_code,
        fact.destination_code,
        fact.developer_id,
        fact.version,
        fact.checksum,
        fact.checksum_complement,
        copier_header,
    );
    report.evidence.push(evidence(
        report,
        IdentityKind::SnesHeader,
        IdentityStatus::Verified,
        Some(fact.mode.label().to_string()),
        IdentityConfidence::StructuredMetadata,
        &diagnostic,
        "snes_header_evidence::best_snes_header_candidate",
    ));
}

/// Emits the canonical (byte-order-normalized `Z64`) SHA-256 for an N64
/// loose ROM, when [`detect_n64_byte_order`] recognizes `bytes`'s header and
/// [`normalize_to_z64`] succeeds - both pure, tested, already-existing
/// primitives from [`crate::n64_byte_order`], reused unchanged.
///
/// Silently adds nothing (only a retained warning) when the header is
/// unrecognized or the buffer's length doesn't match what the detected
/// order requires: a physically valid file with an unrecognized/malformed
/// byte-order header still keeps its exact-bytes [`IdentityKind::LooseRomSha256`] -
/// see the module docs on "physical vs. normalized identity" - it simply
/// never gets a canonical fact fabricated on top of it.
fn push_n64_canonical_evidence(report: &mut GameIdentityReport, bytes: &[u8]) {
    let Some(order) = detect_n64_byte_order(bytes) else {
        retain_warning(
            report,
            "N64 byte-order header not recognized; canonical normalized identity is \
             unavailable, but the exact physical-file SHA-256 above remains verified",
        );
        return;
    };
    let normalized = match normalize_to_z64(bytes, order) {
        Ok(result) => result,
        Err(_) => {
            retain_warning(
                report,
                "N64 byte-order normalization failed on a malformed buffer length; canonical \
                 normalized identity is unavailable, but the exact physical-file SHA-256 above \
                 remains verified",
            );
            return;
        }
    };
    let mut hasher = Sha256::new();
    hasher.update(&normalized.bytes);
    let canonical_sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    report.evidence.push(evidence(
        report,
        IdentityKind::LooseRomCanonicalSha256,
        IdentityStatus::Verified,
        Some(canonical_sha256),
        IdentityConfidence::ExactBytes,
        "SHA-256 of the byte-order-normalized (canonical Z64) image; distinct from the exact \
         physical-file SHA-256, and identical across z64/v64/n64 dumps of the same ROM",
        "byte-order detection + n64_byte_order::normalize_to_z64",
    ));
    let Some(cic) = cic_lookup(&normalized.bytes) else {
        return;
    };
    report.evidence.push(evidence(
        report,
        IdentityKind::N64Cic,
        IdentityStatus::Verified,
        Some(cic.label().to_string()),
        IdentityConfidence::StructuredMetadata,
        "canonical IPL3 bootcode CRC32 matches a bounded, two-source-verified CIC lookup; CIC is hardware/security metadata only",
        "n64_cic_evidence::cic_lookup",
    ));
    if let Some(header) = parse_n64_header(&normalized.bytes)
        && let Some(valid) = validate_crc1_crc2(&normalized.bytes, &header, cic)
    {
        report.evidence.push(evidence(
            report,
            IdentityKind::N64CrcValidation,
            if valid {
                IdentityStatus::Verified
            } else {
                IdentityStatus::Invalid
            },
            Some(if valid { "valid" } else { "invalid" }.to_string()),
            IdentityConfidence::StructuredMetadata,
            if valid {
                "header CRC1/CRC2 match the CIC-specific IPL3 checksum over the first 1 MiB after the boot area"
            } else {
                "header CRC1/CRC2 do not match the CIC-specific IPL3 checksum; no release identity is inferred"
            },
            "n64_cic_evidence::validate_crc1_crc2",
        ));
    }
}

fn loose_rom_read_was_stable(
    before: &StableFileMetadata,
    after: &StableFileMetadata,
    bytes_read: u64,
) -> bool {
    before == after && bytes_read == before.len
}

fn normalized_loose_rom_title(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let mut normalized = String::with_capacity(stem.len());
    let mut separator = true;
    for character in stem.chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
            separator = false;
        } else if !separator {
            normalized.push(' ');
            separator = true;
        }
    }
    normalized.truncate(normalized.trim_end().len());
    (!normalized.is_empty()).then_some(normalized)
}

fn add_loose_rom_unavailable(
    report: &mut GameIdentityReport,
    status: IdentityStatus,
    diagnostic: &str,
) {
    for kind in [IdentityKind::LooseRomSha256, IdentityKind::LooseRomFormat] {
        report.evidence.push(evidence(
            report,
            kind,
            status,
            None,
            IdentityConfidence::Unavailable,
            diagnostic,
            "loose cartridge ROM safety eligibility",
        ));
    }
}

fn hash_bounded_file(file: &mut File, maximum: u64) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("loose ROM byte count overflow"))?;
        if total > maximum {
            return Err(io::Error::other("loose ROM hash byte limit reached"));
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok((
        digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        total,
    ))
}

#[derive(Debug, PartialEq, Eq)]
struct StableFileMetadata {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl StableFileMetadata {
    fn from_file(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}

fn retain_warning(report: &mut GameIdentityReport, warning: &str) {
    if report.warnings.len() < MAX_LOOSE_ROM_WARNINGS.min(MAX_RETAINED_WARNINGS) {
        report.warnings.push(warning.to_string());
    }
}

fn inspect_direct_iso(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Iso;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    let mut source = FileSource {
        file,
        len,
        bytes_read: 0,
    };
    inspect_iso_source(report, &mut source, None, None);
    report.bytes_read = source.bytes_read;
}

fn inspect_direct_pkg(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Pkg;
    let fact = match crate::ps3_disc_evidence::observe_pkg_file(&report.archive_path, trusted) {
        Ok(fact) => fact,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    report.bytes_read = crate::ps3_disc_evidence::PKG_HEADER_BYTES as u64;
    let Some(title_id) =
        crate::ps3_disc_evidence::derive_title_id_from_content_id(&fact.content_id)
            .filter(|value| valid_ps3_title_id(value))
    else {
        add_unavailable(
            report,
            IdentityStatus::Missing,
            "valid PS3 PKG structure found, but its Content ID has no validated TITLE_ID",
        );
        return;
    };
    push_with_source(
        report,
        IdentityKind::Ps3TitleId,
        IdentityStatus::Verified,
        Some(title_id),
        IdentityConfidence::StructuredMetadata,
        None,
        None,
        "bounded PS3 PKG Content ID",
        "verified from the PKG fixed header; package contents, installation, and release identity remain uninspected",
    );
    report.complete = true;
}

fn inspect_cue(report: &mut GameIdentityReport, _trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Iso;
    let track = match resolve_data_track(&report.archive_path) {
        Ok(track) => track,
        Err(error) => {
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                &format!("CUE data track could not be resolved: {error}"),
            );
            return;
        }
    };
    let member_path = relative_member_path(report, &track.path);
    let mut source = match track.mode {
        CueDataTrackMode::Mode1_2048 => match open_cooked_cd_file_logical_media(&track.path) {
            Ok(media) => CueMediaSource::Cooked(MediaSource::new(media)),
            Err(error) => {
                add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
                return;
            }
        },
        CueDataTrackMode::Mode1_2352 | CueDataTrackMode::Mode2_2352 => {
            match open_raw_cd_file_logical_media(&track.path) {
                Ok(media) => CueMediaSource::Raw(MediaSource::new(media)),
                Err(error) => {
                    add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
                    return;
                }
            }
        }
    };
    inspect_iso_source(report, &mut source, member_path, None);
    report.bytes_read = source.bytes_read();
}

/// The path a resolved CUE/GDI data-track file should be recorded as in
/// [`IdentityProvenance::member_path`] - relative to the descriptor's own
/// directory when possible (matching a ZIP member name's shape: a name
/// relative to its container, not an absolute filesystem path), falling
/// back to the resolved path verbatim if it somehow does not share that
/// parent (it always does in practice, since [`resolve_data_track`]/
/// [`resolve_gdi_data_track`] only ever resolve within the descriptor's own
/// directory tree).
///
/// Without this, a CUE/GDI-derived [`IdentityEvidence`]'s provenance named
/// only the `.cue`/`.gdi` descriptor itself (`report.archive_path`) - never
/// which physical file the verified bytes actually came from. For a
/// multi-file release (a CUE with an audio track alongside its data track)
/// that made every verified fact indistinguishable from a plain single-file
/// `.iso` read, even though the bytes proving identity live in a *different*
/// physical file than the one the report names.
fn relative_member_path(report: &GameIdentityReport, resolved_path: &Path) -> Option<Vec<u8>> {
    let relative = report
        .archive_path
        .parent()
        .and_then(|dir| resolved_path.strip_prefix(dir).ok())
        .unwrap_or(resolved_path);
    Some(relative.to_string_lossy().into_owned().into_bytes())
}

/// Authoritative Dreamcast identity from a `.gdi` descriptor's own
/// high-density data track - the same [`inspect_iso_source`]/
/// [`inspect_dreamcast_source`] standard [`inspect_cue`] already meets for
/// CUE, reached through [`resolve_gdi_data_track`] instead of
/// [`resolve_data_track`]. No IP.BIN/product-code logic is duplicated here;
/// only the container-opening step differs from `inspect_cue`.
fn inspect_gdi(report: &mut GameIdentityReport, _trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Gdi;
    let track = match resolve_gdi_data_track(&report.archive_path) {
        Ok(track) => track,
        Err(error) => {
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                &format!("GDI data track could not be resolved: {error}"),
            );
            return;
        }
    };
    let member_path = relative_member_path(report, &track.path);
    let mut source = match track.mode {
        GdiDataTrackMode::Cooked2048 => match open_cooked_cd_file_logical_media(&track.path) {
            Ok(media) => CueMediaSource::Cooked(MediaSource::new(media)),
            Err(error) => {
                add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
                return;
            }
        },
        GdiDataTrackMode::Raw2352 => match open_raw_cd_file_logical_media(&track.path) {
            Ok(media) => CueMediaSource::Raw(MediaSource::new(media)),
            Err(error) => {
                add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
                return;
            }
        },
    };
    inspect_iso_source(report, &mut source, member_path, None);
    report.bytes_read = source.bytes_read();
}

fn inspect_zip_iso(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::ZipContainingIso;
    report.nested_container_depth = 1;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let mut archive = match ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                &format!("invalid ZIP: {error}"),
            );
            return;
        }
    };
    if archive.len() > MAX_ARCHIVE_MEMBERS {
        report.archive_members_inspected = MAX_ARCHIVE_MEMBERS;
        add_unavailable(
            report,
            IdentityStatus::ResourceLimitReached,
            "ZIP member limit reached before identity inspection",
        );
        return;
    }
    let mut iso_members = Vec::new();
    for index in 0..archive.len() {
        report.archive_members_inspected += 1;
        let raw = match archive.by_index_raw(index) {
            Ok(raw) => raw,
            Err(error) => {
                add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
                return;
            }
        };
        if raw.encrypted() {
            add_unavailable(
                report,
                IdentityStatus::Unsupported,
                "encrypted ZIP entries are refused",
            );
            return;
        }
        if !raw.is_dir() && ascii_extension_is_iso(raw.name_raw()) {
            iso_members.push((index, raw.name_raw().to_vec(), raw.size()));
        }
    }
    if iso_members.is_empty() {
        add_unavailable(
            report,
            IdentityStatus::Missing,
            "ZIP contains no ISO member",
        );
        return;
    }
    if iso_members.len() != 1 {
        add_unavailable(
            report,
            IdentityStatus::Ambiguous,
            "ZIP contains multiple ISO members; none was selected implicitly",
        );
        return;
    }
    let (index, member_path, member_size) = iso_members.remove(0);
    if member_path.len() > MAX_PATH_BYTES {
        add_unavailable(
            report,
            IdentityStatus::ResourceLimitReached,
            "ISO member path exceeds the path-length limit",
        );
        return;
    }
    let mut entry = match archive.by_index(index) {
        Ok(entry) => entry,
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    let read_cap = match report.platform {
        IdentityPlatform::GameCube | IdentityPlatform::Wii => {
            member_size.min(DOLPHIN_HEADER_BYTES as u64)
        }
        IdentityPlatform::PlayStation
        | IdentityPlatform::PlayStation2
        | IdentityPlatform::Psp
        | IdentityPlatform::PlayStation3
        | IdentityPlatform::Saturn
        | IdentityPlatform::Dreamcast
        | IdentityPlatform::SegaCd
        | IdentityPlatform::MegaDrive
        | IdentityPlatform::Snes
        | IdentityPlatform::Nes
        | IdentityPlatform::GameBoy
        | IdentityPlatform::GameBoyColor
        | IdentityPlatform::GameBoyAdvance
        | IdentityPlatform::N64
        | IdentityPlatform::Atari2600
        | IdentityPlatform::Atari5200
        | IdentityPlatform::Atari7800
        | IdentityPlatform::Atari8Bit
        | IdentityPlatform::AtariLynx
        | IdentityPlatform::AtariJaguar
        | IdentityPlatform::AtariST
        | IdentityPlatform::WiiU
        | IdentityPlatform::ThreeDS
        | IdentityPlatform::Switch
        | IdentityPlatform::Xbox
        | IdentityPlatform::Xbox360
        | IdentityPlatform::ScummVM
        | IdentityPlatform::ThreeDo
        | IdentityPlatform::Pcfx
        | IdentityPlatform::PcEngineCd
        | IdentityPlatform::NeoGeoCd
        | IdentityPlatform::Ngp
        | IdentityPlatform::Ngpc
        | IdentityPlatform::Other => member_size.min(MAX_BYTES_READ),
    };
    let mut data = Vec::with_capacity(read_cap.min(usize::MAX as u64) as usize);
    if let Err(error) = entry.by_ref().take(read_cap).read_to_end(&mut data) {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            &format!("could not read ISO member: {error}"),
        );
        return;
    }
    report.bytes_read = data.len() as u64;
    let mut source = SliceSource {
        data: &data,
        declared_len: member_size,
        truncated: member_size > data.len() as u64,
    };
    inspect_iso_source(report, &mut source, Some(member_path), Some(index));
}

fn inspect_direct_xex(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Xex;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    let mut source = FileSource {
        file,
        len,
        bytes_read: 0,
    };
    inspect_xex_header(report, &mut source, None, None);
    report.bytes_read = source.bytes_read;
}

fn inspect_zip_xex(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::ZipContainingXex;
    report.nested_container_depth = 1;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let mut archive = match ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                &format!("invalid ZIP: {error}"),
            );
            return;
        }
    };
    if archive.len() > MAX_ARCHIVE_MEMBERS {
        report.archive_members_inspected = MAX_ARCHIVE_MEMBERS;
        add_unavailable(
            report,
            IdentityStatus::ResourceLimitReached,
            "ZIP member limit reached before identity inspection",
        );
        return;
    }
    let mut xex_members = Vec::new();
    for index in 0..archive.len() {
        report.archive_members_inspected += 1;
        let raw = match archive.by_index_raw(index) {
            Ok(raw) => raw,
            Err(error) => {
                add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
                return;
            }
        };
        if raw.encrypted() {
            add_unavailable(
                report,
                IdentityStatus::Unsupported,
                "encrypted ZIP entries are refused",
            );
            return;
        }
        if !raw.is_dir() && ascii_extension_is_xex(raw.name_raw()) {
            xex_members.push((index, raw.name_raw().to_vec(), raw.size()));
        }
    }
    if xex_members.is_empty() {
        add_unavailable(
            report,
            IdentityStatus::Missing,
            "ZIP contains no XEX member",
        );
        return;
    }
    if xex_members.len() != 1 {
        add_unavailable(
            report,
            IdentityStatus::Ambiguous,
            "ZIP contains multiple XEX members; none was selected implicitly",
        );
        return;
    }
    let (index, member_path, member_size) = xex_members.remove(0);
    if member_path.len() > MAX_PATH_BYTES {
        add_unavailable(
            report,
            IdentityStatus::ResourceLimitReached,
            "XEX member path exceeds the path-length limit",
        );
        return;
    }
    let mut entry = match archive.by_index(index) {
        Ok(entry) => entry,
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    let read_cap = member_size.min(XEX_HEADER_PREFIX_BYTES);
    let mut data = Vec::with_capacity(read_cap.min(usize::MAX as u64) as usize);
    if let Err(error) = entry.by_ref().take(read_cap).read_to_end(&mut data) {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            &format!("could not read XEX member: {error}"),
        );
        return;
    }
    report.bytes_read = data.len() as u64;
    let mut source = SliceSource {
        data: &data,
        declared_len: member_size,
        truncated: member_size > data.len() as u64,
    };
    inspect_xex_header(report, &mut source, Some(member_path), Some(index));
}

/// Reads the unencrypted, uncompressed XEX2 module header: magic, the
/// optional-header table, and (when present) the execution-info optional
/// header holding `media_id`/`title_id`. Never reads the compressed or
/// encrypted module body.
fn inspect_xex_header(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let mut base = [0_u8; XEX_BASE_HEADER_BYTES];
    if let Err(error) = source.read_exact_at(0, &mut base) {
        let status = source_error_status(&error);
        push_with_source(
            report,
            IdentityKind::XexTitleId,
            status,
            None,
            IdentityConfidence::Unavailable,
            member_path.clone(),
            member_index,
            "bounded XEX header read",
            "XEX header is truncated or unavailable",
        );
        push_with_source(
            report,
            IdentityKind::XexMediaId,
            status,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "bounded XEX header read",
            "XEX header is truncated or unavailable",
        );
        return;
    }
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    if base[0..4] != XEX_MAGIC {
        push_with_source(
            report,
            IdentityKind::XexTitleId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path.clone(),
            member_index,
            "XEX2 magic check",
            "file does not begin with the XEX2 magic",
        );
        push_with_source(
            report,
            IdentityKind::XexMediaId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XEX2 magic check",
            "file does not begin with the XEX2 magic",
        );
        return;
    }
    let header_count = u32::from_be_bytes(
        base[XEX_HEADER_COUNT_OFFSET..XEX_HEADER_COUNT_OFFSET + 4]
            .try_into()
            .expect("4-byte slice"),
    );
    if header_count == 0 || header_count > MAX_XEX_OPT_HEADERS {
        push_with_source(
            report,
            IdentityKind::XexTitleId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path.clone(),
            member_index,
            "XEX optional header count",
            "optional header count is zero or exceeds the bounded limit",
        );
        push_with_source(
            report,
            IdentityKind::XexMediaId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XEX optional header count",
            "optional header count is zero or exceeds the bounded limit",
        );
        return;
    }
    let mut table = vec![0_u8; (u64::from(header_count) * XEX_OPT_HEADER_ENTRY_BYTES) as usize];
    if let Err(error) = source.read_exact_at(XEX_OPT_HEADER_TABLE_OFFSET, &mut table) {
        let status = source_error_status(&error);
        push_with_source(
            report,
            IdentityKind::XexTitleId,
            status,
            None,
            IdentityConfidence::Unavailable,
            member_path.clone(),
            member_index,
            "XEX optional header table read",
            "optional header table is truncated or unavailable",
        );
        push_with_source(
            report,
            IdentityKind::XexMediaId,
            status,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XEX optional header table read",
            "optional header table is truncated or unavailable",
        );
        return;
    }
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    let execution_info_offset = table.chunks_exact(8).find_map(|entry| {
        let key = u32::from_be_bytes(entry[0..4].try_into().expect("4-byte slice"));
        (key == XEX_EXECUTION_INFO_KEY)
            .then(|| u32::from_be_bytes(entry[4..8].try_into().expect("4-byte slice")))
    });
    let Some(offset) = execution_info_offset else {
        push_with_source(
            report,
            IdentityKind::XexTitleId,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
            member_path.clone(),
            member_index,
            "XEX optional header table",
            "no execution-info optional header is present",
        );
        push_with_source(
            report,
            IdentityKind::XexMediaId,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XEX optional header table",
            "no execution-info optional header is present",
        );
        return;
    };
    let mut execution_info = [0_u8; XEX_EXECUTION_INFO_BYTES];
    if let Err(error) = source.read_exact_at(u64::from(offset), &mut execution_info) {
        let status = source_error_status(&error);
        push_with_source(
            report,
            IdentityKind::XexTitleId,
            status,
            None,
            IdentityConfidence::Unavailable,
            member_path.clone(),
            member_index,
            "XEX execution-info optional header read",
            "execution-info header is truncated or unavailable",
        );
        push_with_source(
            report,
            IdentityKind::XexMediaId,
            status,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XEX execution-info optional header read",
            "execution-info header is truncated or unavailable",
        );
        return;
    }
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    let media_id = u32::from_be_bytes(execution_info[0x0..0x4].try_into().expect("4-byte slice"));
    let title_id = u32::from_be_bytes(execution_info[0xC..0x10].try_into().expect("4-byte slice"));
    push_with_source(
        report,
        IdentityKind::XexTitleId,
        IdentityStatus::Verified,
        Some(format!("{title_id:08X}")),
        IdentityConfidence::ExactBytes,
        member_path.clone(),
        member_index,
        "XEX execution-info optional header (title_id)",
        "verified directly from the reviewed XEX execution-info header",
    );
    push_with_source(
        report,
        IdentityKind::XexMediaId,
        IdentityStatus::Verified,
        Some(format!("{media_id:08X}")),
        IdentityConfidence::ExactBytes,
        member_path,
        member_index,
        "XEX execution-info optional header (media_id)",
        "verified directly from the reviewed XEX execution-info header",
    );
    report.complete = true;
}

fn inspect_direct_xbe(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Xbe;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    let mut source = FileSource {
        file,
        len,
        bytes_read: 0,
    };
    inspect_xbe_header(report, &mut source, None, None);
    report.bytes_read = source.bytes_read;
}

fn inspect_zip_xbe(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::ZipContainingXbe;
    report.nested_container_depth = 1;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let mut archive = match ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                &format!("invalid ZIP: {error}"),
            );
            return;
        }
    };
    if archive.len() > MAX_ARCHIVE_MEMBERS {
        report.archive_members_inspected = MAX_ARCHIVE_MEMBERS;
        add_unavailable(
            report,
            IdentityStatus::ResourceLimitReached,
            "ZIP member limit reached before identity inspection",
        );
        return;
    }
    let mut xbe_members = Vec::new();
    for index in 0..archive.len() {
        report.archive_members_inspected += 1;
        let raw = match archive.by_index_raw(index) {
            Ok(raw) => raw,
            Err(error) => {
                add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
                return;
            }
        };
        if raw.encrypted() {
            add_unavailable(
                report,
                IdentityStatus::Unsupported,
                "encrypted ZIP entries are refused",
            );
            return;
        }
        if !raw.is_dir() && ascii_extension_is_xbe(raw.name_raw()) {
            xbe_members.push((index, raw.name_raw().to_vec(), raw.size()));
        }
    }
    if xbe_members.is_empty() {
        add_unavailable(
            report,
            IdentityStatus::Missing,
            "ZIP contains no XBE member",
        );
        return;
    }
    if xbe_members.len() != 1 {
        add_unavailable(
            report,
            IdentityStatus::Ambiguous,
            "ZIP contains multiple XBE members; none was selected implicitly",
        );
        return;
    }
    let (index, member_path, member_size) = xbe_members.remove(0);
    if member_path.len() > MAX_PATH_BYTES {
        add_unavailable(
            report,
            IdentityStatus::ResourceLimitReached,
            "XBE member path exceeds the path-length limit",
        );
        return;
    }
    let mut entry = match archive.by_index(index) {
        Ok(entry) => entry,
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    // Bounded to the same generous prefix `xbox_boot_evidence` already reads
    // for a real disc's `default.xbe` (header plus a realistic certificate
    // offset) - reused here rather than inventing a new, unreviewed bound.
    let read_cap = member_size.min(crate::xbox_boot_evidence::XBE_PREFIX_READ_BYTES as u64);
    let mut data = Vec::with_capacity(read_cap.min(usize::MAX as u64) as usize);
    if let Err(error) = entry.by_ref().take(read_cap).read_to_end(&mut data) {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            &format!("could not read XBE member: {error}"),
        );
        return;
    }
    report.bytes_read = data.len() as u64;
    let mut source = SliceSource {
        data: &data,
        declared_len: member_size,
        truncated: member_size > data.len() as u64,
    };
    inspect_xbe_header(report, &mut source, Some(member_path), Some(index));
}

/// Authoritative original-Xbox identity from a whole disc image
/// (`.iso`/`.xiso`): locates `/default.xbe` in the XDVDFS filesystem via
/// [`crate::xdvdfs_traversal`]'s bounded, random-access, file-backed reader
/// (never materializing the whole image, and transparently handling both
/// raw-XISO and Redump-style offset-shifted dumps - see that module's own
/// doc comment), then reuses [`inspect_xbe_header`] unchanged on a bounded
/// prefix of its bytes - exactly the same identity authority a direct loose
/// `.xbe` file already has, just reached through a disc filesystem instead
/// of the plain filesystem.
fn inspect_direct_xbox_disc(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::XboxDiscImage;
    let mut file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    report.metadata_paths_inspected += 1;
    let entry = match crate::xdvdfs_traversal::find_path_in_disc_image(&mut file, "default.xbe") {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            add_unavailable(
                report,
                IdentityStatus::Missing,
                "no default.xbe was found at the Xbox disc image root",
            );
            return;
        }
        Err(error) => {
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                &format!("XDVDFS traversal failed: {error}"),
            );
            return;
        }
    };
    if entry.is_directory {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "default.xbe exists but is a directory, not a file",
        );
        return;
    }
    // Bounded to the same generous prefix `xbox_boot_evidence` already reads
    // for a real disc's `default.xbe` (header plus a realistic certificate
    // offset) - reused here rather than inventing a new, unreviewed bound.
    let prefix = match crate::xdvdfs_traversal::read_file_prefix_in_disc_image(
        &mut file,
        "default.xbe",
        crate::xbox_boot_evidence::XBE_PREFIX_READ_BYTES,
    ) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                "default.xbe could not be read after being located",
            );
            return;
        }
        Err(error) => {
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                &format!("XDVDFS traversal failed: {error}"),
            );
            return;
        }
    };
    report.bytes_read = prefix.len() as u64;
    let declared_len = u64::from(entry.size);
    let mut source = SliceSource {
        data: &prefix,
        declared_len,
        truncated: declared_len > prefix.len() as u64,
    };
    inspect_xbe_header(report, &mut source, None, None);
}

/// Reads the unencrypted original-Xbox XBE header plus the certificate at
/// its own declared (virtual-address-translated) file offset, holding the
/// `title_id` this platform's verified identity rests on. Reuses
/// [`crate::executable_signatures`]'s existing `looks_like_xbe`/
/// `parse_xbe_header`/`xbe_certificate_file_offset` unchanged - this
/// function only supplies the bounded I/O those pure functions need, never
/// reimplementing XBE's byte layout.
fn inspect_xbe_header(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let mut header = vec![0_u8; XBE_HEADER_PREFIX_BYTES];
    if let Err(error) = source.read_exact_at(0, &mut header) {
        let status = source_error_status(&error);
        push_with_source(
            report,
            IdentityKind::XbeTitleId,
            status,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "bounded XBE header read",
            "XBE header is truncated or unavailable",
        );
        return;
    }
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    if !looks_like_xbe(&header) {
        push_with_source(
            report,
            IdentityKind::XbeTitleId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XBE magic check",
            "file does not begin with the XBEH magic",
        );
        return;
    }
    let Some(certificate_offset) = xbe_certificate_file_offset(&header) else {
        push_with_source(
            report,
            IdentityKind::XbeTitleId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XBE certificate address",
            "certificate address underflows the header's own base address",
        );
        return;
    };
    let mut certificate = vec![0_u8; XBE_CERTIFICATE_READ_BYTES];
    if let Err(error) = source.read_exact_at(certificate_offset, &mut certificate) {
        let status = source_error_status(&error);
        push_with_source(
            report,
            IdentityKind::XbeTitleId,
            status,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XBE certificate read",
            "certificate is truncated or unavailable",
        );
        return;
    }
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    // `looks_like_xbe` already passed above, so `parse_xbe_header` can only
    // return `None` here if the header itself were somehow shorter than
    // `XBE_HEADER_PREFIX_BYTES` - impossible given the fixed-size read above
    // succeeded. Handled explicitly anyway rather than unwrapped, so a
    // future change to either function still fails closed instead of
    // panicking.
    let Some(fact) = parse_xbe_header(&header, Some(&certificate)) else {
        push_with_source(
            report,
            IdentityKind::XbeTitleId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XBE header parse",
            "XBE header failed to parse",
        );
        return;
    };
    let Some(title_id) = fact.title_id else {
        push_with_source(
            report,
            IdentityKind::XbeTitleId,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "XBE certificate",
            "no title ID is present in the certificate",
        );
        return;
    };
    push_with_source(
        report,
        IdentityKind::XbeTitleId,
        IdentityStatus::Verified,
        Some(title_id),
        IdentityConfidence::ExactBytes,
        member_path,
        member_index,
        "XBE certificate (title_id)",
        "verified directly from the reviewed XBE certificate",
    );
    report.complete = true;
}

/// Authoritative PS3 identity from an ISO9660 disc. The existing PS3 evidence
/// standard is deliberately strict: the PS3_GAME layout, a bounded
/// PARAM.SFO TITLE_ID, and a valid SELF header in USRDIR/EBOOT.BIN must all
/// agree before a title ID becomes verified.
fn inspect_ps3_iso(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let root = match iso_root(source) {
        Ok(root) => root,
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::Ps3TitleId,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "ISO 9660 primary volume descriptor",
                &diagnostic,
            );
            return;
        }
    };
    report.metadata_paths_inspected += 1;
    let has_game_dir =
        matches!(find_iso_path(source, root, &[b"PS3_GAME"]), Ok(Some(record)) if record.directory);
    if !has_game_dir {
        push_with_source(
            report,
            IdentityKind::Ps3TitleId,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PS3 disc layout validation",
            "PS3_GAME directory is missing",
        );
        return;
    }
    let sfo = match find_iso_path(source, root, &[b"PS3_GAME", b"PARAM.SFO"]) {
        Ok(Some(record)) if !record.directory => record,
        Ok(_) => {
            push_with_source(
                report,
                IdentityKind::Ps3TitleId,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "PS3 PARAM.SFO lookup",
                "PS3_GAME/PARAM.SFO is missing",
            );
            return;
        }
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::Ps3TitleId,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "PS3 PARAM.SFO lookup",
                &diagnostic,
            );
            return;
        }
    };
    if sfo.size > crate::param_sfo::MAX_SFO_BYTES as u64 {
        push_with_source(
            report,
            IdentityKind::Ps3TitleId,
            IdentityStatus::ResourceLimitReached,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "bounded PARAM.SFO read",
            "PARAM.SFO exceeds the bounded PS3 metadata size",
        );
        return;
    }
    let bytes = match read_iso_record(source, sfo) {
        Ok(bytes) => bytes,
        Err(error) => {
            push_with_source(
                report,
                IdentityKind::Ps3TitleId,
                source_error_status(&error),
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "bounded PARAM.SFO read",
                &error.to_string(),
            );
            return;
        }
    };
    let Some(sfo) = crate::param_sfo::parse_param_sfo(&bytes) else {
        push_with_source(
            report,
            IdentityKind::Ps3TitleId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PARAM.SFO validation",
            "PARAM.SFO is malformed",
        );
        return;
    };
    let Some(raw_title_id) = sfo.get_text("TITLE_ID") else {
        push_with_source(
            report,
            IdentityKind::Ps3TitleId,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PARAM.SFO TITLE_ID lookup",
            "TITLE_ID is missing",
        );
        return;
    };
    let title_id = raw_title_id.trim().to_ascii_uppercase();
    if !valid_ps3_title_id(&title_id) {
        push_with_source(
            report,
            IdentityKind::Ps3TitleId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PARAM.SFO TITLE_ID validation",
            "TITLE_ID is malformed",
        );
        return;
    }
    let eboot = match find_iso_path(source, root, &[b"PS3_GAME", b"USRDIR", b"EBOOT.BIN"]) {
        Ok(Some(record)) if !record.directory && record.size >= 4 => record,
        Ok(_) => {
            push_with_source(
                report,
                IdentityKind::Ps3TitleId,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "PS3 SELF lookup",
                "PS3_GAME/USRDIR/EBOOT.BIN is missing or too small",
            );
            return;
        }
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::Ps3TitleId,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "PS3 SELF lookup",
                &diagnostic,
            );
            return;
        }
    };
    let mut header = [0_u8; 4];
    if let Err(error) = read_iso_record_prefix(source, eboot, &mut header) {
        push_with_source(
            report,
            IdentityKind::Ps3TitleId,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "bounded PS3 SELF header read",
            &error.to_string(),
        );
        return;
    }
    if !looks_like_self(&header) {
        push_with_source(
            report,
            IdentityKind::Ps3TitleId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PS3 SELF validation",
            "EBOOT.BIN does not have the PS3 SELF magic",
        );
        return;
    }
    push_with_source(
        report,
        IdentityKind::Ps3TitleId,
        IdentityStatus::Verified,
        Some(title_id),
        IdentityConfidence::StructuredMetadata,
        member_path,
        member_index,
        "PS3_GAME/PARAM.SFO TITLE_ID plus PS3 SELF EBOOT",
        "verified from PS3 disc content",
    );
    report.complete = true;
}

fn valid_ps3_title_id(value: &str) -> bool {
    value.len() == 9
        && value.as_bytes()[..4]
            .iter()
            .all(|byte| byte.is_ascii_uppercase())
        && value.as_bytes()[4..]
            .iter()
            .all(|byte| byte.is_ascii_digit())
}

fn inspect_ps3_directory_identity(report: &mut GameIdentityReport) {
    let root = &report.archive_path;
    if !ps3_directory_paths_are_regular(root) {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "PS3 directory contains an unsafe path component or symlink",
        );
        return;
    }
    let observation = observe_ps3_directory(root);
    let Some(raw_title_id) = observation.layout.title_id() else {
        add_unavailable(
            report,
            IdentityStatus::Missing,
            "PS3 PARAM.SFO TITLE_ID is missing",
        );
        return;
    };
    let title_id = raw_title_id.trim().to_ascii_uppercase();
    if !valid_ps3_title_id(&title_id) {
        add_unavailable(report, IdentityStatus::Invalid, "PS3 TITLE_ID is malformed");
        return;
    }
    if !observation.layout.ps3_game_dir_present
        || !observation.layout.usrdir_present
        || !observation.layout.eboot_bin_present
        || observation.layout.eboot_self_magic_present != Some(true)
    {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "PS3_GAME, USRDIR/EBOOT.BIN, and valid SELF magic are required",
        );
        return;
    }
    push_with_source(
        report,
        IdentityKind::Ps3TitleId,
        IdentityStatus::Verified,
        Some(title_id),
        IdentityConfidence::StructuredMetadata,
        None,
        None,
        "PS3_GAME/PARAM.SFO TITLE_ID plus PS3 SELF EBOOT",
        "verified from PS3 folder content",
    );
    report.complete = true;
}

fn ps3_directory_paths_are_regular(root: &Path) -> bool {
    if !root.is_absolute()
        || root.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return false;
    }
    let game_dir = if root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("PS3_GAME"))
    {
        root.to_path_buf()
    } else {
        root.join("PS3_GAME")
    };
    let paths = [
        root.to_path_buf(),
        game_dir.clone(),
        game_dir.join("USRDIR"),
        game_dir.join("PARAM.SFO"),
        game_dir.join("USRDIR").join("EBOOT.BIN"),
    ];
    let mut component_path = PathBuf::new();
    let components_safe = root.components().all(|component| {
        component_path.push(component.as_os_str());
        std::fs::symlink_metadata(&component_path)
            .is_ok_and(|metadata| !metadata.file_type().is_symlink())
    });
    components_safe
        && paths.iter().all(|path| {
            std::fs::symlink_metadata(path).is_ok_and(|metadata| {
                !metadata.file_type().is_symlink()
                    && ((path.extension().is_some() && metadata.is_file())
                        || (path.extension().is_none() && metadata.is_dir()))
            })
        })
}

/// Inspects a direct `.pbp` using the existing bounded PBP container parser.
/// The complete file is never required: the fixed header and the beginning of
/// the declared sections are read through the normal 64 MiB identity bound.
/// A PBP is shared by native PSP content and PS1 Classics, so this routine
/// never turns the extension or the container alone into a PSP/PS1 identity.
fn inspect_pbp(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Pbp;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let file_len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    let read_len = file_len.min(MAX_BYTES_READ);
    let mut bytes = Vec::with_capacity(read_len as usize);
    if let Err(error) = file.take(read_len).read_to_end(&mut bytes) {
        add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
        return;
    }
    report.bytes_read = bytes.len() as u64;
    let Some(header) = parse_pbp_header(&bytes[..bytes.len().min(PBP_HEADER_BYTES)]) else {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "PBP fixed header is malformed or truncated",
        );
        return;
    };
    if let Err(error) = validate_pbp_offsets(&header, file_len) {
        add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
        return;
    }
    let sfo = read_pbp_param_sfo(&bytes, &header);
    let container_detail = observe_pbp_evidence(sfo.as_ref())
        .into_iter()
        .next()
        .map(|item| item.detail)
        .unwrap_or_else(|| "PBP fixed header and section offsets validated".to_string());
    report.warnings.push(container_detail);

    if report.platform == IdentityPlatform::Psp {
        let Some(raw_id) = sfo.as_ref().and_then(|value| value.get_text("DISC_ID")) else {
            add_unavailable(
                report,
                IdentityStatus::Missing,
                "PBP PARAM.SFO has no DISC_ID",
            );
            return;
        };
        let id = raw_id.trim().to_ascii_uppercase();
        let valid = id.len() == 9
            && id.as_bytes()[..4].iter().all(u8::is_ascii_uppercase)
            && id.as_bytes()[4..].iter().all(u8::is_ascii_digit);
        if !valid {
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                "PBP PARAM.SFO DISC_ID is not a valid product identifier",
            );
            return;
        }
        push_with_source(
            report,
            IdentityKind::PspDiscId,
            IdentityStatus::Verified,
            Some(id),
            IdentityConfidence::StructuredMetadata,
            None,
            None,
            "PBP fixed header plus PARAM.SFO DISC_ID",
            "verified from a structurally valid PBP container",
        );
        report.complete = true;
    } else {
        add_unavailable(
            report,
            IdentityStatus::Unsupported,
            "PBP structure is shared with PS1 Classics; no PS1 serial is manufactured from PSAR markers",
        );
    }
}

fn inspect_iso_source(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    match report.platform {
        IdentityPlatform::PlayStation => inspect_ps1_iso(report, source, member_path, member_index),
        IdentityPlatform::Psp => inspect_psp_iso(report, source, member_path, member_index),
        IdentityPlatform::PlayStation3 => {
            inspect_ps3_iso(report, source, member_path, member_index)
        }
        IdentityPlatform::Saturn => {
            inspect_saturn_source(report, source, member_path, member_index)
        }
        IdentityPlatform::Dreamcast => {
            inspect_dreamcast_source(report, source, member_path, member_index)
        }
        IdentityPlatform::SegaCd => {
            inspect_sega_cd_source(report, source, member_path, member_index)
        }
        IdentityPlatform::ThreeDo => {
            inspect_threedo_source(report, source, member_path, member_index)
        }
        IdentityPlatform::Pcfx => inspect_pcfx_source(report, source, member_path, member_index),
        IdentityPlatform::PcEngineCd => {
            inspect_pcengine_cd_source(report, source, member_path, member_index)
        }
        IdentityPlatform::NeoGeoCd => {
            inspect_neogeocd_source(report, source, member_path, member_index)
        }
        IdentityPlatform::GameCube | IdentityPlatform::Wii => {
            inspect_dolphin_header(report, source, member_path, member_index, 0)
        }
        IdentityPlatform::PlayStation2 => {
            inspect_ps2_iso(report, source, member_path, member_index)
        }
        IdentityPlatform::MegaDrive
        | IdentityPlatform::Snes
        | IdentityPlatform::Nes
        | IdentityPlatform::GameBoy
        | IdentityPlatform::GameBoyColor
        | IdentityPlatform::GameBoyAdvance
        | IdentityPlatform::N64
        | IdentityPlatform::Atari2600
        | IdentityPlatform::Atari5200
        | IdentityPlatform::Atari7800
        | IdentityPlatform::Atari8Bit
        | IdentityPlatform::AtariLynx
        | IdentityPlatform::AtariJaguar
        | IdentityPlatform::AtariST
        | IdentityPlatform::WiiU
        | IdentityPlatform::ThreeDS
        | IdentityPlatform::Switch
        | IdentityPlatform::Xbox
        | IdentityPlatform::Xbox360
        | IdentityPlatform::ScummVM
        | IdentityPlatform::Ngp
        | IdentityPlatform::Ngpc
        | IdentityPlatform::Other => {}
    }
}

/// Authoritative PSP identity from a UMD-style ISO filesystem. `PSP_GAME` and
/// `PARAM.SFO` are not sufficient on their own because they overlap with the
/// Sony ecosystem; the root `UMD_DATA.BIN` marker and a structurally valid
/// `DISC_ID` are required together. This reuses the existing bounded ISO
/// reader and PARAM.SFO parser and never consults a filename.
fn inspect_psp_iso(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let root = match iso_root(source) {
        Ok(root) => root,
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::PspDiscId,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "ISO 9660 primary volume descriptor",
                &diagnostic,
            );
            return;
        }
    };
    report.metadata_paths_inspected += 1;
    let psp_dir =
        matches!(find_iso_path(source, root, &[b"PSP_GAME"]), Ok(Some(record)) if record.directory);
    let umd = matches!(find_iso_path(source, root, &[b"UMD_DATA.BIN"]), Ok(Some(record)) if !record.directory);
    if !psp_dir || !umd {
        push_with_source(
            report,
            IdentityKind::PspDiscId,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PSP UMD layout validation",
            "PSP_GAME and root UMD_DATA.BIN are both required",
        );
        return;
    }
    let sfo = match find_iso_path(source, root, &[b"PSP_GAME", b"PARAM.SFO"]) {
        Ok(Some(record)) if !record.directory => record,
        Ok(_) => {
            push_with_source(
                report,
                IdentityKind::PspDiscId,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "PSP PARAM.SFO lookup",
                "PSP_GAME/PARAM.SFO is missing",
            );
            return;
        }
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::PspDiscId,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "PSP PARAM.SFO lookup",
                &diagnostic,
            );
            return;
        }
    };
    if sfo.size > MAX_SYSTEM_CNF_BYTES {
        push_with_source(
            report,
            IdentityKind::PspDiscId,
            IdentityStatus::ResourceLimitReached,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "bounded PARAM.SFO read",
            "PARAM.SFO exceeds 64 KiB",
        );
        return;
    }
    let bytes = match read_iso_record(source, sfo) {
        Ok(bytes) => bytes,
        Err(error) => {
            push_with_source(
                report,
                IdentityKind::PspDiscId,
                source_error_status(&error),
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "bounded PARAM.SFO read",
                &error.to_string(),
            );
            return;
        }
    };
    let Some(sfo) = parse_param_sfo(&bytes) else {
        push_with_source(
            report,
            IdentityKind::PspDiscId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PARAM.SFO validation",
            "PARAM.SFO is malformed",
        );
        return;
    };
    let Some(raw_id) = sfo.get_text("DISC_ID") else {
        push_with_source(
            report,
            IdentityKind::PspDiscId,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PARAM.SFO DISC_ID lookup",
            "DISC_ID is missing",
        );
        return;
    };
    let id = raw_id.trim().to_ascii_uppercase();
    let valid = id.len() == 9
        && id.as_bytes()[..4].iter().all(u8::is_ascii_uppercase)
        && id.as_bytes()[4..].iter().all(u8::is_ascii_digit);
    if !valid {
        push_with_source(
            report,
            IdentityKind::PspDiscId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PARAM.SFO DISC_ID validation",
            "DISC_ID is not a valid PSP product identifier",
        );
        return;
    }
    push_with_source(
        report,
        IdentityKind::PspDiscId,
        IdentityStatus::Verified,
        Some(id),
        IdentityConfidence::StructuredMetadata,
        member_path,
        member_index,
        "PSP UMD_DATA.BIN plus PARAM.SFO DISC_ID",
        "verified from PSP UMD content",
    );
    report.complete = true;
}

/// Authoritative Saturn identity from the fixed System ID at logical sector
/// zero. The source may be a plain ISO, a CUE/BIN logical view, a ZIP member,
/// or another already-opened bounded logical medium; media/container parsing
/// remains outside this identity check.
fn inspect_saturn_source(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let mut header = [0_u8; SATURN_SYSTEM_ID_BYTES];
    if let Err(error) = source.read_exact_at(0, &mut header) {
        push_with_source(
            report,
            IdentityKind::SaturnProductNumber,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Saturn System ID bounded read",
            &error.to_string(),
        );
        return;
    }
    let Some(fact) = parse_saturn_system_id(&header) else {
        push_with_source(
            report,
            IdentityKind::SaturnProductNumber,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Saturn System ID parse",
            "Saturn System ID is truncated or malformed",
        );
        return;
    };
    if !fact.hardware_id_recognized {
        push_with_source(
            report,
            IdentityKind::SaturnProductNumber,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Saturn System ID hardware signature",
            "disc does not contain the verified SEGA SEGASATURN boot signature",
        );
        return;
    }
    if fact.product_number.is_empty()
        || !fact
            .product_number
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
    {
        push_with_source(
            report,
            IdentityKind::SaturnProductNumber,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Saturn System ID product number",
            "Saturn System ID has no valid printable product number",
        );
        return;
    }
    push_with_source(
        report,
        IdentityKind::SaturnProductNumber,
        IdentityStatus::Verified,
        Some(fact.product_number),
        IdentityConfidence::ExactBytes,
        member_path,
        member_index,
        "Saturn System ID product number",
        "product number read from a verified SEGA SEGASATURN System ID",
    );
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    report.complete = true;
}

/// Authoritative Dreamcast identity from the existing fixed IP.BIN metadata
/// area at logical offset zero. Container and sector decoding stay in the
/// caller's already-opened bounded logical source.
fn inspect_dreamcast_source(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let mut header = [0_u8; IP_BIN_META_BYTES];
    if let Err(error) = source.read_exact_at(0, &mut header) {
        push_with_source(
            report,
            IdentityKind::DreamcastProductCode,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Dreamcast IP.BIN bounded read",
            &error.to_string(),
        );
        return;
    }
    let Some(fact) = parse_ip_bin_meta(&header) else {
        push_with_source(
            report,
            IdentityKind::DreamcastProductCode,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Dreamcast IP.BIN parse",
            "IP.BIN metadata is truncated or malformed",
        );
        return;
    };
    if !fact.hardware_id_recognized {
        push_with_source(
            report,
            IdentityKind::DreamcastProductCode,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Dreamcast IP.BIN hardware signature",
            "disc does not contain a recognised Sega Dreamcast boot signature",
        );
        return;
    }
    if fact.product_number.is_empty()
        || !fact
            .product_number
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
    {
        push_with_source(
            report,
            IdentityKind::DreamcastProductCode,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Dreamcast IP.BIN product code",
            "IP.BIN has no valid printable product code",
        );
        return;
    }
    push_with_source(
        report,
        IdentityKind::DreamcastProductCode,
        IdentityStatus::Verified,
        Some(fact.product_number),
        IdentityConfidence::ExactBytes,
        member_path,
        member_index,
        "Dreamcast IP.BIN product code",
        "product code read from a recognised Sega Dreamcast IP.BIN boot structure",
    );
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    report.complete = true;
}

/// Authoritative Sega CD identity from the fixed Disc ID product field at
/// `$180` in the validated `SEGADISCSYSTEM` boot sector. The caller supplies
/// an already-decoded 2048-byte logical sector view, so ISO, CUE/BIN, and the
/// existing simple-track CHD reader use exactly the same bounded check.
fn inspect_sega_cd_source(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let mut header = [0_u8; SEGA_CD_DISC_ID_BYTES];
    if let Err(error) = source.read_exact_at(0, &mut header) {
        push_with_source(
            report,
            IdentityKind::SegaCdProductCode,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Sega CD Disc ID bounded read",
            &error.to_string(),
        );
        return;
    }
    let Some(fact) = parse_segacd_product_code(&header) else {
        push_with_source(
            report,
            IdentityKind::SegaCdProductCode,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Sega CD Disc ID product field",
            "logical sector zero is not a valid Sega CD system area with a product code",
        );
        return;
    };
    push_with_source(
        report,
        IdentityKind::SegaCdProductCode,
        IdentityStatus::Verified,
        Some(fact.product_code),
        IdentityConfidence::ExactBytes,
        member_path,
        member_index,
        "Sega CD Disc ID product field",
        &format!(
            "product code read from SEGADISCSYSTEM Disc ID; raw field {:?}",
            fact.raw_product_code
        ),
    );
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    report.complete = true;
}

/// Authoritative 3DO identity from the Opera filesystem volume header at
/// logical offset zero. The composite value uses only documented structured
/// fields (volume identifier, root unique identifier, and block count); the
/// volume label and path are deliberately not identity authority.
fn inspect_threedo_source(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let mut header = [0_u8; OPERA_HEADER_BYTES];
    if let Err(error) = source.read_exact_at(0, &mut header) {
        push_with_source(
            report,
            IdentityKind::ThreeDoDiscId,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "3DO Opera volume header bounded read",
            &error.to_string(),
        );
        return;
    }
    let Some(fact) = parse_opera_volume_header(&header) else {
        push_with_source(
            report,
            IdentityKind::ThreeDoDiscId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "3DO Opera volume header parse",
            "Opera volume header is truncated or malformed",
        );
        return;
    };
    if !fact.identity_is_valid() {
        push_with_source(
            report,
            IdentityKind::ThreeDoDiscId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "3DO Opera volume header validation",
            "Opera header signature or logical block geometry is invalid",
        );
        return;
    }
    let Some(declared_bytes) = (fact.block_size as u64).checked_mul(fact.block_count as u64) else {
        push_with_source(
            report,
            IdentityKind::ThreeDoDiscId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "3DO Opera volume size arithmetic",
            "declared Opera volume size overflows",
        );
        return;
    };
    if declared_bytes > source.len() {
        push_with_source(
            report,
            IdentityKind::ThreeDoDiscId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "3DO Opera volume size validation",
            "declared Opera volume extends beyond the readable image",
        );
        return;
    }
    let value = fact.disc_identity();
    push_with_source(
        report,
        IdentityKind::ThreeDoDiscId,
        IdentityStatus::Verified,
        Some(value),
        IdentityConfidence::StructuredMetadata,
        member_path,
        member_index,
        "3DO Opera volume header identity",
        "volume identifier, root unique identifier, and block count read from validated OperaFS header",
    );
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    report.complete = true;
}

/// Authoritative PC-FX identity using the documented local disc hash:
/// sector-0 signature bytes, sector-1 volume header, and the header-directed
/// boot code. All reads are bounded and use the existing logical-media
/// source; no filename or title database participates.
fn inspect_pcfx_source(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let mut sector_zero = [0_u8; PCFX_BOOT_SECTOR_BYTES];
    if let Err(error) = source.read_exact_at(0, &mut sector_zero) {
        push_with_source(
            report,
            IdentityKind::PcfxDiscHash,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC-FX sector-zero bounded read",
            &error.to_string(),
        );
        return;
    }
    let boot_fact = parse_pcfx_boot_sector(&sector_zero);
    if !boot_fact.any_magic_present() {
        push_with_source(
            report,
            IdentityKind::PcfxDiscHash,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC-FX boot signature validation",
            "sector zero does not contain a reviewed PC-FX boot signature",
        );
        return;
    }
    let mut sector_one = [0_u8; PCFX_BOOT_SECTOR_BYTES];
    if let Err(error) = source.read_exact_at(PCFX_BOOT_SECTOR_BYTES as u64, &mut sector_one) {
        push_with_source(
            report,
            IdentityKind::PcfxDiscHash,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC-FX sector-one bounded read",
            &error.to_string(),
        );
        return;
    }
    let Some(header) = parse_pcfx_volume_header(&sector_one[..PCFX_VOLUME_HEADER_BYTES]) else {
        push_with_source(
            report,
            IdentityKind::PcfxDiscHash,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC-FX volume header validation",
            "sector one has no valid boot-sector/count geometry",
        );
        return;
    };
    let Some(boot_offset) = (header.boot_sector as u64).checked_mul(PCFX_BOOT_SECTOR_BYTES as u64)
    else {
        push_with_source(
            report,
            IdentityKind::PcfxDiscHash,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC-FX boot offset arithmetic",
            "boot-sector offset overflows",
        );
        return;
    };
    let boot_bytes = match usize::try_from(header.boot_sector_count)
        .ok()
        .and_then(|count| count.checked_mul(PCFX_BOOT_SECTOR_BYTES))
    {
        Some(bytes) => bytes,
        None => {
            push_with_source(
                report,
                IdentityKind::PcfxDiscHash,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "PC-FX boot length arithmetic",
                "boot-code length overflows",
            );
            return;
        }
    };
    let mut boot_code = vec![0_u8; boot_bytes];
    if let Err(error) = source.read_exact_at(boot_offset, &mut boot_code) {
        push_with_source(
            report,
            IdentityKind::PcfxDiscHash,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC-FX boot-code bounded read",
            &error.to_string(),
        );
        return;
    }
    let Some(hash) = pcfx_disc_hash(&sector_zero, &sector_one, &boot_code) else {
        push_with_source(
            report,
            IdentityKind::PcfxDiscHash,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC-FX disc hash construction",
            "PC-FX identity hash inputs were incomplete",
        );
        return;
    };
    push_with_source(
        report,
        IdentityKind::PcfxDiscHash,
        IdentityStatus::Verified,
        Some(hash),
        IdentityConfidence::ExactBytes,
        member_path,
        member_index,
        "PC-FX documented custom disc hash",
        "hash covers sector-zero signature bytes, the sector-one volume header, and header-directed boot sectors",
    );
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    report.complete = true;
}

/// PC Engine CD-ROM² / TurboGrafx-CD structural identity from the IPL
/// boot-record in the first data track's second sector (LBA 1). The only
/// authority is the fixed `PC Engine CD-ROM SYSTEM` signature at offset 32
/// plus a self-consistent boot-program pointer; no serial, title or region is
/// read. All reads are bounded and use the shared logical-media source.
fn inspect_pcengine_cd_source(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let mut ipl_record = [0_u8; PCE_CD_IPL_HEADER_BYTES];
    if let Err(error) = source.read_exact_at(PCE_CD_IPL_SECTOR_OFFSET, &mut ipl_record) {
        push_with_source(
            report,
            IdentityKind::PceCdBootStructure,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC Engine CD IPL boot-record bounded read",
            &error.to_string(),
        );
        return;
    }
    let Some(fact) = parse_pce_cd_ipl(&ipl_record) else {
        push_with_source(
            report,
            IdentityKind::PceCdBootStructure,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC Engine CD IPL signature validation",
            "the first data track's second sector has no `PC Engine CD-ROM SYSTEM` IPL signature at offset 32",
        );
        return;
    };
    let Some(boot_span) = fact.boot_span_bytes() else {
        push_with_source(
            report,
            IdentityKind::PceCdBootStructure,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC Engine CD IPL boot-span arithmetic",
            "the declared boot-program span overflows",
        );
        return;
    };
    if boot_span > source.len() {
        push_with_source(
            report,
            IdentityKind::PceCdBootStructure,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PC Engine CD IPL boot-span validation",
            "the IPL boot-record points its boot program past the end of the readable data track",
        );
        return;
    }
    push_with_source(
        report,
        IdentityKind::PceCdBootStructure,
        IdentityStatus::Verified,
        Some("PC Engine CD-ROM SYSTEM".to_string()),
        IdentityConfidence::StructuredMetadata,
        member_path,
        member_index,
        "PC Engine CD-ROM² IPL boot-record",
        &format!(
            "IPL signature present at offset 32 of the data track's second sector; boot program \
             {} sector(s) from sector {} lies within the image. Structural platform/media \
             evidence only - exact game identity comes from a DAT/hash match.",
            fact.boot_sector_count, fact.boot_start_sector
        ),
    );
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    report.complete = true;
}

/// Neo Geo CD structural identity from the disc's own `IPL.TXT` load
/// manifest in the ISO 9660 root directory. Neo Geo CD discs are plain
/// ISO 9660 CD-ROMs (the filesystem proves nothing on its own - PSX, Sega
/// CD, CD-i, 3DO and PC Engine CD share it); the authority is the parsed,
/// structurally validated `IPL.TXT` (bounded entry list, terminator byte
/// present), read through the *existing* bounded ISO 9660 reader
/// ([`iso_root`]/[`find_iso_path`]) and the *existing* bounded
/// [`parse_ipl_txt`] parser. No new IPL or optical parser: this only turns
/// their results into an [`IdentityStatus`]. No serial, title or region is
/// read - the IPL manifest carries none - so exact game identity still
/// comes from a DAT/hash match. A file merely *named* `IPL.TXT` that does
/// not parse fails closed, never `Verified`, and the filename is never
/// consulted.
fn inspect_neogeocd_source(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let root = match iso_root(source) {
        Ok(root) => root,
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::NeoGeoCdBootStructure,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "Neo Geo CD ISO 9660 root directory lookup",
                &diagnostic,
            );
            return;
        }
    };
    let ipl_record = match find_iso_path(source, root, &[b"IPL.TXT"]) {
        Ok(Some(record)) if !record.directory => record,
        Ok(Some(_)) | Ok(None) => {
            push_with_source(
                report,
                IdentityKind::NeoGeoCdBootStructure,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "Neo Geo CD IPL.TXT root lookup",
                "the ISO 9660 root directory has no IPL.TXT file",
            );
            return;
        }
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::NeoGeoCdBootStructure,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "Neo Geo CD IPL.TXT root lookup",
                &diagnostic,
            );
            return;
        }
    };
    let want = (ipl_record.size as usize).min(MAX_IPL_TXT_BYTES);
    let mut buffer = vec![0_u8; want];
    if let Err(error) = read_iso_record_prefix(source, ipl_record, &mut buffer) {
        push_with_source(
            report,
            IdentityKind::NeoGeoCdBootStructure,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Neo Geo CD IPL.TXT bounded read",
            &error.to_string(),
        );
        return;
    }
    let fact = parse_ipl_txt(&buffer);
    if !fact.is_structurally_valid() {
        push_with_source(
            report,
            IdentityKind::NeoGeoCdBootStructure,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "Neo Geo CD IPL.TXT structural validation",
            "a file named IPL.TXT is present but does not parse as a valid Neo Geo CD load manifest (empty/over-long entry list, or the 0x1A terminator byte is absent)",
        );
        return;
    }
    let detail = if fact.has_required_extensions() {
        "IPL.TXT parsed with a valid entry list, terminator byte present, and all five loader-required file types (PRG/FIX/SPR/Z80/PCM) present. Structural platform/media evidence only - exact game identity comes from a DAT/hash match."
    } else {
        "IPL.TXT parsed with a valid entry list and terminator byte present, though not every loader-required file type was found. Structural platform/media evidence only - exact game identity comes from a DAT/hash match."
    };
    push_with_source(
        report,
        IdentityKind::NeoGeoCdBootStructure,
        IdentityStatus::Verified,
        Some("IPL.TXT".to_string()),
        IdentityConfidence::StructuredMetadata,
        member_path,
        member_index,
        "Neo Geo CD IPL.TXT load manifest",
        detail,
    );
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    report.complete = true;
}

/// `header_offset` is the absolute byte offset of the 0x20-byte Dolphin
/// disc header within `source` - `0` for a direct ISO/GCM or a ZIP-member
/// ISO, or the format-specific location of an embedded copy of those same
/// bytes (e.g. RVZ's `wia_disc_t.dhead`, CISO's first stored block).
fn inspect_dolphin_header(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
    header_offset: u64,
) {
    let mut header = [0_u8; DOLPHIN_HEADER_BYTES];
    if let Err(error) = source.read_exact_at(header_offset, &mut header) {
        let status = if error.kind() == io::ErrorKind::UnexpectedEof {
            IdentityStatus::Invalid
        } else {
            IdentityStatus::ResourceLimitReached
        };
        push_with_source(
            report,
            IdentityKind::DolphinGameId,
            status,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "bounded disc-header read",
            "disc header is truncated or unavailable",
        );
        return;
    }
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    let has_gc_magic = header[GAMECUBE_MAGIC_OFFSET..GAMECUBE_MAGIC_OFFSET + 4] == GAMECUBE_MAGIC;
    let has_wii_magic = header[WII_MAGIC_OFFSET..WII_MAGIC_OFFSET + 4] == WII_MAGIC;
    let expected_magic = match report.platform {
        IdentityPlatform::GameCube => has_gc_magic,
        IdentityPlatform::Wii => has_wii_magic,
        _ => false,
    };
    if !expected_magic {
        push_with_source(
            report,
            IdentityKind::DolphinGameId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "reviewed disc-header magic",
            "disc magic does not match the selected platform",
        );
        return;
    }
    let id = &header[..6];
    if !id
        .iter()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        push_with_source(
            report,
            IdentityKind::DolphinGameId,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "disc-header bytes 0x00..0x06",
            "Game ID must contain six uppercase ASCII letters or digits",
        );
        return;
    }
    let id = String::from_utf8_lossy(id).into_owned();
    push_with_source(
        report,
        IdentityKind::DolphinGameId,
        IdentityStatus::Verified,
        Some(id),
        IdentityConfidence::ExactBytes,
        member_path.clone(),
        member_index,
        "disc-header bytes 0x00..0x06 with platform magic validation",
        "verified directly from the reviewed disc header",
    );
    push_with_source(
        report,
        IdentityKind::DolphinDiscNumber,
        IdentityStatus::Verified,
        Some(header[6].to_string()),
        IdentityConfidence::ExactBytes,
        member_path.clone(),
        member_index,
        "disc-header byte 0x06",
        "verified directly from the reviewed disc header",
    );
    let revision_status = if report.platform == IdentityPlatform::GameCube {
        IdentityStatus::Verified
    } else {
        IdentityStatus::Candidate
    };
    push_with_source(
        report,
        IdentityKind::DolphinRevision,
        revision_status,
        Some(header[7].to_string()),
        if revision_status == IdentityStatus::Verified {
            IdentityConfidence::ExactBytes
        } else {
            IdentityConfidence::StructuredMetadata
        },
        member_path.clone(),
        member_index,
        "outer disc-header byte 0x07",
        if revision_status == IdentityStatus::Verified {
            "verified GameCube revision"
        } else {
            "Wii outer-header revision is not promoted because Dolphin may use the game-partition header"
        },
    );
    push_with_source(
        report,
        IdentityKind::DolphinRegion,
        IdentityStatus::Verified,
        Some(char::from(header[3]).to_string()),
        IdentityConfidence::ExactBytes,
        member_path,
        member_index,
        "fourth Game ID byte",
        "raw region code byte; no locale name inferred",
    );
    report.complete = true;
}

/// RVZ (and WIA-layout-compatible) direct identity: reads only the fixed,
/// documented, always-uncompressed header region - never the compressed
/// disc body. See the `RVZ_MAGIC`/`WIA_*` constants for the exact layout
/// and source. A magic or `disc_type` mismatch is reported as `Invalid`
/// ("malformed format" in the beginner-facing states this maps to); a
/// missing/unrecognised inner disc magic (checked by
/// `inspect_dolphin_header` itself) is reported the same way.
fn inspect_rvz(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Rvz;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    let mut source = FileSource {
        file,
        len,
        bytes_read: 0,
    };
    let mut magic = [0_u8; 4];
    if source.read_exact_at(0, &mut magic).is_err() {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "RVZ file header is truncated",
        );
        report.bytes_read = source.bytes_read();
        return;
    }
    if magic != RVZ_MAGIC {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "RVZ magic bytes do not match the documented wia_file_head_t header",
        );
        report.bytes_read = source.bytes_read();
        return;
    }
    let mut disc_type_bytes = [0_u8; 4];
    if source
        .read_exact_at(WIA_DISC_TYPE_OFFSET, &mut disc_type_bytes)
        .is_err()
    {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "RVZ wia_disc_t header is truncated",
        );
        report.bytes_read = source.bytes_read();
        return;
    }
    let disc_type = u32::from_be_bytes(disc_type_bytes);
    let expected_disc_type = match report.platform {
        IdentityPlatform::GameCube => WIA_DISC_TYPE_GAMECUBE,
        IdentityPlatform::Wii => WIA_DISC_TYPE_WII,
        _ => 0,
    };
    if disc_type != expected_disc_type {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "RVZ wia_disc_t.disc_type does not match the expected platform",
        );
        report.bytes_read = source.bytes_read();
        return;
    }
    inspect_dolphin_header(report, &mut source, None, None, WIA_DHEAD_OFFSET);
    report.bytes_read = source.bytes_read();
}

/// GameCube/Wii `.ciso` direct identity: reads only the fixed 0x8000-byte
/// header (magic, block size, block-presence map) plus, if present, the
/// first stored block - the disc header block is never compressed in this
/// format, so no decompression is required. See the `CISO_*` constants for
/// the exact layout and source.
fn inspect_ciso(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Ciso;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    let mut source = FileSource {
        file,
        len,
        bytes_read: 0,
    };
    let mut magic = [0_u8; 4];
    if source.read_exact_at(0, &mut magic).is_err() {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "CISO file header is truncated",
        );
        report.bytes_read = source.bytes_read();
        return;
    }
    if magic != CISO_MAGIC {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "CISO magic bytes do not match the documented CISO header",
        );
        report.bytes_read = source.bytes_read();
        return;
    }
    let mut block_size_bytes = [0_u8; 4];
    if source.read_exact_at(4, &mut block_size_bytes).is_err() {
        add_unavailable(report, IdentityStatus::Invalid, "CISO header is truncated");
        report.bytes_read = source.bytes_read();
        return;
    }
    let block_size = u32::from_le_bytes(block_size_bytes);
    if (block_size as u64) < DOLPHIN_HEADER_BYTES as u64 {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "CISO block size is smaller than the disc header",
        );
        report.bytes_read = source.bytes_read();
        return;
    }
    let mut first_map_byte = [0_u8; 1];
    if source
        .read_exact_at(CISO_MAP_OFFSET, &mut first_map_byte)
        .is_err()
    {
        add_unavailable(
            report,
            IdentityStatus::Invalid,
            "CISO block map is truncated",
        );
        report.bytes_read = source.bytes_read();
        return;
    }
    if first_map_byte[0] != 1 {
        push_with_source(
            report,
            IdentityKind::DolphinGameId,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
            None,
            None,
            "CISO block-presence map, block 0",
            "the disc-header block is not stored in this CISO image",
        );
        report.bytes_read = source.bytes_read();
        return;
    }
    // Block 0, if present, is always the first block physically stored
    // (the map's running "used" count starts at 0), so its data begins
    // immediately after the fixed-size header - no need to scan the rest
    // of the map to locate it.
    inspect_dolphin_header(report, &mut source, None, None, CISO_HEADER_SIZE);
    report.bytes_read = source.bytes_read();
}

/// Locate the single occupied disc slot in a WBFS container, returning the
/// absolute byte offset of its stored disc-header copy. Every step uses
/// checked arithmetic and is validated against the real file length, so a
/// malformed or hostile header can only produce an error - never an
/// out-of-range or wrapped read. Returns the diagnostic for an `Invalid`
/// or `Ambiguous` result rather than choosing a slot silently.
fn checked_wbfs_sector_offset(sector: u64, sector_size: u64) -> Option<u64> {
    sector.checked_mul(sector_size)
}

fn wbfs_disc_header_offset(
    source: &mut dyn ByteSource,
) -> Result<u64, (IdentityStatus, &'static str)> {
    let mut header = [0_u8; WBFS_DISC_TABLE_OFFSET as usize];
    source
        .read_exact_at(0, &mut header)
        .map_err(|_| (IdentityStatus::Invalid, "WBFS file header is truncated"))?;
    if header[..4] != WBFS_MAGIC {
        return Err((
            IdentityStatus::Invalid,
            "WBFS magic bytes do not match the documented wbfs_head_t header",
        ));
    }
    let n_hd_sec = u64::from(u32::from_be_bytes(
        header[WBFS_N_HD_SEC_OFFSET..WBFS_N_HD_SEC_OFFSET + 4]
            .try_into()
            .expect("fixed WBFS header slice"),
    ));
    if n_hd_sec == 0 {
        return Err((
            IdentityStatus::Invalid,
            "WBFS n_hd_sec declares an empty source device",
        ));
    }
    let hd_sec_sz_s = u32::from(header[WBFS_HD_SEC_SZ_S_OFFSET as usize]);
    let wbfs_sec_sz_s = u32::from(header[WBFS_WBFS_SEC_SZ_S_OFFSET as usize]);
    if !(WBFS_MIN_HD_SECTOR_SHIFT..=WBFS_MAX_HD_SECTOR_SHIFT).contains(&hd_sec_sz_s) {
        return Err((
            IdentityStatus::Invalid,
            "WBFS hd_sec_sz_s is outside the supported 512 B..64 KiB range",
        ));
    }
    if !(WBFS_MIN_SECTOR_SHIFT..=WBFS_MAX_SECTOR_SHIFT).contains(&wbfs_sec_sz_s) {
        return Err((
            IdentityStatus::Invalid,
            "WBFS wbfs_sec_sz_s is outside the supported 64 KiB..64 MiB range",
        ));
    }
    if wbfs_sec_sz_s <= hd_sec_sz_s {
        return Err((
            IdentityStatus::Invalid,
            "WBFS sector size must be larger than the host sector size",
        ));
    }
    let hd_sec_sz = 1_u64.checked_shl(hd_sec_sz_s).ok_or((
        IdentityStatus::Invalid,
        "WBFS host sector-size shift overflows",
    ))?;
    let wbfs_sec_sz = 1_u64
        .checked_shl(wbfs_sec_sz_s)
        .ok_or((IdentityStatus::Invalid, "WBFS sector-size shift overflows"))?;
    let declared_bytes = n_hd_sec.checked_mul(hd_sec_sz).ok_or((
        IdentityStatus::Invalid,
        "WBFS declared source size overflows",
    ))?;
    if declared_bytes != source.len() {
        return Err((
            IdentityStatus::Invalid,
            "WBFS declared source size does not match the file length",
        ));
    }

    // The disc table fills the remainder of the first HD sector, one byte
    // per slot, and is additionally capped so a large `hd_sec_sz` cannot
    // drive an unbounded read.
    let slots = hd_sec_sz.checked_sub(WBFS_DISC_TABLE_OFFSET).ok_or((
        IdentityStatus::Invalid,
        "WBFS hd_sec_sz is smaller than the wbfs_head_t it must contain",
    ))?;
    let mut table = vec![0_u8; slots as usize];
    source
        .read_exact_at(WBFS_DISC_TABLE_OFFSET, &mut table)
        .map_err(|_| (IdentityStatus::Invalid, "WBFS disc table is truncated"))?;
    let mut occupied = table
        .iter()
        .enumerate()
        .filter_map(|(slot, entry)| (*entry != 0).then_some(slot));
    let Some(slot) = occupied.next() else {
        return Err((
            IdentityStatus::Invalid,
            "WBFS disc table is empty; the container holds no disc",
        ));
    };
    if occupied.next().is_some() {
        return Err((
            IdentityStatus::Ambiguous,
            "WBFS container holds more than one disc; EmuWiz will not choose one silently",
        ));
    }

    let n_wbfs_sec = declared_bytes / wbfs_sec_sz;

    // `wbfs_disc_info_t` stores a 0x100-byte disc-header copy followed by
    // one big-endian u16 physical-sector mapping per logical WBFS sector.
    // Reading and validating this small table proves every non-empty mapping
    // remains within the same file; the mapped disc data itself is not read.
    let sectors_per_disc = WBFS_WII_DISC_SIZE.div_ceil(wbfs_sec_sz);
    let wlba_bytes = sectors_per_disc
        .checked_mul(2)
        .ok_or((IdentityStatus::Invalid, "WBFS WLBA table size overflows"))?;
    let unaligned_disc_info_bytes = WBFS_DISC_INFO_HEADER_BYTES
        .checked_add(wlba_bytes)
        .ok_or((IdentityStatus::Invalid, "WBFS disc-info size overflows"))?;
    let disc_info_bytes = unaligned_disc_info_bytes
        .checked_add(hd_sec_sz - 1)
        .map(|value| value & !(hd_sec_sz - 1))
        .ok_or((
            IdentityStatus::Invalid,
            "WBFS aligned disc-info size overflows",
        ))?;
    if disc_info_bytes > wbfs_sec_sz - hd_sec_sz {
        return Err((
            IdentityStatus::Invalid,
            "WBFS disc-info slot does not fit before mapped data sectors",
        ));
    }
    let max_disc_slots = (wbfs_sec_sz - hd_sec_sz) / disc_info_bytes;
    let slot = u64::try_from(slot).map_err(|_| {
        (
            IdentityStatus::ResourceLimitReached,
            "WBFS disc-table slot exceeds host index range",
        )
    })?;
    if slot >= max_disc_slots {
        return Err((
            IdentityStatus::Invalid,
            "WBFS occupied disc-table slot lies outside the packed disc-info area",
        ));
    }
    let offset = slot
        .checked_mul(disc_info_bytes)
        .and_then(|relative| hd_sec_sz.checked_add(relative))
        .ok_or((
            IdentityStatus::Invalid,
            "WBFS disc-info slot offset overflows",
        ))?;
    let end = offset
        .checked_add(unaligned_disc_info_bytes)
        .ok_or((IdentityStatus::Invalid, "WBFS disc-info offset overflows"))?;
    if end > source.len() {
        return Err((
            IdentityStatus::Invalid,
            "WBFS disc-info or WLBA table lies beyond the end of the file",
        ));
    }
    let wlba_len = usize::try_from(wlba_bytes).map_err(|_| {
        (
            IdentityStatus::ResourceLimitReached,
            "WBFS WLBA table exceeds the bounded host allocation",
        )
    })?;
    let mut wlba = vec![0_u8; wlba_len];
    source
        .read_exact_at(offset + WBFS_DISC_INFO_HEADER_BYTES, &mut wlba)
        .map_err(|_| (IdentityStatus::Invalid, "WBFS WLBA table is truncated"))?;
    let mut mapped_sectors = 0_usize;
    for entry in wlba.chunks_exact(2) {
        let physical = u64::from(u16::from_be_bytes([entry[0], entry[1]]));
        if physical == 0 {
            continue;
        }
        mapped_sectors += 1;
        if physical >= n_wbfs_sec {
            return Err((
                IdentityStatus::Invalid,
                "WBFS WLBA entry points beyond the declared container sectors",
            ));
        }
        let physical_offset = checked_wbfs_sector_offset(physical, wbfs_sec_sz)
            .ok_or((IdentityStatus::Invalid, "WBFS WLBA sector offset overflows"))?;
        let physical_end = physical_offset
            .checked_add(wbfs_sec_sz)
            .ok_or((IdentityStatus::Invalid, "WBFS WLBA sector end overflows"))?;
        if physical_end > source.len() {
            return Err((
                IdentityStatus::Invalid,
                "WBFS WLBA entry points beyond the end of the file",
            ));
        }
    }
    if mapped_sectors == 0 {
        return Err((
            IdentityStatus::Invalid,
            "WBFS disc has no mapped logical Wii sectors",
        ));
    }
    Ok(offset)
}

/// WBFS direct identity: reads the container head, the disc table, and the
/// stored copy of the Wii disc header belonging to the single occupied
/// slot. The scrubbed disc body is never read, nothing is decompressed or
/// mounted, no external tool is launched, and the file is opened read-only
/// with symlinks refused by `open_read_only_regular`.
fn inspect_wbfs(report: &mut GameIdentityReport, trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Wbfs;
    let file = match open_read_only_regular(&report.archive_path, trusted) {
        Ok(file) => file,
        Err(message) => {
            add_unavailable(report, IdentityStatus::Invalid, &message);
            return;
        }
    };
    let before = match StableFileMetadata::from_file(&file) {
        Ok(metadata) => metadata,
        Err(error) => {
            add_unavailable(report, IdentityStatus::Invalid, &error.to_string());
            return;
        }
    };
    let len = before.len;
    let mut source = FileSource {
        file,
        len,
        bytes_read: 0,
    };
    let evidence_start = report.evidence.len();
    match wbfs_disc_header_offset(&mut source) {
        Ok(offset) => inspect_dolphin_header(report, &mut source, None, None, offset),
        Err((status, diagnostic)) => add_unavailable(report, status, diagnostic),
    }
    report.bytes_read = source.bytes_read();
    for item in &mut report.evidence[evidence_start..] {
        item.provenance.method =
            "WBFS-contained Wii disc-info header copy after disc-table and WLBA validation"
                .to_string();
    }
    match StableFileMetadata::from_file(&source.file) {
        Ok(after) if after == before => {}
        Ok(_) | Err(_) => {
            report.evidence.truncate(evidence_start);
            report.complete = false;
            add_unavailable(
                report,
                IdentityStatus::Invalid,
                "WBFS source metadata changed during bounded identity inspection",
            );
        }
    }
}

fn inspect_ps2_iso(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let root = match iso_root(source) {
        Ok(root) => root,
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::Ps2Serial,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "ISO 9660 primary volume descriptor",
                &diagnostic,
            );
            return;
        }
    };
    report.metadata_paths_inspected += 1;
    let cnf = match find_iso_path(source, root, &[b"SYSTEM.CNF"]) {
        Ok(Some(record)) => record,
        Ok(None) => {
            push_with_source(
                report,
                IdentityKind::Ps2Serial,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "ISO 9660 root directory lookup",
                "SYSTEM.CNF is missing",
            );
            return;
        }
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::Ps2Serial,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "ISO 9660 root directory lookup",
                &diagnostic,
            );
            return;
        }
    };
    if cnf.size > MAX_SYSTEM_CNF_BYTES {
        push_with_source(
            report,
            IdentityKind::Ps2Serial,
            IdentityStatus::ResourceLimitReached,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "SYSTEM.CNF bounded read",
            "SYSTEM.CNF exceeds 64 KiB",
        );
        return;
    }
    let cnf_bytes = match read_iso_record(source, cnf) {
        Ok(bytes) => bytes,
        Err(error) => {
            push_with_source(
                report,
                IdentityKind::Ps2Serial,
                source_error_status(&error),
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF bounded read",
                &error.to_string(),
            );
            return;
        }
    };
    let boot = match parse_system_cnf_boot2(&cnf_bytes) {
        Ok(boot) => boot,
        Err(message) => {
            push_with_source(
                report,
                IdentityKind::Ps2Serial,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF BOOT2 assignment",
                &message,
            );
            return;
        }
    };
    let serial = match serial_from_boot_path(&boot) {
        Some(serial) => serial,
        None => {
            push_with_source(
                report,
                IdentityKind::Ps2Serial,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF BOOT2 executable name",
                "boot executable does not contain a valid PS2 product code",
            );
            return;
        }
    };
    push_with_source(
        report,
        IdentityKind::Ps2Serial,
        IdentityStatus::Verified,
        Some(serial),
        IdentityConfidence::StructuredMetadata,
        member_path.clone(),
        member_index,
        "SYSTEM.CNF BOOT2 on ISO 9660",
        "serial derived from the exact boot executable path, not an archive filename",
    );
    report.metadata_paths_inspected += 1;
    let components: Vec<&[u8]> = boot.split(|byte| *byte == b'\\' || *byte == b'/').collect();
    let executable = match find_iso_path(source, root, &components) {
        Ok(Some(record)) => record,
        Ok(None) => {
            push_with_source(
                report,
                IdentityKind::Pcsx2ExecutableCrc,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF BOOT2 ISO lookup",
                "boot executable is missing",
            );
            return;
        }
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::Pcsx2ExecutableCrc,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF BOOT2 ISO lookup",
                &diagnostic,
            );
            return;
        }
    };
    if executable.size > MAX_EXECUTABLE_BYTES {
        push_with_source(
            report,
            IdentityKind::Pcsx2ExecutableCrc,
            IdentityStatus::ResourceLimitReached,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "bounded boot executable read",
            "boot executable exceeds 32 MiB",
        );
        return;
    }
    let executable = match read_iso_record(source, executable) {
        Ok(bytes) => bytes,
        Err(error) => {
            push_with_source(
                report,
                IdentityKind::Pcsx2ExecutableCrc,
                source_error_status(&error),
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "bounded boot executable read",
                &error.to_string(),
            );
            return;
        }
    };
    if executable.len() < 4 || executable[..4] != [0x7f, b'E', b'L', b'F'] {
        push_with_source(
            report,
            IdentityKind::Pcsx2ExecutableCrc,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "ELF signature validation",
            "boot executable is not an ELF file",
        );
        return;
    }
    let crc = pcsx2_executable_crc(&executable);
    push_with_source(
        report,
        IdentityKind::Pcsx2ExecutableCrc,
        IdentityStatus::Verified,
        Some(format!("{crc:08X}")),
        IdentityConfidence::ExactBytes,
        member_path,
        member_index,
        "PCSX2 ELF word-XOR algorithm over exact executable bytes",
        "full bounded boot executable was read and hashed with the reviewed PCSX2 algorithm",
    );
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    report.complete = true;
}

fn inspect_ps1_iso(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    let root = match iso_root(source) {
        Ok(root) => root,
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "ISO 9660 primary volume descriptor",
                &diagnostic,
            );
            return;
        }
    };
    report.metadata_paths_inspected += 1;
    let cnf = match find_iso_path(source, root, &[b"SYSTEM.CNF"]) {
        Ok(Some(record)) => record,
        Ok(None) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "ISO 9660 root directory lookup",
                "SYSTEM.CNF is missing",
            );
            return;
        }
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "ISO 9660 root directory lookup",
                &diagnostic,
            );
            return;
        }
    };
    if cnf.size > MAX_SYSTEM_CNF_BYTES {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::ResourceLimitReached,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "SYSTEM.CNF bounded read",
            "SYSTEM.CNF exceeds 64 KiB",
        );
        return;
    }
    let cnf_bytes = match read_iso_record(source, cnf) {
        Ok(bytes) => bytes,
        Err(error) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                source_error_status(&error),
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF bounded read",
                &error.to_string(),
            );
            return;
        }
    };
    let boot = match parse_system_cnf_boot(&cnf_bytes) {
        Some(fact) if fact.boot_key == "BOOT" => fact,
        Some(_) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF BOOT assignment",
                "SYSTEM.CNF contains BOOT2, which is not a PS1 boot key",
            );
            return;
        }
        None => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF BOOT assignment",
                "SYSTEM.CNF has no valid BOOT assignment",
            );
            return;
        }
    };
    let Some(serial) = boot.serial_candidate.clone() else {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "SYSTEM.CNF BOOT executable name",
            "BOOT executable does not contain a valid PS1 product code",
        );
        return;
    };
    if !is_supported_ps1_serial(&serial) {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "SYSTEM.CNF BOOT product-code family",
            "BOOT executable contains an unsupported PS1 product-code family",
        );
        return;
    }
    let Some(executable_path) = boot.executable_path else {
        return;
    };
    let components: Vec<&[u8]> = executable_path
        .split(['\\', '/'])
        .map(str::as_bytes)
        .collect();
    report.metadata_paths_inspected += 1;
    let executable = match find_iso_path(source, root, &components) {
        Ok(Some(record)) => record,
        Ok(None) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF BOOT ISO lookup",
                "BOOT executable is missing",
            );
            return;
        }
        Err((status, diagnostic)) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                status,
                None,
                IdentityConfidence::Unavailable,
                member_path,
                member_index,
                "SYSTEM.CNF BOOT ISO lookup",
                &diagnostic,
            );
            return;
        }
    };
    let header_len = (executable.size as usize).min(PSX_EXECUTABLE_HEADER_BYTES);
    let mut header = vec![0_u8; header_len];
    if let Err(error) = read_iso_record_prefix(source, executable, &mut header) {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            source_error_status(&error),
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "bounded PS-X EXE header read",
            &error.to_string(),
        );
        return;
    }
    if !looks_like_psx_exe(&header) {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            member_path,
            member_index,
            "PS-X EXE signature validation",
            "BOOT executable is not a PS-X EXE",
        );
        return;
    }
    push_with_source(
        report,
        IdentityKind::Ps1Serial,
        IdentityStatus::Verified,
        Some(serial),
        IdentityConfidence::StructuredMetadata,
        member_path,
        member_index,
        "SYSTEM.CNF BOOT plus PS-X EXE on ISO 9660",
        "serial derived from the exact boot executable path and corroborated by its PS-X EXE header",
    );
    report.bytes_read = report.bytes_read.max(source.bytes_read());
    report.complete = true;
}

/// Authoritative PS1 identity for a `.chd` - the exact same standard as
/// [`inspect_ps1_iso`] (valid `BOOT=` serial, a supported PS1 product-code
/// family, the referenced executable present, and a valid PS-X EXE header),
/// reached through the *existing* bounded CHD reader
/// ([`read_bounded_chd_bytes`]/[`open_chd_iso9660`] in
/// [`crate::disc_evidence_collector`]) and the *existing* ISO 9660 filesystem
/// reader ([`find_path`] in [`crate::iso9660`]) instead of this module's own
/// `ByteSource`/[`iso_root`] pair - `inspect_ps1_iso` reads a plain ISO via a
/// byte-offset abstraction because a plain ISO *is* just bytes at an offset;
/// a CHD is compressed, so it is decoded through
/// [`crate::chd_logical_media::ChdTrackLogicalMedia`] instead, but every
/// actual PS1-content check below - [`parse_system_cnf_boot`],
/// [`is_supported_ps1_serial`], [`looks_like_psx_exe`] - is the identical
/// function call `inspect_ps1_iso` makes, so a CHD and an ISO of the same
/// disc produce equivalent `Ps1Serial` authority. No CHD/ISO 9660 parsing is
/// duplicated here; only the control flow that turns their results into
/// [`IdentityStatus`] values is (necessarily) written twice, once per
/// underlying media abstraction.
fn inspect_disc_chd(report: &mut GameIdentityReport, _trusted: &TrustedRoots) {
    report.format = IdentityImageFormat::Chd;
    let bytes = match read_bounded_chd_bytes(&report.archive_path) {
        Ok(bytes) => bytes,
        Err(refusal) => {
            push_disc_chd_refusal(report, &refusal);
            return;
        }
    };
    report.bytes_read = report.bytes_read.max(bytes.len() as u64);

    if report.platform == IdentityPlatform::Dreamcast {
        match chd_needs_specialist_optical_backend(&bytes) {
            Ok(true) => {
                inspect_dreamcast_gdrom_chd(report);
                return;
            }
            Ok(false) => {
                // Falls through to the existing single-track path below,
                // exactly as before this branch existed.
            }
            Err(refusal) => {
                push_disc_chd_refusal(report, &refusal);
                return;
            }
        }
    }

    if report.platform == IdentityPlatform::ThreeDo {
        // 3DO discs carry a Panasonic OperaFS volume, not ISO 9660, so the
        // `open_chd_iso9660` path below always refuses them. The raw
        // data-track reader applies the identical CHD container safety and
        // resource bounds, and 3DO identity is a single bounded read of the
        // OperaFS volume header at logical offset 0 - the exact same
        // `inspect_threedo_source` the raw ISO/CUE paths already use, so a
        // 3DO `.chd` and a plain 3DO disc produce an equivalent
        // `ThreeDoDiscId`.
        let media = match open_chd_raw_track(&bytes) {
            Ok(media) => media,
            Err(refusal) => {
                push_disc_chd_refusal(report, &refusal);
                return;
            }
        };
        let mut source = MediaSource::new(media);
        inspect_threedo_source(report, &mut source, None, None);
        report.bytes_read = report.bytes_read.max(source.bytes_read());
        return;
    }

    if report.platform == IdentityPlatform::PcEngineCd {
        // PC Engine CD discs predate ISO 9660, so use the bounded raw data
        // track reader and the same IPL inspection as ISO/CUE inputs.
        let media = match open_chd_raw_track(&bytes) {
            Ok(media) => media,
            Err(refusal) => {
                push_disc_chd_refusal(report, &refusal);
                return;
            }
        };
        let mut source = MediaSource::new(media);
        inspect_pcengine_cd_source(report, &mut source, None, None);
        report.bytes_read = report.bytes_read.max(source.bytes_read());
        return;
    }

    let (media, filesystem) = match open_chd_iso9660(&bytes) {
        Ok(pair) => pair,
        Err(refusal) => {
            push_disc_chd_refusal(report, &refusal);
            return;
        }
    };

    if matches!(
        report.platform,
        IdentityPlatform::Saturn
            | IdentityPlatform::Dreamcast
            | IdentityPlatform::SegaCd
            | IdentityPlatform::Pcfx
            | IdentityPlatform::NeoGeoCd
            | IdentityPlatform::PlayStation2
    ) {
        // Every one of these platforms identifies from a bounded read over
        // the decoded data track, so they all reuse the exact `MediaSource`
        // over the track `open_chd_iso9660` already returned - the same
        // single-track, zero-pregap, fail-closed contract that path
        // enforces. PC-FX joins here: `inspect_pcfx_source` reads sector 0
        // for the `PC-FX:Hu_CD-ROM` boot magic and the documented disc hash,
        // never a filename. A PC-FX `.chd` whose track is not ISO 9660, is
        // not track 1, or carries a pregap is refused by `open_chd_iso9660`
        // above rather than guessed at. Neo Geo CD also joins here: its
        // discs *are* ISO 9660 (unlike PC Engine CD / 3DO), so
        // `inspect_neogeocd_source` looks up the root `IPL.TXT` file through
        // the same bounded ISO 9660 reader the raw ISO/CUE path uses and
        // validates its parsed manifest - never a filename. PlayStation 2
        // joins here too: PS2 discs are ISO 9660, and `inspect_ps2_iso` is
        // the exact same function the raw ISO/CUE path already calls - it
        // reads `SYSTEM.CNF`, parses `BOOT2`, validates the referenced ELF,
        // and hashes it with the reviewed PCSX2 algorithm, all over the
        // `MediaSource` `open_chd_iso9660` already returned. A PS2 `.chd`
        // whose decoded track is not ISO 9660, is not track 1, or carries a
        // pregap is refused by `open_chd_iso9660` above, exactly as for the
        // Sega platforms; nothing here is derived from the filename.
        let mut source = MediaSource::new(media);
        if report.platform == IdentityPlatform::Saturn {
            inspect_saturn_source(report, &mut source, None, None);
        } else if report.platform == IdentityPlatform::Dreamcast {
            inspect_dreamcast_source(report, &mut source, None, None);
        } else if report.platform == IdentityPlatform::SegaCd {
            inspect_sega_cd_source(report, &mut source, None, None);
        } else if report.platform == IdentityPlatform::NeoGeoCd {
            inspect_neogeocd_source(report, &mut source, None, None);
        } else if report.platform == IdentityPlatform::PlayStation2 {
            inspect_ps2_iso(report, &mut source, None, None);
        } else {
            inspect_pcfx_source(report, &mut source, None, None);
        }
        report.bytes_read = report.bytes_read.max(source.bytes_read());
        return;
    }

    report.metadata_paths_inspected += 1;
    let cnf = match find_path(&media, &filesystem, "SYSTEM.CNF") {
        Ok(Some(entry)) if !entry.is_directory => entry,
        Ok(_) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                None,
                None,
                "CHD ISO 9660 root directory lookup",
                "SYSTEM.CNF is missing",
            );
            return;
        }
        Err(error) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                None,
                None,
                "CHD ISO 9660 root directory lookup",
                &error.to_string(),
            );
            return;
        }
    };
    if cnf.size as u64 > MAX_SYSTEM_CNF_BYTES {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::ResourceLimitReached,
            None,
            IdentityConfidence::Unavailable,
            None,
            None,
            "SYSTEM.CNF bounded read",
            "SYSTEM.CNF exceeds 64 KiB",
        );
        return;
    }
    let cnf_offset = cnf.extent_lba as u64 * filesystem.logical_block_size as u64;
    let mut cnf_bytes = vec![0_u8; cnf.size as usize];
    if media.read_at(cnf_offset, &mut cnf_bytes).is_err() {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            None,
            None,
            "SYSTEM.CNF bounded read",
            "SYSTEM.CNF could not be read from the decoded CHD track",
        );
        return;
    }
    let boot = match parse_system_cnf_boot(&cnf_bytes) {
        Some(fact) if fact.boot_key == "BOOT" => fact,
        Some(_) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                None,
                None,
                "SYSTEM.CNF BOOT assignment",
                "SYSTEM.CNF contains BOOT2, which is not a PS1 boot key",
            );
            return;
        }
        None => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                None,
                None,
                "SYSTEM.CNF BOOT assignment",
                "SYSTEM.CNF has no valid BOOT assignment",
            );
            return;
        }
    };
    let Some(serial) = boot.serial_candidate.clone() else {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            None,
            None,
            "SYSTEM.CNF BOOT executable name",
            "BOOT executable does not contain a valid PS1 product code",
        );
        return;
    };
    if !is_supported_ps1_serial(&serial) {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            None,
            None,
            "SYSTEM.CNF BOOT product-code family",
            "BOOT executable contains an unsupported PS1 product-code family",
        );
        return;
    }
    let Some(executable_path) = boot.executable_path else {
        return;
    };
    // `find_path` (unlike this module's own `find_iso_path`) splits only on
    // `/`; a `BOOT=cdrom:\SLUS_014.18;1`-shaped value normalizes to a
    // backslash-separated path, so it is translated here rather than
    // teaching `find_path` a second separator convention for this one
    // caller.
    let lookup_path = executable_path.replace('\\', "/");
    report.metadata_paths_inspected += 1;
    let executable = match find_path(&media, &filesystem, &lookup_path) {
        Ok(Some(entry)) if !entry.is_directory => entry,
        Ok(_) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Missing,
                None,
                IdentityConfidence::Unavailable,
                None,
                None,
                "SYSTEM.CNF BOOT CHD ISO 9660 lookup",
                "BOOT executable is missing",
            );
            return;
        }
        Err(error) => {
            push_with_source(
                report,
                IdentityKind::Ps1Serial,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                None,
                None,
                "SYSTEM.CNF BOOT CHD ISO 9660 lookup",
                &error.to_string(),
            );
            return;
        }
    };
    let header_len = (executable.size as usize).min(PSX_EXECUTABLE_HEADER_BYTES);
    let executable_offset = executable.extent_lba as u64 * filesystem.logical_block_size as u64;
    let mut header = vec![0_u8; header_len];
    if media.read_at(executable_offset, &mut header).is_err() {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            None,
            None,
            "bounded PS-X EXE header read",
            "BOOT executable header could not be read from the decoded CHD track",
        );
        return;
    }
    if !looks_like_psx_exe(&header) {
        push_with_source(
            report,
            IdentityKind::Ps1Serial,
            IdentityStatus::Invalid,
            None,
            IdentityConfidence::Unavailable,
            None,
            None,
            "PS-X EXE signature validation",
            "BOOT executable is not a PS-X EXE",
        );
        return;
    }
    push_with_source(
        report,
        IdentityKind::Ps1Serial,
        IdentityStatus::Verified,
        Some(serial),
        IdentityConfidence::StructuredMetadata,
        None,
        None,
        "SYSTEM.CNF BOOT plus PS-X EXE on CHD-decoded ISO 9660",
        "serial derived from the exact boot executable path and corroborated by its PS-X EXE header",
    );
    report.complete = true;
}

/// Authoritative Dreamcast identity for a multi-track GD-ROM `.chd` - a
/// shape the pure-Rust [`open_chd_iso9660`] path always refuses (its own
/// [`crate::chd_identity::select_candidate_data_track`] would otherwise
/// silently pick the small low-density warning-text track, never the real
/// game - see that function's own doc comment), reached only when
/// [`chd_needs_specialist_optical_backend`] has already confirmed this
/// disc genuinely has high-density data beyond it.
///
/// This never re-implements GD-ROM addressing: the optional
/// [`crate::chd_optical_specialist`] backend (present only when the
/// `chd-optical-specialist` build feature is enabled - see this function's
/// `#[cfg(not(...))]` twin below for the honest fail-closed behavior when
/// it is not) already does the absolute-LBA-to-physical-track rebasing and
/// exposes plain 2048-byte logical sectors; from there this calls the
/// exact same [`inspect_dreamcast_source`] the single-track ISO/CUE/GDI/
/// CHD paths already use, so a GD-ROM CHD and a plain Dreamcast disc
/// produce equivalent `DreamcastProductCode` authority. No IP.BIN/
/// product-code logic is duplicated here.
#[cfg(feature = "chd-optical-specialist")]
fn inspect_dreamcast_gdrom_chd(report: &mut GameIdentityReport) {
    let media =
        match crate::chd_optical_specialist::open_chd_optical_specialist(&report.archive_path) {
            Ok(media) => media,
            Err(error) => {
                push_with_source(
                    report,
                    IdentityKind::DreamcastProductCode,
                    IdentityStatus::Invalid,
                    None,
                    IdentityConfidence::Unavailable,
                    None,
                    None,
                    "Dreamcast GD-ROM specialist optical backend",
                    &error.to_string(),
                );
                return;
            }
        };
    let mut source = MediaSource::new(media);
    inspect_dreamcast_source(report, &mut source, None, None);
}

/// The fail-closed twin of the function above for builds where the
/// optional `chd-optical-specialist` feature is not compiled in (the
/// default - see this crate's `Cargo.toml`). Never falls back to the
/// pure-Rust single-track reader for this shape (that would silently read
/// the wrong, low-density track) and never guesses a product code from a
/// filename - the honest answer is that this build cannot read this disc.
#[cfg(not(feature = "chd-optical-specialist"))]
fn inspect_dreamcast_gdrom_chd(report: &mut GameIdentityReport) {
    add_unavailable(
        report,
        IdentityStatus::Unsupported,
        "this CHD is a multi-track Dreamcast GD-ROM; reading its high-density data area \
         requires the optional chd-optical-specialist build feature, which is not enabled in \
         this build",
    );
}

/// Authoritative Dreamcast identity for a DiscJuggler `.cdi` image. Never
/// duplicates Dreamcast identity logic: the selected data track from
/// [`crate::dreamcast_cdi::open_dreamcast_cdi_logical_media`] is wrapped
/// in the same [`MediaSource`] and fed to the exact same
/// [`inspect_dreamcast_source`] every other Dreamcast source (ISO, CUE,
/// GDI, GD-ROM CHD) already uses.
#[cfg(feature = "dreamcast-cdi")]
fn inspect_disc_cdi(report: &mut GameIdentityReport) {
    report.format = IdentityImageFormat::Cdi;
    let media = match crate::dreamcast_cdi::open_dreamcast_cdi_logical_media(&report.archive_path) {
        Ok(media) => media,
        Err(error) => {
            push_with_source(
                report,
                IdentityKind::DreamcastProductCode,
                IdentityStatus::Invalid,
                None,
                IdentityConfidence::Unavailable,
                None,
                None,
                "Dreamcast CDI data track resolution",
                &error.to_string(),
            );
            return;
        }
    };
    let mut source = MediaSource::new(media);
    inspect_dreamcast_source(report, &mut source, None, None);
}

/// The fail-closed twin of the function above for builds where the
/// `dreamcast-cdi` feature (default-on) is disabled. CDI parsing depends
/// on the same optional `opticaldiscs` dependency that feature gates -
/// see [`crate::dreamcast_cdi`]'s module documentation. Never scans for a
/// magic string or guesses a product code from a filename as a
/// substitute.
#[cfg(not(feature = "dreamcast-cdi"))]
fn inspect_disc_cdi(report: &mut GameIdentityReport) {
    report.format = IdentityImageFormat::Cdi;
    add_unavailable(
        report,
        IdentityStatus::Unsupported,
        "this is a Dreamcast DiscJuggler CDI image; reading it requires the optional \
         dreamcast-cdi build feature, which is not enabled in this build",
    );
}

/// Maps a [`DiscCollectionRefusal`] from opening/decoding a `.chd` into the
/// honest [`IdentityStatus`] for that failure - never `Verified`, and never
/// a guess at what the content might have been.
fn push_disc_chd_refusal(report: &mut GameIdentityReport, refusal: &DiscCollectionRefusal) {
    let status = match refusal {
        DiscCollectionRefusal::TooLarge { .. } => IdentityStatus::ResourceLimitReached,
        DiscCollectionRefusal::NoLogicalReaderAvailable => IdentityStatus::Unsupported,
        DiscCollectionRefusal::NotReadable(_)
        | DiscCollectionRefusal::NotRecognizedContainer
        | DiscCollectionRefusal::ChdHeaderDidNotParse(_)
        | DiscCollectionRefusal::NotIso9660
        | DiscCollectionRefusal::Iso9660DidNotParse(_) => IdentityStatus::Invalid,
        DiscCollectionRefusal::NotGcOrWii(_) => IdentityStatus::Invalid,
    };
    push_with_source(
        report,
        match report.platform {
            IdentityPlatform::PlayStation2 => IdentityKind::Ps2Serial,
            IdentityPlatform::Saturn => IdentityKind::SaturnProductNumber,
            IdentityPlatform::Dreamcast => IdentityKind::DreamcastProductCode,
            IdentityPlatform::SegaCd => IdentityKind::SegaCdProductCode,
            IdentityPlatform::ThreeDo => IdentityKind::ThreeDoDiscId,
            IdentityPlatform::PcEngineCd => IdentityKind::PceCdBootStructure,
            IdentityPlatform::NeoGeoCd => IdentityKind::NeoGeoCdBootStructure,
            IdentityPlatform::Pcfx => IdentityKind::PcfxDiscHash,
            _ => IdentityKind::Ps1Serial,
        },
        status,
        None,
        IdentityConfidence::Unavailable,
        None,
        None,
        "CHD container/ISO 9660 opening",
        &format!("{refusal:?}"),
    );
}

pub fn parse_system_cnf_boot2(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() as u64 > MAX_SYSTEM_CNF_BYTES {
        return Err("SYSTEM.CNF exceeds 64 KiB".to_string());
    }
    let mut result = None;
    for line in bytes.split(|byte| *byte == b'\n' || *byte == b'\r') {
        let line = trim_ascii(line);
        let Some(equals) = line.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        if !trim_ascii(&line[..equals]).eq_ignore_ascii_case(b"BOOT2") {
            continue;
        }
        if result.is_some() {
            return Err("SYSTEM.CNF contains multiple BOOT2 assignments".to_string());
        }
        let mut value = trim_ascii(&line[equals + 1..]);
        let lower: Vec<u8> = value.iter().map(u8::to_ascii_lowercase).collect();
        let prefix_len = if lower.starts_with(b"cdrom0:") {
            7
        } else if lower.starts_with(b"cdrom:") {
            6
        } else {
            return Err("BOOT2 must use cdrom: or cdrom0:".to_string());
        };
        value = &value[prefix_len..];
        while value
            .first()
            .is_some_and(|byte| *byte == b'/' || *byte == b'\\')
        {
            value = &value[1..];
        }
        if let Some(version) = value.iter().position(|byte| *byte == b';') {
            value = &value[..version];
        }
        if value.is_empty() || value.len() > MAX_PATH_BYTES {
            return Err("BOOT2 path is empty or exceeds 512 bytes".to_string());
        }
        if value
            .split(|byte| *byte == b'/' || *byte == b'\\')
            .any(|component| component.is_empty() || component == b"." || component == b"..")
        {
            return Err("BOOT2 path contains an empty or traversal component".to_string());
        }
        result = Some(value.to_vec());
    }
    result.ok_or_else(|| "SYSTEM.CNF has no BOOT2 assignment".to_string())
}

pub fn serial_from_boot_path(path: &[u8]) -> Option<String> {
    let name = path.rsplit(|byte| *byte == b'/' || *byte == b'\\').next()?;
    let name = std::str::from_utf8(name).ok()?.to_ascii_uppercase();
    let bytes = name.as_bytes();
    if bytes.len() < 11
        || !bytes[..4].iter().all(u8::is_ascii_alphanumeric)
        || !matches!(bytes[4], b'_' | b'-')
        || !bytes[5..8].iter().all(u8::is_ascii_digit)
        || bytes[8] != b'.'
        || !bytes[9..11].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    Some(format!("{}-{}{}", &name[..4], &name[5..8], &name[9..11]))
}

fn is_supported_ps1_serial(serial: &str) -> bool {
    matches!(
        serial.get(..4),
        Some("SLUS" | "SCUS" | "SLES" | "SCES" | "SLPS" | "SLPM")
    )
}

/// PCSX2's executable "CRC": XOR each complete little-endian 32-bit ELF word.
/// Trailing one-to-three bytes are intentionally ignored to match PCSX2.
pub fn pcsx2_executable_crc(bytes: &[u8]) -> u32 {
    bytes.chunks_exact(4).fold(0_u32, |crc, word| {
        crc ^ u32::from_le_bytes([word[0], word[1], word[2], word[3]])
    })
}

#[derive(Clone, Copy)]
struct IsoRecord {
    extent: u32,
    size: u64,
    directory: bool,
}

fn iso_root(source: &mut dyn ByteSource) -> Result<IsoRecord, (IdentityStatus, String)> {
    let mut sector = [0_u8; ISO_SECTOR_SIZE as usize];
    for descriptor in 0..MAX_ISO_DESCRIPTORS {
        let offset = (16 + descriptor as u64) * ISO_SECTOR_SIZE;
        source.read_exact_at(offset, &mut sector).map_err(|error| {
            (
                source_error_status(&error),
                format!("volume descriptor unavailable: {error}"),
            )
        })?;
        if &sector[1..6] != b"CD001" || sector[6] != 1 {
            return Err((
                IdentityStatus::Invalid,
                "invalid ISO 9660 volume descriptor".to_string(),
            ));
        }
        match sector[0] {
            1 => {
                return parse_iso_record(&sector[156..]).ok_or((
                    IdentityStatus::Invalid,
                    "invalid ISO root directory record".to_string(),
                ));
            }
            255 => break,
            _ => {}
        }
    }
    Err((
        IdentityStatus::Missing,
        "ISO 9660 primary volume descriptor not found".to_string(),
    ))
}

fn find_iso_path(
    source: &mut dyn ByteSource,
    mut directory: IsoRecord,
    components: &[&[u8]],
) -> Result<Option<IsoRecord>, (IdentityStatus, String)> {
    if components.is_empty() || components.len() > MAX_METADATA_PATHS {
        return Err((
            IdentityStatus::ResourceLimitReached,
            "metadata path-component limit reached".to_string(),
        ));
    }
    for (component_index, wanted) in components.iter().enumerate() {
        if wanted.is_empty() || wanted.len() > MAX_PATH_BYTES {
            return Err((
                IdentityStatus::Invalid,
                "invalid ISO path component".to_string(),
            ));
        }
        if directory.size > MAX_DIRECTORY_BYTES {
            return Err((
                IdentityStatus::ResourceLimitReached,
                "ISO directory exceeds 1 MiB".to_string(),
            ));
        }
        let data = read_iso_record(source, directory).map_err(|error| {
            (
                source_error_status(&error),
                format!("ISO directory read failed: {error}"),
            )
        })?;
        let mut offset = 0_usize;
        let mut entries = 0_usize;
        let mut found = None;
        while offset < data.len() {
            let length = data[offset] as usize;
            if length == 0 {
                offset = ((offset / ISO_SECTOR_SIZE as usize) + 1) * ISO_SECTOR_SIZE as usize;
                continue;
            }
            if offset + length > data.len() || length < 34 {
                return Err((
                    IdentityStatus::Invalid,
                    "malformed ISO directory record".to_string(),
                ));
            }
            entries += 1;
            if entries > MAX_ISO_DIRECTORY_ENTRIES {
                return Err((
                    IdentityStatus::ResourceLimitReached,
                    "ISO directory-entry limit reached".to_string(),
                ));
            }
            let record_bytes = &data[offset..offset + length];
            let name_len = record_bytes[32] as usize;
            if 33 + name_len > record_bytes.len() {
                return Err((
                    IdentityStatus::Invalid,
                    "malformed ISO filename".to_string(),
                ));
            }
            let name = strip_iso_version(&record_bytes[33..33 + name_len]);
            if name.eq_ignore_ascii_case(wanted) {
                found = Some(parse_iso_record(record_bytes).ok_or((
                    IdentityStatus::Invalid,
                    "unsupported or inconsistent ISO directory record".to_string(),
                ))?);
                break;
            }
            offset += length;
        }
        let Some(record) = found else { return Ok(None) };
        let last = component_index + 1 == components.len();
        if !last && !record.directory {
            return Ok(None);
        }
        directory = record;
    }
    Ok(Some(directory))
}

fn parse_iso_record(bytes: &[u8]) -> Option<IsoRecord> {
    let length = *bytes.first()? as usize;
    if length < 34 || bytes.len() < length {
        return None;
    }
    let extent = u32::from_le_bytes(bytes[2..6].try_into().ok()?);
    let size = u32::from_le_bytes(bytes[10..14].try_into().ok()?) as u64;
    if extent != u32::from_be_bytes(bytes[6..10].try_into().ok()?)
        || size != u64::from(u32::from_be_bytes(bytes[14..18].try_into().ok()?))
        || bytes[26] != 0
        || bytes[27] != 0
        || bytes[25] & 0x80 != 0
    {
        return None;
    }
    Some(IsoRecord {
        extent,
        size,
        directory: bytes[25] & 0x02 != 0,
    })
}

fn read_iso_record(source: &mut dyn ByteSource, record: IsoRecord) -> io::Result<Vec<u8>> {
    let offset = u64::from(record.extent)
        .checked_mul(ISO_SECTOR_SIZE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ISO extent overflow"))?;
    let end = offset
        .checked_add(record.size)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ISO record overflow"))?;
    if end > source.len() || record.size > usize::MAX as u64 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "ISO record is outside the readable image",
        ));
    }
    let mut bytes = vec![0; record.size as usize];
    source.read_exact_at(offset, &mut bytes)?;
    Ok(bytes)
}

fn read_iso_record_prefix(
    source: &mut dyn ByteSource,
    record: IsoRecord,
    buffer: &mut [u8],
) -> io::Result<()> {
    if record.directory || buffer.len() as u64 > record.size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "ISO record is smaller than requested prefix",
        ));
    }
    let offset = u64::from(record.extent)
        .checked_mul(ISO_SECTOR_SIZE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ISO extent overflow"))?;
    source.read_exact_at(offset, buffer)
}

trait ByteSource {
    fn len(&self) -> u64;
    fn bytes_read(&self) -> u64;
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()>;
}

struct MediaSource<M> {
    media: M,
    bytes_read: u64,
}

enum CueMediaSource {
    Cooked(MediaSource<crate::raw_cd_logical_media::CookedCdFileLogicalMedia>),
    Raw(MediaSource<crate::raw_cd_logical_media::RawCdFileLogicalMedia>),
}

impl ByteSource for CueMediaSource {
    fn len(&self) -> u64 {
        match self {
            Self::Cooked(source) => source.len(),
            Self::Raw(source) => source.len(),
        }
    }

    fn bytes_read(&self) -> u64 {
        match self {
            Self::Cooked(source) => source.bytes_read(),
            Self::Raw(source) => source.bytes_read(),
        }
    }

    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        match self {
            Self::Cooked(source) => source.read_exact_at(offset, buffer),
            Self::Raw(source) => source.read_exact_at(offset, buffer),
        }
    }
}

impl<M> MediaSource<M> {
    fn new(media: M) -> Self {
        Self {
            media,
            bytes_read: 0,
        }
    }
}

impl<M: crate::logical_media::LogicalMedia> ByteSource for MediaSource<M> {
    fn len(&self) -> u64 {
        self.media.len()
    }

    fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        self.media
            .read_at(offset, buffer)
            .map_err(|error| io::Error::new(io::ErrorKind::UnexpectedEof, error.to_string()))?;
        self.bytes_read = self.bytes_read.saturating_add(buffer.len() as u64);
        Ok(())
    }
}

struct FileSource {
    file: File,
    len: u64,
    bytes_read: u64,
}

impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn bytes_read(&self) -> u64 {
        self.bytes_read
    }
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        if self.bytes_read.saturating_add(buffer.len() as u64) > MAX_BYTES_READ {
            return Err(io::Error::other("64 MiB identity read limit reached"));
        }
        let end = offset
            .checked_add(buffer.len() as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "read offset overflow"))?;
        if end > self.len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "read exceeds image",
            ));
        }
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buffer)?;
        self.bytes_read += buffer.len() as u64;
        Ok(())
    }
}

struct SliceSource<'a> {
    data: &'a [u8],
    declared_len: u64,
    truncated: bool,
}

impl ByteSource for SliceSource<'_> {
    fn len(&self) -> u64 {
        self.declared_len
    }
    fn bytes_read(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "offset exceeds buffered ISO prefix",
            )
        })?;
        let end = start
            .checked_add(buffer.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "read overflow"))?;
        if end > self.data.len() {
            let message = if self.truncated {
                "64 MiB ZIP member read limit reached"
            } else {
                "read exceeds image"
            };
            return Err(if self.truncated {
                io::Error::other(message)
            } else {
                io::Error::new(io::ErrorKind::UnexpectedEof, message)
            });
        }
        buffer.copy_from_slice(&self.data[start..end]);
        Ok(())
    }
}

/// Opens one identity source read-only.
///
/// Delegates to [`crate::safe_read::open_bounded_read`], which is the single
/// place this build decides what may be opened: absolute paths only, no `.` or
/// `..`, no symlinked component, `O_NOFOLLOW`, and a device/inode re-check
/// after opening. A symlink is followed only when `trusted` names a source root
/// that contains both the link and its canonical target - so passing
/// [`TrustedRoots::none`] keeps the historical behaviour of refusing every
/// symlink outright.
fn open_read_only_regular(path: &Path, trusted: &TrustedRoots) -> Result<File, String> {
    crate::safe_read::open_bounded_read(path, trusted)
        .map(crate::safe_read::SafeFile::into_file)
        .map_err(|refusal| refusal.detail())
}

fn evidence(
    report: &GameIdentityReport,
    kind: IdentityKind,
    status: IdentityStatus,
    value: Option<String>,
    confidence: IdentityConfidence,
    diagnostic: &str,
    method: &str,
) -> IdentityEvidence {
    IdentityEvidence {
        kind,
        status,
        value,
        confidence,
        provenance: IdentityProvenance {
            archive_path: report.archive_path.clone(),
            member_path: None,
            member_index: None,
            method: method.to_string(),
        },
        diagnostic: diagnostic.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_with_source(
    report: &mut GameIdentityReport,
    kind: IdentityKind,
    status: IdentityStatus,
    value: Option<String>,
    confidence: IdentityConfidence,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
    method: &str,
    diagnostic: &str,
) {
    report.evidence.push(IdentityEvidence {
        kind,
        status,
        value,
        confidence,
        provenance: IdentityProvenance {
            archive_path: report.archive_path.clone(),
            member_path,
            member_index,
            method: method.to_string(),
        },
        diagnostic: diagnostic.to_string(),
    });
}

fn add_unavailable(report: &mut GameIdentityReport, status: IdentityStatus, diagnostic: &str) {
    let kinds: &[IdentityKind] = match report.platform {
        IdentityPlatform::PlayStation => &[IdentityKind::Ps1Serial],
        IdentityPlatform::PlayStation2 => {
            &[IdentityKind::Ps2Serial, IdentityKind::Pcsx2ExecutableCrc]
        }
        IdentityPlatform::Psp => &[IdentityKind::PspDiscId],
        IdentityPlatform::PlayStation3 => &[IdentityKind::Ps3TitleId],
        IdentityPlatform::Saturn => &[IdentityKind::SaturnProductNumber],
        IdentityPlatform::Dreamcast => &[IdentityKind::DreamcastProductCode],
        IdentityPlatform::SegaCd => &[IdentityKind::SegaCdProductCode],
        IdentityPlatform::ThreeDo => &[IdentityKind::ThreeDoDiscId],
        IdentityPlatform::Pcfx => &[IdentityKind::PcfxDiscHash],
        IdentityPlatform::PcEngineCd => &[IdentityKind::PceCdBootStructure],
        IdentityPlatform::NeoGeoCd => &[IdentityKind::NeoGeoCdBootStructure],
        IdentityPlatform::Ngp | IdentityPlatform::Ngpc => &[IdentityKind::LooseRomSha256],
        IdentityPlatform::Atari2600
        | IdentityPlatform::Atari5200
        | IdentityPlatform::Atari7800
        | IdentityPlatform::Atari8Bit
        | IdentityPlatform::AtariLynx
        | IdentityPlatform::AtariJaguar => &[IdentityKind::LooseRomSha256],
        IdentityPlatform::AtariST => &[],
        IdentityPlatform::GameCube | IdentityPlatform::Wii => {
            &[IdentityKind::DolphinGameId, IdentityKind::DolphinRevision]
        }
        IdentityPlatform::Xbox360 => &[IdentityKind::XexTitleId, IdentityKind::XexMediaId],
        IdentityPlatform::Xbox => &[IdentityKind::XbeTitleId],
        IdentityPlatform::ScummVM => &[IdentityKind::ScummVmGameId],
        IdentityPlatform::MegaDrive
        | IdentityPlatform::Snes
        | IdentityPlatform::Nes
        | IdentityPlatform::GameBoy
        | IdentityPlatform::GameBoyColor
        | IdentityPlatform::GameBoyAdvance
        | IdentityPlatform::N64
        | IdentityPlatform::WiiU
        | IdentityPlatform::ThreeDS
        | IdentityPlatform::Switch
        | IdentityPlatform::Other => &[],
    };
    for kind in kinds {
        report.evidence.push(evidence(
            report,
            *kind,
            status,
            None,
            IdentityConfidence::Unavailable,
            diagnostic,
            "format and safety eligibility",
        ));
    }
}

fn inspect_scummvm_directory(report: &mut GameIdentityReport) {
    report.format = IdentityImageFormat::ScummVmDirectory;
    let detection = crate::scummvm_detection::detect_scummvm_directory(&report.archive_path);
    apply_scummvm_detection(report, detection);
}

/// Test/integration seam for the same report construction used by the normal
/// installed-detector path. The caller supplies an explicitly selected
/// executable; no folder or filename is interpreted as identity.
pub fn inspect_scummvm_directory_with_executable(
    path: &Path,
    executable: &Path,
) -> GameIdentityReport {
    let mut report = GameIdentityReport {
        archive_path: path.to_path_buf(),
        platform: IdentityPlatform::ScummVM,
        format: IdentityImageFormat::ScummVmDirectory,
        evidence: Vec::new(),
        warnings: Vec::new(),
        bytes_read: 0,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: false,
    };
    report.evidence.push(evidence(
        &report,
        IdentityKind::Platform,
        IdentityStatus::Verified,
        Some("ScummVM".into()),
        IdentityConfidence::StructuredMetadata,
        "explicit ScummVM identity inspection requested",
        "caller-selected platform",
    ));
    apply_scummvm_detection(
        &mut report,
        crate::scummvm_detection::detect_scummvm_directory_with_executable(path, executable),
    );
    report
}

fn apply_scummvm_detection(
    report: &mut GameIdentityReport,
    detection: Result<
        crate::scummvm_detection::ScummVmDetectedGame,
        crate::scummvm_detection::ScummVmDetectionError,
    >,
) {
    match detection {
        Ok(game) => {
            let mut diagnostic = format!(
                "ScummVM native detector matched engine `{}` and game `{}`",
                game.engine_id, game.game_id
            );
            for (label, value) in [
                ("platform", game.platform.as_deref()),
                ("language", game.language.as_deref()),
                ("variant", game.variant.as_deref()),
            ] {
                if let Some(value) = value {
                    diagnostic.push_str(&format!(", {label} `{value}`"));
                }
            }
            if let Some(demo) = game.demo {
                diagnostic.push_str(if demo { ", demo" } else { ", full release" });
            }
            report.evidence.push(evidence(
                report,
                IdentityKind::ScummVmGameId,
                IdentityStatus::Verified,
                Some(format!("{}:{}", game.engine_id, game.game_id)),
                IdentityConfidence::StructuredMetadata,
                &diagnostic,
                "installed ScummVM --detect",
            ));
            report.complete = true;
        }
        Err(error) => {
            let status = match error {
                crate::scummvm_detection::ScummVmDetectionError::DetectorUnavailable => {
                    IdentityStatus::Deferred
                }
                crate::scummvm_detection::ScummVmDetectionError::Ambiguous(_)
                | crate::scummvm_detection::ScummVmDetectionError::MalformedOutput(_) => {
                    IdentityStatus::Ambiguous
                }
                crate::scummvm_detection::ScummVmDetectionError::NoMatch => {
                    IdentityStatus::Unsupported
                }
                _ => IdentityStatus::Invalid,
            };
            add_unavailable(report, status, &error.to_string());
        }
    }
}

fn add_filename_candidate(report: &mut GameIdentityReport) {
    let Some(stem) = report.archive_path.file_stem() else {
        return;
    };
    let stem = stem.to_string_lossy().to_ascii_uppercase();
    match report.platform {
        IdentityPlatform::GameCube | IdentityPlatform::Wii => {
            for token in stem.split(|character: char| !character.is_ascii_alphanumeric()) {
                if token.len() == 6
                    && token
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
                {
                    report.evidence.push(evidence(
                        report,
                        IdentityKind::DolphinGameId,
                        IdentityStatus::Candidate,
                        Some(token.to_string()),
                        IdentityConfidence::FilenameOnly,
                        "archive filename is candidate evidence only",
                        "archive filename token",
                    ));
                    break;
                }
            }
        }
        IdentityPlatform::PlayStation | IdentityPlatform::PlayStation2 => {
            let bytes = stem.as_bytes();
            for start in 0..bytes.len() {
                if let Some(serial) = bytes
                    .get(start..start.saturating_add(11))
                    .and_then(serial_from_boot_path)
                {
                    report.evidence.push(evidence(
                        report,
                        IdentityKind::Ps2Serial,
                        IdentityStatus::Candidate,
                        Some(serial),
                        IdentityConfidence::FilenameOnly,
                        "archive filename is candidate evidence only",
                        "archive filename token",
                    ));
                    break;
                }
            }
        }
        IdentityPlatform::Psp => {}
        IdentityPlatform::Xbox360 => {
            for token in stem.split(|character: char| !character.is_ascii_alphanumeric()) {
                if token.len() == 8 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    report.evidence.push(evidence(
                        report,
                        IdentityKind::XexTitleId,
                        IdentityStatus::Candidate,
                        Some(token.to_string()),
                        IdentityConfidence::FilenameOnly,
                        "archive filename is candidate evidence only",
                        "archive filename token",
                    ));
                    break;
                }
            }
        }
        IdentityPlatform::Saturn
        | IdentityPlatform::Dreamcast
        | IdentityPlatform::SegaCd
        | IdentityPlatform::PlayStation3
        | IdentityPlatform::MegaDrive
        | IdentityPlatform::Snes
        | IdentityPlatform::Nes
        | IdentityPlatform::GameBoy
        | IdentityPlatform::GameBoyColor
        | IdentityPlatform::GameBoyAdvance
        | IdentityPlatform::N64
        | IdentityPlatform::WiiU
        | IdentityPlatform::ThreeDS
        | IdentityPlatform::Switch
        | IdentityPlatform::Xbox
        | IdentityPlatform::ScummVM
        | IdentityPlatform::Pcfx
        | IdentityPlatform::PcEngineCd
        | IdentityPlatform::NeoGeoCd
        | IdentityPlatform::Ngp
        | IdentityPlatform::Ngpc
        | IdentityPlatform::ThreeDo
        | IdentityPlatform::Atari2600
        | IdentityPlatform::Atari5200
        | IdentityPlatform::Atari7800
        | IdentityPlatform::Atari8Bit
        | IdentityPlatform::AtariLynx
        | IdentityPlatform::AtariJaguar
        | IdentityPlatform::AtariST
        | IdentityPlatform::Other => {}
    }
}

fn source_error_status(error: &io::Error) -> IdentityStatus {
    if error.kind() == io::ErrorKind::Other {
        IdentityStatus::ResourceLimitReached
    } else {
        IdentityStatus::Invalid
    }
}

fn ascii_extension_is_iso(path: &[u8]) -> bool {
    let Some(name) = path.rsplit(|byte| *byte == b'/' || *byte == b'\\').next() else {
        return false;
    };
    let Some(dot) = name.iter().rposition(|byte| *byte == b'.') else {
        return false;
    };
    name[dot + 1..].eq_ignore_ascii_case(b"iso")
}

fn ascii_extension_is_xex(path: &[u8]) -> bool {
    let Some(name) = path.rsplit(|byte| *byte == b'/' || *byte == b'\\').next() else {
        return false;
    };
    let Some(dot) = name.iter().rposition(|byte| *byte == b'.') else {
        return false;
    };
    name[dot + 1..].eq_ignore_ascii_case(b"xex")
}

fn ascii_extension_is_xbe(path: &[u8]) -> bool {
    let Some(name) = path.rsplit(|byte| *byte == b'/' || *byte == b'\\').next() else {
        return false;
    };
    let Some(dot) = name.iter().rposition(|byte| *byte == b'.') else {
        return false;
    };
    name[dot + 1..].eq_ignore_ascii_case(b"xbe")
}

fn strip_iso_version(name: &[u8]) -> &[u8] {
    name.iter()
        .position(|byte| *byte == b';')
        .map_or(name, |position| &name[..position])
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcfx_boot_evidence::PCFX_PRIMARY_MAGIC;
    use crate::segacd_boot_evidence::{
        SEGA_CD_BOOT_SIGNATURE, SEGA_CD_PRODUCT_FIELD_BYTES, SEGA_CD_PRODUCT_FIELD_OFFSET,
    };
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use zip::CompressionMethod;
    use zip::write::{SimpleFileOptions, ZipWriter};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct FixtureDir(PathBuf);

    impl FixtureDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "archivefs-game-identity-{label}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ps3_sfo(title_id: &str) -> Vec<u8> {
        let key = b"TITLE_ID\0";
        let mut value = title_id.as_bytes().to_vec();
        value.push(0);
        let key_start = 36_u32;
        let data_start = key_start + key.len() as u32;
        let mut bytes = vec![0_u8; data_start as usize + value.len()];
        bytes[..4].copy_from_slice(crate::param_sfo::SFO_MAGIC);
        bytes[4..8].copy_from_slice(&0x0001_0100_u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&key_start.to_le_bytes());
        bytes[12..16].copy_from_slice(&data_start.to_le_bytes());
        bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
        bytes[20..22].copy_from_slice(&0_u16.to_le_bytes());
        bytes[22..24].copy_from_slice(&0x0204_u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&(value.len() as u32).to_le_bytes());
        bytes[28..32].copy_from_slice(&(value.len() as u32).to_le_bytes());
        bytes[32..36].copy_from_slice(&0_u32.to_le_bytes());
        bytes[key_start as usize..data_start as usize].copy_from_slice(key);
        bytes[data_start as usize..].copy_from_slice(&value);
        bytes
    }

    fn ps3_folder(root: &Path, title_id: &str, self_magic: bool) {
        let game = root.join("PS3_GAME");
        fs::create_dir_all(game.join("USRDIR")).unwrap();
        fs::write(game.join("PARAM.SFO"), ps3_sfo(title_id)).unwrap();
        let eboot: &[u8] = if self_magic {
            b"SCE\0valid"
        } else {
            b"not-self"
        };
        fs::write(game.join("USRDIR").join("EBOOT.BIN"), eboot).unwrap();
    }

    fn ps3_iso(title_id: &str) -> Vec<u8> {
        let mut iso = vec![0_u8; 25 * ISO_SECTOR_SIZE as usize];
        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        iso[terminator] = 255;
        iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        iso[terminator + 6] = 1;

        let root_offset = 20 * ISO_SECTOR_SIZE as usize;
        let game = directory_record(b"PS3_GAME", 21, ISO_SECTOR_SIZE as u32, true);
        iso[root_offset..root_offset + game.len()].copy_from_slice(&game);

        let game_offset = 21 * ISO_SECTOR_SIZE as usize;
        let dot = directory_record(&[0], 21, ISO_SECTOR_SIZE as u32, true);
        let parent = directory_record(&[1], 20, ISO_SECTOR_SIZE as u32, true);
        let sfo = directory_record(b"PARAM.SFO", 22, ps3_sfo(title_id).len() as u32, false);
        let usrdir = directory_record(b"USRDIR", 23, ISO_SECTOR_SIZE as u32, true);
        let mut cursor = game_offset;
        for record in [&dot, &parent, &sfo, &usrdir] {
            iso[cursor..cursor + record.len()].copy_from_slice(record);
            cursor += record.len();
        }
        let usrdir_offset = 23 * ISO_SECTOR_SIZE as usize;
        iso[usrdir_offset..usrdir_offset + dot.len()].copy_from_slice(&dot);
        iso[usrdir_offset + dot.len()..usrdir_offset + dot.len() + parent.len()]
            .copy_from_slice(&parent);
        let eboot = directory_record(b"EBOOT.BIN", 24, 9, false);
        let eboot_start = usrdir_offset + dot.len() + parent.len();
        iso[eboot_start..eboot_start + eboot.len()].copy_from_slice(&eboot);
        let sfo_bytes = ps3_sfo(title_id);
        iso[22 * ISO_SECTOR_SIZE as usize..22 * ISO_SECTOR_SIZE as usize + sfo_bytes.len()]
            .copy_from_slice(&sfo_bytes);
        iso[24 * ISO_SECTOR_SIZE as usize..24 * ISO_SECTOR_SIZE as usize + 9]
            .copy_from_slice(b"SCE\0valid");
        iso
    }

    fn ps3_pkg(content_id: &str) -> Vec<u8> {
        let total_size = 0x180_u64;
        let mut pkg = vec![0_u8; total_size as usize];
        pkg[..4].copy_from_slice(crate::ps3_disc_evidence::PKG_MAGIC);
        pkg[4..6].copy_from_slice(&0x8000_u16.to_be_bytes());
        pkg[6..8].copy_from_slice(&1_u16.to_be_bytes());
        pkg[8..12].copy_from_slice(&0x80_u32.to_be_bytes());
        pkg[12..16].copy_from_slice(&1_u32.to_be_bytes());
        pkg[16..20].copy_from_slice(&0x80_u32.to_be_bytes());
        pkg[20..24].copy_from_slice(&1_u32.to_be_bytes());
        pkg[24..32].copy_from_slice(&total_size.to_be_bytes());
        pkg[32..40].copy_from_slice(&0x90_u64.to_be_bytes());
        pkg[40..48].copy_from_slice(&0xf0_u64.to_be_bytes());
        let content_id = content_id.as_bytes();
        pkg[48..48 + content_id.len()].copy_from_slice(content_id);
        pkg
    }

    #[test]
    fn ps3_folder_resolves_title_id_from_content_and_bridge() {
        let directory = FixtureDir::new("ps3 – 日本語");
        ps3_folder(&directory.0, "BLUS30000", true);
        let report = inspect_game_identity(&directory.0, Some("PS3"));
        assert_eq!(report.platform, IdentityPlatform::PlayStation3);
        assert_eq!(report.verified_ps3_title_id(), Some("BLUS30000"));
        let (status, facts) =
            crate::launch::evidence_bridge::canonical_identity_from_game_report(&report);
        assert!(matches!(
            status,
            crate::launch::planning::CanonicalIdentityStatus::Resolved(_)
        ));
        assert_eq!(
            facts,
            vec![
                crate::launch::input_projection::VerifiedIdentityFact::Ps3TitleId(
                    "BLUS30000".to_string()
                )
            ]
        );
        let wrong_platform = inspect_game_identity(&directory.0, Some("PS2"));
        assert_eq!(wrong_platform.verified_ps3_title_id(), None);

        let iso_path = write_fixture(
            &directory,
            "PS3 content – BLUS30000.iso",
            &ps3_iso("BLUS30000"),
        );
        let iso_report = inspect_game_identity(&iso_path, Some("PS3"));
        assert_eq!(iso_report.verified_ps3_title_id(), Some("BLUS30000"));
    }

    #[test]
    fn ps3_folder_rejects_missing_malformed_or_non_self_content() {
        let directory = FixtureDir::new("ps3-invalid");
        ps3_folder(&directory.0, "BLUS30000", false);
        assert_eq!(
            inspect_game_identity(&directory.0, Some("PS3")).verified_ps3_title_id(),
            None
        );
        let missing = FixtureDir::new("ps3-missing");
        ps3_folder(&missing.0, "not-a-title", true);
        assert_eq!(
            inspect_game_identity(&missing.0, Some("PS3")).verified_ps3_title_id(),
            None
        );
        let filename_only = write_fixture(&directory, "BLUS30000", b"not a PS3 folder");
        assert_eq!(
            inspect_game_identity(&filename_only, Some("PS3")).verified_ps3_title_id(),
            None
        );
    }

    #[test]
    fn ps3_pkg_production_path_resolves_title_id_without_reading_payload() {
        let directory = FixtureDir::new("ps3-pkg");
        let path = write_fixture(
            &directory,
            "filename-does-not-matter.pkg",
            &ps3_pkg("EP0102-NPEB00342_00-CONTENT0000DLPKG"),
        );
        let report = inspect_game_identity(&path, Some("PS3"));
        assert_eq!(report.platform, IdentityPlatform::PlayStation3);
        assert_eq!(report.format, IdentityImageFormat::Pkg);
        assert_eq!(report.verified_ps3_title_id(), Some("NPEB00342"));
        assert_eq!(
            report.bytes_read,
            crate::ps3_disc_evidence::PKG_HEADER_BYTES as u64
        );
        assert!(report.complete);
        assert_eq!(
            crate::detect_platform(&path, &directory.0).as_deref(),
            Some("PS3")
        );
    }

    #[test]
    fn ps3_pkg_production_path_rejects_truncation_and_bad_structure() {
        let directory = FixtureDir::new("ps3-pkg-invalid");
        let truncated = write_fixture(&directory, "truncated.pkg", &[0x7f, b'P', b'K', b'G']);
        assert_eq!(
            inspect_game_identity(&truncated, Some("PS3")).verified_ps3_title_id(),
            None
        );

        let mut bad_type = ps3_pkg("EP0102-NPEB00342_00-CONTENT0000DLPKG");
        bad_type[6..8].copy_from_slice(&2_u16.to_be_bytes());
        let bad_type = write_fixture(&directory, "bad-type.pkg", &bad_type);
        assert_eq!(
            inspect_game_identity(&bad_type, Some("PS3")).verified_ps3_title_id(),
            None
        );

        let filename_only = write_fixture(&directory, "NPEB00342.pkg", b"not a package");
        assert_eq!(crate::detect_platform(&filename_only, &directory.0), None);
    }

    #[cfg(unix)]
    #[test]
    fn ps3_folder_rejects_symlinked_content() {
        use std::os::unix::fs::symlink;
        let directory = FixtureDir::new("ps3-symlink");
        let outside = FixtureDir::new("ps3-outside");
        ps3_folder(&outside.0, "BLUS30000", true);
        fs::create_dir(directory.0.join("PS3_GAME")).unwrap();
        symlink(
            outside.0.join("PS3_GAME").join("PARAM.SFO"),
            directory.0.join("PS3_GAME").join("PARAM.SFO"),
        )
        .unwrap();
        fs::create_dir(directory.0.join("PS3_GAME").join("USRDIR")).unwrap();
        fs::write(
            directory
                .0
                .join("PS3_GAME")
                .join("USRDIR")
                .join("EBOOT.BIN"),
            b"SCE\0",
        )
        .unwrap();
        assert_eq!(
            inspect_game_identity(&directory.0, Some("PS3")).verified_ps3_title_id(),
            None
        );
    }

    fn dolphin_fixture(platform: IdentityPlatform, id: &[u8; 6], revision: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; DOLPHIN_HEADER_BYTES];
        bytes[..6].copy_from_slice(id);
        bytes[6] = 1;
        bytes[7] = revision;
        match platform {
            IdentityPlatform::GameCube => {
                bytes[GAMECUBE_MAGIC_OFFSET..][..4].copy_from_slice(&GAMECUBE_MAGIC)
            }
            IdentityPlatform::Wii => bytes[WII_MAGIC_OFFSET..][..4].copy_from_slice(&WII_MAGIC),
            _ => unreachable!(),
        }
        bytes
    }

    fn directory_record(name: &[u8], extent: u32, size: u32, directory: bool) -> Vec<u8> {
        let length = 33 + name.len() + usize::from(name.len().is_multiple_of(2));
        let mut record = vec![0_u8; length];
        record[0] = length as u8;
        record[2..6].copy_from_slice(&extent.to_le_bytes());
        record[6..10].copy_from_slice(&extent.to_be_bytes());
        record[10..14].copy_from_slice(&size.to_le_bytes());
        record[14..18].copy_from_slice(&size.to_be_bytes());
        record[25] = if directory { 2 } else { 0 };
        record[28..30].copy_from_slice(&1_u16.to_le_bytes());
        record[30..32].copy_from_slice(&1_u16.to_be_bytes());
        record[32] = name.len() as u8;
        record[33..33 + name.len()].copy_from_slice(name);
        record
    }

    fn ps2_iso(cnf: &[u8], include_elf: bool, declared_cnf_size: Option<u32>) -> Vec<u8> {
        const SECTORS: usize = 24;
        let mut iso = vec![0_u8; SECTORS * ISO_SECTOR_SIZE as usize];
        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        iso[terminator] = 255;
        iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        iso[terminator + 6] = 1;

        let root_offset = 20 * ISO_SECTOR_SIZE as usize;
        let cnf_record = directory_record(
            b"SYSTEM.CNF;1",
            21,
            declared_cnf_size.unwrap_or(cnf.len() as u32),
            false,
        );
        iso[root_offset..root_offset + cnf_record.len()].copy_from_slice(&cnf_record);
        let mut cursor = root_offset + cnf_record.len();
        if include_elf {
            let elf_record = directory_record(b"SLUS_123.45;1", 22, 12, false);
            iso[cursor..cursor + elf_record.len()].copy_from_slice(&elf_record);
            cursor += elf_record.len();
            let elf_offset = 22 * ISO_SECTOR_SIZE as usize;
            iso[elf_offset..elf_offset + 12]
                .copy_from_slice(&[0x7f, b'E', b'L', b'F', 1, 2, 3, 4, 5, 6, 7, 8]);
        }
        iso[cursor] = 0;
        let cnf_offset = 21 * ISO_SECTOR_SIZE as usize;
        iso[cnf_offset..cnf_offset + cnf.len()].copy_from_slice(cnf);
        iso
    }

    fn ps1_iso(serial_name: &[u8], cnf: &[u8], include_executable: bool) -> Vec<u8> {
        const SECTORS: usize = 24;
        let mut iso = vec![0_u8; SECTORS * ISO_SECTOR_SIZE as usize];
        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        iso[terminator] = 255;
        iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        iso[terminator + 6] = 1;

        let root_offset = 20 * ISO_SECTOR_SIZE as usize;
        let cnf_record = directory_record(b"SYSTEM.CNF;1", 21, cnf.len() as u32, false);
        iso[root_offset..root_offset + cnf_record.len()].copy_from_slice(&cnf_record);
        let mut cursor = root_offset + cnf_record.len();
        if include_executable {
            let executable_record = directory_record(serial_name, 22, 12, false);
            iso[cursor..cursor + executable_record.len()].copy_from_slice(&executable_record);
            let executable_offset = 22 * ISO_SECTOR_SIZE as usize;
            iso[executable_offset..executable_offset + 12].copy_from_slice(b"PS-X EXE\0\0\0\0");
            cursor += executable_record.len();
        }
        iso[cursor] = 0;
        let cnf_offset = 21 * ISO_SECTOR_SIZE as usize;
        iso[cnf_offset..cnf_offset + cnf.len()].copy_from_slice(cnf);
        iso
    }

    fn psp_sfo(disc_id: &[u8]) -> Vec<u8> {
        let mut value = disc_id.to_vec();
        value.push(0);
        let key = b"DISC_ID\0";
        let key_start = 20 + 16;
        let data_start = key_start + 8;
        let mut out = vec![0_u8; data_start];
        out[0..4].copy_from_slice(crate::param_sfo::SFO_MAGIC);
        out[4..8].copy_from_slice(&0x0101_u32.to_le_bytes());
        out[8..12].copy_from_slice(&(key_start as u32).to_le_bytes());
        out[12..16].copy_from_slice(&(data_start as u32).to_le_bytes());
        out[16..20].copy_from_slice(&1_u32.to_le_bytes());
        out[20..22].copy_from_slice(&0_u16.to_le_bytes());
        out[22..24].copy_from_slice(&0x0204_u16.to_le_bytes());
        out[24..28].copy_from_slice(&(value.len() as u32).to_le_bytes());
        out[28..32].copy_from_slice(&(value.len() as u32).to_le_bytes());
        out[32..36].copy_from_slice(&0_u32.to_le_bytes());
        out[key_start..key_start + key.len()].copy_from_slice(key);
        out.extend_from_slice(&value);
        out
    }

    fn psp_iso(disc_id: &[u8], include_umd: bool) -> Vec<u8> {
        let sfo = psp_sfo(disc_id);
        let mut iso = vec![0_u8; 28 * ISO_SECTOR_SIZE as usize];
        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        iso[terminator] = 255;
        iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        iso[terminator + 6] = 1;
        let root_offset = 20 * ISO_SECTOR_SIZE as usize;
        let dir = directory_record(b"PSP_GAME", 21, ISO_SECTOR_SIZE as u32, true);
        iso[root_offset..root_offset + dir.len()].copy_from_slice(&dir);
        let umd = directory_record(b"UMD_DATA.BIN;1", 23, 3, false);
        let umd_offset = root_offset + dir.len();
        if include_umd {
            iso[umd_offset..umd_offset + umd.len()].copy_from_slice(&umd);
        }
        iso[umd_offset + if include_umd { umd.len() } else { 0 }] = 0;
        let psp_offset = 21 * ISO_SECTOR_SIZE as usize;
        let sfo_record = directory_record(b"PARAM.SFO;1", 22, sfo.len() as u32, false);
        iso[psp_offset..psp_offset + sfo_record.len()].copy_from_slice(&sfo_record);
        iso[psp_offset + sfo_record.len()] = 0;
        iso[22 * ISO_SECTOR_SIZE as usize..22 * ISO_SECTOR_SIZE as usize + sfo.len()]
            .copy_from_slice(&sfo);
        iso[23 * ISO_SECTOR_SIZE as usize..23 * ISO_SECTOR_SIZE as usize + 3]
            .copy_from_slice(b"UMD");
        iso
    }

    fn psp_pbp(disc_id: &[u8]) -> Vec<u8> {
        let sfo = psp_sfo(disc_id);
        let sfo_start = crate::psp_pbp_evidence::PBP_HEADER_BYTES as u32;
        let sfo_end = sfo_start + sfo.len() as u32;
        let mut offsets = [sfo_end; crate::psp_pbp_evidence::PBP_SECTION_COUNT];
        offsets[crate::psp_pbp_evidence::PBP_SECTION_PARAM_SFO] = sfo_start;
        let mut out = vec![0_u8; crate::psp_pbp_evidence::PBP_HEADER_BYTES];
        out[..4].copy_from_slice(crate::psp_pbp_evidence::PBP_MAGIC);
        for (index, offset) in offsets.iter().enumerate() {
            out[8 + index * 4..12 + index * 4].copy_from_slice(&offset.to_le_bytes());
        }
        out.extend_from_slice(&sfo);
        out
    }

    fn saturn_iso(product_number: &[u8]) -> Vec<u8> {
        let mut iso = vec![0_u8; 24 * ISO_SECTOR_SIZE as usize];
        let mut system_id = vec![b' '; SATURN_SYSTEM_ID_BYTES];
        system_id[..16].copy_from_slice(b"SEGA SEGASATURN ");
        system_id[0x10..0x20].copy_from_slice(b"SEGA ENTERPRISES");
        let product_len = product_number.len().min(10);
        system_id[0x20..0x20 + product_len].copy_from_slice(&product_number[..product_len]);
        system_id[0x2a..0x30].copy_from_slice(b"V1.004");
        system_id[0x30..0x38].copy_from_slice(b"19961117");
        system_id[0x38..0x40].copy_from_slice(b"CD-1/1  ");
        iso[..SATURN_SYSTEM_ID_BYTES].copy_from_slice(&system_id);

        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        iso[terminator] = 255;
        iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        iso[terminator + 6] = 1;
        iso
    }

    fn dreamcast_iso(product_code: &[u8]) -> Vec<u8> {
        let mut iso = vec![0_u8; 24 * ISO_SECTOR_SIZE as usize];
        let mut ip_bin = vec![b' '; IP_BIN_META_BYTES];
        ip_bin[..16].copy_from_slice(b"SEGA SEGAKATANA ");
        ip_bin[0x10..0x20].copy_from_slice(b"SEGA ENTERPRISES");
        let product_len = product_code.len().min(10);
        ip_bin[0x40..0x40 + product_len].copy_from_slice(&product_code[..product_len]);
        ip_bin[0x4a..0x50].copy_from_slice(b"V1.000");
        ip_bin[0x50..0x60].copy_from_slice(b"20000915        ");
        ip_bin[0x60..0x70].copy_from_slice(b"1ST_READ.BIN    ");
        iso[..IP_BIN_META_BYTES].copy_from_slice(&ip_bin);

        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        iso[terminator] = 255;
        iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        iso[terminator + 6] = 1;
        iso
    }

    fn sega_cd_iso(product_code: &[u8]) -> Vec<u8> {
        let mut iso = vec![0_u8; 24 * ISO_SECTOR_SIZE as usize];
        iso[..SEGA_CD_BOOT_SIGNATURE.len()].copy_from_slice(SEGA_CD_BOOT_SIGNATURE);
        iso[SEGA_CD_PRODUCT_FIELD_OFFSET
            ..SEGA_CD_PRODUCT_FIELD_OFFSET + SEGA_CD_PRODUCT_FIELD_BYTES]
            .copy_from_slice(product_code);

        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        iso[terminator] = 255;
        iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        iso[terminator + 6] = 1;
        iso
    }

    /// Wraps a 2048-byte-sectored `image` into a genuine,
    /// `open_chd_track_logical_media`-openable uncompressed CHD v5 file (one
    /// MODE1_RAW track, track 1, zero pregap - the exact single-track shape
    /// the pure-Rust CHD reader supports), so CHD identity tests exercise
    /// the real bounded CHD decode path rather than a shortcut. This
    /// deliberately re-derives the same minimal CHD v5 header/metadata/map/
    /// hunk-data layout that `chd_logical_media`'s own private test-only
    /// `build_uncompressed_chd` uses (that helper cannot be imported across
    /// module boundaries) - it is not a second CHD *reader*, only a second
    /// CHD *test fixture writer*, mirroring one that already exists and is
    /// already trusted. Each `LOGICAL_BLOCK_BYTES` (2048-byte) block of
    /// `image` becomes one `RAW_SECTOR_BYTES` (2352-byte) MODE1_RAW sector,
    /// matching `chd_logical_media`'s own `mode1_sectors_for` test helper.
    /// Makes no assumption that `image` is ISO 9660: [`ps1_chd`] layers the
    /// one ISO 9660 PVD field its filesystem path needs on top of this,
    /// while a caller whose on-disc structure is not ISO 9660 (3DO OperaFS -
    /// see [`threedo_chd`]) wraps the image verbatim.
    fn uncompressed_mode1_raw_chd(image: &[u8]) -> Vec<u8> {
        use crate::dat::archive::chd::CHD_MAGIC;
        use crate::raw_cd_sector::{LOGICAL_BLOCK_BYTES, MODE1_USER_DATA_OFFSET, RAW_SECTOR_BYTES};

        fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
            bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
        }

        let sectors: Vec<[u8; RAW_SECTOR_BYTES]> = image
            .chunks(LOGICAL_BLOCK_BYTES)
            .map(|block| {
                let mut sector = [0u8; RAW_SECTOR_BYTES];
                sector[MODE1_USER_DATA_OFFSET..MODE1_USER_DATA_OFFSET + block.len()]
                    .copy_from_slice(block);
                sector
            })
            .collect();
        let frames = sectors.len() as u32;
        let frames_per_hunk = frames.max(1);
        let unit_bytes = RAW_SECTOR_BYTES as u32;
        let hunk_bytes = unit_bytes * frames_per_hunk;
        let logical_bytes = frames as u64 * unit_bytes as u64;

        let mut data = vec![0u8; 124];
        data[0..8].copy_from_slice(CHD_MAGIC);
        put_u32(&mut data, 8, 124);
        put_u32(&mut data, 12, 5);
        put_u64(&mut data, 32, logical_bytes);
        put_u32(&mut data, 56, hunk_bytes);
        put_u32(&mut data, 60, unit_bytes);

        let meta_offset = data.len() as u64;
        let payload = format!(
            "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:{frames} PREGAP:0 PGTYPE:NONE PGSUB:NONE POSTGAP:0"
        )
        .into_bytes();
        data.extend_from_slice(&u32::from_be_bytes(*b"CHT2").to_be_bytes());
        data.push(0);
        let length = payload.len() as u32;
        data.extend_from_slice(&length.to_be_bytes()[1..]);
        data.extend_from_slice(&0u64.to_be_bytes());
        data.extend_from_slice(&payload);

        let hunk_count = logical_bytes.div_ceil(hunk_bytes as u64) as u32;
        let map_offset = data.len() as u64;
        let map_end = map_offset + hunk_count as u64 * 4;
        let hunk_data_start = map_end.div_ceil(hunk_bytes as u64).max(1) * hunk_bytes as u64;
        let base_index = hunk_data_start / hunk_bytes as u64;
        for index in 0..hunk_count {
            let value = (base_index + index as u64) as u32;
            data.extend_from_slice(&value.to_be_bytes());
        }

        data.resize(hunk_data_start as usize, 0);
        for sector in &sectors {
            data.extend_from_slice(sector);
        }

        put_u64(&mut data, 40, map_offset);
        put_u64(&mut data, 48, meta_offset);
        data
    }

    /// Wraps an ISO 9660 image (e.g. from [`ps1_iso`]) into a genuine
    /// uncompressed CHD v5 file, first completing the one PVD
    /// logical-block-size field the shared plain-ISO fixtures leave zeroed.
    /// `ps1_iso` omits it because the plain-ISO `ByteSource` reader
    /// (`find_iso_path`) never reads it - it works purely off fixed
    /// 2048-byte offsets - whereas the CHD path goes through
    /// `crate::iso9660::observe_iso9660`, which (correctly, for a real disc)
    /// insists this field says 2048 both-endian. Completing it here is
    /// filling in a real ISO 9660 field, not working around a bug.
    fn ps1_chd(image: &[u8]) -> Vec<u8> {
        use crate::raw_cd_sector::LOGICAL_BLOCK_BYTES;

        let mut image = image.to_vec();
        let pvd = 16 * LOGICAL_BLOCK_BYTES;
        image[pvd + 128..pvd + 130].copy_from_slice(&(LOGICAL_BLOCK_BYTES as u16).to_le_bytes());
        image[pvd + 130..pvd + 132].copy_from_slice(&(LOGICAL_BLOCK_BYTES as u16).to_be_bytes());
        uncompressed_mode1_raw_chd(&image)
    }

    /// Wraps a 3DO OperaFS image (e.g. from [`threedo_fixture`]) into the
    /// same genuine uncompressed single-track CHD v5 file. OperaFS is not
    /// ISO 9660, so unlike [`ps1_chd`] there is no PVD field to complete -
    /// the bytes are wrapped verbatim, exactly as a real 3DO CHD's decoded
    /// data track would present them.
    fn threedo_chd(image: &[u8]) -> Vec<u8> {
        uncompressed_mode1_raw_chd(image)
    }

    fn ps1_raw_bin(image: &[u8]) -> Vec<u8> {
        use crate::raw_cd_sector::{
            LOGICAL_BLOCK_BYTES, MODE1_USER_DATA_OFFSET, RAW_SECTOR_BYTES, SYNC_PATTERN,
        };

        image
            .chunks_exact(LOGICAL_BLOCK_BYTES)
            .flat_map(|logical| {
                let mut sector = [0_u8; RAW_SECTOR_BYTES];
                sector[..SYNC_PATTERN.len()].copy_from_slice(&SYNC_PATTERN);
                sector[15] = 1;
                sector[MODE1_USER_DATA_OFFSET..MODE1_USER_DATA_OFFSET + LOGICAL_BLOCK_BYTES]
                    .copy_from_slice(logical);
                sector
            })
            .collect()
    }

    fn write_fixture(directory: &FixtureDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = directory.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn parses_system_cnf_and_serial_without_filename_elevation() {
        let boot =
            parse_system_cnf_boot2(b"VER = 1.00\r\nBOOT2 = cdrom0:\\SLUS_123.45;1\r\n").unwrap();
        assert_eq!(boot, b"SLUS_123.45");
        assert_eq!(serial_from_boot_path(&boot).as_deref(), Some("SLUS-12345"));
    }

    #[test]
    fn traversal_boot_path_is_rejected() {
        assert!(parse_system_cnf_boot2(b"BOOT2=cdrom0:\\..\\SLUS_123.45;1").is_err());
    }

    #[test]
    fn pcsx2_crc_is_little_endian_word_xor_and_ignores_tail() {
        assert_eq!(
            pcsx2_executable_crc(&[1, 2, 3, 4, 5, 6, 7, 8, 9]),
            0x0c04_0404
        );
    }

    #[test]
    fn unsupported_filename_never_becomes_verified() {
        let report = inspect_game_identity(Path::new("/games/GAME01.chd"), Some("GameCube"));
        assert_eq!(report.verified_dolphin_game_id(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::DolphinGameId
                && item.status == IdentityStatus::Candidate
                && item.value.as_deref() == Some("GAME01")
        }));
        assert!(
            report
                .evidence
                .iter()
                .all(|item| item.kind != IdentityKind::DolphinGameId
                    || item.status != IdentityStatus::Verified)
        );
    }

    #[test]
    fn production_identity_reader_has_no_write_execution_or_network_path() {
        let production = include_str!("game_identity.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in [
            "File::create",
            "fs::write",
            ".write(",
            "create_dir",
            "remove_dir",
            "Command::",
            "std::process",
            "TcpStream",
            "ureq::",
            "http://",
            "https://",
        ] {
            assert!(
                !production.contains(forbidden),
                "production identity reader contains forbidden path: {forbidden}"
            );
        }
    }

    #[test]
    fn verifies_gamecube_id_disc_number_and_revision() {
        let directory = FixtureDir::new("gamecube");
        let path = write_fixture(
            &directory,
            "not-an-id.iso",
            &dolphin_fixture(IdentityPlatform::GameCube, b"GM8E01", 3),
        );
        let report = inspect_game_identity(&path, Some("Nintendo GameCube"));
        assert_eq!(report.verified_dolphin_game_id(), Some("GM8E01"));
        assert_eq!(report.verified_dolphin_revision(), Some(3));
        assert_eq!(report.bytes_read, DOLPHIN_HEADER_BYTES as u64);
        assert!(report.complete);
    }

    #[test]
    fn verifies_wii_id_but_keeps_outer_revision_candidate() {
        let directory = FixtureDir::new("wii");
        let path = write_fixture(
            &directory,
            "wii.iso",
            &dolphin_fixture(IdentityPlatform::Wii, b"RMGE01", 7),
        );
        let report = inspect_game_identity(&path, Some("Wii"));
        assert_eq!(report.verified_dolphin_game_id(), Some("RMGE01"));
        assert_eq!(report.verified_dolphin_revision(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::DolphinRevision
                && item.status == IdentityStatus::Candidate
                && item.value.as_deref() == Some("7")
        }));
    }

    #[test]
    fn malformed_and_truncated_dolphin_headers_are_invalid() {
        let directory = FixtureDir::new("bad-header");
        let truncated = write_fixture(&directory, "short.iso", b"GM8E01");
        let malformed = write_fixture(&directory, "wrong.iso", &[0_u8; DOLPHIN_HEADER_BYTES]);
        for path in [truncated, malformed] {
            let report = inspect_game_identity(&path, Some("GameCube"));
            assert_eq!(report.verified_dolphin_game_id(), None);
            assert!(
                report
                    .evidence
                    .iter()
                    .any(|item| item.status == IdentityStatus::Invalid)
            );
        }
    }

    #[test]
    fn zip_with_one_iso_reads_only_the_dolphin_header() {
        let directory = FixtureDir::new("zip-iso");
        let path = directory.0.join("container.zip");
        let file = fs::File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file(
                "disc.iso",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        let mut image = dolphin_fixture(IdentityPlatform::GameCube, b"GALE01", 2);
        image.resize(1024 * 1024, 0);
        writer.write_all(&image).unwrap();
        writer.finish().unwrap();

        let report = inspect_game_identity(&path, Some("GameCube"));
        assert_eq!(report.verified_dolphin_game_id(), Some("GALE01"));
        assert_eq!(report.bytes_read, DOLPHIN_HEADER_BYTES as u64);
        assert_eq!(report.archive_members_inspected, 1);
        assert_eq!(report.nested_container_depth, 1);
    }

    #[test]
    fn direct_gcm_is_verified_exactly_like_direct_iso() {
        let directory = FixtureDir::new("gcm");
        let path = write_fixture(
            &directory,
            "Army Men - RTS (USA).gcm",
            &dolphin_fixture(IdentityPlatform::GameCube, b"GA2E7D", 0),
        );
        let report = inspect_game_identity(&path, Some("GameCube"));
        assert_eq!(report.verified_dolphin_game_id(), Some("GA2E7D"));
        assert_eq!(report.bytes_read, DOLPHIN_HEADER_BYTES as u64);
        assert!(report.complete);
    }

    fn rvz_fixture(disc_type: u32, dhead: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0_u8; (WIA_DHEAD_OFFSET as usize) + 0x80];
        bytes[..4].copy_from_slice(&RVZ_MAGIC);
        bytes[WIA_DISC_TYPE_OFFSET as usize..][..4].copy_from_slice(&disc_type.to_be_bytes());
        let dhead_start = WIA_DHEAD_OFFSET as usize;
        bytes[dhead_start..dhead_start + dhead.len()].copy_from_slice(dhead);
        bytes
    }

    #[test]
    fn rvz_recovers_exact_game_id_from_the_uncompressed_header() {
        let directory = FixtureDir::new("rvz");
        let dhead = dolphin_fixture(IdentityPlatform::GameCube, b"GZ2E01", 1);
        let path = write_fixture(
            &directory,
            "ZooCube (USA).rvz",
            &rvz_fixture(WIA_DISC_TYPE_GAMECUBE, &dhead),
        );
        let report = inspect_game_identity(&path, Some("GameCube"));
        assert_eq!(report.format, IdentityImageFormat::Rvz);
        assert_eq!(report.verified_dolphin_game_id(), Some("GZ2E01"));
        assert_eq!(report.verified_dolphin_revision(), Some(1));
        assert!(report.complete);
    }

    #[test]
    fn rvz_with_wrong_magic_or_disc_type_is_invalid_not_stuck() {
        let directory = FixtureDir::new("rvz-bad");
        let dhead = dolphin_fixture(IdentityPlatform::GameCube, b"GZ2E01", 1);

        let mut bad_magic = rvz_fixture(WIA_DISC_TYPE_GAMECUBE, &dhead);
        bad_magic[..4].copy_from_slice(b"ZZZZ");
        let path = write_fixture(&directory, "bad-magic.rvz", &bad_magic);
        let report = inspect_game_identity(&path, Some("GameCube"));
        assert_eq!(report.verified_dolphin_game_id(), None);
        assert!(report.complete || !report.evidence.is_empty());
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Invalid)
        );

        // disc_type says Wii while the catalogue platform hint says GameCube.
        let wrong_type = rvz_fixture(WIA_DISC_TYPE_WII, &dhead);
        let path = write_fixture(&directory, "wrong-type.rvz", &wrong_type);
        let report = inspect_game_identity(&path, Some("GameCube"));
        assert_eq!(report.verified_dolphin_game_id(), None);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Invalid)
        );
    }

    #[test]
    fn rvz_never_leaves_identity_pending() {
        let directory = FixtureDir::new("rvz-final");
        let dhead = dolphin_fixture(IdentityPlatform::GameCube, b"GZ2E01", 1);
        let path = write_fixture(
            &directory,
            "final-state.rvz",
            &rvz_fixture(WIA_DISC_TYPE_GAMECUBE, &dhead),
        );
        let report = inspect_game_identity(&path, Some("GameCube"));
        // Every evidence item reaches a terminal status - none is `Deferred`
        // for an RVZ file that was actually decoded.
        assert!(
            report
                .evidence
                .iter()
                .all(|item| item.status != IdentityStatus::Deferred)
        );
    }

    fn ciso_fixture(block_size: u32, block0_present: bool, dhead: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0_u8; CISO_HEADER_SIZE as usize + block_size as usize];
        bytes[..4].copy_from_slice(&CISO_MAGIC);
        bytes[4..8].copy_from_slice(&block_size.to_le_bytes());
        if block0_present {
            bytes[CISO_MAP_OFFSET as usize] = 1;
            let data_start = CISO_HEADER_SIZE as usize;
            bytes[data_start..data_start + dhead.len()].copy_from_slice(dhead);
        }
        bytes
    }

    fn wbfs_fixture(file_name_id: &[u8; 6], revision: u8) -> Vec<u8> {
        const HD_SHIFT: u8 = 9;
        const WBFS_SHIFT: u8 = 21;
        let file_len = 4_usize << WBFS_SHIFT;
        let mut bytes = vec![0_u8; file_len];
        bytes[..4].copy_from_slice(&WBFS_MAGIC);
        let hd_sectors = u32::try_from(file_len >> HD_SHIFT).unwrap();
        bytes[4..8].copy_from_slice(&hd_sectors.to_be_bytes());
        bytes[WBFS_HD_SEC_SZ_S_OFFSET as usize] = HD_SHIFT;
        bytes[WBFS_WBFS_SEC_SZ_S_OFFSET as usize] = WBFS_SHIFT;
        bytes[WBFS_DISC_TABLE_OFFSET as usize] = 1;

        let disc_info = 1_usize << HD_SHIFT;
        let mut header = dolphin_fixture(IdentityPlatform::Wii, file_name_id, revision);
        header[6] = 0;
        bytes[disc_info..disc_info + header.len()].copy_from_slice(&header);
        // Logical Wii sector zero is stored in physical WBFS sector one.
        bytes[disc_info + WBFS_DISC_INFO_HEADER_BYTES as usize..][..2]
            .copy_from_slice(&1_u16.to_be_bytes());
        bytes
    }

    fn evidence_status(report: &GameIdentityReport, status: IdentityStatus) -> bool {
        report.evidence.iter().any(|item| item.status == status)
    }

    #[test]
    fn ciso_recovers_exact_game_id_from_the_first_stored_block() {
        let directory = FixtureDir::new("ciso");
        let dhead = dolphin_fixture(IdentityPlatform::GameCube, b"GC3E01", 0);
        let path = write_fixture(&directory, "disc.ciso", &ciso_fixture(2048, true, &dhead));
        let report = inspect_game_identity(&path, Some("GameCube"));
        assert_eq!(report.format, IdentityImageFormat::Ciso);
        assert_eq!(report.verified_dolphin_game_id(), Some("GC3E01"));
        assert!(report.complete);
    }

    #[test]
    fn ciso_missing_first_block_reports_missing_not_stuck() {
        let directory = FixtureDir::new("ciso-missing");
        let dhead = dolphin_fixture(IdentityPlatform::GameCube, b"GC3E01", 0);
        let path = write_fixture(
            &directory,
            "sparse.ciso",
            &ciso_fixture(2048, false, &dhead),
        );
        let report = inspect_game_identity(&path, Some("GameCube"));
        assert_eq!(report.verified_dolphin_game_id(), None);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Missing)
        );
    }

    #[test]
    fn wbfs_extracts_verified_wii_identity_from_bytes_not_filename() {
        let directory = FixtureDir::new("wbfs-valid");
        let path = write_fixture(
            &directory,
            "Wrong Game ID [RZDE01].wbfs",
            &wbfs_fixture(b"SMNE01", 1),
        );
        let report = inspect_catalogued_game_identity(&path, Some("Wii"));
        assert_eq!(report.format, IdentityImageFormat::Wbfs);
        assert_eq!(report.verified_dolphin_game_id(), Some("SMNE01"));
        assert_eq!(
            report.verified_value(IdentityKind::DolphinDiscNumber),
            Some("0")
        );
        assert_eq!(
            report.verified_value(IdentityKind::DolphinRegion),
            Some("E")
        );
        assert_eq!(report.verified_dolphin_revision(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::DolphinRevision
                && item.status == IdentityStatus::Candidate
                && item.value.as_deref() == Some("1")
        }));
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::DolphinGameId
                && item.status == IdentityStatus::Candidate
                && item.value.as_deref() == Some("RZDE01")
        }));
        assert!(report.bytes_read > DOLPHIN_HEADER_BYTES as u64);
        assert!(report.bytes_read < 128 * 1024);
        assert!(report.complete);
    }

    #[test]
    fn wbfs_rejects_invalid_and_truncated_headers() {
        let directory = FixtureDir::new("wbfs-header-errors");
        let mut bad_magic = wbfs_fixture(b"SMNE01", 0);
        bad_magic[..4].copy_from_slice(b"NOPE");
        let report = inspect_catalogued_game_identity(
            &write_fixture(&directory, "bad.wbfs", &bad_magic),
            Some("Wii"),
        );
        assert!(evidence_status(&report, IdentityStatus::Invalid));
        assert_eq!(report.verified_dolphin_game_id(), None);

        let truncated = write_fixture(&directory, "short.wbfs", b"WBFS\0\0");
        let report = inspect_catalogued_game_identity(&truncated, Some("Wii"));
        assert!(evidence_status(&report, IdentityStatus::Invalid));
        assert_eq!(report.bytes_read, 0);
    }

    #[test]
    fn wbfs_rejects_invalid_sector_shifts_and_declared_length() {
        let directory = FixtureDir::new("wbfs-size-errors");
        for (name, offset, value) in [
            ("host", WBFS_HD_SEC_SZ_S_OFFSET as usize, 8),
            ("wbfs", WBFS_WBFS_SEC_SZ_S_OFFSET as usize, 63),
        ] {
            let mut bytes = wbfs_fixture(b"SMNE01", 0);
            bytes[offset] = value;
            let report = inspect_catalogued_game_identity(
                &write_fixture(&directory, &format!("{name}.wbfs"), &bytes),
                Some("Wii"),
            );
            assert!(evidence_status(&report, IdentityStatus::Invalid));
        }
        let mut wrong_length = wbfs_fixture(b"SMNE01", 0);
        wrong_length[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        let report = inspect_catalogued_game_identity(
            &write_fixture(&directory, "declared.wbfs", &wrong_length),
            Some("Wii"),
        );
        assert!(evidence_status(&report, IdentityStatus::Invalid));
        assert_eq!(checked_wbfs_sector_offset(u64::MAX, 2), None);
    }

    #[test]
    fn wbfs_rejects_empty_ambiguous_and_out_of_range_disc_slots() {
        let directory = FixtureDir::new("wbfs-slots");
        let mut empty = wbfs_fixture(b"SMNE01", 0);
        empty[WBFS_DISC_TABLE_OFFSET as usize] = 0;
        let report = inspect_catalogued_game_identity(
            &write_fixture(&directory, "empty.wbfs", &empty),
            Some("Wii"),
        );
        assert!(evidence_status(&report, IdentityStatus::Invalid));

        let mut ambiguous = wbfs_fixture(b"SMNE01", 0);
        ambiguous[WBFS_DISC_TABLE_OFFSET as usize + 1] = 1;
        let report = inspect_catalogued_game_identity(
            &write_fixture(&directory, "multi.wbfs", &ambiguous),
            Some("Wii"),
        );
        assert!(evidence_status(&report, IdentityStatus::Ambiguous));

        let mut outside = wbfs_fixture(b"SMNE01", 0);
        outside[WBFS_DISC_TABLE_OFFSET as usize] = 0;
        outside[WBFS_DISC_TABLE_OFFSET as usize + 500] = 1;
        let report = inspect_catalogued_game_identity(
            &write_fixture(&directory, "outside.wbfs", &outside),
            Some("Wii"),
        );
        assert!(evidence_status(&report, IdentityStatus::Invalid));
    }

    #[test]
    fn wbfs_rejects_invalid_mapping_and_game_id_bytes() {
        let directory = FixtureDir::new("wbfs-content-errors");
        let mut mapping = wbfs_fixture(b"SMNE01", 0);
        let wlba = (1_usize << 9) + WBFS_DISC_INFO_HEADER_BYTES as usize;
        mapping[wlba..wlba + 2].copy_from_slice(&u16::MAX.to_be_bytes());
        let report = inspect_catalogued_game_identity(
            &write_fixture(&directory, "mapping.wbfs", &mapping),
            Some("Wii"),
        );
        assert!(evidence_status(&report, IdentityStatus::Invalid));

        let mut invalid_id = wbfs_fixture(b"SMNE01", 0);
        invalid_id[1_usize << 9] = 0xff;
        let report = inspect_catalogued_game_identity(
            &write_fixture(&directory, "invalid-id.wbfs", &invalid_id),
            Some("Wii"),
        );
        assert!(evidence_status(&report, IdentityStatus::Invalid));
        assert_eq!(report.verified_dolphin_game_id(), None);
    }

    #[cfg(unix)]
    #[test]
    fn wbfs_symlink_is_refused() {
        use std::os::unix::fs::symlink;

        let directory = FixtureDir::new("wbfs-symlink");
        let target = write_fixture(&directory, "target.wbfs", &wbfs_fixture(b"SMNE01", 0));
        let link = directory.0.join("SMNE01.wbfs");
        symlink(&target, &link).unwrap();
        let report = inspect_catalogued_game_identity(&link, Some("Wii"));
        assert_eq!(report.verified_dolphin_game_id(), None);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.diagnostic.contains("symlink refused"))
        );
    }

    /// A symlinked library is the normal arrangement, so bounded identity must
    /// work through one when the roots are supplied - and keep refusing when
    /// they are not, which is what `wbfs_symlink_is_refused` above asserts.
    #[test]
    fn wbfs_identity_through_a_trusted_symlink_is_verified() {
        use std::os::unix::fs::symlink;

        let directory = FixtureDir::new("wbfs-symlink-trusted");
        let downloads = directory.0.join("downloads");
        let library = directory.0.join("library");
        fs::create_dir_all(&downloads).unwrap();
        fs::create_dir_all(&library).unwrap();
        let target = downloads.join("target.wbfs");
        fs::write(&target, wbfs_fixture(b"SMNE01", 0)).unwrap();
        let link = library.join("SMNE01.wbfs");
        symlink(&target, &link).unwrap();

        let trusted = TrustedRoots::from_paths([&library, &downloads]);
        let report = inspect_catalogued_game_identity_in_roots(&link, Some("Wii"), &trusted);
        assert_eq!(
            report.verified_dolphin_game_id(),
            Some("SMNE01"),
            "identity must resolve through a link inside the configured roots: {:?}",
            report.evidence
        );
        assert!(report.complete);
        assert!(
            report.bytes_read > 0 && report.bytes_read < MAX_BYTES_READ,
            "the read must stay bounded: {} bytes",
            report.bytes_read
        );
    }

    #[test]
    fn gamecube_identity_through_a_trusted_symlink_is_verified() {
        use std::os::unix::fs::symlink;

        let directory = FixtureDir::new("gcm-symlink-trusted");
        let downloads = directory.0.join("downloads");
        let library = directory.0.join("library");
        fs::create_dir_all(&downloads).unwrap();
        fs::create_dir_all(&library).unwrap();
        let target = downloads.join("target.gcm");
        fs::write(
            &target,
            dolphin_fixture(IdentityPlatform::GameCube, b"GALE01", 2),
        )
        .unwrap();
        let link = library.join("melee.gcm");
        symlink(&target, &link).unwrap();

        let trusted = TrustedRoots::from_paths([&library, &downloads]);
        let report = inspect_catalogued_game_identity_in_roots(&link, Some("GameCube"), &trusted);
        assert_eq!(report.verified_dolphin_game_id(), Some("GALE01"));
        assert!(report.bytes_read > 0);
    }

    #[test]
    fn identity_through_a_symlink_escaping_the_trusted_roots_is_refused() {
        use std::os::unix::fs::symlink;

        let directory = FixtureDir::new("wbfs-symlink-escape");
        let outside = directory.0.join("outside");
        let library = directory.0.join("library");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&library).unwrap();
        let target = outside.join("target.wbfs");
        fs::write(&target, wbfs_fixture(b"SMNE01", 0)).unwrap();
        let link = library.join("SMNE01.wbfs");
        symlink(&target, &link).unwrap();

        // Only the library is trusted, so the target is out of bounds.
        let trusted = TrustedRoots::from_paths([&library]);
        let report = inspect_catalogued_game_identity_in_roots(&link, Some("Wii"), &trusted);
        assert_eq!(report.verified_dolphin_game_id(), None);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.diagnostic.contains("symlink refused")),
            "the refusal must be stated: {:?}",
            report.evidence
        );
    }

    #[test]
    fn identity_through_a_broken_or_looping_symlink_is_refused() {
        use std::os::unix::fs::symlink;

        let directory = FixtureDir::new("wbfs-symlink-broken");
        let library = directory.0.join("library");
        fs::create_dir_all(&library).unwrap();
        let broken = library.join("broken.wbfs");
        symlink(library.join("absent.wbfs"), &broken).unwrap();
        let first = library.join("loop-a.wbfs");
        let second = library.join("loop-b.wbfs");
        symlink(&second, &first).unwrap();
        symlink(&first, &second).unwrap();

        let trusted = TrustedRoots::from_paths([&library]);
        for path in [broken, first] {
            let report = inspect_catalogued_game_identity_in_roots(&path, Some("Wii"), &trusted);
            assert_eq!(report.verified_dolphin_game_id(), None);
            assert!(
                report
                    .evidence
                    .iter()
                    .any(|item| item.diagnostic.contains("symlink refused")),
                "{} must be refused with a reason",
                path.display()
            );
        }
    }

    #[test]
    fn identity_through_a_trusted_symlink_never_writes_to_the_tree() {
        use std::os::unix::fs::symlink;

        let directory = FixtureDir::new("wbfs-symlink-read-only");
        let downloads = directory.0.join("downloads");
        let library = directory.0.join("library");
        fs::create_dir_all(&downloads).unwrap();
        fs::create_dir_all(&library).unwrap();
        let target = downloads.join("target.wbfs");
        fs::write(&target, wbfs_fixture(b"SMNE01", 0)).unwrap();
        let link = library.join("SMNE01.wbfs");
        symlink(&target, &link).unwrap();

        let snapshot = |root: &Path| {
            let mut entries = std::collections::BTreeMap::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(current) = stack.pop() {
                let Ok(read_dir) = fs::read_dir(&current) else {
                    continue;
                };
                for entry in read_dir.filter_map(Result::ok) {
                    let path = entry.path();
                    let Ok(metadata) = fs::symlink_metadata(&path) else {
                        continue;
                    };
                    if metadata.is_dir() && !metadata.file_type().is_symlink() {
                        stack.push(path.clone());
                    }
                    entries.insert(
                        path.to_string_lossy().into_owned(),
                        format!(
                            "{:?}|{}|{:?}",
                            metadata.file_type(),
                            metadata.len(),
                            metadata.modified().ok()
                        ),
                    );
                }
            }
            entries
        };
        let before = snapshot(&directory.0);
        let trusted = TrustedRoots::from_paths([&library, &downloads]);
        let report = inspect_catalogued_game_identity_in_roots(&link, Some("Wii"), &trusted);
        assert_eq!(report.verified_dolphin_game_id(), Some("SMNE01"));
        assert_eq!(
            snapshot(&directory.0),
            before,
            "reading identity through a link must not change anything"
        );
    }

    #[test]
    fn gcz_remains_honestly_deferred_not_unsupported() {
        let directory = FixtureDir::new("gcz");
        for extension in ["gcz", "chd", "cso"] {
            let path = write_fixture(&directory, &format!("disc.{extension}"), b"whatever");
            let report = inspect_game_identity(&path, Some("GameCube"));
            assert_eq!(report.format, IdentityImageFormat::Deferred);
            assert!(
                report
                    .evidence
                    .iter()
                    .any(|item| item.status == IdentityStatus::Deferred)
            );
        }
        let wbfs = write_fixture(&directory, "gamecube.wbfs", &wbfs_fixture(b"SMNE01", 0));
        let report = inspect_game_identity(&wbfs, Some("GameCube"));
        assert_eq!(report.format, IdentityImageFormat::Deferred);
        assert_eq!(report.verified_dolphin_game_id(), None);
    }

    #[test]
    fn ps2_iso_verifies_serial_and_exact_executable_crc() {
        let directory = FixtureDir::new("ps2");
        let bytes = ps2_iso(b"BOOT2 = cdrom0:\\SLUS_123.45;1\r\n", true, None);
        let path = write_fixture(&directory, "unrelated.iso", &bytes);
        let report = inspect_game_identity(&path, Some("PlayStation 2"));
        assert_eq!(
            report.verified_value(IdentityKind::Ps2Serial),
            Some("SLUS-12345")
        );
        let expected = format!(
            "{:08X}",
            pcsx2_executable_crc(&bytes[22 * 2048..22 * 2048 + 12])
        );
        assert_eq!(report.verified_pcsx2_crc(), Some(expected.as_str()));
        assert!(report.complete);
    }

    #[test]
    fn psp_iso_verifies_disc_id_from_umd_content() {
        let directory = FixtureDir::new("psp");
        let path = write_fixture(
            &directory,
            "not-the-disc-id – 日本語.iso",
            &psp_iso(b"ULUS10000", true),
        );
        let report = inspect_game_identity(&path, Some("PSP"));
        assert_eq!(report.platform, IdentityPlatform::Psp);
        assert_eq!(report.verified_psp_disc_id(), Some("ULUS10000"));
        let (status, facts) =
            crate::launch::evidence_bridge::canonical_identity_from_game_report(&report);
        assert!(matches!(
            status,
            crate::launch::planning::CanonicalIdentityStatus::Resolved(_)
        ));
        assert_eq!(
            facts,
            vec![
                crate::launch::input_projection::VerifiedIdentityFact::PspDiscId(
                    "ULUS10000".to_string()
                )
            ]
        );

        let wrong_platform = inspect_game_identity(&path, Some("PlayStation 2"));
        assert_eq!(wrong_platform.platform, IdentityPlatform::PlayStation2);
        assert_eq!(wrong_platform.verified_psp_disc_id(), None);
    }

    #[test]
    fn psp_identity_fails_closed_without_umd_or_for_malformed_sfo() {
        let directory = FixtureDir::new("psp-invalid");
        let missing_umd = write_fixture(&directory, "missing.iso", &psp_iso(b"ULUS10000", false));
        assert_eq!(
            inspect_game_identity(&missing_umd, Some("PSP")).verified_psp_disc_id(),
            None
        );
        let malformed = write_fixture(
            &directory,
            "malformed.iso",
            &psp_iso(b"not-a-disc-id", true),
        );
        assert_eq!(
            inspect_game_identity(&malformed, Some("PSP")).verified_psp_disc_id(),
            None
        );
        let filename_only = write_fixture(&directory, "ULUS10000.iso", &[0_u8; 32]);
        let report = inspect_game_identity(&filename_only, Some("PSP"));
        assert_eq!(report.verified_psp_disc_id(), None);
        assert!(
            report
                .evidence
                .iter()
                .all(|item| item.status != IdentityStatus::Verified)
        );
    }

    #[test]
    fn psp_pbp_verifies_disc_id_but_ps1_context_stays_without_serial() {
        let directory = FixtureDir::new("psp-pbp");
        let path = write_fixture(&directory, "EBOOT.pbp", &psp_pbp(b"ULUS10000"));
        let report = inspect_game_identity(&path, Some("PSP"));
        assert_eq!(report.format, IdentityImageFormat::Pbp);
        assert_eq!(report.verified_psp_disc_id(), Some("ULUS10000"));
        assert!(report.complete);

        let ps1 = inspect_game_identity(&path, Some("PS1"));
        assert_eq!(ps1.format, IdentityImageFormat::Pbp);
        assert_eq!(ps1.verified_ps1_serial(), None);
    }

    #[test]
    fn malformed_pbp_does_not_produce_psp_identity() {
        let directory = FixtureDir::new("psp-pbp-invalid");
        let path = write_fixture(&directory, "EBOOT.pbp", b"not a PBP");
        let report = inspect_game_identity(&path, Some("PSP"));
        assert_eq!(report.format, IdentityImageFormat::Pbp);
        assert_eq!(report.verified_psp_disc_id(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_iso_verifies_boot_serial_and_psx_executable() {
        let directory = FixtureDir::new("ps1");
        let path = write_fixture(
            &directory,
            "unrelated-name.iso",
            &ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true),
        );
        let report = inspect_game_identity(&path, Some("PSX"));

        assert_eq!(report.platform, IdentityPlatform::PlayStation);
        assert_eq!(report.verified_ps1_serial(), Some("SLUS-12345"));
        assert!(report.complete);
    }

    #[test]
    fn ps1_identity_fails_closed_without_system_cnf_or_psx_executable() {
        let directory = FixtureDir::new("ps1-fail-closed");
        let missing_cnf = write_fixture(
            &directory,
            "missing-cnf.iso",
            &ps1_iso(b"SLUS_123.45;1", b"", true),
        );
        let report = inspect_game_identity(&missing_cnf, Some("PlayStation"));
        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);

        let missing_executable = write_fixture(
            &directory,
            "missing-executable.iso",
            &ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\n", false),
        );
        let report = inspect_game_identity(&missing_executable, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_serial_families_are_normalized_and_unsupported_families_rejected() {
        let directory = FixtureDir::new("ps1-families");
        for (index, (family, expected)) in [
            ("SLUS", "SLUS-12345"),
            ("SCUS", "SCUS-12345"),
            ("SLES", "SLES-23456"),
            ("SCES", "SCES-23456"),
            ("SLPS", "SLPS-34567"),
            ("SLPM", "SLPM-34567"),
        ]
        .into_iter()
        .enumerate()
        {
            let code = if family == "SLUS" || family == "SCUS" {
                format!("{family}_123.45")
            } else if family == "SLES" || family == "SCES" {
                format!("{family}_234.56")
            } else {
                format!("{family}_345.67")
            };
            let path = write_fixture(
                &directory,
                &format!("family-{index}.iso"),
                &ps1_iso(
                    format!("{code};1").as_bytes(),
                    format!("BOOT=cdrom:\\{code};1\n").as_bytes(),
                    true,
                ),
            );
            let report = inspect_game_identity(&path, Some("PS1"));
            assert_eq!(report.verified_ps1_serial(), Some(expected));
        }

        let unsupported = write_fixture(
            &directory,
            "unsupported-family.iso",
            &ps1_iso(b"ABCD_123.45;1", b"BOOT=cdrom:\\ABCD_123.45;1\n", true),
        );
        let report = inspect_game_identity(&unsupported, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_malformed_boot_data_and_non_psx_executable_fail_closed() {
        let directory = FixtureDir::new("ps1-malformed");
        for (name, cnf) in [
            ("malformed.iso", b"BOOT cdrom:\\SLUS_123.45;1\n".as_slice()),
            ("missing-boot.iso", b"TCB=4\n".as_slice()),
            (
                "boot2-only.iso",
                b"BOOT2=cdrom0:\\SLUS_123.45;1\n".as_slice(),
            ),
            (
                "malformed-target.iso",
                b"BOOT=cdrom:\\..\\SLUS_123.45;1\n".as_slice(),
            ),
        ] {
            let path = write_fixture(&directory, name, &ps1_iso(b"SLUS_123.45;1", cnf, true));
            let report = inspect_game_identity(&path, Some("PS1"));
            assert_eq!(report.verified_ps1_serial(), None, "{name}");
            assert!(!report.complete, "{name}");
        }

        let mut wrong_executable =
            ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        wrong_executable[22 * ISO_SECTOR_SIZE as usize..22 * ISO_SECTOR_SIZE as usize + 8]
            .copy_from_slice(b"NOT-PSX!");
        let path = write_fixture(&directory, "wrong-executable.iso", &wrong_executable);
        let report = inspect_game_identity(&path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_bin_cue_identity_uses_the_data_track_not_the_filename() {
        let directory = FixtureDir::new("ps1-cue");
        let bin_path = directory.0.join("actual-disc.bin");
        fs::write(
            &bin_path,
            ps1_raw_bin(&ps1_iso(
                b"SLUS_123.45;1",
                b"BOOT=cdrom:\\SLUS_123.45;1\r\n",
                true,
            )),
        )
        .unwrap();
        let cue_path = write_fixture(
            &directory,
            "unrelated-title.cue",
            b"FILE \"actual-disc.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PSX"));
        assert_eq!(report.verified_ps1_serial(), Some("SLUS-12345"));
        assert!(report.complete);
    }

    #[test]
    fn saturn_iso_cue_and_chd_verify_the_system_id_product_number() {
        let directory = FixtureDir::new("saturn-identity");
        let iso = saturn_iso(b"T-7101G");
        let iso_path = write_fixture(&directory, "unrelated-name.iso", &iso);
        let report = inspect_game_identity(&iso_path, Some("Sega Saturn"));
        assert_eq!(report.platform, IdentityPlatform::Saturn);
        assert_eq!(report.verified_saturn_product_number(), Some("T-7101G"));
        assert!(report.complete);

        let bin_path = directory.0.join("actual-disc.bin");
        fs::write(&bin_path, ps1_raw_bin(&iso)).unwrap();
        let cue_path = write_fixture(
            &directory,
            "unrelated-title.cue",
            b"FILE \"actual-disc.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("Saturn"));
        assert_eq!(report.verified_saturn_product_number(), Some("T-7101G"));
        assert!(report.complete);

        let chd_path = write_fixture(&directory, "unrelated-title.chd", &ps1_chd(&iso));
        let report = inspect_game_identity(&chd_path, Some("Saturn"));
        assert_eq!(report.verified_saturn_product_number(), Some("T-7101G"));
        assert!(report.complete);
    }

    #[test]
    fn saturn_identity_fails_closed_for_wrong_signature_or_missing_product() {
        let directory = FixtureDir::new("saturn-invalid");
        let mut wrong = saturn_iso(b"T-7101G");
        wrong[..16].copy_from_slice(b"NOT A SATURN    ");
        let path = write_fixture(&directory, "wrong.iso", &wrong);
        let report = inspect_game_identity(&path, Some("Saturn"));
        assert_eq!(report.verified_saturn_product_number(), None);
        assert!(!report.complete);

        let path = write_fixture(&directory, "missing-product.iso", &saturn_iso(b""));
        let report = inspect_game_identity(&path, Some("Saturn"));
        assert_eq!(report.verified_saturn_product_number(), None);
        assert!(!report.complete);
    }

    #[test]
    fn dreamcast_iso_cue_and_chd_verify_ip_bin_product_code() {
        let directory = FixtureDir::new("dreamcast-identity");
        let iso = dreamcast_iso(b"T-8109N");
        let iso_path = write_fixture(&directory, "unrelated-name.iso", &iso);
        let report = inspect_game_identity(&iso_path, Some("Sega Dreamcast"));
        assert_eq!(report.platform, IdentityPlatform::Dreamcast);
        assert_eq!(report.verified_dreamcast_product_code(), Some("T-8109N"));
        assert!(report.complete);

        let bin_path = directory.0.join("actual-disc.bin");
        fs::write(&bin_path, ps1_raw_bin(&iso)).unwrap();
        let cue_path = write_fixture(
            &directory,
            "unrelated-title.cue",
            b"FILE \"actual-disc.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), Some("T-8109N"));

        let chd_path = write_fixture(&directory, "unrelated-title.chd", &ps1_chd(&iso));
        let report = inspect_game_identity(&chd_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), Some("T-8109N"));
    }

    #[test]
    fn sega_cd_iso_cue_and_chd_verify_disc_id_product_code() {
        let directory = FixtureDir::new("sega-cd-identity");
        let iso = sega_cd_iso(b"GM T-12345 -00");
        let iso_path = write_fixture(&directory, "filename-does-not-matter.iso", &iso);
        let report = inspect_game_identity(&iso_path, Some("Sega CD"));
        assert_eq!(report.platform, IdentityPlatform::SegaCd);
        assert_eq!(
            report.verified_sega_cd_product_code(),
            Some("GM T-12345-00")
        );
        assert!(report.complete);

        let bin_path = directory.0.join("content.bin");
        fs::write(&bin_path, ps1_raw_bin(&iso)).unwrap();
        let cue_path = write_fixture(
            &directory,
            "unrelated-title.cue",
            b"FILE \"content.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("Mega CD"));
        assert_eq!(
            report.verified_sega_cd_product_code(),
            Some("GM T-12345-00")
        );
        assert!(report.complete);

        let chd_path = write_fixture(&directory, "unrelated-title.chd", &ps1_chd(&iso));
        let report = inspect_game_identity(&chd_path, Some("Sega CD"));
        assert_eq!(
            report.verified_sega_cd_product_code(),
            Some("GM T-12345-00")
        );
        assert!(report.complete);
    }

    #[test]
    fn sega_cd_identity_rejects_wrong_signature_or_product_field() {
        let directory = FixtureDir::new("sega-cd-invalid");
        let mut wrong = sega_cd_iso(b"GM T-12345 -00");
        wrong[..SEGA_CD_BOOT_SIGNATURE.len()].copy_from_slice(b"NOTASEGACDSIGN");
        let path = write_fixture(&directory, "wrong.iso", &wrong);
        let report = inspect_game_identity(&path, Some("Sega CD"));
        assert_eq!(report.verified_sega_cd_product_code(), None);
        assert!(!report.complete);

        let path = write_fixture(
            &directory,
            "missing-product.iso",
            &sega_cd_iso(b"              "),
        );
        let report = inspect_game_identity(&path, Some("Sega CD"));
        assert_eq!(report.verified_sega_cd_product_code(), None);
        assert!(!report.complete);
    }

    #[test]
    fn dreamcast_identity_fails_closed_without_signature_or_product_code() {
        let directory = FixtureDir::new("dreamcast-invalid");
        let mut wrong = dreamcast_iso(b"T-8109N");
        wrong[..16].copy_from_slice(b"NOT A DREAMCAST ");
        let path = write_fixture(&directory, "wrong.iso", &wrong);
        let report = inspect_game_identity(&path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);

        let path = write_fixture(&directory, "missing-product.iso", &dreamcast_iso(b""));
        let report = inspect_game_identity(&path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    /// A normal 3-track Dreamcast GDI: a small low-density track (ignored),
    /// an audio track (ignored, never opened as identity evidence), and the
    /// real high-density data track at/after the documented GD-ROM
    /// boundary - selected by metadata alone.
    fn dreamcast_gdi_descriptor(data_filename: &str, sector_size: u32) -> String {
        format!(
            "3\n\
             1 0 4 2352 track01.bin 0\n\
             2 600 0 2352 track02.raw 0\n\
             3 45000 4 {sector_size} {data_filename} 0\n"
        )
    }

    #[test]
    fn dreamcast_gdi_raw_2352_data_track_verifies_product_code() {
        let directory = FixtureDir::new("dreamcast-gdi-raw");
        let iso = dreamcast_iso(b"T-8109N");
        fs::write(
            directory.0.join("track01.bin"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES],
        )
        .unwrap();
        fs::write(
            directory.0.join("track02.raw"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES],
        )
        .unwrap();
        fs::write(directory.0.join("game.bin"), ps1_raw_bin(&iso)).unwrap();
        let gdi_path = write_fixture(
            &directory,
            "unrelated-name.gdi",
            dreamcast_gdi_descriptor("game.bin", 2352).as_bytes(),
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.platform, IdentityPlatform::Dreamcast);
        assert_eq!(report.format, IdentityImageFormat::Gdi);
        assert_eq!(report.verified_dreamcast_product_code(), Some("T-8109N"));
        assert!(report.complete);
    }

    #[test]
    fn dreamcast_gdi_cooked_2048_data_track_verifies_product_code() {
        let directory = FixtureDir::new("dreamcast-gdi-cooked");
        let iso = dreamcast_iso(b"T-8109N");
        fs::write(
            directory.0.join("track01.bin"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES],
        )
        .unwrap();
        fs::write(
            directory.0.join("track02.raw"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES],
        )
        .unwrap();
        fs::write(directory.0.join("game.iso"), &iso).unwrap();
        let gdi_path = write_fixture(
            &directory,
            "game.gdi",
            dreamcast_gdi_descriptor("game.iso", 2048).as_bytes(),
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), Some("T-8109N"));
        assert!(report.complete);
    }

    #[test]
    fn dreamcast_gdi_filename_disagreement_is_irrelevant() {
        let directory = FixtureDir::new("dreamcast-gdi-filename-disagreement");
        let iso = dreamcast_iso(b"T-8109N");
        fs::write(
            directory.0.join("track01.bin"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES],
        )
        .unwrap();
        fs::write(
            directory.0.join("track02.raw"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES],
        )
        .unwrap();
        fs::write(
            directory.0.join("totally_unrelated_name.dat"),
            ps1_raw_bin(&iso),
        )
        .unwrap();
        let gdi_path = write_fixture(
            &directory,
            "Some Other Game Title.gdi",
            dreamcast_gdi_descriptor("totally_unrelated_name.dat", 2352).as_bytes(),
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), Some("T-8109N"));
    }

    #[test]
    fn dreamcast_gdi_malformed_descriptor_fails_closed() {
        let directory = FixtureDir::new("dreamcast-gdi-malformed");
        let gdi_path = write_fixture(&directory, "malformed.gdi", b"not-a-track-count\n");
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.format, IdentityImageFormat::Gdi);
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    #[test]
    fn dreamcast_gdi_missing_track_file_fails_closed() {
        let directory = FixtureDir::new("dreamcast-gdi-missing-track");
        let gdi_path = write_fixture(
            &directory,
            "missing.gdi",
            b"1\n1 45000 4 2352 does_not_exist.bin 0\n",
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    #[test]
    fn dreamcast_gdi_traversal_fails_closed() {
        let directory = FixtureDir::new("dreamcast-gdi-traversal");
        let gdi_path = write_fixture(
            &directory,
            "traversal.gdi",
            b"1\n1 45000 4 2352 ../outside.bin 0\n",
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    #[cfg(unix)]
    #[test]
    fn dreamcast_gdi_symlink_escape_fails_closed() {
        let directory = FixtureDir::new("dreamcast-gdi-symlink");
        let iso = dreamcast_iso(b"T-8109N");
        let outside = directory
            .0
            .parent()
            .unwrap()
            .join(format!("gdi-outside-{}", std::process::id()));
        fs::write(&outside, ps1_raw_bin(&iso)).unwrap();
        std::os::unix::fs::symlink(&outside, directory.0.join("escape.bin")).unwrap();
        let gdi_path = write_fixture(
            &directory,
            "symlink.gdi",
            b"1\n1 45000 4 2352 escape.bin 0\n",
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn dreamcast_gdi_duplicate_track_number_fails_closed() {
        let directory = FixtureDir::new("dreamcast-gdi-duplicate-number");
        fs::write(
            directory.0.join("a.bin"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES],
        )
        .unwrap();
        fs::write(
            directory.0.join("b.bin"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES * 2],
        )
        .unwrap();
        let gdi_path = write_fixture(
            &directory,
            "duplicate.gdi",
            b"2\n1 0 4 2352 a.bin 0\n1 45000 4 2352 b.bin 0\n",
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    #[test]
    fn dreamcast_gdi_ambiguous_data_tracks_fail_closed() {
        let directory = FixtureDir::new("dreamcast-gdi-ambiguous");
        fs::write(
            directory.0.join("a.bin"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES * 2],
        )
        .unwrap();
        fs::write(
            directory.0.join("b.bin"),
            vec![0_u8; crate::raw_cd_sector::RAW_SECTOR_BYTES * 2],
        )
        .unwrap();
        let gdi_path = write_fixture(
            &directory,
            "ambiguous.gdi",
            b"2\n1 45000 4 2352 a.bin 0\n2 50000 4 2352 b.bin 0\n",
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    #[test]
    fn dreamcast_gdi_unsupported_sector_size_fails_closed() {
        let directory = FixtureDir::new("dreamcast-gdi-bad-sector-size");
        fs::write(directory.0.join("a.bin"), vec![0_u8; 2336 * 2]).unwrap();
        let gdi_path = write_fixture(
            &directory,
            "bad-sector-size.gdi",
            b"1\n1 45000 4 2336 a.bin 0\n",
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    #[test]
    fn dreamcast_gdi_truncated_data_track_fails_closed() {
        let directory = FixtureDir::new("dreamcast-gdi-truncated");
        // A `.bin` claiming 2352-byte sectors but truncated mid-sector.
        fs::write(directory.0.join("game.bin"), vec![0_u8; 2352 + 100]).unwrap();
        let gdi_path = write_fixture(
            &directory,
            "truncated.gdi",
            b"1\n1 45000 4 2352 game.bin 0\n",
        );
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    #[test]
    fn dreamcast_gdi_non_dreamcast_platform_is_unsupported_not_verified() {
        let directory = FixtureDir::new("dreamcast-gdi-wrong-platform");
        let iso = dreamcast_iso(b"T-8109N");
        fs::write(directory.0.join("game.bin"), ps1_raw_bin(&iso)).unwrap();
        let gdi_path = write_fixture(&directory, "game.gdi", b"1\n1 45000 4 2352 game.bin 0\n");
        let report = inspect_game_identity(&gdi_path, Some("PlayStation"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert_ne!(report.format, IdentityImageFormat::Gdi);
    }

    #[test]
    fn dreamcast_gdi_non_dreamcast_content_does_not_verify() {
        let directory = FixtureDir::new("dreamcast-gdi-non-dreamcast-content");
        // Valid ISO9660 content but no Dreamcast IP.BIN signature at all.
        let mut not_dreamcast = vec![0_u8; 24 * ISO_SECTOR_SIZE as usize];
        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        not_dreamcast[pvd] = 1;
        not_dreamcast[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        not_dreamcast[pvd + 6] = 1;
        fs::write(directory.0.join("game.bin"), ps1_raw_bin(&not_dreamcast)).unwrap();
        let gdi_path = write_fixture(&directory, "game.gdi", b"1\n1 45000 4 2352 game.bin 0\n");
        let report = inspect_game_identity(&gdi_path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    /// A metadata-only synthetic CHD shaped like a real multi-track
    /// Dreamcast GD-ROM (mirrors the real Jet Set Radio / Mr. Driller
    /// layout `chd_identity.rs`'s own
    /// `real_world_shaped_gd_rom_needs_a_specialist_backend` test and
    /// `disc_evidence_collector`'s own local mirror both use): track 1
    /// (low-density, small), track 2 (audio), track 3 (high-density game
    /// data, past frame 45000). No real hunk/sector data is included -
    /// sufficient to drive the *routing* decision
    /// (`chd_needs_specialist_optical_backend`), not a real decode, which
    /// this crate's own `chd_optical_specialist` tests document as
    /// requiring genuine `chdman`-produced bytes this repo never commits.
    fn gdrom_chd_bytes() -> Vec<u8> {
        use crate::chd_identity::{CHD_METADATA_HEADER_BYTES, meta_tag};
        use crate::dat::archive::chd::CHD_MAGIC;

        fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
            bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
        }

        let mut data = vec![0u8; 124];
        data[0..8].copy_from_slice(CHD_MAGIC);
        put_u32(&mut data, 8, 124);
        put_u32(&mut data, 12, 5);
        put_u64(&mut data, 32, 0x1234_5678_0000_0000);
        put_u32(&mut data, 56, 0x0002_0000);
        put_u32(&mut data, 60, 0x0000_0800);

        let entries: [(u32, &[u8]); 3] = [
            (
                meta_tag::GDROM_TRACK,
                b"TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:6835 PAD:0 PREGAP:0 PGTYPE:NONE PGSUB:NONE POSTGAP:0",
            ),
            (
                meta_tag::GDROM_TRACK,
                b"TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:38165 PAD:0 PREGAP:150 PGTYPE:SILENCE PGSUB:NONE POSTGAP:0",
            ),
            (
                meta_tag::GDROM_TRACK,
                b"TRACK:3 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:504150 PAD:0 PREGAP:0 PGTYPE:NONE PGSUB:NONE POSTGAP:0",
            ),
        ];
        let meta_start = data.len() as u64;
        let mut offsets = Vec::with_capacity(entries.len());
        let mut cursor = meta_start;
        for (_, payload) in &entries {
            offsets.push(cursor);
            cursor += CHD_METADATA_HEADER_BYTES as u64 + payload.len() as u64;
        }
        for (index, (tag, payload)) in entries.iter().enumerate() {
            let next = offsets.get(index + 1).copied().unwrap_or(0);
            data.extend_from_slice(&tag.to_be_bytes());
            data.push(0);
            let length = payload.len() as u32;
            data.extend_from_slice(&length.to_be_bytes()[1..]);
            data.extend_from_slice(&next.to_be_bytes());
            data.extend_from_slice(payload);
        }
        put_u64(&mut data, 48, meta_start);
        data
    }

    #[test]
    fn dreamcast_multi_track_gdrom_chd_routes_to_the_specialist_backend() {
        let directory = FixtureDir::new("dreamcast-gdrom-chd-routing");
        let path = write_fixture(&directory, "game.chd", &gdrom_chd_bytes());
        let report = inspect_game_identity(&path, Some("Dreamcast"));
        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
        // Without the `chd-optical-specialist` feature this is the exact,
        // specific refusal reason - proves the *new* routing branch (not
        // the old generic `open_chd_iso9660` refusal) was taken.
        #[cfg(not(feature = "chd-optical-specialist"))]
        {
            let evidence = report
                .evidence
                .iter()
                .find(|item| item.kind == IdentityKind::DreamcastProductCode)
                .expect("a DreamcastProductCode evidence item must exist");
            assert!(
                evidence.diagnostic.contains("chd-optical-specialist"),
                "expected the specialist-backend-unavailable message, got: {}",
                evidence.diagnostic
            );
        }
    }

    #[cfg(feature = "chd-optical-specialist")]
    #[test]
    fn dreamcast_multi_track_gdrom_chd_with_the_feature_enabled_calls_the_real_specialist_backend()
    {
        // No genuine hunk/sector data exists in this metadata-only
        // fixture, so `libchdman-rs` itself refuses it - this proves the
        // *wiring* reaches the real backend (a distinct error path from
        // the feature-off "not compiled in" message above), not that a
        // full real GD-ROM decode succeeds - see `chd_optical_specialist`'s
        // own module doc for why that needs a genuine `chdman`-produced
        // file this repo never commits.
        let directory = FixtureDir::new("dreamcast-gdrom-chd-feature-enabled");
        let path = write_fixture(&directory, "game.chd", &gdrom_chd_bytes());
        let report = inspect_game_identity(&path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
        let evidence = report
            .evidence
            .iter()
            .find(|item| item.kind == IdentityKind::DreamcastProductCode)
            .expect("a DreamcastProductCode evidence item must exist");
        assert!(
            !evidence.diagnostic.contains("chd-optical-specialist"),
            "the feature-enabled path must not report the feature as unavailable, got: {}",
            evidence.diagnostic
        );
    }

    #[test]
    fn saturn_hinted_gdrom_shaped_chd_keeps_the_existing_generic_refusal() {
        // Saturn/PS1 semantics are untouched: the new routing branch is
        // gated to `IdentityPlatform::Dreamcast` only, so a non-Dreamcast
        // platform hint over the identical GD-ROM-shaped metadata still
        // takes the exact old `open_chd_iso9660` refusal path.
        let directory = FixtureDir::new("saturn-gdrom-shaped-chd");
        let path = write_fixture(&directory, "game.chd", &gdrom_chd_bytes());
        let report = inspect_game_identity(&path, Some("Saturn"));
        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert!(!report.complete);
        let evidence = report
            .evidence
            .iter()
            .find(|item| item.kind == IdentityKind::SaturnProductNumber);
        if let Some(evidence) = evidence {
            assert!(!evidence.diagnostic.contains("chd-optical-specialist"));
        }
    }

    #[test]
    fn existing_single_track_dreamcast_chd_is_unaffected_by_gdrom_routing() {
        // Regression: the plain single-track Dreamcast CHD path (already
        // covered by `dreamcast_iso_cue_and_chd_verify_ip_bin_product_code`
        // above) must still take the pure-Rust `open_chd_iso9660` path,
        // never the new specialist branch, since
        // `chd_needs_specialist_optical_backend` is `false` for it.
        let directory = FixtureDir::new("dreamcast-single-track-chd-unaffected");
        let iso = dreamcast_iso(b"T-8109N");
        let path = write_fixture(&directory, "unrelated-title.chd", &ps1_chd(&iso));
        let report = inspect_game_identity(&path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), Some("T-8109N"));
        assert!(report.complete);
    }

    /// One CDI test-fixture track spec:
    /// `(track_mode, read_mode, start_address, track_length, pregap)`.
    type CdiTrackSpec = (u32, u32, u32, u32, u32);

    /// Mirrors `crate::dreamcast_cdi`'s own private test-fixture builder
    /// exactly (private to that module's own test mod, so re-derived here
    /// per this crate's established per-file-fixture convention).
    /// `sessions[i]` is a list of `(track_mode, read_mode, start_address,
    /// track_length, pregap)`; only cooked (`read_mode == 0`) tracks are
    /// used here since these tests need real, correctly-placed IP.BIN
    /// content, not just structural parsing.
    fn cdi_bytes(sessions: &[Vec<CdiTrackSpec>]) -> Vec<u8> {
        fn push_track(
            desc: &mut Vec<u8>,
            track_mode: u32,
            read_mode: u32,
            start_address: u32,
            track_length: u32,
            pregap: u32,
        ) {
            desc.extend_from_slice(&[0u8; 16]);
            desc.push(0);
            desc.extend_from_slice(&[0u8; 29]);
            desc.extend_from_slice(&[0u8; 2]);
            desc.extend_from_slice(&2u16.to_le_bytes());
            desc.extend_from_slice(&pregap.to_le_bytes());
            desc.extend_from_slice(&(track_length - pregap).to_le_bytes());
            desc.extend_from_slice(&0u32.to_le_bytes());
            desc.extend_from_slice(&[0u8; 2]);
            desc.extend_from_slice(&track_mode.to_le_bytes());
            desc.extend_from_slice(&[0u8; 4]);
            desc.extend_from_slice(&0u32.to_le_bytes());
            desc.extend_from_slice(&0u32.to_le_bytes());
            desc.extend_from_slice(&start_address.to_le_bytes());
            desc.extend_from_slice(&track_length.to_le_bytes());
            desc.extend_from_slice(&[0u8; 16]);
            desc.extend_from_slice(&read_mode.to_le_bytes());
            desc.extend_from_slice(&0u32.to_le_bytes());
            desc.extend_from_slice(&[0u8; 9]);
            desc.extend_from_slice(&[0u8; 12]);
            desc.extend_from_slice(&0u32.to_le_bytes());
            desc.extend_from_slice(&[0u8; 99]);
        }
        let mut total_bytes = 0u64;
        for session in sessions {
            for &(_, _read_mode, _, length, _) in session {
                total_bytes += 2048u64 * u64::from(length); // cooked-only fixture
            }
        }
        let mut desc = Vec::new();
        desc.push(sessions.len() as u8);
        for session in sessions {
            desc.push(0);
            desc.push(session.len() as u8);
            desc.extend_from_slice(&[0u8; 13]);
            for &(track_mode, read_mode, start_address, length, pregap) in session {
                push_track(
                    &mut desc,
                    track_mode,
                    read_mode,
                    start_address,
                    length,
                    pregap,
                );
            }
        }
        desc.push(0);
        desc.push(0);
        desc.extend_from_slice(&[0u8; 13]);
        let dlen = (desc.len() + 4) as u32;
        let mut file = vec![0u8; total_bytes as usize];
        file.extend_from_slice(&desc);
        file.extend_from_slice(&dlen.to_le_bytes());
        file
    }

    /// Stamps a recognised Dreamcast IP.BIN hardware signature plus a
    /// non-copyrightable synthetic product code at the start of a cooked
    /// data region, matching `dreamcast_cdi`'s own test helper.
    #[cfg(feature = "dreamcast-cdi")]
    fn stamp_cdi_ip_bin(data: &mut [u8], product_code: &[u8; 10]) {
        data[0..16].copy_from_slice(b"SEGA SEGAKATANA ");
        data[0x40..0x4A].copy_from_slice(product_code);
    }

    #[test]
    #[cfg(feature = "dreamcast-cdi")]
    fn dreamcast_single_session_cdi_verifies_product_code_and_reaches_the_evidence_bridge() {
        let directory = FixtureDir::new("dreamcast-cdi-single-session");
        let mut bytes = cdi_bytes(&[vec![(1, 0, 0, 4, 0)]]);
        stamp_cdi_ip_bin(&mut bytes[..4 * 2048], b"TEST00001 ");
        let path = write_fixture(&directory, "game.cdi", &bytes);
        let report = inspect_game_identity(&path, Some("Dreamcast"));
        assert_eq!(report.format, IdentityImageFormat::Cdi);
        assert_eq!(report.verified_dreamcast_product_code(), Some("TEST00001"));
        assert!(report.complete);

        // The existing format-agnostic evidence bridge and Flycast input
        // projection reuse this verified fact unchanged.
        let (_status, facts) =
            crate::launch::evidence_bridge::canonical_identity_from_game_report(&report);
        assert!(
            facts.iter().any(|fact| matches!(
                fact,
                crate::launch::input_projection::VerifiedIdentityFact::DreamcastProductCode(code)
                    if code == "TEST00001"
            )),
            "expected a DreamcastProductCode(\"TEST00001\") fact, got: {facts:?}"
        );
    }

    #[test]
    #[cfg(feature = "dreamcast-cdi")]
    fn dreamcast_multi_session_gdrom_cdi_selects_the_high_density_track() {
        let directory = FixtureDir::new("dreamcast-cdi-multi-session");
        let mut bytes = cdi_bytes(&[vec![(1, 0, 0, 4, 0)], vec![(1, 0, 45000, 4, 0)]]);
        stamp_cdi_ip_bin(&mut bytes[..4 * 2048], b"WRONGCODE0");
        stamp_cdi_ip_bin(&mut bytes[4 * 2048..8 * 2048], b"TEST00002 ");
        let path = write_fixture(&directory, "game.cdi", &bytes);
        let report = inspect_game_identity(&path, Some("Dreamcast"));
        assert_eq!(report.verified_dreamcast_product_code(), Some("TEST00002"));
    }

    #[test]
    fn dreamcast_cdi_extension_alone_is_insufficient() {
        let directory = FixtureDir::new("dreamcast-cdi-extension-alone");
        let path = write_fixture(&directory, "game.cdi", b"not a real discjuggler image");
        let report = inspect_game_identity(&path, Some("Dreamcast"));
        assert_eq!(report.format, IdentityImageFormat::Cdi);
        assert_eq!(report.verified_dreamcast_product_code(), None);
        assert!(!report.complete);
    }

    #[test]
    fn non_dreamcast_cdi_fails_closed() {
        let directory = FixtureDir::new("non-dreamcast-cdi");
        let bytes = cdi_bytes(&[vec![(1, 0, 0, 4, 0)]]);
        let path = write_fixture(&directory, "game.cdi", &bytes);
        let report = inspect_game_identity(&path, Some("PlayStation"));
        assert_ne!(report.format, IdentityImageFormat::Cdi);
        assert!(!report.complete);
    }

    #[test]
    #[cfg(not(feature = "dreamcast-cdi"))]
    fn dreamcast_cdi_without_the_specialist_feature_fails_closed_with_a_specific_message() {
        let directory = FixtureDir::new("dreamcast-cdi-feature-off");
        let bytes = cdi_bytes(&[vec![(1, 0, 0, 4, 0)]]);
        let path = write_fixture(&directory, "game.cdi", &bytes);
        let report = inspect_game_identity(&path, Some("Dreamcast"));
        assert_eq!(report.format, IdentityImageFormat::Cdi);
        assert_eq!(report.verified_dreamcast_product_code(), None);
        let evidence = report
            .evidence
            .iter()
            .find(|item| item.kind == IdentityKind::DreamcastProductCode)
            .expect("a DreamcastProductCode evidence item must exist");
        assert!(
            evidence.diagnostic.contains("dreamcast-cdi"),
            "expected the specialist-backend-unavailable message, got: {}",
            evidence.diagnostic
        );
    }

    #[test]
    fn ps1_mode1_2048_cue_identity_is_supported() {
        let directory = FixtureDir::new("ps1-cue-2048");
        let bin_path = directory.0.join("data.bin");
        fs::write(
            &bin_path,
            ps1_iso(b"SLES_234.56;1", b"BOOT=cdrom:\\SLES_234.56;1\r\n", true),
        )
        .unwrap();
        let cue_path = write_fixture(
            &directory,
            "game.cue",
            b"FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PlayStation"));
        assert_eq!(report.verified_ps1_serial(), Some("SLES-23456"));
    }

    #[test]
    fn ps1_multi_bin_cue_selects_the_unambiguous_data_track() {
        let directory = FixtureDir::new("ps1-cue-multi-bin");
        fs::write(directory.0.join("audio.bin"), vec![0_u8; 2352 * 2]).unwrap();
        fs::write(
            directory.0.join("data.bin"),
            ps1_raw_bin(&ps1_iso(
                b"SLPS_345.67;1",
                b"BOOT=cdrom:\\SLPS_345.67;1\r\n",
                true,
            )),
        )
        .unwrap();
        let cue_path = write_fixture(
            &directory,
            "multi.cue",
            b"FILE \"audio.bin\" BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE \"data.bin\" BINARY\nTRACK 02 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), Some("SLPS-34567"));
    }

    #[test]
    fn ps1_cue_track_order_data_before_audio_is_also_unambiguous() {
        // The mirror of `ps1_multi_bin_cue_selects_the_unambiguous_data_track`
        // with the data track declared first - track declaration order must
        // never affect which file is selected.
        let directory = FixtureDir::new("ps1-cue-data-first");
        fs::write(
            directory.0.join("data.bin"),
            ps1_raw_bin(&ps1_iso(
                b"SLPS_456.78;1",
                b"BOOT=cdrom:\\SLPS_456.78;1\r\n",
                true,
            )),
        )
        .unwrap();
        fs::write(directory.0.join("audio.bin"), vec![0_u8; 2352 * 2]).unwrap();
        let cue_path = write_fixture(
            &directory,
            "multi.cue",
            b"FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), Some("SLPS-45678"));
    }

    #[test]
    fn ps1_cue_reports_iso_format_and_names_the_actual_bin_in_provenance() {
        // Requirement: verified identity must remain tied to the physical
        // release actually inspected - the report's own `format` must not
        // stay `Unsupported` for a CUE that verified cleanly, and every
        // piece of evidence's provenance must name the resolved data-track
        // file (`disc.bin`), never leave that implicit or point only at
        // the `.cue` itself.
        let directory = FixtureDir::new("ps1-cue-provenance");
        fs::write(
            directory.0.join("disc.bin"),
            ps1_raw_bin(&ps1_iso(
                b"SLUS_555.55;1",
                b"BOOT=cdrom:\\SLUS_555.55;1\r\n",
                true,
            )),
        )
        .unwrap();
        let cue_path = write_fixture(
            &directory,
            "game.cue",
            b"FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), Some("SLUS-55555"));
        assert_eq!(report.format, IdentityImageFormat::Iso);
        let serial_evidence = report
            .evidence
            .iter()
            .find(|item| {
                item.kind == IdentityKind::Ps1Serial && item.status == IdentityStatus::Verified
            })
            .expect("a Verified Ps1Serial evidence item must exist");
        assert_eq!(
            serial_evidence
                .provenance
                .member_path
                .as_deref()
                .map(String::from_utf8_lossy),
            Some(std::borrow::Cow::Borrowed("disc.bin"))
        );
    }

    #[test]
    fn ps1_cue_quoted_filename_with_spaces_verifies() {
        let directory = FixtureDir::new("ps1-cue-spaces");
        fs::write(
            directory.0.join("my disc image.bin"),
            ps1_raw_bin(&ps1_iso(
                b"SCUS_111.11;1",
                b"BOOT=cdrom:\\SCUS_111.11;1\r\n",
                true,
            )),
        )
        .unwrap();
        let cue_path = write_fixture(
            &directory,
            "game.cue",
            b"FILE \"my disc image.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), Some("SCUS-11111"));
    }

    #[test]
    fn ps1_cue_unicode_bin_path_verifies() {
        let directory = FixtureDir::new("ps1-cue-unicode");
        let bin_name = "ゲームディスク.bin";
        fs::write(
            directory.0.join(bin_name),
            ps1_raw_bin(&ps1_iso(
                b"SLES_222.22;1",
                b"BOOT=cdrom:\\SLES_222.22;1\r\n",
                true,
            )),
        )
        .unwrap();
        let cue_path = write_fixture(
            &directory,
            "game.cue",
            format!("FILE \"{bin_name}\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n")
                .as_bytes(),
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), Some("SLES-22222"));
    }

    #[test]
    fn ps1_cue_relative_subdirectory_reference_verifies() {
        let directory = FixtureDir::new("ps1-cue-subdir");
        let tracks_dir = directory.0.join("tracks");
        fs::create_dir(&tracks_dir).unwrap();
        fs::write(
            tracks_dir.join("data.bin"),
            ps1_raw_bin(&ps1_iso(
                b"SLPM_333.33;1",
                b"BOOT=cdrom:\\SLPM_333.33;1\r\n",
                true,
            )),
        )
        .unwrap();
        let cue_path = write_fixture(
            &directory,
            "game.cue",
            b"FILE \"tracks/data.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), Some("SLPM-33333"));
    }

    #[test]
    fn ps1_cue_missing_referenced_bin_fails_closed() {
        let directory = FixtureDir::new("ps1-cue-missing-bin");
        let cue_path = write_fixture(
            &directory,
            "game.cue",
            b"FILE \"does-not-exist.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_cue_referencing_a_non_disc_bin_fails_closed() {
        // The referenced file exists (so this is not the missing-file
        // case), but it is not a PS1 disc at all - a "wrong/mismatched"
        // BIN must never be reported Verified merely because the CUE
        // successfully pointed at *some* real file.
        let directory = FixtureDir::new("ps1-cue-wrong-bin");
        fs::write(directory.0.join("not-a-disc.bin"), vec![0x55_u8; 2352 * 4]).unwrap();
        let cue_path = write_fixture(
            &directory,
            "game.cue",
            b"FILE \"not-a-disc.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_cue_ambiguous_data_tracks_fail_closed() {
        let directory = FixtureDir::new("ps1-cue-ambiguous");
        fs::write(
            directory.0.join("one.bin"),
            ps1_raw_bin(&ps1_iso(
                b"SLUS_777.77;1",
                b"BOOT=cdrom:\\SLUS_777.77;1\r\n",
                true,
            )),
        )
        .unwrap();
        fs::write(
            directory.0.join("two.bin"),
            ps1_raw_bin(&ps1_iso(
                b"SLUS_888.88;1",
                b"BOOT=cdrom:\\SLUS_888.88;1\r\n",
                true,
            )),
        )
        .unwrap();
        let cue_path = write_fixture(
            &directory,
            "game.cue",
            b"FILE \"one.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nFILE \"two.bin\" BINARY\nTRACK 02 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.diagnostic.contains("data track could not be resolved")),
            "{:?}",
            report.evidence
        );
    }

    #[test]
    fn ps1_cue_traversal_reference_fails_closed() {
        let directory = FixtureDir::new("ps1-cue-traversal");
        let outside = std::env::temp_dir().join(format!(
            "archivefs-game-identity-outside-{}",
            std::process::id()
        ));
        fs::write(
            &outside,
            ps1_raw_bin(&ps1_iso(
                b"SLUS_999.99;1",
                b"BOOT=cdrom:\\SLUS_999.99;1\r\n",
                true,
            )),
        )
        .unwrap();
        let cue_path = write_fixture(
            &directory,
            "game.cue",
            format!(
                "FILE \"../{}\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
                outside.file_name().unwrap().to_str().unwrap()
            )
            .as_bytes(),
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        let _ = fs::remove_file(&outside);
        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_cue_filename_alone_never_authorizes_identity() {
        // The BIN's own filename looks exactly like a real PS1 serial
        // shape, but its content is not a PS1 disc at all - the filename
        // must never substitute for verified content.
        let directory = FixtureDir::new("ps1-cue-filename-trap");
        fs::write(directory.0.join("SLUS-99999.bin"), vec![0xAA_u8; 2352 * 4]).unwrap();
        let cue_path = write_fixture(
            &directory,
            "SLUS-99999.cue",
            b"FILE \"SLUS-99999.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        );
        let report = inspect_game_identity(&cue_path, Some("PS1"));
        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_chd_is_no_longer_categorically_deferred_but_a_missing_file_still_fails_closed() {
        // A nonexistent path can no longer be *guessed* Verified just
        // because the extension is `.chd` - it must fail closed on the
        // concrete "file not readable" reason, never on the old blanket
        // "format has no existing safe bounded reader" deferral.
        let chd = inspect_game_identity(Path::new("/games/does-not-exist.chd"), Some("PS1"));
        assert_eq!(chd.format, IdentityImageFormat::Chd);
        assert_eq!(chd.verified_ps1_serial(), None);
        assert!(
            chd.evidence.iter().any(|item| {
                item.kind == IdentityKind::Ps1Serial && item.status != IdentityStatus::Verified
            }),
            "a missing CHD must never be silently reported Verified"
        );
    }

    #[test]
    fn chd_for_a_non_optical_platform_hint_still_defers() {
        // Format/platform guarding: a `.chd` is only ever authoritatively
        // inspected for the specific optical platforms whose backend can
        // decode it. A platform outside that set (here GameCube, whose CHD
        // support is honestly deferred) must still fall through to Deferred
        // rather than be guessed from the `.chd` extension.
        let chd = inspect_game_identity(Path::new("/games/game.chd"), Some("GameCube"));
        assert_eq!(chd.format, IdentityImageFormat::Deferred);
    }

    #[test]
    fn ps2_chd_is_no_longer_categorically_deferred_but_a_missing_file_still_fails_closed() {
        // A `.chd` with a PS2 platform hint is now inspected through the same
        // bounded CHD backend as PS1, so it must fail closed on the concrete
        // "file not readable" reason - never the old blanket deferral, and
        // never a guessed Verified.
        let chd = inspect_game_identity(Path::new("/games/does-not-exist.chd"), Some("PS2"));
        assert_eq!(chd.format, IdentityImageFormat::Chd);
        assert_eq!(chd.verified_ps2_serial(), None);
        assert!(!chd.complete);
        assert!(
            chd.evidence.iter().any(|item| {
                item.kind == IdentityKind::Ps2Serial && item.status != IdentityStatus::Verified
            }),
            "a missing CHD must never be silently reported Verified"
        );
    }

    #[test]
    fn valid_ps1_chd_produces_a_verified_serial_matching_iso_authority() {
        let directory = FixtureDir::new("ps1-chd-valid");
        let image = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let path = write_fixture(&directory, "unrelated-name.chd", &ps1_chd(&image));

        let report = inspect_game_identity(&path, Some("PS1"));

        assert_eq!(report.platform, IdentityPlatform::PlayStation);
        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert_eq!(report.verified_ps1_serial(), Some("SLUS-12345"));
        assert!(report.complete);
    }

    #[test]
    fn ps1_chd_serial_shapes_remain_valid_across_regions() {
        for (serial_name, cnf, expected) in [
            (
                b"SLUS_123.45;1".as_slice(),
                b"BOOT=cdrom:\\SLUS_123.45;1\r\n".as_slice(),
                "SLUS-12345",
            ),
            (
                b"SLES_123.45;1".as_slice(),
                b"BOOT=cdrom:\\SLES_123.45;1\r\n".as_slice(),
                "SLES-12345",
            ),
            (
                b"SLPS_123.45;1".as_slice(),
                b"BOOT=cdrom:\\SLPS_123.45;1\r\n".as_slice(),
                "SLPS-12345",
            ),
        ] {
            let directory = FixtureDir::new("ps1-chd-regions");
            let image = ps1_iso(serial_name, cnf, true);
            let path = write_fixture(&directory, "game.chd", &ps1_chd(&image));

            let report = inspect_game_identity(&path, Some("PS1"));

            assert_eq!(report.verified_ps1_serial(), Some(expected), "{expected}");
        }
    }

    #[test]
    fn ps1_chd_verification_ignores_filename() {
        let directory = FixtureDir::new("ps1-chd-filename-disagreement");
        let image = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let path = write_fixture(&directory, "Totally Unrelated Title.chd", &ps1_chd(&image));

        let report = inspect_game_identity(&path, Some("PS1"));

        assert_eq!(report.verified_ps1_serial(), Some("SLUS-12345"));
    }

    #[test]
    fn ps1_chd_with_missing_system_cnf_fails_closed() {
        let directory = FixtureDir::new("ps1-chd-no-cnf");
        // No SYSTEM.CNF directory record at all - only the root directory.
        let mut image = vec![0u8; 24 * ISO_SECTOR_SIZE as usize];
        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        image[pvd] = 1;
        image[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        image[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        image[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        image[terminator] = 255;
        image[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        image[terminator + 6] = 1;
        let path = write_fixture(&directory, "no-cnf.chd", &ps1_chd(&image));

        let report = inspect_game_identity(&path, Some("PS1"));

        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert_eq!(report.verified_ps1_serial(), None);
    }

    #[test]
    fn ps1_chd_with_malformed_boot_line_fails_closed() {
        let directory = FixtureDir::new("ps1-chd-malformed-boot");
        let image = ps1_iso(b"SLUS_123.45;1", b"NOT-A-BOOT-LINE\r\n", true);
        let path = write_fixture(&directory, "game.chd", &ps1_chd(&image));

        let report = inspect_game_identity(&path, Some("PS1"));

        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_chd_with_invalid_psx_executable_header_fails_closed() {
        let directory = FixtureDir::new("ps1-chd-bad-exe");
        let mut image = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let executable_offset = 22 * ISO_SECTOR_SIZE as usize;
        image[executable_offset..executable_offset + 8].copy_from_slice(b"NOT-PSX!");
        let path = write_fixture(&directory, "game.chd", &ps1_chd(&image));

        let report = inspect_game_identity(&path, Some("PS1"));

        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps1_chd_with_unsupported_serial_prefix_fails_closed() {
        let directory = FixtureDir::new("ps1-chd-unsupported-prefix");
        let image = ps1_iso(b"ABCD_123.45;1", b"BOOT=cdrom:\\ABCD_123.45;1\r\n", true);
        let path = write_fixture(&directory, "game.chd", &ps1_chd(&image));

        let report = inspect_game_identity(&path, Some("PS1"));

        assert_eq!(report.verified_ps1_serial(), None);
    }

    #[test]
    fn ps1_chd_with_non_iso9660_content_does_not_become_verified_ps1() {
        let directory = FixtureDir::new("ps1-chd-non-iso9660");
        // Valid CHD wrapping content that is not ISO9660 at all - identity
        // must not fabricate PS1 evidence from unreadable disc content.
        let image = vec![0xAB_u8; 24 * ISO_SECTOR_SIZE as usize];
        let path = write_fixture(&directory, "game.chd", &ps1_chd(&image));

        let report = inspect_game_identity(&path, Some("PS1"));

        assert_eq!(report.verified_ps1_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn evidence_bridge_emits_ps1_serial_for_verified_chd_identity() {
        let directory = FixtureDir::new("ps1-chd-evidence-bridge");
        let image = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let path = write_fixture(&directory, "game.chd", &ps1_chd(&image));
        let report = inspect_game_identity(&path, Some("PSX"));

        let (status, facts) =
            crate::launch::evidence_bridge::canonical_identity_from_game_report(&report);

        assert!(matches!(
            status,
            crate::launch::planning::CanonicalIdentityStatus::Resolved(_)
        ));
        assert!(facts.iter().any(|fact| matches!(
            fact,
            crate::launch::input_projection::VerifiedIdentityFact::Ps1Serial(serial)
                if serial == "SLUS-12345"
        )));
    }

    // --- PlayStation 2 CHD (reuses the exact PS2 ISO evidence pipeline) ---

    /// A valid PS2 ISO 9660 image (from [`ps2_iso`]) wrapped verbatim into a
    /// genuine uncompressed single-track CHD v5 file. [`ps1_chd`] is the
    /// shared ISO->CHD wrapper - it only completes the one PVD
    /// logical-block-size field the plain-ISO fixtures leave zeroed, which
    /// `open_chd_iso9660` (correctly, for a real disc) requires.
    fn ps2_chd(image: &[u8]) -> Vec<u8> {
        ps1_chd(image)
    }

    #[test]
    fn valid_ps2_chd_resolves_playstation2_and_matches_iso_serial_authority() {
        let directory = FixtureDir::new("ps2-chd-valid");
        let iso = ps2_iso(b"BOOT2 = cdrom0:\\SLUS_123.45;1\r\n", true, None);
        let path = write_fixture(&directory, "unrelated-name.chd", &ps2_chd(&iso));

        let report = inspect_game_identity(&path, Some("PlayStation 2"));

        assert_eq!(report.platform, IdentityPlatform::PlayStation2);
        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert_eq!(report.verified_ps2_serial(), Some("SLUS-12345"));
        assert!(report.complete);
    }

    #[test]
    fn valid_ps2_chd_yields_the_same_pcsx2_executable_crc_as_the_equivalent_iso() {
        let directory = FixtureDir::new("ps2-chd-crc");
        let iso = ps2_iso(b"BOOT2 = cdrom0:\\SLUS_123.45;1\r\n", true, None);
        let expected = format!(
            "{:08X}",
            pcsx2_executable_crc(&iso[22 * 2048..22 * 2048 + 12])
        );

        let iso_path = write_fixture(&directory, "disc.iso", &iso);
        let iso_report = inspect_game_identity(&iso_path, Some("PS2"));
        assert_eq!(iso_report.verified_pcsx2_crc(), Some(expected.as_str()));

        let chd_path = write_fixture(&directory, "disc.chd", &ps2_chd(&iso));
        let chd_report = inspect_game_identity(&chd_path, Some("PS2"));
        assert_eq!(chd_report.verified_pcsx2_crc(), Some(expected.as_str()));
        assert_eq!(chd_report.verified_ps2_serial(), Some("SLUS-12345"));
        assert!(chd_report.complete);
    }

    #[test]
    fn ps2_chd_verification_ignores_filename_and_folder() {
        let directory = FixtureDir::new("ps2-chd-filename-disagreement");
        let iso = ps2_iso(b"BOOT2 = cdrom0:\\SLES_555.55;1\r\n", true, None);
        let path = write_fixture(&directory, "Totally Unrelated Title.chd", &ps2_chd(&iso));

        let report = inspect_game_identity(&path, Some("PS2"));

        assert_eq!(report.verified_ps2_serial(), Some("SLES-55555"));
    }

    #[test]
    fn filename_only_ps2_chd_never_creates_ps2_identity() {
        let directory = FixtureDir::new("ps2-chd-filename-only");
        // Not a CHD container at all; the extension and a PS2-looking name
        // must never manufacture identity.
        let path = write_fixture(&directory, "SLUS_123.45.chd", b"not a chd at all");

        let report = inspect_game_identity(&path, Some("PS2"));

        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn malformed_or_truncated_ps2_chd_fails_closed() {
        let directory = FixtureDir::new("ps2-chd-malformed");

        // Not a CHD container.
        let garbage = write_fixture(&directory, "garbage.chd", &[0xAB_u8; 4096]);
        let report = inspect_game_identity(&garbage, Some("PS2"));
        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);

        // A real CHD truncated inside its own header.
        let iso = ps2_iso(b"BOOT2 = cdrom0:\\SLUS_123.45;1\r\n", true, None);
        let mut truncated = ps2_chd(&iso);
        truncated.truncate(48);
        let truncated_path = write_fixture(&directory, "truncated.chd", &truncated);
        let report = inspect_game_identity(&truncated_path, Some("PS2"));
        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);

        // A genuine CHD whose decoded track is not ISO 9660 at all -
        // `open_chd_iso9660` refuses it rather than guessing PS2.
        let non_iso = write_fixture(
            &directory,
            "raw-track.chd",
            &uncompressed_mode1_raw_chd(&vec![0xCD_u8; 24 * ISO_SECTOR_SIZE as usize]),
        );
        let report = inspect_game_identity(&non_iso, Some("PS2"));
        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps2_chd_with_unsupported_track_position_or_pregap_stays_unresolved() {
        // These are enforced by the shared `open_chd_iso9660` backend - the
        // same single-track, track-1-only, zero-pregap, data-track-only
        // contract PS1 / PC-FX / Sega CHDs already go through.
        let directory = FixtureDir::new("ps2-chd-track-limits");
        let iso = ps2_iso(b"BOOT2 = cdrom0:\\SLUS_123.45;1\r\n", true, None);

        let mut non_track_one = ps2_chd(&iso);
        replace_once_in_place(&mut non_track_one, b"TRACK:1 ", b"TRACK:2 ");
        let path = write_fixture(&directory, "track2.chd", &non_track_one);
        let report = inspect_game_identity(&path, Some("PS2"));
        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);

        let mut non_zero_pregap = ps2_chd(&iso);
        replace_once_in_place(&mut non_zero_pregap, b"PREGAP:0 ", b"PREGAP:9 ");
        let path = write_fixture(&directory, "pregap.chd", &non_zero_pregap);
        let report = inspect_game_identity(&path, Some("PS2"));
        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps2_chd_with_iso9660_but_no_system_cnf_does_not_become_verified_ps2() {
        let directory = FixtureDir::new("ps2-chd-no-cnf");
        // Valid ISO 9660: PVD + terminator + root directory record, but no
        // SYSTEM.CNF entry anywhere.
        let mut image = vec![0u8; 24 * ISO_SECTOR_SIZE as usize];
        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        image[pvd] = 1;
        image[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        image[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        image[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        image[terminator] = 255;
        image[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        image[terminator + 6] = 1;
        let path = write_fixture(&directory, "no-cnf.chd", &ps2_chd(&image));

        let report = inspect_game_identity(&path, Some("PS2"));

        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn ps2_chd_with_malformed_boot2_does_not_create_ps2_serial() {
        let directory = FixtureDir::new("ps2-chd-malformed-boot2");
        let iso = ps2_iso(b"NOT-A-BOOT2-ASSIGNMENT\r\n", true, None);
        let path = write_fixture(&directory, "bad-boot2.chd", &ps2_chd(&iso));

        let report = inspect_game_identity(&path, Some("PS2"));

        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn generic_chd_with_a_ps2_hint_remains_unresolved_not_deferred() {
        let directory = FixtureDir::new("ps2-chd-generic");
        let image = vec![0x5A_u8; 24 * ISO_SECTOR_SIZE as usize];
        let path = write_fixture(&directory, "unknown.chd", &ps1_chd(&image));

        let report = inspect_game_identity(&path, Some("PS2"));

        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn adding_ps2_chd_dispatch_leaves_neighbouring_optical_chd_families_unchanged() {
        let directory = FixtureDir::new("ps2-chd-neighbours");

        let ps1 = write_fixture(
            &directory,
            "ps1.chd",
            &ps1_chd(&ps1_iso(
                b"SLUS_123.45;1",
                b"BOOT=cdrom:\\SLUS_123.45;1\r\n",
                true,
            )),
        );
        assert_eq!(
            inspect_game_identity(&ps1, Some("PS1")).verified_ps1_serial(),
            Some("SLUS-12345")
        );

        let saturn = write_fixture(&directory, "saturn.chd", &ps1_chd(&saturn_iso(b"T-7101G")));
        assert_eq!(
            inspect_game_identity(&saturn, Some("Saturn")).verified_saturn_product_number(),
            Some("T-7101G")
        );

        let dreamcast = write_fixture(&directory, "dc.chd", &ps1_chd(&dreamcast_iso(b"T-8109N")));
        assert_eq!(
            inspect_game_identity(&dreamcast, Some("Dreamcast")).verified_dreamcast_product_code(),
            Some("T-8109N")
        );

        // A PS2 CHD hint over Saturn disc content must not become PS2.
        let wrong_family = write_fixture(
            &directory,
            "sat-as-ps2.chd",
            &ps1_chd(&saturn_iso(b"T-7101G")),
        );
        let report = inspect_game_identity(&wrong_family, Some("PS2"));
        assert_eq!(report.verified_ps2_serial(), None);
        assert!(!report.complete);
    }

    #[test]
    fn missing_boot_executable_is_reported_without_crc() {
        let directory = FixtureDir::new("missing-elf");
        let path = write_fixture(
            &directory,
            "missing.iso",
            &ps2_iso(b"BOOT2=cdrom0:\\SLUS_123.45;1\n", false, None),
        );
        let report = inspect_game_identity(&path, Some("PS2"));
        assert_eq!(
            report.verified_value(IdentityKind::Ps2Serial),
            Some("SLUS-12345")
        );
        assert_eq!(report.verified_pcsx2_crc(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::Pcsx2ExecutableCrc && item.status == IdentityStatus::Missing
        }));
    }

    #[test]
    fn oversized_system_cnf_stops_at_the_declared_bound() {
        let directory = FixtureDir::new("large-cnf");
        let path = write_fixture(
            &directory,
            "large.iso",
            &ps2_iso(
                b"BOOT2=cdrom0:\\SLUS_123.45;1\n",
                true,
                Some(MAX_SYSTEM_CNF_BYTES as u32 + 1),
            ),
        );
        let report = inspect_game_identity(&path, Some("PS2"));
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::Ps2Serial
                && item.status == IdentityStatus::ResourceLimitReached
        }));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_refused_and_non_utf8_archive_path_is_preserved() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;

        let directory = FixtureDir::new("path-safety");
        let mut name = Vec::from(&b"game-"[..]);
        name.push(0xff);
        name.extend_from_slice(b".iso");
        let path = directory.0.join(std::ffi::OsString::from_vec(name));
        fs::write(
            &path,
            dolphin_fixture(IdentityPlatform::GameCube, b"GM8E01", 0),
        )
        .unwrap();
        let report = inspect_game_identity(&path, Some("GameCube"));
        assert_eq!(report.archive_path, path);
        assert_eq!(report.verified_dolphin_game_id(), Some("GM8E01"));

        let link = directory.0.join("link.iso");
        symlink(&path, &link).unwrap();
        let refused = inspect_game_identity(&link, Some("GameCube"));
        assert_eq!(refused.verified_dolphin_game_id(), None);
        assert!(
            refused
                .evidence
                .iter()
                .any(|item| item.diagnostic.contains("symlink refused"))
        );
    }

    #[test]
    fn mega_drive_loose_formats_receive_verified_local_byte_identity() {
        let directory = FixtureDir::new("loose-mega-drive");
        for extension in ["md", "gen", "smd"] {
            let bytes = format!("synthetic-{extension}-bytes").into_bytes();
            let path = write_fixture(
                &directory,
                &format!("Alien 3 (USA, Europe).{extension}"),
                &bytes,
            );
            let report = inspect_catalogued_game_identity(&path, Some("MegaDrive"));
            assert_eq!(report.platform, IdentityPlatform::MegaDrive);
            assert_eq!(report.format, IdentityImageFormat::LooseCartridgeRom);
            assert_eq!(report.bytes_read, bytes.len() as u64);
            let expected = sha256_hex(&bytes);
            assert_eq!(report.verified_loose_rom_sha256(), Some(expected.as_str()));
            assert!(report.complete);
            assert!(report.evidence.iter().any(|item| {
                item.kind == IdentityKind::LooseRomSha256
                    && item.diagnostic.contains("not a known-good dump claim")
            }));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn contextual_bin_requires_trusted_exact_platform_evidence() {
        let directory = FixtureDir::new("loose-bin-context");
        let path = write_fixture(&directory, "Game.bin", b"bytes");
        let candidate = inspect_game_identity(&path, Some("MegaDrive"));
        assert_eq!(candidate.verified_loose_rom_sha256(), None);
        assert!(candidate.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Ambiguous
        }));

        let verified = inspect_catalogued_game_identity(&path, Some("MegaDrive"));
        let expected = sha256_hex(b"bytes");
        assert_eq!(
            verified.verified_loose_rom_sha256(),
            Some(expected.as_str())
        );
        let unrelated = inspect_catalogued_game_identity(&path, Some("SNES"));
        assert_eq!(unrelated.verified_loose_rom_sha256(), None);
    }

    #[test]
    fn snes_loose_formats_receive_verified_local_byte_identity() {
        let directory = FixtureDir::new("loose-snes");
        for extension in ["sfc", "smc"] {
            let bytes = format!("synthetic-{extension}-bytes").into_bytes();
            let path = write_fixture(&directory, &format!("Chrono Quest.{extension}"), &bytes);
            let report = inspect_catalogued_game_identity(&path, Some("SNES"));
            assert_eq!(report.platform, IdentityPlatform::Snes);
            let expected = sha256_hex(&bytes);
            assert_eq!(report.verified_loose_rom_sha256(), Some(expected.as_str()));
        }
    }

    fn production_snes_fixture(mode: SnesMapMode, copier_header: bool) -> Vec<u8> {
        let minimum_len = mode.base_offset() + crate::snes_header_evidence::SNES_HEADER_LEN;
        let payload_len = minimum_len.div_ceil(32 * 1024) * (32 * 1024);
        let mut payload = vec![0u8; payload_len];
        let base = mode.base_offset();
        payload[base..base + 15].copy_from_slice(b"PRODUCTION SNES");
        payload[base + 0x15] = match mode {
            SnesMapMode::LoRom => 0x20,
            SnesMapMode::HiRom => 0x21,
            SnesMapMode::ExHiRom => 0x25,
        };
        payload[base + 0x16] = 0x01;
        payload[base + 0x17] = 0x0c;
        payload[base + 0x18] = 0x03;
        payload[base + 0x19] = 0x01;
        payload[base + 0x1a] = 0x33;
        payload[base + 0x1b] = 0x00;
        let checksum = 0x1234u16;
        payload[base + 0x1c..base + 0x1e].copy_from_slice(&(checksum ^ 0xffff).to_le_bytes());
        payload[base + 0x1e..base + 0x20].copy_from_slice(&checksum.to_le_bytes());
        if copier_header {
            let mut bytes = vec![0u8; 512];
            bytes.extend_from_slice(&payload);
            bytes
        } else {
            payload
        }
    }

    #[test]
    fn valid_snes_headers_reach_production_identity() {
        let directory = FixtureDir::new("snes-production-headers");
        for (extension, mode) in [("sfc", SnesMapMode::LoRom), ("smc", SnesMapMode::HiRom)] {
            let path = write_fixture(
                &directory,
                &format!("Headered {extension}.{extension}"),
                &production_snes_fixture(mode, false),
            );
            let report = inspect_catalogued_game_identity(&path, Some("SNES"));
            assert_eq!(report.verified_snes_header(), Some(mode.label()));
            let evidence = report
                .evidence
                .iter()
                .find(|item| item.kind == IdentityKind::SnesHeader)
                .unwrap();
            assert!(evidence.diagnostic.contains("PRODUCTION SNES"));
            assert!(evidence.diagnostic.contains("ROM size code 0x0c"));
            assert!(evidence.diagnostic.contains("checksum 0x1234"));
        }
    }

    #[test]
    fn snes_copier_header_is_structurally_recognised() {
        let directory = FixtureDir::new("snes-copier-production");
        let path = write_fixture(
            &directory,
            "same.smc",
            &production_snes_fixture(SnesMapMode::LoRom, true),
        );
        let report = inspect_catalogued_game_identity(&path, Some("SNES"));
        assert_eq!(report.verified_snes_header(), Some("LoROM"));
        let evidence = report
            .evidence
            .iter()
            .find(|item| item.kind == IdentityKind::SnesHeader)
            .unwrap();
        assert!(evidence.diagnostic.contains("copier header true"));
        assert!(evidence.diagnostic.contains("PRODUCTION SNES"));
    }

    #[test]
    fn random_truncated_and_ambiguous_snes_headers_fail_closed_in_production() {
        let directory = FixtureDir::new("snes-production-negative");
        for (name, bytes) in [
            ("SNES title only.sfc", b"SNES title only".to_vec()),
            ("random.smc", vec![0xa5; 128 * 1024]),
            (
                "truncated.sfc",
                production_snes_fixture(SnesMapMode::LoRom, false)[..0x7fc0].to_vec(),
            ),
        ] {
            let path = write_fixture(&directory, name, &bytes);
            let report = inspect_catalogued_game_identity(&path, Some("SNES"));
            assert_eq!(report.verified_snes_header(), None, "{name}");
        }

        let mut ambiguous = production_snes_fixture(SnesMapMode::HiRom, false);
        let lorom = production_snes_fixture(SnesMapMode::LoRom, false);
        ambiguous[0x7fc0..0x7fc0 + 0x20].copy_from_slice(&lorom[0x7fc0..0x7fc0 + 0x20]);
        let path = write_fixture(&directory, "ambiguous.sfc", &ambiguous);
        let report = inspect_catalogued_game_identity(&path, Some("SNES"));
        assert_eq!(report.verified_snes_header(), None);
    }

    #[test]
    fn nes_loose_format_receives_verified_local_byte_identity() {
        let directory = FixtureDir::new("loose-nes");
        let bytes = b"synthetic-nes-bytes".to_vec();
        let path = write_fixture(&directory, "Mega Man (USA).nes", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("NES"));
        assert_eq!(report.platform, IdentityPlatform::Nes);
        assert_eq!(report.format, IdentityImageFormat::LooseCartridgeRom);
        assert_eq!(report.bytes_read, bytes.len() as u64);
        let expected = sha256_hex(&bytes);
        assert_eq!(report.verified_loose_rom_sha256(), Some(expected.as_str()));
        assert!(report.complete);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256
                && item.diagnostic.contains("not a known-good dump claim")
        }));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn valid_ines_header_reaches_production_identity_with_structured_facts() {
        let directory = FixtureDir::new("ines-production");
        let mut bytes = vec![0u8; 16 + 512 + 16 * 1024 + 8 * 1024];
        bytes[0..4].copy_from_slice(b"NES\x1a");
        bytes[4] = 1;
        bytes[5] = 1;
        bytes[6] = 0x7f;
        bytes[7] = 0x20;
        let path = write_fixture(&directory, "Headered.nes", &bytes);

        let report = inspect_catalogued_game_identity(&path, Some("NES"));
        let header = report
            .evidence
            .iter()
            .find(|item| item.kind == IdentityKind::NesHeader)
            .expect("valid iNES header should be exposed in the production report");
        assert_eq!(header.value.as_deref(), Some("iNES"));
        assert!(header.diagnostic.contains("mapper 39"));
        assert!(header.diagnostic.contains("trainer true"));
        assert!(header.diagnostic.contains("battery true"));
        assert!(header.diagnostic.contains("FourScreen"));
    }

    #[test]
    fn valid_nes20_header_preserves_mapper_and_submapper_facts() {
        let directory = FixtureDir::new("nes20-production");
        let mut bytes = vec![0u8; 16 + 16 * 1024];
        bytes[0..4].copy_from_slice(b"NES\x1a");
        bytes[4] = 1;
        bytes[6] = 0x10;
        bytes[7] = 0x08;
        bytes[8] = 0x52;
        let path = write_fixture(&directory, "Headered.nes", &bytes);

        let report = inspect_catalogued_game_identity(&path, Some("NES"));
        let header = report
            .evidence
            .iter()
            .find(|item| item.kind == IdentityKind::NesHeader)
            .expect("valid NES 2.0 header should be exposed");
        assert_eq!(header.value.as_deref(), Some("NES 2.0"));
        assert!(header.diagnostic.contains("mapper 513"));
        assert!(header.diagnostic.contains("submapper 5"));
    }

    #[test]
    fn malformed_nes_headers_and_unf_do_not_gain_ines_evidence() {
        let directory = FixtureDir::new("nes-header-negative");
        for (name, bytes) in [
            ("wrong.nes", b"not an ines header".to_vec()),
            ("truncated.nes", b"NES\x1a\x01".to_vec()),
            ("unif.unf", b"UNIF\x00not an iNES header".to_vec()),
        ] {
            let path = write_fixture(&directory, name, &bytes);
            let report = inspect_catalogued_game_identity(&path, Some("NES"));
            assert!(
                !report
                    .evidence
                    .iter()
                    .any(|item| item.kind == IdentityKind::NesHeader),
                "{name}"
            );
        }
    }

    #[test]
    fn nes_platform_hint_recognizes_every_catalogue_synonym() {
        for hint in ["NES", "Nintendo Entertainment System", "Famicom", "nes"] {
            assert_eq!(
                IdentityPlatform::from_catalogue(Some(hint)),
                IdentityPlatform::Nes,
                "{hint} must resolve to IdentityPlatform::Nes"
            );
        }
    }

    #[test]
    fn nes_loose_rom_requires_trusted_exact_platform_evidence() {
        // The mirror of `contextual_bin_requires_trusted_exact_platform_evidence`
        // for NES: an *uncatalogued* platform hint (a scanner guess, not a
        // trusted/manual assignment) must never authorize a verified local
        // hash - filename/context guessing alone is never identity.
        let directory = FixtureDir::new("loose-nes-untrusted");
        let path = write_fixture(&directory, "Mystery Game.nes", b"bytes");
        let candidate = inspect_game_identity(&path, Some("NES"));
        assert_eq!(candidate.verified_loose_rom_sha256(), None);
        assert!(candidate.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Ambiguous
        }));
        assert!(!candidate.complete);
    }

    #[test]
    fn nes_wrong_extension_is_unsupported_not_guessed() {
        let directory = FixtureDir::new("loose-nes-wrong-ext");
        let path = write_fixture(&directory, "Mystery Game.bin", b"bytes");
        let report = inspect_catalogued_game_identity(&path, Some("NES"));
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert!(!report.complete);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Unsupported
        }));
    }

    // ------------------------------------------------------------------
    // Game Boy / Game Boy Color / Game Boy Advance loose-ROM identity
    // ------------------------------------------------------------------

    /// Builds a minimal, valid GB/GBC header: real Nintendo logo, the given
    /// `cgb_flag`, and a correctly-computed header checksum - everything
    /// [`gameboy_extension_conflict`]'s real content-based check needs to
    /// actually run against, not a placeholder.
    fn valid_gb_header(cgb_flag: u8) -> Vec<u8> {
        use crate::gb_header_evidence::{GB_HEADER_BYTES, compute_header_checksum};
        const NINTENDO_LOGO_OFFSET: usize = 0x104;
        const NINTENDO_LOGO: [u8; 48] = [
            0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C,
            0x00, 0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6,
            0xDD, 0xDD, 0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC,
            0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
        ];
        const CGB_FLAG_OFFSET: usize = 0x143;
        const HEADER_CHECKSUM_OFFSET: usize = 0x14D;
        let mut bytes = vec![0u8; GB_HEADER_BYTES];
        bytes[NINTENDO_LOGO_OFFSET..NINTENDO_LOGO_OFFSET + NINTENDO_LOGO.len()]
            .copy_from_slice(&NINTENDO_LOGO);
        bytes[CGB_FLAG_OFFSET] = cgb_flag;
        bytes[HEADER_CHECKSUM_OFFSET] = compute_header_checksum(&bytes).unwrap();
        bytes
    }

    #[test]
    fn game_boy_loose_format_receives_verified_local_byte_identity() {
        let directory = FixtureDir::new("loose-gb");
        let bytes = valid_gb_header(0x00); // DMG-only: a real, ordinary .gb cartridge
        let path = write_fixture(&directory, "Tetris (World).gb", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Game Boy"));
        assert_eq!(report.platform, IdentityPlatform::GameBoy);
        assert_eq!(report.format, IdentityImageFormat::LooseCartridgeRom);
        assert_eq!(report.bytes_read, bytes.len() as u64);
        let expected = sha256_hex(&bytes);
        assert_eq!(report.verified_loose_rom_sha256(), Some(expected.as_str()));
        assert!(report.complete);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256
                && item.diagnostic.contains("not a known-good dump claim")
        }));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn game_boy_color_loose_format_receives_verified_local_byte_identity() {
        let directory = FixtureDir::new("loose-gbc");
        let bytes = valid_gb_header(0x80); // CGB-enhanced, still DMG-compatible
        let path = write_fixture(&directory, "Pokemon Gold (USA).gbc", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Game Boy Color"));
        assert_eq!(report.platform, IdentityPlatform::GameBoyColor);
        let expected = sha256_hex(&bytes);
        assert_eq!(report.verified_loose_rom_sha256(), Some(expected.as_str()));
        assert!(report.complete);
    }

    #[test]
    fn game_boy_advance_loose_format_receives_verified_local_byte_identity() {
        let directory = FixtureDir::new("loose-gba");
        let bytes = b"synthetic-gba-cartridge-bytes".to_vec();
        let path = write_fixture(&directory, "Metroid Fusion (USA).gba", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Game Boy Advance"));
        assert_eq!(report.platform, IdentityPlatform::GameBoyAdvance);
        assert_eq!(report.format, IdentityImageFormat::LooseCartridgeRom);
        assert_eq!(report.bytes_read, bytes.len() as u64);
        let expected = sha256_hex(&bytes);
        assert_eq!(report.verified_loose_rom_sha256(), Some(expected.as_str()));
        assert!(report.complete);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn game_boy_actual_content_hash_drives_identity_not_filename() {
        let directory = FixtureDir::new("loose-gb-content");
        let same_name = "Final Fantasy Legend.gb";
        let a = write_fixture(&directory, same_name, b"content-one");
        let report_a = inspect_catalogued_game_identity(&a, Some("Game Boy"));
        fs::remove_file(&a).unwrap();
        let b = write_fixture(&directory, same_name, b"content-two");
        let report_b = inspect_catalogued_game_identity(&b, Some("Game Boy"));
        assert_ne!(
            report_a.verified_loose_rom_sha256(),
            report_b.verified_loose_rom_sha256(),
            "identical filenames with different content must not share an identity"
        );
        assert_eq!(
            report_b.verified_loose_rom_sha256(),
            Some(sha256_hex(b"content-two")).as_deref()
        );
    }

    #[test]
    fn game_boy_filename_only_game_like_names_do_not_resolve_when_untrusted() {
        let directory = FixtureDir::new("loose-gb-filename-trap");
        // A thoroughly game-like, plausible commercial title - the point is
        // that no filename, however convincing, ever substitutes for a
        // trusted platform assignment.
        let path = write_fixture(
            &directory,
            "The Legend of Zelda - Link's Awakening (USA, Europe).gb",
            b"unverified bytes",
        );
        let candidate = inspect_game_identity(&path, Some("Game Boy"));
        assert_eq!(candidate.verified_loose_rom_sha256(), None);
        assert!(!candidate.complete);
        assert!(candidate.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Ambiguous
        }));
    }

    #[test]
    fn game_boy_untrusted_platform_evidence_fails_closed() {
        let directory = FixtureDir::new("loose-gb-untrusted");
        let path = write_fixture(&directory, "Mystery.gb", b"bytes");
        let candidate = inspect_game_identity(&path, Some("Game Boy"));
        assert_eq!(candidate.verified_loose_rom_sha256(), None);
        assert!(candidate.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Ambiguous
        }));

        let trusted = inspect_catalogued_game_identity(&path, Some("Game Boy"));
        assert_eq!(
            trusted.verified_loose_rom_sha256(),
            Some(sha256_hex(b"bytes")).as_deref()
        );
    }

    #[test]
    fn game_boy_wrong_extension_is_unsupported_not_guessed() {
        let directory = FixtureDir::new("loose-gb-wrong-ext");
        let path = write_fixture(&directory, "Mystery Game.bin", b"bytes");
        let report = inspect_catalogued_game_identity(&path, Some("Game Boy"));
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert!(!report.complete);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Unsupported
        }));
    }

    #[test]
    fn game_boy_zip_extension_is_unsupported_not_extracted() {
        // No archive-mount/ZIP-member path exists for any loose-cartridge
        // platform (Mega Drive/SNES/NES included) - a `.zip` extension
        // simply fails closed as unsupported, exactly like any other wrong
        // extension. This crate never adds ZIP extraction for this pass.
        let directory = FixtureDir::new("loose-gb-zip");
        let path = write_fixture(&directory, "Mystery Game.zip", b"bytes");
        let report = inspect_catalogued_game_identity(&path, Some("Game Boy"));
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Unsupported
        }));
    }

    #[test]
    fn game_boy_extension_paired_with_cgb_only_content_conflicts_and_fails_closed() {
        // The header's own cgb_flag (0xC0) proves this title cannot run on
        // original Game Boy hardware - a genuine content/platform
        // contradiction under a plain .gb/"Game Boy" assignment, not merely
        // unverified. This is the one case this module fails closed on
        // instead of trusting the caller's platform assignment outright.
        let directory = FixtureDir::new("loose-gb-cgb-conflict");
        let bytes = valid_gb_header(0xC0);
        let path = write_fixture(&directory, "Genuinely CGB-Only Game.gb", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Game Boy"));
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert!(!report.complete);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256
                && item.status == IdentityStatus::Invalid
                && item.diagnostic.contains("Game Boy Color-exclusive")
        }));
    }

    #[test]
    fn game_boy_color_extension_accepts_cgb_only_content_without_conflict() {
        // The identical CGB-only header is perfectly consistent under a
        // Game Boy Color assignment - only the .gb/"Game Boy" pairing is a
        // structural contradiction, never .gbc/"Game Boy Color" itself.
        let directory = FixtureDir::new("loose-gbc-cgb-only");
        let bytes = valid_gb_header(0xC0);
        let path = write_fixture(&directory, "Genuinely CGB-Only Game.gbc", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Game Boy Color"));
        assert_eq!(report.platform, IdentityPlatform::GameBoyColor);
        let expected = sha256_hex(&bytes);
        assert_eq!(report.verified_loose_rom_sha256(), Some(expected.as_str()));
        assert!(report.complete);
    }

    #[test]
    fn game_boy_cgb_enhanced_content_is_not_a_conflict_under_dot_gb() {
        // cgb_flag=0x80 (CGB-enhanced) is still a genuine, backward-
        // compatible Game Boy cartridge - it must verify normally under a
        // plain .gb/"Game Boy" assignment, unlike the 0xC0 (CGB-only) case.
        let directory = FixtureDir::new("loose-gb-cgb-enhanced");
        let bytes = valid_gb_header(0x80);
        let path = write_fixture(&directory, "Dual Mode Game.gb", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Game Boy"));
        assert_eq!(report.platform, IdentityPlatform::GameBoy);
        let expected = sha256_hex(&bytes);
        assert_eq!(report.verified_loose_rom_sha256(), Some(expected.as_str()));
        assert!(report.complete);
    }

    #[test]
    fn game_boy_advance_extension_cannot_be_verified_under_the_game_boy_platform() {
        let directory = FixtureDir::new("loose-gba-cross-ext");
        let path = write_fixture(&directory, "Mystery.gba", b"bytes");
        let report = inspect_catalogued_game_identity(&path, Some("Game Boy"));
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Unsupported
        }));
    }

    #[test]
    fn game_boy_family_quoted_spaces_and_unicode_paths_verify() {
        let directory = FixtureDir::new("loose-gb-unicode");
        for (name, extension, platform) in [
            ("Pokémon Blue Version (USA).gb", "gb", "Game Boy"),
            (
                "ゼルダの伝説 夢をみる島 (Japan).gbc",
                "gbc",
                "Game Boy Color",
            ),
            (
                "Kirby & the Amazing Mirror (USA).gba",
                "gba",
                "Game Boy Advance",
            ),
        ] {
            let bytes = format!("synthetic-{extension}-bytes").into_bytes();
            let path = write_fixture(&directory, name, &bytes);
            let report = inspect_catalogued_game_identity(&path, Some(platform));
            let expected = sha256_hex(&bytes);
            assert_eq!(
                report.verified_loose_rom_sha256(),
                Some(expected.as_str()),
                "failed for {name}"
            );
        }
    }

    #[test]
    fn game_boy_family_platform_hints_recognize_every_catalogue_synonym() {
        for (hint, expected) in [
            ("Game Boy", IdentityPlatform::GameBoy),
            ("game boy", IdentityPlatform::GameBoy),
            ("GB", IdentityPlatform::GameBoy),
            ("Nintendo Game Boy", IdentityPlatform::GameBoy),
            ("Game Boy Color", IdentityPlatform::GameBoyColor),
            ("GBC", IdentityPlatform::GameBoyColor),
            ("Nintendo Game Boy Color", IdentityPlatform::GameBoyColor),
            ("Game Boy Advance", IdentityPlatform::GameBoyAdvance),
            ("GBA", IdentityPlatform::GameBoyAdvance),
            (
                "Nintendo Game Boy Advance",
                IdentityPlatform::GameBoyAdvance,
            ),
        ] {
            assert_eq!(
                IdentityPlatform::from_catalogue(Some(hint)),
                expected,
                "{hint} must resolve to {expected:?}"
            );
        }
    }

    // ------------------------------------------------------------------
    // N64 loose-ROM identity
    // ------------------------------------------------------------------

    /// A synthetic, canonical (`Z64`) N64 ROM: the real magic header
    /// followed by distinct, non-repeating per-word content so a
    /// header-only-transform bug (rather than a whole-buffer transform)
    /// would be caught by comparing normalized output - the same
    /// discipline [`crate::n64_byte_order`]'s own tests use.
    fn synthetic_z64_rom() -> Vec<u8> {
        use crate::n64_byte_order::N64ByteOrder;
        let mut bytes = N64ByteOrder::Z64.magic().to_vec();
        for word in 0u32..64 {
            bytes.extend_from_slice(&(0x1000_0000u32.wrapping_add(word)).to_be_bytes());
        }
        bytes
    }

    fn to_v64(z64: &[u8]) -> Vec<u8> {
        use crate::n64_byte_order::{N64ByteOrder, normalize_to_z64};
        // `normalize_to_z64` is its own inverse for a byte-pair swap, so
        // running it again on canonical bytes under `V64` produces a
        // correctly V64-ordered buffer - reusing the tested primitive
        // rather than hand-rolling a second swap implementation here.
        normalize_to_z64(z64, N64ByteOrder::V64).unwrap().bytes
    }

    fn to_n64(z64: &[u8]) -> Vec<u8> {
        use crate::n64_byte_order::{N64ByteOrder, normalize_to_z64};
        normalize_to_z64(z64, N64ByteOrder::N64).unwrap().bytes
    }

    #[test]
    fn n64_z64_receives_verified_physical_and_canonical_identity() {
        let directory = FixtureDir::new("loose-n64-z64");
        let bytes = synthetic_z64_rom();
        let path = write_fixture(&directory, "Super Mario 64 (USA).z64", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("N64"));
        assert_eq!(report.platform, IdentityPlatform::N64);
        assert_eq!(report.format, IdentityImageFormat::LooseCartridgeRom);
        assert_eq!(report.bytes_read, bytes.len() as u64);
        assert_eq!(
            report.verified_loose_rom_sha256(),
            Some(sha256_hex(&bytes)).as_deref()
        );
        // Already canonical: physical and normalized bytes are identical,
        // so the canonical hash must equal the physical hash exactly.
        assert_eq!(
            report.verified_loose_rom_canonical_sha256(),
            Some(sha256_hex(&bytes)).as_deref()
        );
        assert!(report.complete);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn n64_v64_receives_verified_physical_and_canonical_identity() {
        let directory = FixtureDir::new("loose-n64-v64");
        let z64 = synthetic_z64_rom();
        let v64 = to_v64(&z64);
        let path = write_fixture(&directory, "Super Mario 64 (USA).v64", &v64);
        let report = inspect_catalogued_game_identity(&path, Some("N64"));
        assert_eq!(report.platform, IdentityPlatform::N64);
        assert_eq!(
            report.verified_loose_rom_sha256(),
            Some(sha256_hex(&v64)).as_deref(),
            "physical hash must cover the exact on-disk (V64-ordered) bytes"
        );
        assert_eq!(
            report.verified_loose_rom_canonical_sha256(),
            Some(sha256_hex(&z64)).as_deref(),
            "canonical hash must equal the same ROM's Z64 hash"
        );
        assert!(report.complete);
    }

    #[test]
    fn n64_n64_receives_verified_physical_and_canonical_identity() {
        let directory = FixtureDir::new("loose-n64-n64");
        let z64 = synthetic_z64_rom();
        let n64 = to_n64(&z64);
        let path = write_fixture(&directory, "Super Mario 64 (USA).n64", &n64);
        let report = inspect_catalogued_game_identity(&path, Some("N64"));
        assert_eq!(report.platform, IdentityPlatform::N64);
        assert_eq!(
            report.verified_loose_rom_sha256(),
            Some(sha256_hex(&n64)).as_deref(),
            "physical hash must cover the exact on-disk (N64-ordered) bytes"
        );
        assert_eq!(
            report.verified_loose_rom_canonical_sha256(),
            Some(sha256_hex(&z64)).as_deref(),
            "canonical hash must equal the same ROM's Z64 hash"
        );
        assert!(report.complete);
    }

    #[test]
    fn n64_byte_order_variants_share_one_canonical_identity_but_differ_physically() {
        // The core claim this milestone exists to prove: z64/v64/n64 dumps
        // of literally the same ROM must never be treated as different
        // games merely because their raw file hashes differ.
        let directory = FixtureDir::new("loose-n64-equivalence");
        let z64 = synthetic_z64_rom();
        let v64 = to_v64(&z64);
        let n64 = to_n64(&z64);

        let report_z64 = inspect_catalogued_game_identity(
            &write_fixture(&directory, "Game.z64", &z64),
            Some("N64"),
        );
        let report_v64 = inspect_catalogued_game_identity(
            &write_fixture(&directory, "Game.v64", &v64),
            Some("N64"),
        );
        let report_n64 = inspect_catalogued_game_identity(
            &write_fixture(&directory, "Game.n64", &n64),
            Some("N64"),
        );

        let canonical = report_z64.verified_loose_rom_canonical_sha256();
        assert!(canonical.is_some());
        assert_eq!(canonical, report_v64.verified_loose_rom_canonical_sha256());
        assert_eq!(canonical, report_n64.verified_loose_rom_canonical_sha256());

        // But the exact physical bytes genuinely differ, so the physical
        // hash must differ too - normalization must never overwrite or
        // hide the real on-disk identity.
        assert_ne!(
            report_z64.verified_loose_rom_sha256(),
            report_v64.verified_loose_rom_sha256()
        );
        assert_ne!(
            report_z64.verified_loose_rom_sha256(),
            report_n64.verified_loose_rom_sha256()
        );
    }

    #[test]
    fn n64_malformed_header_is_rejected_but_physical_identity_still_verifies() {
        // Bytes with no recognizable z64/v64/n64 magic - a genuine,
        // structurally invalid header. The physical hash is still real
        // (it covers whatever bytes are actually on disk), but no
        // canonical fact may be fabricated on top of an unrecognized
        // header.
        let directory = FixtureDir::new("loose-n64-malformed");
        let bytes = b"this is not an n64 rom header at all!!".to_vec();
        let path = write_fixture(&directory, "Mystery.z64", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("N64"));
        assert_eq!(
            report.verified_loose_rom_sha256(),
            Some(sha256_hex(&bytes)).as_deref()
        );
        assert_eq!(report.verified_loose_rom_canonical_sha256(), None);
        assert!(report.complete);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("byte-order header not recognized"))
        );
    }

    #[test]
    fn n64_filename_only_identity_is_refused_when_untrusted() {
        let directory = FixtureDir::new("loose-n64-untrusted");
        let bytes = synthetic_z64_rom();
        let path = write_fixture(&directory, "Super Mario 64 (USA).z64", &bytes);
        let candidate = inspect_game_identity(&path, Some("N64"));
        assert_eq!(candidate.verified_loose_rom_sha256(), None);
        assert_eq!(candidate.verified_loose_rom_canonical_sha256(), None);
        assert!(candidate.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Ambiguous
        }));
        assert!(!candidate.complete);
    }

    #[test]
    fn n64_wrong_extension_platform_combination_fails_closed() {
        let directory = FixtureDir::new("loose-n64-wrong-ext");
        let bytes = synthetic_z64_rom();
        let path = write_fixture(&directory, "Mystery Game.gba", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("N64"));
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert_eq!(report.verified_loose_rom_canonical_sha256(), None);
        assert!(!report.complete);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Unsupported
        }));
    }

    #[test]
    fn n64_zip_extension_is_unsupported_not_extracted() {
        let directory = FixtureDir::new("loose-n64-zip");
        let bytes = synthetic_z64_rom();
        let path = write_fixture(&directory, "Mystery Game.zip", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("N64"));
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256 && item.status == IdentityStatus::Unsupported
        }));
    }

    #[test]
    fn n64_platform_hint_recognizes_every_catalogue_synonym() {
        for hint in ["N64", "Nintendo 64", "nintendo64", "n64"] {
            assert_eq!(
                IdentityPlatform::from_catalogue(Some(hint)),
                IdentityPlatform::N64,
                "{hint} must resolve to IdentityPlatform::N64"
            );
        }
    }

    #[test]
    fn modern_nintendo_catalogue_platforms_round_trip_aliases_and_serde() {
        for (aliases, expected, label) in [
            (
                &["WiiU", "Wii U", "Nintendo Wii U"][..],
                IdentityPlatform::WiiU,
                "Wii U",
            ),
            (
                &["3DS", "Nintendo 3DS", "New 3DS"][..],
                IdentityPlatform::ThreeDS,
                "Nintendo 3DS",
            ),
            (
                &["Switch", "Nintendo Switch"][..],
                IdentityPlatform::Switch,
                "Nintendo Switch",
            ),
        ] {
            for alias in aliases {
                assert_eq!(IdentityPlatform::from_catalogue(Some(alias)), expected);
            }
            assert_eq!(expected.label(), label);
            let encoded = serde_json::to_string(&expected).unwrap();
            assert_eq!(
                serde_json::from_str::<IdentityPlatform>(&encoded).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn modern_nintendo_platform_context_stays_unsupported_without_a_parser() {
        let directory = FixtureDir::new("modern-nintendo-unsupported");
        let path = write_fixture(&directory, "Mario.xci", b"not a Switch parser fixture");
        let report = inspect_catalogued_game_identity(&path, Some("Nintendo Switch"));
        assert_eq!(report.platform, IdentityPlatform::Switch);
        assert_eq!(report.verified_value(IdentityKind::Ps3TitleId), None);
        assert!(!report.complete);
        let (status, facts) =
            crate::launch::evidence_bridge::canonical_identity_from_game_report(&report);
        assert!(matches!(
            status,
            crate::launch::planning::CanonicalIdentityStatus::Unknown
        ));
        assert!(facts.is_empty());
    }

    #[test]
    fn n64_spaces_and_unicode_paths_verify() {
        let directory = FixtureDir::new("loose-n64-unicode");
        for name in [
            "Star Fox 64 (USA) (Rev A).z64",
            "ゼルダの伝説 時のオカリナ (Japan).v64",
            "Paper Mario - Édition Spéciale.n64",
        ] {
            let bytes = synthetic_z64_rom();
            let path = write_fixture(&directory, name, &bytes);
            let report = inspect_catalogued_game_identity(&path, Some("N64"));
            assert_eq!(
                report.verified_loose_rom_sha256(),
                Some(sha256_hex(&bytes)).as_deref(),
                "failed for {name}"
            );
        }
    }

    #[test]
    fn n64_content_hash_drives_identity_not_filename() {
        let directory = FixtureDir::new("loose-n64-content");
        let same_name = "Same Name.z64";
        let a_bytes = synthetic_z64_rom();
        let mut b_bytes = a_bytes.clone();
        b_bytes[8] ^= 0xFF; // still a valid Z64 header, different content
        let a = write_fixture(&directory, same_name, &a_bytes);
        let report_a = inspect_catalogued_game_identity(&a, Some("N64"));
        fs::remove_file(&a).unwrap();
        let b = write_fixture(&directory, same_name, &b_bytes);
        let report_b = inspect_catalogued_game_identity(&b, Some("N64"));
        assert_ne!(
            report_a.verified_loose_rom_sha256(),
            report_b.verified_loose_rom_sha256(),
            "identical filenames with different content must not share an identity"
        );
    }

    #[test]
    fn n64_no_fabricated_verified_identity_fact_beyond_hashes() {
        // Only LooseRomSha256/LooseRomCanonicalSha256/LooseRomFormat/
        // LooseRomTitle may ever appear Verified for N64 - no invented
        // platform-specific fact (e.g. a cartridge serial) exists.
        let directory = FixtureDir::new("loose-n64-no-fabrication");
        let bytes = synthetic_z64_rom();
        let path = write_fixture(&directory, "Game.z64", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("N64"));
        for item in report
            .evidence
            .iter()
            .filter(|item| item.status == IdentityStatus::Verified)
        {
            assert!(
                matches!(
                    item.kind,
                    IdentityKind::Platform
                        | IdentityKind::LooseRomSha256
                        | IdentityKind::LooseRomCanonicalSha256
                        | IdentityKind::LooseRomFormat
                        | IdentityKind::LooseRomTitle
                ),
                "unexpected verified N64 identity kind: {:?}",
                item.kind
            );
        }
    }

    #[test]
    fn oversized_loose_rom_fails_closed_without_hashing() {
        let directory = FixtureDir::new("loose-oversized");
        let path = directory.0.join("Too Large.md");
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_LOOSE_ROM_BYTES + 1).unwrap();
        let report = inspect_catalogued_game_identity(&path, Some("MegaDrive"));
        assert_eq!(report.bytes_read, 0);
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::LooseRomSha256
                && item.status == IdentityStatus::ResourceLimitReached
        }));
    }

    #[test]
    fn loose_rom_stability_check_rejects_file_mutation() {
        let directory = FixtureDir::new("loose-mutated");
        let path = write_fixture(&directory, "Changing.md", b"initial bytes");
        let file = std::fs::OpenOptions::new().read(true).open(&path).unwrap();
        let before = StableFileMetadata::from_file(&file).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        let after = StableFileMetadata::from_file(&file).unwrap();
        assert!(!loose_rom_read_was_stable(&before, &after, before.len));
    }

    #[cfg(unix)]
    #[test]
    fn loose_rom_refuses_symlinked_parent_and_preserves_non_utf8_path() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;

        let directory = FixtureDir::new("loose-paths");
        let real_parent = directory.0.join("real");
        fs::create_dir(&real_parent).unwrap();
        let link_parent = directory.0.join("linked");
        symlink(&real_parent, &link_parent).unwrap();
        let real = real_parent.join("Game.md");
        fs::write(&real, b"rom").unwrap();
        let refused =
            inspect_catalogued_game_identity(&link_parent.join("Game.md"), Some("MegaDrive"));
        assert_eq!(refused.verified_loose_rom_sha256(), None);

        let file_link = directory.0.join("linked-file.md");
        symlink(&real, &file_link).unwrap();
        let refused = inspect_catalogued_game_identity(&file_link, Some("MegaDrive"));
        assert_eq!(refused.verified_loose_rom_sha256(), None);

        let mut name = b"game-".to_vec();
        name.push(0xff);
        name.extend_from_slice(b".md");
        let path = directory.0.join(std::ffi::OsString::from_vec(name));
        fs::write(&path, b"non utf8 rom").unwrap();
        let report = inspect_catalogued_game_identity(&path, Some("MegaDrive"));
        assert_eq!(report.archive_path, path);
        assert!(report.verified_loose_rom_sha256().is_some());
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("path encoding"))
        );
        assert!(real.exists());
    }

    /// A minimal, well-formed XEX2 header: magic, one optional header
    /// entry (execution-info) pointing at a real `xex2_opt_execution_info`
    /// struct holding `media_id`/`title_id`.
    fn xex_fixture(title_id: u32, media_id: u32) -> Vec<u8> {
        const EXECUTION_INFO_OFFSET: u32 = 0x30;
        let mut bytes = vec![0_u8; EXECUTION_INFO_OFFSET as usize + XEX_EXECUTION_INFO_BYTES];
        bytes[0..4].copy_from_slice(&XEX_MAGIC);
        bytes[XEX_HEADER_COUNT_OFFSET..XEX_HEADER_COUNT_OFFSET + 4]
            .copy_from_slice(&1_u32.to_be_bytes());
        let table_offset = XEX_OPT_HEADER_TABLE_OFFSET as usize;
        bytes[table_offset..table_offset + 4]
            .copy_from_slice(&XEX_EXECUTION_INFO_KEY.to_be_bytes());
        bytes[table_offset + 4..table_offset + 8]
            .copy_from_slice(&EXECUTION_INFO_OFFSET.to_be_bytes());
        let info_offset = EXECUTION_INFO_OFFSET as usize;
        bytes[info_offset..info_offset + 4].copy_from_slice(&media_id.to_be_bytes());
        bytes[info_offset + 0xC..info_offset + 0x10].copy_from_slice(&title_id.to_be_bytes());
        bytes
    }

    #[test]
    fn verifies_xex_title_id_and_media_id_from_execution_info() {
        let directory = FixtureDir::new("xex");
        let path = write_fixture(
            &directory,
            "default.xex",
            &xex_fixture(0x4156_07D2, 0x4C27_792A),
        );
        let report = inspect_game_identity(&path, Some("Xbox 360"));
        assert_eq!(report.verified_xex_title_id(), Some("415607D2"));
        assert_eq!(report.verified_xex_media_id(), Some("4C27792A"));
        assert!(report.complete);
    }

    #[test]
    fn xex_wrong_magic_never_becomes_verified() {
        let directory = FixtureDir::new("xex-bad-magic");
        let mut bytes = xex_fixture(0x4156_07D2, 0);
        bytes[0..4].copy_from_slice(b"NOPE");
        let path = write_fixture(&directory, "default.xex", &bytes);
        let report = inspect_game_identity(&path, Some("Xbox 360"));
        assert_eq!(report.verified_xex_title_id(), None);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Invalid)
        );
    }

    #[test]
    fn truncated_xex_header_is_invalid_not_verified() {
        let directory = FixtureDir::new("xex-truncated");
        let path = write_fixture(&directory, "default.xex", b"XEX2");
        let report = inspect_game_identity(&path, Some("Xbox 360"));
        assert_eq!(report.verified_xex_title_id(), None);
        assert_eq!(report.verified_xex_media_id(), None);
    }

    #[test]
    fn xex_with_no_execution_info_header_is_missing_not_fabricated() {
        let directory = FixtureDir::new("xex-no-exec-info");
        let mut bytes = xex_fixture(0x4156_07D2, 0);
        // Point the one optional-header entry at an unrelated key so no
        // execution-info header is found.
        let table_offset = XEX_OPT_HEADER_TABLE_OFFSET as usize;
        bytes[table_offset..table_offset + 4].copy_from_slice(&0x0002_0000_u32.to_be_bytes());
        let path = write_fixture(&directory, "default.xex", &bytes);
        let report = inspect_game_identity(&path, Some("Xbox 360"));
        assert_eq!(report.verified_xex_title_id(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::XexTitleId && item.status == IdentityStatus::Missing
        }));
    }

    #[test]
    fn zip_with_one_xex_reads_only_the_xex_header() {
        let directory = FixtureDir::new("zip-xex");
        let path = directory.0.join("container.zip");
        let file = fs::File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file(
                "default.xex",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        let mut image = xex_fixture(0x4156_07D2, 0x0000_0000);
        image.resize(2 * 1024 * 1024, 0);
        writer.write_all(&image).unwrap();
        writer.finish().unwrap();

        let report = inspect_game_identity(&path, Some("Xbox 360"));
        assert_eq!(report.verified_xex_title_id(), Some("415607D2"));
        assert_eq!(report.archive_members_inspected, 1);
        assert_eq!(report.nested_container_depth, 1);
        assert!(report.bytes_read <= XEX_HEADER_PREFIX_BYTES);
    }

    #[test]
    fn zip_with_multiple_xex_members_is_ambiguous_not_guessed() {
        let directory = FixtureDir::new("zip-xex-ambiguous");
        let path = directory.0.join("container.zip");
        let file = fs::File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);
        for name in ["default.xex", "dash.xex"] {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&xex_fixture(0x4156_07D2, 0)).unwrap();
        }
        writer.finish().unwrap();

        let report = inspect_game_identity(&path, Some("Xbox 360"));
        assert_eq!(report.verified_xex_title_id(), None);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Ambiguous)
        );
    }

    #[test]
    fn xex_filename_token_is_only_ever_a_candidate() {
        let report = inspect_game_identity(Path::new("/games/415607D2.chd"), Some("Xbox 360"));
        assert_eq!(report.verified_xex_title_id(), None);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::XexTitleId
                && item.status == IdentityStatus::Candidate
                && item.value.as_deref() == Some("415607D2")
        }));
    }

    #[test]
    fn xbox_360_never_reads_an_iso_extension_as_a_disc_image() {
        let directory = FixtureDir::new("xex-not-iso");
        let path = write_fixture(
            &directory,
            "game.iso",
            &dolphin_fixture(IdentityPlatform::GameCube, b"GM8E01", 0),
        );
        let report = inspect_game_identity(&path, Some("Xbox 360"));
        assert_eq!(report.verified_xex_title_id(), None);
        assert_eq!(report.format, IdentityImageFormat::Unsupported);
    }

    // ------------------------------------------------------------------
    // Original Xbox (XBE) identity
    // ------------------------------------------------------------------

    /// A minimal, well-formed XBE header + certificate: magic, `base`/
    /// `certificate_addr` pointing the certificate right after the header,
    /// and a `title_id` encoded at the certificate's own fixed offset.
    fn xbe_fixture(title_id: u32) -> Vec<u8> {
        const XBE_BASE_OFFSET: usize = 0x104;
        const XBE_CERT_ADDR_OFFSET: usize = 0x118;
        const XBE_CERT_TITLE_ID_OFFSET: usize = 0x8;
        let base = 0x10000_u32;
        let cert_file_offset = 0x200_usize;
        let cert_addr = base + cert_file_offset as u32;

        let mut bytes = vec![0_u8; cert_file_offset + XBE_CERTIFICATE_READ_BYTES];
        bytes[0..4].copy_from_slice(b"XBEH");
        bytes[XBE_BASE_OFFSET..XBE_BASE_OFFSET + 4].copy_from_slice(&base.to_le_bytes());
        bytes[XBE_CERT_ADDR_OFFSET..XBE_CERT_ADDR_OFFSET + 4]
            .copy_from_slice(&cert_addr.to_le_bytes());
        bytes[cert_file_offset + XBE_CERT_TITLE_ID_OFFSET
            ..cert_file_offset + XBE_CERT_TITLE_ID_OFFSET + 4]
            .copy_from_slice(&title_id.to_le_bytes());
        bytes
    }

    #[test]
    fn verifies_xbe_title_id_from_certificate_when_platform_is_trusted() {
        let directory = FixtureDir::new("xbe");
        let path = write_fixture(&directory, "default.xbe", &xbe_fixture(0x4D53_0058));
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.platform, IdentityPlatform::Xbox);
        assert_eq!(report.format, IdentityImageFormat::Xbe);
        assert_eq!(report.verified_xbox_title_id(), Some("4D530058"));
        assert!(report.complete);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::XbeTitleId
                && item.status == IdentityStatus::Verified
                && item.confidence == IdentityConfidence::ExactBytes
        }));
    }

    #[test]
    fn untrusted_xbox_platform_evidence_never_authorizes_identity() {
        let directory = FixtureDir::new("xbe-untrusted");
        let path = write_fixture(&directory, "default.xbe", &xbe_fixture(0x4D53_0058));
        // Same real, structurally valid bytes as the verified test above -
        // only the platform trust differs, and that alone must be decisive.
        let report = inspect_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert!(!report.complete);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::XbeTitleId && item.status == IdentityStatus::Ambiguous
        }));
    }

    #[test]
    fn malformed_xbe_magic_fails_closed_not_verified() {
        let directory = FixtureDir::new("xbe-bad-magic");
        let mut bytes = xbe_fixture(0x4D53_0058);
        bytes[0..4].copy_from_slice(b"NOPE");
        let path = write_fixture(&directory, "default.xbe", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Invalid)
        );
    }

    #[test]
    fn truncated_xbe_header_fails_closed_not_verified() {
        let directory = FixtureDir::new("xbe-truncated");
        let path = write_fixture(&directory, "default.xbe", b"XBEH");
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert!(!report.complete);
    }

    #[test]
    fn zip_with_one_xbe_reads_only_the_xbe_header() {
        let directory = FixtureDir::new("zip-xbe");
        let path = directory.0.join("container.zip");
        let file = fs::File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file(
                "default.xbe",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        let image = xbe_fixture(0x4D53_0058);
        writer.write_all(&image).unwrap();
        writer.finish().unwrap();

        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), Some("4D530058"));
        assert_eq!(report.archive_members_inspected, 1);
        assert_eq!(report.nested_container_depth, 1);
    }

    #[test]
    fn zip_with_multiple_xbe_members_is_ambiguous_not_guessed() {
        let directory = FixtureDir::new("zip-xbe-ambiguous");
        let path = directory.0.join("container.zip");
        let file = fs::File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);
        for name in ["default.xbe", "dash.xbe"] {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&xbe_fixture(0x4D53_0058)).unwrap();
        }
        writer.finish().unwrap();

        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Ambiguous)
        );
    }

    #[test]
    fn xbe_filename_token_never_resolves_identity() {
        // A thoroughly XBE-title-ID-looking filename, with no real file
        // content at all - filename alone must never become verified
        // identity. Unlike Xbox 360's XEX path, this platform's
        // `add_filename_candidate` does not even emit a filename candidate
        // fact for it (see the catch-all group), so there is no evidence at
        // all beyond an outright refusal.
        let report =
            inspect_catalogued_game_identity(Path::new("/games/4D530058.xbe"), Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
    }

    #[test]
    fn xbox_never_reads_an_xex_extension_as_its_own_content() {
        let directory = FixtureDir::new("xbox-not-xex");
        let path = write_fixture(&directory, "default.xex", &xex_fixture(0x4156_07D2, 0));
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert_eq!(report.format, IdentityImageFormat::Unsupported);
    }

    #[test]
    fn xbox_iso_with_non_xdvdfs_content_fails_closed_not_unsupported() {
        // `.iso` under a trusted Xbox assignment is a genuinely supported
        // format (routed to the XDVDFS disc-image reader) - but content that
        // is not actually an XDVDFS volume (a GameCube disc image, here)
        // must fail closed as structurally invalid, never silently guessed
        // at or misreported as merely "Unsupported".
        let directory = FixtureDir::new("xbox-non-xdvdfs-iso");
        let path = write_fixture(
            &directory,
            "game.iso",
            &dolphin_fixture(IdentityPlatform::GameCube, b"GM8E01", 0),
        );
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert_eq!(report.format, IdentityImageFormat::XboxDiscImage);
        assert!(!report.complete);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Invalid)
        );
    }

    #[test]
    fn xbox_and_xbox_360_never_cross_authorize() {
        // The exact same real, structurally valid XEX content, trusted, must
        // verify only as Xbox 360 - never as original Xbox - and vice versa
        // for a real XBE under an Xbox 360 platform assignment.
        let directory = FixtureDir::new("xbox-vs-xbox360");
        let xex_path = write_fixture(
            &directory,
            "default.xex",
            &xex_fixture(0x4156_07D2, 0x4C27_792A),
        );
        let as_xbox360 = inspect_catalogued_game_identity(&xex_path, Some("Xbox 360"));
        assert_eq!(as_xbox360.verified_xex_title_id(), Some("415607D2"));
        assert_eq!(as_xbox360.verified_xbox_title_id(), None);

        let xbe_path = write_fixture(&directory, "default.xbe", &xbe_fixture(0x4D53_0058));
        let as_xbox = inspect_catalogued_game_identity(&xbe_path, Some("Xbox"));
        assert_eq!(as_xbox.verified_xbox_title_id(), Some("4D530058"));
        assert_eq!(as_xbox.verified_xex_title_id(), None);

        // And swapping platform assignments across formats resolves nothing
        // for either: an XBE trusted as "Xbox 360" is an unsupported
        // extension for that platform, and an XEX trusted as "Xbox" is
        // likewise unsupported for it.
        let xbe_as_xbox360 = inspect_catalogued_game_identity(&xbe_path, Some("Xbox 360"));
        assert_eq!(xbe_as_xbox360.verified_xex_title_id(), None);
        assert_eq!(xbe_as_xbox360.verified_xbox_title_id(), None);
        let xex_as_xbox = inspect_catalogued_game_identity(&xex_path, Some("Xbox"));
        assert_eq!(xex_as_xbox.verified_xex_title_id(), None);
        assert_eq!(xex_as_xbox.verified_xbox_title_id(), None);
    }

    #[test]
    fn xbox_platform_hint_recognizes_every_catalogue_synonym() {
        for hint in ["Xbox", "xbox", "Original Xbox", "Microsoft Xbox"] {
            assert_eq!(
                IdentityPlatform::from_catalogue(Some(hint)),
                IdentityPlatform::Xbox,
                "{hint} must resolve to IdentityPlatform::Xbox"
            );
        }
        // These must resolve to Xbox 360, never original Xbox.
        for hint in ["Xbox360", "Xbox 360", "Microsoft Xbox 360"] {
            assert_eq!(
                IdentityPlatform::from_catalogue(Some(hint)),
                IdentityPlatform::Xbox360,
                "{hint} must resolve to IdentityPlatform::Xbox360, not Xbox"
            );
        }
    }

    // ------------------------------------------------------------------
    // Original Xbox disc image (XDVDFS `.iso`/`.xiso`) identity
    // ------------------------------------------------------------------

    /// A minimal, valid XDVDFS disc image with a real, structurally valid
    /// `default.xbe` (via [`xbe_fixture`]) as its only root file.
    fn xbox_disc_image_fixture(title_id: u32) -> Vec<u8> {
        crate::xdvdfs_traversal::test_support::synthetic_single_root_file_image(
            "DEFAULT.XBE",
            &xbe_fixture(title_id),
        )
    }

    #[test]
    fn xbox_disc_image_verifies_title_id_from_structure_not_filename() {
        let directory = FixtureDir::new("xbox-disc-valid");
        let path = write_fixture(
            &directory,
            "Mystery Game.iso",
            &xbox_disc_image_fixture(0x4D53_0058),
        );
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.platform, IdentityPlatform::Xbox);
        assert_eq!(report.format, IdentityImageFormat::XboxDiscImage);
        assert_eq!(report.verified_xbox_title_id(), Some("4D530058"));
        assert!(report.complete);
        // The real, on-disk path is exactly what a later xemu `dvd_path`
        // launch would need - never rewritten, extracted, or discarded.
        assert_eq!(report.archive_path, path);
    }

    #[test]
    fn xbox_disc_image_supports_the_xiso_extension_too() {
        let directory = FixtureDir::new("xbox-disc-xiso-ext");
        let path = write_fixture(
            &directory,
            "game.xiso",
            &xbox_disc_image_fixture(0x4D53_0058),
        );
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.format, IdentityImageFormat::XboxDiscImage);
        assert_eq!(report.verified_xbox_title_id(), Some("4D530058"));
    }

    #[test]
    fn xbox_disc_image_malformed_volume_fails_closed() {
        let directory = FixtureDir::new("xbox-disc-bad-volume");
        let path = write_fixture(&directory, "game.iso", b"not an xdvdfs volume at all");
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert!(!report.complete);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Invalid)
        );
    }

    #[test]
    fn xbox_disc_image_missing_default_xbe_is_missing_not_fabricated() {
        let directory = FixtureDir::new("xbox-disc-no-xbe");
        let image = crate::xdvdfs_traversal::test_support::synthetic_single_root_file_image(
            "README.TXT",
            b"not an xbe",
        );
        let path = write_fixture(&directory, "game.iso", &image);
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert!(!report.complete);
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::XbeTitleId && item.status == IdentityStatus::Missing
        }));
    }

    #[test]
    fn xbox_disc_image_malformed_xbe_content_fails_closed() {
        let directory = FixtureDir::new("xbox-disc-bad-xbe");
        let image = crate::xdvdfs_traversal::test_support::synthetic_single_root_file_image(
            "DEFAULT.XBE",
            b"NOPE not an xbe header",
        );
        let path = write_fixture(&directory, "game.iso", &image);
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert!(!report.complete);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Invalid)
        );
    }

    #[test]
    fn xbox_disc_image_huge_overflow_data_sector_is_refused_not_panicked() {
        // Patch DEFAULT.XBE's own data_sector field (the 4 bytes
        // immediately preceding attrs/name_len/name in the fixed on-disk
        // dirent layout) to u32::MAX - a sector whose byte offset can never
        // be satisfied by this tiny backing file. Must fail closed with a
        // real diagnostic, never panic or silently return garbage.
        let directory = FixtureDir::new("xbox-disc-overflow-sector");
        let mut image = xbox_disc_image_fixture(0x4D53_0058);
        let needle = b"DEFAULT.XBE";
        let name_pos = image
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("DEFAULT.XBE name bytes must be present in the synthetic image");
        let data_sector_offset = name_pos - 10;
        image[data_sector_offset..data_sector_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let path = write_fixture(&directory, "game.iso", &image);
        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert!(!report.complete);
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.status == IdentityStatus::Invalid)
        );
    }

    #[test]
    fn xbox_disc_image_filename_only_content_never_authorizes_identity() {
        // A title-ID-looking filename with no real backing file at all -
        // filename alone must never become verified identity.
        let report =
            inspect_catalogued_game_identity(Path::new("/games/4D530058.iso"), Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), None);
        assert!(!report.complete);
    }

    #[test]
    fn xbox_disc_image_and_xbox_360_never_cross_authorize() {
        // The exact same real, structurally valid Xbox disc image content,
        // trusted as "Xbox 360" instead, must never verify as either
        // platform: Xbox 360 has no `.iso`/`.xiso` dispatch at all (XEX
        // identity is loose-file/ZIP only), so this must fail closed as
        // unsupported rather than silently reinterpreted.
        let directory = FixtureDir::new("xbox-disc-vs-xbox360");
        let path = write_fixture(
            &directory,
            "game.iso",
            &xbox_disc_image_fixture(0x4D53_0058),
        );
        let as_xbox = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(as_xbox.verified_xbox_title_id(), Some("4D530058"));

        let as_xbox360 = inspect_catalogued_game_identity(&path, Some("Xbox 360"));
        assert_eq!(as_xbox360.verified_xbox_title_id(), None);
        assert_eq!(as_xbox360.verified_xex_title_id(), None);
        assert_eq!(as_xbox360.format, IdentityImageFormat::Unsupported);
    }

    #[test]
    fn xbox_disc_image_never_materializes_the_whole_image() {
        // A multi-gigabyte sparse file (a hole - no real disk I/O for the
        // padding) with the real XDVDFS volume placed only at the real XGD1
        // offset, simulating a full, unstripped Redump-style dump. If the
        // identity pipeline ever tried to read the whole file into memory,
        // this would be a multi-gigabyte allocation.
        const XGD1_OFFSET: u64 = 405_798_912;
        let directory = FixtureDir::new("xbox-disc-huge-sparse");
        let path = directory.0.join("game.iso");
        let image = xbox_disc_image_fixture(0x4D53_0058);
        let mut file = fs::File::create(&path).unwrap();
        file.set_len(XGD1_OFFSET + image.len() as u64).unwrap();
        file.seek(SeekFrom::Start(XGD1_OFFSET)).unwrap();
        file.write_all(&image).unwrap();
        drop(file);

        let report = inspect_catalogued_game_identity(&path, Some("Xbox"));
        assert_eq!(report.verified_xbox_title_id(), Some("4D530058"));
        assert!(report.complete);
    }

    fn threedo_fixture(volume_id: u32, root_id: u32, blocks: u32) -> Vec<u8> {
        let mut image = vec![0_u8; 2048];
        image[0] = 1;
        image[1..6].fill(0x5A);
        image[6] = 1;
        image[40..51].copy_from_slice(b"CD-ROM TEST");
        image[72..76].copy_from_slice(&volume_id.to_be_bytes());
        image[76..80].copy_from_slice(&2048_u32.to_be_bytes());
        image[80..84].copy_from_slice(&blocks.to_be_bytes());
        image[84..88].copy_from_slice(&root_id.to_be_bytes());
        image[88..92].copy_from_slice(&1_u32.to_be_bytes());
        image[92..96].copy_from_slice(&2048_u32.to_be_bytes());
        image
    }

    #[test]
    fn threedo_opera_header_provides_verified_structured_identity() {
        let directory = FixtureDir::new("threedo-identity");
        let path = write_fixture(
            &directory,
            "misleading-name.iso",
            &threedo_fixture(0x198E_EB79, 0x1F23_77CF, 1),
        );
        let report = inspect_catalogued_game_identity(&path, Some("3DO"));
        assert_eq!(report.platform, IdentityPlatform::ThreeDo);
        assert_eq!(report.format, IdentityImageFormat::Iso);
        assert_eq!(
            report.verified_threedo_disc_id(),
            Some("VOL198EEB79-ROOT1F2377CF-BLOCKS00000001")
        );
        assert!(report.complete);
    }

    #[test]
    fn threedo_filename_and_human_label_are_not_identity_authority() {
        let directory = FixtureDir::new("threedo-renamed");
        let path = write_fixture(
            &directory,
            "Totally-Wrong-Title.iso",
            &threedo_fixture(0x1234_5678, 0x9ABC_DEF0, 1),
        );
        let report = inspect_catalogued_game_identity(&path, Some("Panasonic 3DO"));
        assert_eq!(
            report.verified_threedo_disc_id(),
            Some("VOL12345678-ROOT9ABCDEF0-BLOCKS00000001")
        );
        assert!(!report.evidence.iter().any(|evidence| {
            evidence.status == IdentityStatus::Verified
                && evidence.confidence == IdentityConfidence::FilenameOnly
        }));
    }

    #[test]
    fn ordinary_iso_and_truncated_opera_headers_fail_closed() {
        let directory = FixtureDir::new("threedo-invalid");
        let ordinary = write_fixture(&directory, "ordinary.iso", &[0_u8; 2048]);
        let ordinary_report = inspect_catalogued_game_identity(&ordinary, Some("3DO"));
        assert_eq!(ordinary_report.verified_threedo_disc_id(), None);
        assert!(!ordinary_report.complete);

        let truncated = write_fixture(&directory, "truncated.iso", &[1_u8; 95]);
        let truncated_report = inspect_catalogued_game_identity(&truncated, Some("3DO"));
        assert_eq!(truncated_report.verified_threedo_disc_id(), None);
        assert!(!truncated_report.complete);
    }

    #[test]
    fn threedo_mode1_2048_cue_uses_the_same_verified_header_path() {
        let directory = FixtureDir::new("threedo-cue");
        fs::write(
            directory.0.join("data with spaces.bin"),
            threedo_fixture(0x1020_3040, 0x5060_7080, 1),
        )
        .unwrap();
        let cue = write_fixture(
            &directory,
            "renamed.cue",
            b"FILE \"data with spaces.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        );
        let report = inspect_catalogued_game_identity(&cue, Some("3DO"));
        assert_eq!(report.format, IdentityImageFormat::Iso);
        assert_eq!(
            report.verified_threedo_disc_id(),
            Some("VOL10203040-ROOT50607080-BLOCKS00000001")
        );
        assert!(report.complete);
    }

    #[test]
    fn threedo_chd_verifies_the_same_disc_id_as_the_raw_disc() {
        let directory = FixtureDir::new("threedo-chd-parity");
        let image = threedo_fixture(0x198E_EB79, 0x1F23_77CF, 1);

        let iso_path = write_fixture(&directory, "raw-disc.iso", &image);
        let raw = inspect_catalogued_game_identity(&iso_path, Some("3DO"));
        assert_eq!(raw.format, IdentityImageFormat::Iso);
        assert_eq!(
            raw.verified_threedo_disc_id(),
            Some("VOL198EEB79-ROOT1F2377CF-BLOCKS00000001")
        );
        assert!(raw.complete);

        // Deliberately misleading name and extension: identity must come
        // from the OperaFS volume header inside the decoded CHD track.
        let chd_path = write_fixture(
            &directory,
            "Definitely A PS1 Game (USA).chd",
            &threedo_chd(&image),
        );
        let chd = inspect_catalogued_game_identity(&chd_path, Some("Panasonic 3DO"));
        assert_eq!(chd.platform, IdentityPlatform::ThreeDo);
        assert_eq!(chd.format, IdentityImageFormat::Chd);
        assert_eq!(
            chd.verified_threedo_disc_id(),
            raw.verified_threedo_disc_id()
        );
        assert!(chd.complete);
        assert!(!chd.evidence.iter().any(|evidence| {
            evidence.kind == IdentityKind::ThreeDoDiscId
                && evidence.confidence == IdentityConfidence::FilenameOnly
        }));
    }

    #[test]
    fn threedo_chd_hint_over_a_non_threedo_disc_fails_closed() {
        let directory = FixtureDir::new("threedo-chd-wrong-platform");
        // A genuine, openable ISO 9660 PS1 CHD, asked for as 3DO: the
        // OperaFS header validation at logical offset 0 must reject it
        // rather than emit a bogus disc id.
        let ps1 = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let chd_path = write_fixture(&directory, "mislabelled.chd", &ps1_chd(&ps1));
        let report = inspect_catalogued_game_identity(&chd_path, Some("3DO"));
        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert_eq!(report.verified_threedo_disc_id(), None);
        assert!(!report.complete);
    }

    #[test]
    fn malformed_or_truncated_threedo_chd_fails_closed() {
        let directory = FixtureDir::new("threedo-chd-malformed");

        // Not a CHD container at all.
        let not_chd = write_fixture(&directory, "garbage.chd", &[0xAA_u8; 512]);
        let report = inspect_catalogued_game_identity(&not_chd, Some("3DO"));
        assert_eq!(report.verified_threedo_disc_id(), None);
        assert!(!report.complete);

        // A real CHD whose single decoded sector is not an OperaFS header.
        let empty_track = write_fixture(&directory, "blank.chd", &threedo_chd(&[0_u8; 2048]));
        let report = inspect_catalogued_game_identity(&empty_track, Some("3DO"));
        assert_eq!(report.verified_threedo_disc_id(), None);
        assert!(!report.complete);

        // A CHD file truncated inside its own header.
        let mut truncated = threedo_chd(&threedo_fixture(0x1234_5678, 0x9ABC_DEF0, 1));
        truncated.truncate(60);
        let truncated_path = write_fixture(&directory, "truncated.chd", &truncated);
        let report = inspect_catalogued_game_identity(&truncated_path, Some("3DO"));
        assert_eq!(report.verified_threedo_disc_id(), None);
        assert!(!report.complete);
    }

    #[test]
    fn adding_threedo_chd_dispatch_leaves_neighbouring_platforms_unchanged() {
        let directory = FixtureDir::new("threedo-chd-neighbours");

        let saturn = write_fixture(&directory, "s.chd", &ps1_chd(&saturn_iso(b"T-7101G")));
        assert_eq!(
            inspect_game_identity(&saturn, Some("Saturn")).verified_saturn_product_number(),
            Some("T-7101G")
        );

        let dreamcast = write_fixture(&directory, "d.chd", &ps1_chd(&dreamcast_iso(b"T-8109N")));
        assert_eq!(
            inspect_game_identity(&dreamcast, Some("Dreamcast")).verified_dreamcast_product_code(),
            Some("T-8109N")
        );

        let sega_cd = write_fixture(
            &directory,
            "m.chd",
            &ps1_chd(&sega_cd_iso(b"GM T-12345 -00")),
        );
        assert_eq!(
            inspect_game_identity(&sega_cd, Some("Sega CD")).verified_sega_cd_product_code(),
            Some("GM T-12345-00")
        );

        let ps1 = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let ps1_path = write_fixture(&directory, "p.chd", &ps1_chd(&ps1));
        assert_eq!(
            inspect_game_identity(&ps1_path, Some("PlayStation")).verified_ps1_serial(),
            Some("SLUS-12345")
        );
    }

    fn pcfx_fixture(boot_sector: u32, boot_sector_count: u32, seed: u8) -> Vec<u8> {
        let end_sector = boot_sector
            .checked_add(boot_sector_count)
            .expect("synthetic fixture geometry must fit");
        let mut image = vec![0_u8; end_sector as usize * PCFX_BOOT_SECTOR_BYTES];
        image[..PCFX_PRIMARY_MAGIC.len()].copy_from_slice(PCFX_PRIMARY_MAGIC);
        let volume = PCFX_BOOT_SECTOR_BYTES;
        image[volume + 32..volume + 36].copy_from_slice(&boot_sector.to_le_bytes());
        image[volume + 36..volume + 40].copy_from_slice(&boot_sector_count.to_le_bytes());
        for (index, byte) in image[boot_sector as usize * PCFX_BOOT_SECTOR_BYTES..]
            .iter_mut()
            .enumerate()
        {
            *byte = seed.wrapping_add(index as u8);
        }
        image
    }

    #[test]
    fn pcfx_disc_hash_verifies_from_disc_content_not_filename() {
        let directory = FixtureDir::new("pcfx-identity");
        let path = write_fixture(
            &directory,
            "misleading-title.iso",
            &pcfx_fixture(2, 2, 0x31),
        );
        let report = inspect_catalogued_game_identity(&path, Some("PC-FX"));
        assert_eq!(report.platform, IdentityPlatform::Pcfx);
        assert_eq!(report.format, IdentityImageFormat::Iso);
        assert!(report.verified_pcfx_disc_hash().is_some());
        assert!(report.complete);
        assert!(!report.evidence.iter().any(|evidence| {
            evidence.kind == IdentityKind::PcfxDiscHash
                && evidence.confidence == IdentityConfidence::FilenameOnly
        }));
    }

    #[test]
    fn pcfx_renamed_copy_has_the_same_verified_identity() {
        let directory = FixtureDir::new("pcfx-renamed");
        let bytes = pcfx_fixture(2, 1, 0x52);
        let first = write_fixture(&directory, "first.iso", &bytes);
        let second = write_fixture(&directory, "deliberately-wrong-name.iso", &bytes);
        let first_report = inspect_catalogued_game_identity(&first, Some("NEC PC-FX"));
        let second_report = inspect_catalogued_game_identity(&second, Some("PCFX"));
        assert_eq!(
            first_report.verified_pcfx_disc_hash(),
            second_report.verified_pcfx_disc_hash()
        );
    }

    #[test]
    fn pcfx_mode1_2048_cue_supports_spaces_and_unicode_paths() {
        let directory = FixtureDir::new("pcfx-cue-unicode");
        let bin_name = "données with spaces.bin";
        fs::write(directory.0.join(bin_name), pcfx_fixture(2, 1, 0x63)).unwrap();
        let cue = write_fixture(
            &directory,
            "renamed-disc.cue",
            format!("FILE \"{bin_name}\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n")
                .as_bytes(),
        );
        let report = inspect_catalogued_game_identity(&cue, Some("PC-FX"));
        assert!(report.verified_pcfx_disc_hash().is_some());
        assert!(report.complete);
    }

    #[test]
    fn pcfx_invalid_or_unrelated_content_fails_closed() {
        let directory = FixtureDir::new("pcfx-invalid");
        let ordinary = write_fixture(&directory, "ordinary.iso", &[0_u8; 4096]);
        assert_eq!(
            inspect_catalogued_game_identity(&ordinary, Some("PC-FX")).verified_pcfx_disc_hash(),
            None
        );

        let truncated = write_fixture(&directory, "truncated.iso", &[0_u8; 2048]);
        assert_eq!(
            inspect_catalogued_game_identity(&truncated, Some("PC-FX")).verified_pcfx_disc_hash(),
            None
        );

        let valid = write_fixture(&directory, "valid.iso", &pcfx_fixture(2, 1, 0x74));
        let wrong_platform = inspect_catalogued_game_identity(&valid, Some("PlayStation"));
        assert_eq!(wrong_platform.verified_pcfx_disc_hash(), None);
    }

    #[test]
    fn pcfx_missing_or_invalid_boot_geometry_fails_closed() {
        let directory = FixtureDir::new("pcfx-bad-geometry");
        let mut missing_count = pcfx_fixture(2, 1, 0x85);
        missing_count[PCFX_BOOT_SECTOR_BYTES + 36..PCFX_BOOT_SECTOR_BYTES + 40]
            .copy_from_slice(&0_u32.to_le_bytes());
        let path = write_fixture(&directory, "missing-count.iso", &missing_count);
        let report = inspect_catalogued_game_identity(&path, Some("PC-FX"));
        assert_eq!(report.verified_pcfx_disc_hash(), None);
        assert!(!report.complete);
    }

    /// A PC-FX disc image that is *also* a minimal valid ISO 9660 volume, so
    /// it can be wrapped by [`ps1_chd`] and read back through the shared
    /// `open_chd_iso9660` path exactly as a real PC-FX disc would be. The
    /// PC-FX identity bytes (primary magic at sector 0, volume header at
    /// sector 1, header-directed boot code) sit ahead of the ISO 9660 PVD at
    /// sector 16, just as they do on a real disc.
    fn pcfx_iso9660_fixture(boot_sector: u32, boot_sector_count: u32, seed: u8) -> Vec<u8> {
        const SECTORS: usize = 32;
        let mut iso = vec![0_u8; SECTORS * ISO_SECTOR_SIZE as usize];

        iso[..PCFX_PRIMARY_MAGIC.len()].copy_from_slice(PCFX_PRIMARY_MAGIC);
        let volume = PCFX_BOOT_SECTOR_BYTES;
        iso[volume + 32..volume + 36].copy_from_slice(&boot_sector.to_le_bytes());
        iso[volume + 36..volume + 40].copy_from_slice(&boot_sector_count.to_le_bytes());
        let boot_start = boot_sector as usize * PCFX_BOOT_SECTOR_BYTES;
        let boot_end = boot_start + boot_sector_count as usize * PCFX_BOOT_SECTOR_BYTES;
        for (index, byte) in iso[boot_start..boot_end].iter_mut().enumerate() {
            *byte = seed.wrapping_add(index as u8);
        }

        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        iso[pvd + 128..pvd + 130].copy_from_slice(&2048_u16.to_le_bytes());
        iso[pvd + 130..pvd + 132].copy_from_slice(&2048_u16.to_be_bytes());
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        iso[terminator] = 255;
        iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        iso[terminator + 6] = 1;
        iso
    }

    /// Replaces the first occurrence of `needle` with `replacement` in place.
    /// Used to corrupt a single field of an otherwise-valid CHD metadata
    /// string without changing its byte length.
    fn replace_once_in_place(haystack: &mut [u8], needle: &[u8], replacement: &[u8]) {
        assert_eq!(needle.len(), replacement.len());
        let position = haystack
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("needle must be present");
        haystack[position..position + needle.len()].copy_from_slice(replacement);
    }

    #[test]
    fn pcfx_chd_reaches_real_identity_inspection_instead_of_deferred() {
        let directory = FixtureDir::new("pcfx-chd-valid");
        let image = pcfx_iso9660_fixture(2, 2, 0x40);

        let raw_path = write_fixture(&directory, "raw-disc.iso", &image);
        let raw = inspect_catalogued_game_identity(&raw_path, Some("PC-FX"));
        assert_eq!(raw.format, IdentityImageFormat::Iso);
        assert!(raw.verified_pcfx_disc_hash().is_some());
        assert!(raw.complete);

        // Deliberately misleading name: identity must come from the decoded
        // CHD track content, never the filename.
        let chd_path = write_fixture(&directory, "Totally A PS1 Game (USA).chd", &ps1_chd(&image));
        let chd = inspect_catalogued_game_identity(&chd_path, Some("PC-FX"));
        assert_eq!(chd.platform, IdentityPlatform::Pcfx);
        assert_eq!(
            chd.format,
            IdentityImageFormat::Chd,
            "a PC-FX .chd must no longer fall through to Deferred"
        );
        assert_ne!(chd.format, IdentityImageFormat::Deferred);
        assert_eq!(chd.verified_pcfx_disc_hash(), raw.verified_pcfx_disc_hash());
        assert!(chd.complete);
        assert!(
            chd.evidence.iter().any(|evidence| {
                evidence.kind == IdentityKind::PcfxDiscHash
                    && evidence.status == IdentityStatus::Verified
            }),
            "the existing PcfxDiscHash evidence must still appear for a CHD"
        );
        assert!(!chd.evidence.iter().any(|evidence| {
            evidence.kind == IdentityKind::PcfxDiscHash
                && evidence.confidence == IdentityConfidence::FilenameOnly
        }));
    }

    #[test]
    fn pcfx_chd_hint_over_non_pcfx_iso_contents_fails_closed() {
        // A genuine, openable ISO 9660 PS1 CHD asked for as PC-FX: sector 0
        // carries no PC-FX boot magic, so `inspect_pcfx_source` must reject
        // it rather than emit a bogus disc hash.
        let directory = FixtureDir::new("pcfx-chd-wrong-contents");
        let ps1 = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let path = write_fixture(&directory, "mislabelled.chd", &ps1_chd(&ps1));
        let report = inspect_catalogued_game_identity(&path, Some("PC-FX"));
        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert_eq!(report.verified_pcfx_disc_hash(), None);
        assert!(!report.complete);
    }

    #[test]
    fn an_unrelated_chd_with_a_pcfx_like_filename_is_not_identified_as_pcfx() {
        let directory = FixtureDir::new("pcfx-chd-filename-lie");
        let ps1 = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let path = write_fixture(
            &directory,
            "Zenki FX - Vajra Fight (Japan) PC-FX.chd",
            &ps1_chd(&ps1),
        );

        // Asked as PC-FX: the PS1 contents have no PC-FX boot magic.
        let as_pcfx = inspect_catalogued_game_identity(&path, Some("PC-FX"));
        assert_eq!(as_pcfx.verified_pcfx_disc_hash(), None);
        assert!(!as_pcfx.complete);

        // Asked as PlayStation: still a normal PS1 CHD, unaffected by the
        // PC-FX-flavoured filename.
        let as_ps1 = inspect_catalogued_game_identity(&path, Some("PlayStation"));
        assert_eq!(as_ps1.verified_ps1_serial(), Some("SLUS-12345"));
        assert!(as_ps1.verified_pcfx_disc_hash().is_none());
    }

    #[test]
    fn malformed_or_truncated_pcfx_chd_fails_closed() {
        let directory = FixtureDir::new("pcfx-chd-malformed");

        // Not a CHD container at all.
        let garbage = write_fixture(&directory, "garbage.chd", &[0xAB_u8; 4096]);
        let report = inspect_catalogued_game_identity(&garbage, Some("PC-FX"));
        assert_eq!(report.verified_pcfx_disc_hash(), None);
        assert!(!report.complete);

        // A real CHD whose decoded track is not ISO 9660 - `open_chd_iso9660`
        // refuses it, so PC-FX stays unsupported rather than guessed.
        let non_iso = write_fixture(
            &directory,
            "raw-track.chd",
            &uncompressed_mode1_raw_chd(&pcfx_fixture(2, 2, 0x51)),
        );
        let report = inspect_catalogued_game_identity(&non_iso, Some("PC-FX"));
        assert_eq!(report.verified_pcfx_disc_hash(), None);
        assert!(!report.complete);

        // A CHD file truncated inside its own header.
        let mut truncated = ps1_chd(&pcfx_iso9660_fixture(2, 2, 0x52));
        truncated.truncate(48);
        let truncated_path = write_fixture(&directory, "truncated.chd", &truncated);
        let report = inspect_catalogued_game_identity(&truncated_path, Some("PC-FX"));
        assert_eq!(report.verified_pcfx_disc_hash(), None);
        assert!(!report.complete);
    }

    #[test]
    fn pcfx_chd_with_an_unsupported_track_position_or_pregap_stays_unsupported() {
        let directory = FixtureDir::new("pcfx-chd-track-limits");
        let image = pcfx_iso9660_fixture(2, 2, 0x60);

        let mut non_track_one = ps1_chd(&image);
        replace_once_in_place(&mut non_track_one, b"TRACK:1 ", b"TRACK:2 ");
        let path = write_fixture(&directory, "track2.chd", &non_track_one);
        let report = inspect_catalogued_game_identity(&path, Some("PC-FX"));
        assert_eq!(report.verified_pcfx_disc_hash(), None);
        assert!(!report.complete);

        let mut non_zero_pregap = ps1_chd(&image);
        replace_once_in_place(&mut non_zero_pregap, b"PREGAP:0 ", b"PREGAP:9 ");
        let path = write_fixture(&directory, "pregap.chd", &non_zero_pregap);
        let report = inspect_catalogued_game_identity(&path, Some("PC-FX"));
        assert_eq!(report.verified_pcfx_disc_hash(), None);
        assert!(!report.complete);
    }

    #[test]
    fn adding_pcfx_chd_dispatch_leaves_neighbouring_optical_platforms_unchanged() {
        let directory = FixtureDir::new("pcfx-chd-neighbours");

        let saturn = write_fixture(&directory, "s.chd", &ps1_chd(&saturn_iso(b"T-7101G")));
        assert_eq!(
            inspect_game_identity(&saturn, Some("Saturn")).verified_saturn_product_number(),
            Some("T-7101G")
        );

        let dreamcast = write_fixture(&directory, "d.chd", &ps1_chd(&dreamcast_iso(b"T-8109N")));
        assert_eq!(
            inspect_game_identity(&dreamcast, Some("Dreamcast")).verified_dreamcast_product_code(),
            Some("T-8109N")
        );

        let sega_cd = write_fixture(
            &directory,
            "m.chd",
            &ps1_chd(&sega_cd_iso(b"GM T-12345 -00")),
        );
        assert_eq!(
            inspect_game_identity(&sega_cd, Some("Sega CD")).verified_sega_cd_product_code(),
            Some("GM T-12345-00")
        );

        let ps1 = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let ps1_path = write_fixture(&directory, "p.chd", &ps1_chd(&ps1));
        assert_eq!(
            inspect_game_identity(&ps1_path, Some("PlayStation")).verified_ps1_serial(),
            Some("SLUS-12345")
        );

        let threedo = write_fixture(
            &directory,
            "t.chd",
            &threedo_chd(&threedo_fixture(0x1111_2222, 0x3333_4444, 1)),
        );
        assert!(
            inspect_game_identity(&threedo, Some("3DO"))
                .verified_threedo_disc_id()
                .is_some()
        );
    }

    #[test]
    fn a_pcfx_chd_never_infers_the_platform_from_the_extension_alone() {
        // No platform hint at all: a `.chd` must not become PC-FX just
        // because its bytes happen to be a PC-FX disc.
        let directory = FixtureDir::new("pcfx-chd-no-hint");
        let path = write_fixture(
            &directory,
            "game.chd",
            &ps1_chd(&pcfx_iso9660_fixture(2, 2, 0x70)),
        );
        let report = inspect_catalogued_game_identity(&path, None);
        assert_ne!(report.platform, IdentityPlatform::Pcfx);
        assert_eq!(report.verified_pcfx_disc_hash(), None);
    }

    // --- PC Engine CD / TurboGrafx-CD ---------------------------------
    fn pce_cd_image(start_sector: u32, count: u8, seed: u8) -> Vec<u8> {
        use crate::pcengine_cd_boot_evidence::{PCE_CD_SIGNATURE, PCE_CD_SIGNATURE_OFFSET};
        assert!(start_sector >= 2);
        let total_sectors = (start_sector as usize + count as usize).max(8);
        let mut image = vec![0_u8; total_sectors * 2048];
        let ipl = 2048;
        let start = start_sector.to_be_bytes();
        image[ipl..ipl + 4].copy_from_slice(&[start[1], start[2], start[3], count]);
        image
            [ipl + PCE_CD_SIGNATURE_OFFSET..ipl + PCE_CD_SIGNATURE_OFFSET + PCE_CD_SIGNATURE.len()]
            .copy_from_slice(PCE_CD_SIGNATURE);
        image[ipl + 106..ipl + 122].copy_from_slice(b"MISLEADING TITLE");
        for (index, byte) in image[start_sector as usize * 2048..].iter_mut().enumerate() {
            *byte = seed.wrapping_add(index as u8);
        }
        image
    }

    fn pce_cd_chd(image: &[u8]) -> Vec<u8> {
        uncompressed_mode1_raw_chd(image)
    }

    #[test]
    fn pce_cd_raw_iso_cue_and_supported_chd_reach_structural_identity() {
        let directory = FixtureDir::new("pce-cd-valid");
        let image = pce_cd_image(2, 4, 0x40);
        let iso = write_fixture(&directory, "Some Turbo Game.iso", &image);
        let iso_report = inspect_catalogued_game_identity(&iso, Some("PC Engine CD"));
        assert_eq!(iso_report.platform, IdentityPlatform::PcEngineCd);
        assert_eq!(
            iso_report.verified_pcengine_cd_boot_structure(),
            Some("PC Engine CD-ROM SYSTEM")
        );
        assert!(iso_report.complete);

        fs::write(directory.0.join("disc.bin"), &image).unwrap();
        let cue = write_fixture(
            &directory,
            "renamed.cue",
            b"FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        );
        let cue_report = inspect_catalogued_game_identity(&cue, Some("TurboGrafx-CD"));
        assert_eq!(
            cue_report.verified_pcengine_cd_boot_structure(),
            Some("PC Engine CD-ROM SYSTEM")
        );

        let chd = write_fixture(&directory, "disc.chd", &pce_cd_chd(&image));
        let chd_report = inspect_catalogued_game_identity(&chd, Some("PC Engine CD"));
        assert_eq!(
            chd_report.verified_pcengine_cd_boot_structure(),
            Some("PC Engine CD-ROM SYSTEM")
        );
    }

    #[test]
    fn pce_cd_invalid_content_fails_closed_and_never_uses_filename() {
        let directory = FixtureDir::new("pce-cd-invalid");
        let random = write_fixture(&directory, "Ys PC Engine CD.iso", &vec![0x5A_u8; 64 * 1024]);
        let report = inspect_catalogued_game_identity(&random, Some("PC Engine CD"));
        assert_eq!(report.verified_pcengine_cd_boot_structure(), None);
        assert!(!report.complete);

        let mut image = pce_cd_image(2, 2, 0x11);
        image[2048 + 3] = 250;
        let path = write_fixture(&directory, "oob.iso", &image);
        let report = inspect_catalogued_game_identity(&path, Some("PC Engine CD"));
        assert_eq!(report.verified_pcengine_cd_boot_structure(), None);
        assert!(!report.complete);
    }

    #[test]
    fn other_optical_signatures_do_not_cross_resolve_to_pc_engine_cd() {
        let directory = FixtureDir::new("pce-cd-cross-platform");
        for (name, bytes) in [
            ("saturn.iso", saturn_iso(b"T-7101G")),
            ("segacd.iso", sega_cd_iso(b"GM T-12345 -00")),
            ("pcfx.iso", pcfx_fixture(2, 2, 0x31)),
            (
                "ps1.iso",
                ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true),
            ),
        ] {
            let path = write_fixture(&directory, name, &bytes);
            let report = inspect_catalogued_game_identity(&path, Some("PC Engine CD"));
            assert_eq!(report.verified_pcengine_cd_boot_structure(), None, "{name}");
            assert!(!report.complete, "{name}");
        }
    }

    // --- Neo Geo CD ------------------------------------------------------

    /// A minimal but genuine ISO 9660 image whose root directory carries an
    /// `IPL.TXT` file with `ipl` as its exact contents - the same fixed
    /// sector layout as [`ps1_iso`] (PVD at 16, terminator at 17, root dir
    /// at 20, one file at 21).
    fn neogeocd_iso(ipl: &[u8]) -> Vec<u8> {
        const SECTORS: usize = 24;
        let mut iso = vec![0_u8; SECTORS * ISO_SECTOR_SIZE as usize];
        let pvd = 16 * ISO_SECTOR_SIZE as usize;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 6] = 1;
        let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let terminator = 17 * ISO_SECTOR_SIZE as usize;
        iso[terminator] = 255;
        iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
        iso[terminator + 6] = 1;

        let root_offset = 20 * ISO_SECTOR_SIZE as usize;
        let ipl_record = directory_record(b"IPL.TXT;1", 21, ipl.len() as u32, false);
        iso[root_offset..root_offset + ipl_record.len()].copy_from_slice(&ipl_record);
        iso[root_offset + ipl_record.len()] = 0;
        let ipl_offset = 21 * ISO_SECTOR_SIZE as usize;
        iso[ipl_offset..ipl_offset + ipl.len()].copy_from_slice(ipl);
        iso
    }

    fn valid_ipl_txt() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MAIN.PRG,0,00000000\r\n");
        bytes.extend_from_slice(b"MAIN.FIX,0,00010000\r\n");
        bytes.extend_from_slice(b"MAIN.SPR,1,00000000\r\n");
        bytes.extend_from_slice(b"MAIN.Z80,0,00000000\r\n");
        bytes.extend_from_slice(b"MAIN.PCM,2,00000000\r\n");
        bytes.push(0x1A);
        bytes
    }

    fn a78_image(payload: &[u8], declared_size: u32) -> Vec<u8> {
        let mut bytes = vec![0_u8; A78_HEADER_BYTES + payload.len()];
        bytes[1..1 + b"ATARI7800".len()].copy_from_slice(b"ATARI7800");
        bytes[0x11..0x11 + 4].copy_from_slice(b"Test");
        bytes[0x31..0x35].copy_from_slice(&declared_size.to_be_bytes());
        bytes[0x35..0x37].copy_from_slice(&1_u16.to_be_bytes());
        bytes[0x37] = 1;
        bytes[0x39] = 0;
        bytes[A78_HEADER_BYTES..].copy_from_slice(payload);
        bytes
    }

    fn lnx_image(payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0_u8; LYNX_HEADER_BYTES + payload.len()];
        bytes[..4].copy_from_slice(b"LYNX");
        bytes[4..6].copy_from_slice(&256_u16.to_le_bytes());
        bytes[6..8].copy_from_slice(&0_u16.to_le_bytes());
        bytes[8..10].copy_from_slice(&1_u16.to_le_bytes());
        bytes[0x0A..0x0A + 4].copy_from_slice(b"Test");
        bytes[LYNX_HEADER_BYTES..].copy_from_slice(payload);
        bytes
    }

    #[test]
    fn neogeocd_iso_cue_and_chd_verify_the_ipl_txt_boot_structure() {
        let directory = FixtureDir::new("neogeocd-valid");
        let ipl = valid_ipl_txt();
        let image = neogeocd_iso(&ipl);

        let iso = write_fixture(&directory, "Totally Unrelated Title.iso", &image);
        let iso_report = inspect_catalogued_game_identity(&iso, Some("Neo Geo CD"));
        assert_eq!(iso_report.platform, IdentityPlatform::NeoGeoCd);
        assert_eq!(
            iso_report.verified_neogeocd_boot_structure(),
            Some("IPL.TXT")
        );
        assert!(iso_report.complete);

        fs::write(directory.0.join("disc.bin"), &image).unwrap();
        let cue = write_fixture(
            &directory,
            "renamed.cue",
            b"FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        );
        let cue_report = inspect_catalogued_game_identity(&cue, Some("Neo Geo CD"));
        assert_eq!(
            cue_report.verified_neogeocd_boot_structure(),
            Some("IPL.TXT")
        );
        assert!(cue_report.complete);

        let chd = write_fixture(&directory, "disc.chd", &ps1_chd(&image));
        let chd_report = inspect_catalogued_game_identity(&chd, Some("Neo Geo CD"));
        assert_eq!(chd_report.format, IdentityImageFormat::Chd);
        assert_eq!(
            chd_report.verified_neogeocd_boot_structure(),
            Some("IPL.TXT")
        );
        assert!(chd_report.complete);
    }

    #[test]
    fn neogeocd_identity_is_structural_only_never_a_serial_or_dat_release() {
        let directory = FixtureDir::new("neogeocd-structural-only");
        let image = neogeocd_iso(&valid_ipl_txt());
        let iso = write_fixture(&directory, "game.iso", &image);
        let report = inspect_catalogued_game_identity(&iso, Some("Neo Geo CD"));
        // The only verified content-derived fact is the structural boot
        // marker (`Platform` is catalogue context, not read from bytes); no
        // serial, product code, or title is ever asserted from content.
        let verified_from_content: Vec<_> = report
            .evidence
            .iter()
            .filter(|e| e.status == IdentityStatus::Verified && e.kind != IdentityKind::Platform)
            .map(|e| e.kind)
            .collect();
        assert_eq!(
            verified_from_content,
            vec![IdentityKind::NeoGeoCdBootStructure]
        );
        assert_eq!(report.verified_neogeocd_boot_structure(), Some("IPL.TXT"));
        // No serial/product-code identity kind is present at all.
        assert!(!report.evidence.iter().any(|e| matches!(
            e.kind,
            IdentityKind::Ps1Serial
                | IdentityKind::SegaCdProductCode
                | IdentityKind::SaturnProductNumber
                | IdentityKind::PceCdBootStructure
        )));
    }

    #[test]
    fn neogeocd_malformed_ipl_txt_fails_closed() {
        let directory = FixtureDir::new("neogeocd-malformed-ipl");
        // Present, named IPL.TXT, but no terminator byte and unparseable
        // lines - structurally invalid.
        let image = neogeocd_iso(b"this is not a load manifest at all\r\nnope\r\n");
        let iso = write_fixture(&directory, "game.iso", &image);
        let report = inspect_catalogued_game_identity(&iso, Some("Neo Geo CD"));
        assert_eq!(report.verified_neogeocd_boot_structure(), None);
        assert!(!report.complete);
        assert!(report.evidence.iter().any(|e| {
            e.kind == IdentityKind::NeoGeoCdBootStructure && e.status == IdentityStatus::Invalid
        }));
    }

    #[test]
    fn neogeocd_filename_only_ipl_txt_does_not_prove_the_platform() {
        let directory = FixtureDir::new("neogeocd-filename-only");
        // A disc whose IPL.TXT contents are just its own name repeated -
        // the filename convention alone is never structural proof.
        let image = neogeocd_iso(b"IPL.TXT IPL.TXT IPL.TXT");
        let iso = write_fixture(&directory, "Neo Geo CD (Japan) IPL.TXT.iso", &image);
        let report = inspect_catalogued_game_identity(&iso, Some("Neo Geo CD"));
        assert_eq!(report.verified_neogeocd_boot_structure(), None);
        assert!(!report.complete);
    }

    #[test]
    fn neogeocd_missing_ipl_txt_fails_closed() {
        let directory = FixtureDir::new("neogeocd-missing-ipl");
        // A real ISO 9660 disc (a PS1 one) with no IPL.TXT in its root.
        let image = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\r\n", true);
        let iso = write_fixture(&directory, "not-neogeo.iso", &image);
        let report = inspect_catalogued_game_identity(&iso, Some("Neo Geo CD"));
        assert_eq!(report.verified_neogeocd_boot_structure(), None);
        assert!(!report.complete);
        assert!(report.evidence.iter().any(|e| {
            e.kind == IdentityKind::NeoGeoCdBootStructure && e.status == IdentityStatus::Missing
        }));
    }

    #[test]
    fn neogeocd_truncated_or_non_chd_bytes_refuse_never_verified() {
        let directory = FixtureDir::new("neogeocd-bad-chd");

        // Not a CHD container at all.
        let garbage = write_fixture(&directory, "garbage.chd", &[0xAB_u8; 4096]);
        let report = inspect_catalogued_game_identity(&garbage, Some("Neo Geo CD"));
        assert_eq!(report.format, IdentityImageFormat::Chd);
        assert_eq!(report.verified_neogeocd_boot_structure(), None);
        assert!(!report.complete);

        // A real CHD truncated inside its own header.
        let mut truncated = ps1_chd(&neogeocd_iso(&valid_ipl_txt()));
        truncated.truncate(60);
        let truncated_path = write_fixture(&directory, "truncated.chd", &truncated);
        let report = inspect_catalogued_game_identity(&truncated_path, Some("Neo Geo CD"));
        assert_eq!(report.verified_neogeocd_boot_structure(), None);
        assert!(!report.complete);
    }

    #[test]
    fn neogeocd_dispatch_leaves_sega_cd_identity_unchanged() {
        let directory = FixtureDir::new("neogeocd-segacd-regression");

        // A genuine Sega CD disc, hinted as Sega CD, still verifies its own
        // product code - the new Neo Geo CD arm never intercepts it.
        let segacd = write_fixture(&directory, "segacd.iso", &sega_cd_iso(b"GM T-12345 -00"));
        let segacd_report = inspect_catalogued_game_identity(&segacd, Some("Sega CD"));
        assert_eq!(segacd_report.platform, IdentityPlatform::SegaCd);
        assert!(segacd_report.verified_sega_cd_product_code().is_some());

        // A Neo Geo CD disc mis-hinted as Sega CD carries no SEGADISCSYSTEM
        // boot sector, so it fails closed rather than cross-resolving.
        let ngcd = write_fixture(&directory, "ngcd.iso", &neogeocd_iso(&valid_ipl_txt()));
        let ngcd_as_segacd = inspect_catalogued_game_identity(&ngcd, Some("Sega CD"));
        assert!(ngcd_as_segacd.verified_sega_cd_product_code().is_none());
        assert!(!ngcd_as_segacd.complete);

        // And a Sega CD disc mis-hinted as Neo Geo CD has no IPL.TXT, so it
        // also fails closed.
        let segacd_as_ngcd = inspect_catalogued_game_identity(&segacd, Some("Neo Geo CD"));
        assert_eq!(segacd_as_ngcd.verified_neogeocd_boot_structure(), None);
        assert!(!segacd_as_ngcd.complete);
    }

    #[test]
    fn neogeocd_generic_iso_without_ipl_txt_stays_ambiguous_not_neogeocd() {
        let directory = FixtureDir::new("neogeocd-generic-iso");
        // An ISO 9660 image with only a SYSTEM.CNF-less unrelated file:
        // generic optical content, no Neo Geo CD proof.
        let image = ps2_iso(b"BOOT2=cdrom0:\\UNUSED;1\r\n", false, None);
        let iso = write_fixture(&directory, "generic.iso", &image);
        let report = inspect_catalogued_game_identity(&iso, Some("Neo Geo CD"));
        assert_eq!(report.verified_neogeocd_boot_structure(), None);
        assert!(!report.complete);
    }

    #[test]
    fn neogeocd_never_inferred_from_extension_without_a_platform_hint() {
        let directory = FixtureDir::new("neogeocd-no-hint");
        let iso = write_fixture(&directory, "game.iso", &neogeocd_iso(&valid_ipl_txt()));
        // No catalogue platform: a `.iso` must not become Neo Geo CD just
        // because it happens to contain an IPL.TXT.
        let report = inspect_game_identity(&iso, None);
        assert_ne!(report.platform, IdentityPlatform::NeoGeoCd);
        assert_eq!(report.verified_neogeocd_boot_structure(), None);
    }

    #[test]
    fn atari7800_headered_rom_emits_structural_and_canonical_identity() {
        let directory = FixtureDir::new("atari7800-loose");
        let payload = b"a78 payload";
        let bytes = a78_image(payload, payload.len() as u32);
        let path = write_fixture(&directory, "title.a78", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Atari7800"));

        assert_eq!(report.platform, IdentityPlatform::Atari7800);
        assert_eq!(
            report.verified_loose_rom_sha256(),
            Some(sha256_hex(&bytes).as_str())
        );
        assert_eq!(
            report.verified_loose_rom_canonical_sha256(),
            Some(sha256_hex(payload).as_str())
        );
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::Platform
                && item.status == IdentityStatus::Verified
                && item.diagnostic.contains("ATARI7800 header validated")
        }));
        assert!(report.complete);
    }

    #[test]
    fn malformed_atari7800_header_is_refused_and_filename_does_not_verify() {
        let directory = FixtureDir::new("atari7800-malformed");
        let payload = b"a78 payload";
        let bytes = a78_image(payload, (payload.len() + 1) as u32);
        let path = write_fixture(&directory, "Atari 7800 title.a78", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Atari7800"));
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert!(!report.complete);

        let candidate = inspect_game_identity(&path, Some("Atari7800"));
        assert_eq!(candidate.verified_loose_rom_sha256(), None);
        assert!(!candidate.complete);
    }

    #[test]
    fn lynx_headered_rom_emits_structural_and_canonical_identity() {
        let directory = FixtureDir::new("lynx-loose");
        let payload = b"lynx payload";
        let bytes = lnx_image(payload);
        let path = write_fixture(&directory, "title.lnx", &bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Atari Lynx"));

        assert_eq!(report.platform, IdentityPlatform::AtariLynx);
        assert_eq!(
            report.verified_loose_rom_canonical_sha256(),
            Some(sha256_hex(payload).as_str())
        );
        assert!(report.evidence.iter().any(|item| {
            item.kind == IdentityKind::Platform
                && item.status == IdentityStatus::Verified
                && item.diagnostic.contains("LYNX header validated")
        }));
        assert!(report.complete);
    }

    #[test]
    fn headerless_lynx_lyx_remains_hash_only_and_untrusted_filename_is_not_identity() {
        let directory = FixtureDir::new("lynx-headerless");
        let bytes = b"headerless lynx payload";
        let path = write_fixture(&directory, "Lynx title.lyx", bytes);
        let report = inspect_catalogued_game_identity(&path, Some("Atari Lynx"));
        assert_eq!(
            report.verified_loose_rom_sha256(),
            Some(sha256_hex(bytes).as_str())
        );
        assert_eq!(report.verified_loose_rom_canonical_sha256(), None);
        assert!(report.complete);

        let candidate = inspect_game_identity(&path, Some("Atari Lynx"));
        assert_eq!(candidate.verified_loose_rom_sha256(), None);
        assert!(!candidate.complete);
    }

    #[test]
    fn atari_identity_aliases_round_trip_to_their_canonical_rows() {
        for (alias, expected) in [
            ("Atari 2600", IdentityPlatform::Atari2600),
            ("Atari 5200", IdentityPlatform::Atari5200),
            ("Atari 7800", IdentityPlatform::Atari7800),
            ("Atari 8-bit", IdentityPlatform::Atari8Bit),
            ("Atari Lynx", IdentityPlatform::AtariLynx),
            ("Atari Jaguar", IdentityPlatform::AtariJaguar),
            ("Atari ST", IdentityPlatform::AtariST),
        ] {
            assert_eq!(
                IdentityPlatform::from_catalogue(Some(alias)),
                expected,
                "{alias}"
            );
        }
    }

    fn ngp_image(system_flag: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; crate::ngp_header_evidence::NGP_HEADER_BYTES + 16];
        bytes[..28].copy_from_slice(b"COPYRIGHT BY SNK CORPORATION");
        bytes[0x1c..0x20].copy_from_slice(&0x1000_u32.to_le_bytes());
        bytes[0x20..0x22].copy_from_slice(&0x0042_u16.to_le_bytes());
        bytes[0x22] = 1;
        bytes[0x23] = system_flag;
        bytes[0x24..0x2c].copy_from_slice(b"TESTGAME");
        bytes
    }

    #[test]
    fn ngp_identity_aliases_round_trip() {
        assert_eq!(
            IdentityPlatform::from_catalogue(Some("Neo Geo Pocket")),
            IdentityPlatform::Ngp
        );
        assert_eq!(
            IdentityPlatform::from_catalogue(Some("ngp")),
            IdentityPlatform::Ngp
        );
        assert_eq!(
            IdentityPlatform::from_catalogue(Some("Neo Geo Pocket Color")),
            IdentityPlatform::Ngpc
        );
        assert_eq!(
            IdentityPlatform::from_catalogue(Some("ngpc")),
            IdentityPlatform::Ngpc
        );
    }

    #[test]
    fn ngp_header_controls_platform_over_crossed_extensions() {
        let directory = FixtureDir::new("ngp-crossed-extensions");
        let ngp = write_fixture(&directory, "colour.ngp", &ngp_image(0x10));
        let ngc = write_fixture(&directory, "mono.ngc", &ngp_image(0x00));

        let colour = inspect_catalogued_game_identity(&ngp, Some("Neo Geo Pocket"));
        assert_eq!(colour.platform, IdentityPlatform::Ngpc);
        assert!(colour.complete);
        assert!(colour.verified_loose_rom_sha256().is_some());

        let mono = inspect_catalogued_game_identity(&ngc, Some("Neo Geo Pocket Color"));
        assert_eq!(mono.platform, IdentityPlatform::Ngp);
        assert!(mono.complete);
        assert!(mono.verified_loose_rom_sha256().is_some());
    }

    #[test]
    fn ngp_unknown_or_truncated_header_fails_closed() {
        let directory = FixtureDir::new("ngp-invalid-header");
        for (name, bytes) in [
            ("unknown.ngp", ngp_image(0x55)),
            ("unknown.ngc", ngp_image(0x55)),
            ("truncated.ngp", ngp_image(0x00)[..32].to_vec()),
        ] {
            let path = write_fixture(&directory, name, &bytes);
            let hint = if name.ends_with(".ngc") {
                "Neo Geo Pocket Color"
            } else {
                "Neo Geo Pocket"
            };
            let report = inspect_catalogued_game_identity(&path, Some(hint));
            assert!(!report.complete, "{name}");
            assert_eq!(report.verified_loose_rom_sha256(), None, "{name}");
        }
    }

    #[test]
    fn ngp_identity_never_uses_filename_without_validated_header() {
        let directory = FixtureDir::new("ngp-filename-only");
        let path = write_fixture(&directory, "Neo Geo Pocket Color.ngp", b"not a header");
        let report = inspect_catalogued_game_identity(&path, Some("Neo Geo Pocket"));
        assert!(!report.complete);
        assert_eq!(report.verified_loose_rom_sha256(), None);
        assert!(report.evidence.iter().all(|item| {
            item.kind != IdentityKind::LooseRomTitle || item.status != IdentityStatus::Verified
        }));
    }
}

#[test]
fn a_deferred_identity_status_reads_not_available_yet() {
    assert_eq!(IdentityStatus::Deferred.to_string(), "Not available yet");
    assert_ne!(IdentityStatus::Deferred.to_string(), "Deferred");
}
