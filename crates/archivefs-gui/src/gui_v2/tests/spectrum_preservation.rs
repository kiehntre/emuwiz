//! A preservation-only RZX recording is shown in the Library with its own
//! label and status, and Play is never offered for it; a Spectrum game beside
//! it keeps its Play button. Organisation says it leaves recordings out.
use super::*;

fn recording(id: i64) -> PersistedArchive {
    let mut row = archive(id, "Manic Miner", Some("ZX Spectrum"));
    row.relative_path = "Manic Miner.rzx".into();
    row.absolute_path = "/fixture/Manic Miner.rzx".into();
    row.archive_kind = "preservation_recording".into();
    row
}

fn app_with(context: &egui::Context, blue: bool) -> App {
    let mut app = fixture(context);
    app.simple.enabled = blue;
    app.library = Arc::new(Library::new(vec![
        archive(1, "Manic Miner", Some("ZX Spectrum")),
        recording(2),
    ]));
    app.loaded = true;
    app
}

fn shown(context: &egui::Context, app: &mut App, route: Route, size: [f32; 2]) -> Vec<String> {
    app.router.current = route;
    for _ in 0..3 {
        frame(context, app, size);
    }
    text(&frame(context, app, size))
}

#[test]
fn recording_row_is_labelled_preservation_only_and_has_no_play() {
    let row = Game::from_archive(recording(2));
    assert!(row.is_preservation_only());
    assert!(
        !Game::from_archive(archive(1, "Manic Miner", Some("ZX Spectrum"))).is_preservation_only()
    );
    assert_eq!(
        row.status(),
        super::super::library::PRESERVATION_ONLY_STATUS
    );
    assert_eq!(
        media_kind_label("preservation_recording"),
        "Spectrum gameplay recording"
    );

    for blue in [true, false] {
        for size in [[1280.0, 720.0], [700.0, 520.0]] {
            let context = egui::Context::default();
            let mut app = app_with(&context, blue);

            // The game itself offers Play (at 1280x720; at 700x520 Blue
            // paints Play below the fold even for games, a known main
            // layout finding, so there the recording checks stand alone).
            if size[0] > 1000.0 {
                let game = shown(&context, &mut app, Route::BrowsePlayGame(1), size);
                assert!(
                    game.iter().any(|s| s == "Play"),
                    "blue={blue} {size:?}: the game itself lost Play"
                );
            }

            // The recording: the same panel says preservation-only, no Play.
            let rec = shown(&context, &mut app, Route::BrowsePlayGame(2), size);
            assert!(
                has(&rec, super::super::library::PRESERVATION_ONLY_STATUS),
                "blue={blue} {size:?}: recording status not shown: {rec:?}"
            );
            assert!(
                !rec.iter().any(|s| s == "Play"),
                "blue={blue} {size:?}: Play offered for a recording"
            );
        }
        let context = egui::Context::default();
        let mut app = app_with(&context, blue);
        let details = shown(&context, &mut app, Route::Game(2), [1280.0, 720.0]);
        assert!(
            has(&details, "Spectrum gameplay recording"),
            "blue={blue}: details do not name the recording: {details:?}"
        );
    }
}

#[test]
fn organisation_says_recordings_are_left_out_instead_of_omitting_them() {
    let none = Library::new(vec![archive(1, "Manic Miner", Some("ZX Spectrum"))]);
    assert_eq!(
        super::super::organisation::preservation_only_note(&none),
        None
    );
    let note = super::super::organisation::preservation_only_note(&Library::new(vec![
        archive(1, "Manic Miner", Some("ZX Spectrum")),
        recording(2),
    ]))
    .unwrap();
    assert_eq!(
        note,
        "1 preservation-only gameplay recording kept in the Library is not organised, \
         renamed or published by these tools."
    );
    for blue in [true, false] {
        let context = egui::Context::default();
        let mut app = app_with(&context, blue);
        let page = shown(
            &context,
            &mut app,
            Route::Section(Section::Build),
            [1280.0, 720.0],
        );
        assert!(
            has(&page, "preservation-only gameplay recording"),
            "blue={blue}: Organisation hides the recording exclusion: {page:?}"
        );
    }
}
