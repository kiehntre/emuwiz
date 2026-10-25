use super::*;
use std::process::Command;
fn run(case: &str) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("bin")).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "patch_manager::melonds_install::tests::isolated_child",
            "--nocapture",
        ])
        .env("EMUWIZ_MELONDS_CASE", case)
        .env("EMUWIZ_MELONDS_ROOT", root.path())
        .env("PATH", root.path().join("bin"))
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{case}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
macro_rules! cases { ($($name:ident),* $(,)?) => {$ ( #[test] fn $name() { run(stringify!($name)); } )*}; }
cases!(
    install_history_guarded_undo,
    wrong_hash,
    wrong_profile,
    changed_rom,
    changed_source,
    changed_config,
    changed_executable,
    replaced_identical_rom,
    existing_destination,
    no_confirmation,
    unsupported_codes,
    multiple_entries,
    malformed_source,
    relative_config,
    portable_override,
    multi_instance,
    dsi_mode,
    corrupt_pending,
    disabled_entry,
    explicit_enabled,
    interrupted_receipt,
    injected_write_failure,
    destination_symlink,
    unknown_binary
);
#[test]
fn isolated_child() {
    let Ok(case) = std::env::var("EMUWIZ_MELONDS_CASE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("EMUWIZ_MELONDS_ROOT").unwrap());
    let profile = root.join("config/melonDS");
    let cheats = root.join("cheats");
    fs::create_dir_all(&profile).unwrap();
    fs::create_dir(&cheats).unwrap();
    let exe = root.join("bin/melonDS");
    fs::write(&exe, b"synthetic melonDS 1.1").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = profile.join("melonDS.toml");
    fs::write(
        &config,
        format!(
            "[Instance0]\nCheatFilePath = {:?}\nEnableCheats = false\n",
            cheats.to_str().unwrap()
        ),
    )
    .unwrap();
    let rom = root.join("fixture.nds");
    fs::write(&rom, b"disposable exact DS cartridge bytes").unwrap();
    let inputs = root.join("inputs");
    fs::create_dir(&inputs).unwrap();
    let source = inputs.join("local.mch");
    fs::write(&source, b"ROOT\nCODE 0 Lives\n02000000 00000009\n").unwrap();
    let roots = MelonDsProfileDiscoveryRoots {
        home: root.clone(),
        xdg_config_home: root.join("config"),
        explicit_configuration_roots: vec![],
        portable_configuration_roots: vec![],
        explicit_executables: vec![],
        known_version_outputs: Default::default(),
        appimage_directory: None,
    };
    let mut request = MelonDsInstallRequest {
        rom: rom.clone(),
        expected_rom_sha256: hash(&fs::read(&rom).unwrap()),
        source: source.clone(),
        expected_source_sha256: hash(&fs::read(&source).unwrap()),
        selected: vec![(0, false)],
        profile_id: format!("melonds:{}", profile.display()),
    };
    let destination = cheats.join("fixture.mch");
    match case.as_str() {
        "wrong_hash" => request.expected_rom_sha256 = "a".repeat(64),
        "wrong_profile" => request.profile_id = "wrong".into(),
        "unsupported_codes" => {
            fs::write(&source, b"ROOT\nCODE 0 Conditional\n92000000 00000009\n").unwrap();
            request.expected_source_sha256 = hash(&fs::read(&source).unwrap());
        }
        "multiple_entries" => request.selected.push((0, true)),
        "malformed_source" => {
            fs::write(
                &source,
                b"ROOT\nCODE 0 Lives\nnot code\n02000000 00000009\n",
            )
            .unwrap();
            request.expected_source_sha256 = hash(&fs::read(&source).unwrap());
        }
        "relative_config" => {
            fs::write(&config, "[Instance0]\nCheatFilePath = 'cheats'\n").unwrap();
        }
        "portable_override" => {
            fs::create_dir(exe.parent().unwrap().join("portable")).unwrap();
        }
        "multi_instance" => {
            use std::io::Write;
            fs::OpenOptions::new()
                .append(true)
                .open(&config)
                .unwrap()
                .write_all(b"[Instance1]\nEnableCheats = true\n")
                .unwrap();
        }
        "dsi_mode" => {
            fs::write(
                &config,
                format!(
                    "[Emu]\nConsoleType = 1\n[Instance0]\nCheatFilePath = {:?}\n",
                    cheats.to_str().unwrap()
                ),
            )
            .unwrap();
        }
        "unknown_binary" => {
            fs::write(&exe, b"unknown executable version 1.1").unwrap();
        }
        "existing_destination" => {
            fs::write(&destination, b"user content").unwrap();
        }
        "destination_symlink" => {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&source, &destination).unwrap();
        }
        _ => (),
    }
    if case == "explicit_enabled" {
        request.selected[0].1 = true;
    }
    let p = preview_melonds_install(&request, &roots, None);
    if matches!(
        case.as_str(),
        "wrong_hash"
            | "wrong_profile"
            | "unsupported_codes"
            | "multiple_entries"
            | "malformed_source"
            | "relative_config"
            | "portable_override"
            | "multi_instance"
            | "dsi_mode"
            | "unknown_binary"
            | "existing_destination"
            | "destination_symlink"
    ) {
        assert!(p.is_err(), "{case} unexpectedly accepted");
        return;
    }
    let p = p.unwrap();
    assert!(!destination.exists());
    let mut options = SharedApplyOptions {
        dry_run: false,
        confirmation: Some(SharedApplyConfirmation {
            plan_id: p.plan.plan_id.clone(),
            general_approved: true,
            replacement_approved: false,
        }),
        operation_id: generate_shared_operation_id(),
        timestamp_unix_seconds: 1,
        current_context: p.plan.context.clone(),
        history_root: root.join("history"),
        backup_root: root.join("backups"),
    };
    match case.as_str() {
        "changed_rom" => fs::write(&rom, b"changed ROM").unwrap(),
        "changed_source" => fs::write(&source, b"ROOT\nCODE 0 New\n02000000 00000001\n").unwrap(),
        "changed_config" => {
            fs::write(&config, b"[Instance0]\nCheatFilePath='elsewhere'\n").unwrap()
        }
        "changed_executable" => fs::write(&exe, b"changed executable").unwrap(),
        "replaced_identical_rom" => {
            let b = fs::read(&rom).unwrap();
            fs::rename(&rom, root.join("old.nds")).unwrap();
            fs::write(&rom, b).unwrap();
        }
        "corrupt_pending" => {
            fs::create_dir(&options.history_root).unwrap();
            fs::write(options.history_root.join("bad.pending.json"), b"broken").unwrap();
        }
        "no_confirmation" => options.confirmation = None,
        _ => (),
    }
    if case == "interrupted_receipt" {
        super::super::shared_transaction::inject_fault(Some(
            super::super::shared_transaction::FaultPoint::TerminalJournal,
        ));
    }
    if case == "injected_write_failure" {
        super::super::shared_transaction::inject_fault(Some(
            super::super::shared_transaction::FaultPoint::Flush,
        ));
    }
    let result = apply_melonds_install(&p, &roots, None, &options);
    super::super::shared_transaction::inject_fault(None);
    if matches!(
        case.as_str(),
        "changed_rom"
            | "changed_source"
            | "changed_config"
            | "changed_executable"
            | "replaced_identical_rom"
            | "corrupt_pending"
    ) {
        assert!(result.is_err());
        assert!(!destination.exists());
        return;
    }
    let r = result.unwrap();
    if case == "interrupted_receipt" {
        assert_ne!(r.journal.status, SharedApplyStatus::Success);
        assert!(
            !melonds_install_attention(&options.history_root, &rom)
                .unwrap()
                .is_empty()
        );
        assert!(
            apply_melonds_install(&p, &roots, None, &options)
                .unwrap_err()
                .contains("Earlier pending")
        );
        return;
    }
    if case == "injected_write_failure" {
        assert_ne!(r.journal.status, SharedApplyStatus::Success);
        assert!(!destination.exists());
        return;
    }
    if case == "no_confirmation" {
        assert!(!destination.exists());
        return;
    }
    assert_eq!(r.journal.status, SharedApplyStatus::Success, "{r:?}");
    assert_eq!(
        hash(&fs::read(&source).unwrap()),
        request.expected_source_sha256
    );
    assert_eq!(hash(&fs::read(&config).unwrap()), p.evidence[1].hash);
    let output = fs::read_to_string(&destination).unwrap();
    assert!(output.contains(if case == "explicit_enabled" {
        "CODE 1 Lives"
    } else {
        "CODE 0 Lives"
    }));
    assert_eq!(
        fs::read(&rom).unwrap(),
        b"disposable exact DS cartridge bytes"
    );
    let history = discover_shared_apply_history(&options.history_root);
    assert!(history.complete);
    assert_eq!(history.journals.len(), 1);
    let undo = preview_shared_rollback(
        r.journal_path.as_ref().unwrap(),
        &cheats,
        &options.backup_root,
    );
    assert!(undo.available);
    fs::write(&destination, b"user edit").unwrap();
    assert!(
        !preview_shared_rollback(
            r.journal_path.as_ref().unwrap(),
            &cheats,
            &options.backup_root
        )
        .available
    );
    fs::write(&destination, output).unwrap();
    let undo = preview_shared_rollback(
        r.journal_path.as_ref().unwrap(),
        &cheats,
        &options.backup_root,
    );
    let result = execute_shared_rollback(
        &undo,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: undo.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 2,
            history_root: options.history_root,
            backup_root: options.backup_root,
        },
    );
    assert_eq!(result.status, SharedApplyStatus::Success);
    assert!(!destination.exists());
}
#[test]
fn upstream_limits_and_opcode_subset_are_enforced() {
    let mut e = inspect_entry("ROOT\nCODE 0 Lives\n02000000 00000009\n");
    assert!(melonds_entry_installable(&e));
    e.name = "x".repeat(128);
    assert!(!melonds_entry_installable(&e));
    e = inspect_entry("ROOT\nCODE 0 Misaligned\n02000001 00000009\n");
    assert!(!melonds_entry_installable(&e));
    e = inspect_entry("ROOT\nCODE 0 Outside RAM\n01000000 00000009\n");
    assert!(!melonds_entry_installable(&e));
}
fn inspect_entry(text: &str) -> MelonDsCheatEntry {
    match parse_melonds_cheat_file(text).items.remove(0) {
        MelonDsCheatItem::RootCode(e) => e,
        _ => panic!(),
    }
}

#[test]
fn canonical_source_grammar_cannot_reinterpret_states_or_categories() {
    for text in [
        "CODE 0 MissingRoot\n02000000 00000009\n",
        "ROOT\nCODE 10 Ambiguous\n02000000 00000009\n",
        "CAT 1UP\nCODE 0 Ambiguous\n02000000 00000009\n",
        "ROOT\nCODE 0 Unicode data\n界界界界界X\n",
        "ROOT\nCODE 0 Truncated\n02000000 00000009 extra\n",
    ] {
        assert!(parse_install_source(text.as_bytes()).is_err(), "{text}");
    }
    let source = b"CAT 1 Options\nCODE 0 Lives\nDESC Local test entry\n02000000 00000009\n";
    let parsed = parse_install_source(source).unwrap();
    assert!(parsed.issues.is_empty());
    assert_eq!(parsed.entries[0].name, "Lives");
}

#[test]
fn source_snapshot_not_a_later_read_supplies_the_reviewed_entry() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.mch");
    let approved = b"ROOT\nCODE 0 Reviewed\n02000000 00000009\n";
    fs::write(&path, approved).unwrap();
    let (captured, evidence) = read(&path, MELONDS_CHEAT_MAX_BYTES as u64).unwrap();
    fs::write(&path, b"ROOT\nCODE 0 Different\n02000000 00000001\n").unwrap();
    let parsed = parse_install_source(&captured).unwrap();
    assert_eq!(evidence.hash, hash(approved));
    assert_eq!(parsed.entries[0].name, "Reviewed");
    assert_ne!(
        inspect_melonds_local_cheats(&path).unwrap().1,
        evidence.hash
    );
}

#[test]
fn flat_mch_destinations_retain_shared_path_guards() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("local.mch");
    fs::write(&source, b"local MCH fixture").unwrap();
    let destination = root.path().join("cheats");
    fs::create_dir(&destination).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.path(), destination.join("link")).unwrap();
    for relative in [
        PathBuf::from("link/escaped.mch"),
        PathBuf::from("game.mch"),
        PathBuf::from("../escaped.mch"),
        root.path().join("absolute.mch"),
    ] {
        let report = build_shared_preview(&SharedPreviewRequest {
            adapter: PreviewAdapter::MelonDs,
            selected_archive: root.path().join("game.nds"),
            platform: Some("NDS".into()),
            identity: PreviewIdentity {
                kind: PreviewIdentityKind::MelonDsRomIdentity,
                state: PreviewIdentityState::Verified,
                value: Some("a".repeat(64)),
                archive_path: root.path().join("game.nds"),
                revision: None,
            },
            destination_root: destination.clone(),
            source_items: vec![PreviewSourceItem {
                adapter: PreviewAdapter::MelonDs,
                source_path: source.clone(),
                expected_source_digest: Some(hash(b"local MCH fixture")),
                destination_relative_paths: vec![relative.clone()],
                match_strength: PreviewMatchStrength::VerifiedExact,
            }],
        })
        .unwrap();
        let plan = build_shared_transaction_plan(
            &report,
            "fixture",
            "melonds-native-local-cheats",
            root.path(),
        );
        assert_eq!(
            plan.is_ok(),
            relative == Path::new("game.mch"),
            "{report:?}"
        );
    }
}

#[test]
fn native_filename_mapping_refuses_fallback_and_ambiguous_paths() {
    for path in [
        "/games/.nds",
        "/games/pack.zip|game.nds",
        "/games/back\\slash.nds",
        "/games/not-a-ds.zip",
    ] {
        assert!(cheat_filename(Path::new(path)).is_err(), "{path}");
    }
    for (path, expected) in [
        ("/games/multi.part.nds", "multi.part.mch"),
        ("/games/伝説.NDS", "伝説.mch"),
    ] {
        assert_eq!(
            cheat_filename(Path::new(path)).unwrap(),
            Path::new(expected)
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/games/non-unicode-\xff.nds"));
        assert!(cheat_filename(path).is_err());
    }
}

#[test]
fn configured_cheat_root_cannot_turn_into_a_relative_native_load_path() {
    assert!(config_options(b"[Instance0]\nCheatFilePath = '/'\n").is_err());
    assert!(config_options(b"[Instance0]\nCheatFilePath = '////'\n").is_err());
    let root = tempfile::tempdir().unwrap();
    let text = format!(
        "[Instance0]\nCheatFilePath = {:?}\n",
        format!("{}///", root.path().display())
    );
    assert_eq!(config_options(text.as_bytes()).unwrap().0, root.path());
}

#[test]
fn ignored_comments_cannot_hide_native_buffer_continuation_codes() {
    let mut comment = "#".to_string();
    comment.push_str(&"x".repeat(1022));
    comment.push_str("CODE 1 Hidden");
    comment.push_str(&" ".repeat(1023 - "CODE 1 Hidden".len()));
    comment.push_str("92000000 00000001");
    let text = format!("ROOT\n{comment}\nCODE 0 Reviewed\n02000000 00000009\n");
    assert!(parse_install_source(text.as_bytes()).is_err());
    let short = b"ROOT\n# ordinary local note\nCODE 0 Reviewed\n02000000 00000009\n";
    assert!(parse_install_source(short).unwrap().issues.is_empty());
    for prefix in ["\u{000c}", "\u{3000}"] {
        let text =
            format!("ROOT\n{prefix}# not a native comment\nCODE 0 Reviewed\n02000000 00000009\n");
        assert!(parse_install_source(text.as_bytes()).is_err());
    }
}
