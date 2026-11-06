//! Blue action for the alternative native launch of RollerCoaster Tycoon 2
//! through an already-installed OpenRCT2. Core re-discovers the executable,
//! re-inspects the chosen data folder and re-runs the alternative-provider
//! planner at click time; nothing is installed, copied or modified.
use super::*;
use archivefs_core::launch::{
    OpenRct2LaunchError, OpenRct2LaunchRequest, openrct2_search_dirs, preflight_and_launch_openrct2,
};

pub(crate) const ADAPTER_KEY: &str = "openrct2";

/// Adapter keys for this launch are `openrct2:<executable>`.
pub(crate) fn is_openrct2(adapter: &str) -> bool {
    adapter
        .strip_prefix(ADAPTER_KEY)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
}

#[derive(Clone)]
pub(crate) struct Request(OpenRct2LaunchRequest);

impl Request {
    pub(crate) fn new(request: OpenRct2LaunchRequest) -> Self {
        Self(request)
    }

    /// Keyed by the game folder *and* the exact executable, so a launch
    /// started from one candidate is never shown on another candidate's card.
    pub(crate) fn key(&self) -> (PathBuf, String) {
        (
            self.0.data.root.clone(),
            format!("{ADAPTER_KEY}:{}", self.0.install.executable.display()),
        )
    }

    pub(crate) fn execute(
        self,
    ) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
        preflight_and_launch_openrct2(&self.0, &openrct2_search_dirs()).map_err(refusal_text)
    }
}

pub(crate) fn refusal_text(error: OpenRct2LaunchError) -> String {
    match error {
        OpenRct2LaunchError::NotReady(_, reason) => {
            format!("OpenRCT2 was not started: {reason}. Check again.")
        }
        OpenRct2LaunchError::Changed(detail) => {
            format!("OpenRCT2 was not started: {detail}. Check again.")
        }
        OpenRct2LaunchError::Spawn(error) => format!("OpenRCT2 could not be started: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_and_refusals_read_as_sentences() {
        let text = refusal_text(OpenRct2LaunchError::Changed(
            "the executable changed".into(),
        ));
        assert!(text.starts_with("OpenRCT2 was not started"), "{text}");
        assert!(!text.contains('{'), "{text}");
        let text = refusal_text(OpenRct2LaunchError::NotReady(
            archivefs_core::alternative_launch_provider_planner::AlternativeLaunchReadiness::ProviderMissing,
            "provider executable is not present".into(),
        ));
        assert!(text.contains("Check again"), "{text}");
    }
}

#[cfg(test)]
mod key_and_error_tests {
    use super::*;
    use archivefs_core::launch::process_spawn::CapturedFileIdentity;
    use archivefs_core::launch::{OpenRct2Install, inspect_rct2_data};

    fn install(path: &str) -> OpenRct2Install {
        OpenRct2Install {
            executable: path.into(),
            executable_identity: CapturedFileIdentity {
                device: 1,
                inode: 2,
                size: 3,
                modified: None,
            },
            version: Some("0.4.14".into()),
            content_sha256: Some([0; 32]),
        }
    }

    fn request(exe: &str, root: &std::path::Path) -> StandaloneLaunchRequest {
        StandaloneLaunchRequest::OpenRct2(Request::new(OpenRct2LaunchRequest {
            install: install(exe),
            data: inspect_rct2_data(root),
        }))
    }

    #[test]
    fn two_candidates_sharing_one_game_folder_have_distinct_launch_keys() {
        let dir = tempfile::tempdir().unwrap();
        let a = request("/opt/a/openrct2", dir.path());
        let b = request("/opt/b/openrct2", dir.path());
        let (a_path, a_adapter) = a.key();
        let (b_path, b_adapter) = b.key();
        assert_eq!(a_path, b_path, "same game folder");
        assert_ne!(a_adapter, b_adapter, "different executable");
        assert!(is_openrct2(&a_adapter) && is_openrct2(&b_adapter));
        assert!(is_openrct2("openrct2") && !is_openrct2("openrct20") && !is_openrct2("dosbox"));
    }

    #[test]
    fn a_refused_launch_shows_readable_text_not_a_debug_string() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = StandaloneLaunchState::default();
        state.start(request("/nonexistent/open\"rct2", dir.path()));
        let mut detail = None;
        for _ in 0..500 {
            state.poll();
            detail = state.failure_detail();
            if detail.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let detail = detail.expect("the launch is refused");
        assert!(detail.starts_with("OpenRCT2 "), "{detail}");
        assert!(
            !detail.contains('\\') && !detail.starts_with('"'),
            "{detail}"
        );
    }
}
