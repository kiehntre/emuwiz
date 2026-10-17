//! Saves disc order proven by verified DAT identities into the existing
//! `media_topology_evidence` table. See `crate::media_set::dat_disc_order`
//! for what counts as proof. Reads only the catalogue database and file
//! metadata; never opens, hashes or changes a disc image.
use super::*;
use crate::dat::library_identity_summary::{
    DatProvenanceFreshness, DatVerificationState, LibraryItemHashes,
};
use crate::media_set::dat_disc_order::{
    DAT_DISC_ORDER_PRODUCER, DatDiscCandidate, catalogue_disc_total, content_evidence,
    dat_disc_record, dat_disc_token, dat_order_family,
};
use crate::media_set::{TopologyStats, index_media, resolve_index};

/// What one disc-order refresh found and saved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatDiscOrderOutcome {
    /// Discs, disks or tapes numbered by the catalogue and passed on to the
    /// topology engine. Refused images are counted only in `refused`.
    pub discs: usize,
    /// Of those, discs whose catalogue does not establish the total.
    pub unknown_total: usize,
    /// Numbered images refused: their current DAT identities disagree or
    /// are ambiguous, the platform is unknown, the file changed since the
    /// scan, or the entry is not in the same revision's saved inventory.
    pub refused: usize,
    /// Verified releases saved or refreshed.
    pub written: usize,
    /// Saved releases kept because the new claim disagrees with them.
    pub conflicts_kept: usize,
    /// Sources whose catalogue was never validated, so the installed entry
    /// list needed to confirm and count discs is not saved. Validate them,
    /// then audit again.
    pub validation_missing: BTreeSet<String>,
    /// Sources whose saved validation is for a different catalogue version
    /// than the audit's identities. Validate the current file, then audit.
    pub validation_stale: BTreeSet<String>,
}

/// A catalogue source's saved revision, skipped duplicate names and entry names.
type SavedInventory = (Option<String>, u64, HashSet<String>);

const STRONG_HASHES: [&str; 3] = ["SHA-256", "SHA-1", "MD5"];

fn catalogue_platform(archive: &PersistedArchive) -> Option<String> {
    if let Some(platform) = archive.platform.as_deref() {
        return crate::canonical_platform_for_alias(platform).map(str::to_owned);
    }
    archive
        .relative_path
        .components()
        .find_map(|component| match component {
            std::path::Component::Normal(value) => value
                .to_str()
                .and_then(crate::canonical_platform_for_alias)
                .map(str::to_owned),
            _ => None,
        })
}

fn unchanged_since_scan(archive: &PersistedArchive) -> bool {
    let Ok(metadata) = fs::symlink_metadata(&archive.absolute_path) else {
        return false;
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);
    metadata.is_file()
        && Some(metadata.len()) == archive.size_bytes
        && modified.is_some()
        && modified == archive.modified_time_unix_seconds
}

impl Database {
    /// Rebuilds DAT-proven disc order from saved identities and saves every
    /// trusted release through
    /// [`Database::persist_media_topology_evidence_checked`]. A disc counts
    /// only when its own bytes matched exactly one entry by SHA-256, SHA-1 or
    /// MD5, that identity is still current, and the entry is in the same
    /// catalogue revision's saved inventory.
    pub fn refresh_dat_disc_order(&mut self, generation: u64) -> Result<DatDiscOrderOutcome> {
        let mut outcome = DatDiscOrderOutcome::default();
        let mut inventories: HashMap<String, Option<SavedInventory>> = HashMap::new();
        let mut records = Vec::new();
        for archive in self.load_archives()? {
            let format = archive
                .absolute_path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let Some(family) = dat_order_family(&format) else {
                continue;
            };
            if archive.last_verified_missing_at.is_some() {
                continue;
            }
            let current = LibraryItemHashes {
                size_bytes: archive.size_bytes,
                ..Default::default()
            };
            let mut claims = Vec::new();
            // Every current, content-based identity for this image, strong or
            // not: any entry name or ambiguity it carries must agree with the
            // claim. Filename-only results are ignored.
            let mut named = BTreeSet::new();
            let mut ambiguous = false;
            for persisted in self.library_dat_identities_for_item(archive.id)? {
                let Some(summary) = self.library_dat_identity_summary_for_item(
                    archive.id,
                    &persisted.source.source_id,
                    Some(&current),
                    None,
                    true,
                )?
                else {
                    continue;
                };
                if summary.provenance_freshness == DatProvenanceFreshness::Current
                    && content_evidence(&summary.verification_state)
                {
                    ambiguous |= summary.is_ambiguous();
                    named.extend(summary.canonical.canonical_dat_name.clone());
                }
                let strong = matches!(&summary.verification_state,
                    DatVerificationState::VerifiedSingleMatch { algorithm }
                        if STRONG_HASHES.contains(&algorithm.as_str()));
                if strong
                    && summary.provenance_freshness == DatProvenanceFreshness::Current
                    && persisted.archive_member.is_none()
                    && let Some(name) = summary.canonical.canonical_dat_name.clone()
                    && let Some(revision) = summary.source.source_revision.clone()
                {
                    claims.push((summary, name, revision));
                }
            }
            let Some(token) = claims
                .first()
                .filter(|_| !ambiguous && named.len() == 1)
                .and_then(|(_, name, _)| dat_disc_token(name))
                .filter(|token| token.fits(family))
            else {
                outcome.refused +=
                    usize::from(!claims.is_empty() && (ambiguous || named.len() > 1));
                continue;
            };
            claims.sort_by(|a, b| a.0.source.source_id.cmp(&b.0.source.source_id));
            let (summary, name, revision) = claims.swap_remove(0);
            let Some(platform) = catalogue_platform(&archive) else {
                outcome.refused += 1;
                continue;
            };
            if !unchanged_since_scan(&archive) {
                outcome.refused += 1;
                continue;
            }
            let source_id = summary.source.source_id.clone();
            if !inventories.contains_key(&source_id) {
                let inventory = match self.expected_dat_inventory_meta(&source_id)? {
                    Some(meta) => Some((
                        meta.source_revision,
                        meta.duplicate_names_skipped,
                        self.expected_dat_canonical_identities(&source_id)?,
                    )),
                    None => None,
                };
                inventories.insert(source_id.clone(), inventory);
            }
            // The entry must be in the installed catalogue revision's saved
            // list. A missing or other-version list is a prerequisite the user
            // can fix; it is reported apart from ordinary refusals.
            let entries = match &inventories[&source_id] {
                None => {
                    outcome.validation_missing.insert(source_id);
                    outcome.refused += 1;
                    continue;
                }
                Some((saved, _, _)) if saved.as_deref() != Some(revision.as_str()) => {
                    outcome.validation_stale.insert(source_id);
                    outcome.refused += 1;
                    continue;
                }
                Some((_, duplicates, entries)) if *duplicates == 0 && entries.contains(&name) => {
                    entries
                }
                Some(_) => {
                    outcome.refused += 1;
                    continue;
                }
            };
            let total = token
                .total
                .or_else(|| catalogue_disc_total(&token, entries.iter().map(String::as_str)));
            let candidate = DatDiscCandidate {
                path: archive.absolute_path.clone(),
                platform,
                format,
                source_id,
                source_revision: revision,
                catalogue_sha256: summary.source.catalogue_sha256.clone(),
                canonical_name: name,
                matched_algorithm: summary
                    .hash_evidence
                    .matched_algorithm
                    .clone()
                    .unwrap_or_default(),
                matched_value: summary.hash_evidence.matched_value.clone(),
            };
            outcome.discs += 1;
            outcome.unknown_total += usize::from(total.is_none());
            records.push(dat_disc_record(&candidate, family, &token, total));
        }
        let report = TopologyReport {
            sets: resolve_index(index_media(records)).sets,
            stats: TopologyStats::default(),
        };
        let saved = self.persist_media_topology_evidence_checked(
            &report,
            DAT_DISC_ORDER_PRODUCER,
            MEDIA_TOPOLOGY_PRODUCER_VERSION,
            generation,
        )?;
        outcome.written = saved.written;
        outcome.conflicts_kept = saved.conflicts_kept;
        Ok(outcome)
    }
}
