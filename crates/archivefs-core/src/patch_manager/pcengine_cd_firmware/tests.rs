//! No network anywhere in this suite, and **no PC Engine CD BIOS bytes are
//! embedded**. The `KNOWN_SYSTEM_CARDS` table holds only size/CRC32/SHA-1
//! values, each verified against two public sources (MAME `hash/pce.xml` /
//! `hash/tg16.xml` and the Beetle PCE / Mednafen firmware list). Because a
//! file's bytes cannot be manufactured to hash to a given SHA-1, the
//! "known hash is recognised" tests exercise `match_known_system_card`
//! against a `ComputedFirmwareDigests` built from a table row's own fields;
//! the file-level tests use invented bytes and assert only the safety and
//! non-match behaviour.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::dat::firmware_evidence::{ComputedFirmwareDigests, hash_firmware_file};
use crate::launch::readiness::{FirmwareReadiness, pcengine_cd_firmware_readiness};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn fixture_root(label: &str) -> PathBuf {
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "archivefs-pce-firmware-{label}-{}-{sequence}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn digests_of(card: &KnownSystemCard) -> ComputedFirmwareDigests {
    ComputedFirmwareDigests {
        size_bytes: card.size_bytes,
        crc32: card.crc32.to_string(),
        md5: String::new(),
        sha1: card.sha1.to_string(),
    }
}

fn card_of_class(class: SystemCardClass) -> KnownSystemCard {
    *KNOWN_SYSTEM_CARDS
        .iter()
        .find(|card| card.class == class)
        .expect("class present in the table")
}

fn verified(class: SystemCardClass) -> (PathBuf, PceCdFirmwareOutcome) {
    let card = card_of_class(class);
    (
        PathBuf::from("/does/not/matter"),
        PceCdFirmwareOutcome::Verified(VerifiedSystemCard {
            path: PathBuf::from("/does/not/matter"),
            size_bytes: card.size_bytes,
            crc32: card.crc32.to_string(),
            sha1: card.sha1.to_string(),
            card,
        }),
    )
}

// --- embedded table integrity ----------------------------------------

#[test]
fn every_known_system_card_record_is_structurally_well_formed() {
    let mut ids = std::collections::BTreeSet::new();
    for card in KNOWN_SYSTEM_CARDS {
        assert!(
            ids.insert(card.canonical_id),
            "duplicate id {}",
            card.canonical_id
        );
        assert_eq!(card.sha1.len(), 40, "{}", card.canonical_id);
        assert!(
            card.sha1
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(card.crc32.len(), 8, "{}", card.canonical_id);
        assert!(
            card.crc32
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert!(
            card.size_bytes == 262_144 || card.size_bytes == 32_768,
            "{} has an implausible size {}",
            card.canonical_id,
            card.size_bytes
        );
    }
    // The six dual-sourced records, including one JP and one US Super
    // System Card v3.0 (the class an Arcade Card game also uses).
    assert_eq!(KNOWN_SYSTEM_CARDS.len(), 6);
    assert_eq!(
        KNOWN_SYSTEM_CARDS
            .iter()
            .filter(|c| c.class == SystemCardClass::SuperSystemCardV3)
            .count(),
        2
    );
}

// --- known-hash recognition (positive) ------------------------------

#[test]
fn each_known_system_card_hash_is_recognized_by_content_only() {
    for card in KNOWN_SYSTEM_CARDS {
        let matched = match_known_system_card(&digests_of(card))
            .unwrap_or_else(|| panic!("{} should match itself", card.canonical_id));
        assert_eq!(matched.canonical_id, card.canonical_id);
    }
}

#[test]
fn super_system_card_v3_and_arcade_card_class_resolve_to_the_same_bios() {
    // The Arcade Card Pro/Duo add RAM, not a BIOS - an Arcade Card game
    // uses the Super System Card v3.0. So there is exactly one v3.0 class,
    // recognised for both JP and US dumps.
    for card in KNOWN_SYSTEM_CARDS
        .iter()
        .filter(|c| c.class == SystemCardClass::SuperSystemCardV3)
    {
        assert_eq!(
            match_known_system_card(&digests_of(card)).unwrap().class,
            SystemCardClass::SuperSystemCardV3
        );
    }
}

#[test]
fn filename_is_never_part_of_recognition() {
    // A digest built from a real table row matches regardless of any path;
    // `match_known_system_card` takes no filename at all.
    let jp_v3 = card_of_class(SystemCardClass::SuperSystemCardV3);
    assert_eq!(
        match_known_system_card(&digests_of(&jp_v3))
            .unwrap()
            .canonical_id,
        jp_v3.canonical_id
    );
}

// --- wrong / partial hash NOT recognized (negative) ----------------

#[test]
fn a_correct_size_and_crc32_with_a_wrong_sha1_is_not_recognized() {
    let card = card_of_class(SystemCardClass::SuperSystemCardV3);
    let tampered = ComputedFirmwareDigests {
        size_bytes: card.size_bytes,
        crc32: card.crc32.to_string(),
        md5: String::new(),
        sha1: "0000000000000000000000000000000000000000".to_string(),
    };
    assert!(match_known_system_card(&tampered).is_none());
}

#[test]
fn a_correct_sha1_with_a_wrong_size_or_crc32_is_not_recognized() {
    let card = card_of_class(SystemCardClass::CdRom2V2);
    let wrong_size = ComputedFirmwareDigests {
        size_bytes: card.size_bytes + 1,
        crc32: card.crc32.to_string(),
        md5: String::new(),
        sha1: card.sha1.to_string(),
    };
    assert!(match_known_system_card(&wrong_size).is_none());
    let wrong_crc = ComputedFirmwareDigests {
        size_bytes: card.size_bytes,
        crc32: "deadbeef".to_string(),
        md5: String::new(),
        sha1: card.sha1.to_string(),
    };
    assert!(match_known_system_card(&wrong_crc).is_none());
}

// --- file classification: safety + non-match ----------------------

#[test]
fn a_random_rom_is_classified_unknown_never_verified() {
    let root = fixture_root("random");
    let bytes: Vec<u8> = (0..262_144u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    let path = root.join("mystery.pce");
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        classify_system_card_file(&path),
        PceCdFirmwareOutcome::Unknown { .. }
    ));
}

#[test]
fn a_file_named_syscard3_pce_with_wrong_bytes_is_not_verified() {
    // Filename-only "verification" is impossible: content is the only proof.
    let root = fixture_root("filename-lie");
    let path = root.join("syscard3.pce");
    fs::write(&path, vec![0xE5_u8; 262_144]).unwrap();
    assert!(matches!(
        classify_system_card_file(&path),
        PceCdFirmwareOutcome::Unknown { .. }
    ));
}

#[test]
fn a_truncated_candidate_is_not_verified() {
    let root = fixture_root("truncated");
    let path = root.join("syscard3.pce");
    fs::write(&path, vec![0x00_u8; 1024]).unwrap();
    assert!(matches!(
        classify_system_card_file(&path),
        PceCdFirmwareOutcome::Unknown { .. }
    ));
}

#[test]
fn an_unrelated_hucard_game_is_not_verified() {
    let root = fixture_root("hucard");
    // A 512 KiB HuCard-sized blob - not a System Card at all.
    let path = root.join("Bonk's Adventure (USA).pce");
    fs::write(&path, vec![0x42_u8; 512 * 1024]).unwrap();
    assert!(matches!(
        classify_system_card_file(&path),
        PceCdFirmwareOutcome::Unknown { .. }
    ));
}

#[test]
fn a_missing_path_is_missing_and_a_symlink_is_refused_before_reading() {
    let root = fixture_root("safety");
    assert_eq!(
        classify_system_card_file(&root.join("nope.pce")),
        PceCdFirmwareOutcome::Missing
    );

    let real = root.join("real.pce");
    fs::write(&real, vec![0u8; 4096]).unwrap();
    let link = root.join("link.pce");
    symlink(&real, &link).unwrap();
    assert!(matches!(
        classify_system_card_file(&link),
        PceCdFirmwareOutcome::Unsafe { .. }
    ));

    assert!(matches!(
        classify_system_card_file(&root),
        PceCdFirmwareOutcome::Unsafe { .. }
    ));
}

#[test]
fn an_oversized_candidate_is_refused_without_a_full_read() {
    let root = fixture_root("oversized");
    let path = root.join("huge.pce");
    fs::write(&path, vec![0u8; (MAX_SYSTEM_CARD_BYTES + 1) as usize]).unwrap();
    assert!(matches!(
        classify_system_card_file(&path),
        PceCdFirmwareOutcome::Unreadable { .. }
    ));
}

#[test]
fn inventory_only_looks_at_plausible_extensions_and_is_deterministic() {
    let root = fixture_root("inventory");
    fs::write(root.join("syscard3.pce"), vec![0u8; 4096]).unwrap();
    fs::write(root.join("notes.txt"), b"hello").unwrap();
    fs::write(root.join("a.bin"), vec![0u8; 4096]).unwrap();
    let first = inventory_system_cards(&root);
    let second = inventory_system_cards(&root);
    assert_eq!(first, second);
    assert_eq!(first.len(), 2, "only .pce and .bin, not the .txt");
    assert!(
        first
            .iter()
            .all(|(_, outcome)| matches!(outcome, PceCdFirmwareOutcome::Unknown { .. }))
    );
}

// --- readiness resolution ----------------------------------------

#[test]
fn an_embedding_emulator_needs_no_external_firmware() {
    let status = resolve_pcengine_cd_firmware(
        &[],
        PceCdEmulatorFirmwarePolicy::EmbedsSystemCard,
        PceCdTitleRequirement::Unknown,
    );
    assert_eq!(status, PceCdFirmwareReadiness::EmulatorProvidesFirmware);
    assert_eq!(
        pcengine_cd_firmware_readiness(status),
        FirmwareReadiness::NotRequired
    );
}

#[test]
fn a_verified_super_system_card_is_sufficient_even_when_the_title_requirement_is_unknown() {
    let inventory = [verified(SystemCardClass::SuperSystemCardV3)];
    let status = resolve_pcengine_cd_firmware(
        &inventory,
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
        PceCdTitleRequirement::Unknown,
    );
    assert_eq!(
        status,
        PceCdFirmwareReadiness::VerifiedSufficient {
            class: SystemCardClass::SuperSystemCardV3
        }
    );
    assert_eq!(
        pcengine_cd_firmware_readiness(status),
        FirmwareReadiness::Verified
    );
}

#[test]
fn a_verified_v2_card_with_an_unknown_title_requirement_does_not_become_ready() {
    let inventory = [verified(SystemCardClass::CdRom2V2)];
    let status = resolve_pcengine_cd_firmware(
        &inventory,
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
        PceCdTitleRequirement::Unknown,
    );
    assert_eq!(
        status,
        PceCdFirmwareReadiness::VerifiedButRequirementUnknown {
            class: SystemCardClass::CdRom2V2
        }
    );
    // Unknown, never Verified - a guessed pass is worse than honest doubt.
    assert_eq!(
        pcengine_cd_firmware_readiness(status),
        FirmwareReadiness::Unknown
    );
}

#[test]
fn a_verified_v2_card_is_sufficient_only_when_the_title_is_known_plain_cdrom2() {
    let inventory = [verified(SystemCardClass::CdRom2V2)];
    let status = resolve_pcengine_cd_firmware(
        &inventory,
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
        PceCdTitleRequirement::AnyCdRom2,
    );
    assert_eq!(
        status,
        PceCdFirmwareReadiness::VerifiedSufficient {
            class: SystemCardClass::CdRom2V2
        }
    );
    // ...but a v2 card can never satisfy a title that needs Super CD-ROM².
    let strict = resolve_pcengine_cd_firmware(
        &inventory,
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
        PceCdTitleRequirement::SuperSystemCard,
    );
    assert_eq!(
        strict,
        PceCdFirmwareReadiness::VerifiedButRequirementUnknown {
            class: SystemCardClass::CdRom2V2
        }
    );
}

#[test]
fn no_verified_firmware_blocks_only_when_the_emulator_needs_an_external_card() {
    let missing = resolve_pcengine_cd_firmware(
        &[],
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
        PceCdTitleRequirement::Unknown,
    );
    assert_eq!(missing, PceCdFirmwareReadiness::NoVerifiedFirmware);
    assert_eq!(
        pcengine_cd_firmware_readiness(missing),
        FirmwareReadiness::Missing
    );

    // Unknown emulator policy + nothing verified -> not called "missing".
    let unknown = resolve_pcengine_cd_firmware(
        &[],
        PceCdEmulatorFirmwarePolicy::Unknown,
        PceCdTitleRequirement::Unknown,
    );
    assert_eq!(unknown, PceCdFirmwareReadiness::EmulatorRequirementUnknown);
    assert_eq!(
        pcengine_cd_firmware_readiness(unknown),
        FirmwareReadiness::Unknown
    );
}

#[test]
fn an_unverified_candidate_present_is_present_unverified_not_missing() {
    let root = fixture_root("candidate-present");
    fs::write(root.join("syscard3.pce"), vec![0xAA_u8; 262_144]).unwrap();
    let inventory = inventory_system_cards(&root);
    let status = resolve_pcengine_cd_firmware(
        &inventory,
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
        PceCdTitleRequirement::Unknown,
    );
    assert_eq!(status, PceCdFirmwareReadiness::CandidatePresentHashUnknown);
    assert_eq!(
        pcengine_cd_firmware_readiness(status),
        FirmwareReadiness::PresentUnverified
    );
}

#[test]
fn best_verified_card_class_prefers_the_super_system_card() {
    let inventory = [
        verified(SystemCardClass::CdRom2V1),
        verified(SystemCardClass::SuperSystemCardV3),
        verified(SystemCardClass::CdRom2V2),
    ];
    assert_eq!(
        best_verified_card_class(&inventory),
        Some(SystemCardClass::SuperSystemCardV3)
    );
}

#[test]
fn summary_strings_never_tell_the_user_to_rename_a_file() {
    for status in [
        PceCdFirmwareReadiness::EmulatorProvidesFirmware,
        PceCdFirmwareReadiness::VerifiedSufficient {
            class: SystemCardClass::SuperSystemCardV3,
        },
        PceCdFirmwareReadiness::VerifiedButRequirementUnknown {
            class: SystemCardClass::CdRom2V1,
        },
        PceCdFirmwareReadiness::CandidatePresentHashUnknown,
        PceCdFirmwareReadiness::NoVerifiedFirmware,
        PceCdFirmwareReadiness::EmulatorRequirementUnknown,
    ] {
        let summary = status.summary().to_ascii_lowercase();
        assert!(!summary.contains("rename"));
        assert!(!summary.contains(".pce"));
        assert!(!summary.is_empty());
    }
}

// --- cross-module independence -----------------------------------

#[test]
fn a_computed_digest_from_hash_firmware_file_of_invented_bytes_never_matches_a_known_card() {
    let root = fixture_root("independence");
    let path = root.join("blob.pce");
    fs::write(&path, vec![0x7F_u8; 262_144]).unwrap();
    let digests = hash_firmware_file(&path, MAX_SYSTEM_CARD_BYTES, 64 * 1024).unwrap();
    assert!(match_known_system_card(&digests).is_none());
}

#[test]
fn missing_firmware_does_not_change_pc_engine_cd_media_identity() {
    use crate::game_identity::{IdentityPlatform, inspect_catalogued_game_identity};

    // A minimal raw PC Engine CD data-track image: the IPL boot-record in
    // sector 1 with the signature at offset 32 and a sane boot pointer.
    let mut image = vec![0_u8; 8 * 2048];
    let ipl = 2048;
    image[ipl + 3] = 1; // one boot sector
    image[ipl] = 0;
    image[ipl + 1] = 0;
    image[ipl + 2] = 2; // start sector 2
    image[ipl + 32..ipl + 32 + b"PC Engine CD-ROM SYSTEM".len()]
        .copy_from_slice(b"PC Engine CD-ROM SYSTEM");

    let root = fixture_root("media-vs-firmware");
    let path = root.join("Game (Japan).iso");
    fs::write(&path, &image).unwrap();

    let report = inspect_catalogued_game_identity(&path, Some("PC Engine CD"));
    assert_eq!(report.platform, IdentityPlatform::PcEngineCd);
    assert_eq!(
        report.verified_pcengine_cd_boot_structure(),
        Some("PC Engine CD-ROM SYSTEM")
    );

    // With no firmware anywhere, readiness is a blocker but identity is
    // completely unaffected.
    let firmware = resolve_pcengine_cd_firmware(
        &inventory_system_cards(&root),
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
        PceCdTitleRequirement::Unknown,
    );
    assert_eq!(firmware, PceCdFirmwareReadiness::NoVerifiedFirmware);
    assert_eq!(
        report.verified_pcengine_cd_boot_structure(),
        Some("PC Engine CD-ROM SYSTEM"),
        "firmware resolution must not touch media identity"
    );
}

#[test]
fn no_known_system_card_is_a_pc_fx_or_other_system_hash() {
    // The table is System-Card-only: no PC-FX BIOS, no PS1/PS2/Xbox BIOS.
    for card in KNOWN_SYSTEM_CARDS {
        assert!(card.canonical_id.starts_with("pce-") || card.canonical_id.starts_with("tg16cd-"));
        assert!(!card.label.to_ascii_lowercase().contains("pc-fx"));
    }
    // `FirmwareSystem` (PS1/PS2/Xbox DAT-derived evidence) is untouched by
    // this module - it does not even reference it.
    use crate::dat::firmware_evidence::FirmwareSystem;
    assert_eq!(
        FirmwareSystem::PlayStation2.redump_dataset_label(),
        "Sony - PlayStation 2 - BIOS Images"
    );
}
