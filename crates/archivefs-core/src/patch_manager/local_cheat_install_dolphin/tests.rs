use super::*;
use crate::patch_manager::dolphin_gecko_install_plan::{
    DolphinInstallPreviewRequest, build_dolphin_install_preview, load_dolphin_ini,
};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "archivefs-dolphin-local-install-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn candidate(root: &Path, game_id: &str, revision: Option<u16>) -> DolphinCandidate {
    DolphinCandidate {
        game_id: game_id.to_string(),
        region: None,
        revision,
        path: root.join("GameSettings").join(format!("{game_id}.ini")),
        cheat_count: 0,
        enabled_count: 0,
        evidence: Vec::new(),
        installable: true,
    }
}

fn empty_destination(path: &Path) -> LoadedDolphinDestination {
    LoadedDolphinDestination {
        path: path.to_path_buf(),
        existed: false,
        digest: None,
        document: parse_dolphin_ini(""),
    }
}

const VALID_GECKO_INI: &str = "[Gecko]\n$Infinite Health\n042318AC 3B8003E7\n";
const VALID_AR_INI: &str = "[ActionReplay]\n$Infinite Ammo\n0224CD50 00003E7F\n";
const VALID_BOTH_INI: &str = "[Gecko]\n$Infinite Health\n042318AC 3B8003E7\n\n[ActionReplay]\n$Infinite Ammo\n0224CD50 00003E7F\n";

#[test]
fn valid_gecko_file_is_discovered_and_staged() {
    let root = temp("gecko-ok");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);

    let discovery =
        discover_local_dolphin_cheat_file(&file, &identity).expect("discovery succeeds");
    assert_eq!(discovery.kind, LocalDolphinCodeKind::Gecko);
    assert_eq!(discovery.gecko_codes.len(), 1);
    assert!(discovery.action_replay_codes.is_empty());

    let destination = empty_destination(&identity.path);
    let staging_root = root.join("staging");
    let staged = stage_local_dolphin_codes(&staging_root, &destination, &discovery).unwrap();
    assert!(staged.contents.contains("[Gecko]"));
    assert!(staged.contents.contains("Infinite Health"));
    assert!(staged.contents.contains("[Gecko_Enabled]"));

    let preview = build_dolphin_install_preview(&DolphinInstallPreviewRequest {
        selected_archive: root.join("game.iso"),
        configuration_path: root.clone(),
        game_id: identity.game_id.clone(),
        revision: identity.revision,
        staged,
    })
    .expect("preview succeeds");
    assert!(!preview.report.entries.is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn valid_gecko_file_applies_atomically_to_disk() {
    let root = temp("gecko-apply");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);
    let discovery = discover_local_dolphin_cheat_file(&file, &identity).unwrap();
    let destination = empty_destination(&identity.path);
    let staging_root = root.join("staging");
    let staged = stage_local_dolphin_codes(&staging_root, &destination, &discovery).unwrap();

    std::fs::create_dir_all(identity.path.parent().unwrap()).unwrap();
    std::fs::write(&identity.path, &staged.contents).unwrap();
    let applied = load_dolphin_ini(&identity.path).unwrap();
    assert_eq!(applied.document.gecko_codes.len(), 1);
    assert_eq!(
        applied.document.gecko_enabled_names,
        vec!["Infinite Health"]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn valid_action_replay_file_is_discovered_and_staged() {
    let root = temp("ar-ok");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, VALID_AR_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);

    let discovery =
        discover_local_dolphin_cheat_file(&file, &identity).expect("discovery succeeds");
    assert_eq!(discovery.kind, LocalDolphinCodeKind::ActionReplay);
    assert_eq!(discovery.action_replay_codes.len(), 1);

    let destination = empty_destination(&identity.path);
    let staging_root = root.join("staging");
    let staged = stage_local_dolphin_codes(&staging_root, &destination, &discovery).unwrap();
    assert!(staged.contents.contains("[ActionReplay]"));
    assert!(staged.contents.contains("Infinite Ammo"));
    assert!(staged.contents.contains("[ActionReplay_Enabled]"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mixed_gecko_and_action_replay_file_is_discovered_and_staged() {
    let root = temp("both-ok");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, VALID_BOTH_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);

    let discovery =
        discover_local_dolphin_cheat_file(&file, &identity).expect("discovery succeeds");
    assert_eq!(discovery.kind, LocalDolphinCodeKind::Both);
    assert_eq!(discovery.gecko_codes.len(), 1);
    assert_eq!(discovery.action_replay_codes.len(), 1);

    let destination = empty_destination(&identity.path);
    let staging_root = root.join("staging");
    let staged = stage_local_dolphin_codes(&staging_root, &destination, &discovery).unwrap();
    assert!(staged.contents.contains("[Gecko]"));
    assert!(staged.contents.contains("[ActionReplay]"));
    assert!(staged.contents.contains("Infinite Health"));
    assert!(staged.contents.contains("Infinite Ammo"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn apply_and_undo_round_trip_restores_prior_state() {
    let root = temp("undo");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);
    let game_settings_dir = identity.path.parent().unwrap();
    std::fs::create_dir_all(game_settings_dir).unwrap();

    let prior_contents = "[Core]\nCPUThread = True\n";
    std::fs::write(&identity.path, prior_contents).unwrap();

    let discovery = discover_local_dolphin_cheat_file(&file, &identity).unwrap();
    let loaded = load_dolphin_ini(&identity.path).unwrap();
    let destination = LoadedDolphinDestination {
        path: identity.path.clone(),
        existed: true,
        digest: Some(loaded.digest.clone()),
        document: loaded.document,
    };
    let staging_root = root.join("staging");
    let staged = stage_local_dolphin_codes(&staging_root, &destination, &discovery).unwrap();

    // Apply: overwrite the real destination with the staged content.
    std::fs::write(&identity.path, &staged.contents).unwrap();
    let after_apply = std::fs::read_to_string(&identity.path).unwrap();
    assert!(after_apply.contains("[Gecko]"));
    assert!(after_apply.contains("CPUThread = True"));

    // Undo: restore the prior content, simulating shared_transaction rollback.
    std::fs::write(&identity.path, prior_contents).unwrap();
    let after_undo = std::fs::read_to_string(&identity.path).unwrap();
    assert_eq!(after_undo, prior_contents);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reapplying_the_exact_same_file_is_reported_already_installed() {
    let root = temp("idempotent");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);
    std::fs::create_dir_all(identity.path.parent().unwrap()).unwrap();

    let discovery = discover_local_dolphin_cheat_file(&file, &identity).unwrap();
    let destination = empty_destination(&identity.path);
    assert_eq!(
        check_local_dolphin_install_state(&destination, &discovery),
        LocalDolphinInstallState::New
    );
    let staging_root = root.join("staging");
    let staged = stage_local_dolphin_codes(&staging_root, &destination, &discovery).unwrap();
    std::fs::write(&identity.path, &staged.contents).unwrap();

    let rediscovered = discover_local_dolphin_cheat_file(&file, &identity).unwrap();
    let reloaded = load_dolphin_ini(&identity.path).unwrap();
    let reloaded_destination = LoadedDolphinDestination {
        path: identity.path.clone(),
        existed: true,
        digest: Some(reloaded.digest),
        document: reloaded.document,
    };
    assert_eq!(
        check_local_dolphin_install_state(&reloaded_destination, &rediscovered),
        LocalDolphinInstallState::AlreadyInstalled
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn existing_unrelated_codes_are_preserved_across_a_merge() {
    let root = temp("preserve");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);
    std::fs::create_dir_all(identity.path.parent().unwrap()).unwrap();

    let existing = "[Gecko]\n$Unrelated Existing Code\nAAAAAAAA BBBBBBBB\n\n[Gecko_Enabled]\n$Unrelated Existing Code\n";
    std::fs::write(&identity.path, existing).unwrap();
    let loaded = load_dolphin_ini(&identity.path).unwrap();
    let destination = LoadedDolphinDestination {
        path: identity.path.clone(),
        existed: true,
        digest: Some(loaded.digest),
        document: loaded.document,
    };

    let discovery = discover_local_dolphin_cheat_file(&file, &identity).unwrap();
    let staging_root = root.join("staging");
    let staged = stage_local_dolphin_codes(&staging_root, &destination, &discovery).unwrap();

    assert!(staged.contents.contains("Unrelated Existing Code"));
    assert!(staged.contents.contains("Infinite Health"));

    let merged = parse_dolphin_ini(&staged.contents);
    assert!(
        merged
            .gecko_enabled_names
            .iter()
            .any(|name| name == "Unrelated Existing Code")
    );
    assert!(
        merged
            .gecko_enabled_names
            .iter()
            .any(|name| name == "Infinite Health")
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn wrong_game_id_in_filename_is_blocked() {
    let root = temp("wrong-id");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("GALE99.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);

    let error = discover_local_dolphin_cheat_file(&file, &identity).unwrap_err();
    assert!(matches!(
        error,
        LocalDolphinFileError::IdentityConflict { .. }
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn wrong_revision_in_filename_is_blocked_even_with_matching_game_id() {
    let root = temp("wrong-rev");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("GALE01r2.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", Some(1));

    let error = discover_local_dolphin_cheat_file(&file, &identity).unwrap_err();
    assert!(matches!(
        error,
        LocalDolphinFileError::IdentityConflict { .. }
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_arbitrary_filename_with_no_encoded_identity_is_accepted() {
    let root = temp("arbitrary-name");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my favorite cheats.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);

    let discovery =
        discover_local_dolphin_cheat_file(&file, &identity).expect("discovery succeeds");
    assert_eq!(discovery.kind, LocalDolphinCodeKind::Gecko);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn malformed_gecko_code_rejects_the_whole_file() {
    let root = temp("malformed-gecko");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, "[Gecko]\n$Broken Code\nnot a valid line\n").unwrap();
    let identity = candidate(&root, "GALE01", None);

    let error = discover_local_dolphin_cheat_file(&file, &identity).unwrap_err();
    assert!(matches!(error, LocalDolphinFileError::Malformed { .. }));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn malformed_action_replay_code_rejects_the_whole_file() {
    let root = temp("malformed-ar");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, "[ActionReplay]\n$Broken Code\nZZZZ\n").unwrap();
    let identity = candidate(&root, "GALE01", None);

    let error = discover_local_dolphin_cheat_file(&file, &identity).unwrap_err();
    assert!(matches!(error, LocalDolphinFileError::Malformed { .. }));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn empty_code_section_with_no_codes_is_rejected() {
    let root = temp("no-codes");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, "[Core]\nCPUThread = True\n").unwrap();
    let identity = candidate(&root, "GALE01", None);

    let error = discover_local_dolphin_cheat_file(&file, &identity).unwrap_err();
    assert!(matches!(error, LocalDolphinFileError::NoCodesFound { .. }));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn unsupported_extension_is_rejected() {
    let root = temp("ext");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("cheats.txt");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);

    let error = discover_local_dolphin_cheat_file(&file, &identity).unwrap_err();
    assert!(matches!(
        error,
        LocalDolphinFileError::UnsupportedExtension { .. }
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn symlinked_source_file_is_rejected() {
    let root = temp("symlink");
    std::fs::create_dir_all(&root).unwrap();
    let real = root.join("real.ini");
    std::fs::write(&real, VALID_GECKO_INI).unwrap();
    let link = root.join("my_cheats.ini");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let identity = candidate(&root, "GALE01", None);
        let error = discover_local_dolphin_cheat_file(&link, &identity).unwrap_err();
        assert!(matches!(error, LocalDolphinFileError::IsSymlink { .. }));
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn directory_source_is_rejected() {
    let root = temp("dir");
    std::fs::create_dir_all(&root).unwrap();
    let identity = candidate(&root, "GALE01", None);
    let error = discover_local_dolphin_cheat_file(&root, &identity).unwrap_err();
    assert!(matches!(error, LocalDolphinFileError::IsDirectory { .. }));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn oversized_source_is_rejected() {
    let root = temp("oversized");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    let mut contents = String::from("[Gecko]\n");
    while (contents.len() as u64) <= MAX_LOCAL_DOLPHIN_INI_BYTES {
        contents.push_str("$Padding\n01234567 01234567\n");
    }
    std::fs::write(&file, &contents).unwrap();
    let identity = candidate(&root, "GALE01", None);

    let error = discover_local_dolphin_cheat_file(&file, &identity).unwrap_err();
    assert!(matches!(error, LocalDolphinFileError::TooLarge { .. }));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn discovery_never_mutates_the_selected_file() {
    let root = temp("no-mutate");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let before = std::fs::read(&file).unwrap();
    let identity = candidate(&root, "GALE01", None);

    let _ = discover_local_dolphin_cheat_file(&file, &identity);

    let after = std::fs::read(&file).unwrap();
    assert_eq!(before, after);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn no_mutation_happens_before_staging_is_explicitly_requested() {
    let root = temp("no-premature-write");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("my_cheats.ini");
    std::fs::write(&file, VALID_GECKO_INI).unwrap();
    let identity = candidate(&root, "GALE01", None);

    let _ = discover_local_dolphin_cheat_file(&file, &identity).unwrap();

    // Discovery alone must never create the destination file or its
    // parent directory.
    assert!(!identity.path.exists());
    assert!(!identity.path.parent().unwrap().exists());

    let _ = std::fs::remove_dir_all(root);
}
