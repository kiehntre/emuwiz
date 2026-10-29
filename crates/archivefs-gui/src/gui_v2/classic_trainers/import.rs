//! Bounded local ZX .pok import. A copy beside the game is only a candidate;
//! the separate target pin and current DAT identity establish association.
use std::io::{Read, Write};
use std::path::Path;

use archivefs_core::classic_trainer_evidence::TRAINER_FILE_MAX_BYTES;
use archivefs_core::open_retro_cheat_providers::parse_zx_pok;
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
            match import(game, Path::new(self.source.trim())) {
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

fn import(game: &Path, source: &Path) -> Result<usize, String> {
    if !source
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pok"))
    {
        return Err("Select a local .pok file.".into());
    }
    let target = game.with_extension("pok");
    if target.symlink_metadata().is_ok() || game.with_extension("POK").symlink_metadata().is_ok() {
        return Err("A same-name trainer already exists; import never replaces it.".into());
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
    let text = std::str::from_utf8(&bytes).map_err(|_| "Source is not UTF-8 .pok text.")?;
    let trainers = parse_zx_pok(text).map_err(|e| format!("Invalid Spectrum .pok: {e:?}"))?;
    let parent = target.parent().ok_or("Game has no parent folder.")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    staged
        .write_all(&bytes)
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    staged
        .persist_noclobber(&target)
        .map_err(|e| format!("Could not publish candidate without replacing a file: {e}"))?;
    Ok(trainers.len())
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
        assert_eq!(import(&game, &source).unwrap(), 1);
        assert_eq!(std::fs::read(&game).unwrap(), b"original media");
        assert!(import(&game, &source).is_err());
    }
    #[test]
    fn malformed_and_oversized_imports_leave_no_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game.z80");
        let source = dir.path().join("source.pok");
        std::fs::write(&source, "N Empty\nY\n").unwrap();
        assert!(import(&game, &source).is_err());
        std::fs::write(&source, vec![b'X'; 256 * 1024 + 1]).unwrap();
        assert!(import(&game, &source).is_err());
        assert!(!game.with_extension("pok").exists());
    }
}
