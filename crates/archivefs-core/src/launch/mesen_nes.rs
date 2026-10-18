//! NES/FDS-specific read-only launch gates. Contract pinned to Mesen2
//! b9fa69ddc6d0a331fb103fdb5eef6904305703c2: ConfigManager.cs,
//! PreferencesConfig.cs, FirmwareTypeExtensions.cs and FirmwareHelper.h.
//! A filename locates a BIOS candidate; only exact bytes establish readiness.
use super::readiness::FirmwareReadiness;
use crate::patch_manager::{MesenProfile, MesenProfileDiscoveryRoots};
use crate::safe_read::{TrustedRoots, open_bounded_read};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MESEN_SAFE_MEDIA_SWITCHES: &[&str] = &[
    "--Nes.OverwriteOriginalRom=false",
    "--Preferences.AutoLoadPatches=false",
];
// Upstream UI/Interop/FirmwareTypeExtensions.cs, FirmwareType.FDS.
const FDS_SHA256: &[&str] = &[
    "99c18490ed9002d9c6d999b9d8d15be5c051bdfa7cc7e73318053c9a994b0178",
    "a0a9d57cbace21bf9c85c2b85e86656317f0768d7772acc90c7411ab1dbff2bf",
];
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenNesReadiness {
    pub firmware: FirmwareReadiness,
    pub firmware_sha256: Option<String>,
    pub detail: String,
}
fn read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let safe = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| e.detail())?;
    if safe.len() > limit as u64 {
        return Err("file exceeds inspection bound".into());
    }
    let mut file = safe.into_file();
    let before = file.metadata().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = file.metadata().map_err(|e| e.to_string())?;
    use super::process_spawn::CapturedFileIdentity;
    if bytes.len() > limit
        || CapturedFileIdentity::capture(&before) != CapturedFileIdentity::capture(&after)
        || std::fs::symlink_metadata(path)
            .map(|m| CapturedFileIdentity::capture(&m))
            .ok()
            != Some(CapturedFileIdentity::capture(&after))
    {
        return Err("file changed during inspection".into());
    }
    Ok(bytes)
}
/// Refuses an arbitrary explicit profile that the executable would not use.
pub fn validate_mesen_profile_binding(
    profile: &MesenProfile,
    roots: &MesenProfileDiscoveryRoots,
    executable: &Path,
) -> Result<(), String> {
    let portable = executable
        .parent()
        .ok_or("Mesen executable has no parent")?
        .join("settings.json");
    let actual = if portable.try_exists().map_err(|e| e.to_string())? {
        portable
    } else {
        roots.xdg_config_home.join("Mesen2/settings.json")
    };
    if actual != profile.config_path {
        return Err(
            "Mesen would use a different settings.json (portable settings take precedence)".into(),
        );
    }
    let bytes = read(
        &actual,
        crate::patch_manager::MESEN_MAX_CONFIG_BYTES as usize,
    )?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Mesen settings.json is malformed: {e}"))?;
    if !value.is_object() {
        return Err("Mesen settings.json must be a configuration object".into());
    }
    // Mesen's typed deserializer can reject syntactically valid settings and
    // replace them with defaults. Only this compact known schema is currently
    // certified; richer production profiles remain explicitly unsupported.
    let object = value.as_object().unwrap();
    if object.keys().any(|key| key != "Preferences")
        || value
            .get("Preferences")
            .and_then(|v| v.as_object())
            .is_none_or(|p| p.keys().any(|key| key != "SingleInstance"))
    {
        return Err("Mesen typed settings schema is not established for this profile; native launch is refused without changing settings".into());
    }
    if value
        .pointer("/Preferences/SingleInstance")
        .and_then(|v| v.as_bool())
        != Some(false)
    {
        return Err("Disable Single instance in Mesen preferences and save settings before launching from Blue; forwarding cannot bind the checked BIOS directory".into());
    }
    Ok(())
}
/// Structural evidence is reused, never inferred from the suffix alone.
pub fn validate_mesen_nes_media(path: &Path) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "fds" => {
            let evidence = crate::disk_format::inspect_disk_format(path, &TrustedRoots::none(), crate::disk_format::DiskFormatContext { folder_platform: Some("NES") }, None);
            if evidence.format == Some(crate::disk_format::DiskFormat::FamicomDiskSystem) { Ok(()) }
            else { Err(format!("FDS structure was not validated: {}", evidence.summary())) }
        }
        "nes" => {
            let mut safe = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| e.detail())?;
            let header = safe.read_exact_at(0, 16, 16).ok_or("NES header is truncated")?;
            let fact = crate::nes_header_evidence::parse_ines_header(&header).ok_or("NES header is not recognized")?;
            if fact.mapper == 20 { return Err("FDS mapper cartridge containers are unsupported; use structurally validated direct .fds media".into()); }
            // The shared parser deliberately does not decode exponent sizes.
            if fact.is_nes20 && (header[9] & 15 == 15 || header[9] >> 4 == 15) { return Err("NES 2.0 exponent-sized images are not supported by this launch inspector".into()); }
            let required = 16 + u64::from(fact.prg_rom_16k_units) * 16384 + u64::from(fact.chr_rom_8k_units) * 8192 + if fact.trainer {512} else {0};
            if fact.prg_rom_16k_units == 0 || required > safe.len() { Err("NES payload is missing or truncated".into()) } else { Ok(()) }
        }
        _ => Err("Mesen media inspection supports direct .nes and structurally validated .fds files; archives and UNIF are not supported".into()),
    }
}
/// Primary precedence mirrors Mesen. We conservatively refuse an unsafe or
/// wrong-size primary instead of guessing which fallback a future build uses.
pub fn inspect_mesen_nes_firmware(profile: &MesenProfile, content: &Path) -> MesenNesReadiness {
    if !content
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("fds"))
    {
        return MesenNesReadiness {
            firmware: FirmwareReadiness::NotRequired,
            firmware_sha256: None,
            detail: "NES cartridge launch does not require the FDS BIOS".into(),
        };
    }
    let directory = profile.configuration_path.join("Firmware");
    if directory
        .ancestors()
        .any(|path| std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()))
    {
        return MesenNesReadiness {
            firmware: FirmwareReadiness::Unknown,
            firmware_sha256: None,
            detail: "FDS firmware inspection refused a symlinked profile or Firmware directory"
                .into(),
        };
    }
    let mut path: PathBuf = directory.join("disksys.rom");
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => path = directory.join("FdsBios.bin"),
        _ => {}
    }
    let missing =
        std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
    match read(&path, 8192) {
        Ok(bytes) => {
            let digest: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let verified = bytes.len() == 8192 && FDS_SHA256.contains(&digest.as_str());
            MesenNesReadiness {
                firmware: if verified {
                    FirmwareReadiness::Verified
                } else {
                    FirmwareReadiness::PresentUnverified
                },
                firmware_sha256: verified.then_some(digest),
                detail: if verified {
                    format!("FDS BIOS verified by exact SHA256: {}", path.display())
                } else {
                    format!(
                        "FDS BIOS at {} is present but its size/SHA256 is not recognized; filename matching is not verification",
                        path.display()
                    )
                },
            }
        }
        Err(e) => MesenNesReadiness {
            firmware: if missing {
                FirmwareReadiness::Missing
            } else {
                FirmwareReadiness::Unknown
            },
            firmware_sha256: None,
            detail: format!(
                "FDS requires a verified 8192-byte BIOS in {} (disksys.rom or FdsBios.bin): {e}",
                directory.display()
            ),
        },
    }
}
#[cfg(test)]
mod tests;
