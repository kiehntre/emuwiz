//! Planning and preview of a single-file relocation. Read-only: planning
//! hashes only the one file being moved (no library rehash), reads the
//! catalogue, and builds the immutable plan whose digest the user approves.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::engine;
use super::graph::{self, Verdict};
use super::journal;
use super::model::*;
use crate::Database;
use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::rename_apply::model::RenameTransaction;
use crate::database::organisation_paths;

pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the user is shown before approving.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub source: PathBuf,
    pub destination: PathBuf,
    /// Present only when there are no blockers.
    pub plan: Option<Plan>,
    /// SHA-256 of the canonical plan bytes; apply requires exactly this.
    pub digest: Option<String>,
    pub blockers: Vec<String>,
    pub effects: Vec<String>,
    pub dependencies: Option<Verdict>,
    pub strategy: Option<Strategy>,
}

use super::shape::plain_absolute;

/// Plans moving the loose file `source` into the folder `destination_dir`
/// (keeping its name). Both must lie in enabled, registered `games` library
/// roots in `db`.
pub fn plan_relocation(db: &Database, source: &Path, destination_dir: &Path) -> Preview {
    plan_with(db, source, destination_dir, None)
}

/// This crate's unit tests only: plans for scratch roots `authority`
/// created, under the test scratch filesystem policy.
#[cfg(test)]
pub(crate) fn plan_relocation_scratch(
    db: &Database,
    source: &Path,
    destination_dir: &Path,
    authority: &super::authority::ScratchAuthority,
) -> Preview {
    plan_with(db, source, destination_dir, Some(authority))
}

fn plan_with(
    db: &Database,
    source: &Path,
    destination_dir: &Path,
    authority: Option<&super::authority::ScratchAuthority>,
) -> Preview {
    let mut preview = Preview {
        source: source.to_path_buf(),
        destination: PathBuf::new(),
        plan: None,
        digest: None,
        blockers: Vec::new(),
        effects: Vec::new(),
        dependencies: None,
        strategy: None,
    };
    match build(db, source, destination_dir, authority, &mut preview) {
        Ok(plan) if preview.blockers.is_empty() => {
            preview.digest = Some(engine::plan_digest(&plan));
            preview.plan = Some(plan);
        }
        Ok(_) => {}
        Err(blocker) => preview.blockers.push(blocker),
    }
    preview
}

fn build(
    db: &Database,
    source: &Path,
    destination_dir: &Path,
    authority: Option<&super::authority::ScratchAuthority>,
    preview: &mut Preview,
) -> Result<Plan, String> {
    if !plain_absolute(source) || !plain_absolute(destination_dir) {
        return Err("source and destination must be absolute, normalised paths".into());
    }
    let name = source.file_name().ok_or("the source has no file name")?;
    let source_parent = source.parent().ok_or("the source has no folder")?;
    let real_parent =
        std::fs::canonicalize(source_parent).map_err(|e| format!("source folder: {e}"))?;
    if real_parent != source_parent {
        return Err("the source path passes through a symbolic link; give its real path".into());
    }
    let real_dest =
        std::fs::canonicalize(destination_dir).map_err(|e| format!("destination folder: {e}"))?;
    if real_dest != destination_dir {
        return Err(
            "the destination path passes through a symbolic link; give its real path".into(),
        );
    }
    let meta = std::fs::symlink_metadata(source).map_err(|e| format!("source: {e}"))?;
    if !meta.file_type().is_file() {
        return Err(
            "only a loose regular file can be relocated (not a link, folder or device)".into(),
        );
    }
    if !std::fs::metadata(destination_dir)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        return Err("the destination is not a folder".into());
    }
    let destination = destination_dir.join(name);
    preview.destination = destination.clone();
    if destination == source {
        return Err("the file is already in that folder".into());
    }
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err(
            "a file with that name already exists at the destination; nothing will be overwritten"
                .into(),
        );
    }
    let (sid, sroot, sbind, srole) = organisation_paths::catalogue_root(db, source)?;
    let (did, droot, dbind, drole) = organisation_paths::catalogue_root(db, &destination)?;
    for (root, role) in [(&sroot, &srole), (&droot, &drole)] {
        if role != "games" {
            return Err(format!(
                "{} is a '{role}' library root; relocation is limited to game library roots",
                root.display()
            ));
        }
    }
    // Fail closed on any filesystem whose crash guarantees are not
    // established, and on paths that sit on a mount nested in their root.
    let mut filesystems = Vec::new();
    let mut fs_notes = Vec::new();
    let filesystem_policy = match authority {
        None => super::fscompat::FsPolicy::Production,
        Some(a) => {
            for root in [&sroot, &droot] {
                if let Err(e) = a.admits(root) {
                    preview.blockers.push(e);
                }
            }
            a.policy()
        }
    };
    for (root, paths) in [
        (&sroot, vec![source_parent, source]),
        (&droot, vec![destination_dir]),
    ] {
        match super::fscompat::check_root(root, &paths, filesystem_policy) {
            Ok((facts, notes)) => {
                fs_notes.push(format!("filesystem: {}", facts.summary()));
                fs_notes.extend(notes);
                filesystems.push(facts);
            }
            Err(e) => preview.blockers.push(e),
        }
    }
    // The catalogue and its rollback journal live in the catalogue's
    // folder; retirement depends on that commit being durable, so its drive
    // meets the same policy (and is bound like the roots').
    match std::fs::canonicalize(db.path())
        .map_err(|e| e.to_string())
        .and_then(|c| {
            let dir = c
                .parent()
                .map(Path::to_path_buf)
                .ok_or("the catalogue has no folder")?;
            super::fscompat::check_path(&dir, filesystem_policy)
        }) {
        Ok((facts, notes)) => {
            fs_notes.push(format!("catalogue filesystem: {}", facts.summary()));
            fs_notes.extend(notes);
            filesystems.push(facts);
        }
        Err(e) => preview.blockers.push(format!("the catalogue's drive: {e}")),
    }
    let verdict = graph::relocation_verdict(source);
    preview.blockers.extend(verdict.blockers.iter().cloned());
    preview.dependencies = Some(verdict.clone());
    let declared = organisation_paths::audited_set_dependencies(db, source)?;
    if !declared.is_empty() {
        preview.blockers.push(format!(
            "the DAT audit records {} set dependenc{} (parent, BIOS, device or merged files) for this file; \
             arcade-style sets are not relocated by this slice",
            declared.len(),
            if declared.len() == 1 { "y" } else { "ies" }
        ));
    }
    let identity =
        capture_identity(source).map_err(|e| format!("the source cannot be read: {e}"))?;
    let txn_id = journal::new_txn_id().map_err(|e| e.to_string())?;

    // The shared catalogue move binding: validated now (row identity,
    // size/time, collisions, projections) and carried verbatim to apply.
    let mut transaction: RenameTransaction = serde_json::from_value(serde_json::json!({
        "transaction_id": txn_id,
        "plan_generation": 0,
        "created_at_unix": now_unix(),
        "source_scan_root": sroot,
        "entries": [{
            "source_path": source,
            "destination_path": destination,
            "original_basename": name.to_string_lossy(),
            "proposed_basename": name.to_string_lossy(),
            "identity": identity,
            "operation": { "kind": "rename_move" },
        }],
    }))
    .map_err(|e| e.to_string())?;
    organisation_paths::bind(&mut transaction, db)?;
    let binding = transaction.entries[0]
        .unknown
        .get(organisation_paths::CATALOGUE_BINDING_KEY)
        .cloned()
        .ok_or("the catalogue has no row for this file; scan it first")?;
    let catalogue = std::fs::canonicalize(db.path()).map_err(|e| e.to_string())?;

    let same_fs = std::fs::metadata(source_parent)
        .map_err(|e| e.to_string())?
        .dev()
        == std::fs::metadata(destination_dir)
            .map_err(|e| e.to_string())?
            .dev();
    let strategy = if same_fs {
        Strategy::AtomicRename
    } else {
        Strategy::CopyVerifyRetire
    };
    preview.strategy = Some(strategy);
    let (steps, pnr, retired_path) =
        super::shape::forward(strategy, source, &destination, &sroot, &droot, &txn_id)?;
    let mut effects = vec![
        format!("the catalogue row for this game will name {}", destination.display()),
        "saved DAT and verified-identity evidence is bound to the old file identity; it becomes \
         non-current and must be re-checked (it is never carried over)"
            .into(),
        "scan-run discovery records and MAME member evidence keep the path they observed, by design".into(),
    ];
    match strategy {
        Strategy::AtomicRename => {
            effects.push("same filesystem: one atomic no-overwrite rename; no copy".into())
        }
        Strategy::CopyVerifyRetire => {
            effects.push(format!(
                "different filesystems ({} -> {}): copy to private staging, verify by full re-read, publish without overwrite",
                sbind.filesystem_name(),
                dbind.filesystem_name()
            ));
            effects.push(format!(
                "the original is never deleted: it is retained byte-for-byte at {} until explicitly released",
                retired_path.as_ref().expect("retired path").display()
            ));
            effects.push(format!(
                "needs {} bytes free on the destination",
                identity.size_bytes
            ));
        }
    }
    effects.extend(verdict.notes.iter().cloned());
    effects.extend(fs_notes);
    preview.effects = effects.clone();
    Ok(Plan {
        format: PLAN_FORMAT.into(),
        version: PLAN_VERSION,
        engine: ENGINE.into(),
        txn_id,
        kind: PlanKind::Relocate,
        created_at_unix: now_unix(),
        catalogue,
        catalogue_binding: binding,
        source_root: RootBinding {
            path: sroot,
            binding: sbind,
            source_folder_id: sid,
        },
        destination_root: RootBinding {
            path: droot,
            binding: dbind,
            source_folder_id: did,
        },
        source_path: source.into(),
        destination_path: destination,
        source_identity: identity,
        strategy,
        steps,
        point_of_no_return: pnr,
        retention: RetentionPolicy::RetainUntilReleased,
        dependency_digest: verdict.digest,
        effects,
        filesystems,
        filesystem_policy,
        published_identity: None,
        retired_path,
    })
}
