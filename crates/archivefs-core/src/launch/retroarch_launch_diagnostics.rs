//! Detailed logging for RetroArch launches.
//!
//! Without `--verbose` a RetroArch that fails to start prints one line or
//! nothing, which leaves a failed launch unexplained. With it, RetroArch
//! says what it loaded and what went wrong, with no console window and no
//! change to how the game runs.
//!
//! The flag is added at the very end of a launch, after every identity,
//! content, core and executable check has passed, to the argv that is about
//! to be spawned. The planned and verified command is not altered, nothing
//! is retried, and no setting is passed: the only difference from a quiet
//! launch is the single argument `--verbose`.
//!
//! # What was verified
//!
//! On RetroArch 1.21.0, a launch with `--verbose` leaves `retroarch.cfg`
//! byte-identical to a launch without it, for a clean exit, SIGTERM and
//! SIGKILL, and where RetroArch does rewrite its settings the file written
//! is byte-identical with and without the flag (`log_verbosity` stays as it
//! was). With RetroArch's default settings no log file appeared. Not
//! checked: a RetroArch whose own "log to file" setting is on, which may
//! write the detailed log to its own log folder instead of printing it.
//! `--appendconfig`, by contrast, is written back into the settings file and
//! is never used here. The opt-in test
//! `retroarch_verbose_leaves_settings_untouched` repeats the clean-exit
//! comparison against a real RetroArch; the SIGTERM and SIGKILL comparisons
//! were made by hand.
//!
//! # Limits
//!
//! EmuWiz does not know the RetroArch version at launch time, so the flag is
//! not version-gated. `-v`/`--verbose` is part of RetroArch's command line
//! in every release EmuWiz has been used with; setting
//! [`QUIET_LAUNCH_VARIABLE`] to `1` turns the flag and the wider capture off
//! for anyone who needs the previous behaviour.

use std::ffi::OsString;

use super::process_spawn::OutputCapture;

/// The one argument a diagnostic launch adds.
pub const VERBOSE_FLAG: &str = "--verbose";

/// Set to `1` in EmuWiz's environment to launch RetroArch exactly as before:
/// no `--verbose`, standard output discarded, only the start of standard
/// error kept.
pub const QUIET_LAUNCH_VARIABLE: &str = "EMUWIZ_RETROARCH_QUIET_LAUNCH";

fn quiet_launch_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

/// Ask RetroArch for detailed logging by inserting [`VERBOSE_FLAG`]
/// immediately before its `-L` argument. Returns whether the argv now asks
/// for it.
///
/// `-L` is always RetroArch's own argument, so the flag lands inside
/// RetroArch's part of the command even when a wrapper (`flatpak run ...`)
/// comes first. If RetroArch's arguments cannot be located unambiguously
/// (no `-L`, or more than one) nothing is changed: a diagnostic must never
/// be the reason a launch behaves differently.
///
/// A command that already holds `--verbose` or `-v` anywhere is left as it
/// is, so the flag is never given twice. Where that flag belongs to a
/// wrapper and not to RetroArch, RetroArch is not asked for detail and the
/// capture simply holds less.
pub fn request_verbose_logging(arguments: &mut Vec<OsString>) -> bool {
    let mut core_flags = arguments
        .iter()
        .enumerate()
        .filter(|(_, argument)| *argument == "-L");
    let (Some((index, _)), None) = (core_flags.next(), core_flags.next()) else {
        return false;
    };
    let already = arguments
        .iter()
        .any(|argument| argument == VERBOSE_FLAG || argument == "-v");
    if !already {
        arguments.insert(index, OsString::from(VERBOSE_FLAG));
    }
    true
}

/// Decide how one RetroArch launch is logged and captured, adjusting the
/// argv that is about to be spawned. Diagnostic capture is used only when
/// RetroArch was actually asked for detailed logging.
pub(crate) fn prepare_launch(arguments: &mut Vec<OsString>) -> OutputCapture {
    prepare_launch_with(
        arguments,
        std::env::var_os(QUIET_LAUNCH_VARIABLE).as_deref(),
    )
}

fn prepare_launch_with(
    arguments: &mut Vec<OsString>,
    quiet_variable: Option<&std::ffi::OsStr>,
) -> OutputCapture {
    if !quiet_launch_requested(quiet_variable) && request_verbose_logging(arguments) {
        OutputCapture::Diagnostic
    } else {
        OutputCapture::StderrStart
    }
}

#[cfg(test)]
#[path = "retroarch_launch_diagnostics/tests.rs"]
mod tests;
