//! Provider evidence presentation without network or real user data.
use super::*;

#[test]
fn job40_cached_artwork_from_a_stale_record_stays_visible_with_a_warning() {
    use archivefs_core::identity_source::model::ExternalIdentityRecord;
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let library = Library::new(vec![archive(40, "Saved provider picture", Some("NES"))]);
    let record: ExternalIdentityRecord = serde_json::from_value(serde_json::json!({
        "provider":"romm", "server_id":"fixture", "provider_game_id":"40",
        "provider_path":"/game", "regions":[], "hashes":[],
        "metadata_provider_ids":[], "related_files":[], "sibling_game_ids":[],
        "imported_at_unix_seconds":1, "verification":"stale", "conflicts":[], "evidence":[],
        "artwork":{"reference":"/cover.png", "small_reference":"/cover.png"}
    }))
    .unwrap();
    let mut index = MediaIndex::default();
    index.covers.insert(
        40,
        Source::Remote {
            record: Arc::new(record),
            kind: Kind::Cover,
        },
    );
    app.artwork.index = Some(Arc::new(index));
    let key = app.artwork.key(40, Kind::Cover);
    let texture = context.load_texture(
        "job40",
        egui::ColorImage::filled([8, 8], egui::Color32::GREEN),
        Default::default(),
    );
    app.artwork.pictures.insert(
        key,
        Picture::Ready {
            texture,
            timings: super::super::thumbnail::Timings {
                cache_hit: true,
                ..Default::default()
            },
        },
    );
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            app.picture(
                ui,
                library.game(40).unwrap(),
                Kind::Cover,
                egui::vec2(180.0, 240.0),
            );
        });
    });
    assert!(
        visible_text(&output, "Saved artwork · stale record"),
        "known stale provider evidence must be labelled"
    );
    assert!(
        matches!(app.artwork.pictures.get(&key), Some(Picture::Ready { .. })),
        "staleness must not silently discard a cached picture"
    );
    let index = Arc::get_mut(app.artwork.index.as_mut().unwrap()).unwrap();
    let Source::Remote { record, .. } = index.covers.get_mut(&40).unwrap() else {
        unreachable!()
    };
    Arc::make_mut(record).verification =
        archivefs_core::identity_source::model::ExternalVerification::StrongExternal;
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            app.picture(
                ui,
                library.game(40).unwrap(),
                Kind::Cover,
                egui::vec2(180.0, 240.0),
            );
        });
    });
    assert!(
        !visible_text(&output, "Saved artwork · stale record"),
        "a refreshed association must clear the stale label"
    );
}

#[test]
fn job40_provider_outage_does_not_block_native_play_preflight() {
    use archivefs_core::identity_source::romm::connectivity::RommConnectivity;
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(40, "Local game", Some("SNES"))]));
    app.indices = vec![0];
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    let key = app.artwork.key(40, Kind::Cover);
    app.artwork.pictures.insert(
        key,
        Picture::Unavailable {
            issue: RommConnectivity::DnsFailure,
            message: "RomM cannot currently be reached.".into(),
        },
    );
    app.router.current = Route::Task {
        section: Section::Launch,
        game: 40,
    };
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(
        strings
            .iter()
            .any(|text| text.contains("Play / Launch readiness"))
    );
    assert!(app.native_workflows.is_some());
    assert!(
        app.activity.running() > 0,
        "native preflight still starts despite the picture failure"
    );
    assert!(matches!(
        app.artwork.pictures.get(&key),
        Some(Picture::Unavailable {
            issue: RommConnectivity::DnsFailure,
            ..
        })
    ));
}
