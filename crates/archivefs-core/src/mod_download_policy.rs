//! Pure policy decisions for a future external mod-payload transport.
//!
//! This module performs no DNS lookup, filesystem access, socket access,
//! subprocess execution, or download. Hostname resolution and streamed byte
//! enforcement are represented as required future checks.

use std::net::{IpAddr, Ipv4Addr};

use serde::Serialize;
use url::Url;

use crate::mod_catalogue::{ModCatalogueHash, ModCatalogueHashAlgorithm};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModDownloadPolicyDecision {
    AllowedForTransport,
    RequiresReview,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModRedirectPolicy {
    HttpsOnly,
    AllowHttpsHostChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModDownloadHashClass {
    StrongExpectedHash,
    LegacyIntegrityHash,
    UnsupportedHash,
    NoExpectedHash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPayloadHostRelation {
    SameHost,
    DifferentHost,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModDownloadPolicyWarning {
    HttpTransport,
    DifferentPayloadHost,
    NoExpectedHash,
    LegacyHashOnly,
    ExecutableOrScriptPayload,
    DnsResolutionRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModDownloadPolicyBlocker {
    MalformedUrl,
    UnsupportedScheme,
    MissingHost,
    UserInfoNotAllowed,
    UnsafeAddressLiteral,
    NonStandardPort,
    HttpTransportNotAllowed,
    RedirectLimitExceeded,
    RedirectLoop,
    HttpsDowngrade,
    HostChangeNotAllowed,
    DeclaredSizeExceedsLimit,
    ContentLengthExceedsLimit,
    SizeOverflow,
    UnsupportedExpectedHash,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModDownloadPolicyInput {
    pub provider: String,
    pub provider_source_page_url: Option<String>,
    pub payload_url: String,
    pub redirects: Vec<String>,
    pub declared_size: Option<u64>,
    pub content_length: Option<u64>,
    pub hard_size_limit: u64,
    pub expected_hash: Option<ModCatalogueHash>,
    pub payload_filename: Option<String>,
    pub redirect_policy: ModRedirectPolicy,
    pub max_redirects: usize,
    pub allow_http_with_review: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModDownloadPolicyDecisionResult {
    pub decision: ModDownloadPolicyDecision,
    pub provider: String,
    pub initial_payload_host: Option<String>,
    pub final_payload_host: Option<String>,
    pub payload_host_relation: ModPayloadHostRelation,
    pub normalized_final_url: Option<String>,
    pub hash_class: ModDownloadHashClass,
    pub warnings: Vec<ModDownloadPolicyWarning>,
    pub blockers: Vec<ModDownloadPolicyBlocker>,
    pub required_checks: Vec<String>,
}

/// Shapes for a future fake or real transport. They contain no network logic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TransportResponseMetadata {
    pub status_code: u16,
    pub content_length: Option<u64>,
    pub location: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RedirectHop {
    pub url: String,
    pub response: TransportResponseMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TransferProgressObservation {
    pub bytes_received: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TransferCompletionMetadata {
    pub bytes_received: u64,
    pub sha256: String,
}

pub fn evaluate_mod_download_policy(
    input: &ModDownloadPolicyInput,
) -> ModDownloadPolicyDecisionResult {
    let mut result = ModDownloadPolicyDecisionResult {
        decision: ModDownloadPolicyDecision::AllowedForTransport,
        provider: input.provider.clone(),
        initial_payload_host: None,
        final_payload_host: None,
        payload_host_relation: ModPayloadHostRelation::Unknown,
        normalized_final_url: None,
        hash_class: classify_hash(input.expected_hash.as_ref()),
        warnings: Vec::new(),
        blockers: Vec::new(),
        required_checks: Vec::new(),
    };
    let Some(initial) = validate_url(&input.payload_url, &mut result, input) else {
        result.blockers.push(ModDownloadPolicyBlocker::MalformedUrl);
        return finish(result);
    };
    result.initial_payload_host = initial.host_str().map(str::to_owned);
    let provider_host = input
        .provider_source_page_url
        .as_deref()
        .and_then(|value| Url::parse(value).ok())
        .and_then(|url| url.host_str().map(str::to_owned));
    let mut previous = initial;
    let mut visited = vec![canonical_url(&previous)];
    if input.redirects.len() > input.max_redirects {
        result
            .blockers
            .push(ModDownloadPolicyBlocker::RedirectLimitExceeded);
    }
    for redirect in input.redirects.iter().take(input.max_redirects) {
        let Some(next) = validate_url(redirect, &mut result, input) else {
            result.blockers.push(ModDownloadPolicyBlocker::MalformedUrl);
            continue;
        };
        let canonical = canonical_url(&next);
        if visited.iter().any(|seen| seen == &canonical) {
            result.blockers.push(ModDownloadPolicyBlocker::RedirectLoop);
        }
        if previous.scheme() == "https" && next.scheme() == "http" {
            result
                .blockers
                .push(ModDownloadPolicyBlocker::HttpsDowngrade);
        }
        if previous.host_str() != next.host_str() {
            match input.redirect_policy {
                ModRedirectPolicy::HttpsOnly => result
                    .blockers
                    .push(ModDownloadPolicyBlocker::HostChangeNotAllowed),
                ModRedirectPolicy::AllowHttpsHostChange => result
                    .warnings
                    .push(ModDownloadPolicyWarning::DifferentPayloadHost),
            }
        }
        visited.push(canonical);
        previous = next;
    }
    result.final_payload_host = previous.host_str().map(str::to_owned);
    result.normalized_final_url = Some(canonical_url(&previous));
    result.payload_host_relation = match (&provider_host, &result.final_payload_host) {
        (Some(provider), Some(payload)) if provider.eq_ignore_ascii_case(payload) => {
            ModPayloadHostRelation::SameHost
        }
        (Some(_), Some(_)) => ModPayloadHostRelation::DifferentHost,
        _ => ModPayloadHostRelation::Unknown,
    };
    if result.payload_host_relation == ModPayloadHostRelation::DifferentHost {
        result
            .warnings
            .push(ModDownloadPolicyWarning::DifferentPayloadHost);
    }
    if input
        .declared_size
        .is_some_and(|size| size > input.hard_size_limit)
    {
        result
            .blockers
            .push(ModDownloadPolicyBlocker::DeclaredSizeExceedsLimit);
    }
    if input
        .content_length
        .is_some_and(|size| size > input.hard_size_limit)
    {
        result
            .blockers
            .push(ModDownloadPolicyBlocker::ContentLengthExceedsLimit);
    }
    if input.hard_size_limit == 0 {
        result.blockers.push(ModDownloadPolicyBlocker::SizeOverflow);
    }
    match result.hash_class {
        ModDownloadHashClass::NoExpectedHash => {
            result
                .warnings
                .push(ModDownloadPolicyWarning::NoExpectedHash);
            result.required_checks.extend(
                [
                    "calculate local SHA-256 after download",
                    "inspect payload before apply",
                    "obtain stronger user review before apply",
                ]
                .into_iter()
                .map(str::to_owned),
            );
        }
        ModDownloadHashClass::LegacyIntegrityHash => {
            result
                .warnings
                .push(ModDownloadPolicyWarning::LegacyHashOnly);
            result
                .required_checks
                .push("calculate local SHA-256 after download".into());
        }
        ModDownloadHashClass::StrongExpectedHash => result
            .required_checks
            .push("calculate SHA-256 and compare expected hash".into()),
        ModDownloadHashClass::UnsupportedHash => result
            .blockers
            .push(ModDownloadPolicyBlocker::UnsupportedExpectedHash),
    }
    if input
        .payload_filename
        .as_deref()
        .is_some_and(is_executable_like)
    {
        result
            .warnings
            .push(ModDownloadPolicyWarning::ExecutableOrScriptPayload);
        result
            .required_checks
            .push("never execute or load payload automatically".into());
    }
    result.required_checks.extend(
        [
            "enforce streamed byte ceiling",
            "inspect archive or patch safely",
            "recheck selected-game compatibility",
            "review before any apply transaction",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    finish(result)
}

fn validate_url(
    value: &str,
    result: &mut ModDownloadPolicyDecisionResult,
    input: &ModDownloadPolicyInput,
) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    match url.scheme() {
        "https" => {}
        "http" if input.allow_http_with_review => result
            .warnings
            .push(ModDownloadPolicyWarning::HttpTransport),
        "http" => {
            result
                .warnings
                .push(ModDownloadPolicyWarning::HttpTransport);
            result
                .blockers
                .push(ModDownloadPolicyBlocker::HttpTransportNotAllowed);
        }
        _ => result
            .blockers
            .push(ModDownloadPolicyBlocker::UnsupportedScheme),
    }
    if url.host_str().is_none() {
        result.blockers.push(ModDownloadPolicyBlocker::MissingHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        result
            .blockers
            .push(ModDownloadPolicyBlocker::UserInfoNotAllowed);
    }
    if let Some(port) = url.port() {
        let standard = match url.scheme() {
            "https" => 443,
            "http" => 80,
            _ => 0,
        };
        if port != standard {
            result
                .blockers
                .push(ModDownloadPolicyBlocker::NonStandardPort);
        }
    }
    if let Some(host) = url.host_str() {
        if host.eq_ignore_ascii_case("localhost") || is_unsafe_literal(host) {
            result
                .blockers
                .push(ModDownloadPolicyBlocker::UnsafeAddressLiteral);
        } else if host.parse::<IpAddr>().is_err() {
            result
                .warnings
                .push(ModDownloadPolicyWarning::DnsResolutionRequired);
            if result
                .required_checks
                .iter()
                .all(|check| !check.contains("DNS"))
            {
                result
                    .required_checks
                    .push("resolve DNS and validate every address before connection".into());
            }
        }
    }
    Some(url)
}

fn classify_hash(hash: Option<&ModCatalogueHash>) -> ModDownloadHashClass {
    let Some(hash) = hash else {
        return ModDownloadHashClass::NoExpectedHash;
    };
    let expected_len = match hash.algorithm {
        ModCatalogueHashAlgorithm::Sha256 => 64,
        ModCatalogueHashAlgorithm::Sha1 => 40,
        ModCatalogueHashAlgorithm::Md5 => 32,
        ModCatalogueHashAlgorithm::Unknown(_) => return ModDownloadHashClass::UnsupportedHash,
    };
    if hash.value.len() != expected_len || !hash.value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return ModDownloadHashClass::UnsupportedHash;
    }
    if matches!(hash.algorithm, ModCatalogueHashAlgorithm::Sha256) {
        ModDownloadHashClass::StrongExpectedHash
    } else {
        ModDownloadHashClass::LegacyIntegrityHash
    }
}

fn is_unsafe_literal(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    let Ok(address) = host.parse::<IpAddr>() else {
        return false;
    };
    is_forbidden_resolved_address(address)
}

/// Returns whether a resolved address is forbidden for external payload fetches.
/// This is shared by the pure literal check and the future transport resolver.
pub fn is_forbidden_resolved_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => unsafe_ipv4(address),
        IpAddr::V6(address) => {
            address
                .to_ipv4_mapped()
                .is_some_and(|mapped| unsafe_ipv4(mapped))
                || address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local()
                || address.is_multicast()
        }
    }
}

fn unsafe_ipv4(address: Ipv4Addr) -> bool {
    let value = u32::from(address);
    address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_broadcast()
        || address.is_multicast()
        || (value & 0xff00_0000) == 0
        || (value & 0xffff_ff00) == 0xc000_0200
        || (value & 0xffff_ff00) == 0xc633_6400
        || (value & 0xffff_ff00) == 0xcb00_7100
}

fn canonical_url(url: &Url) -> String {
    url.to_string()
}

fn is_executable_like(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    [
        ".exe", ".dll", ".bat", ".cmd", ".ps1", ".sh", ".py", ".elf", ".so", ".xex", ".sprx",
        ".prx",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn finish(mut result: ModDownloadPolicyDecisionResult) -> ModDownloadPolicyDecisionResult {
    result
        .warnings
        .sort_by_key(|warning| format!("{warning:?}"));
    result.warnings.dedup();
    result
        .blockers
        .sort_by_key(|blocker| format!("{blocker:?}"));
    result.blockers.dedup();
    result.required_checks.sort();
    result.required_checks.dedup();
    result.decision = if result.blockers.is_empty() {
        if result.warnings.is_empty() {
            ModDownloadPolicyDecision::AllowedForTransport
        } else {
            ModDownloadPolicyDecision::RequiresReview
        }
    } else {
        ModDownloadPolicyDecision::Blocked
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(url: &str) -> ModDownloadPolicyInput {
        ModDownloadPolicyInput {
            provider: "fixture".into(),
            provider_source_page_url: Some("https://catalogue.example.test/mod/1".into()),
            payload_url: url.into(),
            redirects: Vec::new(),
            declared_size: Some(10),
            content_length: Some(10),
            hard_size_limit: 1024,
            expected_hash: Some(ModCatalogueHash {
                algorithm: ModCatalogueHashAlgorithm::Sha256,
                value: "a".repeat(64),
            }),
            payload_filename: Some("patch.bps".into()),
            redirect_policy: ModRedirectPolicy::AllowHttpsHostChange,
            max_redirects: 5,
            allow_http_with_review: false,
        }
    }

    #[test]
    fn hostname_requires_dns_check_without_resolving() {
        let result = evaluate_mod_download_policy(&input("https://mods.example.test/a.zip"));
        assert_eq!(result.decision, ModDownloadPolicyDecision::RequiresReview);
        assert!(
            result
                .warnings
                .contains(&ModDownloadPolicyWarning::DnsResolutionRequired)
        );
    }
    #[test]
    fn literal_local_private_and_reserved_addresses_block() {
        for host in [
            "localhost",
            "127.0.0.1",
            "10.1.2.3",
            "172.16.1.1",
            "192.168.1.1",
            "169.254.1.1",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            let result = evaluate_mod_download_policy(&input(&format!("https://[{host}]/x")));
            assert_eq!(
                result.decision,
                ModDownloadPolicyDecision::Blocked,
                "{host}"
            );
        }
    }
    #[test]
    fn unsupported_schemes_block() {
        for scheme in ["file", "data", "javascript", "ftp", "custom"] {
            assert_eq!(
                evaluate_mod_download_policy(&input(&format!("{scheme}://example.test/x")))
                    .decision,
                ModDownloadPolicyDecision::Blocked
            );
        }
    }

    #[test]
    fn http_is_blocked_unless_explicitly_reviewed() {
        let mut fixture = input("http://mods.example.test/a.zip");
        assert!(
            evaluate_mod_download_policy(&fixture)
                .blockers
                .contains(&ModDownloadPolicyBlocker::HttpTransportNotAllowed)
        );
        fixture.allow_http_with_review = true;
        let result = evaluate_mod_download_policy(&fixture);
        assert_eq!(result.decision, ModDownloadPolicyDecision::RequiresReview);
        assert!(
            result
                .warnings
                .contains(&ModDownloadPolicyWarning::HttpTransport)
        );
    }
    #[test]
    fn redirects_are_revalidated() {
        let mut fixture = input("https://mods.example.test/a.zip");
        fixture.redirects = vec!["https://cdn.example.test/a.zip".into()];
        let result = evaluate_mod_download_policy(&fixture);
        assert_eq!(result.decision, ModDownloadPolicyDecision::RequiresReview);
        assert_eq!(
            result.payload_host_relation,
            ModPayloadHostRelation::DifferentHost
        );
        fixture.redirects = vec!["https://mods.example.test/a.zip".into()];
        assert!(
            evaluate_mod_download_policy(&fixture)
                .blockers
                .contains(&ModDownloadPolicyBlocker::RedirectLoop)
        );
    }
    #[test]
    fn redirect_limits_downgrades_and_private_targets_block() {
        let mut fixture = input("https://mods.example.test/a.zip");
        fixture.redirects = vec!["https://cdn.example.test/a".into(); 6];
        assert!(
            evaluate_mod_download_policy(&fixture)
                .blockers
                .contains(&ModDownloadPolicyBlocker::RedirectLimitExceeded)
        );
        fixture.redirects = vec!["http://cdn.example.test/a".into()];
        assert!(
            evaluate_mod_download_policy(&fixture)
                .blockers
                .contains(&ModDownloadPolicyBlocker::HttpsDowngrade)
        );
        fixture.redirects = vec!["https://127.0.0.1/a".into()];
        assert!(
            evaluate_mod_download_policy(&fixture)
                .blockers
                .contains(&ModDownloadPolicyBlocker::UnsafeAddressLiteral)
        );
    }
    #[test]
    fn size_hash_and_executable_rules_are_typed() {
        let mut fixture = input("https://mods.example.test/a.zip");
        fixture.declared_size = Some(1025);
        fixture.payload_filename = Some("tool.exe".into());
        let result = evaluate_mod_download_policy(&fixture);
        assert_eq!(result.decision, ModDownloadPolicyDecision::Blocked);
        assert!(
            result
                .warnings
                .contains(&ModDownloadPolicyWarning::ExecutableOrScriptPayload)
        );
        fixture.declared_size = Some(1);
        fixture.expected_hash = None;
        assert_eq!(
            evaluate_mod_download_policy(&fixture).hash_class,
            ModDownloadHashClass::NoExpectedHash
        );
    }
    #[test]
    fn legacy_and_bad_hashes_are_not_strong() {
        let mut fixture = input("https://mods.example.test/a.zip");
        fixture.expected_hash = Some(ModCatalogueHash {
            algorithm: ModCatalogueHashAlgorithm::Sha1,
            value: "a".repeat(40),
        });
        assert_eq!(
            evaluate_mod_download_policy(&fixture).hash_class,
            ModDownloadHashClass::LegacyIntegrityHash
        );
        fixture.expected_hash.as_mut().unwrap().value = "bad".into();
        assert_eq!(
            evaluate_mod_download_policy(&fixture).hash_class,
            ModDownloadHashClass::UnsupportedHash
        );
    }
    #[test]
    fn same_input_is_deterministic() {
        let fixture = input("https://mods.example.test/a.zip");
        assert_eq!(
            evaluate_mod_download_policy(&fixture),
            evaluate_mod_download_policy(&fixture)
        );
    }
}
