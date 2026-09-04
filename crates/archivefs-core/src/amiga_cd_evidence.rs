//! Shared, read-only evidence and readiness for Amiga CD32 and CDTV media.
//!
//! The platform registry already keeps `Amiga`, `AmigaCD32`, and
//! `Commodore CDTV` distinct. This module supplies the missing typed contract
//! that future Amiberry and FS-UAE launchers can consume. It never creates a
//! launch plan, opens media, mutates firmware, or performs network I/O.

use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::readiness::LaunchReadiness;

pub const AMIGA_PLATFORM_ID: &str = "Amiga";
pub const AMIGA_CD32_PLATFORM_ID: &str = "AmigaCD32";
pub const AMIGA_CDTV_PLATFORM_ID: &str = "Commodore CDTV";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaCdMachine {
    OrdinaryAmiga,
    Cd32,
    Cdtv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaCdFirmwareState {
    Verified,
    PresentUnverified,
    Missing,
    Unreadable,
    WrongMachine,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmigaCdFirmwareEvidence {
    pub main_kickstart: AmigaCdFirmwareState,
    pub extended_rom: AmigaCdFirmwareState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaCdMediaFormat {
    CueBin,
    Iso,
    Chd,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmigaCdMediaEvidence {
    pub format: AmigaCdMediaFormat,
    pub complete: bool,
    /// A platform claim from disc/provider metadata, not an extension guess.
    pub identified_platform: Option<AmigaCdMachine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaCdEvidenceSource {
    CanonicalPlatform,
    ProviderDat,
    DiscMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmigaCdPlatformClaim {
    pub machine: AmigaCdMachine,
    pub source: AmigaCdEvidenceSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmigaCdIdentityEvidence {
    pub claims: Vec<AmigaCdPlatformClaim>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaCdReadinessBlocker {
    IdentityUnresolved,
    IdentityConflict,
    PlatformUnproven,
    PlatformConflict,
    MachineMismatch,
    FirmwareMissing,
    FirmwareUnreadable,
    FirmwareWrongMachine,
    FirmwareUnknown,
    MediaUnsupported,
    MediaIncomplete,
    MediaPlatformMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaCdMachineReadiness {
    pub machine: AmigaCdMachine,
    pub platform_id: Option<String>,
    pub firmware_evidence: AmigaCdFirmwareEvidence,
    pub media_evidence: AmigaCdMediaEvidence,
    pub blockers: Vec<AmigaCdReadinessBlocker>,
    pub readiness: LaunchReadiness,
}

fn platform_for_identity(
    identity: &CanonicalIdentityStatus,
) -> Result<AmigaCdMachine, AmigaCdReadinessBlocker> {
    match identity {
        CanonicalIdentityStatus::Resolved(value) => match value.platform_id.as_str() {
            AMIGA_PLATFORM_ID => Ok(AmigaCdMachine::OrdinaryAmiga),
            AMIGA_CD32_PLATFORM_ID => Ok(AmigaCdMachine::Cd32),
            AMIGA_CDTV_PLATFORM_ID => Ok(AmigaCdMachine::Cdtv),
            _ => Err(AmigaCdReadinessBlocker::PlatformUnproven),
        },
        CanonicalIdentityStatus::Unknown => Err(AmigaCdReadinessBlocker::IdentityUnresolved),
        CanonicalIdentityStatus::Conflicting => Err(AmigaCdReadinessBlocker::IdentityConflict),
    }
}

fn firmware_blocker(state: AmigaCdFirmwareState) -> Option<AmigaCdReadinessBlocker> {
    match state {
        AmigaCdFirmwareState::Verified | AmigaCdFirmwareState::PresentUnverified => None,
        AmigaCdFirmwareState::Missing => Some(AmigaCdReadinessBlocker::FirmwareMissing),
        AmigaCdFirmwareState::Unreadable => Some(AmigaCdReadinessBlocker::FirmwareUnreadable),
        AmigaCdFirmwareState::WrongMachine => Some(AmigaCdReadinessBlocker::FirmwareWrongMachine),
        AmigaCdFirmwareState::Unknown => Some(AmigaCdReadinessBlocker::FirmwareUnknown),
    }
}

/// Classifies already-collected evidence for future native Amiberry/FS-UAE
/// adapters. This function is pure and does not infer platform from a path.
pub fn assess_amiga_cd_readiness(
    identity: &CanonicalIdentityStatus,
    identity_evidence: &AmigaCdIdentityEvidence,
    machine: AmigaCdMachine,
    firmware_evidence: AmigaCdFirmwareEvidence,
    media_evidence: AmigaCdMediaEvidence,
) -> AmigaCdMachineReadiness {
    let canonical = platform_for_identity(identity);
    let platform_id = match identity {
        CanonicalIdentityStatus::Resolved(value) => Some(value.platform_id.clone()),
        _ => None,
    };
    let mut blockers = Vec::new();
    let canonical_machine = match canonical {
        Ok(value) => Some(value),
        Err(blocker) => {
            blockers.push(blocker);
            None
        }
    };
    if canonical_machine == Some(AmigaCdMachine::OrdinaryAmiga) {
        // A generic Amiga identity does not identify which CD-capable
        // platform owns the disc. A CD extension cannot promote it.
        blockers.push(AmigaCdReadinessBlocker::PlatformUnproven);
    }
    let mut claimed = identity_evidence.claims.iter().map(|claim| claim.machine);
    if let Some(first) = claimed.next() {
        if claimed.any(|value| value != first) {
            blockers.push(AmigaCdReadinessBlocker::PlatformConflict);
        } else if canonical_machine != Some(first) {
            blockers.push(AmigaCdReadinessBlocker::PlatformConflict);
        }
    } else if canonical_machine.is_none() {
        blockers.push(AmigaCdReadinessBlocker::PlatformUnproven);
    }
    if canonical_machine != Some(machine) {
        blockers.push(AmigaCdReadinessBlocker::MachineMismatch);
    }
    if let Some(blocker) = firmware_blocker(firmware_evidence.main_kickstart) {
        blockers.push(blocker);
    }
    if machine != AmigaCdMachine::OrdinaryAmiga
        && let Some(blocker) = firmware_blocker(firmware_evidence.extended_rom)
    {
        blockers.push(blocker);
    }
    if !matches!(
        media_evidence.format,
        AmigaCdMediaFormat::CueBin | AmigaCdMediaFormat::Iso | AmigaCdMediaFormat::Chd
    ) {
        blockers.push(AmigaCdReadinessBlocker::MediaUnsupported);
    }
    if !media_evidence.complete {
        blockers.push(AmigaCdReadinessBlocker::MediaIncomplete);
    }
    if media_evidence.identified_platform != canonical_machine {
        blockers.push(AmigaCdReadinessBlocker::MediaPlatformMismatch);
    }
    let warning = firmware_evidence.main_kickstart == AmigaCdFirmwareState::PresentUnverified
        || (machine != AmigaCdMachine::OrdinaryAmiga
            && firmware_evidence.extended_rom == AmigaCdFirmwareState::PresentUnverified);
    AmigaCdMachineReadiness {
        machine,
        platform_id,
        firmware_evidence,
        media_evidence,
        readiness: if blockers.is_empty() {
            if warning {
                LaunchReadiness::ReadyWithWarnings
            } else {
                LaunchReadiness::Ready
            }
        } else {
            LaunchReadiness::Blocked
        },
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::ResolvedIdentity;

    fn identity(platform: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: "disc".into(),
        })
    }
    fn evidence(machine: AmigaCdMachine) -> AmigaCdIdentityEvidence {
        AmigaCdIdentityEvidence {
            claims: vec![AmigaCdPlatformClaim {
                machine,
                source: AmigaCdEvidenceSource::ProviderDat,
            }],
        }
    }
    fn firmware() -> AmigaCdFirmwareEvidence {
        AmigaCdFirmwareEvidence {
            main_kickstart: AmigaCdFirmwareState::Verified,
            extended_rom: AmigaCdFirmwareState::Verified,
        }
    }
    fn media(machine: AmigaCdMachine, format: AmigaCdMediaFormat) -> AmigaCdMediaEvidence {
        AmigaCdMediaEvidence {
            format,
            complete: true,
            identified_platform: Some(machine),
        }
    }

    #[test]
    fn cd32_and_cdtv_are_distinct_canonical_machines() {
        let cd32 = assess_amiga_cd_readiness(
            &identity(AMIGA_CD32_PLATFORM_ID),
            &evidence(AmigaCdMachine::Cd32),
            AmigaCdMachine::Cd32,
            firmware(),
            media(AmigaCdMachine::Cd32, AmigaCdMediaFormat::CueBin),
        );
        let cdtv = assess_amiga_cd_readiness(
            &identity(AMIGA_CDTV_PLATFORM_ID),
            &evidence(AmigaCdMachine::Cdtv),
            AmigaCdMachine::Cdtv,
            firmware(),
            media(AmigaCdMachine::Cdtv, AmigaCdMediaFormat::Iso),
        );
        assert_eq!(cd32.readiness, LaunchReadiness::Ready);
        assert_eq!(cdtv.readiness, LaunchReadiness::Ready);
        assert_ne!(cd32.machine, cdtv.machine);
    }

    #[test]
    fn generic_amiga_and_extension_alone_do_not_prove_cd_platform() {
        let report = assess_amiga_cd_readiness(
            &identity(AMIGA_PLATFORM_ID),
            &AmigaCdIdentityEvidence::default(),
            AmigaCdMachine::OrdinaryAmiga,
            firmware(),
            media(AmigaCdMachine::OrdinaryAmiga, AmigaCdMediaFormat::Iso),
        );
        assert!(
            report
                .blockers
                .contains(&AmigaCdReadinessBlocker::PlatformUnproven)
        );
        let report = assess_amiga_cd_readiness(
            &CanonicalIdentityStatus::Unknown,
            &AmigaCdIdentityEvidence::default(),
            AmigaCdMachine::Cd32,
            firmware(),
            media(AmigaCdMachine::Cd32, AmigaCdMediaFormat::Iso),
        );
        assert_eq!(report.readiness, LaunchReadiness::Blocked);
    }

    #[test]
    fn firmware_and_media_fail_closed() {
        let mut bad = firmware();
        bad.main_kickstart = AmigaCdFirmwareState::WrongMachine;
        bad.extended_rom = AmigaCdFirmwareState::Missing;
        let report = assess_amiga_cd_readiness(
            &identity(AMIGA_CD32_PLATFORM_ID),
            &evidence(AmigaCdMachine::Cd32),
            AmigaCdMachine::Cd32,
            bad,
            media(AmigaCdMachine::Cd32, AmigaCdMediaFormat::CueBin),
        );
        assert!(
            report
                .blockers
                .contains(&AmigaCdReadinessBlocker::FirmwareWrongMachine)
        );
        assert!(
            report
                .blockers
                .contains(&AmigaCdReadinessBlocker::FirmwareMissing)
        );
        let report = assess_amiga_cd_readiness(
            &identity(AMIGA_CDTV_PLATFORM_ID),
            &evidence(AmigaCdMachine::Cdtv),
            AmigaCdMachine::Cdtv,
            firmware(),
            AmigaCdMediaEvidence {
                format: AmigaCdMediaFormat::Unsupported,
                complete: false,
                identified_platform: Some(AmigaCdMachine::Cdtv),
            },
        );
        assert!(
            report
                .blockers
                .contains(&AmigaCdReadinessBlocker::MediaUnsupported)
        );
        assert!(
            report
                .blockers
                .contains(&AmigaCdReadinessBlocker::MediaIncomplete)
        );
    }

    #[test]
    fn conflicting_platform_evidence_is_ambiguous() {
        let evidence = AmigaCdIdentityEvidence {
            claims: vec![
                AmigaCdPlatformClaim {
                    machine: AmigaCdMachine::Cd32,
                    source: AmigaCdEvidenceSource::ProviderDat,
                },
                AmigaCdPlatformClaim {
                    machine: AmigaCdMachine::Cdtv,
                    source: AmigaCdEvidenceSource::DiscMetadata,
                },
            ],
        };
        let report = assess_amiga_cd_readiness(
            &identity(AMIGA_CD32_PLATFORM_ID),
            &evidence,
            AmigaCdMachine::Cd32,
            firmware(),
            media(AmigaCdMachine::Cd32, AmigaCdMediaFormat::Chd),
        );
        assert!(
            report
                .blockers
                .contains(&AmigaCdReadinessBlocker::PlatformConflict)
        );
    }

    #[test]
    fn unverified_firmware_is_warning_but_not_verified() {
        let firmware_evidence = AmigaCdFirmwareEvidence {
            main_kickstart: AmigaCdFirmwareState::PresentUnverified,
            extended_rom: AmigaCdFirmwareState::PresentUnverified,
        };
        let report = assess_amiga_cd_readiness(
            &identity(AMIGA_CD32_PLATFORM_ID),
            &evidence(AmigaCdMachine::Cd32),
            AmigaCdMachine::Cd32,
            firmware_evidence,
            media(AmigaCdMachine::Cd32, AmigaCdMediaFormat::Iso),
        );
        assert_eq!(report.readiness, LaunchReadiness::ReadyWithWarnings);
        assert_ne!(
            report.firmware_evidence.main_kickstart,
            AmigaCdFirmwareState::Verified
        );
    }
}
