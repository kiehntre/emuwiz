//! Nested FUSE mounts inside one source during a scan: whether each one is
//! trusted, what was proven about it this run, and which of its catalogue
//! rows Missing reconciliation may touch.
//!
//! Rules (all fail closed):
//! - A mount is trusted only when its stable identity (see
//!   [`crate::catalogue_health::StableAreaIdentity`]) equals the one recorded
//!   by an earlier complete scan. A normal restart keeps that identity, so it
//!   is re-accepted automatically; the transient kernel mount ID and device
//!   number only prove "same mount instance" within this scan.
//! - A new, changed or unidentifiable mount is still read: its games are
//!   indexed (fresh inspection, no cached observations), its new identity is
//!   recorded, and nothing beneath it is reconciled as missing this run.
//! - A mount that is gone, unsupported, incomplete, or replaced while the
//!   scan runs is excluded: its rows are left exactly as they were.
use super::*;
use crate::catalogue_health::{
    BoundRoot, LiveArea, NestedBoundaryRecord, StableAreaIdentity, open_area, stable_identity,
};
use serde::Serialize;

/// What one scan established about one nested mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NestedMountState {
    /// Same trusted mount as last time; fully reconciled.
    Verified,
    /// Restarted since last time with the same stable identity; fully
    /// reconciled.
    Restarted,
    /// First time seen; games indexed, missing reconciliation from the next
    /// scan.
    New,
    /// Content or mount configuration differs from what was recorded; games
    /// re-read and indexed, missing reconciliation from the next scan.
    Changed,
    /// Some entries could not be read; what was read is indexed, nothing
    /// reconciled.
    Incomplete,
    /// A remembered mount is not mounted now; its entries are kept.
    Unavailable,
    /// Not a FUSE archive mount; not entered, entries kept.
    Unsupported,
    /// Replaced or removed while the scan ran; nothing from it saved.
    Failed,
}

impl NestedMountState {
    pub fn reconciled(self) -> bool {
        matches!(self, Self::Verified | Self::Restarted)
    }
    pub fn indexed(self) -> bool {
        matches!(
            self,
            Self::Verified | Self::Restarted | Self::New | Self::Changed | Self::Incomplete
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NestedMountOutcome {
    pub source: PathBuf,
    pub path: PathBuf,
    pub state: NestedMountState,
    /// Games found inside this run (0 when it was not read).
    pub archives: usize,
    pub detail: Option<String>,
}

/// Pre-discovery decision for one remembered mount.
pub(super) enum Trust {
    /// Stable identity matched, observed on this mount instance.
    Trusted {
        identity: StableAreaIdentity,
        mount_id: u64,
    },
    Untrusted(&'static str),
}

pub(super) struct AreaPlan {
    path: PathBuf,
    record: Option<NestedBoundaryRecord>,
    trust: Trust,
}

/// Everything persistence and reconciliation need from this module.
#[derive(Default)]
pub(super) struct SettledAreas {
    pub(super) outcomes: Vec<NestedMountOutcome>,
    /// Paths beneath which Missing must not be written this run.
    pub(super) excluded: Vec<PathBuf>,
    /// Areas whose discovered games must not be saved (not this mount).
    pub(super) dropped: Vec<PathBuf>,
    /// Pinned roots every saved game beneath an area must be resolved from.
    pub(super) owners: Vec<(PathBuf, (u64, u64))>,
    records: Vec<(PathBuf, NestedBoundaryRecord)>,
}

impl SettledAreas {
    /// The pinned area owning `path`, if any (innermost wins).
    pub(super) fn owner_of(&self, path: &Path) -> Option<&(PathBuf, (u64, u64))> {
        self.owners
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.as_os_str().len())
    }

    pub(super) fn is_dropped(&self, path: &Path) -> bool {
        self.dropped.iter().any(|root| path.starts_with(root))
    }

    /// Every owning area is still the exact mount it was when settled.
    pub(super) fn owners_current(&self) -> bool {
        self.owners.iter().all(|(root, identity)| {
            crate::catalogue_health::source_root_identity(root) == Some(*identity)
        })
    }
}

fn remembered(
    database: &Database,
    source: i64,
    root: &BoundRoot,
) -> Result<Vec<(PathBuf, Option<String>)>> {
    let mut stmt = database
        .connection
        .prepare("SELECT relative_path,binding_json FROM source_nested_boundaries WHERE source_folder_id=?1 ORDER BY relative_path")
        .map_err(|e| db_error("prepare remembered nested mounts", e))?;
    let rows: Vec<(Vec<u8>, Option<String>)> = stmt
        .query_map([source], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| db_error("read remembered nested mounts", e))?
        .collect::<rusqlite::Result<_>>()
        .map_err(|e| db_error("decode remembered nested mounts", e))?;
    Ok(rows
        .into_iter()
        .map(|(relative, json)| {
            (
                root.root_path()
                    .join(PathBuf::from(OsString::from_vec(relative))),
                json,
            )
        })
        .collect())
}

/// Before discovery: which remembered mounts are the same trusted mount.
pub(super) fn plan_areas(
    database: &Database,
    source: i64,
    root: &BoundRoot,
) -> Result<Vec<AreaPlan>> {
    let mut plans = Vec::new();
    for (path, json) in remembered(database, source, root)? {
        let record = json.as_deref().and_then(NestedBoundaryRecord::decode);
        let trust = match record.as_ref().and_then(|record| record.stable.as_ref()) {
            None => Trust::Untrusted("no identity recorded yet"),
            Some(expected) => match open_area(root, &path) {
                Err(reason) => Trust::Untrusted(reason),
                Ok(live) => match stable_identity(root, &live) {
                    Ok(identity) if &identity == expected => Trust::Trusted {
                        identity,
                        mount_id: live.mount_id,
                    },
                    Ok(_) => Trust::Untrusted("content or mount configuration changed"),
                    Err(reason) => Trust::Untrusted(reason),
                },
            },
        };
        plans.push(AreaPlan {
            path,
            record,
            trust,
        });
    }
    Ok(plans)
}

pub(super) fn trusted_paths(plans: &[AreaPlan]) -> Vec<PathBuf> {
    plans
        .iter()
        .filter(|plan| matches!(plan.trust, Trust::Trusted { .. }))
        .map(|plan| plan.path.clone())
        .collect()
}

/// After discovery (and after `record_nested_boundaries`): decide each
/// mount's outcome against the live filesystem.
pub(super) fn settle_areas(
    database: &Database,
    source: i64,
    root: &BoundRoot,
    plans: &[AreaPlan],
    discovered: &[crate::DiscoveredNestedArea],
) -> Result<SettledAreas> {
    let mut settled = SettledAreas::default();
    let mut outcome_of = vec![None::<NestedMountState>; discovered.len()];
    for (index, area) in discovered.iter().enumerate() {
        let parent_unsaved = area
            .parent_area
            .and_then(|parent| outcome_of[parent])
            .is_some_and(|state| !state.indexed());
        let plan = plans.iter().find(|plan| plan.path == area.path);
        // Trusted before discovery, but discovery read another instance: the
        // walk may have reused cached observations that belong to the first.
        let restarted_mid_scan = matches!(
            plan.map(|plan| &plan.trust),
            Some(Trust::Trusted { mount_id, .. }) if *mount_id != area.mount_id
        );
        let (state, detail, live) = if restarted_mid_scan {
            (
                NestedMountState::Failed,
                Some("the mount restarted while it was being scanned".to_string()),
                None,
            )
        } else if parent_unsaved {
            (
                NestedMountState::Failed,
                Some("the mount it is inside was not saved".to_string()),
                None,
            )
        } else if !area.supported {
            (
                NestedMountState::Unsupported,
                Some("only FUSE archive mounts are scanned inside a source".into()),
                None,
            )
        } else {
            match open_area(root, &area.path) {
                Err(reason) => (
                    NestedMountState::Failed,
                    Some(format!("{reason} after it was read")),
                    None,
                ),
                Ok(live)
                    if live.mount_id != area.mount_id
                        || live.binding().device != area.device
                        || live.binding().inode != area.inode =>
                {
                    (
                        NestedMountState::Failed,
                        Some("the mount was replaced while it was being scanned".into()),
                        None,
                    )
                }
                Ok(live) => {
                    let (state, detail) = live_outcome(root, area, plan, &live, &mut settled);
                    (state, detail, Some(live))
                }
            }
        };
        outcome_of[index] = Some(state);
        if let Some(live) = &live {
            settled.owners.push((
                area.path.clone(),
                (live.binding().device, live.binding().inode),
            ));
        }
        if !state.indexed() {
            settled.dropped.push(area.path.clone());
        }
        if !state.reconciled() {
            settled.excluded.push(area.path.clone());
        }
        settled.outcomes.push(NestedMountOutcome {
            source: root.root_path().to_path_buf(),
            path: area.path.clone(),
            state,
            archives: if state.indexed() { area.archives } else { 0 },
            detail,
        });
    }
    // Remembered mounts this walk never met (unmounted, so the walk saw an
    // ordinary folder or nothing there) and quarantined records.
    for (path, _) in remembered(database, source, root)? {
        if discovered.iter().any(|area| area.path == path) {
            continue;
        }
        settled.excluded.push(path.clone());
        settled.outcomes.push(NestedMountOutcome {
            source: root.root_path().to_path_buf(),
            path,
            state: NestedMountState::Unavailable,
            archives: 0,
            detail: Some("not mounted now; its games were kept as they were".into()),
        });
    }
    settled.excluded.sort();
    settled.excluded.dedup();
    Ok(settled)
}

fn live_outcome(
    root: &BoundRoot,
    area: &crate::DiscoveredNestedArea,
    plan: Option<&AreaPlan>,
    live: &LiveArea,
    settled: &mut SettledAreas,
) -> (NestedMountState, Option<String>) {
    if !area.is_complete() {
        let first = area
            .scan_errors
            .first()
            .map(|error| format!("{}: {}", error.path.display(), error.message))
            .unwrap_or_default();
        return (
            NestedMountState::Incomplete,
            Some(format!(
                "{} entr{} could not be read ({first})",
                area.scan_errors_total,
                if area.scan_errors_total == 1 {
                    "y"
                } else {
                    "ies"
                }
            )),
        );
    }
    if let Some(AreaPlan {
        trust: Trust::Trusted { identity, mount_id },
        record,
        ..
    }) = plan
        && *mount_id == live.mount_id
    {
        // Same stable identity on this very instance. A different recorded
        // instance means the mount restarted since the last scan.
        let restarted = record
            .as_ref()
            .is_none_or(|record| record.mount_id != Some(live.mount_id));
        settled.records.push((
            area.path.clone(),
            NestedBoundaryRecord {
                binding: live.binding().clone(),
                stable: Some(identity.clone()),
                mount_id: Some(live.mount_id),
            },
        ));
        return (
            if restarted {
                NestedMountState::Restarted
            } else {
                NestedMountState::Verified
            },
            None,
        );
    }
    let had_identity = plan
        .and_then(|plan| plan.record.as_ref())
        .is_some_and(|record| record.stable.is_some());
    let why = match plan.map(|plan| &plan.trust) {
        Some(Trust::Untrusted(reason)) => Some(*reason),
        Some(Trust::Trusted { .. }) => Some("the mount restarted since it was identified"),
        None => None,
    };
    match stable_identity(root, live) {
        Ok(identity) => {
            settled.records.push((
                area.path.clone(),
                NestedBoundaryRecord {
                    binding: live.binding().clone(),
                    stable: Some(identity),
                    mount_id: Some(live.mount_id),
                },
            ));
            if had_identity {
                (
                    NestedMountState::Changed,
                    Some(format!(
                        "{}; games were re-read, and missing games are checked from the next scan",
                        why.unwrap_or("identity changed")
                    )),
                )
            } else {
                (
                    NestedMountState::New,
                    Some("missing games are checked from the next scan".into()),
                )
            }
        }
        Err(reason) => (
            NestedMountState::Incomplete,
            Some(format!("its identity could not be proven ({reason})")),
        ),
    }
}

/// Writes the identities settled this run, each only while the live mount is
/// still the one it describes. Runs inside the folder's refresh savepoint.
pub(super) fn record_identities(
    database: &Database,
    source: i64,
    root: &BoundRoot,
    settled: &SettledAreas,
) -> Result<()> {
    for (path, record) in &settled.records {
        let live = open_area(root, path).map_err(|reason| {
            ArchiveFsError::Database(format!(
                "nested mount {} {reason} before its identity was saved",
                path.display()
            ))
        })?;
        if live.binding() != &record.binding || Some(live.mount_id) != record.mount_id {
            return Err(ArchiveFsError::Database(format!(
                "nested mount {} changed before its identity was saved",
                path.display()
            )));
        }
        let relative = path
            .strip_prefix(root.root_path())
            .map_err(|_| ArchiveFsError::Database("nested mount outside its source".into()))?;
        let json =
            serde_json::to_string(record).map_err(|e| ArchiveFsError::Database(e.to_string()))?;
        database
            .connection
            .execute(
                "INSERT INTO source_nested_boundaries(source_folder_id,relative_path,binding_json) VALUES(?1,?2,?3) \
                 ON CONFLICT(source_folder_id,relative_path) DO UPDATE SET binding_json=excluded.binding_json",
                params![source, relative.as_os_str().as_bytes(), json],
            )
            .map_err(|e| db_error("record nested mount identity", e))?;
    }
    Ok(())
}

/// Lets a test act on the filesystem between discovery and settlement, the
/// window in which a mount can be replaced or removed mid-scan.
#[cfg(test)]
pub(crate) mod test_hook {
    use std::cell::RefCell;
    thread_local!(static AFTER_DISCOVERY: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None));
    pub(crate) fn after_discovery(action: impl FnOnce() + 'static) {
        AFTER_DISCOVERY.with(|slot| *slot.borrow_mut() = Some(Box::new(action)));
    }
    pub(crate) fn run() {
        if let Some(action) = AFTER_DISCOVERY.with(|slot| slot.borrow_mut().take()) {
            action();
        }
    }
}

#[cfg(test)]
mod tests;
