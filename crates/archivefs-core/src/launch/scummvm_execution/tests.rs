use super::*;

/// Writes a tiny shell script that prints `output`, and does not return until
/// that script is genuinely spawnable.
///
/// Unlike the other launch fixtures, ScummVM's preflight actually `execve`s
/// this file (through the detector). In a multithreaded test binary a
/// concurrent `fork`+`exec` in another test can briefly leave a writable
/// descriptor to a just-written file alive, so the first `execve` here can
/// transiently fail with `ETXTBSY` and surface as a spurious
/// `ScummVmGameIdUnavailable`. The write is made fully durable and its handle
/// dropped before `chmod`, then spawnability is confirmed with a bounded,
/// yield-only retry (no timed sleep) so every caller sees a ready executable.
#[cfg(unix)]
fn executable_fixture(root: &std::path::Path, output: &str) -> std::path::PathBuf {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    let path = root.join("scummvm-fixture");
    {
        let mut file = std::fs::File::create(&path).unwrap();
        write!(file, "#!/bin/sh\nprintf '%s\\n' '{output}'\n").unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();
    } // handle closed here, before the mode change
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

    const ETXTBSY: i32 = 26;
    for attempt in 0.. {
        match Command::new(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let _ = child.wait();
                break;
            }
            Err(error) if error.raw_os_error() == Some(ETXTBSY) && attempt < 10_000 => {
                std::thread::yield_now();
            }
            Err(error) => panic!("scummvm fixture never became spawnable: {error}"),
        }
    }
    path
}

#[test]
fn malformed_request_is_rejected_before_any_spawn() {
    let result = preflight_scummvm_launch(&ScummVmLaunchRequest {
        selected_game_folder: "relative/game".into(),
        expected_game_key: "scumm:game".into(),
        expected_executable: "/does/not/exist".into(),
        trainer: None,
    });
    assert_eq!(
        result.unwrap_err().kind,
        ScummVmLaunchPreflightErrorKind::ContentPathNotAbsolute
    );
}

#[cfg(unix)]
#[test]
fn fresh_detector_evidence_builds_a_command_for_a_renamed_folder() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder-name-is-not-used");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("resource.dat"), b"content").unwrap();
    let executable = executable_fixture(root.path(), "Game ID: sci:monkey");
    let command = preflight_scummvm_launch(&ScummVmLaunchRequest {
        selected_game_folder: folder.clone(),
        expected_game_key: "sci:monkey".into(),
        expected_executable: executable,
        trainer: None,
    })
    .unwrap();
    assert_eq!(command.arguments[0], "-p");
    assert_eq!(command.arguments[1], folder.as_os_str());
    assert_eq!(command.arguments[2], "sci:monkey");
}

#[cfg(unix)]
#[test]
fn fresh_detector_disagreement_refuses_before_command_creation() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let executable = executable_fixture(root.path(), "Game ID: sci:other");
    let error = preflight_scummvm_launch(&ScummVmLaunchRequest {
        selected_game_folder: folder,
        expected_game_key: "sci:monkey".into(),
        expected_executable: executable,
        trainer: None,
    })
    .unwrap_err();
    assert_eq!(
        error.kind,
        ScummVmLaunchPreflightErrorKind::IdentityMismatch
    );
}

#[cfg(target_os = "linux")]
fn trainer_request(root: &std::path::Path) -> ScummVmLaunchRequest {
    let folder = root.join("game with spaces");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("resource.dat"), b"synthetic game").unwrap();
    let config = root.join("owned.ini");
    std::fs::write(&config, format!("[scummvm]\nsavepath={}\n[emuwiz-game]\nengineid=hypno\ngameid=demo\npath={}\ninfiniteHealth=true\n", root.join("saves").display(), folder.display())).unwrap();
    ScummVmLaunchRequest {
        selected_game_folder: folder,
        expected_game_key: "hypno:demo".into(),
        expected_executable: executable_fixture(root, "Game ID: hypno:demo"),
        trainer: Some(ScummVmTrainerLaunchBinding {
            configuration: config,
            target_name: "emuwiz-game".into(),
        }),
    }
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_preflight_is_repeatable_and_preserves_config_media_and_saves() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let before = std::fs::read(config).unwrap();
    std::fs::create_dir(root.path().join("saves")).unwrap();
    let save = root.path().join("saves/existing.sav");
    std::fs::write(&save, b"valuable save fixture").unwrap();
    let command = preflight_scummvm_launch(&request).unwrap();
    assert_eq!(command, preflight_scummvm_launch(&request).unwrap());
    assert_eq!(
        command.arguments,
        vec![
            std::ffi::OsString::from("-c"),
            config.as_os_str().into(),
            "-p".into(),
            request.selected_game_folder.as_os_str().into(),
            "emuwiz-game".into()
        ]
    );
    assert_eq!(std::fs::read(config).unwrap(), before);
    assert_eq!(
        std::fs::read(request.selected_game_folder.join("resource.dat")).unwrap(),
        b"synthetic game"
    );
    assert_eq!(std::fs::read(save).unwrap(), b"valuable save fixture");
    assert!(
        !command
            .arguments
            .iter()
            .any(|a| a.to_string_lossy().contains("savepath"))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_target_mismatch_is_blocked_before_detector_runs() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let original = std::fs::read_to_string(config).unwrap();
    // Removing the executable proves the config rejection happens first.
    std::fs::remove_file(&request.expected_executable).unwrap();
    for changed in [
        original.replace("engineid=hypno", "engineid=scumm"),
        original.replace("gameid=demo", "gameid=other"),
        original.replace("gameid=demo", "gameid=hypno:demo"),
        original.replace(
            &request.selected_game_folder.display().to_string(),
            "/other/game",
        ),
        original.replace("emuwiz-game", "another-target"),
    ] {
        std::fs::write(config, &changed).unwrap();
        assert_eq!(
            preflight_scummvm_launch(&request).unwrap_err().kind,
            ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
        );
        assert_eq!(std::fs::read_to_string(config).unwrap(), changed);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_malformed_duplicate_and_missing_bindings_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let original = std::fs::read_to_string(config).unwrap();
    for changed in [
        original.replace("engineid=hypno\n", ""),
        original.replace("gameid=demo\n", ""),
        original.replace("gameid=demo", "gameid=demo\ngameid=other"),
        original.replace("gameid=demo", "gameid=demo\nGAMEID=demo"),
        format!("{original}[emuwiz-game]\ngameid=demo\n"),
        original.replace("[emuwiz-game]", "[emuwiz-game"),
        format!("{original}junk\n"),
        format!("{original}\0\n"),
        String::new(),
    ] {
        std::fs::write(config, changed).unwrap();
        assert_eq!(
            preflight_scummvm_launch(&request).unwrap_err().kind,
            ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_missing_oversized_long_line_and_invalid_utf8_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    std::fs::remove_file(config).unwrap();
    assert_eq!(
        preflight_scummvm_launch(&request).unwrap_err().kind,
        ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
    );
    for bytes in [
        vec![b'x'; trainer_config::MAX_CONFIG_BYTES + 1],
        vec![b'x'; 8193],
        vec![0xff],
    ] {
        std::fs::write(config, bytes).unwrap();
        assert_eq!(
            preflight_scummvm_launch(&request).unwrap_err().kind,
            ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_symlinks_and_special_files_are_refused_without_blocking() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let mut request = trainer_request(root.path());
    let config = request.trainer.as_ref().unwrap().configuration.clone();
    let link = root.path().join("linked.ini");
    symlink(&config, &link).unwrap();
    request.trainer.as_mut().unwrap().configuration = link;
    assert_eq!(
        preflight_scummvm_launch(&request).unwrap_err().kind,
        ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
    );
    let directory = root.path().join("linked-parent");
    symlink(root.path(), &directory).unwrap();
    request.trainer.as_mut().unwrap().configuration = directory.join("owned.ini");
    assert_eq!(
        preflight_scummvm_launch(&request).unwrap_err().kind,
        ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
    );
    let fifo = root.path().join("fifo.ini");
    let fifo_c =
        std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(fifo.as_os_str())).unwrap();
    // SAFETY: fifo_c is a live NUL-terminated fixture path; no pointer escapes.
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    request.trainer.as_mut().unwrap().configuration = fifo;
    assert_eq!(
        preflight_scummvm_launch(&request).unwrap_err().kind,
        ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
    );
    assert!(config.is_file());
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_validation_accepts_native_bom_crlf_comments_and_literal_paths() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let original = std::fs::read_to_string(config).unwrap();
    std::fs::write(
        config,
        format!(
            "\u{feff}# native comment\r\n{}",
            original.replace('\n', "\r\n")
        ),
    )
    .unwrap();
    assert!(preflight_scummvm_launch(&request).is_ok());
    std::fs::write(
        config,
        original.replace(
            &request.selected_game_folder.display().to_string(),
            &format!("\"{}\"", request.selected_game_folder.display()),
        ),
    )
    .unwrap();
    assert_eq!(
        preflight_scummvm_launch(&request).unwrap_err().kind,
        ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
    );
}

#[cfg(target_os = "linux")]
#[test]
fn changed_trainer_binding_is_rejected_again_at_spawn() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let command = preflight_scummvm_launch(&request).unwrap();
    let config = &request.trainer.as_ref().unwrap().configuration;
    let changed = std::fs::read_to_string(config)
        .unwrap()
        .replace("gameid=demo", "gameid=other");
    std::fs::write(config, changed).unwrap();
    assert!(
        matches!(spawn_scummvm(command), Err(ScummVmLaunchSpawnError::Spawn(e)) if e.kind() == std::io::ErrorKind::InvalidInput)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_requires_explicit_native_savepath_with_target_precedence() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let original = std::fs::read_to_string(config).unwrap();
    let without = original
        .lines()
        .filter(|line| !line.starts_with("savepath="))
        .collect::<Vec<_>>()
        .join("\n");
    for text in [
        without,
        format!("{original}\nsavepath=relative\n"),
        format!("{original}\nsavepath=\n"),
        format!("{original}\nsavepath=/saves/../other\n"),
    ] {
        std::fs::write(config, text).unwrap();
        assert_eq!(
            preflight_scummvm_launch(&request).unwrap_err().kind,
            ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
        );
    }
    // Native target savepath takes precedence over the global setting.
    let override_text = format!(
        "{}\nsavepath={}\n",
        original.replace(
            &format!("savepath={}", root.path().join("saves").display()),
            "savepath=relative"
        ),
        root.path().join("target-saves").display()
    );
    std::fs::write(config, &override_text).unwrap();
    let command = preflight_scummvm_launch(&request).unwrap();
    assert_eq!(std::fs::read_to_string(config).unwrap(), override_text);
    assert!(
        !command
            .arguments
            .iter()
            .any(|a| a.to_string_lossy().contains("savepath"))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_global_savepath_requires_exact_native_application_section() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let original = std::fs::read_to_string(config).unwrap();
    assert!(preflight_scummvm_launch(&request).is_ok());
    for section in ["SCUMMVM", "ScummVM", " scummvm "] {
        let text = original.replace("[scummvm]", &format!("[{section}]"));
        std::fs::write(config, &text).unwrap();
        assert_eq!(
            preflight_scummvm_launch(&request).unwrap_err().kind,
            ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
        );
        assert_eq!(std::fs::read_to_string(config).unwrap(), text);
    }
    // The noncanonical misc domain does not replace the canonical application
    // domain, and its savepath must not contribute to the selected game.
    std::fs::write(config, format!("[SCUMMVM]\nsavepath=relative\n{original}")).unwrap();
    assert!(preflight_scummvm_launch(&request).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_game_target_lookup_uses_native_case_insensitive_domain_map() {
    let root = tempfile::tempdir().unwrap();
    let mut request = trainer_request(root.path());
    let config = request.trainer.as_ref().unwrap().configuration.clone();
    let original = std::fs::read_to_string(&config).unwrap();
    let mixed = original.replace("[emuwiz-game]", "[EmuWiz-Game]");
    std::fs::write(&config, &mixed).unwrap();
    assert!(preflight_scummvm_launch(&request).is_ok());
    request.trainer.as_mut().unwrap().target_name = "EMUWIZ-GAME".into();
    assert!(preflight_scummvm_launch(&request).is_ok());
    for text in [
        mixed.replace("[EmuWiz-Game]", "[another-game]"),
        mixed.replace("[EmuWiz-Game]", "[ EmuWiz-Game ]"),
        format!("{mixed}\n[emuwiz-game]\ngameid=demo\n"),
    ] {
        std::fs::write(&config, &text).unwrap();
        assert_eq!(
            preflight_scummvm_launch(&request).unwrap_err().kind,
            ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
        );
        assert_eq!(std::fs::read_to_string(&config).unwrap(), text);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_ascii_savepath_whitespace_matches_native_trim_without_rewriting() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let original = std::fs::read_to_string(config).unwrap();
    let absolute = root.path().join("saves").display().to_string();
    for value in [
        absolute.clone(),
        format!(" {absolute}"),
        format!("{absolute} "),
        format!(" \t{absolute}\t "),
    ] {
        let text = original.replace(
            &format!("savepath={absolute}"),
            &format!(" \tSAVEPATH \t= {value}"),
        );
        std::fs::write(config, &text).unwrap();
        assert!(preflight_scummvm_launch(&request).is_ok());
        assert_eq!(std::fs::read_to_string(config).unwrap(), text);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_unicode_whitespace_cannot_fake_an_absolute_savepath_or_binding() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let original = std::fs::read_to_string(config).unwrap();
    let absolute = root.path().join("saves").display().to_string();
    // These malformed inputs must be refused before invoking any executable.
    std::fs::remove_file(&request.expected_executable).unwrap();
    for space in ['\u{a0}', '\u{2003}', '\u{202f}', '\u{3000}'] {
        for text in [
            original.replace(
                &format!("savepath={absolute}"),
                &format!("savepath={space}{absolute}"),
            ),
            format!("{original}\nsavepath={space}{absolute}\n"),
            original.replace("gameid=demo", &format!("gameid={space}demo")),
            original.replace("engineid=hypno", &format!("engineid=hypno{space}")),
            original.replace("savepath=", &format!("{space}savepath=")),
            format!("{space}\n{original}"),
        ] {
            std::fs::write(config, &text).unwrap();
            assert_eq!(
                preflight_scummvm_launch(&request).unwrap_err().kind,
                ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
            );
            assert_eq!(std::fs::read_to_string(config).unwrap(), text);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_literal_unicode_savepath_is_preserved_with_native_target_precedence() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let original = std::fs::read_to_string(config).unwrap();
    // Unicode whitespace inside/at the end of an absolute POSIX path is
    // literal filename data, not whitespace to trim or silently repair.
    let saves = root.path().join("saves\u{2003}literal\u{a0}");
    std::fs::create_dir(&saves).unwrap();
    let sentinel = saves.join("existing.sav");
    std::fs::write(&sentinel, b"unchanged save").unwrap();
    let text = format!("{original}\nsavepath={}\n", saves.display());
    std::fs::write(config, &text).unwrap();
    assert!(preflight_scummvm_launch(&request).is_ok());
    assert_eq!(std::fs::read_to_string(config).unwrap(), text);
    assert_eq!(std::fs::read(sentinel).unwrap(), b"unchanged save");
}

#[cfg(target_os = "linux")]
#[test]
fn trainer_duplicate_savepaths_are_rejected_in_application_and_target_domains() {
    let root = tempfile::tempdir().unwrap();
    let request = trainer_request(root.path());
    let config = &request.trainer.as_ref().unwrap().configuration;
    let original = std::fs::read_to_string(config).unwrap();
    let absolute = root.path().join("saves").display().to_string();
    for text in [
        original.replace(
            &format!("savepath={absolute}"),
            &format!("savepath={absolute}\nsavepath={absolute}"),
        ),
        original.replace(
            &format!("savepath={absolute}"),
            &format!("savepath={absolute}\nSAVEPATH=/other"),
        ),
        format!("{original}\nsavepath={absolute}\nsavepath={absolute}\n"),
        format!("{original}\nsavepath={absolute}\nSAVEPATH=/other\n"),
    ] {
        std::fs::write(config, &text).unwrap();
        assert_eq!(
            preflight_scummvm_launch(&request).unwrap_err().kind,
            ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid
        );
        assert_eq!(std::fs::read_to_string(config).unwrap(), text);
    }
}
