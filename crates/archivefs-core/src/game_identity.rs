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

use crate::disc_evidence_collector::{
    DiscCollectionRefusal, chd_needs_specialist_optical_backend, open_chd_iso9660,
    read_bounded_chd_bytes,
};
use crate::dreamcast_boot_evidence::{IP_BIN_META_BYTES, parse_ip_bin_meta};
use crate::gb_header_evidence::{GB_HEADER_BYTES, GbColorSupport, parse_gb_header};
use crate::ingestion::cue_bin::{CueDataTrackMode, resolve_data_track};
use crate::ingestion::gdi::{GdiDataTrackMode, resolve_gdi_data_track};
use crate::iso9660::find_path;
use crate::logical_media::LogicalMedia as _;
use crate::playstation_boot_evidence::{
    PSX_EXECUTABLE_HEADER_BYTES, looks_like_psx_exe, parse_system_cnf_boot,
};
use crate::raw_cd_logical_media::{
    open_cooked_cd_file_logical_media, open_raw_cd_file_logical_media,
};
use crate::saturn_boot_evidence::{SATURN_SYSTEM_ID_BYTES, parse_saturn_system_id};
use crate::segacd_boot_evidence::{SEGA_CD_DISC_ID_BYTES, parse_segacd_product_code};

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
    SaturnProductNumber,
    DreamcastProductCode,
    SegaCdProductCode,
    Pcsx2ExecutableCrc,
    DolphinGameId,
    DolphinRevision,
    DolphinDiscNumber,
    DolphinRegion,
    LooseRomSha256,
    LooseRomFormat,
    LooseRomTitle,
    XexTitleId,
    XexMediaId,
}

impl fmt::Display for IdentityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Platform => "Platform",
            Self::Ps1Serial => "PS1 serial",
            Self::Ps2Serial => "PS2 serial",
            Self::SaturnProductNumber => "Saturn product number",
            Self::DreamcastProductCode => "Dreamcast product code",
            Self::SegaCdProductCode => "Sega CD product code",
            Self::Pcsx2ExecutableCrc => "PCSX2 executable CRC",
            Self::DolphinGameId => "Dolphin Game ID",
            Self::DolphinRevision => "Dolphin revision",
            Self::DolphinDiscNumber => "Dolphin disc number",
            Self::DolphinRegion => "Dolphin region code",
            Self::LooseRomSha256 => "Local ROM SHA-256",
            Self::LooseRomFormat => "Loose ROM format",
            Self::LooseRomTitle => "Normalized ROM title",
            Self::XexTitleId => "Xbox 360 Title ID",
            Self::XexMediaId => "Xbox 360 Media ID",
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
    Saturn,
    Dreamcast,
    SegaCd,
    GameCube,
    Wii,
    MegaDrive,
    Snes,
    Nes,
    GameBoy,
    GameBoyColor,
    GameBoyAdvance,
    Xbox360,
    Other,
}

impl IdentityPlatform {
    pub fn from_catalogue(value: Option<&str>) -> Self {
        let value = value.unwrap_or_default().trim().to_ascii_lowercase();
        match value.as_str() {
            "playstation" | "playstation 1" | "playstation1" | "psx" | "ps1"
            | "sony playstation" => Self::PlayStation,
            "playstation 2" | "playstation2" | "ps2" | "sony playstation 2" => Self::PlayStation2,
            "saturn" | "sega saturn" | "sega saturn console" => Self::Saturn,
            "dreamcast" | "sega dreamcast" => Self::Dreamcast,
            "sega cd" | "sega-cd" | "segacd" | "mega cd" | "mega-cd" | "megacd" => Self::SegaCd,
            "gamecube" | "nintendo gamecube" | "gc" | "gcn" => Self::GameCube,
            "wii" | "nintendo wii" => Self::Wii,
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
            "xbox360" | "xbox 360" | "microsoft xbox 360" => Self::Xbox360,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PlayStation => "PlayStation",
            Self::PlayStation2 => "PlayStation 2",
            Self::Saturn => "Sega Saturn",
            Self::Dreamcast => "Sega Dreamcast",
            Self::SegaCd => "Sega Mega-CD / Sega CD",
            Self::GameCube => "GameCube",
            Self::Wii => "Wii",
            Self::MegaDrive => "Mega Drive / Genesis",
            Self::Snes => "SNES",
            Self::Nes => "NES",
            Self::GameBoy => "Game Boy",
            Self::GameBoyColor => "Game Boy Color",
            Self::GameBoyAdvance => "Game Boy Advance",
            Self::Xbox360 => "Xbox 360",
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

    pub fn is_verified_loose_rom(&self) -> bool {
        self.format == IdentityImageFormat::LooseCartridgeRom
            && self.verified_loose_rom_sha256().is_some()
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
    ) {
        inspect_loose_rom(&mut report, trusted_platform, trusted);
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
                    | IdentityPlatform::Saturn
                    | IdentityPlatform::Dreamcast
                    | IdentityPlatform::SegaCd
            ) =>
        {
            inspect_cue(&mut report, trusted);
        }
        "iso" | "gcm" if platform != IdentityPlatform::Xbox360 => {
            inspect_direct_iso(&mut report, trusted)
        }
        "xex" if platform == IdentityPlatform::Xbox360 => inspect_direct_xex(&mut report, trusted),
        "zip" if platform == IdentityPlatform::Xbox360 => inspect_zip_xex(&mut report, trusted),
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
                    | IdentityPlatform::Saturn
                    | IdentityPlatform::Dreamcast
                    | IdentityPlatform::SegaCd
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
            "only direct ISO/GCM, RVZ and CISO for GameCube/Wii, a single ISO inside ZIP, direct XEX, and a single XEX inside ZIP are supported",
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
        (IdentityPlatform::GameBoy, "gb") => Some("gb"),
        (IdentityPlatform::GameBoyColor, "gbc") => Some("gbc"),
        (IdentityPlatform::GameBoyAdvance, "gba") => Some("gba"),
        _ => None,
    }
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
    let digest = match hash_bounded_file(&mut file, MAX_LOOSE_ROM_BYTES) {
        Ok((digest, bytes_read)) => {
            report.bytes_read = bytes_read;
            digest
        }
        Err(error) => {
            add_loose_rom_unavailable(report, source_error_status(&error), &error.to_string());
            return;
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
        | IdentityPlatform::Saturn
        | IdentityPlatform::Dreamcast
        | IdentityPlatform::SegaCd
        | IdentityPlatform::MegaDrive
        | IdentityPlatform::Snes
        | IdentityPlatform::Nes
        | IdentityPlatform::GameBoy
        | IdentityPlatform::GameBoyColor
        | IdentityPlatform::GameBoyAdvance
        | IdentityPlatform::Xbox360
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

fn inspect_iso_source(
    report: &mut GameIdentityReport,
    source: &mut dyn ByteSource,
    member_path: Option<Vec<u8>>,
    member_index: Option<usize>,
) {
    match report.platform {
        IdentityPlatform::PlayStation => inspect_ps1_iso(report, source, member_path, member_index),
        IdentityPlatform::Saturn => {
            inspect_saturn_source(report, source, member_path, member_index)
        }
        IdentityPlatform::Dreamcast => {
            inspect_dreamcast_source(report, source, member_path, member_index)
        }
        IdentityPlatform::SegaCd => {
            inspect_sega_cd_source(report, source, member_path, member_index)
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
        | IdentityPlatform::Xbox360
        | IdentityPlatform::Other => {}
    }
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
        .split(|character| character == '\\' || character == '/')
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

    let (media, filesystem) = match open_chd_iso9660(&bytes) {
        Ok(pair) => pair,
        Err(refusal) => {
            push_disc_chd_refusal(report, &refusal);
            return;
        }
    };

    if matches!(
        report.platform,
        IdentityPlatform::Saturn | IdentityPlatform::Dreamcast | IdentityPlatform::SegaCd
    ) {
        let mut source = MediaSource::new(media);
        if report.platform == IdentityPlatform::Saturn {
            inspect_saturn_source(report, &mut source, None, None);
        } else if report.platform == IdentityPlatform::Dreamcast {
            inspect_dreamcast_source(report, &mut source, None, None);
        } else {
            inspect_sega_cd_source(report, &mut source, None, None);
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
            IdentityPlatform::Saturn => IdentityKind::SaturnProductNumber,
            IdentityPlatform::Dreamcast => IdentityKind::DreamcastProductCode,
            IdentityPlatform::SegaCd => IdentityKind::SegaCdProductCode,
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
        IdentityPlatform::Saturn => &[IdentityKind::SaturnProductNumber],
        IdentityPlatform::Dreamcast => &[IdentityKind::DreamcastProductCode],
        IdentityPlatform::SegaCd => &[IdentityKind::SegaCdProductCode],
        IdentityPlatform::GameCube | IdentityPlatform::Wii => {
            &[IdentityKind::DolphinGameId, IdentityKind::DolphinRevision]
        }
        IdentityPlatform::Xbox360 => &[IdentityKind::XexTitleId, IdentityKind::XexMediaId],
        IdentityPlatform::MegaDrive
        | IdentityPlatform::Snes
        | IdentityPlatform::Nes
        | IdentityPlatform::GameBoy
        | IdentityPlatform::GameBoyColor
        | IdentityPlatform::GameBoyAdvance
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
        | IdentityPlatform::MegaDrive
        | IdentityPlatform::Snes
        | IdentityPlatform::Nes
        | IdentityPlatform::GameBoy
        | IdentityPlatform::GameBoyColor
        | IdentityPlatform::GameBoyAdvance
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

    /// Wraps an ISO9660 image (e.g. from [`ps1_iso`]) into a genuine,
    /// `open_chd_track_logical_media`-openable uncompressed CHD v5 file, so
    /// PS1 CHD identity tests exercise the real bounded CHD decode path
    /// rather than a shortcut. This deliberately re-derives the same
    /// minimal CHD v5 header/metadata/map/hunk-data layout that
    /// `chd_logical_media`'s own private test-only `build_uncompressed_chd`
    /// uses (that helper cannot be imported across module boundaries) - it
    /// is not a second CHD *reader*, only a second CHD *test fixture
    /// writer*, mirroring one that already exists and is already trusted.
    /// Each `LOGICAL_BLOCK_BYTES` (2048-byte) block of `image` becomes one
    /// `RAW_SECTOR_BYTES` (2352-byte) MODE1_RAW sector, matching
    /// `chd_logical_media`'s own `mode1_sectors_for` test helper.
    fn ps1_chd(image: &[u8]) -> Vec<u8> {
        use crate::dat::archive::chd::CHD_MAGIC;
        use crate::raw_cd_sector::{LOGICAL_BLOCK_BYTES, MODE1_USER_DATA_OFFSET, RAW_SECTOR_BYTES};

        fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
            bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
        }

        // `ps1_iso` never fills in the PVD's logical-block-size field
        // because the plain-ISO `ByteSource` reader (`find_iso_path`) never
        // reads it - it works purely off fixed 2048-byte offsets. The CHD
        // path instead goes through `crate::iso9660::observe_iso9660`,
        // which - correctly, for a real disc - insists this field says
        // 2048 both-endian. Patching it here is completing a real ISO9660
        // field the shared fixture happens to leave zeroed, not working
        // around a bug.
        let mut image = image.to_vec();
        let pvd = 16 * LOGICAL_BLOCK_BYTES;
        image[pvd + 128..pvd + 130].copy_from_slice(&(LOGICAL_BLOCK_BYTES as u16).to_le_bytes());
        image[pvd + 130..pvd + 132].copy_from_slice(&(LOGICAL_BLOCK_BYTES as u16).to_be_bytes());

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

        let mut wrong_executable = ps1_iso(b"SLUS_123.45;1", b"BOOT=cdrom:\\SLUS_123.45;1\n", true);
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

    /// Mirrors `crate::dreamcast_cdi`'s own private test-fixture builder
    /// exactly (private to that module's own test mod, so re-derived here
    /// per this crate's established per-file-fixture convention).
    /// `sessions[i]` is a list of `(track_mode, read_mode, start_address,
    /// track_length, pregap)`; only cooked (`read_mode == 0`) tracks are
    /// used here since these tests need real, correctly-placed IP.BIN
    /// content, not just structural parsing.
    fn cdi_bytes(sessions: &[Vec<(u32, u32, u32, u32, u32)>]) -> Vec<u8> {
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
    fn ps1_chd_for_a_non_playstation_platform_hint_still_defers() {
        // Format/platform guarding: a `.chd` is only ever authoritatively
        // inspected when the platform hint itself says PlayStation - this
        // task must not make every CHD look like a PS1 disc.
        let chd = inspect_game_identity(Path::new("/games/game.chd"), Some("PS2"));
        assert_eq!(chd.format, IdentityImageFormat::Deferred);
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
}

#[test]
fn a_deferred_identity_status_reads_not_available_yet() {
    assert_eq!(IdentityStatus::Deferred.to_string(), "Not available yet");
    assert_ne!(IdentityStatus::Deferred.to_string(), "Deferred");
}
