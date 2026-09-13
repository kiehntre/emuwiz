//! Bounded background receipt ingestion, separate from page rendering. Reuses
//! the registry's projections; never previews rollback, hashes backups, walks
//! library roots, or probes publication destinations.
use super::*;
use crate::attention::{AttentionSnapshot, operation_attention};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct AttentionReceiptPaths {
    pub rename: Option<PathBuf>,
    pub library_views: Option<PathBuf>,
    pub shared_apply: Option<PathBuf>,
    pub database: Option<PathBuf>,
}

const MAX_DIRECTORY_ENTRIES: usize = 4096;
const MAX_RECEIPT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

fn note(snapshot: &mut AttentionSnapshot, message: String) {
    snapshot.limited = true;
    if snapshot.coverage_notes.len() < 20 {
        snapshot.coverage_notes.push(message);
    }
}

/// Only four known, flat application receipt directories are read. Limits are
/// explicit coverage gaps, never a claim that an omitted source is healthy.
pub fn attention_receipt_snapshot(paths: &AttentionReceiptPaths) -> AttentionSnapshot {
    let mut snapshot = AttentionSnapshot::default();
    let mut budget = MAX_TOTAL_BYTES;
    let mut views = BTreeMap::<String, (String, crate::attention::AttentionItem)>::new();
    let mut shared_records = BTreeMap::new();
    let mut rollbacks = BTreeMap::new();
    let database_parent = paths
        .database
        .as_ref()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf);
    for (kind, directory) in [
        ("rename", &paths.rename),
        ("library-view", &paths.library_views),
        ("shared", &paths.shared_apply),
        ("restore", &database_parent),
    ] {
        let Some(directory) = directory else {
            note(
                &mut snapshot,
                format!("{kind} receipt location is unavailable."),
            );
            continue;
        };
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                note(&mut snapshot, format!("Cannot read {kind} receipts: {e}"));
                continue;
            }
        };
        for (index, entry) in entries.enumerate() {
            if index == MAX_DIRECTORY_ENTRIES {
                note(
                    &mut snapshot,
                    format!(
                        "{kind} receipt directory exceeded {MAX_DIRECTORY_ENTRIES} entries. Review complete history in its workflow."
                    ),
                );
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    note(
                        &mut snapshot,
                        format!("Cannot enumerate {kind} receipt: {e}"),
                    );
                    continue;
                }
            };
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.ends_with(".json") {
                continue;
            }
            if kind == "restore"
                && !paths
                    .database
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .is_some_and(|db| {
                        name.starts_with(&format!("{}.restore-", db.to_string_lossy()))
                    })
            {
                continue;
            }
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                note(
                    &mut snapshot,
                    format!(
                        "Non-regular {kind} receipt was not read: {}",
                        path.display()
                    ),
                );
                continue;
            }
            if budget == 0 {
                note(
                    &mut snapshot,
                    "Receipt byte budget reached; history coverage is partial.".into(),
                );
                break;
            }
            let bytes = (|| -> std::io::Result<Vec<u8>> {
                let mut bytes = Vec::new();
                fs::File::open(&path)?
                    .take(MAX_RECEIPT_BYTES.min(budget) + 1)
                    .read_to_end(&mut bytes)?;
                Ok(bytes)
            })();
            let allowance = MAX_RECEIPT_BYTES.min(budget);
            if let Ok(bytes) = &bytes {
                budget = budget.saturating_sub(bytes.len() as u64);
            }
            let bytes = match bytes {
                Ok(bytes) if bytes.len() as u64 <= allowance => bytes,
                _ => {
                    note(
                        &mut snapshot,
                        format!("Unreadable or oversized {kind} receipt: {}", path.display()),
                    );
                    continue;
                }
            };
            snapshot.source_rows += 1;
            let reference = Some(path.display().to_string());
            if kind == "shared" && name.ends_with(".rollback.json") {
                match serde_json::from_slice::<SharedRollbackPreview>(&bytes) {
                    Ok(receipt) if receipt.schema_version == 1 => {
                        rollbacks.insert(receipt.original_operation_id.clone(), receipt);
                    }
                    _ => note(
                        &mut snapshot,
                        format!(
                            "Unsupported or malformed rollback receipt: {}",
                            path.display()
                        ),
                    ),
                }
                continue;
            }
            let record = match kind {
                "rename" => serde_json::from_slice::<RenameTransaction>(&bytes)
                    .ok()
                    .map(|transaction| {
                        let kind = if transaction.entries.iter().any(|e| {
                            matches!(e.operation, TransactionOperation::CreateSymlink { .. })
                        }) {
                            OperationKind::PlayingLibrary
                        } else if disc_conversion_role(&transaction).is_some() {
                            OperationKind::DiscConversion
                        } else if transaction.entries.iter().any(|e| {
                            e.destination_path
                                .components()
                                .any(|c| c.as_os_str() == QUARANTINE_DIRECTORY_NAME)
                        }) {
                            OperationKind::DuplicateQuarantine
                        } else {
                            OperationKind::DatRename
                        };
                        // No live disc-output verification in a saved-state index.
                        operation_from_transaction(&transaction, kind, reference)
                    }),
                "shared" => serde_json::from_slice::<SharedApplyJournal>(&bytes)
                    .ok()
                    .filter(|j| j.schema_version == 1)
                    .map(|journal| shared_apply_operation(&journal, reference, None)),
                "restore" => serde_json::from_slice::<crate::DatabaseRestoreReceipt>(&bytes)
                    .ok()
                    .filter(|r| r.schema_version == 1)
                    .map(|receipt| database_restore_receipt_operation(&receipt, reference)),
                "library-view" => {
                    if let Ok(history) = serde_json::from_slice::<LibraryViewHistoryRecord>(&bytes)
                    {
                        if history.schema_version != 1 {
                            note(
                                &mut snapshot,
                                format!(
                                    "Unsupported Library View receipt version: {}",
                                    path.display()
                                ),
                            );
                            continue;
                        }
                        let Some(observed) =
                            crate::attention::receipt_utc_seconds(&history.timestamp)
                        else {
                            note(
                                &mut snapshot,
                                format!(
                                    "Invalid Library View receipt timestamp: {}",
                                    path.display()
                                ),
                            );
                            continue;
                        };
                        let mut record = library_view_history_operation(&history, reference);
                        record.kind = match history.profile_kind {
                            crate::FrontendProfileKind::Romm => OperationKind::RommPublish,
                            crate::FrontendProfileKind::EsDe => OperationKind::EsDePublish,
                            crate::FrontendProfileKind::Generic => {
                                OperationKind::LibraryViewPublish
                            }
                        };
                        if let Some(mut item) = operation_attention(&record) {
                            // One current outcome per view/destination. A successful
                            // later apply/remove replaces an older failure, not a
                            // manually managed resolved marker.
                            let key =
                                format!("view:{}:{}", history.view_id, history.destination_root);
                            item.id.clone_from(&key);
                            item.last_observed = Some(observed);
                            item.provenance = format!(
                                "Library View history at {}; profile {:?}",
                                history.timestamp, history.profile_kind
                            );
                            if views
                                .get(&key)
                                .is_none_or(|(time, _)| time < &history.timestamp)
                            {
                                views.insert(key, (history.timestamp, item));
                            }
                        }
                        continue;
                    }
                    None
                }
                _ => unreachable!(),
            };
            match record {
                Some(mut record) if kind == "shared" => {
                    record.input.freshness_evidence.clear();
                    shared_records.insert(record.operation_id.clone(), record);
                }
                Some(record) => {
                    if let Some(item) = operation_attention(&record) {
                        snapshot.insert(item);
                    }
                }
                None => note(
                    &mut snapshot,
                    format!(
                        "Unsupported or malformed {kind} receipt: {}",
                        path.display()
                    ),
                ),
            }
        }
    }
    for (_, item) in views.into_values() {
        snapshot.insert(item);
    }
    for mut record in shared_records.into_values() {
        if let Some(receipt) = rollbacks.get(&record.operation_id) {
            project_completed_shared_rollback(&mut record, receipt);
        }
        if let Some(item) = operation_attention(&record) {
            snapshot.insert(item);
        }
    }
    snapshot.coverage_notes.push("Saved operation outcomes only. Destination drift, rollback safety and missing tools are checked in their existing workflows, not on page load. Legacy generic rename receipts cannot identify RomM/ES-DE targets.".into());
    snapshot
}

#[cfg(test)]
mod tests;
