//! Normal-user "core folder" repair layer for RetroArch in Emulator Setup.
//!
//! Everything here is a pure projection of evidence the existing
//! `archivefs-core` RetroArch discovery already gathered - the same
//! discovery `main.rs::start_retroarch_profile_scan` runs through
//! [`archivefs_core::patch_manager::discover_retroarch_cheat_setup_profiles_with_core_directory_override`].
//! This module never scans the filesystem itself beyond a single
//! `metadata` check on a directory the user just picked, and it never
//! reads or writes `retroarch.cfg`. It also never re-implements core
//! enumeration or platform mapping: the launch chain
//! (`matching_retroarch_cores` -> `build_retroarch_candidates` ->
//! `build_launch_plan` -> `gamer_readiness`) keeps owning launch
//! readiness; this layer only translates its inputs into plain wording.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use archivefs_core::emulator_environment::retroarch::CoreInfoFinding;
use archivefs_core::launch::retroarch_platform_candidate;
use archivefs_core::patch_manager::RetroArchCheatSetupDiscovery;

use crate::ui::components::StatusTone;

/// The stable diagnostic code the core layer emits when an EmuWiz
/// core-directory override points at something it cannot read. Kept as a
/// named constant so the normal-user translation and the technical-details
/// view agree on exactly one string.
pub(crate) const OVERRIDE_UNUSABLE_DIAGNOSTIC: &str = "retroarch_core_directory_override_unusable";
/// The companion "override accepted" diagnostic code.
pub(crate) const OVERRIDE_APPLIED_DIAGNOSTIC: &str = "retroarch_core_directory_override_applied";

/// Which core folder RetroArch discovery is currently pointed at, in words
/// a non-technical user can act on - never a `ResolutionState` name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CoreFolderMode {
    /// No EmuWiz override: RetroArch's own configured directory is used.
    Automatic,
    /// An explicit EmuWiz override is persisted.
    Custom(PathBuf),
}

impl CoreFolderMode {
    pub(crate) fn from_override(value: Option<&Path>) -> Self {
        match value {
            Some(path) => Self::Custom(path.to_path_buf()),
            None => Self::Automatic,
        }
    }

    /// The plain, normal-user label for the active source.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Automatic => "Automatic core folder",
            Self::Custom(_) => "Custom core folder",
        }
    }

    pub(crate) fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    pub(crate) fn custom_path(&self) -> Option<&Path> {
        match self {
            Self::Custom(path) => Some(path.as_path()),
            Self::Automatic => None,
        }
    }
}

/// Verdict for a directory the user just picked, decided *before* it is
/// persisted. [`PickedCoreFolder::Directory`] is the only value that leads
/// to a save + rescan; anything else is reported and never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickedCoreFolder {
    /// Exists and is a real directory.
    Directory,
    /// Missing, a file, a broken symlink, or otherwise not a usable
    /// directory.
    Unusable,
}

/// A single `metadata` probe - the only filesystem access this module
/// performs. Whether the directory actually contains usable libretro
/// cores is decided later, by the real discovery pass, never here.
pub(crate) fn classify_picked_core_folder(path: &Path) -> PickedCoreFolder {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => PickedCoreFolder::Directory,
        _ => PickedCoreFolder::Unusable,
    }
}

/// Distinct usable libretro cores across every discovered profile. An
/// override forces the same directory on each profile, so cores are keyed
/// by stem to avoid double-counting. "Usable" means the `*_libretro.so`
/// was found *and* its `.info` metadata parsed
/// ([`CoreInfoFinding::Found`]) - the exact precondition
/// [`retroarch_platform_candidate`] needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CoreInventory {
    pub(crate) usable_cores: usize,
    pub(crate) total_cores: usize,
    /// Distinct canonical platforms at least one usable core maps to.
    pub(crate) mapped_platforms: usize,
}

pub(crate) fn core_inventory(discovery: &RetroArchCheatSetupDiscovery) -> CoreInventory {
    let mut usable: BTreeSet<&str> = BTreeSet::new();
    let mut total: BTreeSet<&str> = BTreeSet::new();
    let mut platforms: BTreeSet<&'static str> = BTreeSet::new();
    for profile in &discovery.environment.profiles {
        for core in &profile.cores {
            total.insert(core.core_stem.as_str());
            if matches!(core.info, CoreInfoFinding::Found { .. }) {
                usable.insert(core.core_stem.as_str());
            }
            if let Some(platform) = retroarch_platform_candidate(&core.info) {
                platforms.insert(platform);
            }
        }
    }
    CoreInventory {
        usable_cores: usable.len(),
        total_cores: total.len(),
        mapped_platforms: platforms.len(),
    }
}

/// Whether any discovered profile carries `code` among its diagnostics.
/// The override provenance lands on the affected *profile* (report-level
/// diagnostics are untouched), so this looks across every profile.
pub(crate) fn discovery_has_profile_diagnostic(
    discovery: &RetroArchCheatSetupDiscovery,
    code: &str,
) -> bool {
    discovery
        .environment
        .profiles
        .iter()
        .flat_map(|profile| profile.diagnostics.iter())
        .any(|diagnostic| diagnostic.code == code)
}

/// The RetroArch scan state, borrowed, in the shape this projection needs.
/// `main.rs` builds it from its `RetroArchProfilesState` so the readiness
/// wording stays a pure function that tests can drive directly.
pub(crate) enum CoreFolderScan<'a> {
    NotScanned,
    Scanning,
    /// The scan errored. The detail string is carried for parity with the
    /// source state and for tests that assert it never leaks into the
    /// one-line summary; the readiness projection deliberately does not
    /// place it in user-facing text.
    Failed(#[allow(dead_code)] &'a str),
    Ready(&'a RetroArchCheatSetupDiscovery),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoreFolderReadinessKind {
    NotScanned,
    Scanning,
    Ready,
    NeedsSetup,
    CustomFolderUnavailable,
    Error,
}

/// The normal-user summary for the RetroArch core-folder card: one badge,
/// an optional headline, and exactly one sentence. Blocker IDs, raw
/// `PathFinding` data and long paths are deliberately absent - those live
/// under Technical details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoreFolderReadiness {
    pub(crate) kind: CoreFolderReadinessKind,
    pub(crate) badge_label: &'static str,
    pub(crate) badge_tone: StatusTone,
    pub(crate) headline: Option<&'static str>,
    pub(crate) sentence: String,
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Pure projection of scan state + active mode into normal-user wording.
pub(crate) fn core_folder_readiness(
    scan: CoreFolderScan<'_>,
    mode: &CoreFolderMode,
) -> CoreFolderReadiness {
    match scan {
        CoreFolderScan::NotScanned => CoreFolderReadiness {
            kind: CoreFolderReadinessKind::NotScanned,
            badge_label: "Not checked",
            badge_tone: StatusTone::Pending,
            headline: None,
            sentence: "EmuWiz has not checked which RetroArch cores are available yet.".to_string(),
        },
        CoreFolderScan::Scanning => CoreFolderReadiness {
            kind: CoreFolderReadinessKind::Scanning,
            badge_label: "Scanning",
            badge_tone: StatusTone::Active,
            headline: None,
            sentence: "Scanning RetroArch cores...".to_string(),
        },
        CoreFolderScan::Failed(_) => CoreFolderReadiness {
            kind: CoreFolderReadinessKind::Error,
            badge_label: "Needs attention",
            badge_tone: StatusTone::Blocked,
            headline: Some("RetroArch core check could not finish"),
            sentence: "EmuWiz could not check RetroArch's cores. Open Technical details for the \
                       exact error."
                .to_string(),
        },
        CoreFolderScan::Ready(discovery) => {
            let inventory = core_inventory(discovery);
            if inventory.usable_cores > 0 {
                let sentence = if mode.is_custom() {
                    format!(
                        "Core folder accepted. EmuWiz found {} usable libretro core{}.",
                        inventory.usable_cores,
                        plural(inventory.usable_cores)
                    )
                } else {
                    format!(
                        "EmuWiz found {} usable libretro core{} in RetroArch's configured folder.",
                        inventory.usable_cores,
                        plural(inventory.usable_cores)
                    )
                };
                CoreFolderReadiness {
                    kind: CoreFolderReadinessKind::Ready,
                    badge_label: "Ready",
                    badge_tone: StatusTone::Success,
                    headline: None,
                    sentence,
                }
            } else if mode.is_custom()
                && discovery_has_profile_diagnostic(discovery, OVERRIDE_UNUSABLE_DIAGNOSTIC)
            {
                CoreFolderReadiness {
                    kind: CoreFolderReadinessKind::CustomFolderUnavailable,
                    badge_label: "Needs setup",
                    badge_tone: StatusTone::Blocked,
                    headline: Some("Custom core folder is unavailable"),
                    sentence: "EmuWiz cannot read the core folder you selected. Choose another \
                               folder or reset to automatic detection."
                        .to_string(),
                }
            } else if mode.is_custom() {
                CoreFolderReadiness {
                    kind: CoreFolderReadinessKind::NeedsSetup,
                    badge_label: "Needs setup",
                    badge_tone: StatusTone::Warning,
                    headline: None,
                    sentence: "No usable libretro cores were found in this folder.".to_string(),
                }
            } else {
                CoreFolderReadiness {
                    kind: CoreFolderReadinessKind::NeedsSetup,
                    badge_label: "Needs setup",
                    badge_tone: StatusTone::Warning,
                    headline: None,
                    sentence: "RetroArch is installed, but EmuWiz cannot find a usable libretro \
                               core in the folder it is currently using."
                        .to_string(),
                }
            }
        }
    }
}

/// Plain one-liners for the RetroArch diagnostic codes this repair flow can
/// surface. Used only to annotate the raw code under Technical details -
/// the normal summary never shows either the code or this text verbatim.
pub(crate) fn humanize_retroarch_diagnostic(code: &str) -> Option<&'static str> {
    match code {
        OVERRIDE_UNUSABLE_DIAGNOSTIC => Some("The custom core folder could not be read."),
        OVERRIDE_APPLIED_DIAGNOSTIC => Some("Using your custom core folder."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
    use archivefs_core::emulator_environment::retroarch::DiscoveryEnvironment;
    use archivefs_core::patch_manager::discover_retroarch_cheat_setup_profiles_with_core_directory_override;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "emuwiz-core-folder-{label}-{}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        fn mkdir(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(&path).unwrap();
            path
        }

        fn env(&self) -> DiscoveryEnvironment {
            DiscoveryEnvironment {
                home: Some(self.root.clone().into_os_string()),
                xdg_config_home: None,
                path: None,
                user_flatpak_root: self.root.join("user-flatpak"),
                system_flatpak_root: self.root.join("system-flatpak"),
                app_image_search_roots: Vec::new(),
                desktop_file_roots: Vec::new(),
            }
        }

        /// A native `retroarch.cfg` whose `libretro_directory` is a real
        /// PUAE-only directory and whose `libretro_info_path` holds a Game
        /// Boy `.info` file (matched by stem). Returns a separate override
        /// directory containing only a `gambatte` core.
        fn with_gb_override(&self) -> PathBuf {
            self.write(
                ".config/retroarch/retroarch.cfg",
                &format!(
                    "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\n",
                    self.root.join("configured-cores").display(),
                    self.root.join("info").display()
                ),
            );
            self.write("configured-cores/puae_libretro.so", "stub");
            self.write(
                "info/gambatte.info",
                "display_name = \"Nintendo - Game Boy / Color (Gambatte)\"\n\
                 systemname = \"Game Boy/Game Boy Color\"\n\
                 database = \"Nintendo - Game Boy|Nintendo - Game Boy Color\"\n\
                 supported_extensions = \"gb|gbc|dmg\"\n",
            );
            self.write("override-cores/gambatte_libretro.so", "stub");
            self.root.join("override-cores")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn discover(
        env: &DiscoveryEnvironment,
        override_dir: Option<&Path>,
    ) -> RetroArchCheatSetupDiscovery {
        discover_retroarch_cheat_setup_profiles_with_core_directory_override(
            &HostReadOnlyFilesystem,
            env,
            None,
            override_dir,
        )
        .unwrap()
    }

    #[test]
    fn mode_label_is_plain_words_never_an_enum_name() {
        assert_eq!(
            CoreFolderMode::from_override(None).label(),
            "Automatic core folder"
        );
        assert_eq!(
            CoreFolderMode::from_override(Some(Path::new("/x/cores"))).label(),
            "Custom core folder"
        );
        assert!(!CoreFolderMode::from_override(None).is_custom());
        assert!(CoreFolderMode::from_override(Some(Path::new("/x"))).is_custom());
    }

    #[test]
    fn a_missing_or_file_pick_is_unusable_a_real_directory_is_a_directory() {
        let fixture = Fixture::new("pick");
        assert_eq!(
            classify_picked_core_folder(&fixture.root.join("nope")),
            PickedCoreFolder::Unusable
        );
        fixture.write("a-file", "x");
        assert_eq!(
            classify_picked_core_folder(&fixture.root.join("a-file")),
            PickedCoreFolder::Unusable
        );
        let dir = fixture.mkdir("real-dir");
        assert_eq!(
            classify_picked_core_folder(&dir),
            PickedCoreFolder::Directory
        );
    }

    #[test]
    fn automatic_puae_only_folder_reads_as_needs_setup_not_custom_unavailable() {
        let fixture = Fixture::new("auto-needs-setup");
        let _override = fixture.with_gb_override();
        let discovery = discover(&fixture.env(), None);
        let readiness = core_folder_readiness(
            CoreFolderScan::Ready(&discovery),
            &CoreFolderMode::Automatic,
        );
        assert_eq!(readiness.kind, CoreFolderReadinessKind::NeedsSetup);

        assert!(readiness.sentence.contains("RetroArch is installed"));
    }

    #[test]
    fn a_valid_gb_override_folder_reads_as_ready_and_maps_a_platform() {
        let fixture = Fixture::new("override-ready");
        let override_dir = fixture.with_gb_override();
        let discovery = discover(&fixture.env(), Some(&override_dir));

        let inventory = core_inventory(&discovery);
        assert_eq!(inventory.usable_cores, 1);
        assert!(
            inventory.mapped_platforms >= 1,
            "the existing platform mapping must recognise the override's core"
        );

        let mode = CoreFolderMode::Custom(override_dir);
        let readiness = core_folder_readiness(CoreFolderScan::Ready(&discovery), &mode);
        assert_eq!(readiness.kind, CoreFolderReadinessKind::Ready);
        assert!(readiness.sentence.starts_with("Core folder accepted."));
    }

    #[test]
    fn an_empty_override_folder_is_needs_setup_and_never_claims_ready() {
        let fixture = Fixture::new("override-empty");
        let _override = fixture.with_gb_override();
        let empty = fixture.mkdir("empty-cores");
        let discovery = discover(&fixture.env(), Some(&empty));

        assert_eq!(core_inventory(&discovery).usable_cores, 0);
        let mode = CoreFolderMode::Custom(empty);
        let readiness = core_folder_readiness(CoreFolderScan::Ready(&discovery), &mode);
        assert_ne!(readiness.kind, CoreFolderReadinessKind::Ready);

        assert!(
            readiness
                .sentence
                .contains("No usable libretro cores were found")
        );
    }

    #[test]
    fn a_missing_override_folder_reads_as_custom_folder_unavailable() {
        let fixture = Fixture::new("override-missing");
        let _override = fixture.with_gb_override();
        let missing = fixture.root.join("was-removed");
        let discovery = discover(&fixture.env(), Some(&missing));

        assert!(discovery_has_profile_diagnostic(
            &discovery,
            OVERRIDE_UNUSABLE_DIAGNOSTIC
        ));
        let mode = CoreFolderMode::Custom(missing);
        let readiness = core_folder_readiness(CoreFolderScan::Ready(&discovery), &mode);
        assert_eq!(
            readiness.kind,
            CoreFolderReadinessKind::CustomFolderUnavailable
        );
        assert_eq!(
            readiness.headline,
            Some("Custom core folder is unavailable")
        );
    }

    #[test]
    fn scanning_and_not_scanned_and_error_states_have_plain_sentences() {
        let scanning = core_folder_readiness(CoreFolderScan::Scanning, &CoreFolderMode::Automatic);
        assert_eq!(scanning.kind, CoreFolderReadinessKind::Scanning);
        assert_eq!(scanning.sentence, "Scanning RetroArch cores...");

        let not_scanned =
            core_folder_readiness(CoreFolderScan::NotScanned, &CoreFolderMode::Automatic);
        assert_eq!(not_scanned.kind, CoreFolderReadinessKind::NotScanned);

        let failed = core_folder_readiness(
            CoreFolderScan::Failed("permission denied on /x"),
            &CoreFolderMode::Automatic,
        );
        assert_eq!(failed.kind, CoreFolderReadinessKind::Error);
        // The raw error string is never surfaced in the one-line sentence.
        assert!(!failed.sentence.contains("permission denied"));
    }

    #[test]
    fn raw_diagnostic_codes_are_translated_but_only_known_ones() {
        assert!(humanize_retroarch_diagnostic(OVERRIDE_UNUSABLE_DIAGNOSTIC).is_some());
        assert!(humanize_retroarch_diagnostic(OVERRIDE_APPLIED_DIAGNOSTIC).is_some());
        assert!(humanize_retroarch_diagnostic("some_unrelated_code").is_none());
    }

    #[test]
    fn override_directory_naturally_yields_a_game_boy_retroarch_launch_candidate() {
        use archivefs_core::launch::{
            CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
            LaunchTarget, ResolvedIdentity, build_launch_plan,
        };

        let fixture = Fixture::new("override-launch");
        let override_dir = fixture.with_gb_override();
        let discovery = discover(&fixture.env(), Some(&override_dir));

        let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Game Boy".to_string(),
            game_key: "override-launch-key".to_string(),
        });
        let content = LaunchContentRef {
            kind: Some(LaunchContentKind::Cartridge),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(fixture.root.join("aladdin.gb")),
            requires_mount: false,
            provenance: "test loose Game Boy ROM".to_string(),
        };
        let plan = build_launch_plan(&identity, &content, &[], &discovery.environment, &[]);

        assert!(
            plan.candidates.iter().any(|candidate| matches!(
                &candidate.target,
                LaunchTarget::RetroArchCore { core_stem, .. } if core_stem == "gambatte"
            )),
            "the launch planner must naturally see the override's Game Boy core"
        );
    }
}
