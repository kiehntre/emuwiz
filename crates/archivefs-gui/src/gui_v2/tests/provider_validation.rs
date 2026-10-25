//! Provider evidence presentation without network or real user data.
use super::*;

#[test]
fn job40_small_provider_thumbnail_remains_available_when_large_cover_is_uncached() {
    use archivefs_core::identity_source::{
        artwork::{ArtworkCache, ArtworkKind, ArtworkRequest},
        model::IdentityProvider,
        net_policy::StaticResolver,
        romm::{
            client::{RommHttpResponse, RommRequestError, RommTransport},
            config::{RommSourceConfig, RommToken, ValidatedRommSource},
        },
    };
    struct ImageServer;
    impl RommTransport for ImageServer {
        fn get(
            &self,
            _: &str,
            _: Option<&str>,
            _: usize,
            _: std::time::Duration,
        ) -> Result<RommHttpResponse, RommRequestError> {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            Ok(RommHttpResponse {
                status: 200,
                body: bytes.into_inner(),
                location: None,
            })
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let settings = RommSourceConfig {
        enabled: true,
        url: "http://127.0.0.1:8080".into(),
        ..Default::default()
    };
    let token = RommToken::parse("job40-synthetic-token").unwrap();
    let source =
        ValidatedRommSource::validate(&settings, &token, &[], &StaticResolver::new()).unwrap();
    let cache = ArtworkCache::new(directory.path(), IdentityProvider::Romm);
    let request = ArtworkRequest {
        provider_game_id: "40",
        kind: ArtworkKind::Cover,
        large_reference: Some("/large.png?ts=2"),
        small_reference: Some("/small.png?ts=2"),
        public_reference: None,
    };
    let small = cache
        .fetch(&source, &ImageServer, &request.without_large(), 1, None)
        .unwrap();
    let saved_bytes = fs::read(&small.path).unwrap();
    // A fresh instance models restart; lookup has no transport and must work offline.
    let reopened = ArtworkCache::new(directory.path(), IdentityProvider::Romm);
    let found = reopened.lookup(&settings.url, &request).expect(
        "the exact matching small cover is an offline fallback for an uncached large cover",
    );
    assert_eq!(found.path, small.path);
    assert_eq!(fs::read(found.path).unwrap(), saved_bytes);
    let changed = ArtworkRequest {
        small_reference: Some("/small.png?ts=3"),
        ..request
    };
    assert!(
        reopened.lookup(&settings.url, &changed).is_none(),
        "a changed provider reference must still invalidate the thumbnail"
    );
    assert!(
        reopened.lookup("http://127.0.0.2:8080", &request).is_none(),
        "another server cannot borrow the picture"
    );
}

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
