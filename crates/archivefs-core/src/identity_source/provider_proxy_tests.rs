//! Environment-proxy isolation for the ScreenScraper and Hasheous transports.
//!
//! `ureq` 3 reads `ALL_PROXY` / `HTTPS_PROXY` / `HTTP_PROXY` (either case) by
//! default. EmuWiz never routes provider traffic through a proxy it was not
//! explicitly configured to use, so both transports must opt out.
//!
//! Every case is its own test so a failure names the variable under test. Each
//! runs [`child`] (this test binary, re-executed) with the *entire*
//! environment cleared except `PATH`, plus only the variables the case lists,
//! so neither the CI host's proxy state nor another case can leak in. The
//! variable value points at a recording loopback listener. Each case then
//! checks two legs: a plain-HTTP request to a loopback target that must arrive
//! directly, and an HTTPS request to a `.invalid` name that can only ever fail
//! to resolve and must not reach the proxy either. Only fake secrets are used;
//! no real provider host is contacted.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::hasheous::client::{HasheousTransport, UreqTransport as HasheousUreq};
use super::screenscraper::{ScreenScraperTransport, UreqTransport as ScreenScraperUreq};

const URL_VAR: &str = "EMUWIZ_PROVIDER_PROXY_CHILD_URL";
const PROVIDER_VAR: &str = "EMUWIZ_PROVIDER_PROXY_CHILD_PROVIDER";
const FAKE_SECRET: &str = "fake-provider-secret-0000";
const CHILD_TEST: &str = "identity_source::provider_proxy_tests::child";

/// Child role. Does nothing unless re-executed by [`run_case`]. Makes one
/// request through the named provider's production transport; the result is
/// ignored because the parent inspects sockets, not responses.
#[test]
fn child() {
    let (Ok(url), Ok(provider)) = (std::env::var(URL_VAR), std::env::var(PROVIDER_VAR)) else {
        return;
    };
    let timeout = Duration::from_secs(5);
    if provider == "screenscraper" {
        let _ = ScreenScraperUreq::new().get(&url, 4096, timeout);
    } else {
        let body = format!(r#"{{"md5":"{FAKE_SECRET}"}}"#);
        let _ = HasheousUreq::new(timeout).post_json(&url, body.as_bytes());
    }
}

/// Accepts connections, keeps every byte each one sends, and answers 200. A
/// `CONNECT` is answered and then read again, so a proxy that completes the
/// tunnel also sees what is tunnelled. Stops once `stop` is set and the queue
/// of connections already made is drained.
fn record(listener: TcpListener, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<u8>> {
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut buffer = [0u8; 4096];
            let count = stream.read(&mut buffer).unwrap_or(0);
            seen.extend_from_slice(&buffer[..count]);
            if buffer[..count].starts_with(b"CONNECT") {
                let _ = stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
                let count = stream.read(&mut buffer).unwrap_or(0);
                seen.extend_from_slice(&buffer[..count]);
            } else {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                );
                // A POST body can arrive in a later segment than its headers.
                while let Ok(count @ 1..) = stream.read(&mut buffer) {
                    seen.extend_from_slice(&buffer[..count]);
                }
            }
        }
        seen
    })
}

/// What each listener received during one child run.
struct Seen {
    proxy: String,
    target: String,
}

/// Re-executes [`child`] against `https` or a loopback HTTP target with an
/// empty environment plus `vars`. A value of `"<proxy>"` is replaced by the
/// recording proxy's address.
fn run_child(provider: &str, path: &str, https: bool, vars: &[(&str, &str)]) -> Seen {
    let stop = Arc::new(AtomicBool::new(false));
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let target_port = target.local_addr().unwrap().port();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let (target, proxy) = (record(target, stop.clone()), record(proxy, stop.clone()));
    let url = if https {
        format!("https://provider-proxy-test.invalid{path}")
    } else {
        format!("http://127.0.0.1:{target_port}{path}")
    };
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(URL_VAR, url)
        .env(PROVIDER_VAR, provider);
    for (name, value) in vars {
        let value = if *value == "<proxy>" {
            proxy_url.as_str()
        } else {
            value
        };
        child.env(name, value);
    }
    assert!(child.status().unwrap().success(), "child process failed");
    stop.store(true, Ordering::SeqCst);
    Seen {
        proxy: String::from_utf8_lossy(&proxy.join().unwrap()).into_owned(),
        target: String::from_utf8_lossy(&target.join().unwrap()).into_owned(),
    }
}

/// One case: neither an HTTP nor an HTTPS request may touch the proxy, and the
/// plain-HTTP request must reach its target directly with its payload intact.
fn run_case(provider: &str, path: &str, label: &str, vars: &[(&str, &str)]) {
    // Both legs run before anything is asserted, so a failure reports both.
    let plain = run_child(provider, path, false, vars);
    let tls = run_child(provider, path, true, vars);
    assert!(
        plain.proxy.is_empty() && tls.proxy.is_empty(),
        "{label}: the {provider} transport reached the environment proxy \
         (HTTP leg: {:?}, HTTPS leg: {:?})",
        plain.proxy.lines().next(),
        tls.proxy.lines().next()
    );
    assert!(
        plain.target.contains(FAKE_SECRET),
        "{label}: the {provider} request did not reach its target directly"
    );
}

const SCREENSCRAPER_PATH: &str = "/jeuInfos.php?devpassword=fake-provider-secret-0000";
const HASHEOUS_PATH: &str = "/api/v1/Lookup/ByHash";

macro_rules! matrix {
    ($provider:literal, $path:expr, $($name:ident: $vars:expr,)+) => {
        $(
            #[test]
            fn $name() {
                run_case($provider, $path, stringify!($name), &$vars);
            }
        )+
    };
}

mod screenscraper {
    use super::*;

    matrix! {
        "screenscraper", SCREENSCRAPER_PATH,
        no_proxy_variables: [],
        http_proxy_upper_only: [("HTTP_PROXY", "<proxy>")],
        https_proxy_upper_only: [("HTTPS_PROXY", "<proxy>")],
        all_proxy_upper_only: [("ALL_PROXY", "<proxy>")],
        http_proxy_lower_only: [("http_proxy", "<proxy>")],
        https_proxy_lower_only: [("https_proxy", "<proxy>")],
        all_proxy_lower_only: [("all_proxy", "<proxy>")],
        all_proxy_with_unrelated_no_proxy: [("ALL_PROXY", "<proxy>"), ("NO_PROXY", "example.invalid")],
        all_proxy_with_no_proxy_star: [("ALL_PROXY", "<proxy>"), ("NO_PROXY", "*")],
    }
}

mod hasheous {
    use super::*;

    matrix! {
        "hasheous", HASHEOUS_PATH,
        no_proxy_variables: [],
        http_proxy_upper_only: [("HTTP_PROXY", "<proxy>")],
        https_proxy_upper_only: [("HTTPS_PROXY", "<proxy>")],
        all_proxy_upper_only: [("ALL_PROXY", "<proxy>")],
        http_proxy_lower_only: [("http_proxy", "<proxy>")],
        https_proxy_lower_only: [("https_proxy", "<proxy>")],
        all_proxy_lower_only: [("all_proxy", "<proxy>")],
        all_proxy_with_unrelated_no_proxy: [("ALL_PROXY", "<proxy>"), ("NO_PROXY", "example.invalid")],
        all_proxy_with_no_proxy_star: [("ALL_PROXY", "<proxy>"), ("NO_PROXY", "*")],
    }
}
