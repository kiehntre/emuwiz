//! Pure, read-only structural evidence for DOSBox configuration files.
//!
//! # A file named `dosbox.conf` is not evidence
//!
//! The DOS platform registry carries a layout rule that used to fire on the
//! mere *presence* of a file called `dosbox.conf` next to a game. A file
//! name proves nothing: anyone can drop an empty or unrelated file with
//! that name. This module replaces that name-only trust with a bounded
//! structural check of the file's actual contents.
//!
//! # What counts as corroborating DOS evidence
//!
//! | Observation                                         | DOS evidence |
//! |----------------------------------------------------|--------------|
//! | file named `dosbox.conf`, contents not checked      | none         |
//! | contents do not parse as a DOSBox config            | none         |
//! | well-formed config, but no `[autoexec]` section     | none         |
//! | well-formed config **with a real `[autoexec]`**     | corroborating |
//!
//! DOSBox only ever runs DOS software, so a genuine DOSBox configuration
//! with an `[autoexec]` block is honest *corroboration* that a folder is a
//! DOS game - never proof on its own, and never a title/release identity.
//! It is emitted as a single `Corroborated` [`ContentEvidenceKind::BootStructure`]
//! fact ([`DOSBOX_CONFIG_AUTOEXEC`]); [`crate::platform_evidence_fusion`]
//! treats it as a DOS *candidate* leg, not a resolver.
//!
//! # Format verified, not assumed
//!
//! The `.conf` syntax is cross-checked against two independent sources:
//!
//! - the DOSBox project wiki, "Dosbox.conf"
//!   (<https://www.dosbox.com/wiki/Dosbox.conf>), and
//! - the DOSBox Staging manual, "Configuration"
//!   (<https://www.dosbox-staging.org/0.83/manual/using-dosbox-staging/configuration/>).
//!
//! Both agree: sections are `[name]` on their own line; option lines are
//! `key = value`; `#` starts a comment (Staging also documents `#` as the
//! comment character, classic DOSBox strips from `#` to end of line); the
//! `[autoexec]` section is a freeform block of raw DOS command lines rather
//! than settings, and is conventionally the last section. Classic DOSBox
//! matches section names case-insensitively. This module additionally
//! tolerates a leading `;` comment (ubiquitous in INI-family files) and
//! trims surrounding whitespace on headers.
//!
//! # Bounded, and never executed
//!
//! At most [`MAX_DOSBOX_CONFIG_BYTES`] are read; a longer line than
//! [`MAX_DOSBOX_CONFIG_LINE_BYTES`] or a NUL / control-heavy body is
//! refused. The `[autoexec]` commands are **counted, never parsed**: no
//! `mount`, `imgmount`, `boot`, `call`, executable name, drive letter or
//! path is interpreted, and nothing here influences rename authority -
//! exact identity stays DAT/hash-driven.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};
use crate::safe_read::{TrustedRoots, open_bounded_read};

/// The most of a `.conf` this module will read. A real DOSBox config is a
/// few kilobytes; this is generous headroom, not a claim about any file.
pub const MAX_DOSBOX_CONFIG_BYTES: usize = 64 * 1024;

/// A single line longer than this makes the structure unreliable (a binary
/// blob with no newlines, a pathological generated file) and is refused.
pub const MAX_DOSBOX_CONFIG_LINE_BYTES: usize = 4 * 1024;

/// Lines scanned past this point are ignored - the structure is long
/// settled by then. Not an error.
pub const MAX_DOSBOX_CONFIG_LINES: usize = 8 * 1024;

/// Distinct section names tracked; more than this and the extras are simply
/// not recorded.
pub const MAX_TRACKED_SECTIONS: usize = 64;

/// `[autoexec]` command lines counted; the exact count past this does not
/// matter to structure recognition.
pub const MAX_TRACKED_AUTOEXEC_LINES: usize = 512;

/// The neutral [`ContentEvidence::value`] emitted for a verified DOSBox
/// config that carries a real `[autoexec]` section. Shared verbatim with
/// [`crate::platform_evidence_fusion`] and [`crate::content_evidence_scope`].
pub const DOSBOX_CONFIG_AUTOEXEC: &str = "DOSBox config with [autoexec] section";

/// Why a candidate `.conf` was not accepted as a structural DOSBox config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DosboxConfigRefusal {
    /// Zero bytes.
    Empty,
    /// Larger than [`MAX_DOSBOX_CONFIG_BYTES`].
    TooLarge { length: usize, maximum: usize },
    /// A line exceeded [`MAX_DOSBOX_CONFIG_LINE_BYTES`].
    LineTooLong { length: usize, maximum: usize },
    /// A NUL byte, or a control-character-heavy body: not text.
    BinaryContent,
    /// Not valid UTF-8 (this crate's text contract).
    InvalidUtf8,
    /// A `[` line with no closing `]`, an empty `[]`, or trailing junk after
    /// `]` that is not a comment.
    MalformedSection { detail: String },
    /// Parsed as text but contains no `[section]` header at all.
    NoSection,
}

impl DosboxConfigRefusal {
    pub fn detail(&self) -> String {
        match self {
            Self::Empty => "the file is empty".to_string(),
            Self::TooLarge { length, maximum } => {
                format!("{length} bytes is past the {maximum}-byte inspection limit")
            }
            Self::LineTooLong { length, maximum } => {
                format!("a {length}-byte line is past the {maximum}-byte line limit")
            }
            Self::BinaryContent => "the file is binary or control-character heavy".to_string(),
            Self::InvalidUtf8 => "the file is not valid UTF-8".to_string(),
            Self::MalformedSection { detail } => format!("malformed section header: {detail}"),
            Self::NoSection => "no `[section]` header anywhere in the file".to_string(),
        }
    }
}

/// What a structurally valid DOSBox config declared. Every field is a
/// bounded, counted observation - never an interpreted command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DosboxConfigFact {
    /// Distinct section names, lowercased, in first-seen order (bounded by
    /// [`MAX_TRACKED_SECTIONS`]).
    pub sections: Vec<String>,
    /// Whether a well-formed `[autoexec]` header was seen.
    pub has_autoexec: bool,
    /// Whether a `[dosbox]` header was seen (corroboration only).
    pub has_dosbox_section: bool,
    /// Non-blank, non-comment lines inside `[autoexec]` (bounded count).
    /// Their text is never parsed.
    pub autoexec_command_lines: usize,
    /// `key = value` option lines seen outside `[autoexec]` (bounded count).
    pub option_lines: usize,
}

impl DosboxConfigFact {
    /// Whether this is a DOSBox config that genuinely corroborates a DOS
    /// game folder: it has a real `[autoexec]` section and at least one
    /// other sign of a real config (a `[dosbox]` section, an actual
    /// autoexec command, or an option line) rather than a lone `[autoexec]`
    /// header with nothing under or around it.
    pub fn is_verified_dos_layout(&self) -> bool {
        self.has_autoexec
            && (self.has_dosbox_section || self.autoexec_command_lines > 0 || self.option_lines > 0)
    }
}

fn is_comment(trimmed: &str) -> bool {
    trimmed.starts_with('#') || trimmed.starts_with(';')
}

/// Parses `bytes` as a DOSBox configuration file, structurally only.
///
/// Pure: `bytes` is the sole input. Recognises section headers
/// (case-insensitively), counts `[autoexec]` command lines and `key = value`
/// option lines, and ignores blank lines and `#` / `;` comments. Fails
/// closed on an empty / oversized / binary / non-UTF-8 body, an
/// over-long line, a malformed `[section` header, or a file with no section
/// at all.
pub fn parse_dosbox_config(bytes: &[u8]) -> Result<DosboxConfigFact, DosboxConfigRefusal> {
    if bytes.is_empty() {
        return Err(DosboxConfigRefusal::Empty);
    }
    if bytes.len() > MAX_DOSBOX_CONFIG_BYTES {
        return Err(DosboxConfigRefusal::TooLarge {
            length: bytes.len(),
            maximum: MAX_DOSBOX_CONFIG_BYTES,
        });
    }
    if bytes.contains(&0) {
        return Err(DosboxConfigRefusal::BinaryContent);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| DosboxConfigRefusal::InvalidUtf8)?;
    // Control-character-heavy bodies are not configuration text. Tab, CR and
    // LF are the only control characters a `.conf` legitimately contains.
    let control = text
        .bytes()
        .filter(|byte| byte.is_ascii_control() && !matches!(byte, b'\t' | b'\r' | b'\n'))
        .count();
    if control.saturating_mul(20) > text.len() {
        return Err(DosboxConfigRefusal::BinaryContent);
    }

    let mut fact = DosboxConfigFact::default();
    let mut in_autoexec = false;
    let mut seen_section = false;

    for (index, raw_line) in text.split('\n').enumerate() {
        if index >= MAX_DOSBOX_CONFIG_LINES {
            break;
        }
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.len() > MAX_DOSBOX_CONFIG_LINE_BYTES {
            return Err(DosboxConfigRefusal::LineTooLong {
                length: line.len(),
                maximum: MAX_DOSBOX_CONFIG_LINE_BYTES,
            });
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || is_comment(trimmed) {
            continue;
        }

        if trimmed.starts_with('[') {
            let Some(close) = trimmed.find(']') else {
                return Err(DosboxConfigRefusal::MalformedSection {
                    detail: "no closing `]`".to_string(),
                });
            };
            let after = trimmed[close + 1..].trim();
            if !after.is_empty() && !is_comment(after) {
                return Err(DosboxConfigRefusal::MalformedSection {
                    detail: "text after `]`".to_string(),
                });
            }
            let name = trimmed[1..close].trim().to_ascii_lowercase();
            if name.is_empty() {
                return Err(DosboxConfigRefusal::MalformedSection {
                    detail: "empty `[]`".to_string(),
                });
            }
            seen_section = true;
            if !fact.sections.iter().any(|existing| existing == &name)
                && fact.sections.len() < MAX_TRACKED_SECTIONS
            {
                fact.sections.push(name.clone());
            }
            in_autoexec = name == "autoexec";
            if in_autoexec {
                fact.has_autoexec = true;
            } else if name == "dosbox" {
                fact.has_dosbox_section = true;
            }
            continue;
        }

        // A content line before any section: DOSBox ignores it; so do we.
        if !seen_section {
            continue;
        }
        if in_autoexec {
            if fact.autoexec_command_lines < MAX_TRACKED_AUTOEXEC_LINES {
                fact.autoexec_command_lines += 1;
            }
        } else if line.contains('=') {
            fact.option_lines = fact.option_lines.saturating_add(1);
        }
    }

    if !seen_section {
        return Err(DosboxConfigRefusal::NoSection);
    }
    Ok(fact)
}

/// The result of inspecting a `.conf` file on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct DosboxConfigInspection {
    /// The parsed config, when it validated structurally.
    pub fact: Option<DosboxConfigFact>,
    /// Why nothing was concluded, when nothing was.
    pub refusal: Option<DosboxConfigRefusal>,
    pub bytes_inspected: usize,
    pub read_via_symlink: bool,
}

impl DosboxConfigInspection {
    fn refused(refusal: DosboxConfigRefusal, bytes_inspected: usize) -> Self {
        Self {
            fact: None,
            refusal: Some(refusal),
            bytes_inspected,
            read_via_symlink: false,
        }
    }

    /// Whether the file is a verified DOSBox config with a real `[autoexec]`.
    pub fn is_verified_dos_layout(&self) -> bool {
        self.fact
            .as_ref()
            .is_some_and(DosboxConfigFact::is_verified_dos_layout)
    }
}

/// Reads `path` under the caller's trusted-root policy and parses it as a
/// DOSBox config, bounded to [`MAX_DOSBOX_CONFIG_BYTES`]. Never executes or
/// interprets anything in the file.
pub fn inspect_dosbox_config(
    path: &Path,
    trusted: &TrustedRoots,
    cancel: Option<&AtomicBool>,
) -> DosboxConfigInspection {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return DosboxConfigInspection::refused(DosboxConfigRefusal::Empty, 0);
    }
    let mut file = match open_bounded_read(path, trusted) {
        Ok(file) => file,
        Err(_) => return DosboxConfigInspection::refused(DosboxConfigRefusal::Empty, 0),
    };
    let read_via_symlink = file.resolved_via_symlink();
    let length = file.len();
    if length == 0 {
        let mut refused = DosboxConfigInspection::refused(DosboxConfigRefusal::Empty, 0);
        refused.read_via_symlink = read_via_symlink;
        return refused;
    }
    if length > MAX_DOSBOX_CONFIG_BYTES as u64 {
        let mut refused = DosboxConfigInspection::refused(
            DosboxConfigRefusal::TooLarge {
                length: usize::try_from(length).unwrap_or(usize::MAX),
                maximum: MAX_DOSBOX_CONFIG_BYTES,
            },
            0,
        );
        refused.read_via_symlink = read_via_symlink;
        return refused;
    }
    let want = length as usize;
    let Some(bytes) = file.read_exact_at(0, want, MAX_DOSBOX_CONFIG_BYTES) else {
        let mut refused = DosboxConfigInspection::refused(DosboxConfigRefusal::BinaryContent, 0);
        refused.read_via_symlink = read_via_symlink;
        return refused;
    };

    match parse_dosbox_config(&bytes) {
        Ok(fact) => DosboxConfigInspection {
            fact: Some(fact),
            refusal: None,
            bytes_inspected: bytes.len(),
            read_via_symlink,
        },
        Err(refusal) => {
            let mut refused = DosboxConfigInspection::refused(refusal, bytes.len());
            refused.read_via_symlink = read_via_symlink;
            refused
        }
    }
}

/// Neutral evidence: one `Corroborated` [`ContentEvidenceKind::BootStructure`]
/// fact ([`DOSBOX_CONFIG_AUTOEXEC`]) for a verified DOSBox config that
/// carries a real `[autoexec]` section - and nothing at all otherwise, no
/// matter how well-formed a config without `[autoexec]` is. Never emits a
/// title, a path, or an executable name.
pub fn observe_dosbox_config_evidence(fact: &DosboxConfigFact) -> Vec<ContentEvidence> {
    if !fact.is_verified_dos_layout() {
        return Vec::new();
    }
    vec![ContentEvidence::new(
        ContentEvidenceKind::BootStructure,
        DOSBOX_CONFIG_AUTOEXEC,
        ContentEvidenceConfidence::Corroborated,
        "a structurally valid DOSBox configuration with a real [autoexec] section - \
         corroborating DOS layout evidence, never a resolver and never a release identity",
    )]
}

#[cfg(test)]
mod tests;
