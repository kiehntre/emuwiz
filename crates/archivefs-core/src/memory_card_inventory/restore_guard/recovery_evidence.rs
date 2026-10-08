//! Read-only recovery evidence. Inspection failure is never proof of absence.
use super::*;

#[cfg(test)]
thread_local! {
    pub(super) static INSPECTION_ERROR: std::cell::RefCell<Option<(PathBuf, i32)>> = const { std::cell::RefCell::new(None) };
}

fn inspect(path: &Path) -> Result<Option<(FileIdentity, String)>, String> {
    let fail = |error: std::io::Error| {
        format!(
            "Recovery evidence unavailable at {}: {error}",
            path.display()
        )
    };
    #[cfg(test)]
    if let Some(errno) = INSPECTION_ERROR.with(|fault| {
        fault
            .borrow()
            .as_ref()
            .filter(|(p, _)| p == path)
            .map(|(_, errno)| *errno)
    }) {
        return Err(fail(std::io::Error::from_raw_os_error(errno)));
    }
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.is_symlink() => identity_of(&metadata),
        Ok(_) => {
            return Err(format!(
                "Unsupported recovery artifact at {}; symlinks are not trusted evidence",
                path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A missing/unobservable parent does not prove an artifact absent.
            let parent = path
                .parent()
                .ok_or_else(|| "Recovery artifact has no parent".to_string())?;
            let metadata = fs::symlink_metadata(parent).map_err(&fail)?;
            if !metadata.is_dir() || metadata.is_symlink() {
                return Err(format!(
                    "Recovery artifact parent is unsupported at {}",
                    parent.display()
                ));
            }
            let directory = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
                .open(parent)
                .map_err(&fail)?;
            let pinned = directory.metadata().map_err(&fail)?;
            let current = fs::symlink_metadata(parent).map_err(&fail)?;
            if identity_of(&metadata) != identity_of(&pinned)
                || identity_of(&metadata) != identity_of(&current)
                || !matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(format!(
                    "Recovery artifact parent or absence changed during inspection at {}",
                    path.display()
                ));
            }
            return Ok(None);
        }
        Err(error) => return Err(fail(error)),
    };
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(&fail)?;
    let metadata = file.metadata().map_err(&fail)?;
    if !metadata.is_file()
        || identity_of(&metadata) != before
        || before.size > PS2_MAX_CARD_BYTES as u64
    {
        return Err(format!(
            "Recovery artifact changed or is unsupported at {}",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(PS2_MAX_CARD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(&fail)?;
    if bytes.len() as u64 != before.size
        || identity_of(&file.metadata().map_err(&fail)?) != before
        || path_identity(path).ok() != Some(before)
    {
        return Err(format!(
            "Recovery artifact changed during inspection at {}",
            path.display()
        ));
    }
    Ok(Some((before, sha256_hex(&bytes))))
}

pub(super) fn confirmed_absent(path: &Path) -> bool {
    matches!(inspect(path), Ok(None))
}

/// Used both by recovery and by read-only History after restart. Refusal leaves
/// the durable in-flight receipt intact so a corrected permission/lookup can retry.
pub(super) fn problem(journal: &Ps2RestoreJournal) -> Option<String> {
    if !journal.phase.needs_recovery() {
        return None;
    }
    match inspect(&journal.card_path) {
        Err(detail) => return Some(detail),
        Ok(None) => {
            return Some(format!(
                "Live card is missing at {}; recovery evidence retained",
                journal.card_path.display()
            ));
        }
        Ok(Some(_)) => {}
    }
    let mut artifacts = vec![(journal.backup_path.clone(), "backup")];
    if let Some(stage) = &journal.staged_path {
        artifacts.push((stage.clone(), "staged image"));
    }
    if journal.phase == Ps2RestorePhase::UndoIntent {
        if let Some(parent) = journal.card_path.parent() {
            artifacts.push((undo_temp_path(parent, &journal.operation_id), "undo image"));
        }
    }
    for (path, kind) in artifacts {
        let evidence = match inspect(&path) {
            Ok(evidence) => evidence,
            Err(detail) => return Some(detail),
        };
        let Some((identity, sha)) = evidence else {
            continue;
        };
        let valid = match kind {
            "backup" => sha == journal.original_sha256,
            "staged image" => {
                (Some(sha.as_str()) == journal.staged_sha256.as_deref()
                    && Some(identity) == journal.post_identity)
                    || (sha == journal.original_sha256 && identity == journal.original_identity)
            }
            _ => {
                (sha == journal.original_sha256 && Some(identity) == journal.undo_identity)
                    || (Some(sha.as_str()) == journal.post_sha256.as_deref()
                        && Some(identity) == journal.post_identity)
            }
        };
        if !valid {
            return Some(format!(
                "Incomplete, changed or unowned {kind} retained at {}. Preserve the artifact and card for inspection.",
                path.display()
            ));
        }
    }
    None
}
