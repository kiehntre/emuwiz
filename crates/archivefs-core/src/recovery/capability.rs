//! The engine's reviewed capability matrix.
//!
//! One row per (system, stored format): how the engine recognises it, which
//! emulator route can run it, which firmware that needs, what the engine may
//! safely prepare, and where it stops. A row is only `FixableByPreparation`
//! when the transformation has been validated end to end; everything else is
//! stated as unsupported or not assessed rather than implied.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// EmuWiz starts this format as it is; the engine has nothing to add.
    PlayableDirectly,
    /// The engine can prepare a verified playable copy.
    FixableByPreparation,
    /// A reviewed rule: no safe preparation exists for this format.
    Unsupported,
    /// Not reviewed. The engine makes no claim.
    NotAssessed,
}

impl CapabilityStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::PlayableDirectly => "playable directly",
            Self::FixableByPreparation => "fixable by verified preparation",
            Self::Unsupported => "unsupported",
            Self::NotAssessed => "not assessed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CapabilityRow {
    pub platform_id: &'static str,
    /// Lower-case extension of the stored file, or `folder`.
    pub stored_format: &'static str,
    pub contents: &'static str,
    pub detection: &'static str,
    pub emulator_route: &'static str,
    pub firmware: &'static str,
    pub preparation: &'static str,
    pub status: CapabilityStatus,
    pub limits: &'static str,
}

pub const AMIGA_CD32: &str = crate::amiga_cd_evidence::AMIGA_CD32_PLATFORM_ID;

pub const CAPABILITY_MATRIX: &[CapabilityRow] = &[
    CapabilityRow {
        platform_id: AMIGA_CD32,
        stored_format: "zip",
        contents: "cue sheet + data track + WAV audio tracks",
        detection: "ZIP index; cue sheet decoded and CRC-checked; every named track present",
        emulator_route: "RetroArch PUAE core",
        firmware: "kick40060.CD32 and kick40060.CD32.ext in RetroArch's system folder (your own files)",
        preparation: "verified extraction of the set to new files",
        status: CapabilityStatus::FixableByPreparation,
        limits: "top-level files only; at most 128 files, 1 GiB per file, 2 GiB per set; deflate or stored",
    },
    CapabilityRow {
        platform_id: AMIGA_CD32,
        stored_format: "zip",
        contents: "a single .iso with no cue sheet",
        detection: "ZIP index",
        emulator_route: "RetroArch PUAE core",
        firmware: "as above",
        preparation: "none yet",
        status: CapabilityStatus::NotAssessed,
        limits: "extraction would be the same operation; launch of a bare ISO is not validated",
    },
    CapabilityRow {
        platform_id: AMIGA_CD32,
        stored_format: "cue",
        contents: "loose cue sheet with its tracks",
        detection: "catalogue row",
        emulator_route: "RetroArch PUAE core",
        firmware: "as above",
        preparation: "not needed",
        status: CapabilityStatus::PlayableDirectly,
        limits: "system known from the folder only: the Launch page asks before the first start",
    },
    CapabilityRow {
        platform_id: AMIGA_CD32,
        stored_format: "chd",
        contents: "compressed CD image",
        detection: "catalogue row",
        emulator_route: "none proven",
        firmware: "-",
        preparation: "none",
        status: CapabilityStatus::Unsupported,
        limits: "CHD support in the installed Amiga emulators is not proven; no conversion is offered",
    },
    CapabilityRow {
        platform_id: "Commodore CDTV",
        stored_format: "zip",
        contents: "cue sheet + tracks",
        detection: "ZIP index",
        emulator_route: "none validated",
        firmware: "CDTV Kickstart and extended ROM",
        preparation: "none yet",
        status: CapabilityStatus::NotAssessed,
        limits: "same container as CD32, but the CDTV route has not been validated",
    },
    CapabilityRow {
        platform_id: "Amiga",
        stored_format: "lha",
        contents: "WHDLoad package",
        detection: "existing WHDLoad inspection",
        emulator_route: "FS-UAE or Amiberry WHDLoad",
        firmware: "Kickstart named by the game's slave",
        preparation: "not needed",
        status: CapabilityStatus::PlayableDirectly,
        limits: "needs the person's 7-Zip with LHA support",
    },
    CapabilityRow {
        platform_id: "Amiga",
        stored_format: "hdf",
        contents: "hard-disk image",
        detection: "existing HDF inspection",
        emulator_route: "RetroArch PUAE core, from a private working copy",
        firmware: "Kickstart for the chosen machine",
        preparation: "not needed (existing working-copy feature)",
        status: CapabilityStatus::PlayableDirectly,
        limits: "-",
    },
    CapabilityRow {
        platform_id: "Amiga",
        stored_format: "7z",
        contents: "any",
        detection: "extension",
        emulator_route: "none",
        firmware: "-",
        preparation: "none",
        status: CapabilityStatus::Unsupported,
        limits: "EmuWiz has no bounded 7z member extractor; nothing is extracted",
    },
    CapabilityRow {
        platform_id: "Xbox",
        stored_format: "iso",
        contents: "XISO or full disc image",
        detection: "existing XDVDFS identity",
        emulator_route: "xemu",
        firmware: "MCPX boot ROM, flash BIOS, EEPROM, hard-disk image",
        preparation: "not needed",
        status: CapabilityStatus::PlayableDirectly,
        limits: "-",
    },
    CapabilityRow {
        platform_id: "Xbox",
        stored_format: "zip",
        contents: "a disc image (.iso)",
        detection: "ZIP index (existing Launch page message)",
        emulator_route: "xemu, once extracted",
        firmware: "as above",
        preparation: "none yet",
        status: CapabilityStatus::Unsupported,
        limits: "a 7.3 GiB DVD image is above this engine's validated size; whether xemu starts a full disc image as extracted is not proven here",
    },
    CapabilityRow {
        platform_id: "Xbox",
        stored_format: "7z",
        contents: "any",
        detection: "extension",
        emulator_route: "none",
        firmware: "-",
        preparation: "none",
        status: CapabilityStatus::Unsupported,
        limits: "no bounded 7z member extractor; building a disc image from loose files is not a validated transformation",
    },
    CapabilityRow {
        platform_id: "Xbox",
        stored_format: "xbe",
        contents: "loose executable or extracted game folder",
        detection: "existing XBE identity",
        emulator_route: "none",
        firmware: "-",
        preparation: "none",
        status: CapabilityStatus::Unsupported,
        limits: "xemu starts disc images only; EmuWiz does not build one",
    },
    CapabilityRow {
        platform_id: "Xbox360",
        stored_format: "iso",
        contents: "disc image",
        detection: "catalogue row",
        emulator_route: "none (Xenia route accepts .xex only)",
        firmware: "-",
        preparation: "none",
        status: CapabilityStatus::Unsupported,
        limits: "no installed route starts an Xbox 360 disc image",
    },
    CapabilityRow {
        platform_id: "PS3",
        stored_format: "iso",
        contents: "disc image, or an extracted PS3_GAME folder",
        detection: "existing PARAM.SFO identity",
        emulator_route: "RPCS3",
        firmware: "PS3 system software installed in RPCS3",
        preparation: "not needed",
        status: CapabilityStatus::PlayableDirectly,
        limits: "encrypted disc images are not handled",
    },
    CapabilityRow {
        platform_id: "PS3",
        stored_format: "pkg",
        contents: "install package",
        detection: "package header (existing Launch page message)",
        emulator_route: "none",
        firmware: "-",
        preparation: "none",
        status: CapabilityStatus::Unsupported,
        limits: "installing a package changes RPCS3's own state and needs a licence file; EmuWiz does neither",
    },
];

/// Every row for this system and stored format (there can be more than one:
/// the same container can hold different things).
pub fn capability_rows(platform_id: &str, stored_format: &str) -> Vec<&'static CapabilityRow> {
    CAPABILITY_MATRIX
        .iter()
        .filter(|row| {
            row.platform_id == platform_id && row.stored_format.eq_ignore_ascii_case(stored_format)
        })
        .collect()
}

/// The matrix as a Markdown table, as printed by the CLI and kept in the docs.
pub fn capability_matrix_markdown() -> String {
    let mut text = String::from(
        "| System | Stored as | Contents | Detection evidence | Emulator route | Firmware | Safe preparation | Status | Limits |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    for row in CAPABILITY_MATRIX {
        text.push_str(&format!(
            "| {} | `.{}` | {} | {} | {} | {} | {} | {} | {} |\n",
            row.platform_id,
            row.stored_format,
            row.contents,
            row.detection,
            row.emulator_route,
            row.firmware,
            row.preparation,
            row.status.label(),
            row.limits,
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_validated_transformation_is_offered_as_fixable() {
        let fixable: Vec<_> = CAPABILITY_MATRIX
            .iter()
            .filter(|row| row.status == CapabilityStatus::FixableByPreparation)
            .collect();
        assert_eq!(fixable.len(), 1);
        assert_eq!(fixable[0].platform_id, AMIGA_CD32);
        assert_eq!(fixable[0].stored_format, "zip");
    }

    #[test]
    fn every_platform_named_is_a_registered_platform() {
        for row in CAPABILITY_MATRIX {
            assert!(
                crate::platform::platform_by_id(row.platform_id).is_some(),
                "{} is not a canonical platform id",
                row.platform_id
            );
        }
    }

    #[test]
    fn the_documented_matrix_is_the_matrix_in_code() {
        let documented = include_str!("../../../../docs/GAME_RECOVERY_ENGINE.md");
        for line in capability_matrix_markdown().lines() {
            assert!(
                documented.contains(line),
                "docs/GAME_RECOVERY_ENGINE.md is missing this capability row:\n{line}"
            );
        }
    }
}
