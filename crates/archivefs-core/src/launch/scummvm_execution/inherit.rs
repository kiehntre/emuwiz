//! Which of the user's own global ScummVM settings a private trainer launch may
//! keep. An ordinary launch uses the whole configuration; a trainer launch runs
//! on a private copy, so anything not named here is dropped rather than copied
//! on trust. Values are checked one by one and every refusal is reported.

use std::fs;
use std::path::{Component, Path};

/// Settings whose values are plain tokens (numbers, booleans, driver names).
const PLAIN_KEYS: &[&str] = &[
    "music_driver",
    "output_rate",
    "midi_gain",
    "music_volume",
    "sfx_volume",
    "speech_volume",
    "mute",
    "music_mute",
    "sfx_mute",
    "speech_mute",
    "subtitles",
    "talkspeed",
    "gfx_mode",
    "fullscreen",
    "aspect_ratio",
    "filtering",
    "vsync",
    "opl_driver",
    "multi_midi",
    "theme",
];

/// Settings whose values name a file or directory ScummVM will read.
const FILE_KEYS: &[&str] = &["soundfont"];
const DIRECTORY_KEYS: &[&str] = &["themepath", "extrapath"];

const MAX_VALUE_LEN: usize = 4096;
const MAX_REPORTED_SKIPS: usize = 32;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Inherited {
    /// `key=value` lines safe to write into the private `[scummvm]` section.
    pub settings: Vec<(String, String)>,
    /// `(key, reason)` for every setting that was present but not carried over.
    pub skipped: Vec<(String, String)>,
}

fn plain_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+'))
}

/// A path ScummVM may be handed: absolute, free of `..` and control
/// characters, and with no symlink in any component, so what is checked here is
/// what ScummVM opens.
fn plain_real_path(value: &str, directory: bool) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty() || value.len() > MAX_VALUE_LEN || value.chars().any(char::is_control) {
        return Err("the value is empty, too long or has control characters".into());
    }
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return Err("the path is not a plain absolute path".into());
    }
    let mut walked = std::path::PathBuf::new();
    for component in path.components() {
        walked.push(component);
        let metadata = fs::symlink_metadata(&walked)
            .map_err(|e| format!("{} is unavailable: {e}", walked.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("{} is a symbolic link", walked.display()));
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    match (directory, metadata.is_dir(), metadata.is_file()) {
        (true, true, _) | (false, _, true) => Ok(()),
        (true, ..) => Err("the path is not a directory".into()),
        (false, ..) => Err("the path is not a regular file".into()),
    }
}

/// Reads the global `[scummvm]` section of `configuration` text and returns the
/// settings that may be inherited. `savepath` is deliberately never inherited
/// here: the trainer launch resolves and validates it separately, and a second
/// source for it could conflict.
pub fn inherit_global_settings(configuration: &str) -> Inherited {
    let mut result = Inherited::default();
    let mut in_global = false;
    let mut seen = std::collections::BTreeSet::new();
    for line in configuration.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_global = name == "scummvm";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_global) else {
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        if key == "savepath" {
            continue;
        }
        // ScummVM keeps the last value of a repeated key, so a repeated key is
        // dropped entirely rather than resolved here in a way that could differ.
        let verdict = if !seen.insert(key.clone()) {
            result.settings.retain(|(existing, _)| existing != &key);
            Err("the setting appears more than once".to_string())
        } else if PLAIN_KEYS.contains(&key.as_str()) {
            if plain_token(value) {
                Ok(())
            } else {
                Err("the value is not a plain token".into())
            }
        } else if FILE_KEYS.contains(&key.as_str()) {
            plain_real_path(value, false)
        } else if DIRECTORY_KEYS.contains(&key.as_str()) {
            plain_real_path(value, true)
        } else {
            Err("not a setting EmuWiz carries into a trainer launch".into())
        };
        match verdict {
            Ok(()) => result.settings.push((key, value.to_string())),
            Err(reason) if result.skipped.len() < MAX_REPORTED_SKIPS => {
                result.skipped.push((key, reason))
            }
            Err(_) => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_plain_audio_settings_and_drops_everything_else() {
        let found = inherit_global_settings(
            "[scummvm]\nmusic_driver=fluidsynth\noutput_rate=44100\nsavepath=/saves\nbrowser_lastpath=/x\n[other]\nmute=true\n",
        );
        assert_eq!(
            found.settings,
            vec![
                ("music_driver".into(), "fluidsynth".into()),
                ("output_rate".into(), "44100".into())
            ]
        );
        assert_eq!(found.skipped.len(), 1);
        assert_eq!(found.skipped[0].0, "browser_lastpath");
    }

    #[test]
    fn rejects_unsafe_values_and_duplicates() {
        let found = inherit_global_settings(
            "[scummvm]\nmusic_driver=a b;rm\nmute=true\nmute=false\ntheme=../evil\n",
        );
        assert!(
            found.settings.is_empty(),
            "a repeated key is dropped entirely"
        );
        let keys: Vec<_> = found.skipped.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"music_driver"));
        assert!(keys.contains(&"theme"));
        assert!(
            found
                .skipped
                .iter()
                .any(|(_, r)| r.contains("more than once"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn soundfont_and_theme_paths_must_be_real_and_unlinked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let font = root.join("a.sf2");
        fs::write(&font, b"x").unwrap();
        let themes = root.join("themes");
        fs::create_dir(&themes).unwrap();
        let link = root.join("link.sf2");
        std::os::unix::fs::symlink(&font, &link).unwrap();
        let text = format!(
            "[scummvm]\nsoundfont={}\nthemepath={}\nextrapath={}\n",
            font.display(),
            themes.display(),
            root.join("missing").display()
        );
        let found = inherit_global_settings(&text);
        assert_eq!(found.settings.len(), 2);
        assert_eq!(found.skipped[0].0, "extrapath");
        let linked = inherit_global_settings(&format!("[scummvm]\nsoundfont={}\n", link.display()));
        assert!(linked.settings.is_empty());
        assert!(linked.skipped[0].1.contains("symbolic link"));
        let wrong = inherit_global_settings(&format!("[scummvm]\nthemepath={}\n", font.display()));
        assert!(wrong.settings.is_empty());
        let relative = inherit_global_settings("[scummvm]\nsoundfont=rel/a.sf2\n");
        assert!(relative.settings.is_empty());
    }
}
