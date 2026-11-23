//! What a reconciliation says, and how strongly.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// Paths are written as text even when they are not valid UTF-8 (the invalid
// bytes are replaced), so one odd file name can never stop a report from
// being produced.
fn lossy<S: serde::Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

fn lossy_option<S: serde::Serializer>(
    path: &Option<PathBuf>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match path {
        Some(path) => serializer.serialize_some(&*path.to_string_lossy()),
        None => serializer.serialize_none(),
    }
}

fn lossy_list<S: serde::Serializer>(paths: &[PathBuf], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(paths.iter().map(|path| path.to_string_lossy()))
}

/// Bumped when a field changes meaning or is removed. Additions keep it.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// Where a record without a file came from. The three sets never overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordSet {
    /// The library flags the row "file not found".
    FlaggedMissing,
    /// The row is not flagged, but nothing is at its recorded path.
    AbsentNeverFlagged,
    /// Removed from the library by Forget Missing and kept in its export.
    Forgotten,
}

impl RecordSet {
    pub const ALL: [Self; 3] = [
        Self::FlaggedMissing,
        Self::AbsentNeverFlagged,
        Self::Forgotten,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::FlaggedMissing => "Flagged missing",
            Self::AbsentNeverFlagged => "Absent, never flagged",
            Self::Forgotten => "Removed by Forget Missing",
        }
    }
}

/// How strong the evidence is. Each level says what was shown, and nothing
/// below `Confirmed` claims that two files have the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    Confirmed,
    Strong,
    Metadata,
    Weak,
    /// Not a move at all: the file is where the record says.
    Present,
    Candidate,
    Unresolved,
}

impl EvidenceClass {
    pub const ALL: [Self; 7] = [
        Self::Confirmed,
        Self::Strong,
        Self::Metadata,
        Self::Weak,
        Self::Present,
        Self::Candidate,
        Self::Unresolved,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "Confirmed",
            Self::Strong => "Strong",
            Self::Metadata => "Metadata match",
            Self::Weak => "Weak",
            Self::Present => "Present",
            Self::Candidate => "Candidate",
            Self::Unresolved => "Unresolved",
        }
    }
}

/// The exact finding for one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTier {
    /// EmuWiz's own move record, the same inode, and an independently
    /// recorded content hash of the old path that matches the file today.
    ConfirmedContent,
    /// EmuWiz's own move record and the same inode.
    ConfirmedObject,
    /// One file with the record's name, size and timestamp, whose inode
    /// existed before the old path was last seen and whose own row first
    /// appeared only afterwards.
    SameFileObject,
    /// Exactly one file with the same name, size and timestamp.
    MetadataMatch,
    /// Same name and size; the timestamp differs or several files qualify.
    NameAndSize,
    /// A broken link of this name exists; its relocated target has the
    /// record's size.
    NameAndSizeViaLink,
    /// The file is at the recorded path; the flag is stale.
    PresentAtRecordedPath,
    /// One file of identical byte size and type under another name.
    SizeOnlyCandidate,
    /// Something shares the name. Not evidence of identity.
    NameOnlyLead,
    /// No file; only a broken link to storage that no longer exists.
    BrokenLinkOnly,
    NoTrace,
}

impl EvidenceTier {
    pub const ALL: [Self; 11] = [
        Self::ConfirmedContent,
        Self::ConfirmedObject,
        Self::SameFileObject,
        Self::MetadataMatch,
        Self::NameAndSize,
        Self::NameAndSizeViaLink,
        Self::PresentAtRecordedPath,
        Self::SizeOnlyCandidate,
        Self::NameOnlyLead,
        Self::BrokenLinkOnly,
        Self::NoTrace,
    ];

    pub fn class(self) -> EvidenceClass {
        match self {
            Self::ConfirmedContent | Self::ConfirmedObject => EvidenceClass::Confirmed,
            Self::SameFileObject => EvidenceClass::Strong,
            Self::MetadataMatch => EvidenceClass::Metadata,
            Self::NameAndSize | Self::NameAndSizeViaLink => EvidenceClass::Weak,
            Self::PresentAtRecordedPath => EvidenceClass::Present,
            Self::SizeOnlyCandidate | Self::NameOnlyLead => EvidenceClass::Candidate,
            Self::BrokenLinkOnly | Self::NoTrace => EvidenceClass::Unresolved,
        }
    }

    /// Short stable code for reports, filters and tests.
    pub fn code(self) -> &'static str {
        match self {
            Self::ConfirmedContent => "T1",
            Self::ConfirmedObject => "T2",
            Self::SameFileObject => "T3",
            Self::MetadataMatch => "T4",
            Self::NameAndSize => "T5",
            Self::NameAndSizeViaLink => "T5L",
            Self::PresentAtRecordedPath => "T6",
            Self::SizeOnlyCandidate => "T7",
            Self::NameOnlyLead => "T8",
            Self::BrokenLinkOnly => "T9L",
            Self::NoTrace => "T9",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::ConfirmedContent => {
                "EmuWiz moved this file itself, the same file is on disk, and its contents match a hash recorded for the old path"
            }
            Self::ConfirmedObject => "EmuWiz moved this file itself and the same file is on disk",
            Self::SameFileObject => {
                "One file has this name, size and date; it existed before the old path was last seen, and the scan that saw the old path did not find it"
            }
            Self::MetadataMatch => {
                "One file matches on recorded facts only: the same name, size and date, or a move record that could not be confirmed"
            }
            Self::NameAndSize => {
                "A file has this name and size, but the date differs or there are several"
            }
            Self::NameAndSizeViaLink => {
                "A broken link has this name, and one file in the folder its target was moved to has this size"
            }
            Self::PresentAtRecordedPath => "The file is where the library says it is",
            Self::SizeOnlyCandidate => "One file of exactly this size exists under another name",
            Self::NameOnlyLead => "Something shares the name; that alone proves nothing",
            Self::BrokenLinkOnly => "Only a broken link remains, and its storage is gone",
            Self::NoTrace => "No move record and no file of this name",
        }
    }

    /// True when the record is tied to a file that exists now.
    pub fn tied_to_file(self) -> bool {
        matches!(
            self.class(),
            EvidenceClass::Confirmed
                | EvidenceClass::Strong
                | EvidenceClass::Metadata
                | EvidenceClass::Weak
        )
    }
}

/// One library record without a file, and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordFinding {
    pub set: RecordSet,
    pub archive_id: i64,
    #[serde(serialize_with = "lossy")]
    pub old_path: PathBuf,
    pub platform: Option<String>,
    pub archive_kind: String,
    pub size_bytes: Option<u64>,
    pub modified_unix_seconds: Option<i64>,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub flagged_missing_at: Option<String>,
    pub tier: EvidenceTier,
    pub class: EvidenceClass,
    pub code: &'static str,
    /// The file this record is tied to, or the candidate it points at.
    #[serde(serialize_with = "lossy_option")]
    pub target: Option<PathBuf>,
    /// The library row of `target`, when it has one.
    pub target_archive_id: Option<i64>,
    /// Plain statements of what was observed, strongest first.
    pub evidence: Vec<String>,
}

/// Why a link does not resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkCause {
    /// The top-level location no longer exists (a mount point or folder removed).
    LocationGone,
    /// The storage folder exists but is empty (emptied, or its mount is not attached).
    StorageEmpty,
    /// The folder exists and has content, but this file is not in it.
    FileNotInFolder,
    /// A folder on the way no longer exists (moved, renamed or removed).
    FolderGone,
    /// The link points back at itself through other links.
    LinkLoop,
    PermissionDenied,
    Other,
}

impl LinkCause {
    /// True when the storage behind the link is not there, as opposed to one
    /// file missing from a folder that is.
    pub fn storage_unavailable(self) -> bool {
        matches!(self, Self::LocationGone | Self::StorageEmpty)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LocationGone => "the location it points at no longer exists",
            Self::StorageEmpty => "the folder it points into is empty",
            Self::FileNotInFolder => "the folder exists but the file is not in it",
            Self::FolderGone => "a folder on the way no longer exists",
            Self::LinkLoop => "the link points back at itself",
            Self::PermissionDenied => "permission denied on the way to the target",
            Self::Other => "the target could not be read",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkGrade {
    Resolves,
    /// The target was found at the same relative path under a relocated
    /// folder, and that file is older than the link.
    TargetIdentified,
    /// A file exists where the target would be, but it is newer than the
    /// link, or only its name matches.
    PlausibleCandidate,
    Unresolved,
}

impl LinkGrade {
    pub fn label(self) -> &'static str {
        match self {
            Self::Resolves => "Works",
            Self::TargetIdentified => "Target identified",
            Self::PlausibleCandidate => "Plausible candidate",
            Self::Unresolved => "Unresolved",
        }
    }
}

/// One symbolic link found in the library folders. Links are never library
/// records, so they are counted apart from them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkFinding {
    #[serde(serialize_with = "lossy")]
    pub link: PathBuf,
    /// The link text exactly as stored.
    #[serde(serialize_with = "lossy")]
    pub stored_target: PathBuf,
    /// The absolute path that text names.
    #[serde(serialize_with = "lossy")]
    pub resolved_target: PathBuf,
    pub grade: LinkGrade,
    pub cause: Option<LinkCause>,
    #[serde(serialize_with = "lossy_option")]
    pub deepest_existing_ancestor: Option<PathBuf>,
    /// Where the target was found, or the candidate.
    #[serde(serialize_with = "lossy_option")]
    pub found: Option<PathBuf>,
    pub found_inode: Option<u64>,
    pub evidence: Vec<String>,
}

/// Links that broke for one reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkGroup {
    #[serde(serialize_with = "lossy")]
    pub link_folder: PathBuf,
    #[serde(serialize_with = "lossy")]
    pub target_root: PathBuf,
    pub grade: LinkGrade,
    pub cause: Option<LinkCause>,
    pub links: u64,
    #[serde(serialize_with = "lossy")]
    pub example_link: PathBuf,
    #[serde(serialize_with = "lossy")]
    pub example_target: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LinkSummary {
    pub total: u64,
    pub resolve: u64,
    pub broken: u64,
    pub target_identified: u64,
    pub plausible_candidate: u64,
    pub unresolved: u64,
    /// Broken because the storage the link points into is gone or empty
    /// (a drive or remote mount that is not attached, or a folder that moved).
    pub storage_unavailable: u64,
    /// Broken although the folder it points into is there: the file itself is gone.
    pub target_missing: u64,
    pub groups: Vec<LinkGroup>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TierCount {
    pub records: u64,
    pub recorded_bytes: u64,
}

/// Counts that must not be added together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Measures {
    pub rows_in_library: u64,
    pub rows_present_as_recorded: u64,
    pub rows_present_but_changed: u64,
    pub rows_present_folders: u64,
    pub rows_flagged_missing: u64,
    pub rows_absent_never_flagged: u64,
    /// Library rows that could not be judged: their folder is missing, empty
    /// (a drive that is not attached looks like that) or on a filesystem that
    /// was not entered. Nothing is claimed about them.
    pub rows_not_checked: u64,
    /// The same, for records in a Forget Missing export.
    pub forgotten_records_not_checked: u64,
    pub distinct_files_behind_present_rows: u64,
    pub bytes_of_distinct_present_files: u64,
    pub records_removed_by_forget_missing: u64,
    /// Links that are also library rows. They are counted as records, and
    /// shown here so a link is never mistaken for a second game.
    pub links_that_are_library_rows: u64,
    pub records_examined: u64,
    pub records_tied_to_a_file: u64,
    pub distinct_files_reached: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RootSummary {
    #[serde(serialize_with = "lossy")]
    pub root: PathBuf,
    #[serde(serialize_with = "lossy")]
    pub real_root: PathBuf,
    pub directories: u64,
    pub files: u64,
    pub links: u64,
    /// Folders on another filesystem that were not entered.
    #[serde(serialize_with = "lossy_list")]
    pub other_filesystems_not_entered: Vec<PathBuf>,
    pub errors: Vec<String>,
    pub complete: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InventorySummary {
    pub roots: Vec<RootSummary>,
    pub complete: bool,
    pub entries: u64,
    pub directories_listed: u64,
    pub seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReceiptSummary {
    #[serde(serialize_with = "lossy")]
    pub path: PathBuf,
    pub created_at: Option<String>,
    pub state: Option<String>,
    pub records: u64,
    /// False when the export names another database path (a copy is being read).
    pub written_for_this_database: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ContentCheckSummary {
    pub requested: bool,
    pub compared: u64,
    pub matched: u64,
    pub differed: u64,
    pub not_comparable: u64,
    pub bytes_read: u64,
}

/// The whole result. Read-only: producing it changes nothing and writes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReconciliationReport {
    pub schema_version: u32,
    #[serde(serialize_with = "lossy")]
    pub database_path: PathBuf,
    pub generated_at_unix: u64,
    pub inventory: InventorySummary,
    pub measures: Measures,
    /// Set label, then tier code, then the count.
    pub record_sets: BTreeMap<RecordSet, BTreeMap<&'static str, TierCount>>,
    pub by_class: BTreeMap<EvidenceClass, u64>,
    pub receipts: Vec<ReceiptSummary>,
    pub content_check: ContentCheckSummary,
    pub links: LinkSummary,
    pub records: Vec<RecordFinding>,
    pub link_findings: Vec<LinkFinding>,
    /// Things the reader must know before trusting a number above.
    pub notes: Vec<String>,
}
