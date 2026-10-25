//! Vectors below were produced by the original MIT-licensed cb2util C
//! implementation (v1.9, `cb_encrypt_code`) from the plaintext shown in each
//! comment, and decrypt identically with the `codebreaker` crate for the
//! accepted shapes. See docs/research/PS2_CODEBREAKER_PNACH_PREVIEW.md.

use std::path::PathBuf;

use super::*;
use crate::patch_manager::pcsx2_identity::Pcsx2IdentityState;

const V1_DIRECT: &str = "2AFF014C 2411FFFF\n0AF344DE 000000FF\n1A0B132F 0000FFFF\n"; // 2043AFCC 2411FFFF / 0043AFD0 FF / 1043AFD2 FFFF
const V7_SETUP_DIRECT: &str =
    "B4336FA9 4DFEFB79\n397951B0 41569FE0\nF7B2B6BF F20729FD\n1C498585 7056D64A\n";
const V7_COMMON_DIRECT: &str = "397951B0 41569FE0\nF7B2B6BF F20729FD\n1C498585 7056D64A\n";
const V7_SETUP_SEED: &str = "B4336FA9 B83284E1\nC9AEFB03 4D3AC2BD\n"; // seed 12345678
const V1_POINTER: &str = "2AFF014C 2411FFFF\n6AE060AC 896952E1\n0A22080D 00000010\n";
const V7_CONDITIONAL: &str =
    "B4336FA9 4DFEFB79\n397951B0 41569FE0\nFE300E79 237D478A\n3E3161ED A8FE0C1D\n";
const V7_REKEY: &str =
    "B4336FA9 4DFEFB79\n397951B0 41569FE0\n6CD9EB96 0F617DA3\nF6854503 78B44B26\n";
// Common key, BEEFC0DE first: the crate and the C original disagree here.
const COMMON_KEY_REKEY: &str =
    "8787C575 1AC4C1B4\n02210430 184C16E8\n32E2A916 7E6017BA\nCBB720FD D61505E0\n";
const V7_DF: &str = "B4336FA9 4DFEFB79\nEF3A41A5 8F460BA4\n99AD6CE6 C5C9C4ED\n";
const V1_MASTER_FIRST: &str = "9A545CC6 188CBCFB\n2AFF014C 2411FFFF\n";
const V1_UNALIGNED: &str = "2AFE014C 00000001\n"; // 2043AFCD word
const V1_OUTSIDE_RAM: &str = "28FB014C 00000001\n"; // 2243AFD0 word
const V1_NONCANONICAL: &str = "0AF344DE 00000100\n"; // byte value 0x100
const V1_CONFLICT: &str = "2AFF014C 00000001\n2AFF014C 00000002\n";
const V1_TRUNCATED_SLIDE: &str = "2AFF014C 00000001\n4ADC7B2C C4F2182B\n";

fn identity(serial: Option<&str>, crc: Option<&str>) -> Pcsx2GameIdentity {
    Pcsx2GameIdentity {
        archive_path: PathBuf::from("/games/test.iso"),
        title: "Test Game".to_string(),
        region: None,
        serial: serial.map(str::to_string),
        executable_crc: crc.map(str::to_string),
        state: if crc.is_some() {
            Pcsx2IdentityState::Verified
        } else {
            Pcsx2IdentityState::MissingCrc
        },
        evidence: Vec::new(),
        plain_failure_reason: None,
    }
}

fn verified() -> Pcsx2GameIdentity {
    identity(Some("SLUS-20312"), Some("a1b2c3d4"))
}

fn preview(input: &str, version: Ps2CodebreakerVersion) -> Ps2CodebreakerPreview {
    preview_ps2_codebreaker(input, version, "Test cheat", &verified(), None, &[]).expect("preview")
}

fn translated_lines(preview: &Ps2CodebreakerPreview) -> Vec<String> {
    preview
        .records
        .iter()
        .filter_map(|record| match &record.role {
            Ps2CodebreakerRole::Translated { pnach_line } => Some(pnach_line.clone()),
            _ => None,
        })
        .collect()
}

fn refusals(preview: &Ps2CodebreakerPreview) -> Vec<Ps2CodebreakerRefusal> {
    preview
        .records
        .iter()
        .filter_map(|record| match &record.role {
            Ps2CodebreakerRole::Refused { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

fn assert_refused(preview: &Ps2CodebreakerPreview) {
    assert!(
        matches!(preview.outcome, Ps2CodebreakerOutcome::Refused { .. }),
        "expected a refusal, got {:?}",
        preview.outcome
    );
}

const EXPECTED_LINES: [&str; 3] = [
    "patch=1,EE,0043AFCC,word,2411FFFF",
    "patch=1,EE,0043AFD0,byte,FF",
    "patch=1,EE,0043AFD2,short,FFFF",
];

fn assert_translated_direct(preview: &Ps2CodebreakerPreview) {
    assert_eq!(translated_lines(preview), EXPECTED_LINES);
    let Ps2CodebreakerOutcome::Translatable {
        destination_file_name,
        cheat,
        pnach_preview,
        ..
    } = &preview.outcome
    else {
        panic!("expected translatable, got {:?}", preview.outcome);
    };
    assert_eq!(destination_file_name, "SLUS-20312_A1B2C3D4.pnach");
    assert_eq!(cheat.patch_lines.len(), 3);
    for line in EXPECTED_LINES {
        assert!(pnach_preview.contains(line), "{pnach_preview}");
    }
}

#[test]
fn v1_to_v6_direct_writes_translate_to_exact_pnach() {
    let result = preview(V1_DIRECT, Ps2CodebreakerVersion::V1ToV6);
    assert_translated_direct(&result);
    assert_eq!(result.provenance.key_mode, Ps2CodebreakerKeyMode::V1ToV6);
    assert_eq!(result.provenance.bound_crc, "A1B2C3D4");
    assert_eq!(
        result.provenance.bound_serial.as_deref(),
        Some("SLUS-20312")
    );
}

#[test]
fn v7_setup_line_and_common_key_decrypt_to_the_same_writes() {
    let setup = preview(V7_SETUP_DIRECT, Ps2CodebreakerVersion::V7WithSetupLine);
    assert_translated_direct(&setup);
    assert_eq!(
        setup.provenance.key_mode,
        Ps2CodebreakerKeyMode::V7SetupLine { seed: 0 }
    );
    assert_eq!(setup.records[0].role, Ps2CodebreakerRole::KeySetup);
    assert_eq!(setup.records[0].decrypted, Some((0xBEEF_C0DE, 0)));

    let common = preview(V7_COMMON_DIRECT, Ps2CodebreakerVersion::V7CommonKey);
    assert_translated_direct(&common);
    assert_eq!(
        common.provenance.key_mode,
        Ps2CodebreakerKeyMode::V7CommonKey
    );
}

#[test]
fn v7_setup_line_with_nonzero_seed_decrypts() {
    let result = preview(V7_SETUP_SEED, Ps2CodebreakerVersion::V7WithSetupLine);
    assert_eq!(
        result.provenance.key_mode,
        Ps2CodebreakerKeyMode::V7SetupLine { seed: 0x1234_5678 }
    );
    assert_eq!(
        translated_lines(&result),
        ["patch=1,EE,0043AFCC,word,2411FFFF"]
    );
}

#[test]
fn a_wrong_version_never_falls_back_to_another_one() {
    // v7 list read as v1-v6: the decrypted setup line is a version mismatch.
    let result = preview(V7_SETUP_DIRECT, Ps2CodebreakerVersion::V1ToV6);
    assert_refused(&result);
    assert_eq!(refusals(&result), [Ps2CodebreakerRefusal::VersionMismatch]);
    assert!(translated_lines(&result).is_empty());
    assert!(result.records[1..].iter().all(|record| record.role
        == Ps2CodebreakerRole::NotDecrypted
        && record.decrypted.is_none()));

    // v1 list read as "v7 with setup line": no setup line, nothing is tried.
    let result = preview(V1_DIRECT, Ps2CodebreakerVersion::V7WithSetupLine);
    assert_refused(&result);
    assert_eq!(refusals(&result), [Ps2CodebreakerRefusal::MissingSetupLine]);
    assert!(translated_lines(&result).is_empty());

    // v1 list read with the common key, and a v7 list read as v1.
    let result = preview(V1_DIRECT, Ps2CodebreakerVersion::V7CommonKey);
    assert_refused(&result);
    assert!(translated_lines(&result).is_empty());
    let result = preview(V7_COMMON_DIRECT, Ps2CodebreakerVersion::V1ToV6);
    assert_refused(&result);
    assert!(translated_lines(&result).is_empty());
}

#[test]
fn non_direct_write_families_refuse_the_whole_set() {
    let pointer = preview(V1_POINTER, Ps2CodebreakerVersion::V1ToV6);
    assert_refused(&pointer);
    assert_eq!(
        refusals(&pointer),
        [Ps2CodebreakerRefusal::NotDirectWrite {
            family: Ps2CodebreakerFamily::Pointer
        }]
    );
    // The leading direct write is not offered on its own, and the pointer's
    // second line is a continuation, not a separate record.
    assert_eq!(translated_lines(&pointer).len(), 1);
    assert!(matches!(
        pointer.records[2].role,
        Ps2CodebreakerRole::Continuation { of_line: 2 }
    ));

    let conditional = preview(V7_CONDITIONAL, Ps2CodebreakerVersion::V7WithSetupLine);
    assert_refused(&conditional);
    assert_eq!(
        refusals(&conditional),
        [Ps2CodebreakerRefusal::NotDirectWrite {
            family: Ps2CodebreakerFamily::Conditional
        }]
    );

    // `9029BEAC 0C0A9225` is a type 9 hook: previewable only after the user
    // acknowledges it (see the master-code tests below), never silently.
    let master = preview(V1_MASTER_FIRST, Ps2CodebreakerVersion::V1ToV6);
    assert!(matches!(
        master.outcome,
        Ps2CodebreakerOutcome::AcknowledgementRequired { .. }
    ));
    assert_eq!(master.records[0].role, Ps2CodebreakerRole::ExcludedHook);
}

#[test]
fn mid_list_rekey_and_beefcodf_are_refused() {
    let rekey = preview(V7_REKEY, Ps2CodebreakerVersion::V7WithSetupLine);
    assert_refused(&rekey);
    assert_eq!(
        refusals(&rekey),
        [Ps2CodebreakerRefusal::UnvalidatedKeySequence]
    );
    assert_eq!(rekey.records[3].role, Ps2CodebreakerRole::NotDecrypted);
    assert!(rekey.records[3].decrypted.is_none());

    let df = preview(V7_DF, Ps2CodebreakerVersion::V7WithSetupLine);
    assert_refused(&df);
    assert_eq!(
        refusals(&df),
        [Ps2CodebreakerRefusal::UnvalidatedKeySequence]
    );
}

/// A cipher that returns whatever the test scripts, standing in for any
/// version of the dependency.
struct ScriptedCipher {
    v1: fn(u32, u32) -> (u32, u32),
    /// Output for the n-th line of a V7 stream.
    stream: Vec<(u32, u32)>,
}

impl CipherBackend for ScriptedCipher {
    fn v1_line(&self, address: u32, value: u32) -> (u32, u32) {
        (self.v1)(address, value)
    }

    fn v7_stream(&self, _with_setup: bool) -> Box<dyn FnMut(u32, u32) -> (u32, u32)> {
        let mut outputs = self.stream.clone().into_iter();
        Box::new(move |address, value| outputs.next().unwrap_or((address, value)))
    }
}

fn preview_scripted(
    cipher: &ScriptedCipher,
    input: &str,
    version: Ps2CodebreakerVersion,
) -> Ps2CodebreakerPreview {
    preview_with_cipher(cipher, input, version, "Test cheat", &verified(), None, &[])
        .expect("preview")
}

/// The rejection of re-key shapes must not depend on what the dependency
/// returns, now or in a later version: it needs only that the key line
/// itself decrypts to `BEEFC0DE`, which the crate and the C original agree on.
/// Evidence of the observed disagreement is kept in
/// docs/research/CODEBREAKER_RS_C_DISCREPANCY.md.
#[test]
fn re_key_shapes_are_rejected_whatever_the_cipher_returns() {
    let key_line = (0xBEEF_C0DE, 0);
    // What the C original decrypts COMMON_KEY_REKEY to, a plausible "fixed
    // crate", and a crate returning something else entirely after the key.
    let c_original = [
        key_line,
        (0x9029_BEAC, 0x0C0A_9225),
        (0x201F_6024, 0),
        (0x2096_F5B8, 0xBE),
    ];
    let fixed_crate = [
        key_line,
        (0x2043_AFCC, 0x2411_FFFF),
        (0x0043_AFD0, 0xFF),
        (0x1043_AFD2, 0xFFFF),
    ];
    let other = [
        key_line,
        (0x2043_AFCC, 1),
        (0x2043_AFD0, 2),
        (0x2043_AFD4, 3),
    ];
    for stream in [c_original.to_vec(), fixed_crate.to_vec(), other.to_vec()] {
        let cipher = ScriptedCipher {
            v1: |a, v| (a, v),
            stream,
        };
        let result = preview_scripted(
            &cipher,
            COMMON_KEY_REKEY,
            Ps2CodebreakerVersion::V7CommonKey,
        );
        assert_refused(&result);
        assert!(translated_lines(&result).is_empty());
        assert_eq!(
            refusals(&result),
            [Ps2CodebreakerRefusal::UnvalidatedKeySequence]
        );
        // Nothing after the key line is decrypted, so nothing after it can
        // be interpreted, whatever the cipher would have said.
        assert!(
            result.records[1..]
                .iter()
                .all(|record| record.decrypted.is_none())
        );
    }

    // A second key line after the setup line, and BEEFC0DF, for the setup mode.
    for (late_key, input) in [((0xBEEF_C0DE, 7), V7_REKEY), ((0xBEEF_C0DF, 0), V7_DF)] {
        let cipher = ScriptedCipher {
            v1: |_, _| (0xBEEF_C0DE, 0),
            stream: vec![
                (0xBEEF_C0DE, 0),
                (0x2043_AFCC, 1),
                late_key,
                (0x2043_AFD4, 2),
            ],
        };
        let result = preview_scripted(&cipher, input, Ps2CodebreakerVersion::V7WithSetupLine);
        assert_refused(&result);
        assert!(refusals(&result).contains(&Ps2CodebreakerRefusal::UnvalidatedKeySequence));
    }
}

/// With the real crate: the same shape is refused end to end.
#[test]
fn common_key_with_a_rekey_line_is_refused_with_the_real_crate() {
    let result = preview(COMMON_KEY_REKEY, Ps2CodebreakerVersion::V7CommonKey);
    assert_refused(&result);
    assert_eq!(
        refusals(&result),
        [Ps2CodebreakerRefusal::UnvalidatedKeySequence]
    );
}

/// Independent model of what PCSX2 does with an emitted `patch=` line
/// (`Patch::PatchFunc::patch` parses place/cpu/hex address/type/hex data;
/// `ApplyPatch` does `IdempotentWrite8/16/32(addr, data)`) and of what the
/// CodeBreaker raw type 0/1/2 command means (PCSX2 `handle_extended_t`
/// writes `addr & 0x0FFFFFFF`, width from the command nibble).
fn pcsx2_patch_effect(line: &str) -> (u32, u32, u32) {
    let fields = line
        .strip_prefix("patch=")
        .expect("prefix")
        .split(',')
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 5, "{line}");
    assert_eq!(
        fields[0], "1",
        "place 1 = continuously, like a cheat device"
    );
    assert_eq!(fields[1], "EE");
    let address = u32::from_str_radix(fields[2], 16).unwrap();
    let (width, digits) = match fields[3] {
        "byte" => (1, 2),
        "short" => (2, 4),
        "word" => (4, 8),
        other => panic!("unexpected width {other}"),
    };
    assert_eq!(fields[2].len(), 8, "{line}");
    assert_eq!(fields[4].len(), digits, "fixed-width value in {line}");
    assert!(
        line.bytes()
            .all(|byte| !byte.is_ascii_lowercase() || b"patch=ybtesortwd".contains(&byte)),
        "hex digits are uppercase: {line}"
    );
    (width, address, u32::from_str_radix(fields[4], 16).unwrap())
}

fn cb_raw_effect(address: u32, value: u32) -> (u32, u32, u32) {
    match address >> 28 {
        0 => (1, address & 0x0FFF_FFFF, value & 0xFF),
        1 => (2, address & 0x0FFF_FFFF, value & 0xFFFF),
        2 => (4, address & 0x0FFF_FFFF, value),
        _ => panic!("not a write"),
    }
}

#[test]
fn every_accepted_direct_write_matches_the_pcsx2_effect_exactly() {
    let values = [
        0u32,
        1,
        0x7F,
        0x80,
        0xFF,
        0x100,
        0x7FFF,
        0xFFFF,
        0x1_0000,
        u32::MAX,
    ];
    let offsets = [
        0u32, 1, 2, 3, 4, 6, 8, 0xFFFC, 0x43_AFCC, 0x1FF_FFF8, 0x1FF_FFFC, 0x1FF_FFFD, 0x1FF_FFFE,
        0x1FF_FFFF, 0x200_0000, 0x200_0004, 0xFFF_FFFC, 0xFFF_FFFF,
    ];
    let (mut accepted, mut refused) = (0, 0);
    for command in 0u32..=2 {
        let width = 1u32 << command;
        for offset in offsets {
            for value in values {
                let address = (command << 28) | offset;
                let in_ram = offset < 0x0200_0000;
                let aligned = offset % width == 0;
                let fits = u64::from(value) < (1u64 << (8 * width));
                match translate_direct_write(address, value) {
                    Ok(line) => {
                        assert!(in_ram && aligned && fits, "{address:08X} {value:08X}");
                        assert_eq!(
                            pcsx2_patch_effect(&line),
                            cb_raw_effect(address, value),
                            "{line}"
                        );
                        // Exact text, independently spelled out.
                        let name = ["byte", "short", "word"][command as usize];
                        let digits = 2 * width as usize;
                        assert_eq!(
                            line,
                            format!("patch=1,EE,{offset:08X},{name},{value:0digits$X}")
                        );
                        PnachPatchLine::parse(&line).expect("accepted by the managed validator");
                        accepted += 1;
                    }
                    Err(reason) => {
                        assert!(!(in_ram && aligned && fits), "{address:08X} {value:08X}");
                        let expected_kind = if !in_ram {
                            matches!(reason, Ps2CodebreakerRefusal::AddressOutsideEeRam { .. })
                        } else if !aligned {
                            matches!(reason, Ps2CodebreakerRefusal::UnalignedAddress { .. })
                        } else {
                            matches!(reason, Ps2CodebreakerRefusal::NoncanonicalValue { .. })
                        };
                        assert!(expected_kind, "{address:08X} {value:08X} {reason:?}");
                        refused += 1;
                    }
                }
            }
        }
    }
    assert!(accepted > 100 && refused > 100, "{accepted} {refused}");
}

#[test]
fn high_address_bits_other_than_the_command_never_survive() {
    // Command nibble 0/1/2 plus address bits 24-27 above the RAM limit are
    // refused, not masked into RAM.
    assert!(matches!(
        translate_direct_write(0x2200_0000, 1),
        Err(Ps2CodebreakerRefusal::AddressOutsideEeRam {
            address: 0x0200_0000
        })
    ));
    assert!(matches!(
        translate_direct_write(0x0FFF_FFFC, 1),
        Err(Ps2CodebreakerRefusal::AddressOutsideEeRam { .. })
    ));
}

#[test]
fn bad_addresses_and_values_are_refused_with_reasons() {
    let unaligned = preview(V1_UNALIGNED, Ps2CodebreakerVersion::V1ToV6);
    assert_eq!(
        refusals(&unaligned),
        [Ps2CodebreakerRefusal::UnalignedAddress {
            address: 0x0043_AFCD,
            width_bytes: 4
        }]
    );
    let outside = preview(V1_OUTSIDE_RAM, Ps2CodebreakerVersion::V1ToV6);
    assert_eq!(
        refusals(&outside),
        [Ps2CodebreakerRefusal::AddressOutsideEeRam {
            address: 0x0243_AFD0
        }]
    );
    let noncanonical = preview(V1_NONCANONICAL, Ps2CodebreakerVersion::V1ToV6);
    assert_eq!(
        refusals(&noncanonical),
        [Ps2CodebreakerRefusal::NoncanonicalValue {
            value: 0x100,
            width_bytes: 1
        }]
    );
    for result in [&unaligned, &outside, &noncanonical] {
        assert_refused(result);
        assert!(translated_lines(result).is_empty());
    }
}

#[test]
fn sequence_level_checks_catch_conflicts_and_truncation() {
    let conflict = preview(V1_CONFLICT, Ps2CodebreakerVersion::V1ToV6);
    assert_refused(&conflict);
    assert_eq!(
        refusals(&conflict),
        [Ps2CodebreakerRefusal::ConflictingWrites {
            address: 0x0043_AFCC
        }]
    );

    let repeated = format!("{V1_DIRECT}{V1_DIRECT}");
    let identical = preview(&repeated, Ps2CodebreakerVersion::V1ToV6);
    assert!(
        matches!(
            identical.outcome,
            Ps2CodebreakerOutcome::Translatable { .. }
        ),
        "identical repeats do not conflict"
    );

    // A cut-off multi-line code is refused at its head record.
    let truncated = preview(V1_TRUNCATED_SLIDE, Ps2CodebreakerVersion::V1ToV6);
    assert_refused(&truncated);
    assert_eq!(
        refusals(&truncated),
        [Ps2CodebreakerRefusal::NotDirectWrite {
            family: Ps2CodebreakerFamily::Slide
        }]
    );
}

#[test]
fn original_lines_are_preserved_verbatim_with_provenance() {
    let input = "  2aff014c   2411ffff  \r\n\r\n0AF344DE 000000FF\n";
    let result = preview(input, Ps2CodebreakerVersion::V1ToV6);
    assert_eq!(result.records[0].original, "2aff014c   2411ffff");
    assert_eq!(result.records[0].line, 1);
    assert_eq!(result.records[0].encrypted, (0x2AFF_014C, 0x2411_FFFF));
    assert_eq!(result.records[1].line, 3);
    assert_eq!(
        result.provenance.decoder_version,
        PS2_CODEBREAKER_PREVIEW_VERSION
    );
    assert_eq!(result.provenance.cipher_crate, PS2_CODEBREAKER_CIPHER_CRATE);
    assert_eq!(result.provenance.input_sha256.len(), 64);
    assert_eq!(
        result.provenance.input_sha256,
        preview(input, Ps2CodebreakerVersion::V1ToV6)
            .provenance
            .input_sha256
    );
    let refused = preview(V1_POINTER, Ps2CodebreakerVersion::V1ToV6);
    assert_eq!(refused.records[1].original, "6AE060AC 896952E1");
    assert_eq!(refused.records[1].encrypted, (0x6AE0_60AC, 0x8969_52E1));
}

#[test]
fn translated_lines_round_trip_through_the_pnach_validators() {
    let result = preview(V1_DIRECT, Ps2CodebreakerVersion::V1ToV6);
    for line in translated_lines(&result) {
        let parsed = PnachPatchLine::parse(&line).expect("valid patch line");
        assert_eq!(parsed.as_str(), line);
    }
    assert_eq!(
        pnach_line_to_ir("patch=1,EE,0043AFD0,byte,FF"),
        CheatOperation::Write8 {
            address: 0x0043_AFD0,
            value: 0xFF
        }
    );
}

#[test]
fn malformed_and_oversized_input_is_rejected_before_decryption() {
    let version = Ps2CodebreakerVersion::V1ToV6;
    let id = verified();
    let run = |input: &str| preview_ps2_codebreaker(input, version, "x", &id, None, &[]);
    assert_eq!(run(""), Err(Ps2CodebreakerError::Empty));
    assert_eq!(run("\n  \n"), Err(Ps2CodebreakerError::Empty));
    assert_eq!(
        run("Infinite Health\n2AFF014C 2411FFFF"),
        Err(Ps2CodebreakerError::MalformedLine { line: 1 })
    );
    assert_eq!(
        run("2AFF014C 2411FFFF // note"),
        Err(Ps2CodebreakerError::MalformedLine { line: 1 })
    );
    assert_eq!(
        run("2AFF014C 2411FFF"),
        Err(Ps2CodebreakerError::MalformedLine { line: 1 })
    );
    assert_eq!(
        run("2AFF014C 2411FFFG"),
        Err(Ps2CodebreakerError::MalformedLine { line: 1 })
    );
    assert_eq!(run("2AFF014C é"), Err(Ps2CodebreakerError::NonAsciiInput));
    assert_eq!(
        run(&"2AFF014C 2411FFFF\n".repeat(MAX_CODEBREAKER_LINES + 1)),
        Err(Ps2CodebreakerError::TooManyLines)
    );
    assert_eq!(
        run(&" ".repeat(MAX_CODEBREAKER_INPUT_BYTES + 1)),
        Err(Ps2CodebreakerError::InputTooLarge)
    );
}

#[test]
fn names_must_be_short_and_printable() {
    let id = verified();
    for name in ["", "   ", "a\nb", &"x".repeat(MAX_CHEAT_NAME_CHARS + 1)] {
        assert_eq!(
            preview_ps2_codebreaker(
                V1_DIRECT,
                Ps2CodebreakerVersion::V1ToV6,
                name,
                &id,
                None,
                &[]
            ),
            Err(Ps2CodebreakerError::InvalidName)
        );
    }
}

#[test]
fn identity_must_be_verified_and_agree_with_any_declared_game() {
    let version = Ps2CodebreakerVersion::V1ToV6;
    let unresolved = identity(Some("SLUS-20312"), None);
    assert_eq!(
        preview_ps2_codebreaker(V1_DIRECT, version, "x", &unresolved, None, &[]),
        Err(Ps2CodebreakerError::IdentityUnresolved)
    );

    let id = verified();
    let wrong_crc = Ps2CodebreakerDeclaredGame {
        serial: None,
        crc: Some("DEADBEEF".to_string()),
    };
    assert!(matches!(
        preview_ps2_codebreaker(V1_DIRECT, version, "x", &id, Some(&wrong_crc), &[]),
        Err(Ps2CodebreakerError::IdentityConflict { .. })
    ));
    let wrong_serial = Ps2CodebreakerDeclaredGame {
        serial: Some("SLES-50001".to_string()),
        crc: None,
    };
    assert!(matches!(
        preview_ps2_codebreaker(V1_DIRECT, version, "x", &id, Some(&wrong_serial), &[]),
        Err(Ps2CodebreakerError::IdentityConflict { .. })
    ));
    let malformed = Ps2CodebreakerDeclaredGame {
        serial: Some("nope".to_string()),
        crc: None,
    };
    assert!(matches!(
        preview_ps2_codebreaker(V1_DIRECT, version, "x", &id, Some(&malformed), &[]),
        Err(Ps2CodebreakerError::IdentityConflict { .. })
    ));
    let matching = Ps2CodebreakerDeclaredGame {
        serial: Some("slus-20312".to_string()),
        crc: Some("A1B2C3D4".to_string()),
    };
    assert!(preview_ps2_codebreaker(V1_DIRECT, version, "x", &id, Some(&matching), &[]).is_ok());
    // A declared serial cannot be satisfied by an identity without one.
    let no_serial = identity(None, Some("A1B2C3D4"));
    assert!(matches!(
        preview_ps2_codebreaker(
            V1_DIRECT,
            version,
            "x",
            &no_serial,
            Some(&wrong_serial),
            &[]
        ),
        Err(Ps2CodebreakerError::IdentityConflict { .. })
    ));
}

#[test]
fn crc_only_identity_uses_the_bare_crc_filename() {
    let id = identity(None, Some("a1b2c3d4"));
    let result = preview_ps2_codebreaker(
        V1_DIRECT,
        Ps2CodebreakerVersion::V1ToV6,
        "x",
        &id,
        None,
        &[],
    )
    .expect("preview");
    let Ps2CodebreakerOutcome::Translatable {
        destination_file_name,
        ..
    } = result.outcome
    else {
        panic!("expected translatable");
    };
    assert_eq!(destination_file_name, "A1B2C3D4.pnach");
}

// Master-code (type 9 hook) vectors, encrypted by the cb2util v1.9 C code from
// the ps2rd-documented examples: hook 902D51F8 0C0B95F6 and constant writes
// 002BAA31 00000063 / 107657B2 0000FFFF / 20417A64 42C80000.
const HOOK_V1: &str =
    "9A802071 1859C47F\n0A523CDB 00000063\n1AEB6087 0000FFFF\n2A570387 42C80000\n";
const HOOK_V7_SETUP: &str = "B4336FA9 4DFEFB79\n6636DB86 4AEE2E3F\nC99FD924 C5542525\n382DD736 FB6A5001\n25D7F73D 6FF75EA0\n";
const HOOK_V7_COMMON: &str =
    "6636DB86 4AEE2E3F\nC99FD924 C5542525\n382DD736 FB6A5001\n25D7F73D 6FF75EA0\n";
const TWO_HOOKS_V1: &str = "9A802071 1859C47F\n9A882420 18526A38\n0A523CDB 00000063\n"; // 9031A000 0C0C0000 second
const C_TYPE_V1: &str = "CAA78917 9AA357F4\n0A523CDB 00000063\n"; // C0153880 03E00008
const F_TYPE_V1: &str = "FA7A006E 32D08898\n0A523CDB 00000063\n"; // F0100008 0000000E
const HOOK_CONDITIONAL_V1: &str = "9A802071 1859C47F\nDA7DF030 B35A74E0\n2AFCC51F 10C00033\n";
const HOOK_POINTER_V1: &str = "9A802071 1859C47F\n6AE44BE5 896D7DD6\n0A230B0D 00000156\n";
const HOOK_BIT25_V1: &str = "98802071 1A59C47F\n0A523CDB 00000063\n"; // 922D51F8 0C0B95F6
const HOOK_ONLY_V1: &str = "9A802071 1859C47F\n";
const WRITE_THEN_HOOK_V1: &str = "0A523CDB 00000063\n9A802071 1859C47F\n";

const HOOK_WRITES: [&str; 3] = [
    "patch=1,EE,002BAA31,byte,63",
    "patch=1,EE,007657B2,short,FFFF",
    "patch=1,EE,00417A64,word,42C80000",
];

fn preview_ack(
    input: &str,
    version: Ps2CodebreakerVersion,
    acknowledged: &[usize],
) -> Ps2CodebreakerPreview {
    preview_ps2_codebreaker(
        input,
        version,
        "Test cheat",
        &verified(),
        None,
        acknowledged,
    )
    .expect("preview")
}

#[test]
fn hooks_with_writes_need_an_exact_acknowledgement_in_every_version() {
    for (input, version, hook_line) in [
        (HOOK_V1, Ps2CodebreakerVersion::V1ToV6, 1),
        (HOOK_V7_SETUP, Ps2CodebreakerVersion::V7WithSetupLine, 2),
        (HOOK_V7_COMMON, Ps2CodebreakerVersion::V7CommonKey, 1),
    ] {
        // Without acknowledgement: no PNACH is produced at all.
        let pending = preview_ack(input, version, &[]);
        let Ps2CodebreakerOutcome::AcknowledgementRequired {
            excluded_hooks,
            rationale,
        } = &pending.outcome
        else {
            panic!("expected acknowledgement, got {:?}", pending.outcome);
        };
        assert_eq!(excluded_hooks.len(), 1);
        assert_eq!(excluded_hooks[0].line, hook_line);
        assert_eq!(excluded_hooks[0].decrypted, (0x902D_51F8, 0x0C0B_95F6));
        assert_eq!(rationale, &PS2_CODEBREAKER_HOOK_EXCLUSION_RATIONALE);
        assert!(refusals(&pending).is_empty());

        // The hook is kept verbatim as a record, never as a translated line.
        let record = &pending.records[hook_line - 1];
        assert_eq!(record.role, Ps2CodebreakerRole::ExcludedHook);
        assert_eq!(record.original, excluded_hooks[0].original);
        assert_eq!(translated_lines(&pending), HOOK_WRITES);

        // With the exact acknowledgement the remaining writes are previewed.
        let accepted = preview_ack(input, version, &[hook_line]);
        let Ps2CodebreakerOutcome::Translatable {
            cheat,
            pnach_preview,
            excluded_hooks,
            ..
        } = &accepted.outcome
        else {
            panic!("expected translatable, got {:?}", accepted.outcome);
        };
        assert_eq!(excluded_hooks.len(), 1);
        assert_eq!(
            cheat
                .patch_lines
                .iter()
                .map(PnachPatchLine::as_str)
                .collect::<Vec<_>>(),
            HOOK_WRITES
        );
        assert!(!pnach_preview.contains("902D51F8") && !pnach_preview.contains("0C0B95F6"));
        for line in HOOK_WRITES {
            assert!(pnach_preview.contains(line), "{pnach_preview}");
        }
    }
}

#[test]
fn an_acknowledgement_must_name_exactly_the_hook_lines() {
    let version = Ps2CodebreakerVersion::V1ToV6;
    for wrong in [&[][..], &[2], &[1, 2], &[99], &[0]] {
        assert!(
            matches!(
                preview_ack(HOOK_V1, version, wrong).outcome,
                Ps2CodebreakerOutcome::AcknowledgementRequired { .. }
            ),
            "{wrong:?}"
        );
    }
    // Duplicates and order do not matter.
    assert!(matches!(
        preview_ack(HOOK_V1, version, &[1, 1]).outcome,
        Ps2CodebreakerOutcome::Translatable { .. }
    ));

    // Two hooks (multi-ELF game): both must be acknowledged.
    let partial = preview_ack(TWO_HOOKS_V1, version, &[1]);
    let Ps2CodebreakerOutcome::AcknowledgementRequired { excluded_hooks, .. } = &partial.outcome
    else {
        panic!("{:?}", partial.outcome);
    };
    assert_eq!(
        excluded_hooks
            .iter()
            .map(|hook| hook.line)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(matches!(
        preview_ack(TWO_HOOKS_V1, version, &[2, 1]).outcome,
        Ps2CodebreakerOutcome::Translatable { .. }
    ));
}

#[test]
fn acknowledgement_never_overrides_a_refusal_and_nothing_else_is_excluded() {
    let version = Ps2CodebreakerVersion::V1ToV6;
    for (input, family) in [
        (C_TYPE_V1, Ps2CodebreakerFamily::Control),
        (F_TYPE_V1, Ps2CodebreakerFamily::Control),
        (HOOK_CONDITIONAL_V1, Ps2CodebreakerFamily::Conditional),
        (HOOK_POINTER_V1, Ps2CodebreakerFamily::Pointer),
        (HOOK_BIT25_V1, Ps2CodebreakerFamily::Control),
    ] {
        // Acknowledging every line still cannot make the list previewable.
        let result = preview_ack(input, version, &[1, 2, 3]);
        assert_refused(&result);
        assert_eq!(
            refusals(&result)[0],
            Ps2CodebreakerRefusal::NotDirectWrite { family },
            "{input}"
        );
        assert!(!result.records.iter().any(|record| matches!(
            record.role,
            Ps2CodebreakerRole::Translated { .. }
        ) && record.line == refusal_line(&result)),);
    }
    // The hook beside a conditional is still recognised, not swallowed.
    let result = preview_ack(HOOK_CONDITIONAL_V1, version, &[1]);
    assert_eq!(result.records[0].role, Ps2CodebreakerRole::ExcludedHook);
    assert_eq!(refusal_line(&result), 2);

    // A hook alone, or a list with no writes, has nothing to preview.
    assert_refused(&preview_ack(HOOK_ONLY_V1, version, &[1]));
}

fn refusal_line(preview: &Ps2CodebreakerPreview) -> usize {
    preview
        .records
        .iter()
        .find(|record| matches!(record.role, Ps2CodebreakerRole::Refused { .. }))
        .map(|record| record.line)
        .expect("a refused record")
}

#[test]
fn hook_position_does_not_change_the_writes_and_hooks_change_nothing_else() {
    let version = Ps2CodebreakerVersion::V1ToV6;
    let after = preview_ack(WRITE_THEN_HOOK_V1, version, &[2]);
    let without = preview("0A523CDB 00000063\n", Ps2CodebreakerVersion::V1ToV6);
    let (
        Ps2CodebreakerOutcome::Translatable { cheat: a, .. },
        Ps2CodebreakerOutcome::Translatable { cheat: b, .. },
    ) = (&after.outcome, &without.outcome)
    else {
        panic!("both previewable");
    };
    assert_eq!(a.patch_lines, b.patch_lines);

    // Every original line and encrypted pair of the hook list is preserved.
    let result = preview_ack(HOOK_V1, version, &[1]);
    assert_eq!(result.records[0].original, "9A802071 1859C47F");
    assert_eq!(result.records[0].encrypted, (0x9A80_2071, 0x1859_C47F));
}

#[test]
fn only_a_clean_single_line_type_nine_is_a_hook() {
    assert!(is_hook(0x902D_51F8));
    assert!(is_hook(0x91FF_FFFF));
    for address in [
        0x922D_51F8,
        0x940D_51F8,
        0x980D_51F8,
        0x8000_0000,
        0xA02D_51F8,
        0xF010_0008,
        0xC015_3880,
    ] {
        assert!(!is_hook(address), "{address:08X}");
    }
}

#[test]
fn acknowledged_line_numbers_count_blank_lines_and_ignore_crlf_in_every_mode() {
    for (input, version, hook_line) in [
        (HOOK_V1, Ps2CodebreakerVersion::V1ToV6, 3),
        (HOOK_V7_SETUP, Ps2CodebreakerVersion::V7WithSetupLine, 4),
        (HOOK_V7_COMMON, Ps2CodebreakerVersion::V7CommonKey, 3),
    ] {
        let padded = format!("\r\n\r\n{}", input.replace('\n', "\r\n"));
        let pending = preview_ack(&padded, version, &[]);
        let Ps2CodebreakerOutcome::AcknowledgementRequired { excluded_hooks, .. } =
            &pending.outcome
        else {
            panic!("{:?}", pending.outcome);
        };
        assert_eq!(excluded_hooks[0].line, hook_line);
        // The reported line number is the pasted line number.
        assert_eq!(
            padded.lines().nth(hook_line - 1).map(str::trim),
            Some(excluded_hooks[0].original.as_str())
        );
        assert!(matches!(
            preview_ack(&padded, version, &[hook_line]).outcome,
            Ps2CodebreakerOutcome::Translatable { .. }
        ));
    }
    // A lone carriage return is not a line separator here.
    assert_eq!(
        preview_ps2_codebreaker(
            "2AFF014C 2411FFFF\r0AF344DE 000000FF",
            Ps2CodebreakerVersion::V1ToV6,
            "x",
            &verified(),
            None,
            &[],
        ),
        Err(Ps2CodebreakerError::MalformedLine { line: 1 })
    );
}

#[test]
fn acknowledging_a_list_without_hooks_is_an_error() {
    assert_eq!(
        preview_ps2_codebreaker(
            V1_DIRECT,
            Ps2CodebreakerVersion::V1ToV6,
            "x",
            &verified(),
            None,
            &[1],
        ),
        Err(Ps2CodebreakerError::UnexpectedAcknowledgement)
    );
}

#[test]
fn the_managed_id_depends_on_the_version() {
    let id = |version| match preview(V7_COMMON_DIRECT, version).outcome {
        Ps2CodebreakerOutcome::Translatable { cheat, .. } => cheat.id,
        other => panic!("{other:?}"),
    };
    let first = id(Ps2CodebreakerVersion::V7CommonKey);
    assert!(first.starts_with("cb-") && first.len() == 19);
    assert_eq!(first, id(Ps2CodebreakerVersion::V7CommonKey));
    // The same text under a version that also translates gets another id.
    let other = match preview(V1_DIRECT, Ps2CodebreakerVersion::V1ToV6).outcome {
        Ps2CodebreakerOutcome::Translatable { cheat, .. } => cheat.id,
        other => panic!("{other:?}"),
    };
    assert_ne!(first, other);
    assert_ne!(
        managed_id(Ps2CodebreakerVersion::V1ToV6, "abc"),
        managed_id(Ps2CodebreakerVersion::V7CommonKey, "abc")
    );
}
