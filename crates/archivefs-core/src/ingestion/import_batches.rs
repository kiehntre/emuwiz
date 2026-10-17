//! Read-only scheduling over sealed native evidence. Scheduling grants no Apply
//! authority: each subset must still be freshly prepared and explicitly confirmed.
use super::bounded_import::{BoundedImportPlan, stable_candidate_id};
use super::preview::MAX_COMPARISON_INPUT_BYTES;
use super::saved_discovery::MAX_SELECTED;
use crate::ArchiveKind;
use std::path::PathBuf;

pub const DEFAULT_BATCH_SIZE: usize = 100;

#[derive(Clone, Debug, Default)]
pub struct ImportBatches {
    pub batches: Vec<Vec<String>>,
    pub deferred: Vec<(PathBuf, String)>,
}

impl BoundedImportPlan {
    /// Deterministic candidate identities; never caller supplied paths. Estimates
    /// reserve native file/companion reads and the extra archive-input pass.
    /// Reference and decoded-member budgets remain enforced by prepare_subset.
    pub fn automatic_batches(&self) -> ImportBatches {
        schedule(&self.evidence.candidates)
    }
}

fn schedule(candidates: &[super::bounded_import::Candidate]) -> ImportBatches {
    let mut schedule = ImportBatches::default();
    let mut batch = Vec::new();
    let mut charged = 0u64;
    let mut candidates: Vec<_> = candidates
        .iter()
        .filter(|c| !c.ambiguous && c.relationships_complete)
        .collect();
    candidates.sort_by(|a, b| a.hashes.fingerprint.path.cmp(&b.hashes.fingerprint.path));
    for c in candidates {
        let archive = matches!(
            ArchiveKind::from_storage(&c.kind),
            Some(ArchiveKind::Zip | ArchiveKind::SevenZip | ArchiveKind::Rar | ArchiveKind::Lha)
        );
        let cost = c
            .companions
            .iter()
            .try_fold(c.hashes.bytes_hashed, |sum, h| {
                sum.checked_add(h.bytes_hashed)
            })
            .and_then(|sum| sum.checked_add(if archive { c.hashes.bytes_hashed } else { 0 }));
        let Some(cost) = cost.filter(|cost| *cost <= MAX_COMPARISON_INPUT_BYTES) else {
            schedule.deferred.push((c.hashes.fingerprint.path.clone(),
                    "Media and companion inputs exceed one native batch budget. Choose a smaller supported representation; no import prepared.".into()));
            continue;
        };
        if !batch.is_empty()
            && (batch.len() >= DEFAULT_BATCH_SIZE.min(MAX_SELECTED)
                || cost > MAX_COMPARISON_INPUT_BYTES.saturating_sub(charged))
        {
            schedule.batches.push(std::mem::take(&mut batch));
            charged = 0;
        }
        charged += cost;
        batch.push(stable_candidate_id(c));
    }
    if !batch.is_empty() {
        schedule.batches.push(batch);
    }
    schedule
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingestion::bounded_import::Candidate;
    fn candidate(path: &str, bytes: u64) -> Candidate {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("proof");
        std::fs::write(&p, b"proof").unwrap();
        let mut hashes = crate::identity_source::hashing::hash_file(
            &p,
            &crate::safe_read::TrustedRoots::none(),
            None,
        )
        .unwrap();
        hashes.fingerprint.path = path.into();
        hashes.fingerprint.size_bytes = bytes;
        hashes.bytes_hashed = bytes;
        Candidate {
            hashes,
            platform: "ZX Spectrum".into(),
            kind: ArchiveKind::DirectGameImage.storage_name().into(),
            companions: vec![],
            alternate: None,
            multipart: None,
            ambiguous: false,
            relationships_complete: true,
        }
    }
    #[test]
    fn deterministic_batches_cover_exact_identities_once_within_count_and_byte_limits() {
        let mut candidates: Vec<_> = (0..205)
            .map(|i| candidate(&format!("/fixtures/{i:03}.z80"), 49182))
            .collect();
        candidates.reverse();
        let a = schedule(&candidates);
        assert_eq!(
            a.batches.iter().map(Vec::len).collect::<Vec<_>>(),
            [100, 100, 5]
        );
        let all: std::collections::BTreeSet<_> = a.batches.iter().flatten().collect();
        assert_eq!(all.len(), 205);
        candidates.reverse();
        assert_eq!(a.batches, schedule(&candidates).batches);
        let half = MAX_COMPARISON_INPUT_BYTES / 2;
        let a = schedule(&[
            candidate("/a", half),
            candidate("/b", half + 1),
            candidate("/c", half),
        ]);
        assert_eq!(
            a.batches.iter().map(Vec::len).collect::<Vec<_>>(),
            [1, 1, 1]
        );
    }
    #[test]
    fn oversized_companion_and_archive_costs_are_deferred_and_ineligible_never_scheduled() {
        let mut archive = candidate("/archive.zip", MAX_COMPARISON_INPUT_BYTES / 2 + 1);
        archive.kind = ArchiveKind::Zip.storage_name().into();
        let mut companion = candidate("/disc.cue", 10);
        companion
            .companions
            .push(candidate("/track.bin", MAX_COMPARISON_INPUT_BYTES).hashes);
        let mut ambiguous = candidate("/ambiguous", 1);
        ambiguous.ambiguous = true;
        let mut incomplete = candidate("/incomplete", 1);
        incomplete.relationships_complete = false;
        let a = schedule(&[
            archive,
            companion,
            ambiguous,
            incomplete,
            candidate("/good", 1),
        ]);
        assert_eq!(a.deferred.len(), 2);
        assert_eq!(a.batches.len(), 1);
        assert_eq!(a.batches[0].len(), 1);
    }
}
