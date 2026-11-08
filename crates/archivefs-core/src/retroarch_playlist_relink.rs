//! Re-points RetroArch playlist, history and favourites entries at games that
//! EmuWiz moved. Only the JSON playlist format (RetroArch 1.7.6+: an object
//! with an `items` array) is rewritten, and only the `path` of items whose
//! game file (before any `#member` suffix) is one of the moved files. Every
//! other byte of meaning is kept: other fields, other items, order.
//!
//! Before a file changes, a copy is kept beside it
//! (`<name>.emuwiz-before-<unix>`, created exclusively, never overwritten) and
//! the new text replaces the file atomically. Legacy six-line playlists and
//! unreadable files are reported, never rewritten. Nothing is written while
//! RetroArch is running, because it rewrites its playlists on exit.
//! Saves need no change: RetroArch keys them by the content file name, which a
//! move never changes.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RelinkReport {
    /// Playlist files that were rewritten, with the backup kept for each.
    pub rewritten: Vec<(PathBuf, PathBuf)>,
    /// Items re-pointed across all files.
    pub items: usize,
    /// Files left untouched, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
    /// Set when nothing was written because RetroArch is open.
    pub retroarch_running: bool,
}

/// The playlist folders of the native and Flatpak installs under `home`.
pub fn playlist_dirs(home: &Path) -> Vec<PathBuf> {
    [
        ".config/retroarch/playlists",
        ".config/retroarch",
        ".var/app/org.libretro.RetroArch/config/retroarch/playlists",
        ".var/app/org.libretro.RetroArch/config/retroarch",
    ]
    .iter()
    .map(|relative| home.join(relative))
    .filter(|dir| dir.is_dir())
    .collect()
}

/// Whether a process named `retroarch` is running (read-only `/proc` scan).
pub fn retroarch_is_running() -> bool {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .any(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| comm.trim().eq_ignore_ascii_case("retroarch"))
        })
}

/// Rewrites one playlist's text. `None` when nothing in it refers to a moved
/// game; `Err` when it is not a JSON playlist.
pub fn relink_text(
    text: &str,
    moves: &BTreeMap<PathBuf, PathBuf>,
) -> Result<Option<(String, usize)>, String> {
    let mut value: Value = serde_json::from_str(text)
        .map_err(|_| "not a JSON playlist (legacy format)".to_string())?;
    let Some(items) = value.get_mut("items").and_then(Value::as_array_mut) else {
        return Err("JSON without an items list".into());
    };
    let mut changed = 0;
    for item in items {
        let Some(path) = item.get("path").and_then(Value::as_str) else {
            continue;
        };
        let (file, member) = match path.split_once('#') {
            Some((file, member)) => (file, Some(member)),
            None => (path, None),
        };
        if let Some(new) = moves.get(Path::new(file)) {
            let mut new_path = new.to_string_lossy().into_owned();
            if let Some(member) = member {
                new_path.push('#');
                new_path.push_str(member);
            }
            item["path"] = Value::String(new_path);
            changed += 1;
        }
    }
    if changed == 0 {
        return Ok(None);
    }
    let mut out = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(Some((out, changed)))
}

/// Re-points every playlist in `dirs` (non-recursive `*.lpl`, regular files
/// only) at the moved games.
pub fn relink_playlists(dirs: &[PathBuf], moves: &BTreeMap<PathBuf, PathBuf>) -> RelinkReport {
    relink_playlists_with(dirs, moves, retroarch_is_running())
}

pub fn relink_playlists_with(
    dirs: &[PathBuf],
    moves: &BTreeMap<PathBuf, PathBuf>,
    running: bool,
) -> RelinkReport {
    let mut report = RelinkReport::default();
    if moves.is_empty() {
        return report;
    }
    if running {
        report.retroarch_running = true;
        return report;
    }
    let stamp = crate::dat::sources::now_unix();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lpl")))
            .collect();
        files.sort();
        for file in files {
            let regular = std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_file());
            if !regular {
                report.skipped.push((file, "not a regular file".into()));
                continue;
            }
            let text = match std::fs::read_to_string(&file) {
                Ok(text) => text,
                Err(error) => {
                    report.skipped.push((file, format!("unreadable: {error}")));
                    continue;
                }
            };
            match relink_text(&text, moves) {
                Ok(None) => {}
                Ok(Some((new_text, count))) => {
                    let name = file
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let backup = file.with_file_name(format!("{name}.emuwiz-before-{stamp}"));
                    let kept = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&backup)
                        .and_then(|mut out| {
                            use std::io::Write;
                            out.write_all(text.as_bytes())?;
                            out.sync_all()
                        });
                    if let Err(error) = kept {
                        report
                            .skipped
                            .push((file, format!("could not keep a backup: {error}")));
                        continue;
                    }
                    match crate::atomic_write_text(&file, &new_text) {
                        Ok(()) => {
                            report.items += count;
                            report.rewritten.push((file, backup));
                        }
                        Err(error) => report
                            .skipped
                            .push((file, format!("not rewritten: {error}"))),
                    }
                }
                Err(reason) => {
                    // Only worth reporting when the file mentions a moved game.
                    if moves
                        .keys()
                        .any(|old| text.contains(&*old.to_string_lossy()))
                    {
                        report.skipped.push((file, reason));
                    }
                }
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moves() -> BTreeMap<PathBuf, PathBuf> {
        BTreeMap::from([(
            PathBuf::from("/games/snes/Game (USA).zip"),
            PathBuf::from("/games/nes/Game (USA).zip"),
        )])
    }

    #[test]
    fn json_items_are_repointed_with_archive_members_and_other_fields_kept() {
        let text = r#"{"version":"1.5","items":[
            {"path":"/games/snes/Game (USA).zip#Game (USA).unh","label":"Game","core_path":"DETECT","crc32":"00|crc","db_name":"x.lpl"},
            {"path":"/games/snes/Other.zip","label":"Other"}]}"#;
        let (out, n) = relink_text(text, &moves()).unwrap().unwrap();
        assert_eq!(n, 1);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["items"][0]["path"],
            "/games/nes/Game (USA).zip#Game (USA).unh"
        );
        assert_eq!(v["items"][0]["label"], "Game");
        assert_eq!(v["items"][0]["crc32"], "00|crc");
        assert_eq!(v["items"][1]["path"], "/games/snes/Other.zip");
        assert_eq!(v["version"], "1.5");
    }

    #[test]
    fn unrelated_playlists_are_untouched_and_legacy_format_is_refused() {
        assert!(
            relink_text(r#"{"items":[{"path":"/x.zip"}]}"#, &moves())
                .unwrap()
                .is_none()
        );
        assert!(relink_text("/games/snes/Game (USA).zip\nGame\nDETECT\n", &moves()).is_err());
    }

    #[test]
    fn files_are_backed_up_then_replaced_and_nothing_is_written_while_running() {
        let dir = tempfile::tempdir().unwrap();
        let lpl = dir.path().join("Nintendo - SNES.lpl");
        let original = r#"{"items":[{"path":"/games/snes/Game (USA).zip","label":"G"}]}"#;
        std::fs::write(&lpl, original).unwrap();
        let legacy = dir.path().join("old.lpl");
        std::fs::write(&legacy, "/games/snes/Game (USA).zip\nG\n").unwrap();
        let dirs = vec![dir.path().to_path_buf()];

        let held = relink_playlists_with(&dirs, &moves(), true);
        assert!(held.retroarch_running);
        assert_eq!(std::fs::read_to_string(&lpl).unwrap(), original);

        let report = relink_playlists_with(&dirs, &moves(), false);
        assert_eq!(report.items, 1);
        let (file, backup) = &report.rewritten[0];
        assert_eq!(file, &lpl);
        assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
        assert!(
            std::fs::read_to_string(&lpl)
                .unwrap()
                .contains("/games/nes/Game (USA).zip")
        );
        assert!(
            report
                .skipped
                .iter()
                .any(|(p, why)| p == &legacy && why.contains("legacy"))
        );
        assert_eq!(
            std::fs::read_to_string(&legacy).unwrap(),
            "/games/snes/Game (USA).zip\nG\n"
        );
    }
}
