//! Manual, explicitly approved disposable runtime acceptance harness.
//! No downloaded material and no production profiles. See the PCSX2 report.
#[cfg(target_os = "linux")]
mod supported {
    use archivefs_core::{
        dat::{
            firmware_evidence::{FirmwareIdentityRecord, FirmwareSystem},
            model::DatEcosystem,
        },
        game_identity::inspect_catalogued_game_identity,
        identity_source::hashing::Crc32,
        launch::{
            Pcsx2LaunchRequest,
            cheat_runtime::{
                CheatRuntimeRoots, CleanupOutcome, WorkspaceDisposition, cleanup_stale_workspace,
                scan_cheat_runtime_workspaces,
            },
            cheat_runtime_pcsx2::{launch_pcsx2_cheats, preview_pcsx2_cheats},
        },
        patch_manager::{
            Pcsx2GameIdentity, Pcsx2InstallationType, Pcsx2ProfileDiscoveryRoots,
            discover_pcsx2_profiles, resolve_pcsx2_native_launch_binding,
        },
    };
    use std::{
        path::PathBuf,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    pub fn run() -> Result<(), String> {
        let args: Vec<_> = std::env::args().collect();
        if args.len() != 5
            || args[1] != "--fixture-root"
            || args[4] != "--approve-disposable-activation"
            || !matches!(args[3].as_str(), "activate" | "interrupt" | "recover")
        {
            return Err("Usage: pcsx2_temp_cheat_journey --fixture-root /tmp/emuwiz-pcsx2-journey-NAME activate|interrupt|recover --approve-disposable-activation".into());
        }
        let root = PathBuf::from(&args[2]);
        if root.parent() != Some(std::path::Path::new("/tmp"))
            || !root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with("emuwiz-pcsx2-journey-")
            || std::fs::canonicalize(&root).map_err(|e| e.to_string())? != root
        {
            return Err(
                "Harness requires an existing private /tmp/emuwiz-pcsx2-journey-* fixture".into(),
            );
        }
        for (key, leaf) in [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("TMPDIR", "tmp"),
        ] {
            if std::env::var_os(key).as_deref() != Some(root.join(leaf).as_os_str()) {
                return Err(format!("{key} must point inside this fixture"));
            }
        }
        let runtime = CheatRuntimeRoots::default();
        if args[3] == "recover" {
            // Advance only this fixture's observation clock past the normal
            // grace period. Production UI uses the real clock and normal grace.
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 180;
            let rows = scan_cheat_runtime_workspaces(&runtime, now, 120);
            for row in rows {
                println!("RECOVERY {:?}: {}", row.disposition, row.path.display());
                if row.disposition == WorkspaceDisposition::OwnedStale {
                    cleanup_stale_workspace(&runtime, &row.path, now, 120)
                        .map_err(|e| format!("{e:?}"))?;
                    println!("RECOVERED {}", row.path.display());
                }
            }
            return Ok(());
        }
        let roots = Pcsx2ProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?;
        let profiles = discover_pcsx2_profiles(&roots).map_err(|e| format!("{e:?}"))?;
        let profile = profiles
            .profiles
            .iter()
            .find(|p| p.installation_type == Pcsx2InstallationType::FlatpakUser)
            .ok_or("Fixture Flatpak profile not discovered")?;
        if !profile.configuration_path.starts_with(&root) {
            return Err("Discovered profile escaped fixture".into());
        }
        let binding = resolve_pcsx2_native_launch_binding(profile, &roots).map_err(|e| e.detail)?;
        let game = root.join("game.iso");
        let report = inspect_catalogued_game_identity(&game, Some("PS2"));
        let identity = Pcsx2GameIdentity::from_report("Original homebrew fixture", &report);
        let serial = identity
            .serial
            .clone()
            .ok_or("No verified fixture serial")?;
        let crc = identity
            .verified_crc()
            .ok_or("No verified fixture executable CRC")?;
        let request = Pcsx2LaunchRequest {
            selected_content_path: game,
            expected_platform_id: "PS2".into(),
            expected_game_key: serial.clone(),
            expected_ps2_serial: serial.clone(),
            profile_id: profile.profile_id.clone(),
            expected_executable: binding.executable,
            expected_user_directory_mode: binding.user_directory_mode,
            expected_installation: binding.installation,
        };
        let bios = std::fs::read(profile.configuration_path.join("bios/probe-bios.bin"))
            .map_err(|e| e.to_string())?;
        use md5::Md5;
        use sha1::{Sha1, digest::Digest};
        let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() };
        let firmware = vec![FirmwareIdentityRecord {
            system: FirmwareSystem::PlayStation2,
            provider: DatEcosystem::Redump,
            name: "Authorized local QA BIOS copy".into(),
            description: None,
            size_bytes: bios.len() as u64,
            crc32: Crc32::of(&bios),
            md5: hex(&Md5::digest(&bios)),
            sha1: hex(&Sha1::digest(&bios)),
            dat_version: Some("disposable-runtime-acceptance".into()),
        }];
        let source = root.join("source").join(format!("{serial}_{crc}.pnach"));
        let id = format!(
            "pcsx2-journey-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let preview = preview_pcsx2_cheats(&request, &roots, &firmware, &source, &id, &runtime)?;
        println!(
            "PREVIEW {} profile={} source_sha256={} patches={}",
            preview.identity(),
            preview.profile_path().display(),
            preview.source_hash(),
            preview.patch_count()
        );
        let mut launch = launch_pcsx2_cheats(preview, &roots, &firmware, &runtime)?;
        let workspace = launch
            .session
            .receipt()
            .workspace_root
            .as_ref()
            .ok_or("No materialized workspace")?
            .clone();
        println!("WORKSPACE {}", workspace.display());
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut observed = false;
        loop {
            launch.poll();
            let observation = launch.observation();
            if let Some(detail) = &observation.detail {
                return Err(detail.clone());
            }
            if observation.file_loaded && observation.selection_enabled && !observed {
                observed = true;
                println!("OBSERVED {}", observation.label());
                if args[3] == "interrupt" {
                    println!("INTERRUPTING OWNER; workspace retained for canonical recovery");
                    std::process::exit(0);
                }
            }
            if launch.session.is_finished() {
                println!(
                    "RECEIPT {}",
                    serde_json::to_string(launch.session.receipt()).map_err(|e| e.to_string())?
                );
                if !observed {
                    return Err("PCSX2 load/enablement was not observed".into());
                }
                if launch.session.receipt().cleanup != CleanupOutcome::Completed
                    || launch.session.receipt().has_violation()
                {
                    return Err("Cleanup or preservation verification failed".into());
                }
                assert!(!workspace.exists());
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("Fixture observation deadline exceeded; no unowned process was stopped and workspace remains recoverable".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), String> {
    supported::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This disposable acceptance harness supports Linux only.");
}
