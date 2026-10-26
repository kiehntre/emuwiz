//! Strict, lossless-enough INI overlay for the tested PCSX2 runtime.
//! Unknown settings and comments are retained; ambiguous settings are refused.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub(super) struct Ini {
    sections: Vec<(String, Vec<String>)>,
}

impl Ini {
    pub(super) fn sections_present(&self, name: &str) -> bool {
        self.sections
            .iter()
            .any(|(s, _)| s.eq_ignore_ascii_case(name))
    }
    pub(super) fn parse(text: &str) -> Result<Self, String> {
        if text.contains('\0') {
            return Err("PCSX2 settings contain a NUL byte".into());
        }
        let mut sections = vec![(String::new(), Vec::new())];
        let mut seen_sections = BTreeSet::new();
        let mut seen_keys = BTreeSet::new();
        let mut section = String::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('[') {
                let name = line
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .filter(|s| !s.is_empty())
                    .ok_or("Malformed PCSX2 settings section")?;
                if name != name.trim() {
                    return Err("Ambiguous whitespace inside PCSX2 settings section".into());
                }
                if !seen_sections.insert(name.to_ascii_lowercase()) {
                    return Err(format!("Repeated PCSX2 settings section: {name}"));
                }
                for known in [
                    "Folders",
                    "EmuCore",
                    "Cheats",
                    "Logging",
                    "MemoryCards",
                    "Filenames",
                ] {
                    if name.eq_ignore_ascii_case(known) && name != known {
                        return Err(format!("Ambiguous PCSX2 settings section spelling: {name}"));
                    }
                }
                section = name.to_string();
                sections.push((section.clone(), Vec::new()));
            } else {
                if !line.is_empty() && !line.starts_with([';', '#']) {
                    let (key, _) = line.split_once('=').ok_or("Malformed PCSX2 setting")?;
                    if !seen_keys.insert((
                        section.to_ascii_lowercase(),
                        key.trim().to_ascii_lowercase(),
                    )) {
                        return Err(format!("Repeated PCSX2 setting: {section}/{}", key.trim()));
                    }
                }
                sections.last_mut().unwrap().1.push(raw.to_string());
            }
        }
        Ok(Self { sections })
    }

    pub(super) fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .iter()
            .find(|(s, _)| s.eq_ignore_ascii_case(section))?
            .1
            .iter()
            .find_map(|line| {
                let (k, value) = line.split_once('=')?;
                (k.trim().eq_ignore_ascii_case(key)).then(|| value.trim())
            })
    }

    pub(super) fn set(&mut self, section: &str, key: &str, value: &str) -> Result<(), String> {
        if value.contains(['\0', '\r', '\n']) {
            return Err("Unsafe setting value".into());
        }
        if !self.sections.iter().any(|(s, _)| s == section) {
            self.sections.push((section.into(), Vec::new()));
        }
        let lines = &mut self
            .sections
            .iter_mut()
            .find(|(s, _)| s == section)
            .unwrap()
            .1;
        if let Some(line) = lines.iter_mut().find(|line| {
            line.split_once('=')
                .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        }) {
            let (old_key, _) = line.split_once('=').unwrap();
            if old_key.trim() != key {
                return Err(format!("Ambiguous PCSX2 setting spelling: {key}"));
            }
            *line = format!("{key} = {value}");
        } else {
            lines.push(format!("{key} = {value}"));
        }
        Ok(())
    }

    pub(super) fn text(&self) -> String {
        let mut text = String::new();
        for (name, lines) in &self.sections {
            if !name.is_empty() {
                text.push_str(&format!("\n[{name}]\n"));
            }
            for line in lines {
                text.push_str(line);
                text.push('\n');
            }
        }
        text
    }

    pub(super) fn enable_selection(&mut self) -> Result<(), String> {
        self.set("EmuCore", "EnableCheats", "true")?;
        self.set("Cheats", "Enable", SELECTED_GROUP)
    }

    pub(super) fn validate_hardcore(&self, global: Option<&Ini>) -> Result<(), String> {
        let enabled = |key| -> Result<bool, String> {
            match self
                .get("Achievements", key)
                .or_else(|| global.and_then(|g| g.get("Achievements", key)))
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                None | Some("false" | "no" | "off" | "0") => Ok(false),
                Some("true" | "yes" | "on" | "1") => Ok(true),
                Some(_) => Err(format!("Ambiguous PCSX2 achievement setting: {key}")),
            }
        };
        if enabled("Enabled")? && enabled("ChallengeMode")? {
            return Err("PCSX2 achievement challenge/hardcore mode blocks cheats. Review that setting in ordinary PCSX2; EmuWiz will not disable it automatically.".into());
        }
        Ok(())
    }

    pub(super) fn validate_memory_cards(&self) -> Result<(), String> {
        if let Some((_, lines)) = self.sections.iter().find(|(name, _)| name == "MemoryCards") {
            for line in lines {
                if let Some((key, value)) = line.split_once('=') {
                    let value = value.trim();
                    if key.trim().to_ascii_lowercase().ends_with("_filename")
                        && !value.is_empty()
                        && (Path::new(value).components().count() != 1
                            || !normal_path_component(value)
                            || value.contains('\\')
                            || value.chars().any(|c| c.is_control()))
                    {
                        return Err("Memory-card filenames must stay inside the selected memory-card folder".into());
                    }
                }
            }
        }
        Ok(())
    }
}

fn normal_path_component(value: &str) -> bool {
    matches!(
        Path::new(value).components().next(),
        Some(std::path::Component::Normal(_))
    )
}

pub(super) fn read_source(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    validate_destination_root(path.parent().ok_or("Source has no parent")?)
        .map_err(|e| format!("Unsafe source parent: {e:?}"))?;
    let mut file =
        open_regular_file_nonblocking(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let before = CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?);
    if before.size > limit {
        return Err(format!("{} exceeds the size limit", path.display()));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit
        || CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?) != before
    {
        return Err("A source changed while reading it".into());
    }
    Ok(bytes)
}

pub(super) fn actual_folder_paths(ini: &Ini) -> Result<Vec<(String, PathBuf)>, String> {
    let labels = [
        ("Bios", "BIOS"),
        ("Snapshots", "Snapshots"),
        ("Savestates", "Savestates"),
        ("MemoryCards", "MemoryCards"),
        ("Logs", "Logs"),
        ("Cheats", "Cheats"),
        ("Patches", "Patches"),
        ("Covers", "Covers"),
        ("GameSettings", "Game Settings"),
        ("UserResources", "User Resources"),
        ("Cache", "Cache"),
        ("Textures", "Textures"),
        ("InputProfiles", "Input Profile"),
        ("Videos", "Video Dumping"),
        ("DebuggerLayouts", "Debugger Layouts"),
        ("DebuggerSettings", "Debugger Settings"),
    ];
    labels
        .into_iter()
        .map(|(key, label)| {
            Ok((
                label.into(),
                PathBuf::from(ini.get("Folders", key).ok_or("Missing pinned folder")?),
            ))
        })
        .collect()
}

/// Copy verified BIOS bytes and optional console state into the canonical
/// plan. A read-only source BIOS directory is insufficient: PCSX2 creates
/// .mec/.nvm beside the image, even when a game never changes console state.
pub(super) fn stage_bios(
    overlay: &mut Overlay,
    firmware: &[FirmwareIdentityRecord],
    data: &Path,
    sources: &mut Vec<SourceBinding>,
    files: &mut Vec<RuntimeFile>,
) -> Result<(), String> {
    let configured = overlay
        .ini
        .get("Filenames", "BIOS")
        .filter(|s| !s.is_empty())
        .ok_or("Choose the BIOS in ordinary PCSX2 setup before using temporary cheats")?;
    let source = if Path::new(configured).is_absolute() {
        PathBuf::from(configured)
    } else {
        overlay
            .folders
            .get("Bios")
            .ok_or("Missing BIOS folder")?
            .join(configured)
    };
    let bytes = read_source(&source, 8 * 1024 * 1024)?;
    use crate::dat::firmware_evidence::{
        ComputedFirmwareDigests, FirmwareSystem, matching_firmware_records,
    };
    use md5::Md5;
    use sha1::{Sha1, digest::Digest};
    let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() };
    let digests = ComputedFirmwareDigests {
        size_bytes: bytes.len() as u64,
        crc32: crate::identity_source::hashing::Crc32::of(&bytes),
        md5: hex(&Md5::digest(&bytes)),
        sha1: hex(&Sha1::digest(&bytes)),
    };
    if !matching_firmware_records(&digests, firmware)
        .iter()
        .any(|r| r.system == FirmwareSystem::PlayStation2)
    {
        return Err("The exact BIOS copy does not match the verified PS2 firmware evidence".into());
    }
    let name = source.file_name().ok_or("BIOS has no filename")?;
    let destination = data.join("bios").join(name);
    overlay.ini.set(
        "Filenames",
        "BIOS",
        name.to_str().ok_or("BIOS filename is not UTF-8")?,
    )?;
    sources.push(SourceBinding {
        path: source.clone(),
        expected_sha256: hash(&bytes),
    });
    files.push(RuntimeFile::new(
        destination.clone(),
        bytes,
        RuntimeFileKind::VerifiedResource,
    ));
    for ext in ["nvm", "mec", "rom1", "rom2"] {
        let path = source.with_extension(ext);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                let resource = matches!(ext, "rom1" | "rom2");
                let bytes = read_source(
                    &path,
                    if resource {
                        8 * 1024 * 1024
                    } else {
                        PCSX2_MAX_CONFIG_BYTES
                    },
                )?;
                sources.push(SourceBinding {
                    path,
                    expected_sha256: hash(&bytes),
                });
                files.push(RuntimeFile::new(
                    destination.with_extension(ext),
                    bytes,
                    if resource {
                        RuntimeFileKind::VerifiedResource
                    } else {
                        RuntimeFileKind::Config
                    },
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

pub(super) fn source_ini(path: &Path, sources: &mut Vec<SourceBinding>) -> Result<Ini, String> {
    let bytes = read_source(path, PCSX2_MAX_CONFIG_BYTES)?;
    let ini = Ini::parse(std::str::from_utf8(&bytes).map_err(|_| "PCSX2 settings are not UTF-8")?)?;
    sources.push(SourceBinding {
        path: path.into(),
        expected_sha256: hash(&bytes),
    });
    ini.validate_memory_cards()?;
    Ok(ini)
}

pub(super) fn global_path(profile: &Pcsx2Profile) -> Result<PathBuf, String> {
    let modern = profile.configuration_path.join("inis/PCSX2.ini");
    match std::fs::symlink_metadata(&modern) {
        Ok(_) => Ok(modern),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(profile.configuration_path.join("PCSX2.ini"))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Resolve only normal paths. Parent traversal, symlink ancestors and broad
/// filesystem grants are refused rather than guessed or canonicalized away.
pub(super) fn folder(value: Option<&str>, base: &Path, default: &str) -> Result<PathBuf, String> {
    let value = value.filter(|s| !s.is_empty()).unwrap_or(default);
    let raw = Path::new(value);
    if value.contains([';', '\\', '\n', '\r', '\0']) {
        return Err("Folder contains ambiguous INI/sandbox characters".into());
    }
    let path = if raw.is_absolute() {
        raw.into()
    } else {
        base.join(raw)
    };
    if path.components().any(|p| {
        !matches!(
            p,
            std::path::Component::RootDir | std::path::Component::Normal(_)
        )
    }) {
        return Err(format!(
            "Folder contains parent traversal or ambiguous components: {}",
            path.display()
        ));
    }
    validate_destination_root(&path)
        .map_err(|e| format!("Unsafe PCSX2 folder {}: {e:?}", path.display()))?;
    Ok(path)
}

pub(super) struct Overlay {
    pub ini: Ini,
    pub routing: Pcsx2FolderRouting,
    pub folders: BTreeMap<String, PathBuf>,
    pub games: PathBuf,
    pub visibility: VisibilityPlan,
}

pub(super) fn overlay(
    mut ini: Ini,
    profile: &Pcsx2Profile,
    workspace: &Path,
    content: &Path,
) -> Result<Overlay, String> {
    ini.validate_hardcore(None)?;
    // Classifier also refuses duplicate/misspelled routing keys.
    if matches!(
        Pcsx2FolderRouting::from_ini_text(&ini.text()),
        Pcsx2FolderRouting::Unknown { .. }
    ) {
        return Err("PCSX2 folder routing is ambiguous".into());
    }
    let real = &profile.configuration_path;
    let settings = real.join("inis");
    let mut folders = BTreeMap::new();
    let mut visibility = VisibilityPlan::new();
    visibility
        .content(content)
        .read_write(workspace, "temporary PCSX2 profile");
    visibility.read_only(real, "selected PCSX2 profile resources");
    let mut games = PathBuf::new();
    for (key, default) in [
        ("Bios", "bios"),
        ("Snapshots", "snaps"),
        ("Savestates", "sstates"),
        ("MemoryCards", "memcards"),
        ("Logs", "logs"),
        ("Patches", "patches"),
        ("Covers", "covers"),
        ("GameSettings", "gamesettings"),
        ("UserResources", "resources"),
        ("Cache", "cache"),
        ("Textures", "textures"),
        ("InputProfiles", "inputprofiles"),
        ("Videos", "videos"),
        ("DebuggerLayouts", "debuggerlayouts"),
        ("DebuggerSettings", "debuggersettings"),
    ] {
        let base = if key.starts_with("Debugger") {
            &settings
        } else {
            real
        };
        let original = folder(ini.get("Folders", key), base, default)?;
        let ephemeral = matches!(
            key,
            "Bios" | "GameSettings" | "Logs" | "Cache" | "DebuggerLayouts" | "DebuggerSettings"
        );
        let actual = if ephemeral {
            workspace.join("PCSX2").join(default)
        } else {
            original.clone()
        };
        if key == "GameSettings" {
            games = original.clone();
        }
        if !ephemeral {
            if matches!(key, "MemoryCards" | "Savestates" | "Snapshots" | "Videos") {
                // Real writable state must already exist: never create a new
                // real save/card folder as a side effect of this feature.
                if !std::fs::symlink_metadata(&original)
                    .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                {
                    return Err(format!(
                        "Real {key} folder is unavailable: {}. Open PCSX2 normally to set it up first.",
                        original.display()
                    ));
                }
                visibility.read_write(&original, "existing PCSX2 user state");
            } else if !original.starts_with(real) || key == "Bios" {
                visibility.read_only(&original, "PCSX2 read-only resources");
            }
        }
        ini.set(
            "Folders",
            key,
            actual.to_str().ok_or("Folder is not UTF-8")?,
        )?;
        folders.insert(key.to_string(), original);
    }
    // The reviewed planner classifies the normalized real routing. The
    // adapter alone pins Cheats to an owned workspace after that projection.
    ini.set("Folders", "Cheats", "")?;
    let routing = Pcsx2FolderRouting::from_ini_text(&ini.text());
    ini.set(
        "Folders",
        "Cheats",
        workspace
            .join("PCSX2/cheats")
            .to_str()
            .ok_or("Workspace is not UTF-8")?,
    )?;
    ini.enable_selection()?;
    ini.set("Logging", "EnableFileLogging", "true")?;
    ini.set("Logging", "EnableSystemConsole", "false")?;
    ini.set("Logging", "EnableEEConsole", "false")?;
    ini.set("Logging", "EnableIOPConsole", "false")?;
    visibility.flatpak_options().map_err(|e| e.to_string())?;
    Ok(Overlay {
        ini,
        routing,
        folders,
        games,
        visibility,
    })
}
