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
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unset".to_string());
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
/// succeeded, and the exact init error if it failed) are printed to
/// stderr exactly once, at construction - never on every frame, and
/// never including clipboard content.
pub(crate) struct NativeClipboard {
    pub(crate) inner: Option<arboard::Clipboard>,
    pub(crate) init_error: Option<String>,
    /// The last `Unavailable` reason actually printed to stderr, so a
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
                eprintln!("archivefs-gui: clipboard backend initialisation failed: {message}");
                Self {
                    inner: None,
                    init_error: Some(message),
                    last_logged_error: None,
                }
            }
        }
    }

    /// Logs `message` to stderr, but only the first time (or the first
    /// time it changes) - see `last_logged_error`'s doc comment.
    pub(crate) fn log_error_once(&mut self, message: &str) {
        if self.last_logged_error.as_deref() != Some(message) {
            eprintln!("archivefs-gui: clipboard error: {message}");
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
