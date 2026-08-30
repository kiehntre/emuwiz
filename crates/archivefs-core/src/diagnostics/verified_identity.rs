//! Read-only Doctor findings derived from the persisted verified-identity
//! fact cache ([`crate::verified_identity_cache`]).
//!
//! # What this reports, and what it never claims
//!
//! For a catalogued game whose platform is resolved and whose emulator
//! launch is gated on a specific verified identity fact, this notes when
//! that fact is:
//!
//! - **Missing** - never persisted, so a read-only consumer cannot explain
//!   identity and the existing launch path will stay blocked until a fresh
//!   inspection verifies it;
//! - **Stale** - persisted earlier, but the game file changed at the same
//!   path since, so the cached value describes a previous state;
//! - **Unknown** - persisted, but its freshness could not be compared
//!   against the current file.
//!
//! A **Current** fact produces no finding: there is nothing to report.
//!
//! Every finding here is [`DoctorSeverity::Info`]. A missing or stale launch
//! ID is not corruption and is never described as such - the game file may
//! be perfectly intact and simply not yet inspected. The persisted facts
//! are a cache; launch and cheat/mod execution re-verify from a fresh
//! [`crate::game_identity::GameIdentityReport`] regardless of what is cached
//! here, and the wording says so.
//!
//! # What is deliberately not reported
//!
//! - a platform that does not resolve to a known system;
//! - a platform whose launch does not require a per-game identity fact;
//! - any fact kind beyond the launch-gating (and, for PS2, patch/cheat)
//!   ones each platform actually needs.

use crate::game_identity::{IdentityKind, IdentityPlatform};
use crate::verified_identity_cache::{IdentityFactFreshness, PersistedIdentityFact};

use super::{DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding};
use crate::emulator_environment::EncodedPath;

/// One catalogued archive's persisted verified-identity facts, each already
/// paired with its freshness against the archive file's current identity.
///
/// Built outside the pure Doctor runner (freshness needs the file's current
/// `(device, inode, size, mtime)`); the runner only evaluates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveIdentityFactStatus {
    pub archive_id: i64,
    /// The catalogue display name, used only as the finding's affected
    /// resource so a person can tell which game each note is about.
    pub display_name: String,
    /// The archive's current platform assignment, as stored in the
    /// catalogue. `None` when no platform is assigned.
    pub platform_id: Option<String>,
    pub facts: Vec<(PersistedIdentityFact, IdentityFactFreshness)>,
}

impl ArchiveIdentityFactStatus {
    fn freshness_of(&self, kind: IdentityKind) -> Option<IdentityFactFreshness> {
        self.facts
            .iter()
            .find(|(fact, _)| fact.kind == kind)
            .map(|(_, freshness)| *freshness)
    }
}

/// What a platform's emulator launch needs from the identity cache.
struct LaunchIdentityRequirement {
    platform_label: &'static str,
    emulator: &'static str,
    /// The fact that gates launch for this platform.
    launch_fact: IdentityKind,
    launch_fact_label: &'static str,
    /// A second fact that does not gate launch but does gate patch/cheat
    /// compatibility (PS2's PCSX2 executable CRC). `None` for every other
    /// platform.
    patch_fact: Option<(IdentityKind, &'static str)>,
    /// An alternative to `launch_fact`: when set, the launch requirement is
    /// satisfied by *either* fact, and only the total absence of both is a
    /// missing-fact finding (Xbox 360's title ID / media ID).
    launch_fact_alternative: Option<(IdentityKind, &'static str)>,
}

fn requirement_for(platform: IdentityPlatform) -> Option<LaunchIdentityRequirement> {
    let requirement = match platform {
        IdentityPlatform::PlayStation => LaunchIdentityRequirement {
            platform_label: "PlayStation",
            emulator: "DuckStation",
            launch_fact: IdentityKind::Ps1Serial,
            launch_fact_label: "PlayStation serial",
            patch_fact: None,
            launch_fact_alternative: None,
        },
        IdentityPlatform::PlayStation2 => LaunchIdentityRequirement {
            platform_label: "PlayStation 2",
            emulator: "PCSX2",
            launch_fact: IdentityKind::Ps2Serial,
            launch_fact_label: "PlayStation 2 serial",
            patch_fact: Some((IdentityKind::Pcsx2ExecutableCrc, "PCSX2 executable CRC")),
            launch_fact_alternative: None,
        },
        IdentityPlatform::Psp => LaunchIdentityRequirement {
            platform_label: "PSP",
            emulator: "PPSSPP",
            launch_fact: IdentityKind::PspDiscId,
            launch_fact_label: "PSP disc ID",
            patch_fact: None,
            launch_fact_alternative: None,
        },
        IdentityPlatform::PlayStation3 => LaunchIdentityRequirement {
            platform_label: "PlayStation 3",
            emulator: "RPCS3",
            launch_fact: IdentityKind::Ps3TitleId,
            launch_fact_label: "PlayStation 3 title ID",
            patch_fact: None,
            launch_fact_alternative: None,
        },
        IdentityPlatform::Dreamcast => LaunchIdentityRequirement {
            platform_label: "Dreamcast",
            emulator: "Flycast",
            launch_fact: IdentityKind::DreamcastProductCode,
            launch_fact_label: "Dreamcast product code",
            patch_fact: None,
            launch_fact_alternative: None,
        },
        IdentityPlatform::GameCube => LaunchIdentityRequirement {
            platform_label: "GameCube",
            emulator: "Dolphin",
            launch_fact: IdentityKind::DolphinGameId,
            launch_fact_label: "Dolphin Game ID",
            patch_fact: None,
            launch_fact_alternative: None,
        },
        IdentityPlatform::Wii => LaunchIdentityRequirement {
            platform_label: "Wii",
            emulator: "Dolphin",
            launch_fact: IdentityKind::DolphinGameId,
            launch_fact_label: "Dolphin Game ID",
            patch_fact: None,
            launch_fact_alternative: None,
        },
        IdentityPlatform::Xbox => LaunchIdentityRequirement {
            platform_label: "Xbox",
            emulator: "xemu",
            launch_fact: IdentityKind::XbeTitleId,
            launch_fact_label: "Xbox title ID",
            patch_fact: None,
            launch_fact_alternative: None,
        },
        IdentityPlatform::Xbox360 => LaunchIdentityRequirement {
            platform_label: "Xbox 360",
            emulator: "Xenia",
            launch_fact: IdentityKind::XexTitleId,
            launch_fact_label: "Xbox 360 title ID",
            patch_fact: None,
            launch_fact_alternative: Some((IdentityKind::XexMediaId, "Xbox 360 media ID")),
        },
        // Every other platform either resolves its identity a different
        // way (hash/DAT-driven) or does not gate launch on a per-game
        // fact. Reporting a "missing" fact there would be noise.
        IdentityPlatform::Saturn
        | IdentityPlatform::SegaCd
        | IdentityPlatform::MegaDrive
        | IdentityPlatform::Snes
        | IdentityPlatform::Nes
        | IdentityPlatform::GameBoy
        | IdentityPlatform::GameBoyColor
        | IdentityPlatform::GameBoyAdvance
        | IdentityPlatform::N64
        | IdentityPlatform::ScummVM
        | IdentityPlatform::ThreeDo
        | IdentityPlatform::Pcfx
        | IdentityPlatform::PcEngineCd
        | IdentityPlatform::NeoGeoCd
        | IdentityPlatform::Atari2600
        | IdentityPlatform::Atari5200
        | IdentityPlatform::Atari7800
        | IdentityPlatform::Atari8Bit
        | IdentityPlatform::AtariLynx
        | IdentityPlatform::AtariJaguar
        | IdentityPlatform::AtariST
        | IdentityPlatform::WiiU
        | IdentityPlatform::ThreeDS
        | IdentityPlatform::Switch
        | IdentityPlatform::Other => return None,
    };
    Some(requirement)
}

/// Read-only Doctor findings for the persisted verified-identity fact
/// cache. One `Info` finding per game whose launch-gating (or, for PS2,
/// patch/cheat) identity fact is missing, stale, or of unknown freshness;
/// nothing for a game whose required facts are all current, and nothing for
/// a platform that is unresolved or does not need such a fact.
pub fn findings_from_verified_identity_facts(
    statuses: &[ArchiveIdentityFactStatus],
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for status in statuses {
        let platform = IdentityPlatform::from_catalogue(status.platform_id.as_deref());
        let Some(requirement) = requirement_for(platform) else {
            continue;
        };
        let affected = EncodedPath::from_path(std::path::Path::new(&status.display_name));

        match requirement.launch_fact_alternative {
            // Xbox 360: the launch requirement is met by either fact. Only
            // the total absence of both is a missing-fact finding; if one
            // is present, we still note it when it is stale / unknown.
            Some((alt_kind, alt_label)) => {
                let primary = status.freshness_of(requirement.launch_fact);
                let alternate = status.freshness_of(alt_kind);
                if primary.is_none() && alternate.is_none() {
                    findings.push(missing_finding(
                        &affected,
                        &requirement,
                        &format!("{} or {}", requirement.launch_fact_label, alt_label),
                        LaunchGating::Launch,
                    ));
                } else {
                    if let Some(freshness) = primary {
                        push_non_current(
                            &mut findings,
                            &affected,
                            &requirement,
                            requirement.launch_fact_label,
                            freshness,
                        );
                    }
                    if let Some(freshness) = alternate {
                        push_non_current(
                            &mut findings,
                            &affected,
                            &requirement,
                            alt_label,
                            freshness,
                        );
                    }
                }
            }
            None => {
                emit_for_fact(
                    &mut findings,
                    status,
                    &affected,
                    &requirement,
                    requirement.launch_fact,
                    requirement.launch_fact_label,
                    LaunchGating::Launch,
                );
            }
        }

        if let Some((patch_kind, patch_label)) = requirement.patch_fact {
            emit_for_fact(
                &mut findings,
                status,
                &affected,
                &requirement,
                patch_kind,
                patch_label,
                LaunchGating::PatchCompatibility,
            );
        }
    }

    findings
}

#[derive(Clone, Copy)]
enum LaunchGating {
    /// The fact gates whether the emulator will launch the game at all.
    Launch,
    /// The fact gates patch / cheat compatibility only (PS2's PCSX2
    /// executable CRC) - launch itself does not depend on it.
    PatchCompatibility,
}

fn emit_for_fact(
    findings: &mut Vec<Finding>,
    status: &ArchiveIdentityFactStatus,
    affected: &EncodedPath,
    requirement: &LaunchIdentityRequirement,
    kind: IdentityKind,
    label: &str,
    gating: LaunchGating,
) {
    match status.freshness_of(kind) {
        None => findings.push(missing_finding(affected, requirement, label, gating)),
        Some(IdentityFactFreshness::Current) => {}
        Some(freshness) => push_non_current(findings, affected, requirement, label, freshness),
    }
}

fn push_non_current(
    findings: &mut Vec<Finding>,
    affected: &EncodedPath,
    requirement: &LaunchIdentityRequirement,
    label: &str,
    freshness: IdentityFactFreshness,
) {
    match freshness {
        IdentityFactFreshness::Current => {}
        IdentityFactFreshness::Stale => findings.push(stale_finding(affected, requirement, label)),
        IdentityFactFreshness::Unknown => {
            findings.push(unknown_finding(affected, requirement, label))
        }
    }
}

fn base(id: &str, requirement: &LaunchIdentityRequirement, title: String, body: String) -> Finding {
    Finding::new(
        id,
        DoctorCategory::Emulators,
        DoctorSubsystem::EmulatorReadiness,
        DoctorSeverity::Info,
        title,
        body,
    )
    .with_evidence([format!(
        "Platform resolved to {}; {} launch reads this identity from a fresh inspection, not \
         from the catalogue cache.",
        requirement.platform_label, requirement.emulator
    )])
}

fn missing_finding(
    affected: &EncodedPath,
    requirement: &LaunchIdentityRequirement,
    label: &str,
    gating: LaunchGating,
) -> Finding {
    let (body, guidance) = match gating {
        LaunchGating::Launch => (
            format!(
                "No verified {label} is available. {} launch will remain blocked for this game \
                 until an identity inspection verifies one.",
                requirement.emulator
            ),
            format!(
                "Run an identity inspection for this game so a verified {label} can be cached \
                 and {} launch can proceed.",
                requirement.emulator
            ),
        ),
        LaunchGating::PatchCompatibility => (
            format!(
                "No verified {label} is available. {} patch and cheat compatibility cannot be \
                 confirmed for this game until an identity inspection verifies one; launch \
                 itself does not depend on it.",
                requirement.emulator
            ),
            format!(
                "Run an identity inspection for this game if you need {} patch or cheat \
                 compatibility confirmed.",
                requirement.emulator
            ),
        ),
    };
    base(
        "emulators.verified_identity_missing",
        requirement,
        format!("{}: no verified {label}", requirement.platform_label),
        body,
    )
    .with_affected(affected.clone())
    .with_guidance(
        "Read-only consumers (Library, Doctor) cannot explain this game's identity or launch \
         readiness without the verified fact.",
        guidance,
    )
}

fn stale_finding(
    affected: &EncodedPath,
    requirement: &LaunchIdentityRequirement,
    label: &str,
) -> Finding {
    base(
        "emulators.verified_identity_stale",
        requirement,
        format!(
            "{}: cached {label} is out of date",
            requirement.platform_label
        ),
        format!(
            "A verified {label} was cached earlier, but this game's file has changed since. The \
             cached value is kept so the earlier identity can still be explained; {} will \
             re-verify from the current content before launch.",
            requirement.emulator
        ),
    )
    .with_affected(affected.clone())
    .with_guidance(
        "A stale cached fact stays visible for explanation but never authorizes a launch.",
        format!("Re-run an identity inspection so the cached {label} matches the current file.",),
    )
}

fn unknown_finding(
    affected: &EncodedPath,
    requirement: &LaunchIdentityRequirement,
    label: &str,
) -> Finding {
    base(
        "emulators.verified_identity_unknown_freshness",
        requirement,
        format!(
            "{}: cached {label} freshness unknown",
            requirement.platform_label
        ),
        format!(
            "A verified {label} is cached, but it could not be compared against this game's \
             current file, so whether it still describes the same content is unknown. {} \
             re-verifies from the current content before launch regardless.",
            requirement.emulator
        ),
    )
    .with_affected(affected.clone())
    .with_guidance(
        "Unknown freshness is treated as not-current: the cached fact explains history but does \
         not authorize a launch.",
        format!(
            "Re-run an identity inspection to refresh the cached {label} and its file snapshot.",
        ),
    )
}

#[cfg(test)]
mod tests;
