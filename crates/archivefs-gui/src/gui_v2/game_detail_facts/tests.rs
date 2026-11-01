use super::*;
use crate::{
    gui_v2::launch_readiness_summary::{self, EmulatorSummary},
    multidisc_launch_view::{DiscGuidance, DiscRow},
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

    let mut refused = project(&LaunchReadinessInput::IdentityUnknown);
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

#[test]
fn a_file_name_never_becomes_a_region() {
    let game = game("/roms/Fixture (USA).iso", "iso");
    let checking = fact(&facts(&game, Sources::default()), "Region");
    assert_eq!(checking.value, CHECKING);
    let checked = Sources {
        region_checked: true,
        ..Sources::default()
    };
    assert_eq!(fact(&facts(&game, checked), "Region").value, NOT_RECORDED);
    let sourced = Sources {
        region: Some(("Europe", "ScreenScraper (saved record)")),
        region_checked: true,
        ..Sources::default()
    };
    let row = fact(&facts(&game, sourced), "Region");
    assert_eq!(row.value, "Europe");
    assert_eq!(row.note.unwrap(), "From ScreenScraper (saved record)");
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

fn guidance(discs: Vec<DiscRow>) -> DiscGuidance {
    DiscGuidance {
        headline: "3-disc game · order not verified".into(),
        order_verified: false,
        discs,
        next_discs: vec![],
        missing_media: vec![],
        conflicts: vec![],
        provenance: vec![],
        selected_launch_disc: None,
        problems: vec![],
        launch: None,
    }
}

fn row(label: &str, path: Option<&str>, this_entry: bool) -> DiscRow {
    DiscRow {
        label: label.into(),
        path: path.map(PathBuf::from),
        this_entry,
        start: false,
    }
}

#[test]
fn links_lead_only_to_other_discs_with_their_own_library_entry() {
    let library = std::sync::Arc::new(Library::new(vec![
        archive(1, "/roms/Fixture 1.iso", "iso"),
        archive(2, "/roms/Fixture 2.iso", "iso"),
    ]));
    let set = guidance(vec![
        row("Disc 1", Some("/roms/Fixture 1.iso"), true),
        row("Disc 2", Some("/roms/Fixture 2.iso"), false),
        row("Disc 3", None, false),
        row("Disc 4", Some("/roms/Not in the library.iso"), false),
    ]);
    let mut state = State::default();
    assert!(state.note_discs(1, &library, &set));
    let summary = state.discs_for(1, &library).unwrap();
    assert_eq!(summary.links, vec![("Disc 2".to_string(), 2)]);
    assert_eq!(summary.headline, set.headline);
    assert!(state.discs_for(2, &library).is_none());
    assert!(!state.note_discs(1, &library, &set));

    let reloaded =
        std::sync::Arc::new(Library::new(vec![archive(1, "/roms/Fixture 1.iso", "iso")]));
    assert!(state.discs_for(1, &reloaded).is_none());
}
