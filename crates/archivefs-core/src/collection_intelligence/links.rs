//! Every symbolic link in the library folders: does it work, and if not, why,
//! and where did its target go?
//!
//! Read-only: link text and `lstat` results. A broken link is explained by the
//! deepest folder on its path that still exists. Its target is then looked for
//! at the same relative path under a nearby folder, which is what a moved or
//! renamed folder leaves behind. A file found that way counts as the target
//! only when it is older than the link; otherwise it is a later copy and stays
//! a candidate. A name on its own never identifies a target.

use super::inventory::{Entry, EntryKind, Inventory};
use super::model::{LinkCause, LinkFinding, LinkGrade, LinkGroup, LinkSummary};
use super::reconcile::looks_like_game_file;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// How far above the deepest existing folder a moved folder is looked for.
const MAX_LEVELS_UP: usize = 3;
/// How many folder pairs are remembered. Bounds the probing per link.
const MAX_LEARNED_FOLDERS: usize = 64;
/// How many trailing path components are tried under the link's own folder.
const MAX_SUFFIX_COMPONENTS: usize = 6;

fn absolute_target(link: &Path, stored: &Path) -> PathBuf {
    let joined = if stored.is_absolute() {
        stored.to_path_buf()
    } else {
        link.parent().unwrap_or(Path::new("/")).join(stored)
    };
    // Lexical only: nothing is resolved through other links.
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn deepest_existing(target: &Path) -> (PathBuf, Option<std::io::ErrorKind>) {
    let mut current = PathBuf::from("/");
    for component in target.components().skip(1) {
        let next = current.join(component.as_os_str());
        match fs::symlink_metadata(&next) {
            Ok(_) => current = next,
            Err(error) => return (current, Some(error.kind())),
        }
    }
    (current, None)
}

fn regular_file(path: &Path) -> Option<fs::Metadata> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
}

fn born(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .created()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

fn depth(path: &Path) -> usize {
    path.components().count()
}

/// Audits every link in `inventory`. Findings are in path order.
pub fn audit_links(inventory: &Inventory, cancel: &AtomicBool) -> (Vec<LinkFinding>, LinkSummary) {
    let mut by_name: HashMap<&OsStr, Vec<&Entry>> = HashMap::new();
    for entry in &inventory.entries {
        if entry.kind == EntryKind::File
            && looks_like_game_file(&entry.path)
            && let Some(name) = entry.path.file_name()
        {
            by_name.entry(name).or_default().push(entry);
        }
    }
    // (folder the broken targets were under, folder found instead, was it a moved folder)
    let mut learned: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    let mut emptiness: HashMap<PathBuf, Option<bool>> = HashMap::new();
    let mut findings = Vec::new();
    for entry in &inventory.entries {
        if entry.kind != EntryKind::Link {
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let stored = entry.link_target.clone().unwrap_or_default();
        let resolved = absolute_target(&entry.path, &stored);
        let mut finding = LinkFinding {
            link: entry.path.clone(),
            stored_target: stored,
            resolved_target: resolved.clone(),
            grade: LinkGrade::Resolves,
            cause: None,
            deepest_existing_ancestor: None,
            found: None,
            found_inode: None,
            evidence: Vec::new(),
        };
        let (failure, looped) = match fs::metadata(&entry.path) {
            Ok(_) => {
                findings.push(finding);
                continue;
            }
            Err(error) => (error.kind(), error.raw_os_error() == Some(libc::ELOOP)),
        };
        let (ancestor, _) = deepest_existing(&resolved);
        let missing = resolved
            .strip_prefix(&ancestor)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let missing_components = depth(&missing);
        // Is the folder empty? One entry is enough to know, and the answer is
        // remembered: thousands of links often point into the same folder.
        let ancestor_empty = *emptiness.entry(ancestor.clone()).or_insert_with(|| {
            fs::read_dir(&ancestor)
                .ok()
                .map(|mut reader| reader.next().is_none())
        });
        finding.cause = Some(if looped {
            LinkCause::LinkLoop
        } else if failure == std::io::ErrorKind::PermissionDenied {
            LinkCause::PermissionDenied
        } else if depth(&ancestor) <= 2 {
            LinkCause::LocationGone
        } else if ancestor_empty == Some(true) {
            LinkCause::StorageEmpty
        } else if missing_components == 1 {
            LinkCause::FileNotInFolder
        } else if missing_components > 1 {
            LinkCause::FolderGone
        } else {
            LinkCause::Other
        });
        finding.deepest_existing_ancestor = Some(ancestor.clone());

        // Where would the target be if its folder had been moved? Two shapes
        // are looked for, and they are not equally strong.
        //   moved: the missing part of the path reappears under a parent of
        //          the deepest folder that still exists. The folder was moved
        //          up, and a file there that is older than the link is the
        //          link's target.
        //   beside: the tail of the old path exists under the link's own
        //          folder. Whatever its age, that file was never shown to be
        //          what the link pointed at; it is a candidate only.
        let mut candidate: Option<(PathBuf, fs::Metadata, PathBuf, PathBuf, bool)> = None;
        let mut consider = |path: PathBuf, old: PathBuf, new: PathBuf, moved: bool| {
            if candidate.is_none()
                && path != entry.path
                && let Some(metadata) = regular_file(&path)
            {
                candidate = Some((path, metadata, old, new, moved));
            }
        };
        for (old, new, moved) in &learned {
            if let Ok(rest) = resolved.strip_prefix(old) {
                consider(new.join(rest), old.clone(), new.clone(), *moved);
            }
        }
        // Only when a whole folder is gone, so a single missing file never
        // sends probes around the tree.
        if missing_components >= 2 {
            for base in ancestor.ancestors().skip(1).take(MAX_LEVELS_UP) {
                if depth(base) < 3 {
                    break;
                }
                consider(
                    base.join(&missing),
                    ancestor.clone(),
                    base.to_path_buf(),
                    true,
                );
            }
        }
        if let Some(link_dir) = entry.path.parent() {
            let parts: Vec<_> = resolved.components().collect();
            for take in (2..=MAX_SUFFIX_COMPONENTS.min(parts.len().saturating_sub(1))).rev() {
                let tail: PathBuf = parts[parts.len() - take..].iter().collect();
                let old: PathBuf = parts[..parts.len() - take].iter().collect();
                consider(link_dir.join(&tail), old, link_dir.to_path_buf(), false);
            }
        }
        if let Some((path, metadata, old, new, moved)) = candidate {
            if learned.len() < MAX_LEARNED_FOLDERS
                && !learned.iter().any(|(a, b, _)| *a == old && *b == new)
            {
                learned.push((old.clone(), new.clone(), moved));
            }
            let link_made = entry.mtime;
            // `None` when the filesystem does not record creation times.
            let older = born(&metadata).map(|born| born < link_made);
            finding.grade = if moved && older == Some(true) {
                LinkGrade::TargetIdentified
            } else {
                LinkGrade::PlausibleCandidate
            };
            finding.evidence.push(format!(
                "a file exists at the same relative path under {} (the link names {})",
                new.display(),
                old.display()
            ));
            // Only what was observed is said: where the file is, and whether
            // its inode is older than the link. Where the file used to be is
            // not known.
            let place = if moved {
                "that folder is a parent of where the link points"
            } else {
                "that file is in the link's own folder, not in a folder its target is known to have moved to"
            };
            finding.evidence.push(match older {
                Some(true) if moved => {
                    format!("{place}, and the file was created before the link was made")
                }
                Some(true) => format!(
                    "{place}; it was created before the link was made, which does not show that the link pointed at it"
                ),
                Some(false) => format!(
                    "{place}; it was created after the link was made, so it is a later copy, not the original target"
                ),
                None => format!(
                    "{place}; when it was created is not recorded, so it cannot be told from a later copy"
                ),
            });
            finding.found_inode = Some(metadata.ino());
            finding.found = Some(path);
        } else {
            let named = resolved
                .file_name()
                .and_then(|name| by_name.get(name))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            match named {
                [] => finding.grade = LinkGrade::Unresolved,
                [one] => {
                    finding.grade = LinkGrade::PlausibleCandidate;
                    finding.found = Some(one.path.clone());
                    finding.found_inode = Some(one.inode);
                    finding.evidence.push(
                        "one file in the library has the same name; nothing else is known about the target".into(),
                    );
                }
                several => {
                    finding.grade = LinkGrade::PlausibleCandidate;
                    finding.evidence.push(format!(
                        "{} files in the library have the same name",
                        several.len()
                    ));
                }
            }
        }
        findings.push(finding);
    }

    let mut summary = LinkSummary {
        total: findings.len() as u64,
        ..LinkSummary::default()
    };
    let mut groups: BTreeMap<(PathBuf, PathBuf, LinkGrade, Option<LinkCause>), LinkGroup> =
        BTreeMap::new();
    for finding in &findings {
        match finding.grade {
            LinkGrade::Resolves => summary.resolve += 1,
            LinkGrade::TargetIdentified => summary.target_identified += 1,
            LinkGrade::PlausibleCandidate => summary.plausible_candidate += 1,
            LinkGrade::Unresolved => summary.unresolved += 1,
        }
        match finding.cause {
            Some(cause) if cause.storage_unavailable() => summary.storage_unavailable += 1,
            Some(_) => summary.target_missing += 1,
            None => {}
        }
        let link_folder = finding
            .link
            .parent()
            .unwrap_or(Path::new("/"))
            .to_path_buf();
        let target_root: PathBuf = finding.resolved_target.components().take(5).collect();
        let key = (
            link_folder.clone(),
            target_root.clone(),
            finding.grade,
            finding.cause,
        );
        groups
            .entry(key)
            .and_modify(|group| group.links += 1)
            .or_insert_with(|| LinkGroup {
                link_folder,
                target_root,
                grade: finding.grade,
                cause: finding.cause,
                links: 1,
                example_link: finding.link.clone(),
                example_target: finding.resolved_target.clone(),
            });
    }
    summary.broken = summary.total - summary.resolve;
    summary.groups = groups.into_values().collect();
    summary.groups.sort_by(|a, b| {
        b.links
            .cmp(&a.links)
            .then(a.link_folder.cmp(&b.link_folder))
    });
    (findings, summary)
}
