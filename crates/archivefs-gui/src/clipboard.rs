//! The GUI's clipboard backend.
//!
//! One trait so every surface that copies text - the Selected panel, the
//! Archive Inspector, the Activity Log, Doctor, RomM config - can be driven
//! by a fake in tests, and one native implementation over `arboard` that
//! reports *why* it is unavailable rather than failing silently. Nothing
//! here decides what gets copied; that stays with each surface.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipboardTextStatus {
    /// Usable, non-empty text is available to paste.
    Ready(String),
    /// The clipboard was reachable and read successfully, but has no
    /// text (empty, or holds a non-text format such as an image).
    Empty,
    /// The clipboard backend could not be reached at all, or the read
    /// itself failed for a reason other than "no text".
    Unavailable(String),
}

/// Clipboard access, isolated behind one small trait so the production
/// path (the real OS clipboard) and tests (a deterministic in-memory
/// stand-in) share every byte of the actual Cut/Copy/Paste logic above
/// it. Neither egui nor eframe exposes a way to read or write the
/// clipboard *synchronously*, at the moment a context-menu item is
/// clicked - `ViewportCommand::RequestPaste`/`RequestCopy`/`RequestCut`
/// are the only public hooks, and they are asynchronous round-trips
/// through eframe's platform backend that (per live testing) cannot be
/// relied on to land back on the field the user actually clicked. Direct
/// clipboard access was explicitly permitted for exactly this case.
pub(crate) trait ClipboardBackend {
    fn get_text_status(&mut self) -> ClipboardTextStatus;
    /// `Err` carries a short, safe error summary - never the text that
    /// failed to be written.
    fn set_text(&mut self, text: String) -> Result<(), String>;
}

pub(crate) fn clipboard_environment_summary() -> String {
    let session_type = match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("x11") => "x11",
        Ok("wayland") => "wayland",
        Ok("tty") => "tty",
        Err(std::env::VarError::NotPresent) => "unset",
        _ => "unknown",
    };
    let display = if std::env::var_os("DISPLAY").is_some() {
        "present"
    } else {
        "absent"
    };
    let wayland_display = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "present"
    } else {
        "absent"
    };
    format!("XDG_SESSION_TYPE={session_type} DISPLAY={display} WAYLAND_DISPLAY={wayland_display}")
}

/// The real OS clipboard via `arboard` - the same crate eframe's own
/// "clipboard" feature already links in transitively (see
/// `egui-winit`'s `Clipboard`, which every keyboard Ctrl+X/C/V in this
/// app already goes through); this only opens a second, independent
/// handle to the identical native mechanism; it does not add a
/// competing implementation. `archivefs-gui`'s own `Cargo.toml` enables
/// arboard's `wayland-data-control` feature explicitly - without it,
/// arboard's Linux backend is unconditionally X11 regardless of session
/// type (confirmed by reading arboard's own source), which is exactly
/// why Paste failed to see text copied from a native-Wayland Firefox on
/// the real Nobara session this was live-tested against.
///
/// Constructed once and kept for the app's entire lifetime (see
/// `ArchiveFsApp::clipboard`), never per-click: on X11, the application
/// that last copied something must keep its clipboard connection alive
/// to serve paste requests from other apps, so recreating and dropping a
/// connection on every click risks silently discarding what was just
/// copied.
///
/// `inner` is `None` if the platform clipboard could not be reached at
/// all (headless environment, no display server, etc.) - `init_error`
/// then holds *why*, so a broken clipboard is never silently reported as
/// merely empty. Every operation safely reports `Unavailable` instead of
/// panicking. Diagnostics (the environment summary, whether init
/// succeeded, or a fixed failure summary) are printed to stderr exactly
/// once at construction. Backend errors remain available to the UI, but
/// arbitrary error text and clipboard content must never enter stderr.
pub(crate) struct NativeClipboard {
    pub(crate) inner: Option<arboard::Clipboard>,
    pub(crate) init_error: Option<String>,
    /// The last `Unavailable` reason reported with a fixed stderr summary, so a
    /// repeated identical failure (e.g. the context menu re-checking
    /// Paste's enabled state on every frame it stays open) is logged
    /// once, not every frame - while a *new* or *changed* failure is
    /// still always visible.
    pub(crate) last_logged_error: Option<String>,
}

impl NativeClipboard {
    pub(crate) fn new() -> Self {
        eprintln!(
            "archivefs-gui: clipboard environment: {}",
            clipboard_environment_summary()
        );
        match arboard::Clipboard::new() {
            Ok(clipboard) => {
                eprintln!("archivefs-gui: clipboard backend initialised");
                Self {
                    inner: Some(clipboard),
                    init_error: None,
                    last_logged_error: None,
                }
            }
            Err(error) => {
                let message = error.to_string();
                eprintln!("archivefs-gui: clipboard backend initialisation failed");
                Self {
                    inner: None,
                    init_error: Some(message),
                    last_logged_error: None,
                }
            }
        }
    }

    /// Logs a fixed summary once per changed backend failure. The raw message
    /// is retained for UI state and deduplication, never interpolated into logs.
    pub(crate) fn log_error_once(&mut self, message: &str) {
        if self.last_logged_error.as_deref() != Some(message) {
            eprintln!("archivefs-gui: clipboard error: backend unavailable");
            self.last_logged_error = Some(message.to_string());
        }
    }
}

impl ClipboardBackend for NativeClipboard {
    fn get_text_status(&mut self) -> ClipboardTextStatus {
        let Some(clipboard) = self.inner.as_mut() else {
            let message = self
                .init_error
                .clone()
                .unwrap_or_else(|| "clipboard backend not initialised".to_string());
            self.log_error_once(&message);
            return ClipboardTextStatus::Unavailable(message);
        };
        match clipboard.get_text() {
            Ok(text) if !text.is_empty() => ClipboardTextStatus::Ready(text),
            Ok(_) => ClipboardTextStatus::Empty,
            // `ContentNotAvailable` is arboard's own way of reporting "the
            // clipboard is empty or holds a non-text format" - a normal,
            // expected outcome, not a failure worth logging.
            Err(arboard::Error::ContentNotAvailable) => ClipboardTextStatus::Empty,
            Err(error) => {
                let message = error.to_string();
                self.log_error_once(&message);
                ClipboardTextStatus::Unavailable(message)
            }
        }
    }

    fn set_text(&mut self, text: String) -> Result<(), String> {
        let Some(clipboard) = self.inner.as_mut() else {
            let message = self
                .init_error
                .clone()
                .unwrap_or_else(|| "clipboard backend not initialised".to_string());
            self.log_error_once(&message);
            return Err(message);
        };
        clipboard.set_text(text).map_err(|error| {
            let message = error.to_string();
            self.log_error_once(&message);
            message
        })
    }
}

pub(crate) fn clipboard_status_label(status: &ClipboardTextStatus) -> String {
    match status {
        ClipboardTextStatus::Ready(_) => "Clipboard ready".to_string(),
        ClipboardTextStatus::Empty => "Clipboard contains no text".to_string(),
        ClipboardTextStatus::Unavailable(reason) => format!("Clipboard unavailable: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_probe() {
        if std::env::var_os("EMUWIZ_CLIPBOARD_DIAGNOSTIC_PROBE").is_none() {
            return;
        }
        let mut clipboard = NativeClipboard {
            inner: None,
            init_error: Some("synthetic-private-backend-detail".into()),
            last_logged_error: None,
        };
        assert!(matches!(
            clipboard.get_text_status(),
            ClipboardTextStatus::Unavailable(reason) if reason == "synthetic-private-backend-detail"
        ));
        assert!(
            clipboard
                .set_text("synthetic-private-clipboard-text".into())
                .is_err_and(|reason| reason == "synthetic-private-backend-detail")
        );
        assert!(clipboard.last_logged_error.is_some());
        eprintln!("{}", clipboard_environment_summary());
    }

    #[test]
    fn diagnostics_do_not_echo_backend_errors_or_unrecognised_session_values() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "clipboard::tests::diagnostic_probe",
                "--nocapture",
            ])
            .env("EMUWIZ_CLIPBOARD_DIAGNOSTIC_PROBE", "1")
            .env("XDG_SESSION_TYPE", "synthetic-private-session")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated clipboard diagnostic probe failed"
        );
        for bytes in [&output.stdout, &output.stderr] {
            let text = String::from_utf8_lossy(bytes);
            for private in [
                "synthetic-private-backend-detail",
                "synthetic-private-clipboard-text",
                "synthetic-private-session",
            ] {
                assert!(
                    !text.contains(private),
                    "clipboard diagnostics exposed private data"
                );
            }
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(stderr.matches("archivefs-gui: clipboard error:").count(), 1);
    }
}
