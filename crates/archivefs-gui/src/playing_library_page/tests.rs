//! Focused tests for the Build Playing Library page.
//!
//! Every DAT/source fixture is a real temp file/folder; `preview()` and
//! `confirm_apply()` run the real core planner and the real shared
//! `rename_apply` executor - nothing here is a render-only mock.

use std::path::PathBuf;

use archivefs_core::dat::identity::{DatPlatformConfidence, DatPlatformIdentity};
use archivefs_core::dat::rename_apply::model::TransactionState;
use archivefs_core::emulator_environment::es_de::{
    DiscoveryEnvironment, discover_es_de_environment,
};
use archivefs_core::playing_library::{RetroDeckVisibility, build_retrodeck_projection};

use super::*;

#[test]
fn attention_publication_errors_deduplicate_and_follow_workflow_resolution() {
    use archivefs_core::attention::*;
    let mut state = PlayingLibraryPageState {
        romm_error: Some(
            "The destination changed; review the existing publication preview.".into(),
        ),
        esde_preview_error: Some(("Publication is stale".into(), None)),
        esde_recovery_error: Some(("Recovery review is required".into(), None)),
        esde_recovery_gamelist_path: Some("/disposable/gamelist.xml".into()),
        ..Default::default()
    };
    let snapshot = state.attention_snapshot();
    assert_eq!(snapshot.page(&AttentionFilters::default()).total, 2);
    assert_eq!(
        snapshot
            .items()
            .filter(|i| i.destination == AttentionDestination::EsDe)
            .count(),
        1
    );
    state.romm_error = None;
    state.esde_preview_error = None;
    state.esde_recovery_error = None;
    state.esde_recovery_done = true;
    assert_eq!(
        state
            .attention_snapshot()
            .page(&AttentionFilters::default())
            .total,
        0
    );
}

/// SHA-1 of `b"test"` (4 bytes).
const SHA1_TEST: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
/// SHA-1 of `b"abc"` (3 bytes).
const SHA1_ABC: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "archivefs-gui-playing-library-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture root");
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn write_dat(fixture: &Fixture, name: &str, body: &str) -> PathBuf {
    let path = fixture.path(name);
    std::fs::write(&path, body).unwrap();
    path
}

fn base_state(fixture: &Fixture) -> PlayingLibraryPageState {
    let journal_dir = fixture.path("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let mut state = PlayingLibraryPageState::with_journal_dir(journal_dir);
    state.exclude_beta = false;
    state.exclude_proto = false;
    state.exclude_demo = false;
    state.exclude_sample = false;
    state.prefer_newest_revision = false;
    state.prefer_parent = false;
    state.preferred_regions_draft.clear();
    state
}

fn confirm_library(state: &mut PlayingLibraryPageState) {
    let count = state.pending_apply.expect("pending confirmation");
    state.confirm_text = playing_library_confirmation_phrase(count);
    state.confirm_apply();
}

fn confirm_romm(state: &mut PlayingLibraryPageState) {
    let count = state
        .romm_projection
        .as_ref()
        .map_or(0, |plan| plan.total_files);
    state.romm_confirm_text = playing_library_confirmation_phrase(count);
    state.confirm_romm_apply();
}

fn confirm_retrodeck(state: &mut PlayingLibraryPageState) {
    let count = state
        .retrodeck_projection
        .as_ref()
        .map_or(0, |plan| plan.total_files);
    state.retrodeck_confirm_text = playing_library_confirmation_phrase(count);
    state.confirm_retrodeck_apply();
}

// --- real-widget render/interaction helpers -----------------------------
//
// These drive the page exactly the way a real frame loop would: a single
// `egui::Context` persists across multiple `run()` calls (so focus memory
// set in one frame survives into the next, the same as a real app), text is
// typed by giving a field's stable id keyboard focus and then sending
// `egui::Event::Text` (the same pattern this crate's own
// `set_text_edit_caret`/`apply_select_all` in `main.rs` already use to drive
// a `TextEdit` programmatically), and a click is one `PointerMoved` +
// press + release batch at the exact center of the target's own rendered
// text, located from the previous frame's real output - never by mutating
// page state directly.

fn screen() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 1600.0))
}

fn render(
    ctx: &egui::Context,
    state: &mut PlayingLibraryPageState,
    input: egui::RawInput,
) -> (egui::FullOutput, Option<PlayingLibraryPageAction>) {
    let mut action = None;
    let output = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = show_playing_library_page(ui, state);
        });
    });
    (output, action)
}

fn base_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(screen()),
        ..Default::default()
    }
}

fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|shape| shape_contains(shape, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

fn find_exact_text_center(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn find_in_shape(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text_shape) => (text_shape.galley.text() == needle)
                .then(|| text_shape.pos + text_shape.galley.size() / 2.0),
            egui::Shape::Vec(nested) => nested.iter().find_map(|s| find_in_shape(s, needle)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|clipped| find_in_shape(&clipped.shape, needle))
}

fn click_event(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ]
}

/// Clicks on the rendered widget whose own text is exactly `needle` (a
/// button label or a checkbox's label), from a fresh render. Panics if
/// nothing renders that exact text - a test relying on this should already
/// know the widget is on screen.
fn click_text(
    ctx: &egui::Context,
    state: &mut PlayingLibraryPageState,
    needle: &str,
) -> (egui::FullOutput, Option<PlayingLibraryPageAction>) {
    let (before, _) = render(ctx, state, base_input());
    let pos = find_exact_text_center(&before, needle)
        .unwrap_or_else(|| panic!("expected to find rendered text {needle:?} to click"));
    let (_, action) = render(
        ctx,
        state,
        egui::RawInput {
            screen_rect: Some(screen()),
            events: click_event(pos),
            ..Default::default()
        },
    );
    // egui applies the click during this frame, but the newly expanded
    // widget is emitted on the next frame. Return that settled frame so
    // assertions inspect the same state a real event loop would display.
    let (settled, _) = render(ctx, state, base_input());
    (settled, action)
}

/// Types `text` into the field with widget id `field_id` by giving it
/// keyboard focus and then sending it as a real `egui::Event::Text` -
/// exactly what a physical keyboard would produce, never a direct field
/// assignment.
fn type_into_field(
    ctx: &egui::Context,
    state: &mut PlayingLibraryPageState,
    field_id: &str,
    text: &str,
) {
    let _ = render(ctx, state, base_input());
    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new(field_id)));
    let _ = render(
        ctx,
        state,
        egui::RawInput {
            screen_rect: Some(screen()),
            events: vec![egui::Event::Text(text.to_string())],
            ..Default::default()
        },
    );
}

// --- source/destination selection --------------------------------------

#[test]
fn preview_requires_a_source_folder() {
    let fixture = Fixture::new("no-source");
    let dat = write_dat(
        &fixture,
        "one.dat",
        &format!(
            r#"<datafile><header><name>One</name></header>
<game name="Alpha"><rom name="alpha.bin" size="4" sha1="{SHA1_TEST}"/></game>
</datafile>"#
        ),
    );
    let mut state = base_state(&fixture);
    state.dat_path_draft = dat.display().to_string();
    state.destination_root_draft = fixture.path("playing").display().to_string();

    state.preview();

    assert!(state.plan().is_none());
    assert!(state.error().unwrap().contains("source"));
}

#[test]
fn preview_requires_an_absolute_destination() {
    let fixture = Fixture::new("relative-dest");
    let source = fixture.path("roms");
    std::fs::create_dir_all(&source).unwrap();
    let dat = write_dat(
        &fixture,
        "one.dat",
        &format!(
            r#"<datafile><header><name>One</name></header>
<game name="Alpha"><rom name="alpha.bin" size="4" sha1="{SHA1_TEST}"/></game>
</datafile>"#
        ),
    );
    let mut state = base_state(&fixture);
    state.dat_path_draft = dat.display().to_string();
    state.source_root_draft = source.display().to_string();
    state.destination_root_draft = "playing".to_string(); // relative, on purpose

    state.preview();

    assert!(state.plan().is_none());
    assert!(state.error().unwrap().contains("absolute"));
}

// --- policy building (region/language/revision/parent/exclusions) ------

#[test]
fn region_preference_order_is_preserved_in_declaration_order() {
    let fixture = Fixture::new("region-order");
    let mut state = base_state(&fixture);
    state.preferred_regions_draft = "Japan, Europe, USA".to_string();

    let policy = state.build_policy();

    assert_eq!(policy.preferred_regions, vec!["Japan", "Europe", "USA"]);
}

#[test]
fn language_preference_translates_plain_english_to_the_recognized_code() {
    let fixture = Fixture::new("language-pref");
    let mut state = base_state(&fixture);
    state.preferred_languages_draft = "English, Fr".to_string();

    let policy = state.build_policy();

    assert_eq!(policy.preferred_languages, vec!["en", "Fr"]);
}

#[test]
fn revision_toggle_maps_directly_to_the_policy_flag() {
    let fixture = Fixture::new("revision-toggle");
    let mut state = base_state(&fixture);
    assert!(!state.build_policy().prefer_newest_revision);
    state.prefer_newest_revision = true;
    assert!(state.build_policy().prefer_newest_revision);
}

#[test]
fn parent_toggle_maps_directly_to_the_policy_flag() {
    let fixture = Fixture::new("parent-toggle");
    let mut state = base_state(&fixture);
    assert!(!state.build_policy().prefer_parent);
    state.prefer_parent = true;
    assert!(state.build_policy().prefer_parent);
}

#[test]
fn release_class_exclusions_map_one_to_one() {
    let fixture = Fixture::new("exclusions");
    let mut state = base_state(&fixture);
    state.exclude_beta = true;
    state.exclude_sample = true;

    let policy = state.build_policy();

    assert_eq!(
        policy.excluded_release_classes,
        vec![
            archivefs_core::playing_library::ReleaseClass::Beta,
            archivefs_core::playing_library::ReleaseClass::Sample
        ]
    );
}

// --- preview uses the real core planner ---------------------------------

#[test]
fn preview_uses_the_real_core_planner() {
    let fixture = Fixture::new("real-planner");
    let source = fixture.path("roms");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("europe.bin"), b"test").unwrap();
    std::fs::write(source.join("usa.bin"), b"abc").unwrap();
    let dat = write_dat(
        &fixture,
        "one.dat",
        &format!(
            r#"<datafile><header><name>One</name></header>
<game name="Sonic (Europe)"><rom name="europe.bin" size="4" sha1="{SHA1_TEST}"/></game>
<game name="Sonic (USA)" cloneof="Sonic (Europe)"><rom name="usa.bin" size="3" sha1="{SHA1_ABC}"/></game>
</datafile>"#
        ),
    );
    let mut state = base_state(&fixture);
    state.dat_path_draft = dat.display().to_string();
    state.source_root_draft = source.display().to_string();
    state.destination_root_draft = fixture.path("playing").display().to_string();
    state.preferred_regions_draft = "Europe, USA".to_string();

    state.preview();

    let plan = state.plan().expect("a real plan from the core planner");
    assert_eq!(plan.archives_examined, 2);
    assert_eq!(plan.families_examined, 1);
    assert_eq!(plan.elected_games.len(), 1);
    assert_eq!(plan.elected_games[0].dat_entry_name, "Sonic (Europe)");
    // Nothing was written: preview only reads the DAT and hashes candidates.
    assert!(!fixture.path("playing").exists());
}

#[test]
fn individual_override_rehashes_and_selects_only_a_verified_family_member() {
    let fixture = Fixture::new("manual-override");
    let source = fixture.path("roms");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("europe.bin"), b"test").unwrap();
    std::fs::write(source.join("usa.bin"), b"abc").unwrap();
    let dat = write_dat(
        &fixture,
        "one.dat",
        &format!(
            r#"<datafile><header><name>One</name></header>
<game name="Sonic (Europe)"><rom name="europe.bin" size="4" sha1="{SHA1_TEST}"/></game>
<game name="Sonic (USA)" cloneof="Sonic (Europe)"><rom name="usa.bin" size="3" sha1="{SHA1_ABC}"/></game>
</datafile>"#
        ),
    );
    let mut state = base_state(&fixture);
    state.dat_path_draft = dat.display().to_string();
    state.source_root_draft = source.display().to_string();
    state.destination_root_draft = fixture.path("playing").display().to_string();
    state.preferred_regions_draft = "Europe, USA".into();
    state.preview();
    assert_eq!(
        state.plan().unwrap().elected_games[0].dat_entry_name,
        "Sonic (Europe)"
    );
    state.set_candidate_override("Sonic (Europe)".into(), source.join("usa.bin"));
    state.preview();
    assert_eq!(
        state.plan().unwrap().elected_games[0].dat_entry_name,
        "Sonic (USA)"
    );
    std::fs::write(source.join("usa.bin"), b"changed").unwrap();
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().is_some());
    assert!(!fixture.path("playing/unknown/usa.bin").exists());
}

#[test]
fn unresolved_groups_remain_unresolved_in_the_preview() {
    let fixture = Fixture::new("unresolved");
    let source = fixture.path("roms");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("a.bin"), b"test").unwrap();
    std::fs::write(source.join("b.bin"), b"abc").unwrap();
    let dat = write_dat(
        &fixture,
        "one.dat",
        &format!(
            r#"<datafile><header><name>One</name></header>
<game name="Tetris (Japan)"><rom name="a.bin" size="4" sha1="{SHA1_TEST}"/></game>
<game name="Tetris (Asia)" cloneof="Tetris (Japan)"><rom name="b.bin" size="3" sha1="{SHA1_ABC}"/></game>
</datafile>"#
        ),
    );
    let mut state = base_state(&fixture);
    state.dat_path_draft = dat.display().to_string();
    state.source_root_draft = source.display().to_string();
    state.destination_root_draft = fixture.path("playing").display().to_string();
    // No preferences configured at all: nothing can distinguish the tie.

    state.preview();

    let plan = state.plan().expect("plan");
    assert!(plan.elected_games.is_empty());
    assert_eq!(plan.unresolved_groups.len(), 1);
    assert_eq!(plan.unresolved_groups[0].tied_candidates.len(), 2);
}

#[test]
fn a_destination_conflict_blocks_that_operation_from_applying() {
    let fixture = Fixture::new("conflict");
    let source_a = fixture.path("a");
    let source_b = fixture.path("b");
    std::fs::create_dir_all(&source_a).unwrap();
    std::fs::create_dir_all(&source_b).unwrap();
    std::fs::write(source_a.join("game.bin"), b"test").unwrap();
    std::fs::write(source_b.join("game.bin"), b"abc").unwrap();
    // A single flat source folder is what the page actually scans; nest
    // both same-named files under it so the resulting elections collide on
    // one destination basename exactly like the core-level conflict test.
    let source = fixture.path("roms");
    std::fs::create_dir_all(source.join("a")).unwrap();
    std::fs::create_dir_all(source.join("b")).unwrap();
    std::fs::write(source.join("a").join("game.bin"), b"test").unwrap();
    std::fs::write(source.join("b").join("game.bin"), b"abc").unwrap();
    let dat = write_dat(
        &fixture,
        "one.dat",
        &format!(
            r#"<datafile><header><name>One</name></header>
<game name="Family One (Europe)"><rom name="game.bin" size="4" sha1="{SHA1_TEST}"/></game>
<game name="Family Two (Japan)"><rom name="game.bin" size="3" sha1="{SHA1_ABC}"/></game>
</datafile>"#
        ),
    );
    let mut state = base_state(&fixture);
    state.dat_path_draft = dat.display().to_string();
    state.source_root_draft = source.display().to_string();
    state.destination_root_draft = fixture.path("playing").display().to_string();

    state.preview();

    let plan = state.plan().expect("plan");
    assert_eq!(plan.elected_games.len(), 2);
    assert_eq!(plan.conflicts.len(), 1);
    assert!(plan.operations.is_empty());

    state.request_apply();
    confirm_library(&mut state);

    assert!(
        state.apply_error().unwrap().contains("conflict"),
        "applying while a conflict remains must be refused: {:?}",
        state.apply_error()
    );
    assert!(state.applied().is_none());
    assert!(!fixture.path("playing").exists());
}

// --- plain-English election explanation ---------------------------------

#[test]
fn the_page_shows_a_plain_winner_explanation_and_why_the_loser_lost_with_technical_detail_hidden() {
    let fixture = Fixture::new("explanation");
    let source = fixture.path("roms");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("europe.bin"), b"test").unwrap();
    std::fs::write(source.join("usa.bin"), b"abc").unwrap();
    let dat = write_dat(
        &fixture,
        "one.dat",
        &format!(
            r#"<datafile><header><name>One</name></header>
<game name="Sonic (Europe)"><rom name="europe.bin" size="4" sha1="{SHA1_TEST}"/></game>
<game name="Sonic (USA)" cloneof="Sonic (Europe)"><rom name="usa.bin" size="3" sha1="{SHA1_ABC}"/></game>
</datafile>"#
        ),
    );
    let mut state = base_state(&fixture);
    state.dat_path_draft = dat.display().to_string();
    state.source_root_draft = source.display().to_string();
    state.destination_root_draft = fixture.path("playing").display().to_string();
    state.preferred_regions_draft = "Europe".to_string();
    state.preview();
    assert_eq!(state.plan().expect("plan").elected_games.len(), 1);

    let ctx = egui::Context::default();
    let (output, action) = click_text(&ctx, &mut state, "Why this one?");
    if let Some(PlayingLibraryPageAction::SelectFamily(family)) = action {
        state.select_family(family);
    }
    let (expanded, _) = render(&ctx, &mut state, base_input());
    let _ = output;

    assert!(rendered_text_contains(
        &expanded,
        "EmuWiz prefers this release:"
    ));
    assert!(rendered_text_contains(&expanded, "Not selected:"));
    assert!(rendered_text_contains(
        &expanded,
        "Not selected: Sonic (USA)"
    ));
    // The plain-English evidence line is always visible...
    assert!(
        rendered_text_contains(&expanded, "region: Europe")
            || rendered_text_contains(
                &expanded,
                "region: Europe - language: unknown - revision: unknown - declared parent"
            )
    );
    // ...but the raw structured debug dump stays behind "Technical details"
    // until that header is expanded.
    assert!(rendered_text_contains(&expanded, "Technical details"));
    assert!(!rendered_text_contains(
        &expanded,
        "CandidateEvidenceSummary {"
    ));

    let (with_technical_detail, _) = click_text(&ctx, &mut state, "Technical details");
    assert!(rendered_text_contains(
        &with_technical_detail,
        "CandidateEvidenceSummary {"
    ));
}

// --- apply through the existing linked-library transaction seam --------

fn preview_a_single_election(fixture: &Fixture) -> (PlayingLibraryPageState, PathBuf, PathBuf) {
    let source = fixture.path("roms");
    std::fs::create_dir_all(&source).unwrap();
    let original = source.join("europe.bin");
    std::fs::write(&original, b"test").unwrap();
    let dat = write_dat(
        fixture,
        "one.dat",
        &format!(
            r#"<datafile><header><name>One</name></header>
<game name="Sonic (Europe)"><rom name="europe.bin" size="4" sha1="{SHA1_TEST}"/></game>
</datafile>"#
        ),
    );
    let destination = fixture.path("playing");
    let mut state = base_state(fixture);
    state.dat_path_draft = dat.display().to_string();
    state.source_root_draft = source.display().to_string();
    state.destination_root_draft = destination.display().to_string();
    state.preview();
    assert_eq!(
        state.plan().expect("plan").elected_games.len(),
        1,
        "fixture must produce exactly one election"
    );
    (state, original, destination)
}

fn retrodeck_profile(
    fixture: &Fixture,
) -> archivefs_core::emulator_environment::es_de::EsDeProfile {
    let home = fixture.path("retrodeck-home");
    std::fs::create_dir_all(home.join("ES-DE/custom_systems")).unwrap();
    let systems = format!(
        "<systemList><system><name>psx</name><fullname>PlayStation</fullname><path>{}/roms/psx</path><extension>.cue .bin</extension><platform>psx</platform><theme>psx</theme></system></systemList>",
        home.display()
    );
    std::fs::write(home.join("ES-DE/custom_systems/es_systems.xml"), systems).unwrap();
    discover_es_de_environment(
        &archivefs_core::emulator_environment::HostReadOnlyFilesystem,
        &DiscoveryEnvironment {
            home: Some(home.into_os_string()),
            path: Some("".into()),
            explicit_bundled_systems_files: vec![],
            appimage_search_roots: vec![],
            explicit_root: None,
            explicit_appimages: vec![],
            explicit_portables: vec![],
        },
    )
    .unwrap()
    .profiles
    .into_iter()
    .find(|profile| !profile.system_data.is_empty())
    .unwrap()
}

fn install_retrodeck_projection(
    state: &mut PlayingLibraryPageState,
    fixture: &Fixture,
    visibility: RetroDeckVisibility,
) {
    let plan = state.plan.clone().unwrap();
    let identity = state.dat_platform_identity.clone().unwrap();
    let profile = retrodeck_profile(fixture);
    let gamelist = PathBuf::from(
        &profile
            .system_data
            .iter()
            .find(|entry| entry.system_name == "psx")
            .unwrap()
            .gamelist_file
            .path
            .display,
    );
    std::fs::create_dir_all(gamelist.parent().unwrap()).unwrap();
    std::fs::write(&gamelist, "<gameList>\n<!-- keep -->\n</gameList>\n").unwrap();
    let projection = build_retrodeck_projection(
        &plan,
        &identity,
        fixture.path("retrodeck-destination"),
        visibility,
        &profile,
    )
    .unwrap();
    state.output_projection = Some(LibraryOutputProjection::from_plan_with_ownership(
        LibraryOutputProfile::RetroDeck,
        projection.destination_root.clone(),
        &projection.playing_library_plan,
        Vec::new(),
        &inspect_library_output_ownership(&projection.playing_library_plan, &state.journal_dir),
    ));
    state.retain_retrodeck_projection(projection);
}

#[test]
fn retrodeck_gui_apply_requires_confirmation_then_publishes_and_rolls_back() {
    let fixture = Fixture::new("retrodeck-gui-success");
    let (mut state, original, _destination) = preview_a_single_election(&fixture);
    state.set_destination(PlayingLibraryDestination::RetroDeck);
    state.preview();
    state.dat_platform_identity = Some(DatPlatformIdentity::Resolved {
        platform: "PSX".into(),
        machine_key: None,
        confidence: DatPlatformConfidence::Strong,
        evidence: Vec::new(),
    });
    install_retrodeck_projection(
        &mut state,
        &fixture,
        RetroDeckVisibility::verified_same_path_bind(
            fixture.path("roms"),
            fixture.path("retrodeck-destination"),
        )
        .unwrap(),
    );
    let (gamelist, destinations) = {
        let projection = state.retrodeck_projection.as_ref().unwrap();
        assert_eq!(projection.es_de_publication.added.len(), 1);
        (
            projection.es_de_publication.gamelist_path.clone(),
            projection
                .playing_library_plan
                .operations
                .iter()
                .map(|op| op.destination_path.clone())
                .collect::<Vec<_>>(),
        )
    };
    state.request_retrodeck_apply();
    assert!(state.retrodeck_pending_apply);
    assert!(!destinations.iter().any(|path| path.exists()));
    confirm_retrodeck(&mut state);
    assert!(
        state.retrodeck_error.is_none(),
        "{:?}",
        state.retrodeck_error
    );
    assert!(state.retrodeck_applied.is_some());
    assert!(
        std::fs::read_to_string(&gamelist)
            .unwrap()
            .contains("<game>")
    );
    assert_eq!(std::fs::read(&original).unwrap(), b"test");
    let ctx = egui::Context::default();
    let receipt_input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 2600.0),
        )),
        ..Default::default()
    };
    let (output, _) = render(&ctx, &mut state, receipt_input());
    assert!(rendered_text_contains(&output, "RetroDECK library created"));
    state.rollback_retrodeck();
    assert_eq!(
        std::fs::read_to_string(&gamelist).unwrap(),
        "<gameList>\n<!-- keep -->\n</gameList>\n"
    );
    assert!(!destinations.iter().any(|path| path.exists()));
    let (output, _) = render(&ctx, &mut state, receipt_input());
    assert!(rendered_text_contains(
        &output,
        "RetroDECK library rolled back"
    ));
}

#[test]
fn retrodeck_gui_cancel_and_publication_failure_make_no_false_success() {
    let fixture = Fixture::new("retrodeck-gui-failure");
    let (mut state, original, _destination) = preview_a_single_election(&fixture);
    state.dat_platform_identity = Some(DatPlatformIdentity::Resolved {
        platform: "PSX".into(),
        machine_key: None,
        confidence: DatPlatformConfidence::Strong,
        evidence: Vec::new(),
    });
    install_retrodeck_projection(
        &mut state,
        &fixture,
        RetroDeckVisibility::verified_same_path_bind(
            fixture.path("roms"),
            fixture.path("retrodeck-destination"),
        )
        .unwrap(),
    );
    let (gamelist, destinations) = {
        let projection = state.retrodeck_projection.as_ref().unwrap();
        (
            projection.es_de_publication.gamelist_path.clone(),
            projection
                .playing_library_plan
                .operations
                .iter()
                .map(|op| op.destination_path.clone())
                .collect::<Vec<_>>(),
        )
    };
    state.request_retrodeck_apply();
    state.cancel_retrodeck_apply();
    assert!(state.retrodeck_applied.is_none());
    assert!(!destinations.iter().any(|path| path.exists()));
    let parent = gamelist.parent().unwrap().to_path_buf();
    std::fs::remove_dir_all(&parent).unwrap();
    std::fs::create_dir_all(parent.parent().unwrap()).unwrap();
    std::fs::write(&parent, b"blocking file").unwrap();
    state.request_retrodeck_apply();
    confirm_retrodeck(&mut state);
    assert!(state.retrodeck_applied.is_none());
    assert!(state.retrodeck_error.is_some());
    assert!(!destinations.iter().any(|path| path.exists()));
    assert_eq!(std::fs::read(&original).unwrap(), b"test");
}

#[test]
fn create_playing_library_uses_the_existing_linked_library_transaction_seam() {
    let fixture = Fixture::new("apply-seam");
    let (mut state, original, destination) = preview_a_single_election(&fixture);

    state.request_apply();
    confirm_library(&mut state);

    assert!(state.apply_error().is_none(), "{:?}", state.apply_error());
    let transaction = state.applied().expect("an applied transaction");
    assert_eq!(transaction.state, TransactionState::Applied);
    assert_eq!(transaction.applied_count(), 1);

    // The journal this produced is a real, ordinary rename_apply journal,
    // readable through the same public API every other apply path uses -
    // proof this ran through the existing engine, not a new one.
    let on_disk = archivefs_core::dat::rename_apply::read_journal(
        &archivefs_core::dat::rename_apply::journal_path(
            &state.journal_dir,
            &transaction.transaction_id,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(on_disk.state, TransactionState::Applied);

    let link = destination.join("unknown/europe.bin");
    assert!(link.is_symlink());
    assert_eq!(std::fs::read_link(&link).unwrap(), original);
}

#[test]
fn every_batch_requires_the_exact_link_confirmation() {
    let fixture = Fixture::new("typed-one-link");
    let (mut state, _, destination) = preview_a_single_election(&fixture);
    state.request_apply();
    state.confirm_apply();
    assert!(state.apply_error().unwrap().contains("typed confirmation"));
    assert!(!destination.join("unknown/europe.bin").exists());
    confirm_library(&mut state);
    assert!(state.apply_error().is_none());
}

#[test]
fn compact_1024_by_600_view_keeps_the_main_action_reachable() {
    let fixture = Fixture::new("compact-action");
    let (mut state, _, _) = preview_a_single_election(&fixture);
    let ctx = egui::Context::default();
    let small = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 600.0),
        )),
        ..Default::default()
    };
    let (output, _) = render(&ctx, &mut state, small.clone());
    let center = find_exact_text_center(&output, "Create reviewed links").expect("create action");
    assert!(
        center.y < 600.0,
        "create action must be in the compact viewport: {center:?}"
    );
    let clicked = egui::RawInput {
        events: click_event(center),
        ..small
    };
    let (_, action) = render(&ctx, &mut state, clicked);
    assert!(matches!(
        action,
        Some(PlayingLibraryPageAction::RequestApply)
    ));
}

#[test]
fn a_replaced_source_refuses_the_reviewed_preview() {
    let fixture = Fixture::new("source-swap");
    let (mut state, original, destination) = preview_a_single_election(&fixture);
    let replacement = fixture.path("replacement.bin");
    std::fs::write(&replacement, b"test").unwrap();
    std::fs::rename(&replacement, &original).unwrap();
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().unwrap().contains("source changed"));
    assert!(!destination.join("unknown/europe.bin").exists());
}

#[test]
fn a_changed_dat_refuses_the_reviewed_preview() {
    let fixture = Fixture::new("dat-change");
    let (mut state, _, destination) = preview_a_single_election(&fixture);
    std::fs::write(
        &state.dat_path_draft,
        b"<datafile><header><name>changed</name></header></datafile>",
    )
    .unwrap();
    state.request_apply();
    confirm_library(&mut state);
    assert!(
        state
            .apply_error()
            .unwrap()
            .contains("collection or DAT changed")
    );
    assert!(!destination.join("unknown/europe.bin").exists());
}

#[test]
fn a_new_preview_creates_only_new_links_after_an_earlier_receipt() {
    let fixture = Fixture::new("incremental-library");
    let (mut state, original, destination) = preview_a_single_election(&fixture);
    state.request_apply();
    confirm_library(&mut state);
    let first_receipt = state.applied().unwrap().transaction_id.clone();
    let second = original.parent().unwrap().join("second.bin");
    std::fs::write(&second, b"abc").unwrap();
    std::fs::write(
        &state.dat_path_draft,
        format!(
            r#"<datafile><header><name>Two</name></header>
<game name="First (Europe)"><rom name="europe.bin" size="4" sha1="{SHA1_TEST}"/></game>
<game name="Second (Europe)"><rom name="second.bin" size="3" sha1="{SHA1_ABC}"/></game>
</datafile>"#
        ),
    )
    .unwrap();
    state.preview();
    assert_eq!(state.output_projection.as_ref().unwrap().operation_count, 1);
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().is_none(), "{:?}", state.apply_error());
    assert_ne!(state.applied().unwrap().transaction_id, first_receipt);
    assert_eq!(state.applied().unwrap().applied_count(), 1);
    assert!(destination.join("unknown/europe.bin").is_symlink());
    assert!(destination.join("unknown/second.bin").is_symlink());
}

#[test]
fn a_history_receipt_can_undo_links_after_page_restart() {
    let fixture = Fixture::new("history-restart-undo");
    let (mut state, original, destination) = preview_a_single_election(&fixture);
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().is_none(), "{:?}", state.apply_error());
    let receipt = state.applied().unwrap().clone();
    let mut restored = PlayingLibraryPageState::with_journal_dir(state.journal_dir.clone());
    restored.load_generic_history_receipt(receipt);
    restored.rollback_last();
    assert!(
        restored.apply_error().is_none(),
        "{:?}",
        restored.apply_error()
    );
    assert!(!destination.join("unknown/europe.bin").exists());
    assert_eq!(std::fs::read(original).unwrap(), b"test");
}

#[cfg(unix)]
#[test]
fn a_source_replaced_by_a_symlink_never_becomes_a_link_target() {
    let fixture = Fixture::new("source-symlink-swap");
    let (mut state, original, destination) = preview_a_single_election(&fixture);
    let outside = fixture.path("outside.rom");
    std::fs::write(&outside, b"test").unwrap();
    std::fs::remove_file(&original).unwrap();
    std::os::unix::fs::symlink(&outside, &original).unwrap();
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().is_some());
    assert!(!destination.join("unknown/europe.bin").exists());
    assert_eq!(std::fs::read(&outside).unwrap(), b"test");
}

#[cfg(unix)]
#[test]
fn a_symlinked_destination_ancestor_is_refused_before_creating_links() {
    let fixture = Fixture::new("destination-symlink");
    let (mut state, _, destination) = preview_a_single_election(&fixture);
    let outside = fixture.path("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &destination).unwrap();
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().unwrap().contains("unsafe"));
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
}

#[test]
fn original_source_files_remain_untouched_after_apply() {
    let fixture = Fixture::new("untouched");
    let (mut state, original, _destination) = preview_a_single_election(&fixture);

    state.request_apply();
    confirm_library(&mut state);

    assert!(state.apply_error().is_none());
    assert!(original.exists(), "the original file must still exist");
    assert!(
        !original.is_symlink(),
        "the original must remain a real file, never converted to a link"
    );
    assert_eq!(std::fs::read(&original).unwrap(), b"test");
}

#[test]
fn successful_apply_creates_symlinks_only_nothing_else_under_destination() {
    let fixture = Fixture::new("symlinks-only");
    let (mut state, _original, destination) = preview_a_single_election(&fixture);

    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().is_none());

    let entries: Vec<_> = std::fs::read_dir(destination.join("unknown"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "exactly one destination entry: {entries:?}"
    );
    let metadata = std::fs::symlink_metadata(&entries[0]).unwrap();
    assert!(
        metadata.is_symlink(),
        "the sole destination entry must be a symlink"
    );
}

#[test]
fn romm_preview_shows_slug_counts_visibility_and_keeps_details_collapsed() {
    let fixture = Fixture::new("romm-preview");
    let (mut state, _original, _destination) = preview_a_single_election(&fixture);
    state.set_destination(PlayingLibraryDestination::Romm);
    state.preview();
    state.dat_platform_identity = Some(DatPlatformIdentity::Resolved {
        platform: "Game Boy Advance".to_string(),
        machine_key: None,
        confidence: DatPlatformConfidence::Strong,
        evidence: Vec::new(),
    });
    state.preview_romm();
    let ctx = egui::Context::default();
    let (output, action) = render(&ctx, &mut state, base_input());
    assert!(action.is_none());
    assert!(rendered_text_contains(&output, "Destination:"));
    assert!(rendered_text_contains(
        &output,
        "reviewed RomM platform `gba`"
    ));
    assert!(rendered_text_contains(&output, "1 game(s), 1 file(s)"));
    assert!(rendered_text_contains(&output, "Visibility: Needs setup"));
    assert!(rendered_text_contains(&output, "Apply is blocked"));
    assert!(!rendered_text_contains(&output, "Launcher:"));
}

#[test]
fn retrodeck_preview_card_is_visible_and_unverified_apply_is_blocked() {
    let fixture = Fixture::new("retrodeck-preview");
    let (mut state, _original, _destination) = preview_a_single_election(&fixture);
    state.set_destination(PlayingLibraryDestination::RetroDeck);
    state.preview();
    state.dat_platform_identity = Some(DatPlatformIdentity::Resolved {
        platform: "PSX".to_string(),
        machine_key: None,
        confidence: DatPlatformConfidence::Strong,
        evidence: Vec::new(),
    });
    let ctx = egui::Context::default();
    let (output, action) = render(&ctx, &mut state, base_input());
    assert!(action.is_none());
    assert!(rendered_text_contains(&output, "Build RetroDECK Library"));
    assert!(rendered_text_contains(
        &output,
        "RetroDECK destination root:"
    ));
    assert!(rendered_text_contains(
        &output,
        "Sandbox-visible source root:"
    ));
    assert!(!rendered_text_contains(&output, "Create RetroDECK Library"));
}

#[test]
fn romm_apply_requires_verified_visibility_then_applies_and_rolls_back() {
    let fixture = Fixture::new("romm-apply");
    let (mut state, original, destination) = preview_a_single_election(&fixture);
    state.set_destination(PlayingLibraryDestination::Romm);
    state.preview();
    state.dat_platform_identity = Some(DatPlatformIdentity::Resolved {
        platform: "Game Boy Advance".to_string(),
        machine_key: None,
        confidence: DatPlatformConfidence::Strong,
        evidence: Vec::new(),
    });
    state.preview_romm();
    state.request_romm_apply();
    confirm_romm(&mut state);
    assert!(state.romm_applied.is_none());
    assert!(
        state
            .romm_error
            .as_deref()
            .unwrap()
            .contains("not verified visible"),
        "unexpected RomM error: {:?}",
        state.romm_error
    );

    state.romm_visibility_verified = true;
    state.romm_visible_source_root_draft = state.source_root_draft.clone();
    state.preview_romm();
    state.request_romm_apply();
    confirm_romm(&mut state);
    assert!(state.romm_error.is_none(), "{:?}", state.romm_error);
    let transaction = state.romm_applied.as_ref().expect("RomM transaction");
    assert_eq!(transaction.state, TransactionState::Applied);
    let link = destination.join("roms/gba/europe.bin");
    assert!(link.is_symlink());
    assert_eq!(std::fs::read_link(&link).unwrap(), original);
    assert_eq!(std::fs::read(&original).unwrap(), b"test");
    let ctx = egui::Context::default();
    // This is a receipt/transaction assertion, not a viewport-accessibility test.
    // The review summary and asynchronously loaded catalogue can push the
    // profile receipt below the old 1600px fixture. Render the complete profile
    // content while keeping every transaction and message assertion intact.
    let receipt_input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 2600.0),
        )),
        ..Default::default()
    };
    let (applied_output, _) = render(&ctx, &mut state, receipt_input());
    assert!(rendered_text_contains(
        &applied_output,
        "RomM library created: 1 link(s)"
    ));

    state.rollback_romm_last();
    assert!(!link.exists());
    assert_eq!(std::fs::read(&original).unwrap(), b"test");
    assert_eq!(
        state.romm_applied.as_ref().unwrap().state,
        TransactionState::RolledBack
    );
    let (rolled_back_output, _) = render(&ctx, &mut state, receipt_input());
    assert!(rendered_text_contains(
        &rolled_back_output,
        "RomM library rolled back; no generated links remain."
    ));
}

#[test]
fn rollback_uses_the_existing_journal_backed_rollback_path() {
    let fixture = Fixture::new("rollback");
    let (mut state, original, destination) = preview_a_single_election(&fixture);
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().is_none());
    let link = destination.join("unknown/europe.bin");
    assert!(link.is_symlink());

    state.rollback_last();

    assert!(state.apply_error().is_none(), "{:?}", state.apply_error());
    assert!(!link.exists(), "rollback must remove the created symlink");
    assert!(original.exists(), "rollback must never touch the original");
    assert_eq!(std::fs::read(&original).unwrap(), b"test");

    // The rollback is recorded in the very same journal file apply wrote -
    // the existing rename_apply journal path, not a second bookkeeping
    // system.
    let transaction_id = state.applied().unwrap().transaction_id.clone();
    let on_disk = archivefs_core::dat::rename_apply::read_journal(
        &archivefs_core::dat::rename_apply::journal_path(&state.journal_dir, &transaction_id)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(on_disk.state, TransactionState::RolledBack);
}

// --- explanation reuses ElectionExplanation, invents nothing new --------

#[test]
fn selecting_a_family_exposes_its_own_election_explanation_unmodified() {
    let fixture = Fixture::new("explanation");
    let (mut state, _original, _destination) = preview_a_single_election(&fixture);
    let name = state.plan().unwrap().elected_games[0]
        .dat_entry_name
        .clone();

    assert!(state.selected_family().is_none());
    state.select_family(Some(name.clone()));
    assert_eq!(state.selected_family(), Some(name.as_str()));
    state.select_family(None);
    assert!(state.selected_family().is_none());
}

// --- real widget interaction: driven through the rendered page, not by ---
// --- mutating state directly in test setup -------------------------------

#[test]
fn the_dat_path_field_can_be_edited_through_the_real_text_widget() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();

    type_into_field(&ctx, &mut state, DAT_PATH_FIELD_ID, "/tmp/library.dat");

    assert_eq!(state.dat_path_draft, "/tmp/library.dat");
}

#[test]
fn the_source_path_field_can_be_edited_through_the_real_text_widget() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();

    type_into_field(&ctx, &mut state, SOURCE_ROOT_FIELD_ID, "/tmp/roms");

    assert_eq!(state.source_root_draft, "/tmp/roms");
}

#[test]
fn the_destination_path_field_can_be_edited_through_the_real_text_widget() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();

    type_into_field(&ctx, &mut state, DESTINATION_ROOT_FIELD_ID, "/tmp/playing");

    assert_eq!(state.destination_root_draft, "/tmp/playing");
}

#[test]
fn the_revision_checkbox_toggles_through_a_real_click() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();
    assert!(state.prefer_newest_revision, "default is on");

    click_text(&ctx, &mut state, "Prefer newest verified revision");
    assert!(!state.prefer_newest_revision);

    click_text(&ctx, &mut state, "Prefer newest verified revision");
    assert!(state.prefer_newest_revision);
}

#[test]
fn the_parent_checkbox_toggles_through_a_real_click() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();
    assert!(state.prefer_parent, "default is on");

    click_text(&ctx, &mut state, "Prefer declared parent");
    assert!(!state.prefer_parent);
}

#[test]
fn each_release_class_exclusion_checkbox_toggles_through_a_real_click() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();
    assert!(
        state.exclude_beta && state.exclude_proto && state.exclude_demo && state.exclude_sample
    );

    click_text(&ctx, &mut state, "Beta");
    assert!(!state.exclude_beta);
    click_text(&ctx, &mut state, "Proto");
    assert!(!state.exclude_proto);
    click_text(&ctx, &mut state, "Demo");
    assert!(!state.exclude_demo);
    click_text(&ctx, &mut state, "Sample");
    assert!(!state.exclude_sample);

    assert!(
        !state.exclude_beta && !state.exclude_proto && !state.exclude_demo && !state.exclude_sample
    );
}

#[test]
fn editing_the_region_field_through_the_widget_updates_the_built_policy() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();
    // The field defaults to a non-empty draft ("Europe, USA, Japan");
    // clear it first so typing produces an exact, predictable result to
    // assert on, rather than testing the unrelated "typing appends at the
    // cursor" behaviour every ordinary text field already has.
    state.preferred_regions_draft.clear();

    type_into_field(
        &ctx,
        &mut state,
        PREFERRED_REGIONS_FIELD_ID,
        "Japan, Europe",
    );

    assert_eq!(
        state.build_policy().preferred_regions,
        vec!["Japan", "Europe"]
    );
}

#[test]
fn editing_the_language_field_through_the_widget_updates_the_built_policy() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();

    type_into_field(&ctx, &mut state, PREFERRED_LANGUAGES_FIELD_ID, "English");

    assert_eq!(state.build_policy().preferred_languages, vec!["en"]);
}

#[test]
fn preview_is_disabled_until_every_required_field_is_filled_then_becomes_clickable() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();

    // Nothing filled in yet: clicking where the button is must not produce
    // an action, and the disabled hint must be visible.
    let (output, action) = click_text(&ctx, &mut state, "Preview playing library");
    assert!(action.is_none(), "a disabled button must never click");
    assert!(rendered_text_contains(
        &output,
        "Choose a DAT catalogue, a source, and a destination first."
    ));

    type_into_field(&ctx, &mut state, DAT_PATH_FIELD_ID, "/tmp/library.dat");
    type_into_field(&ctx, &mut state, SOURCE_ROOT_FIELD_ID, "/tmp/roms");
    type_into_field(&ctx, &mut state, DESTINATION_ROOT_FIELD_ID, "/tmp/playing");

    let (output, action) = click_text(&ctx, &mut state, "Preview playing library");
    assert!(
        !rendered_text_contains(
            &output,
            "Choose a DAT catalogue, a source, and a destination first."
        ),
        "the disabled hint must disappear once every required field is filled"
    );
    assert!(
        matches!(action, Some(PlayingLibraryPageAction::Preview)),
        "an enabled button must click"
    );
}

#[test]
fn an_invalid_legacy_dat_path_is_reported_by_the_test_fallback() {
    let fixture = Fixture::new("invalid-dat");
    let source = fixture.path("roms");
    std::fs::create_dir_all(&source).unwrap();
    let mut state = base_state(&fixture);
    state.dat_path_draft = fixture.path("missing.dat").display().to_string();
    state.source_root_draft = source.display().to_string();
    state.destination_root_draft = fixture.path("playing").display().to_string();

    state.preview();

    assert!(
        state
            .error()
            .unwrap()
            .contains("could not read the DAT catalogue")
    );
}

#[test]
fn an_invalid_source_folder_shows_a_plain_inline_error() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();

    type_into_field(
        &ctx,
        &mut state,
        SOURCE_ROOT_FIELD_ID,
        "/definitely/does/not/exist/roms",
    );
    let (output, _) = render(&ctx, &mut state, base_input());

    assert!(rendered_text_contains(
        &output,
        "This folder was not found."
    ));
}

#[test]
fn a_real_existing_path_shows_no_inline_error() {
    let fixture = Fixture::new("valid-path-render");
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();

    type_into_field(
        &ctx,
        &mut state,
        SOURCE_ROOT_FIELD_ID,
        &fixture.dir.display().to_string(),
    );
    let (output, _) = render(&ctx, &mut state, base_input());

    assert!(!rendered_text_contains(
        &output,
        "This folder was not found."
    ));
}

// --- "Add to ES-DE / RetroDECK" -------------------------------------------------

mod esde {
    use archivefs_core::launch::es_de_publish::es_de_gamelist_recovery_path;

    use super::*;

    const PSX_SYSTEMS_XML: &str = r#"<systemList>
    <system>
        <name>psx</name>
        <fullname>Sony PlayStation</fullname>
        <path>%ROMPATH%/psx</path>
        <extension>.chd .cue</extension>
        <command>retroarch %ROM%</command>
        <platform>psx</platform>
        <theme>psx</theme>
    </system>
</systemList>"#;

    /// A `~/ES-DE`-shaped home directory declaring one `psx` system, ready
    /// for `PlayingLibraryPageState::with_esde_home_override`.
    fn esde_home(fixture: &Fixture) -> PathBuf {
        let home = fixture.path("es-de-home");
        std::fs::create_dir_all(home.join("custom_systems")).unwrap();
        std::fs::write(home.join("custom_systems/es_systems.xml"), PSX_SYSTEMS_XML).unwrap();
        home
    }

    fn gamelist_path(home: &std::path::Path) -> PathBuf {
        home.join("gamelists/psx/gamelist.xml")
    }

    /// Builds a real, applied playing library (one election, real symlink
    /// on disk) plus a real ES-DE home with a `psx` system - the shared
    /// starting point for every "Add to ES-DE / RetroDECK" test.
    fn library_and_esde_home(tag: &str) -> (Fixture, PlayingLibraryPageState) {
        let fixture = Fixture::new(tag);
        let (mut state, _original, _destination) = preview_a_single_election(&fixture);
        state.request_apply();
        confirm_library(&mut state);
        assert!(state.apply_error().is_none(), "{:?}", state.apply_error());
        assert!(state.applied().is_some());

        let home = esde_home(&fixture);
        let state = state.with_esde_home_override(home);
        (fixture, state)
    }

    #[test]
    fn publish_section_is_absent_before_the_library_is_created() {
        let ctx = egui::Context::default();
        let fixture = Fixture::new("esde-absent-before-create");
        let (mut state, _original, _destination) = preview_a_single_election(&fixture);

        let (output, _) = render(&ctx, &mut state, base_input());
        assert!(!rendered_text_contains(&output, "Add to ES-DE / RetroDECK"));
    }

    #[test]
    fn publish_section_appears_once_the_library_is_created() {
        let ctx = egui::Context::default();
        let (_fixture, mut state) = library_and_esde_home("esde-appears-after-create");

        let (output, _) = render(&ctx, &mut state, base_input());
        assert!(rendered_text_contains(&output, "Add to ES-DE / RetroDECK"));
    }

    #[test]
    fn publish_section_disappears_again_after_rollback() {
        let ctx = egui::Context::default();
        let (_fixture, mut state) = library_and_esde_home("esde-hidden-after-rollback");
        state.rollback_last();
        assert!(state.apply_error().is_none());

        let (output, _) = render(&ctx, &mut state, base_input());
        assert!(!rendered_text_contains(&output, "Add to ES-DE / RetroDECK"));
    }

    #[test]
    fn preview_never_writes_anything() {
        let (fixture, mut state) = library_and_esde_home("esde-preview-non-mutating");
        state.select_esde_platform(Some("PSX"));

        state.preview_esde_publication();

        assert!(
            state.esde_discovery_error().is_none(),
            "{:?}",
            state.esde_discovery_error()
        );
        assert!(
            state.esde_preview_error().is_none(),
            "{:?}",
            state.esde_preview_error()
        );
        let publication = state.esde_publication().expect("a preview");
        assert_eq!(publication.added.len(), 1);
        assert!(!gamelist_path(&fixture.path("es-de-home")).exists());
    }

    #[test]
    fn existing_es_de_entries_are_reported_as_already_present() {
        let (fixture, mut state) = library_and_esde_home("esde-already-present");
        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();
        let elected_path = state
            .esde_publication()
            .unwrap()
            .added
            .first()
            .unwrap()
            .destination_path
            .clone();

        // Seed a gamelist that already contains this exact election, plus
        // one unrelated pre-existing entry. The fixture's own path never
        // contains XML-significant characters, so no escaping is needed
        // here.
        let escaped = elected_path.to_string_lossy().into_owned();
        let path = gamelist_path(&fixture.path("es-de-home"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "<?xml version=\"1.0\"?>\n<gameList>\n\t<game>\n\t\t<path>/library/Unrelated Game.chd</path>\n\t\t<name>Unrelated Game</name>\n\t</game>\n\t<game>\n\t\t<path>{escaped}</path>\n\t\t<name>Sonic (Europe)</name>\n\t</game>\n</gameList>\n"
            ),
        )
        .unwrap();

        state.preview_esde_publication();

        let publication = state.esde_publication().expect("a preview");
        assert!(publication.added.is_empty());
        assert_eq!(publication.already_present.len(), 1);
        assert!(publication.is_unchanged());
    }

    #[test]
    fn publish_requires_explicit_confirmation() {
        let (fixture, mut state) = library_and_esde_home("esde-requires-confirm");
        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();
        assert!(state.esde_publication().is_some());

        state.request_esde_publish();
        assert!(state.esde_pending_publish());
        assert!(!state.esde_published());
        assert!(!gamelist_path(&fixture.path("es-de-home")).exists());
    }

    #[test]
    fn cancelling_the_publish_confirmation_does_nothing() {
        let (fixture, mut state) = library_and_esde_home("esde-cancel-does-nothing");
        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();

        state.request_esde_publish();
        state.cancel_esde_publish();

        assert!(!state.esde_pending_publish());
        assert!(!state.esde_published());
        assert!(!gamelist_path(&fixture.path("es-de-home")).exists());
    }

    #[test]
    fn confirmed_publish_writes_through_the_core_api_and_shows_the_count() {
        let (fixture, mut state) = library_and_esde_home("esde-confirmed-publish");
        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();
        let expected_added = state.esde_publication().unwrap().added.len();
        assert_eq!(expected_added, 1);

        state.request_esde_publish();
        state.confirm_esde_publish();

        assert!(
            state.esde_publish_error().is_none(),
            "{:?}",
            state.esde_publish_error()
        );
        assert!(state.esde_published());
        let on_disk = std::fs::read_to_string(gamelist_path(&fixture.path("es-de-home"))).unwrap();
        assert!(on_disk.contains("europe.bin") || on_disk.contains("Sonic (Europe)"));
    }

    #[test]
    fn republishing_the_identical_plan_reports_unchanged() {
        let (_fixture, mut state) = library_and_esde_home("esde-idempotent");
        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();
        state.request_esde_publish();
        state.confirm_esde_publish();
        assert!(state.esde_published());

        state.preview_esde_publication();
        let publication = state.esde_publication().expect("a second preview");
        assert!(publication.is_unchanged());
        assert_eq!(publication.already_present.len(), 1);
    }

    #[test]
    fn an_unresolved_recovery_record_blocks_publication_and_offers_restore() {
        let (fixture, mut state) = library_and_esde_home("esde-recovery-blocks");
        let home = fixture.path("es-de-home");
        let path = gamelist_path(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Simulate a crash mid-publish: a recovery record exists, naming
        // this exact gamelist path, with no prior content.
        std::fs::write(
            es_de_gamelist_recovery_path(&path),
            format!(
                "{{\"schema_version\":1,\"gamelist_path\":{:?},\"previous_content\":null}}",
                path.to_string_lossy()
            ),
        )
        .unwrap();

        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();

        assert!(state.esde_publication().is_none());
        assert_eq!(state.esde_recovery_gamelist_path(), Some(path.as_path()));
    }

    #[test]
    fn legacy_recovery_is_refused_and_preserves_current_content_and_evidence() {
        let (fixture, mut state) = library_and_esde_home("esde-recovery-confirm");
        let home = fixture.path("es-de-home");
        let path = gamelist_path(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let previous = "<?xml version=\"1.0\"?>\n<gameList>\n\t<game>\n\t\t<path>/library/Other.chd</path>\n\t\t<name>Other</name>\n\t</game>\n</gameList>\n";
        // A record describing a crash that happened after the real write:
        // the file on disk currently differs from `previous_content`.
        std::fs::write(&path, "<gameList><game><name>corrupted mid-write").unwrap();
        std::fs::write(
            es_de_gamelist_recovery_path(&path),
            format!(
                "{{\"schema_version\":1,\"gamelist_path\":{:?},\"previous_content\":{:?}}}",
                path.to_string_lossy(),
                previous
            ),
        )
        .unwrap();

        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();
        assert!(state.esde_recovery_gamelist_path().is_some());

        state.request_esde_recovery();
        assert!(state.esde_recovery_pending());
        state.confirm_esde_recovery();

        assert!(state.esde_recovery_error().is_some());
        assert!(!state.esde_recovery_done());
        assert_eq!(state.esde_recovery_gamelist_path(), Some(path.as_path()));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "<gameList><game><name>corrupted mid-write"
        );
        assert!(has_unresolved_es_de_gamelist_recovery(&path));
        assert!(
            state
                .esde_recovery_error()
                .unwrap()
                .1
                .unwrap()
                .contains("manual review required")
        );

        // The master ROM and its playing-library link, entirely unrelated
        // to ES-DE's own gamelist, are never touched by recovery.
        let link = fixture.path("playing").join("unknown/europe.bin");
        assert!(link.is_symlink());
        assert_eq!(
            std::fs::read(fixture.path("roms").join("europe.bin")).unwrap(),
            b"test"
        );
    }

    #[test]
    fn cancelling_the_recovery_confirmation_does_nothing() {
        let (fixture, mut state) = library_and_esde_home("esde-recovery-cancel");
        let home = fixture.path("es-de-home");
        let path = gamelist_path(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            es_de_gamelist_recovery_path(&path),
            format!(
                "{{\"schema_version\":1,\"gamelist_path\":{:?},\"previous_content\":null}}",
                path.to_string_lossy()
            ),
        )
        .unwrap();

        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();
        state.request_esde_recovery();
        state.cancel_esde_recovery();

        assert!(!state.esde_recovery_pending());
        assert!(!state.esde_recovery_done());
        assert!(has_unresolved_es_de_gamelist_recovery(&path));
    }

    #[test]
    fn a_malformed_gamelist_produces_a_friendly_refusal_not_raw_xml_detail() {
        let (fixture, mut state) = library_and_esde_home("esde-malformed-friendly");
        let home = fixture.path("es-de-home");
        let path = gamelist_path(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "<gameList><game><name>no closing tag at all").unwrap();

        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();

        assert!(state.esde_publication().is_none());
        let (friendly, detail) = state.esde_preview_error().expect("a friendly error");
        assert!(!friendly.to_lowercase().contains("gamelist"));
        assert!(!friendly.to_lowercase().contains("</"));
        assert!(friendly.contains("ES-DE"));
        // The raw technical detail still exists, but only behind the
        // separate, explicitly-expandable channel - never inside the
        // beginner-facing text itself.
        assert!(detail.unwrap().to_lowercase().contains("gamelist"));
    }

    #[test]
    fn an_oversized_gamelist_also_produces_a_friendly_refusal() {
        let (fixture, mut state) = library_and_esde_home("esde-oversized-friendly");
        let home = fixture.path("es-de-home");
        let path = gamelist_path(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(archivefs_core::launch::es_de_publish::MAX_GAMELIST_BYTES + 1)
            .unwrap();
        drop(file);

        state.select_esde_platform(Some("PSX"));
        state.preview_esde_publication();

        let (friendly, _detail) = state.esde_preview_error().expect("a friendly error");
        assert!(!friendly.contains("MAX_GAMELIST_BYTES"));
        assert!(friendly.contains("large") || friendly.contains("too large"));
    }
}

/// These exercise the presentation against the existing planner/matcher, with
/// disposable files only. Filters and paint must not alter the reviewed plan.
mod organisation_usability {
    use super::*;
    use crate::playing_library_page::presentation::{ROW_LIMIT, ReviewCounts};

    fn review_frame(
        ctx: &egui::Context,
        state: &PlayingLibraryPageState,
        input: egui::RawInput,
    ) -> egui::FullOutput {
        let mut action = None;
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_preview_summary(ui, state.plan().unwrap(), state, &mut action);
            });
        });
        assert!(action.is_none());
        output
    }

    fn preview_output(state: &PlayingLibraryPageState) -> egui::FullOutput {
        review_frame(&egui::Context::default(), state, base_input())
    }

    fn two_releases(fixture: &Fixture, preference: &str) -> PlayingLibraryPageState {
        let source = fixture.path("roms");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("europe.bin"), b"test").unwrap();
        std::fs::write(source.join("usa.bin"), b"abc").unwrap();
        let dat = write_dat(
            fixture,
            "regions.dat",
            &format!(
                r#"<datafile><header><name>One</name></header>
<game name="Sonic (Europe)"><rom name="europe.bin" size="4" sha1="{SHA1_TEST}"/></game>
<game name="Sonic (USA)" cloneof="Sonic (Europe)"><rom name="usa.bin" size="3" sha1="{SHA1_ABC}"/></game></datafile>"#
            ),
        );
        let mut state = base_state(fixture);
        state.source_root_draft = source.display().to_string();
        state.destination_root_draft = fixture.path("playing").display().to_string();
        state.dat_path_draft = dat.display().to_string();
        state.preferred_regions_draft = preference.into();
        state.preview();
        state
    }

    #[test]
    fn source_preservation_and_link_type_visible_without_advanced() {
        let f = Fixture::new("organisation-source");
        let (s, _, _) = preview_a_single_election(&f);
        let output = preview_output(&s);
        assert!(rendered_text_contains(
            &output,
            "original ROMs remain unchanged"
        ));
        assert!(rendered_text_contains(
            &output,
            "links to originals, not copies"
        ));
        assert!(rendered_text_contains(&output, "Undo is available"));
    }

    #[test]
    fn preferred_release_shows_existing_region_reason_normally() {
        let f = Fixture::new("organisation-regions");
        let s = two_releases(&f, "Europe, USA");
        let output = preview_output(&s);
        let step = &s.plan().unwrap().elected_games[0].explanation.steps[0];
        assert!(step.contains("Europe"));
        assert!(rendered_text_contains(&output, step));
        assert!(s.selected_family().is_none());
    }

    #[test]
    fn filename_only_is_needs_review_and_never_selected() {
        let f = Fixture::new("organisation-filename");
        let (mut s, original, _) = preview_a_single_election(&f);
        std::fs::write(&original, b"wrong bytes").unwrap();
        s.preview();
        assert!(s.plan().unwrap().elected_games.is_empty());
        assert_eq!(s.review_paths, vec![original]);
        let output = preview_output(&s);
        assert!(rendered_text_contains(&output, "Needs review"));
        assert!(rendered_text_contains(
            &output,
            "No verified games eligible"
        ));
    }

    #[test]
    fn tied_verified_releases_need_review_in_normal_view() {
        let f = Fixture::new("organisation-tie");
        let s = two_releases(&f, "");
        assert_eq!(s.plan().unwrap().unresolved_groups.len(), 1);
        let output = preview_output(&s);
        assert!(rendered_text_contains(
            &output,
            "not strong enough to choose automatically"
        ));
    }

    #[test]
    fn counts_include_alternatives_and_stay_whole_under_filters() {
        let f = Fixture::new("organisation-counts");
        let mut s = two_releases(&f, "Europe, USA");
        let before = ReviewCounts::from_state(s.plan().unwrap(), &s);
        assert_eq!(before.selected, 1);
        assert_eq!(before.excluded, 1);
        s.preview_filter = PreviewFilter::Excluded;
        assert_eq!(ReviewCounts::from_state(s.plan().unwrap(), &s), before);
        let output = preview_output(&s);
        assert!(rendered_text_contains(
            &output,
            "Selected: 1 · Excluded releases: 1"
        ));
        assert!(rendered_text_contains(
            &output,
            "Sonic (USA) · Excluded alternative"
        ));
        assert!(!rendered_text_contains(
            &output,
            "Sonic (Europe) · Selected"
        ));
    }

    #[test]
    fn filters_only_change_presentation_and_not_input_fingerprint() {
        let f = Fixture::new("organisation-filter");
        let (mut s, _, _) = preview_a_single_election(&f);
        let fingerprint = s.input_fingerprint();
        let plan = s.plan().unwrap().clone();
        s.preview_filter = PreviewFilter::NeedsReview;
        preview_output(&s);
        assert_eq!(s.input_fingerprint(), fingerprint);
        assert_eq!(s.plan().unwrap(), &plan);
        assert!(s.selected_family().is_none());
    }

    #[test]
    fn filtered_empty_and_no_conflicts_have_distinct_messages() {
        let f = Fixture::new("organisation-empty-filter");
        let (mut s, _, _) = preview_a_single_election(&f);
        s.preview_filter = PreviewFilter::NeedsReview;
        assert!(rendered_text_contains(
            &preview_output(&s),
            "Filters hide all results"
        ));
        s.preview_filter = PreviewFilter::Conflicts;
        let output = preview_output(&s);
        assert!(rendered_text_contains(&output, "No conflicts found"));
        assert!(!rendered_text_contains(&output, "Filters hide all results"));
    }

    #[test]
    fn empty_source_is_distinct_from_no_eligible_identity() {
        let f = Fixture::new("organisation-empty-source");
        let (mut s, original, _) = preview_a_single_election(&f);
        std::fs::remove_file(original).unwrap();
        s.preview();
        assert_eq!(s.source_files_examined, 0);
        assert!(rendered_text_contains(
            &preview_output(&s),
            "No games available to organise"
        ));
    }

    #[test]
    fn destination_and_preview_precede_apply() {
        let f = Fixture::new("organisation-before-apply");
        let (mut s, _, dest) = preview_a_single_election(&f);
        let ctx = egui::Context::default();
        let (output, action) = render(&ctx, &mut s, base_input());
        assert!(action.is_none());
        assert!(rendered_text_contains(&output, &dest.display().to_string()));
        let preview = find_exact_text_center(&output, "Preview summary").unwrap();
        let apply = find_exact_text_center(&output, "Apply output profile").unwrap();
        assert!(preview.y < apply.y);
        assert!(!dest.exists());
    }

    #[test]
    fn preview_only_has_explicit_reason_and_no_apply() {
        let f = Fixture::new("organisation-preview-only");
        let (mut s, _, _) = preview_a_single_election(&f);
        s.output_projection = None;
        let (output, action) = render(&egui::Context::default(), &mut s, base_input());
        assert!(action.is_none());
        assert!(rendered_text_contains(&output, "This plan is preview-only"));
        assert!(!rendered_text_contains(&output, "Apply output profile"));
    }

    #[test]
    fn destination_conflict_shows_refusal_without_force_apply() {
        let f = Fixture::new("organisation-conflict");
        let (mut s, _, _) = preview_a_single_election(&f);
        let conflict = archivefs_core::playing_library::DestinationConflict {
            destination_basename: "same.bin".into(),
            contenders: vec!["A".into(), "B".into()],
            destinations: vec![f.path("playing/same.bin")],
        };
        s.plan.as_mut().unwrap().conflicts.push(conflict.clone());
        s.output_projection
            .as_mut()
            .unwrap()
            .conflicts
            .push(conflict);
        let (output, action) = render(&egui::Context::default(), &mut s, base_input());
        assert!(action.is_none());
        assert!(rendered_text_contains(
            &output,
            "Apply unavailable: the destination has conflicts"
        ));
        assert!(!rendered_text_contains(&output, "Apply output profile"));
    }

    #[test]
    fn current_counts_ignore_prior_receipts() {
        let f = Fixture::new("organisation-current");
        let (mut s, _, _) = preview_a_single_election(&f);
        s.request_apply();
        confirm_library(&mut s);
        assert!(s.applied().is_some());
        s.preview();
        s.plan.as_mut().unwrap().elected_games.clear();
        assert_eq!(ReviewCounts::from_state(s.plan().unwrap(), &s).selected, 0);
        let (output, _) = render(&egui::Context::default(), &mut s, base_input());
        assert!(rendered_text_contains(
            &output,
            "this receipt is not current collection evidence"
        ));
    }

    #[test]
    fn advanced_retains_source_output_and_identity_evidence() {
        let f = Fixture::new("organisation-advanced");
        let (s, original, _) = preview_a_single_election(&f);
        let ctx = egui::Context::default();
        let closed = review_frame(&ctx, &s, base_input());
        assert!(!rendered_text_contains(
            &closed,
            &original.display().to_string()
        ));
        // Exercise the real disclosure click in the feature-owned preview.
        // The full page also contains asynchronous catalogue-picker disclosures
        // with the same visible label; they are not this test's target.
        let position = find_exact_text_center(&closed, "Technical details").unwrap();
        review_frame(
            &ctx,
            &s,
            egui::RawInput {
                events: click_event(position),
                ..base_input()
            },
        );
        let open = review_frame(&ctx, &s, base_input());
        assert!(rendered_text_contains(
            &open,
            &original.display().to_string()
        ));
        assert!(rendered_text_contains(&open, "DAT entry: Sonic (Europe)"));
        assert!(rendered_text_contains(&open, "CandidateEvidenceSummary"));
        assert!(rendered_text_contains(&open, "-> Output:"));
    }

    #[test]
    fn large_preview_is_bounded_but_counts_are_complete() {
        let f = Fixture::new("organisation-large");
        let (mut s, _, _) = preview_a_single_election(&f);
        let template = s.plan().unwrap().elected_games[0].clone();
        s.plan.as_mut().unwrap().elected_games = (0..ROW_LIMIT + 20)
            .map(|i| {
                let mut game = template.clone();
                game.dat_entry_name = format!("Game {i:04}");
                game
            })
            .collect();
        let output = preview_output(&s);
        assert!(rendered_text_contains(&output, "Selected: 220"));
        assert!(rendered_text_contains(
            &output,
            "Showing review items 1–200 of 220"
        ));
        assert!(!rendered_text_contains(&output, "Game 0200"));
        s.preview_page = 1;
        let next = preview_output(&s);
        assert!(rendered_text_contains(&next, "Game 0200"));
        assert!(!rendered_text_contains(&next, "Game 0000"));
        assert!(rendered_text_contains(&next, "Selected: 220"));
        assert!(rendered_text_contains(
            &next,
            "Showing review items 201–220 of 220"
        ));
    }

    #[test]
    fn paint_only_never_creates_output_or_changes_selection_or_plan() {
        let f = Fixture::new("organisation-paint");
        let (mut s, original, dest) = preview_a_single_election(&f);
        let plan = s.plan().unwrap().clone();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            let (_, action) = render(&ctx, &mut s, base_input());
            assert!(action.is_none());
        }
        assert_eq!(s.plan().unwrap(), &plan);
        assert!(s.selected_family().is_none());
        assert!(!dest.exists());
        assert_eq!(std::fs::read(original).unwrap(), b"test");
    }

    #[test]
    fn repeated_playing_library_buttons_select_their_semantic_game_id() {
        let f = Fixture::new("organisation-row-ids");
        let (mut s, _, _) = preview_a_single_election(&f);
        let mut second = s.plan().unwrap().elected_games[0].clone();
        second.dat_entry_name = "Second game".into();
        s.plan.as_mut().unwrap().elected_games.push(second);
        let ctx = egui::Context::default();
        let (_, action) = click_last_text(&ctx, &mut s, "Why this one?");
        assert!(
            matches!(action, Some(PlayingLibraryPageAction::SelectFamily(Some(ref family))) if family == "Second game")
        );
        // Row order is not part of the widget identity.
        assert_ne!(
            egui::Id::new(("playing-family", "Second game")),
            egui::Id::new(("playing-family", "Sonic (Europe)"))
        );
    }

    #[test]
    fn companion_files_are_not_counted_as_unknown_identity() {
        let main: PathBuf = "/synthetic/disc.cue".into();
        let companion: PathBuf = "/synthetic/disc.bin".into();
        let unknown: PathBuf = "/synthetic/unknown.bin".into();
        let outcome = archivefs_core::playing_library::matching::MatchOutcome {
            matches: vec![archivefs_core::playing_library::DatArchiveMatch {
                archive_path: main.clone(),
                dat_entry_index: 0,
                companion_paths: vec![companion.clone()],
            }],
            rejected_launchers: vec![],
        };
        assert_eq!(
            presentation::unrepresented_paths(&[main, companion, unknown.clone()], &outcome),
            vec![unknown]
        );
    }
}

fn find_last_exact_text_center(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn find_in_shape(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text_shape) => (text_shape.galley.text() == needle)
                .then(|| text_shape.pos + text_shape.galley.size() / 2.0),
            egui::Shape::Vec(nested) => nested
                .iter()
                .rev()
                .find_map(|shape| find_in_shape(shape, needle)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .rev()
        .find_map(|clipped| find_in_shape(&clipped.shape, needle))
}

fn click_last_text(
    ctx: &egui::Context,
    state: &mut PlayingLibraryPageState,
    needle: &str,
) -> (egui::FullOutput, Option<PlayingLibraryPageAction>) {
    let (before, _) = render(ctx, state, base_input());
    let pos = find_last_exact_text_center(&before, needle)
        .unwrap_or_else(|| panic!("expected to find rendered text {needle:?} to click"));
    let (_, action) = render(
        ctx,
        state,
        egui::RawInput {
            screen_rect: Some(screen()),
            events: click_event(pos),
            ..Default::default()
        },
    );
    let (settled, _) = render(ctx, state, base_input());
    (settled, action)
}

#[test]
fn failed_undo_is_visible_for_every_linked_library_destination() {
    for destination_kind in [
        PlayingLibraryDestination::Generic,
        PlayingLibraryDestination::Romm,
        PlayingLibraryDestination::RetroDeck,
    ] {
        let fixture = Fixture::new("failed-undo-projection");
        let (mut state, original, destination) = preview_a_single_election(&fixture);
        state.request_apply();
        confirm_library(&mut state);
        let transaction = state.applied.take().unwrap();
        let id = transaction.transaction_id.clone();
        let link = destination.join("unknown/europe.bin");
        // Preserve the original link object and put a different object in its slot.
        std::fs::rename(&link, fixture.path("saved-link")).unwrap();
        std::fs::write(&link, b"another writer's file").unwrap();
        state.destination = destination_kind;
        match destination_kind {
            PlayingLibraryDestination::Generic => {
                state.applied = Some(transaction);
                state.rollback_last();
            }
            PlayingLibraryDestination::Romm => {
                state.romm_applied = Some(transaction);
                state.rollback_romm_last();
            }
            PlayingLibraryDestination::RetroDeck => {
                state.retrodeck_applied = Some(transaction);
                state.rollback_retrodeck();
            }
            PlayingLibraryDestination::EsDe => unreachable!(),
        }
        assert_eq!(
            state.destination_transaction().unwrap().state,
            if destination_kind == PlayingLibraryDestination::RetroDeck {
                // An unbound legacy receipt must be refused before child Undo,
                // preserving both its historical state and the foreign file.
                TransactionState::Applied
            } else {
                TransactionState::RollbackFailed
            }
        );
        assert!(
            state.destination_error().is_some(),
            "{destination_kind:?}: background completion must not show the success banner"
        );
        let persisted = archivefs_core::dat::rename_apply::read_journal(
            &archivefs_core::dat::rename_apply::journal_path(&state.journal_dir, &id).unwrap(),
        )
        .unwrap();
        assert_eq!(persisted, *state.destination_transaction().unwrap());
        assert_eq!(std::fs::read(&link).unwrap(), b"another writer's file");
        assert_eq!(std::fs::read(&original).unwrap(), b"test");
    }
}

#[test]
fn incomplete_linked_library_receipt_never_uses_created_success_label() {
    render_incomplete_receipt(TransactionState::RollbackFailed);
}

#[test]
fn contradictory_rolled_back_receipt_never_claims_success() {
    render_incomplete_receipt(TransactionState::RolledBack);
}

fn render_incomplete_receipt(summary: TransactionState) {
    for destination in [
        PlayingLibraryDestination::Romm,
        PlayingLibraryDestination::RetroDeck,
    ] {
        let fixture = Fixture::new("incomplete-receipt-render");
        let (mut state, _, _) = preview_a_single_election(&fixture);
        state.request_apply();
        confirm_library(&mut state);
        let mut transaction = state.applied.take().unwrap();
        state.plan = state.applied_plan.clone();
        if summary == TransactionState::RollbackFailed {
            transaction.entries[0].state =
                archivefs_core::dat::rename_apply::model::EntryState::RollbackFailed;
            transaction.entries[0].failure_reason = Some("changed destination".into());
            // A genuinely blocked link: it now points elsewhere, so the
            // guarded retry must refuse it rather than complete the undo.
            let link = transaction.entries[0].destination_path.clone();
            let elsewhere = link.with_extension("elsewhere");
            std::fs::write(&elsewhere, b"unrelated").unwrap();
            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        }
        // Reloaded receipts have no in-memory operation error. Prove the core
        // refuses the contradictory terminal summary without undoing the link.
        transaction.state = summary;
        write_journal(&state.journal_dir, &transaction).unwrap();
        let result = rollback_transaction(
            &mut transaction,
            &state.journal_dir,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(matches!(
            result.result,
            archivefs_core::dat::rename_apply::model::RollbackResult::RollbackFailed { .. }
        ));
        assert_eq!(transaction.state, summary);
        assert!(transaction.entries[0].destination_path.is_symlink());
        state.dat_platform_identity = Some(DatPlatformIdentity::Resolved {
            platform: if destination == PlayingLibraryDestination::Romm {
                "Game Boy Advance"
            } else {
                "PSX"
            }
            .into(),
            machine_key: None,
            confidence: DatPlatformConfidence::Strong,
            evidence: Vec::new(),
        });
        let name = if destination == PlayingLibraryDestination::Romm {
            state.preview_romm();
            assert!(state.romm_projection.is_some());
            state.romm_applied = Some(transaction);
            "RomM"
        } else {
            install_retrodeck_projection(
                &mut state,
                &fixture,
                RetroDeckVisibility::verified_same_path_bind(
                    fixture.path("roms"),
                    fixture.path("retrodeck-destination"),
                )
                .unwrap(),
            );
            state.retrodeck_applied = Some(transaction);
            "RetroDECK"
        };
        let mut action = None;
        let output = egui::Context::default().run(base_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if destination == PlayingLibraryDestination::Romm {
                    show_romm_projection_summary(ui, &mut state, &mut action);
                } else {
                    show_retrodeck_projection_summary(ui, &mut state, &mut action);
                }
            });
        });
        assert!(
            !rendered_text_contains(&output, &format!("{name} library created:")),
            "incomplete Undo cannot display a success receipt"
        );
        assert!(
            !rendered_text_contains(&output, &format!("{name} library rolled back")),
            "contradictory Undo cannot display a success receipt"
        );
        assert!(rendered_text_contains(&output, "Recovery needs review"));
    }
}

#[test]
fn reviewed_source_replacement_is_refused_instead_of_recaptured_at_confirmation() {
    for same_bytes in [false, true] {
        let fixture = Fixture::new("reviewed-replacement");
        let (mut state, source, _destination) = preview_a_single_election(&fixture);
        let output = state.plan().unwrap().operations[0].destination_path.clone();
        let old = fixture.path("reviewed-original");
        std::fs::rename(&source, &old).unwrap();
        let replacement = if same_bytes { b"test" } else { b"evil" };
        std::fs::write(&source, replacement).unwrap();
        state.request_apply();
        confirm_library(&mut state);
        assert!(
            state.applied().is_none(),
            "unreviewed replacement was published"
        );
        assert!(state.apply_error().is_some());
        assert_eq!(std::fs::read(&old).unwrap(), b"test");
        assert_eq!(std::fs::read(&source).unwrap(), replacement);
        assert!(std::fs::symlink_metadata(&output).is_err());
    }
}

#[test]
fn reviewed_source_in_place_edit_during_confirmation_is_refused() {
    let fixture = Fixture::new("reviewed-edit");
    let (mut state, source, _destination) = preview_a_single_election(&fixture);
    let output = state.plan().unwrap().operations[0].destination_path.clone();
    state.request_apply();
    let old_mtime = std::fs::metadata(&source).unwrap().modified().unwrap();
    let writer_path = source.clone();
    // Join is the deterministic hand-off: the write happens after Preview
    // and confirmation opens, but before the real Apply boundary.
    std::thread::spawn(move || {
        std::fs::write(&writer_path, b"evil").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&writer_path)
            .unwrap()
            .set_modified(old_mtime)
            .unwrap();
    })
    .join()
    .unwrap();
    confirm_library(&mut state);
    assert!(
        state.applied().is_none(),
        "edited bytes were silently adopted"
    );
    assert!(state.apply_error().is_some());
    assert_eq!(std::fs::read(&source).unwrap(), b"evil");
    assert!(std::fs::symlink_metadata(&output).is_err());
}

#[test]
fn reviewed_source_symlink_and_parent_redirection_refuse_without_touching_foreign_bytes() {
    for replace_parent in [false, true] {
        let fixture = Fixture::new("reviewed-symlink");
        let (mut state, source, _destination) = preview_a_single_election(&fixture);
        let output = state.plan().unwrap().operations[0].destination_path.clone();
        let foreign = fixture.path("foreign");
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("europe.bin"), b"foreign").unwrap();
        if replace_parent {
            let parent = source.parent().unwrap();
            std::fs::rename(parent, fixture.path("original-parent")).unwrap();
            std::os::unix::fs::symlink(&foreign, parent).unwrap();
        } else {
            std::fs::rename(&source, fixture.path("reviewed-original")).unwrap();
            std::os::unix::fs::symlink(foreign.join("europe.bin"), &source).unwrap();
        }
        state.request_apply();
        confirm_library(&mut state);
        assert!(state.applied().is_none());
        assert!(state.apply_error().is_some());
        assert_eq!(
            std::fs::read(foreign.join("europe.bin")).unwrap(),
            b"foreign"
        );
        assert!(std::fs::symlink_metadata(&output).is_err());
    }
}

#[test]
fn reviewed_apply_refuses_an_occupied_destination_and_preserves_it() {
    let fixture = Fixture::new("reviewed-occupied");
    let (mut state, source, _) = preview_a_single_election(&fixture);
    let output = state.plan().unwrap().operations[0].destination_path.clone();
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    std::fs::write(&output, b"foreign output").unwrap();
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.applied().is_none());
    assert!(state.apply_error().is_some());
    assert_eq!(std::fs::read(&output).unwrap(), b"foreign output");
    assert_eq!(std::fs::read(&source).unwrap(), b"test");
}

#[test]
fn reviewed_apply_refuses_changed_plan_without_creating_a_journal_or_output() {
    let fixture = Fixture::new("reviewed-plan-change");
    let (mut state, source, destination) = preview_a_single_election(&fixture);
    state.plan.as_mut().unwrap().operations[0].destination_path = fixture.path("unreviewed.bin");
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().unwrap().contains("stale"));
    assert!(state.applied().is_none());
    assert!(
        std::fs::read_dir(&state.journal_dir)
            .unwrap()
            .next()
            .is_none()
    );
    assert!(!destination.exists());
    assert!(!fixture.path("unreviewed.bin").exists());
    assert_eq!(std::fs::read(source).unwrap(), b"test");
}

#[test]
fn reviewed_apply_binding_is_single_use_across_background_result_merge() {
    let fixture = Fixture::new("reviewed-async-retry");
    let (mut state, source, _) = preview_a_single_election(&fixture);
    let output = state.plan().unwrap().operations[0].destination_path.clone();
    let saved = fixture.path("reviewed-original");
    std::fs::rename(&source, &saved).unwrap();
    std::fs::write(&source, b"evil").unwrap();
    state.request_apply();
    let mut worker = state.clone();
    confirm_library(&mut worker);
    assert!(worker.apply_error().is_some());
    state.merge_async_apply_result(worker);
    assert!(
        state.preview_transaction.is_none(),
        "background consumption must reach the live state"
    );
    let journal_dir = state.journal_dir.clone();
    let journals = || {
        let mut files: Vec<_> = std::fs::read_dir(&journal_dir)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (path.clone(), std::fs::read(path).unwrap())
            })
            .collect();
        files.sort();
        files
    };
    let evidence = journals();
    assert!(
        evidence.is_empty(),
        "a stale source is refused before journaling"
    );
    std::fs::remove_file(&source).unwrap();
    std::fs::rename(&saved, &source).unwrap();
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().unwrap().contains("already used"));
    assert!(state.applied().is_none());
    assert_eq!(
        journals(),
        evidence,
        "retry must not invent an operation journal"
    );
    assert!(std::fs::symlink_metadata(&output).is_err());
    state.preview();
    state.request_apply();
    confirm_library(&mut state);
    assert!(state.apply_error().is_none(), "{:?}", state.apply_error());
    assert_eq!(state.applied().unwrap().state, TransactionState::Applied);
    assert_eq!(std::fs::read(output).unwrap(), b"test");
}

fn reviewed_profile_fixture(
    fixture: &Fixture,
    retrodeck: bool,
) -> (PlayingLibraryPageState, PathBuf) {
    let (mut state, source, _) = preview_a_single_election(fixture);
    state.set_destination(if retrodeck {
        PlayingLibraryDestination::RetroDeck
    } else {
        PlayingLibraryDestination::Romm
    });
    state.preview();
    state.dat_platform_identity = Some(DatPlatformIdentity::Resolved {
        platform: if retrodeck { "PSX" } else { "Game Boy Advance" }.into(),
        machine_key: None,
        confidence: DatPlatformConfidence::Strong,
        evidence: Vec::new(),
    });
    if retrodeck {
        let profile = retrodeck_profile(fixture);
        let gamelist = PathBuf::from(
            &profile
                .system_data
                .iter()
                .find(|entry| entry.system_name == "psx")
                .unwrap()
                .gamelist_file
                .path
                .display,
        );
        std::fs::create_dir_all(gamelist.parent().unwrap()).unwrap();
        std::fs::write(&gamelist, "<gameList>\n<!-- preserved -->\n</gameList>\n").unwrap();
        state.esde_home_override = Some(fixture.path("retrodeck-home/ES-DE"));
        state.retrodeck_visibility_verified = true;
        state.retrodeck_visible_source_root_draft = state.source_root_draft.clone();
        state.retrodeck_destination_root_draft =
            fixture.path("retrodeck-destination").display().to_string();
        state.preview_retrodeck();
        assert!(
            state.retrodeck_error.is_none(),
            "{:?}",
            state.retrodeck_error
        );
        assert!(state.retrodeck_projection.is_some());
    } else {
        state.romm_visibility_verified = true;
        state.romm_visible_source_root_draft = state.source_root_draft.clone();
        state.preview_romm();
        assert!(state.romm_error.is_none(), "{:?}", state.romm_error);
        assert!(state.romm_projection.is_some());
    }
    (state, source)
}

fn apply_reviewed_profile(state: &mut PlayingLibraryPageState, retrodeck: bool) {
    if retrodeck {
        state.request_retrodeck_apply();
        confirm_retrodeck(state);
    } else {
        state.request_romm_apply();
        confirm_romm(state);
    }
}

fn profile_stale_source_cases(retrodeck: bool) {
    for attack in [
        "replacement",
        "identical replacement",
        "in-place",
        "missing",
        "symlink",
        "parent symlink",
    ] {
        let fixture = Fixture::new("profile-reviewed-source");
        let (mut state, source) = reviewed_profile_fixture(&fixture, retrodeck);
        let outputs: Vec<_> = if retrodeck {
            &state
                .retrodeck_projection
                .as_ref()
                .unwrap()
                .playing_library_plan
        } else {
            &state.romm_projection.as_ref().unwrap().playing_library_plan
        }
        .operations
        .iter()
        .map(|operation| operation.destination_path.clone())
        .collect();
        let gamelist = state
            .retrodeck_projection
            .as_ref()
            .map(|projection| projection.es_de_publication.gamelist_path.clone());
        let original_gamelist = gamelist.as_ref().map(|path| std::fs::read(path).unwrap());
        match attack {
            "replacement" | "identical replacement" => {
                std::fs::rename(&source, fixture.path("saved-original")).unwrap();
                std::fs::write(
                    &source,
                    if attack == "replacement" {
                        b"evil"
                    } else {
                        b"test"
                    },
                )
                .unwrap();
            }
            "in-place" => {
                let mtime = std::fs::metadata(&source).unwrap().modified().unwrap();
                let source = source.clone();
                std::thread::spawn(move || {
                    std::fs::write(&source, b"evil").unwrap();
                    std::fs::File::options()
                        .write(true)
                        .open(source)
                        .unwrap()
                        .set_modified(mtime)
                        .unwrap();
                })
                .join()
                .unwrap();
            }
            "missing" => {
                std::fs::rename(&source, fixture.path("saved-original")).unwrap();
            }
            "symlink" => {
                std::fs::rename(&source, fixture.path("saved-original")).unwrap();
                std::fs::write(fixture.path("foreign"), b"foreign").unwrap();
                std::os::unix::fs::symlink(fixture.path("foreign"), &source).unwrap();
            }
            "parent symlink" => {
                std::fs::rename(source.parent().unwrap(), fixture.path("saved-parent")).unwrap();
                std::fs::create_dir(fixture.path("foreign-parent")).unwrap();
                std::fs::write(fixture.path("foreign-parent/europe.bin"), b"foreign").unwrap();
                std::os::unix::fs::symlink(
                    fixture.path("foreign-parent"),
                    source.parent().unwrap(),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        apply_reviewed_profile(&mut state, retrodeck);
        let applied = if retrodeck {
            &state.retrodeck_applied
        } else {
            &state.romm_applied
        };
        let error = if retrodeck {
            &state.retrodeck_error
        } else {
            &state.romm_error
        };
        assert!(
            applied.is_none(),
            "{retrodeck}/{attack}: stale source was published"
        );
        assert!(
            error.is_some(),
            "{retrodeck}/{attack}: refusal needs a visible reason"
        );
        assert!(
            outputs
                .iter()
                .all(|path| std::fs::symlink_metadata(path).is_err())
        );
        if let Some(path) = gamelist {
            assert_eq!(std::fs::read(path).unwrap(), original_gamelist.unwrap());
        }
    }
}

#[test]
fn romm_rejects_sources_changed_after_profile_preview() {
    profile_stale_source_cases(false);
}
#[test]
fn retrodeck_rejects_sources_changed_after_profile_preview() {
    profile_stale_source_cases(true);
}

#[test]
fn profile_preview_does_not_reapprove_sources_changed_since_base_preview() {
    for retrodeck in [false, true] {
        let fixture = Fixture::new("profile-base-proof");
        let (mut state, source) = reviewed_profile_fixture(&fixture, retrodeck);
        std::fs::write(&source, b"evil").unwrap();
        if retrodeck {
            state.preview_retrodeck();
        } else {
            state.preview_romm();
        }
        apply_reviewed_profile(&mut state, retrodeck);
        let error = if retrodeck {
            &state.retrodeck_error
        } else {
            &state.romm_error
        };
        assert!(
            error
                .as_ref()
                .unwrap()
                .contains("fresh Playing Library Preview"),
            "{error:?}"
        );
        assert!(
            std::fs::read_dir(&state.journal_dir)
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(std::fs::read(source).unwrap(), b"evil");
    }
}

#[test]
fn profile_destination_conflicts_preserve_files_and_changed_symlink_targets() {
    for retrodeck in [false, true] {
        for symlink in [false, true] {
            let fixture = Fixture::new("profile-stale-output");
            let (mut state, source) = reviewed_profile_fixture(&fixture, retrodeck);
            let plan = if retrodeck {
                &state
                    .retrodeck_projection
                    .as_ref()
                    .unwrap()
                    .playing_library_plan
            } else {
                &state.romm_projection.as_ref().unwrap().playing_library_plan
            };
            let output = plan.operations[0].destination_path.clone();
            std::fs::create_dir_all(output.parent().unwrap()).unwrap();
            if symlink {
                std::fs::write(fixture.path("foreign"), b"foreign").unwrap();
                std::os::unix::fs::symlink(&source, &output).unwrap();
                std::fs::remove_file(&output).unwrap();
                std::os::unix::fs::symlink(fixture.path("foreign"), &output).unwrap();
            } else {
                std::fs::write(&output, b"foreign").unwrap();
            }
            let gamelist = state
                .retrodeck_projection
                .as_ref()
                .map(|p| p.es_de_publication.gamelist_path.clone());
            let xml = gamelist.as_ref().map(|path| std::fs::read(path).unwrap());
            apply_reviewed_profile(&mut state, retrodeck);
            assert!(if retrodeck {
                state.retrodeck_applied.is_none() && state.retrodeck_error.is_some()
            } else {
                state.romm_applied.is_none() && state.romm_error.is_some()
            });
            assert_eq!(std::fs::read(&output).unwrap(), b"foreign");
            if symlink {
                assert_eq!(
                    std::fs::read_link(&output).unwrap(),
                    fixture.path("foreign")
                );
            }
            assert_eq!(std::fs::read(source).unwrap(), b"test");
            if let Some(path) = gamelist {
                assert_eq!(std::fs::read(path).unwrap(), xml.unwrap());
            }
        }
    }
}

#[test]
fn profile_failed_attempt_cannot_reuse_or_overwrite_its_journal() {
    for retrodeck in [false, true] {
        let fixture = Fixture::new("profile-single-attempt");
        let (mut state, _) = reviewed_profile_fixture(&fixture, retrodeck);
        let tx = if retrodeck {
            state
                .retrodeck_preview_transaction
                .as_ref()
                .unwrap()
                .1
                .as_ref()
                .unwrap()
        } else {
            state
                .romm_preview_transaction
                .as_ref()
                .unwrap()
                .1
                .as_ref()
                .unwrap()
        };
        let journal =
            archivefs_core::dat::rename_apply::journal_path(&state.journal_dir, &tx.transaction_id)
                .unwrap();
        let output = tx.entries[0].destination_path.clone();
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, b"foreign").unwrap();
        apply_reviewed_profile(&mut state, retrodeck);
        let receipt = std::fs::read(&journal).unwrap();
        std::fs::remove_file(&output).unwrap();
        // Special-profile background jobs replace the live state with this
        // worker result, retaining the consumed (None) binding.
        let mut worker = state.clone();
        apply_reviewed_profile(&mut worker, retrodeck);
        state = worker;
        let error = if retrodeck {
            &state.retrodeck_error
        } else {
            &state.romm_error
        };
        assert!(error.as_ref().unwrap().contains("already used"));
        assert_eq!(std::fs::read(&journal).unwrap(), receipt);
        assert!(std::fs::symlink_metadata(&output).is_err());
    }
}

#[test]
fn profile_snapshot_refuses_projection_changes_and_base_preview_invalidation() {
    for retrodeck in [false, true] {
        for new_base_preview in [false, true] {
            let fixture = Fixture::new("profile-stale-plan");
            let (mut state, source) = reviewed_profile_fixture(&fixture, retrodeck);
            if new_base_preview {
                state.preview();
            } else if retrodeck {
                state.retrodeck_projection.as_mut().unwrap().total_files += 1;
            } else {
                state.romm_projection.as_mut().unwrap().total_files += 1;
            }
            apply_reviewed_profile(&mut state, retrodeck);
            assert!(if retrodeck {
                state.retrodeck_applied.is_none()
            } else {
                state.romm_applied.is_none()
            });
            assert!(
                std::fs::read_dir(&state.journal_dir)
                    .unwrap()
                    .next()
                    .is_none()
            );
            assert_eq!(std::fs::read(source).unwrap(), b"test");
        }
    }
}

#[test]
fn real_profile_previews_apply_and_undo_unchanged_files_with_persisted_receipts() {
    use std::os::unix::fs::MetadataExt;
    for retrodeck in [false, true] {
        let fixture = Fixture::new("profile-live-seam");
        let (mut state, source) = reviewed_profile_fixture(&fixture, retrodeck);
        let before = std::fs::metadata(&source).unwrap();
        apply_reviewed_profile(&mut state, retrodeck);
        let transaction = if retrodeck {
            state.retrodeck_applied.as_ref().unwrap()
        } else {
            state.romm_applied.as_ref().unwrap()
        };
        assert_eq!(transaction.state, TransactionState::Applied);
        let output = transaction.entries[0].destination_path.clone();
        assert_eq!(std::fs::read_link(&output).unwrap(), source);
        let journal = archivefs_core::dat::rename_apply::journal_path(
            &state.journal_dir,
            &transaction.transaction_id,
        )
        .unwrap();
        assert_eq!(
            archivefs_core::dat::rename_apply::read_journal(&journal).unwrap(),
            *transaction
        );
        if retrodeck {
            state.rollback_retrodeck();
        } else {
            state.rollback_romm_last();
        }
        assert!(std::fs::symlink_metadata(output).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"test");
        let after = std::fs::metadata(source).unwrap();
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        assert_eq!(
            archivefs_core::dat::rename_apply::read_journal(&journal)
                .unwrap()
                .state,
            TransactionState::RolledBack
        );
    }
}

#[path = "retrodeck_tests.rs"]
mod retrodeck_campaign;

#[test]
fn destination_change_requires_a_new_confirmation_after_repreview() {
    for destination in [
        PlayingLibraryDestination::Romm,
        PlayingLibraryDestination::EsDe,
        PlayingLibraryDestination::RetroDeck,
    ] {
        let fixture = Fixture::new("destination-confirmation-reset");
        let (mut state, source, _) = preview_a_single_election(&fixture);
        let output = state.plan().unwrap().operations[0].destination_path.clone();
        state.request_apply();
        state.confirm_text = "previous approval".into();
        state.romm_pending_apply = true;
        state.romm_confirm_text = "previous RomM approval".into();
        state.retrodeck_pending_apply = true;
        state.retrodeck_confirm_text = "previous RetroDECK approval".into();
        state.selected_family = Some("old election".into());
        state.error = Some("old preview error".into());
        state.apply_error = Some("old apply error".into());
        state.set_destination(destination);
        assert!(state.pending_apply.is_none());
        assert!(!state.romm_pending_apply && !state.retrodeck_pending_apply);
        assert!(
            state.confirm_text.is_empty()
                && state.romm_confirm_text.is_empty()
                && state.retrodeck_confirm_text.is_empty()
        );
        assert!(state.selected_family.is_none() && state.dat_platform_identity.is_none());
        assert!(state.error.is_none() && state.apply_error.is_none());
        state.set_destination(PlayingLibraryDestination::Generic);
        state.preview();
        assert!(state.plan().is_some());
        state.confirm_apply(); // stale approval must not authorize a new preview
        assert!(state.applied().is_none());
        assert!(std::fs::symlink_metadata(&output).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"test");
        state.request_apply();
        confirm_library(&mut state);
        assert!(
            state.applied().is_some(),
            "fresh explicit confirmation still works"
        );
    }
}

#[test]
fn destination_change_preserves_completed_receipts_and_undo() {
    let fixture = Fixture::new("destination-receipt-preservation");
    let (mut state, source, _) = preview_a_single_election(&fixture);
    state.request_apply();
    confirm_library(&mut state);
    let receipt = state.applied().unwrap().clone();
    let output = receipt.entries[0].destination_path.clone();
    state.apply_error = Some("Retained receipt diagnostic".into());
    state.romm_applied = Some(receipt.clone());
    state.romm_error = Some("Retained RomM receipt diagnostic".into());
    state.retrodeck_selected_id = Some("retained-publication-operation".into());
    state.retrodeck_error = Some("Retained recovery diagnostic".into());
    state.set_destination(PlayingLibraryDestination::Romm);
    assert_eq!(state.applied().unwrap(), &receipt);
    assert_eq!(state.romm_applied.as_ref(), Some(&receipt));
    assert_eq!(
        state.apply_error.as_deref(),
        Some("Retained receipt diagnostic")
    );
    assert_eq!(
        state.romm_error.as_deref(),
        Some("Retained RomM receipt diagnostic")
    );
    assert_eq!(
        state.retrodeck_error.as_deref(),
        Some("Retained recovery diagnostic")
    );
    assert_eq!(
        state.retrodeck_selected_id.as_deref(),
        Some("retained-publication-operation")
    );
    assert!(state.applied_plan.is_some());
    assert!(output.is_symlink());
    // Existing receipt-owned rollback remains available after changing destination.
    state.rollback_last();
    assert!(std::fs::symlink_metadata(&output).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"test");
}

#[test]
fn failed_apply_and_failed_undo_are_worded_by_what_the_receipt_records() {
    let empty = RenameTransaction {
        transaction_id: "t".into(),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: 0,
        source_scan_root: String::new(),
        state: TransactionState::Planned,
        entries: Vec::new(),
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let (title, body) = apply_error_heading(None, false);
    assert_eq!(title, "Not created");
    assert!(!body.contains("may have been created"), "{body}");
    // A receipt holding no recorded links is the same refusal, with no Undo claim.
    assert_eq!(apply_error_heading(Some(&empty), false).0, "Not created");
    // A failed Undo is never presented as a failed creation.
    let (title, body) = apply_error_heading(Some(&empty), true);
    assert_eq!(title, "Undo did not finish");
    assert!(!body.contains("still in the Playing Library"), "{body}");
    assert!(!body.contains("retry Undo"), "{body}");
    assert!(body.contains("no links still awaiting"), "{body}");
}

#[test]
fn failed_undo_distinguishes_recorded_links_awaiting_reversal_from_a_reversed_receipt() {
    use archivefs_core::dat::rename_apply::model::{EntryState, TransactionEntry};
    let exe = std::env::current_exe().unwrap();
    let entry = |state| TransactionEntry {
        source_path: exe.clone(),
        destination_path: PathBuf::from("/library/link"),
        original_basename: "a".into(),
        proposed_basename: "link".into(),
        identity: capture_identity(&exe).unwrap(),
        operation: Default::default(),
        preflight_passed: true,
        preflight_failures: Vec::new(),
        state,
        failure_reason: None,
        applied_at_unix: Some(1),
        rolled_back_at_unix: None,
        unknown: Default::default(),
    };
    let mut tx = RenameTransaction {
        transaction_id: "t".into(),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: 0,
        source_scan_root: String::new(),
        state: TransactionState::RollbackFailed,
        entries: vec![
            entry(EntryState::RolledBack),
            entry(EntryState::RollbackFailed),
        ],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let (_, body) = apply_error_heading(Some(&tx), true);
    assert!(body.contains("still records links"), "{body}");
    assert!(!body.contains("no links still awaiting"), "{body}");
    // Every entry reversed: nothing is claimed to remain.
    tx.entries = vec![entry(EntryState::RolledBack)];
    let (_, body) = apply_error_heading(Some(&tx), true);
    assert!(body.contains("no links still awaiting"), "{body}");
    assert!(!body.contains("still records links"), "{body}");
}

#[test]
fn receipt_view_shows_the_journal_based_outcome_badge() {
    use crate::transaction_outcome::test_support::{journal, rendered_text};
    use archivefs_core::dat::rename_apply::model::EntryState;
    let render = |transaction| {
        let mut state = PlayingLibraryPageState::default();
        state.applied = Some(transaction);
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_playing_library_page(ui, &mut state);
            });
        });
        rendered_text(&output)
    };
    let live = render(journal(
        TransactionState::Applied,
        &[EntryState::Applied, EntryState::Applied],
        &[],
    ));
    assert!(live.contains("Completed"), "{live}");
    assert!(live.contains("Undo available now"), "{live}");
    // After Undo the same receipt reads Undone and offers no Undo.
    let undone = render(journal(
        TransactionState::RolledBack,
        &[EntryState::RolledBack, EntryState::RolledBack],
        &[],
    ));
    assert!(undone.contains("Undone"), "{undone}");
    assert!(!undone.contains("Undo available"), "{undone}");
}
