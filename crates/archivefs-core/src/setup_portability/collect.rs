use std::path::{Path, PathBuf};

use crate::dat::sources::config::DatSourcesConfig;
use crate::emulator_inventory::InventoryEmulator;
use crate::identity_source::settings::ProviderSettings;

use super::io::read_optional;
use super::*;

// These are the existing Emulator Setup override files, not emulator configs.
// A new override channel must opt in here rather than exporting arbitrary files.
const OVERRIDES: &[(InventoryEmulator, Option<&str>, Option<&str>)] = &[
    (
        InventoryEmulator::Mame,
        Some("mame_executable_override.txt"),
        None,
    ),
    (
        InventoryEmulator::Pcsx2,
        Some("pcsx2_executable_override.txt"),
        None,
    ),
    (
        InventoryEmulator::Rpcs3,
        Some("rpcs3_executable_override.txt"),
        Some("rpcs3_configuration_folder_override.txt"),
    ),
    (
        InventoryEmulator::Ppsspp,
        Some("ppsspp_executable_override.txt"),
        Some("ppsspp_configuration_folder_override.txt"),
    ),
    (
        InventoryEmulator::DuckStation,
        Some("duckstation_executable_override.txt"),
        Some("duckstation_configuration_folder_override.txt"),
    ),
    (
        InventoryEmulator::Xemu,
        Some("xemu_executable_override.txt"),
        Some("xemu_configuration_folder_override.txt"),
    ),
    (
        InventoryEmulator::Dolphin,
        None,
        Some("dolphin_configuration_folder_override.txt"),
    ),
];

pub fn collect_setup_default() -> Result<SetupManifest, String> {
    let config_root =
        crate::app_dirs::config_dir().map_err(|_| "Could not locate EmuWiz settings.")?;
    let identity_root = crate::identity_source::settings::default_identity_root()?;
    collect_setup(&config_root, &identity_root)
}

/// Reads only known configuration files below the supplied roots. The caller
/// may inject roots for tests/portable installations without changing process
/// environment. Referenced roots, token files, ROMs and emulator executables
/// are not opened or executed by collection.
pub fn collect_setup(config_root: &Path, identity_root: &Path) -> Result<SetupManifest, String> {
    let mut manifest = SetupManifest::default();
    if let Some(text) = read_text(
        &config_root.join("config.toml"),
        SetupArea::Library,
        &mut manifest,
    ) {
        match (
            crate::parse_config(&text),
            crate::parse_source_folder_configs(&text),
        ) {
            (Ok(config), Ok(sources)) => {
                manifest.library = SetupLibrary {
                    sources: sources
                        .into_iter()
                        .map(|s| SetupSource {
                            path: s.path,
                            enabled: s.enabled,
                        })
                        .collect(),
                    mount_root: Some(config.mount_root),
                    master_rom_root: config.master_rom_root,
                    ratarmount_bin: Some(config.ratarmount_bin),
                };
                notice(
                    &mut manifest,
                    SetupArea::Library,
                    SetupCoverage::Included,
                    "Library folder settings are included; game files are not.",
                );
            }
            _ => notice(
                &mut manifest,
                SetupArea::Library,
                SetupCoverage::RequiresAttention,
                "Library settings could not be parsed and were omitted. Check them in Setup.",
            ),
        }
    } else {
        notice(
            &mut manifest,
            SetupArea::Library,
            SetupCoverage::NotIncluded,
            "Library folder settings were not available for export.",
        );
    }
    if let Some(text) = read_text(
        &config_root.join("dat_sources.toml"),
        SetupArea::Dat,
        &mut manifest,
    ) {
        match toml::from_str::<DatSourcesConfig>(&text) {
            Ok(mut config) => {
                let mut unknown = !config.unknown_fields.is_empty();
                if let Some(policy) = &mut config.policy {
                    unknown |= !policy.unknown_fields.is_empty();
                    policy.unknown_fields.clear();
                    if let Some(platforms) = &mut policy.platforms {
                        for platform in platforms.values_mut() {
                            unknown |= !platform.unknown_fields.is_empty();
                            platform.unknown_fields.clear();
                        }
                    }
                }
                manifest.dat_policy = config.policy;
                for source in config.sources.unwrap_or_default() {
                    unknown |= !source.unknown_fields.is_empty();
                    manifest.dat_sources.push(SetupDatSource {
                        id: source.id,
                        display_name: source.display_name,
                        path: PathBuf::from(source.path),
                        kind: source.kind,
                        ownership: source.ownership,
                        enabled: source.enabled,
                        priority: source.priority,
                        platform: source.platform,
                    });
                }
                notice(
                    &mut manifest,
                    SetupArea::Dat,
                    SetupCoverage::Included,
                    "DAT registrations and matching preferences are included; catalogues and saved validation results are not.",
                );
                if unknown {
                    notice(
                        &mut manifest,
                        SetupArea::Dat,
                        SetupCoverage::RequiresAttention,
                        "Unknown DAT settings were omitted; review them separately on the destination.",
                    );
                }
            }
            Err(_) => notice(
                &mut manifest,
                SetupArea::Dat,
                SetupCoverage::RequiresAttention,
                "DAT settings could not be parsed and were omitted.",
            ),
        }
    }
    for &(emulator, executable_file, configuration_file) in OVERRIDES {
        let mut load = |file: Option<&str>| {
            file.and_then(|file| {
                read_text(&config_root.join(file), SetupArea::Emulators, &mut manifest)
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| PathBuf::from(text.trim()))
            })
        };
        let executable = load(executable_file);
        let configuration_folder = load(configuration_file);
        if executable.is_some() || configuration_folder.is_some() {
            manifest.emulators.push(SetupEmulator {
                emulator,
                executable,
                configuration_folder,
            });
        }
    }
    manifest.retroarch_core_directory = read_text(
        &config_root.join("retroarch_core_directory_override.txt"),
        SetupArea::Emulators,
        &mut manifest,
    )
    .filter(|text| !text.trim().is_empty())
    .map(|text| PathBuf::from(text.trim()));
    notice(
        &mut manifest,
        SetupArea::Emulators,
        SetupCoverage::RequiresAttention,
        "Supported manual emulator locations and the RetroArch core-folder override are included. Automatically discovered installations, emulator versions and remembered per-game profiles must be checked again in Setup.",
    );

    if let Some(text) = read_text(
        &identity_root.join("romm/config.json"),
        SetupArea::Providers,
        &mut manifest,
    ) {
        match serde_json::from_str::<ProviderSettings>(&text) {
            Ok(settings) => {
                let server_origin = url::Url::parse(&settings.source.url)
                    .ok()
                    .filter(|url| {
                        matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
                    })
                    .map(|url| url.origin().ascii_serialization());
                manifest.romm = Some(SetupRomm {
                    enabled: settings.source.enabled,
                    server_origin,
                    mappings: settings.source.mappings,
                    media_mapping: settings.source.media_mapping,
                    provider_path_kind: settings.source.provider_path_kind,
                    page_size: settings.page_size,
                    import_timeout_seconds: settings.import_timeout_seconds,
                });
                notice(
                    &mut manifest,
                    SetupArea::Providers,
                    SetupCoverage::RequiresAttention,
                    "RomM enablement, server origin, folder mappings and paging preferences are included. Re-enter the server base path and credentials, review mappings, and test the connection explicitly. Other provider settings are not included.",
                );
            }
            Err(_) => notice(
                &mut manifest,
                SetupArea::Providers,
                SetupCoverage::RequiresAttention,
                "RomM settings could not be parsed and were omitted. Credentials were not read.",
            ),
        }
    } else {
        notice(
            &mut manifest,
            SetupArea::Providers,
            SetupCoverage::NotIncluded,
            "No RomM settings were available. Other provider settings and all credentials are not included.",
        );
    }
    for (area, message) in [
        (
            SetupArea::Artwork,
            "Artwork choices and cached pictures are not included; review Sources & Providers on the destination.",
        ),
        (
            SetupArea::Controllers,
            "Controller mappings are not included; configure controllers in each emulator.",
        ),
        (
            SetupArea::Launch,
            "Launch and per-game profile choices are not included; review them on the destination.",
        ),
        (
            SetupArea::Conversion,
            "Conversion preferences and jobs are not included.",
        ),
        (
            SetupArea::CheatsMods,
            "Cheat/mod choices, installed content and review history are not included.",
        ),
        (
            SetupArea::SavesConfigs,
            "Saves and emulator configuration contents are not included. Use the existing selected-game save snapshot workflow separately.",
        ),
        (
            SetupArea::OtherSettings,
            "Other settings, library database, caches, ROMs and recovery journals are not included. This is a setup summary, not a full backup.",
        ),
    ] {
        notice(&mut manifest, area, SetupCoverage::NotIncluded, message);
    }
    manifest.validate()?;
    Ok(manifest)
}

fn notice(manifest: &mut SetupManifest, area: SetupArea, coverage: SetupCoverage, message: &str) {
    manifest.notices.push(SetupNotice {
        area,
        coverage,
        message: message.into(),
    });
}

fn read_text(path: &Path, area: SetupArea, manifest: &mut SetupManifest) -> Option<String> {
    match read_optional(path).and_then(|bytes| {
        bytes
            .map(|bytes| {
                String::from_utf8(bytes).map_err(|_| "Settings are not UTF-8.".to_string())
            })
            .transpose()
    }) {
        Ok(text) => text,
        Err(_) => {
            // Deliberately name only the known file; neither parser errors nor
            // file contents are echoed into an export or the normal GUI.
            notice(
                manifest,
                area,
                SetupCoverage::RequiresAttention,
                &format!(
                    "{} could not be read safely and was omitted.",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
            );
            None
        }
    }
}
