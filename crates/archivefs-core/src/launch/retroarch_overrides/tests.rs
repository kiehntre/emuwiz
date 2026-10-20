use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;
use std::time::{Duration, Instant};

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
        discover_retroarch_overrides(&self.config, "Snes9x", &self.content)
    }
}

#[test]
fn no_overrides_means_nothing_applies() {
    let world = World::new();
    assert_eq!(world.discover(), OverrideDiscovery::default());
    assert!(world.discover().refusal().is_none());
}

#[test]
fn a_missing_override_folder_is_not_a_problem() {
    let world = World::new();
    fs::remove_dir_all(world.config.parent().unwrap().join("config")).unwrap();
    assert_eq!(world.discover(), OverrideDiscovery::default());
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
    assert_eq!(world.discover(), OverrideDiscovery::default());
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
fn an_unusable_library_name_or_unreadable_config_is_refused() {
    let world = World::new();
    for name in ["", "..", "a/b", "x\ny"] {
        let found = discover_retroarch_overrides(&world.config, name, &world.content);
        assert!(found.refusal().is_some(), "{name:?}");
    }
    let link = world.dir.path().join("linked.cfg");
    symlink(&world.config, &link).unwrap();
    assert!(
        discover_retroarch_overrides(&link, "Snes9x", &world.content)
            .refusal()
            .is_some()
    );
}
