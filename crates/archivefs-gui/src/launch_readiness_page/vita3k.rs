//! Blue action for a title installed in Vita3K, over core's installation-only
//! launch identity and the existing Vita3K preflight/spawn.
//!
//! The identity is a *supported launch target* derived from fresh Vita3K
//! inventory (`param.sfo` validated by the existing inspector, bound to the
//! selected installation and the title's own bytes). It is not a catalogue
//! match and is never persisted.
use super::*;
use archivefs_core::{
    launch::{
        Vita3kInstalledEntry, Vita3kInstalledLaunchError, Vita3kInstalledLaunchRequest,
        preflight_and_launch_vita3k_installed,
    },
    patch_manager::Vita3kProfileDiscoveryRoots,
};

#[derive(Clone)]
pub(crate) struct Request(Vita3kInstalledLaunchRequest);

impl Request {
    pub(crate) fn new(entry: Vita3kInstalledEntry) -> Self {
        Self(Vita3kInstalledLaunchRequest { entry })
    }

    pub(crate) fn key(&self) -> (PathBuf, String) {
        (self.0.entry.title_root.clone(), "vita3k".into())
    }

    /// Core rescans Vita3K and re-checks the title and installation first.
    pub(crate) fn execute(
        self,
    ) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
        let roots =
            Vita3kProfileDiscoveryRoots::from_environment().map_err(|error| error.to_string())?;
        self.execute_with(&roots)
    }

    fn execute_with(
        self,
        roots: &Vita3kProfileDiscoveryRoots,
    ) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
        preflight_and_launch_vita3k_installed(&self.0, roots).map_err(refusal_text)
    }
}

pub(crate) fn refusal_text(error: Vita3kInstalledLaunchError) -> String {
    use Vita3kInstalledLaunchError as E;
    match error {
        E::Missing(detail) => format!("Vita3K was not started: {detail}. Refresh the list."),
        E::Ambiguous(detail) => format!("Vita3K was not started: {detail}."),
        E::Refused(detail) => format!("Vita3K was not started: {detail}."),
        E::Changed(detail) => {
            format!("Vita3K was not started: {detail}. Refresh the list and try again.")
        }
        E::Preflight(error) => format!(
            "Vita3K was not started: {}.",
            error.detail.trim_end_matches('.')
        ),
        E::Spawn(error) => format!("Vita3K could not be started: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::process_spawn::CapturedFileIdentity;
    use archivefs_core::patch_manager::{Vita3kFirmwareState, Vita3kLicenseState};

    fn entry(root: &Path) -> Vita3kInstalledEntry {
        let identity = CapturedFileIdentity {
            device: 1,
            inode: 2,
            size: 3,
            modified: None,
        };
        Vita3kInstalledEntry {
            profile_id: "vita3k:/x".into(),
            executable: root.join("Vita3K"),
            executable_identity: identity,
            config_identity: None,
            title_id: "PCSE00001".into(),
            title_root: root.join("ux0/app/PCSE00001"),
            title_identity: identity,
            param_identity: identity,
            param_sha256: "00".repeat(32),
            display_title: Some("PCSE00002 Not This One".into()),
            category: Some("HB".into()),
            license: Vita3kLicenseState::NotRequired,
            firmware: Vita3kFirmwareState::Unknown,
            blocker: None,
        }
    }

    fn roots(dir: &Path) -> Vita3kProfileDiscoveryRoots {
        Vita3kProfileDiscoveryRoots {
            home: dir.join("home"),
            xdg_data_home: dir.join("xdg"),
            explicit_configuration_roots: vec![dir.join("cfg")],
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: Default::default(),
        }
    }

    #[test]
    fn routing_uses_the_installed_title_folder_and_the_vita3k_adapter() {
        let dir = tempfile::tempdir().unwrap();
        let request = StandaloneLaunchRequest::Vita3k(Request::new(entry(dir.path())));
        assert_eq!(request.adapter_name(), "Vita3K");
        let (path, adapter) = request.key();
        assert_eq!(adapter, "vita3k");
        assert_eq!(path, dir.path().join("ux0/app/PCSE00001"));
    }

    #[test]
    fn a_stale_listing_is_refused_with_refresh_guidance_and_nothing_starts() {
        let dir = tempfile::tempdir().unwrap();
        let error = Request::new(entry(dir.path()))
            .execute_with(&roots(dir.path()))
            .err()
            .unwrap();
        assert!(error.contains("Refresh the list"), "{error}");
        assert!(error.starts_with("Vita3K was not started"), "{error}");
    }

    #[test]
    fn refusals_read_as_sentences_without_debug_syntax() {
        for error in [
            Vita3kInstalledLaunchError::Ambiguous("two installations".into()),
            Vita3kInstalledLaunchError::Changed("the title's param.sfo changed".into()),
            Vita3kInstalledLaunchError::Refused("licence missing".into()),
        ] {
            let text = refusal_text(error);
            assert!(text.starts_with("Vita3K was not started"), "{text}");
            assert!(!text.contains('{') && !text.contains("Err("), "{text}");
        }
    }
}
