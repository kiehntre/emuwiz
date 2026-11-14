//! RetroArch: log signatures and settings evidence.
//!
//! Signatures were captured from RetroArch 1.21.0 (Linux, built
//! 2025-05-28) on 2026-11-13, by running it against deliberately broken
//! setups in a private display with disposable settings. Other versions
//! may word things differently; a line that does not match exactly is not
//! an event, and the rules then say they lack log evidence rather than
//! guessing.
//!
//! Two properties of RetroArch shape what can be concluded:
//!
//! - It logs to standard error, and without `--verbose` it prints almost
//!   nothing: a failed start usually leaves one `Fatal error received in`
//!   line or no line at all. [`LogCoverage::verbose`] records which case a
//!   scan saw.
//! - Lines a core prints are prefixed `[libretro ...]` and are never
//!   matched here. A core can still print a line break followed by text
//!   that looks like RetroArch's own, which is why every event is
//!   untrusted text.

use std::path::Path;

use super::evidence::{
    ConfigLayer, EvidenceArea, EvidenceId, EvidenceSet, EvidenceState, Fact, LogMeaning, LogStream,
    SourceFingerprint, Trust,
};
use super::signatures::{self, Matcher, Signature};
use super::text;
use crate::emulator_environment::retroarch::{MAX_CONFIG_BYTES, config_key_occurrences};

pub const EMULATOR: &str = "retroarch";

/// The function RetroArch names when its video driver cannot start.
pub const VIDEO_INIT_FUNCTION: &str = "video_driver_init_internal()";

pub const SIGNATURES: &[Signature] = &[
    Signature {
        id: "retroarch.vulkan.instance_failed",
        version: 1,
        meaning: LogMeaning::VulkanUnavailable,
        matcher: Matcher::Exact("[ERROR] Failed to create Vulkan instance."),
    },
    Signature {
        id: "retroarch.vulkan.context_failed",
        version: 1,
        meaning: LogMeaning::VulkanUnavailable,
        matcher: Matcher::Exact("[ERROR] [Vulkan]: Failed to create Vulkan context."),
    },
    Signature {
        id: "retroarch.video.driver_open_failed",
        version: 1,
        meaning: LogMeaning::VideoDriverInitFailed,
        matcher: Matcher::Exact("[ERROR] [Video]: Cannot open video driver.. Exiting.."),
    },
    // Printed with and without `--verbose`; the only line a non-verbose
    // run leaves when the video driver fails. It names where RetroArch
    // gave up, not why.
    Signature {
        id: "retroarch.fatal.video_driver_init",
        version: 1,
        meaning: LogMeaning::VideoDriverInitFailed,
        matcher: Matcher::Exact("Fatal error received in: \"video_driver_init_internal()\""),
    },
    Signature {
        id: "retroarch.fatal.video_driver_init_logged",
        version: 1,
        meaning: LogMeaning::VideoDriverInitFailed,
        matcher: Matcher::Exact(
            "[ERROR] Fatal error received in: \"video_driver_init_internal()\"",
        ),
    },
    // Any other fatal error: recorded and shown, never used as a cause.
    Signature {
        id: "retroarch.fatal.other",
        version: 1,
        meaning: LogMeaning::FatalErrorUnspecific,
        matcher: Matcher::PrefixQuoted("Fatal error received in: "),
    },
    Signature {
        id: "retroarch.fatal.other_logged",
        version: 1,
        meaning: LogMeaning::FatalErrorUnspecific,
        matcher: Matcher::PrefixQuoted("[ERROR] Fatal error received in: "),
    },
    Signature {
        id: "retroarch.video.driver_ready",
        version: 1,
        meaning: LogMeaning::VideoDriverReady,
        matcher: Matcher::PrefixQuoted("[INFO] [Display]: Found display driver: "),
    },
    Signature {
        id: "retroarch.vulkan.device_selected",
        version: 1,
        meaning: LogMeaning::VulkanDeviceSelected,
        matcher: Matcher::PrefixQuoted("[INFO] [Vulkan]: Using GPU: "),
    },
    Signature {
        id: "retroarch.gl.renderer",
        version: 1,
        meaning: LogMeaning::OpenGlRendererReported,
        matcher: Matcher::PrefixRest("[INFO] [GL]: Vendor: "),
    },
    Signature {
        id: "retroarch.core.load_started",
        version: 1,
        meaning: LogMeaning::CoreLoadStarted,
        matcher: Matcher::PrefixQuoted("[INFO] [Core]: Loading dynamic libretro core from: "),
    },
    // RetroArch then carries on with no core and can still exit with
    // status 0.
    Signature {
        id: "retroarch.core.request_ignored",
        version: 1,
        meaning: LogMeaning::CoreRequestIgnored,
        matcher: Matcher::PrefixQuoted("[WARN] --libretro argument "),
    },
    Signature {
        id: "retroarch.core.open_failed",
        version: 1,
        meaning: LogMeaning::CoreOpenFailed,
        matcher: Matcher::PrefixQuoted("[ERROR] Failed to open libretro core: "),
    },
    Signature {
        id: "retroarch.core.output_reported",
        version: 1,
        meaning: LogMeaning::CoreReportedOutput,
        matcher: Matcher::PrefixBetween {
            prefix: "[INFO] [Core]: Geometry: ",
            after: "Sample rate: ",
            until: " Hz.",
        },
    },
    Signature {
        id: "retroarch.content.load_started",
        version: 1,
        meaning: LogMeaning::ContentLoadStarted,
        matcher: Matcher::PrefixQuoted("[INFO] [Content]: Loading content file: "),
    },
    // The same line is printed for a missing file and for one that exists
    // but cannot be read; only a look at the file tells them apart.
    Signature {
        id: "retroarch.content.read_failed",
        version: 1,
        meaning: LogMeaning::ContentReadFailed,
        matcher: Matcher::PrefixQuoted("[ERROR] [Content]: Could not read content file "),
    },
    Signature {
        id: "retroarch.content.rejected",
        version: 1,
        meaning: LogMeaning::ContentRejected,
        matcher: Matcher::Exact("[ERROR] [Content]: Failed to load content"),
    },
    Signature {
        id: "retroarch.content.ran_for",
        version: 1,
        meaning: LogMeaning::ContentRanFor,
        matcher: Matcher::PrefixRest("[INFO] [Core]: Content ran for a total of: "),
    },
    // Also printed on healthy runs of cores that produce no sound, so on
    // its own it is not a fault.
    Signature {
        id: "retroarch.audio.driver_failed",
        version: 1,
        meaning: LogMeaning::AudioDriverInitFailed,
        matcher: Matcher::Exact(
            "[ERROR] Failed to initialize audio driver. Will continue without audio.",
        ),
    },
    Signature {
        id: "retroarch.save.write_failed",
        version: 1,
        meaning: LogMeaning::SaveWriteFailed,
        matcher: Matcher::Exact("[ERROR] [SRAM]: Failed to save SRAM."),
    },
];

/// Settings read for diagnosis. Nothing else in the file is recorded.
pub const DIAGNOSTIC_KEYS: &[&str] = &[
    "video_driver",
    "audio_driver",
    "input_driver",
    "input_joypad_driver",
    "savefile_directory",
    "savestate_directory",
    "system_directory",
    "libretro_directory",
];

/// Scan RetroArch output and add it to the set. Returns the event ids.
pub fn record_log(set: &mut EvidenceSet, bytes: &[u8], stream: LogStream) -> Vec<EvidenceId> {
    let examined = text::examine(bytes);
    let mut scan = signatures::scan(&examined, SIGNATURES, EMULATOR, stream);
    scan.coverage.verbose = examined
        .lines()
        .any(|line| line.text.starts_with("[INFO] "));
    let source = match stream {
        LogStream::LogFile => "RetroArch log file",
        LogStream::Stderr => "standard error of the launched process",
    };
    signatures::record_scan(set, scan, source)
}

/// Whole seconds from RetroArch's `00 hours, 00 minutes, 04 seconds.`
pub fn ran_for_seconds(value: &str) -> Option<u64> {
    let mut parts = value.trim_end_matches('.').split(", ");
    let mut field =
        |unit: &str| -> Option<u64> { parts.next()?.strip_suffix(unit)?.trim().parse().ok() };
    let hours = field(" hours")?;
    let minutes = field(" minutes")?;
    let seconds = field(" seconds")?;
    hours
        .checked_mul(3600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)
}

/// Read the diagnostic settings from one RetroArch settings file.
///
/// The file is read once, without following a final symbolic link, and
/// every item carries a fingerprint of exactly the bytes that were parsed,
/// so a later change to the file is detectable
/// ([`EvidenceSet::refresh_freshness`]). Each key in
/// [`DIAGNOSTIC_KEYS`] yields one item whether or not it is set. A file
/// that cannot be read yields one `Unavailable` item and no values.
pub fn record_config(set: &mut EvidenceSet, path: &Path, layer: ConfigLayer) -> Vec<EvidenceId> {
    let source = format!("RetroArch settings file {}", path.display());
    let (bytes, fingerprint) =
        match SourceFingerprint::read_with_digest(path, MAX_CONFIG_BYTES as u64) {
            Ok(read) => read,
            Err(error) => {
                return set
                    .unavailable(
                        EvidenceArea::EmulatorConfiguration,
                        source,
                        format!("could not be read: {}", error.kind()),
                    )
                    .into_iter()
                    .collect();
            }
        };
    let Ok(contents) = std::str::from_utf8(&bytes) else {
        return set
            .unavailable(
                EvidenceArea::EmulatorConfiguration,
                source,
                "is not valid UTF-8 text",
            )
            .into_iter()
            .collect();
    };
    let found = config_key_occurrences(contents, DIAGNOSTIC_KEYS);
    let mut ids = Vec::new();
    let mut repeated_keys = Vec::new();
    for key in DIAGNOSTIC_KEYS {
        let mut occurrences = found
            .occurrences
            .iter()
            .filter(|(_, found_key, _)| found_key == key);
        // RetroArch honours the first occurrence of a repeated key.
        let first = occurrences.next();
        if occurrences.next().is_some() {
            repeated_keys.push((*key).to_string());
        }
        ids.extend(set.push(
            EvidenceArea::EmulatorConfiguration,
            EvidenceState::Observed,
            Trust::Measured,
            source.clone(),
            Some(fingerprint.clone()),
            Fact::ConfigValue {
                emulator: EMULATOR.to_string(),
                layer,
                key: (*key).to_string(),
                value: first.map(|(_, _, value)| text::sanitise(value, text::PATH_LIMIT)),
                line: first.map(|(line, _, _)| *line),
            },
        ));
    }
    ids.extend(set.push(
        EvidenceArea::EmulatorConfiguration,
        EvidenceState::Observed,
        Trust::Measured,
        source,
        Some(fingerprint),
        Fact::ConfigStructure {
            emulator: EMULATOR.to_string(),
            layer,
            includes: found.include_lines.len() as u32,
            malformed_lines: found.malformed_lines.len() as u32,
            repeated_keys,
        },
    ));
    ids
}

#[cfg(test)]
#[path = "retroarch/tests.rs"]
mod tests;
