use super::super::client::{MAX_RESPONSE_BYTES, RommHttpResponse, UreqTransport};
use super::super::config::{RommSourceConfig, RommToken};
use super::*;
use crate::identity_source::model::ExternalVerification;
use crate::identity_source::net_policy::SystemResolver;
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const TOKEN: &str = "rk_browser_fixture_secret_not_for_urls";

struct Request {
    method: String,
    target: String,
    authorization: Option<String>,
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    delay: Duration,
    body_delay: Duration,
    location: Option<String>,
}

impl Reply {
    fn json(value: Value) -> Self {
        Self::bytes(200, serde_json::to_vec(&value).unwrap())
    }
    fn bytes(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            location: None,
        }
    }
}

/// An actual deterministic loopback HTTP server exercising the production client.
struct MockServer {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl MockServer {
    fn new(reply: impl Fn(&Request) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let request = read_request(&mut stream);
                        let response = reply(&request);
                        seen.lock().unwrap().push(request);
                        thread::sleep(response.delay);
                        let location = response
                            .location
                            .map(|s| format!("Location: {s}\r\n"))
                            .unwrap_or_default();
                        let header = format!(
                            "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{location}\r\n",
                            response.status,
                            response.body.len()
                        );
                        let _ = stream.write_all(header.as_bytes());
                        thread::sleep(response.body_delay);
                        let _ = stream.write_all(&response.body);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => panic!("mock accept: {e}"),
                }
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(handle),
        }
    }

    fn source(&self) -> ValidatedRommSource {
        ValidatedRommSource::validate(
            &RommSourceConfig {
                url: self.url.clone(),
                enabled: true,
                ..Default::default()
            },
            &RommToken::parse(TOKEN).unwrap(),
            &[],
            &SystemResolver,
        )
        .unwrap()
    }

    fn targets(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.target.clone())
            .collect()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    while bytes.len() < 16 * 1024 && !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 1);
        bytes.push(byte[0]);
    }
    let text = String::from_utf8(bytes).unwrap();
    let mut lines = text.lines();
    let mut first = lines.next().unwrap().split_whitespace();
    Request {
        method: first.next().unwrap().to_owned(),
        target: first.next().unwrap().to_owned(),
        authorization: lines.find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim().to_owned())
        }),
    }
}

fn api() -> Value {
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
    .into_iter()
    .map(|name| json!({"name": name, "in": "query"}))
    .collect::<Vec<_>>();
    json!({"openapi":"3.1.0", "info":{"version":"5.3.1"}, "paths": {
        "/api/platforms":{"get":{"security":[{"OAuth2PasswordBearer":["platforms.read"]}]}},
        "/api/roms":{"get":{"parameters":params,"security":[{"OAuth2PasswordBearer":["roms.read"]}]}},
        "/api/roms/{id}":{"get":{}},
        "/api/client-tokens":{"post":{}},
        "/api/roms/{id}/content":{"get":{}},
        "/api/scan":{"post":{}}
    }})
}

fn game(id: u64) -> Value {
    json!({"id":id,"platform_id":7,"platform_slug":"gb","platform_fs_slug":"gb",
        "fs_name":format!("Game{id}.gb"),"fs_path":"roms/gb","fs_size_bytes":32768,
        "name":format!("Game {id}"),"regions":["USA"],"revision":"Rev 1",
        "crc_hash":"DEADBEEF","md5_hash":"00112233445566778899aabbccddeeff",
        "sha1_hash":"0123456789abcdef0123456789abcdef01234567","igdb_id":101,
        "path_cover_small":"/assets/romm/resources/cover_small.png",
        "path_cover_large":"/assets/romm/resources/cover_large.png",
        "merged_screenshots":["/assets/romm/resources/screen.png"],
        "created_at":"2026-10-01T00:00:00+00:00","updated_at":"2026-10-02T00:00:00+00:00",
        "files":[],"sibling_roms":[],"metadatum":{"genres":["Puzzle"]}})
}

fn healthy(request: &Request) -> Reply {
    let url = url::Url::parse(&format!("http://fixture{}", request.target)).unwrap();
    match url.path() {
        "/api/heartbeat" => Reply::json(json!({"SYSTEM":{"VERSION":"5.3.1"}})),
        "/openapi.json" => Reply::json(api()),
        "/api/platforms" => Reply::json(json!([
            {"id":7,"slug":"gb","fs_slug":"gb","name":"Game Boy","rom_count":5,"igdb_id":33},
            {"id":8,"slug":"custom-console","name":"Custom","rom_count":0}
        ])),
        "/api/roms" => {
            if request.authorization.as_deref() != Some(format!("Bearer {TOKEN}").as_str()) {
                return Reply::bytes(401, Vec::new());
            }
            let query: BTreeMap<String, String> = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let offset: u32 = query["offset"].parse().unwrap();
            let limit: u32 = query["limit"].parse().unwrap();
            let mut games = (1..=5).map(game).collect::<Vec<_>>();
            if let Some(text) = query.get("search_term") {
                games.retain(|g| g["name"].as_str().unwrap().contains(text));
            }
            if let Some(platform) = query.get("platform_ids") {
                games.retain(|g| g["platform_id"].as_u64().unwrap().to_string() == *platform);
            }
            let total = games.len();
            let items = games
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect::<Vec<_>>();
            Reply::json(
                json!({"items":items,"total":total,"offset":offset,"limit":limit,"rom_id_index":[],"char_index":{},"filter_values":{}}),
            )
        }
        "/api/roms/1" => {
            let mut value = game(1);
            value["summary"] = json!("Provider synopsis");
            value["files"] = json!([{"id":19,"rom_id":1,"file_name":"Disc 1.bin","full_path":"roms/gb/Disc 1.bin","file_size_bytes":700000000,"crc_hash":"DEADBEEF"}]);
            value["sibling_roms"] = json!([{"id":2}]);
            Reply::json(value)
        }
        _ => Reply::bytes(404, Vec::new()),
    }
}

fn with_browser(run: impl FnOnce(&MockServer, &RommBrowser<'_, UreqTransport>)) {
    let server = MockServer::new(healthy);
    let source = server.source();
    let transport = UreqTransport::new();
    let mut browser = RommBrowser::new(&source, &transport, 42);
    assert_eq!(browser.discover(None).status, RommServerStatus::Supported);
    run(&server, &browser);
}

#[test]
fn discovery_is_explicit_bounded_and_authenticates_one_small_page() {
    let server = MockServer::new(healthy);
    let source = server.source();
    let transport = UreqTransport::new();
    let mut browser = RommBrowser::new(&source, &transport, 42);
    assert!(server.targets().is_empty());
    assert_eq!(
        browser.platforms(None),
        Err(RommBrowseError::DiscoveryRequired)
    );
    let info = browser.discover(None);
    assert_eq!(info.status, RommServerStatus::Supported);
    assert_eq!(info.version.as_deref(), Some("5.3.1"));
    assert_eq!(info.declared_read_scopes, ["platforms.read", "roms.read"]);
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[..2].iter().all(|r| r.authorization.is_none()));
    assert_eq!(
        requests[2].authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(requests[2].target.contains("limit=1&offset=0"));
    for field in [
        "with_files",
        "with_char_index",
        "with_filter_values",
        "with_rom_id_index",
        "group_by_meta_id",
    ] {
        assert!(requests[2].target.contains(&format!("{field}=false")));
    }
}

#[test]
fn unsupported_old_and_future_major_versions_do_not_send_credentials() {
    for version in ["3.10.0", "6.0.0"] {
        let server = MockServer::new(move |_| Reply::json(json!({"SYSTEM":{"VERSION":version}})));
        let source = server.source();
        let transport = UreqTransport::new();
        let mut browser = RommBrowser::new(&source, &transport, 42);
        assert_eq!(
            browser.discover(None).status,
            RommServerStatus::UnsupportedVersion
        );
        assert_eq!(
            browser.platforms(None),
            Err(RommBrowseError::UnsupportedVersion)
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        assert!(server.requests.lock().unwrap()[0].authorization.is_none());
    }
}

#[test]
fn discovery_reports_auth_required_separately_from_auth_failed() {
    for (path, status, expected) in [
        ("/api/heartbeat", 401, RommServerStatus::AuthRequired),
        ("/api/roms", 403, RommServerStatus::AuthFailed),
    ] {
        let server = MockServer::new(move |request| {
            if request.target.starts_with(path) {
                Reply::bytes(status, TOKEN.as_bytes().to_vec())
            } else {
                healthy(request)
            }
        });
        let source = server.source();
        let transport = UreqTransport::new();
        let mut browser = RommBrowser::new(&source, &transport, 42);
        let info = browser.discover(None);
        assert_eq!(info.status, expected);
        assert!(!format!("{browser:?} {info:?}").contains(TOKEN));
    }
}

#[test]
fn platforms_reuse_normalisation_and_preserve_unknowns() {
    with_browser(|_, browser| {
        let platforms = browser.platforms(None).unwrap();
        assert_eq!(platforms[0].id, 7);
        assert_eq!(platforms[0].game_count, Some(5));
        assert_eq!(
            platforms[0].mapping,
            RommPlatformMapping::KnownAlias {
                canonical: "Game Boy".into()
            }
        );
        assert_eq!(platforms[0].system_identifiers["igdb_id"], "33");
        assert_eq!(platforms[1].mapping, RommPlatformMapping::Unknown);
        assert_eq!(platforms[0].provenance.observed_at_unix_seconds, 42);
    });
}

#[test]
fn exact_and_conflicting_platform_fields_are_explicit() {
    assert_eq!(
        platform_mapping(Some("Game Boy"), "Game Boy", None),
        RommPlatformMapping::Exact {
            canonical: "Game Boy".into()
        }
    );
    assert_eq!(
        platform_mapping(Some("NES"), "nes", Some("snes")),
        RommPlatformMapping::Ambiguous {
            candidates: vec!["NES".into(), "SNES".into()]
        }
    );
    assert_eq!(
        platform_mapping(None, "future-system", None),
        RommPlatformMapping::Unknown
    );
}

#[test]
fn pagination_exposes_total_next_previous_without_fetching_other_pages() {
    with_browser(|server, browser| {
        let first = browser.games(0, 2, &Default::default(), None).unwrap();
        assert_eq!(first.games.iter().map(|g| g.id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(
            (first.total, first.next_offset, first.previous_offset),
            (Some(5), Some(2), None)
        );
        assert_eq!(server.targets().len(), 4);
        let last = browser.games(4, 2, &Default::default(), None).unwrap();
        assert_eq!(
            (last.games.len(), last.next_offset, last.previous_offset),
            (1, None, Some(2))
        );
        assert!(!first.games[0].includes_file_detail);
    });
}

#[test]
fn search_and_platform_filter_are_server_side_and_query_encoded() {
    with_browser(|server, browser| {
        let result = browser
            .games(
                0,
                2,
                &RommBrowseFilter {
                    text: Some("Game 3".into()),
                    platform_id: Some(7),
                },
                None,
            )
            .unwrap();
        assert_eq!(result.games[0].id, 3);
        assert_eq!(result.total, Some(1));
        assert!(
            server
                .targets()
                .last()
                .unwrap()
                .contains("search_term=Game+3&platform_ids=7")
        );
        let text = "&platform_ids=8/#?";
        browser
            .games(
                0,
                2,
                &RommBrowseFilter {
                    text: Some(text.into()),
                    platform_id: None,
                },
                None,
            )
            .unwrap();
        let target = server.targets().pop().unwrap();
        let url = url::Url::parse(&format!("http://fixture{target}")).unwrap();
        assert_eq!(
            url.query_pairs()
                .filter(|(k, _)| k == "search_term")
                .next()
                .unwrap()
                .1,
            text
        );
        assert!(!url.query_pairs().any(|(k, _)| k == "platform_ids"));
    });
}

#[test]
fn game_summary_uses_existing_hashes_provider_ids_and_artwork_references() {
    with_browser(|server, browser| {
        let page = browser.games(0, 1, &Default::default(), None).unwrap();
        let game = &page.games[0];
        assert_eq!(game.identity.title.as_deref(), Some("Game 1"));
        assert_eq!(game.identity.regions, ["USA"]);
        assert_eq!(game.identity.revision.as_deref(), Some("Rev 1"));
        assert_eq!(game.identity.file_size_bytes, Some(32768));
        assert_eq!(game.identity.hashes.len(), 3);
        assert_eq!(
            game.identity.hash(HashAlgorithm::Crc32).unwrap().value,
            "deadbeef"
        );
        assert!(
            game.identity
                .metadata_provider_ids
                .iter()
                .any(|id| id.provider == "igdb" && id.id == "101")
        );
        assert_eq!(
            game.artwork
                .cover
                .as_ref()
                .unwrap()
                .small_reference
                .as_deref(),
            Some("/assets/romm/resources/cover_small.png")
        );
        assert_eq!(game.artwork.screenshots.len(), 1);
        assert_eq!(game.identity.verification, ExternalVerification::Unmatched);
        assert!(game.identity.conflicts.is_empty());
        assert_eq!(game.provenance.provider, IdentityProvider::Romm);
        assert_eq!(game.provenance.server_id, server.url);
        assert!(server.targets().iter().all(|p| !p.starts_with("/assets")));
    });
}

#[test]
fn detail_retains_related_files_siblings_metadata_and_provenance() {
    with_browser(|server, browser| {
        let detail = browser.game_detail(1, None).unwrap();
        assert!(detail.game.includes_file_detail);
        assert_eq!(detail.files[0].id, 19);
        assert_eq!(detail.files[0].size_bytes, Some(700000000));
        assert_eq!(detail.files[0].hashes[0].value, "deadbeef");
        assert_eq!(detail.game.identity.related_files, ["roms/gb/Disc 1.bin"]);
        assert_eq!(detail.game.identity.sibling_game_ids, ["2"]);
        assert_eq!(
            detail.game.identity.synopsis.as_deref(),
            Some("Provider synopsis")
        );
        assert_eq!(detail.game.provenance.endpoint, "/api/roms/1");
        assert_eq!(server.targets().last().unwrap(), "/api/roms/1");
    });
}

#[test]
fn unsupported_filters_are_explicit_and_do_not_request_a_catalogue() {
    let server = MockServer::new(|r| {
        if r.target == "/openapi.json" {
            let mut api = api();
            api["paths"]["/api/roms"]["get"]["parameters"]
                .as_array_mut()
                .unwrap()
                .retain(|p| {
                    !["search_term", "platform_ids"].contains(&p["name"].as_str().unwrap())
                });
            Reply::json(api)
        } else {
            healthy(r)
        }
    });
    let source = server.source();
    let transport = UreqTransport::new();
    let mut browser = RommBrowser::new(&source, &transport, 42);
    assert_eq!(
        browser.discover(None).status,
        RommServerStatus::PartiallySupported
    );
    let before = server.targets().len();
    for filter in [
        RommBrowseFilter {
            text: Some("Game".into()),
            platform_id: None,
        },
        RommBrowseFilter {
            text: None,
            platform_id: Some(7),
        },
    ] {
        assert_eq!(
            browser.games(0, 2, &filter, None),
            Err(RommBrowseError::UnsupportedFilter)
        );
    }
    assert_eq!(server.targets().len(), before);
}

#[test]
fn optional_fields_can_be_absent_and_invalid_hashes_remain_rejected_evidence() {
    with_browser(|_, browser| {
        let mut value = json!({"id":1,"crc_hash":"not-a-checksum"});
        let game = browser.project_game(&value, "/api/roms", false).unwrap();
        assert!(game.identity.hashes.is_empty());
        assert_eq!(game.normalisation.rejected_hashes.len(), 1);
        assert_eq!(game.platform_mapping, RommPlatformMapping::Unknown);
        value["crc_hash"] = json!(42);
        assert_eq!(
            browser.project_game(&value, "/api/roms", false),
            Err(RommBrowseError::SchemaIncompatibility)
        );
    });
}

#[test]
fn existing_normalised_identity_is_preserved_byte_for_byte() {
    with_browser(|server, browser| {
        let value = game(1);
        let mut report = NormalisationReport::default();
        let expected = normalise_rom(
            &value,
            &server.url,
            browser.source.mappings(),
            42,
            &mut report,
        )
        .unwrap();
        let actual = browser.project_game(&value, "/api/roms", false).unwrap();
        assert_eq!(
            serde_json::to_vec(&actual.identity).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        assert_eq!(actual.normalisation, report);
    });
}

#[test]
fn malformed_json_is_a_safe_typed_error() {
    let server = MockServer::new(|r| {
        if r.target == "/openapi.json" {
            Reply::bytes(200, format!("{{ {TOKEN}").into_bytes())
        } else {
            healthy(r)
        }
    });
    let source = server.source();
    let transport = UreqTransport::new();
    let mut browser = RommBrowser::new(&source, &transport, 42);
    let info = browser.discover(None);
    assert_eq!(info.error, Some(RommBrowseError::MalformedResponse));
    assert!(!format!("{info:?} {browser:?}").contains(TOKEN));
}

#[test]
fn oversized_http_body_is_refused_while_reading() {
    let server = MockServer::new(|r| {
        if r.target == "/openapi.json" {
            Reply::bytes(200, vec![b' '; MAX_RESPONSE_BYTES + 1])
        } else {
            healthy(r)
        }
    });
    let source = server.source();
    let transport = UreqTransport::new();
    let mut browser = RommBrowser::new(&source, &transport, 42);
    assert_eq!(
        browser.discover(None).error,
        Some(RommBrowseError::ResponseTooLarge {
            limit: MAX_RESPONSE_BYTES
        })
    );
}

#[test]
fn http_errors_are_typed_and_never_echo_provider_bodies() {
    for status in [400, 404, 429, 500, 503] {
        let server = MockServer::new(move |r| {
            if r.target == "/api/platforms" {
                Reply::bytes(status, TOKEN.as_bytes().to_vec())
            } else {
                healthy(r)
            }
        });
        let source = server.source();
        let transport = UreqTransport::new();
        let mut browser = RommBrowser::new(&source, &transport, 42);
        assert_eq!(browser.discover(None).status, RommServerStatus::Supported);
        let error = browser.platforms(None).unwrap_err();
        let expected = match status {
            429 => RommBrowseError::RateLimited,
            500..=599 => RommBrowseError::HttpServer { status },
            _ => RommBrowseError::HttpClient { status },
        };
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains(TOKEN));
    }
}

struct ShortTimeout(UreqTransport);
impl RommTransport for ShortTimeout {
    fn get(
        &self,
        url: &str,
        auth: Option<&str>,
        max: usize,
        timeout: Duration,
    ) -> Result<RommHttpResponse, RommRequestError> {
        assert_eq!(timeout, REQUEST_TIMEOUT);
        self.0.get(url, auth, max, Duration::from_millis(40))
    }
}

#[test]
fn timeout_is_bounded_and_distinct_from_unreachable() {
    let server = MockServer::new(|_| {
        let mut reply = Reply::json(json!({}));
        reply.delay = Duration::from_millis(150);
        reply
    });
    let source = server.source();
    let transport = ShortTimeout(UreqTransport::new());
    let mut browser = RommBrowser::new(&source, &transport, 42);
    let info = browser.discover(None);
    assert_eq!(info.status, RommServerStatus::Unreachable);
    assert_eq!(info.error, Some(RommBrowseError::Timeout));
}

#[test]
fn timeout_while_reading_a_response_body_remains_typed() {
    let server = MockServer::new(|_| {
        let mut reply = Reply::json(json!({"SYSTEM":{"VERSION":"5.3.1"}}));
        reply.body_delay = Duration::from_millis(150);
        reply
    });
    let source = server.source();
    let transport = ShortTimeout(UreqTransport::new());
    let mut browser = RommBrowser::new(&source, &transport, 42);
    assert_eq!(browser.discover(None).error, Some(RommBrowseError::Timeout));
}

#[test]
fn unexpected_transport_errors_do_not_format_arbitrary_credentials() {
    let error = ureq::Error::Other(Box::new(std::io::Error::other(format!(
        "https://user:{TOKEN}@server/"
    ))));
    let message = super::super::client::classify_transport_error(&error);
    assert_eq!(message, "an unexpected transport error occurred");
    assert!(!message.contains(TOKEN));
}

#[test]
fn tls_transport_failure_is_typed_and_secret_safe() {
    let error = RommBrowseError::from(RommRequestError::Transport {
        detail: format!("TLS handshake failed at https://user:{TOKEN}@server/"),
    });
    assert_eq!(error, RommBrowseError::Tls);
    assert!(!format!("{error:?} {error}").contains(TOKEN));
}

#[test]
fn cancelled_browsing_emits_no_request() {
    with_browser(|server, browser| {
        let count = server.targets().len();
        assert_eq!(
            browser.games(0, 1, &Default::default(), Some(&AtomicBool::new(true))),
            Err(RommBrowseError::Cancelled)
        );
        assert_eq!(server.targets().len(), count);
    });
}

#[test]
fn all_browser_requests_are_get_and_no_download_or_write_endpoint_is_exposed() {
    with_browser(|server, browser| {
        browser.platforms(None).unwrap();
        browser.games(0, 2, &Default::default(), None).unwrap();
        browser.game_detail(1, None).unwrap();
        let requests = server.requests.lock().unwrap();
        assert!(requests.iter().all(|r| r.method == "GET"));
        assert!(requests.iter().all(|r| !r.target.contains("content")
            && !r.target.contains("scan")
            && !r.target.contains("client-tokens")));
        assert!(requests.iter().all(|r| !r.target.contains(TOKEN)));
    });
}

#[test]
fn credentials_never_appear_in_browser_debug_or_configuration_serialisation() {
    with_browser(|_, browser| {
        assert!(!format!("{browser:?} {:?}", browser.source.token()).contains(TOKEN));
        assert!(
            !serde_json::to_string(browser.source.token())
                .unwrap()
                .contains(TOKEN)
        );
    });
}

#[test]
fn invalid_page_requests_are_refused_before_http() {
    with_browser(|server, browser| {
        let count = server.targets().len();
        for limit in [0, MAX_PAGE_SIZE + 1, u32::MAX] {
            assert_eq!(
                browser.games(0, limit, &Default::default(), None),
                Err(RommBrowseError::InvalidRequest)
            );
        }
        for text in ["x".repeat(MAX_SEARCH_BYTES + 1), "hello\nworld".into()] {
            assert_eq!(
                browser.games(
                    0,
                    1,
                    &RommBrowseFilter {
                        text: Some(text),
                        platform_id: None
                    },
                    None
                ),
                Err(RommBrowseError::InvalidRequest)
            );
        }
        assert_eq!(server.targets().len(), count);
    });
}

#[test]
fn inconsistent_pages_duplicate_ids_and_ignored_platform_filters_are_refused() {
    with_browser(|_, browser| {
        for value in [
            json!({"items":[game(1)],"total":5,"limit":2,"offset":0}),
            json!({"items":[game(1),game(1)],"total":2,"limit":2,"offset":0}),
            json!({"items":[game(1),game(2)],"total":1,"limit":2,"offset":0}),
            json!({"items":[],"total":0,"limit":2,"offset":1}),
            json!({"items":[],"total":0,"limit":"2","offset":0}),
        ] {
            assert_eq!(
                browser.project_page(&value, 2, 0, &Default::default()),
                Err(RommBrowseError::PaginationInconsistency)
            );
        }
        assert_eq!(
            browser.project_page(
                &json!({"items":[game(1)],"total":1}),
                1,
                0,
                &RommBrowseFilter {
                    text: None,
                    platform_id: Some(8)
                }
            ),
            Err(RommBrowseError::PaginationInconsistency)
        );
    });
}

#[test]
fn missing_total_is_explicit_and_progress_overflow_is_refused() {
    with_browser(|_, browser| {
        let page = browser
            .project_page(
                &json!({"items":[game(1)],"total":null}),
                1,
                0,
                &Default::default(),
            )
            .unwrap();
        assert_eq!((page.total, page.next_offset), (None, Some(1)));
        assert_eq!(
            browser.project_page(
                &json!({"items":[game(1)]}),
                1,
                u32::MAX,
                &Default::default()
            ),
            Err(RommBrowseError::PaginationInconsistency)
        );
    });
}

#[test]
fn excessive_strings_lists_nesting_and_numeric_overflow_fail_cleanly() {
    with_browser(|_, browser| {
        for (field, value) in [
            ("name", json!("x".repeat(MAX_BROWSE_STRING_BYTES + 1))),
            ("files", json!(vec![json!({}); MAX_RELATED_FILES + 1])),
            ("regions", json!([3])),
            ("id", json!(0)),
            ("id", json!(u64::MAX)),
            ("fs_size_bytes", json!(-1)),
        ] {
            let mut raw = game(1);
            raw[field] = value;
            assert!(browser.project_game(&raw, "/api/roms", false).is_err());
        }
        let mut nested = json!(null);
        for _ in 0..MAX_BROWSE_DEPTH + 1 {
            nested = json!({"child":nested});
        }
        let mut raw = game(1);
        raw["metadatum"] = nested;
        assert_eq!(
            browser.project_game(&raw, "/api/roms", false),
            Err(RommBrowseError::LimitExceeded)
        );
    });
}

#[test]
fn local_openapi_parameter_references_are_supported_but_cycles_are_bounded() {
    let mut document = api();
    document["components"] = json!({"parameters":{"limit":{"name":"limit","in":"query"}}});
    document["paths"]["/api/roms"]["get"]["parameters"][0] =
        json!({"$ref":"#/components/parameters/limit"});
    assert!(capabilities(&document).unwrap().games);
    document["components"]["parameters"]["limit"] = json!({"$ref":"#/components/parameters/limit"});
    assert_eq!(capabilities(&document), Err(RommBrowseError::LimitExceeded));
    document["components"]["parameters"]["limit"] = json!({"$ref":"https://untrusted/schema"});
    assert_eq!(
        capabilities(&document),
        Err(RommBrowseError::SchemaIncompatibility)
    );
}

#[test]
fn deterministic_projection_performs_no_cache_or_catalogue_writes() {
    with_browser(|_, browser| {
        let before = browser.source.server_id().to_owned();
        let one = browser.games(0, 2, &Default::default(), None).unwrap();
        let two = browser.games(0, 2, &Default::default(), None).unwrap();
        assert_eq!(
            serde_json::to_vec(&one).unwrap(),
            serde_json::to_vec(&two).unwrap()
        );
        assert_eq!(browser.source.server_id(), before);
        assert!(
            one.games
                .iter()
                .all(|g| g.identity.verification == ExternalVerification::Unmatched)
        );
        // Browser construction accepts no cache/database/writer; projections have no persistence path.
    });
}

#[test]
fn environment_proxy_is_ignored_by_the_real_browser_transport() {
    if std::env::var_os("EMUWIZ_ROMM_BROWSER_PROXY_CHILD").is_some() {
        with_browser(|_, browser| {
            browser.platforms(None).unwrap();
            browser.games(0, 1, &Default::default(), None).unwrap();
        });
        return;
    }
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let name = "identity_source::romm::browser::tests::environment_proxy_is_ignored_by_the_real_browser_transport";
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", name, "--nocapture"])
        .env("EMUWIZ_ROMM_BROWSER_PROXY_CHILD", "1");
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        child.env(key, &proxy_url);
    }
    for key in ["NO_PROXY", "no_proxy"] {
        child.env_remove(key);
    }
    let result = child.output().unwrap();
    assert!(
        result.status.success(),
        "proxy child failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(matches!(proxy.accept(),Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}

#[test]
#[ignore = "requires an explicitly selected configured RomM; GET requests only"]
fn configured_real_romm_read_only_smoke() {
    use crate::identity_source::settings::{SettingsLocation, load_token_file};
    let root = std::env::var_os("EMUWIZ_ROMM_BROWSER_REAL_IDENTITY_ROOT")
        .expect("select the existing identity configuration directory explicitly");
    let location = SettingsLocation::new(std::path::Path::new(&root), IdentityProvider::Romm);
    let settings = location
        .load()
        .expect("load existing non-secret configuration");
    assert!(settings.source.enabled);
    let token = load_token_file(settings.source.token_path.as_deref())
        .expect("load the existing credential through its regular-file/permission policy");
    let source = ValidatedRommSource::validate(&settings.source, &token, &[], &SystemResolver)
        .expect("the configured endpoint and mappings must pass current policy");
    let transport = UreqTransport::new();
    let mut browser = RommBrowser::new(&source, &transport, 0);
    let info = browser.discover(None);
    assert!(
        info.error.is_none(),
        "safe discovery error: {:?}",
        info.error
    );
    let platforms = browser.platforms(None).expect("read platform summaries");
    let page = browser
        .games(0, 3, &Default::default(), None)
        .expect("read one small page");
    let detail = page.games.first().map(|game| {
        browser
            .game_detail(game.id, None)
            .expect("read one selected detail")
    });
    println!(
        "real RomM: version={:?}, status={:?}, platforms={}, page_items={}, total={:?}, detail_files={:?}",
        info.version,
        info.status,
        platforms.len(),
        page.games.len(),
        page.total,
        detail.as_ref().map(|d| d.files.len())
    );
}
