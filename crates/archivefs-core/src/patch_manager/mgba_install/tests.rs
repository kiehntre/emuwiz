use super::*;
use std::process::Command;

// Child tests receive a private PATH/HOME without mutating this parallel test
// process's environment or observing a real mGBA installation.
fn run(case: &str) {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "patch_manager::mgba_install::tests::isolated_child",
            "--nocapture",
        ])
        .env("EMUWIZ_MGBA_CASE", case)
        .env("EMUWIZ_MGBA_ROOT", root.path())
        .env("PATH", &bin)
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
    if case == "interruption_is_discoverable_after_restart" {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "patch_manager::mgba_install::tests::isolated_child",
                "--nocapture",
            ])
            .env("EMUWIZ_MGBA_CASE", "inspect_pending_after_restart")
            .env("EMUWIZ_MGBA_ROOT", root.path())
            .env("PATH", &bin)
            .env("HOME", root.path())
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
}
macro_rules! cases { ($($name:ident),* $(,)?) => {$ ( #[test] fn $name() { run(stringify!($name)); } )*}; }
cases!(
    normal_install_history_and_undo,
    missing_remembered_profile_refuses,
    wrong_profile_refuses,
    wrong_rom_hash_refuses,
    changed_rom_refuses,
    changed_source_refuses,
    changed_executable_refuses,
    changed_config_refuses,
    replaced_same_bytes_refuses,
    destination_conflict_refuses,
    existing_user_cheats_refuse,
    unsupported_format_refuses,
    conflicting_ram_writes_refuse,
    relative_or_missing_config_refuses,
    symlink_substitution_refuses,
    external_edit_blocks_undo,
    repeated_undo_refuses,
    injected_write_failure_retains_truth,
    incomplete_journal_refuses_undo,
    no_confirmation_is_dry_run,
    directory_replacement_refuses,
    source_change_before_preview_refuses,
    disabled_entries_remain_disabled,
    missing_source_refuses,
    missing_executable_refuses,
    overlapping_ram_entries_refuse,
    explicit_enable_state,
    malformed_truncated_source_refuses,
    corrupt_pending_blocks_install,
    interruption_is_discoverable_after_restart
);

struct Fixture {
    root: PathBuf,
    roots: MgbaProfileDiscoveryRoots,
    request: MgbaInstallRequest,
    destination: PathBuf,
    config: PathBuf,
    exe: PathBuf,
}
impl Fixture {
    fn new(root: PathBuf) -> Self {
        let profile = root.join("config/mgba");
        let cheats = root.join("cheats");
        fs::create_dir_all(&profile).unwrap();
        fs::create_dir(&cheats).unwrap();
        let exe = root.join("bin/mgba");
        fs::write(&exe, b"synthetic executable, never run").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = profile.join("config.ini");
        fs::write(
            &config,
            format!("cheatsPath={}\ncheatAutoload=1\n", cheats.display()),
        )
        .unwrap();
        let rom = root.join("fixture.gba");
        fs::write(&rom, b"synthetic replaceable GBA bytes").unwrap();
        let inputs = root.join("inputs");
        fs::create_dir(&inputs).unwrap();
        let source = inputs.join("local.cheats");
        fs::write(&source, b"# lives\n02000000:09\n# second\n03000000:01\n").unwrap();
        let request = MgbaInstallRequest {
            rom: rom.clone(),
            expected_rom_sha256: hash(&fs::read(&rom).unwrap()),
            source: source.clone(),
            expected_source_sha256: hash(&fs::read(&source).unwrap()),
            selected: vec![(0, false)],
            profile_id: format!("mgba:{}", profile.display()),
        };
        let roots = MgbaProfileDiscoveryRoots {
            home: root.clone(),
            xdg_config_home: root.join("config"),
            explicit_configuration_roots: vec![],
            portable_configuration_roots: vec![],
            explicit_executables: vec![],
            known_version_outputs: Default::default(),
        };
        Self {
            root,
            roots,
            request,
            destination: cheats.join("fixture.cheats"),
            config,
            exe,
        }
    }
    fn preview(&self) -> MgbaInstallPreview {
        preview_mgba_install(&self.request, &self.roots, None).unwrap()
    }
    fn options(&self, p: &MgbaInstallPreview) -> SharedApplyOptions {
        SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: p.plan.plan_id.clone(),
                general_approved: true,
                replacement_approved: false,
            }),
            operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 1,
            current_context: p.plan.context.clone(),
            history_root: self.root.join("history"),
            backup_root: self.root.join("backups"),
        }
    }
    fn apply(&self, p: &MgbaInstallPreview) -> Result<SharedApplyResult, String> {
        apply_mgba_install(p, &self.roots, None, &self.options(p))
    }
    fn undo_preview(&self, r: &SharedApplyResult) -> SharedRollbackPreview {
        preview_shared_rollback(
            r.journal_path.as_ref().unwrap(),
            self.destination.parent().unwrap(),
            &self.root.join("backups"),
        )
    }
    fn undo(&self, p: &SharedRollbackPreview) -> SharedRollbackResult {
        execute_shared_rollback(
            p,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: p.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: 2,
                history_root: self.root.join("history"),
                backup_root: self.root.join("backups"),
            },
        )
    }
}
#[test]
fn isolated_child() {
    let Ok(case) = std::env::var("EMUWIZ_MGBA_CASE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("EMUWIZ_MGBA_ROOT").unwrap());
    if case == "inspect_pending_after_restart" {
        let attention =
            mgba_install_attention(&root.join("history"), &root.join("fixture.gba")).unwrap();
        assert!(!attention.is_empty());
        assert!(root.join("cheats/fixture.cheats").is_file());
        return;
    }
    let mut f = Fixture::new(root);
    let rom_before = read(&f.request.rom, 1024).unwrap();
    let source_before = fs::read(&f.request.source).unwrap();
    let config_before = fs::read(&f.config).unwrap();
    if case == "explicit_enable_state" {
        f.request.selected[0].1 = true;
    }
    match case.as_str() {
        "missing_remembered_profile_refuses" => {
            assert!(select_mgba_cheat_profile(&f.roots, Some("missing")).is_err());
            return;
        }
        "overlapping_ram_entries_refuse" => {
            fs::write(
                &f.request.source,
                b"# one\n02000000:01\n# two\n02000000:02\n",
            )
            .unwrap();
            f.request.expected_source_sha256 = hash(&fs::read(&f.request.source).unwrap());
            f.request.selected = vec![(0, true), (1, true)];
            assert!(preview_mgba_install(&f.request, &f.roots, None).is_err());
            return;
        }
        "malformed_truncated_source_refuses" => {
            let mut bytes = b"# too many\n".to_vec();
            for _ in 0..129 {
                bytes.extend_from_slice(b"02000000:09\n");
            }
            fs::write(&f.request.source, &bytes).unwrap();
            f.request.expected_source_sha256 = hash(&bytes);
            let e = preview_mgba_install(&f.request, &f.roots, None).unwrap_err();
            assert!(e.contains("truncated"));
            return;
        }
        "wrong_profile_refuses" => {
            f.request.profile_id = "other".into();
            assert!(preview_mgba_install(&f.request, &f.roots, None).is_err());
            return;
        }
        "wrong_rom_hash_refuses" => {
            f.request.expected_rom_sha256 = "0".repeat(64);
            assert!(preview_mgba_install(&f.request, &f.roots, None).is_err());
            return;
        }
        "existing_user_cheats_refuse" => {
            fs::write(&f.destination, b"user-created").unwrap();
            assert!(preview_mgba_install(&f.request, &f.roots, None).is_err());
            assert_eq!(fs::read(&f.destination).unwrap(), b"user-created");
            return;
        }
        "unsupported_format_refuses" => {
            fs::write(&f.request.source, b"!GSAv1\n# opaque\n12345678 90ABCDEF\n").unwrap();
            f.request.expected_source_sha256 = hash(&fs::read(&f.request.source).unwrap());
            assert!(preview_mgba_install(&f.request, &f.roots, None).is_err());
            return;
        }
        "conflicting_ram_writes_refuse" => {
            f.request.selected = vec![(0, false), (0, true)];
            assert!(preview_mgba_install(&f.request, &f.roots, None).is_err());
            return;
        }
        "relative_or_missing_config_refuses" => {
            for bytes in [
                "cheatsPath=relative\n",
                "cheatAutoload=1\n",
                "[ports.sdl]\ncheatsPath=/tmp\n",
            ] {
                fs::write(&f.config, bytes).unwrap();
                assert!(preview_mgba_install(&f.request, &f.roots, None).is_err());
            }
            return;
        }
        "source_change_before_preview_refuses" => {
            fs::write(&f.request.source, b"# unexpected\n02000000:01\n").unwrap();
            assert!(preview_mgba_install(&f.request, &f.roots, None).is_err());
            return;
        }
        _ => {}
    }
    let p = f.preview();
    match case.as_str() {
        "missing_source_refuses" => {
            fs::remove_file(&f.request.source).unwrap();
            assert!(f.apply(&p).is_err());
        }
        "missing_executable_refuses" => {
            fs::remove_file(&f.exe).unwrap();
            assert!(f.apply(&p).is_err());
        }
        "corrupt_pending_blocks_install" => {
            fs::create_dir(f.root.join("history")).unwrap();
            fs::write(f.root.join("history/uninterpretable.pending.json"), b"{").unwrap();
            assert!(mgba_install_attention(&f.root.join("history"), &f.request.rom).is_err());
            assert!(f.apply(&p).unwrap_err().contains("Uninterpretable"));
        }
        "interruption_is_discoverable_after_restart" => {
            super::super::shared_transaction::inject_fault(Some(
                super::super::shared_transaction::FaultPoint::TerminalJournal,
            ));
            let r = f.apply(&p).unwrap();
            super::super::shared_transaction::inject_fault(None);
            assert_ne!(r.journal.status, SharedApplyStatus::Success);
            assert!(
                !mgba_install_attention(&f.root.join("history"), &f.request.rom)
                    .unwrap()
                    .is_empty()
            );
            assert!(f.apply(&p).unwrap_err().contains("Earlier pending"));
            assert!(f.destination.exists());
            return;
        }
        "changed_rom_refuses" => {
            fs::write(&f.request.rom, b"changed").unwrap();
            assert!(f.apply(&p).is_err());
        }
        "changed_source_refuses" => {
            fs::write(&f.request.source, b"# changed\n02000000:00\n").unwrap();
            assert!(f.apply(&p).is_err());
        }
        "changed_config_refuses" => {
            fs::write(&f.config, b"cheatsPath=/other\n").unwrap();
            assert!(f.apply(&p).is_err());
        }
        "changed_executable_refuses" => {
            fs::write(&f.exe, b"other executable").unwrap();
            assert!(f.apply(&p).is_err());
        }
        "replaced_same_bytes_refuses" => {
            let b = fs::read(&f.exe).unwrap();
            let t = f.root.join("new-exe");
            fs::write(&t, b).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&t, fs::Permissions::from_mode(0o700)).unwrap();
            }
            fs::rename(t, &f.exe).unwrap();
            assert!(f.apply(&p).is_err());
        }
        "destination_conflict_refuses" => {
            fs::write(&f.destination, b"foreign").unwrap();
            assert!(f.apply(&p).is_err());
            assert_eq!(fs::read(&f.destination).unwrap(), b"foreign");
            return;
        }
        "directory_replacement_refuses" => {
            let d = f.destination.parent().unwrap();
            fs::rename(d, f.root.join("old-cheats")).unwrap();
            fs::create_dir(d).unwrap();
            assert!(f.apply(&p).is_err());
        }
        "symlink_substitution_refuses" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                fs::rename(&f.request.source, f.root.join("original-source")).unwrap();
                symlink(f.root.join("original-source"), &f.request.source).unwrap();
                assert!(f.apply(&p).is_err());
            }
        }
        "no_confirmation_is_dry_run" => {
            let mut options = f.options(&p);
            options.confirmation = None;
            let r = apply_mgba_install(&p, &f.roots, None, &options).unwrap();
            assert_eq!(r.journal.status, SharedApplyStatus::DryRun);
        }
        "injected_write_failure_retains_truth" => {
            super::super::shared_transaction::inject_fault(Some(
                super::super::shared_transaction::FaultPoint::Flush,
            ));
            let r = f.apply(&p).unwrap();
            super::super::shared_transaction::inject_fault(None);
            assert_ne!(r.journal.status, SharedApplyStatus::Success);
            assert!(!f.destination.exists());
            assert!(
                !discover_shared_apply_history(&f.root.join("history"))
                    .journals
                    .is_empty()
            );
        }
        "normal_install_history_and_undo"
        | "external_edit_blocks_undo"
        | "repeated_undo_refuses"
        | "incomplete_journal_refuses_undo"
        | "disabled_entries_remain_disabled"
        | "explicit_enable_state" => {
            let r = f.apply(&p).unwrap();
            assert_eq!(
                r.journal.status,
                SharedApplyStatus::Success,
                "{:?}",
                r.journal.entries
            );
            assert_eq!(fs::read(&f.destination).unwrap(), p.materialized.bytes);
            assert_eq!(
                read(&f.request.rom, 1024).unwrap().1.identity,
                rom_before.1.identity
            );
            assert_eq!(fs::read(&f.request.rom).unwrap(), rom_before.0);
            assert_eq!(
                discover_shared_apply_history(&f.root.join("history"))
                    .journals
                    .len(),
                1
            );
            assert_eq!(fs::read(&f.request.source).unwrap(), source_before);
            assert_eq!(fs::read(&f.config).unwrap(), config_before);
            if case == "explicit_enable_state" {
                assert_eq!(
                    parse_mgba_cheat_file(&fs::read(&f.destination).unwrap())
                        .unwrap()
                        .entries[0]
                        .state,
                    MgbaCheatState::Enabled
                );
                return;
            }
            if case == "disabled_entries_remain_disabled" {
                assert_eq!(
                    parse_mgba_cheat_file(&fs::read(&f.destination).unwrap())
                        .unwrap()
                        .entries[0]
                        .state,
                    MgbaCheatState::Disabled
                );
                return;
            }
            if case == "external_edit_blocks_undo" {
                fs::write(&f.destination, b"user change").unwrap();
                assert!(!f.undo_preview(&r).available);
                assert_eq!(fs::read(&f.destination).unwrap(), b"user change");
                return;
            }
            if case == "incomplete_journal_refuses_undo" {
                fs::write(r.journal_path.as_ref().unwrap(), b"{").unwrap();
                assert!(!f.undo_preview(&r).available);
                assert!(f.destination.exists());
                return;
            }
            let undo = f.undo_preview(&r);
            assert!(undo.available, "{undo:?}");
            let result = f.undo(&undo);
            assert_eq!(result.status, SharedApplyStatus::Success);
            assert!(!f.destination.exists());
            if case == "repeated_undo_refuses" {
                assert_ne!(f.undo(&undo).status, SharedApplyStatus::Success);
            }
        }
        _ => panic!("unknown case {case}"),
    }
    if !matches!(
        case.as_str(),
        "normal_install_history_and_undo" | "repeated_undo_refuses"
    ) {
        assert!(!f.destination.exists());
    }
}
