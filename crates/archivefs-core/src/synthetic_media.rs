//! Synthetic, legally generated media for every family the platform × format
//! coverage suite exercises end to end. Each generator builds the smallest
//! structure a real parser accepts - a boot block and root block, a cartridge
//! header with a consistent checksum, a snapshot with sane registers - from
//! documented format rules, so the bytes prove *format* handling and nothing
//! else. No generator contains or derives from any copyrighted game.
//!
//! Test-support only: compiled for tests and the `test-support` feature.

use crate::verified_game_fixture::synthetic_rom;

/// One sample: which platform the bytes belong to, the extension a collector
/// would give them, and what the bytes are.
#[derive(Debug, Clone)]
pub struct MediaSample {
    pub platform_id: &'static str,
    pub extension: &'static str,
    /// Short description of what makes the bytes valid.
    pub structure: &'static str,
    pub bytes: Vec<u8>,
}

fn put_be32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

/// An AmigaDOS floppy image: `DOS` boot identifier with the given DOS type
/// byte (0 = OFS, 1 = FFS, ...), a root block whose block checksum is valid
/// and whose volume label is `volume`. Sized at 128 blocks so it stays small;
/// the parser validates structure, not the standard 880 KiB length.
pub fn amiga_adf(dos_type: u8, volume: &[u8]) -> Vec<u8> {
    const SECTORS: usize = 128;
    const ROOT: usize = SECTORS / 2;
    let mut image = vec![0_u8; SECTORS * 512];
    image[..3].copy_from_slice(b"DOS");
    image[3] = dos_type;
    put_be32(&mut image, 8, ROOT as u32);
    let mut root = [0_u8; 512];
    put_be32(&mut root, 0, 2); // T_HEADER
    put_be32(&mut root, 12, 72); // hash-table size
    let name_len = volume.len().min(30);
    root[0x1B0] = name_len as u8;
    root[0x1B1..0x1B1 + name_len].copy_from_slice(&volume[..name_len]);
    put_be32(&mut root, 508, 1); // ST_ROOT
    let mut sum = 0_u32;
    for offset in (0..512).step_by(4) {
        if offset != 20 {
            sum = sum.wrapping_add(u32::from_be_bytes(
                root[offset..offset + 4].try_into().expect("4 bytes"),
            ));
        }
    }
    put_be32(&mut root, 20, (sum as i32).wrapping_neg() as u32);
    image[ROOT * 512..(ROOT + 1) * 512].copy_from_slice(&root);
    image
}

/// A 48K `.sna` snapshot: 27-byte header with SP in RAM and interrupt mode 1,
/// then 48 KiB of memory.
pub fn zx_sna_48k() -> Vec<u8> {
    let mut file = vec![0_u8; 27 + 49152];
    file[23] = 0x00;
    file[24] = 0x80; // SP = 0x8000
    file[25] = 1; // IM 1
    file[26] = 7; // border
    file
}

/// A version-1 uncompressed `.z80` snapshot: 30-byte header with PC and SP
/// set, then 48 KiB of memory.
pub fn zx_z80_v1() -> Vec<u8> {
    let mut header = [0_u8; 30];
    header[6] = 0x00;
    header[7] = 0x80; // PC = 0x8000
    header[8] = 0x00;
    header[9] = 0xC0; // SP = 0xC000
    header[27] = 1;
    header[28] = 1;
    header[29] = 0x01; // IM 1
    let mut file = header.to_vec();
    file.extend(std::iter::repeat_n(0_u8, 49152));
    file
}

/// A ZX Spectrum `.tap`: one two-byte block whose flag and checksum XOR to
/// zero.
pub fn zx_tap() -> Vec<u8> {
    vec![2, 0, 0xFF, 0xFF]
}

/// A `.tzx` with the `ZXTape!` signature, a text-description block and one
/// standard-speed data block.
pub fn zx_tzx() -> Vec<u8> {
    let mut file = b"ZXTape!\x1a\x01\x14".to_vec();
    file.extend_from_slice(&[0x30, 6]);
    file.extend_from_slice(b"EmuWiz");
    file.extend_from_slice(&[0x10, 0xE8, 0x03, 0x02, 0x00, 0xFF, 0xFF]);
    file
}

/// A LoROM SNES image of `size` bytes with a 21-character title, map mode
/// 0x20 and a checksum/complement pair consistent with the whole image.
pub fn snes_lorom(size: usize) -> Vec<u8> {
    const BASE: usize = 0x7FC0;
    let mut rom = vec![0_u8; size.max(BASE + 0x20)];
    let title = b"EMUWIZ SYNTHETIC ROM ";
    rom[BASE..BASE + 21].copy_from_slice(&title[..21]);
    rom[BASE + 0x15] = 0x20; // map mode LoROM
    rom[BASE + 0x17] = 0x08; // ROM size code
    rom[BASE + 0x19] = 0x01; // destination: North America
    // With complement/checksum preset to FFFF/0000 the pair always sums to
    // 0x1FE, so the checksum over the whole image can be computed once.
    rom[BASE + 0x1C] = 0xFF;
    rom[BASE + 0x1D] = 0xFF;
    rom[BASE + 0x1E] = 0x00;
    rom[BASE + 0x1F] = 0x00;
    let checksum = rom
        .iter()
        .fold(0_u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
    rom[BASE + 0x1C..BASE + 0x1E].copy_from_slice(&(checksum ^ 0xFFFF).to_le_bytes());
    rom[BASE + 0x1E..BASE + 0x20].copy_from_slice(&checksum.to_le_bytes());
    rom
}

/// A raw Mega Drive image with a complete 0x100 header (console name,
/// titles, serial, region) and a checksum consistent with its payload.
pub fn megadrive_bin(size: usize) -> Vec<u8> {
    let mut rom = vec![0_u8; size.max(0x400)];
    rom[0x100..0x100 + 15].copy_from_slice(b"SEGA MEGA DRIVE");
    let title = b"EMUWIZ SYNTHETIC CARTRIDGE";
    rom[0x120..0x120 + title.len()].copy_from_slice(title);
    rom[0x150..0x150 + title.len()].copy_from_slice(title);
    rom[0x180..0x180 + 11].copy_from_slice(b"GM 00000000");
    rom[0x1F0..0x1F3].copy_from_slice(b"JUE");
    let checksum = rom[0x200..].chunks_exact(2).fold(0_u16, |sum, pair| {
        sum.wrapping_add(u16::from_be_bytes([pair[0], pair[1]]))
    });
    rom[0x18E..0x190].copy_from_slice(&checksum.to_be_bytes());
    rom
}

/// The same Mega Drive image as a Super Magic Drive dump: 512-byte SMD
/// header then 16 KiB blocks with even/odd bytes split.
pub fn megadrive_smd() -> Vec<u8> {
    let canonical = megadrive_bin(16384 * 2);
    let payload = crate::smd_normalization::interleave_bin_to_smd(&canonical)
        .expect("two whole blocks interleave");
    let mut header = vec![0_u8; 512];
    header[0] = (canonical.len() / 16384) as u8;
    header[1] = 0x03;
    header[8] = 0xAA;
    header[9] = 0xBB;
    header.extend_from_slice(&payload);
    header
}

/// An iNES image: 16-byte header, one 16 KiB PRG bank, one 8 KiB CHR bank.
pub fn nes_ines() -> Vec<u8> {
    let mut rom = vec![0_u8; 16 + 16384 + 8192];
    rom[..4].copy_from_slice(b"NES\x1a");
    rom[4] = 1;
    rom[5] = 1;
    rom
}

/// A big-endian (`.z64`) Nintendo 64 image with the standard header fields.
pub fn n64_z64() -> Vec<u8> {
    let mut rom = vec![0_u8; 0x1000];
    rom[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
    put_be32(&mut rom, 0x04, 0x0000_0F41);
    put_be32(&mut rom, 0x08, 0x8000_0400);
    put_be32(&mut rom, 0x10, 0xDEAD_BEEF);
    put_be32(&mut rom, 0x14, 0xCAFE_BABE);
    let name = b"EMUWIZ SYNTHETIC";
    rom[0x20..0x20 + name.len()].copy_from_slice(name);
    rom[0x3B] = b'N';
    rom[0x3C..0x3E].copy_from_slice(b"SM");
    rom[0x3E] = b'E';
    rom
}

/// A Game Boy image with the Nintendo logo, a title and a valid header
/// checksum; `cgb_flag` 0x00 for Game Boy, 0xC0 for Game Boy Color only.
pub fn gameboy(cgb_flag: u8) -> Vec<u8> {
    const LOGO: [u8; 48] = [
        0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00,
        0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD,
        0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB,
        0xB9, 0x33, 0x3E,
    ];
    let mut rom = vec![0_u8; 32 * 1024];
    rom[0x104..0x104 + 48].copy_from_slice(&LOGO);
    let title = b"EMUWIZ TEST";
    rom[0x134..0x134 + title.len()].copy_from_slice(title);
    rom[0x143] = cgb_flag;
    rom[0x146] = 0x03;
    rom[0x147] = 0x1B;
    rom[0x148] = 0x00;
    rom[0x149] = 0x03;
    let checksum = rom[0x134..=0x14C]
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_sub(*byte).wrapping_sub(1));
    rom[0x14D] = checksum;
    rom
}

/// The Game Boy Advance cartridge the verified-game fixture uses.
pub fn gba() -> Vec<u8> {
    synthetic_rom(0x1EE7)
}

/// Every sample the coverage suite scans end to end, in platform order.
pub fn samples() -> Vec<MediaSample> {
    vec![
        MediaSample {
            platform_id: "Amiga",
            extension: "adf",
            structure: "AmigaDOS OFS boot block + valid root block",
            bytes: amiga_adf(0, b"EmuWizOFS"),
        },
        MediaSample {
            platform_id: "Amiga",
            extension: "adf",
            structure: "AmigaDOS FFS boot block + valid root block",
            bytes: amiga_adf(1, b"EmuWizFFS"),
        },
        MediaSample {
            platform_id: "ZX Spectrum",
            extension: "sna",
            structure: "48K SNA header with SP in RAM and IM 1",
            bytes: zx_sna_48k(),
        },
        MediaSample {
            platform_id: "ZX Spectrum",
            extension: "z80",
            structure: "Z80 v1 uncompressed snapshot",
            bytes: zx_z80_v1(),
        },
        MediaSample {
            platform_id: "ZX Spectrum",
            extension: "tap",
            structure: "one ZX TAP block with a valid checksum (shared format)",
            bytes: zx_tap(),
        },
        MediaSample {
            platform_id: "ZX Spectrum",
            extension: "tzx",
            structure: "ZXTape! container with description and data blocks (shared with CPC)",
            bytes: zx_tzx(),
        },
        MediaSample {
            platform_id: "SNES",
            extension: "sfc",
            structure: "LoROM header with consistent checksum complement",
            bytes: snes_lorom(0x8000),
        },
        MediaSample {
            platform_id: "SNES",
            extension: "smc",
            structure: "LoROM header with consistent checksum complement",
            bytes: snes_lorom(0x8000),
        },
        MediaSample {
            platform_id: "MegaDrive",
            extension: "md",
            structure: "SEGA MEGA DRIVE header with consistent checksum",
            bytes: megadrive_bin(0x400),
        },
        MediaSample {
            platform_id: "MegaDrive",
            extension: "bin",
            structure: "SEGA MEGA DRIVE header with consistent checksum (shared extension)",
            bytes: megadrive_bin(0x400),
        },
        MediaSample {
            platform_id: "MegaDrive",
            extension: "smd",
            structure: "Super Magic Drive interleaved dump",
            bytes: megadrive_smd(),
        },
        MediaSample {
            platform_id: "NES",
            extension: "nes",
            structure: "iNES header, 1 PRG + 1 CHR bank",
            bytes: nes_ines(),
        },
        MediaSample {
            platform_id: "N64",
            extension: "z64",
            structure: "big-endian N64 header",
            bytes: n64_z64(),
        },
        MediaSample {
            platform_id: "Game Boy",
            extension: "gb",
            structure: "Nintendo logo + valid header checksum",
            bytes: gameboy(0x00),
        },
        MediaSample {
            platform_id: "Game Boy Color",
            extension: "gbc",
            structure: "Nintendo logo + CGB-only flag + valid header checksum",
            bytes: gameboy(0xC0),
        },
        MediaSample {
            platform_id: "Game Boy Color",
            extension: "gbc",
            structure: "Nintendo logo + CGB-enhanced (dual-mode) flag + valid header checksum",
            bytes: gameboy(0x80),
        },
        MediaSample {
            platform_id: "Game Boy Advance",
            extension: "gba",
            structure: "verified-game fixture cartridge",
            bytes: gba(),
        },
    ]
}
