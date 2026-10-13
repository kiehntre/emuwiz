//! Fixtures live in a temp HOME; nothing reads the real machine.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use archivefs_core::emulator_inventory::InventoryEmulator;
use archivefs_core::emulator_profile_resolver::{
    CandidateIdentity, EmulatorSelectionMode, remember_selection_to,
};
use tempfile::TempDir;

use super::super::{
    ProfileResolutionPresentation, ResolutionHealth, ResolverSubject, present_resolution_for,
};
use super::*;

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

fn profile(root: &Path, used: bool) {
    write(&root.join("PSP/SYSTEM/ppsspp.ini"), "[General]\n");
    if used {
        write(&root.join("PSP/SAVEDATA/ULUS10041/DATA.BIN"), "x");
    }
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

    fn bin(&self) -> PathBuf {
        self.home.join(".local/bin")
    }

    /// Native executable plus the per-user XDG configuration profile.
    fn native(&self, used: bool) -> (PathBuf, PathBuf) {
        let exe = self.bin().join("ppsspp");
        executable(&exe);
        let root = self.home.join(".config/ppsspp");
        profile(&root, used);
        (exe, root)
    }

    /// An AppImage (portable provenance) with a caller-supplied portable root.
    fn portable(&self) -> (PathBuf, PathBuf) {
        let exe = self.home.join("Applications/PPSSPP.AppImage");
        executable(&exe);
        let root = self.home.join("portable-ppsspp");
        profile(&root, true);
        (exe, root)
    }

    /// A deployed Flatpak plus its sandbox profile.
    fn flatpak(&self) -> (PathBuf, PathBuf) {
        let program = self.bin().join("flatpak");
        executable(&program);
        write(
            &self
                .home
                .join(".local/share/flatpak/app/org.ppsspp.PPSSPP/current/active/metadata"),
            "[Application]\nname=org.ppsspp.PPSSPP\n",
        );
        let root = self.home.join(".var/app/org.ppsspp.PPSSPP/config/ppsspp");
        profile(&root, true);
        (program, root)
    }

    fn inputs(
        &self,
        portable: &[&Path],
        explicit_exes: &[&Path],
        explicit_roots: &[&Path],
    ) -> PpssppResolutionInputs {
        PpssppResolutionInputs {
            roots: PpssppProfileDiscoveryRoots {
                home: self.home.clone(),
                xdg_config_home: self.home.join(".config"),
                xdg_data_home: self.home.join(".local/share"),
                explicit_configuration_roots: explicit_roots
                    .iter()
                    .map(|p| p.to_path_buf())
                    .collect(),
                portable_configuration_roots: portable.iter().map(|p| p.to_path_buf()).collect(),
                explicit_executables: explicit_exes.iter().map(|p| p.to_path_buf()).collect(),
                known_version_outputs: BTreeMap::new(),
                appimage_directory: None,
                path_override: Some(vec![self.bin()]),
            },
            desktop_directories: vec![self.home.join(".local/share/applications")],
            selection_path: Some(self.selection.clone()),
        }
    }

    fn remember(&self, mode: &EmulatorSelectionMode) {
        remember_selection_to(&self.selection, InventoryEmulator::Ppsspp, mode).unwrap();
    }
}

fn identity(exe: &Path, root: &Path) -> CandidateIdentity {
    CandidateIdentity {
        emulator: InventoryEmulator::Ppsspp,
        executable: Some(exe.to_path_buf()),
        profile_root: root.to_path_buf(),
    }
}

fn present_inputs(inputs: &PpssppResolutionInputs) -> ProfileResolutionPresentation {
    present_resolution_for(&compute_ppsspp_resolution(inputs), ResolverSubject::Ppsspp)
}

fn present(machine: &Machine) -> ProfileResolutionPresentation {
    present_inputs(&machine.inputs(&[], &[], &[]))
}

fn all_text(view: &ProfileResolutionPresentation) -> String {
    let mut text = vec![view.headline.clone()];
    text.extend(view.why.iter().cloned());
    text.extend(view.warnings.iter().cloned());
    text.extend(view.next_steps.iter().cloned());
    text.extend(view.candidates.iter().cloned());
    text.join("\n")
}

#[test]
fn a_healthy_native_installation_shows_its_exact_pair_and_why() {
    let machine = Machine::new();
    let (exe, root) = machine.native(true);
    let view = present(&machine);
    assert_eq!(view.health, ResolutionHealth::Healthy, "{view:#?}");
    assert_eq!(view.executable.as_deref(), Some(exe.as_path()));
    assert_eq!(view.profile_directory.as_deref(), Some(root.as_path()));
    assert_eq!(view.mode_label, "AUTO");
    assert!(view.layout.unwrap().starts_with("User configuration"));
    assert!(view.headline.contains("PPSSPP"), "{view:#?}");
    assert!(!view.why.is_empty(), "{view:#?}");
    assert_eq!(view.details_id, "ppsspp-resolution-candidates");
    assert!(!all_text(&view).contains("DuckStation"), "{view:#?}");
}

#[test]
fn nothing_installed_gives_next_steps_and_creates_nothing() {
    let machine = Machine::new();
    let before = tree(&machine.home);
    let view = present(&machine);
    assert_eq!(view.health, ResolutionHealth::NothingUsable, "{view:#?}");
    assert!(view.executable.is_none() && view.profile_directory.is_none());
    assert!(view.next_steps.len() >= 2);
    assert!(view.headline.contains("PPSSPP"));
    assert!(!all_text(&view).contains("DuckStation"), "{view:#?}");
    assert_eq!(tree(&machine.home), before);
    assert!(!machine.selection.exists());
}

#[test]
fn two_equally_good_profiles_are_ambiguous_and_nothing_is_guessed() {
    let machine = Machine::new();
    let exe = machine.bin().join("ppsspp");
    executable(&exe);
    profile(&machine.home.join(".config/ppsspp"), false);
    profile(&machine.home.join(".local/share/ppsspp"), false);
    let view = present(&machine);
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
    assert!(view.headline.contains("will not guess"));
}

#[test]
fn a_stale_preferred_choice_falls_back_with_a_warning_and_is_not_erased() {
    let machine = Machine::new();
    let (exe, root) = machine.native(true);
    let gone = machine.home.join("Applications/Gone/PPSSPP.AppImage");
    machine.remember(&EmulatorSelectionMode::Preferred(identity(
        &gone,
        &machine.home.join("Applications/Gone"),
    )));
    let before = fs::read(&machine.selection).unwrap();
    let view = present(&machine);
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
    assert_eq!(fs::read(&machine.selection).unwrap(), before);
}

#[test]
fn a_forced_choice_that_is_gone_is_refused_without_substitution() {
    let machine = Machine::new();
    machine.native(true);
    let gone = machine.home.join("Applications/Gone/PPSSPP.AppImage");
    machine.remember(&EmulatorSelectionMode::Forced(identity(
        &gone,
        &machine.home.join("Applications/Gone"),
    )));
    let view = present(&machine);
    assert_eq!(view.mode_label, "FORCED");
    assert_eq!(view.health, ResolutionHealth::Unavailable, "{view:#?}");
    assert!(view.executable.is_none() && view.profile_directory.is_none());
    assert!(view.headline.contains("nothing else was substituted"));
    assert!(view.headline.contains("PPSSPP"));
}

#[test]
fn a_forced_pair_ppsspps_own_rule_does_not_allow_is_refused() {
    let machine = Machine::new();
    let (_, root) = machine.native(true);
    let stranger = machine.home.join("Applications/other/ppsspp");
    executable(&stranger);
    machine.remember(&EmulatorSelectionMode::Forced(identity(&stranger, &root)));
    let view = present_inputs(&machine.inputs(&[], &[], &[]));
    assert_eq!(view.health, ResolutionHealth::Unavailable, "{view:#?}");
    assert!(view.profile_directory.is_none(), "{view:#?}");
    assert!(
        view.why
            .iter()
            .any(|l| l.contains("different configuration folder")),
        "{view:#?}"
    );
}

#[test]
fn a_forced_profile_with_a_missing_main_config_names_ppsspp_ini() {
    let machine = Machine::new();
    let (exe, root) = machine.native(true);
    fs::remove_file(root.join("PSP/SYSTEM/ppsspp.ini")).unwrap();
    machine.remember(&EmulatorSelectionMode::Forced(identity(&exe, &root)));
    let view = present(&machine);
    assert_eq!(view.health, ResolutionHealth::Unavailable, "{view:#?}");
    let text = all_text(&view);
    assert!(text.contains("ppsspp.ini"), "{text}");
    assert!(!text.contains("settings.ini"), "{text}");
}

#[test]
fn portable_flatpak_and_explicit_layouts_are_labelled_by_how_they_pair() {
    let machine = Machine::new();
    let (exe, root) = machine.portable();
    let view = present_inputs(&machine.inputs(&[&root], &[], &[]));
    assert_eq!(view.health, ResolutionHealth::Healthy, "{view:#?}");
    assert!(view.layout.unwrap().starts_with("Portable"), "{view:#?}");
    assert_eq!(view.executable.as_deref(), Some(exe.as_path()));
    assert_eq!(view.profile_directory.as_deref(), Some(root.as_path()));

    let flat = Machine::new();
    let (program, sandbox) = flat.flatpak();
    let view = present(&flat);
    assert_eq!(view.health, ResolutionHealth::Healthy, "{view:#?}");
    assert_eq!(view.layout, Some("Flatpak sandbox"));
    assert_eq!(view.executable.as_deref(), Some(program.as_path()));
    assert_eq!(view.profile_directory.as_deref(), Some(sandbox.as_path()));

    let explicit = Machine::new();
    let exe = explicit.home.join("tools/PPSSPPSDL");
    executable(&exe);
    let root = explicit.home.join("custom-ppsspp");
    profile(&root, true);
    let view = present_inputs(&explicit.inputs(&[], &[&exe], &[&root]));
    assert_eq!(view.layout, Some("Folder you chose"), "{view:#?}");
    assert_eq!(view.executable.as_deref(), Some(exe.as_path()));
    assert_eq!(view.profile_directory.as_deref(), Some(root.as_path()));
}

#[test]
fn a_flatpak_profile_without_the_flatpak_is_unusable_not_recommended() {
    let machine = Machine::new();
    let root = machine
        .home
        .join(".var/app/org.ppsspp.PPSSPP/config/ppsspp");
    profile(&root, true);
    let view = present(&machine);
    assert_ne!(view.health, ResolutionHealth::Healthy, "{view:#?}");
    assert!(view.executable.is_none(), "{view:#?}");
    assert!(
        view.candidates
            .iter()
            .any(|l| l.starts_with("Unusable") && l.contains("Flatpak is not installed")),
        "{view:#?}"
    );
}

#[test]
fn alternatives_are_listed_in_the_technical_details() {
    let machine = Machine::new();
    let (exe, _) = machine.native(true);
    // A second, unused profile for the same program loses on use history.
    profile(&machine.home.join(".local/share/ppsspp"), false);
    let view = present(&machine);
    assert_eq!(view.health, ResolutionHealth::Healthy, "{view:#?}");
    assert_eq!(view.executable.as_deref(), Some(exe.as_path()));
    assert!(view.candidates.iter().any(|l| l.starts_with("In use")));
    assert!(view.candidates.iter().any(|l| l.starts_with("Not used")));
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
fn resolution_does_not_modify_configuration_or_create_folders() {
    let machine = Machine::new();
    machine.native(true);
    machine.flatpak();
    let (_, portable_root) = machine.portable();
    machine.remember(&EmulatorSelectionMode::Preferred(identity(
        &machine.home.join("Applications/Gone/PPSSPP.AppImage"),
        &machine.home.join("Applications/Gone"),
    )));
    let before_home = tree(&machine.home);
    let before_file = fs::read(&machine.selection).unwrap();
    let inputs = machine.inputs(&[&portable_root], &[], &[]);
    let _ = compute_ppsspp_resolution(&inputs);
    let _ = compute_ppsspp_resolution(&inputs);
    assert_eq!(tree(&machine.home), before_home);
    assert_eq!(fs::read(&machine.selection).unwrap(), before_file);
}

#[test]
fn an_unreadable_remembered_choice_is_reported_not_hidden() {
    let machine = Machine::new();
    machine.native(true);
    write(
        &machine.selection,
        "[[emulator_selection]]\nemulator = \"PPSSPP\"\nmode = \"forced\"\n",
    );
    let view = present(&machine);
    assert_eq!(view.mode_label, "AUTO");
    assert!(
        view.warnings
            .iter()
            .any(|w| w.contains("could not be read")),
        "{view:#?}"
    );
    assert_eq!(view.health, ResolutionHealth::HealthyWithNotes);
}

#[test]
fn a_duckstation_preference_does_not_leak_into_the_ppsspp_answer() {
    let machine = Machine::new();
    machine.native(true);
    remember_selection_to(
        &machine.selection,
        InventoryEmulator::DuckStation,
        &EmulatorSelectionMode::Forced(CandidateIdentity {
            emulator: InventoryEmulator::DuckStation,
            executable: Some(machine.home.join("nowhere/DuckStation.AppImage")),
            profile_root: machine.home.join("nowhere"),
        }),
    )
    .unwrap();
    let view = present(&machine);
    assert_eq!(view.mode_label, "AUTO");
    assert_eq!(view.health, ResolutionHealth::Healthy, "{view:#?}");
}
