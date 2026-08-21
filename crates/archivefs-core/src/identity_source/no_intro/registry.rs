//! Resolves the persisted DAT source registry (`crate::dat::sources`) down to
//! the No-Intro identity relevant to one platform, honestly.
//!
//! # Why this is not `dat::policy`
//!
//! [`crate::dat::policy::evaluate`] ranks already-verified candidates for
//! planning and renaming, and its source ordering is a deliberate tie-break
//! by user-assigned priority. This module answers a narrower, earlier
//! question for the evidence panel: "does exactly one enabled, platform-
//! relevant registered source identify itself as No-Intro?" It never
//! consults priority to break a tie, because doing so would be exactly the
//! planner's job, done early and silently. When more than one source
//! qualifies, that is reported as [`NoIntroSourceSelection::Ambiguous`]
//! rather than resolved.
//!
//! # Reuse
//!
//! Every DAT this module opens is opened through
//! [`super::import::import_no_intro_dat`], unchanged. A source that is
//! enabled and relevant to the platform but does not identify as No-Intro
//! (wrong ecosystem, unreadable, unparseable) is simply not a candidate here
//! - it is reported elsewhere, by DAT Sources validation.
//!
//! # Not free
//!
//! [`select_no_intro_source`] parses and hashes every candidate DAT file. It
//! must not be called on every frame or every selection change; pair it with
//! [`no_intro_selection_fingerprint`], which is cheap, to decide whether the
//! registry state this resolution depends on has actually changed.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use crate::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry, discover_dat_files};

use super::import::{ImportedNoIntroSource, NoIntroImportError, import_no_intro_dat};

/// One DAT file drawn from an enabled, platform-relevant registry entry. A
/// file source contributes itself; a folder source contributes each `.dat`
/// file discovered directly inside it (never recursively - see
/// `dat::sources::validation`'s own rule for why).
struct Candidate {
    source_id: String,
    display_name: String,
    path: PathBuf,
}

fn candidates_for(entries: &[&DatSourceEntry]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for entry in entries {
        match entry.kind {
            DatSourceKind::File => out.push(Candidate {
                source_id: entry.id.clone(),
                display_name: entry.display_name.clone(),
                path: entry.path.clone(),
            }),
            DatSourceKind::Folder => {
                // An unreadable or refused folder contributes no candidates
                // here; that failure is already surfaced by DAT Sources
                // validation, and is not this module's job to repeat.
                if let Ok(scan) = discover_dat_files(&entry.path) {
                    for file in scan.files {
                        out.push(Candidate {
                            source_id: entry.id.clone(),
                            display_name: entry.display_name.clone(),
                            path: file,
                        });
                    }
                }
            }
        }
    }
    out
}

/// One enabled, platform-relevant source that identified as No-Intro, kept
/// as a label (not a re-parseable handle) so an ambiguity report can name
/// every competing source without holding more than one parsed DAT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoIntroSourceLabel {
    pub source_id: String,
    pub display_name: String,
    pub artifact_path: PathBuf,
}

/// What the registry currently says about No-Intro identity for one
/// platform (or, with `platform_id: None`, globally).
#[derive(Debug, Clone)]
pub enum NoIntroSourceSelection {
    /// No enabled, platform-relevant registered source identifies itself as
    /// No-Intro. Distinct from "no sources at all are registered" - a
    /// registry full of TOSEC sources for this platform lands here too, and
    /// that is the honest answer.
    NotImported,
    /// Exactly one does. Already imported and ready to look hashes up in.
    Selected(Box<ImportedNoIntroSource>),
    /// More than one does. Never resolved to a first or a "best" one - see
    /// the module's own doc comment for why.
    Ambiguous(Vec<NoIntroSourceLabel>),
}

/// Resolves the enabled, platform-relevant registered DAT sources to their
/// No-Intro identity, or the honest absence/ambiguity of one.
///
/// Performs file I/O, parsing, and hashing for every candidate - this is not
/// a UI-thread-safe operation to call unconditionally. Callers should gate
/// repeat calls behind [`no_intro_selection_fingerprint`] (the GUI's
/// `selected_evidence_no_intro` cache does exactly this).
pub fn select_no_intro_source(
    registry: &DatSourceRegistry,
    platform_id: Option<&str>,
) -> NoIntroSourceSelection {
    let entries = match platform_id {
        Some(id) => registry.sorted_enabled_for_platform(id),
        None => registry.sorted_enabled(),
    };

    let mut matches: Vec<(NoIntroSourceLabel, ImportedNoIntroSource)> = Vec::new();
    for candidate in candidates_for(&entries) {
        match import_no_intro_dat(&candidate.path) {
            Ok(imported) => matches.push((
                NoIntroSourceLabel {
                    source_id: candidate.source_id,
                    display_name: candidate.display_name,
                    artifact_path: candidate.path,
                },
                imported,
            )),
            Err(NoIntroImportError::NotNoIntro { .. } | NoIntroImportError::Io { .. }) => {}
            Err(NoIntroImportError::Parse(_)) => {}
        }
    }

    match matches.len() {
        0 => NoIntroSourceSelection::NotImported,
        1 => {
            let (_, imported) = matches.into_iter().next().expect("length checked above");
            NoIntroSourceSelection::Selected(Box::new(imported))
        }
        _ => {
            let mut labels: Vec<NoIntroSourceLabel> =
                matches.into_iter().map(|(label, _)| label).collect();
            labels.sort_by(|a, b| {
                a.source_id
                    .cmp(&b.source_id)
                    .then_with(|| a.artifact_path.cmp(&b.artifact_path))
            });
            NoIntroSourceSelection::Ambiguous(labels)
        }
    }
}

/// A cheap fingerprint of exactly the registry state
/// [`select_no_intro_source`] depends on for `platform_id`: which entries are
/// enabled and relevant, their kind, path and platform assignment, and (best
/// effort, for change detection only) the on-disk size/mtime of a file
/// source or the mtime of a folder source's directory entry.
///
/// Two calls with an equal fingerprint are expected to resolve identically
/// without re-parsing anything. This is a change-detection heuristic, not a
/// content hash: editing a file *inside* an already-registered folder
/// without changing the folder's own mtime (some tools preserve it) will not
/// be caught here. It is caught the next time the process reads the
/// directory listing for an unrelated reason, and by an explicit
/// "Validate"/reload in DAT Sources, which always re-parses.
pub fn no_intro_selection_fingerprint(
    registry: &DatSourceRegistry,
    platform_id: Option<&str>,
) -> u64 {
    let entries = match platform_id {
        Some(id) => registry.sorted_enabled_for_platform(id),
        None => registry.sorted_enabled(),
    };

    let mut hasher = DefaultHasher::new();
    platform_id.hash(&mut hasher);
    entries.len().hash(&mut hasher);
    for entry in entries {
        entry.id.hash(&mut hasher);
        entry.path.hash(&mut hasher);
        matches!(entry.kind, DatSourceKind::Folder).hash(&mut hasher);
        entry.platform.hash(&mut hasher);
        match std::fs::metadata(&entry.path) {
            Ok(metadata) => {
                metadata.len().hash(&mut hasher);
                match metadata.modified() {
                    Ok(modified) => modified.hash(&mut hasher),
                    Err(_) => "unknown-mtime".hash(&mut hasher),
                }
            }
            Err(_) => "unreadable".hash(&mut hasher),
        }
    }
    hasher.finish()
}
