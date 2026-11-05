//! Explicit, user-triggered GitHub Releases lookup for a repository the user
//! names (or that a catalogue entry's own source link names). Metadata only:
//! the existing provider performs bounded GETs to api.github.com and never
//! fetches an asset. Nothing here downloads, installs or runs anything.

use super::view::{self, Entry};
use archivefs_core::homebrew_github::{
    GithubEnumerationOutcome, GithubProviderError, GithubReleasePolicy, GithubReleasesProvider,
    GithubRepositoryRef, GithubTransport, UreqGithubTransport,
};
use std::sync::mpsc::{self, Receiver};

pub(super) struct Pending {
    pub repository: String,
    pub receiver: Receiver<Result<Entry, String>>,
}

/// Starts one lookup on a worker thread. The caller decides when; this is
/// never invoked from a render path without a click.
pub(super) fn start(repository: String, context: &eframe::egui::Context) -> Pending {
    let (tx, rx) = mpsc::channel();
    let context = context.clone();
    let name = repository.clone();
    std::thread::spawn(move || {
        let _ = tx.send(lookup(UreqGithubTransport::new(), &name));
        context.request_repaint();
    });
    Pending {
        repository,
        receiver: rx,
    }
}

pub(super) fn lookup<T: GithubTransport>(transport: T, repository: &str) -> Result<Entry, String> {
    let reference = GithubRepositoryRef::parse(repository.trim()).map_err(|e| e.to_string())?;
    let provider =
        GithubReleasesProvider::with_transport(transport, GithubReleasePolicy::default());
    match provider.enumerate(&reference, None) {
        Ok(GithubEnumerationOutcome::Releases(found)) => {
            let mut entry = view::from_github(&found.repository, &found.listings, &found.details);
            if found.truncated {
                entry.provenance.push(
                    "More releases exist than were requested; only the newest are shown.".into(),
                );
            }
            if found.listings.is_empty() {
                entry
                    .provenance
                    .push("The repository has no published releases with assets.".into());
            }
            Ok(entry)
        }
        Ok(GithubEnumerationOutcome::NotModified) => {
            Err("GitHub reported no change; nothing new to show.".into())
        }
        Err(GithubProviderError::RateLimited { reset_unix }) => Err(match reset_unix {
            Some(reset) => {
                format!("GitHub rate limit reached (resets at unix time {reset}). Try again later.")
            }
            None => "GitHub rate limit reached. Try again later.".into(),
        }),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::homebrew_github::{GithubRequest, GithubResponse, GithubValidators};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    struct Fake {
        urls: Arc<Mutex<Vec<String>>>,
        fail: Option<fn() -> GithubProviderError>,
    }

    fn ok(body: Value) -> GithubResponse {
        GithubResponse {
            status: 200,
            validators: GithubValidators::default(),
            rate_limit_remaining: Some(50),
            rate_limit_reset_unix: None,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    impl GithubTransport for Fake {
        fn get(
            &self,
            request: &GithubRequest,
            _max: usize,
        ) -> Result<GithubResponse, GithubProviderError> {
            self.urls.lock().unwrap().push(request.url.clone());
            if let Some(fail) = self.fail {
                return Err(fail());
            }
            Ok(if request.url.contains("/releases?") {
                if request.url.ends_with("page=1") {
                    ok(json!([{
                        "id": 9, "tag_name": "v1.2", "name": "One two", "draft": false,
                        "prerelease": false, "created_at": "2024-03-01T09:00:00Z",
                        "published_at": "2024-03-01T10:00:00Z",
                        "html_url": "https://github.com/owner/demo/releases/tag/v1.2",
                        "body": "Notes",
                        "assets": [{
                            "id": 5, "name": "demo.gba", "size": 100,
                            "content_type": "application/octet-stream", "state": "uploaded",
                            "created_at": "2024-03-01T10:00:00Z", "updated_at": "2024-03-01T10:00:00Z",
                            "browser_download_url": "https://github.com/owner/demo/releases/download/v1.2/demo.gba"
                        }]
                    }]))
                } else {
                    ok(json!([]))
                }
            } else {
                ok(json!({"id": 4242, "full_name": "owner/demo",
                          "html_url": "https://github.com/owner/demo",
                          "license": {"spdx_id": "MIT"}}))
            })
        }
    }

    fn fake(fail: Option<fn() -> GithubProviderError>) -> (Fake, Arc<Mutex<Vec<String>>>) {
        let urls = Arc::new(Mutex::new(Vec::new()));
        (
            Fake {
                urls: urls.clone(),
                fail,
            },
            urls,
        )
    }

    #[test]
    fn lookup_shows_version_platform_licence_and_only_requests_metadata() {
        let (transport, urls) = fake(None);
        let entry = lookup(transport, "owner/demo").unwrap();
        assert_eq!(entry.title, "owner/demo");
        assert_eq!(entry.releases[0].label, "v1.2");
        assert!(entry.releases[0].builds[0].starts_with("demo.gba"));
        assert!(
            entry.platforms.iter().any(|p| p == "Game Boy Advance"),
            "{:?}",
            entry.platforms
        );
        assert!(entry.licence[0].contains("MIT"));
        assert!(entry.licence.iter().any(|l| l == "Redistribution: unknown"));
        // Metadata endpoints only: never an asset download URL.
        assert!(
            urls.lock()
                .unwrap()
                .iter()
                .all(|u| u.starts_with("https://api.github.com/"))
        );
    }

    #[test]
    fn invalid_repository_is_rejected_before_any_request() {
        let (transport, urls) = fake(None);
        assert!(lookup(transport, "not a repo").is_err());
        assert!(urls.lock().unwrap().is_empty());
    }

    #[test]
    fn network_and_rate_limit_failures_are_reported_plainly() {
        let (transport, _) = fake(Some(|| GithubProviderError::Transport("offline".into())));
        let error = lookup(transport, "owner/demo").unwrap_err();
        assert!(error.contains("could not be reached") && error.contains("offline"));
        let (transport, _) = fake(Some(|| GithubProviderError::RateLimited {
            reset_unix: Some(7),
        }));
        assert!(
            lookup(transport, "owner/demo")
                .unwrap_err()
                .contains("rate limit")
        );
        let (transport, _) = fake(Some(|| GithubProviderError::NotFound));
        assert!(
            lookup(transport, "owner/demo")
                .unwrap_err()
                .contains("not found")
        );
    }
}
