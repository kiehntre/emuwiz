//! Real-library launch census (ignored; run by hand inside a read-only
//! sandbox). For a stratified sample of every platform and file kind in a
//! library database, it opens Blue Game Details headlessly, waits for the
//! readiness card to settle, and records what the person would see: the
//! readiness state, headline, primary action, identity line, and whether Play
//! and Choose emulator are offered. It never clicks anything and never
//! launches; it only paints frames.
//!
//! Environment: `CENSUS_DB` (a copy of the library database), `CENSUS_OUT`
//! (TSV path), optional `CENSUS_PER_GROUP` (default 3), `CENSUS_PLATFORMS`
//! (comma-separated filter), `CENSUS_SECONDS` (per-game settle limit, 45).

use super::super::launch_readiness_summary::{GameReadinessSummary, ReadinessPresentationState};
use super::super::{App, Route};
use crate::launch_readiness_page::LaunchReadinessInput;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SIZE: [f32; 2] = [1600.0, 2600.0];

fn raw_input() -> eframe::egui::RawInput {
    eframe::egui::RawInput {
        screen_rect: Some(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(SIZE[0], SIZE[1]),
        )),
        ..Default::default()
    }
}

fn texts(output: &eframe::egui::FullOutput) -> Vec<String> {
    fn gather(shape: &eframe::egui::Shape, out: &mut Vec<String>) {
        match shape {
            eframe::egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            eframe::egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| gather(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|s| gather(&s.shape, &mut out));
    out
}

fn paint(app: &mut App, context: &eframe::egui::Context) -> Vec<String> {
    app.mods.poll();
    if let Some(workflows) = app.native_workflows.as_mut() {
        workflows.poll(context, &mut app.activity);
    }
    app.poll(context);
    let output = context.run(raw_input(), |ctx| app.show(ctx));
    texts(&output)
}

fn input_name(input: &LaunchReadinessInput) -> String {
    match input {
        LaunchReadinessInput::AtariSt(_) => "AtariSt".into(),
        LaunchReadinessInput::EvidenceNotLoaded => "EvidenceNotLoaded".into(),
        LaunchReadinessInput::RetroArchNotScanned => "RetroArchNotScanned".into(),
        LaunchReadinessInput::IdentityUnknown(reason) => format!("IdentityUnknown({reason:?})"),
        LaunchReadinessInput::DosBoxReview => "DosBoxReview".into(),
        LaunchReadinessInput::DiscFilesUnresolved(_) => "DiscFilesUnresolved".into(),
        LaunchReadinessInput::InspectionIncomplete => "InspectionIncomplete".into(),
        LaunchReadinessInput::IdentityConflicting => "IdentityConflicting".into(),
        LaunchReadinessInput::Plan { plan, .. } => {
            let targets: Vec<String> = plan
                .candidates
                .iter()
                .map(|c| {
                    let target = format!("{:?}", c.target);
                    let short: String = target.chars().take(60).collect();
                    let blockers: Vec<String> = c
                        .blockers
                        .iter()
                        .map(|b| format!("{b:?}").chars().take(80).collect())
                        .collect();
                    format!(
                        "{short}={:?}{}",
                        c.readiness,
                        if blockers.is_empty() {
                            String::new()
                        } else {
                            format!("[{}]", blockers.join(";"))
                        }
                    )
                })
                .collect();
            format!(
                "Plan(platform={:?}; {})",
                plan.platform_id,
                targets.join(" | ")
            )
        }
    }
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn extension(game: &super::super::library::Game) -> String {
    let kind = &game.archive.archive_kind;
    let ext = game
        .archive
        .absolute_path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext.is_empty() || ext.len() > 6 {
        kind.clone()
    } else {
        ext
    }
}

#[test]
#[ignore = "real-library census; needs CENSUS_DB and a sandboxed real profile"]
fn real_library_launch_census() {
    let db = std::path::PathBuf::from(std::env::var("CENSUS_DB").expect("CENSUS_DB"));
    let out = std::path::PathBuf::from(std::env::var("CENSUS_OUT").expect("CENSUS_OUT"));
    let per_group: usize = std::env::var("CENSUS_PER_GROUP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let seconds: u64 = std::env::var("CENSUS_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(45);
    let filter: Vec<String> = std::env::var("CENSUS_PLATFORMS")
        .ok()
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let library = super::super::backend::load_library(&db).expect("read-only load");

    // Stratify by (platform, file kind); spread picks across each group.
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (index, game) in library.games.iter().enumerate() {
        if !filter.is_empty() && !filter.contains(&game.platform.to_ascii_lowercase()) {
            continue;
        }
        groups
            .entry((game.platform.clone(), extension(game)))
            .or_default()
            .push(index);
    }
    let mut picks = Vec::new();
    for ((platform, ext), members) in &groups {
        let take = per_group.min(members.len());
        for k in 0..take {
            let index = members[k * members.len() / take];
            picks.push((platform.clone(), ext.clone(), members.len(), index));
        }
    }
    eprintln!("census: {} groups, {} games", groups.len(), picks.len());

    let context = eframe::egui::Context::default();
    context.style_mut(|style| style.animation_time = 0.0);
    let mut app = super::fixture(&context);
    app.library = Arc::new(library);
    let mut file = std::fs::File::create(&out).expect("CENSUS_OUT");
    writeln!(
        file,
        "platform\text\tgroup_size\tid\tpath\tstatus\tprimary_action\theadline\tidentity\tfirmware\temulator\tplay_shown\tchoose_emulator_shown\tsettle_ms\tinput\tcard_text\tlaunch_play_button\tlaunch_identity_unresolved\tlaunch_choice_needed\tlaunch_text"
    )
    .unwrap();

    // Warm up: open the first game and wait until every emulator discovery
    // Blue's own background checker waits for has finished.
    if let Some((_, _, _, index)) = picks.first() {
        app.router.current = Route::BrowsePlayGame(app.library.games[*index].archive.id);
        let warm = Instant::now();
        loop {
            paint(&mut app, &context);
            let ready = app.native_workflows.as_ref().is_some_and(|w| {
                super::super::bg_readiness::EnvSnapshot::capture(&w.app.emulator_readiness)
                    .is_some()
            });
            if ready || warm.elapsed() > Duration::from_secs(600) {
                eprintln!(
                    "census: emulator discovery ready={ready} after {:?}",
                    warm.elapsed()
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(w) = app.native_workflows.as_ref()
            && let crate::emulator_setup::RetroArchProfilesState::Ready(d) =
                &w.app.emulator_readiness.retroarch_profiles
        {
            eprintln!(
                "census: RetroArch profiles discovered: {}",
                d.profiles.len()
            );
        }
    }

    for (n, (platform, ext, size, index)) in picks.iter().enumerate() {
        let game = app.library.games[*index].clone();
        let id = game.archive.id;
        app.router.current = Route::BrowsePlayGame(id);
        let started = Instant::now();
        let deadline = started + Duration::from_secs(seconds);
        let mut last_summary: Option<(GameReadinessSummary, String)> = None;
        let mut stable = 0;
        let mut frame;
        loop {
            frame = paint(&mut app, &context);
            // Exactly the inputs Game Details passes (`detail_readiness`):
            // the Identity line's own verified answer and its DAT identity.
            let identity = app.detail_identity(&game);
            let verified = identity.as_ref().is_some_and(|i| i.is_verified());
            let dat_identity = identity.as_ref().and_then(|i| i.card_identity());
            let summary = app.native_workflows.as_mut().map(|workflows| {
                let (mut summary, input) = workflows.readiness_projection(
                    &context,
                    id,
                    &game.archive.absolute_path,
                    game.archive.identity_report.as_ref(),
                    verified,
                    true,
                    &mut app.activity,
                );
                if let Some(identity) = dat_identity {
                    super::super::launch_readiness_summary::note_dat_identity(
                        &mut summary,
                        identity,
                    );
                }
                (summary, input_name(&input))
            });
            let settled = summary
                .as_ref()
                .is_some_and(|(s, _)| s.status != ReadinessPresentationState::Checking);
            let same = summary.as_ref().map(|(s, i)| (&s.status, &s.headline, i))
                == last_summary
                    .as_ref()
                    .map(|(s, i)| (&s.status, &s.headline, i));
            last_summary = summary;
            if settled && same {
                stable += 1;
                if stable >= 5 {
                    break;
                }
            } else {
                stable = 0;
            }
            if Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        let card: Vec<&String> = frame
            .iter()
            .skip_while(|t| t.as_str() != "Can I play it?")
            .take(40)
            .collect();
        let play_shown = card
            .iter()
            .any(|t| t.as_str() == "Play" || t.starts_with("Play —"));
        let choose_shown = card.iter().any(|t| t.contains("Choose emulator"));
        let (status, action, headline, identity, firmware, emulator, input) = match &last_summary {
            Some((s, input)) => (
                format!("{:?}", s.status),
                format!("{:?}", s.primary_action),
                clean(&s.headline),
                format!("{:?}", s.identity),
                format!("{:?}", s.firmware),
                s.emulator
                    .as_ref()
                    .map(|e| format!("{} · {}", e.name, e.profile))
                    .unwrap_or_default(),
                clean(input),
            ),
            None => (
                "NoWorkflows".into(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ),
        };
        let settle_ms = started.elapsed().as_millis();
        // Where Play leads: the Launch page. Paint it until its text settles
        // and record what it offers. Nothing is clicked.
        app.router.current = Route::Task {
            section: super::super::Section::Launch,
            game: id,
        };
        let launch_started = Instant::now();
        let mut previous: Vec<String> = Vec::new();
        let mut same_frames = 0;
        let mut launch_frame;
        loop {
            launch_frame = paint(&mut app, &context);
            let checking = launch_frame
                .iter()
                .any(|t| t.contains("Checking") || t.contains("checking"));
            if launch_frame == previous && !checking {
                same_frames += 1;
                if same_frames >= 8 {
                    break;
                }
            } else {
                same_frames = 0;
            }
            previous = launch_frame.clone();
            if launch_started.elapsed() > Duration::from_secs(seconds) {
                break;
            }
            std::thread::sleep(Duration::from_millis(80));
        }
        let launch_text: Vec<&String> = launch_frame
            .iter()
            .skip_while(|t| !t.starts_with("Play / Launch readiness"))
            .take(60)
            .collect();
        let launch_play = launch_frame
            .iter()
            .any(|t| t.starts_with("Play —") || t.starts_with("Launch "));
        let launch_identity_unresolved = launch_frame
            .iter()
            .any(|t| t.contains("Identity unresolved"));
        let launch_choice_needed = launch_frame.iter().any(|t| t.contains("Choice needed"));
        writeln!(
            file,
            "{platform}\t{ext}\t{size}\t{id}\t{}\t{status}\t{action}\t{headline}\t{identity}\t{firmware}\t{emulator}\t{play_shown}\t{choose_shown}\t{settle_ms}\t{input}\t{}\t{launch_play}\t{launch_identity_unresolved}\t{launch_choice_needed}\t{}",
            clean(&game.archive.absolute_path.display().to_string()),
            clean(&card.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ¦ ")),
            clean(&launch_text.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ¦ ")),
        )
        .unwrap();
        file.flush().unwrap();
        eprintln!(
            "census {}/{}: {platform} {ext} -> {status}",
            n + 1,
            picks.len()
        );
    }
}
