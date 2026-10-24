use super::*;
use crate::multidisc_launch_view::DiscRow;
use archivefs_core::amiga_game_readiness::{
    AmigaMediaKind, AmigaProfileFacts, WhdloadPackageFacts, WhdloadSlaveFacts,
};
use archivefs_core::patch_manager::{AmigaEmulatorKind, AmigaKickstartState};
use std::path::Path;
use std::sync::Arc;

fn game(path: &Path, platform: &str) -> Game {
    let mut archive = super::super::tests::archive(9, "Disposable Amiga", Some(platform));
    archive.absolute_path = path.to_path_buf();
    archive.archive_kind = "direct_file".into();
    Game::from_archive(archive)
}

fn visible_text(output: &egui::FullOutput) -> String {
    fn collect(shape: &egui::epaint::Shape, text: &mut String) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                text.push_str(t.galley.text());
                text.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, text)),
            _ => {}
        }
    }
    let mut text = String::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut text);
    }
    text
}

fn rendered(readiness: &AmigaGameReadiness) -> (String, Option<AmigaFixRoute>) {
    let context = egui::Context::default();
    let mut fix = None;
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            fix = render(ui, readiness, false).0;
        });
    });
    (visible_text(&output), fix)
}

fn fsuae(model: &str, kickstart: AmigaKickstartState) -> AmigaProfileFacts {
    AmigaProfileFacts {
        emulator: AmigaEmulatorKind::FsUae,
        profile_id: "fs-uae".into(),
        eligible: true,
        executable_found: true,
        kickstart,
        machine_model: Some(model.into()),
        chipset: None,
        whdload_support_present: true,
    }
}

fn facts(profiles: Vec<AmigaProfileFacts>) -> AmigaEnvironmentFacts {
    AmigaEnvironmentFacts {
        profiles,
        discovery_available: true,
        whdload: None,
        adf_identity: None,
    }
}

#[test]
fn only_amiga_family_platforms_get_the_card() {
    let path = Path::new("/games/x.adf");
    assert_eq!(
        machine(&game(path, "Amiga")),
        Some(AmigaCdMachine::OrdinaryAmiga)
    );
    assert_eq!(
        machine(&game(Path::new("x.cue"), "AmigaCD32")),
        Some(AmigaCdMachine::Cd32)
    );
    assert_eq!(machine(&game(Path::new("x.iso"), "PSP")), None);
    assert_eq!(AmigaMediaKind::from_path(path), AmigaMediaKind::Adf);
}

#[test]
fn headline_shows_machine_format_and_disk_count() {
    let single = assess_amiga_game(
        AmigaCdMachine::OrdinaryAmiga,
        Path::new("a.adf"),
        false,
        None,
        None,
    );
    assert_eq!(headline(&single), "Amiga · ADF floppy image · 1 disk");
    let guidance = DiscGuidance {
        headline: "3-disk game · order verified".into(),
        order_verified: true,
        discs: (1..=3)
            .map(|n| DiscRow {
                label: format!("Disk {n}"),
                path: Some(PathBuf::from(format!("d{n}.adf"))),
                this_entry: n == 1,
                start: n == 1,
            })
            .collect(),
        next_discs: vec!["Disk 2".into(), "Disk 3".into()],
        missing_media: Vec::new(),
        conflicts: Vec::new(),
        provenance: Vec::new(),
        selected_launch_disc: None,
        problems: Vec::new(),
        launch: None,
    };
    let set = assess_amiga_game(
        AmigaCdMachine::OrdinaryAmiga,
        Path::new("d1.adf"),
        false,
        disk_facts(Some(&guidance)),
        None,
    );
    assert_eq!(headline(&set), "Amiga · ADF floppy image · 3 disks");
    let cd = assess_amiga_game(AmigaCdMachine::Cd32, Path::new("g.cue"), false, None, None);
    assert_eq!(headline(&cd), "Amiga CD32 · CD image");
}

#[test]
fn missing_firmware_card_links_to_existing_pages_and_offers_no_play() {
    let readiness = assess_amiga_game(
        AmigaCdMachine::OrdinaryAmiga,
        Path::new("a.adf"),
        false,
        None,
        Some(&facts(vec![fsuae("A500", AmigaKickstartState::Missing)])),
    );
    let (text, fix) = rendered(&readiness);
    assert!(fix.is_none());
    assert!(text.contains("Amiga game"), "{text}");
    assert!(text.contains("Kickstart"));
    assert!(text.contains("Check firmware"));
    assert!(text.contains("Open Mods & Cheats"));
    assert!(text.contains("Not available"));
    // The card's Play row is a status, never a button label of its own.
    assert!(!text.lines().any(|line| line.trim() == "Play now"));
}

#[test]
fn wrong_machine_and_missing_emulator_route_to_emulator_setup() {
    let readiness = assess_amiga_game(
        AmigaCdMachine::OrdinaryAmiga,
        Path::new("a.adf"),
        false,
        None,
        Some(&facts(vec![fsuae(
            "CD32",
            AmigaKickstartState::PresentUnverified,
        )])),
    );
    let (text, _) = rendered(&readiness);
    assert!(text.contains("Wrong machine"), "{text}");
    assert!(text.contains("Set up emulator"));
    assert_eq!(
        route(AmigaFixRoute::EmulatorSetup, 9),
        Route::Section(Section::Emulators)
    );
    assert_eq!(
        route(AmigaFixRoute::Firmware, 9),
        Route::Section(Section::Firmware)
    );
    assert_eq!(
        route(AmigaFixRoute::MultiDisk, 9),
        Route::Section(Section::MultiDisc)
    );
    assert_eq!(
        route(AmigaFixRoute::Trainers, 9),
        Route::Task {
            section: Section::Mods,
            game: 9
        }
    );
}

#[test]
fn whdload_trainers_are_never_described_as_active_at_launch() {
    let mut environment = facts(vec![fsuae("A1200", AmigaKickstartState::PresentUnverified)]);
    environment.whdload = Some(WhdloadPackageFacts::Slaves(vec![WhdloadSlaveFacts {
        member_path: "Game/Game.Slave".into(),
        requires_aga: true,
        requires_68020: true,
    }]));
    let readiness = assess_amiga_game(
        AmigaCdMachine::OrdinaryAmiga,
        Path::new("Game.lha"),
        false,
        None,
        Some(&environment),
    );
    let (text, _) = rendered(&readiness);
    assert!(text.contains("Slave Game/Game.Slave found"), "{text}");
    assert!(text.contains("AGA chipset"));
    assert!(text.contains("not passed to FS-UAE or Amiberry"));
    assert!(text.contains("launch card"));
}

#[test]
fn blue_game_details_shows_the_card_at_1024_wide_without_changing_the_game() {
    use super::super::{library::Library, media_sources::MediaIndex};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Disposable.adf");
    std::fs::write(&path, vec![0u8; 901_120]).unwrap();
    let before = std::fs::read(&path).unwrap();
    let context = egui::Context::default();
    let mut app = super::super::tests::fixture(&context);
    app.simple = super::super::simple_shell::ShellState::default();
    let mut archive = super::super::tests::archive(91, "Disposable Amiga", Some("Amiga"));
    archive.absolute_path = path.clone();
    archive.archive_kind = "direct_file".into();
    let record = archive.clone();
    app.library = Arc::new(Library::new(vec![archive]));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Game(91);
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 3000.0),
            )),
            ..Default::default()
        },
        |ctx| app.show(ctx),
    );
    let text = visible_text(&output);
    assert!(text.contains("Amiga game"), "{text}");
    assert!(text.contains("Amiga · ADF floppy image · 1 disk"));
    assert!(text.contains("Play"));
    assert_eq!(app.router.current, Route::Game(91));
    assert_eq!(app.library.game(91).unwrap().archive, record);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
