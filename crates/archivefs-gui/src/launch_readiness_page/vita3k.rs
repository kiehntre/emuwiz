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
        LaunchContentRef, VITA3K_INSTALLED_KEY_PREFIX, Vita3kInstalledEntry,
        Vita3kInstalledLaunchError, Vita3kInstalledLaunchRequest, Vita3kLibraryLaunchError,
        Vita3kLibraryLaunchRequest, Vita3kLibraryResolution, VpkEvidence, VpkRefusal, inspect_vpk,
        preflight_and_launch_vita3k_installed, preflight_and_launch_vita3k_library, resolve_vpk,
    },
    patch_manager::Vita3kProfileDiscoveryRoots,
};

#[derive(Clone)]
pub(crate) enum Request {
    /// A title listed from Vita3K's own inventory (All Tools page).
    Installed(Box<Vita3kInstalledLaunchRequest>),
    /// A library `.vpk` whose title id is installed in Vita3K.
    Library(Box<Vita3kLibraryLaunchRequest>),
}

impl Request {
    pub(crate) fn new(entry: Vita3kInstalledEntry) -> Self {
        Self::Installed(Box::new(Vita3kInstalledLaunchRequest { entry }))
    }

    pub(crate) fn key(&self) -> (PathBuf, String) {
        match self {
            Self::Installed(r) => (r.entry.title_root.clone(), "vita3k".into()),
            Self::Library(r) => (r.vpk.path.clone(), "vita3k".into()),
        }
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
        match self {
            Self::Installed(r) => {
                preflight_and_launch_vita3k_installed(&r, roots).map_err(refusal_text)
            }
            Self::Library(r) => {
                preflight_and_launch_vita3k_library(&r, roots).map_err(library_refusal_text)
            }
        }
    }
}

pub(crate) const EXTRACTED_FOLDER_TEXT: &str = "Extracted PlayStation Vita title folders are not launched from the library. \
Install the title in Vita3K, then start it from All Tools \u{2192} PlayStation Vita (Vita3K) titles, \
or add its .vpk package to the library.";

/// A real directory holding a regular `sce_sys/param.sfo`. Worker-only.
fn is_extracted_vita_folder(path: &Path) -> bool {
    let real = |p: &Path| std::fs::symlink_metadata(p);
    real(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        && real(&path.join("sce_sys/param.sfo")).is_ok_and(|m| m.is_file())
}

/// A catalogued platform other than PlayStation Vita means other evidence
/// places the file elsewhere; the launch-only identity must not override it.
pub(crate) fn platform_allows(catalogued: Option<&str>) -> bool {
    catalogued.is_none_or(|platform| {
        platform.is_empty()
            || archivefs_core::canonical_platform_for_alias(platform) == Some("PlayStation Vita")
    })
}

/// Launch-only installed-title keys are never stored as a per-game default.
pub(crate) fn game_default_allowed(game_key: &str) -> bool {
    !game_key.starts_with(VITA3K_INSTALLED_KEY_PREFIX)
}

/// One check of a library `.vpk` against Vita3K's installed titles.
#[derive(Clone)]
pub(crate) struct Context {
    pub selected: PathBuf,
    pub result: Result<(VpkEvidence, Vita3kLibraryResolution), VpkRefusal>,
    /// The selection is an extracted Vita title folder (unsupported here).
    pub extracted_folder: bool,
}

impl Context {
    /// Worker-only: reads the package directory and rescans Vita3K.
    pub(crate) fn inspect(selected: PathBuf) -> Self {
        match Vita3kProfileDiscoveryRoots::from_environment() {
            Ok(roots) => Self::inspect_with(selected, &roots),
            Err(error) => Self {
                selected,
                result: Err(VpkRefusal {
                    kind: archivefs_core::launch::VpkRefusalKind::Unreadable,
                    detail: error.to_string(),
                }),
                extracted_folder: false,
            },
        }
    }

    pub(crate) fn inspect_with(selected: PathBuf, roots: &Vita3kProfileDiscoveryRoots) -> Self {
        let extracted_folder =
            !archivefs_core::launch::is_vpk(&selected) && is_extracted_vita_folder(&selected);
        let result = inspect_vpk(&selected).map(|evidence| {
            let resolution = resolve_vpk(&evidence, roots);
            (evidence, resolution)
        });
        Self {
            selected,
            result,
            extracted_folder,
        }
    }

    /// Only a `.vpk` or an extracted Vita folder is ever this module's business;
    /// any other selection (a ScummVM folder, a ROM) must see no trace of it.
    pub(crate) fn relevant(&self) -> bool {
        archivefs_core::launch::is_vpk(&self.selected) || self.extracted_folder
    }

    fn installed(&self) -> Option<(&VpkEvidence, &Vita3kInstalledEntry)> {
        match self.result.as_ref().ok()? {
            (evidence, Vita3kLibraryResolution::Installed { entry, .. })
                if evidence.path == self.selected =>
            {
                Some((evidence, entry))
            }
            _ => None,
        }
    }

    /// Supplies a launch identity only when nothing else resolved one and the
    /// title is installed exactly once and launchable.
    pub(crate) fn inputs(
        &self,
        prior: &CanonicalIdentityStatus,
    ) -> Option<(CanonicalIdentityStatus, LaunchContentRef)> {
        if !matches!(prior, CanonicalIdentityStatus::Unknown) {
            return None;
        }
        let (_, entry) = self.installed()?;
        Some((entry.identity_status(), entry.content_ref()))
    }

    /// The installation Play will recheck, as the planner's profile input.
    pub(crate) fn profile_input(&self) -> Option<archivefs_core::launch::StandaloneProfileInput> {
        use archivefs_core::{launch::FirmwareReadiness, patch_manager::Vita3kFirmwareState};
        let (_, entry) = self.installed()?;
        Some(archivefs_core::launch::StandaloneProfileInput {
            adapter_id: "vita3k",
            profile_id: entry.profile_id.clone(),
            profile_path: None,
            eligible: true,
            firmware: match entry.firmware {
                Vita3kFirmwareState::PresentUnverified => FirmwareReadiness::PresentUnverified,
                Vita3kFirmwareState::Missing => FirmwareReadiness::Missing,
                Vita3kFirmwareState::Unknown => FirmwareReadiness::Unknown,
            },
        })
    }

    pub(crate) fn request(
        &self,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
    ) -> Option<Request> {
        let (evidence, entry) = self.installed()?;
        let LaunchTarget::Standalone {
            adapter_id: "vita3k",
            profile_id,
            ..
        } = &candidate.target
        else {
            return None;
        };
        let ready = matches!(
            candidate.readiness,
            LaunchReadiness::Ready | LaunchReadiness::ReadyWithWarnings
        );
        (ready
            && candidate.blockers.is_empty()
            && !candidate.content.requires_mount
            && profile_id == &entry.profile_id
            && candidate.content.resolved_path.as_deref() == Some(entry.title_root.as_path())
            && plan.platform_id.as_deref() == Some("PlayStation Vita")
            && plan.game_key.as_deref() == Some(entry.game_key().as_str()))
        .then(|| {
            Request::Library(Box::new(Vita3kLibraryLaunchRequest {
                vpk: evidence.clone(),
                entry: entry.clone(),
            }))
        })
    }

    /// The truthful status line for this package, whatever its state.
    pub(crate) fn status_text(&self) -> String {
        if self.extracted_folder {
            return EXTRACTED_FOLDER_TEXT.into();
        }
        match &self.result {
            Err(refusal) => format!("Not launchable as a Vita package: {}.", refusal.detail),
            Ok((evidence, resolution)) => {
                let id = &evidence.title_id;
                match resolution {
                    Vita3kLibraryResolution::Installed { param_differs, .. } => {
                        let mut text = format!(
                            "Vita3K has {id} installed. Play starts that installed copy, not                              this package; EmuWiz never installs titles."
                        );
                        if *param_differs {
                            text.push_str(
                                " The installed copy's metadata differs from this package                                  (for example an update).",
                            );
                        }
                        text
                    }
                    Vita3kLibraryResolution::Blocked(reason) => {
                        format!("{id} is installed in Vita3K but cannot start: {reason}.")
                    }
                    Vita3kLibraryResolution::Ambiguous => format!(
                        "{id} is installed in more than one Vita3K installation; Play is                          disabled until only one holds it."
                    ),
                    Vita3kLibraryResolution::NotInstalled => format!(
                        "{id} is not installed in Vita3K. Install this package in Vita3K                          yourself, then re-check; EmuWiz does not install Vita packages."
                    ),
                    Vita3kLibraryResolution::NoInstallation(reason) => {
                        format!("No usable Vita3K installation: {reason}.")
                    }
                }
            }
        }
    }

    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
        state: &mut StandaloneLaunchState,
    ) {
        if state.is_active()
            && !state
                .tracked
                .as_ref()
                .is_some_and(|(path, adapter, _)| path == &self.selected && adapter == "vita3k")
        {
            ui.label("Another standalone launch is active; wait before starting Vita3K.");
            return;
        }
        if let Some(request) = self.request(plan, candidate) {
            ui.small(
                "Vita package (.vpk): a launch target, not a catalogue-verified game. \
                 Vita3K is started with the title already installed under the same title ID; \
                 EmuWiz cannot confirm it runs.",
            );
            show_standalone_launch_action(ui, state, StandaloneLaunchRequest::Vita3k(request));
        }
    }
}

pub(crate) fn library_refusal_text(error: Vita3kLibraryLaunchError) -> String {
    match error {
        Vita3kLibraryLaunchError::Package(refusal) => format!(
            "Vita3K was not started: the package is no longer a valid .vpk ({}). Recheck readiness.",
            refusal.detail
        ),
        Vita3kLibraryLaunchError::PackageChanged(detail) => {
            format!("Vita3K was not started: {detail}. Recheck readiness before playing.")
        }
        Vita3kLibraryLaunchError::Installed(error) => refusal_text(error),
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
            changed: None,
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

    use archivefs_core::launch::{FirmwareReadiness, StandaloneProfileInput, build_launch_plan};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    fn sfo(entries: &[(&str, &str)]) -> Vec<u8> {
        let key_start = 20 + entries.len() * 16;
        let (mut keys, mut data, mut index) = (Vec::new(), Vec::new(), Vec::new());
        for (key, value) in entries {
            let (ko, dofs) = (keys.len() as u16, data.len() as u32);
            keys.extend_from_slice(key.as_bytes());
            keys.push(0);
            data.extend_from_slice(value.as_bytes());
            data.push(0);
            let len = (value.len() + 1) as u32;
            index.extend_from_slice(&ko.to_le_bytes());
            index.extend_from_slice(&0x0204_u16.to_le_bytes());
            index.extend_from_slice(&len.to_le_bytes());
            index.extend_from_slice(&len.to_le_bytes());
            index.extend_from_slice(&dofs.to_le_bytes());
        }
        while keys.len() % 4 != 0 {
            keys.push(0);
        }
        let mut out = vec![0, b'P', b'S', b'F'];
        out.extend_from_slice(&0x0101_u32.to_le_bytes());
        out.extend_from_slice(&(key_start as u32).to_le_bytes());
        out.extend_from_slice(&((key_start + keys.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&index);
        out.extend_from_slice(&keys);
        out.extend_from_slice(&data);
        out
    }

    struct Fixture {
        dir: tempfile::TempDir,
        vpk: PathBuf,
        roots: Vita3kProfileDiscoveryRoots,
        context: Context,
        plan: LaunchPlan,
    }

    fn write_vpk(path: &Path, id: &str) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("eboot.bin", options).unwrap();
        zip.write_all(b"x").unwrap();
        zip.start_file("sce_sys/param.sfo", options).unwrap();
        zip.write_all(&sfo(&[("TITLE_ID", id), ("TITLE", "Synthetic")]))
            .unwrap();
        zip.finish().unwrap();
    }

    fn fixture(installed: &[&str], package_id: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("cfg");
        std::fs::create_dir_all(cfg.join("vs0/sys/external")).unwrap();
        for id in installed {
            let sys = cfg.join("ux0/app").join(id).join("sce_sys");
            std::fs::create_dir_all(&sys).unwrap();
            std::fs::write(
                sys.join("param.sfo"),
                sfo(&[("TITLE_ID", id), ("TITLE", "Synthetic"), ("CATEGORY", "gd")]),
            )
            .unwrap();
            std::fs::create_dir_all(cfg.join("ux0/license/app").join(id)).unwrap();
        }
        let executable = dir.path().join("Vita3K");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let roots = Vita3kProfileDiscoveryRoots {
            home: dir.path().join("home"),
            xdg_data_home: dir.path().join("xdg"),
            explicit_configuration_roots: vec![cfg],
            portable_configuration_roots: Vec::new(),
            explicit_executables: vec![executable],
            known_version_outputs: Default::default(),
        };
        let vpk = dir.path().join("Game.vpk");
        write_vpk(&vpk, package_id);
        let context = Context::inspect_with(vpk.clone(), &roots);
        let plan = plan_for(&context);
        Fixture {
            dir,
            vpk,
            roots,
            context,
            plan,
        }
    }

    fn plan_for(context: &Context) -> LaunchPlan {
        let (identity, content) = context
            .inputs(&CanonicalIdentityStatus::Unknown)
            .unwrap_or((
                CanonicalIdentityStatus::Unknown,
                LaunchContentRef {
                    kind: None,
                    container: None,
                    resolved_path: None,
                    requires_mount: false,
                    provenance: String::new(),
                },
            ));
        let profiles: Vec<StandaloneProfileInput> = context.profile_input().into_iter().collect();
        let _ = FirmwareReadiness::Unknown;
        let env = archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![],
            diagnostics: vec![],
        };
        build_launch_plan(&identity, &content, &profiles, &env, &[])
    }

    fn play_action(f: &Fixture) -> GamerPlayAction {
        gamer_play_action(&LaunchReadinessInput::Plan {
            plan: f.plan.clone(),
            retroarch: None,
            retroarch_scanned: true,
            standalone_scans_complete: true,
            dolphin: None,
            pcsx2: None,
            duckstation: None,
            ppsspp: None,
            rpcs3: None,
            xemu: None,
            xenia: None,
            melonds: None,
            cemu: None,
            flycast: None,
            xroar: None,
            azahar: None,
            vita3k: Some(f.context.clone()),
            vice: None,
            amiga_whdload: None,
            #[cfg(target_os = "linux")]
            shadps4: None,
        })
    }

    fn is_vita_play(action: &GamerPlayAction) -> bool {
        matches!(
            action,
            GamerPlayAction::Launch(request)
                if matches!(**request, TypedLaunchRequest::Standalone(StandaloneLaunchRequest::Vita3k(Request::Library(_))))
        )
    }

    #[test]
    fn an_installed_title_reaches_the_blue_play_action() {
        let f = fixture(&["PCSE00001"], "PCSE00001");
        assert!(
            is_vita_play(&play_action(&f)),
            "{}",
            f.context.status_text()
        );
        assert_eq!(f.plan.platform_id.as_deref(), Some("PlayStation Vita"));
    }

    #[test]
    fn uninstalled_mismatched_or_ambiguous_titles_get_no_play_and_say_why() {
        let f = fixture(&["PCSE00002"], "PCSE00001");
        assert!(!is_vita_play(&play_action(&f)));
        assert!(f.context.status_text().contains("not installed"));
        let none = fixture(&[], "PCSE00001");
        assert!(!is_vita_play(&play_action(&none)));
        assert!(none.context.status_text().contains("Vita3K"));
        let bad = fixture(&["PCSE00001"], "PCSE00001");
        std::fs::write(&bad.vpk, b"not a zip").unwrap();
        let context = Context::inspect_with(bad.vpk.clone(), &bad.roots);
        assert!(context.inputs(&CanonicalIdentityStatus::Unknown).is_none());
        assert!(context.status_text().contains("Not launchable"));
    }

    #[test]
    fn stronger_identity_is_never_overridden_and_keys_are_not_defaults() {
        let f = fixture(&["PCSE00001"], "PCSE00001");
        assert!(
            f.context
                .inputs(&CanonicalIdentityStatus::Conflicting)
                .is_none()
        );
        assert!(!game_default_allowed("vita3k-installed:PCSE00001"));
        assert!(game_default_allowed("catalogue-key"));
        assert!(platform_allows(None) && platform_allows(Some("PlayStation Vita")));
        assert!(!platform_allows(Some("PSP")));
    }

    #[test]
    fn request_requires_the_matching_plan_key_and_vita3k_candidate() {
        let f = fixture(&["PCSE00001"], "PCSE00001");
        let candidate = &f.plan.candidates[0];
        assert!(f.context.request(&f.plan, candidate).is_some());
        let mut wrong = f.plan.clone();
        wrong.game_key = Some("vita3k-installed:PCSE00009".into());
        assert!(f.context.request(&wrong, candidate).is_none());
        wrong = f.plan.clone();
        wrong.platform_id = Some("PSP".into());
        assert!(f.context.request(&wrong, candidate).is_none());
    }

    #[test]
    fn stale_selection_is_refused_at_click_and_nothing_starts() {
        let f = fixture(&["PCSE00001"], "PCSE00001");
        let request = f.context.request(&f.plan, &f.plan.candidates[0]).unwrap();
        // The package is swapped for another title's after Play was offered.
        write_vpk(&f.vpk, "PCSE00002");
        let error = request.clone().execute_with(&f.roots).err().unwrap();
        assert!(error.starts_with("Vita3K was not started"), "{error}");
        // Restored package, but the installed title was removed.
        write_vpk(&f.vpk, "PCSE00001");
        let fresh = f.context.request(&f.plan, &f.plan.candidates[0]).unwrap();
        let cfg = f.dir.path().join("cfg/ux0/app/PCSE00001");
        std::fs::remove_dir_all(cfg).unwrap();
        assert!(fresh.execute_with(&f.roots).is_err());
    }

    #[test]
    fn play_hands_off_to_the_existing_spawn_and_is_monitored() {
        let f = fixture(&["PCSE00001"], "PCSE00001");
        let request = f.context.request(&f.plan, &f.plan.candidates[0]).unwrap();
        assert_eq!(request.key().1, "vita3k");
        let mut process = request.execute_with(&f.roots).unwrap();
        for _ in 0..200 {
            if process.poll().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("fixture process never exited");
    }

    #[test]
    fn unrelated_selections_see_no_vita_context() {
        let f = fixture(&["PCSE00001"], "PCSE00001");
        let folder = f.dir.path().join("some-scummvm-game");
        std::fs::create_dir(&folder).unwrap();
        let context = Context::inspect_with(folder.clone(), &f.roots);
        assert!(!context.relevant());
        let controller = crate::emulator_setup::vita3k_library::Controller::with_context(
            folder.clone(),
            context,
        );
        assert!(controller.context(Some(&folder)).is_none());
        // An extracted Vita folder is relevant and says it is unsupported.
        let vita = f.dir.path().join("PCSE00001");
        std::fs::create_dir_all(vita.join("sce_sys")).unwrap();
        std::fs::write(vita.join("sce_sys/param.sfo"), b"x").unwrap();
        let context = Context::inspect_with(vita, &f.roots);
        assert!(context.relevant());
        assert_eq!(context.status_text(), EXTRACTED_FOLDER_TEXT);
        assert!(context.inputs(&CanonicalIdentityStatus::Unknown).is_none());
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
