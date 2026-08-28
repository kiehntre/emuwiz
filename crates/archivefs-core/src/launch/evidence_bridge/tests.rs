//! Tests for the identity/content evidence bridge.
//!
//! Every fixture builds real [`GameIdentityReport`]/[`ArchiveRecord`] values
//! by hand (all fields are public) rather than running any real detector -
//! this module's own conversion logic is what is under test, not the
//! detectors that normally produce these structures.

use std::path::PathBuf;

use super::*;
use crate::game_identity::{IdentityImageFormat, IdentityProvenance};
use crate::{Archive, ArchiveHealth, ArchiveIdentity, ArchiveMetadata, MountPlan};

// ---------------------------------------------------------------------------
// Identity fixtures
// ---------------------------------------------------------------------------

fn provenance(archive_path: &str) -> IdentityProvenance {
    IdentityProvenance {
        archive_path: PathBuf::from(archive_path),
        member_path: None,
        member_index: None,
        method: "test fixture".to_string(),
    }
}

fn evidence(
    kind: IdentityKind,
    status: IdentityStatus,
    value: Option<&str>,
    confidence: IdentityConfidence,
) -> IdentityEvidence {
    IdentityEvidence {
        kind,
        status,
        value: value.map(str::to_string),
        confidence,
        provenance: provenance("/library/game.iso"),
        diagnostic: "test fixture evidence".to_string(),
    }
}

fn report(platform: IdentityPlatform, evidence: Vec<IdentityEvidence>) -> GameIdentityReport {
    GameIdentityReport {
        archive_path: PathBuf::from("/library/game.iso"),
        platform,
        format: IdentityImageFormat::Iso,
        evidence,
        warnings: Vec::new(),
        bytes_read: 4096,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    }
}

// --- verified resolved identity -> Resolved + correct fact(s) --------------

#[test]
fn verified_ps2_serial_resolves_with_a_matching_fact() {
    let source = report(
        IdentityPlatform::PlayStation2,
        vec![evidence(
            IdentityKind::Ps2Serial,
            IdentityStatus::Verified,
            Some("SLUS-98765"),
            IdentityConfidence::ExactBytes,
        )],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "PS2".to_string(),
            game_key: "SLUS-98765".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::Ps2Serial("SLUS-98765".to_string())]
    );
}

#[test]
fn verified_psp_disc_id_resolves_to_ppsspp_fact() {
    let source = report(
        IdentityPlatform::Psp,
        vec![evidence(
            IdentityKind::PspDiscId,
            IdentityStatus::Verified,
            Some("ULUS10000"),
            IdentityConfidence::StructuredMetadata,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "PSP".to_string(),
            game_key: "ULUS10000".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::PspDiscId("ULUS10000".to_string())]
    );
}

#[test]
fn verified_ps3_title_id_resolves_to_rpcs3_fact() {
    let source = report(
        IdentityPlatform::PlayStation3,
        vec![evidence(
            IdentityKind::Ps3TitleId,
            IdentityStatus::Verified,
            Some("BLUS30000"),
            IdentityConfidence::StructuredMetadata,
        )],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "PS3".to_string(),
            game_key: "BLUS30000".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::Ps3TitleId("BLUS30000".to_string())]
    );
}

#[test]
fn verified_ps1_serial_resolves_to_duckstation_fact() {
    let source = report(
        IdentityPlatform::PlayStation,
        vec![evidence(
            IdentityKind::Ps1Serial,
            IdentityStatus::Verified,
            Some("SLUS-12345"),
            IdentityConfidence::StructuredMetadata,
        )],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "PSX".to_string(),
            game_key: "SLUS-12345".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::Ps1Serial("SLUS-12345".to_string())]
    );
}

#[test]
fn verified_saturn_product_number_resolves_to_saturn_fact() {
    let source = report(
        IdentityPlatform::Saturn,
        vec![evidence(
            IdentityKind::SaturnProductNumber,
            IdentityStatus::Verified,
            Some("T-7101G"),
            IdentityConfidence::ExactBytes,
        )],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Saturn".to_string(),
            game_key: "T-7101G".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::SaturnProductCode(
            "T-7101G".to_string()
        )]
    );
}

#[test]
fn verified_dreamcast_product_code_resolves_to_flycast_fact() {
    let source = report(
        IdentityPlatform::Dreamcast,
        vec![evidence(
            IdentityKind::DreamcastProductCode,
            IdentityStatus::Verified,
            Some("T-8109N"),
            IdentityConfidence::ExactBytes,
        )],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Dreamcast".to_string(),
            game_key: "T-8109N".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::DreamcastProductCode(
            "T-8109N".to_string()
        )]
    );
}

#[test]
fn verified_sega_cd_product_code_resolves_to_retroarch_fact() {
    let source = report(
        IdentityPlatform::SegaCd,
        vec![evidence(
            IdentityKind::SegaCdProductCode,
            IdentityStatus::Verified,
            Some("GM T-12345-00"),
            IdentityConfidence::ExactBytes,
        )],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Sega CD".to_string(),
            game_key: "GM T-12345-00".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::SegaCdProductCode(
            "GM T-12345-00".to_string()
        )]
    );
}

#[test]
fn conflicting_sega_cd_product_codes_never_produce_a_retroarch_fact() {
    let source = report(
        IdentityPlatform::SegaCd,
        vec![
            evidence(
                IdentityKind::SegaCdProductCode,
                IdentityStatus::Verified,
                Some("GM T-12345-00"),
                IdentityConfidence::ExactBytes,
            ),
            evidence(
                IdentityKind::SegaCdProductCode,
                IdentityStatus::Verified,
                Some("GM T-99999-00"),
                IdentityConfidence::ExactBytes,
            ),
        ],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(status, CanonicalIdentityStatus::Conflicting);
    assert!(facts.is_empty());
}

#[test]
fn conflicting_dreamcast_product_codes_never_produce_a_flycast_fact() {
    let source = report(
        IdentityPlatform::Dreamcast,
        vec![
            evidence(
                IdentityKind::DreamcastProductCode,
                IdentityStatus::Verified,
                Some("T-8109N"),
                IdentityConfidence::ExactBytes,
            ),
            evidence(
                IdentityKind::DreamcastProductCode,
                IdentityStatus::Verified,
                Some("T-9999N"),
                IdentityConfidence::ExactBytes,
            ),
        ],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(status, CanonicalIdentityStatus::Conflicting);
    assert!(facts.is_empty());
}

#[test]
fn conflicting_ps1_serials_never_produce_a_duckstation_fact() {
    let source = report(
        IdentityPlatform::PlayStation,
        vec![
            evidence(
                IdentityKind::Ps1Serial,
                IdentityStatus::Verified,
                Some("SLUS-12345"),
                IdentityConfidence::StructuredMetadata,
            ),
            evidence(
                IdentityKind::Ps1Serial,
                IdentityStatus::Verified,
                Some("SLES-23456"),
                IdentityConfidence::StructuredMetadata,
            ),
        ],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(status, CanonicalIdentityStatus::Conflicting);
    assert!(facts.is_empty());
}

#[test]
fn verified_ps2_serial_and_crc_resolve_with_both_facts() {
    let source = report(
        IdentityPlatform::PlayStation2,
        vec![
            evidence(
                IdentityKind::Ps2Serial,
                IdentityStatus::Verified,
                Some("SLUS-98765"),
                IdentityConfidence::ExactBytes,
            ),
            evidence(
                IdentityKind::Pcsx2ExecutableCrc,
                IdentityStatus::Verified,
                Some("ABCDEF01"),
                IdentityConfidence::ExactBytes,
            ),
        ],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "PS2".to_string(),
            game_key: "SLUS-98765".to_string(),
        })
    );
    assert_eq!(facts.len(), 2);
    assert!(facts.contains(&VerifiedIdentityFact::Ps2Serial("SLUS-98765".to_string())));
    assert!(facts.contains(&VerifiedIdentityFact::Ps2ExecutableCrc(
        "ABCDEF01".to_string()
    )));
}

#[test]
fn verified_dolphin_game_id_resolves_to_gamecube_or_wii_by_platform() {
    let gamecube = report(
        IdentityPlatform::GameCube,
        vec![evidence(
            IdentityKind::DolphinGameId,
            IdentityStatus::Verified,
            Some("GALE01"),
            IdentityConfidence::ExactBytes,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&gamecube);
    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "GameCube".to_string(),
            game_key: "GALE01".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::GameCubeGameId("GALE01".to_string())]
    );

    let wii = report(
        IdentityPlatform::Wii,
        vec![evidence(
            IdentityKind::DolphinGameId,
            IdentityStatus::Verified,
            Some("RMCE01"),
            IdentityConfidence::ExactBytes,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&wii);
    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Wii".to_string(),
            game_key: "RMCE01".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::WiiGameId("RMCE01".to_string())]
    );
}

#[test]
fn verified_loose_rom_sha256_resolves_with_no_fabricated_fact() {
    let source = report(
        IdentityPlatform::MegaDrive,
        vec![
            evidence(
                IdentityKind::LooseRomSha256,
                IdentityStatus::Verified,
                Some("aa".repeat(32).as_str()),
                IdentityConfidence::ExactBytes,
            ),
            // A display title accompanies the hash in real reports, but must
            // never itself become identity - see the next test.
            evidence(
                IdentityKind::LooseRomTitle,
                IdentityStatus::Verified,
                Some("sonic the hedgehog"),
                IdentityConfidence::CatalogueContext,
            ),
        ],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "MegaDrive".to_string(),
            game_key: "aa".repeat(32),
        })
    );
    assert!(
        facts.is_empty(),
        "no VerifiedIdentityFact variant exists for a generic cartridge hash - the bridge must \
         not invent one"
    );
}

#[test]
fn nes_verified_loose_rom_sha256_resolves_with_no_fabricated_fact() {
    let source = report(
        IdentityPlatform::Nes,
        vec![evidence(
            IdentityKind::LooseRomSha256,
            IdentityStatus::Verified,
            Some("bb".repeat(32).as_str()),
            IdentityConfidence::ExactBytes,
        )],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "NES".to_string(),
            game_key: "bb".repeat(32),
        })
    );
    assert!(
        facts.is_empty(),
        "no VerifiedIdentityFact variant exists for a generic cartridge hash - the bridge must \
         not invent one"
    );
}

#[test]
fn game_boy_family_verified_loose_rom_sha256_resolves_with_no_fabricated_fact() {
    for (platform, platform_id) in [
        (IdentityPlatform::GameBoy, "Game Boy"),
        (IdentityPlatform::GameBoyColor, "Game Boy Color"),
        (IdentityPlatform::GameBoyAdvance, "Game Boy Advance"),
    ] {
        let source = report(
            platform,
            vec![evidence(
                IdentityKind::LooseRomSha256,
                IdentityStatus::Verified,
                Some("cc".repeat(32).as_str()),
                IdentityConfidence::ExactBytes,
            )],
        );

        let (status, facts) = canonical_identity_from_game_report(&source);

        assert_eq!(
            status,
            CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: platform_id.to_string(),
                game_key: "cc".repeat(32),
            }),
            "failed for {platform:?}"
        );
        assert!(
            facts.is_empty(),
            "no VerifiedIdentityFact variant exists for a generic cartridge hash - the bridge \
             must not invent one ({platform:?})"
        );
    }
}

#[test]
fn n64_prefers_canonical_hash_as_game_key_with_no_fabricated_fact() {
    let source = report(
        IdentityPlatform::N64,
        vec![
            evidence(
                IdentityKind::LooseRomSha256,
                IdentityStatus::Verified,
                Some("dd".repeat(32).as_str()),
                IdentityConfidence::ExactBytes,
            ),
            evidence(
                IdentityKind::LooseRomCanonicalSha256,
                IdentityStatus::Verified,
                Some("ee".repeat(32).as_str()),
                IdentityConfidence::ExactBytes,
            ),
        ],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "N64".to_string(),
            game_key: "ee".repeat(32),
        }),
        "the canonical (byte-order-normalized) hash must be preferred as the game key"
    );
    assert!(
        facts.is_empty(),
        "no VerifiedIdentityFact variant exists for a generic cartridge hash - the bridge must \
         not invent one"
    );
}

#[test]
fn n64_falls_back_to_physical_hash_when_canonical_is_unavailable() {
    // A malformed/unrecognized header still leaves a real physical hash -
    // the resolved identity must fall back to it rather than reporting
    // Unknown just because normalization couldn't happen.
    let source = report(
        IdentityPlatform::N64,
        vec![evidence(
            IdentityKind::LooseRomSha256,
            IdentityStatus::Verified,
            Some("ff".repeat(32).as_str()),
            IdentityConfidence::ExactBytes,
        )],
    );

    let (status, facts) = canonical_identity_from_game_report(&source);

    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "N64".to_string(),
            game_key: "ff".repeat(32),
        })
    );
    assert!(facts.is_empty());
}

// --- unknown stays Unknown ---------------------------------------------------

#[test]
fn no_verified_evidence_stays_unknown() {
    let source = report(
        IdentityPlatform::PlayStation2,
        vec![evidence(
            IdentityKind::Ps2Serial,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(status, CanonicalIdentityStatus::Unknown);
    assert!(facts.is_empty());
}

#[test]
fn undetermined_platform_stays_unknown_even_with_verified_evidence() {
    // `Other` means the platform itself was never determined - there is no
    // launch-planner platform id to hand a `ResolvedIdentity` without
    // inventing one, regardless of what content evidence exists.
    let source = report(
        IdentityPlatform::Other,
        vec![evidence(
            IdentityKind::LooseRomSha256,
            IdentityStatus::Verified,
            Some("bb".repeat(32).as_str()),
            IdentityConfidence::ExactBytes,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(status, CanonicalIdentityStatus::Unknown);
    assert!(facts.is_empty());
}

// --- ambiguous/conflicting stays Conflicting --------------------------------

#[test]
fn two_different_verified_serials_conflict() {
    let source = report(
        IdentityPlatform::PlayStation2,
        vec![
            evidence(
                IdentityKind::Ps2Serial,
                IdentityStatus::Verified,
                Some("SLUS-98765"),
                IdentityConfidence::ExactBytes,
            ),
            evidence(
                IdentityKind::Ps2Serial,
                IdentityStatus::Verified,
                Some("SLUS-11111"),
                IdentityConfidence::ExactBytes,
            ),
        ],
    );
    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(status, CanonicalIdentityStatus::Conflicting);
    assert!(
        facts.is_empty(),
        "a conflicting result must never carry a fact - there is no single correct one to name"
    );
}

// --- weak filename-only evidence never becomes Resolved ---------------------

#[test]
fn filename_only_confidence_never_resolves_even_when_marked_verified() {
    // No real detector in this codebase currently does this - this proves
    // the bridge's own defensive filter, not a real detector bug.
    let source = report(
        IdentityPlatform::PlayStation2,
        vec![evidence(
            IdentityKind::Ps2Serial,
            IdentityStatus::Verified,
            Some("SLUS-98765"),
            IdentityConfidence::FilenameOnly,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(status, CanonicalIdentityStatus::Unknown);
    assert!(facts.is_empty());
}

#[test]
fn loose_rom_title_never_becomes_identity_on_its_own() {
    // `LooseRomTitle` is `Verified` status in real reports (it is a
    // deterministic display title) but is explicitly documented as "not
    // content identity" - it must never resolve anything by itself.
    let source = report(
        IdentityPlatform::MegaDrive,
        vec![evidence(
            IdentityKind::LooseRomTitle,
            IdentityStatus::Verified,
            Some("sonic the hedgehog"),
            IdentityConfidence::CatalogueContext,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(status, CanonicalIdentityStatus::Unknown);
    assert!(facts.is_empty());
}

// --- no fabricated verified identity facts ----------------------------------

#[test]
fn xbox_360_resolves_identity_but_never_the_original_xbox_fact_variant() {
    let source = report(
        IdentityPlatform::Xbox360,
        vec![evidence(
            IdentityKind::XexTitleId,
            IdentityStatus::Verified,
            Some("4D5307E6"),
            IdentityConfidence::ExactBytes,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Xbox360".to_string(),
            game_key: "4D5307E6".to_string(),
        })
    );
    assert!(
        facts.is_empty(),
        "VerifiedIdentityFact::XboxTitleId names the original Xbox, a different platform - it \
         must never be fabricated for a 360 title id"
    );
}

#[test]
fn xbox_resolves_identity_and_produces_the_xbox_title_id_fact() {
    let source = report(
        IdentityPlatform::Xbox,
        vec![evidence(
            IdentityKind::XbeTitleId,
            IdentityStatus::Verified,
            Some("4D530058"),
            IdentityConfidence::ExactBytes,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&source);
    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Xbox".to_string(),
            game_key: "4D530058".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::XboxTitleId("4D530058".to_string())]
    );
}

#[test]
fn xbox_and_xbox_360_verified_facts_never_cross_authorize() {
    // A verified Xbox 360 XEX title ID must never resolve as original Xbox,
    // and a verified original-Xbox XBE title ID must never resolve as Xbox
    // 360 - the two platforms are wholly distinct even though their names
    // and evidence kinds look similar.
    let as_xbox360 = report(
        IdentityPlatform::Xbox360,
        vec![evidence(
            IdentityKind::XexTitleId,
            IdentityStatus::Verified,
            Some("4D5307E6"),
            IdentityConfidence::ExactBytes,
        )],
    );
    let (status, _) = canonical_identity_from_game_report(&as_xbox360);
    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Xbox360".to_string(),
            game_key: "4D5307E6".to_string(),
        })
    );

    let as_xbox = report(
        IdentityPlatform::Xbox,
        vec![evidence(
            IdentityKind::XbeTitleId,
            IdentityStatus::Verified,
            Some("4D530058"),
            IdentityConfidence::ExactBytes,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&as_xbox);
    assert_eq!(
        status,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Xbox".to_string(),
            game_key: "4D530058".to_string(),
        })
    );
    assert_eq!(
        facts,
        vec![VerifiedIdentityFact::XboxTitleId("4D530058".to_string())]
    );

    // A report claiming platform Xbox but only ever carrying a 360-shaped
    // XexTitleId fact (never genuinely produced by real `game_identity`
    // code, but this bridge must not assume that) resolves to Unknown, not
    // a fabricated Xbox identity - `XexTitleId` is not in Xbox's own
    // identity-conferring lookup.
    let mismatched = report(
        IdentityPlatform::Xbox,
        vec![evidence(
            IdentityKind::XexTitleId,
            IdentityStatus::Verified,
            Some("4D5307E6"),
            IdentityConfidence::ExactBytes,
        )],
    );
    let (status, facts) = canonical_identity_from_game_report(&mismatched);
    assert_eq!(status, CanonicalIdentityStatus::Unknown);
    assert!(facts.is_empty());
}

// ---------------------------------------------------------------------------
// Content fixtures
// ---------------------------------------------------------------------------

fn sample_archive_identity(display_name: &str) -> ArchiveIdentity {
    ArchiveIdentity {
        display_name: display_name.to_string(),
        normalized_name: display_name.to_ascii_lowercase(),
        source_root: PathBuf::from("/library"),
        size_bytes: None,
        modified_time: None,
        platform: None,
        platform_provenance: None,
        region: None,
        content_hash: None,
        archive_hash: None,
        internal_listing_hash: None,
        filesystem_device: None,
        filesystem_inode: None,
        source_filesystem_device: None,
        source_filesystem_inode: None,
    }
}

fn sample_archive(path: &str, kind: ArchiveKind) -> Archive {
    Archive {
        path: PathBuf::from(path),
        kind,
        identity: sample_archive_identity(path),
        health: ArchiveHealth::Pending,
    }
}

fn sample_record(archive: Archive, mount_state: MountState) -> ArchiveRecord {
    let mount_path = PathBuf::from("/mnt/emuwiz").join(&archive.identity.normalized_name);
    let mount_plan = MountPlan::new(archive, mount_path);
    ArchiveRecord::new(
        mount_plan,
        mount_state,
        ArchiveMetadata::empty(),
        ArchiveHealth::Pending,
    )
}

// --- loose/direct content with exact path -> resolved LaunchContentRef -----

#[test]
fn direct_game_image_resolves_to_its_own_path() {
    let record = sample_record(
        sample_archive("/library/Game.iso", ArchiveKind::DirectGameImage),
        MountState::NotMountable,
    );

    let content = launch_content_ref_from_archive_record(&record, None);

    assert_eq!(
        content.resolved_path,
        Some(PathBuf::from("/library/Game.iso"))
    );
    assert!(!content.requires_mount);
    assert!(content.has_runnable_path());
}

#[test]
fn loose_mega_drive_rom_resolves_to_its_own_path_as_cartridge_content() {
    let record = sample_record(
        sample_archive("/library/Sonic.md", ArchiveKind::MegaDriveRom),
        MountState::NotMountable,
    );

    let content = launch_content_ref_from_archive_record(&record, None);

    assert_eq!(
        content.resolved_path,
        Some(PathBuf::from("/library/Sonic.md"))
    );
    assert!(!content.requires_mount);
    assert_eq!(content.kind, Some(LaunchContentKind::Cartridge));
}

// --- archive container -> outer archive path is NOT used as runnable content

#[test]
fn zip_container_never_uses_its_own_path_as_the_runnable_content() {
    let record = sample_record(
        sample_archive("/library/Game.zip", ArchiveKind::Zip),
        MountState::Mounted,
    );

    // No resolved inner member supplied at all.
    let content = launch_content_ref_from_archive_record(&record, None);

    assert_ne!(
        content.resolved_path,
        Some(PathBuf::from("/library/Game.zip"))
    );
    assert_eq!(content.resolved_path, None);
    assert!(content.requires_mount);
    assert_eq!(content.container, Some(LaunchContainerKind::Archive));
}

// --- mounted archive with no selected inner member remains unresolved ------

#[test]
fn mounted_archive_without_a_resolved_member_remains_unresolved() {
    let record = sample_record(
        sample_archive("/library/Game.zip", ArchiveKind::Zip),
        MountState::Mounted,
    );

    let content = launch_content_ref_from_archive_record(&record, None);

    assert!(!content.has_runnable_path());
    assert!(content.requires_mount);
}

#[test]
fn mounted_archive_with_a_resolved_member_becomes_runnable() {
    let record = sample_record(
        sample_archive("/library/Game.zip", ArchiveKind::Zip),
        MountState::Mounted,
    );
    let member = PathBuf::from("/mnt/emuwiz/game.zip/Disc 1/Game.iso");

    let content = launch_content_ref_from_archive_record(&record, Some(&member));

    assert_eq!(content.resolved_path, Some(member));
    assert!(content.requires_mount);
    assert!(content.has_runnable_path());
}

// --- unresolved content remains unresolved ----------------------------------

#[test]
fn unmounted_archive_with_a_claimed_member_path_still_stays_unresolved() {
    // A caller-supplied member path against a record that is not actually
    // mounted must never be trusted at face value.
    let record = sample_record(
        sample_archive("/library/Game.7z", ArchiveKind::SevenZip),
        MountState::Pending,
    );
    let claimed_member = PathBuf::from("/mnt/emuwiz/game.7z/Game.iso");

    let content = launch_content_ref_from_archive_record(&record, Some(&claimed_member));

    assert!(!content.has_runnable_path());
    assert_eq!(content.resolved_path, None);
}

// --- requires_mount semantics are honest ------------------------------------

#[test]
fn requires_mount_is_true_for_every_container_kind_regardless_of_mount_state() {
    for (kind, state) in [
        (ArchiveKind::Zip, MountState::Pending),
        (ArchiveKind::SevenZip, MountState::Mounted),
        (ArchiveKind::Rar, MountState::MountPathExists),
    ] {
        let record = sample_record(sample_archive("/library/Game", kind), state);
        let content = launch_content_ref_from_archive_record(&record, None);
        assert!(
            content.requires_mount,
            "{kind:?}/{state:?} must require a mount"
        );
    }
}

#[test]
fn requires_mount_is_false_for_every_loose_direct_kind() {
    for kind in [ArchiveKind::DirectGameImage, ArchiveKind::MegaDriveRom] {
        let record = sample_record(sample_archive("/library/Game", kind), MountState::Pending);
        let content = launch_content_ref_from_archive_record(&record, None);
        assert!(
            !content.requires_mount,
            "{kind:?} must never require a mount"
        );
    }
}
