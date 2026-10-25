//! Explicit, create-only pinning. Reuses the evidence parser and Gold's atomic
//! text writer. Pins assert a user association; they never prove compatibility.
use super::*;
use crate::patch_manager::validate_destination_root;
use crate::safe_read::{TrustedRoots, open_bounded_read};
use std::os::unix::fs::MetadataExt;

const GAME_MAX: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observation {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinPreview {
    game: ClassicTrainerGame,
    game_path: PathBuf,
    trainer_path: PathBuf,
    destination: PathBuf,
    contents: String,
    parent: (u64, u64),
    game_file: Observation,
    trainer_file: Observation,
}

impl PinPreview {
    pub fn destination(&self) -> &Path {
        &self.destination
    }
    pub fn trainer_path(&self) -> &Path {
        &self.trainer_path
    }
    pub fn contents(&self) -> &str {
        &self.contents
    }
}

fn observe(
    path: &Path,
    roots: &TrustedRoots,
    limit: u64,
) -> Result<(Vec<u8>, Observation), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > limit {
        return Err(
            "Expected a bounded, regular file with one link; symlinks and hardlinks are refused."
                .into(),
        );
    }
    let mut file = open_bounded_read(path, roots)
        .map_err(|e| e.detail())?
        .into_file();
    let before = file.metadata().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = file.metadata().map_err(|e| e.to_string())?;
    let digest = hex_sha256(&bytes);
    let observation = |m: &std::fs::Metadata| Observation {
        device: m.dev(),
        inode: m.ino(),
        size: m.len(),
        modified: (m.mtime(), m.mtime_nsec()),
        changed: (m.ctime(), m.ctime_nsec()),
        sha256: digest.clone(),
    };
    if bytes.len() as u64 > limit
        || observation(&metadata) != observation(&before)
        || observation(&before) != observation(&after)
    {
        return Err("File changed while reading; refresh and preview again.".into());
    }
    let observed = observation(&after);
    Ok((bytes, observed))
}

/// Only same-name sidecars can be represented by Gold's existing pin format.
/// A fresh current DAT projection must be supplied again at confirmation.
pub fn preview_pin(
    game: &ClassicTrainerGame,
    game_path: &Path,
    roots: &TrustedRoots,
) -> Result<PinPreview, String> {
    if !game.identity_verified
        || poke_platform_for_label(&game.platform) != Some(PokePlatform::ZxSpectrum)
    {
        return Err("A current DAT-verified ZX Spectrum identity is required.".into());
    }
    let parent = game_path.parent().ok_or("Game has no parent directory.")?;
    validate_destination_root(parent).map_err(|e| format!("Unsafe destination: {e}"))?;
    let directory = std::fs::metadata(parent).map_err(|e| e.to_string())?;
    if !roots.contains_canonical(parent) {
        return Err("Game folder is outside configured trusted roots.".into());
    }
    let trainer_path = find_sidecar(game_path).ok_or("No existing same-name .pok candidate.")?;
    if ["pok", "POK"]
        .iter()
        .filter(|ext| game_path.with_extension(ext).symlink_metadata().is_ok())
        .count()
        != 1
    {
        return Err(
            "Multiple .pok candidates conflict; choose one sidecar outside EmuWiz first.".into(),
        );
    }
    let destination = trainer_target_path(game_path);
    match std::fs::symlink_metadata(&destination) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
        Ok(_) => return Err(
            "A pin already exists (possibly conflicting or unsafe). It will never be overwritten."
                .into(),
        ),
    }
    let (game_bytes, game_file) = observe(game_path, roots, GAME_MAX)?;
    let identity = game
        .identities
        .first()
        .ok_or("Verified identity has no exact hash.")?;
    let (algorithm, expected) = identity.split_once(':').ok_or("Invalid exact identity.")?;
    let actual = match algorithm {
        "sha256" => hex_sha256(&game_bytes),
        "sha1" => sha1::Sha1::digest(&game_bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        "md5" => md5::Md5::digest(&game_bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        _ => return Err("Unsupported exact hash; filename and CRC are never verification.".into()),
    };
    if !valid_digest(algorithm, expected) || actual != expected {
        return Err(
            "Game bytes differ from the current DAT identity; rescan before pinning.".into(),
        );
    }
    let (trainer_bytes, trainer_file) = observe(&trainer_path, roots, TRAINER_FILE_MAX_BYTES)?;
    let text = std::str::from_utf8(&trainer_bytes).map_err(|_| "Trainer is not plain text.")?;
    let trainers = parse_zx_pok(text).map_err(|e| format!("Invalid .pok: {e:?}"))?;
    if trainers.is_empty() {
        return Err("Trainer has no options to associate.".into());
    }
    let contents = format!(
        "trainer-sha256 {}\ngame-{algorithm} {expected}\n",
        trainer_file.sha256
    );
    parse_pin(&contents)?;
    Ok(PinPreview {
        game: game.clone(),
        game_path: game_path.into(),
        trainer_path,
        destination,
        contents,
        parent: (directory.dev(), directory.ino()),
        game_file,
        trainer_file,
    })
}

/// Rebuilds every observation, including actual game hashes, before exclusive
/// publication. Existing pins are never replaced; this has no Undo adapter.
pub fn create_pin(
    preview: &PinPreview,
    current_game: &ClassicTrainerGame,
    roots: &TrustedRoots,
) -> Result<(), String> {
    let current = preview_pin(current_game, &preview.game_path, roots)?;
    if &current != preview {
        return Err("Identity, source or destination changed after preview. Preview again.".into());
    }
    crate::atomic_text::create_new(&preview.destination, &preview.contents, preview.parent)
        .map_err(|e| e.to_string())?;
    // Detect source edits or pathname replacement during publication too. A
    // post-publication failure never authorizes cleanup of a possibly changed pin.
    let (_, game_file) = observe(&preview.game_path, roots, GAME_MAX)
        .map_err(|e| format!("Pin was published; game could not be rechecked: {e}"))?;
    let (_, trainer_file) = observe(&preview.trainer_path, roots, TRAINER_FILE_MAX_BYTES)
        .map_err(|e| format!("Pin was published; trainer could not be rechecked: {e}"))?;
    if game_file != preview.game_file || trainer_file != preview.trainer_file {
        return Err("Pin was published but a source changed during creation. Inspect the pin and refresh; success is not confirmed.".into());
    }
    match read_pin(&preview.game_path, &preview.trainer_file.sha256) {
        ClassicTrainerPin::Pinned { game_identity } if preview.game.identities.contains(&game_identity) => Ok(()),
        _ => Err("Pin publication completed but verification failed. Inspect the destination before retrying; nothing was removed.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn fixture() -> (tempfile::TempDir, PathBuf, ClassicTrainerGame, TrustedRoots) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.tap");
        fs::write(&path, b"synthetic game").unwrap();
        fs::write(path.with_extension("pok"), "N Lives\nZ 8 35899 0 53\nY\n").unwrap();
        let game = ClassicTrainerGame {
            platform: "ZX Spectrum".into(),
            identity_verified: true,
            identities: vec![format!("sha256:{}", hex_sha256(b"synthetic game"))],
        };
        let roots = TrustedRoots::from_paths([dir.path()]);
        (dir, path, game, roots)
    }

    #[test]
    fn preview_is_read_only_exact_and_create_uses_existing_evidence() {
        let (_dir, path, game, roots) = fixture();
        let trainer = fs::read(path.with_extension("pok")).unwrap();
        let preview = preview_pin(&game, &path, &roots).unwrap();
        assert!(!trainer_target_path(&path).exists());
        assert_eq!(
            preview.contents(),
            format!(
                "trainer-sha256 {}\ngame-sha256 {}\n",
                hex_sha256(&trainer),
                hex_sha256(b"synthetic game")
            )
        );
        create_pin(&preview, &game, &roots).unwrap();
        assert_eq!(
            fs::read_to_string(preview.destination()).unwrap(),
            preview.contents()
        );
        let report = assess_sidecar_pok(&game, &path).unwrap().unwrap();
        assert_eq!(report.verified().count(), 1);
        assert_eq!(fs::read(&path).unwrap(), b"synthetic game");
        assert_eq!(fs::read(path.with_extension("pok")).unwrap(), trainer);
        assert!(create_pin(&preview, &game, &roots).is_err());
    }

    #[test]
    fn unverified_wrong_platform_and_stale_hash_refuse() {
        let (_dir, path, mut game, roots) = fixture();
        game.identity_verified = false;
        assert!(preview_pin(&game, &path, &roots).is_err());
        game.identity_verified = true;
        game.platform = "Commodore 64".into();
        assert!(preview_pin(&game, &path, &roots).is_err());
        game.platform = "ZX Spectrum".into();
        fs::write(&path, b"changed game").unwrap();
        assert!(
            preview_pin(&game, &path, &roots)
                .unwrap_err()
                .contains("Game bytes differ")
        );
        assert!(!trainer_target_path(&path).exists());
    }

    #[test]
    fn changed_sources_and_withdrawn_identity_refuse_after_preview() {
        for change in 0..4 {
            let (_dir, path, mut game, roots) = fixture();
            let preview = preview_pin(&game, &path, &roots).unwrap();
            match change {
                0 => fs::write(&path, b"different game").unwrap(),
                1 => {
                    fs::write(path.with_extension("pok"), "N Changed\nZ 8 35899 1 53\nY\n").unwrap()
                }
                2 => game.identity_verified = false,
                _ => {
                    fs::remove_file(path.with_extension("pok")).unwrap();
                }
            }
            assert!(create_pin(&preview, &game, &roots).is_err());
            assert!(!preview.destination().exists());
        }
    }

    #[test]
    fn existing_conflicting_and_unsafe_destinations_are_preserved() {
        for kind in 0..4 {
            let (dir, path, game, roots) = fixture();
            let preview = preview_pin(&game, &path, &roots).unwrap();
            let destination = preview.destination();
            let decoy = dir.path().join("decoy");
            fs::write(&decoy, "preserve").unwrap();
            match kind {
                0 => fs::write(destination, "conflicting pin").unwrap(),
                1 => fs::create_dir(destination).unwrap(),
                2 => symlink(&decoy, destination).unwrap(),
                _ => symlink(dir.path().join("absent"), destination).unwrap(),
            }
            assert!(create_pin(&preview, &game, &roots).is_err());
            assert_eq!(fs::read_to_string(&decoy).unwrap(), "preserve");
            assert!(destination.symlink_metadata().is_ok());
        }
    }

    #[test]
    fn unsafe_sources_multiple_candidates_and_untrusted_root_refuse() {
        for kind in 0..5 {
            let (dir, path, game, roots) = fixture();
            let trainer = path.with_extension("pok");
            let mut roots = roots;
            match kind {
                0 => {
                    fs::rename(&trainer, dir.path().join("real")).unwrap();
                    symlink(dir.path().join("real"), &trainer).unwrap();
                }
                1 => {
                    fs::hard_link(&trainer, dir.path().join("other")).unwrap();
                }
                2 => {
                    fs::write(path.with_extension("POK"), fs::read(&trainer).unwrap()).unwrap();
                }
                3 => roots = TrustedRoots::none(),
                _ => {
                    fs::write(&trainer, vec![b'x'; TRAINER_FILE_MAX_BYTES as usize + 1]).unwrap();
                }
            }
            assert!(preview_pin(&game, &path, &roots).is_err(), "kind {kind}");
            assert!(!trainer_target_path(&path).exists());
        }
    }

    #[test]
    fn parent_replacement_and_symlinked_parent_refuse() {
        let (dir, path, game, roots) = fixture();
        let nested = dir.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let new_path = nested.join("game.tap");
        fs::rename(&path, &new_path).unwrap();
        fs::rename(path.with_extension("pok"), new_path.with_extension("pok")).unwrap();
        let preview = preview_pin(&game, &new_path, &roots).unwrap();
        let old = dir.path().join("old");
        fs::rename(&nested, &old).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::copy(old.join("game.tap"), &new_path).unwrap();
        fs::copy(old.join("game.pok"), new_path.with_extension("pok")).unwrap();
        assert!(create_pin(&preview, &game, &roots).is_err());
        fs::remove_dir_all(&nested).unwrap();
        symlink(&old, &nested).unwrap();
        assert!(preview_pin(&game, &new_path, &roots).is_err());
        assert!(!old.join("game.pok.target").exists());
    }
}
