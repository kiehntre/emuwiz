use super::*;

// Independently published vectors and exact provenance are recorded in
// docs/research/GC_WII_AR_DASH_DECRYPTOR.md. Verifiers include the cipher's
// checksum nibble; historical GCNcrypt displays clear that nibble.
const PUBLIC_VECTORS: [(&str, &str, [u32; 2]); 4] = [
    (
        "G12C-TMX0-WRT5C\nG2ND-C1RJ-G4TZ1",
        "00690E90 000004FF",
        [0x21E22DC2, 0x08000000],
    ),
    (
        "XAUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD",
        "021F11DA 00000001",
        [0x704E01EF, 0x08000000],
    ),
    (
        "GKMU-93RZ-82YV6\nZFPE-UYKV-PX95X",
        "C435E298 0000FF01",
        [0x0776EB42, 0x98000000],
    ),
    (
        "NT40-E3MT-TTTN4\nT1MV-XZ0P-2YDR5\n99DR-JVGX-Z6DAF",
        "057E6CF8 4BEB46D0\n057E6CFC 000009C0",
        [0x5F7E1000, 0x88000000],
    ),
];

// These ciphertexts were generated independently with OpenSSL DES-ECB and
// checked against pinned Dolphin. They are not claimed as public vectors.
const GECKO_ENCRYPTED: &str = "MW93-4G33-3JE7T\n1AXC-RQB8-H78K3";
const FAMILIES_ENCRYPTED: &str = "4MJ1-JJM8-7JF18\n3041-UP6W-KQGJE\nEF78-K2N7-03X8F\nQTRA-6WT7-UZTB9\nPF3N-BE7M-AKBQE\nFQAA-E2GT-54ZRH\nN3GY-C825-4CWJH\nEXFB-B6X7-W4W7V";
const FAMILIES_RAW: &str = "002E4BB3 000000FF\n0224CD50 00003E7F\n063B8760 3F800000\n80234C58 00000001\nA00AE4D0 00000001\n202E4C84 00000000\n042E4C88 00000001";

#[test]
fn all_independent_public_vectors_decode_exactly() {
    for (encrypted, raw, verifier) in PUBLIC_VECTORS {
        let result = decode_gamecube_ar(encrypted).unwrap();
        assert_eq!(result.raw_ar_text, raw);
        assert_eq!(result.verification.verifier_words, verifier);
        assert_eq!(result.original_encrypted_text, encrypted);
        assert_eq!(result.decoder_version, GAMECUBE_AR_DECODER_VERSION);
    }
}

#[test]
fn standard_des_known_answer_and_reverse_direction() {
    let keys = des_subkeys(0x133457799BBCDFF1);
    assert_eq!(des_decrypt(0x85E813540F0AB405, &keys), 0x0123456789ABCDEF);
    let mut encryption_keys = keys;
    encryption_keys.reverse();
    assert_eq!(
        des_decrypt(0x0123456789ABCDEF, &encryption_keys),
        0x85E813540F0AB405
    );
}

// Test-only inverse, not another selectable production engine. Independently
// generated OpenSSL ciphertexts above prevent a circular round-trip oracle.
fn encrypt_words(words: &[u32]) -> String {
    assert!(words.len().is_multiple_of(2));
    let mut keys = des_subkeys(GAMECUBE_AR_KEY);
    keys.reverse();
    words
        .chunks_exact(2)
        .map(|pair| {
            let raw = (u64::from(pair[0]) << 32) | u64::from(pair[1]);
            let block = swap_word_bytes(des_decrypt(swap_word_bytes(raw), &keys));
            let packed = (u128::from(block) << 1) | u128::from(block.count_ones() & 1);
            let mut line = String::with_capacity(15);
            for index in 0..13 {
                if index == 4 || index == 8 {
                    line.push('-');
                }
                line.push(ALPHABET[((packed >> ((12 - index) * 5)) & 31) as usize] as char);
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn generated_set(body: &[u32], flags: u32) -> String {
    let mut words = vec![0x004F2345, flags];
    words.extend_from_slice(body);
    let crc = crc16_kermit(&words);
    words[0] |= u32::from((crc ^ (crc >> 4) ^ (crc >> 8) ^ (crc >> 12)) & 15) << 28;
    encrypt_words(&words)
}

#[test]
fn public_vectors_reencrypt_to_original_dash_text() {
    for (encrypted, raw, verifier) in PUBLIC_VECTORS {
        let mut words = verifier.to_vec();
        words.extend(
            raw.split_whitespace()
                .map(|word| u32::from_str_radix(word, 16).unwrap()),
        );
        assert_eq!(encrypt_words(&words), encrypted);
    }
}

#[test]
fn independently_generated_opcode_families_are_byte_exact() {
    assert_eq!(
        decode_gamecube_ar(GECKO_ENCRYPTED).unwrap().raw_ar_text,
        "04001000 AABBCCDD"
    );
    let decoded = decode_gamecube_ar(FAMILIES_ENCRYPTED).unwrap();
    assert_eq!(decoded.raw_ar_text, FAMILIES_RAW);
    assert_eq!(decoded.verification.encrypted_lines, 8);
    assert_eq!(decoded.verification.crc16, 12886);
    assert_eq!(decoded.verification.game_id, 0x27);
    assert_eq!(decoded.verification.code_id, 0x12345);
    assert!(!decoded.verification.is_master);
}

#[test]
fn ascii_case_outer_whitespace_blank_lines_and_crlf_preserve_original() {
    let input = " \t\r\n xauq-995v-emm2k \t\r\n\r\n hhc0-6eh5-tq6ud \r\n";
    let decoded = decode_gamecube_ar(input).unwrap();
    assert_eq!(decoded.original_encrypted_text, input);
    assert_eq!(decoded.raw_ar_text, PUBLIC_VECTORS[1].1);
    assert_eq!(decoded.verification.game_id, 0x27);
    assert_eq!(decoded.verification.region, 0);
    assert_eq!(decoded.verification.encrypted_lines, 2);
}

#[test]
fn wrong_dash_placement_and_length_refused() {
    for malformed in [
        "XAUQ995VEMM2K",
        "XAU-Q995V-EMM2K",
        "XAUQ-995-VEMM2K",
        "XAUQ-995V-EMM2",
        "XAUQ-995V-EMM2KK",
        "XAUQ-995V_EMM2K",
        "XAUQ-995V-EM M2K",
        "XAUQ-995V-EMM2K comment",
    ] {
        assert!(
            matches!(
                decode_gamecube_ar(&format!("{malformed}\nHHC0-6EH5-TQ6UD")),
                Err(GameCubeArDecodeError::MalformedLine { line: 1 })
            ),
            "{malformed}"
        );
    }
}

#[test]
fn noncanonical_alphabet_and_ambiguous_aliases_refused() {
    for character in ['I', 'L', 'O', 'S', '?', '_', '-', '\0'] {
        let input = format!("{character}AUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD");
        assert_eq!(
            decode_gamecube_ar(&input),
            Err(GameCubeArDecodeError::InvalidAlphabet { line: 1 })
        );
    }
}

#[test]
fn unicode_input_refused() {
    for input in [
        "ＸAUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD",
        "\u{200b}XAUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD",
    ] {
        assert_eq!(
            decode_gamecube_ar(input),
            Err(GameCubeArDecodeError::NonAsciiInput)
        );
    }
}

#[test]
fn parity_bit_corruption_refused_with_line_number() {
    assert_eq!(
        decode_gamecube_ar("XAUQ-995V-EMM2M\nHHC0-6EH5-TQ6UD"),
        Err(GameCubeArDecodeError::ParityMismatch { line: 1 })
    );
    assert_eq!(
        decode_gamecube_ar("\nXAUQ-995V-EMM2K\nHHC0-6EH5-TQ6UC"),
        Err(GameCubeArDecodeError::ParityMismatch { line: 3 })
    );
}

#[test]
fn parity_preserving_single_character_checksum_failure_is_not_recovered() {
    assert!(matches!(
        decode_gamecube_ar("0AUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD"),
        Err(GameCubeArDecodeError::ChecksumMismatch { .. })
    ));
}

#[test]
fn corrupted_check_nibble_refused_even_with_valid_line_parity() {
    let encrypted = encrypt_words(&[0x604E01EF, 0x08000000, 0x021F11DA, 1]);
    assert_eq!(
        decode_gamecube_ar(&encrypted),
        Err(GameCubeArDecodeError::ChecksumMismatch {
            expected: 6,
            actual: 7
        })
    );
}

#[test]
fn checksum_collision_still_fails_reserved_verifier_gate() {
    // This single-character mutation passes Dolphin's four-bit checksum.
    assert!(matches!(
        decode_gamecube_ar("YAUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD"),
        Err(GameCubeArDecodeError::UnsupportedVerifier { .. })
    ));
}

#[test]
fn valid_checksum_is_explicitly_not_payload_authentication() {
    // Independently confirmed with Dolphin and OpenSSL. This one-symbol change
    // is another valid checked body; no decoder can infer the intended text.
    let decoded = decode_gamecube_ar("XAUQ-995V-EMM2K\nRHC0-6EH5-TQ6UD").unwrap();
    assert_eq!(decoded.verification.verifier_words, PUBLIC_VECTORS[1].2);
    assert_eq!(decoded.raw_ar_text, "3758E9EE 149E646D");
    assert_eq!(
        decoded.original_encrypted_text,
        "XAUQ-995V-EMM2K\nRHC0-6EH5-TQ6UD"
    );
}

#[test]
fn expanded_unknown_padding_and_reserved_region_verifiers_refused() {
    for encrypted in [
        "G3GN-6CB5-9FUFE\nZZHF-4YM9-NE8YA",
        "0AVA-2GQR-97XPF\nZZHF-4YM9-NE8YA",
        "KZH4-8AFK-4JXHH\nZZHF-4YM9-NE8YA",
        "UPC8-TW5E-PAE5M\nZZHF-4YM9-NE8YA",
    ] {
        assert!(matches!(
            decode_gamecube_ar(encrypted),
            Err(GameCubeArDecodeError::UnsupportedVerifier { .. })
        ));
    }
}

#[test]
fn master_verifier_cannot_disappear_with_an_otherwise_supported_body() {
    let decoded = decode_gamecube_ar("Y1HA-TP30-Y5PRA\nZZHF-4YM9-NE8YA").unwrap();
    assert!(decoded.verification.is_master);
    assert_eq!(decoded.raw_ar_text, "04001000 00000001");
    assert!(
        decode_gamecube_ar(PUBLIC_VECTORS[2].0)
            .unwrap()
            .verification
            .is_master
    );
    assert!(
        decode_gamecube_ar(PUBLIC_VECTORS[3].0)
            .unwrap()
            .verification
            .is_master
    );
}

#[test]
fn only_a_complete_encrypted_body_is_accepted() {
    for input in ["", " \r\n", "XAUQ-995V-EMM2K"] {
        assert_eq!(
            decode_gamecube_ar(input),
            Err(GameCubeArDecodeError::MissingBody)
        );
    }
    assert!(matches!(
        decode_gamecube_ar("XAUQ-995V-EMM2K\n021F11DA 00000001"),
        Err(GameCubeArDecodeError::MalformedLine { line: 2 })
    ));
    assert!(matches!(
        decode_gamecube_ar("021F11DA 00000001\nHHC0-6EH5-TQ6UD"),
        Err(GameCubeArDecodeError::MalformedLine { line: 1 })
    ));
    assert!(matches!(
        decode_gamecube_ar("Silver knife\nXAUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD"),
        Err(GameCubeArDecodeError::MalformedLine { line: 1 })
    ));
}

#[test]
fn byte_and_line_limits_refuse_before_unbounded_work() {
    assert_eq!(
        decode_gamecube_ar(&" ".repeat(MAX_ENCRYPTED_BYTES + 1)),
        Err(GameCubeArDecodeError::InputTooLarge)
    );
    let too_many = "XAUQ-995V-EMM2K\n".repeat(MAX_ENCRYPTED_LINES + 1);
    assert_eq!(
        decode_gamecube_ar(&too_many),
        Err(GameCubeArDecodeError::TooManyLines)
    );
}

#[test]
fn maximum_permitted_set_decodes_exactly() {
    let body = [0x04001000, 1].repeat(MAX_ENCRYPTED_LINES - 1);
    let input = generated_set(&body, 0x08000000);
    let decoded = decode_gamecube_ar(&input).unwrap();
    assert_eq!(decoded.verification.encrypted_lines, MAX_ENCRYPTED_LINES);
    assert_eq!(decoded.raw_ar_text.lines().count(), 255);
    assert_eq!(decoded.raw_ar_text.split_whitespace().count(), 510);
}

#[test]
fn trailing_payload_changes_invalidate_whole_set_checksum() {
    let encrypted = format!("{}\nHHC0-6EH5-TQ6UD", PUBLIC_VECTORS[0].0);
    assert!(matches!(
        decode_gamecube_ar(&encrypted),
        Err(GameCubeArDecodeError::ChecksumMismatch { .. })
    ));
}

#[test]
fn errors_explain_browse_only_refusal() {
    assert!(
        GameCubeArDecodeError::MalformedLine { line: 2 }
            .to_string()
            .contains("mixed raw/encrypted")
    );
    assert!(
        GameCubeArDecodeError::ParityMismatch { line: 2 }
            .to_string()
            .contains("line 2")
    );
    assert!(
        GameCubeArDecodeError::ChecksumMismatch {
            expected: 1,
            actual: 2
        }
        .to_string()
        .contains("checksum failed")
    );
}
