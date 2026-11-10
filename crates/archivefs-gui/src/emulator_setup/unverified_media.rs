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
    /// The person ticked "Remember this launch choice" for this unchanged
    /// file (owner decision 2026-11-10). Launch-only; never identity.
    Remembered,
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
        SystemBasis::Remembered => format!(
            "System not confirmed: you chose to remember {name} as the launch choice for this unchanged file. Neither the system nor the game was confirmed from the file; your choice is not identification, and it stops applying if the file changes."
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
///
/// `remembered` is the person's saved "Remember this launch choice" for this
/// unchanged file. It is consulted only where the recorded system alone would
/// need confirmation (a folder-only guess): a manual system choice, the
/// file's own evidence and a one-time try all come first, and it passes the
/// same format, content and media checks as a try.
pub(crate) fn resolve(
    path: &Path,
    platform_hint: &str,
    platform_source: Option<&str>,
    content_platform: Option<IdentityPlatform>,
    trial: Option<&'static str>,
    remembered: Option<&'static str>,
) -> Outcome {
    let outcome = resolve_for(
        path,
        platform_hint,
        platform_source,
        content_platform,
        trial,
    );
    // No system recorded at all (owner decision 2026-11-10): a remembered
    // choice answers that exactly as it answers a folder guess.
    let system_unknown =
        trial.is_none() && archivefs_core::canonical_platform_for_alias(platform_hint).is_none();
    let remembered_applies = |outcome: &Outcome| match outcome {
        Outcome::NeedsConfirmation(_) => true,
        Outcome::NotApplicable => system_unknown,
        _ => false,
    };
    match (outcome, remembered) {
        (outcome, Some(system)) if remembered_applies(&outcome) => {
            match resolve_for(path, system, None, content_platform, Some(system)) {
                Outcome::Allowed(identity, _) => {
                    Outcome::Allowed(identity, SystemBasis::Remembered)
                }
                other => other,
            }
        }
        (outcome, _) => outcome,
    }
}

fn resolve_for(
    path: &Path,
    platform_hint: &str,
    platform_source: Option<&str>,
    content_platform: Option<IdentityPlatform>,
    trial: Option<&'static str>,
) -> Outcome {
    let Some(platform) =
        trial.or_else(|| archivefs_core::canonical_platform_for_alias(platform_hint))
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
            // No launch identity kind exists for this system (C64, VIC-20,
            // ...), but a report that names the very system recorded - for
            // example the library's own platform context - is no
            // contradiction; the file's bytes are still checked below.
            None if content == IdentityPlatform::from_catalogue(Some(platform)) => {}
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
    // Content naming another system is refused first, even for a format
    // this system does not list (a C64 cartridge filed under VIC-20): the
    // conflict is explained instead of a generic review. Refusing is never
    // a launch permission.
    let media =
        archivefs_core::launch::unverified_media::media_system_evidence_cached(path, platform);
    if let MediaSystemEvidence::Contradicts(other) = media {
        return Outcome::Contradicted {
            recorded: platform,
            content: other,
        };
    }
    // The format must be one this system lists; otherwise nothing is offered.
    let Some(identity) =
        archivefs_core::launch::unverified_media::platform_media_identity(path, platform)
    else {
        return Outcome::NotApplicable;
    };
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

/// Systems a one-time "Try anyway" (or a remembered launch choice) may use
/// for this file, recorded system first:
///
/// - the system lists the file's format, a try as it would pass every check
///   of this lane (loose-ROM header rules, content naming another system),
///   and `has_route` says an installed launch route exists for it;
/// - when the file's own bytes or a format only some systems use point at
///   particular systems ([`MediaSystemEvidence::Agrees`]), only those.
///
/// Display order otherwise follows the registry.
pub(crate) fn trial_systems(
    path: &Path,
    recorded: Option<&str>,
    has_route: &dyn Fn(&str) -> bool,
) -> Vec<&'static str> {
    let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
        return Vec::new();
    };
    let recorded = recorded.and_then(archivefs_core::canonical_platform_for_alias);
    let mut systems: Vec<(&'static str, bool)> = archivefs_core::platform::PLATFORMS
        .iter()
        .map(|p| p.id)
        .filter(|id| {
            archivefs_core::launch::unverified_media::platform_lists_extension(id, extension)
        })
        .filter(|id| has_route(id))
        .filter(|id| {
            matches!(
                resolve_for(path, id, None, None, Some(id)),
                Outcome::Allowed(..)
            )
        })
        .map(|id| {
            let agrees =
                archivefs_core::launch::unverified_media::media_system_evidence_cached(path, id)
                    == MediaSystemEvidence::Agrees;
            (id, agrees)
        })
        .collect();
    if systems.iter().any(|(_, agrees)| *agrees) {
        systems.retain(|(_, agrees)| *agrees);
    }
    let mut systems: Vec<&'static str> = systems.into_iter().map(|(id, _)| id).collect();
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
    /// The five-input form, with nothing remembered.
    fn resolve5(
        path: &Path,
        hint: &str,
        source: Option<&str>,
        content: Option<IdentityPlatform>,
        trial: Option<&'static str>,
    ) -> Outcome {
        resolve(path, hint, source, content, trial, None)
    }

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
        assert_eq!(
            allowed(&resolve5(&tap, "ZX Spectrum", FOLDER, None, None)),
            Some(SystemBasis::Media)
        );
        let tzx = write(&dir, "Jetpac.tzx", b"ZXTape!\x1a\x01\x14\x20\x00\x00");
        assert_eq!(
            allowed(&resolve5(&tzx, "ZX Spectrum", None, None, None)),
            Some(SystemBasis::Media)
        );
        assert_eq!(
            resolve5(&tzx, "Commodore 64", FOLDER, None, None),
            Outcome::NotApplicable
        );
    }

    #[test]
    fn a_system_known_only_from_the_folder_needs_confirmation_for_a_shared_format() {
        let dir = tempfile::tempdir().unwrap();
        // Confirmed platform (folder), unknown game, shared disc format.
        let chd = write(&dir, "The 7th Guest (Europe).chd", &[0u8; 4096]);
        assert_eq!(
            resolve5(&chd, "Philips CD-i", FOLDER, None, None),
            Outcome::NeedsConfirmation("Philips CD-i")
        );
        // Ambiguous ZIP in a cartridge folder.
        let zip = write(&dir, "Game (USA).zip", b"PK\x03\x04rest-of-archive");
        assert_eq!(
            resolve5(&zip, "SNES", FOLDER, None, None),
            Outcome::NeedsConfirmation("SNES")
        );
        // An unrecorded source is no better than a folder.
        assert_eq!(
            resolve5(&chd, "Dreamcast", None, None, None),
            Outcome::NeedsConfirmation("Dreamcast")
        );
    }

    #[test]
    fn the_persons_own_system_choice_allows_the_same_file_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let chd = write(&dir, "Jet Set Radio (Europe).chd", &[0u8; 4096]);
        assert_eq!(
            allowed(&resolve5(&chd, "Dreamcast", MANUAL, None, None)),
            Some(SystemBasis::Chosen)
        );
        let zip = write(&dir, "Game (USA).zip", b"PK\x03\x04rest-of-archive");
        assert_eq!(
            allowed(&resolve5(&zip, "SNES", MANUAL, None, None)),
            Some(SystemBasis::Chosen)
        );
        assert!(
            warning_text("Dreamcast", SystemBasis::Chosen).contains("you chose Sega Dreamcast")
                || warning_text("Dreamcast", SystemBasis::Chosen).contains("you chose")
        );
    }

    #[test]
    fn the_files_own_format_or_header_establishes_the_system_without_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        // A Dreamcast-only disc format in a Dreamcast folder.
        let cdi = write(&dir, "Headhunter (Europe).cdi", &[0u8; 4096]);
        assert_eq!(
            allowed(&resolve5(&cdi, "Dreamcast", FOLDER, None, None)),
            Some(SystemBasis::Media)
        );
        // A header-detected platform recorded by the scan.
        let chd = write(&dir, "Disc.chd", &[0u8; 4096]);
        assert_eq!(
            allowed(&resolve5(
                &chd,
                "Dreamcast",
                Some("header_identity"),
                None,
                None
            )),
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
        assert_eq!(
            resolve5(&gb, "Game Boy", MANUAL, None, None),
            Outcome::NotApplicable
        );
        assert_eq!(
            resolve5(&gb, "Game Boy", FOLDER, None, Some("Game Boy")),
            Outcome::NotApplicable
        );
        let nes = ines(&dir, "Game (USA).nes");
        assert_eq!(
            resolve5(&nes, "NES", FOLDER, None, None),
            Outcome::NotApplicable
        );
    }

    #[test]
    fn nes_games_misfiled_under_snes_are_refused_or_never_offered() {
        let dir = tempfile::tempdir().unwrap();
        // A .nes file in the SNES folder: not a format SNES lists, and its
        // own header names NES, so the conflict is explained (2026-11-10:
        // content conflicts are reported before the format check).
        let nes = ines(&dir, "Misfiled (USA).nes");
        assert_eq!(
            resolve5(&nes, "SNES", FOLDER, None, None),
            Outcome::Contradicted {
                recorded: "SNES",
                content: "NES"
            }
        );
        // An NES game zipped/renamed under a shared SNES format: its own
        // header contradicts the folder, even when the person chose SNES.
        let renamed = ines(&dir, "Misfiled (USA).bin");
        let conflict = Outcome::Contradicted {
            recorded: "SNES",
            content: "NES",
        };
        assert_eq!(resolve5(&renamed, "SNES", FOLDER, None, None), conflict);
        assert_eq!(resolve5(&renamed, "SNES", MANUAL, None, None), conflict);
    }

    #[test]
    fn content_pointing_to_another_system_is_never_overridden() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "Disc (Europe).cdi", &[1u8; 64]);
        // .cdi is Dreamcast-only, so the format itself establishes the system.
        assert!(allowed(&resolve5(&path, "Dreamcast", FOLDER, None, None)).is_some());
        assert!(
            allowed(&resolve5(
                &path,
                "Dreamcast",
                FOLDER,
                Some(IdentityPlatform::Other),
                None
            ))
            .is_some()
        );
        assert!(matches!(
            resolve5(
                &path,
                "Dreamcast",
                MANUAL,
                Some(IdentityPlatform::PlayStation2),
                None
            ),
            Outcome::Contradicted {
                recorded: "Dreamcast",
                ..
            }
        ));
        assert_eq!(
            resolve5(&path, "a folder name that is no system", MANUAL, None, None),
            Outcome::NotApplicable
        );
    }

    #[test]
    fn unsupported_formats_stay_unsupported_even_when_chosen() {
        let dir = tempfile::tempdir().unwrap();
        // The Acorn systems do not list .zip: nothing is manufactured.
        let zip = write(&dir, "Acheton.zip", b"PK\x03\x04rest-of-archive");
        assert_eq!(
            resolve5(&zip, "Acorn Archimedes", MANUAL, None, None),
            Outcome::NotApplicable
        );
    }

    #[test]
    fn try_anyway_is_one_plan_only_and_never_beats_the_files_own_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let chd = write(&dir, "Avenger (Japan).chd", &[0u8; 4096]);
        // Folder-only: blocked until the person tries it.
        assert_eq!(
            resolve5(&chd, "PC Engine CD", FOLDER, None, None),
            Outcome::NeedsConfirmation("PC Engine CD")
        );
        // A trial of the recorded system plans with the trial warning.
        assert_eq!(
            allowed(&resolve5(
                &chd,
                "PC Engine CD",
                FOLDER,
                None,
                Some("PC Engine CD")
            )),
            Some(SystemBasis::Trial)
        );
        // A temporary *different* system that lists the format is allowed for
        // that one plan; its identity names the trial system, not the folder.
        match resolve5(&chd, "PC Engine CD", FOLDER, None, Some("Dreamcast")) {
            Outcome::Allowed(identity, SystemBasis::Trial) => {
                assert_eq!(identity.platform_id, "Dreamcast")
            }
            other => panic!("{other:?}"),
        }
        // Without the trial (next plan, later rescan) nothing was remembered.
        assert_eq!(
            resolve5(&chd, "PC Engine CD", FOLDER, None, None),
            Outcome::NeedsConfirmation("PC Engine CD")
        );
        // A format the trial system does not list is never manufactured.
        assert_eq!(
            resolve5(&chd, "PC Engine CD", FOLDER, None, Some("SNES")),
            Outcome::NotApplicable
        );
        // Content identification after a rescan outranks a trial.
        assert!(matches!(
            resolve5(
                &chd,
                "PC Engine CD",
                FOLDER,
                Some(IdentityPlatform::PlayStation2),
                Some("PC Engine CD")
            ),
            Outcome::Contradicted {
                recorded: "PC Engine CD",
                ..
            }
        ));
        assert!(warning_text("PC Engine CD", SystemBasis::Trial).contains("nothing is saved"));
    }

    #[test]
    fn try_anyway_cannot_launch_an_nes_game_as_snes() {
        let dir = tempfile::tempdir().unwrap();
        let renamed = ines(&dir, "Misfiled (USA).bin");
        assert_eq!(
            resolve5(&renamed, "SNES", FOLDER, None, Some("SNES")),
            Outcome::Contradicted {
                recorded: "SNES",
                content: "NES"
            }
        );
        // The trial list never offers a system the header rules out.
        assert!(!trial_systems(&renamed, Some("SNES"), &|_| true).contains(&"SNES"));
        let zip = write(&dir, "Game (USA).zip", b"PK\x03\x04rest-of-archive");
        let systems = trial_systems(&zip, Some("SNES"), &|_| true);
        assert_eq!(systems.first(), Some(&"SNES"));
        assert!(
            systems.contains(&"NES"),
            "an ambiguous ZIP may be tried as another system: {systems:?}"
        );
    }

    #[test]
    fn a_remembered_launch_choice_replaces_try_anyway_only_for_a_folder_guess() {
        let dir = tempfile::tempdir().unwrap();
        let chd = write(&dir, "Jet Set Radio.chd", &[0u8; 4096]);
        let remembered = Some("Dreamcast");
        assert_eq!(
            allowed(&resolve(&chd, "Dreamcast", FOLDER, None, None, remembered)),
            Some(SystemBasis::Remembered)
        );
        // It may name another system that lists the format; the identity
        // names that system, and it is still launch-only.
        match resolve(&chd, "PC Engine CD", FOLDER, None, None, remembered) {
            Outcome::Allowed(identity, SystemBasis::Remembered) => {
                assert_eq!(identity.platform_id, "Dreamcast");
                assert!(identity.game_key.starts_with("unverified:"));
            }
            other => panic!("{other:?}"),
        }
        // A manual (permanent) system choice and a one-time try come first.
        assert_eq!(
            allowed(&resolve(
                &chd,
                "PC Engine CD",
                MANUAL,
                None,
                None,
                remembered
            )),
            Some(SystemBasis::Chosen)
        );
        assert_eq!(
            allowed(&resolve(
                &chd,
                "PC Engine CD",
                FOLDER,
                None,
                Some("PC Engine CD"),
                remembered
            )),
            Some(SystemBasis::Trial)
        );
        // Content identification still wins over the remembered choice.
        assert!(matches!(
            resolve(
                &chd,
                "Dreamcast",
                FOLDER,
                Some(IdentityPlatform::PlayStation2),
                None,
                remembered
            ),
            Outcome::Contradicted { .. }
        ));
        // A remembered system that does not list the format offers nothing.
        assert_eq!(
            resolve(&chd, "Dreamcast", FOLDER, None, None, Some("SNES")),
            Outcome::NotApplicable
        );
        assert!(warning_text("Dreamcast", SystemBasis::Remembered).contains("not identification"));
    }

    #[test]
    fn a_remembered_choice_never_overrides_a_header_naming_another_system() {
        let dir = tempfile::tempdir().unwrap();
        let renamed = ines(&dir, "Misfiled (USA).bin");
        assert_eq!(
            resolve(&renamed, "SNES", FOLDER, None, None, Some("SNES")),
            Outcome::Contradicted {
                recorded: "SNES",
                content: "NES"
            }
        );
    }

    fn crt(dir: &tempfile::TempDir, name: &str, signature: &[u8; 16]) -> std::path::PathBuf {
        let mut bytes = signature.to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0x40, 1, 0, 0, 0, 0, 1]);
        bytes.resize(0x40 + 0x10 + 0x2000, 0);
        write(dir, name, &bytes)
    }

    /// Census 2026-11-10: every C64 and VIC-20 cartridge stopped at "identity
    /// needs review" because these systems have no launch identity kind and
    /// the library's own platform context was read as a contradiction.
    #[test]
    fn commodore_cartridges_follow_their_own_crt_signature() {
        let dir = tempfile::tempdir().unwrap();
        let c64 = crt(&dir, "1942.crt", b"C64 CARTRIDGE   ");
        let vic = crt(&dir, "Omega Race.crt", b"VIC20 CARTRIDGE ");
        assert_eq!(
            allowed(&resolve5(
                &c64,
                "Commodore 64",
                FOLDER,
                Some(IdentityPlatform::Commodore64),
                None
            )),
            Some(SystemBasis::Media)
        );
        // VIC-20 has no `.crt` launch format in the shared registry; that is
        // deliberately unchanged so library scans classify `.crt` as before.
        assert_eq!(
            resolve5(&vic, "VIC-20", FOLDER, Some(IdentityPlatform::Vic20), None),
            Outcome::NotApplicable
        );
        // A VIC-20 cartridge is never started as a C64 one, even by choice.
        assert_eq!(
            resolve5(&vic, "Commodore 64", MANUAL, None, None),
            Outcome::Contradicted {
                recorded: "Commodore 64",
                content: "VIC-20"
            }
        );
        // A C64 cartridge filed under VIC-20 (real library: Gorf) is refused
        // with the conflict named, not left at a generic review.
        assert_eq!(
            resolve5(&c64, "VIC-20", FOLDER, Some(IdentityPlatform::Vic20), None),
            Outcome::Contradicted {
                recorded: "VIC-20",
                content: "Commodore 64"
            }
        );
        assert_eq!(
            resolve(&c64, "VIC-20", FOLDER, None, None, Some("VIC-20")),
            Outcome::Contradicted {
                recorded: "VIC-20",
                content: "Commodore 64"
            }
        );
        // Without a signature the format decides exactly as before: `.crt`
        // is a strong extension only the C64 lists.
        let bare = write(&dir, "Unknown.crt", &[0u8; 128]);
        assert_eq!(
            allowed(&resolve5(&bare, "Commodore 64", FOLDER, None, None)),
            Some(SystemBasis::Media)
        );
        // A report naming another system without a launch kind still stops.
        assert_eq!(
            resolve5(
                &bare,
                "Commodore 64",
                FOLDER,
                Some(IdentityPlatform::Enterprise),
                None
            ),
            Outcome::NotApplicable
        );
    }

    /// Owner decision 2026-11-10: the 1,815 games with no system at all use
    /// the same lane. Nothing is offered on a guess; the person picks a
    /// compatible system for one try or remembers it for the unchanged file.
    #[test]
    fn a_game_with_no_system_can_be_tried_or_remembered_but_never_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let chd = write(&dir, "Avenger (Japan).chd", &[0u8; 4096]);
        assert_eq!(
            resolve(&chd, "", None, None, None, None),
            Outcome::NotApplicable
        );
        match resolve(&chd, "", None, None, Some("PC Engine CD"), None) {
            Outcome::Allowed(identity, SystemBasis::Trial) => {
                assert_eq!(identity.platform_id, "PC Engine CD");
                assert!(identity.game_key.starts_with("unverified:"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            allowed(&resolve(&chd, "", None, None, None, Some("Dreamcast"))),
            Some(SystemBasis::Remembered)
        );
        // The file's own header still wins over a try or a remembered choice.
        let nes = ines(&dir, "Unknown (USA).bin");
        assert!(matches!(
            resolve(&nes, "", None, None, None, Some("SNES")),
            Outcome::Contradicted { content: "NES", .. }
        ));
        assert!(matches!(
            resolve(&nes, "", None, None, Some("SNES"), None),
            Outcome::Contradicted { content: "NES", .. }
        ));
    }

    #[test]
    fn only_systems_the_media_allows_and_an_installed_route_serves_are_offered() {
        let dir = tempfile::tempdir().unwrap();
        let chd = write(&dir, "Avenger (Japan).chd", &[0u8; 4096]);
        let every = trial_systems(&chd, None, &|_| true);
        assert!(
            every.len() > 1 && every.contains(&"PC Engine CD"),
            "{every:?}"
        );
        // Only systems with an installed launch route.
        assert_eq!(
            trial_systems(&chd, None, &|system| system == "PC Engine CD"),
            vec!["PC Engine CD"]
        );
        assert!(trial_systems(&chd, None, &|_| false).is_empty());
        // A file whose own header names its system offers that system only,
        // never one it rules out.
        let rom = ines(&dir, "Unknown (USA).nes");
        let offered = trial_systems(&rom, None, &|_| true);
        assert!(!offered.contains(&"SNES"), "{offered:?}");
        // Formats no system lists offer nothing.
        let odd = write(&dir, "notes.xyz", b"hello");
        assert!(trial_systems(&odd, None, &|_| true).is_empty());
    }
}
