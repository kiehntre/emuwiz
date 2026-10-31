//! Real native-page frames: only clipped, on-screen text counts as reachable.
use super::*;
use crate::emulator_setup::profile_resolution::ppsspp::PpssppResolutionState;
use crate::emulator_setup::profile_resolution::{DuckStationResolutionState, ResolutionView};
use archivefs_core::emulator_profile_resolver::{EmulatorSelectionMode, ResolutionOutcome};

const CASES: [([f32; 2], f32); 3] = [
    ([1280., 820.], 1.),
    ([1024., 600.], 1.),
    ([1024., 600.], 1.6),
];
fn ready(context: &egui::Context, section: Section, simple: bool, scale: f32) -> App {
    let mut app = super::tests::fixture(context);
    app.simple.enabled = simple;
    app.simple.set_ui_scale(scale);
    app.router.current = Route::Section(section);
    app.library = Arc::new(Library::new(
        (0..12)
            .map(|i| {
                super::tests::archive(i, &format!("Game {i}"), Some(&format!("Platform {i:02}")))
            })
            .collect(),
    ));
    let mut native = native_workflows::NativeWorkflows::new(context.clone());
    let empty = || ResolutionView {
        mode: EmulatorSelectionMode::Auto,
        selection_note: None,
        outcome: ResolutionOutcome::NoInstallation {
            assessments: vec![],
        },
    };
    native
        .app
        .emulator_readiness
        .duckstation_resolution_generation = 7;
    native.app.emulator_readiness.duckstation_resolution = DuckStationResolutionState::Ready {
        generation: 7,
        view: Box::new(empty()),
    };
    native.app.emulator_readiness.ppsspp_resolution_generation = 9;
    native.app.emulator_readiness.ppsspp_resolution = PpssppResolutionState::Ready {
        generation: 9,
        view: Box::new(empty()),
    };
    app.native_workflows = Some(native);
    app
}
fn frame(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            events,
            ..Default::default()
        },
        |ctx| app.show(ctx),
    )
}
fn visible(
    output: &egui::FullOutput,
    size: [f32; 2],
    needle: &str,
) -> Option<(egui::Rect, egui::Rect)> {
    fn visit(
        shape: &egui::Shape,
        clip: egui::Rect,
        needle: &str,
    ) -> Option<(egui::Rect, egui::Rect)> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == needle => {
                let rect = egui::Rect::from_min_size(t.pos, t.galley.size());
                clip.contains_rect(rect).then_some((rect, clip))
            }
            egui::Shape::Vec(v) => v.iter().find_map(|s| visit(s, clip, needle)),
            _ => None,
        }
    }
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
    output
        .shapes
        .iter()
        .find_map(|s| visit(&s.shape, s.clip_rect.intersect(screen), needle))
}
fn settled(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    let mut output = frame(context, app, size, vec![]);
    for _ in 0..4 {
        output = frame(context, app, size, vec![]);
    }
    output
}
fn wheel(context: &egui::Context, app: &mut App, size: [f32; 2], delta: f32) -> egui::FullOutput {
    frame(
        context,
        app,
        size,
        vec![
            egui::Event::PointerMoved(egui::pos2(size[0] * 0.8, size[1] * 0.65)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0., delta),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
    );
    settled(context, app, size)
}
fn targets(section: Section) -> Vec<&'static str> {
    match section {
        Section::Emulators => vec!["Check Emulators"],
        Section::Sources => vec!["Add source", "ROM hack identification (HackHash)"],
        Section::Dat => vec![
            "Import DAT…",
            "No-Intro pack import",
            "Compare two DAT files (DAT Duel)",
        ],
        Section::Check => vec![
            "Choose a platform",
            "Check Platform 00",
            "Check Platform 11",
        ],
        _ => unreachable!(),
    }
}
#[test]
fn native_pages_and_actions_are_reachable_with_bounded_wheel_scrolling() {
    let mut failures = vec![];
    for simple in [false, true] {
        for (window, scale) in CASES {
            for section in [
                Section::Emulators,
                Section::Sources,
                Section::Dat,
                Section::Check,
            ] {
                let context = egui::Context::default();
                let mut app = ready(&context, section, simple, scale);
                let size = [window[0] / scale, window[1] / scale];
                let mut output = settled(&context, &mut app, size);
                assert!((context.zoom_factor() - scale).abs() < 0.001);
                let mut wanted = targets(section);
                for step in 0..=40 {
                    wanted.retain(|needle| {
                        if let Some((_, clip)) = visible(&output, size, needle) {
                            eprintln!(
                                "SCROLL {section:?} {window:?} {scale} simple={simple}: {needle} step={step} viewport_height={}",
                                clip.height()
                            );
                            assert!(clip.height() >= 160., "body viewport too small: {clip:?}");
                            false
                        } else {
                            true
                        }
                    });
                    if wanted.is_empty() {
                        break;
                    }
                    output = wheel(&context, &mut app, size, -120.);
                }
                if !wanted.is_empty() {
                    failures.push(format!(
                        "{section:?} {window:?} {scale} simple={simple}: {wanted:?}"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "unreachable: {failures:#?}");
}

fn key(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    key: egui::Key,
    reverse: bool,
) -> egui::FullOutput {
    let modifiers = egui::Modifiers {
        shift: reverse,
        ..Default::default()
    };
    frame(
        context,
        app,
        size,
        [true, false]
            .into_iter()
            .map(|pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            })
            .collect(),
    );
    settled(context, app, size)
}
#[test]
fn native_primary_actions_receive_visible_tab_and_reverse_tab_focus() {
    for simple in [false, true] {
        for (window, scale) in CASES {
            for section in [
                Section::Emulators,
                Section::Sources,
                Section::Dat,
                Section::Check,
            ] {
                for reverse in [false, true] {
                    let context = egui::Context::default();
                    let mut app = ready(&context, section, simple, scale);
                    let size = [window[0] / scale, window[1] / scale];
                    settled(&context, &mut app, size);
                    let target = match section {
                        Section::Emulators => "Check Emulators",
                        Section::Sources => "Add source",
                        Section::Dat => "Import DAT…",
                        Section::Check => "Check Platform 11",
                        _ => unreachable!(),
                    };
                    let mut reached = false;
                    for step in 0..180 {
                        let output = key(&context, &mut app, size, egui::Key::Tab, reverse);
                        let Some(response) = context
                            .memory(|m| m.focused())
                            .and_then(|id| context.read_response(id))
                        else {
                            continue;
                        };
                        if let Some((label, clip)) = visible(&output, size, target) {
                            if response.rect.contains_rect(label) {
                                assert!(
                                    clip.expand(1.).contains_rect(response.rect),
                                    "focused button clipped: {section:?} {window:?} {scale} simple={simple} reverse={reverse}: {:?} {clip:?}",
                                    response.rect
                                );
                                eprintln!(
                                    "KEYBOARD {section:?} {window:?} {scale} simple={simple} reverse={reverse}: {target} tabs={}",
                                    step + 1
                                );
                                reached = true;
                                break;
                            }
                        }
                    }
                    assert!(
                        reached,
                        "keyboard target unreachable: {section:?} {window:?} {scale} simple={simple} reverse={reverse}: {target}"
                    );
                }
            }
        }
    }
}

// Placement after the candidate grid belongs to the Launch integration lane.
// Keep the distance visible in evidence while proving the single viewport can
// reach the card with ordinary wheel input at every supported test size.
fn assert_card_reachable_with_deferred_placement(title: &str) {
    for simple in [false, true] {
        for (window, scale) in CASES {
            let context = egui::Context::default();
            let mut app = ready(&context, Section::Emulators, simple, scale);
            let size = [window[0] / scale, window[1] / scale];
            let mut output = settled(&context, &mut app, size);
            let mut reached = false;
            for step in 0..=240 {
                if visible(&output, size, title).is_some() {
                    eprintln!("{title} {window:?} {scale} simple={simple}: wheel_steps={step}");
                    reached = true;
                    break;
                }
                output = wheel(&context, &mut app, size, -120.);
            }
            assert!(
                reached,
                "{title} unreachable: {window:?} {scale} simple={simple}"
            );
        }
    }
}

#[test]
fn ppsspp_card_remains_reachable_with_deferred_placement() {
    assert_card_reachable_with_deferred_placement(
        "PPSSPP: shared resolver recommendation (preview)",
    );
}

// DuckStation's card now sits below the candidate grid too (so the page's
// primary action stays visible at 1024x600); it is reached the same way.
#[test]
fn duckstation_card_remains_reachable_with_deferred_placement() {
    assert_card_reachable_with_deferred_placement("DuckStation: shared resolver selection");
}

#[test]
fn check_home_and_end_reveal_actual_viewport_content() {
    for simple in [false, true] {
        for (window, scale) in CASES {
            let context = egui::Context::default();
            let mut app = ready(&context, Section::Check, simple, scale);
            let size = [window[0] / scale, window[1] / scale];
            settled(&context, &mut app, size);
            let end = key(&context, &mut app, size, egui::Key::End, false);
            assert!(visible(&end, size, "Check Platform 11").is_some());
            let home = key(&context, &mut app, size, egui::Key::Home, false);
            assert!(visible(&home, size, "Check Platform 11").is_none());
        }
    }
}

const COMPACT: [f32; 2] = [1024., 600.];
const MODS_PURPOSE: &str =
    "Find improvements for a game and preview every change before installing.";

/// One notch moves a little per settled frame, so a real scroll is several.
fn wheel_down(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    let mut output = wheel(context, app, size, -120.);
    for _ in 0..12 {
        output = wheel(context, app, size, -120.);
    }
    output
}

#[test]
fn blue_page_introductions_scroll_away_but_the_location_stays() {
    let context = egui::Context::default();
    let mut app = ready(&context, Section::Mods, true, 1.);
    let purpose = MODS_PURPOSE;
    let top = settled(&context, &mut app, COMPACT);
    assert!(visible(&top, COMPACT, purpose).is_some());
    let scrolled = wheel_down(&context, &mut app, COMPACT);
    assert!(
        visible(&scrolled, COMPACT, purpose).is_none(),
        "the introduction must leave the viewport at 1024x600"
    );
    assert!(visible(&scrolled, COMPACT, "Mods & Cheats").is_some());
}

#[test]
fn a_notice_stays_in_view_while_the_page_is_scrolled() {
    let context = egui::Context::default();
    let mut app = ready(&context, Section::Mods, true, 1.);
    settled(&context, &mut app, COMPACT);
    wheel_down(&context, &mut app, COMPACT);
    app.notice = Some(super::Notice {
        message: "Pinned notice".into(),
        technical: String::new(),
    });
    let output = settled(&context, &mut app, COMPACT);
    assert!(visible(&output, COMPACT, "Pinned notice").is_some());
}

#[test]
fn a_new_visit_starts_at_the_top_and_back_keeps_the_place() {
    let context = egui::Context::default();
    let mut app = ready(&context, Section::Mods, true, 1.);
    let purpose = MODS_PURPOSE;
    settled(&context, &mut app, COMPACT);
    wheel_down(&context, &mut app, COMPACT);
    app.go(Route::Section(Section::Settings));
    settled(&context, &mut app, COMPACT);
    app.back();
    let returned = settled(&context, &mut app, COMPACT);
    assert!(
        visible(&returned, COMPACT, purpose).is_none(),
        "Back returns to the earlier position"
    );
    app.go(Route::Section(Section::Settings));
    settled(&context, &mut app, COMPACT);
    app.go(Route::Section(Section::Mods));
    let revisited = settled(&context, &mut app, COMPACT);
    assert!(visible(&revisited, COMPACT, purpose).is_some());
}

#[test]
fn reopening_all_tools_starts_at_the_top() {
    let context = egui::Context::default();
    let mut app = ready(&context, Section::Mods, true, 1.);
    app.open_all_tools();
    let top = settled(&context, &mut app, COMPACT);
    let purpose = super::all_tools::PURPOSE;
    assert!(visible(&top, COMPACT, purpose).is_some());
    let scrolled = wheel_down(&context, &mut app, COMPACT);
    assert!(visible(&scrolled, COMPACT, purpose).is_none());
    app.simple.all_tools = false;
    settled(&context, &mut app, COMPACT);
    app.open_all_tools();
    let reopened = settled(&context, &mut app, COMPACT);
    assert!(visible(&reopened, COMPACT, purpose).is_some());
}

#[test]
fn romm_failures_stay_on_their_page_and_other_jobs_keep_the_notice() {
    let context = egui::Context::default();
    let mut app = ready(&context, Section::Mods, true, 1.);
    let romm = app.activity.queue(
        "Previewing RomM import",
        Route::Section(Section::Romm),
        false,
    );
    let scan = app
        .activity
        .queue("Scanning", Route::Section(Section::Games), false);
    assert!(app.failure_stays_on_its_page(romm));
    assert!(!app.failure_stays_on_its_page(scan));
}

#[test]
fn mr_wiz_stays_quiet_where_its_tip_does_not_apply() {
    let context = egui::Context::default();
    let mut app = ready(&context, Section::Mods, true, 1.);
    assert!(app.guidance_context(&Route::MameWorkflow).is_none());
    let mods = Route::Section(Section::Mods);
    assert!(app.guidance_context(&mods).is_none());
    app.mods.select_activation();
    assert!(app.guidance_context(&mods).is_some());
    // Pages with their own tip keep it.
    assert!(
        app.guidance_context(&Route::Section(Section::Sources))
            .is_some()
    );
}

#[test]
fn mame_actions_explain_what_they_are_waiting_for() {
    use super::mame_reconstruction::selection_blocker;
    assert!(selection_blocker(false, true).unwrap().contains("folder"));
    assert!(
        selection_blocker(true, false)
            .unwrap()
            .contains("catalogue")
    );
    assert_eq!(selection_blocker(true, true), None);
}

/// A page tall enough for several presses, read through egui's real state.
fn page_offsets(
    show: impl Fn(&mut egui::Ui),
    salt: impl std::hash::Hash + Copy,
    keys: &[egui::Key],
) -> Vec<f32> {
    let context = egui::Context::default();
    let run = |events: Vec<egui::Event>| {
        let mut offset = 0.;
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 400.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let id = ui.make_persistent_id(egui::Id::new(salt));
                    show(ui);
                    offset = egui::scroll_area::State::load(ui.ctx(), id)
                        .unwrap_or_default()
                        .offset
                        .y;
                });
            },
        );
        offset
    };
    run(vec![]);
    run(vec![]);
    keys.iter()
        .map(|key| {
            run([true, false]
                .into_iter()
                .map(|pressed| egui::Event::Key {
                    key: *key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .collect());
            run(vec![]);
            run(vec![])
        })
        .collect()
}

fn tall(ui: &mut egui::Ui) {
    ui.allocate_space(egui::vec2(100., 5000.));
}

fn assert_each_press_moves_one_page(offsets: &[f32]) {
    let [first, second, third, back] = offsets else {
        panic!("{offsets:?}");
    };
    assert!(*first > 0., "{offsets:?}");
    assert!(second > first && third > second, "{offsets:?}");
    assert!(
        (second - back).abs() < 1.,
        "PageUp moves one page: {offsets:?}"
    );
}

#[test]
fn repeated_paging_moves_a_page_at_a_time_on_the_shared_page_scroller() {
    use egui::Key::{PageDown, PageUp};
    let offsets = page_offsets(
        |ui| {
            super::simple_shell::show_page_scroll(ui, Section::Mods, false, tall);
        },
        ("v2_page_content", Section::Mods),
        &[PageDown, PageDown, PageDown, PageUp],
    );
    assert_each_press_moves_one_page(&offsets);
}

#[test]
fn repeated_paging_moves_a_page_at_a_time_on_check_games() {
    use egui::Key::{PageDown, PageUp};
    let offsets = page_offsets(
        |ui| super::pages::check_scroll(ui, None, tall),
        ("v2_check_scroll", None::<&str>),
        &[PageDown, PageDown, PageDown, PageUp],
    );
    assert_each_press_moves_one_page(&offsets);
}

/// The pages that lost their inner scroller, through the real shell.
#[test]
fn pages_without_an_inner_scroller_page_and_navigate_in_both_views() {
    for simple in [false, true] {
        for section in [
            Section::Setup,
            Section::Mods,
            Section::Problems,
            Section::Build,
            Section::Duplicates,
            Section::Activity,
            Section::Artwork,
        ] {
            let context = egui::Context::default();
            let mut app = ready(&context, section, simple, 1.);
            let top = settled(&context, &mut app, COMPACT);
            // Text only: a loading spinner animates between frames.
            let fingerprint = |output: &egui::FullOutput| {
                output
                    .shapes
                    .iter()
                    .filter_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(text) => Some((text.pos, text.galley.text().to_owned())),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            let before = fingerprint(&top);
            key(&context, &mut app, COMPACT, egui::Key::PageDown, false);
            key(&context, &mut app, COMPACT, egui::Key::PageUp, false);
            let back_at_top = settled(&context, &mut app, COMPACT);
            assert_eq!(
                before,
                fingerprint(&back_at_top),
                "{section:?} simple={simple}"
            );
            app.go(Route::Section(Section::Settings));
            settled(&context, &mut app, COMPACT);
            assert_eq!(app.router.current, Route::Section(Section::Settings));
            app.back();
            settled(&context, &mut app, COMPACT);
            assert_eq!(app.router.current, Route::Section(section));
        }
    }
}

/// Classic keeps its pinned introduction, so only the body moves.
#[test]
fn classic_keeps_its_pinned_title_and_restores_a_fresh_visit() {
    let context = egui::Context::default();
    let mut app = ready(&context, Section::Mods, false, 1.);
    let text = "Choose a game and inspect compatibility before reviewing a change. Browsing this workshop does not change game or emulator files.";
    settled(&context, &mut app, COMPACT);
    let scrolled = wheel_down(&context, &mut app, COMPACT);
    assert!(visible(&scrolled, COMPACT, MODS_PURPOSE).is_some());
    assert!(visible(&scrolled, COMPACT, text).is_none());
    app.go(Route::Section(Section::Settings));
    settled(&context, &mut app, COMPACT);
    app.go(Route::Section(Section::Mods));
    let revisited = settled(&context, &mut app, COMPACT);
    assert!(visible(&revisited, COMPACT, text).is_some());
}
