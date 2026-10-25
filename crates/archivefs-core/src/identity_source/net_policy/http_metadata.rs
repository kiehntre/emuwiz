//! Small wire helpers; no endpoint policy, retries or provider requests.
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_VALUE_BYTES: usize = 1024;

/// Only useful response headers are retained; cookies and tokens are excluded.
pub fn response_headers(headers: &http::HeaderMap) -> BTreeMap<String, String> {
    [
        "retry-after",
        "date",
        "content-type",
        "etag",
        "last-modified",
        "www-authenticate",
        "location",
    ]
    .into_iter()
    .filter_map(|name| {
        let raw = headers.get(name)?;
        let ambiguous_type = name == "content-type" && headers.get_all(name).iter().count() != 1;
        let value = raw.to_str().ok().filter(|value| {
            !ambiguous_type
                && value.len() <= MAX_VALUE_BYTES
                && !value.chars().any(char::is_control)
        });
        // Keep a refusal marker when Content-Type cannot be safely retained.
        // Dropping it must not turn ambiguous metadata into retry permission.
        match value {
            Some(value) => Some((name.to_owned(), value.to_owned())),
            None if name == "content-type" => Some((name.to_owned(), String::new())),
            None => None,
        }
    })
    .collect()
}

/// Maximum error body inspected before a conservative, non-retryable refusal.
pub const ERROR_BODY_LIMIT: usize = 16 * 1024;

/// Retry permission is deliberately narrower than document detection. HTML,
/// XHTML, challenge evidence and unclassified nonempty bodies all fail closed.
/// Body text is never returned as a diagnostic (it may contain credentials).
pub fn refuses_automatic_retry(content_type: Option<&str>, body: &[u8]) -> bool {
    if body.len() > ERROR_BODY_LIMIT {
        return true;
    }
    let media = content_type.map(|value| value.split(';').next().unwrap_or("").trim());
    if content_type.is_none()
        || content_type.is_some_and(|value| value.len() > MAX_VALUE_BYTES)
        || media.is_some_and(|value| {
            value.eq_ignore_ascii_case("text/html")
                || value.eq_ignore_ascii_case("application/xhtml+xml")
                || value.is_empty()
        })
    {
        return true;
    }
    let Ok(text) = std::str::from_utf8(body) else {
        return true;
    };
    let lower = text.to_ascii_lowercase();
    if [
        "<html",
        "<!doctype html",
        "<head",
        "<body",
        "<script",
        "captcha",
        "cloudflare",
        "challenge",
        "access denied",
        "permission denied",
        "forbidden",
        "unauthorized",
        "authentication required",
        "invalid credentials",
        "invalid api key",
        "security check",
        "checking your browser",
        "verify you are human",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return true;
    }
    // A complete empty error with valid metadata contains no challenge payload. Otherwise
    // require a supported media type rather than guessing from missing headers.
    if body.is_empty() {
        return false;
    }
    match media {
        Some(value) if value.eq_ignore_ascii_case("application/json") => {
            serde_json::from_slice::<serde_json::Value>(body).is_err()
        }
        Some(value) if value.eq_ignore_ascii_case("text/plain") => lower.contains('<'),
        _ => true,
    }
}

/// Inspect successful metadata separately from retry permission: valid game
/// titles may mention challenges or CAPTCHA. Only wire/markup evidence and
/// reserved error fields establish a blocking response here.
pub fn is_blocking_response(content_type: Option<&str>, body: &[u8]) -> bool {
    if content_type.is_some_and(|value| {
        let media = value.split(';').next().unwrap_or("").trim();
        value.len() > MAX_VALUE_BYTES
            || media.is_empty()
            || media.eq_ignore_ascii_case("text/html")
            || media.eq_ignore_ascii_case("application/xhtml+xml")
    }) {
        return true;
    }
    let prefix = &body[..body.len().min(ERROR_BODY_LIMIT)];
    if [
        b"<html".as_slice(),
        b"<!doctype html",
        b"<script",
        b"<head",
        b"<body",
    ]
    .iter()
    .any(|marker| {
        prefix
            .windows(marker.len())
            .any(|bytes| bytes.eq_ignore_ascii_case(marker))
    }) {
        return true;
    }
    if let Ok(serde_json::Value::Object(fields)) = serde_json::from_slice(body) {
        let contains_blocking_fields = |fields: &serde_json::Map<String, serde_json::Value>| {
            fields
                .iter()
                .any(|(key, value)| match key.to_ascii_lowercase().as_str() {
                    "error" | "errors" | "challenge" | "captcha" => true,
                    "message" | "detail" | "title" => value.as_str().is_some_and(|value| {
                        let lower = value.to_ascii_lowercase();
                        [
                            "captcha",
                            "challenge",
                            "cloudflare",
                            "verify you are human",
                            "access denied",
                            "security check",
                        ]
                        .iter()
                        .any(|marker| lower.contains(marker))
                    }),
                    _ => false,
                })
        };
        // Both providers tolerate additive metadata fields. Inspect every
        // casing of message keys, including the ScreenScraper user envelope,
        // before their parsers can discard explicit challenge evidence. Do
        // not recurse into game metadata: its titles/descriptions are data.
        return contains_blocking_fields(&fields)
            || fields.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("ssuser")
                    && value.as_object().is_some_and(contains_blocking_fields)
            });
    }
    false
}

/// Only statuses for which this client has an automatic retry policy.
/// Permanent refusals such as 501 and 505 must not become transient outages.
pub fn is_transient_status(status: u16) -> bool {
    matches!(status, 408 | 500 | 502 | 503 | 504)
}

/// Repeated fields preserve the longest valid provider minimum. Ambiguous
/// duplicate values defer instead of falling back to a shorter default wait.
pub fn response_retry_after(headers: &http::HeaderMap, now: SystemTime) -> Option<u64> {
    let values: Vec<_> = headers.get_all("retry-after").iter().collect();
    let mut longest = None;
    for raw in &values {
        let parsed = raw.to_str().ok().and_then(|value| retry_after(value, now));
        match parsed {
            Some(seconds) => longest = Some(longest.unwrap_or(0).max(seconds)),
            None if values.len() > 1
                || raw.to_str().ok().is_some_and(|value| {
                    value.contains(',')
                        && value.split(',').any(|part| {
                            let part = part.trim();
                            !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                        })
                }) =>
            {
                return Some(u64::MAX);
            }
            None => return None,
        }
    }
    longest
}

/// A minimum wait, never capped downwards. Oversized numeric values saturate
/// to u64::MAX so callers defer instead of falling back to a shorter retry.
/// Dates use the local receipt clock; rounding up preserves fractional seconds.
pub fn retry_after(value: &str, now: SystemTime) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.bytes().all(|b| b.is_ascii_digit()) {
        return Some(value.bytes().fold(0u64, |n, b| {
            n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
        }));
    }
    if value.len() > 128 {
        return None;
    }
    let seconds = http_date(value, now)?;
    let date = UNIX_EPOCH.checked_add(Duration::from_secs(seconds))?;
    let delay = date.duration_since(now).unwrap_or(Duration::ZERO);
    Some(
        delay
            .as_secs()
            .saturating_add(u64::from(delay.subsec_nanos() != 0)),
    )
}

fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
fn year_now(now: SystemTime) -> i64 {
    let mut days = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() / 86400;
    let mut year = 1970;
    while year < 9999 {
        let length = if leap(year) { 366 } else { 365 };
        if days < length {
            break;
        }
        days -= length;
        year += 1;
    }
    year
}

/// IMF-fixdate, obsolete RFC850 and ANSI C asctime forms.
fn http_date(value: &str, now: SystemTime) -> Option<u64> {
    let parts: Vec<_> = value.split_ascii_whitespace().collect();
    let (day, month, year, clock, weekday) = match parts.as_slice() {
        [weekday, day, month, year, clock, "GMT"]
            if weekday.len() == 4
                && weekday.ends_with(',')
                && day.len() == 2
                && year.len() == 4 =>
        {
            (
                day.parse::<i64>().ok()?,
                *month,
                year.parse::<i64>().ok()?,
                *clock,
                weekday.trim_end_matches(','),
            )
        }
        [weekday, date, clock, "GMT"]
            if [
                "Sunday,",
                "Monday,",
                "Tuesday,",
                "Wednesday,",
                "Thursday,",
                "Friday,",
                "Saturday,",
            ]
            .contains(weekday) =>
        {
            let date: Vec<_> = date.split('-').collect();
            let [day, month, year] = date.as_slice() else {
                return None;
            };
            if year.len() != 2 {
                return None;
            }
            let short = year.parse::<i64>().ok()?;
            let current = year_now(now);
            let mut year = current / 100 * 100 + short;
            if year > current + 50 {
                year -= 100;
            }
            (
                day.parse::<i64>().ok()?,
                *month,
                year,
                *clock,
                weekday.trim_end_matches(','),
            )
        }
        [weekday, month, day, clock, year] if weekday.len() == 3 && year.len() == 4 => (
            day.parse::<i64>().ok()?,
            *month,
            year.parse::<i64>().ok()?,
            *clock,
            *weekday,
        ),
        _ => return None,
    };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = months.iter().position(|m| *m == month)? + 1;
    let lengths = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1970..=9999).contains(&year) || day < 1 || day > lengths[month - 1] {
        return None;
    }
    let hms: Vec<_> = clock.split(':').collect();
    let [hour, minute, second] = hms.as_slice() else {
        return None;
    };
    if hms.iter().any(|s| s.len() != 2) {
        return None;
    }
    let (hour, minute, second) = (
        hour.parse::<i64>().ok()?,
        minute.parse::<i64>().ok()?,
        second.parse::<i64>().ok()?,
    );
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
        return None;
    }
    let y = year - i64::from(month <= 2);
    let era = y / 400;
    let yo = y - era * 400;
    let m = month as i64 + if month > 2 { -3 } else { 9 };
    let days = era * 146097 + yo * 365 + yo / 4 - yo / 100 + (153 * m + 2) / 5 + day - 1 - 719468;
    let weekday_number = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
        .iter()
        .position(|d| weekday.get(..3) == Some(*d))?;
    if (days + 4).rem_euclid(7) != weekday_number as i64 {
        return None;
    }
    u64::try_from(days * 86400 + hour * 3600 + minute * 60 + second).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_retry_after_fields_preserve_the_longest_minimum() {
        let now = UNIX_EPOCH + Duration::from_secs(784111777);
        for (values, expected) in [
            (vec!["5", "120"], Some(120)),
            (vec!["120", "5"], Some(120)),
            (vec!["5", "Sun, 06 Nov 1994 08:51:37 GMT"], Some(120)),
            (vec!["120", "malformed"], Some(u64::MAX)),
            (vec!["9999999999999999999999999999999"], Some(u64::MAX)),
            (vec!["malformed"], None),
            (vec!["5, 120"], Some(u64::MAX)),
        ] {
            let mut headers = http::HeaderMap::new();
            for value in values {
                headers.append("retry-after", value.parse().unwrap());
            }
            assert_eq!(response_retry_after(&headers, now), expected);
        }
    }

    #[test]
    fn fail_correction_structured_challenges_are_scoped_to_provider_envelopes() {
        for body in [
            r#"{"Message":"Please solve CAPTCHA before continuing","jeux":[],"platform":null}"#,
            r#"{"ssuser":{"message":"Verify you are human"},"jeux":[]}"#,
            r#"{"message":"Lookup complete","MESSAGE":"Cloudflare security check"}"#,
            r#"{"Detail":"Access denied"}"#,
            r#"{"TiTlE":"Security check"}"#,
            r#"{"SSUSER":{"MeSsAgE":"VERIFY YOU ARE HUMAN"}}"#,
            r#"{"ssuser":{"DETAIL":"CAPTCHA required"}}"#,
            r#"{"ssuser":{"Title":"Cloudflare challenge"}}"#,
        ] {
            for media in [None, Some("application/json")] {
                assert!(is_blocking_response(media, body.as_bytes()), "{body}");
            }
        }
        for body in [
            r#"{"jeux":[],"Message":"Lookup complete","ssuser":{"MESSAGE":"Quota information"}}"#,
            r#"{"jeu":{"id":42,"name":"CAPTCHA Challenge","message":"Verify you are human","synopsis":"Cloudflare security check"}}"#,
            r#"{"signature":{"game":{"name":"CAPTCHA Challenge"},"rom":{"name":"Cloudflare Challenge.gb"}}}"#,
            r#"{"ssuser":{"name":"CAPTCHA Challenge"},"platform":null}"#,
            r#"{"Message":null,"Detail":42,"Title":[],"platform":null}"#,
            r#"{}"#,
        ] {
            assert!(
                !is_blocking_response(Some("application/json"), body.as_bytes()),
                "{body}"
            );
        }
    }

    #[test]
    fn successful_metadata_challenge_checks_do_not_reject_game_titles() {
        assert!(!is_blocking_response(
            Some("application/json"),
            br#"{"jeu":{"id":42,"name":"CAPTCHA Challenge"}}"#
        ));
        for media in [
            None,
            Some("application/json"),
            Some("text/html"),
            Some("application/xhtml+xml"),
            Some(""),
        ] {
            assert!(is_blocking_response(
                media,
                b"<!doctype HTML><HTML>security check</HTML>"
            ));
        }
        assert!(is_blocking_response(
            Some("application/json"),
            br#"{"challenge":"synthetic"}"#
        ));
        assert!(is_blocking_response(
            Some("application/json"),
            br#"{"ssuser":{"error":"synthetic"},"jeux":[]}"#
        ));
    }
    #[test]
    fn challenge_detection_is_independent_of_perfect_metadata() {
        for media in [
            None,
            Some("application/xhtml+xml"),
            Some("text/html"),
            Some(""),
        ] {
            assert!(refuses_automatic_retry(
                media,
                b"<!DOCTYPE html><html>challenge</html>"
            ));
        }
        assert!(refuses_automatic_retry(
            Some("application/json"),
            b"<html>captcha</html>"
        ));
        assert!(refuses_automatic_retry(None, b"unclassified error"));
        assert!(refuses_automatic_retry(Some("text/plain"), &[0xff]));
        assert!(refuses_automatic_retry(
            Some("text/plain"),
            &vec![b'x'; ERROR_BODY_LIMIT + 1]
        ));
        assert!(!refuses_automatic_retry(
            Some("text/plain"),
            b"temporary outage"
        ));
        assert!(!refuses_automatic_retry(
            Some("application/json"),
            b"{\"error\":\"temporary outage\"}"
        ));
        assert!(refuses_automatic_retry(None, b""));
    }

    #[test]
    fn rejected_content_type_remains_a_refusal_marker() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "content-type",
            format!("text/html; boundary={}", "x".repeat(1024))
                .parse()
                .unwrap(),
        );
        let retained = response_headers(&headers);
        assert_eq!(retained.get("content-type").map(String::as_str), Some(""));
        assert!(refuses_automatic_retry(
            retained.get("content-type").map(String::as_str),
            b""
        ));
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.append("content-type", "text/html".parse().unwrap());
        assert_eq!(response_headers(&headers)["content-type"], "");
        headers.remove("content-type");
        headers.insert(
            "content-type",
            http::HeaderValue::from_bytes(&[0xff]).unwrap(),
        );
        assert_eq!(response_headers(&headers)["content-type"], "");
    }

    #[test]
    fn response_debug_redacts_all_header_values_and_body() {
        use crate::identity_source::{
            hasheous::client::HasheousWireResponse, screenscraper::ProviderResponse,
        };
        let headers = [
            (
                "location".into(),
                "https://example.invalid/?devpassword=fake-secret&other=second-secret".into(),
            ),
            ("www-authenticate".into(), "Bearer reflected-secret".into()),
        ]
        .into();
        let ss = ProviderResponse {
            status: 302,
            headers,
            body: b"body-secret".to_vec(),
            retry_after: None,
        };
        let hs = HasheousWireResponse {
            status: ss.status,
            headers: ss.headers.clone(),
            body: ss.body.clone(),
            retry_after_secs: None,
        };
        // Check nested diagnostic representations too, not just the leaf.
        for debug in [
            format!("{ss:?}"),
            format!("{hs:?}"),
            format!("{:?}", vec![ss]),
            format!("{:?}", Some(hs)),
        ] {
            for secret in [
                "fake-secret",
                "second-secret",
                "reflected-secret",
                "body-secret",
                "devpassword",
                "https://",
            ] {
                assert!(!debug.contains(secret), "unsafe diagnostic: {debug}");
            }
            assert!(debug.contains("302"));
        }
    }

    #[test]
    fn seconds_are_never_shortened_and_overflow_defers() {
        let now = UNIX_EPOCH;
        assert_eq!(retry_after("120", now), Some(120));
        assert_eq!(retry_after("999", now), Some(999));
        assert_eq!(retry_after(&"9".repeat(4096), now), Some(u64::MAX));
        for invalid in ["", "-1", "+120", "1.2", "soon", "120x"] {
            assert_eq!(retry_after(invalid, now), None);
        }
    }
    #[test]
    fn http_dates_use_deterministic_clock_and_round_up() {
        let now = UNIX_EPOCH + Duration::from_secs(784111777);
        for date in [
            "Sun, 06 Nov 1994 08:51:37 GMT",
            "Sunday, 06-Nov-94 08:51:37 GMT",
            "Sun Nov  6 08:51:37 1994",
        ] {
            assert_eq!(retry_after(date, now), Some(120));
            assert_eq!(
                retry_after(date, now + Duration::from_millis(500)),
                Some(120)
            );
            assert_eq!(retry_after(date, now + Duration::from_secs(121)), Some(0));
        }
        for invalid in [
            "Sunxyz, 06 Nov 1994 08:51:37 GMT",
            "Sun, 31 Feb 1994 08:51:37 GMT",
            "Sun, 06 Nov 1994 25:51:37 GMT",
            "Sun, 06 Nov 1994 08:51:37 PST",
        ] {
            assert_eq!(retry_after(invalid, now), None);
        }
        assert_eq!(retry_after("Sun, 06 Nov 1994 08:51:37 PST", now), None);
    }
    #[test]
    fn header_allowlist_excludes_secrets_and_bounds_values() {
        let mut h = http::HeaderMap::new();
        h.insert("retry-after", "120".parse().unwrap());
        h.insert("set-cookie", "secret".parse().unwrap());
        h.insert("etag", "x".repeat(1025).parse().unwrap());
        let out = response_headers(&h);
        assert_eq!(out.get("retry-after").map(String::as_str), Some("120"));
        assert!(!out.contains_key("set-cookie"));
        assert!(!out.contains_key("etag"));
    }

    // One response only: production transports must not follow the redirect.
    fn server(status: u16, body: &[u8]) -> (String, std::thread::JoinHandle<()>) {
        server_custom(
            status,
            body,
            "Content-Type: application/json\r\nLocation: /not-followed\r\n",
            body.len(),
        )
    }

    fn server_custom(
        status: u16,
        body: &[u8],
        metadata: &str,
        declared: usize,
    ) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let metadata = metadata.to_owned();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/synthetic", listener.local_addr().unwrap());
        let body = body.to_vec();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let mut chunk = [0; 1024];
            loop {
                let n = socket.read(&mut chunk).unwrap();
                assert!(n > 0);
                received.extend_from_slice(&chunk[..n]);
                if let Some(end) = received.windows(4).position(|v| v == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&received[..end]);
                    let length = header
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if received.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            write!(socket, "HTTP/1.1 {status} Synthetic\r\nContent-Length: {}\r\n{metadata}Retry-After: 120\r\nWWW-Authenticate: Bearer\r\nETag: synthetic\r\nSet-Cookie: excluded\r\nConnection: close\r\n\r\n", declared).unwrap();
            socket.write_all(&body).unwrap();
        });
        (url, thread)
    }

    #[test]
    fn production_transports_preserve_error_status_headers_and_bounded_body() {
        use crate::identity_source::hasheous::client::{HasheousTransport, UreqTransport as Hs};
        use crate::identity_source::screenscraper::{ScreenScraperTransport, UreqTransport as Ss};
        for status in [401, 403, 429, 503, 302] {
            let (url, worker) = server(status, b"{\"error\":\"synthetic\"}");
            let response = Ss::default()
                .get(&url, 1024, Duration::from_secs(5))
                .unwrap();
            worker.join().unwrap();
            assert_eq!(response.status, status);
            assert_eq!(response.body, b"{\"error\":\"synthetic\"}");
            assert_eq!(response.retry_after, Some(Duration::from_secs(120)));
            assert_eq!(response.headers["www-authenticate"], "Bearer");
            assert_eq!(response.headers["location"], "/not-followed");
            assert!(!response.headers.contains_key("set-cookie"));
            let (url, worker) = server(status, b"{\"error\":\"synthetic\"}");
            let response = Hs::new(Duration::from_secs(5))
                .post_json_with_metadata(&url, b"{}")
                .unwrap();
            worker.join().unwrap();
            assert_eq!(response.status, status);
            assert_eq!(response.body, b"{\"error\":\"synthetic\"}");
            assert_eq!(response.retry_after_secs, Some(120));
            assert_eq!(response.headers["etag"], "synthetic");
            assert_eq!(response.headers["location"], "/not-followed");
            assert!(!response.headers.contains_key("set-cookie"));
        }
    }

    #[test]
    fn wire_challenge_metadata_and_redirect_secrets_remain_safe() {
        use crate::identity_source::hasheous::client::{HasheousTransport, UreqTransport as Hs};
        use crate::identity_source::screenscraper::{ScreenScraperTransport, UreqTransport as Ss};
        for metadata in [
            String::new(),
            "Content-Type: application/xhtml+xml\r\n".into(),
            format!("Content-Type: text/html; boundary={}\r\n", "x".repeat(1024)),
            "Content-Type: text/html\r\n".into(),
        ] {
            let body = b"<html>captcha body-secret</html>";
            for status in [403, 503] {
                let (url, worker) = server_custom(status, body, &metadata, body.len());
                let response = Ss::default()
                    .get(&url, 1024, Duration::from_secs(5))
                    .unwrap();
                worker.join().unwrap();
                assert_eq!(response.status, status);
                assert_eq!(response.body, body);
                assert_eq!(response.retry_after, Some(Duration::from_secs(120)));
                assert!(refuses_automatic_retry(
                    response.headers.get("content-type").map(String::as_str),
                    &response.body
                ));
                assert!(!format!("{response:?}").contains("body-secret"));
            }
        }
        let metadata = "Location: https://example.invalid/?devpassword=fake-secret&arbitrary=second-secret\r\n";
        let (url, worker) = server_custom(302, b"", metadata, 0);
        let ss = Ss::default()
            .get(&url, 1024, Duration::from_secs(5))
            .unwrap();
        worker.join().unwrap();
        let (url, worker) = server_custom(302, b"", metadata, 0);
        let hs = Hs::default().post_json_with_metadata(&url, b"{}").unwrap();
        worker.join().unwrap();
        assert!(ss.headers["location"].contains("fake-secret")); // available only for validation
        assert!(hs.headers["location"].contains("second-secret"));
        for diagnostic in [format!("{ss:?}"), format!("{hs:?}")] {
            assert!(!diagnostic.contains("fake-secret"));
            assert!(!diagnostic.contains("second-secret"));
        }
    }

    #[test]
    fn truncated_wire_errors_do_not_echo_sensitive_body_bytes() {
        use crate::identity_source::hasheous::client::{HasheousTransport, UreqTransport as Hs};
        use crate::identity_source::screenscraper::{ScreenScraperTransport, UreqTransport as Ss};
        for status in [401, 403, 429, 500, 503] {
            let (url, worker) = server_custom(
                status,
                b"fake-secret",
                "Content-Type: application/json\r\n",
                100,
            );
            let ss = Ss::default()
                .get(&url, 1024, Duration::from_secs(5))
                .unwrap_err();
            worker.join().unwrap();
            let (url, worker) = server_custom(
                status,
                b"fake-secret",
                "Content-Type: application/json\r\n",
                100,
            );
            let hs = Hs::default()
                .post_json_with_metadata(&url, b"{}")
                .unwrap_err();
            worker.join().unwrap();
            assert!(!format!("{ss:?} {hs:?}").contains("fake-secret"));
            assert!(!ss.is_retryable());
            assert!(format!("{ss:?}").contains(&status.to_string()));
            assert!(format!("{hs:?}").contains(&status.to_string()));
        }
    }

    #[test]
    fn production_error_body_still_obeys_payload_bound() {
        use crate::identity_source::screenscraper::{
            ScreenScraperError, ScreenScraperTransport, UreqTransport,
        };
        let (url, worker) = server(503, b"123456789");
        assert_eq!(
            UreqTransport::default().get(&url, 8, Duration::from_secs(5)),
            Err(ScreenScraperError::ResponseTooLarge { limit: 8 })
        );
        worker.join().unwrap();
    }
    #[test]
    fn hasheous_error_body_obeys_existing_payload_limit() {
        use crate::identity_source::hasheous::client::{
            HasheousRequestError, HasheousTransport, MAX_RESPONSE_BYTES, UreqTransport,
        };
        let (url, worker) = server(503, &vec![b'x'; MAX_RESPONSE_BYTES + 1]);
        assert_eq!(
            UreqTransport::new(Duration::from_secs(5)).post_json_with_metadata(&url, b"{}"),
            Err(HasheousRequestError::ResponseTooLarge {
                limit: MAX_RESPONSE_BYTES
            })
        );
        worker.join().unwrap();
    }

    #[test]
    fn transport_diagnostics_do_not_echo_request_secrets() {
        use crate::identity_source::hasheous::client::{HasheousTransport, UreqTransport as Hs};
        use crate::identity_source::screenscraper::{ScreenScraperTransport, UreqTransport as Ss};
        let url = "invalid scheme://synthetic?password=fake-secret";
        let ss = format!(
            "{:?}",
            Ss::default()
                .get(url, 32, Duration::from_secs(5))
                .unwrap_err()
        );
        let hs = format!(
            "{:?}",
            Hs::default()
                .post_json_with_metadata(url, b"{}")
                .unwrap_err()
        );
        assert!(!ss.contains("fake-secret"));
        assert!(!hs.contains("fake-secret"));
    }
}
