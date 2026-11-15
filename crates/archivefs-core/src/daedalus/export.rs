//! Detailed diagnostics as text, for a person who asks for them.
//!
//! The launch history ([`super::history`]) keeps structured summaries only.
//! The detail of a failed launch (what the emulator printed, and the
//! evidence behind each finding) exists in memory after the launch. It is
//! shown, or written to a file, only when a person asks. This module
//! prepares that text.
//!
//! # What is promised, and what is not
//!
//! The text goes through [`Redactor`], which removes the private details
//! it recognises, and through the sanitiser, which makes every character
//! visible. That is all. Emulator output is free text and can hold private
//! details in forms no rule recognises, so:
//!
//! - the text begins with [`PRIVACY_WARNING`], which says so in plain
//!   words;
//! - [`ExportPreview::text`] is exactly what would be written, byte for
//!   byte, so the person can read it first;
//! - nothing can be written without [`ExportPreview::confirmed_by_person`],
//!   which exists so that the one place that calls it is the handler of a
//!   person's explicit confirmation;
//! - the only thing this module can do with the text is create one new,
//!   private file in a folder the caller names. It replaces nothing, sends
//!   nothing and starts nothing.

use std::ffi::OsStr;
use std::path::Path;
use std::time::SystemTime;

use super::history;
use super::redact::Redactor;
use super::render;
use super::rules::DiagnosisReport;
use super::text;
use crate::patch_manager::anchored_fs::DirectoryHandle;

/// Shown with the preview and written at the top of every export.
pub const PRIVACY_WARNING: &str = "\
PRIVACY: read this before you share this file.
This file holds what the emulator printed when the launch failed, and the
details behind the diagnosis. EmuWiz removed what it recognises: your home
folder and user name in paths, the values next to a short list of labels
(such as password, token or api_key), and logins inside web addresses.
It cannot recognise everything. Emulator output is free text and may still
contain names, folder names, network addresses, account details, or
passwords and keys written in a way EmuWiz does not know.
Read all of it, and remove anything you would not want others to see.
EmuWiz has not sent this anywhere and will not.";

/// Most captured output an export holds, after redaction.
pub const EXPORT_OUTPUT_LIMIT: usize = 192 * 1024;

const OUTPUT_HEADING: &str = "Emulator output (as captured; recognised private details removed)";

/// Why an export was not written.
#[derive(Debug, PartialEq, Eq)]
pub enum ExportError {
    /// The file name is not one plain name.
    InvalidName,
    /// A file, link or folder with that name is already there. Nothing was
    /// replaced.
    AlreadyExists,
    /// The folder could not be used or the file could not be written.
    Io(std::io::ErrorKind),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName => f.write_str("that is not a plain file name"),
            Self::AlreadyExists => {
                f.write_str("something with that name already exists; nothing was replaced")
            }
            Self::Io(kind) => write!(f, "the file could not be written: {kind}"),
        }
    }
}

impl std::error::Error for ExportError {}

/// The text of an export, for a person to read before deciding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPreview {
    text: String,
    output_cut: bool,
}

/// An export a person has read the preview of and confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmedExport {
    text: String,
}

/// One plain file name: no folder, no hidden file, nothing that is not
/// shown as itself.
fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && text::sanitise(name, 200) == name
}

impl ExportPreview {
    /// Prepare the text. Reads nothing and writes nothing.
    ///
    /// `output` is the bounded output the launcher captured, when there is
    /// any. It is treated as untrusted text throughout: redacted, made
    /// visible, cut at [`EXPORT_OUTPUT_LIMIT`], and only ever placed below
    /// a heading that says what it is.
    pub fn prepare(
        report: &DiagnosisReport,
        output: Option<&[u8]>,
        redactor: &Redactor,
        now: SystemTime,
    ) -> Self {
        let when = now
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let mut text = format!(
            "EmuWiz detailed launch diagnostics\nPrepared {} because someone asked for it.\n\n\
             {PRIVACY_WARNING}\n\n",
            history::readable_time(when)
        );
        text.push_str(&render::report_text(&redactor.report(report), "Diagnosis"));
        let mut output_cut = false;
        text.push_str(&format!("\n{OUTPUT_HEADING}\n"));
        match output {
            None => text.push_str("  (none was captured)\n"),
            Some(bytes) => {
                let redacted = redactor.text(&String::from_utf8_lossy(bytes));
                let mut kept = 0usize;
                for line in redacted.lines() {
                    let line = text::sanitise(line, text::LOG_LINE_LIMIT);
                    if kept + line.len() + 1 > EXPORT_OUTPUT_LIMIT {
                        output_cut = true;
                        break;
                    }
                    kept += line.len() + 1;
                    // Indented, so no line of output can pass for a heading
                    // or for part of the warning above.
                    text.push_str("  | ");
                    text.push_str(&line);
                    text.push('\n');
                }
                if output_cut {
                    text.push_str("  (the rest of the output was left out)\n");
                }
            }
        }
        Self { text, output_cut }
    }

    /// Exactly what would be written.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The warning to show beside the preview.
    pub fn warning(&self) -> &'static str {
        PRIVACY_WARNING
    }

    /// Some captured output did not fit and was left out.
    pub fn output_cut(&self) -> bool {
        self.output_cut
    }

    /// A name for the file: fixed words and the time, nothing else.
    pub fn suggested_file_name(now: SystemTime) -> String {
        let when = now
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let stamp: String = history::readable_time(when)
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        format!("emuwiz-launch-diagnostics-{stamp}.txt")
    }

    /// Call this from the handler of a person's explicit confirmation,
    /// after the preview and the warning were on screen, and from nowhere
    /// else. Nothing can be written without it.
    pub fn confirmed_by_person(self) -> ConfirmedExport {
        ConfirmedExport { text: self.text }
    }
}

impl ConfirmedExport {
    /// The text that will be written: the preview, unchanged.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Create `file_name` in `directory` holding the text, readable by this
    /// user only. The file is complete or absent, never partial. Anything
    /// already at that name, a link included, is left as it is.
    pub fn write_new(&self, directory: &Path, file_name: &str) -> Result<(), ExportError> {
        if !is_plain_file_name(file_name) {
            return Err(ExportError::InvalidName);
        }
        let directory =
            DirectoryHandle::open(directory).map_err(|error| ExportError::Io(error.kind()))?;
        match directory.lstat(OsStr::new(file_name)) {
            Ok(None) => {}
            Ok(Some(_)) => return Err(ExportError::AlreadyExists),
            Err(error) => return Err(ExportError::Io(error.kind())),
        }
        history::publish(&directory, file_name, self.text.as_bytes()).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                ExportError::AlreadyExists
            } else {
                ExportError::Io(error.kind())
            }
        })
    }
}

#[cfg(test)]
#[path = "export/tests.rs"]
mod tests;
