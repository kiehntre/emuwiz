//! Mode 1 EDC and P/Q parity over the ECMA-130 2352-byte record.
//! Header bytes are preserved; callers never convert sector modes here.
use super::{RAW_SECTOR_BYTES, RawCdSectorMode, detect_sector_mode};
use std::sync::OnceLock;

struct Tables {
    edc: [u32; 256],
    forward: [u8; 256],
    backward: [u8; 256],
}
fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut t = Tables {
            edc: [0; 256],
            forward: [0; 256],
            backward: [0; 256],
        };
        for i in 0..256 {
            let mut crc = i as u32;
            for _ in 0..8 {
                crc = (crc >> 1) ^ if crc & 1 != 0 { 0xd8018001 } else { 0 };
            }
            t.edc[i] = crc;
            let doubled = ((i << 1) ^ if i & 0x80 != 0 { 0x11d } else { 0 }) as u8;
            t.forward[i] = doubled;
            t.backward[(i as u8 ^ doubled) as usize] = i as u8;
        }
        t
    })
}
/// CD-ROM EDC: reflected polynomial D8018001, initial zero, no final XOR.
pub fn mode1_edc(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0, |crc, byte| {
        (crc >> 8) ^ tables().edc[((crc as u8) ^ byte) as usize]
    })
}
fn parity(
    source: &[u8],
    major: usize,
    minor: usize,
    major_mult: usize,
    minor_inc: usize,
    out: &mut [u8],
) {
    let size = major * minor;
    let t = tables();
    for row in 0..major {
        let mut index = (row >> 1) * major_mult + (row & 1);
        let (mut a, mut b) = (0u8, 0u8);
        for _ in 0..minor {
            let value = source[index];
            index = (index + minor_inc) % size;
            a = t.forward[(a ^ value) as usize];
            b ^= value;
        }
        a = t.backward[(t.forward[a as usize] ^ b) as usize];
        out[row] = a;
        out[row + major] = a ^ b;
    }
}
/// Regenerate EDC at 2064, zero reserve at 2068, P at 2076, Q at 2248.
/// The sync, address, mode and 2048-byte payload remain exactly as supplied.
pub fn regenerate_mode1(sector: &mut [u8; RAW_SECTOR_BYTES]) -> Result<(), String> {
    if detect_sector_mode(sector) != Some(RawCdSectorMode::Mode1Raw) {
        return Err("Mode 1 raw sector with valid sync required".into());
    }
    let edc = mode1_edc(&sector[..2064]).to_le_bytes();
    sector[2064..2068].copy_from_slice(&edc);
    sector[2068..2076].fill(0);
    let mut p = [0; 172];
    parity(&sector[12..2076], 86, 24, 2, 86, &mut p);
    sector[2076..2248].copy_from_slice(&p);
    let mut q = [0; 104];
    parity(&sector[12..2248], 52, 43, 86, 88, &mut q);
    sector[2248..2352].copy_from_slice(&q);
    Ok(())
}
/// Full EDC/reserve/ECC comparison; validation never repairs source bytes.
pub fn verify_mode1(sector: &[u8; RAW_SECTOR_BYTES]) -> Result<(), String> {
    let mut expected = *sector;
    regenerate_mode1(&mut expected)?;
    if expected != *sector {
        return Err("Mode 1 EDC/reserve/ECC mismatch".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
