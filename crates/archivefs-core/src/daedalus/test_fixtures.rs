//! Real captured output used by the tests. See
//! `tests/fixtures/daedalus/README.md` for how each file was produced.

macro_rules! fixture {
    ($name:ident, $path:literal) => {
        pub(crate) const $name: &str =
            include_str!(concat!("../../tests/fixtures/daedalus/", $path));
    };
}

fixture!(RA_VULKAN_NO_DRIVER, "retroarch-1.21.0/vk-noicd.log");
fixture!(
    RA_VULKAN_NO_DRIVER_QUIET,
    "retroarch-1.21.0/vk-noicd-quiet.stderr"
);
fixture!(RA_VULKAN_OK, "retroarch-1.21.0/vulkan-ok.log");
fixture!(RA_GL_OK, "retroarch-1.21.0/gl-ok.log");
fixture!(RA_CORE_IGNORED, "retroarch-1.21.0/core-ignored.log");
fixture!(
    RA_CORE_NOT_A_LIBRARY,
    "retroarch-1.21.0/core-not-a-library.log"
);
fixture!(RA_CONTENT_MISSING, "retroarch-1.21.0/content-missing.log");
fixture!(
    RA_CONTENT_UNREADABLE,
    "retroarch-1.21.0/content-unreadable.log"
);
fixture!(RA_CONTENT_GARBAGE, "retroarch-1.21.0/content-garbage.log");
fixture!(
    RA_SAVE_DIR_READ_ONLY,
    "retroarch-1.21.0/save-dir-read-only.log"
);
fixture!(
    RA_SILENT_CORE_AUDIO,
    "retroarch-1.21.0/silent-core-audio.log"
);
fixture!(RA_NO_DISPLAY, "retroarch-1.21.0/no-display.stderr");
fixture!(HOST_LOADER, "host/ldso.stderr");
fixture!(HOST_FLATPAK, "host/flatpak.stderr");
fixture!(HOST_BWRAP, "host/bwrap.stderr");
