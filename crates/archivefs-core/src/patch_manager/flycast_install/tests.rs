use super::*;
use std::process::Command;
const MOCK: &[u8] = b"synthetic Flycast fixture -- never executed\n";
pub(super) fn mock_artifact(path: &Path, hash: &str) -> bool {
    std::env::var_os("EMUWIZ_FLYCAST_MOCK_ARTIFACT")
        .is_some_and(|p| Path::new(&p) == path && hash == digest(MOCK))
}
struct Machine {
    _dir: tempfile::TempDir,
    env: FlycastInstallEnvironment,
    game: PathBuf,
    source: PathBuf,
    history: PathBuf,
    backups: PathBuf,
}
impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config/flycast");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("emu.cfg"),
            b"; user settings\n[config]\naudio = untouched\n\n[other]\nkey = value\n",
        )
        .unwrap();
        let roots = FlycastProfileDiscoveryRoots {
            home: dir.path().join("home"),
            xdg_config_home: dir.path().join("config"),
            xdg_data_home: dir.path().join("data"),
            explicit_configuration_roots: vec![],
            portable_configuration_roots: vec![],
            explicit_executables: vec![],
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        };
        let selection_path = dir.path().join("selection.toml");
        remember_selection_to(
            &selection_path,
            InventoryEmulator::Flycast,
            &EmulatorSelectionMode::Auto,
        )
        .unwrap();
        let env = FlycastInstallEnvironment {
            roots,
            selection_path,
            launch_profile_id: format!("flycast:{}", root.display()),
            launch_profile_root: root,
        };
        let game = dir.path().join("synthetic.iso");
        fs::write(&game, synthetic_game()).unwrap();
        let source = dir.path().join("import.cht");
        fs::write(&source, native(256, 3, 9)).unwrap();
        let history = dir.path().join("history");
        let backups = dir.path().join("backups");
        Self {
            _dir: dir,
            env,
            game,
            source,
            history,
            backups,
        }
    }
    fn preview(&self) -> Checked<FlycastInstallPreview> {
        preview_flycast_install(&self.env, &self.game, "T-TEST", &self.source, &[(0, false)])
    }
    fn options(&self) -> FlycastCheatApplyOptions {
        FlycastCheatApplyOptions {
            general_approved: true,
            replacement_approved: true,
            operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 123,
            history_root: self.history.clone(),
            backup_root: self.backups.clone(),
        }
    }
    fn apply(&self, p: &FlycastInstallPreview) -> Checked<SharedApplyResult> {
        apply_flycast_install(&self.env, p, &self.game, "T-TEST", &self.options())
    }
    fn destination(&self) -> PathBuf {
        self.env.launch_profile_root.join("0-emuwiz-T-TEST.cht")
    }
    fn undo(&self, result: &SharedApplyResult) -> SharedRollbackResult {
        let p = preview_shared_rollback(
            result.journal_path.as_ref().unwrap(),
            &self.env.launch_profile_root,
            &self.backups,
        );
        assert!(p.available, "{p:?}");
        execute_shared_rollback(
            &p,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: p.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: 124,
                history_root: self.history.clone(),
                backup_root: self.backups.clone(),
            },
        )
    }
}
// Isolate PATH discovery in a child test process. No process-global environment
// changes race other tests, and the mock executable is never run.
fn machine(test: impl FnOnce(Machine)) {
    if std::env::var_os("EMUWIZ_FLYCAST_MOCK_ARTIFACT").is_some() {
        test(Machine::new());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("flycast");
    fs::write(&exe, MOCK).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(std::thread::current().name().unwrap())
        .arg("--nocapture")
        .env("PATH", dir.path())
        .env("EMUWIZ_FLYCAST_MOCK_ARTIFACT", &exe)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn native(address: u32, shift: u32, value: u32) -> Vec<u8> {
    format!("cheats = 1\ncheat0_address = {address}\ncheat0_address_bit_position = 0\ncheat0_big_endian = false\ncheat0_cheat_type = 1\ncheat0_code = \ncheat0_desc = Synthetic health\ncheat0_dest_address = 0\ncheat0_enable = false\ncheat0_handler = 1\ncheat0_memory_search_size = {shift}\ncheat0_repeat_add_to_address = 0\ncheat0_repeat_add_to_value = 0\ncheat0_repeat_count = 1\ncheat0_value = {value}\n").into_bytes()
}
#[test]
fn official_saved_native_shape_is_supported_and_defaults_preserved() {
    for shift in [3, 4, 5] {
        let f = validate_native_file(&native(256, shift, 9)).unwrap();
        assert!(!f.entries[0].enabled);
        assert_eq!(f.entries[0].fields["handler"], "1");
    }
}
#[test]
fn refuses_unsafe_native_semantics() {
    for (from, to) in [
        ("cheat_type = 1", "cheat_type = 8"),
        ("memory_search_size = 3", "memory_search_size = 0"),
        ("repeat_count = 1", "repeat_count = 2"),
        ("address_bit_position = 0", "address_bit_position = 255"),
        ("handler = 1", "handler = 0"),
        ("code = ", "code = 01234567"),
        ("big_endian = false", "big_endian = true"),
        ("cheats = 1", "cheats = 2"),
        ("enable = false", "enable = truthy"),
        ("value = 9", "value = 9garbage"),
        ("enable = false", "enable = false "),
    ] {
        let bytes = String::from_utf8(native(256, 3, 9))
            .unwrap()
            .replace(from, to);
        assert!(
            validate_native_file(bytes.as_bytes()).is_err(),
            "{from} -> {to}"
        );
    }
    for bytes in [
        native(16 * 1024 * 1024 - 1, 5, 9),
        native(256, 3, 256),
        b"cheats=1\ncheat0_code=ABC".to_vec(),
        b"\xff".to_vec(),
        vec![b' '; FLYCAST_CHEAT_MAX_BYTES],
    ] {
        assert!(validate_native_file(&bytes).is_err());
    }
}
#[test]
fn refuses_sparse_duplicate_and_truncated_files() {
    for bytes in [
        String::from_utf8(native(256, 3, 9))
            .unwrap()
            .replace("cheat0_", "cheat1_"),
        format!(
            "{}cheats=1\n",
            String::from_utf8(native(256, 3, 9)).unwrap()
        ),
        format!(
            "{}cheat0_value=10\n",
            String::from_utf8(native(256, 3, 9)).unwrap()
        ),
        format!(
            "[cheats]\n{}",
            String::from_utf8(native(256, 3, 9)).unwrap()
        ),
    ] {
        assert!(validate_native_file(bytes.as_bytes()).is_err());
    }
}
#[test]
fn config_mapping_is_exact_and_preserves_unrelated_bytes() {
    let p = Path::new("/disposable/profile/T-TEST.cht");
    for input in [
        "[config]\naudio = user\n",
        "[cheats]\nother = /user/other.cht\n[config]\naudio = user\n",
        "[cheats]\nother = /user/other.cht",
    ] {
        let output = mapped_config(input.as_bytes(), "T-TEST", p).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("T-TEST = \"/disposable/profile/T-TEST.cht\""));
        for original in input.lines() {
            assert!(output.lines().any(|line| line == original));
        }
        assert_eq!(
            mapped_config(output.as_bytes(), "T-TEST", p).unwrap(),
            output.as_bytes()
        );
    }
}
#[test]
fn config_conflicts_are_not_overwritten() {
    let p = Path::new("/disposable/profile/T-TEST.cht");
    for input in [
        "[cheats]\nT-TEST = /user/original.cht\n",
        "[cheats]\n[cheats]\n",
        "[cheats]\nT-TEST = x\nT-TEST = y\n",
        "[broken\n",
        "[config]\nx = 1\nx = 2\n",
    ] {
        assert!(mapped_config(input.as_bytes(), "T-TEST", p).is_err());
    }
    assert!(mapped_config(b"[config]\n", " T-TEST", p).is_err());
}
#[test]
fn unknown_version_names_and_injected_version_output_are_not_proof() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("Flycast-v2.7.AppImage");
    fs::write(&p, b"Flycast v2.7").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(artifact(&p).is_err());
}
#[test]
fn portable_and_unproven_paths_are_refused() {
    machine(|m| {
        let exe = PathBuf::from(std::env::var_os("EMUWIZ_FLYCAST_MOCK_ARTIFACT").unwrap());
        let mut sibling = exe.as_os_str().to_os_string();
        sibling.push(".config");
        fs::create_dir(&sibling).unwrap();
        assert!(
            effective_root(
                &m.env.roots,
                &exe,
                &LaunchInstallation::AppImage {
                    extract_and_run: false
                }
            )
            .is_err()
        );
        fs::remove_dir(&sibling).unwrap();
        let mut e = m.env.clone();
        e.launch_profile_root = m._dir.path().join("portable");
        assert!(e.resolve().is_err());
    });
}
#[test]
fn preview_apply_readback_history_undo_preserves_config_and_game() {
    machine(|m| {
        let original = fs::read(m.env.launch_profile_root.join("emu.cfg")).unwrap();
        let p = m.preview().unwrap();
        assert_eq!(p.changes.len(), 2);
        assert!(!m.destination().exists());
        assert_eq!(
            fs::read(m.env.launch_profile_root.join("emu.cfg")).unwrap(),
            original
        );
        let r = m.apply(&p).unwrap();
        assert_eq!(r.journal.status, SharedApplyStatus::Success, "{r:?}");
        assert!(r.journal.entries.iter().all(|e| e.verification_succeeded));
        assert!(
            !validate_native_file(&fs::read(m.destination()).unwrap())
                .unwrap()
                .entries[0]
                .enabled
        );
        let cfg = fs::read(m.env.launch_profile_root.join("emu.cfg")).unwrap();
        assert!(String::from_utf8(cfg).unwrap().contains("[cheats]"));
        assert_eq!(discover_shared_apply_history(&m.history).journals.len(), 1);
        let u = m.undo(&r);
        assert_eq!(u.status, SharedApplyStatus::Success);
        assert!(!m.destination().exists());
        assert_eq!(
            fs::read(m.env.launch_profile_root.join("emu.cfg")).unwrap(),
            original
        );
        assert_eq!(fs::read(&m.game).unwrap(), synthetic_game());
        assert!(m.preview().is_ok());
    });
}
#[test]
fn current_user_cheats_are_merged_and_undo_restores_exact_bytes() {
    machine(|m| {
        let original = native(1024, 5, 99);
        fs::write(m.destination(), &original).unwrap();
        let config = m.env.launch_profile_root.join("emu.cfg");
        let mapped =
            mapped_config(&fs::read(&config).unwrap(), "T-TEST", &m.destination()).unwrap();
        fs::write(config, mapped).unwrap();
        let p = m.preview().unwrap();
        let r = m.apply(&p).unwrap();
        assert_eq!(r.journal.status, SharedApplyStatus::Success, "{r:?}");
        let f = validate_native_file(&fs::read(m.destination()).unwrap()).unwrap();
        assert_eq!(f.entries.len(), 2);
        assert_eq!(f.entries[0].description, "Synthetic health");
        let undo = m.undo(&r);
        assert_eq!(undo.status, SharedApplyStatus::Success, "{undo:?}");
        assert_eq!(fs::read(m.destination()).unwrap(), original);
    });
}
#[test]
fn overlapping_existing_cheats_and_other_mapping_refuse_without_writes() {
    machine(|m| {
        let original = native(256, 4, 3);
        fs::write(m.destination(), &original).unwrap();
        assert!(m.preview().is_err());
        assert_eq!(fs::read(m.destination()).unwrap(), original);
        fs::remove_file(m.destination()).unwrap();
        let config = m.env.launch_profile_root.join("emu.cfg");
        let bytes = b"[cheats]\nT-TEST = /other/user.cht\n";
        fs::write(&config, bytes).unwrap();
        assert!(m.preview().is_err());
        assert_eq!(fs::read(config).unwrap(), bytes);
        assert!(!m.destination().exists());
    });
}
#[test]
fn stale_source_config_destination_profile_policy_and_game_refuse() {
    machine(|m| {
        let p = m.preview().unwrap();
        fs::write(&m.source, native(256, 4, 9)).unwrap();
        assert!(m.apply(&p).is_err());
        fs::write(&m.source, native(256, 3, 9)).unwrap();
        let p = m.preview().unwrap();
        let config = m.env.launch_profile_root.join("emu.cfg");
        let original = fs::read(&config).unwrap();
        fs::write(&config, b"[config]\nuser = changed\n").unwrap();
        assert!(m.apply(&p).is_err());
        fs::write(&config, &original).unwrap();
        let p = m.preview().unwrap();
        fs::write(m.destination(), b"user change").unwrap();
        let r = m.apply(&p).unwrap();
        assert_ne!(r.journal.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(m.destination()).unwrap(), b"user change");
        assert_eq!(
            fs::read(&config).unwrap(),
            original,
            "A refused cheat must never activate a new mapping"
        );
        fs::remove_file(m.destination()).unwrap();
        // Use another clean machine for subsequent refused preflight checks.
        let mut other = m.env.clone();
        other.launch_profile_root = m._dir.path().join("other");
        assert!(other.resolve().is_err());
        remember_selection_to(
            &m.env.selection_path,
            InventoryEmulator::Flycast,
            &EmulatorSelectionMode::Forced(CandidateIdentity {
                emulator: InventoryEmulator::Flycast,
                executable: Some("/missing/flycast".into()),
                profile_root: m.env.launch_profile_root.clone(),
            }),
        )
        .unwrap();
        assert!(m.apply(&p).is_err());
        assert!(apply_flycast_install(&m.env, &p, &m.game, "OTHER", &m.options()).is_err());
    });
}
#[test]
fn auto_preferred_forced_and_missing_selection_use_shared_policy() {
    machine(|m| {
        let b = m.env.resolve().unwrap();
        let id = CandidateIdentity {
            emulator: InventoryEmulator::Flycast,
            executable: Some(b.executable.clone()),
            profile_root: b.profile_root.clone(),
        };
        for mode in [
            EmulatorSelectionMode::Auto,
            EmulatorSelectionMode::Preferred(id.clone()),
            EmulatorSelectionMode::Forced(id),
        ] {
            remember_selection_to(&m.env.selection_path, InventoryEmulator::Flycast, &mode)
                .unwrap();
            assert_eq!(m.env.resolve().unwrap().selection, mode);
        }
        let missing = CandidateIdentity {
            emulator: InventoryEmulator::Flycast,
            executable: Some("/missing/flycast".into()),
            profile_root: m.env.launch_profile_root.clone(),
        };
        remember_selection_to(
            &m.env.selection_path,
            InventoryEmulator::Flycast,
            &EmulatorSelectionMode::Preferred(missing.clone()),
        )
        .unwrap();
        let b = m.env.resolve().unwrap();
        assert!(b.selection_summary.contains("Bypassed"));
        remember_selection_to(
            &m.env.selection_path,
            InventoryEmulator::Flycast,
            &EmulatorSelectionMode::Forced(missing),
        )
        .unwrap();
        assert!(m.env.resolve().is_err());
        fs::remove_file(&m.env.selection_path).unwrap();
        assert!(m.preview().unwrap_err().contains("save AUTO"));
    });
}
#[test]
fn confirmation_and_guarded_undo_refuse_unapproved_or_modified_files() {
    machine(|m| {
        let p = m.preview().unwrap();
        let mut opts = m.options();
        opts.general_approved = false;
        let r = apply_flycast_install(&m.env, &p, &m.game, "T-TEST", &opts).unwrap();
        assert_ne!(r.journal.status, SharedApplyStatus::Success);
        assert!(!m.destination().exists());
        assert!(refuse_unfinished(&m.history, &m.game).is_ok());
        let retry = m.preview().unwrap();
        assert_eq!(
            m.apply(&retry).unwrap().journal.status,
            SharedApplyStatus::Success
        );
        // A separate machine tests later user edits without changing this receipt.
        let n = Machine::new();
        let p = n.preview().unwrap();
        let r = n.apply(&p).unwrap();
        assert_eq!(r.journal.status, SharedApplyStatus::Success, "{r:?}");
        fs::write(n.destination(), b"later user changes").unwrap();
        let u = preview_shared_rollback(
            r.journal_path.as_ref().unwrap(),
            &n.env.launch_profile_root,
            &n.backups,
        );
        assert!(!u.available);
        assert_eq!(fs::read(n.destination()).unwrap(), b"later user changes");
    });
}
#[test]
fn malformed_history_and_interrupted_operations_block_new_writes() {
    machine(|m| {
        fs::create_dir_all(&m.history).unwrap();
        fs::write(m.history.join("broken.json"), b"bad").unwrap();
        assert!(refuse_unfinished(&m.history, &m.game).is_err());
    });
}
#[test]
fn interrupted_apply_is_not_claimed_complete_and_blocks_retry() {
    machine(|m| {
        use super::super::shared_transaction::{FaultPoint, inject_fault};
        let p = m.preview().unwrap();
        inject_fault(Some(FaultPoint::ApplyCheckpoint));
        let r = m.apply(&p).unwrap();
        inject_fault(None);
        assert!(r.journal_failure.is_some());
        assert!(r.journal_path.is_none());
        assert!(!discover_pending_operations(&m.history).is_empty());
        assert!(refuse_unfinished(&m.history, &m.game).is_err());
    });
}
#[test]
fn successful_guarded_rollback_releases_a_failed_history_block() {
    machine(|m| {
        use super::super::shared_transaction::{FaultPoint, inject_fault};
        let p = m.preview().unwrap();
        inject_fault(Some(FaultPoint::BackupWrite));
        let r = m.apply(&p).unwrap();
        inject_fault(None);
        assert_eq!(r.journal.status, SharedApplyStatus::PartialFailure, "{r:?}");
        assert!(refuse_unfinished(&m.history, &m.game).is_err());
        assert!(refuse_unfinished(&m.history, Path::new("/other/game.gdi")).is_err());
        let undo = m.undo(&r);
        assert_eq!(undo.status, SharedApplyStatus::Success, "{undo:?}");
        assert!(refuse_unfinished(&m.history, &m.game).is_ok());
        assert!(m.preview().is_ok());
    });
}
#[cfg(unix)]
#[test]
fn symlink_destinations_and_source_are_refused() {
    machine(|m| {
        use std::os::unix::fs::symlink;
        let outside = m._dir.path().join("user.cht");
        fs::write(&outside, native(1024, 3, 1)).unwrap();
        symlink(&outside, m.destination()).unwrap();
        assert!(m.preview().is_err());
        assert_eq!(fs::read(&outside).unwrap(), native(1024, 3, 1));
        fs::remove_file(m.destination()).unwrap();
        fs::remove_file(&m.source).unwrap();
        symlink(&outside, &m.source).unwrap();
        assert!(m.preview().is_err());
    });
}

fn synthetic_game() -> Vec<u8> {
    let mut bytes = vec![b' '; 2048];
    bytes[..16].copy_from_slice(b"SEGA SEGAKATANA ");
    bytes[0x40..0x4a].copy_from_slice(b"T-TEST    ");
    bytes
}

#[test]
fn noncanonical_entry_prefixes_are_not_reinterpreted() {
    for prefix in ["cheat00_", "cheat+0_", "cheat01_"] {
        let text = String::from_utf8(native(256, 3, 9))
            .unwrap()
            .replace("cheat0_", prefix);
        assert!(validate_native_file(text.as_bytes()).is_err());
    }
}

#[test]
fn existing_unmapped_user_file_refuses_two_replacement_plan_without_mutation() {
    machine(|m| {
        let original = native(1024, 5, 99);
        fs::write(m.destination(), &original).unwrap();
        let config = m.env.launch_profile_root.join("emu.cfg");
        let before = fs::read(&config).unwrap();
        assert!(m.preview().unwrap_err().contains("one replacement"));
        assert_eq!(fs::read(config).unwrap(), before);
        assert_eq!(fs::read(m.destination()).unwrap(), original);
    });
}

/// Optional release-artifact verification. The fixtures are downloaded outside
/// the repository, pinned above, and never executed. No large binary is vendored.
#[test]
#[ignore = "requires independently downloaded official v2.7 AppImage and enclosed executable"]
fn official_v27_artifacts_use_the_production_gate_and_complete_apply_undo() {
    if std::env::var_os("EMUWIZ_FLYCAST_REAL_ARTIFACT_CHILD").is_some() {
        let m = Machine::new();
        let binding = m.env.resolve().unwrap();
        assert!(matches!(
            binding.executable_sha256.as_str(),
            APPIMAGE | NATIVE
        ));
        let p = m.preview().unwrap();
        let r = m.apply(&p).unwrap();
        assert_eq!(r.journal.status, SharedApplyStatus::Success, "{r:?}");
        assert_eq!(m.undo(&r).status, SharedApplyStatus::Success);
        return;
    }
    for variable in [
        "EMUWIZ_FLYCAST_OFFICIAL_APPIMAGE",
        "EMUWIZ_FLYCAST_OFFICIAL_NATIVE",
    ] {
        let source = std::env::var_os(variable)
            .unwrap_or_else(|| panic!("Supply {variable} as a disposable official artifact"));
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("flycast");
        fs::copy(source, &exe).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .arg("--ignored")
            .arg("--exact")
            .arg(std::thread::current().name().unwrap())
            .arg("--nocapture")
            .env("PATH", dir.path())
            .env("EMUWIZ_FLYCAST_MOCK_ARTIFACT", &exe)
            .env("EMUWIZ_FLYCAST_REAL_ARTIFACT_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{variable}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
#[test]
fn resolver_vocabulary_does_not_claim_update_or_version_probe_support() {
    use crate::emulator_inventory::{InstallationType, probe_version_output};
    assert!(
        crate::emulator_update::official_metadata_url(
            InventoryEmulator::Flycast,
            InstallationType::Manual
        )
        .is_none()
    );
    assert!(
        probe_version_output(
            Path::new("/must-never-be-executed/flycast"),
            InventoryEmulator::Flycast
        )
        .is_none()
    );
}
#[cfg(target_os = "linux")]
#[test]
fn other_flycast_process_evidence_blocks_same_profile_writes() {
    machine(|m| {
        use std::os::unix::fs::symlink;
        let executable = PathBuf::from(std::env::var_os("EMUWIZ_FLYCAST_MOCK_ARTIFACT").unwrap());
        let proc_root = m._dir.path().join("proc");
        fs::create_dir_all(proc_root.join("1234")).unwrap();
        let other = m._dir.path().join("flycast-other");
        fs::write(&other, b"other binary").unwrap();
        symlink(&other, proc_root.join("1234/exe")).unwrap();
        assert!(
            ensure_stopped_at(&executable, &proc_root)
                .unwrap_err()
                .contains("Close all Flycast")
        );
        assert!(!m.destination().exists());
    });
}

#[test]
fn missing_native_description_is_not_renamed_by_merge_reindexing() {
    let input = String::from_utf8(native(256, 3, 9))
        .unwrap()
        .replace("cheat0_desc = Synthetic health\n", "");
    assert!(validate_native_file(input.as_bytes()).is_err());
}
