use super::*;
use std::os::unix::fs::PermissionsExt;

fn sfo(entries: &[(&str, &str)]) -> Vec<u8> {
    let index_table_len = entries.len() * 16;
    let key_table_start = 20 + index_table_len;
    let mut key_table = Vec::new();
    let mut key_offsets = Vec::new();
    for (key, _) in entries {
        key_offsets.push(key_table.len() as u16);
        key_table.extend_from_slice(key.as_bytes());
        key_table.push(0);
    }
    while key_table.len() % 4 != 0 {
        key_table.push(0);
    }
    let data_table_start = key_table_start + key_table.len();
    let mut data_table = Vec::new();
    let mut data_offsets = Vec::new();
    for (_, value) in entries {
        data_offsets.push(data_table.len() as u32);
        data_table.extend_from_slice(value.as_bytes());
        data_table.push(0);
    }
    let mut output = vec![0u8; 20];
    output[0..4].copy_from_slice(&[0, b'P', b'S', b'F']);
    output[4..8].copy_from_slice(&0x0101_u32.to_le_bytes());
    output[8..12].copy_from_slice(&(key_table_start as u32).to_le_bytes());
    output[12..16].copy_from_slice(&(data_table_start as u32).to_le_bytes());
    output[16..20].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    for ((_, value), (key_offset, data_offset)) in entries
        .iter()
        .zip(key_offsets.into_iter().zip(data_offsets))
    {
        let mut index = [0u8; 16];
        index[0..2].copy_from_slice(&key_offset.to_le_bytes());
        index[2..4].copy_from_slice(&0x0204_u16.to_le_bytes());
        let value_len = (value.len() + 1) as u32;
        index[4..8].copy_from_slice(&value_len.to_le_bytes());
        index[8..12].copy_from_slice(&value_len.to_le_bytes());
        index[12..16].copy_from_slice(&data_offset.to_le_bytes());
        output.extend_from_slice(&index);
    }
    output.extend_from_slice(&key_table);
    output.extend_from_slice(&data_table);
    output
}

struct Fixture {
    dir: tempfile::TempDir,
    roots: Vita3kProfileDiscoveryRoots,
    config_root: PathBuf,
    executable: PathBuf,
}

fn fake_vita3k(dir: &Path) -> PathBuf {
    let path = dir.join("Vita3K");
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn install(config_root: &Path, dir_name: &str, title_id: &str, title: &str, category: &str) {
    let sys = config_root.join("ux0/app").join(dir_name).join("sce_sys");
    fs::create_dir_all(&sys).unwrap();
    fs::write(
        sys.join("param.sfo"),
        sfo(&[
            ("TITLE_ID", title_id),
            ("TITLE", title),
            ("CATEGORY", category),
        ]),
    )
    .unwrap();
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let config_root = dir.path().join("profile-a");
    fs::create_dir_all(config_root.join("vs0/sys/external")).unwrap();
    let executable = fake_vita3k(dir.path());
    let roots = Vita3kProfileDiscoveryRoots {
        home: dir.path().join("home"),
        xdg_data_home: dir.path().join("xdg"),
        explicit_configuration_roots: vec![config_root.clone()],
        portable_configuration_roots: Vec::new(),
        explicit_executables: vec![executable.clone()],
        known_version_outputs: Default::default(),
    };
    Fixture {
        dir,
        roots,
        config_root,
        executable,
    }
}

fn request_for(inventory: &Vita3kInventory, title_id: &str) -> Vita3kInstalledLaunchRequest {
    Vita3kInstalledLaunchRequest {
        entry: inventory
            .entries
            .iter()
            .find(|entry| entry.title_id == title_id)
            .expect("title listed")
            .clone(),
    }
}

fn wait(mut process: WatchedProcess) -> bool {
    for _ in 0..200 {
        if process.poll().is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

#[test]
fn an_installed_title_is_listed_with_a_namespaced_installation_identity() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Real Game", "gd");
    fs::create_dir_all(f.config_root.join("ux0/license/app/PCSE00001")).unwrap();
    let inventory = scan_vita3k_installed_titles(&f.roots);
    assert_eq!(inventory.entries.len(), 1);
    let entry = &inventory.entries[0];
    assert_eq!(entry.blocker, None);
    assert_eq!(entry.display_title.as_deref(), Some("Real Game"));
    let CanonicalIdentityStatus::Resolved(identity) = entry.identity_status() else {
        panic!()
    };
    assert_eq!(identity.platform_id, "PlayStation Vita");
    assert_eq!(identity.game_key, "vita3k-installed:PCSE00001");
}

#[test]
fn a_misleading_title_name_never_selects_or_authorises_anything() {
    let f = fixture();
    install(
        &f.config_root,
        "PCSE00001",
        "PCSE00001",
        "PCSE00002 Totally Other Game",
        "HB",
    );
    let inventory = scan_vita3k_installed_titles(&f.roots);
    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(inventory.entries[0].title_id, "PCSE00001");
}

#[test]
fn a_folder_whose_sfo_names_a_different_title_is_not_listed() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00009", "Impostor", "HB");
    let inventory = scan_vita3k_installed_titles(&f.roots);
    assert!(inventory.entries.is_empty());
    assert_eq!(inventory.skipped.len(), 1);
}

#[test]
fn symlinked_title_folders_and_symlinked_param_files_are_not_listed() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Real", "HB");
    let app = f.config_root.join("ux0/app");
    std::os::unix::fs::symlink(app.join("PCSE00001"), app.join("PCSE00002")).unwrap();
    install(&f.config_root, "PCSE00003", "PCSE00003", "Linked sfo", "HB");
    let sfo_path = app.join("PCSE00003/sce_sys/param.sfo");
    let real = f.dir.path().join("elsewhere.sfo");
    fs::rename(&sfo_path, &real).unwrap();
    std::os::unix::fs::symlink(&real, &sfo_path).unwrap();
    let ids: Vec<_> = scan_vita3k_installed_titles(&f.roots)
        .entries
        .into_iter()
        .map(|e| e.title_id)
        .collect();
    assert_eq!(ids, vec!["PCSE00001".to_string()]);
}

#[test]
fn missing_licence_and_missing_firmware_block_the_title() {
    let f = fixture();
    install(
        &f.config_root,
        "PCSE00001",
        "PCSE00001",
        "Needs licence",
        "gd",
    );
    let inventory = scan_vita3k_installed_titles(&f.roots);
    assert!(
        inventory.entries[0]
            .blocker
            .as_deref()
            .unwrap()
            .contains("licence")
    );
    let err =
        preflight_and_launch_vita3k_installed(&request_for(&inventory, "PCSE00001"), &f.roots)
            .err()
            .unwrap();
    assert!(matches!(err, Vita3kInstalledLaunchError::Refused(_)));

    let g = fixture();
    fs::remove_dir_all(g.config_root.join("vs0")).unwrap();
    install(&g.config_root, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let inventory = scan_vita3k_installed_titles(&g.roots);
    assert_eq!(inventory.entries[0].firmware, Vita3kFirmwareState::Unknown);
    assert!(
        inventory.entries[0]
            .blocker
            .as_deref()
            .unwrap()
            .contains("firmware")
    );
    let err =
        preflight_and_launch_vita3k_installed(&request_for(&inventory, "PCSE00001"), &g.roots)
            .err()
            .unwrap();
    assert!(matches!(err, Vita3kInstalledLaunchError::Refused(_)));
}

#[test]
fn a_symlinked_sce_sys_folder_is_not_listed() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Real", "HB");
    let sys = f.config_root.join("ux0/app/PCSE00001/sce_sys");
    let elsewhere = f.dir.path().join("elsewhere-sys");
    fs::rename(&sys, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &sys).unwrap();
    let inventory = scan_vita3k_installed_titles(&f.roots);
    assert!(inventory.entries.is_empty());
    assert!(inventory.skipped[0].contains("sce_sys"));
}

#[test]
fn a_title_id_that_could_pass_as_an_option_is_not_listed() {
    let f = fixture();
    install(
        &f.config_root,
        "-PCSE0001",
        "-PCSE0001",
        "Option-like",
        "HB",
    );
    let inventory = scan_vita3k_installed_titles(&f.roots);
    assert!(inventory.entries.is_empty());
    assert!(inventory.skipped[0].contains("not a usable title id"));
}

#[test]
fn a_changed_vita3k_configuration_after_listing_is_refused() {
    let f = fixture();
    fs::write(f.config_root.join("config.yml"), "# fixture config\n").unwrap();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let request = request_for(&scan_vita3k_installed_titles(&f.roots), "PCSE00001");
    assert!(request.entry.config_identity.is_some());
    fs::write(
        f.config_root.join("config.yml"),
        "# fixture config\n# edited\n",
    )
    .unwrap();
    assert!(matches!(
        preflight_and_launch_vita3k_installed(&request, &f.roots),
        Err(Vita3kInstalledLaunchError::Changed(_))
    ));
}

#[test]
fn a_title_in_two_installations_is_ambiguous_and_refused() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let second = f.dir.path().join("profile-b");
    fs::create_dir_all(second.join("ux0")).unwrap();
    install(&second, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let mut roots = f.roots.clone();
    roots.explicit_configuration_roots.push(second);
    let inventory = scan_vita3k_installed_titles(&roots);
    assert_eq!(inventory.entries.len(), 2);
    assert!(
        inventory
            .entries
            .iter()
            .all(|e| e.blocker.as_deref().unwrap().contains("more than one"))
    );
    let err = preflight_and_launch_vita3k_installed(&request_for(&inventory, "PCSE00001"), &roots)
        .err()
        .unwrap();
    assert!(matches!(err, Vita3kInstalledLaunchError::Ambiguous(_)));
}

#[test]
fn launch_runs_the_existing_preflight_and_spawns() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let inventory = scan_vita3k_installed_titles(&f.roots);
    let process =
        preflight_and_launch_vita3k_installed(&request_for(&inventory, "PCSE00001"), &f.roots)
            .unwrap();
    assert!(wait(process));
}

#[test]
fn changed_param_sfo_after_listing_is_refused() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let request = request_for(&scan_vita3k_installed_titles(&f.roots), "PCSE00001");
    install(
        &f.config_root,
        "PCSE00001",
        "PCSE00001",
        "Renamed (same id)",
        "HB",
    );
    assert!(matches!(
        preflight_and_launch_vita3k_installed(&request, &f.roots),
        Err(Vita3kInstalledLaunchError::Changed(_))
    ));
}

#[test]
fn replaced_title_folder_with_same_bytes_is_refused() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let request = request_for(&scan_vita3k_installed_titles(&f.roots), "PCSE00001");
    let root = f.config_root.join("ux0/app/PCSE00001");
    let moved = f.dir.path().join("moved-away");
    fs::rename(&root, &moved).unwrap();
    // Recreate an identical-looking folder: new inode.
    install(&f.config_root, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let err = preflight_and_launch_vita3k_installed(&request, &f.roots)
        .err()
        .unwrap();
    assert!(
        matches!(err, Vita3kInstalledLaunchError::Changed(_)),
        "{err:?}"
    );
}

#[test]
fn removed_title_is_missing() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let request = request_for(&scan_vita3k_installed_titles(&f.roots), "PCSE00001");
    fs::remove_dir_all(f.config_root.join("ux0/app/PCSE00001")).unwrap();
    assert!(matches!(
        preflight_and_launch_vita3k_installed(&request, &f.roots),
        Err(Vita3kInstalledLaunchError::Missing(_))
    ));
}

#[test]
fn replaced_or_removed_executable_is_refused() {
    let f = fixture();
    install(&f.config_root, "PCSE00001", "PCSE00001", "Homebrew", "HB");
    let request = request_for(&scan_vita3k_installed_titles(&f.roots), "PCSE00001");
    fs::remove_file(&f.executable).unwrap();
    // Gone: the installation is no longer eligible, so the title is missing.
    assert!(matches!(
        preflight_and_launch_vita3k_installed(&request, &f.roots),
        Err(Vita3kInstalledLaunchError::Missing(_))
    ));
    // Replaced by a new file (new inode): the executable drifted.
    fake_vita3k(f.dir.path());
    assert!(matches!(
        preflight_and_launch_vita3k_installed(&request, &f.roots),
        Err(Vita3kInstalledLaunchError::Changed(_))
    ));
}

#[test]
fn install_packages_and_loose_files_are_never_listed() {
    let f = fixture();
    fs::create_dir_all(f.config_root.join("ux0/app")).unwrap();
    fs::write(f.config_root.join("ux0/app/PCSE00001.vpk"), b"pk").unwrap();
    fs::create_dir_all(f.config_root.join("ux0/app/NOT-A-TITLE")).unwrap();
    assert!(scan_vita3k_installed_titles(&f.roots).entries.is_empty());
}

#[test]
fn no_vita3k_installation_gives_an_empty_inventory_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let roots = Vita3kProfileDiscoveryRoots {
        home: dir.path().join("home"),
        xdg_data_home: dir.path().join("xdg"),
        explicit_configuration_roots: vec![dir.path().join("cfg")],
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: Default::default(),
    };
    let inventory = scan_vita3k_installed_titles(&roots);
    assert!(inventory.entries.is_empty());
    assert!(inventory.profiles.iter().all(|p| !p.eligible));
}

#[test]
fn a_configured_pref_path_with_sibling_ux0_and_vs0_lists_titles() {
    let f = fixture();
    let pref = f.dir.path().join("pref");
    fs::create_dir_all(pref.join("vs0/sys/external")).unwrap();
    fs::write(
        f.config_root.join("config.yml"),
        format!("pref-path: {}\n", pref.display()),
    )
    .unwrap();
    // The profile root's own ux0 is ignored; the configured pref root rules.
    install(&pref, "PCSE00001", "PCSE00001", "Configured layout", "HB");
    install(&f.config_root, "PCSE00002", "PCSE00002", "Wrong tree", "HB");
    let inventory = scan_vita3k_installed_titles(&f.roots);
    let ids: Vec<_> = inventory
        .entries
        .iter()
        .map(|e| e.title_id.as_str())
        .collect();
    assert_eq!(ids, vec!["PCSE00001"]);
    assert_eq!(inventory.entries[0].blocker, None);
}
