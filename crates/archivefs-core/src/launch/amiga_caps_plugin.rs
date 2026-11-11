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

/// Ok when `capsimg.so` is a non-empty regular file (not a link) in the
/// system folder.
pub fn require_caps_plugin(system_directory: Option<&Path>) -> Result<(), String> {
    let Some(directory) = system_directory else {
        return Err("Amiga IPF games need the CAPS/SPS plugin (capsimg.so) in RetroArch's system folder, and RetroArch's system folder could not be found.".into());
    };
    let plugin = directory.join(CAPS_PLUGIN_FILE);
    match std::fs::symlink_metadata(&plugin) {
        Ok(meta) if meta.file_type().is_file() && meta.len() > 0 => Ok(()),
        _ => Err(format!(
            "Amiga IPF games need the CAPS/SPS plugin (capsimg.so), which is not in RetroArch's system folder ({}).",
            directory.display()
        )),
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
        assert!(require_caps_plugin(Some(dir.path())).is_ok());
    }
}
