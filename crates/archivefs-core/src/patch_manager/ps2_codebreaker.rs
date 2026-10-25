//! Preview-only PS2 CodeBreaker → PCSX2 PNACH translation.
//!
//! The user pastes encrypted CodeBreaker lines (`XXXXXXXX YYYYYYYY`) and names
//! the CodeBreaker generation they came from. The lines are decrypted with the
//! `codebreaker` crate (MIT OR Apache-2.0), and only provably equivalent direct
//! writes are translated into PNACH `patch=` lines through the existing neutral
//! IR encoder. Nothing here writes a file, enables a cheat, or contacts a
//! provider; the result is a preview that the existing PCSX2 install pipeline
//! could stage later.
//!
//! # What is translated
//!
//! Decrypted command types `0` (8-bit), `1` (16-bit) and `2` (32-bit) write a
//! constant to `addr & 0x0FFFFFFF`; PCSX2's own `extended` handler decodes them
//! the same way. A device re-applies such a code every frame, which is PNACH
//! `place` 1, the place the shared IR encoder already emits. A write is only
//! translated when it lies in EE RAM (below 32 MiB), is naturally aligned and
//! carries no value bits outside its width.
//!
//! # What is refused
//!
//! The whole code set is refused, not partially translated, when any record is
//! not a direct write. Dropping a conditional while keeping the writes behind
//! it would change what the cheat does. Increment/decrement, slide, copy,
//! pointer, boolean, conditional (`C`, `D`, `E`), `F` and other control types
//! and unknown command types are therefore refusals, as are mid-list key
//! changes.
//!
//! # The one exclusion: type 9 hooks
//!
//! A positively identified type `9` code (`9aaaaaaa vvvvvvvv`, one line,
//! 25-bit address) is the cheat engine's hook: it patches a `jal` into the
//! game so the device's engine runs, and is "essential for most of the other
//! types to work" on hardware (ps2rd `Documentation/code_types.txt`). PCSX2
//! applies PNACH patches itself, so the hook has no PNACH equivalent, and its
//! official PNACH documentation lists `9aaaaaaa` as not supported. A list
//! made only of hooks and direct writes can therefore be previewed with the
//! hooks excluded, but only when the caller passes the exact line numbers of
//! every excluded hook as an acknowledgement. Without it the result is
//! [`Ps2CodebreakerOutcome::AcknowledgementRequired`] and no PNACH text is
//! produced. The hook lines stay verbatim in the records. What is lost is the
//! hook's own value check (a guard that the running game matches the list),
//! and the device's timing; the selected game's verified CRC is the only guard
//! left. Hooks next to any other non-write record keep the list refused.
//!
//! # Versions and key sequences
//!
//! The version is always chosen by the user; it is never guessed and the
//! decoder never falls back from one version to another.
//!
//! - [`Ps2CodebreakerVersion::V1ToV6`] decrypts every line independently. A
//!   list whose decrypted form contains `BEEFC0DE`/`BEEFC0DF` is a V7+ list and
//!   is refused as a version mismatch.
//! - [`Ps2CodebreakerVersion::V7WithSetupLine`] requires the first line to be
//!   a V1-encrypted `BEEFC0DE` key-setup line (for example `B4336FA9 4DFEFB79`)
//!   and refuses a list without one.
//! - [`Ps2CodebreakerVersion::V7CommonKey`] decrypts with the common key
//!   published on CMGSCCC.com, for lists that omit the setup line. Any
//!   `BEEFC0DE`/`BEEFC0DF` line is refused.
//!
//! Every re-key after the single leading setup line, and every `BEEFC0DF`, is
//! refused. Validation against the original C implementation found that the
//! `codebreaker` crate disagrees with it for V7 streams that re-key (including
//! a `BEEFC0DE` line under the common key), so those sequences cannot be
//! established reliably. The accepted shapes (V1 per-line, a leading setup
//! line, and the common key with no key lines) matched on every tested line.
//!
//! A wrong version usually decrypts to a non-direct-write or out-of-RAM record
//! and is refused. A single wrong-version 32-bit record can still look like a
//! plausible write, so the decrypted values are always shown for review.
//!
//! CodeBreaker lines carry no game identity. The preview is bound to the
//! selected game's verified PCSX2 serial/CRC; an optional declared source
//! game must agree with it.
//!
//! See `docs/research/PS2_CODEBREAKER_PNACH_PREVIEW.md` for the validation
//! evidence.

use std::fmt;

use codebreaker::{Codebreaker, cb1};
use sha2::{Digest, Sha256};

use super::cheat_ir::{CheatOperation, CheatTargetFormat, encode_operation, pnach_line_to_ir};
use super::pcsx2::{normalize_crc, normalize_serial};
use super::pcsx2_identity::Pcsx2GameIdentity;
use super::pcsx2_install_plan::pcsx2_pnach_filename;
use super::pcsx2_pnach::{
    ManagedPnachCheat, PnachPatchLine, merge_managed_pnach_cheats, parse_pnach_document,
};

pub const PS2_CODEBREAKER_PREVIEW_VERSION: &str = "emuwiz-ps2-codebreaker-preview-v1";
pub const PS2_CODEBREAKER_CIPHER_CRATE: &str = "codebreaker 0.4.0 (MIT OR Apache-2.0)";
pub const MAX_CODEBREAKER_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_CODEBREAKER_LINES: usize = 512;
const MAX_CHEAT_NAME_CHARS: usize = 80;
/// PS2 EE RAM is 32 MiB; anything above is not a plain RAM write.
const EE_RAM_LIMIT: u32 = 0x0200_0000;
const BEEFCODE: u32 = 0xBEEF_C0DE;
const BEEFCODF: u32 = 0xBEEF_C0DF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ps2CodebreakerVersion {
    V1ToV6,
    V7WithSetupLine,
    V7CommonKey,
}

impl Ps2CodebreakerVersion {
    pub fn label(self) -> &'static str {
        match self {
            Self::V1ToV6 => "CodeBreaker v1-v6",
            Self::V7WithSetupLine => "CodeBreaker v7+ (setup line)",
            Self::V7CommonKey => "CodeBreaker v7+ (common key)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ps2CodebreakerKeyMode {
    /// V1 per-line cipher; no key sequence.
    V1ToV6,
    /// A leading V1-encrypted `BEEFC0DE` line supplied the V7 key seed.
    V7SetupLine { seed: u32 },
    /// The user chose the common CMGSCCC.com V7 key; there is no setup line.
    V7CommonKey,
}

/// Game the code list claims to belong to, as typed by the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ps2CodebreakerDeclaredGame {
    pub serial: Option<String>,
    pub crc: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ps2CodebreakerError {
    Empty,
    InputTooLarge,
    TooManyLines,
    NonAsciiInput,
    MalformedLine { line: usize },
    InvalidName,
    IdentityUnresolved,
    IdentityConflict { detail: String },
    Pnach { detail: String },
}

impl fmt::Display for Ps2CodebreakerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("no CodeBreaker code lines were supplied"),
            Self::InputTooLarge => write!(
                formatter,
                "CodeBreaker input exceeds {MAX_CODEBREAKER_INPUT_BYTES} bytes"
            ),
            Self::TooManyLines => write!(
                formatter,
                "CodeBreaker input exceeds {MAX_CODEBREAKER_LINES} lines"
            ),
            Self::NonAsciiInput => formatter.write_str("CodeBreaker input must be ASCII text"),
            Self::MalformedLine { line } => write!(
                formatter,
                "line {line} must be exactly two 8-digit hexadecimal words (XXXXXXXX YYYYYYYY); \
                 titles, comments and mixed text are refused"
            ),
            Self::InvalidName => write!(
                formatter,
                "the cheat name must be 1-{MAX_CHEAT_NAME_CHARS} printable characters"
            ),
            Self::IdentityUnresolved => {
                formatter.write_str("the selected game has no verified PCSX2 executable CRC yet")
            }
            Self::IdentityConflict { detail } => formatter.write_str(detail),
            Self::Pnach { detail } => write!(formatter, "PNACH preview failed: {detail}"),
        }
    }
}

impl std::error::Error for Ps2CodebreakerError {}

/// Decrypted command family of a record that is not a direct write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ps2CodebreakerFamily {
    IncrementDecrement,
    Slide,
    Copy,
    Pointer,
    BooleanOp,
    Conditional,
    /// Master, hook, delay and other control types (8, 9, A, B, C, F).
    Control,
}

impl Ps2CodebreakerFamily {
    fn of(address: u32) -> Option<Self> {
        match address >> 28 {
            0..=2 => None,
            3 => Some(Self::IncrementDecrement),
            4 => Some(Self::Slide),
            5 => Some(Self::Copy),
            6 => Some(Self::Pointer),
            7 => Some(Self::BooleanOp),
            0xD | 0xE => Some(Self::Conditional),
            _ => Some(Self::Control),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::IncrementDecrement => "increment/decrement",
            Self::Slide => "slide (multi-write)",
            Self::Copy => "memory copy",
            Self::Pointer => "pointer",
            Self::BooleanOp => "boolean (OR/AND/XOR)",
            Self::Conditional => "conditional",
            Self::Control => "master, hook, delay or other control",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ps2CodebreakerRefusal {
    NotDirectWrite {
        family: Ps2CodebreakerFamily,
    },
    AddressOutsideEeRam {
        address: u32,
    },
    UnalignedAddress {
        address: u32,
        width_bytes: u8,
    },
    NoncanonicalValue {
        value: u32,
        width_bytes: u8,
    },
    /// Decrypted list contains a V7 key line but V1-V6 was selected.
    VersionMismatch,
    /// A key line that is not the single leading setup line.
    UnvalidatedKeySequence,
    /// `V7WithSetupLine` was chosen but the first line is not a setup line.
    MissingSetupLine,
    /// The same address is written twice with different values.
    ConflictingWrites {
        address: u32,
    },
    /// A multi-line code is cut off at the end of the list.
    TruncatedCode,
}

impl Ps2CodebreakerRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::NotDirectWrite { family } => format!(
                "{} codes are not translated; the whole set is refused so no behaviour is dropped",
                family.label()
            ),
            Self::AddressOutsideEeRam { address } => {
                format!("address {address:08X} is outside the 32 MiB EE RAM range")
            }
            Self::UnalignedAddress {
                address,
                width_bytes,
            } => format!("address {address:08X} is not aligned to {width_bytes} bytes"),
            Self::NoncanonicalValue { value, width_bytes } => {
                format!("value {value:08X} has bits outside its {width_bytes}-byte width")
            }
            Self::VersionMismatch => "the decrypted list switches to CodeBreaker v7+ keys; \
                                      choose v7+ or check the source"
                .to_string(),
            Self::UnvalidatedKeySequence => "a mid-list re-key or BEEFC0DF line is not validated \
                                             (the cipher crate and the original C implementation \
                                             disagree there) and is refused"
                .to_string(),
            Self::MissingSetupLine => "v7+ (setup line) was chosen but the first line is not a \
                                       V1-encrypted BEEFC0DE key line; no other version is tried"
                .to_string(),
            Self::ConflictingWrites { address } => {
                format!("address {address:08X} is written more than once with different values")
            }
            Self::TruncatedCode => {
                "a multi-line code is cut off at the end of the list".to_string()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ps2CodebreakerRole {
    /// The leading V7 key-setup line; consumed as a key, not a cheat.
    KeySetup,
    Translated {
        pnach_line: String,
    },
    /// A type 9 hook: kept verbatim, left out of the PNACH, and only after
    /// the user acknowledges this exact line.
    ExcludedHook,
    /// A further line of the multi-line code started at `of_line`.
    Continuation {
        of_line: usize,
    },
    Refused {
        reason: Ps2CodebreakerRefusal,
    },
    /// Not decrypted because an earlier key sequence was refused.
    NotDecrypted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ps2CodebreakerRecord {
    /// 1-based line of the pasted text.
    pub line: usize,
    /// The pasted line, trimmed of surrounding whitespace only.
    pub original: String,
    pub encrypted: (u32, u32),
    pub decrypted: Option<(u32, u32)>,
    pub role: Ps2CodebreakerRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ps2CodebreakerProvenance {
    pub decoder_version: &'static str,
    pub cipher_crate: &'static str,
    pub version: Ps2CodebreakerVersion,
    pub key_mode: Ps2CodebreakerKeyMode,
    /// SHA-256 of the trimmed code lines joined with `\n`.
    pub input_sha256: String,
    pub bound_serial: Option<String>,
    pub bound_crc: String,
    /// The code lines carry no game identity, so this is an assertion by the
    /// user about the selected game, not evidence from the source.
    pub association_note: &'static str,
}

/// A hook line left out of the PNACH, with the pasted original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ps2CodebreakerExcludedHook {
    pub line: usize,
    pub original: String,
    pub decrypted: (u32, u32),
}

pub const PS2_CODEBREAKER_HOOK_EXCLUSION_RATIONALE: &str = "A type 9 code is the cheat engine's \
    hook: on a real CodeBreaker it makes the device run the cheats. PCSX2 applies PNACH patches \
    itself, so the hook has no PNACH equivalent. Its value check, which guards that the game \
    matches the list, and the device's timing are not reproduced; the selected game's verified \
    CRC is the only guard.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ps2CodebreakerOutcome {
    /// The remaining writes can be previewed, but only after the user
    /// acknowledges the hooks that would be left out. No PNACH text yet.
    AcknowledgementRequired {
        excluded_hooks: Vec<Ps2CodebreakerExcludedHook>,
        rationale: &'static str,
    },
    Translatable {
        destination_file_name: String,
        /// Hooks left out of `cheat`, each acknowledged by line number.
        excluded_hooks: Vec<Ps2CodebreakerExcludedHook>,
        cheat: ManagedPnachCheat,
        /// Exact bytes a fresh PNACH file would contain after install.
        pnach_preview: String,
    },
    Refused {
        reasons: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ps2CodebreakerPreview {
    pub provenance: Ps2CodebreakerProvenance,
    pub records: Vec<Ps2CodebreakerRecord>,
    pub outcome: Ps2CodebreakerOutcome,
}

struct ParsedLine {
    line: usize,
    original: String,
    encrypted: (u32, u32),
}

pub fn preview_ps2_codebreaker(
    input: &str,
    version: Ps2CodebreakerVersion,
    cheat_name: &str,
    identity: &Pcsx2GameIdentity,
    declared: Option<&Ps2CodebreakerDeclaredGame>,
    acknowledged_hook_lines: &[usize],
) -> Result<Ps2CodebreakerPreview, Ps2CodebreakerError> {
    preview_with_cipher(
        &CrateCipher,
        input,
        version,
        cheat_name,
        identity,
        declared,
        acknowledged_hook_lines,
    )
}

/// The one place the `codebreaker` crate is called. The acceptance policy
/// below never relies on what the cipher returns for lines after a key line,
/// so tests substitute scripted ciphers to prove that rejection does not
/// depend on the dependency's behaviour.
trait CipherBackend {
    /// V1-V6 decryption of one line (stateless).
    fn v1_line(&self, address: u32, value: u32) -> (u32, u32);
    /// A V7 stream: `with_setup` starts from the V1 cipher and switches at a
    /// `BEEFC0DE` line, otherwise the common CMGSCCC.com key is used.
    fn v7_stream(&self, with_setup: bool) -> Box<dyn FnMut(u32, u32) -> (u32, u32)>;
}

struct CrateCipher;

impl CipherBackend for CrateCipher {
    fn v1_line(&self, address: u32, value: u32) -> (u32, u32) {
        cb1::decrypt_code(address, value)
    }

    fn v7_stream(&self, with_setup: bool) -> Box<dyn FnMut(u32, u32) -> (u32, u32)> {
        let mut cb = if with_setup {
            Codebreaker::new()
        } else {
            Codebreaker::new_v7()
        };
        Box::new(move |address, value| cb.decrypt_code(address, value))
    }
}

fn preview_with_cipher(
    cipher: &dyn CipherBackend,
    input: &str,
    version: Ps2CodebreakerVersion,
    cheat_name: &str,
    identity: &Pcsx2GameIdentity,
    declared: Option<&Ps2CodebreakerDeclaredGame>,
    acknowledged_hook_lines: &[usize],
) -> Result<Ps2CodebreakerPreview, Ps2CodebreakerError> {
    let name = cheat_name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_CHEAT_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(Ps2CodebreakerError::InvalidName);
    }
    let lines = parse_lines(input)?;
    let (bound_serial, bound_crc) = bind_identity(identity, declared)?;

    let input_sha256 = {
        let joined = lines
            .iter()
            .map(|line| line.original.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let digest = Sha256::digest(joined.as_bytes());
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };

    let (mut records, key_mode) = decrypt_lines(cipher, &lines, version);
    classify_records(&mut records);

    let provenance = Ps2CodebreakerProvenance {
        decoder_version: PS2_CODEBREAKER_PREVIEW_VERSION,
        cipher_crate: PS2_CODEBREAKER_CIPHER_CRATE,
        version,
        key_mode,
        input_sha256: input_sha256.clone(),
        bound_serial: bound_serial.clone(),
        bound_crc: bound_crc.clone(),
        association_note: "CodeBreaker lines carry no game identity; the association is the \
                           user's assertion checked only against the selected game's verified \
                           serial/CRC",
    };

    let mut reasons = Vec::new();
    for record in &records {
        if let Ps2CodebreakerRole::Refused { reason } = &record.role {
            reasons.push(format!("line {}: {}", record.line, reason.message()));
        }
    }
    let patch_lines = records
        .iter()
        .filter_map(|record| match &record.role {
            Ps2CodebreakerRole::Translated { pnach_line } => Some(pnach_line.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if reasons.is_empty() && patch_lines.is_empty() {
        reasons.push("the list contains no cheat records".to_string());
    }

    let excluded_hooks = records
        .iter()
        .filter(|record| record.role == Ps2CodebreakerRole::ExcludedHook)
        .map(|record| Ps2CodebreakerExcludedHook {
            line: record.line,
            original: record.original.clone(),
            decrypted: record.decrypted.unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    let mut acknowledged = acknowledged_hook_lines.to_vec();
    acknowledged.sort_unstable();
    acknowledged.dedup();
    let hook_lines = excluded_hooks
        .iter()
        .map(|hook| hook.line)
        .collect::<Vec<_>>();

    let outcome = if reasons.is_empty() && acknowledged != hook_lines {
        // Also covers an acknowledgement naming lines that are not hooks.
        Ps2CodebreakerOutcome::AcknowledgementRequired {
            excluded_hooks,
            rationale: PS2_CODEBREAKER_HOOK_EXCLUSION_RATIONALE,
        }
    } else if reasons.is_empty() {
        let destination_file_name = pcsx2_pnach_filename(bound_serial.as_deref(), &bound_crc)
            .map_err(|error| Ps2CodebreakerError::Pnach {
                detail: error.to_string(),
            })?;
        let patch_lines = patch_lines
            .iter()
            .map(|line| PnachPatchLine::parse(line))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| Ps2CodebreakerError::Pnach {
                detail: error.detail,
            })?;
        let cheat = ManagedPnachCheat {
            id: format!("cb-{}", &input_sha256[..16]),
            name: name.to_string(),
            description: Some(format!(
                "Translated from {} codes (source sha256 {})",
                version.label(),
                &input_sha256[..16]
            )),
            patch_lines,
        };
        let empty = parse_pnach_document(b"").map_err(|error| Ps2CodebreakerError::Pnach {
            detail: error.detail,
        })?;
        let bytes =
            merge_managed_pnach_cheats(&empty, std::slice::from_ref(&cheat)).map_err(|error| {
                Ps2CodebreakerError::Pnach {
                    detail: error.detail,
                }
            })?;
        let pnach_preview =
            String::from_utf8(bytes).map_err(|error| Ps2CodebreakerError::Pnach {
                detail: error.to_string(),
            })?;
        Ps2CodebreakerOutcome::Translatable {
            destination_file_name,
            excluded_hooks,
            cheat,
            pnach_preview,
        }
    } else {
        Ps2CodebreakerOutcome::Refused { reasons }
    };

    Ok(Ps2CodebreakerPreview {
        provenance,
        records,
        outcome,
    })
}

fn parse_lines(input: &str) -> Result<Vec<ParsedLine>, Ps2CodebreakerError> {
    if input.len() > MAX_CODEBREAKER_INPUT_BYTES {
        return Err(Ps2CodebreakerError::InputTooLarge);
    }
    if !input.is_ascii() {
        return Err(Ps2CodebreakerError::NonAsciiInput);
    }
    let mut parsed = Vec::new();
    for (index, raw) in input.lines().enumerate() {
        let text = raw.trim();
        if text.is_empty() {
            continue;
        }
        if parsed.len() >= MAX_CODEBREAKER_LINES {
            return Err(Ps2CodebreakerError::TooManyLines);
        }
        let line = index + 1;
        let mut words = text.split_ascii_whitespace();
        let (Some(address), Some(value), None) = (words.next(), words.next(), words.next()) else {
            return Err(Ps2CodebreakerError::MalformedLine { line });
        };
        let hex = |word: &str| {
            (word.len() == 8 && word.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then(|| u32::from_str_radix(word, 16).ok())
                .flatten()
        };
        let (Some(address), Some(value)) = (hex(address), hex(value)) else {
            return Err(Ps2CodebreakerError::MalformedLine { line });
        };
        parsed.push(ParsedLine {
            line,
            original: text.to_string(),
            encrypted: (address, value),
        });
    }
    if parsed.is_empty() {
        return Err(Ps2CodebreakerError::Empty);
    }
    Ok(parsed)
}

fn bind_identity(
    identity: &Pcsx2GameIdentity,
    declared: Option<&Ps2CodebreakerDeclaredGame>,
) -> Result<(Option<String>, String), Ps2CodebreakerError> {
    let verified = identity
        .verified_crc()
        .and_then(normalize_crc)
        .ok_or(Ps2CodebreakerError::IdentityUnresolved)?;
    let serial = identity.serial.as_deref().and_then(normalize_serial);
    if let Some(declared) = declared {
        if let Some(crc) = declared.crc.as_deref() {
            let Some(crc) = normalize_crc(crc) else {
                return Err(Ps2CodebreakerError::IdentityConflict {
                    detail: format!("declared CRC {crc} is not eight hexadecimal digits"),
                });
            };
            if crc != verified {
                return Err(Ps2CodebreakerError::IdentityConflict {
                    detail: format!(
                        "the codes are declared for CRC {crc}, which does not match the selected \
                         game's verified CRC {verified}"
                    ),
                });
            }
        }
        if let Some(declared_serial) = declared.serial.as_deref() {
            let Some(declared_serial) = normalize_serial(declared_serial) else {
                return Err(Ps2CodebreakerError::IdentityConflict {
                    detail: format!("declared serial {declared_serial} is not a PS2 serial"),
                });
            };
            if serial.as_deref() != Some(declared_serial.as_str()) {
                return Err(Ps2CodebreakerError::IdentityConflict {
                    detail: format!(
                        "the codes are declared for serial {declared_serial}, which does not \
                         match the selected game's verified serial {}",
                        serial.as_deref().unwrap_or("(none)")
                    ),
                });
            }
        }
    }
    Ok((serial, verified))
}

fn record(
    line: &ParsedLine,
    decrypted: Option<(u32, u32)>,
    role: Ps2CodebreakerRole,
) -> Ps2CodebreakerRecord {
    Ps2CodebreakerRecord {
        line: line.line,
        original: line.original.clone(),
        encrypted: line.encrypted,
        decrypted,
        role,
    }
}

/// Decrypts in list order. Records the decrypted pair; classification of the
/// pairs happens afterwards. Roles set here are only the key-sequence ones.
fn decrypt_lines(
    cipher: &dyn CipherBackend,
    lines: &[ParsedLine],
    version: Ps2CodebreakerVersion,
) -> (Vec<Ps2CodebreakerRecord>, Ps2CodebreakerKeyMode) {
    // Placeholder role for "decrypted, awaiting classification".
    let pending = Ps2CodebreakerRole::Continuation { of_line: 0 };
    let mut records = Vec::with_capacity(lines.len());
    match version {
        Ps2CodebreakerVersion::V1ToV6 => {
            for (index, line) in lines.iter().enumerate() {
                let (address, value) = cipher.v1_line(line.encrypted.0, line.encrypted.1);
                if is_beefcode(address) {
                    records.push(record(
                        line,
                        Some((address, value)),
                        Ps2CodebreakerRole::Refused {
                            reason: Ps2CodebreakerRefusal::VersionMismatch,
                        },
                    ));
                    push_undecrypted(&mut records, &lines[index + 1..]);
                    break;
                }
                records.push(record(line, Some((address, value)), pending.clone()));
            }
            (records, Ps2CodebreakerKeyMode::V1ToV6)
        }
        Ps2CodebreakerVersion::V7WithSetupLine | Ps2CodebreakerVersion::V7CommonKey => {
            let with_setup = version == Ps2CodebreakerVersion::V7WithSetupLine;
            let first = lines[0].encrypted;
            let probe = cipher.v1_line(first.0, first.1);
            if with_setup && probe.0 != BEEFCODE {
                let reason = if probe.0 == BEEFCODF {
                    Ps2CodebreakerRefusal::UnvalidatedKeySequence
                } else {
                    Ps2CodebreakerRefusal::MissingSetupLine
                };
                records.push(record(
                    &lines[0],
                    Some(probe),
                    Ps2CodebreakerRole::Refused { reason },
                ));
                push_undecrypted(&mut records, &lines[1..]);
                return (records, Ps2CodebreakerKeyMode::V7SetupLine { seed: 0 });
            }
            let mut stream = cipher.v7_stream(with_setup);
            let key_mode = if with_setup {
                Ps2CodebreakerKeyMode::V7SetupLine { seed: probe.1 }
            } else {
                Ps2CodebreakerKeyMode::V7CommonKey
            };
            for (index, line) in lines.iter().enumerate() {
                let (address, value) = stream(line.encrypted.0, line.encrypted.1);
                if with_setup && index == 0 {
                    records.push(record(
                        line,
                        Some((address, value)),
                        Ps2CodebreakerRole::KeySetup,
                    ));
                    continue;
                }
                if is_beefcode(address) {
                    records.push(record(
                        line,
                        Some((address, value)),
                        Ps2CodebreakerRole::Refused {
                            reason: Ps2CodebreakerRefusal::UnvalidatedKeySequence,
                        },
                    ));
                    push_undecrypted(&mut records, &lines[index + 1..]);
                    break;
                }
                records.push(record(line, Some((address, value)), pending.clone()));
            }
            (records, key_mode)
        }
    }
}

fn push_undecrypted(records: &mut Vec<Ps2CodebreakerRecord>, rest: &[ParsedLine]) {
    for line in rest {
        records.push(record(line, None, Ps2CodebreakerRole::NotDecrypted));
    }
}

/// A positively identified hook: command `9` with a clean 25-bit address.
/// Anything else with a `9` nibble is not recognised and stays refused.
fn is_hook(address: u32) -> bool {
    address >> 28 == 9 && address & 0x0E00_0000 == 0
}

fn is_beefcode(address: u32) -> bool {
    address & 0xFFFF_FFFE == BEEFCODE
}

/// Number of lines a decrypted code occupies, as in the original C
/// implementation (and matching PCSX2's two-line `3040`/`3050`, slide, copy
/// and pointer handlers). Only used to label continuation lines: any code
/// longer than one direct write already refuses the whole set.
fn code_line_count(address: u32) -> usize {
    match address >> 28 {
        3 if address & 0x0040_0000 != 0 => 2,
        4..=6 => 2,
        _ => 1,
    }
}

fn classify_records(records: &mut [Ps2CodebreakerRecord]) {
    let mut remaining_continuations = 0usize;
    let mut head_line = 0usize;
    for record in records.iter_mut() {
        // Only freshly decrypted records carry the placeholder role.
        if !matches!(record.role, Ps2CodebreakerRole::Continuation { of_line: 0 }) {
            continue;
        }
        let Some((address, value)) = record.decrypted else {
            continue;
        };
        if remaining_continuations > 0 {
            remaining_continuations -= 1;
            record.role = Ps2CodebreakerRole::Continuation { of_line: head_line };
            continue;
        }
        remaining_continuations = code_line_count(address) - 1;
        head_line = record.line;
        if is_hook(address) {
            record.role = Ps2CodebreakerRole::ExcludedHook;
            continue;
        }
        record.role = match translate_direct_write(address, value) {
            Ok(pnach_line) => Ps2CodebreakerRole::Translated { pnach_line },
            Err(reason) => Ps2CodebreakerRole::Refused { reason },
        };
    }
    check_whole_sequence(records, remaining_continuations);
}

/// Sequence-level checks that no single line can show.
fn check_whole_sequence(records: &mut [Ps2CodebreakerRecord], missing_continuations: usize) {
    if missing_continuations > 0
        && let Some(last_head) = records.iter_mut().rev().find(|record| {
            !matches!(
                record.role,
                Ps2CodebreakerRole::Continuation { .. } | Ps2CodebreakerRole::NotDecrypted
            )
        })
        && !matches!(last_head.role, Ps2CodebreakerRole::Refused { .. })
    {
        last_head.role = Ps2CodebreakerRole::Refused {
            reason: Ps2CodebreakerRefusal::TruncatedCode,
        };
    }
    // PNACH applies lines in order like the device does, so a repeated
    // address is only safe when every write to it agrees.
    let mut seen: Vec<(String, String)> = Vec::new();
    for record in records.iter_mut() {
        let Ps2CodebreakerRole::Translated { pnach_line } = &record.role else {
            continue;
        };
        let fields = pnach_line.split(',').collect::<Vec<_>>();
        let (address, rest) = (
            fields[2].to_string(),
            format!("{},{}", fields[3], fields[4]),
        );
        if seen
            .iter()
            .any(|(seen_address, seen_rest)| *seen_address == address && *seen_rest != rest)
        {
            record.role = Ps2CodebreakerRole::Refused {
                reason: Ps2CodebreakerRefusal::ConflictingWrites {
                    address: u32::from_str_radix(&address, 16).unwrap_or(0),
                },
            };
        } else {
            seen.push((address, rest));
        }
    }
}

fn translate_direct_write(address: u32, value: u32) -> Result<String, Ps2CodebreakerRefusal> {
    if let Some(family) = Ps2CodebreakerFamily::of(address) {
        return Err(Ps2CodebreakerRefusal::NotDirectWrite { family });
    }
    let (width_bytes, operation_for): (u8, fn(u32, u32) -> CheatOperation) = match address >> 28 {
        0 => (1, |a, v| CheatOperation::Write8 {
            address: u64::from(a),
            value: v as u8,
        }),
        1 => (2, |a, v| CheatOperation::Write16 {
            address: u64::from(a),
            value: v as u16,
        }),
        _ => (4, |a, v| CheatOperation::Write32 {
            address: u64::from(a),
            value: v,
        }),
    };
    let target = address & 0x0FFF_FFFF;
    if target >= EE_RAM_LIMIT {
        return Err(Ps2CodebreakerRefusal::AddressOutsideEeRam { address: target });
    }
    if !target.is_multiple_of(u32::from(width_bytes)) {
        return Err(Ps2CodebreakerRefusal::UnalignedAddress {
            address: target,
            width_bytes,
        });
    }
    let max = match width_bytes {
        1 => 0xFF,
        2 => 0xFFFF,
        _ => u32::MAX,
    };
    if value > max {
        return Err(Ps2CodebreakerRefusal::NoncanonicalValue { value, width_bytes });
    }
    let operation = operation_for(target, value);
    let line = encode_operation(&operation, &CheatTargetFormat::Pnach)
        .expect("direct writes always encode to PNACH");
    // The emitted text must parse back to the identical operation.
    debug_assert_eq!(pnach_line_to_ir(&line), operation);
    Ok(line)
}

#[cfg(test)]
mod tests;
