//! Read-only access to RetroArch's own content history ("History" playlist).
//!
//! RetroArch records the games it ran in `content_history.lpl`, most recent
//! first, at the path `retroarch.cfg` names in `content_history_path`. EmuWiz
//! keeps no play history of its own, so this is the only recorded "recently
//! played" evidence on the machine.
//!
//! Rules, matching the rest of this module:
//! - Nothing is written: `retroarch.cfg` and the history file are only read,
//!   through [`ReadOnlyHostFilesystem`], bounded in bytes and entries.
//! - The configured path is respected and never guessed: an unset, relative
//!   or colon-alias value is reported, not replaced with a default.
//! - The playlist is parsed by the existing [`super::read_playlist_file`], so
//!   paths, archive members and CRCs are classified exactly as everywhere
//!   else. Labels, CRCs and database names stay what RetroArch wrote; they are
//!   hints, never verified identity.
//! - The file has no timestamps or play counts, so none are produced; the
//!   order is the file's order.

use std::path::{Path, PathBuf};

use super::{
    ConfigReadOutcome, DiscoveryEnvironment, RetroArchPlaylistEntry, read_config_file,
    read_playlist_file, resolve_configured_value, resolve_xdg_config_home,
};
use crate::emulator_environment::{FsProbe, ReadOnlyHostFilesystem};

/// The most history entries returned. RetroArch's own `content_history_size`
/// defaults to 200; anything past this is reported as truncated.
pub const MAX_HISTORY_ENTRIES: usize = 500;

const CONTENT_HISTORY_PATH_KEY: &str = "content_history_path";
const HISTORY_LIST_ENABLE_KEY: &str = "history_list_enable";

/// Why the history could, or could not, be read. Only [`Self::Read`] carries
/// entries; every other state means "nothing to show", with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentHistoryState {
    /// No `retroarch.cfg` exists in the native profile.
    ConfigMissing,
    /// `retroarch.cfg` exists but could not be read (a symlink, too large,
    /// not UTF-8, inaccessible).
    ConfigUnreadable,
    /// `retroarch.cfg` does not set `content_history_path`.
    PathNotConfigured,
    /// `content_history_path` is relative or a colon alias, which EmuWiz
    /// does not resolve.
    PathUnresolved { raw: String },
    /// The configured history file does not exist (RetroArch has not
    /// recorded anything yet).
    FileMissing,
    /// The history file exists but is not a readable regular file (a
    /// symlink, a directory, inaccessible).
    FileUnreadable,
    /// The history file is larger than the playlist read limit.
    FileTooLarge,
    /// The history file is not UTF-8 JSON in the playlist format.
    FileMalformed,
    /// The file was read; `entries` may still be empty.
    Read,
}

#[derive(Debug, Clone)]
pub struct RetroArchContentHistory {
    pub config_file: PathBuf,
    /// `history_list_enable` as written in `retroarch.cfg`; `None` when the
    /// key is absent or not `true`/`false`.
    pub history_enabled: Option<bool>,
    /// The resolved configured history path, when there is one.
    pub history_file: Option<PathBuf>,
    pub state: ContentHistoryState,
    /// Most recent first, exactly as RetroArch wrote them.
    pub entries: Vec<RetroArchPlaylistEntry>,
    /// More entries existed than [`MAX_HISTORY_ENTRIES`] (or the playlist
    /// parser's own entry limit); only the first ones are in `entries`.
    pub truncated: bool,
}

/// Reads the native profile's history: `$XDG_CONFIG_HOME/retroarch/retroarch.cfg`
/// (or `~/.config/...`), the same profile EmuWiz's RetroArch launches use.
/// `None` only when `HOME` is unset.
pub fn read_native_content_history(
    filesystem: &dyn ReadOnlyHostFilesystem,
    environment: &DiscoveryEnvironment,
) -> Option<RetroArchContentHistory> {
    let home = environment
        .home
        .as_ref()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)?;
    let config_home = resolve_xdg_config_home(environment, &home, &mut Vec::new());
    let config_file = config_home.join("retroarch").join("retroarch.cfg");
    Some(read_content_history_for_config(
        filesystem,
        &config_file,
        &home,
    ))
}

/// Reads the history named by one `retroarch.cfg`. `tilde_home` resolves a
/// leading `~`, as RetroArch itself does.
pub fn read_content_history_for_config(
    filesystem: &dyn ReadOnlyHostFilesystem,
    config_file: &Path,
    tilde_home: &Path,
) -> RetroArchContentHistory {
    let mut history = RetroArchContentHistory {
        config_file: config_file.to_path_buf(),
        history_enabled: None,
        history_file: None,
        state: ContentHistoryState::ConfigMissing,
        entries: Vec::new(),
        truncated: false,
    };
    let (probe, outcome, parsed) = read_config_file(
        filesystem,
        config_file,
        super::MAX_CONFIG_BYTES,
        &[CONTENT_HISTORY_PATH_KEY, HISTORY_LIST_ENABLE_KEY],
    );
    let Some(parsed) = parsed.filter(|_| matches!(outcome, ConfigReadOutcome::Parsed { .. }))
    else {
        history.state = if probe == FsProbe::Missing {
            ContentHistoryState::ConfigMissing
        } else {
            ContentHistoryState::ConfigUnreadable
        };
        return history;
    };
    history.history_enabled = match parsed
        .values
        .get(HISTORY_LIST_ENABLE_KEY)
        .map(String::as_str)
    {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let Some(raw) = parsed
        .values
        .get(CONTENT_HISTORY_PATH_KEY)
        .filter(|value| !value.is_empty())
    else {
        history.state = ContentHistoryState::PathNotConfigured;
        return history;
    };
    let Some(file) = resolve_configured_value(raw, tilde_home) else {
        history.state = ContentHistoryState::PathUnresolved { raw: raw.clone() };
        return history;
    };
    history.history_file = Some(file.clone());
    match filesystem.probe(&file) {
        FsProbe::PresentFile => {}
        FsProbe::Missing => {
            history.state = ContentHistoryState::FileMissing;
            return history;
        }
        _ => {
            history.state = ContentHistoryState::FileUnreadable;
            return history;
        }
    }
    let playlist = read_playlist_file(filesystem, &file, history_profile());
    let code = |wanted: &str| playlist.diagnostics.iter().any(|d| d.code == wanted);
    if code("playlist_too_large") {
        history.state = ContentHistoryState::FileTooLarge;
        return history;
    }
    if code("playlist_malformed_json") || code("playlist_invalid_utf8") {
        history.state = ContentHistoryState::FileMalformed;
        return history;
    }
    history.state = ContentHistoryState::Read;
    history.truncated = !playlist.complete || playlist.entries.len() > MAX_HISTORY_ENTRIES;
    history.entries = playlist.entries;
    history.entries.truncate(MAX_HISTORY_ENTRIES);
    history
}

fn history_profile() -> super::ProfileRef {
    super::ProfileRef {
        profile_kind: super::ProfileKind::Native,
        scope: super::ProfileScope::User,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_environment::HostReadOnlyFilesystem;
    use crate::emulator_environment::retroarch::{ContentPathKind, PlaylistCrc};
    use std::fs;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn read(home: &Path) -> RetroArchContentHistory {
        read_content_history_for_config(
            &HostReadOnlyFilesystem,
            &home.join(".config/retroarch/retroarch.cfg"),
            home,
        )
    }

    const TWO_ITEMS: &str = r#"{
      "version": "1.5",
      "items": [
        {"path": "/games/snes/Second.sfc", "label": "", "core_name": "Snes9x",
         "core_path": "/cores/snes9x_libretro.so", "crc32": "", "db_name": "", "entry_slot": -1},
        {"path": "/games/gb/First.zip#First.gb", "label": "First", "crc32": "a1b2c3d4|crc"}
      ]
    }"#;

    #[test]
    fn reads_the_configured_history_in_file_order_with_tilde_resolved() {
        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".config/retroarch/retroarch.cfg"),
            "history_list_enable = \"true\"\ncontent_history_path = \"~/ra/history.lpl\"\n",
        );
        write(&home.path().join("ra/history.lpl"), TWO_ITEMS);
        let history = read(home.path());
        assert_eq!(history.state, ContentHistoryState::Read);
        assert_eq!(history.history_enabled, Some(true));
        assert_eq!(
            history.history_file.as_deref(),
            Some(home.path().join("ra/history.lpl").as_path())
        );
        assert_eq!(history.entries.len(), 2);
        assert_eq!(
            history.entries[0].content_path.raw.as_deref(),
            Some("/games/snes/Second.sfc")
        );
        assert_eq!(history.entries[0].label, None, "empty label stays absent");
        assert_eq!(history.entries[0].core_name.as_deref(), Some("Snes9x"));
        assert_eq!(
            history.entries[1].content_path.kind,
            ContentPathKind::ArchiveMember
        );
        assert_eq!(
            history.entries[1].crc,
            PlaylistCrc::Verified {
                value: "A1B2C3D4".into()
            }
        );
        assert!(!history.truncated);
    }

    #[test]
    fn missing_config_file_and_path_are_states_not_errors() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(read(home.path()).state, ContentHistoryState::ConfigMissing);

        let cfg = home.path().join(".config/retroarch/retroarch.cfg");
        write(&cfg, "history_list_enable = \"false\"\n");
        let history = read(home.path());
        assert_eq!(history.state, ContentHistoryState::PathNotConfigured);
        assert_eq!(history.history_enabled, Some(false));

        write(&cfg, "content_history_path = \"\"\n");
        assert_eq!(
            read(home.path()).state,
            ContentHistoryState::PathNotConfigured
        );

        write(&cfg, "content_history_path = \":/history.lpl\"\n");
        assert_eq!(
            read(home.path()).state,
            ContentHistoryState::PathUnresolved {
                raw: ":/history.lpl".into()
            }
        );

        write(&cfg, "content_history_path = \"~/nowhere/history.lpl\"\n");
        let history = read(home.path());
        assert_eq!(history.state, ContentHistoryState::FileMissing);
        assert!(history.entries.is_empty());
    }

    #[test]
    fn empty_malformed_and_non_file_histories_show_nothing() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("history.lpl");
        write(
            &home.path().join(".config/retroarch/retroarch.cfg"),
            &format!("content_history_path = \"{}\"\n", file.display()),
        );

        write(&file, r#"{"version": "1.5", "items": []}"#);
        let history = read(home.path());
        assert_eq!(history.state, ContentHistoryState::Read);
        assert!(history.entries.is_empty());

        write(&file, "{ not json");
        assert_eq!(read(home.path()).state, ContentHistoryState::FileMalformed);

        fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read(home.path()).state, ContentHistoryState::FileMalformed);

        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        assert_eq!(read(home.path()).state, ContentHistoryState::FileUnreadable);
    }

    #[test]
    fn a_symlinked_history_file_is_not_followed() {
        let home = tempfile::tempdir().unwrap();
        let real = home.path().join("real.lpl");
        write(&real, TWO_ITEMS);
        let link = home.path().join("history.lpl");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write(
            &home.path().join(".config/retroarch/retroarch.cfg"),
            &format!("content_history_path = \"{}\"\n", link.display()),
        );
        assert_eq!(read(home.path()).state, ContentHistoryState::FileUnreadable);
    }

    #[test]
    fn long_histories_are_bounded_and_marked_truncated() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("history.lpl");
        write(
            &home.path().join(".config/retroarch/retroarch.cfg"),
            &format!("content_history_path = \"{}\"\n", file.display()),
        );
        let items: Vec<String> = (0..MAX_HISTORY_ENTRIES + 3)
            .map(|index| format!(r#"{{"path": "/games/{index}.nes"}}"#))
            .collect();
        write(&file, &format!(r#"{{"items": [{}]}}"#, items.join(",")));
        let history = read(home.path());
        assert_eq!(history.state, ContentHistoryState::Read);
        assert_eq!(history.entries.len(), MAX_HISTORY_ENTRIES);
        assert!(history.truncated);
        assert_eq!(
            history.entries[0].content_path.raw.as_deref(),
            Some("/games/0.nes")
        );
    }

    #[test]
    fn native_profile_follows_xdg_config_home() {
        let home = tempfile::tempdir().unwrap();
        let xdg = home.path().join("xdg");
        write(
            &xdg.join("retroarch/retroarch.cfg"),
            "content_history_path = \"~/h.lpl\"\n",
        );
        write(&home.path().join("h.lpl"), TWO_ITEMS);
        let environment = DiscoveryEnvironment {
            home: Some(home.path().as_os_str().to_owned()),
            xdg_config_home: Some(xdg.into_os_string()),
            path: None,
            explicit_executables: Vec::new(),
            user_flatpak_root: PathBuf::from("/nonexistent"),
            system_flatpak_root: PathBuf::from("/nonexistent"),
            app_image_search_roots: Vec::new(),
            desktop_file_roots: Vec::new(),
        };
        let history = read_native_content_history(&HostReadOnlyFilesystem, &environment).unwrap();
        assert_eq!(history.state, ContentHistoryState::Read);
        assert_eq!(history.entries.len(), 2);

        let no_home = DiscoveryEnvironment {
            home: None,
            ..environment
        };
        assert!(read_native_content_history(&HostReadOnlyFilesystem, &no_home).is_none());
    }
}
