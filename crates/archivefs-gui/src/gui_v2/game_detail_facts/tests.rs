use super::*;
use crate::gui_v2::{
    launch_readiness_summary::{self, EmulatorSummary},
    library::Library,
};
use archivefs_core::PersistedArchive;
use std::path::PathBuf;

fn archive(id: i64, path: &str, kind: &str) -> PersistedArchive {
    PersistedArchive {
        id,
        source_folder_id: 1,
        relative_path: PathBuf::from(path).file_name().unwrap().into(),
        absolute_path: path.into(),
        archive_kind: kind.into(),
        display_name: "Fixture".into(),
        normalized_name: "fixture".into(),
        size_bytes: Some(1536),
        modified_time_unix_seconds: Some(1),
        platform: Some("PlayStation".into()),
        platform_source: Some("heuristic-path-detector".into()),
        last_known_health: "ok".into(),
        last_seen_at: "2026-11-01".into(),
        last_verified_missing_at: None,
        identity_report: None,
    }
}

fn game(path: &str, kind: &str) -> Game {
    Game::from_archive(archive(1, path, kind))
}

fn detail(present: bool, unchanged: bool, installed: &[&str]) -> Detail {
    Detail {
        game: 1,
        file_present: present,
        unchanged,
        installed: installed.iter().map(|name| name.to_string()).collect(),
        ..Detail::default()
    }
}

fn fact(rows: &[Fact], label: &str) -> Fact {
    rows.iter()
        .find(|row| row.label == label)
        .unwrap_or_else(|| panic!("{label} missing: {rows:?}"))
        .clone()
}

fn summary(emulator: Option<(&str, &str)>) -> GameReadinessSummary {
    let mut summary = launch_readiness_summary::not_started_summary();
    summary.status = ReadinessPresentationState::Blocked;
    summary.emulator = emulator.map(|(name, profile)| EmulatorSummary {
        name: name.into(),
        profile: profile.into(),
    });
    summary
}

#[test]
fn a_missing_file_stays_missing_whatever_the_emulator_evidence_says() {
    let game = game("/roms/Fixture.iso", "iso");
    let found = detail(false, false, &["DuckStation"]);
    let mut ready = summary(Some(("DuckStation", "Native")));
    ready.status = ReadinessPresentationState::Ready;
    let rows = facts(
        &game,
        Sources {
            detail: Some(&found),
            readiness: Some(&ready),
            ..Sources::default()
        },
    );
    let file = fact(&rows, "File");
    assert_eq!(file.value, "Not found");
    assert_eq!(file.tone, Tone::Warning);
    for row in &rows {
        let words = format!("{} {:?}", row.value, row.note).to_lowercase();
        assert!(
            !words.contains("playable") && !words.contains("ready"),
            "{row:?}"
        );
    }
}

#[test]
fn a_missing_file_replaces_play_and_checking_but_not_a_specific_refusal() {
    use crate::launch_readiness_page::LaunchReadinessInput;
    let project = |input| launch_readiness_summary::project(input, ReadinessFreshness::Current);
    let mut dos = project(&LaunchReadinessInput::DosBoxReview);
    assert_eq!(dos.primary_action, Some(ReadinessAction::Play));
    launch_readiness_summary::note_file_missing(&mut dos);
    assert_eq!(dos.status, ReadinessPresentationState::SourceUnavailable);
    assert_eq!(dos.primary_action, Some(ReadinessAction::CheckGamesFolder));

    let mut pending = project(&LaunchReadinessInput::EvidenceNotLoaded);
    launch_readiness_summary::note_file_missing(&mut pending);
    assert_eq!(
        pending.status,
        ReadinessPresentationState::SourceUnavailable
    );

    let mut refused = project(&LaunchReadinessInput::IdentityUnknown(
        crate::launch_readiness_page::IdentityUnknownReason::NotConfirmed,
    ));
    let before = refused.clone();
    launch_readiness_summary::note_file_missing(&mut refused);
    assert_eq!(refused, before);
}

#[test]
fn a_flagged_or_changed_file_is_never_shown_as_good() {
    let mut flagged = game("/roms/Fixture.iso", "iso");
    flagged.attention = true;
    flagged.archive.last_known_health = "corrupt".into();
    let present = detail(true, true, &[]);
    let row = fact(
        &facts(
            &flagged,
            Sources {
                detail: Some(&present),
                ..Sources::default()
            },
        ),
        "File",
    );
    assert_eq!(row.tone, Tone::Warning);
    assert!(row.note.unwrap().contains("corrupt"));

    let changed = detail(true, false, &[]);
    let row = fact(
        &facts(
            &game("/roms/Fixture.iso", "iso"),
            Sources {
                detail: Some(&changed),
                ..Sources::default()
            },
        ),
        "File",
    );
    assert_eq!(row.value, "Found, changed since the last scan");
    assert_eq!(row.tone, Tone::Warning);
}

#[test]
fn file_state_is_unknown_until_it_has_been_checked() {
    let game = game("/roms/Fixture.iso", "iso");
    let row = fact(&facts(&game, Sources::default()), "File");
    assert_eq!((row.value.as_str(), row.tone), (CHECKING, Tone::Plain));
    let failed = Sources {
        detail_failed: true,
        ..Sources::default()
    };
    let row = fact(&facts(&game, failed), "File");
    assert_eq!(row.value, "Could not be checked");
    assert_eq!(row.tone, Tone::Warning);
}

fn region_of(game: &Game, sources: Sources<'_>) -> Fact {
    fact(&facts(game, sources), "Region")
}

fn checked_sources(found: &Detail) -> Sources<'_> {
    Sources {
        detail: Some(found),
        region_checked: true,
        ..Sources::default()
    }
}

#[test]
fn saved_metadata_outranks_a_verified_dat_match_and_the_file_name() {
    let game = game("/roms/Fixture (USA).iso", "iso");
    let found = detail(true, true, &[]);
    let row = region_of(
        &game,
        Sources {
            region: Some(("Europe", "ScreenScraper (saved record)")),
            dat_region: Some(("Japan", "No-Intro: Fixture")),
            ..checked_sources(&found)
        },
    );
    assert_eq!(row.value, "Europe");
    assert_eq!(row.note.unwrap(), "From ScreenScraper (saved record)");
}

#[test]
fn a_verified_dat_match_outranks_the_file_name_and_is_not_called_a_hint() {
    let game = game("/roms/Fixture (USA).iso", "iso");
    let found = detail(true, true, &[]);
    let row = region_of(
        &game,
        Sources {
            dat_region: Some(("Japan", "No-Intro: Fixture")),
            dat_current: true,
            ..checked_sources(&found)
        },
    );
    assert_eq!(row.value, "Japan");
    assert_eq!(
        row.note.unwrap(),
        "From the verified match in No-Intro: Fixture"
    );
}

#[test]
fn a_saved_match_that_is_not_current_never_reads_as_verified() {
    let game = game("/roms/Fixture (NA).iso", "iso");
    let found = detail(true, true, &[]);
    let row = region_of(
        &game,
        Sources {
            dat_region: Some(("USA, Europe", "No-Intro: Fixture")),
            ..checked_sources(&found)
        },
    );
    assert_eq!(row.value, "USA, Europe");
    let note = row.note.unwrap();
    assert!(note.contains("Not confirmed as current"), "{note}");
    assert!(!note.contains("verified"), "{note}");
}

#[test]
fn a_filename_hint_says_so_in_the_value_and_the_note() {
    let game = game("/roms/Fixture (USA).iso", "iso");
    let found = detail(true, true, &[]);
    let row = region_of(&game, checked_sources(&found));
    assert_eq!(row.value, "USA (filename hint)");
    assert!(row.note.unwrap().contains("Not verified"));
    assert_eq!(row.tone, Tone::Plain);
}

#[test]
fn no_hint_appears_before_saved_evidence_has_been_read() {
    let game = game("/roms/Fixture (USA).iso", "iso");
    assert_eq!(region_of(&game, Sources::default()).value, CHECKING);
    let waiting = Sources {
        region_checked: true,
        ..Sources::default()
    };
    assert_eq!(region_of(&game, waiting).value, CHECKING);
    let failed = Sources {
        detail_failed: true,
        ..waiting
    };
    assert_eq!(region_of(&game, failed).value, "USA (filename hint)");
}

#[test]
fn unknown_regions_stay_not_recorded() {
    let found = detail(true, true, &[]);
    for path in [
        "/roms/Fixture.iso",
        "/roms/sf2.zip",
        "/roms/Fixture (Beta).iso",
    ] {
        let row = region_of(&game(path, "iso"), checked_sources(&found));
        assert_eq!(row.value, NOT_RECORDED, "{path}");
    }
}

#[test]
fn unambiguous_tags_give_a_hint() {
    for (name, wanted) in [
        ("Game (USA).iso", "USA"),
        ("Game (Europe).chd", "Europe"),
        ("Game (Japan).7z", "Japan"),
        ("Game (USA, Europe).zip", "USA, Europe"),
        ("Game (Europe) (En,Fr,De,Es).chd", "Europe"),
        ("Game (United Kingdom) (v5.12A).zip", "United Kingdom"),
        ("Game (Japan) (Rev 1) [!].iso", "Japan"),
    ] {
        assert_eq!(
            filename_region_hint(std::path::Path::new(name)).as_deref(),
            Some(wanted),
            "{name}"
        );
    }
}

#[test]
fn ambiguous_or_unrelated_text_gives_no_hint() {
    for name in [
        // Abbreviations and codes.
        "Game (U).iso",
        "Game (E).iso",
        "Game (J).iso",
        "Game (UE).iso",
        "Game (JP).iso",
        "Game (UK).iso",
        "Game [USA].iso",
        // Different spelling or case is not the tag.
        "Game (usa).iso",
        "Game (Japanese).iso",
        // Languages and part-region groups.
        "Game (En,Ja).iso",
        "Game (USA, Beta).iso",
        // Title text.
        "Escape from Japan.iso",
        "Europe Racer.iso",
        "Game (Japan Edition).iso",
        // More than one different tag, or nothing before it.
        "Game (USA) (Europe).iso",
        "Trip to (Japan) Land (USA).iso",
        "(USA) Game.iso",
        "(USA).iso",
        // Unbalanced.
        "Game (USA.iso",
    ] {
        assert_eq!(
            filename_region_hint(std::path::Path::new(name)),
            None,
            "{name}"
        );
    }
}

#[test]
fn folders_never_supply_a_hint() {
    assert_eq!(
        filename_region_hint(std::path::Path::new("/roms/Game (USA)/disc.iso")),
        None
    );
    let found = detail(true, true, &[]);
    let folder = game(
        "/roms/Game (USA)",
        ArchiveKind::ScummVmDirectory.storage_name(),
    );
    assert_eq!(
        region_of(&folder, checked_sources(&found)).value,
        NOT_RECORDED
    );
}

#[test]
fn each_disc_of_a_set_reads_its_own_name() {
    for name in ["Game (Europe) (Disc 1).chd", "Game (Disc 2) (Europe).chd"] {
        assert_eq!(
            filename_region_hint(std::path::Path::new(name)).as_deref(),
            Some("Europe"),
            "{name}"
        );
    }
    assert_eq!(
        filename_region_hint(std::path::Path::new("Game (Disc 1).chd")),
        None
    );
}

#[test]
fn size_and_format_say_only_what_is_recorded() {
    let iso = game("/roms/Fixture.iso", "iso");
    let rows = facts(&iso, Sources::default());
    assert_eq!(fact(&rows, "File format").value, "ISO · Game image");
    assert_eq!(fact(&rows, "Size").value, "1.5 KB");

    let zip = game("/roms/Fixture.zip", "zip");
    assert_eq!(
        fact(&facts(&zip, Sources::default()), "File format").value,
        "ZIP"
    );

    let mut unknown = game("/roms/Fixture", "something-new");
    unknown.archive.size_bytes = None;
    let rows = facts(&unknown, Sources::default());
    assert_eq!(fact(&rows, "File format").value, NOT_RECORDED);
    assert_eq!(fact(&rows, "Size").value, NOT_RECORDED);

    let odd = game("/roms/Fixture.a b", "something-new");
    assert_eq!(
        fact(&facts(&odd, Sources::default()), "File format").value,
        NOT_RECORDED
    );
}

#[test]
fn an_index_file_does_not_claim_the_size_or_presence_of_its_disc_data() {
    let cue = game("/roms/Fixture.cue", "iso");
    let found = detail(true, true, &[]);
    let rows = facts(
        &cue,
        Sources {
            detail: Some(&found),
            ..Sources::default()
        },
    );
    assert!(
        fact(&rows, "Size")
            .note
            .unwrap()
            .contains("index file only")
    );
    assert!(
        fact(&rows, "File")
            .note
            .unwrap()
            .contains("index file only")
    );
}

#[test]
fn a_game_folder_is_not_judged_by_the_folder_entry_itself() {
    let mut folder = game(
        "/roms/Fixture.game",
        ArchiveKind::ScummVmDirectory.storage_name(),
    );
    folder.archive.size_bytes = None;
    let found = detail(true, false, &[]);
    let rows = facts(
        &folder,
        Sources {
            detail: Some(&found),
            ..Sources::default()
        },
    );
    assert_eq!(fact(&rows, "File").value, "Folder found");
    assert_eq!(fact(&rows, "File format").value, "ScummVM game folder");
}

#[test]
fn the_system_says_how_it_was_assigned() {
    let mut game = game("/roms/Fixture.iso", "iso");
    let rows = facts(&game, Sources::default());
    assert!(fact(&rows, "System").note.unwrap().contains("location"));
    game.archive.platform_source = Some(MANUAL_PLATFORM_SOURCE.into());
    assert_eq!(
        fact(&facts(&game, Sources::default()), "System")
            .note
            .unwrap(),
        "Set by you"
    );
}

#[test]
fn the_emulator_row_only_names_what_the_launch_check_chose() {
    let game = game("/roms/Fixture.iso", "iso");
    let chosen = summary(Some(("DuckStation", "Flatpak")));
    let row = fact(
        &facts(
            &game,
            Sources {
                readiness: Some(&chosen),
                ..Sources::default()
            },
        ),
        "Emulator",
    );
    assert_eq!(row.value, "DuckStation");
    assert!(row.note.unwrap().starts_with("Flatpak"));

    let mut stale = chosen.clone();
    stale.freshness = ReadinessFreshness::Stale;
    let row = fact(
        &facts(
            &game,
            Sources {
                readiness: Some(&stale),
                ..Sources::default()
            },
        ),
        "Emulator",
    );
    assert_eq!(row.value, "Needs a recheck");

    let none = summary(None);
    let installed = detail(true, true, &["RetroArch"]);
    let row = fact(
        &facts(
            &game,
            Sources {
                readiness: Some(&none),
                detail: Some(&installed),
                ..Sources::default()
            },
        ),
        "Emulator",
    );
    assert_eq!(row.value, "None chosen for this game");
    assert_eq!(row.note.unwrap(), "On this computer: RetroArch");

    let nothing = detail(true, true, &[]);
    let row = fact(
        &facts(
            &game,
            Sources {
                readiness: Some(&none),
                detail: Some(&nothing),
                ..Sources::default()
            },
        ),
        "Emulator",
    );
    assert_eq!(row.value, "None found");

    let mut choose = summary(None);
    choose.primary_action = Some(ReadinessAction::ChooseEmulator);
    let row = fact(
        &facts(
            &game,
            Sources {
                readiness: Some(&choose),
                detail: Some(&installed),
                ..Sources::default()
            },
        ),
        "Emulator",
    );
    assert!(row.value.starts_with("Choose one"));
}

#[test]
fn preservation_only_media_has_no_emulator_row() {
    let recording = game(
        "/roms/Fixture.rzx",
        ArchiveKind::PreservationRecording.storage_name(),
    );
    let rows = facts(&recording, Sources::default());
    assert!(rows.iter().all(|row| row.label != "Emulator"));
}

fn disc_entry(id: i64, folder: &std::path::Path, name: &str) -> PersistedArchive {
    let path = folder.join(name);
    std::fs::write(&path, b"disc").unwrap();
    let mut entry = archive(id, path.to_str().unwrap(), "direct_game_image");
    entry.display_name = name.rsplit_once('.').unwrap().0.into();
    entry
}

#[test]
fn only_an_entry_that_names_a_disc_starts_a_lookup_and_only_once() {
    let library = std::sync::Arc::new(Library::new(vec![
        archive(1, "/roms/Fixture.iso", "iso"),
        {
            let mut disc = archive(2, "/roms/Fixture (Disc 1).iso", "iso");
            disc.display_name = "Fixture (Disc 1)".into();
            disc
        },
    ]));
    let mut state = State::default();
    assert!(!state.should_look_up(library.game(1).unwrap(), &library));
    assert!(state.should_look_up(library.game(2).unwrap(), &library));
    assert!(!state.should_look_up(library.game(2).unwrap(), &library));
    let reloaded = std::sync::Arc::new(Library::new(vec![{
        let mut disc = archive(2, "/roms/Fixture (Disc 1).iso", "iso");
        disc.display_name = "Fixture (Disc 1)".into();
        disc
    }]));
    assert!(state.should_look_up(reloaded.game(2).unwrap(), &reloaded));
}

#[test]
fn a_lookup_stays_in_the_folder_and_links_only_real_other_entries() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let library = std::sync::Arc::new(Library::new(vec![
        disc_entry(1, root.path(), "Fixture (Disc 1).iso"),
        disc_entry(2, root.path(), "Fixture (Disc 2).iso"),
        disc_entry(3, elsewhere.path(), "Fixture (Disc 1).iso"),
        disc_entry(4, elsewhere.path(), "Fixture (Disc 3).iso"),
    ]));
    let found = look_up(&library, 1).expect("two discs in one folder form a set");
    assert_eq!(found.links.len(), 1, "{:?}", found.links);
    assert_eq!(found.links[0].1, 2);
    assert!(found.guidance.headline.starts_with("2-disc game"));
    assert!(found.guidance.headline.contains("order not verified"));
}

#[test]
fn a_flagged_missing_neighbour_gets_no_link_and_a_lone_copy_no_set() {
    let root = tempfile::tempdir().unwrap();
    let mut gone = disc_entry(2, root.path(), "Fixture (Disc 2).iso");
    gone.last_verified_missing_at = Some("2026-11-01".into());
    let library = std::sync::Arc::new(Library::new(vec![
        disc_entry(1, root.path(), "Fixture (Disc 1).iso"),
        gone,
    ]));
    let found = look_up(&library, 1);
    assert!(found.is_none_or(|found| found.links.is_empty()));
}

#[test]
fn a_late_answer_for_another_game_is_dropped() {
    let root = tempfile::tempdir().unwrap();
    let library = std::sync::Arc::new(Library::new(vec![
        disc_entry(1, root.path(), "Fixture (Disc 1).iso"),
        disc_entry(2, root.path(), "Fixture (Disc 2).iso"),
    ]));
    let late = look_up(&library, 1).unwrap();
    let mut state = State::default();
    assert!(state.should_look_up(library.game(2).unwrap(), &library));
    state.finish(late);
    assert!(state.discs_for(1, &library).is_none());
    assert!(state.discs_for(2, &library).is_none());
}
