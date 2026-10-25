//! Bounded, read-only Amiberry CD profile and media evidence.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::amiga_cd_evidence::AmigaCdMachine;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::patch_manager::{
    AmigaEmulatorKind, AmigaGameRequest, AmigaProfile, inspect_amiga_whdload_game,
};

pub const AMIBERRY_CD_DESCRIPTOR_LIMIT: usize = 64 * 1024;
pub const AMIBERRY_CD_REFERENCE_LIMIT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmiberryCdDiscoveryStatus {
    Ready,
    MissingProfile,
    UnreadableProfile,
    MissingDependency,
    UnreadableDependency,
    MalformedDescriptor,
    UnsafePath,
    WrongMachine,
    MediaSlotMissing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCdMediaDependency {
    pub path: PathBuf,
    pub identity: CapturedFileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCdMediaDiscovery {
    pub selected: PathBuf,
    pub dependencies: Vec<AmiberryCdMediaDependency>,
    pub status: AmiberryCdDiscoveryStatus,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCdProfileDiscovery {
    pub profile_id: String,
    pub config_path: Option<PathBuf>,
    pub machine: AmigaCdMachine,
    pub machine_model: Option<String>,
    pub kickstart_path: Option<PathBuf>,
    pub extended_rom_path: Option<PathBuf>,
    pub media_slots: Vec<PathBuf>,
    pub status: AmiberryCdDiscoveryStatus,
    pub detail: String,
}

fn expected_model(machine: AmigaCdMachine) -> Option<&'static str> {
    match machine {
        AmigaCdMachine::Cd32 => Some("CD32"),
        AmigaCdMachine::Cdtv => Some("CDTV"),
        AmigaCdMachine::OrdinaryAmiga => None,
    }
}

fn safe_regular(path: &Path) -> Result<fs::Metadata, AmiberryCdDiscoveryStatus> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| AmiberryCdDiscoveryStatus::MissingDependency)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AmiberryCdDiscoveryStatus::UnsafePath);
    }
    Ok(metadata)
}

fn resolve_config_path(config: &Path, value: &str) -> Option<PathBuf> {
    let raw = Path::new(value.trim());
    if raw.as_os_str().is_empty()
        || raw.is_absolute()
        || raw.components().any(|c| c == Component::ParentDir)
    {
        return None;
    }
    Some(config.parent().unwrap_or_else(|| Path::new(".")).join(raw))
}

fn known_cd_setting(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "cdimage"
            | "cdimage0"
            | "cd_image"
            | "cd_image0"
            | "cdrom"
            | "cdrom_image"
            | "cdrom_path"
            | "cdrom_image0"
            | "extended_rom"
            | "extended_rom_file"
            | "cd32_extended_rom"
            | "cdtv_extended_rom"
    )
}

fn empty_profile(
    profile: &AmigaProfile,
    machine: AmigaCdMachine,
    config_path: Option<PathBuf>,
    status: AmiberryCdDiscoveryStatus,
    detail: &str,
) -> AmiberryCdProfileDiscovery {
    AmiberryCdProfileDiscovery {
        profile_id: profile.profile_id.clone(),
        config_path,
        machine,
        machine_model: None,
        kickstart_path: None,
        extended_rom_path: None,
        media_slots: Vec::new(),
        status,
        detail: detail.into(),
    }
}

/// Inspects one already-discovered Amiberry profile for an exact CD machine.
pub fn discover_amiberry_cd_profile(
    profile: &AmigaProfile,
    machine: AmigaCdMachine,
) -> AmiberryCdProfileDiscovery {
    let Some(expected) = expected_model(machine) else {
        return empty_profile(
            profile,
            machine,
            profile.global_config_path.clone(),
            AmiberryCdDiscoveryStatus::WrongMachine,
            "ordinary Amiga is not a CD profile target",
        );
    };
    if profile.emulator != AmigaEmulatorKind::Amiberry {
        return empty_profile(
            profile,
            machine,
            profile.global_config_path.clone(),
            AmiberryCdDiscoveryStatus::WrongMachine,
            "profile belongs to a different emulator",
        );
    }
    let Some(config_path) = profile.global_config_path.as_ref() else {
        return empty_profile(
            profile,
            machine,
            None,
            AmiberryCdDiscoveryStatus::MissingProfile,
            "Amiberry has no global config",
        );
    };
    let inspected = inspect_amiga_whdload_game(profile, &AmigaGameRequest::default());
    if !inspected.config.exists {
        return empty_profile(
            profile,
            machine,
            Some(config_path.clone()),
            AmiberryCdDiscoveryStatus::MissingProfile,
            "Amiberry profile config is missing",
        );
    }
    if !inspected.config.readable {
        return empty_profile(
            profile,
            machine,
            Some(config_path.clone()),
            AmiberryCdDiscoveryStatus::UnreadableProfile,
            "Amiberry profile config is unreadable",
        );
    }
    let model = inspected.config.machine.machine_model.clone();
    if model.as_deref() != Some(expected) {
        return AmiberryCdProfileDiscovery {
            profile_id: profile.profile_id.clone(),
            config_path: Some(config_path.clone()),
            machine,
            machine_model: model,
            kickstart_path: inspected.config.machine.kickstart_path,
            extended_rom_path: None,
            media_slots: Vec::new(),
            status: AmiberryCdDiscoveryStatus::WrongMachine,
            detail: "profile machine model does not match the requested CD machine".into(),
        };
    }
    let mut media_slots = Vec::new();
    let mut extended_rom_path = None;
    for (key, value) in &inspected.config.machine.unknown {
        if !known_cd_setting(key) {
            continue;
        }
        if key.to_ascii_lowercase().contains("extended") {
            extended_rom_path = resolve_config_path(config_path, value);
        } else if let Some(path) = resolve_config_path(config_path, value) {
            media_slots.push(path);
        }
    }
    media_slots.sort();
    media_slots.dedup();
    let status = if media_slots.is_empty() {
        AmiberryCdDiscoveryStatus::MediaSlotMissing
    } else {
        AmiberryCdDiscoveryStatus::Ready
    };
    AmiberryCdProfileDiscovery {
        profile_id: profile.profile_id.clone(),
        config_path: Some(config_path.clone()),
        machine,
        machine_model: model,
        kickstart_path: inspected.config.machine.kickstart_path,
        extended_rom_path,
        media_slots,
        status,
        detail: if status == AmiberryCdDiscoveryStatus::Ready {
            "exact machine and CD media slot found".into()
        } else {
            "profile has no recognized CD media slot".into()
        },
    }
}

/// Reads and validates a selected ISO or CUE without changing it.
pub fn discover_amiberry_cd_media(path: &Path) -> AmiberryCdMediaDiscovery {
    let selected = path.to_path_buf();
    let metadata = match safe_regular(path) {
        Ok(metadata) => metadata,
        Err(status) => {
            return AmiberryCdMediaDiscovery {
                selected,
                dependencies: Vec::new(),
                status,
                detail: "selected media is missing or unsafe".into(),
            };
        }
    };
    let extension = path
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| v.to_ascii_lowercase());
    if extension.as_deref() == Some("iso") {
        return AmiberryCdMediaDiscovery {
            selected,
            dependencies: vec![AmiberryCdMediaDependency {
                path: path.to_path_buf(),
                identity: CapturedFileIdentity::capture(&metadata),
            }],
            status: AmiberryCdDiscoveryStatus::Ready,
            detail: "regular non-symlink ISO is readable by the caller".into(),
        };
    }
    if extension.as_deref() != Some("cue") {
        return bad(
            path,
            AmiberryCdDiscoveryStatus::MalformedDescriptor,
            "only CUE and ISO are supported by this discovery slice",
        );
    }
    let Ok(file_len) = usize::try_from(metadata.len()) else {
        return bad(
            path,
            AmiberryCdDiscoveryStatus::MalformedDescriptor,
            "CUE size is not representable",
        );
    };
    if file_len > AMIBERRY_CD_DESCRIPTOR_LIMIT {
        return bad(
            path,
            AmiberryCdDiscoveryStatus::MalformedDescriptor,
            "CUE exceeds bounded descriptor size",
        );
    }
    let Ok(contents) = fs::read(path) else {
        return bad(
            path,
            AmiberryCdDiscoveryStatus::UnreadableDependency,
            "CUE is unreadable",
        );
    };
    let Ok(text) = String::from_utf8(contents) else {
        return bad(
            path,
            AmiberryCdDiscoveryStatus::MalformedDescriptor,
            "CUE is not UTF-8 text",
        );
    };
    let mut refs = BTreeSet::new();
    let mut saw_file = false;
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed
            .strip_prefix("FILE")
            .or_else(|| trimmed.strip_prefix("file"))
        else {
            continue;
        };
        saw_file = true;
        let Some(quoted) = rest.trim_start().strip_prefix('"') else {
            return bad(
                path,
                AmiberryCdDiscoveryStatus::MalformedDescriptor,
                "CUE FILE reference is not quoted",
            );
        };
        let Some(end) = quoted.find('"') else {
            return bad(
                path,
                AmiberryCdDiscoveryStatus::MalformedDescriptor,
                "CUE FILE reference is unterminated",
            );
        };
        let raw = &quoted[..end];
        let raw_path = Path::new(raw);
        if raw.is_empty()
            || raw_path.is_absolute()
            || raw_path.components().any(|c| c == Component::ParentDir)
        {
            return bad(
                path,
                AmiberryCdDiscoveryStatus::UnsafePath,
                "CUE member escapes the media directory",
            );
        }
        if refs.len() >= AMIBERRY_CD_REFERENCE_LIMIT {
            return bad(
                path,
                AmiberryCdDiscoveryStatus::MalformedDescriptor,
                "CUE reference limit exceeded",
            );
        }
        refs.insert(
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(raw_path),
        );
    }
    if !saw_file || refs.is_empty() {
        return bad(
            path,
            AmiberryCdDiscoveryStatus::MalformedDescriptor,
            "CUE contains no usable FILE reference",
        );
    }
    let mut dependencies = Vec::new();
    for member in refs {
        let meta = match safe_regular(&member) {
            Ok(meta) => meta,
            Err(status) => return bad(path, status, "a CUE member is missing or unsafe"),
        };
        dependencies.push(AmiberryCdMediaDependency {
            path: member,
            identity: CapturedFileIdentity::capture(&meta),
        });
    }
    AmiberryCdMediaDiscovery {
        selected,
        dependencies,
        status: AmiberryCdDiscoveryStatus::Ready,
        detail: "all CUE members are present regular files".into(),
    }
}

fn bad(path: &Path, status: AmiberryCdDiscoveryStatus, detail: &str) -> AmiberryCdMediaDiscovery {
    AmiberryCdMediaDiscovery {
        selected: path.to_path_buf(),
        dependencies: Vec::new(),
        status,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::AmigaProfileScope;
    use std::io::Write;
    use tempfile::TempDir;

    fn profile(root: &Path, model: &str, media_key: &str) -> AmigaProfile {
        let config = root.join("amiberry.conf");
        fs::write(
            &config,
            format!("amiga_model={model}\n{media_key}=disc.cue\nkickstart_rom_file=kick.rom\n"),
        )
        .unwrap();
        AmigaProfile {
            profile_id: "test".into(),
            emulator: AmigaEmulatorKind::Amiberry,
            installation_type: crate::patch_manager::AmigaInstallationType::Explicit,
            scope: AmigaProfileScope::Explicit,
            configuration_root: root.into(),
            global_config_path: Some(config),
            profile_paths: Vec::new(),
            executable_candidates: Vec::new(),
            eligible: true,
            warnings: Vec::new(),
        }
    }
    #[test]
    fn valid_cd32_and_cdtv_profiles_are_exact_machine_bound() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("disc.cue"), "FILE \"disc.bin\" BINARY\n").unwrap();
        let p = profile(dir.path(), "CD32", "cdimage0");
        assert_eq!(
            discover_amiberry_cd_profile(&p, AmigaCdMachine::Cd32).status,
            AmiberryCdDiscoveryStatus::Ready
        );
        assert_eq!(
            discover_amiberry_cd_profile(&p, AmigaCdMachine::Cdtv).status,
            AmiberryCdDiscoveryStatus::WrongMachine
        );
    }
    #[test]
    fn generic_profile_and_ordinary_machine_are_rejected() {
        let dir = TempDir::new().unwrap();
        let p = profile(dir.path(), "A500", "cdimage0");
        assert_eq!(
            discover_amiberry_cd_profile(&p, AmigaCdMachine::Cd32).status,
            AmiberryCdDiscoveryStatus::WrongMachine
        );
        assert_eq!(
            discover_amiberry_cd_profile(&p, AmigaCdMachine::OrdinaryAmiga).status,
            AmiberryCdDiscoveryStatus::WrongMachine
        );
    }
    #[test]
    fn cue_dependencies_are_complete_and_deduplicated() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("disc.bin"), b"bin").unwrap();
        let cue = dir.path().join("disc.cue");
        fs::write(&cue, "FILE \"disc.bin\" BINARY\nFILE \"disc.bin\" BINARY\n").unwrap();
        let found = discover_amiberry_cd_media(&cue);
        assert_eq!(found.status, AmiberryCdDiscoveryStatus::Ready);
        assert_eq!(found.dependencies.len(), 1);
    }
    #[test]
    fn missing_malformed_and_escape_cues_fail_closed() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing.cue");
        fs::write(&missing, "FILE \"no.bin\" BINARY\n").unwrap();
        assert_eq!(
            discover_amiberry_cd_media(&missing).status,
            AmiberryCdDiscoveryStatus::MissingDependency
        );
        let bad = dir.path().join("bad.cue");
        fs::write(&bad, "FILE \"../outside.bin\" BINARY\n").unwrap();
        assert_eq!(
            discover_amiberry_cd_media(&bad).status,
            AmiberryCdDiscoveryStatus::UnsafePath
        );
    }
    #[test]
    fn iso_is_regular_and_symlink_is_rejected() {
        let dir = TempDir::new().unwrap();
        let iso = dir.path().join("disc.iso");
        fs::File::create(&iso).unwrap().write_all(b"iso").unwrap();
        assert_eq!(
            discover_amiberry_cd_media(&iso).status,
            AmiberryCdDiscoveryStatus::Ready
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link.iso");
            std::os::unix::fs::symlink(&iso, &link).unwrap();
            assert_eq!(
                discover_amiberry_cd_media(&link).status,
                AmiberryCdDiscoveryStatus::UnsafePath
            );
        }
    }
    #[test]
    fn unsupported_media_remains_blocked() {
        let dir = TempDir::new().unwrap();
        let chd = dir.path().join("disc.chd");
        fs::write(&chd, b"chd").unwrap();
        assert_eq!(
            discover_amiberry_cd_media(&chd).status,
            AmiberryCdDiscoveryStatus::MalformedDescriptor
        );
    }
}
