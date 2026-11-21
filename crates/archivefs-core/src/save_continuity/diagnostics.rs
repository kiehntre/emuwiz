//! Read-only checks that explain a limit of snapshots before it bites.
//!
//! A snapshot copies what is on disk. Progress an emulator is still holding
//! in memory is not on disk, and no snapshot can recover it. RetroArch has a
//! setting that decides how often a game's battery save is written out; this
//! module reports it. It reads `retroarch.cfg` and changes nothing.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// How RetroArch is set to write battery saves (SRAM) to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "setting")]
pub enum BatterySaveWriting {
    /// `autosave_interval = "0"`: no periodic writing.
    PeriodicWritingOff,
    /// Written every this many seconds while a game runs.
    Every { seconds: u32 },
    /// The setting is not in the file, or its value could not be read.
    NotEstablished { why: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetroArchSaveWriting {
    /// `native` or `flatpak`.
    pub install: String,
    pub config_file: PathBuf,
    pub battery_saves: BatterySaveWriting,
    /// A separate feature: writing a *save state* automatically when a game
    /// is closed (`savestate_auto_save`). Not the battery save.
    pub save_state_on_exit: Option<bool>,
    /// What this means, in plain words. Empty when there is nothing to say.
    pub advice: Vec<String>,
}

fn setting<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().trim_matches('"'))
    })
}

/// Reads one `retroarch.cfg`. `None` when the file is not there.
pub fn retroarch_save_writing(install: &str, config_file: &Path) -> Option<RetroArchSaveWriting> {
    let text = fs::read_to_string(config_file).ok()?;
    let battery_saves = match setting(&text, "autosave_interval") {
        None => BatterySaveWriting::NotEstablished {
            why: "retroarch.cfg does not contain autosave_interval".to_string(),
        },
        Some(value) => match value.parse::<u32>() {
            Ok(0) => BatterySaveWriting::PeriodicWritingOff,
            Ok(seconds) => BatterySaveWriting::Every { seconds },
            Err(_) => BatterySaveWriting::NotEstablished {
                why: format!("autosave_interval is {value:?}, which is not a number of seconds"),
            },
        },
    };
    let save_state_on_exit = match setting(&text, "savestate_auto_save") {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let mut advice = Vec::new();
    if battery_saves == BatterySaveWriting::PeriodicWritingOff {
        advice.push(
            "Periodic writing of battery saves is off in this RetroArch (autosave_interval = 0). A game's battery-save progress may then stay only in memory until RetroArch closes normally."
                .to_string(),
        );
        advice.push(
            "If RetroArch ends abnormally before that, the progress may never have reached the disk. A snapshot copies what is on disk: it cannot recover progress that was never written."
                .to_string(),
        );
        advice.push(
            "You can turn this on in RetroArch (Settings, Saving, \"SaveRAM Autosave Interval\"). RetroArch then writes the battery save periodically while a game runs. That reduces what can be lost; it is not a guarantee for every core or game, and some keep their saves in other ways. EmuWiz does not change this setting."
                .to_string(),
        );
    }
    advice.push(
        "This reads RetroArch's main configuration file only. Settings overridden for one core or one game are not looked at, so a particular game may behave differently."
            .to_string(),
    );
    advice.push(
        "This is about battery saves only. Save states are a separate feature with their own settings, and are not what this reports."
            .to_string(),
    );
    Some(RetroArchSaveWriting {
        install: install.to_string(),
        config_file: config_file.to_path_buf(),
        battery_saves,
        save_state_on_exit,
        advice,
    })
}

/// The native and Flatpak RetroArch under one home folder, where present.
pub fn retroarch_installs(home: &Path, config: &Path) -> Vec<RetroArchSaveWriting> {
    [
        ("native", config.join("retroarch/retroarch.cfg")),
        (
            "flatpak",
            home.join(".var/app/org.libretro.RetroArch/config/retroarch/retroarch.cfg"),
        ),
    ]
    .into_iter()
    .filter_map(|(install, file)| retroarch_save_writing(install, &file))
    .collect()
}
