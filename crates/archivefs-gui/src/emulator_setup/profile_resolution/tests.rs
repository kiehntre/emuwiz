//! Fixtures live in a temp HOME; nothing reads the real machine.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use archivefs_core::emulator_inventory::InventoryEmulator;
use archivefs_core::emulator_profile_resolver::{
    CandidateIdentity, EmulatorSelectionMode, remember_selection_to,
};
use archivefs_core::patch_manager::DuckStationProfileDiscoveryRoots;
use tempfile::TempDir;

use super::*;

const COMPLETE: &str = "[Main]\nSetupWizardIncomplete = false\n";

struct Machine {
    _dir: TempDir,
    home: PathBuf,
    selection: PathBuf,
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn executable(path: &Path) {
    write(path, "#!/bin/sh\n");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let selection = dir.path().join("config/emulator_selection.toml");
        Self {
            _dir: dir,
            home,
            selection,
        }
    }

    /// Portable AppImage: marker + initialised settings + play history.
    fn portable(&self, folder: &str) -> (PathBuf, PathBuf) {
        let root = self.home.join("Applications").join(folder);
        let exe = root.join("DuckStation.AppImage");
        executable(&exe);
        write(&root.join("portable.txt"), "");
        write(
            &root.join("settings.ini"),
            &format!("{COMPLETE}\n[BIOS]\nSearchDirectory = bios\n"),
        );
        write(
            &root.join("playtime.dat"),
            "SCUS-94601   341   1783636844\n",
        );
        fs::create_dir_all(root.join("bios")).unwrap();
        (exe, root)
    }

    /// AppImage with no marker: uses the per-user default profile.
    fn user_profile_install(&self, folder: &str, initialised: bool) -> (PathBuf, PathBuf) {
        let exe = self
            .home
            .join("Applications")
            .join(folder)
            .join("DuckStation.AppImage");
        executable(&exe);
        let root = self.home.join(".local/share/duckstation");
        if initialised {
            write(
                &root.join("settings.ini"),
                &format!("{COMPLETE}\n[BIOS]\nSearchDirectory = bios\n"),
            );
            fs::create_dir_all(root.join("bios")).unwrap();
        }
        (exe, root)
    }

    fn inputs(&self, explicit: &[&Path]) -> DuckStationResolutionInputs {
        DuckStationResolutionInputs {
            roots: DuckStationProfileDiscoveryRoots {
                home: self.home.clone(),
                xdg_config_home: self.home.join(".config"),
                xdg_data_home: self.home.join(".local/share"),
                xdg_config_home_explicit: false,
                explicit_configuration_roots: Vec::new(),
                portable_configuration_roots: Vec::new(),
                explicit_executables: explicit.iter().map(|p| p.to_path_buf()).collect(),
                known_version_outputs: BTreeMap::new(),
                appimage_directory: None,
                path_override: Some(Vec::new()),
            },
            desktop_directories: vec![self.home.join(".local/share/applications")],
            selection_path: Some(self.selection.clone()),
        }
    }

    fn remember(&self, mode: &EmulatorSelectionMode) {
        remember_selection_to(&self.selection, InventoryEmulator::DuckStation, mode).unwrap();
    }
}

fn identity(exe: &Path, root: &Path) -> CandidateIdentity {
    CandidateIdentity {
        emulator: InventoryEmulator::DuckStation,
        executable: Some(exe.to_path_buf()),
        profile_root: root.to_path_buf(),
    }
}

fn present(machine: &Machine, explicit: &[&Path]) -> ProfileResolutionPresentation {
    present_resolution(&compute_duckstation_resolution(&machine.inputs(explicit)))
}

#[test]
fn one_healthy_installation_shows_its_exact_pair_and_why() {
    let machine = Machine::new();
    let (exe, root) = machine.portable("DuckStation");
    let view = present(&machine, &[&exe]);
    assert_eq!(view.health, ResolutionHealth::Healthy, "{view:#?}");
    assert_eq!(view.executable.as_deref(), Some(exe.as_path()));
    assert_eq!(view.profile_directory.as_deref(), Some(root.as_path()));
    assert_eq!(view.mode_label, "AUTO");
    assert!(
        view.why
            .iter()
            .any(|line| line.contains("only installation")),
        "{view:#?}"
    );
}

#[test]
fn a_clear_winner_among_several_candidates_is_chosen_and_explained() {
    let machine = Machine::new();
    let (a_exe, a_root) = machine.portable("DuckStation");
    // A second install on the unused per-user profile (no history).
    let (b_exe, _) = machine.user_profile_install("other", true);
    let view = present(&machine, &[&a_exe, &b_exe]);
    assert_eq!(view.health, ResolutionHealth::Healthy, "{view:#?}");
    assert_eq!(view.executable.as_deref(), Some(a_exe.as_path()));
    assert_eq!(view.profile_directory.as_deref(), Some(a_root.as_path()));
    assert!(
        view.why.iter().any(|line| line.contains("strongest")),
        "{view:#?}"
    );
    assert!(
        view.candidates
            .iter()
            .any(|line| line.starts_with("Not used")),
        "{view:#?}"
    );
}

#[test]
fn genuine_ambiguity_selects_nothing_and_asks() {
    let machine = Machine::new();
    let (a_exe, _) = machine.portable("DuckStationA");
    let (b_exe, _) = machine.portable("DuckStationB");
    let view = present(&machine, &[&a_exe, &b_exe]);
    assert_eq!(view.health, ResolutionHealth::Ambiguous, "{view:#?}");
    assert!(view.executable.is_none() && view.profile_directory.is_none());
    assert!(
        view.candidates
            .iter()
            .filter(|l| l.starts_with("Tied"))
            .count()
            >= 2,
        "{view:#?}"
    );
    assert!(!view.next_steps.is_empty());
}

#[test]
fn a_stale_preferred_choice_falls_back_with_an_explicit_warning() {
    let machine = Machine::new();
    let (exe, root) = machine.portable("DuckStation");
    let gone = machine.home.join("Applications/Gone/DuckStation.AppImage");
    machine.remember(&EmulatorSelectionMode::Preferred(identity(
        &gone,
        &machine.home.join("Applications/Gone"),
    )));
    let view = present(&machine, &[&exe]);
    assert_eq!(view.mode_label, "PREFERRED");
    assert_eq!(view.health, ResolutionHealth::HealthyWithNotes, "{view:#?}");
    assert_eq!(view.executable.as_deref(), Some(exe.as_path()));
    assert_eq!(view.profile_directory.as_deref(), Some(root.as_path()));
    assert!(
        view.warnings
            .iter()
            .any(|w| w.contains("preferred") && w.contains("not used")),
        "{view:#?}"
    );
    // The preference is shown as kept, not erased: the file is unchanged.
    let stored = fs::read_to_string(&machine.selection).unwrap();
    assert!(stored.contains(&gone.display().to_string()));
}

#[test]
fn a_forced_choice_that_cannot_be_used_is_refused_without_substitution() {
    let machine = Machine::new();
    let (exe, _) = machine.portable("DuckStation");
    let gone = machine.home.join("Applications/Gone/DuckStation.AppImage");
    machine.remember(&EmulatorSelectionMode::Forced(identity(
        &gone,
        &machine.home.join("Applications/Gone"),
    )));
    let view = present(&machine, &[&exe]);
    assert_eq!(view.health, ResolutionHealth::Unavailable, "{view:#?}");
    assert!(
        view.executable.is_none() && view.profile_directory.is_none(),
        "{view:#?}"
    );
    assert!(view.headline.contains("nothing else was substituted"));
    assert!(
        view.why
            .iter()
            .any(|l| l.contains(&gone.display().to_string()))
    );
}

#[test]
fn portable_and_user_profiles_are_labelled_by_how_they_pair() {
    let machine = Machine::new();
    let (p_exe, _) = machine.portable("Portable");
    let portable = present(&machine, &[&p_exe]);
    assert!(
        portable.layout.unwrap().starts_with("Portable"),
        "{portable:#?}"
    );

    let other = Machine::new();
    let (u_exe, u_root) = other.user_profile_install("Plain", true);
    let user = present(&other, &[&u_exe]);
    assert!(
        user.layout.unwrap().starts_with("User configuration"),
        "{user:#?}"
    );
    assert_eq!(user.profile_directory.as_deref(), Some(u_root.as_path()));
}

#[test]
fn a_stale_pairing_is_never_shown_as_the_selected_pair() {
    let machine = Machine::new();
    let (exe, _) = machine.portable("DuckStation");
    // Forced: this executable, but with a profile it would not use.
    let wrong_root = machine.home.join("elsewhere");
    write(&wrong_root.join("settings.ini"), COMPLETE);
    machine.remember(&EmulatorSelectionMode::Forced(identity(&exe, &wrong_root)));
    let view = present(&machine, &[&exe]);
    assert_eq!(view.health, ResolutionHealth::Unavailable, "{view:#?}");
    assert!(view.profile_directory.is_none(), "{view:#?}");
}

#[test]
fn nothing_installed_gives_next_steps_and_creates_nothing() {
    let machine = Machine::new();
    let before = tree(&machine.home);
    let view = present(&machine, &[]);
    assert_eq!(view.health, ResolutionHealth::NothingUsable, "{view:#?}");
    assert!(view.next_steps.len() >= 2);
    assert_eq!(tree(&machine.home), before);
    assert!(!machine.selection.exists());
}

fn tree(root: &Path) -> Vec<(PathBuf, u64)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let stamp = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as u64);
            out.push((path.clone(), meta.len() ^ stamp));
            if meta.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

#[test]
fn discovery_does_not_modify_configuration_or_create_folders() {
    let machine = Machine::new();
    let (a_exe, _) = machine.portable("A");
    let (b_exe, _) = machine.user_profile_install("B", false);
    machine.remember(&EmulatorSelectionMode::Preferred(identity(
        &machine.home.join("Applications/Gone/DuckStation.AppImage"),
        &machine.home.join("Applications/Gone"),
    )));
    let before_home = tree(&machine.home);
    let before_file = fs::read(&machine.selection).unwrap();
    let _ = compute_duckstation_resolution(&machine.inputs(&[&a_exe, &b_exe]));
    let _ = compute_duckstation_resolution(&machine.inputs(&[&a_exe, &b_exe]));
    assert_eq!(tree(&machine.home), before_home);
    assert_eq!(fs::read(&machine.selection).unwrap(), before_file);
    assert!(!machine.home.join(".local/share/duckstation").exists());
}

#[test]
fn an_unreadable_remembered_choice_is_reported_not_hidden() {
    let machine = Machine::new();
    let (exe, _) = machine.portable("DuckStation");
    write(
        &machine.selection,
        "[[emulator_selection]]\nemulator = \"DuckStation\"\nmode = \"forced\"\n",
    );
    let view = present(&machine, &[&exe]);
    assert_eq!(view.mode_label, "AUTO");
    assert!(
        view.warnings
            .iter()
            .any(|w| w.contains("could not be read")),
        "{view:#?}"
    );
    assert_eq!(view.health, ResolutionHealth::HealthyWithNotes);
}

// ---- Launch consumes the shared resolver --------------------------------

mod launch {
    use super::*;
    use archivefs_core::patch_manager::resolve_duckstation_native_launch_binding;

    impl Machine {
        /// An executable using the per-user default profile, which legacy
        /// Launch binding can start. `complete` records finished setup;
        /// `history` adds play history.
        fn default_install(
            &self,
            folder: &str,
            complete: bool,
            history: bool,
        ) -> (PathBuf, PathBuf) {
            let exe = self
                .home
                .join("Applications")
                .join(folder)
                .join("DuckStation.AppImage");
            executable(&exe);
            let root = self.home.join(".local/share/duckstation");
            let main = if complete {
                COMPLETE
            } else {
                "[Main]\nSomething = 1\n"
            };
            write(&root.join("settings.ini"), main);
            if history {
                write(
                    &root.join("playtime.dat"),
                    "SCUS-94601   341   1783636844\n",
                );
            }
            (exe, root)
        }

        /// A portable install the resolver can pick but legacy Launch
        /// binding cannot start when it is only an explicit executable.
        /// A portable install that was never set up or used.
        fn portable_weak(&self, folder: &str) -> (PathBuf, PathBuf) {
            let root = self.home.join("Applications").join(folder);
            let exe = root.join("DuckStation.AppImage");
            executable(&exe);
            write(&root.join("portable.txt"), "");
            write(&root.join("settings.ini"), "[Main]\nSomething = 1\n");
            (exe, root)
        }

        fn portable_explicit(&self, folder: &str) -> (PathBuf, PathBuf) {
            self.portable(folder)
        }
    }

    fn selection(machine: &Machine, explicit: &[&Path]) -> DuckStationLaunchSelection {
        select_for_launch(&machine.inputs(explicit))
    }

    fn authorised(selection: &DuckStationLaunchSelection) -> (&Path, &Path) {
        match &selection.authority {
            DuckStationLaunchAuthority::Authorised {
                executable,
                profile_root,
                ..
            } => (executable, profile_root),
            other => panic!("expected an authorised pair, got {other:?}"),
        }
    }

    fn refused(selection: &DuckStationLaunchSelection) -> &str {
        match &selection.authority {
            DuckStationLaunchAuthority::Refused { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// The binding Launch would build for the single offered profile.
    fn launch_executable(selection: &DuckStationLaunchSelection) -> PathBuf {
        assert_eq!(selection.discovery.profiles.len(), 1, "{selection:#?}");
        resolve_duckstation_native_launch_binding(
            &selection.discovery.profiles[0],
            &selection.roots,
        )
        .expect("launch binding")
        .executable
    }

    #[test]
    fn auto_launches_exactly_the_resolvers_pair() {
        let machine = Machine::new();
        let (exe, root) = machine.default_install("DuckStation", true, true);
        let selection = selection(&machine, &[&exe]);
        assert_eq!(authorised(&selection), (exe.as_path(), root.as_path()));
        assert_eq!(launch_executable(&selection), exe);
    }

    #[test]
    fn auto_with_a_clear_winner_launches_the_winner_not_an_ambiguous_legacy_pick() {
        let machine = Machine::new();
        let (a_exe, a_root) = machine.default_install("Main", true, true);
        let (b_exe, _) = machine.portable_weak("Spare");
        // Legacy discovery alone sees two executables for the default root.
        let legacy = archivefs_core::patch_manager::discover_duckstation_profiles(
            &machine.inputs(&[&a_exe, &b_exe]).roots,
        );
        let blocked = resolve_duckstation_native_launch_binding(
            &legacy.profiles[0],
            &machine.inputs(&[&a_exe, &b_exe]).roots,
        );
        assert!(blocked.is_err(), "legacy alone is ambiguous: {blocked:?}");

        let selection = selection(&machine, &[&a_exe, &b_exe]);
        assert_eq!(authorised(&selection), (a_exe.as_path(), a_root.as_path()));
        assert_eq!(launch_executable(&selection), a_exe);
    }

    #[test]
    fn preferred_is_honoured_over_a_stronger_candidate() {
        let machine = Machine::new();
        let (a_exe, a_root) = machine.default_install("Weak", false, false);
        let (b_exe, _) = machine.portable_explicit("Strong");
        machine.remember(&EmulatorSelectionMode::Preferred(identity(&a_exe, &a_root)));
        let selection = selection(&machine, &[&a_exe, &b_exe]);
        assert_eq!(authorised(&selection), (a_exe.as_path(), a_root.as_path()));
        assert_eq!(launch_executable(&selection), a_exe);
    }

    #[test]
    fn forced_is_honoured_exactly() {
        let machine = Machine::new();
        let (a_exe, a_root) = machine.default_install("Weak", false, false);
        let (b_exe, _) = machine.portable_explicit("Strong");
        machine.remember(&EmulatorSelectionMode::Forced(identity(&a_exe, &a_root)));
        let selection = selection(&machine, &[&a_exe, &b_exe]);
        assert_eq!(authorised(&selection), (a_exe.as_path(), a_root.as_path()));
        assert_eq!(launch_executable(&selection), a_exe);
    }

    #[test]
    fn a_resolver_pair_legacy_launch_cannot_start_is_refused_not_substituted() {
        // AUTO picks the stronger portable explicit executable; legacy
        // Launch binding cannot start it. The weaker, launchable install must
        // NOT be used instead.
        let machine = Machine::new();
        let (a_exe, _) = machine.default_install("Weak", false, false);
        let (b_exe, b_root) = machine.portable_explicit("Strong");
        let selection = selection(&machine, &[&a_exe, &b_exe]);
        assert!(
            refused(&selection).contains("cannot start exactly that pair"),
            "{selection:#?}"
        );
        assert!(refused(&selection).contains(&b_root.display().to_string()));
        assert!(selection.discovery.profiles.is_empty(), "{selection:#?}");
    }

    #[test]
    fn an_unavailable_preferred_target_falls_back_deterministically_with_a_note() {
        let machine = Machine::new();
        let (exe, root) = machine.default_install("DuckStation", true, true);
        let gone = machine.home.join("Applications/Gone");
        machine.remember(&EmulatorSelectionMode::Preferred(identity(
            &gone.join("DuckStation.AppImage"),
            &gone,
        )));
        let first = selection(&machine, &[&exe]);
        let second = selection(&machine, &[&exe]);
        assert_eq!(authorised(&first), (exe.as_path(), root.as_path()));
        assert_eq!(first.authority, second.authority);
        match &first.authority {
            DuckStationLaunchAuthority::Authorised { mode, notes, .. } => {
                assert_eq!(*mode, "PREFERRED");
                assert!(notes.iter().any(|n| n.contains("preferred")), "{notes:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_unavailable_forced_target_blocks_launch_without_falling_back() {
        let machine = Machine::new();
        let (exe, _) = machine.default_install("DuckStation", true, true); // healthy
        let gone = machine.home.join("Applications/Gone");
        machine.remember(&EmulatorSelectionMode::Forced(identity(
            &gone.join("DuckStation.AppImage"),
            &gone,
        )));
        let selection = selection(&machine, &[&exe]);
        assert!(refused(&selection).contains("FORCED"), "{selection:#?}");
        assert!(
            selection.discovery.profiles.is_empty(),
            "a healthy legacy profile must not be offered: {selection:#?}"
        );
    }

    #[test]
    fn ambiguity_and_nothing_installed_refuse_with_the_resolvers_reason() {
        let machine = Machine::new();
        let (a_exe, _) = machine.portable("DuckStationA");
        let (b_exe, _) = machine.portable("DuckStationB");
        let ambiguous = selection(&machine, &[&a_exe, &b_exe]);
        assert!(
            refused(&ambiguous).contains("equally good"),
            "{ambiguous:#?}"
        );
        assert!(ambiguous.discovery.profiles.is_empty());

        let empty = Machine::new();
        let none = selection(&empty, &[]);
        assert!(
            refused(&none).contains("No DuckStation installation"),
            "{none:#?}"
        );
        assert!(none.discovery.profiles.is_empty());
    }

    #[test]
    fn an_unreadable_remembered_choice_is_carried_into_the_launch_decision() {
        let machine = Machine::new();
        let (exe, _) = machine.default_install("DuckStation", true, true);
        write(
            &machine.selection,
            "[[emulator_selection]]\nemulator = \"DuckStation\"\nmode = \"forced\"\n",
        );
        let selection = selection(&machine, &[&exe]);
        match &selection.authority {
            DuckStationLaunchAuthority::Authorised { mode, notes, .. } => {
                assert_eq!(*mode, "AUTO");
                assert!(notes.iter().any(|n| n.contains("could not be read")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn selecting_for_launch_changes_nothing_on_disk() {
        let machine = Machine::new();
        let (a_exe, a_root) = machine.default_install("A", false, false);
        let (b_exe, _) = machine.portable_explicit("B");
        machine.remember(&EmulatorSelectionMode::Preferred(identity(&a_exe, &a_root)));
        let before = tree(&machine.home);
        let stored = fs::read(&machine.selection).unwrap();
        let _ = selection(&machine, &[&a_exe, &b_exe]);
        assert_eq!(tree(&machine.home), before);
        assert_eq!(fs::read(&machine.selection).unwrap(), stored);
    }

    #[test]
    fn a_preferred_fallback_produces_a_launch_visible_notice_naming_the_pair() {
        let machine = Machine::new();
        let (exe, root) = machine.default_install("DuckStation", true, true);
        let gone = machine.home.join("Applications/Gone");
        machine.remember(&EmulatorSelectionMode::Preferred(identity(
            &gone.join("DuckStation.AppImage"),
            &gone,
        )));
        let selection = selection(&machine, &[&exe]);
        let notices = selection.authority.launch_notices();
        let joined = notices.join("\n");
        assert!(
            joined.contains("preferred DuckStation is unavailable"),
            "{joined}"
        );
        assert!(joined.contains(&exe.display().to_string()), "{joined}");
        assert!(joined.contains(&root.display().to_string()), "{joined}");
        assert!(
            joined.contains("saved preference was not changed"),
            "{joined}"
        );
    }

    #[test]
    fn normal_selections_need_no_fallback_notice() {
        let machine = Machine::new();
        let (exe, root) = machine.default_install("DuckStation", true, true);
        assert!(
            selection(&machine, &[&exe])
                .authority
                .launch_notices()
                .is_empty()
        );
        machine.remember(&EmulatorSelectionMode::Preferred(identity(&exe, &root)));
        assert!(
            selection(&machine, &[&exe])
                .authority
                .launch_notices()
                .is_empty()
        );
        machine.remember(&EmulatorSelectionMode::Forced(identity(&exe, &root)));
        assert!(
            selection(&machine, &[&exe])
                .authority
                .launch_notices()
                .is_empty()
        );
    }

    #[test]
    fn refusals_state_that_no_other_installation_will_be_used() {
        let machine = Machine::new();
        let (exe, _) = machine.default_install("DuckStation", true, true);
        let gone = machine.home.join("Applications/Gone");
        machine.remember(&EmulatorSelectionMode::Forced(identity(
            &gone.join("DuckStation.AppImage"),
            &gone,
        )));
        let notices = selection(&machine, &[&exe]).authority.launch_notices();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("will not use any other installation"),
            "{notices:?}"
        );
    }

    #[test]
    fn no_legacy_duckstation_discovery_remains_in_the_launch_input_builder() {
        let source = include_str!("../controller.rs");
        assert!(
            !source.contains("discover_duckstation_profiles("),
            "Launch must obtain DuckStation profiles only through select_for_launch"
        );
        assert!(source.contains("select_for_launch("));
    }
}
