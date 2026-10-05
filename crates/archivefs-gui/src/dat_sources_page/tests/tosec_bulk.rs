//! TOSEC bulk selection, paging, the zero-selection stage guard and the
//! matching-preferences framing. Fixtures only: no real catalogue is touched.
use super::*;

fn dat(system: &str, raw: &str) -> TosecPackDat {
    TosecPackDat {
        relative_path: PathBuf::from(format!("{raw}.dat")),
        raw_catalogue_name: raw.to_string(),
        system: system.to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::Rom,
        raw_category_label: "Games".to_string(),
        classification_confident: true,
        content_sha256: None,
    }
}

fn pack_with(fixture: &Fixture, systems: Vec<String>) -> (DatSourcesPageState, String, PathBuf) {
    let root = fixture.dir("bulk-pack");
    let dats = systems
        .iter()
        .map(|system| dat(system, &format!("{system} - Games - ROM")))
        .collect();
    let mut page = fixture.page();
    page.tosec_packs = vec![PersistedTosecPack {
        pack_id: "bulk-pack".to_string(),
        root_path: root.clone(),
        imported_unix_seconds: 0,
        selections: Default::default(),
        dats,
    }];
    (page, "bulk-pack".to_string(), root)
}

fn selected(page: &DatSourcesPageState) -> usize {
    page.tosec_packs[0].selections.len()
}

fn bulk(page: &mut DatSourcesPageState, filter: &str, enabled: bool) {
    page.apply(DatSourcesPageAction::SetTosecSelectionMatching {
        pack_id: "bulk-pack".to_string(),
        filter: filter.to_string(),
        enabled,
    });
}

fn systems(amiga: usize, other: usize) -> Vec<String> {
    (0..amiga)
        .map(|i| format!("Commodore Amiga {i:03}"))
        .chain((0..other).map(|i| format!("Sinclair Thing {i:03}")))
        .collect()
}

#[test]
fn bulk_enable_reaches_all_888_groups_though_only_200_are_drawn() {
    let fixture = Fixture::new();
    let (mut page, _, _) = pack_with(&fixture, systems(0, 888));
    let output = render(&page.view(), &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(&output, "Showing 1–200 of 888"));
    assert!(rendered_text_count(&output, "· 1 DAT(s)") <= 200);

    bulk(&mut page, "", true);
    assert_eq!(
        selected(&page),
        888,
        "rendering limits are not action limits"
    );
    assert_eq!(page.view().tosec_packs[0].selected_dat_count, 888);
    assert!(page.tosec_action_error.is_none());
}

#[test]
fn bulk_scope_is_the_current_filter_only() {
    let fixture = Fixture::new();
    let (mut page, _, _) = pack_with(&fixture, systems(127, 300));
    bulk(&mut page, "  amiga ", true);
    assert_eq!(selected(&page), 127);
    assert!(
        page.tosec_packs[0]
            .selections
            .iter()
            .all(|key| key.system.contains("Amiga"))
    );
}

#[test]
fn bulk_disable_matching_leaves_other_selections_alone() {
    let fixture = Fixture::new();
    let (mut page, _, _) = pack_with(&fixture, systems(127, 300));
    bulk(&mut page, "", true);
    assert_eq!(selected(&page), 427);
    bulk(&mut page, "amiga", false);
    assert_eq!(selected(&page), 300);
    bulk(&mut page, "", false);
    assert_eq!(selected(&page), 0);
}

#[test]
fn bulk_enable_never_selects_deferred_groups() {
    let fixture = Fixture::new();
    let (mut page, _, _) = pack_with(&fixture, systems(3, 0));
    page.tosec_packs[0].dats.push(dat("PC", "TOSEC-ISO - PC"));
    bulk(&mut page, "", true);
    assert_eq!(selected(&page), 3);
}

#[test]
fn groups_beyond_the_first_page_are_reachable() {
    let fixture = Fixture::new();
    let (page, _, _) = pack_with(&fixture, systems(0, 888));
    let mut ui_state = DatSourcesPageUi::default();
    ui_state.tosec_group_page.insert("bulk-pack".into(), 4);
    let output = render(&page.view(), &mut ui_state);
    assert!(rendered_text_contains(&output, "Showing 801–888 of 888"));
    assert!(rendered_text_contains(&output, "Sinclair Thing 887"));
    assert!(!rendered_text_contains(&output, "Sinclair Thing 000"));
    // A page past the end (for example after the filter narrowed) is clamped.
    ui_state.tosec_group_page.insert("bulk-pack".into(), 99);
    let output = render(&page.view(), &mut ui_state);
    assert!(rendered_text_contains(&output, "Showing 801–888 of 888"));
}

#[test]
fn bulk_buttons_state_the_filtered_count_and_confirmation_names_the_scope() {
    let fixture = Fixture::new();
    let (page, pack_id, _) = pack_with(&fixture, systems(127, 300));
    let mut ui_state = DatSourcesPageUi::default();
    ui_state
        .tosec_group_filter
        .insert(pack_id.clone(), "Amiga".into());
    ui_state.tosec_bulk_confirm = Some(TosecBulkConfirm {
        pack_id,
        filter: "Amiga".into(),
        enabled: true,
        groups: 127,
    });
    let output = render(&page.view(), &mut ui_state);
    assert!(rendered_text_contains(&output, "Enable all 127 matching"));
    assert!(rendered_text_contains(&output, "Disable all 0 matching"));
    assert!(rendered_text_contains(
        &output,
        "Enable 127 matching TOSEC groups?"
    ));
    assert!(rendered_text_contains(
        &output,
        "No game files will be modified"
    ));
    assert!(rendered_text_contains(&output, "Enable 127 groups"));
}

#[test]
fn zero_selected_disables_staging_without_an_error() {
    let fixture = Fixture::new();
    let (mut page, pack_id, _) = pack_with(&fixture, systems(23, 0));
    let output = render(&page.view(), &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(&output, "23 DATs found"));
    assert!(rendered_text_contains(
        &output,
        "Choose which groups you want to use."
    ));
    assert!(rendered_text_contains(
        &output,
        "Select at least one DAT group first."
    ));
    assert!(!rendered_text_contains(&output, "Stage 0"));

    // Even if the action arrives anyway it is a quiet no-op, not a banner.
    page.apply(DatSourcesPageAction::ApplyTosecSelection { pack_id });
    assert!(page.tosec_action_error.is_none());
    assert!(page.tosec_managed_error.is_none());
    assert!(page.view().tosec_managed.staged_preview.is_none());
}

#[test]
fn bulk_enable_changes_selection_only_and_does_not_stage_or_touch_files() {
    let fixture = Fixture::new();
    let (mut page, _, root) = pack_with(&fixture, systems(23, 0));
    let game = fixture.write("games/Keep.adf", "disk");
    let before = std::fs::read(&game).unwrap();
    bulk(&mut page, "", true);
    assert_eq!(selected(&page), 23);
    assert!(page.view().tosec_managed.staged_preview.is_none());
    assert!(
        page.view().rows.is_empty(),
        "nothing is registered by selecting"
    );
    assert_eq!(std::fs::read(&game).unwrap(), before);
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    let output = render(&page.view(), &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(
        &output,
        "23 DATs found · 23 selected"
    ));
    assert!(rendered_text_contains(&output, "Stage 23 selected DATs"));
}

#[test]
fn matching_preferences_are_framed_as_policy_and_never_enable_dats() {
    let fixture = Fixture::new();
    let dat = fixture.write("nes.dat", r#"<?xml version="1.0"?><datafile><header><name>Nintendo - NES</name></header><game name="G"><rom name="g.nes" size="1" crc="00000000"/></game></datafile>"#);
    let mut page = fixture.page();
    page.apply(DatSourcesPageAction::AddFile { path: dat });
    let rows_before = page
        .view()
        .rows
        .iter()
        .map(|row| row.enabled)
        .collect::<Vec<_>>();
    let output = render(&page.view(), &mut DatSourcesPageUi::default());
    assert!(rendered_text_contains(&output, "Matching preferences"));
    assert!(rendered_text_contains(&output, "Editing preferences for:"));
    assert!(rendered_text_contains(
        &output,
        "They do not enable or disable DATs."
    ));
    assert!(!rendered_text_contains(&output, "Applies to:"));

    page.apply(DatSourcesPageAction::SelectPolicyScope {
        scope: Some("NES".to_string()),
    });
    let rows_after = page
        .view()
        .rows
        .iter()
        .map(|row| row.enabled)
        .collect::<Vec<_>>();
    assert_eq!(rows_before, rows_after);
    assert!(
        page.tosec_packs
            .iter()
            .all(|pack| pack.selections.is_empty())
    );
}
