//! Provider tests over synthetic GitHub responses. Nothing here touches the
//! network, writes a file or downloads an asset.

use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;

type Handler =
    Box<dyn Fn(&GithubRequest) -> Result<GithubResponse, GithubProviderError> + Send + Sync>;

struct Fake {
    requests: Mutex<Vec<GithubRequest>>,
    handler: Handler,
}

impl Fake {
    fn new(
        handler: impl Fn(&GithubRequest) -> Result<GithubResponse, GithubProviderError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            handler: Box::new(handler),
        }
    }
}

impl GithubTransport for &Fake {
    fn get(
        &self,
        request: &GithubRequest,
        _max: usize,
    ) -> Result<GithubResponse, GithubProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        (self.handler)(request)
    }
}

fn ok(body: Value) -> Result<GithubResponse, GithubProviderError> {
    Ok(GithubResponse {
        status: 200,
        validators: GithubValidators {
            etag: Some("\"abc\"".into()),
            last_modified: Some("Wed, 01 Jan 2025 00:00:00 GMT".into()),
        },
        rate_limit_remaining: Some(59),
        rate_limit_reset_unix: Some(1_900_000_000),
        body: serde_json::to_vec(&body).unwrap(),
    })
}

fn repository(spdx: Option<&str>) -> Value {
    json!({
        "id": 4242,
        "full_name": "owner/demo",
        "html_url": "https://github.com/owner/demo",
        "license": spdx.map(|id| json!({ "spdx_id": id })).unwrap_or(Value::Null),
    })
}

fn asset(id: u64, name: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "size": 1234,
        "content_type": "application/octet-stream",
        "state": "uploaded",
        "created_at": "2024-03-01T10:00:00Z",
        "updated_at": "2024-03-02T10:00:00Z",
        "browser_download_url": format!("https://github.com/owner/demo/releases/download/v1/{name}"),
    })
}

fn release(id: u64, tag: &str, assets: Vec<Value>) -> Value {
    json!({
        "id": id,
        "tag_name": tag,
        "name": format!("Release {tag}"),
        "draft": false,
        "prerelease": false,
        "created_at": "2024-03-01T09:00:00Z",
        "published_at": "2024-03-01T10:00:00Z",
        "html_url": format!("https://github.com/owner/demo/releases/tag/{tag}"),
        "body": "Notes",
        "assets": assets,
    })
}

/// Serves `releases` on page 1 and an empty list afterwards.
fn serving(spdx: Option<&'static str>, releases: Vec<Value>) -> Fake {
    Fake::new(move |request| {
        if request.url.contains("/releases?") {
            if request.url.ends_with("page=1") {
                ok(Value::Array(releases.clone()))
            } else {
                ok(json!([]))
            }
        } else {
            ok(repository(spdx))
        }
    })
}

fn provider(fake: &Fake, policy: GithubReleasePolicy) -> GithubReleasesProvider<&Fake> {
    GithubReleasesProvider::with_transport(fake, policy)
}

fn repo() -> GithubRepositoryRef {
    GithubRepositoryRef::parse("owner/demo").unwrap()
}

fn enumeration(
    fake: &Fake,
    policy: GithubReleasePolicy,
) -> Result<GithubEnumeration, GithubProviderError> {
    match provider(fake, policy).enumerate(&repo(), None)? {
        GithubEnumerationOutcome::Releases(enumeration) => Ok(*enumeration),
        GithubEnumerationOutcome::NotModified => panic!("unexpected 304"),
    }
}

// ----------------------------------------------------------------- identity

#[test]
fn release_asset_and_repository_identity_use_stable_ids_not_names() {
    let first = serving(
        None,
        vec![release(900, "v1.0", vec![asset(77, "demo.gba")])],
    );
    let second = serving(
        None,
        vec![release(
            900,
            "v1.0-renamed",
            vec![asset(77, "demo-renamed.gba")],
        )],
    );
    let before = enumeration(&first, GithubReleasePolicy::default()).unwrap();
    let after = enumeration(&second, GithubReleasePolicy::default()).unwrap();
    for listing in [&before.listings[0], &after.listings[0]] {
        assert_eq!(listing.project.provider.as_str(), "github");
        assert_eq!(
            listing.project.provider_id, "4242",
            "repository id, not the name"
        );
        assert_eq!(
            listing
                .release
                .as_ref()
                .unwrap()
                .provider_release_id
                .as_deref(),
            Some("900")
        );
        assert_eq!(listing.artifacts[0].provider_asset_id, "77");
    }
    // A tag/name change is changed metadata under the same identity.
    assert_eq!(
        before.listings[0].release.as_ref().unwrap().tag.as_deref(),
        Some("v1.0")
    );
    assert_eq!(
        after.listings[0].release.as_ref().unwrap().tag.as_deref(),
        Some("v1.0-renamed")
    );
    assert_eq!(before.listings[0].project, after.listings[0].project);
}

#[test]
fn release_metadata_is_mapped_and_details_keep_what_the_model_cannot() {
    let mut value = release(900, "v1.0", vec![asset(77, "demo.gba")]);
    value["immutable"] = json!(true);
    let fake = serving(Some("MIT"), vec![value]);
    let result = enumeration(&fake, GithubReleasePolicy::default()).unwrap();
    let release = result.listings[0].release.as_ref().unwrap();
    assert_eq!(release.published_at_unix_secs, Some(1_709_287_200));
    assert!(!release.prerelease);
    let detail = &result.details[0];
    assert_eq!(detail.name.as_deref(), Some("Release v1.0"));
    assert!(!detail.draft);
    assert_eq!(detail.immutable, Some(true));
    assert_eq!(detail.created_at_unix, Some(1_709_283_600));
    assert_eq!(detail.body.as_deref(), Some("Notes"));
    assert_eq!(
        detail.assets[0].content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(detail.assets[0].created_at_unix, Some(1_709_287_200));
    assert_eq!(result.repository.repository_id, 4242);
    assert_eq!(result.validators.etag.as_deref(), Some("\"abc\""));
    assert_eq!(result.rate_limit_remaining, Some(59));
}

#[test]
fn an_absent_immutable_flag_stays_unknown() {
    let fake = serving(None, vec![release(1, "v1", vec![])]);
    assert_eq!(
        enumeration(&fake, GithubReleasePolicy::default())
            .unwrap()
            .details[0]
            .immutable,
        None
    );
}

#[test]
fn timestamps_parse_exactly() {
    assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        parse_rfc3339_utc("2020-01-02T03:04:05Z"),
        Some(1_577_934_245)
    );
    assert_eq!(parse_rfc3339_utc("2020-01-02 03:04:05"), None);
    assert_eq!(parse_rfc3339_utc("2020-13-02T03:04:05Z"), None);
    assert_eq!(parse_rfc3339_utc(""), None);
}

// ------------------------------------------------------------------ policy

#[test]
fn drafts_are_filtered_by_default_and_prereleases_follow_policy() {
    let mut draft = release(1, "draft", vec![]);
    draft["draft"] = json!(true);
    let mut pre = release(2, "v2-rc", vec![]);
    pre["prerelease"] = json!(true);
    let stable = release(3, "v1", vec![]);
    let fake = serving(None, vec![draft, pre, stable]);
    let ids = |policy| {
        enumeration(&fake, policy)
            .unwrap()
            .listings
            .iter()
            .map(|l| {
                l.release
                    .as_ref()
                    .unwrap()
                    .provider_release_id
                    .clone()
                    .unwrap()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(GithubReleasePolicy::default()), vec!["3"]);
    assert_eq!(
        ids(GithubReleasePolicy {
            include_prereleases: true,
            ..Default::default()
        }),
        vec!["2", "3"]
    );
    assert_eq!(
        ids(GithubReleasePolicy {
            include_prereleases: true,
            include_drafts: true,
            ..Default::default()
        }),
        vec!["1", "2", "3"]
    );
}

#[test]
fn the_prerelease_flag_is_carried_into_the_release() {
    let mut pre = release(2, "v2-rc", vec![]);
    pre["prerelease"] = json!(true);
    let fake = serving(None, vec![pre]);
    let result = enumeration(
        &fake,
        GithubReleasePolicy {
            include_prereleases: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(result.listings[0].release.as_ref().unwrap().prerelease);
}

// ------------------------------------------------------------------ assets

#[test]
fn one_release_can_carry_assets_for_several_platforms_and_some_for_none() {
    let fake = serving(
        None,
        vec![release(
            9,
            "v1",
            vec![
                asset(1, "demo.gba"),
                asset(2, "demo.NES"),
                asset(3, "demo.gb.zip"),
                asset(4, "manual.pdf"),
                asset(5, "demo-source.tar.gz"),
                asset(6, "firmware.bin"),
            ],
        )],
    );
    let result = enumeration(&fake, GithubReleasePolicy::default()).unwrap();
    let artifacts = &result.listings[0].artifacts;
    let claim = |index: usize| {
        artifacts[index]
            .platform_claims
            .iter()
            .map(|c| (c.platform.clone(), c.evidence.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        claim(0),
        vec![(
            "Game Boy Advance".into(),
            PlatformClaimEvidence::Extension("gba".into())
        )]
    );
    assert_eq!(
        claim(1),
        vec![("NES".into(), PlatformClaimEvidence::Extension("nes".into()))]
    );
    assert_eq!(
        claim(2),
        vec![(
            "Game Boy".into(),
            PlatformClaimEvidence::Filename(".gb.zip".into())
        )]
    );
    assert!(claim(3).is_empty(), "a manual has no platform claim");
    assert!(claim(4).is_empty(), "source is not a ROM");
    assert!(claim(5).is_empty(), "an ambiguous .bin is never guessed");
}

#[test]
fn every_claimed_platform_is_a_registered_platform_but_is_never_verified() {
    for (extension, platform) in ROM_EXTENSIONS {
        let claims = platform_claims_for(&format!("x.{extension}"));
        let claim = &claims[0];
        assert_eq!(&claim.platform, platform);
        assert!(
            claim.registered_platform().is_some(),
            "{platform} must be a registry id"
        );
        assert!(
            matches!(claim.evidence, PlatformClaimEvidence::Extension(_)),
            "the evidence kind is retained, never upgraded to structural inspection"
        );
    }
    assert!(
        platform_claims_for(".gba.zip").is_empty(),
        "no stem, no claim"
    );
}

#[test]
fn an_artifact_maps_size_link_and_a_browser_handoff_without_permission() {
    let fake = serving(
        Some("MIT"),
        vec![release(9, "v1", vec![asset(1, "demo.gba")])],
    );
    let artifact = &enumeration(&fake, GithubReleasePolicy::default())
        .unwrap()
        .listings[0]
        .artifacts[0];
    assert_eq!(artifact.filename, "demo.gba");
    assert_eq!(artifact.size_bytes, Some(1234));
    assert_eq!(
        artifact.acquisition.link_url.as_deref(),
        Some("https://github.com/owner/demo/releases/download/v1/demo.gba")
    );
    assert_eq!(
        artifact.acquisition.source_page_url,
        "https://github.com/owner/demo/releases/tag/v1"
    );
    assert_eq!(
        artifact.acquisition.mode,
        ArtifactAcquisitionMode::BrowserRequired,
        "a link is a handoff; the provider never asserts direct acquisition is permitted"
    );
}

#[test]
fn unavailable_or_off_allowlist_assets_offer_no_link() {
    let mut pending = asset(1, "a.gba");
    pending["state"] = json!("starter");
    let mut elsewhere = asset(2, "b.gba");
    elsewhere["browser_download_url"] = json!("https://evil.example/b.gba");
    let mut insecure = asset(3, "c.gba");
    insecure["browser_download_url"] = json!("http://github.com/c.gba");
    let fake = serving(
        None,
        vec![release(9, "v1", vec![pending, elsewhere, insecure])],
    );
    let artifacts = enumeration(&fake, GithubReleasePolicy::default())
        .unwrap()
        .listings[0]
        .artifacts
        .clone();
    assert_eq!(
        artifacts[0].acquisition.mode,
        ArtifactAcquisitionMode::Unavailable
    );
    assert!(artifacts[0].acquisition.link_url.is_none());
    for artifact in &artifacts[1..] {
        assert_eq!(
            artifact.acquisition.mode,
            ArtifactAcquisitionMode::Unsupported
        );
        assert!(artifact.acquisition.link_url.is_none());
    }
}

// ----------------------------------------------------------------- digests

#[test]
fn a_github_digest_is_provider_computed_evidence_only() {
    let mut with = asset(1, "a.gba");
    with["digest"] = json!(format!("sha256:{}", "AB".repeat(32)));
    let fake = serving(None, vec![release(9, "v1", vec![with, asset(2, "b.gba")])]);
    let artifacts = &enumeration(&fake, GithubReleasePolicy::default())
        .unwrap()
        .listings[0]
        .artifacts;
    let digest = artifacts[0].provider_digest_evidence().unwrap();
    assert_eq!(
        digest.provenance,
        crate::homebrew_artifact::DigestProvenance::ProviderComputed
    );
    assert_eq!(digest.checksum.algorithm, ArtifactChecksumAlgorithm::Sha256);
    assert_eq!(digest.checksum.value, "ab".repeat(32));
    assert!(
        artifacts[1].provider_digest.is_none(),
        "an absent digest stays absent"
    );
}

#[test]
fn unusable_digests_are_dropped_not_guessed() {
    for bad in ["md5:abcd", "sha256:short", "sha256:zz", "nonsense", "sha1:"] {
        assert!(parse_digest(bad).is_none(), "{bad}");
    }
    assert_eq!(
        parse_digest(&format!("sha1:{}", "0a".repeat(20)))
            .unwrap()
            .algorithm,
        ArtifactChecksumAlgorithm::Sha1
    );
}

// ------------------------------------------------------------------ rights

#[test]
fn the_repository_spdx_id_fills_only_the_software_licence() {
    let fake = serving(
        Some("GPL-3.0-only"),
        vec![release(9, "v1", vec![asset(1, "a.gba")])],
    );
    let rights = &enumeration(&fake, GithubReleasePolicy::default())
        .unwrap()
        .listings[0]
        .artifacts[0]
        .rights;
    match &rights.software_licence {
        RightsFact::Known { value, evidence } => {
            assert_eq!(value, "GPL-3.0-only");
            assert!(evidence.contains("nothing about the asset's own licence"));
        }
        other => panic!("expected a known software licence, got {other:?}"),
    }
    assert_eq!(rights.asset_licence, RightsFact::Unknown);
    assert_eq!(
        rights.redistribution,
        RightsFact::Unknown,
        "downloadable is not redistributable"
    );
    assert_eq!(rights.commercial_status, RightsFact::Unknown);
    assert_eq!(rights.source_availability, RightsFact::Unknown);
}

#[test]
fn no_or_noassertion_licence_leaves_everything_unknown() {
    for spdx in [None, Some("NOASSERTION")] {
        let fake = serving(spdx, vec![release(9, "v1", vec![asset(1, "a.gba")])]);
        let result = enumeration(&fake, GithubReleasePolicy::default()).unwrap();
        assert_eq!(
            result.listings[0].artifacts[0].rights,
            RightsEvidence::default()
        );
    }
}

// ---------------------------------------------------------------- refusals

#[test]
fn malformed_responses_are_refused() {
    let cases: Vec<(&str, Handler)> = vec![
        (
            "not json",
            Box::new(|_| {
                Ok(GithubResponse {
                    status: 200,
                    body: b"<html>".to_vec(),
                    ..Default::default()
                })
            }),
        ),
        (
            "repository without id",
            Box::new(|r| {
                if r.url.contains("/releases?") {
                    ok(json!([]))
                } else {
                    ok(json!({"full_name":"a/b","html_url":"https://github.com/a/b"}))
                }
            }),
        ),
        (
            "release without id",
            Box::new(|r| {
                if r.url.contains("/releases?") {
                    ok(json!([{"tag_name":"v1"}]))
                } else {
                    ok(repository(None))
                }
            }),
        ),
        (
            "asset without name",
            Box::new(|r| {
                if r.url.contains("/releases?") {
                    ok(json!([release(1, "v1", vec![json!({"id":5})])]))
                } else {
                    ok(repository(None))
                }
            }),
        ),
        (
            "asset with a path in its name",
            Box::new(|r| {
                if r.url.contains("/releases?") {
                    ok(json!([release(1, "v1", vec![asset(5, "../evil.gba")])]))
                } else {
                    ok(repository(None))
                }
            }),
        ),
        (
            "releases not an array",
            Box::new(|r| {
                if r.url.contains("/releases?") {
                    ok(json!({"message":"x"}))
                } else {
                    ok(repository(None))
                }
            }),
        ),
    ];
    for (label, handler) in cases {
        let fake = Fake::new(handler);
        let result = provider(&fake, GithubReleasePolicy::default()).enumerate(&repo(), None);
        assert!(
            matches!(result, Err(GithubProviderError::Malformed(_))),
            "{label}: {result:?}"
        );
    }
}

#[test]
fn duplicate_asset_ids_and_release_ids_are_refused() {
    let fake = serving(
        None,
        vec![release(1, "v1", vec![asset(5, "a.gba"), asset(5, "b.gba")])],
    );
    assert!(matches!(
        enumeration(&fake, GithubReleasePolicy::default()),
        Err(GithubProviderError::DuplicateAssetId { .. })
    ));
    let fake = serving(
        None,
        vec![release(1, "v1", vec![]), release(1, "v2", vec![])],
    );
    assert!(matches!(
        enumeration(&fake, GithubReleasePolicy::default()),
        Err(GithubProviderError::DuplicateReleaseId(_))
    ));
}

#[test]
fn an_oversized_response_is_refused_even_if_the_transport_returns_it() {
    let fake = Fake::new(|_| {
        Ok(GithubResponse {
            status: 200,
            body: vec![b' '; MAX_RESPONSE_BYTES + 1],
            ..Default::default()
        })
    });
    assert_eq!(
        provider(&fake, GithubReleasePolicy::default())
            .enumerate(&repo(), None)
            .unwrap_err(),
        GithubProviderError::ResponseTooLarge
    );
}

#[test]
fn a_repository_id_that_does_not_match_the_request_is_refused() {
    let fake = serving(None, vec![]);
    let result = provider(&fake, GithubReleasePolicy::default())
        .enumerate(&GithubRepositoryRef::Id(1), None);
    assert!(matches!(result, Err(GithubProviderError::Malformed(_))));
}

#[test]
fn http_failures_are_typed() {
    let status = |code: u16, remaining: Option<u64>| {
        let fake = Fake::new(move |_| {
            Ok(GithubResponse {
                status: code,
                rate_limit_remaining: remaining,
                rate_limit_reset_unix: Some(123),
                body: b"{}".to_vec(),
                ..Default::default()
            })
        });
        provider(&fake, GithubReleasePolicy::default())
            .enumerate(&repo(), None)
            .unwrap_err()
    };
    assert_eq!(status(404, None), GithubProviderError::NotFound);
    assert_eq!(
        status(403, Some(0)),
        GithubProviderError::RateLimited {
            reset_unix: Some(123)
        }
    );
    assert_eq!(
        status(429, None),
        GithubProviderError::RateLimited {
            reset_unix: Some(123)
        }
    );
    assert_eq!(status(403, Some(10)), GithubProviderError::HttpStatus(403));
    assert_eq!(status(500, None), GithubProviderError::HttpStatus(500));
}

// -------------------------------------------------------------- pagination

#[test]
fn pagination_is_bounded_and_says_when_it_stopped_early() {
    let fake = Fake::new(|request| {
        if request.url.contains("/releases?") {
            let page: u64 = request.url.rsplit("page=").next().unwrap().parse().unwrap();
            let releases: Vec<Value> = (0..2)
                .map(|i| release(page * 10 + i, &format!("t{page}-{i}"), vec![]))
                .collect();
            ok(Value::Array(releases))
        } else {
            ok(repository(None))
        }
    });
    let policy = GithubReleasePolicy {
        per_page: 2,
        max_pages: 3,
        max_releases: 100,
        ..Default::default()
    };
    let result = enumeration(&fake, policy).unwrap();
    assert_eq!(result.listings.len(), 6);
    assert!(result.truncated, "a full final page means more may exist");
    let release_requests = fake
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.url.contains("/releases?"))
        .count();
    assert_eq!(release_requests, 3, "never more pages than the bound");
}

#[test]
fn the_release_count_is_bounded_and_hard_ceilings_apply() {
    let fake = Fake::new(|request| {
        if request.url.contains("/releases?") {
            ok(Value::Array(
                (0..5)
                    .map(|i| release(i + 1, &format!("t{i}"), vec![]))
                    .collect(),
            ))
        } else {
            ok(repository(None))
        }
    });
    let policy = GithubReleasePolicy {
        per_page: 5,
        max_pages: 5,
        max_releases: 3,
        ..Default::default()
    };
    let result = enumeration(&fake, policy).unwrap();
    assert_eq!(result.listings.len(), 3);
    assert!(result.truncated);
    let wild = GithubReleasePolicy {
        per_page: 250,
        max_pages: 250,
        max_releases: usize::MAX,
        ..Default::default()
    }
    .clamped();
    assert_eq!(
        (wild.per_page, wild.max_pages, wild.max_releases),
        (MAX_PER_PAGE, MAX_PAGES_LIMIT, MAX_RELEASES_LIMIT)
    );
}

#[test]
fn a_short_page_ends_enumeration_without_claiming_truncation() {
    let fake = serving(None, vec![release(1, "v1", vec![])]);
    let result = enumeration(&fake, GithubReleasePolicy::default()).unwrap();
    assert!(!result.truncated);
    assert_eq!(
        fake.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.url.contains("/releases?"))
            .count(),
        1
    );
}

#[test]
fn a_long_release_body_is_truncated_as_metadata() {
    let mut value = release(1, "v1", vec![]);
    value["body"] = json!("é".repeat(MAX_BODY_METADATA_BYTES));
    let fake = serving(None, vec![value]);
    let detail = &enumeration(&fake, GithubReleasePolicy::default())
        .unwrap()
        .details[0];
    assert!(detail.body_truncated);
    assert!(detail.body.as_ref().unwrap().len() <= MAX_BODY_METADATA_BYTES);
}

// --------------------------------------------------- conditional requests

#[test]
fn validators_make_the_first_page_conditional_and_a_304_is_not_modified() {
    let fake = Fake::new(|request| {
        if request.url.contains("/releases?") {
            if request.if_none_match.as_deref() == Some("\"abc\"") {
                Ok(GithubResponse {
                    status: 304,
                    ..Default::default()
                })
            } else {
                ok(json!([]))
            }
        } else {
            ok(repository(None))
        }
    });
    let previous = GithubValidators {
        etag: Some("\"abc\"".into()),
        last_modified: None,
    };
    let outcome = provider(&fake, GithubReleasePolicy::default())
        .enumerate(&repo(), Some(&previous))
        .unwrap();
    assert_eq!(outcome, GithubEnumerationOutcome::NotModified);
    // Without validators the same server answers normally.
    assert!(matches!(
        provider(&fake, GithubReleasePolicy::default())
            .enumerate(&repo(), None)
            .unwrap(),
        GithubEnumerationOutcome::Releases(_)
    ));
}

// ------------------------------------------------- input and no discovery

#[test]
fn only_explicit_repository_identities_are_accepted() {
    assert_eq!(
        GithubRepositoryRef::parse("pinobatch/240p-test-mini").unwrap(),
        GithubRepositoryRef::Named {
            owner: "pinobatch".into(),
            repository: "240p-test-mini".into()
        }
    );
    assert_eq!(
        GithubRepositoryRef::parse("id:123").unwrap(),
        GithubRepositoryRef::Id(123)
    );
    for bad in [
        "",
        "owner",
        "owner/",
        "/repo",
        "a/b/c",
        "topic:gba",
        "gba homebrew",
        "owner/repo?x=1",
        "owner/..",
        "../repo",
        "id:",
        "id:abc",
        "owner/re po",
        "language:rust stars:>10",
    ] {
        assert!(
            matches!(
                GithubRepositoryRef::parse(bad),
                Err(GithubProviderError::InvalidRepository(_))
            ),
            "{bad:?} must be refused"
        );
    }
}

#[test]
fn the_trait_refuses_another_providers_project_and_accepts_a_canonical_id() {
    let fake = serving(None, vec![release(1, "v1", vec![asset(2, "a.gba")])]);
    let provider = provider(&fake, GithubReleasePolicy::default());
    let other = HomebrewProjectId {
        provider: HomebrewProviderId::new("itch").unwrap(),
        provider_id: "owner/demo".into(),
    };
    assert_eq!(
        provider.artifact_releases(&other).unwrap_err(),
        GithubProviderError::WrongProvider("itch".into())
    );
    let named = HomebrewProjectId {
        provider: HomebrewProviderId::new("github").unwrap(),
        provider_id: "owner/demo".into(),
    };
    let listings = provider.artifact_releases(&named).unwrap();
    assert_eq!(listings.len(), 1);
    // The canonical (numeric) id it returned is accepted back.
    let canonical = listings[0].project.clone();
    assert_eq!(provider.artifact_releases(&canonical).unwrap().len(), 1);
}

#[test]
fn enumeration_makes_only_repository_and_release_gets_and_never_fetches_an_asset() {
    let fake = serving(
        Some("MIT"),
        vec![release(1, "v1", vec![asset(2, "a.gba"), asset(3, "b.nes")])],
    );
    let _ = enumeration(&fake, GithubReleasePolicy::default()).unwrap();
    let requests = fake.requests.lock().unwrap();
    assert!(!requests.is_empty());
    for request in requests.iter() {
        assert!(api_url_allowed(&request.url), "{}", request.url);
        let path = request.url.strip_prefix("https://api.github.com").unwrap();
        let repository_get = path == "/repos/owner/demo";
        let releases_get = path.starts_with("/repositories/4242/releases?per_page=");
        assert!(repository_get || releases_get, "unexpected request {path}");
        assert!(
            !request
                .url
                .contains("github.com/owner/demo/releases/download")
        );
        assert!(!path.contains("search") && !path.contains("topic"));
    }
}

#[test]
fn the_provider_source_has_no_discovery_search_or_write_paths() {
    let source = include_str!("../homebrew_github.rs");
    let code = source.split("#[cfg(test)]").next().unwrap();
    for forbidden in [
        "search/",
        "topic:",
        "/users/",
        "/orgs/",
        "ProjectDiscoveryProvider",
        "std::fs",
        "File::create",
        "OpenOptions",
        ".post(",
        ".put(",
        ".delete(",
        ".patch(",
        "download_mod_payload",
    ] {
        assert!(
            !code.contains(forbidden),
            "provider must not contain {forbidden}"
        );
    }
}

#[test]
fn the_transport_only_allows_https_api_github_com() {
    assert!(api_url_allowed("https://api.github.com/repos/a/b"));
    for bad in [
        "http://api.github.com/repos/a/b",
        "https://github.com/a/b",
        "https://api.github.com.evil.example/x",
        "https://evil.example/https://api.github.com/",
        "ftp://api.github.com/",
    ] {
        assert!(!api_url_allowed(bad), "{bad}");
    }
    let transport = UreqGithubTransport::new();
    let refused = transport.get(
        &GithubRequest {
            url: "http://api.github.com/repos/a/b".into(),
            ..Default::default()
        },
        1024,
    );
    assert!(matches!(
        refused,
        Err(GithubProviderError::UrlNotAllowed(_))
    ));
}

/// Optional, read-only live smoke test against a known legal repository. It
/// lists release metadata and never downloads an asset. Run explicitly with
/// `cargo test -p archivefs-core --lib homebrew_github -- --ignored`.
#[test]
#[ignore = "touches the live GitHub API; read-only, no downloads"]
fn live_smoke_lists_release_metadata_for_a_known_repository() {
    let provider = GithubReleasesProvider::new();
    let repository = GithubRepositoryRef::parse("pinobatch/240p-test-mini").unwrap();
    let outcome = provider
        .enumerate(&repository, None)
        .expect("live GitHub request");
    let GithubEnumerationOutcome::Releases(result) = outcome else {
        panic!("a first request cannot be not-modified");
    };
    assert_eq!(
        result.repository.full_name.to_lowercase(),
        "pinobatch/240p-test-mini"
    );
    assert!(result.repository.repository_id > 0);
    for listing in &result.listings {
        for artifact in &listing.artifacts {
            assert!(!artifact.provider_asset_id.is_empty());
            assert_eq!(artifact.rights.redistribution, RightsFact::Unknown);
        }
    }
}
