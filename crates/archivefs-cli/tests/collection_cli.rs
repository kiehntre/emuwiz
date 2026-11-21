//! `emuwiz-cli collection reconcile` against a synthetic library in a
//! throwaway profile: a folder is renamed outside EmuWiz between two scans,
//! as happens when a library is reorganised by hand, and the command must
//! explain it without changing anything.

archivefs_core::install_test_environment!();

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct World {
    _temp: tempfile::TempDir,
    root: PathBuf,
    games: PathBuf,
}

fn world() -> World {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let games = root.join("games");
    for dir in [
        "home",
        "data",
        "config",
        "cache",
        "runtime",
        "games/megadrive",
    ] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    fs::write(
        root.join("config/config.toml"),
        format!(
            "source_folders = [{:?}]\nmount_root = {:?}\nratarmount_bin = \"ratarmount\"\n",
            games.display().to_string(),
            root.join("mount").display().to_string()
        ),
    )
    .unwrap();
    World {
        _temp: temp,
        root,
        games,
    }
}

fn run(world: &World, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
        .args(args)
        .env("HOME", world.root.join("home"))
        .env("XDG_CONFIG_HOME", world.root.join("home/.config"))
        .env("XDG_DATA_HOME", world.root.join("home/.local/share"))
        .env("XDG_CACHE_HOME", world.root.join("cache"))
        .env("XDG_STATE_HOME", world.root.join("home/.local/state"))
        .env("XDG_RUNTIME_DIR", world.root.join("runtime"))
        .env("EMUWIZ_DATA_HOME", world.root.join("data"))
        .env("EMUWIZ_CONFIG_HOME", world.root.join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push((path.clone(), fs::read(&path).unwrap_or_default()));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn help_only_prints_help_and_creates_nothing() {
    let world = world();
    for args in [
        &["collection"][..],
        &["collection", "--help"],
        &["collection", "reconcile", "--help"],
    ] {
        let output = run(&world, args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            text(&output.stderr)
        );
        assert!(
            text(&output.stdout).contains("Nothing is changed."),
            "{args:?}"
        );
    }
    assert!(!world.root.join("data/library.sqlite3").exists());
    assert!(
        fs::read_dir(world.root.join("cache"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn without_a_library_it_fails_plainly_and_does_not_create_one() {
    let world = world();
    let output = run(&world, &["collection", "reconcile", "--json"]);
    assert!(!output.status.success());
    assert!(text(&output.stdout).is_empty(), "no JSON on failure");
    assert!(
        text(&output.stderr).contains("library database"),
        "{}",
        text(&output.stderr)
    );
    assert!(!world.root.join("data/library.sqlite3").exists());
    let unknown = run(&world, &["collection", "reconcile", "--delete-everything"]);
    assert!(!unknown.status.success());
    assert!(text(&unknown.stderr).contains("unknown option"));
}

#[test]
fn a_folder_renamed_between_scans_is_explained_and_nothing_is_changed() {
    let world = world();
    let game = world.games.join("megadrive/Example Test.smd");
    fs::write(&game, vec![0x5a; 20_000]).unwrap();
    // The file must be measurably older than the scan that first sees it.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let first = run(&world, &["library-scan"]);
    assert!(
        first.status.success(),
        "{}{}",
        text(&first.stdout),
        text(&first.stderr)
    );

    // The folder is renamed outside EmuWiz, then the library is scanned again.
    // Scan times are whole seconds: the second scan must not share a second
    // with the first, or the new row could not be told to be from a later scan.
    std::thread::sleep(std::time::Duration::from_secs(2));
    fs::rename(world.games.join("megadrive"), world.games.join("genesis")).unwrap();
    let second = run(&world, &["library-scan"]);
    assert!(
        second.status.success(),
        "{}{}",
        text(&second.stdout),
        text(&second.stderr)
    );

    let data_before = snapshot(&world.root.join("data"));
    let games_before = snapshot(&world.games);
    let output = run(
        &world,
        &["collection", "reconcile", "--json", "--records", "--links"],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        snapshot(&world.root.join("data")),
        data_before,
        "the data folder must not change"
    );
    assert_eq!(
        snapshot(&world.games),
        games_before,
        "the games must not change"
    );

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(
        report["measures"]["rows_in_library"], 2,
        "old row and new row: {report:#}"
    );
    let records = report["records"].as_array().unwrap();
    assert_eq!(records.len(), 1, "{report:#}");
    let record = &records[0];
    assert!(
        record["old_path"]
            .as_str()
            .unwrap()
            .ends_with("megadrive/Example Test.smd")
    );
    assert!(
        record["target"]
            .as_str()
            .unwrap()
            .ends_with("genesis/Example Test.smd")
    );
    // Same name, size and date; the file is older than the old path's last
    // sighting; and its row first appears in the later scan. That is the
    // strong tier, and never "confirmed": EmuWiz did not move it.
    let expected = if fs::metadata(world.games.join("genesis/Example Test.smd"))
        .and_then(|metadata| metadata.created())
        .is_ok()
    {
        "strong"
    } else {
        "metadata" // this filesystem does not record creation times
    };
    assert_eq!(record["class"], expected, "{record:#}");
    assert_eq!(report["measures"]["records_tied_to_a_file"], 1);
    assert_eq!(report["links"]["total"], 0);
    // Nothing is kept between runs: the cache directory stays empty.
    assert!(
        fs::read_dir(world.root.join("cache"))
            .unwrap()
            .next()
            .is_none()
    );

    // The text form says the same and ends by saying nothing was changed.
    let plain = run(&world, &["collection", "reconcile", "--records"]);
    let shown = text(&plain.stdout);
    assert!(plain.status.success());
    assert!(shown.contains("do not add them together"), "{shown}");
    assert!(
        shown.contains("Example Test.smd") && shown.ends_with("Nothing was changed.\n"),
        "{shown}"
    );
    assert_eq!(snapshot(&world.root.join("data")), data_before);
}
