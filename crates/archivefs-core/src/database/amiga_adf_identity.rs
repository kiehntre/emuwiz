//! Reads the saved, current DAT evidence that can make one loose Amiga ADF a
//! launch identity. See `crate::dat::amiga_adf_identity` for the rules. Reads
//! only the catalogue database and file metadata; never opens, hashes or
//! changes the ADF.
use super::*;
use crate::dat::amiga_adf_identity::{
    AmigaAdfIdentityRefusal as Refusal, AmigaAdfLaunchIdentity, adf_game_key, catalogue_convention,
    prove_single_disk,
};
use crate::dat::configured_catalogues::ConfiguredCatalogue;
use crate::dat::expected_inventory::ExpectedDatEntryMetadata;
use crate::dat::library_identity_summary::{
    DatProvenanceFreshness, DatVerificationState, DiscFileState, LibraryItemHashes,
};
use crate::media_set::dat_disc_order::content_evidence;

const STRONG_HASHES: [&str; 3] = ["SHA-256", "SHA-1", "MD5"];

/// The ADF's verified identity, or the first reason it has none.
pub type AmigaAdfIdentityDecision = std::result::Result<AmigaAdfLaunchIdentity, Refusal>;

impl Database {
    /// Launch identity for the loose ADF at `path`. Only an exact
    /// SHA-256/SHA-1/MD5 match from a catalogue that is still configured with
    /// the same file bytes counts, the item must be unchanged since it was
    /// scanned and audited, all current content evidence must name the same
    /// entry, and that entry must be in the catalogue revision's saved list
    /// and prove a single-disk release.
    pub fn amiga_adf_launch_identity(
        &self,
        path: &Path,
        catalogues: &HashMap<String, ConfiguredCatalogue>,
    ) -> Result<AmigaAdfIdentityDecision> {
        let is_adf = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("adf"));
        if !is_adf {
            return Ok(Err(Refusal::NotAdf));
        }
        let Some(archive_id) = self.find_archive_id_by_absolute_path(path)? else {
            return Ok(Err(Refusal::NotInLibrary));
        };
        let platform = self.current_platform_for_archive(archive_id)?;
        if platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias)
            != Some("Amiga")
        {
            return Ok(Err(Refusal::NotAmiga));
        }
        let (size, modified, missing): (Option<i64>, Option<i64>, Option<String>) = self
            .connection
            .query_row(
                "SELECT size_bytes, modified_time_unix_seconds, last_verified_missing_at \
                 FROM archives WHERE id = ?1",
                [archive_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| db_error("failed to read ADF catalogue row", error))?;
        let size = size.and_then(|value| u64::try_from(value).ok());
        if missing.is_some() || !unchanged(path, size, modified) {
            return Ok(Err(Refusal::FileChanged));
        }

        let current = LibraryItemHashes {
            size_bytes: size,
            ..Default::default()
        };
        let mut named = BTreeSet::new();
        let mut ambiguous = false;
        let mut stale = false;
        let mut file_changed = false;
        let mut claims = Vec::new();
        for persisted in self.library_dat_identities_for_item(archive_id)? {
            let source_id = persisted.source.source_id.clone();
            let Some(catalogue) = catalogues.get(&source_id) else {
                stale |= persisted.carries_identity();
                continue;
            };
            let Some(mut summary) = self.library_dat_identity_summary_for_item(
                archive_id,
                &source_id,
                Some(&current),
                catalogue.revision.as_deref(),
                true,
            )?
            else {
                continue;
            };
            catalogue.check_saved_identity(&mut summary);
            if summary.provenance_freshness != DatProvenanceFreshness::Current {
                stale |= persisted.carries_identity();
                continue;
            }
            if !content_evidence(&summary.verification_state) {
                continue;
            }
            ambiguous |= summary.is_ambiguous();
            named.extend(summary.canonical.canonical_dat_name.clone());
            let strong = matches!(&summary.verification_state,
                DatVerificationState::VerifiedSingleMatch { algorithm }
                    if STRONG_HASHES.contains(&algorithm.as_str()));
            if !strong || persisted.archive_member.is_some() {
                continue;
            }
            if persisted.file_state_for_path(path) != DiscFileState::Unchanged {
                file_changed = true;
                continue;
            }
            if let (Some(name), Some(revision), Some(digest), Some(algorithm), Some(value)) = (
                summary.canonical.canonical_dat_name.clone(),
                summary.source.source_revision.clone(),
                summary.source.catalogue_sha256.clone(),
                summary.hash_evidence.matched_algorithm.clone(),
                summary.hash_evidence.matched_value.clone(),
            ) {
                claims.push((summary, name, revision, digest, algorithm, value));
            }
        }
        if ambiguous || named.len() > 1 {
            return Ok(Err(Refusal::Conflicting(named.into_iter().collect())));
        }
        if claims.is_empty() {
            return Ok(Err(if file_changed {
                Refusal::FileChanged
            } else if stale {
                Refusal::CatalogueStale
            } else {
                Refusal::NoCurrentMatch
            }));
        }
        claims.sort_by(|a, b| a.0.source.source_id.cmp(&b.0.source.source_id));
        let (summary, name, revision, digest, algorithm, value) = claims.swap_remove(0);
        let source_id = summary.source.source_id.clone();

        let Some(meta) = self.expected_dat_inventory_meta(&source_id)? else {
            return Ok(Err(Refusal::InventoryMissing));
        };
        if meta.source_revision.as_deref() != Some(revision.as_str())
            || self.expected_dat_inventory_sha256(&source_id)?.as_deref() != Some(digest.as_str())
            || meta.duplicate_names_skipped != 0
        {
            return Ok(Err(Refusal::InventoryStale));
        }
        let entries = self.expected_dat_canonical_identities(&source_id)?;
        if !entries.contains(&name) {
            return Ok(Err(Refusal::EntryNotInInventory));
        }
        let metadata: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT metadata_json FROM dat_expected_entries \
                 WHERE dat_source_id = ?1 AND canonical_identity = ?2",
                params![source_id, name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| db_error("failed to read DAT entry metadata", error))?;
        let rom_count = metadata
            .and_then(|bytes| serde_json::from_slice::<ExpectedDatEntryMetadata>(&bytes).ok())
            .map(|metadata| metadata.rom_count)
            .ok_or_else(|| ArchiveFsError::Database("DAT entry metadata has no ROM count".into()));
        let rom_count = match rom_count {
            Ok(count) => count,
            Err(_) => return Ok(Err(Refusal::EntryNotInInventory)),
        };
        // Standalone completeness is a separate fact from the hash match:
        // only the matched catalogue's own entry list can establish it.
        let convention = catalogue_convention(
            [meta.ecosystem, summary.source.ecosystem],
            &summary.source.catalogue_names,
        );
        let standalone = match prove_single_disk(
            &name,
            rom_count,
            summary.canonical.canonical_rom_name.as_deref(),
            entries.iter().map(String::as_str),
            convention,
        ) {
            Ok(standalone) => standalone,
            Err(refusal) => return Ok(Err(refusal)),
        };
        Ok(Ok(AmigaAdfLaunchIdentity {
            game_key: adf_game_key(&algorithm, &value),
            source_id,
            source_revision: revision,
            catalogue_sha256: digest,
            entry_name: name,
            matched_algorithm: algorithm,
            matched_value: value,
            standalone,
        }))
    }
}

fn unchanged(path: &Path, size: Option<u64>, modified: Option<i64>) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    let now = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);
    metadata.is_file() && Some(metadata.len()) == size && now.is_some() && now == modified
}
