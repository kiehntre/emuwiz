//! A typed answer to "can this RomM instance be reached right now?".
//!
//! A provider that cannot be reached is not the same fact as a game that has no
//! artwork, so callers that show pictures need to tell the two apart. This module
//! only *classifies* failures the transport and endpoint policy already produce;
//! it performs no I/O, holds no address, and never sees a token. The configured
//! endpoint is whatever the person entered - nothing here names a host.

use super::client::RommRequestError;
use crate::identity_source::artwork::ArtworkRefusal;
use crate::identity_source::net_policy::EndpointRefusal;

/// Where a RomM source stands, as far as the last attempt showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RommConnectivity {
    /// No RomM address (or token) has been set up.
    NotConfigured,
    /// The source is configured but switched off.
    Disabled,
    /// A check is in progress.
    Resolving,
    /// The last request reached the instance.
    Reachable,
    /// The configured host name could not be resolved.
    DnsFailure,
    /// The host answered and refused the connection.
    ConnectionRefused,
    /// The connection could not be made for another reason (unreachable network,
    /// reset, no route).
    ConnectionFailed,
    /// The instance did not answer in time.
    Timeout,
    /// The secure connection could not be established.
    TlsFailure,
    /// The token is missing, unreadable, or was rejected.
    AuthenticationFailed,
    /// The instance answered with an error status.
    HttpError(u16),
    /// The instance asked us to slow down.
    RateLimited,
    /// The configured address was refused by the endpoint safety policy.
    EndpointRefused,
    /// A failure this classification cannot place.
    UnknownFailure,
}

impl RommConnectivity {
    /// The provider itself cannot serve pictures right now (as opposed to the
    /// picture being absent or undecodable). Only these states justify saying
    /// "RomM cannot currently be reached".
    pub fn is_unreachable(self) -> bool {
        matches!(
            self,
            Self::DnsFailure
                | Self::ConnectionRefused
                | Self::ConnectionFailed
                | Self::Timeout
                | Self::TlsFailure
        )
    }

    /// Whether asking again later can plausibly succeed without the person
    /// changing anything. Configuration and credential problems need a change.
    pub fn is_recoverable_by_waiting(self) -> bool {
        self.is_unreachable() || matches!(self, Self::RateLimited | Self::HttpError(500..=599))
    }

    /// A state in which requests should not be sent (yet).
    pub fn blocks_requests(self) -> bool {
        self.is_unreachable()
            || matches!(
                self,
                Self::NotConfigured
                    | Self::Disabled
                    | Self::AuthenticationFailed
                    | Self::EndpointRefused
            )
    }

    /// One plain sentence for the normal interface. No network jargon.
    pub fn plain_message(self) -> &'static str {
        match self {
            Self::NotConfigured => "RomM is not set up.",
            Self::Disabled => "Online artwork from RomM is switched off.",
            Self::Resolving => "Checking RomM…",
            Self::Reachable => "RomM is reachable.",
            Self::DnsFailure
            | Self::ConnectionRefused
            | Self::ConnectionFailed
            | Self::Timeout
            | Self::TlsFailure => "RomM cannot currently be reached.",
            Self::AuthenticationFailed => {
                "RomM needs attention. Check its connection in Sources & Providers."
            }
            Self::HttpError(_) | Self::RateLimited => "RomM is not answering properly right now.",
            Self::EndpointRefused => {
                "The RomM address was not approved. Check it in Sources & Providers."
            }
            Self::UnknownFailure => "RomM could not be used right now.",
        }
    }

    /// A short technical label for Advanced details.
    pub fn technical_label(self) -> String {
        match self {
            Self::NotConfigured => "not configured".into(),
            Self::Disabled => "disabled".into(),
            Self::Resolving => "resolving".into(),
            Self::Reachable => "reachable".into(),
            Self::DnsFailure => "DNS failure (the host name could not be resolved)".into(),
            Self::ConnectionRefused => "connection refused".into(),
            Self::ConnectionFailed => "connection failed".into(),
            Self::Timeout => "timeout".into(),
            Self::TlsFailure => "TLS failure".into(),
            Self::AuthenticationFailed => "authentication required or failed".into(),
            Self::HttpError(status) => format!("HTTP error {status}"),
            Self::RateLimited => "rate limited".into(),
            Self::EndpointRefused => "endpoint refused by the safety policy".into(),
            Self::UnknownFailure => "unknown failure".into(),
        }
    }

    pub fn from_endpoint_refusal(refusal: &EndpointRefusal) -> Self {
        match refusal {
            EndpointRefusal::UnresolvableHost { .. } | EndpointRefusal::NoAddresses => {
                Self::DnsFailure
            }
            EndpointRefusal::MissingHost => Self::NotConfigured,
            _ => Self::EndpointRefused,
        }
    }

    pub fn from_request_error(error: &RommRequestError) -> Option<Self> {
        Some(match error {
            RommRequestError::Endpoint(refusal) => Self::from_endpoint_refusal(refusal),
            RommRequestError::Unauthorised { .. } => Self::AuthenticationFailed,
            RommRequestError::HttpStatus { status } => Self::HttpError(*status),
            RommRequestError::RateLimited { .. } => Self::RateLimited,
            RommRequestError::Transport { detail } => classify_transport_detail(detail),
            RommRequestError::Timeout => Self::Timeout,
            // Not facts about the provider's reachability.
            RommRequestError::Cancelled
            | RommRequestError::ResponseTooLarge { .. }
            | RommRequestError::MalformedResponse { .. } => return None,
        })
    }

    /// `None` when the refusal says nothing about reachability (no artwork,
    /// an undecodable image, a full disk, a cancelled request).
    pub fn from_artwork_refusal(refusal: &ArtworkRefusal) -> Option<Self> {
        match refusal {
            ArtworkRefusal::Endpoint(refusal) => Some(Self::from_endpoint_refusal(refusal)),
            ArtworkRefusal::Request(error) => Self::from_request_error(error),
            _ => None,
        }
    }
}

/// Places a transport classification (the short text `client.rs` produces) into
/// a state. Kept beside the type so a change to the wording is caught by the
/// tests that feed real transport errors through both.
pub fn classify_transport_detail(detail: &str) -> RommConnectivity {
    let text = detail.to_ascii_lowercase();
    if text.contains("could not be found") || text.contains("could not be resolved") {
        RommConnectivity::DnsFailure
    } else if text.contains("refused") {
        RommConnectivity::ConnectionRefused
    } else if text.contains("timed out") || text.contains("timeout") {
        RommConnectivity::Timeout
    } else if text.contains("tls") {
        RommConnectivity::TlsFailure
    } else if text.contains("connection failed")
        || text.contains("the connection")
        || text.contains("unreachable")
        || text.contains("reset")
        || text.contains("broken pipe")
        || text.contains("i/o error")
    {
        RommConnectivity::ConnectionFailed
    } else {
        RommConnectivity::UnknownFailure
    }
}

/// The address to show in Advanced details: exactly what was configured, minus
/// any user-information part so a credential can never be displayed.
pub fn displayable_endpoint(configured: &str) -> String {
    let trimmed = configured.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_string();
    };
    let (authority, tail) = match rest.find('/') {
        Some(position) => rest.split_at(position),
        None => (rest, ""),
    };
    let host = authority.rsplit('@').next().unwrap_or(authority);
    format!("{scheme}://{host}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_failures_are_told_apart_from_other_network_failures() {
        let dns = RommRequestError::Transport {
            detail: "the host could not be found".into(),
        };
        let refused = RommRequestError::Transport {
            detail: "an I/O error occurred (connection refused)".into(),
        };
        let generic = RommRequestError::Transport {
            detail: "the connection failed".into(),
        };
        assert_eq!(
            RommConnectivity::from_request_error(&dns),
            Some(RommConnectivity::DnsFailure)
        );
        assert_eq!(
            RommConnectivity::from_request_error(&refused),
            Some(RommConnectivity::ConnectionRefused)
        );
        assert_eq!(
            RommConnectivity::from_request_error(&generic),
            Some(RommConnectivity::ConnectionFailed)
        );
        assert_eq!(
            RommConnectivity::from_request_error(&RommRequestError::Timeout),
            Some(RommConnectivity::Timeout)
        );
    }

    #[test]
    fn a_real_transport_error_classification_still_round_trips() {
        // Feeds the transport's own wording through the classifier.
        for (error, expected) in [
            (ureq::Error::HostNotFound, RommConnectivity::DnsFailure),
            (
                ureq::Error::ConnectionFailed,
                RommConnectivity::ConnectionFailed,
            ),
            (
                ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionRefused)),
                RommConnectivity::ConnectionRefused,
            ),
            (
                ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::TimedOut)),
                RommConnectivity::Timeout,
            ),
        ] {
            let detail = crate::identity_source::romm::client::classify_transport_error(&error);
            assert_eq!(classify_transport_detail(&detail), expected, "{detail}");
        }
    }

    #[test]
    fn http_and_authentication_failures_are_not_reachability_failures() {
        let auth =
            RommConnectivity::from_request_error(&RommRequestError::Unauthorised { status: 401 });
        assert_eq!(auth, Some(RommConnectivity::AuthenticationFailed));
        assert!(!RommConnectivity::AuthenticationFailed.is_unreachable());
        let http =
            RommConnectivity::from_request_error(&RommRequestError::HttpStatus { status: 503 })
                .unwrap();
        assert_eq!(http, RommConnectivity::HttpError(503));
        assert!(!http.is_unreachable());
        assert!(http.is_recoverable_by_waiting());
        assert!(!RommConnectivity::HttpError(404).is_recoverable_by_waiting());
    }

    #[test]
    fn facts_that_say_nothing_about_reachability_return_none() {
        assert_eq!(
            RommConnectivity::from_artwork_refusal(&ArtworkRefusal::NoArtwork),
            None
        );
        assert_eq!(
            RommConnectivity::from_artwork_refusal(&ArtworkRefusal::DecodeFailed),
            None
        );
        assert_eq!(
            RommConnectivity::from_artwork_refusal(&ArtworkRefusal::Cancelled),
            None
        );
    }

    #[test]
    fn an_unresolvable_endpoint_is_a_dns_failure_whatever_the_host_name() {
        for host in ["romm.example.com", "romm.local", "my-nas", "romm.saltbox"] {
            let refusal = EndpointRefusal::UnresolvableHost {
                detail: format!("failed to lookup address information for {host}"),
            };
            assert_eq!(
                RommConnectivity::from_endpoint_refusal(&refusal),
                RommConnectivity::DnsFailure
            );
        }
    }

    #[test]
    fn only_unreachable_states_claim_the_server_cannot_be_reached() {
        for state in [
            RommConnectivity::DnsFailure,
            RommConnectivity::ConnectionRefused,
            RommConnectivity::ConnectionFailed,
            RommConnectivity::Timeout,
            RommConnectivity::TlsFailure,
        ] {
            assert!(state.is_unreachable());
            assert_eq!(state.plain_message(), "RomM cannot currently be reached.");
        }
        assert!(!RommConnectivity::Reachable.is_unreachable());
        assert!(!RommConnectivity::NotConfigured.is_unreachable());
        assert!(
            !RommConnectivity::AuthenticationFailed
                .plain_message()
                .contains("reached")
        );
    }

    #[test]
    fn a_displayed_endpoint_never_shows_user_information() {
        assert_eq!(
            displayable_endpoint("http://192.168.1.5:8080"),
            "http://192.168.1.5:8080"
        );
        assert_eq!(
            displayable_endpoint("https://romm.example.com/base"),
            "https://romm.example.com/base"
        );
        assert_eq!(
            displayable_endpoint("http://user:pw@romm.local:80/x"),
            "http://romm.local:80/x"
        );
        assert_eq!(displayable_endpoint(" romm.local "), "romm.local");
    }
}
