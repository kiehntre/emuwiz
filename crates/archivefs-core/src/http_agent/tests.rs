//! Behavioural policy checks. All sockets are loopback, children inherit no
//! environment, and the named destination is resolved by an in-process test
//! resolver. HTTPS fixtures record a complete ClientHello and close; certificate
//! verification is never disabled and no DNS or live TLS service is needed.

mod fixture;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use super::config;
use fixture::{ManagedChild, Reply, Seen, Server};

const CHILD_TEST: &str = "http_agent::tests::child";
const MODE: &str = "EMUWIZ_HTTP_AGENT_TEST_MODE";
const URL: &str = "EMUWIZ_HTTP_AGENT_TEST_URL";
const TARGET: &str = "EMUWIZ_HTTP_AGENT_TEST_TARGET";
const HOST: &str = "http-agent-test.invalid";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const CHILD_DEADLINE: Duration = Duration::from_secs(4);

#[derive(Debug, Serialize, Deserialize)]
struct Probe {
    success: bool,
    target_resolutions: usize,
    proxy_resolutions: usize,
}

#[derive(Debug, Clone)]
struct LocalResolver {
    target: SocketAddr,
    refuse: bool,
    target_calls: Arc<AtomicUsize>,
    proxy_calls: Arc<AtomicUsize>,
}

impl Resolver for LocalResolver {
    fn resolve(
        &self,
        uri: &http::Uri,
        _: &ureq::config::Config,
        _: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let authority = uri.authority().ok_or(ureq::Error::HostNotFound)?;
        let address = if authority.host() == HOST {
            self.target_calls.fetch_add(1, Ordering::SeqCst);
            if self.refuse {
                return Err(ureq::Error::HostNotFound);
            }
            self.target
        } else if authority.host() == "127.0.0.1" {
            // A regressed agent must be able to reach the fake proxy, so the
            // resolver itself cannot conceal a missing boundary opt-out.
            self.proxy_calls.fetch_add(1, Ordering::SeqCst);
            SocketAddr::from(([127, 0, 0, 1], authority.port_u16().unwrap()))
        } else {
            return Err(ureq::Error::HostNotFound);
        };
        let mut addresses = self.empty();
        addresses.push(address);
        Ok(addresses)
    }
}

#[test]
fn child() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    if mode == "hang" {
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    if mode == "panic" {
        panic!("synthetic child failure");
    }
    let target: SocketAddr = std::env::var(TARGET).unwrap().parse().unwrap();
    assert!(target.ip().is_loopback());
    let url = std::env::var(URL).unwrap();
    let parsed = url::Url::parse(&url).unwrap();
    assert!(matches!(parsed.scheme(), "http" | "https"));
    assert!(matches!(parsed.host_str(), Some("127.0.0.1") | Some(HOST)));
    assert_eq!(parsed.port(), Some(target.port()));
    let target_calls = Arc::new(AtomicUsize::new(0));
    let proxy_calls = Arc::new(AtomicUsize::new(0));
    let resolver = LocalResolver {
        target,
        refuse: mode == "resolver_refuses",
        target_calls: target_calls.clone(),
        proxy_calls: proxy_calls.clone(),
    };
    let configure = |builder: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>| {
        builder
            .timeout_global(Some(REQUEST_TIMEOUT))
            .timeout_connect(Some(Duration::from_millis(500)))
            .timeout_resolve(Some(Duration::from_millis(500)))
            .max_redirects(0)
    };
    let agent: ureq::Agent = match mode.as_str() {
        "new_agent" | "request_config" => config(configure).new_agent(),
        "into" => config(configure).into(),
        "with_parts" | "resolver_refuses" => {
            config(configure).with_parts(DefaultConnector::default(), resolver)
        }
        // Deliberate controls live only in this cfg(test) module.
        "control" => configure(ureq::Agent::config_builder()).build().new_agent(),
        "control_with_parts" => ureq::Agent::with_parts(
            configure(ureq::Agent::config_builder()).build(),
            DefaultConnector::default(),
            resolver,
        ),
        other => panic!("unknown child mode {other}"),
    };
    let request = agent.get(&url);
    let outcome = if mode == "request_config" {
        request
            .config()
            .timeout_global(Some(Duration::from_secs(1)))
            .build()
            .call()
    } else {
        request.call()
    };
    if mode == "resolver_refuses" {
        assert!(matches!(outcome, Err(ureq::Error::HostNotFound)));
    }
    println!(
        "PROBE={}",
        serde_json::to_string(&Probe {
            success: outcome.is_ok(),
            target_resolutions: target_calls.load(Ordering::SeqCst),
            proxy_resolutions: proxy_calls.load(Ordering::SeqCst),
        })
        .unwrap()
    );
}

fn command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env_clear()
        .env(MODE, mode);
    command
}

fn run(mode: &str, https: bool, vars: &[(&str, &str)]) -> (Seen, Seen, Probe) {
    let mut target = Server::start(Reply::Http);
    let mut proxy = Server::start(Reply::ProxyRefusal);
    let named = matches!(
        mode,
        "with_parts" | "resolver_refuses" | "control_with_parts"
    );
    let host = if named { HOST } else { "127.0.0.1" };
    let scheme = if https { "https" } else { "http" };
    let url = format!("{scheme}://{host}:{}/boundary-probe", target.address.port());
    let mut command = command(mode);
    command
        .env(URL, url)
        .env(TARGET, target.address.to_string());
    for (key, value) in vars {
        command.env(
            key,
            value.replace("<proxy>", &format!("http://{}", proxy.address)),
        );
    }
    let mut child = ManagedChild::spawn(&mut command).unwrap();
    let output = child.wait(CHILD_DEADLINE).unwrap();
    assert!(
        output.status.success(),
        "child failed: {}{}",
        output.stdout,
        output.stderr
    );
    let probe: Probe = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("PROBE="))
        .map(|json| serde_json::from_str(json).unwrap())
        .expect("child printed no probe");
    (target.finish(), proxy.finish(), probe)
}

const SINGLE_VARIABLES: &[&str] = &[
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

const COMBINATIONS: &[&[(&str, &str)]] = &[
    &[("ALL_PROXY", "<proxy>"), ("NO_PROXY", "unrelated.invalid")],
    &[("all_proxy", "<proxy>"), ("no_proxy", "unrelated.invalid")],
    &[("ALL_PROXY", "<proxy>"), ("NO_PROXY", "*")],
    &[("all_proxy", "<proxy>"), ("no_proxy", "*")],
    &[("HTTP_PROXY", "<proxy>"), ("NO_PROXY", "127.0.0.1")],
    &[("https_proxy", "<proxy>"), ("no_proxy", HOST)],
    &[
        ("HTTP_PROXY", "<proxy>"),
        ("http_proxy", "<proxy>"),
        ("HTTPS_PROXY", "<proxy>"),
        ("https_proxy", "<proxy>"),
        ("ALL_PROXY", "<proxy>"),
        ("all_proxy", "<proxy>"),
        ("NO_PROXY", "unrelated.invalid"),
        ("no_proxy", "another.invalid"),
    ],
];

fn assert_direct(mode: &str) {
    let singles: Vec<_> = SINGLE_VARIABLES
        .iter()
        .map(|key| vec![(*key, "<proxy>")])
        .collect();
    for vars in singles
        .iter()
        .map(Vec::as_slice)
        .chain(COMBINATIONS.iter().copied())
    {
        for https in [false, true] {
            let (target, proxy, probe) = run(mode, https, vars);
            assert_eq!(
                proxy.accepted, 0,
                "{mode}, HTTPS={https}, vars={vars:?}: {proxy:?}"
            );
            assert!(target.errors.is_empty(), "{target:?}");
            assert_eq!(
                target.accepted, 1,
                "request did not reach its local target: {target:?}"
            );
            if https {
                assert_eq!(target.tls_hellos, 1, "no complete ClientHello: {target:?}");
                assert!(
                    !probe.success,
                    "the fixture deliberately refuses the TLS handshake"
                );
            } else {
                assert!(probe.success);
                assert_eq!(target.headers.len(), 1);
                assert!(target.headers[0].starts_with(b"GET /boundary-probe HTTP/1.1\r\n"));
            }
            if mode == "with_parts" {
                assert_eq!(probe.target_resolutions, 1);
                assert_eq!(probe.proxy_resolutions, 0);
            }
        }
    }
}

#[test]
fn new_agent_ignores_proxy_environment() {
    assert_direct("new_agent");
}

#[test]
fn configured_into_ignores_proxy_environment() {
    assert_direct("into");
}

#[test]
fn with_parts_and_custom_resolver_ignore_proxy_environment() {
    assert_direct("with_parts");
}

#[test]
fn request_configuration_inherits_proxy_opt_out() {
    assert_direct("request_config");
}

#[test]
fn controls_reach_the_fake_proxy_for_every_variable_and_both_schemes() {
    for variable in SINGLE_VARIABLES {
        for mode in ["control", "control_with_parts"] {
            for https in [false, true] {
                let (target, proxy, _) = run(mode, https, &[(*variable, "<proxy>")]);
                assert_eq!(target.accepted, 0, "control bypassed the proxy: {target:?}");
                assert_eq!(proxy.accepted, 1, "control reached no proxy: {proxy:?}");
                assert!(proxy.errors.is_empty(), "{proxy:?}");
                assert_eq!(proxy.headers.len(), 1);
                assert!(proxy.headers[0].starts_with(b"CONNECT "));
                assert!(proxy.headers[0].ends_with(b"\r\n\r\n"));
            }
        }
    }
}

#[test]
fn a_custom_resolver_can_still_refuse_the_destination() {
    let (target, proxy, probe) = run("resolver_refuses", true, &[("ALL_PROXY", "<proxy>")]);
    assert_eq!(target.accepted, 0);
    assert_eq!(proxy.accepted, 0);
    assert_eq!(probe.target_resolutions, 1);
    assert_eq!(probe.proxy_resolutions, 0);
    assert!(!probe.success);
}

#[test]
fn all_conversions_preserve_transport_configuration() {
    let configure = |builder: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>| {
        builder
            .https_only(true)
            .http_status_as_error(false)
            .max_redirects(0)
            .tls_config(ureq::tls::TlsConfig::builder().build())
            .timeout_global(Some(Duration::from_secs(7)))
            .timeout_resolve(Some(Duration::from_secs(1)))
            .timeout_connect(Some(Duration::from_secs(2)))
            .timeout_recv_response(None)
            .timeout_recv_body(Some(Duration::from_secs(3)))
    };
    let expected = configure(ureq::Agent::config_builder()).proxy(None).build();
    let agents = [
        config(configure).new_agent(),
        config(configure).into(),
        config(configure).with_parts(
            DefaultConnector::default(),
            LocalResolver {
                target: SocketAddr::from(([127, 0, 0, 1], 1)),
                refuse: false,
                target_calls: Arc::new(AtomicUsize::new(0)),
                proxy_calls: Arc::new(AtomicUsize::new(0)),
            },
        ),
    ];
    for agent in agents {
        assert_eq!(format!("{:?}", agent.config()), format!("{expected:?}"));
    }
}

#[test]
fn proxy_policy_is_applied_after_transport_configuration() {
    let agent =
        config(|builder| builder.proxy(Some(ureq::Proxy::new("http://127.0.0.1:1").unwrap())))
            .new_agent();
    assert!(agent.config().proxy().is_none());
}

#[test]
fn recorder_accumulates_fragmented_headers() {
    let mut server = Server::start(Reply::ProxyRefusal);
    let mut stream = TcpStream::connect(server.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    for part in [
        b"CONNECT ".as_slice(),
        b"http-agent-test.invalid:443 HTTP/1.1\r\n",
        b"Host: test\r\n",
        b"\r\n",
    ] {
        stream.write_all(part).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 502"));
    let seen = server.finish();
    assert_eq!(seen.accepted, 1);
    assert!(seen.errors.is_empty(), "{seen:?}");
    assert_eq!(
        seen.headers,
        [b"CONNECT http-agent-test.invalid:443 HTTP/1.1\r\nHost: test\r\n\r\n".to_vec()]
    );
}

#[test]
fn recorder_bounds_oversized_headers_and_counts_the_connection() {
    let mut server = Server::start(Reply::ProxyRefusal);
    let mut stream = TcpStream::connect(server.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .write_all(&vec![b'x'; fixture::HEADER_LIMIT + 1])
        .unwrap();
    let _ = stream.read(&mut [0u8; 1]);
    let seen = server.finish();
    assert_eq!(seen.accepted, 1);
    assert!(seen.headers.is_empty());
    assert_eq!(seen.errors, ["HTTP header limit exceeded"]);
}

#[test]
fn parent_deadline_kills_and_reaps_a_stalled_child() {
    let mut child = ManagedChild::spawn(&mut command("hang")).unwrap();
    let reaped = child.reaped.clone();
    let isolation_root = child.isolation_root.clone();
    let start = Instant::now();
    let error = child.wait(Duration::from_millis(100)).err().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(reaped.load(Ordering::SeqCst));
    assert!(isolation_root.is_none_or(|root| !root.exists()));
}

#[test]
fn panic_cleanup_joins_listeners_closes_sockets_and_reaps_the_child() {
    let server = Server::start(Reply::Http);
    let address = server.address;
    let joined = server.joined.clone();
    let mut stream = TcpStream::connect(address).unwrap();
    stream.write_all(b"GET /incomplete").unwrap();
    server.wait_for_connection();
    let child = ManagedChild::spawn(&mut command("hang")).unwrap();
    let reaped = child.reaped.clone();
    let isolation_root = child.isolation_root.clone();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _server = server;
        let _child = child;
        assert!(false, "synthetic parent assertion");
    }));
    assert!(panic.is_err());
    assert!(joined.load(Ordering::SeqCst));
    assert!(reaped.load(Ordering::SeqCst));
    assert!(isolation_root.is_none_or(|root| !root.exists()));
    drop(stream);
    // Connection refusal proves the listener is closed without depending on
    // platform-specific TIME_WAIT rules for rebinding an accepted socket.
    let error = TcpStream::connect_timeout(&address, Duration::from_millis(100))
        .err()
        .expect("listener survived unwinding");
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused);
}

#[test]
fn child_assertion_failure_also_cleans_up_the_parent_fixtures() {
    let server = Server::start(Reply::Http);
    let joined = server.joined.clone();
    let mut child = ManagedChild::spawn(&mut command("panic")).unwrap();
    let reaped = child.reaped.clone();
    let isolation_root = child.isolation_root.clone();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _server = server;
        let output = child.wait(CHILD_DEADLINE).unwrap();
        assert!(output.status.success(), "synthetic child assertion failed");
    }));
    assert!(panic.is_err());
    assert!(joined.load(Ordering::SeqCst));
    assert!(reaped.load(Ordering::SeqCst));
    assert!(isolation_root.is_none_or(|root| !root.exists()));
}
