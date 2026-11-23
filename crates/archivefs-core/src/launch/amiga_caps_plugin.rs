//! The CAPS/SPS plugin check for Amiga IPF floppies.
//!
//! IPF preservation images are decoded by the SPS library (`capsimg.so`),
//! which RetroArch's PUAE core loads from RetroArch's system folder. Without
//! it the core boots to an empty drive, so an IPF launch is refused with a
//! clear reason instead.

use std::path::Path;

/// The plugin file PUAE looks for in RetroArch's system folder.
pub const CAPS_PLUGIN_FILE: &str = "capsimg.so";

/// Whether this launch is an Amiga IPF that needs the plugin.
pub fn needs_caps_plugin(platform_id: &str, content: &Path) -> bool {
    platform_id == "Amiga"
        && content
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("ipf"))
}

/// The libretro cores that load the plugin from RetroArch's system folder
/// (PUAE and its frozen 2021 branch).
pub const CAPS_CORE_STEMS: &[&str] = &["puae", "puae2021"];

/// Ok when the selected core is a PUAE build: the plugin check proves
/// nothing for any other core.
pub fn require_caps_core(core_stem: &str) -> Result<(), String> {
    if CAPS_CORE_STEMS.contains(&core_stem) {
        Ok(())
    } else {
        Err(format!(
            "Amiga IPF games start only with RetroArch's PUAE core, which reads the CAPS/SPS plugin; the selected core is {core_stem}."
        ))
    }
}

/// The ELF machine this build runs on, which a loadable plugin must match.
fn host_elf_machine() -> Option<u16> {
    match std::env::consts::ARCH {
        "x86_64" => Some(62),
        "aarch64" => Some(183),
        _ => None,
    }
}

/// Ok when `capsimg.so` in the system folder is a regular file (not a link)
/// holding a 64-bit little-endian ELF shared object for this machine, i.e.
/// one RetroArch on this host could load.
pub fn require_caps_plugin(system_directory: Option<&Path>) -> Result<(), String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let Some(directory) = system_directory else {
        return Err("Amiga IPF games need the CAPS/SPS plugin (capsimg.so) in RetroArch's system folder, and RetroArch's system folder could not be found.".into());
    };
    let plugin = directory.join(CAPS_PLUGIN_FILE);
    let missing = || {
        format!(
            "Amiga IPF games need the CAPS/SPS plugin (capsimg.so), which is not in RetroArch's system folder ({}).",
            directory.display()
        )
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&plugin)
    else {
        return Err(missing());
    };
    if !file
        .metadata()
        .is_ok_and(|m| m.file_type().is_file() && m.len() > 0)
    {
        return Err(missing());
    }
    let mut header = [0u8; 20];
    let loadable = file.read_exact(&mut header).is_ok()
        && header[..4] == *b"\x7fELF"
        && header[4] == 2 // 64-bit
        && header[5] == 1 // little-endian
        && u16::from_le_bytes([header[16], header[17]]) == 3 // shared object
        && host_elf_machine() == Some(u16::from_le_bytes([header[18], header[19]]));
    if loadable {
        Ok(())
    } else {
        Err(format!(
            "The CAPS/SPS plugin in RetroArch's system folder ({}) is not a library this computer can load; install the build for this machine.",
            plugin.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_amiga_ipf_needs_the_plugin() {
        assert!(needs_caps_plugin(
            "Amiga",
            Path::new("/g/Klax (Europe).IPF")
        ));
        assert!(!needs_caps_plugin("Amiga", Path::new("/g/a.adf")));
        assert!(!needs_caps_plugin("Amiga", Path::new("/g/a.hdf")));
        assert!(!needs_caps_plugin("AtariST", Path::new("/g/a.ipf")));
    }

    #[test]
    fn the_plugin_must_be_a_real_non_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(require_caps_plugin(None).is_err());
        let missing = require_caps_plugin(Some(dir.path())).unwrap_err();
        assert!(missing.contains("capsimg.so"), "{missing}");

        let plugin = dir.path().join(CAPS_PLUGIN_FILE);
        std::fs::write(&plugin, b"").unwrap();
        assert!(require_caps_plugin(Some(dir.path())).is_err(), "empty file");

        std::fs::remove_file(&plugin).unwrap();
        let elsewhere = dir.path().join("real.so");
        std::fs::write(&elsewhere, b"ELF").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &plugin).unwrap();
        assert!(require_caps_plugin(Some(dir.path())).is_err(), "symlink");

        std::fs::remove_file(&plugin).unwrap();
        std::fs::write(&plugin, b"ELF").unwrap();
        assert!(
            require_caps_plugin(Some(dir.path())).is_err(),
            "not an ELF library"
        );

        std::fs::remove_file(&plugin).unwrap();
        std::fs::write(&plugin, elf_header(host_elf_machine().unwrap_or(62), 3)).unwrap();
        assert!(require_caps_plugin(Some(dir.path())).is_ok());

        std::fs::remove_file(&plugin).unwrap();
        std::fs::write(&plugin, elf_header(40, 3)).unwrap();
        assert!(
            require_caps_plugin(Some(dir.path())).is_err(),
            "ARM32 build"
        );

        std::fs::remove_file(&plugin).unwrap();
        std::fs::write(&plugin, elf_header(host_elf_machine().unwrap_or(62), 2)).unwrap();
        assert!(
            require_caps_plugin(Some(dir.path())).is_err(),
            "executable, not a library"
        );
    }

    fn elf_header(machine: u16, kind: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[16..18].copy_from_slice(&kind.to_le_bytes());
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        bytes
    }

    #[test]
    fn only_the_puae_core_reads_the_plugin() {
        assert!(require_caps_core("puae").is_ok());
        assert!(require_caps_core("puae2021").is_ok());
        assert!(require_caps_core("uae4arm").is_err());
    }
}
