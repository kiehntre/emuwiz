//! Controlled "identity unverified" launch for games whose system is known
//! but whose exact game is not.
//!
//! Owner decisions: ZX Spectrum tapes (2026-11-07); generalised 2026-11-09
//! with this policy - exact game identity (DAT, title, serial) is not
//! mandatory, but a system inferred only from a folder never silently becomes
//! trusted platform identity:
//!
//! 1. The file's own header, signature or a format only this system uses
//!    establishes the system ([`SystemBasis::Media`]): launch with a visible
//!    "game not identified" warning.
//! 2. The person chose the system ([`SystemBasis::Chosen`]): the same.
//! 3. Only the folder (or another automatic hint) says so and the format is
//!    shared: [`Outcome::NeedsConfirmation`]. The person may either press
//!    "Try anyway" - a one-launch, in-memory trial ([`SystemBasis::Trial`])
//!    that saves nothing, never becomes a manual choice and never outranks
//!    later content identification - or change the system permanently through
//!    the existing Change system flow (owner decision 2026-11-09).
//! 4. The file's own content names another system: [`Outcome::Contradicted`],
//!    refused with the conflict explained.
//!
//! Nothing here is ever a *verified* identity: the key is prefixed
//! `unverified:`, every candidate carries the warning, and media, archive,
//! firmware, executable and click-time checks are unchanged. Systems and
//! formats without a launch route stay unsupported (the format must be one
//! the system lists; the planner still needs a real installed route).

use std::path::Path;

use archivefs_core::game_identity::IdentityPlatform;
use archivefs_core::launch::unverified_media::MediaSystemEvidence;
use archivefs_core::launch::{
    LaunchPlan, LaunchReadiness, LaunchWarning, LaunchWarningKind, ResolvedIdentity,
};

/// Why the system of an unidentified game is trusted for launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemBasis {
    /// The file's own header, structure or system-specific format.
    Media,
    /// The person's own (saved) system choice.
    Chosen,
    /// A one-launch "Try anyway" for a system the person picked on the
    /// Launch page. Held in memory only; nothing is saved.
    Trial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Allowed(ResolvedIdentity, SystemBasis),
    /// Folder-only system, compatible but shared format (canonical id).
    NeedsConfirmation(&'static str),
    /// The file's own content names another system (canonical ids).
    Contradicted {
        recorded: &'static str,
        content: &'static str,
    },
    /// Not a case this lane handles; the existing review block applies.
    NotApplicable,
}

/// Platform sources that come from the file's own bytes or an established
/// catalogue/structure match, never from a folder name.
fn source_is_media_evidence(source: &str) -> bool {
    matches!(
        source,
        "header_identity"
            | "archive_member_structure"
            | archivefs_core::ROMM_PLATFORM_SOURCE
            | archivefs_core::VERIFIED_DAT_PLATFORM_SOURCE
            | archivefs_core::DAT_ROMM_AGREEMENT_SOURCE
    )
}

/// The warning every candidate of an unverified launch carries.
pub(crate) fn warning_text(platform_id: &str, basis: SystemBasis) -> String {
    let name = archivefs_core::platform::platform_by_id(platform_id)
        .map_or(platform_id, |p| p.display_name);
    match basis {
        SystemBasis::Media => format!(
            "Game not identified: the file itself shows it is a {name} game EmuWiz can start, but EmuWiz could not confirm which game it is. It will still play; catalogue (DAT) verification is optional."
        ),
        SystemBasis::Chosen => format!(
            "Game not identified: you chose {name} for this file, and EmuWiz could not confirm which game it is. It will still play; your choice is not catalogue (DAT) verification."
        ),
        SystemBasis::Trial => format!(
            "System not confirmed: this is a one-time try as {name}. Neither the system nor the game was confirmed from the file, and nothing is saved; the game may not start correctly."
        ),
    }
}

/// Applies the owner's policy to one file whose system is recorded as
/// `platform_hint` (from `platform_source`, when known). `content_platform`
/// is what the evidence worker read from the file's own content, if anything.
///
/// `trial` is a one-launch "Try anyway" system for exactly this file. It
/// replaces the recorded system for this plan only and passes the same
/// format, content and media checks; anything the file says about itself
/// still wins over it.
pub(crate) fn resolve(
    path: &Path,
    platform_hint: &str,
    platform_source: Option<&str>,
    content_platform: Option<IdentityPlatform>,
    trial: Option<&'static str>,
) -> Outcome {
    let Some(platform) = trial.or_else(|| archivefs_core::canonical_platform_for_alias(platform_hint))
    else {
        return Outcome::NotApplicable;
    };
    if let Some(content) = content_platform
        && content != IdentityPlatform::Other
    {
        match archivefs_core::launch::evidence_bridge::launch_platform_id(content) {
            Some(other) if other != platform => {
                return Outcome::Contradicted {
                    recorded: platform,
                    content: other,
                };
            }
            Some(_) => {}
            None => return Outcome::NotApplicable,
        }
    }
    // A loose cartridge format the system's own identity check reads is
    // decided by that check alone: valid bytes resolve there, invalid bytes
    // (a failed header or logo) stay blocked as a media-integrity failure.
    let identity_platform = IdentityPlatform::from_catalogue(Some(platform));
    if identity_platform != IdentityPlatform::Other
        && archivefs_core::game_identity::supported_loose_rom_format(path, identity_platform)
            .is_some()
    {
        return Outcome::NotApplicable;
    }
    // The format must be one this system lists; otherwise nothing is offered.
    let Some(identity) =
        archivefs_core::launch::unverified_media::platform_media_identity(path, platform)
    else {
        return Outcome::NotApplicable;
    };
    let media = archivefs_core::launch::unverified_media::media_system_evidence_cached(path, platform);
    if let MediaSystemEvidence::Contradicts(other) = media {
        return Outcome::Contradicted {
            recorded: platform,
            content: other,
        };
    }
    // A ZX tape that passed the strict structural validator is media evidence.
    let zx_tape = platform == archivefs_core::launch::unverified_media::ZX_PLATFORM
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("tap") || e.eq_ignore_ascii_case("tzx"));
    if trial.is_some() {
        return Outcome::Allowed(identity, SystemBasis::Trial);
    }
    if platform_source == Some(archivefs_core::MANUAL_PLATFORM_SOURCE) {
        return Outcome::Allowed(identity, SystemBasis::Chosen);
    }
    if zx_tape
        || media == MediaSystemEvidence::Agrees
        || platform_source.is_some_and(source_is_media_evidence)
    {
        return Outcome::Allowed(identity, SystemBasis::Media);
    }
    Outcome::NeedsConfirmation(platform)
}

/// Systems a one-time "Try anyway" may use for this file: those that list
/// its format and that the file's own bytes do not rule out, recorded system
/// first. Display order otherwise follows the registry.
pub(crate) fn trial_systems(path: &Path, recorded: Option<&str>) -> Vec<&'static str> {
    let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
        return Vec::new();
    };
    let recorded = recorded.and_then(archivefs_core::canonical_platform_for_alias);
    let mut systems: Vec<&'static str> = archivefs_core::platform::PLATFORMS
        .iter()
        .map(|p| p.id)
        .filter(|id| archivefs_core::launch::unverified_media::platform_lists_extension(id, extension))
        .filter(|id| {
            !matches!(
                archivefs_core::launch::unverified_media::media_system_evidence_cached(path, id),
                MediaSystemEvidence::Contradicts(_)
            )
        })
        .collect();
    if let Some(recorded) = recorded
        && let Some(index) = systems.iter().position(|id| *id == recorded)
    {
        let first = systems.remove(index);
        systems.insert(0, first);
    }
    systems
}

/// Adds the visible warning to every usable candidate (never removes a
/// blocker, never touches a blocked candidate) and refreshes the summary.
pub(crate) fn apply_warning(plan: &mut LaunchPlan, basis: SystemBasis) {
    for candidate in &mut plan.candidates {
        // Only the RetroArch route was approved; standalone routes keep their
        // own (stricter) rules and never receive the warning.
        if candidate.readiness == LaunchReadiness::Blocked
            || !matches!(
                candidate.target,
                archivefs_core::launch::LaunchTarget::RetroArchCore { .. }
            )
            || candidate
                .warnings
                .iter()
                .any(|w| w.kind == LaunchWarningKind::IdentityUnverified)
        {
            continue;
        }
        let text = warning_text(plan.platform_id.as_deref().unwrap_or_default(), basis);
        candidate.warnings.push(LaunchWarning::new(
            LaunchWarningKind::IdentityUnverified,
            text,
        ));
        if candidate.readiness == LaunchReadiness::Ready {
            candidate.readiness = LaunchReadiness::ReadyWithWarnings;
        }
    }
    let count = |r: LaunchReadiness| plan.candidates.iter().filter(|c| c.readiness == r).count();
    plan.summary.ready = count(LaunchReadiness::Ready);
    plan.summary.ready_with_warnings = count(LaunchReadiness::ReadyWithWarnings);
    plan.summary.blocked = count(LaunchReadiness::Blocked);
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOLDER: Option<&str> = Some("folder_alias");
    const MANUAL: Option<&str> = Some(archivefs_core::MANUAL_PLATFORM_SOURCE);

    fn allowed(outcome: &Outcome) -> Option<SystemBasis> {
        match outcome {
            Outcome::Allowed(identity, basis) => {
                assert!(archivefs_core::launch::unverified_media::is_unverified_key(
                    &identity.game_key
                ));
                Some(*basis)
            }
            _ => None,
        }
    }

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn ines(dir: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
        let mut bytes = b"NES\x1a".to_vec();
        bytes.resize(16 + 16 * 1024, 0);
        write(dir, name, &bytes)
    }

    #[test]
    fn zx_tapes_keep_the_2026_11_07_lane_whatever_recorded_the_system() {
        let dir = tempfile::tempdir().unwrap();
        let tap = write(&dir, "Jetpac.tap", &[2u8, 0, 0, 1]);
        assert_eq!(allowed(&resolve(&tap, "ZX Spectrum", FOLDER, None, None)), Some(SystemBasis::Media));
        let tzx = write(&dir, "Jetpac.tzx", b"ZXTape!\x1a\x01\x14\x20\x00\x00");
        assert_eq!(allowed(&resolve(&tzx, "ZX Spectrum", None, None, None)), Some(SystemBasis::Media));
        assert_eq!(resolve(&tzx, "Commodore 64", FOLDER, None, None), Outcome::NotApplicable);
    }

    #[test]
    fn a_system_known_only_from_the_folder_needs_confirmation_for_a_shared_format() {
        let dir = tempfile::tempdir().unwrap();
        // Confirmed platform (folder), unknown game, shared disc format.
        let chd = write(&dir, "The 7th Guest (Europe).chd", &[0u8; 4096]);
        assert_eq!(
            resolve(&chd, "Philips CD-i", FOLDER, None, None),
            Outcome::NeedsConfirmation("Philips CD-i")
        );
        // Ambiguous ZIP in a cartridge folder.
        let zip = write(&dir, "Game (USA).zip", b"PK\x03\x04rest-of-archive");
        assert_eq!(resolve(&zip, "SNES", FOLDER, None, None), Outcome::NeedsConfirmation("SNES"));
        // An unrecorded source is no better than a folder.
        assert_eq!(resolve(&chd, "Dreamcast", None, None, None), Outcome::NeedsConfirmation("Dreamcast"));
    }

    #[test]
    fn the_persons_own_system_choice_allows_the_same_file_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let chd = write(&dir, "Jet Set Radio (Europe).chd", &[0u8; 4096]);
        assert_eq!(allowed(&resolve(&chd, "Dreamcast", MANUAL, None, None)), Some(SystemBasis::Chosen));
        let zip = write(&dir, "Game (USA).zip", b"PK\x03\x04rest-of-archive");
        assert_eq!(allowed(&resolve(&zip, "SNES", MANUAL, None, None)), Some(SystemBasis::Chosen));
        assert!(warning_text("Dreamcast", SystemBasis::Chosen).contains("you chose Sega Dreamcast")
            || warning_text("Dreamcast", SystemBasis::Chosen).contains("you chose"));
    }

    #[test]
    fn the_files_own_format_or_header_establishes_the_system_without_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        // A Dreamcast-only disc format in a Dreamcast folder.
        let cdi = write(&dir, "Headhunter (Europe).cdi", &[0u8; 4096]);
        assert_eq!(allowed(&resolve(&cdi, "Dreamcast", FOLDER, None, None)), Some(SystemBasis::Media));
        // A header-detected platform recorded by the scan.
        let chd = write(&dir, "Disc.chd", &[0u8; 4096]);
        assert_eq!(
            allowed(&resolve(&chd, "Dreamcast", Some("header_identity"), None, None)),
            Some(SystemBasis::Media)
        );
    }

    #[test]
    fn loose_cartridges_stay_with_their_own_identity_check() {
        let dir = tempfile::tempdir().unwrap();
        // The cartridge check decides these (valid -> its own identity,
        // invalid header -> blocked); this lane never second-guesses it,
        // even for a saved choice or a trial.
        let gb = write(&dir, "Broken.gb", &[0x55; 0x8000]);
        assert_eq!(resolve(&gb, "Game Boy", MANUAL, None, None), Outcome::NotApplicable);
        assert_eq!(resolve(&gb, "Game Boy", FOLDER, None, Some("Game Boy")), Outcome::NotApplicable);
        let nes = ines(&dir, "Game (USA).nes");
        assert_eq!(resolve(&nes, "NES", FOLDER, None, None), Outcome::NotApplicable);
    }

    #[test]
    fn nes_games_misfiled_under_snes_are_refused_or_never_offered() {
        let dir = tempfile::tempdir().unwrap();
        // A .nes file in the SNES folder: not a format SNES lists.
        let nes = ines(&dir, "Misfiled (USA).nes");
        assert_eq!(resolve(&nes, "SNES", FOLDER, None, None), Outcome::NotApplicable);
        // An NES game zipped/renamed under a shared SNES format: its own
        // header contradicts the folder, even when the person chose SNES.
        let renamed = ines(&dir, "Misfiled (USA).bin");
        let conflict = Outcome::Contradicted { recorded: "SNES", content: "NES" };
        assert_eq!(resolve(&renamed, "SNES", FOLDER, None, None), conflict);
        assert_eq!(resolve(&renamed, "SNES", MANUAL, None, None), conflict);
    }

    #[test]
    fn content_pointing_to_another_system_is_never_overridden() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "Disc (Europe).cdi", &[1u8; 64]);
        // .cdi is Dreamcast-only, so the format itself establishes the system.
        assert!(allowed(&resolve(&path, "Dreamcast", FOLDER, None, None)).is_some());
        assert!(allowed(&resolve(&path, "Dreamcast", FOLDER, Some(IdentityPlatform::Other), None)).is_some());
        assert!(matches!(
            resolve(&path, "Dreamcast", MANUAL, Some(IdentityPlatform::PlayStation2), None),
            Outcome::Contradicted { recorded: "Dreamcast", .. }
        ));
        assert_eq!(
            resolve(&path, "a folder name that is no system", MANUAL, None, None),
            Outcome::NotApplicable
        );
    }

    #[test]
    fn unsupported_formats_stay_unsupported_even_when_chosen() {
        let dir = tempfile::tempdir().unwrap();
        // The Acorn systems do not list .zip: nothing is manufactured.
        let zip = write(&dir, "Acheton.zip", b"PK\x03\x04rest-of-archive");
        assert_eq!(resolve(&zip, "Acorn Archimedes", MANUAL, None, None), Outcome::NotApplicable);
    }

    #[test]
    fn try_anyway_is_one_plan_only_and_never_beats_the_files_own_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let chd = write(&dir, "Avenger (Japan).chd", &[0u8; 4096]);
        // Folder-only: blocked until the person tries it.
        assert_eq!(
            resolve(&chd, "PC Engine CD", FOLDER, None, None),
            Outcome::NeedsConfirmation("PC Engine CD")
        );
        // A trial of the recorded system plans with the trial warning.
        assert_eq!(
            allowed(&resolve(&chd, "PC Engine CD", FOLDER, None, Some("PC Engine CD"))),
            Some(SystemBasis::Trial)
        );
        // A temporary *different* system that lists the format is allowed for
        // that one plan; its identity names the trial system, not the folder.
        match resolve(&chd, "PC Engine CD", FOLDER, None, Some("Dreamcast")) {
            Outcome::Allowed(identity, SystemBasis::Trial) => {
                assert_eq!(identity.platform_id, "Dreamcast")
            }
            other => panic!("{other:?}"),
        }
        // Without the trial (next plan, later rescan) nothing was remembered.
        assert_eq!(
            resolve(&chd, "PC Engine CD", FOLDER, None, None),
            Outcome::NeedsConfirmation("PC Engine CD")
        );
        // A format the trial system does not list is never manufactured.
        assert_eq!(
            resolve(&chd, "PC Engine CD", FOLDER, None, Some("SNES")),
            Outcome::NotApplicable
        );
        // Content identification after a rescan outranks a trial.
        assert!(matches!(
            resolve(&chd, "PC Engine CD", FOLDER, Some(IdentityPlatform::PlayStation2), Some("PC Engine CD")),
            Outcome::Contradicted { recorded: "PC Engine CD", .. }
        ));
        assert!(warning_text("PC Engine CD", SystemBasis::Trial).contains("nothing is saved"));
    }

    #[test]
    fn try_anyway_cannot_launch_an_nes_game_as_snes() {
        let dir = tempfile::tempdir().unwrap();
        let renamed = ines(&dir, "Misfiled (USA).bin");
        assert_eq!(
            resolve(&renamed, "SNES", FOLDER, None, Some("SNES")),
            Outcome::Contradicted { recorded: "SNES", content: "NES" }
        );
        // The trial list never offers a system the header rules out.
        assert!(!trial_systems(&renamed, Some("SNES")).contains(&"SNES"));
        let zip = write(&dir, "Game (USA).zip", b"PK\x03\x04rest-of-archive");
        let systems = trial_systems(&zip, Some("SNES"));
        assert_eq!(systems.first(), Some(&"SNES"));
        assert!(systems.contains(&"NES"), "an ambiguous ZIP may be tried as another system: {systems:?}");
    }
}
