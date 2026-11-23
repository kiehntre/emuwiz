//! Synthetic cards in temporary folders only.

use super::*;
use std::fs;
use std::time::{Duration, Instant};

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

struct Scene {
    _dir: tempfile::TempDir,
    game: PathBuf,
    empty: PathBuf,
    folders: Folders,
    state: State,
    ctx: egui::Context,
}

fn scene() -> Scene {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    fs::create_dir(&cards).unwrap();
    let game = cards.join("game.mcd");
    let empty = cards.join("empty.mcd");
    fs::write(&game, ps1_card(b"BASLUS-00067SAVE")).unwrap();
    fs::write(&empty, ps1_card(b"")).unwrap();
    // Same size as a PS1 card, but another system's save: must not be listed.
    fs::write(cards.join("gba.srm"), vec![0xFFu8; 131_072]).unwrap();
    let folders = Folders {
        backups: dir.path().join("backups"),
        exports: dir.path().join("exports"),
        copies: dir.path().join("copies"),
    };
    Scene {
        state: State::for_folder(cards, folders.clone()),
        _dir: dir,
        game,
        empty,
        folders,
        ctx: egui::Context::default(),
    }
}

fn wait(state: &mut State) {
    let started = Instant::now();
    while state.job.is_some() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "worker did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
        state.poll();
    }
}

fn done(state: &State) -> &str {
    match &state.message {
        Some((true, text)) => text,
        other => panic!("expected a success message, got {other:?}"),
    }
}

#[test]
fn page_finds_reads_backs_up_exports_and_copies_without_changing_a_card() {
    let mut scene = scene();
    let (game_before, empty_before) = (
        fs::read(&scene.game).unwrap(),
        fs::read(&scene.empty).unwrap(),
    );
    let state = &mut scene.state;

    state.refresh(&scene.ctx);
    wait(state);
    let found: Vec<_> = state
        .cards
        .iter()
        .map(|card| {
            card.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(found, ["empty.mcd", "game.mcd"]);

    state.open(&scene.ctx, scene.game.clone());
    wait(state);
    let report = state.report.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(report.health, CardHealth::Healthy, "{:?}", report.notes);
    assert_eq!(report.saves[0].title.as_deref(), Some("Test Save"));
    assert_eq!(report.saves[0].product_code.as_deref(), Some("SLUS-00067"));

    state.back_up(&scene.ctx);
    wait(state);
    assert!(done(state).contains("Backup saved and verified"));
    assert_eq!(state.backups.len(), 1);
    assert_eq!(
        fs::read(&state.backups[0].backup_file).unwrap(),
        game_before
    );
    let record = state.backups[0].record_file();
    state.check(&scene.ctx, record.clone());
    wait(state);
    assert!(state.checks[&record].contains("intact"));

    state.export(&scene.ctx, "BASLUS-00067SAVE".into(), "mcs");
    wait(state);
    let exported = scene.folders.exports.join("BASLUS-00067SAVE.mcs");
    assert!(done(state).contains("Exported BASLUS-00067SAVE"));
    assert_eq!(fs::metadata(&exported).unwrap().len(), 128 + 8192);
    // A second export never overwrites the first.
    state.export(&scene.ctx, "BASLUS-00067SAVE".into(), "mcs");
    wait(state);
    assert!(
        scene
            .folders
            .exports
            .join("BASLUS-00067SAVE-2.mcs")
            .exists()
    );

    state.open(&scene.ctx, scene.empty.clone());
    wait(state);
    state.import_file = exported.display().to_string();
    state.import(&scene.ctx);
    wait(state);
    assert!(done(state).contains("The card you opened is unchanged"));
    let new_card = scene.folders.copies.join("empty-with-BASLUS-00067SAVE.mcd");
    let copy = manager::read_card(&new_card).unwrap();
    assert_eq!(copy.health, CardHealth::Healthy, "{:?}", copy.notes);
    assert_eq!(copy.saves[0].name, "BASLUS-00067SAVE");

    assert_eq!(fs::read(&scene.game).unwrap(), game_before);
    assert_eq!(fs::read(&scene.empty).unwrap(), empty_before);

    // A file that is not a save is refused in plain words; nothing is written.
    state.import_file = scene.game.display().to_string();
    state.import(&scene.ctx);
    wait(state);
    assert!(matches!(&state.message, Some((false, text)) if text.contains("No new card was made")));
    assert_eq!(fs::read_dir(&scene.folders.copies).unwrap().count(), 1);
}

#[test]
fn page_shows_the_card_its_saves_and_the_actions() {
    let mut scene = scene();
    scene.state.refresh(&scene.ctx);
    wait(&mut scene.state);
    scene.state.open(&scene.ctx, scene.game.clone());
    wait(&mut scene.state);
    scene.state.started = true;

    let output = scene.ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 1600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut scene.state));
        },
    );
    fn text(shape: &egui::epaint::Shape, output: &mut String) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                output.push_str(text.galley.text());
                output.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|shape| text(shape, output)),
            _ => {}
        }
    }
    let mut visible = String::new();
    for clipped in output.shapes {
        text(&clipped.shape, &mut visible);
    }
    for expected in [
        "PlayStation memory cards",
        "EmuWiz never changes a card shown here",
        "Cards found (2)",
        "PS1 card (plain 128 KB image)",
        "Readable: two readers agree",
        "14 of 15 blocks free",
        "Test Save",
        "SLUS-00067",
        "America",
        "BASLUS-00067SAVE",
        "Back up this card",
        "Export .mcs",
        "Add a save to a copy of this card",
        "Make the new card",
        "Other card editors",
        "EmuWiz does not include or download them",
    ] {
        assert!(
            visible.contains(expected),
            "missing {expected:?} in:\n{visible}"
        );
    }
    assert!(!visible.contains("gba.srm"));
}
