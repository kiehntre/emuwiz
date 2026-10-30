//! Bounded local ZX .pok import. A copy beside the game is only a candidate;
//! the separate target pin and current DAT identity establish association.
use std::io::Read;
use std::path::Path;

use archivefs_core::classic_trainer_evidence::{TRAINER_FILE_MAX_BYTES, import_zx_pok_candidate};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;

#[derive(Default)]
pub(super) struct Import {
    source: String,
    notice: Option<String>,
}

impl Import {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, game: &Path) -> bool {
        ui.separator();
        ui.label("Import a local Spectrum .pok as a candidate for this game. The source is untrusted; its file name and a World of Spectrum/ZXDB listing do not prove revision compatibility. The game file is untouched.");
        ui.horizontal(|ui| {
            ui.label("Local .pok path");
            ui.text_edit_singleline(&mut self.source);
        });
        let mut changed = false;
        if ui.button("Import local .pok candidate").clicked() {
            let result = archivefs_core::Config::load_default()
                .map_err(|e| e.to_string())
                .and_then(|config| {
                    import(
                        game,
                        Path::new(self.source.trim()),
                        &TrustedRoots::from_paths(config.source_folders),
                    )
                });
            match result {
                Ok(count) => {
                    self.notice = Some(format!(
                        "Imported {count} trainer entries as candidates. Review them and create an exact target pin before any launch selection."
                    ));
                    changed = true;
                }
                Err(error) => self.notice = Some(format!("Import refused: {error}")),
            }
        }
        if let Some(notice) = &self.notice {
            ui.label(notice);
        }
        changed
    }
}

fn import(game: &Path, source: &Path, roots: &TrustedRoots) -> Result<usize, String> {
    if !source
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pok"))
    {
        return Err("Select a local .pok file.".into());
    }
    let metadata = source.symlink_metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > TRAINER_FILE_MAX_BYTES {
        return Err("Source must be a regular .pok file of at most 256 KiB.".into());
    }
    let mut bytes = Vec::new();
    archivefs_core::launch::process_spawn::open_regular_file_nonblocking(source)
        .map_err(|e| e.to_string())?
        .take(TRAINER_FILE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > TRAINER_FILE_MAX_BYTES {
        return Err("Source exceeds 256 KiB.".into());
    }
    import_zx_pok_candidate(game, &bytes, roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_is_bounded_non_replacing_and_keeps_game_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game.z80");
        let source = dir.path().join("source.pok");
        std::fs::write(&game, b"original media").unwrap();
        std::fs::write(&source, "N Lives\nZ 8 40000 1 0\nY\n").unwrap();
        let roots = TrustedRoots::from_paths([dir.path()]);
        assert_eq!(import(&game, &source, &roots).unwrap(), 1);
        assert_eq!(std::fs::read(&game).unwrap(), b"original media");
        assert!(import(&game, &source, &roots).is_err());
    }
    #[test]
    fn malformed_and_oversized_imports_leave_no_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game.z80");
        let source = dir.path().join("source.pok");
        std::fs::write(&source, "N Empty\nY\n").unwrap();
        let roots = TrustedRoots::from_paths([dir.path()]);
        assert!(import(&game, &source, &roots).is_err());
        std::fs::write(&source, vec![b'X'; 256 * 1024 + 1]).unwrap();
        assert!(import(&game, &source, &roots).is_err());
        assert!(!game.with_extension("pok").exists());
    }

    #[cfg(unix)]
    #[test]
    fn linked_or_untrusted_game_folder_refuses_import_before_write() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let source = dir.path().join("source.pok");
        std::fs::write(&source, "N Lives\nZ 8 40000 1 0\nY\n").unwrap();
        let game = linked.join("game.z80");
        assert!(import(&game, &source, &TrustedRoots::from_paths([dir.path()])).is_err());
        assert!(!real.join("game.pok").exists());
        let game = real.join("other.z80");
        assert!(import(&game, &source, &TrustedRoots::none()).is_err());
        assert!(!real.join("other.pok").exists());
    }
}
