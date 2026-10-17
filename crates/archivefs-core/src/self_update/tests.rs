use super::*;
use crate::emulator_download::{EmulatorHttpResponse, EmulatorTransferContext};
use std::sync::Mutex;

#[test]
fn build_metadata_is_not_version_precedence_and_numeric_components_are_numeric() {
    for (current, latest, status) in [
        (
            "1.2.3+local",
            "v1.2.3+release",
            EmuWizUpdateStatus::UpToDate,
        ),
        ("1.2.10", "1.2.9", EmuWizUpdateStatus::DevelopmentBuild),
        (
            "1.2.3-alpha.2",
            "1.2.3-alpha.10",
            EmuWizUpdateStatus::UpdateAvailable,
        ),
        ("1.2.3-rc.1", "1.2.3", EmuWizUpdateStatus::UpdateAvailable),
        ("01.2.3", "1.2.3", EmuWizUpdateStatus::CurrentVersionUnknown),
    ] {
        assert_eq!(compare_emuwiz_versions(current, latest), status);
    }
}

#[test]
fn matching_installed_version_is_up_to_date_after_a_mocked_response() {
    let t = FixtureTransport::ok(&release_json(current_emuwiz_version()));
    assert_eq!(check_emuwiz_update(&t).status, EmuWizUpdateStatus::UpToDate);
}

#[test]
fn missing_malformed_or_inconsistent_fields_are_never_stable_release_evidence() {
    for body in [
        "{}",
        r#"{"tag_name":"v1.2.3"}"#,
        r#"{"tag_name":"v1.2.3","draft":null,"prerelease":false}"#,
        r#"{"tag_name":"invalid","draft":false,"prerelease":false}"#,
        r#"{"tag_name":"v1.2.3-rc.1","draft":false,"prerelease":false}"#,
        r#"{"tag_name":"v1.2.3","draft":true,"prerelease":false}"#,
        "",
        r#"{"tag_name":"v1.2.3","draft":false,"prerelease":false,"body":3}"#,
    ] {
        let r = check_emuwiz_update(&FixtureTransport::ok(body));
        assert_eq!(r.status, EmuWizUpdateStatus::MalformedResponse, "{body}");
        assert!(r.release_url.is_none());
        assert!(r.latest_version.is_none());
    }
}

#[test]
fn http_unavailability_is_distinct_from_connection_failure_and_malformed_data() {
    for status in [204, 301, 302, 307, 308, 403, 404, 429, 500, 503] {
        let t = FixtureTransport::sequence(vec![(status, Vec::new(), None)]);
        assert_eq!(
            check_emuwiz_update(&t).status,
            EmuWizUpdateStatus::Unavailable
        );
        assert_eq!(
            t.requested_urls.lock().unwrap().as_slice(),
            [EMUWIZ_RELEASE_API_URL]
        );
    }
}

#[test]
fn release_links_require_the_exact_official_repository_and_tag() {
    let tag = "v1.2.3";
    assert!(validated_emuwiz_release_url(
        "https://github.com/kiehntre/emuwiz/releases/tag/v1.2.3",
        tag
    ));
    for bad in [
        "http://github.com/kiehntre/emuwiz/releases/tag/v1.2.3",
        "https://github.com.evil.test/kiehntre/emuwiz/releases/tag/v1.2.3",
        "https://evil.test/kiehntre/emuwiz/releases/tag/v1.2.3",
        "https://user@github.com/kiehntre/emuwiz/releases/tag/v1.2.3",
        "https://github.com:444/kiehntre/emuwiz/releases/tag/v1.2.3",
        "https://github.com/other/emuwiz/releases/tag/v1.2.3",
        "https://github.com/kiehntre/emuwiz/releases/tag/v2.0.0",
        "https://github.com/kiehntre/emuwiz/releases/tag/v1.2.3?q=x",
        "https://github.com/kiehntre/emuwiz/releases/tag/v1.2.3#x",
        "https://github.com/kiehntre/emuwiz/releases/download/v1.2.3/emuwiz",
        "https://github.com/kiehntre/emuwiz/releases/tag/%761.2.3",
    ] {
        assert!(!validated_emuwiz_release_url(bad, tag), "{bad}");
        let body =
            serde_json::json!({"tag_name":tag,"draft":false,"prerelease":false,"html_url":bad})
                .to_string();
        let r = check_emuwiz_update(&FixtureTransport::ok(&body));
        assert_eq!(r.status, EmuWizUpdateStatus::MalformedResponse, "{bad}");
        assert!(r.release_url.is_none());
    }
}

#[test]
fn release_notes_are_plain_metadata_and_never_trigger_an_artifact_request() {
    let t = FixtureTransport::ok(
        r#"{"tag_name":"v1.2.3","draft":false,"prerelease":false,"html_url":"https://github.com/kiehntre/emuwiz/releases/tag/v1.2.3","body":"<script>plain text only</script>","assets":[{"browser_download_url":"https://evil.test/artifact"}]}"#,
    );
    let r = check_emuwiz_update(&t);
    assert_eq!(
        r.release_notes.as_deref(),
        Some("<script>plain text only</script>")
    );
    assert_eq!(
        t.requested_urls.lock().unwrap().as_slice(),
        [EMUWIZ_RELEASE_API_URL]
    );
}

#[test]
fn caller_bound_refuses_an_oversized_body_even_if_the_mock_ignores_its_limit() {
    let t = FixtureTransport::ok(&"x".repeat(MAX_RELEASE_METADATA_BYTES as usize + 1));
    assert_eq!(
        check_emuwiz_update(&t).status,
        EmuWizUpdateStatus::MalformedResponse
    );
}

#[test]
fn inconsistent_reported_lengths_are_malformed_not_a_success() {
    struct WrongLength;
    impl EmulatorDownloadTransport for WrongLength {
        fn get(
            &self,
            _: &str,
            _: u64,
            d: &mut dyn Write,
            _: EmulatorTransferContext<'_>,
        ) -> Result<EmulatorHttpResponse, DownloadError> {
            let b = release_json("v1.2.3");
            d.write_all(b.as_bytes()).unwrap();
            Ok(EmulatorHttpResponse {
                status: 200,
                content_length: Some(b.len() as u64 + 1),
                location: None,
                bytes_received: b.len() as u64,
            })
        }
    }
    assert_eq!(
        check_emuwiz_update(&WrongLength).status,
        EmuWizUpdateStatus::MalformedResponse
    );
}

// --- compare_emuwiz_versions: safe, deterministic semver precedence ---

#[test]
fn same_current_and_latest_is_up_to_date() {
    assert_eq!(
        compare_emuwiz_versions("0.8.2", "0.8.2"),
        EmuWizUpdateStatus::UpToDate
    );
}

#[test]
fn newer_official_release_is_update_available() {
    assert_eq!(
        compare_emuwiz_versions("0.8.1-alpha", "0.8.2"),
        EmuWizUpdateStatus::UpdateAvailable
    );
}

#[test]
fn current_prerelease_vs_its_own_stable_release_is_update_available() {
    // Same numeric core; semver defines a prerelease as lower
    // precedence than the plain release of the same core.
    assert_eq!(
        compare_emuwiz_versions("0.8.2-beta.1", "0.8.2"),
        EmuWizUpdateStatus::UpdateAvailable
    );
}

#[test]
fn v_prefixed_release_tag_is_normalised() {
    assert_eq!(
        compare_emuwiz_versions("0.8.1", "v0.8.2"),
        EmuWizUpdateStatus::UpdateAvailable
    );
    assert_eq!(
        compare_emuwiz_versions("0.8.2", "v0.8.2"),
        EmuWizUpdateStatus::UpToDate
    );
}

#[test]
fn malformed_latest_version_is_unknown_not_guessed() {
    assert_eq!(
        compare_emuwiz_versions("0.8.2", "not-a-version"),
        EmuWizUpdateStatus::LatestVersionUnknown
    );
}

#[test]
fn malformed_current_version_is_unknown_not_guessed() {
    assert_eq!(
        compare_emuwiz_versions("not-a-version", "0.8.2"),
        EmuWizUpdateStatus::CurrentVersionUnknown
    );
    // Malformed current is checked first/independently of latest.
    assert_eq!(
        compare_emuwiz_versions("also-not-a-version", "also-not-a-version"),
        EmuWizUpdateStatus::CurrentVersionUnknown
    );
}

#[test]
fn newer_local_build_is_development_build_never_a_downgrade() {
    assert_eq!(
        compare_emuwiz_versions("0.9.0-beta.1", "0.8.2"),
        EmuWizUpdateStatus::DevelopmentBuild
    );
    assert_eq!(
        compare_emuwiz_versions("0.9.0", "0.8.2"),
        EmuWizUpdateStatus::DevelopmentBuild
    );
}

#[test]
fn current_emuwiz_version_is_the_one_authoritative_constant() {
    assert_eq!(current_emuwiz_version(), env!("CARGO_PKG_VERSION"));
}

// --- check_emuwiz_update: fixture transport, no live GitHub dependency ---

#[derive(Default)]
struct FixtureTransport {
    /// Queued (status, body, location) responses, returned in order -
    /// one entry per hop, so a test can script a redirect chain.
    responses: Mutex<Vec<(u16, Vec<u8>, Option<String>)>>,
    requested_urls: Mutex<Vec<String>>,
}

impl FixtureTransport {
    fn ok(body: &str) -> Self {
        Self {
            responses: Mutex::new(vec![(200, body.as_bytes().to_vec(), None)]),
            requested_urls: Mutex::new(Vec::new()),
        }
    }

    fn sequence(steps: Vec<(u16, Vec<u8>, Option<String>)>) -> Self {
        Self {
            responses: Mutex::new(steps),
            requested_urls: Mutex::new(Vec::new()),
        }
    }
}

impl EmulatorDownloadTransport for FixtureTransport {
    fn get(
        &self,
        url: &str,
        _maximum_bytes: u64,
        destination: &mut dyn Write,
        _context: EmulatorTransferContext<'_>,
    ) -> Result<EmulatorHttpResponse, DownloadError> {
        self.requested_urls.lock().unwrap().push(url.to_string());
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Err(DownloadError::Io("fixture exhausted".into()));
        }
        let (status, body, location) = responses.remove(0);
        destination
            .write_all(&body)
            .map_err(|e| DownloadError::Io(e.to_string()))?;
        Ok(EmulatorHttpResponse {
            status,
            content_length: Some(body.len() as u64),
            location,
            bytes_received: body.len() as u64,
        })
    }
}

struct FailingTransport;

impl EmulatorDownloadTransport for FailingTransport {
    fn get(
        &self,
        _url: &str,
        _maximum_bytes: u64,
        _destination: &mut dyn Write,
        _context: EmulatorTransferContext<'_>,
    ) -> Result<EmulatorHttpResponse, DownloadError> {
        Err(DownloadError::Io("simulated network failure".into()))
    }
}

fn release_json(tag: &str) -> String {
    format!(
        r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,"html_url":"https://github.com/kiehntre/emuwiz/releases/tag/{tag}"}}"#
    )
}

#[test]
fn network_failure_produces_check_failed_and_keeps_current_version() {
    let transport = FailingTransport;
    let result = check_emuwiz_update(&transport);
    assert_eq!(result.status, EmuWizUpdateStatus::NetworkFailure);
    assert!(result.detail.is_some());
    assert_eq!(result.current_version, current_emuwiz_version());
    assert_eq!(result.latest_version, None);
}

#[test]
fn a_real_lookup_reports_update_available_against_a_newer_official_release() {
    let transport = FixtureTransport::ok(&release_json("v99.0.0"));
    let result = check_emuwiz_update(&transport);
    assert_eq!(result.status, EmuWizUpdateStatus::UpdateAvailable);
    assert_eq!(result.latest_version.as_deref(), Some("v99.0.0"));
    assert!(
        result
            .release_url
            .unwrap()
            .starts_with(EMUWIZ_OFFICIAL_REPOSITORY)
    );
    assert_eq!(result.current_version, current_emuwiz_version());
}

#[test]
fn checked_at_is_populated() {
    let transport = FixtureTransport::ok(&release_json("v0.1.0"));
    let result = check_emuwiz_update(&transport);
    assert!(result.checked_at_unix_seconds > 0);
}

/// The release-metadata check can never change application readiness:
/// it returns a plain value type with no side effects on any shared
/// state, health signal, or startup gate - there is nothing in this
/// module's API surface that could. This test documents that contract
/// structurally: calling it twice with two independent fixtures never
/// observes or mutates anything beyond its own return value.
#[test]
fn release_metadata_check_has_no_side_effects_on_anything_else() {
    let transport_a = FixtureTransport::ok(&release_json("v1.0.0"));
    let transport_b = FailingTransport;
    let first = check_emuwiz_update(&transport_a);
    let second = check_emuwiz_update(&transport_b);
    // Independent calls never interfere with each other - no shared
    // mutable state anywhere in this module.
    assert_eq!(first.status, EmuWizUpdateStatus::UpdateAvailable);
    assert_eq!(second.status, EmuWizUpdateStatus::NetworkFailure);
}

#[test]
fn a_redirect_to_a_non_github_host_is_rejected() {
    let transport = FixtureTransport::sequence(vec![(
        302,
        Vec::new(),
        Some("https://evil.example/not-a-real-release".to_string()),
    )]);
    let result = check_emuwiz_update(&transport);
    assert_eq!(result.status, EmuWizUpdateStatus::Unavailable);
    let detail = result.detail.unwrap();
    assert!(
        detail.contains("allowlist") || detail.contains("HTTPS"),
        "expected an allowlist/HTTPS rejection, got: {detail}"
    );
}

#[test]
fn even_an_api_github_com_redirect_is_not_followed() {
    let transport = FixtureTransport::sequence(vec![
        (
            302,
            Vec::new(),
            Some("https://api.github.com/repositories/1/releases/latest".to_string()),
        ),
        (200, release_json("v2.0.0").into_bytes(), None),
    ]);
    let result = check_emuwiz_update(&transport);
    assert_eq!(result.status, EmuWizUpdateStatus::Unavailable);
    assert!(result.latest_version.is_none());
    assert_eq!(transport.requested_urls.lock().unwrap().len(), 1);
}

#[test]
fn only_the_one_allowlisted_initial_url_is_ever_requested() {
    let transport = FixtureTransport::ok(&release_json("v1.0.0"));
    let _ = check_emuwiz_update(&transport);
    let requested = transport.requested_urls.lock().unwrap();
    assert_eq!(requested.as_slice(), [EMUWIZ_RELEASE_API_URL]);
}

#[test]
fn a_draft_or_prerelease_from_the_latest_endpoint_is_refused_defensively() {
    let transport = FixtureTransport::ok(
        r#"{"tag_name":"v0.9.0","draft":false,"prerelease":true,"html_url":null}"#,
    );
    let result = check_emuwiz_update(&transport);
    assert_eq!(result.status, EmuWizUpdateStatus::MalformedResponse);
}

#[test]
fn malformed_release_metadata_is_check_failed_not_a_panic() {
    let transport = FixtureTransport::ok("not valid json");
    let result = check_emuwiz_update(&transport);
    assert_eq!(result.status, EmuWizUpdateStatus::MalformedResponse);
}
