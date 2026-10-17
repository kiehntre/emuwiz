//! Worker-only adapter inspection. Uses existing verifiers and generation
//! fingerprints; no writes, network, process calls or profile selection.
use super::*;
use archivefs_core::game_firmware_adapters as adapters;
use std::fs;
use std::os::unix::fs::MetadataExt;
const MAX_BIOS_ENTRIES: usize = 64;
const MAX_BIOS_BYTES: u64 = 128 * 1024 * 1024;
fn pcsx2_bound(profile: &Pcsx2Profile) -> std::result::Result<(), String> {
    let root = profile.configuration_path.join("bios");
    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Cannot inspect selected BIOS directory: {e}")),
    };
    let mut bytes = 0u64;
    for (n, e) in entries.enumerate() {
        if n >= MAX_BIOS_ENTRIES {
            return Err("Selected BIOS directory exceeds the 64-entry guidance bound. Existing Launch behaviour is unchanged.".into());
        }
        let e = e.map_err(|e| e.to_string())?;
        let m = fs::symlink_metadata(e.path()).map_err(|e| e.to_string())?;
        if m.is_file() {
            bytes = bytes.checked_add(m.len()).ok_or("BIOS size overflow")?;
        }
        if bytes > MAX_BIOS_BYTES {
            return Err("Selected BIOS inventory exceeds the 128 MiB guidance read bound.".into());
        }
    }
    Ok(())
}
fn sample(r: &Request) -> std::result::Result<(GameFirmwareReadiness, Vec<PathBuf>, bool), String> {
    let mut paths = vec![r.game_path.clone()];
    let (result, readable) = match &r.source {
        Source::Pcsx2(profile, evidence) => {
            pcsx2_bound(profile)?;
            let inspected = inspect_pcsx2_game_with_firmware_evidence(
                profile,
                &Pcsx2GameRequest {
                    verified_ps2_serial: None,
                    verified_executable_crc: None,
                    emulator_serial: None,
                },
                evidence,
            );
            paths.extend([
                profile.configuration_path.clone(),
                profile.configuration_path.join("bios"),
                inspected.inspection.global_config.path.clone(),
            ]);
            let root = profile.configuration_path.join("bios");
            if let Ok(entries) = fs::read_dir(root) {
                for e in entries.take(MAX_BIOS_ENTRIES) {
                    paths.push(e.map_err(|e| e.to_string())?.path());
                }
            }
            (
                adapters::pcsx2(
                    &r.context,
                    &r.candidate,
                    profile,
                    &inspected.bios_verification,
                ),
                inspected.inspection.global_config.readable,
            )
        }
        Source::DuckStation(profile, evidence, serial) => {
            let inspected = inspect_duckstation_game_with_firmware_evidence(
                profile,
                &DuckStationGameRequest {
                    verified_ps1_serial: serial.clone(),
                    emulator_serial: None,
                    disc_contexts: vec![],
                    playlist_path: None,
                },
                evidence,
            );
            paths.extend([
                profile.configuration_path.clone(),
                profile.bios_path.clone(),
                profile.game_settings_path.clone(),
                inspected.inspection.global_config.path.clone(),
            ]);
            if let Some(config) = &inspected.inspection.per_game_config {
                paths.push(config.path.clone());
            }
            (
                adapters::duckstation(
                    &r.context,
                    &r.candidate,
                    profile,
                    &inspected.bios_verification,
                ),
                inspected.inspection.global_config.readable
                    && inspected
                        .inspection
                        .per_game_config
                        .as_ref()
                        .is_none_or(|c| !c.exists || c.readable),
            )
        }
        Source::Flycast(profile) => {
            let inspected = inspect_flycast_game(
                profile,
                &FlycastGameRequest {
                    canonical_platform: Some(r.context.platform.clone()),
                    flycast_platform: Some(FlycastPlatform::Dreamcast),
                    ..Default::default()
                },
            );
            paths.extend([
                profile.configuration_path.clone(),
                profile.config_path.clone(),
                profile.system_path.clone(),
                profile.system_path.join("dc_boot.bin"),
                profile.system_path.join("dc_flash.bin"),
            ]);
            (
                adapters::flycast(&r.context, &r.candidate, profile, &inspected.health.system),
                inspected.health.config_readable,
            )
        }
        Source::RetroArch(info, system) => {
            use archivefs_core::emulator_environment::retroarch::CoreInfoFinding;
            let mut evidence = vec![explanation(
                "Selected core metadata is a discovered snapshot, not a verified firmware compatibility policy. Standalone firmware evidence is not transferable; current core policy/title sufficiency remains Unknown.",
            )];
            if let CoreInfoFinding::Found { firmware, .. } = info {
                if firmware.is_empty() {
                    evidence.push(explanation("This discovered core metadata snapshot declares no firmware entries. Absence of declarations is not invented compatibility evidence."));
                }
                for f in firmware.iter().take(64) {
                    let mut text = format!(
                        "Core firmware declaration{}: {}; optional={}. ",
                        f.index,
                        f.path.as_deref().unwrap_or("path unspecified"),
                        f.optional
                    );
                    let mut observed = None;
                    if let (Some(root), Some(name)) = (system, &f.path) {
                        let relative = Path::new(name);
                        if relative
                            .components()
                            .all(|c| matches!(c, std::path::Component::Normal(_)))
                            && !relative.as_os_str().is_empty()
                        {
                            let mut path = root.clone();
                            let mut safe = fs::symlink_metadata(root)
                                .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
                            for part in relative.components() {
                                path.push(part.as_os_str());
                                if fs::symlink_metadata(&path)
                                    .is_ok_and(|m| m.file_type().is_symlink())
                                {
                                    safe = false;
                                }
                            }
                            if safe {
                                text.push_str(match fs::symlink_metadata(&path){Ok(m) if m.is_file()=>"A regular file was observed at the recorded system path; its identity and compatibility are unverified.",Err(e) if e.kind()==std::io::ErrorKind::NotFound=>"No file was observed at the recorded system path. Current effective configuration remains unverified.",_=>"The recorded path is unreadable or not a regular file; suitability is Unknown."});
                                observed = Some(path.clone());
                                paths.push(path);
                            } else {
                                text.push_str("The recorded system path is unsafe or unavailable; no file attribution was made.");
                            }
                        } else {
                            text.push_str("Unresolved or unsafe core-relative path; no inspection was attempted.");
                        }
                    } else {
                        text.push_str(
                            "No resolved system directory/path is bound to this profile.",
                        );
                    }
                    let mut e = explanation(&text);
                    e.path = observed;
                    evidence.push(e);
                }
            } else {
                evidence.push(explanation(
                    "The selected core's firmware declarations are unavailable.",
                ));
            }
            if let Some(root) = system {
                paths.push(root.clone());
            }
            let mut c = r.candidate.clone();
            c.firmware = archivefs_core::launch::FirmwareReadiness::Unknown;
            let mut result = GameFirmwareReadiness::project(GameFirmwareReadinessInput {
                game_id: r.context.game_id.clone(),
                platform: r.context.platform.clone(),
                release_identity: r.context.release_identity.clone(),
                game_region: None,
                game_revision: None,
                candidate: &c,
                requirement: FirmwareRequirementDescriptor {
                    id: "selected-core-firmware".into(),
                    label: format!(
                        "Selected core declarations. {}",
                        required_label(&r.context.platform)
                    ),
                    required: true,
                    options: vec![],
                    emulator_managed: false,
                    region_requirement_proven: false,
                    version_requirement_proven: false,
                },
                evidence,
                selected_option_id: None,
                freshness: FirmwareFreshness::current(Default::default()),
            });
            result.state = GameFirmwareState::Unknown;
            (result, true)
        }
        Source::Unavailable(reason) => return Err(reason.clone()),
    };
    paths.extend(result.evidence.iter().filter_map(|e| e.path.clone()));
    // Parent generations also bind a selected file that was absent before
    // inspection, or replaced through a renamed containing directory.
    let parents: Vec<_> = paths
        .iter()
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    paths.extend(parents);
    paths.sort();
    paths.dedup();
    Ok((result, paths, readable))
}
fn stamps(paths: &[PathBuf]) -> std::result::Result<Vec<(PathBuf, String)>, String> {
    paths
        .iter()
        .map(|p| {
            let stamp = match fs::symlink_metadata(p) {
                Ok(m) => format!(
                    "{:?}",
                    (
                        m.dev(),
                        m.ino(),
                        m.mode(),
                        m.len(),
                        m.mtime(),
                        m.mtime_nsec(),
                        m.ctime(),
                        m.ctime_nsec()
                    )
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing".into(),
                Err(e) => return Err(format!("Cannot revalidate {}: {e}", p.display())),
            };
            Ok((p.clone(), stamp))
        })
        .collect()
}
pub(super) fn inspect(mut request: Request) -> std::result::Result<GameFirmwareReadiness, String> {
    // First use the existing inspector to identify its actual input paths.
    // Only the second inspection supplies the displayed result, bracketed by
    // generations of every known input (including BIOS candidate directories).
    let (_, paths, _) = sample(&request)?;
    let before = stamps(&paths)?;
    let game_current = request.game_fingerprint.as_ref().is_some_and(|f| {
        f.object.is_some() && FileFingerprint::observe(&request.game_path).as_ref() == Some(f)
    });
    request.context.freshness = FirmwareFreshnessSnapshot {
        profile_fingerprint: Some(format!("{:?}", request.candidate.target)),
        firmware_path_fingerprint: Some(format!("{paths:?}")),
        firmware_file_fingerprint: Some(format!("{before:?}")),
        game_identity_fingerprint: game_current.then(|| {
            format!(
                "{:?}:{:?}",
                request.context.release_identity, request.game_fingerprint
            )
        }),
        requirement_fingerprint: Some(format!("firmware-guidance-v1:{}", request.context.platform)),
    };
    let (mut result, current_paths, readable) = sample(&request)?;
    #[cfg(test)]
    AFTER_SAMPLE.with(|hook| {
        if let Some(f) = hook.borrow_mut().take() {
            f();
        }
    });
    let after = stamps(&paths)?;
    let game_still_current = request
        .game_fingerprint
        .as_ref()
        .is_some_and(|f| FileFingerprint::observe(&request.game_path).as_ref() == Some(f));
    if before != after || paths != current_paths || (game_current && !game_still_current) {
        result.state = GameFirmwareState::Stale;
        result.freshness.state = FirmwareFreshnessState::Stale;
        result.freshness.stale_reason = Some(FirmwareStaleReason::FirmwareFileChanged);
        result.evidence.push(explanation("Inputs changed during inspection. No current suitability conclusion is available; recheck."));
    } else if !readable {
        result.state = GameFirmwareState::Unknown;
        result.freshness.state = FirmwareFreshnessState::Unknown;
        result.evidence.push(explanation("The selected profile configuration is unreadable. Firmware presence must not be interpreted as a resolved effective configuration."));
    } else if !game_current {
        result.evidence.push(explanation("Current file-bound game identity was not established by the existing selected-evidence snapshot. Firmware observations are retained, but per-game readiness freshness is Unknown."));
    }
    Ok(result)
}
fn explanation(text: &str) -> FirmwareEvidenceSummary {
    FirmwareEvidenceSummary {
        option_id: None,
        path: None,
        filename: None,
        sha256: None,
        strength: FirmwareEvidenceStrength::FilenameOrPresenceOnly,
        status: FirmwareEvidenceStatus::Unknown,
        region: None,
        version: None,
        provenance: text.into(),
    }
}

#[cfg(test)]
thread_local! { static AFTER_SAMPLE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
pub(super) fn after_sample_hook(f: impl FnOnce() + 'static) {
    AFTER_SAMPLE.with(|hook| *hook.borrow_mut() = Some(Box::new(f)));
}
