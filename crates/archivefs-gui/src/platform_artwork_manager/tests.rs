use super::*;
use crate::ui::platform_artwork::{
    CustomArtworkLoadError, PlatformArtworkSource, PlatformArtworkTexture,
    canonical_platform_for_artwork, decode_custom_platform_artwork, platform_asset_id,
    platform_fallback_asset_id,
};

fn session(root: Option<&Path>) -> PlatformArtworkManager {
    // No desktop service, database, user configuration or ArchiveFsApp fixture.
    PlatformArtworkManager::new(root.map(Path::to_path_buf), |_| {
        panic!("read-only tests must not open the desktop file manager")
    })
}

fn write_png(path: &Path, color: [u8; 4]) {
    image::RgbaImage::from_pixel(2, 2, image::Rgba(color))
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

fn paint(
    manager: &mut PlatformArtworkManager,
    context: &egui::Context,
    platform: &str,
) -> PlatformArtworkSource {
    let asset_id = platform_asset_id(platform, false);
    let mut source = None;
    let _ = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let assets = manager.render_assets();
            source = Some(paint_platform_artwork_at(
                ui,
                assets.cache,
                assets.directory,
                PlatformArtworkPaint {
                    center: egui::pos2(50.0, 50.0),
                    size: 64.0,
                    color: egui::Color32::WHITE,
                    asset_id: &asset_id,
                    fallback_asset_id: platform_fallback_asset_id(platform, false),
                },
            ));
        });
    });
    source.unwrap()
}

fn custom_texture(
    manager: &mut PlatformArtworkManager,
    context: &egui::Context,
    asset: &str,
) -> Option<PlatformArtworkTexture> {
    let assets = manager.render_assets();
    assets
        .cache
        .custom_texture(context, assets.directory, asset)
}

fn finish_task(manager: &mut PlatformArtworkManager, context: &egui::Context) {
    // Wait only on this session's temporary-directory worker, then feed the
    // actual reply through the same poll path. No production state is reachable.
    let result = manager
        .state
        .task
        .take()
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    sender.send(result).unwrap();
    manager.state.task = Some(receiver);
    manager.poll(context);
}

#[test]
fn known_platform_and_aliases_keep_canonical_filenames() {
    for alias in ["PSX", "PlayStation", "PS1"] {
        assert_eq!(canonical_platform_for_artwork(alias).unwrap().id, "PSX");
        assert_eq!(platform_asset_id(alias, false), "psx");
    }
    for (platform, file) in [
        ("GameCube", "gamecube"),
        ("WiiU", "wiiu"),
        ("Xbox360", "xbox360"),
    ] {
        assert_eq!(platform_asset_id(platform, false), file);
    }
    assert_ne!(
        platform_asset_id("Wii", false),
        platform_asset_id("WiiU", false)
    );
    assert_eq!(
        platform_asset_id("something with Wii in its name", false),
        "unknown"
    );
}

#[test]
fn missing_unknown_artwork_keeps_glyph_and_feature_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let mut manager = session(Some(temp.path()));
    let context = egui::Context::default();
    assert_eq!(
        paint(&mut manager, &context, "not a platform"),
        PlatformArtworkSource::Glyph
    );
    assert_eq!(
        current_artwork_source(Some(temp.path()), "not a platform", None),
        ("Unknown fallback", false)
    );
    assert!(manager.cache.entries.is_empty());
    assert!(
        manager.state.message.is_none(),
        "expected absence is not an error banner"
    );
}

#[test]
fn dragon_coco_unknown_fallback_is_preserved_not_silently_remapped() {
    assert_eq!(
        platform_asset_category("Dragon / Tandy CoCo"),
        PlatformAssetCategory::Unknown
    );
    assert_eq!(
        current_artwork_source(None, "Dragon / Tandy CoCo", None),
        ("Unknown fallback", false)
    );
}

#[test]
fn unchanged_custom_artwork_reuses_the_same_texture() {
    let temp = tempfile::tempdir().unwrap();
    write_png(&temp.path().join("gamecube.png"), [1, 2, 3, 255]);
    let context = egui::Context::default();
    let mut manager = session(Some(temp.path()));
    let first = custom_texture(&mut manager, &context, "gamecube").unwrap();
    assert_eq!(
        custom_texture(&mut manager, &context, "gamecube"),
        Some(first)
    );
    assert_eq!(manager.cache.entries.len(), 1);
}

#[test]
fn explicit_invalidation_refreshes_custom_and_bundled_textures() {
    let temp = tempfile::tempdir().unwrap();
    write_png(&temp.path().join("gamecube.png"), [1, 2, 3, 255]);
    let context = egui::Context::default();
    let mut manager = session(Some(temp.path()));
    let first = custom_texture(&mut manager, &context, "gamecube").unwrap();
    assert_eq!(
        paint(&mut manager, &context, "PS2"),
        PlatformArtworkSource::Bundled
    );
    manager.invalidate();
    assert!(manager.cache.entries.is_empty());
    assert!(manager.cache.bundled_entries.is_empty());
    assert!(manager.cache.directory.is_none());
    assert_ne!(
        custom_texture(&mut manager, &context, "gamecube"),
        Some(first)
    );
}

#[test]
fn changed_file_metadata_reloads_and_missing_file_does_not_stick() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gamecube.png");
    let context = egui::Context::default();
    let mut manager = session(Some(temp.path()));
    assert!(custom_texture(&mut manager, &context, "gamecube").is_none());
    write_png(&path, [1, 2, 3, 255]);
    let first = custom_texture(&mut manager, &context, "gamecube").unwrap();
    let old_time = manager.cache.entries["gamecube"].fingerprint.modified;
    write_png(&path, [8, 9, 10, 255]);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(old_time + std::time::Duration::from_secs(3)),
        )
        .unwrap();
    assert_ne!(
        custom_texture(&mut manager, &context, "gamecube"),
        Some(first)
    );
    std::fs::remove_file(&path).unwrap();
    assert!(custom_texture(&mut manager, &context, "gamecube").is_none());
    assert!(!manager.cache.entries.contains_key("gamecube"));
}

#[test]
fn malformed_custom_is_negatively_cached_without_poisoning_another_platform() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("gamecube.png"), b"not a PNG").unwrap();
    write_png(&temp.path().join("ps2.png"), [1, 2, 3, 255]);
    let context = egui::Context::default();
    let mut manager = session(Some(temp.path()));
    assert_eq!(
        paint(&mut manager, &context, "GameCube"),
        PlatformArtworkSource::Bundled
    );
    assert!(manager.cache.entries["gamecube"].texture.is_none());
    let failed = manager.cache.entries["gamecube"].fingerprint.clone();
    assert_eq!(
        paint(&mut manager, &context, "PS2"),
        PlatformArtworkSource::Custom
    );
    assert!(manager.cache.entries["ps2"].texture.is_some());
    assert_eq!(
        paint(&mut manager, &context, "GameCube"),
        PlatformArtworkSource::Bundled
    );
    assert_eq!(manager.cache.entries["gamecube"].fingerprint, failed);
    assert_eq!(manager.cache.entries.len(), 2);
}

#[test]
fn platform_switch_uses_its_own_key_and_keeps_previous_valid_cache() {
    let temp = tempfile::tempdir().unwrap();
    write_png(&temp.path().join("gamecube.png"), [1, 2, 3, 255]);
    let context = egui::Context::default();
    let mut manager = session(Some(temp.path()));
    assert_eq!(
        paint(&mut manager, &context, "GameCube"),
        PlatformArtworkSource::Custom
    );
    let first = custom_texture(&mut manager, &context, "gamecube");
    assert_eq!(
        paint(&mut manager, &context, "PS2"),
        PlatformArtworkSource::Bundled
    );
    assert_eq!(
        paint(&mut manager, &context, "unknown"),
        PlatformArtworkSource::Glyph
    );
    assert_eq!(custom_texture(&mut manager, &context, "gamecube"), first);
    assert_eq!(manager.cache.entries.len(), 1);
}

#[test]
fn managed_artwork_source_prefers_custom_over_bundled_artwork() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        current_artwork_source(Some(temp.path()), "PS2", None),
        ("Bundled", false)
    );
    write_png(&temp.path().join("ps2.png"), [1, 2, 3, 255]);
    assert_eq!(
        current_artwork_source(Some(temp.path()), "PS2", None),
        ("Custom", true)
    );
    assert_eq!(
        current_artwork_source(Some(temp.path()), "MasterSystem", None),
        ("Bundled", false)
    );
}

#[test]
fn status_rescan_reports_invalid_custom_and_retains_fallback_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("gamecube.png"), b"bad PNG").unwrap();
    let context = egui::Context::default();
    let mut manager = session(Some(temp.path()));
    manager.prepare_settings(&context);
    finish_task(&mut manager, &context);
    let status = manager.state.status.as_ref().unwrap();
    assert_eq!(status.invalid_custom_files.len(), 1);
    assert_eq!(
        current_artwork_source(Some(temp.path()), "GameCube", Some(status)),
        ("Bundled", false)
    );
    assert_eq!(
        paint(&mut manager, &context, "GameCube"),
        PlatformArtworkSource::Bundled
    );
    assert_eq!(
        std::fs::read(temp.path().join("gamecube.png")).unwrap(),
        b"bad PNG"
    );
}

#[test]
fn mutation_result_invalidates_cache_and_schedules_read_only_status_refresh() {
    let temp = tempfile::tempdir().unwrap();
    write_png(&temp.path().join("ps2.png"), [1, 2, 3, 255]);
    let context = egui::Context::default();
    let mut manager = session(Some(temp.path()));
    assert!(custom_texture(&mut manager, &context, "ps2").is_some());
    let (sender, receiver) = mpsc::channel();
    sender
        .send(PlatformArtworkTaskResult::Mutation(Err(
            "import refused".into()
        )))
        .unwrap();
    manager.state.task = Some(receiver);
    manager.poll(&context);
    assert!(manager.cache.entries.is_empty());
    assert_eq!(
        manager.state.message,
        Some((false, "import refused".into()))
    );
    finish_task(&mut manager, &context);
    assert!(manager.state.status.is_some());
    assert_eq!(
        manager.state.message,
        Some((false, "import refused".into()))
    );
}

#[test]
fn unavailable_root_reports_existing_error_without_starting_work() {
    let mut manager = session(None);
    manager.prepare_settings(&egui::Context::default());
    assert!(manager.state.task.is_none());
    assert_eq!(
        manager.state.message,
        Some((
            false,
            "EmuWiz could not resolve its local data directory.".into()
        ))
    );
}

#[test]
fn unopenable_path_and_unsupported_extension_fail_safely() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        decode_custom_platform_artwork(&temp.path().join("gone.png")),
        Err(CustomArtworkLoadError::Metadata)
    );
    assert_eq!(
        decode_custom_platform_artwork(&temp.path().join("manual.svg")),
        Err(CustomArtworkLoadError::UnsupportedPath)
    );
    let mut manager = session(Some(temp.path()));
    assert_eq!(
        paint(&mut manager, &egui::Context::default(), "GameCube"),
        PlatformArtworkSource::Bundled
    );
}

#[cfg(unix)]
#[test]
fn unreadable_custom_file_falls_back_without_poisoning_other_keys() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gamecube.png");
    write_png(&path, [1, 2, 3, 255]);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root/capability-enabled runners bypass Unix mode bits. The metadata/path
    // refusal test covers the portable failure path; do not pretend chmod can
    // simulate EACCES on those runners.
    if std::fs::File::open(&path).is_ok() {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    assert_eq!(
        decode_custom_platform_artwork(&path),
        Err(CustomArtworkLoadError::Metadata)
    );
    let mut manager = session(Some(temp.path()));
    let context = egui::Context::default();
    assert_eq!(
        paint(&mut manager, &context, "GameCube"),
        PlatformArtworkSource::Bundled
    );
    assert!(manager.cache.entries["gamecube"].texture.is_none());
    assert_eq!(
        paint(&mut manager, &context, "PS2"),
        PlatformArtworkSource::Bundled
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_cannot_redirect_artwork_lookup_outside_managed_directory() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write_png(&outside.path().join("source.png"), [1, 2, 3, 255]);
    std::os::unix::fs::symlink(
        outside.path().join("source.png"),
        temp.path().join("gamecube.png"),
    )
    .unwrap();
    let mut manager = session(Some(temp.path()));
    assert_eq!(
        paint(&mut manager, &egui::Context::default(), "GameCube"),
        PlatformArtworkSource::Bundled
    );
    assert!(manager.cache.entries.is_empty());
}

/// Rendered-text check local to the artwork-picker tests.
#[cfg(test)]
fn picker_output_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
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

#[test]
fn file_pick_drain_keeps_pending_while_empty() {
    let (sender, receiver) = mpsc::channel();
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Pending);
    // A second drain on the still-open channel is still Pending.
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Pending);
    drop(sender);
}

#[test]
fn file_pick_drain_disconnected_is_reported_and_repeated_drains_do_not_panic() {
    let (_sender, receiver) = mpsc::channel::<Option<PathBuf>>();
    drop(_sender);
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Disconnected);
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Disconnected);
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Disconnected);
}

#[test]
fn file_pick_drain_cancel_is_not_an_error() {
    let (sender, receiver) = mpsc::channel();
    sender.send(None).unwrap();
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Cancelled);
    // A cancelled channel then disconnects cleanly.
    drop(sender);
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Disconnected);
}

#[test]
fn file_pick_drain_picked_returns_the_path() {
    let (sender, receiver) = mpsc::channel();
    sender.send(Some(PathBuf::from("/tmp/pic.png"))).unwrap();
    assert_eq!(
        drain_file_pick(&receiver),
        FilePickDrain::Picked(PathBuf::from("/tmp/pic.png"))
    );
}

#[test]
fn a_disconnected_picker_releases_and_shows_a_friendly_error() {
    let (_sender, receiver) = mpsc::channel::<Option<PathBuf>>();
    drop(_sender);
    let mut manager = PlatformArtworkManagerState {
        pending_pick: Some(FilePickRequest {
            platform_id: "gamecube".to_string(),
            custom: false,
            receiver,
        }),
        ..Default::default()
    };
    let mut cache = PlatformArtworkCache {
        directory: None,
        entries: std::collections::HashMap::new(),
        bundled_entries: std::collections::HashMap::new(),
    };
    let mut action = None;
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_platform_artwork_manager(ui, None, &mut cache, &mut manager, &mut action);
        });
    });
    assert!(
        manager.pending_pick.is_none(),
        "a disconnected picker must release pending_pick so a new picker can start"
    );
    assert!(
        picker_output_text_contains(
            &output,
            "The image picker closed unexpectedly. Please try again."
        ),
        "the friendly error must be visible"
    );
}

#[test]
fn a_still_pending_picker_is_not_released_and_blocks_a_second_one() {
    let (sender, receiver) = mpsc::channel();
    let mut manager = PlatformArtworkManagerState {
        pending_pick: Some(FilePickRequest {
            platform_id: "gamecube".to_string(),
            custom: false,
            receiver,
        }),
        ..Default::default()
    };
    let mut cache = PlatformArtworkCache {
        directory: None,
        entries: std::collections::HashMap::new(),
        bundled_entries: std::collections::HashMap::new(),
    };
    let mut action = None;
    let context = egui::Context::default();
    let _output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_platform_artwork_manager(ui, None, &mut cache, &mut manager, &mut action);
        });
    });
    assert!(
        manager.pending_pick.is_some(),
        "an open picker stays pending; no second dialog can start"
    );
    drop(sender);
}

#[test]
fn a_cancelled_picker_releases_without_an_error() {
    let (sender, receiver) = mpsc::channel();
    sender.send(None).unwrap();
    let mut manager = PlatformArtworkManagerState {
        pending_pick: Some(FilePickRequest {
            platform_id: "gamecube".to_string(),
            custom: false,
            receiver,
        }),
        ..Default::default()
    };
    let mut cache = PlatformArtworkCache {
        directory: None,
        entries: std::collections::HashMap::new(),
        bundled_entries: std::collections::HashMap::new(),
    };
    let mut action = None;
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_platform_artwork_manager(ui, None, &mut cache, &mut manager, &mut action);
        });
    });
    assert!(
        manager.pending_pick.is_none(),
        "a cancelled picker is released"
    );
    assert!(
        !picker_output_text_contains(
            &output,
            "The image picker closed unexpectedly. Please try again."
        ),
        "cancelling must not show an error"
    );
    assert!(
        manager.pending_import.is_none() && action.is_none(),
        "cancelling changes nothing"
    );
}

#[test]
fn picked_image_leaves_settings_as_a_typed_action_without_starting_an_import() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("selected.png");
    write_png(&source, [1, 2, 3, 255]);
    let before = std::fs::read(&source).unwrap();
    let mut manager = session(None);
    let (sender, receiver) = mpsc::channel();
    sender.send(Some(source.clone())).unwrap();
    manager.state.pending_pick = Some(FilePickRequest {
        platform_id: "PS2".into(),
        custom: false,
        receiver,
    });
    manager.state.search = "no matching platform".into();
    let context = egui::Context::default();
    let mut action = None;
    let _ = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            action = manager.show(ui);
        });
    });
    assert!(
        matches!(action, Some(PlatformArtworkManagerAction::Import { platform_id, source: selected })
        if platform_id == "PS2" && selected == source)
    );
    assert!(manager.state.pending_pick.is_none());
    assert!(
        manager.state.task.is_none(),
        "the app still arbitrates and dispatches the action"
    );
    assert_eq!(std::fs::read(&source).unwrap(), before);
}

#[test]
fn running_task_blocks_a_second_action_including_folder_opening() {
    let temp = tempfile::tempdir().unwrap();
    let mut manager = session(Some(temp.path()));
    let (sender, receiver) = mpsc::channel();
    manager.state.task = Some(receiver);
    manager.dispatch(
        egui::Context::default(),
        PlatformArtworkManagerAction::OpenFolder,
    );
    assert!(manager.state.task.is_some());
    assert!(manager.state.message.is_none());
    assert!(
        sender
            .send(PlatformArtworkTaskResult::Mutation(Ok(
                "original task".into()
            )))
            .is_ok()
    );
}
