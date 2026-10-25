//! Read-only format dispatch plus descriptor publication by the admitted controller.
//! Markers grant no mutation authority.
use super::*;
const DESCRIPTOR_FORMAT: &str = "emuwiz-enrolled-compact-v3";
const FD_DESCRIPTOR_FORMAT: &str = "emuwiz-enrolled-fd-v4";
const GENERATION_DESCRIPTOR_FORMAT: &str = "emuwiz-enrolled-fd-generation-v5";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    format: String,
    original: Vec<u8>,
    ticket: Ticket,
}
pub(super) fn publish_descriptor(dir: &Path, base: &Authority, ticket: &Ticket) -> io::Result<()> {
    use std::io::Write;
    // The descriptor takes the legacy journal name an older build also sees.
    crate::dat::rename_apply::writer_gate::check().map_err(fail)?;
    let path = crate::dat::rename_apply::journal::journal_path(dir, base.transaction_id())
        .ok_or_else(|| fail("invalid transaction id"))?;
    let descriptor = Descriptor {
        format: if ticket.generation.is_some() {
            GENERATION_DESCRIPTOR_FORMAT
        } else if super::integration::shared_engine::selected(&base.transaction) {
            FD_DESCRIPTOR_FORMAT
        } else {
            DESCRIPTOR_FORMAT
        }
        .into(),
        original: base.original_bytes().to_vec(),
        ticket: ticket.clone(),
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(&serde_json::to_vec(&descriptor).map_err(fail)?)?;
    file.sync_all()?;
    open_dir(dir)?.sync_all()
}
pub(crate) fn is_enrolled(dir: &Path, id: &str) -> bool {
    // Unknown authority evidence closes the legacy route; it grants no R2 authority.
    enrollment_evidence(dir, id).unwrap_or(true)
}
fn enrollment_evidence(dir: &Path, id: &str) -> io::Result<bool> {
    if let Some(path) = crate::dat::rename_apply::journal::journal_path(dir, id) {
        match crate::dat::rename_apply::lookup::symlink_metadata(&path) {
            // Preserve the existing legacy route for objects outside the compact
            // descriptor's structural limit; a format probe is not a legacy limit.
            Ok(meta) if meta.is_file() && meta.nlink() == 1 && meta.len() <= 8 * 1024 * 1024 => {
                let bytes = descriptor_bytes(&path)?;
                if descriptor_marker(&bytes) {
                    return Ok(true);
                }
            }
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    match crate::dat::rename_apply::lookup::symlink_metadata(&dir.join(NAMESPACE)) {
        Ok(_) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    container_owner(dir).map(|owner| owner == id)
}

fn container_owner(dir: &Path) -> io::Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join(NAMESPACE).join(LEAF))?;
    if !file.metadata()?.is_file() {
        return Err(fail("invalid container object"));
    }
    let mut bytes = [0; HEADER * 2];
    file.read_exact_at(&mut bytes, 0)?;
    let header = decoded(&bytes)?;
    if &header[..8] != b"EWSYNV3\0" {
        return Err(fail("unknown container format"));
    }
    let length = u64::from_le_bytes(header[8..16].try_into().unwrap());
    if length > 1024 * 1024 {
        return Err(fail("base limit"));
    }
    let mut original = vec![0; length as usize * 2];
    file.read_exact_at(&mut original, HEADER as u64 * 2)?;
    let base = Authority::decode_legacy(&decoded(&original)?, 1024 * 1024).map_err(protocol)?;
    if header[24..56] != base.binding() {
        return Err(fail("compact base binding mismatch"));
    }
    let mut marker = [0];
    file.read_exact_at(&mut marker, (HEADER as u64 + length) * 2)?;
    if marker[0] != COMMITTED {
        return Err(fail("compact base incomplete"));
    }
    Ok(base.transaction_id().into())
}
pub(crate) fn discovery_path(dir: &Path) -> Option<io::Result<PathBuf>> {
    match crate::dat::rename_apply::lookup::symlink_metadata(&dir.join(NAMESPACE)) {
        Ok(_) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => return Some(Err(error)),
    }
    Some(container_owner(dir).and_then(|id| {
        crate::dat::rename_apply::journal::journal_path(dir, &id)
            .ok_or_else(|| fail("invalid compact transaction id"))
    }))
}
/// The historical bit is local verified reader context, never journal input.
/// Mutation and cleanup callers retain their existing live containment checks.
pub(crate) fn read_with_history(path: &Path) -> Option<io::Result<(RenameTransaction, bool)>> {
    let dir = path.parent()?;
    let id = path.file_stem()?.to_str()?;
    match enrollment_evidence(dir, id) {
        Ok(false) => return None,
        Ok(true) => (),
        Err(error) => return Some(Err(error)),
    }
    Some((|| {
        if let Some(result) = super::integration::active_read(path) {
            return result.map(|tx| (tx, false));
        }
        let mut budget = u64::MAX;
        read_descriptor_with_history(path, &descriptor_bytes(path)?, &mut budget)
    })())
}

/// The background index has already bounded/read the descriptor. Keep its
/// legacy parsing unchanged while charging compact reads to its shared budget.
pub(crate) fn read_bounded(
    path: &Path,
    bytes: &[u8],
    budget: &mut u64,
) -> Option<io::Result<RenameTransaction>> {
    let dir = path.parent()?;
    let id = path.file_stem()?.to_str()?;
    let marker = descriptor_marker(bytes);
    let enrolled = if marker {
        true
    } else {
        match discovery_bounded(dir, budget) {
            None => false,
            Some(Ok(owner)) => owner.file_stem().and_then(|s| s.to_str()) == Some(id),
            Some(Err(error)) => return Some(Err(error)),
        }
    };
    enrolled.then(|| read_descriptor(path, bytes, budget))
}

pub(crate) fn discovery_bounded(dir: &Path, budget: &mut u64) -> Option<io::Result<PathBuf>> {
    match crate::dat::rename_apply::lookup::symlink_metadata(&dir.join(NAMESPACE)) {
        Ok(_) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => return Some(Err(error)),
    }
    // container_owner reads at most the header, 1 MiB encoded base and marker.
    let Some(remaining) = budget.checked_sub(2 * 1024 * 1024 + 129) else {
        return Some(Err(fail("compact discovery byte budget exceeded")));
    };
    *budget = remaining;
    discovery_path(dir)
}

fn read_descriptor(path: &Path, bytes: &[u8], budget: &mut u64) -> io::Result<RenameTransaction> {
    read_descriptor_with_history(path, bytes, budget).map(|(tx, _)| tx)
}
fn read_descriptor_with_history(
    path: &Path,
    bytes: &[u8],
    budget: &mut u64,
) -> io::Result<(RenameTransaction, bool)> {
    let dir = path
        .parent()
        .ok_or_else(|| fail("missing compact parent"))?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| fail("invalid compact name"))?;
    let descriptor: Descriptor = serde_json::from_slice(bytes).map_err(fail)?;
    if !matches!(
        descriptor.format.as_str(),
        DESCRIPTOR_FORMAT | FD_DESCRIPTOR_FORMAT | GENERATION_DESCRIPTOR_FORMAT
    ) || descriptor.ticket.root != dir
    {
        return Err(fail("foreign compact descriptor"));
    }
    let base = Authority::decode_legacy(&descriptor.original, 1024 * 1024).map_err(protocol)?;
    if base.transaction_id() != id {
        return Err(fail("compact journal id mismatch"));
    }
    let store = Store::inspect(&descriptor.ticket, &base, CAP)?;
    *budget = budget
        .checked_sub(store.layout.length)
        .ok_or_else(|| fail("compact receipt byte budget exceeded"))?;
    let snapshot = store.scan(&base)?;
    if snapshot.pending.is_some() {
        return Err(fail("incomplete compact checkpoint; inspection required"));
    }
    let tx = snapshot.runtime.restore(&base, &snapshot.stream)?;
    if let Some(result) = super::integration::deployment::historical_read(
        &store,
        &base,
        &tx,
        &snapshot.cursor,
        budget,
    ) {
        result?;
        store.bound()?;
        return Ok((tx, true));
    }
    // A terminal receipt is historical evidence. Refuse a demonstrable
    // filesystem contradiction instead of offering a success projection.
    if tx.state == crate::dat::rename_apply::model::TransactionState::RolledBack {
        for entry in &tx.entries {
            if crate::dat::rename_apply::identity::capture_identity_bounded(
                &entry.source_path,
                budget,
            )? != entry.identity
            {
                return Err(fail(
                    "terminal compact rollback contradicts source identity",
                ));
            }
            match crate::dat::rename_apply::lookup::symlink_metadata(&entry.destination_path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                _ => {
                    return Err(fail(
                        "terminal compact rollback contradicts destination slot",
                    ));
                }
            }
        }
    }
    store.bound()?;
    Ok((tx, false))
}

fn descriptor_bytes(path: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.len() > 8 * 1024 * 1024 {
        return Err(fail("invalid compact descriptor object"));
    }
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(fail("compact descriptor grew beyond limit"));
    }
    Ok(bytes)
}

fn descriptor_marker(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes).ok()
        .is_some_and(|v| matches!(v.get("format").and_then(|f| f.as_str()), Some(DESCRIPTOR_FORMAT | FD_DESCRIPTOR_FORMAT | GENERATION_DESCRIPTOR_FORMAT)))
        // Legacy unknown root fields are opaque too. An actual container still
        // selects compact by owner even if its descriptor becomes legacy JSON.
        && serde_json::from_slice::<RenameTransaction>(bytes).is_err()
}
