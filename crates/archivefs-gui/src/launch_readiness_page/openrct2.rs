//! Blue action for the alternative native launch of RollerCoaster Tycoon 2
//! through an already-installed OpenRCT2. Core re-discovers the executable,
//! re-inspects the chosen data folder and re-runs the alternative-provider
//! planner at click time; nothing is installed, copied or modified.
use super::*;
use archivefs_core::launch::{
    OpenRct2LaunchError, OpenRct2LaunchRequest, openrct2_search_dirs,
    preflight_and_launch_openrct2, probe_openrct2_version,
};

pub(crate) const ADAPTER_KEY: &str = "openrct2";

#[derive(Clone)]
pub(crate) struct Request(OpenRct2LaunchRequest);

impl Request {
    pub(crate) fn new(request: OpenRct2LaunchRequest) -> Self {
        Self(request)
    }

    pub(crate) fn key(&self) -> (PathBuf, String) {
        (self.0.data.root.clone(), ADAPTER_KEY.into())
    }

    pub(crate) fn execute(
        self,
    ) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
        preflight_and_launch_openrct2(&self.0, &openrct2_search_dirs(), &probe_openrct2_version)
            .map_err(refusal_text)
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
