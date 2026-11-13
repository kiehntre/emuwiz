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
fn only_the_authorised_bytes_are_booted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let cmd = command(dir.path(), "g.st", &[3u8; 2048]);
    let good: String = Sha256::digest([3u8; 2048])
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert!(prepare_session_expecting(&cmd, &root, Some(&good)).is_ok());
    // The file is swapped after it was checked: refused, and no session left.
    std::fs::write(&cmd.selection.content_path, [9u8; 2048]).unwrap();
    let before = std::fs::read_dir(&root).unwrap().count();
    let refused = prepare_session_expecting(&cmd, &root, Some(&good)).unwrap_err();
    assert!(
        refused.contains("changed after it was checked"),
        "{refused}"
    );
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), before);
}

#[test]
fn a_failed_session_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let mut cmd = command(dir.path(), "g.st", &[1u8; 1024]);
    cmd.selection.machine_model = HatariMachineModel::Unknown;
    assert!(prepare_session(&cmd, &root).is_err());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn a_running_session_is_never_pruned_and_names_keep_their_extension() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let cmd = command(dir.path(), "g.st", &[1u8; 1024]);
    let live = prepare_session(&cmd, &root).unwrap();
    record_session_pid(&live, std::process::id()).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    prune_stale_sessions(&root, Duration::from_millis(1));
    assert!(live.root.exists(), "this process is alive");

    let latin1 = Path::new(std::ffi::OsStr::from_bytes(b"/g/Spiel\xe4.msa"));
    assert!(copy_file_name(latin1).ends_with(".msa"));
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

#[test]
fn saves_persist_per_game_and_the_previous_state_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let saves = dir.path().join("saves");
    let cmd = command(dir.path(), "Game.st", &[5u8; 2048]);
    let options = SessionOptions {
        saves_root: Some(&saves),
        ..SessionOptions::default()
    };
    let first = prepare_session_with(&cmd, &dir.path().join("s"), &options).unwrap();
    let working = first.disk_copy.clone();
    assert!(working.starts_with(first.save_dir.as_ref().unwrap()));
    let args: Vec<String> = first
        .arguments
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let protect = args.iter().position(|a| a == "--protect-floppy").unwrap();
    assert_eq!(args[protect + 1], "off", "the game may write its saves");
    assert!(
        !args
            .iter()
            .any(|a| a == "--configfile" || a == "--harddrive")
    );

    // The game saves: the working disk changes; the original never does.
    std::fs::set_permissions(&working, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&working, [6u8; 2048]).unwrap();
    release_game_lock(first.save_dir.as_ref().unwrap()); // the first game exited
    let second = prepare_session_with(&cmd, &dir.path().join("s"), &options).unwrap();
    assert_eq!(second.disk_copy, working);
    assert_eq!(
        std::fs::read(&working).unwrap(),
        vec![6u8; 2048],
        "save kept"
    );
    let previous = working.with_file_name(format!(
        "{}.previous",
        working.file_name().unwrap().to_string_lossy()
    ));
    assert_eq!(std::fs::read(previous).unwrap(), vec![6u8; 2048]);
    assert_eq!(
        std::fs::read(&cmd.selection.content_path).unwrap(),
        vec![5u8; 2048]
    );

    // While that game runs, a second launch of it is refused.
    record_session_pid(&second, std::process::id()).unwrap();
    let busy = prepare_session_with(&cmd, &dir.path().join("s"), &options).unwrap_err();
    assert!(busy.contains("already running"), "{busy}");
}

#[test]
fn a_saved_disk_that_is_not_a_regular_file_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let saves = dir.path().join("saves");
    let cmd = command(dir.path(), "Game.st", &[5u8; 2048]);
    let options = SessionOptions {
        saves_root: Some(&saves),
        ..SessionOptions::default()
    };
    let first = prepare_session_with(&cmd, &dir.path().join("s"), &options).unwrap();
    std::fs::remove_file(&first.disk_copy).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", &first.disk_copy).unwrap();
    assert!(prepare_session_with(&cmd, &dir.path().join("s"), &options).is_err());
    assert!(
        std::fs::symlink_metadata(&first.disk_copy)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn only_validated_hardware_preferences_are_carried_over() {
    let owner = "[Screen]\nnMonitorType = 2\n[Memory]\nnTTRamSize = 4\nSTRamSize_KB = 4096\n\
                 [HardDisk]\nbUseHardDiskDirectory = TRUE\nszHardDiskDirectory = /home\n\
                 [Floppy]\nszDiskBFileName = /x.st\n[System]\nnCpuLevel = 3\nnCpuFreq = 16\n\
                 bCompatibleCpu = TRUE\nbBlitter = FALSE\nnDSPType = 9\nnFPUType = 68882\n";
    let args: Vec<String> = hardware_arguments(owner)
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .map(|i| args[i + 1].clone())
    };
    assert_eq!(value_of("--cpulevel").as_deref(), Some("3"));
    assert_eq!(value_of("--cpuclock").as_deref(), Some("16"));
    assert_eq!(value_of("--compatible").as_deref(), Some("true"));
    assert_eq!(value_of("--blitter").as_deref(), Some("false"));
    assert_eq!(value_of("--fpu").as_deref(), Some("68882"));
    assert_eq!(value_of("--memsize").as_deref(), Some("4096"));
    assert_eq!(value_of("--ttram").as_deref(), Some("4"));
    assert_eq!(value_of("--monitor").as_deref(), Some("vga"));
    assert_eq!(value_of("--dsp"), None, "out-of-range value skipped");
    for line in &args {
        assert!(!line.contains("/home") && !line.contains("/x.st"), "{line}");
    }
    assert!(hardware_arguments("[System]\nnCpuFreq = 7\nnCpuLevel = 5\n").is_empty());

    // Hardware lands after --machine and before the disk.
    let dir = tempfile::tempdir().unwrap();
    let cmd = command(dir.path(), "g.st", &[1u8; 1024]);
    let session = prepare_session_with(
        &cmd,
        &dir.path().join("s"),
        &SessionOptions {
            hardware: hardware_arguments(owner),
            ..SessionOptions::default()
        },
    )
    .unwrap();
    let all: Vec<String> = session
        .arguments
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let at = |flag: &str| all.iter().position(|a| a == flag).unwrap();
    assert!(at("--machine") < at("--monitor") && at("--monitor") < at("--disk-a"));
}

#[test]
fn saves_follow_the_game_content_not_its_file_name() {
    let dir = tempfile::tempdir().unwrap();
    let saves = dir.path().join("saves");
    let options = SessionOptions {
        saves_root: Some(&saves),
        ..SessionOptions::default()
    };
    let first = command(dir.path(), "Old Name.st", &[5u8; 2048]);
    let session = prepare_session_with(&first, &dir.path().join("s"), &options).unwrap();
    release_game_lock(session.save_dir.as_ref().unwrap());
    std::fs::write(&session.disk_copy, [8u8; 2048]).unwrap();
    // Same bytes under a new name (a rename or duplicate): same saved disk.
    let renamed = command(dir.path(), "New Name.st", &[5u8; 2048]);
    let again = prepare_session_with(&renamed, &dir.path().join("s"), &options).unwrap();
    assert_eq!(again.disk_copy, session.disk_copy);
    assert_eq!(std::fs::read(&again.disk_copy).unwrap(), vec![8u8; 2048]);
}

#[test]
fn a_damaged_saved_disk_is_refused_and_the_good_backup_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let saves = dir.path().join("saves");
    let options = SessionOptions {
        saves_root: Some(&saves),
        ..SessionOptions::default()
    };
    let cmd = command(dir.path(), "g.st", &[5u8; 2048]);
    let first = prepare_session_with(&cmd, &dir.path().join("s"), &options).unwrap();
    let save_dir = first.save_dir.clone().unwrap();
    release_game_lock(&save_dir);
    let second = prepare_session_with(&cmd, &dir.path().join("s"), &options).unwrap();
    release_game_lock(&save_dir);
    let previous = save_dir.join("disk.st.previous");
    assert_eq!(std::fs::read(&previous).unwrap(), vec![5u8; 2048]);
    // A crashed session truncated the working disk.
    std::fs::write(&second.disk_copy, [1u8; 100]).unwrap();
    let refused = prepare_session_with(&cmd, &dir.path().join("s"), &options).unwrap_err();
    assert!(refused.contains("looks damaged"), "{refused}");
    assert_eq!(std::fs::read(&previous).unwrap(), vec![5u8; 2048]);
    // ...and the refusal released the lock.
    assert!(!session_is_live(&save_dir));
}

#[test]
fn a_stale_or_reused_pid_never_blocks_the_game() {
    let dir = tempfile::tempdir().unwrap();
    let saves = dir.path().join("saves");
    let options = SessionOptions {
        saves_root: Some(&saves),
        ..SessionOptions::default()
    };
    let cmd = command(dir.path(), "g.st", &[5u8; 2048]);
    let first = prepare_session_with(&cmd, &dir.path().join("s"), &options).unwrap();
    let save_dir = first.save_dir.clone().unwrap();
    // A live pid (this process) but the wrong start time: a reused pid.
    std::fs::write(
        save_dir.join(SESSION_PID_FILE),
        format!("{} 1", std::process::id()),
    )
    .unwrap();
    assert!(prepare_session_with(&cmd, &dir.path().join("s"), &options).is_ok());
    // While this process holds it for real, a second launch is refused.
    let busy = prepare_session_with(&cmd, &dir.path().join("s"), &options).unwrap_err();
    assert!(busy.contains("already running"), "{busy}");
}

#[test]
fn hardware_preferences_apply_only_to_the_machine_they_were_saved_for() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("hatari.cfg");
    std::fs::write(&cfg, "[System]\nnModelType = 0\nnCpuFreq = 16\n").unwrap();
    assert!(!read_hardware_arguments(&cfg, HatariMachineModel::St).is_empty());
    assert!(read_hardware_arguments(&cfg, HatariMachineModel::Falcon).is_empty());
}
