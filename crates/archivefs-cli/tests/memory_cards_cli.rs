//! `emuwiz-cli memory-cards` against synthetic cards in a throwaway HOME.
//! The command must never change a card, never overwrite a file, and never
//! create the library database.

archivefs_core::install_test_environment!();

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct World {
    _temp: tempfile::TempDir,
    home: PathBuf,
    cards: PathBuf,
}

fn world() -> World {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let cards = temp.path().join("cards");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cards).unwrap();
    World {
        _temp: temp,
        home,
        cards,
    }
}

fn run(world: &World, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
        .arg("memory-cards")
        .args(args)
        .env("HOME", &world.home)
        .env("XDG_CONFIG_HOME", world.home.join(".config"))
        .env("XDG_DATA_HOME", world.home.join(".local/share"))
        .env("XDG_CACHE_HOME", world.home.join(".cache"))
        .env("XDG_STATE_HOME", world.home.join(".local/state"))
        .env_remove("EMUWIZ_DATA_HOME")
        .env_remove("EMUWIZ_CONFIG_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn ok(world: &World, args: &[&str]) -> String {
    let output = run(world, args);
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A formatted PS1 card, with one one-block save when `name` is given.
fn ps1_card(name: &[u8]) -> Vec<u8> {
    fn seal(frame: &mut [u8]) {
        frame[127] = frame[..127].iter().fold(0, |sum, byte| sum ^ byte);
    }
    let mut card = vec![0u8; 131_072];
    card[..2].copy_from_slice(b"MC");
    seal(&mut card[..128]);
    for slot in 1..=15 {
        let frame = &mut card[slot * 128..(slot + 1) * 128];
        frame[0] = 0xA0;
        frame[8] = 0xFF;
        frame[9] = 0xFF;
        seal(frame);
    }
    if !name.is_empty() {
        let frame = &mut card[128..256];
        frame[0] = 0x51;
        frame[4..8].copy_from_slice(&8192u32.to_le_bytes());
        frame[10..10 + name.len()].copy_from_slice(name);
        seal(frame);
        card[8192..8196].copy_from_slice(&[b'S', b'C', 0x11, 1]);
        card[8196..8205].copy_from_slice(b"Test Save");
    }
    card
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn help_only_prints_help() {
    let world = world();
    for args in [&["--help"][..], &["import", "--help"], &["-h"], &[]] {
        let out = ok(&world, args);
        assert!(
            out.contains("emuwiz-cli memory-cards <command>"),
            "{args:?}: {out}"
        );
    }
    assert_eq!(
        fs::read_dir(&world.home).unwrap().count(),
        0,
        "help wrote nothing"
    );
}

#[test]
fn list_backup_export_and_import_never_change_a_card() {
    let world = world();
    let game = world.cards.join("game.mcd");
    let empty = world.cards.join("empty.mcd");
    let game_bytes = ps1_card(b"BASLUS-00067SAVE");
    let empty_bytes = ps1_card(b"");
    fs::write(&game, &game_bytes).unwrap();
    fs::write(&empty, &empty_bytes).unwrap();

    let listing = ok(&world, &["list", text(&game)]);
    assert!(
        listing.contains("PS1 card (plain 128 KB image)"),
        "{listing}"
    );
    assert!(listing.contains("readable (two readers agree)"));
    assert!(listing.contains("BASLUS-00067SAVE") && listing.contains("SLUS-00067"));
    assert!(listing.contains("\"Test Save\"") && listing.contains("America"));
    let json: serde_json::Value =
        serde_json::from_str(&ok(&world, &["list", text(&game), "--json"])).unwrap();
    assert_eq!(json["format"], "ps1_raw");
    assert_eq!(json["saves"][0]["product_code"], "SLUS-00067");

    // Backup into EmuWiz's own folder under the throwaway HOME, then verify it.
    let backup = ok(&world, &["backup", text(&game)]);
    assert!(backup.contains("Backup saved and verified"), "{backup}");
    let record = backup
        .lines()
        .find_map(|line| line.trim().strip_prefix("record:"))
        .map(|path| PathBuf::from(path.trim()))
        .unwrap();
    assert!(record.starts_with(&world.home), "{}", record.display());
    assert!(ok(&world, &["verify", text(&record)]).contains("intact"));

    let save = world.cards.join("save.mcs");
    ok(
        &world,
        &["export", text(&game), "BASLUS-00067SAVE", text(&save)],
    );
    assert_eq!(fs::metadata(&save).unwrap().len(), 128 + 8192);
    let again = run(
        &world,
        &["export", text(&game), "BASLUS-00067SAVE", text(&save)],
    );
    assert!(
        !again.status.success(),
        "an existing file is never overwritten"
    );

    let new_card = world.cards.join("new.mcd");
    let import = ok(
        &world,
        &["import", text(&empty), text(&save), text(&new_card)],
    );
    assert!(import.contains("was not changed"), "{import}");
    assert!(ok(&world, &["list", text(&new_card)]).contains("BASLUS-00067SAVE"));
    let refused = run(
        &world,
        &["import", text(&empty), text(&save), text(&new_card)],
    );
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("already exists"));

    assert_eq!(fs::read(&game).unwrap(), game_bytes);
    assert_eq!(fs::read(&empty).unwrap(), empty_bytes);
    // No library database was created or opened along the way.
    let mut stack = vec![world.home.clone()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                assert!(
                    !name.ends_with(".db") && !name.ends_with(".sqlite"),
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn a_file_that_is_not_a_card_is_reported_plainly() {
    let world = world();
    let gba = world.cards.join("gba.srm");
    fs::write(&gba, vec![0xFFu8; 131_072]).unwrap();
    let listing = ok(&world, &["list", text(&gba)]);
    assert!(
        listing.contains("Not a PlayStation memory card"),
        "{listing}"
    );
    let backup = run(&world, &["backup", text(&gba)]);
    assert!(!backup.status.success());
    assert!(String::from_utf8_lossy(&backup.stderr).contains("not a PlayStation memory card"));
}
