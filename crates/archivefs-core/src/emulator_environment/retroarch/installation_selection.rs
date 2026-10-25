//! Installation identity and the existing one-path override convention.
//! Scripts are never interpreted or executed to discover their target.
use super::*;

pub const EXECUTABLE_OVERRIDE_FILE: &str = "retroarch_executable_override.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationExecutable {
    Native,
    Flatpak,
    Unknown,
}

pub fn classify_installation_executable(
    filesystem: &dyn ReadOnlyHostFilesystem,
    path: &Path,
) -> InstallationExecutable {
    let BoundedReadResult::Ok(bytes) = filesystem.executable_header(path) else {
        return InstallationExecutable::Unknown;
    };
    if bytes.starts_with(b"\x7fELF") {
        return InstallationExecutable::Native;
    }
    let Ok(script) = std::str::from_utf8(&bytes) else {
        return InstallationExecutable::Unknown;
    };
    // A deliberately small accepted grammar: a shell shebang, comments,
    // then exactly one literal Flatpak invocation forwarding argv. No
    // substitutions, environment assignments, commands or redirections.
    let mut lines = script.lines();
    if !matches!(
        lines.next().map(str::trim),
        Some("#!/bin/sh" | "#!/bin/bash" | "#!/usr/bin/env bash" | "#!/usr/bin/env sh")
    ) {
        return InstallationExecutable::Unknown;
    }
    let commands: Vec<_> = lines
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let [command] = commands.as_slice() else {
        return InstallationExecutable::Unknown;
    };
    let words: Vec<_> = command.split_whitespace().collect();
    let words = words.strip_prefix(&["exec"]).unwrap_or(&words);
    if words.len() < 3 || !matches!(words[0], "flatpak" | "/usr/bin/flatpak") || words[1] != "run" {
        return InstallationExecutable::Unknown;
    }
    let mut index = 2;
    while words
        .get(index)
        .is_some_and(|word| matches!(*word, "--user" | "--system" | "--branch=stable"))
    {
        index += 1;
    }
    if !matches!(
        words.get(index),
        Some(
            &"org.libretro.RetroArch" | &"\"org.libretro.RetroArch\"" | &"'org.libretro.RetroArch'"
        )
    ) {
        return InstallationExecutable::Unknown;
    }
    if !matches!(&words[index + 1..], [] | ["\"$@\""]) {
        return InstallationExecutable::Unknown;
    }
    InstallationExecutable::Flatpak
}

/// Missing/blank means automatic. Invalid, unreadable, linked or oversized
/// files are errors, so a damaged selection cannot enable fallback.
pub fn load_executable_override_at(path: &Path) -> Result<Option<PathBuf>, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "RetroArch selection cannot be read safely: {error}"
            ));
        }
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("RetroArch selection is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 {
        return Err("RetroArch selection file is too large".into());
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "RetroArch selection is not UTF-8")?
        .trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.lines().count() != 1 || !Path::new(text).is_absolute() {
        return Err("RetroArch selection must contain one absolute program path".into());
    }
    Ok(Some(PathBuf::from(text)))
}

pub(super) fn apply_selection(
    filesystem: &dyn ReadOnlyHostFilesystem,
    profiles: &mut Vec<(RetroArchProfile, PathBuf, Vec<RawDiagnostic>)>,
    selected: &[PathBuf],
) -> Result<(), DiscoveryError> {
    if selected.is_empty() {
        return Ok(());
    }
    let [path] = selected else {
        return Err(DiscoveryError::ExecutableSelectionUnavailable);
    };
    // Keep the exact saved path: canonicalizing a changed symlink would
    // silently authorize a different program.
    if !path.is_absolute()
        || filesystem.probe(path) != FsProbe::PresentFile
        || filesystem.probe_regular_file_executable_bit(path) != Some(true)
    {
        return Err(DiscoveryError::ExecutableSelectionUnavailable);
    }
    if classify_installation_executable(filesystem, path) != InstallationExecutable::Native {
        return Err(DiscoveryError::ExecutableSelectionUnavailable);
    }
    let appimage = profiles
        .iter()
        .enumerate()
        .find_map(|(index, (profile, _, _))| {
            profile
                .app_images
                .iter()
                .find(|candidate| {
                    !candidate.path.lossy
                        && Path::new(&candidate.path.display) == path
                        && candidate.confidence == AppImageIdentificationConfidence::Exact
                        && !matches!(
                            candidate.config_association,
                            ConfigAssociation::Unknown | ConfigAssociation::Ambiguous
                        )
                        && !profile.config_file.path.lossy
                        && candidate.executable == Some(ExecutableState::Executable)
                })
                .map(|candidate| (index, candidate.clone()))
        });
    if let Some((index, candidate)) = appimage {
        let mut entry = profiles.remove(index);
        entry.0.profile_kind = ProfileKind::AppImage;
        entry.0.evidence.executables.clear();
        entry.0.app_images = vec![candidate];
        for diagnostic in &mut entry.2 {
            if let Some(profile) = &mut diagnostic.profile {
                profile.profile_kind = ProfileKind::AppImage;
            }
        }
        *profiles = vec![entry];
        return Ok(());
    }
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("AppImage"))
        || !profiles.iter().any(|(profile, _, _)| {
            profile.profile_kind == ProfileKind::Native
                && profile
                    .evidence
                    .executables
                    .iter()
                    .any(|candidate| !candidate.lossy && Path::new(&candidate.display) == path)
        })
    {
        return Err(DiscoveryError::ExecutableSelectionUnavailable);
    }
    profiles.retain(|(profile, _, _)| profile.profile_kind == ProfileKind::Native);
    let [(profile, _, _)] = profiles.as_mut_slice() else {
        return Err(DiscoveryError::ExecutableSelectionUnavailable);
    };
    if profile.config_file.path.lossy {
        return Err(DiscoveryError::ExecutableSelectionUnavailable);
    }
    profile.evidence.executables = vec![EncodedPath::from_path(path)];
    profile.app_images.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_environment::HostReadOnlyFilesystem;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn executable(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn environment(root: &Path) -> DiscoveryEnvironment {
        DiscoveryEnvironment {
            home: Some(root.into()),
            xdg_config_home: None,
            path: Some(
                std::env::join_paths([root.join("one"), root.join("two"), root.join("wrapper")])
                    .unwrap(),
            ),
            explicit_executables: Vec::new(),
            user_flatpak_root: root.join("flatpak-user"),
            system_flatpak_root: root.join("flatpak-system"),
            app_image_search_roots: vec![root.join("Applications")],
            desktop_file_roots: vec![root.join("desktop")],
        }
    }

    #[test]
    fn only_a_literal_flatpak_shell_launcher_is_classified_as_flatpak() {
        let root = tempfile::tempdir().unwrap();
        for script in [
            "#!/bin/sh\nexec flatpak run org.libretro.RetroArch \"$@\"\n",
            "#!/usr/bin/env bash\n# comment\nexec /usr/bin/flatpak run --user --branch=stable 'org.libretro.RetroArch' \"$@\"\n",
        ] {
            let path = executable(root.path(), "retroarch", script.as_bytes());
            assert_eq!(
                classify_installation_executable(&HostReadOnlyFilesystem, &path),
                InstallationExecutable::Flatpak
            );
        }
        for script in [
            "#!/bin/sh\n# flatpak run org.libretro.RetroArch\nexec /unknown/retroarch \"$@\"\n",
            "#!/bin/sh\nexec flatpak run \"$APP\" \"$@\"\n",
            "#!/bin/sh\nfalse; flatpak run org.libretro.RetroArch \"$@\"\n",
            "#!/bin/sh\nexec flatpak run org.libretro.RetroArch; touch /tmp/unsafe\n",
            "#!/bin/sh\nexec flatpak run org.libretro.RetroArch \"$@\"\nexit 0\n",
            "#!/bin/sh\nexec flatpak run org.other.App \"$@\"\n",
            "#!/bin/sh\nexec /usr/games/retroarch \"$@\"\n",
        ] {
            let path = executable(root.path(), "retroarch", script.as_bytes());
            assert_eq!(
                classify_installation_executable(&HostReadOnlyFilesystem, &path),
                InstallationExecutable::Unknown,
                "{script}"
            );
        }
        let oversized = format!(
            "#!/bin/sh\n{}\nexec flatpak run org.libretro.RetroArch \"$@\"\n",
            "#".repeat(20 * 1024)
        );
        let path = executable(root.path(), "retroarch", oversized.as_bytes());
        assert_eq!(
            classify_installation_executable(&HostReadOnlyFilesystem, &path),
            InstallationExecutable::Unknown
        );
    }

    #[test]
    fn explicit_selection_survives_reload_and_never_falls_back_when_removed_or_replaced() {
        let root = tempfile::tempdir().unwrap();
        let one = executable(root.path(), "one/retroarch", b"\x7fELFone");
        let two = executable(root.path(), "two/retroarch", b"\x7fELFtwo");
        let wrapper = executable(
            root.path(),
            "wrapper/retroarch",
            b"#!/bin/sh\nexec flatpak run org.libretro.RetroArch \"$@\"\n",
        );
        let mut env = environment(root.path());
        let automatic = discover_retroarch_environment(&HostReadOnlyFilesystem, &env).unwrap();
        assert_eq!(automatic.profiles[0].evidence.executables.len(), 2);
        let flatpak = automatic
            .profiles
            .iter()
            .find(|profile| {
                profile.profile_kind == ProfileKind::Flatpak && profile.scope == ProfileScope::User
            })
            .unwrap();
        assert_eq!(
            flatpak.evidence.executables,
            vec![EncodedPath::from_path(&wrapper)]
        );
        let sidecar = root.path().join("emuwiz/retroarch_executable_override.txt");
        fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        fs::write(&sidecar, two.to_str().unwrap()).unwrap();
        env.explicit_executables = load_executable_override_at(&sidecar)
            .unwrap()
            .into_iter()
            .collect();
        let report = discover_retroarch_environment(&HostReadOnlyFilesystem, &env).unwrap();
        assert_eq!(report.profiles.len(), 1);
        assert_eq!(
            report.profiles[0].evidence.executables,
            vec![EncodedPath::from_path(&two)]
        );
        fs::remove_file(&two).unwrap();
        assert_eq!(
            discover_retroarch_environment(&HostReadOnlyFilesystem, &env).unwrap_err(),
            DiscoveryError::ExecutableSelectionUnavailable
        );
        fs::write(
            &two,
            b"#!/bin/sh\nexec flatpak run org.libretro.RetroArch \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&two, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(discover_retroarch_environment(&HostReadOnlyFilesystem, &env).is_err());
        fs::remove_file(&two).unwrap();
        symlink(&one, &two).unwrap();
        assert!(discover_retroarch_environment(&HostReadOnlyFilesystem, &env).is_err());
        assert_eq!(fs::read(&one).unwrap(), b"\x7fELFone");
        assert_eq!(
            fs::read(&wrapper).unwrap(),
            b"#!/bin/sh\nexec flatpak run org.libretro.RetroArch \"$@\"\n"
        );
        fs::remove_file(&sidecar).unwrap();
        assert_eq!(load_executable_override_at(&sidecar).unwrap(), None);
    }

    #[test]
    fn a_verified_appimage_sharing_native_settings_uses_only_its_exact_executable() {
        let root = tempfile::tempdir().unwrap();
        executable(root.path(), "one/retroarch", b"\x7fELFnative");
        let appimage = executable(
            root.path(),
            "Applications/RetroArch.AppImage",
            b"\x7fELFimage",
        );
        fs::create_dir_all(root.path().join("desktop")).unwrap();
        fs::write(
            root.path().join("desktop/retroarch.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=RetroArch\nExec={}\n",
                appimage.display()
            ),
        )
        .unwrap();
        let mut env = environment(root.path());
        env.explicit_executables = vec![appimage.clone()];
        let report = discover_retroarch_environment(&HostReadOnlyFilesystem, &env).unwrap();
        assert_eq!(report.profiles.len(), 1);
        let profile = &report.profiles[0];
        assert_eq!(profile.profile_kind, ProfileKind::AppImage);
        assert_eq!(
            profile.config_file.path,
            EncodedPath::from_path(&root.path().join(".config/retroarch/retroarch.cfg"))
        );
        assert!(profile.evidence.executables.is_empty());
        assert_eq!(profile.app_images.len(), 1);
        assert_eq!(
            profile.app_images[0].path,
            EncodedPath::from_path(&appimage)
        );
        fs::write(
            root.path().join("desktop/retroarch.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=RetroArch\nExec={} --config\n",
                appimage.display()
            ),
        )
        .unwrap();
        assert!(
            discover_retroarch_environment(&HostReadOnlyFilesystem, &env).is_err(),
            "an unresolved config association is never selected"
        );
        fs::write(
            root.path().join("desktop/retroarch.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=RetroArch\nExec={}\n",
                appimage.display()
            ),
        )
        .unwrap();
        fs::write(
            &appimage,
            "#!/bin/sh\nexec flatpak run org.libretro.RetroArch \"$@\"\n",
        )
        .unwrap();
        assert!(discover_retroarch_environment(&HostReadOnlyFilesystem, &env).is_err());
    }

    #[test]
    fn sidecar_absence_blank_and_errors_are_distinguished() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("choice.txt");
        assert_eq!(load_executable_override_at(&path).unwrap(), None);
        fs::write(&path, " \n").unwrap();
        assert_eq!(load_executable_override_at(&path).unwrap(), None);
        for invalid in ["relative/path", "/one\n/two", "/one\r\n/two"] {
            fs::write(&path, invalid).unwrap();
            assert!(load_executable_override_at(&path).is_err());
        }
        fs::remove_file(&path).unwrap();
        let target = root.path().join("target");
        fs::write(&target, "/user/unchanged").unwrap();
        symlink(&target, &path).unwrap();
        assert!(load_executable_override_at(&path).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "/user/unchanged");
    }
}
