//! Finding a RomM that runs in a *local* Docker engine, when the configured
//! address cannot be reached.
//!
//! The configured endpoint always gets the first chance. Only a transport
//! failure (DNS, refused, no route, timeout) ever starts a discovery; an
//! authentication or protocol failure is a different problem and never switches
//! servers. A discovered endpoint is used for the session only: nothing here
//! writes configuration, and the address of a bridge-network container is never
//! used or stored - only a port Docker publishes on the host.
//!
//! Docker is asked through the existing `docker` CLI with a fixed argument list
//! (no shell), a deadline and an output ceiling. It is read-only: `context
//! inspect`, `ps` and `inspect`. No sudo, no `exec`, no network changes.

use std::collections::BTreeSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::client::{RommClient, RommRequestError, RommTransport};
use super::config::{RommSourceConfig, RommToken, ValidatedRommSource};
use super::connectivity::RommConnectivity;
use crate::identity_source::net_policy::HostResolver;

const DOCKER_DEADLINE: Duration = Duration::from_secs(6);
const DOCKER_OUTPUT_CEILING: usize = 4 * 1024 * 1024;
const MAX_CONTAINERS: usize = 64;
/// How long a failed or empty discovery is remembered, so a broken setup costs
/// one `docker` call per window rather than one per picture.
pub const REDISCOVERY_COOLDOWN: Duration = Duration::from_secs(60);
/// RomM listens on this port inside its container unless `ROMM_PORT` says so.
const ROMM_CONTAINER_PORT: u16 = 8080;

/// Why Docker could not be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DockerFailure {
    NotInstalled,
    PermissionDenied,
    Failed,
    TimedOut,
    TooMuchOutput,
}

/// Runs one read-only `docker` command and returns its stdout.
pub trait DockerCli: Send + Sync {
    fn run(&self, args: &[&str]) -> Result<String, DockerFailure>;
}

/// The real `docker` binary, bounded by a deadline and an output ceiling.
#[derive(Debug, Default)]
pub struct SystemDocker;

impl DockerCli for SystemDocker {
    fn run(&self, args: &[&str]) -> Result<String, DockerFailure> {
        let mut child = Command::new("docker")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => DockerFailure::NotInstalled,
                std::io::ErrorKind::PermissionDenied => DockerFailure::PermissionDenied,
                _ => DockerFailure::Failed,
            })?;
        let mut out = child.stdout.take();
        let mut err = child.stderr.take();
        let out_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(out) = out.as_mut() {
                let _ = out
                    .take(DOCKER_OUTPUT_CEILING as u64 + 1)
                    .read_to_end(&mut bytes);
            }
            bytes
        });
        let err_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(err) = err.as_mut() {
                let _ = err.take(16 * 1024).read_to_end(&mut bytes);
            }
            bytes
        });
        let deadline = Instant::now() + DOCKER_DEADLINE;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(15))
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DockerFailure::TimedOut);
                }
                Err(_) => return Err(DockerFailure::Failed),
            }
        };
        let stdout = out_reader.join().unwrap_or_default();
        let stderr = err_reader.join().unwrap_or_default();
        if !status.success() {
            let text = String::from_utf8_lossy(&stderr).to_ascii_lowercase();
            return Err(if text.contains("permission denied") {
                DockerFailure::PermissionDenied
            } else {
                DockerFailure::Failed
            });
        }
        if stdout.len() > DOCKER_OUTPUT_CEILING {
            return Err(DockerFailure::TooMuchOutput);
        }
        String::from_utf8(stdout).map_err(|_| DockerFailure::Failed)
    }
}

/// How a RomM container could be reached from this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Reach {
    /// Docker publishes the RomM port on the host.
    Published { host: String, port: u16 },
    /// The container uses the host network and RomM's port is known.
    HostNetwork { port: u16 },
    /// Running, but not published to the host (only a bridge-network address
    /// exists, which is deliberately never used).
    NotPublished,
    /// Published on IPv6 only, which is not supported here.
    Ipv6Only,
    /// Host network, but RomM's port is not known.
    HostNetworkPortUnknown,
}

/// A container with positive evidence of being RomM. No token, no environment
/// and no container IP is ever kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalRommCandidate {
    pub container: String,
    pub image: String,
    pub network_mode: String,
    pub reach: Reach,
}

impl LocalRommCandidate {
    /// The host URL to try, or `None` when there is no safe one.
    pub fn url(&self) -> Option<String> {
        match &self.reach {
            Reach::Published { host, port } => Some(format!("http://{host}:{port}")),
            Reach::HostNetwork { port } => Some(format!("http://127.0.0.1:{port}")),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum DiscoveryOutcome {
    DockerUnavailable(DockerFailure),
    /// The active engine is SSH/TCP/cloud: never auto-discovered.
    RemoteEngineRefused,
    Candidates(Vec<LocalRommCandidate>),
    /// Docker answered with something that could not be understood.
    Malformed,
}

/// `unix://` is the only local engine. Everything else (ssh, tcp, npipe to a
/// remote, cloud) is refused.
fn is_local_engine(host: &str) -> bool {
    host.trim().starts_with("unix://")
}

/// Positive evidence only: the image repository, or the image's own source
/// label. A container name or a compose service name proves nothing.
fn romm_evidence(image: &str, labels: &serde_json::Value) -> bool {
    let repository = image
        .split('@')
        .next()
        .unwrap_or("")
        .rsplit_once(':')
        .filter(|(_, tag)| !tag.contains('/'))
        .map_or_else(|| image.split('@').next().unwrap_or(""), |(repo, _)| repo)
        .to_ascii_lowercase();
    if repository == "rommapp/romm" || repository.ends_with("/rommapp/romm") {
        return true;
    }
    labels
        .get("org.opencontainers.image.source")
        .and_then(|value| value.as_str())
        .is_some_and(|source| {
            let source = source.trim_end_matches('/').trim_end_matches(".git");
            source
                .to_ascii_lowercase()
                .ends_with("github.com/rommapp/romm")
        })
}

fn reach_of(container: &serde_json::Value, configured_port: Option<u16>) -> Reach {
    let mode = container["HostConfig"]["NetworkMode"]
        .as_str()
        .unwrap_or("");
    if mode == "host" {
        let from_env = container["Config"]["Env"]
            .as_array()
            .and_then(|env| {
                env.iter()
                    .filter_map(|item| item.as_str())
                    .find_map(|item| item.strip_prefix("ROMM_PORT="))
            })
            .and_then(|port| port.trim().parse::<u16>().ok());
        return match from_env.or(configured_port) {
            Some(port) if port != 0 => Reach::HostNetwork { port },
            _ => Reach::HostNetworkPortUnknown,
        };
    }
    let key = format!("{ROMM_CONTAINER_PORT}/tcp");
    let Some(bindings) = container["NetworkSettings"]["Ports"][key.as_str()].as_array() else {
        return Reach::NotPublished;
    };
    let mut ipv6_only = false;
    let mut best: Option<(String, u16)> = None;
    for binding in bindings {
        let (Some(ip), Some(port)) = (
            binding["HostIp"].as_str(),
            binding["HostPort"]
                .as_str()
                .and_then(|port| port.parse::<u16>().ok()),
        ) else {
            continue;
        };
        if port == 0 {
            continue;
        }
        let host = match ip {
            "" | "0.0.0.0" | "127.0.0.1" => "127.0.0.1".to_string(),
            ip if ip.contains(':') => {
                ipv6_only = true;
                continue;
            }
            ip => ip.to_string(),
        };
        // Prefer loopback when several bindings exist.
        if best.is_none() || host == "127.0.0.1" {
            best = Some((host, port));
        }
    }
    match best {
        Some((host, port)) => Reach::Published { host, port },
        None if ipv6_only => Reach::Ipv6Only,
        None => Reach::NotPublished,
    }
}

/// Asks the local engine which running containers are RomM and how each could be
/// reached from the host.
pub fn discover_candidates(
    docker: &dyn DockerCli,
    docker_host_env: Option<&str>,
    configured_port: Option<u16>,
) -> DiscoveryOutcome {
    if let Some(host) = docker_host_env.filter(|host| !host.trim().is_empty())
        && !is_local_engine(host)
    {
        return DiscoveryOutcome::RemoteEngineRefused;
    }
    let context = match docker.run(&[
        "context",
        "inspect",
        "--format",
        "{{.Endpoints.docker.Host}}",
    ]) {
        Ok(text) => text,
        Err(failure) => return DiscoveryOutcome::DockerUnavailable(failure),
    };
    if !is_local_engine(&context) {
        return DiscoveryOutcome::RemoteEngineRefused;
    }
    let listing = match docker.run(&["ps", "--no-trunc", "--format", "{{.ID}}"]) {
        Ok(text) => text,
        Err(failure) => return DiscoveryOutcome::DockerUnavailable(failure),
    };
    let ids: Vec<&str> = listing
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.chars().all(|ch| ch.is_ascii_hexdigit()))
        .take(MAX_CONTAINERS)
        .collect();
    if ids.is_empty() {
        return DiscoveryOutcome::Candidates(Vec::new());
    }
    let mut args = vec!["inspect", "--type", "container"];
    args.extend(ids);
    let inspected = match docker.run(&args) {
        Ok(text) => text,
        Err(failure) => return DiscoveryOutcome::DockerUnavailable(failure),
    };
    let Ok(serde_json::Value::Array(containers)) = serde_json::from_str(&inspected) else {
        return DiscoveryOutcome::Malformed;
    };
    let mut candidates = Vec::new();
    for container in &containers {
        if container["State"]["Running"].as_bool() != Some(true) {
            continue;
        }
        let image = container["Config"]["Image"].as_str().unwrap_or("");
        if !romm_evidence(image, &container["Config"]["Labels"]) {
            continue;
        }
        candidates.push(LocalRommCandidate {
            container: container["Name"]
                .as_str()
                .unwrap_or("")
                .trim_start_matches('/')
                .to_string(),
            // Tag and digest are dropped; only the repository is kept.
            image: image.split('@').next().unwrap_or("").to_string(),
            network_mode: container["HostConfig"]["NetworkMode"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            reach: reach_of(container, configured_port),
        });
    }
    DiscoveryOutcome::Candidates(candidates)
}

/// What asking one address "are you RomM?" showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    Romm,
    NotRomm,
    Unreachable,
}

/// The smallest RomM-specific check: the token-free heartbeat through the
/// existing client, which must report `SYSTEM.VERSION`. The token is only
/// needed to construct the validated source; the heartbeat never sends it.
pub fn verify_romm(
    url: &str,
    token: &RommToken,
    trusted_roots: &[std::path::PathBuf],
    resolver: &impl HostResolver,
    transport: &impl RommTransport,
) -> Verification {
    let config = RommSourceConfig {
        enabled: true,
        url: url.to_string(),
        ..RommSourceConfig::default()
    };
    let Ok(source) = ValidatedRommSource::validate(&config, token, trusted_roots, resolver) else {
        return Verification::Unreachable;
    };
    match RommClient::new(&source, transport).heartbeat(None) {
        Ok(_) => Verification::Romm,
        Err(RommRequestError::Transport { .. } | RommRequestError::Timeout) => {
            Verification::Unreachable
        }
        Err(_) => Verification::NotRomm,
    }
}

/// Validates the endpoint requests should use. The configured endpoint is
/// always tried first; only a DNS failure while validating it starts a local
/// Docker discovery, and a verified result is used for the session without
/// touching `config`. `endpoints` is `None` where discovery is not wanted.
pub fn validate_with_local_fallback(
    endpoints: Option<&EndpointSession>,
    config: &RommSourceConfig,
    token: &RommToken,
    trusted_roots: &[std::path::PathBuf],
    resolver: &impl HostResolver,
    transport: &impl RommTransport,
) -> Result<ValidatedRommSource, super::config::ConfigRefusal> {
    use crate::identity_source::net_policy::EndpointRefusal;
    let Some(session) = endpoints else {
        return ValidatedRommSource::validate(config, token, trusted_roots, resolver);
    };
    let configured = config.url.trim().to_string();
    let mut effective = config.clone();
    effective.url = session.effective(&configured).effective_endpoint;
    let first = ValidatedRommSource::validate(&effective, token, trusted_roots, resolver);
    let dns = matches!(
        &first,
        Err(super::config::ConfigRefusal::Endpoint(
            EndpointRefusal::UnresolvableHost { .. } | EndpointRefusal::NoAddresses
        ))
    );
    if !dns {
        return first;
    }
    if session.effective(&configured).endpoint_source == EndpointSource::LocalDockerFallback {
        session.invalidate();
    }
    let recovered = session.recover(&configured, RommConnectivity::DnsFailure, &mut |url| {
        verify_romm(url, token, trusted_roots, resolver, transport)
    });
    if recovered.endpoint_source == EndpointSource::LocalDockerFallback {
        effective.url = recovered.effective_endpoint;
        return ValidatedRommSource::validate(&effective, token, trusted_roots, resolver);
    }
    first
}

/// One discovery memory for the whole process.
pub fn shared_session() -> &'static EndpointSession {
    static SESSION: std::sync::OnceLock<EndpointSession> = std::sync::OnceLock::new();
    SESSION.get_or_init(EndpointSession::system)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EndpointSource {
    Configured,
    LocalDockerFallback,
}

/// The endpoint requests should use right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveEndpoint {
    pub configured_endpoint: String,
    pub effective_endpoint: String,
    pub endpoint_source: EndpointSource,
}

/// What the interface can say about local RomM discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum LocalRommStatus {
    /// "RomM found locally. Connected automatically through Docker."
    ConnectedAutomatically { endpoint: String },
    /// "RomM is running in Docker, but it is not published to the host."
    FoundNotHostAccessible { reach: Reach },
    /// "Several RomM servers were found on this computer." (for a later choice)
    MultipleFound { endpoints: Vec<String> },
    /// Found, but did not answer like RomM.
    FoundButNotVerified,
    /// No RomM container, no Docker, or a remote engine.
    NoneFound,
}

struct State {
    configured: String,
    fallback: Option<String>,
    status: Option<LocalRommStatus>,
    last_attempt: Option<Instant>,
}

/// Session-scoped memory of a discovered endpoint. Nothing is persisted.
pub struct EndpointSession {
    docker: Box<dyn DockerCli>,
    docker_host_env: Option<String>,
    state: Mutex<State>,
}

impl EndpointSession {
    pub fn new(docker: Box<dyn DockerCli>, docker_host_env: Option<String>) -> Self {
        Self {
            docker,
            docker_host_env,
            state: Mutex::new(State {
                configured: String::new(),
                fallback: None,
                status: None,
                last_attempt: None,
            }),
        }
    }

    /// The real engine, honouring `DOCKER_HOST` only to refuse a remote one.
    pub fn system() -> Self {
        Self::new(Box::new(SystemDocker), std::env::var("DOCKER_HOST").ok())
    }

    /// Cheap and docker-free: the configured endpoint, or the session fallback
    /// when one was verified for this same configured endpoint.
    pub fn effective(&self, configured: &str) -> EffectiveEndpoint {
        let fallback = self.state.lock().ok().and_then(|state| {
            (state.configured == configured)
                .then(|| state.fallback.clone())
                .flatten()
        });
        match fallback {
            Some(url) => EffectiveEndpoint {
                configured_endpoint: configured.to_string(),
                effective_endpoint: url,
                endpoint_source: EndpointSource::LocalDockerFallback,
            },
            None => EffectiveEndpoint {
                configured_endpoint: configured.to_string(),
                effective_endpoint: configured.to_string(),
                endpoint_source: EndpointSource::Configured,
            },
        }
    }

    pub fn status(&self) -> Option<LocalRommStatus> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.status.clone())
    }

    /// The fallback itself failed: forget it and allow exactly one rediscovery.
    pub fn invalidate(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.fallback = None;
            state.last_attempt = None;
        }
    }

    /// Called after the *configured* endpoint failed with `cause`. Only a
    /// transport failure may start discovery, and a discovery is not repeated
    /// within [`REDISCOVERY_COOLDOWN`]. `verify` asks one URL "are you RomM?".
    pub fn recover(
        &self,
        configured: &str,
        cause: RommConnectivity,
        verify: &mut dyn FnMut(&str) -> Verification,
    ) -> EffectiveEndpoint {
        let transport_failure = matches!(
            cause,
            RommConnectivity::DnsFailure
                | RommConnectivity::ConnectionRefused
                | RommConnectivity::ConnectionFailed
                | RommConnectivity::Timeout
        );
        if !transport_failure {
            return self.effective(configured);
        }
        {
            let Ok(mut state) = self.state.lock() else {
                return self.effective(configured);
            };
            if state.configured != configured {
                *state = State {
                    configured: configured.to_string(),
                    fallback: None,
                    status: None,
                    last_attempt: None,
                };
            }
            if state.fallback.is_some()
                || state
                    .last_attempt
                    .is_some_and(|at| at.elapsed() < REDISCOVERY_COOLDOWN)
            {
                drop(state);
                return self.effective(configured);
            }
            state.last_attempt = Some(Instant::now());
        }
        let configured_port = configured
            .rsplit_once(':')
            .and_then(|(_, port)| port.trim_end_matches('/').parse::<u16>().ok());
        let (status, fallback) = self.discover(configured_port, verify);
        if let Ok(mut state) = self.state.lock() {
            state.status = Some(status);
            state.fallback = fallback;
        }
        self.effective(configured)
    }

    fn discover(
        &self,
        configured_port: Option<u16>,
        verify: &mut dyn FnMut(&str) -> Verification,
    ) -> (LocalRommStatus, Option<String>) {
        let outcome = discover_candidates(
            self.docker.as_ref(),
            self.docker_host_env.as_deref(),
            configured_port,
        );
        let candidates = match outcome {
            DiscoveryOutcome::Candidates(candidates) => candidates,
            _ => return (LocalRommStatus::NoneFound, None),
        };
        let mut verified: BTreeSet<String> = BTreeSet::new();
        let mut unverified = false;
        let mut blocked: Option<Reach> = None;
        for candidate in &candidates {
            match candidate.url() {
                Some(url) => match verify(&url) {
                    Verification::Romm => {
                        verified.insert(url);
                    }
                    _ => unverified = true,
                },
                None => blocked = blocked.or(Some(candidate.reach.clone())),
            }
        }
        match verified.len() {
            1 => {
                let endpoint = verified.into_iter().next().unwrap_or_default();
                (
                    LocalRommStatus::ConnectedAutomatically {
                        endpoint: endpoint.clone(),
                    },
                    Some(endpoint),
                )
            }
            0 => (
                match (blocked, unverified) {
                    (Some(reach), _) => LocalRommStatus::FoundNotHostAccessible { reach },
                    (None, true) => LocalRommStatus::FoundButNotVerified,
                    (None, false) => LocalRommStatus::NoneFound,
                },
                None,
            ),
            _ => {
                // Only the configured port may break a tie; anything else is a
                // guess, so it fails closed.
                let matching: Vec<&String> = verified
                    .iter()
                    .filter(|url| {
                        url.rsplit_once(':')
                            .and_then(|(_, port)| port.parse::<u16>().ok())
                            == configured_port
                    })
                    .collect();
                if let [only] = matching.as_slice() {
                    let endpoint = (*only).clone();
                    return (
                        LocalRommStatus::ConnectedAutomatically {
                            endpoint: endpoint.clone(),
                        },
                        Some(endpoint),
                    );
                }
                (
                    LocalRommStatus::MultipleFound {
                        endpoints: verified.into_iter().collect(),
                    },
                    None,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::net_policy::StaticResolver;
    use crate::identity_source::romm::client::RommHttpResponse;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    const SECRET: &str = "tok-SECRET-1234567890";

    /// Replays canned docker answers keyed by the first one or two arguments.
    struct FakeDocker {
        answers: HashMap<String, Result<String, DockerFailure>>,
        calls: StdMutex<Vec<Vec<String>>>,
    }
    impl FakeDocker {
        fn new() -> Self {
            Self {
                answers: HashMap::new(),
                calls: StdMutex::new(Vec::new()),
            }
        }
        fn with(mut self, key: &str, answer: Result<String, DockerFailure>) -> Self {
            self.answers.insert(key.to_string(), answer);
            self
        }
        fn local(self, containers: serde_json::Value) -> Self {
            self.with("context", Ok("unix:///var/run/docker.sock\n".into()))
                .with("ps", Ok("aaaa\nbbbb\n".into()))
                .with("inspect", Ok(containers.to_string()))
        }
        fn calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }
    impl DockerCli for FakeDocker {
        fn run(&self, args: &[&str]) -> Result<String, DockerFailure> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|arg| arg.to_string()).collect());
            self.answers
                .get(args[0])
                .cloned()
                .unwrap_or(Err(DockerFailure::Failed))
        }
    }

    fn container(
        name: &str,
        image: &str,
        mode: &str,
        ports: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "Name": format!("/{name}"),
            "State": {"Running": true},
            "Config": {"Image": image, "Labels": {}, "Env": [format!("SECRET_KEY={SECRET}"), "ROMM_PORT=8080"]},
            "HostConfig": {"NetworkMode": mode},
            "NetworkSettings": {"Ports": ports, "Networks": {"saltbox": {"IPAddress": "172.19.0.24"}}}
        })
    }
    fn published(host_ip: &str, host_port: &str) -> serde_json::Value {
        serde_json::json!({"8080/tcp": [{"HostIp": host_ip, "HostPort": host_port}]})
    }
    fn romm_published(host_port: &str) -> serde_json::Value {
        container(
            "romm",
            "rommapp/romm:5.3.1",
            "bridge",
            published("0.0.0.0", host_port),
        )
    }
    fn outcome(docker: &FakeDocker) -> DiscoveryOutcome {
        discover_candidates(docker, None, Some(8080))
    }
    fn candidates(docker: &FakeDocker) -> Vec<LocalRommCandidate> {
        match outcome(docker) {
            DiscoveryOutcome::Candidates(list) => list,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_published_host_port_is_found_and_the_container_ip_is_never_used() {
        let docker = FakeDocker::new().local(serde_json::json!([romm_published("9090")]));
        let found = candidates(&docker);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].url().as_deref(), Some("http://127.0.0.1:9090"));
        let rendered = format!("{found:?}");
        assert!(!rendered.contains("172.19"), "{rendered}");
        assert!(!rendered.contains(SECRET), "{rendered}");
    }

    #[test]
    fn loopback_and_specific_host_bindings_are_used_as_published() {
        let loop_only = container(
            "romm",
            "rommapp/romm:5",
            "bridge",
            published("127.0.0.1", "18080"),
        );
        let docker = FakeDocker::new().local(serde_json::json!([loop_only]));
        assert_eq!(
            candidates(&docker)[0].url().as_deref(),
            Some("http://127.0.0.1:18080")
        );
    }

    #[test]
    fn an_unpublished_container_gives_a_truthful_blocker_not_its_bridge_address() {
        let hidden = container(
            "romm",
            "rommapp/romm:5.3.1",
            "saltbox",
            serde_json::json!({"6379/tcp": null, "8080/tcp": null}),
        );
        let docker = FakeDocker::new().local(serde_json::json!([hidden]));
        let found = candidates(&docker);
        assert_eq!(found[0].reach, Reach::NotPublished);
        assert_eq!(found[0].url(), None);
        let session = EndpointSession::new(Box::new(docker), None);
        let effective = session.recover(
            "http://romm.saltbox:8080",
            RommConnectivity::DnsFailure,
            &mut |_| panic!("nothing to verify"),
        );
        assert_eq!(effective.endpoint_source, EndpointSource::Configured);
        assert_eq!(
            session.status(),
            Some(LocalRommStatus::FoundNotHostAccessible {
                reach: Reach::NotPublished
            })
        );
    }

    #[test]
    fn ipv6_only_publication_is_reported_unsupported() {
        let v6 = container("romm", "rommapp/romm", "bridge", published("::", "8080"));
        let docker = FakeDocker::new().local(serde_json::json!([v6]));
        assert_eq!(candidates(&docker)[0].reach, Reach::Ipv6Only);
    }

    #[test]
    fn a_container_name_or_port_alone_is_not_romm() {
        let decoy = container(
            "romm",
            "nginx:latest",
            "bridge",
            published("0.0.0.0", "8080"),
        );
        let docker = FakeDocker::new().local(serde_json::json!([decoy]));
        assert!(candidates(&docker).is_empty());
    }

    #[test]
    fn the_image_source_label_is_accepted_when_the_image_is_a_bare_id() {
        let mut labelled = container("x", "sha256:abc", "bridge", published("0.0.0.0", "8080"));
        labelled["Config"]["Labels"] = serde_json::json!({
            "org.opencontainers.image.source": "https://github.com/rommapp/romm"
        });
        let docker = FakeDocker::new().local(serde_json::json!([labelled]));
        assert_eq!(candidates(&docker).len(), 1);
        assert!(!romm_evidence("rommapp/romm-fork", &serde_json::json!({})));
        assert!(romm_evidence(
            "ghcr.io/rommapp/romm:latest",
            &serde_json::json!({})
        ));
        assert!(romm_evidence(
            "localhost:5000/rommapp/romm",
            &serde_json::json!({})
        ));
    }

    #[test]
    fn host_network_needs_a_known_port() {
        let host = container("romm", "rommapp/romm", "host", serde_json::json!({}));
        let docker = FakeDocker::new().local(serde_json::json!([host]));
        assert_eq!(
            candidates(&docker)[0].url().as_deref(),
            Some("http://127.0.0.1:8080")
        );
        let mut unknown = container("romm", "rommapp/romm", "host", serde_json::json!({}));
        unknown["Config"]["Env"] = serde_json::json!([]);
        let docker = FakeDocker::new().local(serde_json::json!([unknown]));
        let found = discover_candidates(&docker, None, None);
        match found {
            DiscoveryOutcome::Candidates(list) => {
                assert_eq!(list[0].reach, Reach::HostNetworkPortUnknown);
                assert_eq!(list[0].url(), None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_remote_engine_is_never_discovered() {
        for host in [
            "ssh://me@server",
            "tcp://10.0.0.5:2375",
            "tcp://127.0.0.1:2375",
        ] {
            let docker = FakeDocker::new()
                .with("context", Ok(format!("{host}\n")))
                .with("ps", Ok("aaaa".into()));
            assert_eq!(
                outcome(&docker),
                DiscoveryOutcome::RemoteEngineRefused,
                "{host}"
            );
            assert_eq!(docker.calls(), 1, "only the context was inspected");
        }
        let docker = FakeDocker::new().local(serde_json::json!([]));
        assert_eq!(
            discover_candidates(&docker, Some("tcp://example:2375"), None),
            DiscoveryOutcome::RemoteEngineRefused
        );
        assert_eq!(docker.calls(), 0);
    }

    #[test]
    fn docker_missing_or_forbidden_or_garbled_fails_safely() {
        for failure in [DockerFailure::NotInstalled, DockerFailure::PermissionDenied] {
            let docker = FakeDocker::new().with("context", Err(failure));
            assert_eq!(
                outcome(&docker),
                DiscoveryOutcome::DockerUnavailable(failure)
            );
        }
        let docker = FakeDocker::new()
            .with("context", Ok("unix:///var/run/docker.sock".into()))
            .with("ps", Ok("aaaa".into()))
            .with("inspect", Ok("{not json".into()));
        assert_eq!(outcome(&docker), DiscoveryOutcome::Malformed);
        let docker = FakeDocker::new()
            .with("context", Ok("unix:///var/run/docker.sock".into()))
            .with("ps", Ok("aaaa".into()))
            .with("inspect", Ok("[1, null, \"x\"]".into()));
        assert_eq!(candidates(&docker).len(), 0);
        let session = EndpointSession::new(Box::new(FakeDocker::new()), None);
        let effective = session.recover(
            "http://romm:8080",
            RommConnectivity::DnsFailure,
            &mut |_| Verification::Romm,
        );
        assert_eq!(effective.endpoint_source, EndpointSource::Configured);
        assert_eq!(session.status(), Some(LocalRommStatus::NoneFound));
    }

    #[test]
    fn zero_candidates_is_none_found() {
        let session = EndpointSession::new(
            Box::new(FakeDocker::new().local(serde_json::json!([]))),
            None,
        );
        session.recover(
            "http://romm:8080",
            RommConnectivity::DnsFailure,
            &mut |_| Verification::Romm,
        );
        assert_eq!(session.status(), Some(LocalRommStatus::NoneFound));
    }

    fn session_with(containers: serde_json::Value) -> EndpointSession {
        EndpointSession::new(Box::new(FakeDocker::new().local(containers)), None)
    }

    #[test]
    fn dns_failure_and_refused_each_fall_back_to_one_verified_local_romm() {
        for cause in [
            RommConnectivity::DnsFailure,
            RommConnectivity::ConnectionRefused,
            RommConnectivity::ConnectionFailed,
        ] {
            let session = session_with(serde_json::json!([romm_published("8080")]));
            let configured = "http://romm.saltbox:8080";
            let effective = session.recover(configured, cause, &mut |url| {
                assert_eq!(url, "http://127.0.0.1:8080");
                Verification::Romm
            });
            assert_eq!(
                effective.endpoint_source,
                EndpointSource::LocalDockerFallback
            );
            assert_eq!(effective.effective_endpoint, "http://127.0.0.1:8080");
            // the configured value is reported unchanged, never rewritten
            assert_eq!(effective.configured_endpoint, configured);
            assert_eq!(session.effective(configured), effective);
        }
    }

    #[test]
    fn authentication_and_protocol_failures_never_switch_servers_or_touch_docker() {
        let docker = FakeDocker::new().local(serde_json::json!([romm_published("8080")]));
        let session = EndpointSession::new(Box::new(docker), None);
        for cause in [
            RommConnectivity::AuthenticationFailed,
            RommConnectivity::HttpError(500),
            RommConnectivity::EndpointRefused,
            RommConnectivity::TlsFailure,
            RommConnectivity::Reachable,
            RommConnectivity::UnknownFailure,
        ] {
            let effective = session.recover("http://romm:8080", cause, &mut |_| {
                panic!("must not verify")
            });
            assert_eq!(
                effective.endpoint_source,
                EndpointSource::Configured,
                "{cause:?}"
            );
        }
        assert_eq!(session.status(), None);
    }

    #[test]
    fn a_healthy_configured_endpoint_never_runs_docker() {
        // the caller only invokes recover() after a transport failure; the
        // cheap path used for every request never calls docker.
        let docker = FakeDocker::new().local(serde_json::json!([romm_published("8080")]));
        let session = EndpointSession::new(Box::new(docker), None);
        let effective = session.effective("http://romm:8080");
        assert_eq!(effective.endpoint_source, EndpointSource::Configured);
    }

    #[test]
    fn several_verified_servers_fail_closed_unless_the_configured_port_decides() {
        let mut second = romm_published("9002");
        second["Name"] = "/romm2".into();
        let two = serde_json::json!([romm_published("9001"), second]);
        let session = session_with(two.clone());
        let effective = session.recover(
            "http://romm.saltbox:8080",
            RommConnectivity::DnsFailure,
            &mut |_| Verification::Romm,
        );
        assert_eq!(effective.endpoint_source, EndpointSource::Configured);
        assert_eq!(
            session.status(),
            Some(LocalRommStatus::MultipleFound {
                endpoints: vec![
                    "http://127.0.0.1:9001".into(),
                    "http://127.0.0.1:9002".into()
                ]
            })
        );
        // the configured port singles one out
        let session = session_with(two);
        let effective = session.recover(
            "http://romm.saltbox:9002",
            RommConnectivity::DnsFailure,
            &mut |_| Verification::Romm,
        );
        assert_eq!(effective.effective_endpoint, "http://127.0.0.1:9002");
    }

    #[test]
    fn a_candidate_that_does_not_answer_like_romm_is_not_used() {
        let session = session_with(serde_json::json!([romm_published("8080")]));
        let effective = session.recover(
            "http://romm:8080",
            RommConnectivity::DnsFailure,
            &mut |_| Verification::NotRomm,
        );
        assert_eq!(effective.endpoint_source, EndpointSource::Configured);
        assert_eq!(session.status(), Some(LocalRommStatus::FoundButNotVerified));
    }

    #[test]
    fn discovery_is_remembered_and_a_broken_one_is_rate_limited() {
        let docker = std::sync::Arc::new(FakeDocker::new().local(serde_json::json!([])));
        struct Shared(std::sync::Arc<FakeDocker>);
        impl DockerCli for Shared {
            fn run(&self, args: &[&str]) -> Result<String, DockerFailure> {
                self.0.run(args)
            }
        }
        let session = EndpointSession::new(Box::new(Shared(docker.clone())), None);
        for _ in 0..50 {
            session.recover(
                "http://romm:8080",
                RommConnectivity::DnsFailure,
                &mut |_| Verification::Romm,
            );
        }
        assert_eq!(
            docker.calls(),
            3,
            "one discovery (context, ps) and no more, no inspect without ids"
        );
    }

    #[test]
    fn a_changed_host_port_is_rediscovered_once_after_the_fallback_fails() {
        struct Moving(Mutex<String>);
        impl DockerCli for Moving {
            fn run(&self, args: &[&str]) -> Result<String, DockerFailure> {
                Ok(match args[0] {
                    "context" => "unix:///var/run/docker.sock".into(),
                    "ps" => "aaaa".into(),
                    _ => serde_json::json!([romm_published(&self.0.lock().unwrap())]).to_string(),
                })
            }
        }
        let moving = std::sync::Arc::new(Moving(Mutex::new("8080".into())));
        struct Shared(std::sync::Arc<Moving>);
        impl DockerCli for Shared {
            fn run(&self, args: &[&str]) -> Result<String, DockerFailure> {
                self.0.run(args)
            }
        }
        let session = EndpointSession::new(Box::new(Shared(moving.clone())), None);
        let first = session.recover("http://romm:1", RommConnectivity::DnsFailure, &mut |_| {
            Verification::Romm
        });
        assert_eq!(first.effective_endpoint, "http://127.0.0.1:8080");
        *moving.0.lock().unwrap() = "8181".into();
        session.invalidate();
        let second = session.recover(
            "http://romm:1",
            RommConnectivity::ConnectionRefused,
            &mut |_| Verification::Romm,
        );
        assert_eq!(second.effective_endpoint, "http://127.0.0.1:8181");
    }

    #[test]
    fn paths_and_arguments_with_spaces_are_never_shell_interpreted() {
        // arguments are an argv slice; a name with spaces or metacharacters
        // is carried as one element and is never concatenated into a command.
        let docker = FakeDocker::new().local(serde_json::json!([container(
            "my romm; rm -rf /",
            "rommapp/romm",
            "bridge",
            published("0.0.0.0", "8080")
        )]));
        let found = candidates(&docker);
        assert_eq!(found[0].container, "my romm; rm -rf /");
        let calls = docker.calls.lock().unwrap();
        assert!(calls.iter().all(|call| {
            call.iter()
                .all(|arg| !arg.contains(' ') || arg.starts_with('{'))
        }));
    }

    // ---- verification through the existing client ----

    struct FakeRomm {
        status: u16,
        body: &'static str,
        error: Option<RommRequestError>,
        seen: StdMutex<Vec<(String, bool)>>,
    }
    impl RommTransport for FakeRomm {
        fn get(
            &self,
            url: &str,
            authorization: Option<&str>,
            _max: usize,
            _timeout: Duration,
        ) -> Result<RommHttpResponse, RommRequestError> {
            self.seen
                .lock()
                .unwrap()
                .push((url.to_string(), authorization.is_some()));
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(RommHttpResponse {
                status: self.status,
                body: self.body.as_bytes().to_vec(),
                location: None,
            })
        }
    }
    fn fake(status: u16, body: &'static str) -> FakeRomm {
        FakeRomm {
            status,
            body,
            error: None,
            seen: StdMutex::new(Vec::new()),
        }
    }
    fn verify(transport: &FakeRomm) -> Verification {
        let token = RommToken::parse(SECRET).unwrap();
        verify_romm(
            "http://127.0.0.1:8080",
            &token,
            &[],
            &StaticResolver::new(),
            transport,
        )
    }

    #[test]
    fn only_a_romm_heartbeat_verifies_and_the_token_is_never_sent() {
        let ok = fake(200, r#"{"SYSTEM":{"VERSION":"5.3.1"}}"#);
        assert_eq!(verify(&ok), Verification::Romm);
        assert!(
            ok.seen.lock().unwrap().iter().all(|(_, auth)| !auth),
            "heartbeat is token-free"
        );
        assert_eq!(
            verify(&fake(200, r#"{"hello":"world"}"#)),
            Verification::NotRomm
        );
        assert_eq!(verify(&fake(200, "<html>")), Verification::NotRomm);
        assert_eq!(verify(&fake(404, "")), Verification::NotRomm);
        assert_eq!(verify(&fake(401, "")), Verification::NotRomm);
        let down = FakeRomm {
            error: Some(RommRequestError::Transport { detail: "x".into() }),
            ..fake(200, "")
        };
        assert_eq!(verify(&down), Verification::Unreachable);
    }

    #[test]
    fn no_diagnostic_structure_contains_a_token_or_a_container_address() {
        let session = session_with(serde_json::json!([romm_published("8080")]));
        let effective = session.recover(
            "http://romm:8080",
            RommConnectivity::DnsFailure,
            &mut |_| Verification::Romm,
        );
        let dump = format!("{effective:?}{:?}", session.status());
        let json = serde_json::to_string(&effective).unwrap()
            + &serde_json::to_string(&session.status()).unwrap();
        for text in [dump, json] {
            assert!(
                !text.contains(SECRET) && !text.contains("172.19") && !text.contains("SECRET_KEY"),
                "{text}"
            );
        }
    }

    #[test]
    fn the_production_transport_ignores_proxy_environment() {
        // The only HTTP used by verification is `UreqTransport`, which is
        // built with `.proxy(None)`; the source of that guarantee is pinned
        // here so a refactor cannot silently reintroduce env proxies.
        let source = include_str!("client.rs");
        assert!(source.contains(".proxy(None)"));
        assert!(source.contains(".max_redirects(0)"));
    }

    #[test]
    fn validation_falls_back_only_on_dns_and_never_rewrites_the_configuration() {
        let token = RommToken::parse(SECRET).unwrap();
        let config = RommSourceConfig {
            enabled: true,
            url: "http://romm.saltbox:8080".into(),
            ..RommSourceConfig::default()
        };
        let before = config.clone();
        let heartbeat = fake(200, r#"{"SYSTEM":{"VERSION":"5.3.1"}}"#);
        let resolver = StaticResolver::new(); // romm.saltbox does not resolve
        let session = session_with(serde_json::json!([romm_published("8080")]));
        let source = validate_with_local_fallback(
            Some(&session),
            &config,
            &token,
            &[],
            &resolver,
            &heartbeat,
        )
        .unwrap();
        assert_eq!(source.endpoint().origin(), "http://127.0.0.1:8080");
        assert_eq!(config, before, "the configured endpoint is untouched");
        // a resolving configured endpoint wins and docker is never asked
        let docker = std::sync::Arc::new(
            FakeDocker::new().local(serde_json::json!([romm_published("8080")])),
        );
        struct Shared(std::sync::Arc<FakeDocker>);
        impl DockerCli for Shared {
            fn run(&self, args: &[&str]) -> Result<String, DockerFailure> {
                self.0.run(args)
            }
        }
        let session = EndpointSession::new(Box::new(Shared(docker.clone())), None);
        let resolver =
            StaticResolver::new().with_v4("romm.saltbox", std::net::Ipv4Addr::new(10, 0, 0, 5));
        let source = validate_with_local_fallback(
            Some(&session),
            &config,
            &token,
            &[],
            &resolver,
            &heartbeat,
        )
        .unwrap();
        assert_eq!(source.endpoint().origin(), "http://romm.saltbox:8080");
        assert_eq!(docker.calls(), 0);
        // no session: plain validation, the DNS failure is reported as before
        let refused = validate_with_local_fallback(
            None,
            &config,
            &token,
            &[],
            &StaticResolver::new(),
            &heartbeat,
        );
        assert!(refused.is_err());
    }

    #[test]
    fn a_policy_refusal_is_not_a_reason_to_switch_servers() {
        let token = RommToken::parse(SECRET).unwrap();
        let config = RommSourceConfig {
            enabled: true,
            url: "http://8.8.8.8:8080".into(), // public address: refused by policy, not DNS
            ..RommSourceConfig::default()
        };
        let docker = FakeDocker::new().local(serde_json::json!([romm_published("8080")]));
        let session = EndpointSession::new(Box::new(docker), None);
        let heartbeat = fake(200, r#"{"SYSTEM":{"VERSION":"5"}}"#);
        let result = validate_with_local_fallback(
            Some(&session),
            &config,
            &token,
            &[],
            &StaticResolver::new(),
            &heartbeat,
        );
        assert!(result.is_err());
        assert_eq!(session.status(), None);
    }

    /// Read-only probe of the real local engine (never part of the normal run):
    /// `cargo test -p archivefs-core --lib -- --ignored real_local_docker_probe --nocapture`.
    #[test]
    #[ignore]
    fn real_local_docker_probe() {
        let host = std::env::var("DOCKER_HOST").ok();
        let outcome = discover_candidates(&SystemDocker, host.as_deref(), Some(8080));
        println!("outcome={outcome:#?}");
        if let DiscoveryOutcome::Candidates(list) = &outcome {
            let token = RommToken::parse("probe-token-never-sent").unwrap();
            for candidate in list {
                let verdict = candidate.url().map(|url| {
                    verify_romm(
                        &url,
                        &token,
                        &[],
                        &crate::identity_source::net_policy::SystemResolver,
                        &crate::identity_source::romm::client::UreqTransport::new(),
                    )
                });
                println!(
                    "{} url={:?} verification={verdict:?}",
                    candidate.container,
                    candidate.url()
                );
            }
        }
        let session = EndpointSession::system();
        let effective = session.recover(
            "http://romm.saltbox:8080",
            RommConnectivity::DnsFailure,
            &mut |url| {
                let token = RommToken::parse("probe-token-never-sent").unwrap();
                verify_romm(
                    url,
                    &token,
                    &[],
                    &crate::identity_source::net_policy::SystemResolver,
                    &crate::identity_source::romm::client::UreqTransport::new(),
                )
            },
        );
        println!("effective={effective:#?}\nstatus={:#?}", session.status());
    }
}
