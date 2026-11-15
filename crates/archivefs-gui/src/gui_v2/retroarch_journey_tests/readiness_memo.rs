//! The readiness card may be a moment old; Play never is. Once a game stops
//! being launchable, a card that still reads Ready must not be able to start
//! it: Play builds its own launch input at the click, and the launch
//! preflight inspects the file again after that.
use super::*;

#[test]
fn a_stale_ready_card_cannot_start_a_game_whose_file_is_gone() {
    let profile = Profile::new();
    profile.run("memo-stale-ready", None);
    assert!(
        !profile.fake.record_dir.join("args.txt").exists(),
        "RetroArch must not have been started"
    );
}

pub(super) fn run(blue: &mut Blue, fixture: &VerifiedGameFixture, record: &Path) {
    use crate::gui_v2::launch_readiness_summary::ReadinessPresentationState as State;
    let game = blue.game;
    let rom = fixture.rom_path.clone();
    let card_reads_ready = |blue: &Blue| {
        blue.app
            .native_workflows
            .as_ref()
            .and_then(|workflows| workflows.readiness_summary(game, &rom))
            .is_some_and(|summary| {
                matches!(summary.status, State::Ready | State::ReadyWithWarnings)
            })
    };
    // Game Details draws its readiness card from the remembered input.
    blue.app.router.current = Route::Game(game);
    blue.wait_until(|blue| card_reads_ready(blue), 45);
    let workflows = blue.app.native_workflows.as_ref().unwrap();
    // From here the remembered input cannot expire by itself.
    workflows.freeze_drawn_launch_input();
    assert!(card_reads_ready(blue));
    let built_for_drawing = workflows.launch_input_builds();

    // The game file disappears. Nothing the remembered input is keyed on
    // changes, and no frame is drawn, so nothing can notice.
    let parked = rom.with_extension("parked");
    fs::rename(&rom, &parked).unwrap();
    assert!(
        card_reads_ready(blue),
        "the card still reads Ready: this is the stale answer the test is about"
    );
    assert_eq!(
        blue.app
            .native_workflows
            .as_ref()
            .unwrap()
            .launch_input_builds(),
        built_for_drawing,
        "reading the card rebuilt nothing"
    );

    // Play, pressed while the stale card says Ready.
    let outcome = blue.app.native_workflows.as_mut().unwrap().try_play(game);
    assert_eq!(
        blue.app
            .native_workflows
            .as_ref()
            .unwrap()
            .launch_input_builds(),
        built_for_drawing + 1,
        "Play built its own input at the click"
    );
    // Whichever layer refuses (the fresh plan or the launch preflight), the
    // emulator is never started.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut frames = 0;
    loop {
        blue.paint();
        frames += 1;
        if frames >= 20 && !blue.state().is_active() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the refused launch never settled; outcome {outcome:?}; last frame: {:?}",
            blue.last
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !record.join("args.txt").exists(),
        "RetroArch was started for a missing game file; Play outcome {outcome:?}"
    );
    println!("memo-stale-ready: Play outcome {outcome:?}, nothing launched");
    fs::rename(&parked, &rom).unwrap();
}
