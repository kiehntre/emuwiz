use super::*;
use archivefs_core::game_identity::{
    GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat,
    IdentityPlatform, IdentityProvenance,
};

fn game(platform: &str, extension: &str) -> Game {
    let mut archive = crate::gui_v2::tests::archive(19, "Synthetic Acorn", Some(platform));
    archive.absolute_path = format!("/synthetic/game.{extension}").into();
    Game::from_archive(archive)
}
fn text(shape: &egui::epaint::Shape, output: &mut String) {
    match shape {
        egui::epaint::Shape::Text(shape) => {
            output.push_str(shape.galley.text());
            output.push('\n');
        }
        egui::epaint::Shape::Vec(shapes) => {
            for shape in shapes {
                text(shape, output);
            }
        }
        _ => {}
    }
}
fn rendered(game: &Game, panel: &Panel) -> String {
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            render(ui, game, panel);
        });
    });
    let mut result = String::new();
    for shape in output.shapes {
        text(&shape.shape, &mut result);
    }
    result
}
#[test]
fn blue_preview_explains_read_only_boundary_and_mr_wiz_guidance() {
    let game = game("Acorn Electron", "ssd");
    let output = rendered(&game, &Panel::default());
    for expected in [
        "Preview-only",
        "Mr Wiz",
        "Elkulator Launch unavailable",
        "No media inspected yet",
        "not proof",
    ] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
    assert!(!output.contains("Ready to play"));
    assert!(applies(&game));
}
#[test]
fn bbc_identity_is_never_relabelled_as_electron_and_shared_formats_stay_ambiguous() {
    let bbc = game("BBC Micro", "dsd");
    assert!(platform_message(&bbc).contains("never converted"));
    assert!(applies(&bbc));
    let other = game("Unknown system", "uef");
    assert!(platform_message(&other).contains("cannot distinguish"));
    assert!(applies(&other));
    assert!(!applies(&game("SNES", "sfc")));
}
#[test]
fn conflicting_platform_evidence_is_visible_without_library_modification() {
    let mut game = game("Acorn Electron", "ssd");
    game.archive.identity_report = Some(GameIdentityReport {
        archive_path: game.archive.absolute_path.clone(),
        platform: IdentityPlatform::Other,
        format: IdentityImageFormat::Deferred,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::Platform,
            status: IdentityStatus::Verified,
            value: Some("BBC Micro".into()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: game.archive.absolute_path.clone(),
                member_path: None,
                member_index: None,
                method: "fixture".into(),
            },
            diagnostic: "fixture".into(),
        }],
        warnings: vec![],
        bytes_read: 0,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    });
    let before = game.archive.identity_report.clone();
    assert!(platform_message(&game).contains("Conflicting"));
    assert_eq!(game.archive.identity_report, before);
    assert_eq!(game.platform, "Acorn Electron");
    game.platform = "Unknown system".into();
    assert!(!platform_message(&game).contains("Conflicting"));
}
#[test]
fn recheck_catches_changed_files_and_missing_emulator_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("tape.uef");
    std::fs::write(&path, b"UEF File!\0\x0a\x00").unwrap();
    let first = inspect(path.clone(), None, Some(OsString::new())).unwrap();
    assert!(first.fresh);
    assert!(first.discovery.executables.is_empty());
    let cfg = temp.path().join("elk.cfg");
    std::fs::write(&cfg, b"sentinel").unwrap();
    let before = std::fs::read(&path).unwrap();
    let count = std::fs::read_dir(temp.path()).unwrap().count();
    let same = inspect(
        path.clone(),
        Some(first.image.clone()),
        Some(OsString::new()),
    )
    .unwrap();
    assert!(same.fresh);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(&cfg).unwrap(), b"sentinel");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), count);
    std::fs::write(&path, b"changed").unwrap();
    let stale = inspect(path, Some(first.image), Some(OsString::new())).unwrap();
    assert!(!stale.fresh);
    let panel = Panel {
        result: Some(stale),
        ..Default::default()
    };
    let output = rendered(&game("Acorn Electron", "uef"), &panel);
    assert!(output.contains("Stale:"), "{output}");
    assert!(output.contains("not discovered"), "{output}");
}

#[test]
fn delayed_worker_reply_retains_original_freshness_timestamp() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("tape.uef");
    std::fs::write(&path, b"UEF File!\0\x0a\x00").unwrap();
    let mut result = inspect(path, None, Some(OsString::new())).unwrap();
    result.checked_at = Instant::now() - Duration::from_secs(10);
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(result)).unwrap();
    let mut panel = Panel {
        job: Some(rx),
        ..Default::default()
    };
    panel.poll();
    assert!(panel.checked.unwrap().elapsed() >= Duration::from_secs(10));
    assert!(panel.result.unwrap().fresh);
}
