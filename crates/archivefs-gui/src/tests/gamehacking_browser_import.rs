use super::*;

use archivefs_core::patch_manager::{
    BrowserImportLocalIdentity, BrowserImportPlatform, plan_gamehacking_browser_import,
};

#[test]
fn browser_import_panel_exposes_only_user_mediated_routes() {
    let cache = tempfile::tempdir().expect("cache");
    let identity = BrowserImportLocalIdentity::GameCube {
        title: "Test Racer".to_string(),
        dolphin_game_id: "GTRE01".to_string(),
        region: Some("E".to_string()),
    };
    let plan = plan_gamehacking_browser_import(
        BrowserImportPlatform::GameCube,
        501,
        Some("https://gamehacking.org/game/501"),
        &identity,
        cache.path(),
    )
    .expect("plan");
    let mut state = BrowserImportState::new(plan, identity, "Test Racer".to_string());
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let _ = show_browser_import(ui, &mut state);
        });
    });

    for expected in [
        "Open game page in browser",
        "Import saved page",
        "Paste page/export",
        "Paste from clipboard",
        "Copy page URL",
        "Cancel",
        "game-501.html",
        "export-501.txt",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    assert!(!rendered_text_contains(&output, "Install"));
    assert!(!rendered_text_contains(&output, "Apply"));
}

#[test]
fn browser_import_failure_banner_keeps_local_reason_visible() {
    let directory = tempfile::tempdir().expect("workflow directory");
    let mut app = dolphin_workflow_with_matched_identity(directory.path(), "GTRE01");
    let workflow = app.cheat_workflow.as_mut().expect("workflow");
    workflow.browser_import_open_error = Some((
        "Local game identity incomplete".to_string(),
        "Verify the local game identity before importing.".to_string(),
    ));
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_browser_import_open_error(ui, workflow);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "Local game identity incomplete"
    ));
    assert!(rendered_text_contains(
        &output,
        "Verify the local game identity before importing."
    ));
}
