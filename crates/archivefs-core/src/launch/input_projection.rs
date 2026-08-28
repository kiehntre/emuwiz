//! Pure per-emulator launch-input projection.
//!
//! Given the canonical identity facts core's identity layer has *already*
//! verified for one game, this module builds the adapter-specific request
//! shape ([`crate::patch_manager::DuckStationGameRequest`],
//! [`crate::patch_manager::Pcsx2GameRequest`], etc.) that adapter's own
//! `inspect_*` function expects - or an explicit
//! [`LaunchInputProjection::Unavailable`] when no verified fact this
//! adapter accepts exists.
//!
//! # What this module is not
//!
//! - It never executes anything: no `std::process::Command`, no
//!   filesystem read, no network call, no config write, no command-line
//!   string is ever built here.
//! - It never resolves or fuses identity. [`VerifiedIdentityFact`] values
//!   must already be genuinely verified by the caller (the same identity
//!   layer [`crate::launch::planning::CanonicalIdentityStatus::Resolved`]
//!   already requires); this module only ever *routes* an already-verified
//!   fact to the one adapter request field it legitimately belongs to.
//! - It never promotes emulator-observed metadata, a filename, or an
//!   extension to verified identity - there is no such variant on
//!   [`VerifiedIdentityFact`] for this module to read in the first place,
//!   so that class of input simply cannot reach any projector function.
//!
//! # Why a fact list instead of reusing [`crate::launch::planning::ResolvedIdentity`]
//!
//! [`crate::launch::planning::ResolvedIdentity`] carries one opaque
//! `game_key` string per platform - exactly what the Phase 1 readiness
//! planner needs, and deliberately nothing more. Several adapters need to
//! distinguish between more than one *kind* of verified identity for the
//! same platform at once (PCSX2's own [`crate::patch_manager::Pcsx2GameRequest`]
//! carries an independent verified PS2 serial *and* an independent
//! verified executable CRC - see that struct's own field docs). A single
//! opaque string cannot carry that distinction, so this module works from
//! [`VerifiedIdentityFact`], a small closed enum where each variant names
//! both a platform and an identity kind. Because the variants are closed
//! and adapter-specific, a caller cannot construct, say, a PSP disc ID and
//! hand it to the PS2 projector by mistake without the compiler and this
//! module's own matching seeing straight through it - the projector for
//! one platform simply never looks for another platform's variant.
//!
//! # Reused request bridges
//!
//! PPSSPP and DuckStation already have a reviewed, tested bridge from a
//! verified identity string to their adapter request type -
//! [`crate::patch_manager::ppsspp_request`] and
//! [`crate::patch_manager::duckstation_request`]. This module calls those
//! directly rather than constructing
//! [`crate::patch_manager::PpssppGameRequest`]/[`crate::patch_manager::DuckStationGameRequest`]
//! by hand, so the two adapters keep exactly one reviewed seam between
//! verified identity and their request shape.

use crate::patch_manager::{
    AmigaGameRequest, DolphinGameRequest, DolphinTargetPlatform, DuckStationGameRequest,
    FlycastGameRequest, FlycastPlatform, HatariIdentityState, HatariSelectedGameRequest,
    Pcsx2GameRequest, PpssppGameRequest, Rpcs3GameRequest, XemuGameRequest, duckstation_request,
    ppsspp_request,
};

/// One already-verified identity fact for one game, tagged by both
/// platform and identity kind. Every variant here must only ever be
/// constructed by a caller from a genuinely verified value - the same
/// standard [`crate::launch::planning::CanonicalIdentityStatus::Resolved`]
/// already requires. There is deliberately no filename, extension, or
/// emulator-metadata variant: that class of input cannot be expressed as a
/// [`VerifiedIdentityFact`] at all, so it cannot reach any projector below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedIdentityFact {
    /// A verified PS1 serial (e.g. `SLUS-12345`).
    Ps1Serial(String),
    /// A verified PS2 serial (e.g. `SLUS-98765`).
    Ps2Serial(String),
    /// A verified PS2 executable CRC, independent of the serial above.
    Ps2ExecutableCrc(String),
    /// A verified PS3 title ID.
    Ps3TitleId(String),
    /// A verified PSP disc ID.
    PspDiscId(String),
    /// A verified original-Xbox title ID.
    XboxTitleId(String),
    /// A verified GameCube disc-header Game ID.
    GameCubeGameId(String),
    /// A verified Wii disc-header Game ID.
    WiiGameId(String),
    /// A verified Dreamcast product code.
    DreamcastProductCode(String),
    /// A verified Sega Saturn product number.
    SaturnProductCode(String),
    /// A verified Sega CD/Mega-CD Disc ID product code.
    SegaCdProductCode(String),
    /// A verified Atari ST title, matching
    /// [`crate::patch_manager::HatariSelectedGameRequest::verified_title`].
    AtariStTitle(String),
    /// A verified Amiga/WHDLoad identity string, matching
    /// [`crate::patch_manager::AmigaGameRequest::verified_amiga_identity`].
    AmigaIdentity(String),
}

/// The minimal request carried by the Sega CD RetroArch projection. The
/// generic RetroArch planner still selects the configured core; this request
/// preserves the independently verified on-disc product identity for adapter
/// consumers without inventing a Sega-CD-specific emulator engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegaCdGameRequest {
    pub verified_product_code: String,
}

fn find_ps1_serial(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::Ps1Serial(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_ps2_serial(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::Ps2Serial(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_ps2_executable_crc(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::Ps2ExecutableCrc(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_ps3_title_id(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::Ps3TitleId(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_psp_disc_id(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::PspDiscId(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_xbox_title_id(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::XboxTitleId(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_gamecube_game_id(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::GameCubeGameId(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_wii_game_id(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::WiiGameId(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_dreamcast_product_code(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::DreamcastProductCode(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_sega_cd_product_code(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::SegaCdProductCode(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_atari_st_title(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::AtariStTitle(value) => Some(value.clone()),
        _ => None,
    })
}

fn find_amiga_identity(facts: &[VerifiedIdentityFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::AmigaIdentity(value) => Some(value.clone()),
        _ => None,
    })
}

/// The outcome of projecting verified identity facts onto one adapter's
/// request shape. `Unavailable` is the fail-closed default: it is what
/// every projector returns when none of the supplied facts is the one
/// kind that adapter accepts (including when the fact list is empty,
/// which is what a caller passes for
/// [`crate::launch::planning::CanonicalIdentityStatus::Unknown`]/
/// [`crate::launch::planning::CanonicalIdentityStatus::Conflicting`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchInputProjection<T> {
    /// A verified fact of the right kind was found and used to build the
    /// adapter's own request type. This is never a guess: `T` is built
    /// only from the one matching [`VerifiedIdentityFact`] variant.
    Authorized(T),
    /// No verified fact this adapter accepts was found. Never a guess and
    /// never populated from another platform's identity or from
    /// emulator/filename/extension context.
    Unavailable { detail: &'static str },
}

/// Projects onto [`PpssppGameRequest`] via the existing
/// [`ppsspp_request`] bridge - see the module doc comment.
pub fn project_ppsspp_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<PpssppGameRequest> {
    match find_psp_disc_id(facts) {
        Some(disc_id) => LaunchInputProjection::Authorized(ppsspp_request(Some(disc_id))),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified PSP disc ID among the supplied identity facts",
        },
    }
}

/// Projects onto [`DuckStationGameRequest`] via the existing
/// [`duckstation_request`] bridge - see the module doc comment.
pub fn project_duckstation_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<DuckStationGameRequest> {
    match find_ps1_serial(facts) {
        Some(serial) => LaunchInputProjection::Authorized(duckstation_request(Some(serial))),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified PS1 serial among the supplied identity facts",
        },
    }
}

/// Projects onto [`Pcsx2GameRequest`]. PCSX2 accepts a verified PS2 serial
/// and/or a verified PS2 executable CRC independently - see that struct's
/// own field docs - so both are read here, but never anything from another
/// platform's fact variants.
pub fn project_pcsx2_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<Pcsx2GameRequest> {
    let serial = find_ps2_serial(facts);
    let crc = find_ps2_executable_crc(facts);
    if serial.is_none() && crc.is_none() {
        return LaunchInputProjection::Unavailable {
            detail: "no verified PS2 serial or executable CRC among the supplied identity facts",
        };
    }
    LaunchInputProjection::Authorized(Pcsx2GameRequest {
        verified_ps2_serial: serial,
        verified_executable_crc: crc,
        emulator_serial: None,
    })
}

/// Projects onto [`Rpcs3GameRequest`].
pub fn project_rpcs3_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<Rpcs3GameRequest> {
    match find_ps3_title_id(facts) {
        Some(title_id) => LaunchInputProjection::Authorized(Rpcs3GameRequest {
            verified_ps3_title_id: Some(title_id),
            emulator_game_id: None,
        }),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified PS3 title ID among the supplied identity facts",
        },
    }
}

/// Projects onto [`XemuGameRequest`].
pub fn project_xemu_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<XemuGameRequest> {
    match find_xbox_title_id(facts) {
        Some(title_id) => LaunchInputProjection::Authorized(XemuGameRequest {
            verified_xbox_title_id: Some(title_id),
            emulator_title_id: None,
            emulator_title_name: None,
        }),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified Xbox title ID among the supplied identity facts",
        },
    }
}

/// Projects onto [`DolphinGameRequest`] for GameCube. A Wii Game ID fact
/// never satisfies this - see [`project_dolphin_wii_launch_input`] for the
/// separate Wii target.
pub fn project_dolphin_gamecube_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<DolphinGameRequest> {
    match find_gamecube_game_id(facts) {
        Some(game_id) => LaunchInputProjection::Authorized(DolphinGameRequest {
            canonical_platform: Some("GameCube".to_string()),
            target_platform: Some(DolphinTargetPlatform::GameCube),
            verified_game_id: Some(game_id),
            verified_revision: None,
            emulator_game_id: None,
            disc_contexts: Vec::new(),
        }),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified GameCube Game ID among the supplied identity facts",
        },
    }
}

/// Projects onto [`DolphinGameRequest`] for Wii. A GameCube Game ID fact
/// never satisfies this - see [`project_dolphin_gamecube_launch_input`]
/// for the separate GameCube target.
pub fn project_dolphin_wii_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<DolphinGameRequest> {
    match find_wii_game_id(facts) {
        Some(game_id) => LaunchInputProjection::Authorized(DolphinGameRequest {
            canonical_platform: Some("Wii".to_string()),
            target_platform: Some(DolphinTargetPlatform::Wii),
            verified_game_id: Some(game_id),
            verified_revision: None,
            emulator_game_id: None,
            disc_contexts: Vec::new(),
        }),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified Wii Game ID among the supplied identity facts",
        },
    }
}

/// Projects onto [`FlycastGameRequest`].
pub fn project_flycast_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<FlycastGameRequest> {
    match find_dreamcast_product_code(facts) {
        Some(product_code) => LaunchInputProjection::Authorized(FlycastGameRequest {
            canonical_platform: Some("Dreamcast".to_string()),
            flycast_platform: Some(FlycastPlatform::Dreamcast),
            verified_dreamcast_product_code: Some(product_code),
            emulator_game_key: None,
            disc_contexts: Vec::new(),
        }),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified Dreamcast product code among the supplied identity facts",
        },
    }
}

/// Projects a verified Sega CD product code for the existing RetroArch
/// launch path. This does not select or install a core; core selection and
/// command construction remain the generic reviewed RetroArch machinery.
pub fn project_sega_cd_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<SegaCdGameRequest> {
    match find_sega_cd_product_code(facts) {
        Some(verified_product_code) => LaunchInputProjection::Authorized(SegaCdGameRequest {
            verified_product_code,
        }),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified Sega CD product code among the supplied identity facts",
        },
    }
}

/// Projects onto [`HatariSelectedGameRequest`].
pub fn project_hatari_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<HatariSelectedGameRequest> {
    match find_atari_st_title(facts) {
        Some(title) => LaunchInputProjection::Authorized(HatariSelectedGameRequest {
            canonical_platform: Some("AtariST".to_string()),
            identity_state: HatariIdentityState::Verified,
            verified_title: Some(title),
        }),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified Atari ST title among the supplied identity facts",
        },
    }
}

/// Projects onto [`AmigaGameRequest`].
pub fn project_amiga_whdload_launch_input(
    facts: &[VerifiedIdentityFact],
) -> LaunchInputProjection<AmigaGameRequest> {
    match find_amiga_identity(facts) {
        Some(identity) => LaunchInputProjection::Authorized(AmigaGameRequest {
            verified_amiga_identity: Some(identity),
            emulator_metadata: None,
            bare_slaves: Vec::new(),
            hdf_inspections: Vec::new(),
            selected_profile_path: None,
        }),
        None => LaunchInputProjection::Unavailable {
            detail: "no verified Amiga identity among the supplied identity facts",
        },
    }
}

/// Xenia (Xbox 360) has no per-game request/inspection type in this build
/// at all - unlike every other Phase 1 adapter, there is no
/// `patch_manager::xenia_local` request struct to project onto. Rather
/// than fabricate one, or silently reuse xemu's (a different platform's)
/// request shape, this always answers `Unavailable`: no legitimate
/// verified-identity projection exists yet for Xenia in this build.
pub fn project_xenia_launch_input(facts: &[VerifiedIdentityFact]) -> LaunchInputProjection<()> {
    let _ = facts;
    LaunchInputProjection::Unavailable {
        detail: "no Xenia launch-input request type exists in this build",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::{
        DuckStationSerialMapping, PpssppGameIdMapping, inspect_duckstation_game,
        inspect_ppsspp_game,
    };

    fn ppsspp_profile() -> crate::patch_manager::PpssppProfile {
        let dir =
            std::env::temp_dir().join(format!("archivefs-launch-ppsspp-{}", std::process::id()));
        crate::patch_manager::PpssppProfile {
            profile_id: "ppsspp:test".to_string(),
            installation_type: crate::patch_manager::PpssppInstallationType::Explicit,
            scope: crate::patch_manager::PpssppProfileScope::Explicit,
            configuration_path: dir.clone(),
            provenance: "test",
            eligible: true,
            blockers: Vec::new(),
            executable_candidates: Vec::new(),
            memstick_path: dir.join("PSP"),
            system_path: dir.join("PSP/SYSTEM"),
            global_config_path: dir.join("PSP/SYSTEM/ppsspp.ini"),
            cheats_path: dir.join("PSP/CHEATS"),
            textures_path: dir.join("PSP/TEXTURES"),
            savedata_path: dir.join("PSP/SAVEDATA"),
            game_path: dir.join("PSP/GAME"),
            state_path: dir.join("PSP/PPSSPP_STATE"),
        }
    }

    fn duckstation_profile() -> crate::patch_manager::DuckStationProfile {
        let dir = std::env::temp_dir().join(format!(
            "archivefs-launch-duckstation-{}",
            std::process::id()
        ));
        crate::patch_manager::DuckStationProfile {
            profile_id: "duckstation:test".to_string(),
            installation_type: crate::patch_manager::DuckStationInstallationType::Explicit,
            configuration_path: dir.clone(),
            provenance: "explicit caller-confirmed configuration root",
            eligible: true,
            blocker: None,
            executable_candidates: Vec::new(),
            global_config_path: dir.join("settings.ini"),
            game_settings_path: dir.join("gamesettings"),
            cheats_path: dir.join("cheats"),
            patches_path: dir.join("patches"),
            textures_path: dir.join("textures"),
            bios_path: dir.join("bios"),
            memory_cards_path: dir.join("memcards"),
            save_states_path: dir.join("savestates"),
        }
    }

    #[test]
    fn pcsx2_crc_cannot_populate_ppsspp() {
        let facts = vec![VerifiedIdentityFact::Ps2ExecutableCrc(
            "A1B2C3D4".to_string(),
        )];
        assert_eq!(
            project_ppsspp_launch_input(&facts),
            LaunchInputProjection::Unavailable {
                detail: "no verified PSP disc ID among the supplied identity facts",
            }
        );
    }

    #[test]
    fn psp_disc_id_cannot_populate_pcsx2() {
        let facts = vec![VerifiedIdentityFact::PspDiscId("ULUS10000".to_string())];
        assert_eq!(
            project_pcsx2_launch_input(&facts),
            LaunchInputProjection::Unavailable {
                detail: "no verified PS2 serial or executable CRC among the supplied identity facts",
            }
        );
    }

    #[test]
    fn psx_identity_cannot_populate_ps2() {
        let facts = vec![VerifiedIdentityFact::Ps1Serial("SLUS-12345".to_string())];
        assert_eq!(
            project_pcsx2_launch_input(&facts),
            LaunchInputProjection::Unavailable {
                detail: "no verified PS2 serial or executable CRC among the supplied identity facts",
            }
        );
        // The reverse direction also fails closed: a PS2 serial is not a
        // PS1 serial, even though both are opaque uppercase-ish strings.
        let ps2_facts = vec![VerifiedIdentityFact::Ps2Serial("SLUS-98765".to_string())];
        assert_eq!(
            project_duckstation_launch_input(&ps2_facts),
            LaunchInputProjection::Unavailable {
                detail: "no verified PS1 serial among the supplied identity facts",
            }
        );
    }

    #[test]
    fn unknown_or_conflicting_identity_produces_no_authorized_request() {
        let facts: Vec<VerifiedIdentityFact> = Vec::new();
        assert!(matches!(
            project_ppsspp_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_duckstation_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_pcsx2_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_rpcs3_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_xemu_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_dolphin_gamecube_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_dolphin_wii_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_flycast_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_hatari_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_amiga_whdload_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_xenia_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
    }

    /// [`VerifiedIdentityFact`] has no filename, extension, or path
    /// variant at all - it cannot be constructed from that kind of
    /// evidence. A caller who only has a filename/extension therefore has
    /// no fact to supply and passes an empty slice, exactly like the
    /// "unknown identity" case above: no projector ever authorizes a
    /// request from it.
    #[test]
    fn filename_or_extension_only_evidence_cannot_authorize_any_request() {
        let facts: Vec<VerifiedIdentityFact> = Vec::new();
        assert!(matches!(
            project_ppsspp_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(matches!(
            project_duckstation_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
    }

    #[test]
    fn xenia_has_no_authorized_projection_even_with_unrelated_facts() {
        let facts = vec![VerifiedIdentityFact::XboxTitleId("XYZ".to_string())];
        assert!(matches!(
            project_xenia_launch_input(&facts),
            LaunchInputProjection::Unavailable { .. }
        ));
    }

    #[test]
    fn ppsspp_projection_matches_the_existing_request_bridge() {
        let facts = vec![VerifiedIdentityFact::PspDiscId("ULUS10000".to_string())];
        let LaunchInputProjection::Authorized(request) = project_ppsspp_launch_input(&facts) else {
            panic!("expected an authorized PPSSPP request");
        };
        assert_eq!(request, ppsspp_request(Some("ULUS10000".to_string())));
        let inspection = inspect_ppsspp_game(&ppsspp_profile(), &request);
        assert_eq!(
            inspection.game_id_mapping,
            PpssppGameIdMapping::VerifiedPspDiscId
        );
    }

    #[test]
    fn duckstation_projection_matches_the_existing_request_bridge() {
        let facts = vec![VerifiedIdentityFact::Ps1Serial("SLUS-12345".to_string())];
        let LaunchInputProjection::Authorized(request) = project_duckstation_launch_input(&facts)
        else {
            panic!("expected an authorized DuckStation request");
        };
        assert_eq!(request, duckstation_request(Some("SLUS-12345".to_string())));
        let inspection = inspect_duckstation_game(&duckstation_profile(), &request);
        assert_eq!(
            inspection.serial_mapping,
            DuckStationSerialMapping::VerifiedPs1Serial
        );
    }

    #[test]
    fn flycast_projection_accepts_verified_dreamcast_product_code() {
        let facts = vec![VerifiedIdentityFact::DreamcastProductCode(
            "T-8109N".to_string(),
        )];
        let LaunchInputProjection::Authorized(request) = project_flycast_launch_input(&facts)
        else {
            panic!("verified Dreamcast product code must authorize Flycast input")
        };
        assert_eq!(request.canonical_platform.as_deref(), Some("Dreamcast"));
        assert_eq!(request.flycast_platform, Some(FlycastPlatform::Dreamcast));
        assert_eq!(
            request.verified_dreamcast_product_code.as_deref(),
            Some("T-8109N")
        );
    }

    #[test]
    fn sega_cd_projection_accepts_only_verified_sega_cd_product_code() {
        let facts = vec![VerifiedIdentityFact::SegaCdProductCode(
            "GM T-12345-00".to_string(),
        )];
        let LaunchInputProjection::Authorized(request) = project_sega_cd_launch_input(&facts)
        else {
            panic!("verified Sega CD product code must authorize the projection")
        };
        assert_eq!(request.verified_product_code, "GM T-12345-00");
        assert!(matches!(
            project_sega_cd_launch_input(&[VerifiedIdentityFact::DreamcastProductCode(
                "T-8109N".to_string()
            )]),
            LaunchInputProjection::Unavailable { .. }
        ));
    }

    #[test]
    fn pcsx2_accepts_serial_and_crc_independently() {
        let serial_only = vec![VerifiedIdentityFact::Ps2Serial("SLUS-98765".to_string())];
        let LaunchInputProjection::Authorized(request) = project_pcsx2_launch_input(&serial_only)
        else {
            panic!("expected an authorized PCSX2 request");
        };
        assert_eq!(request.verified_ps2_serial.as_deref(), Some("SLUS-98765"));
        assert!(request.verified_executable_crc.is_none());

        let both = vec![
            VerifiedIdentityFact::Ps2Serial("SLUS-98765".to_string()),
            VerifiedIdentityFact::Ps2ExecutableCrc("DEADBEEF".to_string()),
        ];
        let LaunchInputProjection::Authorized(request) = project_pcsx2_launch_input(&both) else {
            panic!("expected an authorized PCSX2 request");
        };
        assert_eq!(request.verified_ps2_serial.as_deref(), Some("SLUS-98765"));
        assert_eq!(request.verified_executable_crc.as_deref(), Some("DEADBEEF"));
    }

    #[test]
    fn dolphin_gamecube_and_wii_facts_never_cross_populate() {
        let gamecube_facts = vec![VerifiedIdentityFact::GameCubeGameId("GALE01".to_string())];
        assert!(matches!(
            project_dolphin_wii_launch_input(&gamecube_facts),
            LaunchInputProjection::Unavailable { .. }
        ));
        let wii_facts = vec![VerifiedIdentityFact::WiiGameId("RMCE01".to_string())];
        assert!(matches!(
            project_dolphin_gamecube_launch_input(&wii_facts),
            LaunchInputProjection::Unavailable { .. }
        ));
    }
}
