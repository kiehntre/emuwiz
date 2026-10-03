use super::*;
use archivefs_core::identity_source::{
    model::IdentityProvider,
    net_policy::StaticResolver,
    romm::{
        client::{RommHttpResponse, RommRequestError, RommTransport},
        config::RommSourceConfig,
    },
    settings::{ProviderSettings, SettingsLocation},
};
use serde_json::{Value, json};
use std::sync::Mutex;

const TOKEN: &str = "synthetic-romm-secret-never-render";
struct Fixture {
    version: &'static str,
    status: u16,
    artwork: bool,
    requests: Mutex<Vec<String>>,
}
impl Default for Fixture {
    fn default() -> Self {
        Self {
            version: "5.3.1",
            status: 200,
            artwork: false,
            requests: Mutex::new(vec![]),
        }
    }
}
impl Fixture {
    fn game(&self, id: u64) -> Value {
        let mut value = game(id);
        if self.artwork {
            value["path_cover_small"] = json!("/assets/romm/resources/small.png");
            value["path_cover_large"] = json!("/assets/romm/resources/large.png");
        }
        value
    }
}
fn game(id: u64) -> Value {
    json!({"id":id,"platform_id":7,"platform_slug":"gb","platform_fs_slug":"gb",
        "name":format!("Fixture game {id}"),"fs_name":format!("Fixture{id}.gb"),"fs_path":"roms/gb",
        "fs_size_bytes":32768,"regions":["USA"],"crc_hash":"DEADBEEF","igdb_id":101,
        "files":[],"sibling_roms":[],"metadatum":{"genres":["Puzzle"]}})
}
fn api(version: &str) -> Value {
    let params = [
        "limit",
        "offset",
        "search_term",
        "platform_ids",
        "with_files",
        "with_char_index",
        "with_filter_values",
        "with_rom_id_index",
        "group_by_meta_id",
    ]
    .map(|name| json!({"name":name,"in":"query"}));
    json!({"openapi":"3.1.0","info":{"version":version},"paths":{
        "/api/platforms":{"get":{}},"/api/roms":{"get":{"parameters":params}},"/api/roms/{id}":{"get":{}},
        "/api/roms/{id}/content":{"get":{}},"/api/scan":{"post":{}}}})
}
fn query(url: &str, key: &str) -> Option<String> {
    url.split_once('?')?.1.split('&').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k == key).then(|| v.to_owned())
    })
}
impl RommTransport for Fixture {
    fn get(
        &self,
        url: &str,
        authorization: Option<&str>,
        _: usize,
        _: Duration,
    ) -> Result<RommHttpResponse, RommRequestError> {
        self.requests.lock().unwrap().push(url.to_owned());
        let path = url.split_once("://").unwrap().1.split_once('/').unwrap().1;
        if path.starts_with("assets/romm/resources/") {
            let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([12, 34, 56, 255]));
            let mut bytes = std::io::Cursor::new(vec![]);
            image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            return Ok(RommHttpResponse {
                status: 200,
                body: bytes.into_inner(),
                location: None,
            });
        }
        let value = if path == "api/heartbeat" {
            json!({"SYSTEM":{"VERSION":self.version}})
        } else if path == "openapi.json" {
            api(self.version)
        } else if self.status != 200 {
            return Ok(RommHttpResponse {
                status: self.status,
                body: TOKEN.as_bytes().to_vec(),
                location: None,
            });
        } else if path == "api/platforms" {
            json!([{"id":7,"slug":"gb","fs_slug":"gb","name":"Game Boy","rom_count":55},{"id":8,"slug":"custom","name":"Custom console","rom_count":0}])
        } else if path.starts_with("api/roms?") {
            assert!(authorization.is_some());
            let limit: usize = query(url, "limit").unwrap().parse().unwrap();
            let offset: usize = query(url, "offset").unwrap().parse().unwrap();
            let search = query(url, "search_term");
            let empty = search.as_deref() == Some("Nothing")
                || query(url, "platform_ids").as_deref() == Some("8");
            let total = if empty {
                0
            } else if search.is_some() {
                3
            } else {
                55
            };
            let items = (1..=total)
                .skip(offset)
                .take(limit)
                .map(|id| self.game(id))
                .collect::<Vec<_>>();
            json!({"items":items,"limit":limit,"offset":offset,"total":total})
        } else if let Some(id) = path.strip_prefix("api/roms/") {
            self.game(id.parse().unwrap())
        } else {
            panic!("unexpected read-only fixture endpoint")
        };
        Ok(RommHttpResponse {
            status: 200,
            body: serde_json::to_vec(&value).unwrap(),
            location: None,
        })
    }
}
fn settings(root: &std::path::Path, url: &str) {
    let token = root.join("synthetic-token");
    std::fs::write(&token, TOKEN).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    SettingsLocation::new(root, IdentityProvider::Romm)
        .save(&ProviderSettings {
            source: RommSourceConfig {
                enabled: true,
                url: url.into(),
                token_path: Some(token),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
}
fn call(fixture: &Fixture, request: Request) -> Reply {
    try_call(fixture, request).unwrap()
}
fn try_call(fixture: &Fixture, request: Request) -> Result<Reply, Problem> {
    let root = tempfile::tempdir().unwrap();
    settings(root.path(), "http://127.0.0.1:45678");
    worker::run_with(
        root.path(),
        request,
        &[],
        &StaticResolver::new(),
        fixture,
        &AtomicBool::new(false),
    )
}
fn connected() -> State {
    let ctx = egui::Context::default();
    let mut state = State {
        open: true,
        settings_loaded: true,
        configured: true,
        ..Default::default()
    };
    let reply = call(&Fixture::default(), Request::Connect);
    assert!(state.absorb(&ctx, state.generation, Ok(reply)));
    state
}
fn text(state: &mut State) -> String {
    fn extract(shape: &egui::Shape, strings: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => strings.push(t.galley.text().into()),
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    extract(s, strings)
                }
            }
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 800.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.body(ui);
            });
        },
    );
    let mut strings = vec![];
    for shape in &output.shapes {
        extract(&shape.shape, &mut strings);
    }
    strings.join("\n")
}

#[test]
fn unconfigured_settings_are_read_without_any_http_request() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::default();
    let reply = worker::run_with(
        root.path(),
        Request::Settings,
        &[],
        &StaticResolver::new(),
        &fixture,
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut state = State::default();
    state.absorb(&egui::Context::default(), 0, Ok(reply));
    let text = text(&mut state);
    assert!(text.contains("RomM isn't connected yet"));
    assert!(text.contains("Open RomM Settings"));
    assert!(fixture.requests.lock().unwrap().is_empty());
}
#[test]
fn unreadable_settings_settle_until_the_user_retries() {
    let root = tempfile::tempdir().unwrap();
    settings(root.path(), "http://127.0.0.1:45678");
    let location = SettingsLocation::new(root.path(), IdentityProvider::Romm);
    std::fs::write(location.config_path(), b"{not valid json}").unwrap();
    let fixture = Fixture::default();
    let result = worker::run_with(
        root.path(),
        Request::Settings,
        &[],
        &StaticResolver::new(),
        &fixture,
        &AtomicBool::new(false),
    );
    assert!(matches!(result, Err(Problem::Settings)));
    let ctx = egui::Context::default();
    let mut state = State {
        open: true,
        retry: Request::Settings,
        ..Default::default()
    };
    state.absorb(&ctx, state.generation, result);
    assert!(state.settings_loaded);
    assert!(text(&mut state).contains("Retry"));
    for _ in 0..3 {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.show(ctx);
        });
        assert!(state.browse.pending.is_none());
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
}
#[test]
fn existing_editor_opens_saved_settings_without_loading_a_catalogue() {
    let reply = call(&Fixture::default(), Request::Settings);
    let Reply::Settings(ref settings, _) = reply else {
        panic!()
    };
    let expected = settings.as_ref().clone();
    let mut state = State::default();
    state.absorb(&egui::Context::default(), 0, Ok(reply));
    let draft = state.settings_draft();
    assert_eq!(draft.url, expected.source.url);
    assert_eq!(draft.to_settings(None), expected);
}
#[test]
fn existing_editor_requires_an_explicit_enable_choice() {
    let mut state = State::default();
    state.absorb(
        &egui::Context::default(),
        0,
        Ok(Reply::Settings(Box::default(), None)),
    );
    let mut draft = state.settings_draft();
    assert!(!draft.to_settings(None).source.enabled);
    draft.enabled = Some(true);
    assert!(draft.to_settings(None).source.enabled);
}
#[test]
fn existing_editor_never_prefills_embedded_credentials_or_query_secrets() {
    for address in [
        format!("http://user:{TOKEN}@127.0.0.1"),
        format!("http://127.0.0.1?api_key={TOKEN}"),
    ] {
        let root = tempfile::tempdir().unwrap();
        settings(root.path(), &address);
        let reply = worker::run_with(
            root.path(),
            Request::Settings,
            &[],
            &StaticResolver::new(),
            &Fixture::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut state = State::default();
        state.absorb(&egui::Context::default(), 0, Ok(reply));
        assert!(state.settings_draft().url.is_empty());
        assert_eq!(state.saved_settings.as_ref().unwrap().source.url, address);
        assert!(!text(&mut state).contains(TOKEN));
    }
}
#[test]
fn successful_connection_loads_a_bounded_first_page() {
    let mut state = connected();
    assert_eq!(state.platforms.len(), 2);
    assert_eq!(state.page.as_ref().unwrap().games.len(), 50);
    let text = text(&mut state);
    assert!(text.contains("Connected to your RomM server"));
    assert!(text.contains("Showing 1–50 of 55"));
}
#[test]
fn dns_failure_explains_possible_causes_and_recovery_without_raw_errors() {
    let mut state = State {
        problem: Some(worker::config_problem(
            archivefs_core::identity_source::romm::config::ConfigRefusal::Endpoint(
                archivefs_core::identity_source::net_policy::EndpointRefusal::UnresolvableHost {
                    detail: TOKEN.into(),
                },
            ),
        )),
        configured: true,
        ..Default::default()
    };
    let text = text(&mut state);
    assert!(text.contains("couldn't find that RomM server"));
    assert!(text.contains("hostname may not resolve"));
    assert!(text.contains("Nothing on your RomM server"));
    assert!(text.contains("Retry"));
    assert!(!text.contains(TOKEN));
}
#[test]
fn network_failure_is_distinct_and_recoverable() {
    let mut state = State {
        configured: true,
        problem: Some(Problem::Backend(RommBrowseError::Unreachable)),
        ..Default::default()
    };
    let text = text(&mut state);
    assert!(text.contains("couldn't be reached"));
    assert!(text.contains("network connection"));
    assert!(text.contains("Retry"));
}
#[test]
fn authentication_failure_is_safe_and_explains_the_settings_action() {
    let fixture = Fixture {
        status: 401,
        ..Default::default()
    };
    let mut state = State {
        configured: true,
        ..Default::default()
    };
    state.absorb(
        &egui::Context::default(),
        0,
        Ok(call(&fixture, Request::Connect)),
    );
    let text = text(&mut state);
    assert!(text.contains("RomM rejected the login details"));
    assert!(text.contains("Open RomM Settings"));
    assert!(!text.contains(TOKEN));
}
#[test]
fn tls_failure_does_not_recommend_disabling_verification() {
    let mut state = State {
        problem: Some(Problem::Backend(RommBrowseError::Tls)),
        ..Default::default()
    };
    let text = text(&mut state);
    assert!(text.contains("security certificate"));
    assert!(text.contains("machine's clock"));
    assert!(!text.to_lowercase().contains("disable"));
}
#[test]
fn unsupported_version_has_a_friendly_explanation_and_collapsed_details() {
    let mut state = State {
        configured: true,
        ..Default::default()
    };
    state.absorb(
        &egui::Context::default(),
        0,
        Ok(call(
            &Fixture {
                version: "2.0.0",
                ..Default::default()
            },
            Request::Connect,
        )),
    );
    let text = text(&mut state);
    assert!(text.contains("server can't provide"));
    assert!(text.contains("Details"));
    assert!(!text.contains("RomM version: 2.0.0"));
}
#[test]
fn server_error_bodies_never_become_gui_messages() {
    let mut state = connected();
    let error = try_call(
        &Fixture {
            status: 500,
            ..Default::default()
        },
        Request::Connect,
    )
    .err()
    .unwrap();
    state.absorb(&egui::Context::default(), state.generation, Err(error));
    let text = text(&mut state);
    assert!(!text.contains(TOKEN));
    assert!(text.contains("Nothing on your RomM server"));
}
#[test]
fn credentials_embedded_in_settings_are_refused_before_http_or_rendering() {
    let root = tempfile::tempdir().unwrap();
    settings(
        root.path(),
        &format!("http://{}:{}@127.0.0.1:45678", "fixture-user", TOKEN),
    );
    let fixture = Fixture::default();
    let result = worker::run_with(
        root.path(),
        Request::Connect,
        &[],
        &StaticResolver::new(),
        &fixture,
        &AtomicBool::new(false),
    );
    let mut state = State::default();
    state.absorb(&egui::Context::default(), 0, result);
    assert!(!text(&mut state).contains(TOKEN));
    assert!(fixture.requests.lock().unwrap().is_empty());
}
#[test]
fn platforms_use_friendly_names_and_all_platforms_is_the_default() {
    let mut state = connected();
    assert_eq!(state.platforms[0].name.as_deref(), Some("Game Boy"));
    assert_eq!(state.platforms[1].name.as_deref(), Some("Custom console"));
    assert!(text(&mut state).contains("All platforms"));
    state.platform = Some(7);
    assert!(text(&mut state).contains("Game Boy"));
}
#[test]
fn platform_filter_is_sent_to_the_server() {
    let fixture = Fixture::default();
    let Reply::Page(page, _) = call(
        &fixture,
        Request::Games {
            offset: 0,
            filter: RommBrowseFilter {
                platform_id: Some(7),
                ..Default::default()
            },
        },
    ) else {
        panic!()
    };
    assert_eq!(page.games.len(), 50);
    assert!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|url| url.contains("platform_ids=7"))
    );
}
#[test]
fn search_is_server_side_and_uses_the_backend_page_bounds() {
    let fixture = Fixture::default();
    let Reply::Page(page, _) = call(
        &fixture,
        Request::Games {
            offset: 0,
            filter: RommBrowseFilter {
                text: Some("Mario & Luigi".into()),
                ..Default::default()
            },
        },
    ) else {
        panic!()
    };
    assert_eq!(page.games.len(), 3);
    let urls = fixture.requests.lock().unwrap();
    assert!(
        urls.iter()
            .any(|u| u.contains("search_term=Mario+%26+Luigi"))
    );
    assert!(
        urls.iter()
            .filter(|u| u.contains("/api/roms?"))
            .all(|u| u.contains("limit=1&") || u.contains("limit=50&"))
    );
    assert!(urls.iter().any(|u| u.contains("with_rom_id_index=false")));
}
#[test]
fn search_edits_debounce_and_cancel_the_previous_generation_immediately() {
    let ctx = egui::Context::default();
    let mut state = connected();
    state.search = "A".into();
    state.edit_search(Instant::now());
    let a = state.generation;
    state.search = "B".into();
    state.edit_search(Instant::now());
    assert!(state.generation > a);
    let mut activity = Activity::default();
    state.poll(&ctx, &mut activity);
    assert!(state.browse.pending.is_none());
    assert!(state.browse.running.is_none());
    let root = tempfile::tempdir().unwrap();
    state.test_root = Some(root.path().into());
    state.edited = Some(Instant::now() - SEARCH_DELAY - Duration::from_millis(1));
    state.poll(&ctx, &mut activity);
    assert!(state.browse.running.is_some());
    assert!(matches!(&state.retry,Request::Games {filter,..} if filter.text.as_deref()==Some("B")));
}
#[test]
fn closing_or_opening_settings_cancels_the_debounce_timer() {
    let mut state = connected();
    state.edit_search(Instant::now() - SEARCH_DELAY);
    state.back();
    assert!(state.edited.is_none());
    assert!(state.browse.pending.is_none());
    state.open(vec![]);
    state.edit_search(Instant::now() - SEARCH_DELAY);
    state.settings_opened();
    assert!(state.edited.is_none());
    assert!(state.browse.pending.is_none());
    let mut activity = Activity::default();
    state.poll(&egui::Context::default(), &mut activity);
    assert!(activity.jobs.is_empty());
}
#[test]
fn slower_search_a_cannot_overwrite_search_b() {
    let ctx = egui::Context::default();
    let mut state = connected();
    state.search = "A".into();
    state.edit_search(Instant::now());
    let a = state.generation;
    state.search = "B".into();
    state.edit_search(Instant::now());
    let b = state.generation;
    let reply = || {
        call(
            &Fixture::default(),
            Request::Games {
                offset: 0,
                filter: Default::default(),
            },
        )
    };
    assert!(state.absorb(&ctx, b, Ok(reply())));
    let page = state.page.clone().unwrap();
    assert!(!state.absorb(&ctx, a, Ok(reply())));
    assert!(Arc::ptr_eq(&page, state.page.as_ref().unwrap()));
}
#[test]
fn pagination_requests_one_page_and_displays_the_correct_range() {
    let fixture = Fixture::default();
    let reply = call(
        &fixture,
        Request::Games {
            offset: 50,
            filter: Default::default(),
        },
    );
    let mut state = connected();
    state.absorb(&egui::Context::default(), state.generation, Ok(reply));
    assert_eq!(state.page.as_ref().unwrap().games.len(), 5);
    assert!(text(&mut state).contains("Showing 51–55 of 55"));
}
#[test]
fn detail_uses_external_identity_evidence_and_backend_metadata() {
    let mut state = connected();
    state.selected = Some(1);
    state.absorb(
        &egui::Context::default(),
        0,
        Ok(call(&Fixture::default(), Request::Detail(1))),
    );
    let text = text(&mut state);
    assert!(text.contains("Identity clues from RomM"));
    assert!(text.contains("RomM suggests"));
    assert!(text.contains("external evidence"));
    assert!(text.contains("deadbeef"));
}
#[test]
fn late_detail_for_a_different_selection_is_rejected() {
    let mut state = connected();
    state.selected = Some(2);
    assert!(!state.absorb(
        &egui::Context::default(),
        0,
        Ok(call(&Fixture::default(), Request::Detail(1)))
    ));
    assert!(state.detail.is_none());
}
#[test]
fn missing_artwork_is_a_normal_placeholder_and_does_not_schedule_a_fetch() {
    let mut state = connected();
    state.selected = Some(1);
    state.absorb(
        &egui::Context::default(),
        0,
        Ok(call(&Fixture::default(), Request::Detail(1))),
    );
    assert!(text(&mut state).contains("No cover artwork is available"));
    assert!(!state.art.active());
    assert!(state.problem.is_none());
}
#[test]
fn failed_artwork_does_not_replace_the_page_or_connection() {
    let ctx = egui::Context::default();
    let mut state = connected();
    let page = state.page.clone().unwrap();
    state.absorb(&ctx, 0, Ok(Reply::Cover { id: 1, image: None }));
    assert!(state.covers.contains_key(&1));
    assert!(state.problem.is_none());
    assert!(state.connected());
    assert!(Arc::ptr_eq(&page, state.page.as_ref().unwrap()));
}
#[test]
fn visible_artwork_uses_existing_cache_and_prefers_the_small_cover() {
    let fixture = Fixture {
        artwork: true,
        ..Default::default()
    };
    let Reply::Connection {
        page: Some(page), ..
    } = call(&fixture, Request::Connect)
    else {
        panic!()
    };
    let Reply::Cover { image, .. } =
        call(&fixture, Request::Cover(Box::new(page.games[0].clone())))
    else {
        panic!()
    };
    assert!(image.is_some());
    let requests = fixture.requests.lock().unwrap();
    assert!(requests.iter().any(|url| url.ends_with("/small.png")));
    assert!(!requests.iter().any(|url| url.ends_with("/large.png")));
    assert!(!requests.iter().any(|url| url.contains("/content")));
}
#[test]
fn failed_reconnect_does_not_leave_old_data_marked_connected() {
    let mut state = connected();
    state.queue(Request::Connect);
    assert!(state.info.is_none());
    assert!(state.page.is_none());
    assert!(text(&mut state).contains("Connecting to RomM"));
    state.browse.pending = None;
    state.absorb(
        &egui::Context::default(),
        state.generation,
        Err(Problem::Backend(RommBrowseError::Unreachable)),
    );
    assert!(!state.connected());
    assert!(text(&mut state).contains("Couldn't connect to RomM"));
}
#[test]
fn empty_unknown_total_last_page_keeps_a_way_back() {
    let mut state = connected();
    state.page = Some(Arc::new(RommBrowsePage {
        games: vec![],
        offset: 50,
        page_size: 50,
        total: None,
        next_offset: None,
        previous_offset: Some(0),
    }));
    let view = text(&mut state);
    assert!(view.contains("Previous"));
    assert!(view.contains("no games on this page"));
    assert!(!view.contains("Showing 51–50"));
}
#[test]
fn pagination_inconsistency_offers_a_first_page_recovery() {
    let mut state = connected();
    state.problem = Some(Problem::Backend(RommBrowseError::PaginationInconsistency));
    let view = text(&mut state);
    assert!(view.contains("library may have changed"));
    assert!(view.contains("Start at first page"));
}
#[test]
fn custom_platform_rows_use_the_friendly_platform_name() {
    let mut state = connected();
    let game = &mut Arc::make_mut(state.page.as_mut().unwrap()).games[0];
    game.identity.provider_platform_id = Some("8".into());
    game.identity.platform_candidate = None;
    game.identity.provider_platform_name = Some("custom".into());
    assert!(text(&mut state).contains("Custom console"));
}
#[test]
fn empty_states_distinguish_server_platform_and_search() {
    let mut state = connected();
    state.page = Some(Arc::new(RommBrowsePage {
        games: vec![],
        offset: 0,
        page_size: 50,
        total: Some(0),
        next_offset: None,
        previous_offset: None,
    }));
    assert!(text(&mut state).contains("no games to show for this connection"));
    state.platform = Some(8);
    assert!(text(&mut state).contains("No games are visible for this platform"));
    state.search = "Nothing".into();
    assert!(text(&mut state).contains("No games matched 'Nothing'"));
}
#[test]
fn read_only_copy_is_visible_and_no_mutating_controls_are_present() {
    let mut state = connected();
    let text = text(&mut state);
    assert!(text.contains("READ ONLY"));
    assert!(text.contains("will not upload, delete or change"));
    for control in [
        "Download ROM",
        "Delete game",
        "Upload ROM",
        "Rename remote game",
        "Edit RomM metadata",
        "Sync both ways",
    ] {
        assert!(!text.lines().any(|line| line == control));
    }
}
#[test]
fn back_from_details_preserves_search_filter_and_page() {
    let mut state = connected();
    state.search = "Mario".into();
    state.platform = Some(7);
    state.offset = 50;
    state.selected = Some(1);
    let page = state.page.clone().unwrap();
    state.back();
    assert!(state.open);
    assert!(state.selected.is_none());
    assert_eq!(state.search, "Mario");
    assert_eq!(state.platform, Some(7));
    assert_eq!(state.offset, 50);
    assert!(Arc::ptr_eq(&page, state.page.as_ref().unwrap()));
}
#[test]
fn alt_back_is_consumed_before_underlying_page_navigation() {
    let ctx = egui::Context::default();
    let mut state = connected();
    state.selected = Some(1);
    state.visible_last_frame = true;
    let mut activity = Activity::default();
    let _ = ctx.run(
        egui::RawInput {
            modifiers: egui::Modifiers::ALT,
            events: vec![egui::Event::Key {
                key: egui::Key::ArrowLeft,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::ALT,
            }],
            ..Default::default()
        },
        |ctx| {
            state.poll(ctx, &mut activity);
            assert!(!ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)));
        },
    );
    assert!(state.open);
    assert!(state.selected.is_none());
    assert!(state.page.is_some());
}
#[test]
fn reflected_credentials_are_hidden_without_changing_identity_evidence() {
    let mut state = connected();
    let title = format!("Fixture {TOKEN}");
    Arc::make_mut(state.page.as_mut().unwrap()).games[0]
        .identity
        .title = Some(title.clone());
    state.platforms[0].name = Some(format!("Console {TOKEN}"));
    let view = text(&mut state);
    assert!(view.contains("[hidden]"));
    assert!(!view.contains(TOKEN));
    assert_eq!(
        state.page.as_ref().unwrap().games[0]
            .identity
            .title
            .as_deref(),
        Some(title.as_str())
    );
    state.selected = Some(1);
    state.absorb(
        &egui::Context::default(),
        0,
        Ok(call(&Fixture::default(), Request::Detail(1))),
    );
    Arc::make_mut(state.detail.as_mut().unwrap())
        .game
        .identity
        .synopsis = Some(format!(
        "Bearer {TOKEN} http://fixture-user:fixture-password@localhost/path"
    ));
    let view = text(&mut state);
    assert!(!view.contains(TOKEN));
    assert!(!view.contains("fixture-password"));
    assert!(view.contains("external evidence"));
    assert!(!state.display(&format!("Bearer {TOKEN}")).contains(TOKEN));
    assert_eq!(
        state.display("http://user:password@host/path https://other:secret@elsewhere/"),
        "http://[hidden]@host/path https://[hidden]@elsewhere/"
    );
}
#[test]
fn unavailable_details_and_filters_have_explicit_explanations() {
    let mut state = connected();
    let capabilities = &mut state.info.as_mut().unwrap().capabilities;
    capabilities.game_detail = false;
    capabilities.platform_filter = false;
    capabilities.text_search = false;
    let view = text(&mut state);
    assert!(view.contains("doesn't provide game details"));
    assert!(view.contains("Platform filtering is unavailable"));
    assert!(view.contains("Search is unavailable"));
}
#[test]
fn settings_return_rechecks_configuration_and_rejects_old_results() {
    let mut state = connected();
    let old_generation = state.generation;
    state.settings_opened();
    assert!(state.open);
    assert!(!state.settings_loaded);
    assert!(state.info.is_none());
    assert!(state.token.is_none());
    assert!(!state.absorb(
        &egui::Context::default(),
        old_generation,
        Ok(call(&Fixture::default(), Request::Connect))
    ));
    state.open(vec![]);
    assert!(matches!(state.browse.pending, Some(Request::Settings)));
}
#[test]
fn escape_goes_back_from_details_then_closes_the_browser() {
    let ctx = egui::Context::default();
    let mut state = connected();
    state.selected = Some(1);
    for expected_open in [true, false] {
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            state.show(ctx);
        });
        assert_eq!(state.open, expected_open);
    }
}
#[test]
fn checking_settings_and_browsing_do_not_write_catalogues_or_configurations() {
    let root = tempfile::tempdir().unwrap();
    settings(root.path(), "http://127.0.0.1:45678");
    let path = SettingsLocation::new(root.path(), IdentityProvider::Romm).config_path();
    let before = std::fs::read(&path).unwrap();
    for request in [Request::Settings, Request::Connect, Request::Detail(1)] {
        worker::run_with(
            root.path(),
            request,
            &[],
            &StaticResolver::new(),
            &Fixture::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
    }
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(root.path().join("synthetic-token")).unwrap(),
        TOKEN
    );
    assert!(!root.path().join("romm/cache.json").exists());
    assert!(!root.path().join("archivefs.db").exists());
}
#[test]
fn existing_gui_worker_keeps_slow_network_off_the_ui_thread() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let root = tempfile::tempdir().unwrap();
    settings(root.path(), &endpoint);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let body = b"{}";
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(body);
    });
    let ctx = egui::Context::default();
    let mut state = State {
        test_root: Some(root.path().into()),
        configured: true,
        settings_loaded: true,
        ..Default::default()
    };
    state.queue(Request::Connect);
    let mut activity = Activity::default();
    let start = Instant::now();
    state.poll(&ctx, &mut activity);
    assert!(start.elapsed() < Duration::from_millis(200));
    assert!(state.browse.running.is_some());
    for _ in 0..150 {
        state.poll(&ctx, &mut activity);
        if !state.browse.active() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    server.join().unwrap();
    assert!(!state.browse.active());
    assert!(state.problem.is_some());
    assert!(activity.jobs.values().all(|j| !j.active()));
}
