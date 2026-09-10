use std::fs;
use std::path::PathBuf;

use archivefs_core::content_detector::{ContentDetectionOutcome, ContentDetector};
use archivefs_core::content_evidence::{
    ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind,
};
use archivefs_core::game_identity::{
    IdentityPlatform, IdentityStatus, inspect_catalogued_game_identity, supported_loose_rom_format,
};
use archivefs_core::platform::PLATFORMS;
use archivefs_core::platform_evidence_fusion::{FusionOutcome, fuse_platform_evidence};
use archivefs_core::pokemon_mini_header_evidence::{
    POKEMON_MINI_HEADER_BYTES, POKEMON_MINI_HEADER_OFFSET, PokemonMiniHeaderDetector,
};

fn fixture_bytes() -> Vec<u8> {
    let mut bytes = vec![0u8; POKEMON_MINI_HEADER_OFFSET + POKEMON_MINI_HEADER_BYTES];
    bytes[POKEMON_MINI_HEADER_OFFSET..POKEMON_MINI_HEADER_OFFSET + 2].copy_from_slice(b"PM");
    bytes[POKEMON_MINI_HEADER_OFFSET + 0xA4..POKEMON_MINI_HEADER_OFFSET + 0xAC]
        .copy_from_slice(b"NINTENDO");
    bytes[POKEMON_MINI_HEADER_OFFSET + 0xAC..POKEMON_MINI_HEADER_OFFSET + 0xB0]
        .copy_from_slice(b"ABCD");
    bytes[POKEMON_MINI_HEADER_OFFSET + 0xB0..POKEMON_MINI_HEADER_OFFSET + 0xB4]
        .copy_from_slice(b"TEST");
    bytes
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "archivefs-pokemon-mini-{name}-{}",
        std::process::id()
    ))
}

#[test]
fn detector_requires_structure_and_does_not_promote_extension() {
    let detector = PokemonMiniHeaderDetector;
    assert!(matches!(
        detector.detect(b"not a cartridge"),
        ContentDetectionOutcome::NotRecognized
    ));
    assert!(matches!(
        detector.detect(&fixture_bytes()),
        ContentDetectionOutcome::Recognized { .. }
    ));
}

#[test]
fn media_registration_is_distinct_and_bin_is_not_claimed() {
    assert_eq!(
        supported_loose_rom_format(
            std::path::Path::new("game.min"),
            IdentityPlatform::PokemonMini
        ),
        Some("min")
    );
    assert_eq!(
        supported_loose_rom_format(
            std::path::Path::new("game.bin"),
            IdentityPlatform::PokemonMini
        ),
        None
    );
    let platform = PLATFORMS
        .iter()
        .find(|item| item.id == "Pokemon Mini")
        .unwrap();
    assert!(platform.weak_extensions.contains(&"min"));
    assert!(!platform.strong_extensions.contains(&"min"));
    assert!(!platform.folder_aliases.contains(&"gameboy"));
    assert!(!platform.folder_aliases.contains(&"ngp"));
}

#[test]
fn dat_platform_evidence_can_strengthen_structural_observation() {
    let report = fuse_platform_evidence([ContentEvidence::new(
        ContentEvidenceKind::BootStructure,
        "Pokemon Mini cartridge header",
        ContentEvidenceConfidence::Strong,
        "synthetic bounded header",
    )]);
    assert_eq!(report.outcome, FusionOutcome::Resolved);
    assert_eq!(report.resolved_platform, Some("Pokemon Mini"));
}

#[test]
fn conflicting_strong_platform_evidence_fails_closed() {
    let report = fuse_platform_evidence([
        ContentEvidence::new(
            ContentEvidenceKind::BootStructure,
            "Pokemon Mini cartridge header",
            ContentEvidenceConfidence::Strong,
            "synthetic Pokémon Mini structure",
        ),
        ContentEvidence::new(
            ContentEvidenceKind::BootStructure,
            "SEGA SEGASATURN",
            ContentEvidenceConfidence::Strong,
            "synthetic conflicting structure",
        ),
    ]);
    assert_eq!(report.outcome, FusionOutcome::Conflict);
    assert!(report.resolved_platform.is_none());
}

#[test]
fn catalogued_min_file_emits_verified_platform_and_hash_without_release_guess() {
    let path = temp_path("identity.min");
    fs::write(&path, fixture_bytes()).unwrap();
    let report = inspect_catalogued_game_identity(&path, Some("Pokemon Mini"));
    assert_eq!(report.platform, IdentityPlatform::PokemonMini);
    assert!(report.evidence.iter().any(|e| e.kind
        == archivefs_core::game_identity::IdentityKind::Platform
        && e.status == IdentityStatus::Verified));
    assert!(
        report
            .evidence
            .iter()
            .any(|e| e.kind == archivefs_core::game_identity::IdentityKind::LooseRomSha256)
    );
    fs::remove_file(path).ok();
}
