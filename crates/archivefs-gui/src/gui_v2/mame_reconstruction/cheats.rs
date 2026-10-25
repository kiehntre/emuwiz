//! The user's explicit, per-launch MAME cheat choice for the Playing Library.
//!
//! The user picks one native MAME cheat XML named for the machine, then ticks
//! the cheats to use. Nothing is ticked by default. Native-format parsing and
//! readiness are the core's; a cheat the core cannot launch natively is shown
//! as unsupported and cannot be ticked. Launching is the core's
//! `preflight_and_launch_mame_with_cheats`, which re-reads and re-validates the
//! file, so this choice is never trusted on its own.
use archivefs_core::{
    launch::cheat_runtime_mame::MameCheatSelection,
    patch_manager::{
        MAME_CHEAT_MAX_FILE_BYTES, MameCheatFile, MameCheatProvenance, MameCheatReadiness,
        inspect_mame_cheat, parse_mame_cheat_xml,
    },
};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CheatEntry {
    pub(super) description: String,
    pub(super) chosen: bool,
    /// Shown beside a cheat that can be chosen but needs care.
    pub(super) note: Option<&'static str>,
    /// Why EmuWiz cannot launch this cheat natively; such a cheat cannot be chosen.
    pub(super) unsupported: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CheatChoice {
    pub(super) file: PathBuf,
    pub(super) entries: Vec<CheatEntry>,
}

const OPAQUE_NOTE: &str =
    "uses MAME expressions EmuWiz cannot interpret; MAME evaluates them itself";

pub(super) const START_STATE_NOTE: &str = "MAME starts every cheat switched off: turn the ones you ticked on in MAME's Cheat menu (Tab). EmuWiz cannot tell whether a cheat works in the game. For this launch MAME sees only the cheats you ticked, from a temporary copy that is removed after MAME exits; your own cheat file and MAME settings are not changed.";

impl CheatChoice {
    /// Reads and validates the file for `machine`. Writes nothing.
    pub(super) fn load(file: &Path, machine: &str) -> Result<Self, String> {
        let expected = format!("{machine}.xml");
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name != expected {
            return Err(format!(
                "MAME finds cheats by file name. This machine needs a file named {expected}, but the chosen file is {name:?}. Nothing was changed."
            ));
        }
        let metadata = std::fs::symlink_metadata(file)
            .map_err(|e| format!("The cheat file cannot be read: {e}"))?;
        if !metadata.is_file() {
            return Err("The cheat file must be a regular file, not a link or folder.".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(file)
            .and_then(|f| {
                f.take(MAME_CHEAT_MAX_FILE_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(|e| format!("The cheat file cannot be read: {e}"))?;
        let parsed = parse_mame_cheat_xml(&bytes, machine, MameCheatProvenance::LocalImport)
            .map_err(|e| format!("This is not a usable MAME cheat file: {e}"))?;
        if parsed.cheats.is_empty() {
            return Err("The file contains no cheats.".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        if let Some(duplicate) = parsed
            .cheats
            .iter()
            .find(|c| !seen.insert(c.description.as_str()))
        {
            return Err(format!(
                "Two cheats are both called {:?}. MAME picks by name, so EmuWiz cannot launch exactly the one you tick. Nothing was changed.",
                duplicate.description
            ));
        }
        let entries = parsed
            .cheats
            .iter()
            .map(|cheat| {
                let single = MameCheatFile {
                    cheats: vec![cheat.clone()],
                    ..parsed.clone()
                };
                let (unsupported, note) = match inspect_mame_cheat(&single, machine).readiness {
                    MameCheatReadiness::ReadyNative => (None, None),
                    MameCheatReadiness::ReadyWithOpaqueNativeOps => (None, Some(OPAQUE_NOTE)),
                    other => (Some(format!("{other:?}")), None),
                };
                CheatEntry {
                    description: cheat.description.clone(),
                    chosen: false,
                    note,
                    unsupported,
                }
            })
            .collect();
        Ok(Self {
            file: file.to_path_buf(),
            entries,
        })
    }

    pub(super) fn chosen_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.chosen && e.unsupported.is_none())
            .count()
    }

    /// The core's selection. Refuses when nothing is ticked.
    pub(super) fn selection(&self) -> Result<MameCheatSelection, String> {
        let selected: Vec<String> = self
            .entries
            .iter()
            .filter(|e| e.chosen && e.unsupported.is_none())
            .map(|e| e.description.clone())
            .collect();
        if selected.is_empty() {
            return Err(
                "Tick at least one cheat, or clear the cheat file to launch without cheats.".into(),
            );
        }
        Ok(MameCheatSelection {
            staged_file: self.file.clone(),
            selected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0"?><mamecheat version="1"><cheat desc="Free Play"><script state="on"><action>0x2000 = 1</action></script></cheat><cheat desc="Other"><script state="on"><action>0x2001 = 1</action></script></cheat></mamecheat>"#;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn nothing_is_ticked_until_the_user_ticks_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "clone.xml", XML);
        let mut choice = CheatChoice::load(&file, "clone").unwrap();
        assert_eq!(choice.entries.len(), 2);
        assert_eq!(choice.chosen_count(), 0);
        assert!(
            choice
                .selection()
                .unwrap_err()
                .contains("Tick at least one")
        );
        choice.entries[1].chosen = true;
        let selection = choice.selection().unwrap();
        assert_eq!(selection.selected, vec!["Other".to_string()]);
        assert_eq!(selection.staged_file, file);
    }

    #[test]
    fn a_file_named_for_another_machine_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "parent.xml", XML);
        let error = CheatChoice::load(&file, "clone").unwrap_err();
        assert!(error.contains("clone.xml"), "{error}");
    }

    #[test]
    fn links_folders_and_malformed_or_empty_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = write(dir.path(), "real.xml", XML);
        let link = dir.path().join("clone.xml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(
            CheatChoice::load(&link, "clone")
                .unwrap_err()
                .contains("regular file")
        );
        let bad = tempfile::tempdir().unwrap();
        let garbage = write(bad.path(), "clone.xml", "not xml");
        assert!(
            CheatChoice::load(&garbage, "clone")
                .unwrap_err()
                .contains("not a usable")
        );
        let empty = write(bad.path(), "clone.xml", "");
        assert!(CheatChoice::load(&empty, "clone").is_err());
        assert!(CheatChoice::load(&bad.path().join("missing").join("clone.xml"), "clone").is_err());
    }

    #[test]
    fn two_cheats_with_the_same_name_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let xml = XML.replace("Other", "Free Play");
        let file = write(dir.path(), "clone.xml", &xml);
        let error = CheatChoice::load(&file, "clone").unwrap_err();
        assert!(error.contains("Free Play"), "{error}");
    }

    #[test]
    fn opaque_cheats_are_flagged_and_unsupported_ones_are_never_selected() {
        let dir = tempfile::tempdir().unwrap();
        let xml = r#"<?xml version="1.0"?><mamecheat version="1"><cheat desc="Plain"><script state="on"><action>0x2000 = 1</action></script></cheat><cheat desc="Opaque"><script state="on"><action>maincpu.pb@2000 = (maincpu.pb@2001 + 1)</action></script></cheat></mamecheat>"#;
        let file = write(dir.path(), "clone.xml", xml);
        let mut choice = CheatChoice::load(&file, "clone").unwrap();
        let note = |name: &str| {
            choice
                .entries
                .iter()
                .find(|e| e.description == name)
                .unwrap()
                .note
        };
        assert_eq!(note("Plain"), None);
        assert!(note("Opaque").is_some());
        for entry in &mut choice.entries {
            entry.chosen = true;
        }
        // Defensive: a cheat the core reports as not launchable is never selected.
        choice.entries[0].unsupported = Some("PreviewOnly".into());
        let selection = choice.selection().unwrap();
        assert!(!selection.selected.contains(&"Plain".to_string()));
        assert!(selection.selected.contains(&"Opaque".to_string()));
    }
}
