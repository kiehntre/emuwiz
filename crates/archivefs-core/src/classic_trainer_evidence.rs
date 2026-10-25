//! Per-game view of classic POKE trainers, for Blue Game Details.
//!
//! Read-only evidence: it parses a trainer file that sits next to the game,
//! says where it came from, which emulators could use it and why it is or is
//! not eligible. It never writes memory, launches an emulator or applies
//! anything. The separate `launch` bridge validates an explicit selection
//! and a verified native backend before consuming a Fuse projection.
//!
//! A `.pok` beside a game is a filename association. It cannot prove the
//! trainer was written for this exact release, so such trainers stay
//! candidates. A trainer becomes a verified association only when a target
//! file (`<game>.pok.target`) pins both the exact trainer bytes and an exact
//! game hash, and that hash equals the game's current DAT-verified hash. The
//! pin is a claim; only the DAT-verified hash of the unchanged game file can
//! confirm it, so a pin never verifies anything on its own.

pub mod launch;
pub mod pinning;

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::classic_poke_family::{
    PokeCheat, PokePlatform, PokeProjectionRefusal, PokeRuntimeEmulator, PokeRuntimeSupport,
    normalize_zx_pok_family, poke_runtime_capability, project_poke_runtime,
};
use crate::dat::library_identity_summary::LibraryItemHashes;
use crate::open_retro_cheat_providers::parse_zx_pok;

/// A trainer file larger than this is ignored rather than read.
pub const TRAINER_FILE_MAX_BYTES: u64 = 256 * 1024;
/// A trainer target file larger than this is refused rather than read.
pub const TRAINER_TARGET_MAX_BYTES: u64 = 4 * 1024;
/// Extension of the file that pins a trainer to an exact release.
pub const TRAINER_TARGET_EXTENSION: &str = "pok.target";

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
    /// Exact identities of the game file as `algorithm:hex`, e.g.
    /// `sha1:...`. See [`exact_game_identities`].
    pub identities: Vec<String>,
    /// Whether those identities are verified (a current DAT exact match on
    /// the unchanged file), not hints.
    pub identity_verified: bool,
}

/// The exact identities a game's audited hashes offer a trainer pin. CRC32
/// is left out: it is too weak to tie a trainer to one release.
#[must_use]
pub fn exact_game_identities(hashes: &LibraryItemHashes) -> Vec<String> {
    [
        ("sha256", hashes.sha256.as_deref()),
        ("sha1", hashes.sha1.as_deref()),
        ("md5", hashes.md5.as_deref()),
    ]
    .into_iter()
    .filter_map(|(algorithm, digest)| {
        let digest = digest?.trim().to_ascii_lowercase();
        valid_digest(algorithm, &digest).then(|| format!("{algorithm}:{digest}"))
    })
    .collect()
}

fn valid_digest(algorithm: &str, digest: &str) -> bool {
    let length = match algorithm {
        "sha256" => 64,
        "sha1" => 40,
        "md5" => 32,
        _ => return false,
    };
    digest.len() == length && digest.bytes().all(|b| b.is_ascii_hexdigit())
}

/// How a trainer relates to the game right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassicTrainerMatch {
    /// The trainer is pinned to an exact game hash that equals this game's
    /// current DAT-verified hash.
    VerifiedIdentity,
    /// Found by file name only; the release is not proven.
    FilenameOnly,
    /// The trainer names a target that cannot be confirmed: the game is not
    /// currently DAT-verified, has no verified hash of that kind, or the
    /// target itself is only a hint.
    Unconfirmed,
    /// The trainer is pinned to a hash that differs from this game's
    /// verified hash of the same kind (another release, or a changed file).
    IdentityMismatch,
    /// The trainer is for another platform than the game.
    WrongPlatform,
}

impl ClassicTrainerMatch {
    /// True only for the verified association. Everything else is a candidate.
    #[must_use]
    pub fn is_verified(self) -> bool {
        self == Self::VerifiedIdentity
    }
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

    /// Association and projection evidence; never proof of backend activation.
    #[must_use]
    pub fn state_label(&self) -> &'static str {
        match self.association {
            ClassicTrainerMatch::WrongPlatform => "Not for this platform: ignored",
            ClassicTrainerMatch::IdentityMismatch => {
                "Pinned to a different release than this verified copy: candidate only, not applicable"
            }
            ClassicTrainerMatch::Unconfirmed => {
                "Pinned to an exact release, but this copy is not DAT-verified with a matching kind of hash: candidate only"
            }
            ClassicTrainerMatch::FilenameOnly => {
                "Candidate only: matched by file name, not verified compatibility. Preview only"
            }
            ClassicTrainerMatch::VerifiedIdentity if self.eligible_for_preparation() => {
                "Verified for this release: pinned hash matches the DAT-verified game. No trainer is enabled here; launch requires explicit selection and a separately verified backend"
            }
            ClassicTrainerMatch::VerifiedIdentity => {
                "Verified for this release, but no emulator can use it exactly: preview only"
            }
        }
    }
}

/// What the trainer target file (`<game>.pok.target`) said, if there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassicTrainerPin {
    /// No target file: every trainer is a filename candidate.
    Absent,
    /// The target file pins these exact trainer bytes to `game_identity`.
    Pinned { game_identity: String },
    /// The target file pins different trainer bytes than the `.pok` now
    /// beside the game, so it does not apply.
    TrainerChanged,
    /// The target file is unusable; nothing is pinned.
    Refused(String),
}

impl ClassicTrainerPin {
    /// Plain explanation for the card, when there is something to say.
    #[must_use]
    pub fn note(&self) -> Option<String> {
        match self {
            Self::Absent => None,
            Self::Pinned { game_identity } => Some(format!(
                "Trainer target file pins this trainer file to game {game_identity}. EmuWiz only treats that as verified when it equals this game's current DAT-verified hash."
            )),
            Self::TrainerChanged => Some(
                "Trainer target file was made for different trainer contents than the .pok here, so it is ignored: candidates only.".into(),
            ),
            Self::Refused(why) => Some(format!(
                "Trainer target file ignored ({why}): candidates only."
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassicTrainerReport {
    pub options: Vec<ClassicTrainerOption>,
    /// Why nothing could be listed (unreadable, too large, malformed).
    pub problem: Option<String>,
    /// What the trainer target file said.
    pub pin: ClassicTrainerPin,
}

impl ClassicTrainerReport {
    pub fn verified(&self) -> impl Iterator<Item = &ClassicTrainerOption> {
        self.options.iter().filter(|o| o.association.is_verified())
    }

    pub fn candidates(&self) -> impl Iterator<Item = &ClassicTrainerOption> {
        self.options.iter().filter(|o| !o.association.is_verified())
    }
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
                    Some(id) => match_target(game, id, cheat.target_verified),
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
        pin: ClassicTrainerPin::Absent,
    }
}

fn match_target(
    game: &ClassicTrainerGame,
    target: &str,
    target_exact: bool,
) -> ClassicTrainerMatch {
    if !target_exact || !game.identity_verified {
        return ClassicTrainerMatch::Unconfirmed;
    }
    if game.identities.iter().any(|id| id == target) {
        return ClassicTrainerMatch::VerifiedIdentity;
    }
    let kind = |id: &str| id.split_once(':').map(|(kind, _)| kind.to_string());
    let target_kind = kind(target);
    if target_kind.is_some() && game.identities.iter().any(|id| kind(id) == target_kind) {
        ClassicTrainerMatch::IdentityMismatch
    } else {
        ClassicTrainerMatch::Unconfirmed
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
            pin: ClassicTrainerPin::Absent,
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
    let pin = read_pin(game_path, &hex_sha256(text.as_bytes()));
    let cheats: Vec<PokeCheat> = trainers
        .iter()
        .map(|trainer| {
            let mut cheat = normalize_zx_pok_family(trainer);
            if let ClassicTrainerPin::Pinned { game_identity } = &pin {
                cheat.source = format!("{source} (next to the game, pinned by its target file)");
                cheat.target_identity = Some(game_identity.clone());
                // The pin names an exact hash; whether it is this game is
                // decided only against the game's verified identity.
                cheat.target_verified = true;
            } else {
                cheat.source = format!("{source} (next to the game, matched by file name)");
            }
            cheat
        })
        .collect();
    let mut report = assess_poke_cheats(game, &cheats);
    report.pin = pin;
    Ok(Some(report))
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The trainer target file beside `game_path`, e.g. `Game.pok.target`.
#[must_use]
pub fn trainer_target_path(game_path: &Path) -> PathBuf {
    game_path.with_extension(TRAINER_TARGET_EXTENSION)
}

/// Read the trainer target file. It holds exactly two records, blank lines
/// and `#` comments aside:
///
/// ```text
/// trainer-sha256 <sha256 of the .pok bytes>
/// game-sha1 <hash of the exact game file>   (or game-sha256 / game-md5)
/// ```
///
/// Anything else, a duplicate, a symlink or an oversize file is refused, so a
/// damaged pin never turns into an association.
fn read_pin(game_path: &Path, trainer_sha256: &str) -> ClassicTrainerPin {
    let path = trainer_target_path(game_path);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ClassicTrainerPin::Absent;
        }
        Err(error) => return ClassicTrainerPin::Refused(format!("cannot be read: {error}")),
    };
    if !metadata.is_file() {
        return ClassicTrainerPin::Refused("not a regular file".into());
    }
    if metadata.len() > TRAINER_TARGET_MAX_BYTES {
        return ClassicTrainerPin::Refused("too large".into());
    }
    let mut bytes = Vec::new();
    if let Err(error) = std::fs::File::open(&path).and_then(|file| {
        file.take(TRAINER_TARGET_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
    }) {
        return ClassicTrainerPin::Refused(format!("cannot be read: {error}"));
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return ClassicTrainerPin::Refused("not plain text".into());
    };
    match parse_pin(&text) {
        Err(why) => ClassicTrainerPin::Refused(why),
        Ok((pinned_trainer, _)) if pinned_trainer != trainer_sha256 => {
            ClassicTrainerPin::TrainerChanged
        }
        Ok((_, game_identity)) => ClassicTrainerPin::Pinned { game_identity },
    }
}

fn parse_pin(text: &str) -> Result<(String, String), String> {
    let mut trainer = None;
    let mut game = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(key), Some(value), None) = (fields.next(), fields.next(), fields.next()) else {
            return Err("each line must be a key and one hash".into());
        };
        let value = value.to_ascii_lowercase();
        let (slot, algorithm) = match key {
            "trainer-sha256" => (&mut trainer, "sha256"),
            "game-sha256" => (&mut game, "sha256"),
            "game-sha1" => (&mut game, "sha1"),
            "game-md5" => (&mut game, "md5"),
            _ => return Err(format!("unknown record {key:?}")),
        };
        if !valid_digest(algorithm, &value) {
            return Err(format!("{key} is not a valid {algorithm} hash"));
        }
        if slot.is_some() {
            return Err("more than one trainer or game hash".into());
        }
        *slot = Some(if key == "trainer-sha256" {
            value
        } else {
            format!("{algorithm}:{value}")
        });
    }
    match (trainer, game) {
        (Some(trainer), Some(game)) => Ok((trainer, game)),
        _ => Err("needs one trainer-sha256 and one game hash".into()),
    }
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

    const SHA1_A: &str = "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA1_B: &str = "sha1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn game(platform: &str, identity: Option<&str>, verified: bool) -> ClassicTrainerGame {
        ClassicTrainerGame {
            platform: platform.into(),
            identities: identity.map(str::to_string).into_iter().collect(),
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
            &game("ZX Spectrum", Some(SHA1_A), true),
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
            &game("ZX Spectrum", Some(SHA1_A), true),
            &[zx_cheat(Some(SHA1_A), true)],
        );
        let option = &report.options[0];
        assert_eq!(option.association, ClassicTrainerMatch::VerifiedIdentity);
        assert!(option.eligible_for_preparation());
        assert!(option.state_label().contains("No trainer is enabled here"));
        assert_eq!(report.verified().count(), 1);
        assert_eq!(report.candidates().count(), 0);
    }

    #[test]
    fn stale_or_foreign_identity_is_not_eligible() {
        // Another release of the same kind of hash.
        let report = assess_poke_cheats(
            &game("ZX Spectrum", Some(SHA1_A), true),
            &[zx_cheat(Some(SHA1_B), true)],
        );
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::IdentityMismatch
        );
        assert!(!report.options[0].eligible_for_preparation());
        assert_eq!(report.candidates().count(), 1);
        // A hint target never verifies, even when it equals the game.
        // The game lost its verification since the trainer was pinned.
        // The game has no verified hash of the pinned kind.
        for (game_id, verified, target, target_exact) in [
            (Some(SHA1_A), true, SHA1_A, false),
            (Some(SHA1_A), false, SHA1_A, true),
            (None, false, SHA1_A, true),
            (
                Some(SHA1_A),
                true,
                "md5:cccccccccccccccccccccccccccccccc",
                true,
            ),
        ] {
            let report = assess_poke_cheats(
                &game("ZX Spectrum", game_id, verified),
                &[zx_cheat(Some(target), target_exact)],
            );
            assert_eq!(
                report.options[0].association,
                ClassicTrainerMatch::Unconfirmed
            );
            assert!(!report.options[0].eligible_for_preparation());
        }
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
        cheat.target_identity = Some(SHA1_A.into());
        cheat.target_verified = true;
        let report = assess_poke_cheats(&game("Atari ST", Some(SHA1_A), true), &[cheat]);
        let option = &report.options[0];
        assert_eq!(option.association, ClassicTrainerMatch::VerifiedIdentity);
        assert!(!option.eligible_for_preparation());
        assert_eq!(option.emulators[0].emulator, PokeRuntimeEmulator::Hatari);
        assert_eq!(option.emulators[0].support, PokeRuntimeSupport::PreviewOnly);
        assert!(
            option
                .state_label()
                .contains("no emulator can use it exactly")
        );
    }

    #[test]
    fn sidecar_pok_is_read_bounded_and_labelled_by_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let game_path = dir.path().join("Manic Miner.tap");
        std::fs::write(&game_path, b"tap").unwrap();
        let g = game("ZX Spectrum", Some(SHA1_A), true);
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

    const POK: &str = "N Infinite lives\nZ 8 35899 0 53\nY\n";

    fn pinned_dir(pin: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let game_path = dir.path().join("Manic Miner.tap");
        std::fs::write(&game_path, b"tap").unwrap();
        std::fs::write(dir.path().join("Manic Miner.pok"), POK).unwrap();
        std::fs::write(trainer_target_path(&game_path), pin).unwrap();
        (dir, game_path)
    }

    fn pin_text(game_line: &str) -> String {
        format!(
            "# pinned\ntrainer-sha256 {}\n{game_line}\n",
            hex_sha256(POK.as_bytes())
        )
    }

    #[test]
    fn pinned_trainer_is_verified_only_against_the_dat_verified_hash() {
        let (_dir, game_path) = pinned_dir(&pin_text(
            "game-sha1 AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ));
        let report = assess_sidecar_pok(&game("ZX Spectrum", Some(SHA1_A), true), &game_path)
            .unwrap()
            .unwrap();
        assert_eq!(
            report.pin,
            ClassicTrainerPin::Pinned {
                game_identity: SHA1_A.into()
            }
        );
        let option = &report.options[0];
        assert_eq!(option.association, ClassicTrainerMatch::VerifiedIdentity);
        assert!(option.source.contains("pinned by its target file"));

        // Same pin, but the game copy is not (or no longer) DAT-verified.
        let report = assess_sidecar_pok(&game("ZX Spectrum", Some(SHA1_A), false), &game_path)
            .unwrap()
            .unwrap();
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::Unconfirmed
        );
        // A different verified release.
        let report = assess_sidecar_pok(&game("ZX Spectrum", Some(SHA1_B), true), &game_path)
            .unwrap()
            .unwrap();
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::IdentityMismatch
        );
        // Wrong platform stays ignored whatever the pin says.
        let report = assess_sidecar_pok(&game("Commodore 64", Some(SHA1_A), true), &game_path)
            .unwrap()
            .unwrap();
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::WrongPlatform
        );
    }

    #[test]
    fn changed_trainer_bytes_void_the_pin() {
        let (dir, game_path) = pinned_dir(&pin_text(&format!("game-{}", SHA1_A.replace(':', " "))));
        std::fs::write(
            dir.path().join("Manic Miner.pok"),
            "N Infinite lives\nZ 8 35899 1 53\nY\n",
        )
        .unwrap();
        let report = assess_sidecar_pok(&game("ZX Spectrum", Some(SHA1_A), true), &game_path)
            .unwrap()
            .unwrap();
        assert_eq!(report.pin, ClassicTrainerPin::TrainerChanged);
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::FilenameOnly
        );
        assert!(report.pin.note().unwrap().contains("ignored"));
    }

    #[test]
    fn malformed_pins_are_refused_and_never_verify() {
        let trainer = hex_sha256(POK.as_bytes());
        let sha1 = SHA1_A.trim_start_matches("sha1:");
        for pin in [
            String::new(),
            format!("trainer-sha256 {trainer}\n"),
            format!("game-sha1 {sha1}\n"),
            format!("trainer-sha256 {trainer}\ngame-sha1 {sha1}\ngame-sha1 {sha1}\n"),
            format!("trainer-sha256 {trainer}\ngame-crc32 1234abcd\n"),
            format!("trainer-sha256 {trainer}\ngame-sha1 abc\n"),
            format!("trainer-sha256 {trainer} extra\ngame-sha1 {sha1}\n"),
            "x".repeat(TRAINER_TARGET_MAX_BYTES as usize + 1),
        ] {
            let (_dir, game_path) = pinned_dir(&pin);
            let report = assess_sidecar_pok(&game("ZX Spectrum", Some(SHA1_A), true), &game_path)
                .unwrap()
                .unwrap();
            assert!(
                matches!(report.pin, ClassicTrainerPin::Refused(_)),
                "{pin:?} gave {:?}",
                report.pin
            );
            assert_eq!(report.verified().count(), 0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_pin_is_refused() {
        let (dir, game_path) = pinned_dir("");
        let real = dir.path().join("real.target");
        std::fs::write(&real, pin_text(&format!("game-sha1 {}", &SHA1_A[5..]))).unwrap();
        std::fs::remove_file(trainer_target_path(&game_path)).unwrap();
        std::os::unix::fs::symlink(&real, trainer_target_path(&game_path)).unwrap();
        let report = assess_sidecar_pok(&game("ZX Spectrum", Some(SHA1_A), true), &game_path)
            .unwrap()
            .unwrap();
        assert!(matches!(report.pin, ClassicTrainerPin::Refused(_)));
        assert_eq!(report.verified().count(), 0);
    }

    #[test]
    fn exact_identities_skip_crc_and_malformed_hashes() {
        let hashes = LibraryItemHashes {
            size_bytes: Some(3),
            crc32: Some("1234abcd".into()),
            md5: Some("not-a-hash".into()),
            sha1: Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()),
            sha256: None,
        };
        assert_eq!(exact_game_identities(&hashes), vec![SHA1_A.to_string()]);
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
