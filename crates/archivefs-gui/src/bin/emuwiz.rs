//! Thin launcher for the `emuwiz` executable.
//!
//! All three shipped GUI binaries are identical and exist only so existing
//! launchers keep working during the EmuWiz rename. The GUI itself lives in
//! the `archivefs_gui` library, so it compiles once rather than once per
//! binary name.

fn main() -> eframe::Result<()> {
    archivefs_gui::run()
}
