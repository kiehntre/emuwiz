//! Per-game view of classic POKE trainers, for Blue Game Details.
//!
//! Read-only evidence: it parses a trainer file that sits next to the game,
//! says where it came from, which emulators could use it and why it is or is
//! not eligible. It never writes memory, launches an emulator or applies
//! anything: no launch path consumes a POKE projection yet.
//!
//! A `.pok` beside a game is a filename association. It cannot prove the
//! trainer was written for this exact release, so such trainers stay
//! preview-only. A trainer only becomes eligible when it names a verified
//! identity that still equals the game's current verified identity.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::classic_poke_family::{
    PokeCheat, PokePlatform, PokeProjectionRefusal, PokeRuntimeEmulator, PokeRuntimeSupport,
    normalize_zx_pok_family, poke_runtime_capability, project_poke_runtime,
};
use crate::open_retro_cheat_providers::parse_zx_pok;

/// A trainer file larger than this is ignored rather than read.
pub const TRAINER_FILE_MAX_BYTES: u64 = 256 * 1024;

const ALL_EMULATORS: [PokeRuntimeEmulator; 7] = [
    PokeRuntimeEmulator::Fuse,
    PokeRuntimeEmulator::Vice,
    PokeRuntimeEmulator::Hatari,
    PokeRuntimeEmulator::Caprice32,
    PokeRuntimeEmulator::OpenMsx,
    PokeRuntimeEmulator::BeebEm,
    PokeRuntimeEmulator::BEm,
];

/// The game the trainers are being matched against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassicTrainerGame {
    /// Library platform label, e.g. "ZX Spectrum".
    pub platform: String,
    /// Current identity string, only when one is known.
    pub identity: Option<String>,
    /// Whether that identity is verified (exact hash / DAT), not a hint.
    pub identity_verified: bool,
}

/// How a trainer relates to the game right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassicTrainerMatch {
    /// Verified identity on both sides and they agree.
    VerifiedIdentity,
    /// Found by file name only; the release is not proven.
    FilenameOnly,
    /// The trainer names an identity that is not the game's current one
    /// (stale or for another release).
    IdentityMismatch,
    /// The trainer is for another platform than the game.
    WrongPlatform,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassicTrainerEmulator {
    pub emulator: PokeRuntimeEmulator,
    pub support: PokeRuntimeSupport,
    /// Whether this adapter could emit an exact artifact once the trainer is
    /// verified for the game (independent of the current identity).
    pub exact_when_verified: bool,
    /// Why not, when `exact_when_verified` is false.
    pub blocker: Option<PokeProjectionRefusal>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassicTrainerOption {
    pub title: String,
    pub poke_count: usize,
    pub source: String,
    pub platform: PokePlatform,
    pub association: ClassicTrainerMatch,
    /// Emulators for the trainer's platform only.
    pub emulators: Vec<ClassicTrainerEmulator>,
}

impl ClassicTrainerOption {
    /// True only for a verified match with an adapter that can emit an exact
    /// artifact. Even then nothing is applied: it is a preparation state.
    #[must_use]
    pub fn eligible_for_preparation(&self) -> bool {
        self.association == ClassicTrainerMatch::VerifiedIdentity
            && self.emulators.iter().any(|e| e.exact_when_verified)
    }

    /// Plain statement of what can happen with this trainer.
    #[must_use]
    pub fn state_label(&self) -> &'static str {
        match self.association {
            ClassicTrainerMatch::WrongPlatform => "Not for this platform: ignored",
            ClassicTrainerMatch::IdentityMismatch => {
                "Made for a different release: preview only, not applicable"
            }
            ClassicTrainerMatch::FilenameOnly => {
                "Candidate only: matched by file name, not verified compatibility. Preview only"
            }
            ClassicTrainerMatch::VerifiedIdentity if self.eligible_for_preparation() => {
                "Verified match: an exact emulator file can be prepared, but EmuWiz does not apply it"
            }
            ClassicTrainerMatch::VerifiedIdentity => {
                "Verified match, but no supported emulator path exists: preview only"
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassicTrainerReport {
    pub options: Vec<ClassicTrainerOption>,
    /// Why nothing could be listed (unreadable, too large, malformed).
    pub problem: Option<String>,
}

#[must_use]
pub fn poke_platform_for_label(label: &str) -> Option<PokePlatform> {
    let key: String = label
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    match key.as_str() {
        "zxspectrum" | "sinclairzxspectrum" | "spectrum" => Some(PokePlatform::ZxSpectrum),
        "amstradcpc" | "cpc" => Some(PokePlatform::AmstradCpc),
        "commodore64" | "c64" => Some(PokePlatform::Commodore64),
        "atarist" | "atarisste" => Some(PokePlatform::AtariSt),
        "msx" | "msx2" => Some(PokePlatform::Msx),
        "bbcmicro" | "bbc" => Some(PokePlatform::BbcMicro),
        "acorn8bit" | "acornelectron" | "electron" => Some(PokePlatform::Acorn8Bit),
        _ => None,
    }
}

/// Assess already-parsed trainers against the game's current identity.
#[must_use]
pub fn assess_poke_cheats(game: &ClassicTrainerGame, cheats: &[PokeCheat]) -> ClassicTrainerReport {
    let game_platform = poke_platform_for_label(&game.platform);
    let options = cheats
        .iter()
        .map(|cheat| {
            let association = if game_platform != Some(cheat.platform) {
                ClassicTrainerMatch::WrongPlatform
            } else {
                match cheat.target_identity.as_deref().filter(|id| !id.is_empty()) {
                    None => ClassicTrainerMatch::FilenameOnly,
                    Some(id)
                        if cheat.target_verified
                            && game.identity_verified
                            && game.identity.as_deref() == Some(id) =>
                    {
                        ClassicTrainerMatch::VerifiedIdentity
                    }
                    Some(_) => ClassicTrainerMatch::IdentityMismatch,
                }
            };
            let emulators = if association == ClassicTrainerMatch::WrongPlatform {
                Vec::new()
            } else {
                emulator_rows(cheat)
            };
            ClassicTrainerOption {
                title: cheat.title.clone(),
                poke_count: cheat.operations.len(),
                source: cheat.source.clone(),
                platform: cheat.platform,
                association,
                emulators,
            }
        })
        .collect();
    ClassicTrainerReport {
        options,
        problem: None,
    }
}

fn emulator_rows(cheat: &PokeCheat) -> Vec<ClassicTrainerEmulator> {
    // Ask each adapter whether it could be exact for this trainer if its
    // identity were verified. The real identity is judged separately.
    let mut probe = cheat.clone();
    probe.target_identity = Some("probe".into());
    probe.target_verified = true;
    ALL_EMULATORS
        .into_iter()
        .map(poke_runtime_capability)
        .filter(|capability| capability.platform == cheat.platform)
        .map(|capability| {
            let projection = project_poke_runtime(&probe, capability.emulator);
            ClassicTrainerEmulator {
                emulator: capability.emulator,
                support: capability.support,
                exact_when_verified: capability.support.can_prepare()
                    && projection.refusal.is_none(),
                blocker: projection.refusal,
                detail: capability.detail,
            }
        })
        .collect()
}

/// Read and assess a `.pok` file that sits beside the game, if there is one.
/// `Ok(None)` means there is no sidecar. Nothing is modified.
pub fn assess_sidecar_pok(
    game: &ClassicTrainerGame,
    game_path: &Path,
) -> Result<Option<ClassicTrainerReport>, String> {
    let Some(path) = find_sidecar(game_path) else {
        return Ok(None);
    };
    let source = path.file_name().map_or_else(
        || "trainer file".into(),
        |n| n.to_string_lossy().into_owned(),
    );
    let problem = |text: String| {
        Ok(Some(ClassicTrainerReport {
            options: Vec::new(),
            problem: Some(format!("{source}: {text}")),
        }))
    };
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return problem("not a regular file, ignored".into()),
        Err(error) => return problem(format!("cannot be read ({error})")),
    };
    if metadata.len() > TRAINER_FILE_MAX_BYTES {
        return problem("too large to be a trainer file, ignored".into());
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&path).and_then(|file| {
        file.take(TRAINER_FILE_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
    });
    if let Err(error) = read {
        return problem(format!("cannot be read ({error})"));
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return problem("is not plain text, ignored".into());
    };
    let trainers = match parse_zx_pok(&text) {
        Ok(trainers) => trainers,
        Err(error) => return problem(format!("is not a valid ZX .pok file ({error:?})")),
    };
    let cheats: Vec<PokeCheat> = trainers
        .iter()
        .map(|trainer| {
            let mut cheat = normalize_zx_pok_family(trainer);
            cheat.source = format!("{source} (next to the game, matched by file name)");
            cheat
        })
        .collect();
    Ok(Some(assess_poke_cheats(game, &cheats)))
}

fn find_sidecar(game_path: &Path) -> Option<PathBuf> {
    ["pok", "POK"]
        .into_iter()
        .map(|extension| game_path.with_extension(extension))
        .find(|candidate| {
            std::fs::symlink_metadata(candidate).is_ok() && candidate.as_path() != game_path
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classic_poke_family::{PokeBank, manual_poke};

    fn game(platform: &str, identity: Option<&str>, verified: bool) -> ClassicTrainerGame {
        ClassicTrainerGame {
            platform: platform.into(),
            identity: identity.map(str::to_string),
            identity_verified: verified,
        }
    }

    fn zx_cheat(identity: Option<&str>, verified: bool) -> PokeCheat {
        let mut cheat =
            normalize_zx_pok_family(&parse_zx_pok("N Lives\nZ 8 23456 0 53\nY\n").unwrap()[0]);
        cheat.target_identity = identity.map(str::to_string);
        cheat.target_verified = verified;
        cheat
    }

    #[test]
    fn filename_only_trainer_is_preview_only_even_for_a_verified_game() {
        let report = assess_poke_cheats(
            &game("ZX Spectrum", Some("rel-a"), true),
            &[zx_cheat(None, false)],
        );
        let option = &report.options[0];
        assert_eq!(option.association, ClassicTrainerMatch::FilenameOnly);
        assert!(!option.eligible_for_preparation());
        assert!(option.state_label().starts_with("Candidate only"));
        // Fuse could be exact once verified; Fuse is the only ZX adapter.
        assert_eq!(option.emulators.len(), 1);
        assert!(option.emulators[0].exact_when_verified);
    }

    #[test]
    fn verified_matching_identity_is_eligible_but_never_applied() {
        let report = assess_poke_cheats(
            &game("ZX Spectrum", Some("rel-a"), true),
            &[zx_cheat(Some("rel-a"), true)],
        );
        let option = &report.options[0];
        assert_eq!(option.association, ClassicTrainerMatch::VerifiedIdentity);
        assert!(option.eligible_for_preparation());
        assert!(option.state_label().contains("does not apply"));
    }

    #[test]
    fn stale_or_foreign_identity_is_not_eligible() {
        for (identity, verified) in [("rel-b", true), ("rel-a", false)] {
            let report = assess_poke_cheats(
                &game("ZX Spectrum", Some("rel-a"), true),
                &[zx_cheat(Some(identity), verified)],
            );
            assert_eq!(
                report.options[0].association,
                ClassicTrainerMatch::IdentityMismatch
            );
            assert!(!report.options[0].eligible_for_preparation());
        }
        // The game lost its verification since the trainer was matched.
        let report = assess_poke_cheats(
            &game("ZX Spectrum", Some("rel-a"), false),
            &[zx_cheat(Some("rel-a"), true)],
        );
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::IdentityMismatch
        );
        let report = assess_poke_cheats(
            &game("ZX Spectrum", None, false),
            &[zx_cheat(Some("rel-a"), true)],
        );
        assert!(!report.options[0].eligible_for_preparation());
    }

    #[test]
    fn wrong_platform_trainers_list_no_emulators() {
        let report =
            assess_poke_cheats(&game("Commodore 64", None, false), &[zx_cheat(None, false)]);
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::WrongPlatform
        );
        assert!(report.options[0].emulators.is_empty());
        let report = assess_poke_cheats(&game("Game Boy", None, false), &[zx_cheat(None, false)]);
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::WrongPlatform
        );
    }

    #[test]
    fn unsupported_emulators_stay_preview_only_even_when_verified() {
        let mut cheat = manual_poke(
            PokePlatform::AtariSt,
            "Infinite lives",
            0x1000,
            9,
            None,
            PokeBank::Unspecified,
            None,
            None,
            false,
        )
        .unwrap();
        cheat.target_identity = Some("st-1".into());
        cheat.target_verified = true;
        let report = assess_poke_cheats(&game("Atari ST", Some("st-1"), true), &[cheat]);
        let option = &report.options[0];
        assert_eq!(option.association, ClassicTrainerMatch::VerifiedIdentity);
        assert!(!option.eligible_for_preparation());
        assert_eq!(option.emulators[0].emulator, PokeRuntimeEmulator::Hatari);
        assert_eq!(option.emulators[0].support, PokeRuntimeSupport::PreviewOnly);
        assert!(option.state_label().contains("no supported emulator"));
    }

    #[test]
    fn sidecar_pok_is_read_bounded_and_labelled_by_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let game_path = dir.path().join("Manic Miner.tap");
        std::fs::write(&game_path, b"tap").unwrap();
        let g = game("ZX Spectrum", Some("rel-a"), true);
        assert_eq!(assess_sidecar_pok(&g, &game_path), Ok(None));

        std::fs::write(
            dir.path().join("Manic Miner.pok"),
            "N Infinite lives\nZ 8 35899 0 53\nY\n",
        )
        .unwrap();
        let report = assess_sidecar_pok(&g, &game_path).unwrap().unwrap();
        assert_eq!(report.options.len(), 1);
        assert!(report.options[0].source.contains("Manic Miner.pok"));
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::FilenameOnly
        );
        assert_eq!(report.options[0].poke_count, 1);

        std::fs::write(dir.path().join("Manic Miner.pok"), "garbage").unwrap();
        let report = assess_sidecar_pok(&g, &game_path).unwrap().unwrap();
        assert!(report.options.is_empty());
        assert!(report.problem.unwrap().contains("not a valid ZX .pok"));

        std::fs::write(
            dir.path().join("Manic Miner.pok"),
            vec![b'N'; TRAINER_FILE_MAX_BYTES as usize + 1],
        )
        .unwrap();
        let report = assess_sidecar_pok(&g, &game_path).unwrap().unwrap();
        assert!(report.problem.unwrap().contains("too large"));
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_symlink_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let game_path = dir.path().join("g.tap");
        let secret = dir.path().join("elsewhere.txt");
        std::fs::write(&secret, "N x\nZ 8 1 1 1\nY\n").unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join("g.pok")).unwrap();
        let report = assess_sidecar_pok(&game("ZX Spectrum", None, false), &game_path)
            .unwrap()
            .unwrap();
        assert!(report.options.is_empty());
        assert!(report.problem.unwrap().contains("not a regular file"));
    }

    #[test]
    fn platform_labels_map_and_unknown_is_none() {
        assert_eq!(
            poke_platform_for_label("ZX Spectrum"),
            Some(PokePlatform::ZxSpectrum)
        );
        assert_eq!(
            poke_platform_for_label("commodore-64"),
            Some(PokePlatform::Commodore64)
        );
        assert_eq!(poke_platform_for_label("Nintendo 64"), None);
    }
}
