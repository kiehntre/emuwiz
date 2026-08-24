//! Thin, caller-side bridge from *already-verified* canonical identity to the
//! per-game inspection requests consumed by the PPSSPP and DuckStation
//! adapters.
//!
//! This module exists only to keep the verified-only contract explicit at the
//! seam between core identity and [`super::ppsspp_local`] /
//! [`super::duckstation_local`]. It does **not** produce, derive, or upgrade
//! identity, and it touches neither the identity engine nor the two adapter
//! modules' own logic.
//!
//! ## Contract
//!
//! - **Verified identity only.** Each function accepts an
//!   [`Option<String>`] that the caller must have already established as a
//!   genuinely verified PSP disc ID / PS1 serial. There is no filename,
//!   directory, or emulator-metadata input anywhere in this module, so a
//!   filename-only or unverified value cannot be "upgraded" into a verified
//!   lane - it simply does not exist as an input.
//! - **Emulator metadata stays advisory.** The adapter request's emulator
//!   lane ([`PpssppGameRequest::emulator_game_id`] ,
//!   [`DuckStationGameRequest::emulator_serial`]) is always left `None` here;
//!   anything read from PPSSPP/DuckStation context remains downstream and is
//!   never promoted.
//! - **Unknown / ambiguous / conflicting fails closed.** If the caller has no
//!   verified value it passes `None`, the adapter's own
//!   `Unavailable`/`Conflicting` fail-closed path runs rather than any guessed
//!   identity winning.

use super::duckstation_local::{
    DuckStationGameInspection, DuckStationGameRequest, DuckStationProfile, inspect_duckstation_game,
};
use super::ppsspp_local::{
    PpssppGameInspection, PpssppGameRequest, PpssppProfile, inspect_ppsspp_game,
};

/// Builds a [`PpssppGameRequest`] carrying only a caller-supplied verified PSP
/// disc ID. The emulator-metadata lane is deliberately left empty: PPSSPP
/// context must never be promoted to verified identity.
pub fn ppsspp_request(verified_psp_disc_id: Option<String>) -> PpssppGameRequest {
    PpssppGameRequest {
        verified_psp_disc_id,
        emulator_game_id: None,
    }
}

/// Builds a [`DuckStationGameRequest`] carrying only a caller-supplied
/// verified PS1 serial, with no emulator metadata, disc contexts, or playlist
/// supplied from this side. Those stay advisory and are left at their defaults
/// (empty / `None`).
pub fn duckstation_request(verified_ps1_serial: Option<String>) -> DuckStationGameRequest {
    DuckStationGameRequest {
        verified_ps1_serial,
        ..DuckStationGameRequest::default()
    }
}

/// Routes a verified PSP disc ID (if any) into [`inspect_ppsspp_game`] for one
/// discovered profile. A `None` disc ID is simply an empty request - the
/// adapter reports `Unavailable` rather than guessing.
pub fn inspect_ppsspp_game_for_verified(
    profile: &PpssppProfile,
    verified_psp_disc_id: Option<String>,
) -> PpssppGameInspection {
    inspect_ppsspp_game(profile, &ppsspp_request(verified_psp_disc_id))
}

/// Routes a verified PS1 serial (if any) into [`inspect_duckstation_game`] for
/// one discovered profile. A `None` serial fails closed with the adapter's
/// `Unavailable` serial mapping.
pub fn inspect_duckstation_game_for_verified(
    profile: &DuckStationProfile,
    verified_ps1_serial: Option<String>,
) -> DuckStationGameInspection {
    inspect_duckstation_game(profile, &duckstation_request(verified_ps1_serial))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_psp_disc_id_creates_a_ppsspp_request_with_no_emulator_lane() {
        let request = ppsspp_request(Some("ULUS10000".to_string()));
        assert_eq!(request.verified_psp_disc_id.as_deref(), Some("ULUS10000"));
        // No emulator metadata is ever promoted into the request.
        assert!(request.emulator_game_id.is_none());
    }

    #[test]
    fn unverified_ppsp_identity_fails_closed_to_unavailable() {
        // A caller with no verified ddisc ID supplies None; there is no
        // filename or emulator input that could "upgrade" identity.
        let request = ppsspp_request(None);
        assert!(request.verified_psp_disc_id.is_none());
        assert!(request.emulator_game_id.is_none());
        // Empty request through the adapter reports Unavailable, not a guess.
        let inspection = inspect_ppsspp_game(&temp_ppsspp_profile(), &request);
        assert_eq!(
            inspection.game_id_mapping,
            super::super::ppsspp_local::PpssppGameIdMapping::Unavailable
        );
    }

    #[test]
    fn verified_ps1_serial_creates_a_duckstation_request_with_no_advisory_input() {
        let request = duckstation_request(Some("SLUS-12345".to_string()));
        assert_eq!(request.verified_ps1_serial.as_deref(), Some("SLUS-12345"));
        assert!(request.emulator_serial.is_none());
        assert!(request.disc_contexts.is_empty());
        assert!(request.playlist_path.is_none());
    }

    #[test]
    fn unverified_duckstation_identity_fails_closed_to_unavailable() {
        let request = duckstation_request(None);
        assert!(request.verified_ps1_serial.is_none());
        assert!(request.emulator_serial.is_none());
        let inspection = inspect_duckstation_game(&temp_duckstation_profile(), &request);
        assert_eq!(
            inspection.serial_mapping,
            super::super::duckstation_local::DuckStationSerialMapping::Unavailable
        );
    }

    #[test]
    fn routing_helpers_pass_the_verified_value_through() {
        assert_eq!(
            inspect_ppsspp_game_for_verified(&temp_ppsspp_profile(), Some("ULUS10000".to_string()))
                .game_id
                .as_deref(),
            Some("ULUS10000")
        );
        assert_eq!(
            inspect_duckstation_game_for_verified(
                &temp_duckstation_profile(),
                Some("SLUS-12345".to_string())
            )
            .serial
            .as_deref(),
            Some("SLUS-12345")
        );
    }

    fn temp_ppsspp_profile() -> PpssppProfile {
        let dir =
            std::env::temp_dir().join(format!("archivefs-ppsspp-bridge-{}", std::process::id()));
        PpssppProfile {
            profile_id: "ppsspp:test".to_string(),
            installation_type: super::super::ppsspp_local::PpssppInstallationType::Explicit,
            scope: super::super::ppsspp_local::PpssppProfileScope::Explicit,
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

    fn temp_duckstation_profile() -> DuckStationProfile {
        let dir = std::env::temp_dir().join(format!(
            "archivefs-duckstation-bridge-{}",
            std::process::id()
        ));
        DuckStationProfile {
            profile_id: "duckstation:test".to_string(),
            installation_type:
                super::super::duckstation_local::DuckStationInstallationType::Explicit,
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
}
