use super::*;
use crate::launch::hatari_command::{HatariCommandSelection, HatariMediaFormat};
use std::os::unix::fs::PermissionsExt;

fn command(dir: &Path, name: &str, bytes: &[u8]) -> HatariCommand {
    let content = dir.join(name);
    std::fs::write(&content, bytes).unwrap();
    HatariCommand {
        executable: PathBuf::from("/usr/bin/hatari"),
        arguments: vec![],
        working_directory: None,
        selection: HatariCommandSelection {
            profile_id: "hatari:test".into(),
            platform_id: "AtariST".into(),
            machine_model: HatariMachineModel::Ste,
            content_path: content,
            media_format: HatariMediaFormat::St,
            tos_path: PathBuf::from("/bios/tos206.img"),
        },
    }
}

#[test]
fn the_session_runs_on_a_read_only_copy_and_never_touches_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = command(dir.path(), "Game (1990).st", &[7u8; 4096]);
    let before = std::fs::metadata(&cmd.selection.content_path).unwrap();
    let root = dir.path().join("sessions");
    let session = prepare_session(&cmd, &root).unwrap();

    // Byte-identical, read-only copy inside the private session directory.
    assert!(session.disk_copy.starts_with(&session.root));
    assert_ne!(session.disk_copy, cmd.selection.content_path);
    assert_eq!(std::fs::read(&session.disk_copy).unwrap(), vec![7u8; 4096]);
    let mode = std::fs::metadata(&session.disk_copy)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o400);
    let root_mode = std::fs::metadata(&session.root)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(root_mode, 0o700);

    // The original is byte-for-byte and metadata-wise untouched.
    let after = std::fs::metadata(&cmd.selection.content_path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert_eq!(
        std::fs::read(&cmd.selection.content_path).unwrap(),
        vec![7u8; 4096]
    );
}

#[test]
fn arguments_carry_no_inherited_config_storage_or_drive_b() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = command(dir.path(), "g.st", &[1u8; 1024]);
    let session = prepare_session(&cmd, &dir.path().join("s")).unwrap();
    let args: Vec<String> = session
        .arguments
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    assert!(!has("--configfile"), "no config is merged in");
    for storage in [
        "--harddrive",
        "--acsi",
        "--scsi",
        "--ide-master",
        "--ide-slave",
        "--disk-b",
        "--memstate",
    ] {
        assert!(!has(storage), "{storage}");
    }
    assert_eq!(value_of("--drive-b").as_deref(), Some("off"));
    assert_eq!(value_of("--protect-floppy").as_deref(), Some("on"));
    assert_eq!(value_of("--machine").as_deref(), Some("ste"));
    assert_eq!(value_of("--tos").as_deref(), Some("/bios/tos206.img"));
    assert_eq!(
        value_of("--disk-a").as_deref(),
        Some(session.disk_copy.to_str().unwrap())
    );
    // The original path appears nowhere in what Hatari is given.
    assert!(
        !args
            .iter()
            .any(|a| a.contains(cmd.selection.content_path.to_str().unwrap()))
    );
}

#[test]
fn the_environment_redirects_home_and_xdg_into_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = command(dir.path(), "g.st", &[1u8; 1024]);
    let session = prepare_session(&cmd, &dir.path().join("s")).unwrap();
    let env = |key: &str| {
        session
            .environment
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| PathBuf::from(v))
    };
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        let value = env(key).unwrap_or_else(|| panic!("{key} missing"));
        assert!(value.starts_with(&session.root), "{key}");
        assert!(value.is_dir(), "{key}");
    }
    assert_eq!(session.working_directory, env("HOME").unwrap());
}

#[test]
fn symlinks_oversize_empty_and_unknown_machine_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let cmd = command(dir.path(), "g.st", &[1u8; 1024]);
    // Symlinked media.
    let link = dir.path().join("link.st");
    std::os::unix::fs::symlink(&cmd.selection.content_path, &link).unwrap();
    let mut linked = cmd.clone();
    linked.selection.content_path = link;
    assert!(prepare_session(&linked, &root).is_err());
    // Empty media.
    let empty = command(dir.path(), "empty.st", &[]);
    assert!(prepare_session(&empty, &root).is_err());
    // Oversized media (sparse, so the test stays cheap).
    let big = dir.path().join("big.st");
    let file = std::fs::File::create(&big).unwrap();
    file.set_len(MAX_DISK_BYTES + 1).unwrap();
    let mut oversized = cmd.clone();
    oversized.selection.content_path = big;
    assert!(prepare_session(&oversized, &root).is_err());
    // Unknown machine never guesses.
    let mut unknown = cmd;
    unknown.selection.machine_model = HatariMachineModel::Unknown;
    assert!(prepare_session(&unknown, &root).is_err());
}

#[test]
fn stale_sessions_are_pruned_but_other_directories_and_fresh_sessions_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let cmd = command(dir.path(), "g.st", &[1u8; 1024]);
    let old = prepare_session(&cmd, &root).unwrap();
    let keep_other = root.join("not-a-session");
    std::fs::create_dir_all(&keep_other).unwrap();
    // Nothing is stale yet.
    prune_stale_sessions(&root, Duration::from_secs(3600));
    assert!(old.root.exists());
    // A zero age makes every session stale; the unrelated directory stays.
    std::thread::sleep(Duration::from_millis(20));
    prune_stale_sessions(&root, Duration::from_millis(1));
    assert!(!old.root.exists(), "read-only copy must still be removable");
    assert!(keep_other.exists());
}

#[test]
fn a_real_spawn_sees_the_isolated_environment_and_leaves_the_original_alone() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = command(dir.path(), "g.st", &[9u8; 2048]);
    let session = prepare_session(&cmd, &dir.path().join("s")).unwrap();
    let report = dir.path().join("report.txt");
    let fake = dir.path().join("fake-hatari");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\n{{ echo HOME=$HOME; echo CFG=$XDG_CONFIG_HOME; for a in \"$@\"; do echo ARG=$a; done; }} > '{}'\n",
            report.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = std::process::Command::new(&fake);
    child
        .args(&session.arguments)
        .current_dir(&session.working_directory);
    for (key, value) in &session.environment {
        child.env(key, value);
    }
    // A freshly written script can briefly report "text file busy" while another
    // test thread forks; retry that one transient error.
    let status = loop {
        match child.status() {
            Err(error) if error.raw_os_error() == Some(26) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => break other.unwrap(),
        }
    };
    assert!(status.success());
    let text = std::fs::read_to_string(&report).unwrap();
    assert!(text.contains(&format!("HOME={}", session.working_directory.display())));
    assert!(text.contains(&format!("ARG={}", session.disk_copy.display())));
    assert!(!text.contains("g.st\n") || text.contains("disk-g.st"));
    assert_eq!(
        std::fs::read(&cmd.selection.content_path).unwrap(),
        vec![9u8; 2048]
    );
}
