use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;
use std::time::{Duration, Instant};

fn nothing_found(found: &OverrideDiscovery) -> bool {
    found.findings.is_empty()
        && found.unverifiable.is_empty()
        && found.configuration_problem.is_none()
}

struct World {
    dir: tempfile::TempDir,
    config: PathBuf,
    core_dir: PathBuf,
    content: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("retroarch");
        fs::create_dir_all(home.join("config/Snes9x")).unwrap();
        let config = home.join("retroarch.cfg");
        fs::write(&config, "video_driver = \"vulkan\"\n").unwrap();
        let content_dir = dir.path().join("roms").join("SNES Games");
        fs::create_dir_all(&content_dir).unwrap();
        let content = content_dir.join("Super Game (USA).sfc");
        fs::write(&content, b"rom").unwrap();
        let core_dir = home.join("config/Snes9x");
        Self {
            dir,
            config,
            core_dir,
            content,
        }
    }

    fn discover(&self) -> OverrideDiscovery {
        discover_retroarch_overrides(&self.config, "Snes9x", &self.content, Some(self.dir.path()))
    }
}

#[test]
fn no_overrides_means_nothing_applies() {
    let world = World::new();
    assert!(nothing_found(&world.discover()), "{:?}", world.discover());
    assert!(world.discover().refusal().is_none());
}

#[test]
fn a_missing_override_folder_is_not_a_problem() {
    let world = World::new();
    fs::remove_dir_all(world.config.parent().unwrap().join("config")).unwrap();
    assert!(nothing_found(&world.discover()), "{:?}", world.discover());
}

#[test]
fn core_folder_and_game_overrides_that_set_pinned_keys_are_found() {
    let world = World::new();
    fs::write(
        world.core_dir.join("Snes9x.cfg"),
        "input_player1_a = \"x\"\nsavefile_directory = \"/elsewhere\"\n",
    )
    .unwrap();
    fs::write(
        world.core_dir.join("SNES Games.cfg"),
        "# cheat_database_path = \"/commented\"\nvideo_fullscreen = \"true\"\n",
    )
    .unwrap();
    fs::write(
        world.core_dir.join("Super Game (USA).cfg"),
        "apply_cheats_after_load = \"false\"\nconfig_save_on_exit = \"true\"\n",
    )
    .unwrap();
    let found = world.discover();
    assert!(found.unverifiable.is_empty());
    let by_name = |name: &str| {
        found
            .findings
            .iter()
            .find(|f| f.path.file_name().unwrap().to_str().unwrap() == name)
    };
    assert_eq!(
        by_name("Snes9x.cfg").unwrap().keys,
        vec!["savefile_directory"]
    );
    assert!(
        by_name("SNES Games.cfg").is_none(),
        "a commented key and unrelated keys are not findings"
    );
    let game = by_name("Super Game (USA).cfg").unwrap();
    assert!(game.keys.contains(&"apply_cheats_after_load".to_string()));
    assert!(game.keys.contains(&"config_save_on_exit".to_string()));
}

#[test]
fn an_override_with_only_unrelated_keys_is_not_a_finding() {
    let world = World::new();
    fs::write(world.core_dir.join("Snes9x.cfg"), "video_scale = \"3\"\n").unwrap();
    assert!(nothing_found(&world.discover()), "{:?}", world.discover());
}

#[test]
fn a_linked_override_file_is_never_followed_and_blocks_cheats() {
    let world = World::new();
    let real = world.dir.path().join("elsewhere.cfg");
    fs::write(&real, "savefile_directory = \"/x\"\n").unwrap();
    symlink(&real, world.core_dir.join("Snes9x.cfg")).unwrap();
    let found = world.discover();
    assert!(
        found.findings.is_empty(),
        "the link target must not be read"
    );
    assert_eq!(found.unverifiable.len(), 1);
    assert!(
        found
            .refusal()
            .unwrap()
            .contains("could not be checked safely")
    );
}

#[test]
fn a_linked_override_folder_is_never_followed_and_blocks_cheats() {
    let world = World::new();
    let real = world.dir.path().join("real-snes");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("Snes9x.cfg"), "savefile_directory = \"/x\"\n").unwrap();
    fs::remove_dir_all(&world.core_dir).unwrap();
    symlink(&real, &world.core_dir).unwrap();
    let found = world.discover();
    assert!(found.findings.is_empty());
    assert_eq!(found.unverifiable.len(), 1);
    assert!(found.unverifiable[0].reason.contains("link"));
}

#[test]
fn a_linked_root_folder_is_never_followed() {
    let world = World::new();
    let root = world.config.parent().unwrap().join("config");
    let moved = world.dir.path().join("moved-config");
    fs::rename(&root, &moved).unwrap();
    symlink(&moved, &root).unwrap();
    let found = world.discover();
    assert!(found.findings.is_empty());
    assert_eq!(found.unverifiable.len(), 1);
}

#[test]
fn a_fifo_oversized_or_unreadable_override_is_unverifiable_without_blocking() {
    let world = World::new();
    let fifo = world.core_dir.join("Snes9x.cfg");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        world.core_dir.join("SNES Games.cfg"),
        "x = \"1\"\n".repeat(60_000),
    )
    .unwrap();
    let unreadable = world.core_dir.join("Super Game (USA).cfg");
    fs::write(&unreadable, "savefile_directory = \"/x\"\n").unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    let started = Instant::now();
    let found = world.discover();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "must not block on a pipe"
    );
    // Running as root can read the 0o000 file; the other two are still refused.
    assert!(found.unverifiable.len() >= 2, "{found:?}");
    assert!(found.refusal().is_some());
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn a_custom_override_folder_is_honoured_and_shortcuts_are_refused() {
    let world = World::new();
    let custom = world.dir.path().join("custom-overrides");
    fs::create_dir_all(custom.join("Snes9x")).unwrap();
    fs::write(
        custom.join("Snes9x").join("Snes9x.cfg"),
        "cheat_database_path = \"/x\"\n",
    )
    .unwrap();
    fs::write(
        &world.config,
        format!("rgui_config_directory = \"{}\"\n", custom.display()),
    )
    .unwrap();
    let found = world.discover();
    assert_eq!(found.findings.len(), 1);
    assert_eq!(found.findings[0].keys, vec!["cheat_database_path"]);
    fs::write(&world.config, "rgui_config_directory = \":\\\\config\"\n").unwrap();
    assert!(world.discover().refusal().is_some());
    fs::write(&world.config, "rgui_config_directory = \"relative/path\"\n").unwrap();
    assert!(world.discover().refusal().is_some());
}

#[test]
fn an_unusable_library_name_is_refused_with_its_own_explanation() {
    let world = World::new();
    for name in ["", "..", "a/b", "x\ny"] {
        let found = discover_retroarch_overrides(
            &world.config,
            name,
            &world.content,
            Some(world.dir.path()),
        );
        let why = found.refusal().expect(name);
        assert!(why.contains("library name"), "{name:?}: {why}");
    }
}

#[test]
fn a_leading_tilde_in_the_override_folder_is_expanded_like_retroarch() {
    let world = World::new();
    let overrides = world.dir.path().join("my-overrides");
    fs::create_dir_all(overrides.join("Snes9x")).unwrap();
    fs::write(
        overrides.join("Snes9x").join("Snes9x.cfg"),
        "savefile_directory = \"/x\"\n",
    )
    .unwrap();
    fs::write(
        &world.config,
        "rgui_config_directory = \"~/my-overrides/\"\n",
    )
    .unwrap();
    let found = world.discover();
    assert!(found.refusal().is_none(), "{:?}", found.refusal());
    assert_eq!(found.findings.len(), 1);
    fs::write(&world.config, "rgui_config_directory = \"~someone/else\"\n").unwrap();
    assert!(world.discover().refusal().unwrap().contains("~user"));
    // Without a known home the shortcut cannot be resolved.
    fs::write(
        &world.config,
        "rgui_config_directory = \"~/my-overrides/\"\n",
    )
    .unwrap();
    let blind = discover_retroarch_overrides(&world.config, "Snes9x", &world.content, None);
    assert!(blind.refusal().unwrap().contains("home folder is unknown"));
}

// ---- A managed (symlinked) settings file ------------------------------

/// Makes `retroarch.cfg` a link to a dotfiles copy inside the "home".
fn managed(world: &World, body: &str) -> PathBuf {
    let target = world.dir.path().join("dotfiles").join("retroarch.cfg");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, body).unwrap();
    let _ = fs::remove_file(&world.config);
    symlink(&target, &world.config).unwrap();
    target
}

#[test]
fn a_regular_settings_file_is_bound_and_rechecked() {
    let world = World::new();
    let found = world.discover();
    let binding = found.configuration.expect("a regular file is bound");
    assert!(!binding.was_link);
    binding.ensure_unchanged().unwrap();
    fs::write(&world.config, "video_driver = \"gl\"\n# changed\n").unwrap();
    assert!(
        binding
            .ensure_unchanged()
            .unwrap_err()
            .contains("changed since")
    );
}

#[test]
fn a_managed_link_inside_home_is_followed_read_and_bound() {
    let world = World::new();
    let overrides = world.dir.path().join("my-overrides");
    fs::create_dir_all(overrides.join("Snes9x")).unwrap();
    fs::write(
        overrides.join("Snes9x").join("Snes9x.cfg"),
        "savefile_directory = \"/x\"\n",
    )
    .unwrap();
    let target = managed(
        &world,
        &format!("rgui_config_directory = \"{}\"\n", overrides.display()),
    );
    let found = world.discover();
    assert!(
        found.configuration_problem.is_none(),
        "{:?}",
        found.configuration_problem
    );
    // The setting was read through the link: the override it points at was found.
    assert_eq!(found.findings.len(), 1);
    let binding = found.configuration.unwrap();
    assert!(binding.was_link);
    assert_eq!(binding.target, fs::canonicalize(&target).unwrap());
    binding.ensure_unchanged().unwrap();
}

#[test]
fn a_changed_or_repointed_managed_link_is_refused_at_launch() {
    let world = World::new();
    let target = managed(&world, "video_driver = \"vulkan\"\n");
    let binding = world.discover().configuration.unwrap();
    binding.ensure_unchanged().unwrap();
    // The target is rewritten.
    fs::write(&target, "video_driver = \"vulkan\"\n# edited\n").unwrap();
    assert!(binding.ensure_unchanged().is_err());
    // Re-bind, then repoint the link at a different (identical) file.
    let binding = world.discover().configuration.unwrap();
    let other = world.dir.path().join("dotfiles").join("other.cfg");
    fs::copy(&target, &other).unwrap();
    fs::remove_file(&world.config).unwrap();
    symlink(&other, &world.config).unwrap();
    assert!(binding.ensure_unchanged().is_err());
}

#[test]
fn a_broken_looping_or_escaping_link_is_refused_naming_the_link_not_an_override() {
    let world = World::new();
    fs::remove_file(&world.config).unwrap();
    // Broken.
    symlink(world.dir.path().join("nowhere.cfg"), &world.config).unwrap();
    let why = world.discover().refusal().unwrap();
    assert!(
        why.contains("symbolic link") && why.contains("is broken"),
        "{why}"
    );
    assert!(!why.contains("override"), "{why}");
    // Loop.
    fs::remove_file(&world.config).unwrap();
    let (a, b) = (
        world.dir.path().join("loop-a"),
        world.dir.path().join("loop-b"),
    );
    symlink(&b, &a).unwrap();
    symlink(&a, &b).unwrap();
    symlink(&a, &world.config).unwrap();
    let why = world.discover().refusal().unwrap();
    assert!(
        why.contains("symbolic link") && why.contains("loops"),
        "{why}"
    );
    // Escapes the home folder.
    fs::remove_file(&world.config).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("retroarch.cfg");
    fs::write(&outside_file, "video_driver = \"gl\"\n").unwrap();
    symlink(&outside_file, &world.config).unwrap();
    let why = world.discover().refusal().unwrap();
    assert!(why.contains("leaves your home folder"), "{why}");
}

#[test]
fn a_link_to_a_folder_a_writable_file_or_with_unknown_home_is_refused() {
    let world = World::new();
    fs::remove_file(&world.config).unwrap();
    let folder = world.dir.path().join("a-folder");
    fs::create_dir(&folder).unwrap();
    symlink(&folder, &world.config).unwrap();
    assert!(
        world
            .discover()
            .refusal()
            .unwrap()
            .contains("does not point to a regular file")
    );
    fs::remove_file(&world.config).unwrap();
    let target = managed(&world, "video_driver = \"gl\"\n");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        world
            .discover()
            .refusal()
            .unwrap()
            .contains("other users can write")
    );
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(world.discover().refusal().is_none());
    let blind = discover_retroarch_overrides(&world.config, "Snes9x", &world.content, None);
    assert!(blind.refusal().unwrap().contains("home folder is unknown"));
}

/// Opt-in: checks a real RetroArch profile. `EMUWIZ_REAL_RETROARCH_CFG` names
/// the `retroarch.cfg`; `HOME` is used as the home folder.
#[test]
#[ignore = "needs a real RetroArch profile: set EMUWIZ_REAL_RETROARCH_CFG"]
fn a_real_retroarch_profile_is_understood() {
    let cfg = PathBuf::from(std::env::var_os("EMUWIZ_REAL_RETROARCH_CFG").expect("set the cfg"));
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let found = discover_retroarch_overrides(
        &cfg,
        "Genesis Plus GX",
        Path::new("/roms/Sega/Some Game.md"),
        Some(&home),
    );
    eprintln!(
        "REAL: problem={:?} unverifiable={:?} findings={:?} bound={}",
        found.configuration_problem,
        found.unverifiable,
        found.findings,
        found.configuration.is_some()
    );
    assert!(found.configuration_problem.is_none());
}
