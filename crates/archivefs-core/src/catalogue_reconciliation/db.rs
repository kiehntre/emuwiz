//! Joins the catalogue to the filesystem, read-only: derives each row's
//! presence under the same authority rules Catalogue Health uses (a row is
//! `Missing` only when its source is healthy and bound), walks the configured
//! roots, and hands both to [`super::reconcile`].
use super::walk::{MountOf, RootWalk, RootWalkState, WalkLimits, WalkRoot, walk_roots};
use super::{
    CompanionBasis, CompanionLink, FileFacts, ReconciliationReport, RowFacts, RowPresence,
    StrongHash, UnprovableReason, attach_hashes, reconcile_with_companions,
};
use crate::Result;
use crate::catalogue_health::{BoundRoot, SourceRootBinding, observe_path};
use crate::database::Database;
use crate::emulator_environment::FsProbe;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryReconciliation {
    pub report: ReconciliationReport,
    /// How completely each root was walked; a `Partial` root means the file
    /// list for it is a lower bound, never a proof of absence.
    pub roots: Vec<RootWalk>,
    /// Display paths of the sources in `roots`, by source id.
    pub root_paths: BTreeMap<i64, PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationOutcome {
    Complete(Box<LibraryReconciliation>),
    /// Cancelled part-way: no report is produced from a partial walk.
    Cancelled,
}

enum SourceState {
    Ready {
        root: BoundRoot,
        excluded: Vec<PathBuf>,
        unproven: Vec<PathBuf>,
    },
    Unusable(UnprovableReason),
}

/// `supplied_hashes` are results of an earlier explicit hashing operation (see
/// [`super::hashing`]); they are the only way a physical file gains a hash.
pub fn reconcile_library(
    database: &Database,
    configured_roots: &[PathBuf],
    supplied_hashes: &BTreeMap<PathBuf, Vec<StrongHash>>,
    cancel: &AtomicBool,
    mount_of: MountOf<'_>,
) -> Result<ReconciliationOutcome> {
    let sources = database.reconciliation_sources()?;
    let mut states: HashMap<i64, SourceState> = HashMap::new();
    let mut walk_roots_in: Vec<WalkRoot> = Vec::new();
    let archives = database.load_archives()?;
    let hashes = database.reconciliation_row_hashes()?;
    for source in &sources {
        let active = source.games_role
            && !source.removed_from_config
            && configured_roots.contains(&source.path);
        if !active {
            states.insert(
                source.id,
                SourceState::Unusable(UnprovableReason::StorageUnavailable),
            );
            continue;
        }
        let bound = BoundRoot::open(&source.path).filter(|root| {
            root.current()
                && observe_path(&source.path, true).is_present()
                && std::fs::read_dir(&source.path).is_ok()
        });
        let Some(root) = bound else {
            states.insert(
                source.id,
                SourceState::Unusable(UnprovableReason::StorageUnavailable),
            );
            continue;
        };
        // Storage that is not the storage a person reviewed is never trusted.
        let continuous = match database.catalogue_source_binding(source.id)? {
            Some((recorded, _)) => {
                SourceRootBinding::inspect(&source.path)
                    .and_then(|binding| serde_json::to_string(&binding).ok())
                    .as_deref()
                    == Some(recorded.as_str())
            }
            None => true,
        };
        if !continuous {
            states.insert(
                source.id,
                SourceState::Unusable(UnprovableReason::SourceNeedsReview),
            );
            continue;
        }
        let unproven = database
            .preview_unproven_nested_boundaries(source.id, &root)?
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        let mut excluded: Vec<PathBuf> = configured_roots
            .iter()
            .filter(|other| *other != &source.path && other.starts_with(&source.path))
            .cloned()
            .collect();
        // An extracted arcade set directory is one logical item, not many files.
        excluded.extend(
            archives
                .iter()
                .filter(|a| {
                    a.source_folder_id == source.id && a.archive_kind == "arcade_set_directory"
                })
                .map(|a| a.absolute_path.clone()),
        );
        walk_roots_in.push(WalkRoot {
            source_id: source.id,
            path: source.path.clone(),
            excluded: excluded.clone(),
        });
        states.insert(
            source.id,
            SourceState::Ready {
                root,
                excluded,
                unproven,
            },
        );
    }

    let mut walk = walk_roots(&walk_roots_in, WalkLimits::default(), cancel, mount_of);
    if walk.cancelled {
        return Ok(ReconciliationOutcome::Cancelled);
    }
    // A source that could not be walked is reported, never silently omitted.
    for source in &sources {
        if matches!(states.get(&source.id), Some(SourceState::Unusable(_))) {
            walk.roots.push(RootWalk {
                source_id: source.id,
                state: RootWalkState::Unavailable,
                nested_boundaries: Vec::new(),
                inaccessible: Vec::new(),
                truncated: false,
            });
        }
    }
    walk.roots.sort_by_key(|root| root.source_id);

    let rows: Vec<RowFacts> = archives
        .iter()
        .map(|archive| RowFacts {
            archive_id: archive.id,
            source_id: archive.source_folder_id,
            path: archive.absolute_path.clone(),
            size: archive.size_bytes,
            platform: archive.platform.clone(),
            presence: presence(states.get(&archive.source_folder_id), archive),
            hashes: hashes.get(&archive.id).cloned().unwrap_or_default(),
        })
        .collect();
    let mut files: Vec<FileFacts> = walk.files;
    attach_hashes(&mut files, supplied_hashes);
    let Some(links) = companion_links(&rows, cancel) else {
        return Ok(ReconciliationOutcome::Cancelled);
    };
    Ok(ReconciliationOutcome::Complete(Box::new(
        LibraryReconciliation {
            report: reconcile_with_companions(&rows, &files, &links),
            roots: walk.roots,
            root_paths: sources.iter().map(|s| (s.id, s.path.clone())).collect(),
        },
    )))
}

fn presence(
    state: Option<&SourceState>,
    archive: &crate::database::PersistedArchive,
) -> RowPresence {
    let Some(state) = state else {
        return RowPresence::Unprovable(UnprovableReason::StorageUnavailable);
    };
    let SourceState::Ready {
        root,
        excluded,
        unproven,
    } = state
    else {
        let SourceState::Unusable(reason) = state else {
            unreachable!()
        };
        return RowPresence::Unprovable(*reason);
    };
    let under = |prefixes: &[PathBuf]| {
        prefixes
            .iter()
            .any(|p| path_under(&archive.absolute_path, p))
    };
    if under(excluded) && archive.archive_kind != "arcade_set_directory" || under(unproven) {
        return RowPresence::Unprovable(UnprovableReason::NestedBoundaryUnproven);
    }
    let observation = root.probe(
        &archive.absolute_path,
        archive.archive_kind == "arcade_set_directory",
    );
    match observation.probe {
        FsProbe::PresentFile | FsProbe::PresentDirectory => RowPresence::Present,
        FsProbe::Missing => RowPresence::Missing,
        _ => RowPresence::Unprovable(UnprovableReason::Inaccessible),
    }
}

fn path_under(path: &Path, prefix: &Path) -> bool {
    path.starts_with(prefix)
}

/// Resolves each *present* catalogued CUE/GDI descriptor once, through the
/// existing ingestion resolvers (same bounded read, same path-safety rules: no
/// absolute or `..` references, the canonical result must stay beneath the
/// descriptor's own directory). References that are missing or unsafe resolve
/// to nothing, so they are never accepted as companions. Only descriptors
/// (<= 256 KiB) are read; no game content is. `None` when cancelled.
fn companion_links(rows: &[RowFacts], cancel: &AtomicBool) -> Option<Vec<CompanionLink>> {
    use std::sync::atomic::Ordering;
    let mut links = Vec::new();
    for row in rows.iter().filter(|r| r.presence == RowPresence::Present) {
        let Some((basis, files)) = super::descriptor_companions(&row.path) else {
            continue;
        };
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if !files.is_empty() {
            links.push(CompanionLink {
                parent_archive_id: row.archive_id,
                basis,
                files,
            });
        }
    }
    Some(links)
}
