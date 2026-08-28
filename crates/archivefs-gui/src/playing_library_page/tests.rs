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
    render(
        ctx,
        state,
        egui::RawInput {
            screen_rect: Some(screen()),
            events: click_event(pos),
            ..Default::default()
        },
    )
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
    state.confirm_apply();

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

    assert!(rendered_text_contains(&expanded, "Selected because:"));
    assert!(rendered_text_contains(&expanded, "Not selected:"));
    assert!(rendered_text_contains(
        &expanded,
        "Sonic (USA) - not selected because:"
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
    state.retrodeck_projection = Some(
        build_retrodeck_projection(
            &plan,
            &identity,
            fixture.path("retrodeck-destination"),
            visibility,
            &profile,
        )
        .unwrap(),
    );
}

#[test]
fn retrodeck_gui_apply_requires_confirmation_then_publishes_and_rolls_back() {
    let fixture = Fixture::new("retrodeck-gui-success");
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
    state.confirm_retrodeck_apply();
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
    let (output, _) = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(&output, "RetroDECK library created"));
    state.rollback_retrodeck();
    assert_eq!(
        std::fs::read_to_string(&gamelist).unwrap(),
        "<gameList>\n<!-- keep -->\n</gameList>\n"
    );
    assert!(!destinations.iter().any(|path| path.exists()));
    let (output, _) = render(&ctx, &mut state, base_input());
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
    state.confirm_retrodeck_apply();
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
    state.confirm_apply(); // below the typed-confirmation threshold

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

    let link = destination.join("europe.bin");
    assert!(link.is_symlink());
    assert_eq!(std::fs::read_link(&link).unwrap(), original);
}

#[test]
fn original_source_files_remain_untouched_after_apply() {
    let fixture = Fixture::new("untouched");
    let (mut state, original, _destination) = preview_a_single_election(&fixture);

    state.request_apply();
    state.confirm_apply();

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
    state.confirm_apply();
    assert!(state.apply_error().is_none());

    let entries: Vec<_> = std::fs::read_dir(&destination)
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
    assert!(rendered_text_contains(&output, "Visibility: Unverified"));
    assert!(rendered_text_contains(&output, "Apply is blocked"));
    assert!(!rendered_text_contains(&output, "Launcher:"));
}

#[test]
fn retrodeck_preview_card_is_visible_and_unverified_apply_is_blocked() {
    let fixture = Fixture::new("retrodeck-preview");
    let (mut state, _original, _destination) = preview_a_single_election(&fixture);
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
    state.dat_platform_identity = Some(DatPlatformIdentity::Resolved {
        platform: "Game Boy Advance".to_string(),
        machine_key: None,
        confidence: DatPlatformConfidence::Strong,
        evidence: Vec::new(),
    });
    state.preview_romm();
    state.request_romm_apply();
    state.confirm_romm_apply();
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
    state.confirm_romm_apply();
    assert!(state.romm_error.is_none(), "{:?}", state.romm_error);
    let transaction = state.romm_applied.as_ref().expect("RomM transaction");
    assert_eq!(transaction.state, TransactionState::Applied);
    let link = destination.join("roms/gba/europe.bin");
    assert!(link.is_symlink());
    assert_eq!(std::fs::read_link(&link).unwrap(), original);
    assert_eq!(std::fs::read(&original).unwrap(), b"test");
    let ctx = egui::Context::default();
    let (applied_output, _) = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(
        &applied_output,
        "RomM library created: 1 link(s)"
    ));

    state.rollback_romm_last();
    assert!(!link.exists());
    assert_eq!(std::fs::read(&original).unwrap(), b"test");
    let (rolled_back_output, _) = render(&ctx, &mut state, base_input());
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
    state.confirm_apply();
    assert!(state.apply_error().is_none());
    let link = destination.join("europe.bin");
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
    let (output, action) = click_text(&ctx, &mut state, "Preview Playing Library");
    assert!(action.is_none(), "a disabled button must never click");
    assert!(rendered_text_contains(
        &output,
        "Choose a DAT catalogue, a source, and a destination first."
    ));

    type_into_field(&ctx, &mut state, DAT_PATH_FIELD_ID, "/tmp/library.dat");
    type_into_field(&ctx, &mut state, SOURCE_ROOT_FIELD_ID, "/tmp/roms");
    type_into_field(&ctx, &mut state, DESTINATION_ROOT_FIELD_ID, "/tmp/playing");

    let (output, action) = click_text(&ctx, &mut state, "Preview Playing Library");
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
fn an_invalid_dat_path_shows_a_plain_inline_error() {
    let ctx = egui::Context::default();
    let mut state = PlayingLibraryPageState::default();

    type_into_field(
        &ctx,
        &mut state,
        DAT_PATH_FIELD_ID,
        "/definitely/does/not/exist.dat",
    );
    let (output, _) = render(&ctx, &mut state, base_input());

    assert!(rendered_text_contains(&output, "This file was not found."));
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

// --- "Publish to ES-DE" -------------------------------------------------

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
    /// starting point for every "Publish to ES-DE" test.
    fn library_and_esde_home(tag: &str) -> (Fixture, PlayingLibraryPageState) {
        let fixture = Fixture::new(tag);
        let (mut state, _original, _destination) = preview_a_single_election(&fixture);
        state.request_apply();
        state.confirm_apply();
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
        assert!(!rendered_text_contains(&output, "Publish to ES-DE"));
    }

    #[test]
    fn publish_section_appears_once_the_library_is_created() {
        let ctx = egui::Context::default();
        let (_fixture, mut state) = library_and_esde_home("esde-appears-after-create");

        let (output, _) = render(&ctx, &mut state, base_input());
        assert!(rendered_text_contains(&output, "Publish to ES-DE"));
    }

    #[test]
    fn publish_section_disappears_again_after_rollback() {
        let ctx = egui::Context::default();
        let (_fixture, mut state) = library_and_esde_home("esde-hidden-after-rollback");
        state.rollback_last();
        assert!(state.apply_error().is_none());

        let (output, _) = render(&ctx, &mut state, base_input());
        assert!(!rendered_text_contains(&output, "Publish to ES-DE"));
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
    fn confirmed_recovery_restores_the_exact_prior_content_and_touches_nothing_else() {
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

        assert!(
            state.esde_recovery_error().is_none(),
            "{:?}",
            state.esde_recovery_error()
        );
        assert!(state.esde_recovery_done());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), previous);
        assert!(!has_unresolved_es_de_gamelist_recovery(&path));

        // The master ROM and its playing-library link, entirely unrelated
        // to ES-DE's own gamelist, are never touched by recovery.
        let link = fixture.path("playing").join("europe.bin");
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
