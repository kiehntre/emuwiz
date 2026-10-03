//! Independently written decoder for verified, fixed-key GameCube AR dash sets.
//!
//! No Wii-native cipher compatibility is established. The existing BSFree
//! GameCube bridge consumes this decoder through its normal classification and
//! reviewed install path. See `docs/research/GC_WII_AR_DASH_DECRYPTOR.md` for the
//! original provenance and `GAMECUBE_AR_PRODUCTION_INTEGRATION.md` for integration.
//!
//! The result contains canonical raw AR text, not a new cheat IR. Callers must
//! retain its provenance, refuse master verifiers, and use the existing opcode,
//! device, platform and identity gates before considering installation.

use std::fmt::{self, Write};

use serde::Serialize;

pub const GAMECUBE_AR_DECODER_VERSION: &str = "emuwiz-gcn-ar-des-v1";
pub const MAX_ENCRYPTED_BYTES: usize = 16 * 1024;
/// Includes the verifier. At most 255 executable pairs / 510 decoded words.
pub const MAX_ENCRYPTED_LINES: usize = 256;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRTUVWXYZ";
// Published fixed format parameter; no variable-key recovery is implemented.
const GAMECUBE_AR_KEY: u64 = 0x341C_849E_FDA4_B67B;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameCubeArDecodeError {
    InputTooLarge,
    TooManyLines,
    NonAsciiInput,
    MissingBody,
    MalformedLine { line: usize },
    InvalidAlphabet { line: usize },
    ParityMismatch { line: usize },
    ChecksumMismatch { expected: u8, actual: u8 },
    UnsupportedVerifier { reason: &'static str },
}

impl fmt::Display for GameCubeArDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge => write!(
                formatter,
                "encrypted AR exceeds {MAX_ENCRYPTED_BYTES} bytes"
            ),
            Self::TooManyLines => write!(
                formatter,
                "encrypted AR exceeds {MAX_ENCRYPTED_LINES} lines"
            ),
            Self::NonAsciiInput => formatter.write_str("encrypted AR must use ASCII text"),
            Self::MissingBody => {
                formatter.write_str("encrypted AR needs a verifier and a code body")
            }
            Self::MalformedLine { line } => write!(
                formatter,
                "line {line} must be XXXX-XXXX-XXXXX; mixed raw/encrypted input is refused"
            ),
            Self::InvalidAlphabet { line } => write!(
                formatter,
                "line {line} contains a noncanonical AR alphabet character"
            ),
            Self::ParityMismatch { line } => {
                write!(formatter, "encrypted AR parity failed on line {line}")
            }
            Self::ChecksumMismatch { expected, actual } => write!(
                formatter,
                "encrypted AR checksum failed: verifier {expected:X}, calculated {actual:X}"
            ),
            Self::UnsupportedVerifier { reason } => {
                write!(formatter, "unsupported encrypted AR verifier: {reason}")
            }
        }
    }
}

impl std::error::Error for GameCubeArDecodeError {}

/// Evidence for a complete code set whose every line passed parity and whose
/// folded CRC-16/KERMIT matched its verifier. These checks do not authenticate
/// the provider or prove that any decoded operation is safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameCubeArVerification {
    /// Full decrypted verifier, including its checksum nibble; never executable.
    pub verifier_words: [u32; 2],
    pub crc16: u16,
    pub encrypted_lines: usize,
    /// Internal AR game number; must not be substituted for a Dolphin disc ID.
    pub game_id: u16,
    pub code_id: u32,
    pub region: u8,
    /// A master verifier must force the eventual classifier to Unsupported even
    /// when its operational body happens to contain otherwise supported writes.
    pub is_master: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifiedGameCubeAr {
    /// Exact provider input, including case, whitespace and line endings.
    pub original_encrypted_text: String,
    pub decoder_version: &'static str,
    /// Canonical uppercase `XXXXXXXX XXXXXXXX` pairs, without the verifier.
    pub raw_ar_text: String,
    pub verification: GameCubeArVerification,
}

/// Decode one complete GameCube encrypted code set. Never guesses raw input,
/// handles verifier expansions, reads a file, invokes a program, or uses a
/// network. Failure returns no partial or unverified raw body.
pub fn decode_gamecube_ar(input: &str) -> Result<VerifiedGameCubeAr, GameCubeArDecodeError> {
    use GameCubeArDecodeError as Error;

    if input.len() > MAX_ENCRYPTED_BYTES {
        return Err(Error::InputTooLarge);
    }
    if !input.is_ascii() {
        return Err(Error::NonAsciiInput);
    }

    let mut blocks = Vec::with_capacity(MAX_ENCRYPTED_LINES);
    for (index, line) in input.lines().enumerate() {
        let line = line.trim_ascii();
        if line.is_empty() {
            continue;
        }
        if blocks.len() == MAX_ENCRYPTED_LINES {
            return Err(Error::TooManyLines);
        }
        blocks.push(unpack_line(line, index + 1)?);
    }
    if blocks.len() < 2 {
        return Err(Error::MissingBody);
    }

    let subkeys = des_subkeys(GAMECUBE_AR_KEY);
    let mut words = Vec::with_capacity(blocks.len() * 2);
    for block in blocks {
        let decrypted = swap_word_bytes(des_decrypt(swap_word_bytes(block), &subkeys));
        words.push((decrypted >> 32) as u32);
        words.push(decrypted as u32);
    }

    let verifier_words = [words[0], words[1]];
    let expected = (words[0] >> 28) as u8;
    words[0] &= 0x0FFF_FFFF;
    let crc16 = crc16_kermit(&words);
    let actual = ((crc16 ^ (crc16 >> 4) ^ (crc16 >> 8) ^ (crc16 >> 12)) & 15) as u8;
    if expected != actual {
        return Err(Error::ChecksumMismatch { expected, actual });
    }
    if words[1] & 0x0800_0000 == 0 {
        return Err(Error::UnsupportedVerifier {
            reason: "expansion data / alternate seeds are unverified",
        });
    }
    // Only master, region and expansion-disabled bits have established meaning.
    if words[1] & !0xB800_0000 != 0 || (words[1] >> 28) & 3 == 3 {
        return Err(Error::UnsupportedVerifier {
            reason: "reserved flags, region or unused verifier bits",
        });
    }

    let mut raw_ar_text = String::with_capacity((words.len() / 2 - 1) * 18);
    for pair in words[2..].chunks_exact(2) {
        if !raw_ar_text.is_empty() {
            raw_ar_text.push('\n');
        }
        write!(raw_ar_text, "{:08X} {:08X}", pair[0], pair[1])
            .expect("formatting into a String cannot fail");
    }
    Ok(VerifiedGameCubeAr {
        original_encrypted_text: input.to_owned(),
        decoder_version: GAMECUBE_AR_DECODER_VERSION,
        raw_ar_text,
        verification: GameCubeArVerification {
            verifier_words,
            crc16,
            encrypted_lines: words.len() / 2,
            game_id: ((words[0] >> 17) & 0x7FF) as u16,
            code_id: words[0] & 0x1FFFF,
            region: ((words[1] >> 28) & 3) as u8,
            is_master: words[1] & 0x8000_0000 != 0,
        },
    })
}

fn unpack_line(line: &str, line_number: usize) -> Result<u64, GameCubeArDecodeError> {
    let bytes = line.as_bytes();
    if bytes.len() != 15 || bytes[4] != b'-' || bytes[9] != b'-' {
        return Err(GameCubeArDecodeError::MalformedLine { line: line_number });
    }
    let mut packed = 0u128;
    for (index, &byte) in bytes.iter().enumerate() {
        if index == 4 || index == 9 {
            continue;
        }
        let value = ALPHABET
            .iter()
            .position(|&value| value == byte.to_ascii_uppercase())
            .ok_or(GameCubeArDecodeError::InvalidAlphabet { line: line_number })?;
        packed = (packed << 5) | value as u128;
    }
    let block = (packed >> 1) as u64;
    if block.count_ones() & 1 != (packed & 1) as u32 {
        return Err(GameCubeArDecodeError::ParityMismatch { line: line_number });
    }
    Ok(block)
}

fn crc16_kermit(words: &[u32]) -> u16 {
    let mut crc = 0u16;
    for word in words {
        for byte in word.to_le_bytes() {
            crc ^= u16::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ if crc & 1 != 0 { 0x8408 } else { 0 };
            }
        }
    }
    crc
}

fn swap_word_bytes(block: u64) -> u64 {
    (u64::from(((block >> 32) as u32).swap_bytes()) << 32) | u64::from((block as u32).swap_bytes())
}

// Straight bit-selection DES from NIST FIPS 46-3. These are the standard
// primitive functions, not Datel or Dolphin's combined lookup tables.
fn select_bits(value: u64, width: u8, positions: &[u8]) -> u64 {
    positions
        .iter()
        .fold(0, |out, &bit| (out << 1) | ((value >> (width - bit)) & 1))
}

fn des_subkeys(key: u64) -> [u64; 16] {
    let selected = select_bits(key, 64, &PC1);
    let mut left = (selected >> 28) as u32;
    let mut right = (selected & 0x0FFF_FFFF) as u32;
    let mut keys = [0; 16];
    for (index, shift) in [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1]
        .into_iter()
        .enumerate()
    {
        left = ((left << shift) | (left >> (28 - shift))) & 0x0FFF_FFFF;
        right = ((right << shift) | (right >> (28 - shift))) & 0x0FFF_FFFF;
        keys[index] = select_bits((u64::from(left) << 28) | u64::from(right), 56, &PC2);
    }
    keys
}

fn des_decrypt(block: u64, subkeys: &[u64; 16]) -> u64 {
    let permuted = select_bits(block, 64, &IP);
    let mut left = (permuted >> 32) as u32;
    let mut right = permuted as u32;
    for &key in subkeys.iter().rev() {
        let expanded = select_bits(u64::from(right), 32, &E) ^ key;
        let mut substituted = 0u32;
        for (index, table) in SBOXES.iter().enumerate() {
            let six = ((expanded >> (42 - index * 6)) & 63) as usize;
            let row = ((six >> 4) & 2) | (six & 1);
            let column = (six >> 1) & 15;
            substituted = (substituted << 4) | u32::from(table[row * 16 + column]);
        }
        let next = left ^ select_bits(u64::from(substituted), 32, &P) as u32;
        left = right;
        right = next;
    }
    select_bits((u64::from(right) << 32) | u64::from(left), 64, &FP)
}

const IP: [u8; 64] = [
    58, 50, 42, 34, 26, 18, 10, 2, 60, 52, 44, 36, 28, 20, 12, 4, 62, 54, 46, 38, 30, 22, 14, 6,
    64, 56, 48, 40, 32, 24, 16, 8, 57, 49, 41, 33, 25, 17, 9, 1, 59, 51, 43, 35, 27, 19, 11, 3, 61,
    53, 45, 37, 29, 21, 13, 5, 63, 55, 47, 39, 31, 23, 15, 7,
];
const FP: [u8; 64] = [
    40, 8, 48, 16, 56, 24, 64, 32, 39, 7, 47, 15, 55, 23, 63, 31, 38, 6, 46, 14, 54, 22, 62, 30,
    37, 5, 45, 13, 53, 21, 61, 29, 36, 4, 44, 12, 52, 20, 60, 28, 35, 3, 43, 11, 51, 19, 59, 27,
    34, 2, 42, 10, 50, 18, 58, 26, 33, 1, 41, 9, 49, 17, 57, 25,
];
const E: [u8; 48] = [
    32, 1, 2, 3, 4, 5, 4, 5, 6, 7, 8, 9, 8, 9, 10, 11, 12, 13, 12, 13, 14, 15, 16, 17, 16, 17, 18,
    19, 20, 21, 20, 21, 22, 23, 24, 25, 24, 25, 26, 27, 28, 29, 28, 29, 30, 31, 32, 1,
];
const P: [u8; 32] = [
    16, 7, 20, 21, 29, 12, 28, 17, 1, 15, 23, 26, 5, 18, 31, 10, 2, 8, 24, 14, 32, 27, 3, 9, 19,
    13, 30, 6, 22, 11, 4, 25,
];
const PC1: [u8; 56] = [
    57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59, 51, 43, 35, 27, 19, 11, 3, 60,
    52, 44, 36, 63, 55, 47, 39, 31, 23, 15, 7, 62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29,
    21, 13, 5, 28, 20, 12, 4,
];
const PC2: [u8; 48] = [
    14, 17, 11, 24, 1, 5, 3, 28, 15, 6, 21, 10, 23, 19, 12, 4, 26, 8, 16, 7, 27, 20, 13, 2, 41, 52,
    31, 37, 47, 55, 30, 40, 51, 45, 33, 48, 44, 49, 39, 56, 34, 53, 46, 42, 50, 36, 29, 32,
];
const SBOXES: [[u8; 64]; 8] = [
    [
        14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7, 0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12,
        11, 9, 5, 3, 8, 4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0, 15, 12, 8, 2, 4, 9,
        1, 7, 5, 11, 3, 14, 10, 0, 6, 13,
    ],
    [
        15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10, 3, 13, 4, 7, 15, 2, 8, 14, 12, 0, 1,
        10, 6, 9, 11, 5, 0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15, 13, 8, 10, 1, 3, 15,
        4, 2, 11, 6, 7, 12, 0, 5, 14, 9,
    ],
    [
        10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8, 13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5,
        14, 12, 11, 15, 1, 13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7, 1, 10, 13, 0, 6,
        9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12,
    ],
    [
        7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15, 13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2,
        12, 1, 10, 14, 9, 10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4, 3, 15, 0, 6, 10, 1,
        13, 8, 9, 4, 5, 11, 12, 7, 2, 14,
    ],
    [
        2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9, 14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15,
        10, 3, 9, 8, 6, 4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14, 11, 8, 12, 7, 1, 14,
        2, 13, 6, 15, 0, 9, 10, 4, 5, 3,
    ],
    [
        12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11, 10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13,
        14, 0, 11, 3, 8, 9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6, 4, 3, 2, 12, 9, 5,
        15, 10, 11, 14, 1, 7, 6, 0, 8, 13,
    ],
    [
        4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1, 13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5,
        12, 2, 15, 8, 6, 1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2, 6, 11, 13, 8, 1, 4,
        10, 7, 9, 5, 0, 15, 14, 2, 3, 12,
    ],
    [
        13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7, 1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6,
        11, 0, 14, 9, 2, 7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8, 2, 1, 14, 7, 4, 10,
        8, 13, 15, 12, 9, 0, 3, 5, 6, 11,
    ],
];

#[cfg(test)]
#[path = "gamecube_wii_ar_decrypt/tests.rs"]
mod tests;
