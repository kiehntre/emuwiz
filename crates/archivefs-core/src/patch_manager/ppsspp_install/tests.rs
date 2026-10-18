use super::*;
use std::os::unix::fs::PermissionsExt;
const ID: &str = "ULUS10000";
const CODES: &[u8] = b"_S ULUS-10000\n_G Synthetic\n_C1 Test\n_L 0x20000000 0x00000001\n";
struct Fixture {
    _temp: tempfile::TempDir,
    env: Environment,
    game: PathBuf,
    source: PathBuf,
    history: PathBuf,
    backups: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let exe = root.join("PPSSPP.AppImage");
        fs::write(&exe, b"synthetic pinned PPSSPP").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        let config = root.join("config/ppsspp/PSP/SYSTEM");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("ppsspp.ini"),
            b"[General]\nEnableCheats = False\n# user settings\n",
        )
        .unwrap();
        let roots = PpssppProfileDiscoveryRoots {
            home: root.join("home"),
            xdg_config_home: root.join("config"),
            xdg_data_home: root.join("data"),
            explicit_configuration_roots: vec![],
            portable_configuration_roots: vec![],
            explicit_executables: vec![exe.clone()],
            known_version_outputs: Default::default(),
            appimage_directory: None,
            path_override: Some(vec![]),
        };
        let id = discover_ppsspp_profiles(&roots)
            .profiles
            .into_iter()
            .find(|p| p.configuration_path == root.join("config/ppsspp"))
            .unwrap()
            .profile_id;
        let proc_root = root.join("proc");
        fs::create_dir(&proc_root).unwrap();
        let game = root.join("game.iso");
        fs::write(&game, psp_iso_bytes(ID.as_bytes())).unwrap();
        let source = root.join("source.ini");
        fs::write(&source, CODES).unwrap();
        let env = Environment {
            roots: roots.clone(),
            executable: exe,
            profile_id: id,
            proc_root,
        };
        let history = root.join("history");
        let backups = root.join("backups");
        Self {
            _temp: temp,
            env,
            game,
            source,
            history,
            backups,
        }
    }
    fn preview(&self) -> Preview {
        preview(&self.env, &self.game, ID, &self.source).unwrap()
    }
    fn apply(&self, p: &Preview, approved: bool) -> SharedApplyResult {
        apply(
            &self.env,
            p,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: p.native.transaction.plan_id.clone(),
                    general_approved: approved,
                    replacement_approved: false,
                }),
                operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: 1,
                current_context: p.native.transaction.context.clone(),
                history_root: self.history.clone(),
                backup_root: self.backups.clone(),
            },
        )
        .unwrap()
    }
}
#[test]
fn exact_preview_confirm_install_history_guarded_undo_preserves_user_data() {
    let f = Fixture::new();
    let before = fs::read(
        f.env
            .roots
            .xdg_config_home
            .join("ppsspp/PSP/SYSTEM/ppsspp.ini"),
    )
    .unwrap();
    let game = fs::read(&f.game).unwrap();
    let p = f.preview();
    let target = p.binding.cheats_root.join(format!("{ID}.ini"));
    assert!(!target.exists());
    assert_ne!(
        f.apply(&p, false).journal.status,
        SharedApplyStatus::Success
    );
    assert!(!target.exists());
    let p = f.preview();
    let result = f.apply(&p, true);
    assert_eq!(result.journal.status, SharedApplyStatus::Success);
    assert!(result.journal_failure.is_none());
    let installed = fs::read_to_string(&target).unwrap();
    assert!(installed.contains("_C0 Test"));
    assert!(!installed.contains("_C1"));
    assert!(result.journal.context.source_mode.contains("source.ini"));
    let h = history(&f.env, &f.game, ID, &f.history).unwrap();
    assert!(
        h.iter()
            .any(|(_, j)| j.operation_id == result.journal.operation_id)
    );
    let journal = result.journal_path.unwrap();
    let (u, b) = undo_preview(&f.env, &f.game, ID, &journal, &f.history, &f.backups).unwrap();
    fs::write(&target, b"user changes").unwrap();
    let r = undo(
        &f.env,
        &u,
        &b,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: u.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 2,
            history_root: f.history.clone(),
            backup_root: f.backups.clone(),
        },
    )
    .unwrap();
    assert_ne!(r.status, SharedApplyStatus::Success);
    assert_eq!(fs::read(&target).unwrap(), b"user changes");
    // Restore only this disposable fixture's exact expected bytes for a fresh Undo review.
    fs::write(&target, installed.as_bytes()).unwrap();
    let (u, b) = undo_preview(&f.env, &f.game, ID, &journal, &f.history, &f.backups).unwrap();
    let r = undo(
        &f.env,
        &u,
        &b,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: u.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 3,
            history_root: f.history.clone(),
            backup_root: f.backups.clone(),
        },
    )
    .unwrap();
    assert_eq!(r.status, SharedApplyStatus::Success);
    assert!(!target.exists());
    assert_eq!(
        fs::read(
            f.env
                .roots
                .xdg_config_home
                .join("ppsspp/PSP/SYSTEM/ppsspp.ini")
        )
        .unwrap(),
        before
    );
    assert_eq!(fs::read(&f.source).unwrap(), CODES);
    assert_eq!(fs::read(&f.game).unwrap(), game);
}
#[test]
fn existing_files_stale_sources_settings_and_games_are_refused() {
    let f = Fixture::new();
    let p = f.preview();
    fs::write(&f.source, b"changed").unwrap();
    assert_ne!(f.apply(&p, true).journal.status, SharedApplyStatus::Success);
    let f = Fixture::new();
    let p = f.preview();
    fs::write(
        f.env
            .roots
            .xdg_config_home
            .join("ppsspp/PSP/SYSTEM/ppsspp.ini"),
        b"[General]\nEnableCheats=True\n",
    )
    .unwrap();
    assert!(
        apply(
            &f.env,
            &p,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: None,
                operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: 0,
                current_context: p.native.transaction.context.clone(),
                history_root: f.history.clone(),
                backup_root: f.backups.clone()
            }
        )
        .is_err()
    );
    let f = Fixture::new();
    let p = f.preview();
    fs::create_dir_all(&p.binding.cheats_root).unwrap();
    let target = p.binding.cheats_root.join(format!("{ID}.ini"));
    fs::write(&target, b"user cheat").unwrap();
    assert!(preview(&f.env, &f.game, ID, &f.source).is_err());
    assert_ne!(f.apply(&p, true).journal.status, SharedApplyStatus::Success);
    assert_eq!(fs::read(target).unwrap(), b"user cheat");
    let f = Fixture::new();
    let p = f.preview();
    fs::write(&f.game, psp_iso_bytes(b"ULES00001")).unwrap();
    assert!(fresh_game(&f.game, &p.id).is_err());
}
#[test]
fn unknown_builds_overrides_profiles_processes_and_sources_fail_closed() {
    let f = Fixture::new();
    fs::write(&f.env.executable, b"unknown").unwrap();
    assert!(f.env.resolve().is_err());
    for suffix in [".env", ".home", ".config", ".share"] {
        let f = Fixture::new();
        let mut p = f.env.executable.as_os_str().to_os_string();
        p.push(suffix);
        fs::write(PathBuf::from(p), b"override").unwrap();
        assert!(f.env.resolve().is_err());
    }
    let mut f = Fixture::new();
    f.env.profile_id = "unknown".into();
    assert!(f.env.resolve().is_err());
    let f = Fixture::new();
    fs::create_dir(f.env.proc_root.join("123")).unwrap();
    fs::write(f.env.proc_root.join("123/comm"), b"PPSSPPSDL").unwrap();
    assert!(f.env.resolve().is_err());
    for s in [
        "_S ULES00001\n_C0 A\n_L 0x20000000 0x00000000\n",
        "_S ULUS10000\n_C0 A\n_L 0xD0000000 0x00000000\n",
        "_S ULUS10000\n_C0 A\n_M 0x20000000 0x00000000\n",
        "_S ULUS10000\n_S ULES00001\n",
    ] {
        assert!(disabled_file(s.as_bytes(), ID).is_err());
    }
    let f = Fixture::new();
    let mut id = crate::game_identity::inspect_game_identity(&f.game, Some("PSP"));
    let mut conflict = id
        .evidence
        .iter()
        .find(|e| e.kind == IdentityKind::PspDiscId)
        .unwrap()
        .clone();
    conflict.value = Some("ULES00001".into());
    id.evidence.push(conflict);
    assert!(disc_id(&id, &f.game).is_err());
    id.complete = false;
    assert!(disc_id(&id, &f.game).is_err());
}
const ISO_SECTOR_SIZE: usize = 2048;
fn directory_record(name: &[u8], extent: u32, size: u32, directory: bool) -> Vec<u8> {
    let length = 33 + name.len() + usize::from(name.len().is_multiple_of(2));
    let mut record = vec![0_u8; length];
    record[0] = length as u8;
    record[2..6].copy_from_slice(&extent.to_le_bytes());
    record[6..10].copy_from_slice(&extent.to_be_bytes());
    record[10..14].copy_from_slice(&size.to_le_bytes());
    record[14..18].copy_from_slice(&size.to_be_bytes());
    record[25] = if directory { 2 } else { 0 };
    record[28..30].copy_from_slice(&1_u16.to_le_bytes());
    record[30..32].copy_from_slice(&1_u16.to_be_bytes());
    record[32] = name.len() as u8;
    record[33..33 + name.len()].copy_from_slice(name);
    record
}

fn psp_sfo(disc_id: &[u8]) -> Vec<u8> {
    let mut value = disc_id.to_vec();
    value.push(0);
    let key = b"DISC_ID\0";
    let key_start = 20 + 16;
    let data_start = key_start + 8;
    let mut out = vec![0_u8; data_start];
    out[0..4].copy_from_slice(crate::param_sfo::SFO_MAGIC);
    out[4..8].copy_from_slice(&0x0101_u32.to_le_bytes());
    out[8..12].copy_from_slice(&(key_start as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(data_start as u32).to_le_bytes());
    out[16..20].copy_from_slice(&1_u32.to_le_bytes());
    out[20..22].copy_from_slice(&0_u16.to_le_bytes());
    out[22..24].copy_from_slice(&0x0204_u16.to_le_bytes());
    out[24..28].copy_from_slice(&(value.len() as u32).to_le_bytes());
    out[28..32].copy_from_slice(&(value.len() as u32).to_le_bytes());
    out[32..36].copy_from_slice(&0_u32.to_le_bytes());
    out[key_start..key_start + key.len()].copy_from_slice(key);
    out.extend_from_slice(&value);
    out
}

fn psp_iso_bytes(disc_id: &[u8]) -> Vec<u8> {
    let sfo = psp_sfo(disc_id);
    let mut iso = vec![0_u8; 28 * ISO_SECTOR_SIZE];
    let pvd = 16 * ISO_SECTOR_SIZE;
    iso[pvd] = 1;
    iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
    iso[pvd + 6] = 1;
    let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
    iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
    let terminator = 17 * ISO_SECTOR_SIZE;
    iso[terminator] = 255;
    iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
    iso[terminator + 6] = 1;
    let root_offset = 20 * ISO_SECTOR_SIZE;
    let dir = directory_record(b"PSP_GAME", 21, ISO_SECTOR_SIZE as u32, true);
    iso[root_offset..root_offset + dir.len()].copy_from_slice(&dir);
    let umd = directory_record(b"UMD_DATA.BIN;1", 23, 3, false);
    let umd_offset = root_offset + dir.len();
    iso[umd_offset..umd_offset + umd.len()].copy_from_slice(&umd);
    let psp_offset = 21 * ISO_SECTOR_SIZE;
    let sfo_record = directory_record(b"PARAM.SFO;1", 22, sfo.len() as u32, false);
    iso[psp_offset..psp_offset + sfo_record.len()].copy_from_slice(&sfo_record);
    iso[22 * ISO_SECTOR_SIZE..22 * ISO_SECTOR_SIZE + sfo.len()].copy_from_slice(&sfo);
    iso[23 * ISO_SECTOR_SIZE..23 * ISO_SECTOR_SIZE + 3].copy_from_slice(b"UMD");
    iso
}
