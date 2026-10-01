//! Full-fidelity, panic-free parsing and deterministic rendering of
//! RetroArch/libretro `.cht` cheat files.
//!
//! ## Metadata readers and installation
//!
//! Catalogue and inventory adapters project this parser's bounded evidence
//! into metadata; code bodies are retained only in this document, never in
//! their reports. Selection and installation use the same field validation.
//!
//! ## Guarantees
//!
//! - **Never panics on catalogue input.** Every field is bounded, every
//!   index is checked, and no slicing is done on a non-char boundary.
//! - **Never mutates the source.** Parsing is a pure function of bytes.
//! - **Deterministic rendering.** [`render_cht_file`] is a pure function of
//!   its input slice: same entries in, byte-identical file out.
//! - **No uncertain code repairs.** A parsed value containing a double
//!   quote is reported as [`ChtEntryWarningKind::QuoteNormalized`] and the
//!   whole entry is made unselectable: RetroArch has no escape syntax, so
//!   changing the value would be a guess. The renderer still defensively
//!   sanitizes manually-constructed entries, but parser output can never
//!   reach that path with an altered value.

use sha2::{Digest, Sha256};
use std::fmt;

use super::cheat_ir::{
    CheatApplicability, CheatDocument, CheatIssue, CheatOperation, CheatPlatform,
    CheatReconciliationEntry, CheatSourceFieldEvidence, CheatSourceFormat,
};
use serde::Serialize;

/// Mirrors `cheat_catalogue::MAX_CHEATS_PER_GAME` and
/// `retroarch_inventory::MAX_CHEAT_ENTRIES_PER_FILE`.
pub const MAX_CHT_ENTRIES: usize = super::retroarch_inventory::MAX_CHEAT_ENTRIES_PER_FILE;
/// Matches the existing catalogue read limit; checked before decoding.
pub const MAX_CHT_FILE_BYTES: usize = 8 * 1024 * 1024;
/// Includes key, quoting and whitespace. Oversized lines are rejected locally.
pub const MAX_CHT_LINE_BYTES: usize = 8 * 1024;
/// Code is opaque across cores, but its byte and `+` component counts are bounded.
pub const MAX_CHT_CODE_BYTES: usize = MAX_CHT_FIELD_BYTES;
pub const MAX_CHT_CODE_LINES: usize = 256;
/// Includes a blocking overflow marker so dropped evidence cannot enable an entry.
pub const MAX_CHT_ENTRY_WARNINGS: usize = 32;
/// One `cheatN_*` value, after unquoting. Longer values are truncated and
/// the entry is marked unselectable rather than silently shortened.
pub const MAX_CHT_FIELD_BYTES: usize = 4 * 1024;
/// Preserved non-`cheatN_*` keys (`cheat_delay`, custom tooling keys, ...).
pub const MAX_CHT_GLOBAL_FIELDS: usize = 64;
/// Preserved leading `#` comment lines.
pub const MAX_CHT_PRESERVED_COMMENTS: usize = 32;
/// Preserved `cheatN_<field>` keys other than `desc`/`code`/`enable`.
pub const MAX_CHT_EXTRA_FIELDS_PER_ENTRY: usize = 32;
/// Original assignments retained per entry as review evidence (repeats included).
pub const MAX_CHT_SOURCE_FIELDS_PER_ENTRY: usize = 64;
/// Original file-wide assignments retained as review evidence.
pub const MAX_CHT_GLOBAL_SOURCE_FIELDS: usize = 128;
/// Bounded document-level warning list.
pub const MAX_CHT_DOCUMENT_WARNINGS: usize = 256;

/// A whole-file parse failure. An individual bad *line* never produces one
/// of these - it produces a warning and leaves the rest of the file usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChtParseErrorKind {
    /// The bytes are not valid UTF-8. RetroArch cheat files are ASCII in
    /// practice; anything else is reported rather than lossily decoded.
    UnsupportedEncoding,
    /// A UTF-16/UTF-32 byte-order mark was found. Reported separately from
    /// generic invalid UTF-8 because it is a recognisable, actionable case.
    UnsupportedUtf16Encoding,
    /// The file declares no `cheats = N` key and contains no `cheatN_*`
    /// entry at all - it is not a cheat file.
    NotACheatFile,
    /// Reserved whole-file entry-limit failure for existing API consumers.
    /// This parser instead warns and stops accepting new indices at the limit.
    TooManyEntries,
    /// Input exceeds the standalone parser file bound.
    OversizedInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChtParseError {
    pub kind: ChtParseErrorKind,
    pub detail: String,
}

impl fmt::Display for ChtParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for ChtParseError {}

/// Why one entry is imperfect. Some warnings make an entry unselectable -
/// see [`ChtEntry::is_selectable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChtEntryWarningKind {
    /// No `cheatN_code` key at all. Blocking: nothing could be installed.
    MissingCode,
    /// `cheatN_code` present but empty after unquoting. Blocking.
    EmptyCode,
    /// No `cheatN_desc`. Not blocking; the UI label is explicitly index-derived.
    MissingDescription,
    /// An identical decoded field appeared twice. Observable, not blocking.
    DuplicateField,
    /// `cheatN_enable` had a value other than `true`/`false`. Treated as
    /// `false` for display only. Blocking: the source default is unknown.
    UnparsableEnableValue,
    /// A value exceeded [`MAX_CHT_FIELD_BYTES`]. Blocking, because the
    /// retained value is not the source value.
    OversizedField,
    /// A double quote inside a value will be written as `'` by
    /// [`render_cht_file`]. Blocking: altering a source value is not a
    /// safe correction for a cheat entry.
    QuoteNormalized,
    /// The value contained a control character (newline, NUL, ...) that
    /// cannot appear in a RetroArch config value. Blocking.
    ControlCharacter,
    /// Different decoded values for the same field. First retained for review only.
    ConflictingDuplicate,
    /// A quoted value was not closed on its source line.
    TruncatedValue,
    /// A known numeric/boolean field cannot be interpreted safely.
    InvalidFieldValue,
    /// Opaque code has empty components or too many components.
    MalformedCode,
    /// Unknown forward-compatible field, preserved verbatim.
    UnsupportedField,
    /// Dropped fields or warning evidence: installing a partial entry is unsafe.
    LimitReached,
}

impl ChtEntryWarningKind {
    /// Whether this warning alone makes an entry unsafe to install.
    #[must_use]
    pub fn is_blocking(self) -> bool {
        matches!(
            self,
            Self::MissingCode
                | Self::EmptyCode
                | Self::OversizedField
                | Self::ControlCharacter
                | Self::UnparsableEnableValue
                | Self::ConflictingDuplicate
                | Self::TruncatedValue
                | Self::InvalidFieldValue
                | Self::MalformedCode
                | Self::LimitReached
                // Rendering a quote as a different character changes the
                // source value. Never make that uncertain correction to a
                // cheat entry merely to produce output.
                | Self::QuoteNormalized
        )
    }

    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::MissingCode => "cht_entry_missing_code",
            Self::EmptyCode => "cht_entry_empty_code",
            Self::MissingDescription => "cht_entry_missing_description",
            Self::DuplicateField => "cht_entry_duplicate_field",
            Self::UnparsableEnableValue => "cht_entry_unparsable_enable_value",
            Self::OversizedField => "cht_entry_oversized_field",
            Self::QuoteNormalized => "cht_entry_quote_normalized",
            Self::ControlCharacter => "cht_entry_control_character",
            Self::ConflictingDuplicate => "cht_entry_conflicting_duplicate",
            Self::TruncatedValue => "cht_entry_truncated_value",
            Self::InvalidFieldValue => "cht_entry_invalid_field_value",
            Self::MalformedCode => "cht_entry_malformed_code",
            Self::UnsupportedField => "cht_entry_unsupported_field",
            Self::LimitReached => "cht_entry_limit_reached",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChtEntryWarning {
    pub kind: ChtEntryWarningKind,
    /// 1-based source line when the rejected field was present in the
    /// source. Missing-field warnings use the entry's first known line.
    pub line: Option<u32>,
    /// Original source line or bounded prefix for an oversized line. This is
    /// retained for review; it is never rendered into an installed file.
    pub raw_source: Option<String>,
    pub detail: String,
}

/// A document-level problem that is not attributable to one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChtDocumentWarningKind {
    /// A non-empty, non-comment line with no `=`.
    MalformedLine,
    /// `cheats = <not a number>`.
    MalformedDeclaredCount,
    /// A `cheatN_` key whose `N` is not a plain decimal index.
    MalformedEntryIndex,
    /// An index outside the supported unsigned 32-bit range.
    EntryIndexOutOfRange,
    /// `cheats = N` disagrees with the number of distinct parsed indexes.
    DeclaredCountMismatch,
    /// The parsed indexes are not `0..n` - e.g. `cheat0_*` and `cheat5_*`
    /// with nothing between. Never repaired in place; renumbering happens
    /// only in the rendered output.
    NonContiguousIndexes,
    /// A bound ([`MAX_CHT_GLOBAL_FIELDS`], [`MAX_CHT_PRESERVED_COMMENTS`],
    /// [`MAX_CHT_EXTRA_FIELDS_PER_ENTRY`], [`MAX_CHT_DOCUMENT_WARNINGS`])
    /// was reached and later content of that kind was dropped.
    LimitReached,
    MissingDeclaredCount,
    OversizedDeclaredCount,
    OversizedLine,
    InvalidFieldValue,
    DuplicateField,
    ConflictingDuplicate,
}

impl ChtDocumentWarningKind {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::MalformedLine => "cht_malformed_line",
            Self::MalformedDeclaredCount => "cht_malformed_declared_count",
            Self::MalformedEntryIndex => "cht_malformed_entry_index",
            Self::EntryIndexOutOfRange => "cht_entry_index_out_of_range",
            Self::DeclaredCountMismatch => "cht_declared_count_mismatch",
            Self::NonContiguousIndexes => "cht_non_contiguous_indexes",
            Self::LimitReached => "cht_limit_reached",
            Self::MissingDeclaredCount => "cht_missing_declared_count",
            Self::OversizedDeclaredCount => "cht_oversized_declared_count",
            Self::OversizedLine => "cht_oversized_line",
            Self::InvalidFieldValue => "cht_invalid_field_value",
            Self::DuplicateField => "cht_duplicate_field",
            Self::ConflictingDuplicate => "cht_conflicting_duplicate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChtDocumentWarning {
    pub kind: ChtDocumentWarningKind,
    /// 1-based source line, when the warning is attributable to one.
    pub line: Option<u32>,
    pub detail: String,
}

/// One parsed cheat, retaining everything needed to write it back out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChtEntry {
    /// The `cheatN_` index exactly as the source declared it. Rendering
    /// renumbers; this field never does.
    pub index: u32,
    /// Exact source value before decoding/normalization, including source quotes.
    pub original_description: Option<String>,
    pub original_code: Option<String>,
    pub description: Option<String>,
    pub code: Option<String>,
    pub enabled_by_default: bool,
    /// `cheatN_<field>` pairs other than `desc`/`code`/`enable`, in
    /// first-seen order, with `<field>` (not the full key) as the name.
    pub extra_fields: Vec<(String, String)>,
    pub warnings: Vec<ChtEntryWarning>,
    /// Every original `cheatN_*` assignment in source order (bounded), so a
    /// conflicting or repeated value is reviewable, never silently dropped.
    pub source_fields: Vec<CheatSourceFieldEvidence>,
}

impl ChtEntry {
    /// Whether this entry can be offered for selection and installed. An
    /// entry with only non-blocking warnings stays selectable; the warnings
    /// are still shown.
    #[must_use]
    pub fn is_selectable(&self) -> bool {
        self.code.as_deref().is_some_and(|code| !code.is_empty())
            && !self
                .warnings
                .iter()
                .any(|warning| warning.kind.is_blocking())
    }

    /// The description shown in the picker and written to the installed
    /// file. Falls back to a stable, index-derived label so an entry with
    /// no `cheatN_desc` is never rendered with an empty name.
    #[must_use]
    pub fn effective_description(&self) -> String {
        match self.description.as_deref() {
            Some(text) if !text.trim().is_empty() => text.to_string(),
            _ => format!("Cheat {}", self.index),
        }
    }

    pub fn blocking_warnings(&self) -> impl Iterator<Item = &ChtEntryWarning> {
        self.warnings
            .iter()
            .filter(|warning| warning.kind.is_blocking())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChtDocument {
    /// The `cheats = N` value the source declared, if any.
    pub declared_count: Option<u32>,
    /// Entries in ascending declared-index order - the catalogue's own
    /// order, which the picker and the renderer both preserve.
    pub entries: Vec<ChtEntry>,
    /// Leading `#` comment lines, verbatim minus the leading `#`.
    pub preserved_comments: Vec<String>,
    /// Non-`cheatN_*`, non-`cheats` keys, in first-seen order.
    pub global_fields: Vec<(String, String)>,
    /// Original file-wide assignments, including repeated count/engine keys.
    pub source_fields: Vec<CheatSourceFieldEvidence>,
    pub warnings: Vec<ChtDocumentWarning>,
}

impl ChtDocument {
    #[must_use]
    pub fn selectable_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.is_selectable())
            .count()
    }

    #[must_use]
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty() || self.entries.iter().any(|entry| !entry.warnings.is_empty())
    }

    /// Projects entries into the shared reconciliation model. A projection
    /// only: original values, flags and source lines stay in this document.
    /// Callers supply already-verified identity evidence explicitly.
    pub fn reconciliation_entries(
        &self,
        game_identity: &str,
        identity_verified: bool,
        platform: CheatPlatform,
        source: &str,
        source_path: &str,
    ) -> Vec<CheatReconciliationEntry> {
        let global_conflict = self
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::ConflictingDuplicate);
        let global_truncated = self
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::LimitReached);
        self.entries
            .iter()
            .enumerate()
            .map(|(position, entry)| {
                let title = entry.effective_description();
                let provenance = vec![format!("{source_path}:cheat{}", entry.index)];
                let mut issues: Vec<CheatIssue> = entry
                    .blocking_warnings()
                    .map(|warning| {
                        if warning.kind == ChtEntryWarningKind::ConflictingDuplicate {
                            CheatIssue::SourceIndexConflict
                        } else {
                            CheatIssue::UnsupportedOperation(warning.detail.clone())
                        }
                    })
                    .collect();
                if global_conflict {
                    issues.push(CheatIssue::SourceMetadataConflict);
                }
                if global_truncated {
                    issues.push(CheatIssue::UnsupportedOperation(
                        "file-wide metadata exceeds the retained bound; review the document evidence"
                            .into(),
                    ));
                }
                CheatReconciliationEntry {
                    game_identity: game_identity.into(),
                    identity_verified,
                    title: title.clone(),
                    source: source.into(),
                    source_format: CheatSourceFormat::RetroArch,
                    document: CheatDocument {
                        source_evidence: vec![super::cheat_provenance::CheatRecordProvenance {
                            source_path: Some(std::path::PathBuf::from(source_path)),
                            provider_name: Some(source.into()),
                            source_format: Some("retroarch_cht".into()),
                            record_index: Some(entry.index),
                            ..super::cheat_provenance::CheatRecordProvenance::original(
                                entry.original_description.clone(),
                                entry.original_code.clone(),
                            )
                        }],
                        title,
                        platform: platform.clone(),
                        source_format: CheatSourceFormat::RetroArch,
                        operations: entry
                            .code
                            .iter()
                            .map(|raw| CheatOperation::UnsupportedRaw {
                                source_format: CheatSourceFormat::RetroArch,
                                raw: raw.clone(),
                                reason: "RetroArch engine-specific code retained without decoding"
                                    .into(),
                            })
                            .collect(),
                        issues,
                        provenance: provenance.clone(),
                    },
                    raw_code: entry.code.clone(),
                    provenance,
                    applicability: CheatApplicability {
                        metadata: entry
                            .extra_fields
                            .iter()
                            .map(|(key, value)| (format!("entry:{key}"), value.clone()))
                            .chain(
                                self.global_fields
                                    .iter()
                                    .map(|(key, value)| (format!("global:{key}"), value.clone())),
                            )
                            .collect(),
                        ..Default::default()
                    },
                    source_path: Some(source_path.into()),
                    source_index: Some(entry.index),
                    // File-wide assignments are carried once, not copied per entry.
                    source_fields: self
                        .source_fields
                        .iter()
                        .filter(|_| position == 0)
                        .chain(entry.source_fields.iter())
                        .cloned()
                        .collect(),
                }
            })
            .collect()
    }

    /// Indices needing a deliberate choice before default activation: the same
    /// name with a different body/metadata, or the same body with different
    /// engine metadata. Descriptions and codes are never rewritten.
    pub fn conflicting_entry_indices(&self) -> std::collections::BTreeSet<u32> {
        use std::collections::{BTreeMap, BTreeSet};
        let mut names = BTreeMap::<String, Vec<&ChtEntry>>::new();
        let mut codes = BTreeMap::<String, Vec<&ChtEntry>>::new();
        for entry in &self.entries {
            if let Some(name) = &entry.description {
                let name = name
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_ascii_lowercase();
                if !name.is_empty() {
                    names.entry(name).or_default().push(entry);
                }
            }
            if let Some(code) = &entry.code {
                if !code.trim().is_empty() {
                    codes.entry(code.trim().into()).or_default().push(entry);
                }
            }
        }
        let metadata = |entry: &ChtEntry| {
            entry
                .extra_fields
                .iter()
                .cloned()
                .collect::<BTreeMap<_, _>>()
        };
        let mut conflicts = BTreeSet::new();
        for group in names.values() {
            let first = group[0];
            if group.iter().any(|entry| {
                entry.code.as_deref() != first.code.as_deref() || metadata(entry) != metadata(first)
            }) {
                conflicts.extend(group.iter().map(|entry| entry.index));
            }
        }
        for group in codes.values() {
            if group
                .iter()
                .any(|entry| metadata(entry) != metadata(group[0]) || entry.code != group[0].code)
            {
                conflicts.extend(group.iter().map(|entry| entry.index));
            }
        }
        conflicts
    }

    #[must_use]
    pub fn entry(&self, index: u32) -> Option<&ChtEntry> {
        self.entries.iter().find(|entry| entry.index == index)
    }
}

/// Parses raw catalogue bytes. Returns `Err` only for a whole-file problem
/// (see [`ChtParseErrorKind`]); a file with individually broken lines still
/// parses, with warnings.
pub fn parse_cht_bytes(bytes: &[u8]) -> Result<ChtDocument, ChtParseError> {
    check_file_size(bytes.len())?;
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(ChtParseError {
            kind: ChtParseErrorKind::UnsupportedUtf16Encoding,
            detail:
                "file begins with a UTF-16 byte-order mark; only UTF-8 cheat files are supported"
                    .to_string(),
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|error| ChtParseError {
        kind: ChtParseErrorKind::UnsupportedEncoding,
        detail: format!(
            "file is not valid UTF-8 (first invalid byte at offset {})",
            error.valid_up_to()
        ),
    })?;
    parse_cht_text(text.strip_prefix('\u{feff}').unwrap_or(text))
}

/// Parses already-decoded text. Prefer [`parse_cht_bytes`] for catalogue
/// input so encoding problems are reported rather than assumed away.
pub fn parse_cht_text(text: &str) -> Result<ChtDocument, ChtParseError> {
    check_file_size(text.len())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    use std::collections::BTreeMap;

    struct Draft {
        first_line: u32,
        first_raw_source: String,
        original_description: Option<String>,
        original_code: Option<String>,
        description: Option<String>,
        code: Option<String>,
        enable: Option<String>,
        extra_fields: Vec<(String, String)>,
        warnings: Vec<ChtEntryWarning>,
        source_fields: Vec<CheatSourceFieldEvidence>,
    }

    let mut source_fields: Vec<CheatSourceFieldEvidence> = Vec::new();
    let mut declared_count: Option<u32> = None;
    let mut declared_value: Option<String> = None;
    let mut drafts: BTreeMap<u32, Draft> = BTreeMap::new();
    let mut preserved_comments: Vec<String> = Vec::new();
    let mut global_fields: Vec<(String, String)> = Vec::new();
    let mut warnings: Vec<ChtDocumentWarning> = Vec::new();
    let mut seen_any_body_line = false;
    let mut limit_reported = false;
    let mut source_fields_limit_reported = false;

    let push_warning = |warnings: &mut Vec<ChtDocumentWarning>,
                        kind: ChtDocumentWarningKind,
                        line: Option<u32>,
                        detail: String| {
        if warnings.len() < MAX_CHT_DOCUMENT_WARNINGS {
            warnings.push(ChtDocumentWarning { kind, line, detail });
        } else {
            warnings[MAX_CHT_DOCUMENT_WARNINGS - 1] = ChtDocumentWarning {
                kind: ChtDocumentWarningKind::LimitReached,
                line,
                detail: "document warning limit reached; further evidence omitted".to_string(),
            };
        }
    };

    for (offset, raw_line) in text.lines().enumerate() {
        let line_number = u32::try_from(offset + 1).unwrap_or(u32::MAX);
        // Never decode or copy an unbounded line. Keep enough of an oversized
        // assignment to attribute it to its entry, which must remain blocked.
        let oversized_line = raw_line.len() > MAX_CHT_LINE_BYTES;
        let bounded_line = bounded_prefix(raw_line, MAX_CHT_LINE_BYTES);
        // Leading whitespace must not conceal an attributable entry key
        // beyond the evidence prefix. Trimming scans bounded input, allocates
        // nothing, and the oversized assignment still blocks its entry.
        let line = bounded_prefix(raw_line.trim(), MAX_CHT_LINE_BYTES);
        if oversized_line {
            push_warning(
                &mut warnings,
                ChtDocumentWarningKind::OversizedLine,
                Some(line_number),
                format!("line {line_number} exceeds {MAX_CHT_LINE_BYTES} bytes"),
            );
        }
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            if !seen_any_body_line && preserved_comments.len() < MAX_CHT_PRESERVED_COMMENTS {
                preserved_comments.push(comment.trim().to_string());
            }
            continue;
        }
        let missing_separator = !line.contains('=');
        let (raw_key, raw_value) = line.split_once('=').unwrap_or_else(|| {
            push_warning(
                &mut warnings,
                ChtDocumentWarningKind::MalformedLine,
                Some(line_number),
                format!("line {line_number} has no '=' separator"),
            );
            (line.split_whitespace().next().unwrap_or(line), "")
        });
        seen_any_body_line = true;
        let key = raw_key.trim();
        let (value, mut value_warnings) = decode_value(raw_value.trim());
        // The input file is bounded. Hash a borrowed complete value, never an
        // allocated full line or just its display prefix. No escape decoding.
        let full_value = raw_line
            .trim()
            .split_once('=')
            .map_or("", |(_, v)| v.trim());
        let full_value = full_value
            .strip_prefix('"')
            .map_or(full_value, |v| v.strip_suffix('"').unwrap_or(v));
        let full_value_sha256 = Some(Sha256::digest(full_value.as_bytes()).into());
        if missing_separator {
            value_warnings.push(entry_warning(
                ChtEntryWarningKind::InvalidFieldValue,
                line_number,
                bounded_line,
                "assignment has no '=' separator".to_string(),
            ));
        }
        if oversized_line {
            value_warnings.push(entry_warning(
                ChtEntryWarningKind::OversizedField,
                line_number,
                bounded_line,
                "source line exceeds the line bound".to_string(),
            ));
        }

        let is_entry_assignment = key
            .strip_prefix("cheat")
            .is_some_and(|rest| rest.bytes().next().is_some_and(|b| b.is_ascii_digit()));
        if !is_entry_assignment && !key.is_empty() {
            if source_fields.len() < MAX_CHT_GLOBAL_SOURCE_FIELDS {
                source_fields.push(CheatSourceFieldEvidence {
                    full_value_sha256,
                    field: if key.eq_ignore_ascii_case("cheats") {
                        "cheats".into()
                    } else {
                        key.into()
                    },
                    value: value.clone(),
                    line: line_number,
                    raw_source: bounded_line.to_string(),
                });
            } else if !source_fields_limit_reported {
                source_fields_limit_reported = true;
                push_warning(
                    &mut warnings,
                    ChtDocumentWarningKind::LimitReached,
                    Some(line_number),
                    format!(
                        "more than {MAX_CHT_GLOBAL_SOURCE_FIELDS} file-wide assignments; later evidence omitted"
                    ),
                );
            }
        }

        if key.eq_ignore_ascii_case("cheats") {
            if let Some(first) = &declared_value {
                push_warning(
                    &mut warnings,
                    if first == &value
                        && source_fields
                            .iter()
                            .find(|f| f.field == "cheats")
                            .is_some_and(|f| f.full_value_sha256 == full_value_sha256)
                    {
                        ChtDocumentWarningKind::DuplicateField
                    } else {
                        ChtDocumentWarningKind::ConflictingDuplicate
                    },
                    Some(line_number),
                    format!(
                        "duplicate cheats declaration: first {first:?}, later {value:?}; first retained"
                    ),
                );
            } else {
                declared_value = Some(value.clone());
                if !value_warnings.is_empty() {
                    push_warning(
                        &mut warnings,
                        ChtDocumentWarningKind::MalformedDeclaredCount,
                        Some(line_number),
                        "cheats declaration has an unsafe value".to_string(),
                    );
                } else {
                    match value.parse::<u32>() {
                        Ok(count) => {
                            declared_count = Some(count);
                            if count as usize > MAX_CHT_ENTRIES {
                                push_warning(
                                    &mut warnings,
                                    ChtDocumentWarningKind::OversizedDeclaredCount,
                                    Some(line_number),
                                    format!(
                                        "declared count {count} exceeds {MAX_CHT_ENTRIES}; only actual bounded entries are parsed"
                                    ),
                                );
                            }
                        }
                        Err(_) => push_warning(
                            &mut warnings,
                            ChtDocumentWarningKind::MalformedDeclaredCount,
                            Some(line_number),
                            format!(
                                "line {line_number}: 'cheats' value {value:?} is not an unsigned 32-bit count"
                            ),
                        ),
                    }
                }
            }
            continue;
        }

        // `cheat_delay` and friends are RetroArch's own global cheat keys:
        // they share the `cheat` prefix but continue with `_`, never with a
        // digit, so they are preserved as global fields rather than
        // mistaken for a malformed `cheatN_` entry key.
        let entry_key = key
            .strip_prefix("cheat")
            .filter(|remainder| !remainder.starts_with('_'));
        let Some(remainder) = entry_key else {
            if !value_warnings.is_empty() || key.is_empty() {
                push_warning(
                    &mut warnings,
                    ChtDocumentWarningKind::InvalidFieldValue,
                    Some(line_number),
                    format!("global key {key:?} has an unsafe value; retained for review only"),
                );
            }
            if key.is_empty() {
                continue;
            }
            if let Some((_, first)) = global_fields.iter().find(|(name, _)| name == key) {
                push_warning(
                    &mut warnings,
                    if first == &value
                        && source_fields
                            .iter()
                            .find(|f| f.field == key)
                            .is_some_and(|f| f.full_value_sha256 == full_value_sha256)
                    {
                        ChtDocumentWarningKind::DuplicateField
                    } else {
                        ChtDocumentWarningKind::ConflictingDuplicate
                    },
                    Some(line_number),
                    format!(
                        "duplicate global key {key:?}: first {first:?}, later {value:?}; first retained"
                    ),
                );
            } else if global_fields.len() < MAX_CHT_GLOBAL_FIELDS {
                global_fields.push((key.to_string(), value));
            } else if !limit_reported {
                limit_reported = true;
                push_warning(
                    &mut warnings,
                    ChtDocumentWarningKind::LimitReached,
                    Some(line_number),
                    format!("more than {MAX_CHT_GLOBAL_FIELDS} non-cheat keys; later keys dropped"),
                );
            }
            continue;
        };

        let digit_count = remainder.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count == 0 || !remainder[digit_count..].starts_with('_') {
            push_warning(
                &mut warnings,
                ChtDocumentWarningKind::MalformedEntryIndex,
                Some(line_number),
                format!("line {line_number}: key {key:?} is not a valid cheatN_<field> key"),
            );
            continue;
        }
        let Ok(entry_index) = remainder[..digit_count].parse::<u32>() else {
            push_warning(
                &mut warnings,
                ChtDocumentWarningKind::EntryIndexOutOfRange,
                Some(line_number),
                format!("line {line_number}: entry index in {key:?} is out of numeric range"),
            );
            continue;
        };
        let field = &remainder[digit_count + 1..];
        if !drafts.contains_key(&entry_index) && drafts.len() >= MAX_CHT_ENTRIES {
            push_warning(
                &mut warnings,
                ChtDocumentWarningKind::LimitReached,
                Some(line_number),
                format!(
                    "more than {MAX_CHT_ENTRIES} distinct entries; new index {entry_index} omitted"
                ),
            );
            continue;
        }
        let draft = drafts.entry(entry_index).or_insert_with(|| Draft {
            first_line: line_number,
            first_raw_source: bounded_line.to_string(),
            original_description: None,
            original_code: None,
            description: None,
            code: None,
            enable: None,
            extra_fields: Vec::new(),
            warnings: Vec::new(),
            source_fields: Vec::new(),
        });
        if draft.source_fields.len() < MAX_CHT_SOURCE_FIELDS_PER_ENTRY {
            draft.source_fields.push(CheatSourceFieldEvidence {
                full_value_sha256,
                field: field.to_string(),
                value: value.clone(),
                line: line_number,
                raw_source: bounded_line.to_string(),
            });
        } else if !draft
            .warnings
            .iter()
            .any(|w| w.kind == ChtEntryWarningKind::LimitReached)
        {
            draft.warnings.push(entry_warning(
                ChtEntryWarningKind::LimitReached,
                line_number,
                bounded_line,
                format!("more than {MAX_CHT_SOURCE_FIELDS_PER_ENTRY} assignments for one entry; later evidence omitted"),
            ));
        }
        for warning in &mut value_warnings {
            warning.line = Some(line_number);
            warning.raw_source = Some(bounded_line.to_string());
        }
        // A bounded prefix can make two different over-long values look equal.
        // Compare complete value digests so that difference
        // stays a blocking, observable conflict.
        let truncated_now = value_warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::OversizedField);
        if let Some(previous) = draft
            .source_fields
            .iter()
            .rev()
            .skip(1)
            .find(|item| item.field == field)
        {
            let previous_truncated = draft.warnings.iter().any(|warning| {
                warning.kind == ChtEntryWarningKind::OversizedField
                    && warning.line == Some(previous.line)
            });
            if (truncated_now || previous_truncated)
                && field_values_equivalent(field, &previous.value, &value)
                && previous.full_value_sha256 != full_value_sha256
            {
                draft.warnings.push(entry_warning(
                    ChtEntryWarningKind::ConflictingDuplicate,
                    line_number,
                    bounded_line,
                    format!(
                        "cheat{entry_index}_{field} differs from line {} beyond the retained value bound; review required",
                        previous.line
                    ),
                ));
            }
        }
        draft.warnings.extend(value_warnings);

        if field == "desc" && draft.original_description.is_none() {
            draft.original_description = Some(raw_value.trim().to_string());
        }
        if field == "code" && draft.original_code.is_none() {
            draft.original_code = Some(raw_value.trim().to_string());
        }
        match field {
            "desc" => set_once(
                &mut draft.description,
                value,
                "desc",
                entry_index,
                line_number,
                bounded_line,
                &mut draft.warnings,
            ),
            "code" => {
                if value.len() > MAX_CHT_CODE_BYTES
                    || value.split('+').count() > MAX_CHT_CODE_LINES
                    || (!value.trim().is_empty()
                        && value.split('+').any(|part| part.trim().is_empty()))
                {
                    draft.warnings.push(entry_warning(ChtEntryWarningKind::MalformedCode,
                        line_number, bounded_line,
                        format!("cheat{entry_index}_code has empty components or exceeds the code bounds")));
                }
                set_once(
                    &mut draft.code,
                    value,
                    "code",
                    entry_index,
                    line_number,
                    bounded_line,
                    &mut draft.warnings,
                );
            }
            "enable" => {
                if !value.eq_ignore_ascii_case("true") && !value.eq_ignore_ascii_case("false") {
                    draft.warnings.push(entry_warning(ChtEntryWarningKind::UnparsableEnableValue,
                        line_number, bounded_line, format!("cheat{entry_index}_enable value {value:?} is not true/false; source default is unknown")));
                }
                set_once(
                    &mut draft.enable,
                    value,
                    "enable",
                    entry_index,
                    line_number,
                    bounded_line,
                    &mut draft.warnings,
                );
            }
            "" => {
                draft.warnings.push(entry_warning(
                    ChtEntryWarningKind::InvalidFieldValue,
                    line_number,
                    bounded_line,
                    "entry field name is empty".to_string(),
                ));
                push_warning(
                    &mut warnings,
                    ChtDocumentWarningKind::MalformedEntryIndex,
                    Some(line_number),
                    format!("line {line_number}: key {key:?} has an empty field name"),
                );
            }
            other => {
                if let Some((_, first)) = draft
                    .extra_fields
                    .iter_mut()
                    .find(|(name, _)| name == other)
                {
                    let mut slot = Some(first.clone());
                    set_once(
                        &mut slot,
                        value,
                        other,
                        entry_index,
                        line_number,
                        bounded_line,
                        &mut draft.warnings,
                    );
                } else if draft.extra_fields.len() >= MAX_CHT_EXTRA_FIELDS_PER_ENTRY {
                    draft.warnings.push(entry_warning(
                        ChtEntryWarningKind::LimitReached,
                        line_number,
                        bounded_line,
                        "extra field limit reached; entry is incomplete".to_string(),
                    ));
                } else {
                    if let Some(kind) = validate_extra_field(other, &value) {
                        draft.warnings.push(entry_warning(
                            kind,
                            line_number,
                            bounded_line,
                            format!("cheat{entry_index}_{other} = {value:?}: {}", kind.code()),
                        ));
                    }
                    draft.extra_fields.push((other.to_string(), value));
                }
            }
        }
        if draft.warnings.iter().any(|w| {
            w.line == Some(line_number) && w.kind == ChtEntryWarningKind::ConflictingDuplicate
        }) {
            draft.warnings.retain(|w| {
                w.line != Some(line_number) || w.kind != ChtEntryWarningKind::DuplicateField
            });
        }
        bound_entry_warnings(&mut draft.warnings);
    }

    if declared_value.is_none() && drafts.is_empty() {
        return Err(ChtParseError {
            kind: ChtParseErrorKind::NotACheatFile,
            detail: "no 'cheats' key and no cheatN_* entry was found".to_string(),
        });
    }

    let mut entries: Vec<ChtEntry> = Vec::with_capacity(drafts.len());
    for (index, draft) in drafts {
        let mut entry_warnings = draft.warnings;
        match draft.code.as_deref() {
            None => entry_warnings.push(ChtEntryWarning {
                kind: ChtEntryWarningKind::MissingCode,
                line: Some(draft.first_line),
                raw_source: Some(draft.first_raw_source.clone()),
                detail: format!("cheat{index} has no cheat{index}_code key"),
            }),
            Some(code) if code.trim().is_empty() => entry_warnings.push(ChtEntryWarning {
                kind: ChtEntryWarningKind::EmptyCode,
                line: Some(draft.first_line),
                raw_source: Some(draft.first_raw_source.clone()),
                detail: format!("cheat{index}_code is empty"),
            }),
            Some(_) => {}
        }

        if draft
            .description
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            entry_warnings.push(ChtEntryWarning {
                kind: ChtEntryWarningKind::MissingDescription,
                line: Some(draft.first_line),
                raw_source: Some(draft.first_raw_source.clone()),
                detail: format!("cheat{index} has no usable cheat{index}_desc key"),
            });
        }
        bound_entry_warnings(&mut entry_warnings);
        entries.push(ChtEntry {
            index,
            original_description: draft.original_description,
            original_code: draft.original_code,
            description: draft.description,
            code: draft.code,
            enabled_by_default: draft
                .enable
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("true")),
            extra_fields: draft.extra_fields,
            warnings: entry_warnings,
            source_fields: draft.source_fields,
        });
    }

    if declared_value.is_none() {
        push_warning(
            &mut warnings,
            ChtDocumentWarningKind::MissingDeclaredCount,
            None,
            "no cheats declaration; only actual entries were parsed".to_string(),
        );
    }
    if let Some(count) = declared_count
        && (count as usize != entries.len() || entries.iter().any(|entry| entry.index >= count))
    {
        push_warning(
            &mut warnings,
            ChtDocumentWarningKind::DeclaredCountMismatch,
            None,
            format!(
                "file declares cheats = {count}; {} distinct entries were parsed (count or index range disagrees)",
                entries.len()
            ),
        );
    }
    if entries
        .iter()
        .enumerate()
        .any(|(position, entry)| u32::try_from(position).unwrap_or(u32::MAX) != entry.index)
    {
        push_warning(
            &mut warnings,
            ChtDocumentWarningKind::NonContiguousIndexes,
            None,
            "declared entry indexes are not contiguous from zero; the installed file is renumbered"
                .to_string(),
        );
    }

    Ok(ChtDocument {
        declared_count,
        entries,
        preserved_comments,
        global_fields,
        source_fields,
        warnings,
    })
}

fn set_once(
    slot: &mut Option<String>,
    value: String,
    field: &str,
    index: u32,
    line: u32,
    raw_source: &str,
    warnings: &mut Vec<ChtEntryWarning>,
) {
    if let Some(first) = slot {
        warnings.push(ChtEntryWarning {
            kind: if field_values_equivalent(field, first, &value) { ChtEntryWarningKind::DuplicateField }
                else { ChtEntryWarningKind::ConflictingDuplicate },
            line: Some(line),
            raw_source: Some(raw_source.to_string()),
            detail: format!(
                "cheat{index}_{field} appeared more than once: first {first:?}, later {value:?}; first retained for review"
            ),
        });
        return;
    }
    *slot = Some(value);
}

/// Only normalization justified by a field's grammar: descriptions compare
/// case- and whitespace-insensitively, `enable` compares case-insensitively.
/// Codes and every other field are literal. Originals are never rewritten.
pub(super) fn field_values_equivalent(field: &str, left: &str, right: &str) -> bool {
    let fold = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    };
    match field {
        "desc" => fold(left) == fold(right),
        "enable" => left.eq_ignore_ascii_case(right),
        _ => left == right,
    }
}

fn check_file_size(size: usize) -> Result<(), ChtParseError> {
    if size > MAX_CHT_FILE_BYTES {
        return Err(ChtParseError {
            kind: ChtParseErrorKind::OversizedInput,
            detail: format!("input exceeds {MAX_CHT_FILE_BYTES} bytes"),
        });
    }
    Ok(())
}

fn bounded_prefix(value: &str, limit: usize) -> &str {
    let mut boundary = value.len().min(limit);
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

fn entry_warning(
    kind: ChtEntryWarningKind,
    line: u32,
    raw: &str,
    detail: String,
) -> ChtEntryWarning {
    ChtEntryWarning {
        kind,
        line: Some(line),
        raw_source: Some(raw.to_string()),
        detail,
    }
}

fn bound_entry_warnings(warnings: &mut Vec<ChtEntryWarning>) {
    if warnings.len() > MAX_CHT_ENTRY_WARNINGS {
        warnings.truncate(MAX_CHT_ENTRY_WARNINGS);
        warnings[MAX_CHT_ENTRY_WARNINGS - 1] = ChtEntryWarning {
            kind: ChtEntryWarningKind::LimitReached,
            line: None,
            raw_source: None,
            detail: "entry warning limit reached; further evidence omitted".to_string(),
        };
    }
}

/// Validate only known RetroArch scalar fields. Unknown fields stay observable
/// and preserved, without guessing at their semantics or core-specific ranges.
fn validate_extra_field(field: &str, value: &str) -> Option<ChtEntryWarningKind> {
    let numeric = matches!(
        field,
        "handler"
            | "memory_search_size"
            | "cheat_type"
            | "address"
            | "address_mask"
            | "value"
            | "repeat_count"
            | "repeat_add_to_address"
            | "repeat_add_to_value"
            | "rumble_type"
            | "rumble_value"
            | "rumble_port"
            | "rumble_primary_strength"
            | "rumble_primary_duration"
            | "rumble_secondary_strength"
            | "rumble_secondary_duration"
    );
    if numeric {
        let valid = if let Some(hex) = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
        {
            !hex.is_empty()
                && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                && u32::from_str_radix(hex, 16).is_ok()
        } else {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && value.parse::<u32>().is_ok()
        };
        return (!valid).then_some(ChtEntryWarningKind::InvalidFieldValue);
    }
    if field == "big_endian" {
        return (!value.eq_ignore_ascii_case("true") && !value.eq_ignore_ascii_case("false"))
            .then_some(ChtEntryWarningKind::InvalidFieldValue);
    }
    if !field
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Some(ChtEntryWarningKind::InvalidFieldValue);
    }
    Some(ChtEntryWarningKind::UnsupportedField)
}

/// Decode quoting only; RetroArch values have no backslash escape syntax.
/// Retain the bounded source value for review, never repair a malformed value.
fn decode_value(raw: &str) -> (String, Vec<ChtEntryWarning>) {
    let mut warnings = Vec::new();
    let unquoted = match raw.strip_prefix('"') {
        Some(rest) => match rest.strip_suffix('"') {
            Some(value) => value,
            None => {
                warnings.push(ChtEntryWarning {
                    kind: ChtEntryWarningKind::TruncatedValue,
                    line: None,
                    raw_source: None,
                    detail: "quoted value has no closing quote".to_string(),
                });
                rest
            }
        },
        None => raw,
    };
    if unquoted
        .chars()
        .any(|character| character.is_control() || character == '\u{fffd}')
    {
        warnings.push(ChtEntryWarning { kind: ChtEntryWarningKind::ControlCharacter,
            line: None, raw_source: None,
            detail: "value contains a control or replacement character; original bytes cannot be safely inferred".to_string() });
    }
    if unquoted.contains('"') {
        warnings.push(ChtEntryWarning {
            kind: ChtEntryWarningKind::QuoteNormalized,
            line: None,
            raw_source: None,
            detail: "value contains an interior quote; RetroArch values have no escape syntax"
                .to_string(),
        });
    }
    if unquoted.len() > MAX_CHT_FIELD_BYTES {
        warnings.push(ChtEntryWarning { kind: ChtEntryWarningKind::OversizedField,
            line: None, raw_source: None,
            detail: format!("value exceeded {MAX_CHT_FIELD_BYTES} bytes; bounded prefix retained for review only") });
    }
    (
        bounded_prefix(unquoted, MAX_CHT_FIELD_BYTES).to_string(),
        warnings,
    )
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// One cheat as it will appear in the installed file. Built from a
/// [`ChtEntry`] the user selected, never from a whole document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChtInstallEntry {
    pub description: String,
    pub code: String,
    /// Whether RetroArch should have this cheat *active* on load, as
    /// opposed to merely present in the file.
    pub enabled: bool,
    /// Preserved `cheatN_<field>` pairs carried through from the source.
    pub extra_fields: Vec<(String, String)>,
}

impl ChtInstallEntry {
    /// Builds an installable entry from a parsed one. Returns `None` for an
    /// entry [`ChtEntry::is_selectable`] rejects, so an unsafe entry can
    /// never reach the renderer even if a caller mis-tracks its own
    /// selection state.
    #[must_use]
    pub fn from_entry(entry: &ChtEntry, enabled: bool) -> Option<Self> {
        if !entry.is_selectable() {
            return None;
        }
        Some(Self {
            description: entry.effective_description(),
            code: entry.code.clone()?,
            enabled,
            extra_fields: entry.extra_fields.clone(),
        })
    }
}

/// Renders a complete, RetroArch-loadable `.cht` file.
///
/// Deterministic: the same slice always produces the same bytes. Output
/// indexes are contiguous from zero regardless of the source indexes, the
/// `cheats = N` header always agrees with the number of entries written,
/// and the file always ends with exactly one newline.
///
/// `header_comments` are written as `#` lines before the header. They are
/// the caller's provenance note plus (bounded) source comments; nothing
/// time-varying belongs there, or determinism is lost.
#[must_use]
pub fn render_cht_file(entries: &[ChtInstallEntry], header_comments: &[String]) -> String {
    let mut output = String::new();
    for comment in header_comments.iter().take(MAX_CHT_PRESERVED_COMMENTS) {
        let sanitized: String = comment
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        output.push_str("# ");
        output.push_str(sanitized.trim());
        output.push('\n');
    }
    if !header_comments.is_empty() {
        output.push('\n');
    }

    output.push_str(&format!("cheats = {}\n", entries.len()));
    for (position, entry) in entries.iter().enumerate() {
        output.push('\n');
        output.push_str(&format!(
            "cheat{position}_desc = \"{}\"\n",
            escape_config_value(&entry.description)
        ));
        output.push_str(&format!(
            "cheat{position}_code = \"{}\"\n",
            escape_config_value(&entry.code)
        ));
        output.push_str(&format!(
            "cheat{position}_enable = {}\n",
            if entry.enabled { "true" } else { "false" }
        ));
        for (field, value) in &entry.extra_fields {
            let field: String = field
                .chars()
                .filter(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect();
            if field.is_empty() {
                continue;
            }
            output.push_str(&format!(
                "cheat{position}_{field} = \"{}\"\n",
                escape_config_value(value)
            ));
        }
    }
    output
}

/// Makes one value safe to place inside a double-quoted RetroArch config
/// value. See the module docs for why a quote becomes an apostrophe rather
/// than a backslash escape.
fn escape_config_value(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| if character == '"' { '\'' } else { character })
        .collect()
}

#[cfg(test)]
mod tests;
