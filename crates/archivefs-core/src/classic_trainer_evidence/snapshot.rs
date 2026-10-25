//! Source-aware launch authority for the existing uncompressed v1 48K ZX
//! snapshot scope. A filename, trainer pin or caller-supplied boolean cannot
//! construct this proof. Readiness, preflight and the final spawn share it.
use super::{
    exact_game_identities, hex_sha256,
    pinning::{Observation, observe},
};
use crate::dat::configured_catalogues::load_configured_catalogues;
use crate::dat::library_identity_summary::{
    DatProvenanceFreshness, DatVerificationState, DiscFileState,
};
use crate::launch::{CanonicalIdentityStatus, EvidenceFreshness, ResolvedIdentity};
use crate::safe_read::TrustedRoots;
use crate::{Config, Database};
use sha2::Digest;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SnapshotEvidence {
    path: PathBuf,
    database: PathBuf,
    application_config: PathBuf,
    dat_config: PathBuf,
    observation: Observation,
    /// Complete saved claims and current catalogue digests: re-audits and
    /// provenance changes invalidate the old authorization too.
    claims: Vec<(String, String, String)>,
    roots: Vec<PathBuf>,
    guards: Vec<ReadGuard>,
}
// Guard stamps are refreshed on each resolution. Semantic equality remains
// tied to the exact authority, not unrelated writes elsewhere in the DB.
impl PartialEq for SnapshotEvidence {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.database == other.database
            && self.application_config == other.application_config
            && self.dat_config == other.dat_config
            && self.observation == other.observation
            && self.claims == other.claims
            && self.roots == other.roots
    }
}
impl Eq for SnapshotEvidence {}

type FileStamp = (u64, u64, u64, i64, i64, i64, i64, u64);
#[derive(Clone, Debug)]
struct ReadGuard {
    path: PathBuf,
    stamp: Option<FileStamp>,
}
impl ReadGuard {
    fn capture(path: &Path) -> Result<Self, String> {
        use std::os::unix::fs::MetadataExt;
        let stamp = match std::fs::symlink_metadata(path) {
            Ok(m) if m.is_file() && m.nlink() == 1 => Some((
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
                m.nlink(),
            )),
            Ok(_) => return Err("Unsafe snapshot evidence file.".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        Ok(Self {
            path: path.into(),
            stamp,
        })
    }
    fn check(&self) -> Result<(), String> {
        if Self::capture(&self.path)?.stamp != self.stamp {
            return Err("Snapshot evidence changed during resolution or before spawn.".into());
        }
        Ok(())
    }
}
impl SnapshotEvidence {
    pub fn current(path: &Path) -> Result<Self, String> {
        Self::load_at(
            path,
            &crate::default_database_path().map_err(|e| e.to_string())?,
            &crate::default_config_path().map_err(|e| e.to_string())?,
            &crate::dat::sources::default_dat_sources_config_path().map_err(|e| e.to_string())?,
        )
    }
    /// Explicit paths support disposable environments; still requires real,
    /// current, source-aware Gold database evidence and actual matching bytes.
    pub fn load_at(
        path: &Path,
        database: &Path,
        application_config: &Path,
        dat_config: &Path,
    ) -> Result<Self, String> {
        Self::load_at_inner(path, database, application_config, dat_config, || {})
    }
    fn load_at_inner(
        path: &Path,
        database: &Path,
        application_config: &Path,
        dat_config: &Path,
        after_catalogues: impl FnOnce(),
    ) -> Result<Self, String> {
        if [database, application_config, dat_config]
            .iter()
            .any(|p| !p.is_absolute())
        {
            return Err("Snapshot authority requires absolute evidence-store paths.".into());
        }
        if !path.is_absolute()
            || !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("z80"))
        {
            return Err("ZX launch evidence requires a direct .z80 snapshot.".into());
        }
        let mut guards = vec![
            ReadGuard::capture(application_config)?,
            ReadGuard::capture(dat_config)?,
            ReadGuard::capture(path)?,
        ];
        let config = Config::load_from(application_config).map_err(|e| e.to_string())?;
        let roots = TrustedRoots::from_paths(&config.source_folders);
        let parent = path.parent().ok_or("Snapshot has no source folder.")?;
        crate::patch_manager::validate_destination_root(parent).map_err(|e| e.to_string())?;
        if !roots.contains_canonical(parent) {
            return Err("Snapshot source root was withdrawn.".into());
        }
        let (bytes, observation) = observe(path, &roots, 49182)?;
        if bytes.len() != 49182 || bytes[6..8] == [0, 0] || bytes[12] & 32 != 0 {
            return Err(super::launch::LIMITATION.into());
        }
        let db = Database::open_read_only(database).map_err(|e| e.to_string())?;
        // Start the SQLite read snapshot before capturing DB/WAL guards; a
        // read-only WAL attachment may legitimately create an empty WAL.
        let _ = db
            .find_archive_id_by_absolute_path(path)
            .map_err(|e| e.to_string())?;
        guards.push(ReadGuard::capture(database)?);
        guards.push(ReadGuard::capture(&PathBuf::from(format!(
            "{}-wal",
            database.display()
        )))?);
        let row = db
            .load_archives()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.absolute_path == path)
            .ok_or("Snapshot is no longer in the library.")?;
        if row
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias)
            != Some("ZX Spectrum")
            || EvidenceFreshness::from_stored_health(&row.last_known_health).is_incomplete()
            || row.last_verified_missing_at.is_some()
        {
            return Err(
                "A current, completely inspected ZX Spectrum library identity is required.".into(),
            );
        }
        if !db
            .list_source_folders()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|r| r.id == row.source_folder_id)
        {
            return Err("Library source folder was withdrawn.".into());
        }
        // Gold also consults the managed-source registry and installation
        // states. Bind those policy inputs before it resolves catalogue bytes.
        let managed_config = crate::dat::managed_sources::default_managed_dat_sources_config_path()
            .map_err(|e| e.to_string())?;
        guards.push(ReadGuard::capture(&managed_config)?);
        let managed = crate::dat::managed_sources::load_managed_dat_sources_default()
            .map_err(|e| e.to_string())?;
        let managed_root = crate::dat::updates::managed_dat_root().map_err(|e| e.to_string())?;
        for descriptor in managed.descriptors().map_err(|e| e.to_string())? {
            let state =
                crate::dat::updates::managed_dat_state_path(&managed_root, descriptor.source_id())
                    .map_err(|e| e.to_string())?;
            guards.push(ReadGuard::capture(&state)?);
        }
        let catalogues = load_configured_catalogues(dat_config);
        after_catalogues();
        let current = db
            .current_library_dat_evidence(&catalogues)
            .map_err(|e| e.to_string())?;
        let hashes = current
            .exact_hashes
            .get(&row.id)
            .filter(|_| current.exact.contains_key(&row.id))
            .ok_or("No current unambiguous DAT-verified snapshot identity.")?;
        let identities = exact_game_identities(hashes);
        if identities.is_empty() {
            return Err("DAT identity has no supported exact hash.".into());
        }
        for identity in identities {
            let (kind, expected) = identity.split_once(':').ok_or("Invalid DAT identity.")?;
            let actual = match kind {
                "sha256" => hex_sha256(&bytes),
                "sha1" => sha1::Sha1::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                "md5" => md5::Md5::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                _ => return Err("Unsupported DAT hash.".into()),
            };
            if actual != expected {
                return Err("Snapshot bytes changed since DAT verification.".into());
            }
        }
        let mut claims = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for saved in db
            .library_dat_identities_for_item(row.id)
            .map_err(|e| e.to_string())?
        {
            if !saved.carries_identity() {
                continue;
            }
            let id = &saved.source.source_id;
            let catalogue = catalogues
                .get(id)
                .ok_or("A snapshot catalogue was withdrawn or is ambiguous.")?;
            let guard = ReadGuard::capture(&catalogue.path)?;
            let catalogue_roots = TrustedRoots::from_paths([catalogue
                .path
                .parent()
                .ok_or("Catalogue has no folder.")?]);
            let (_, observed_catalogue) = observe(
                &catalogue.path,
                &catalogue_roots,
                crate::dat::limits::DatLimits::default().max_file_size,
            )?;
            if Some(&observed_catalogue.sha256) != catalogue.sha256.as_ref() {
                return Err("Catalogue changed during snapshot resolution.".into());
            }
            guards.push(guard);
            let mut summary = db
                .library_dat_identity_summary_for_item(
                    row.id,
                    id,
                    Some(hashes),
                    catalogue.revision.as_deref(),
                    true,
                )
                .map_err(|e| e.to_string())?
                .ok_or("DAT evidence disappeared.")?;
            catalogue.check_saved_identity(&mut summary);
            if summary.provenance_freshness != DatProvenanceFreshness::Current
                || !matches!(&summary.verification_state,DatVerificationState::VerifiedSingleMatch{algorithm} if ["SHA-256","SHA-1","MD5"].contains(&algorithm.as_str()))
                || saved.archive_member.is_some()
                || saved.file_state_for_path(path) != DiscFileState::Unchanged
            {
                return Err("Snapshot DAT evidence is stale, ambiguous or unsupported.".into());
            }
            names.insert(
                summary
                    .canonical
                    .canonical_dat_name
                    .clone()
                    .ok_or("DAT match has no canonical identity.")?,
            );
            claims.push((
                id.clone(),
                catalogue
                    .sha256
                    .clone()
                    .ok_or("Catalogue content identity is unknown.")?,
                serde_json::to_string(&saved).map_err(|e| e.to_string())?,
            ));
        }
        if claims.is_empty() || names.len() != 1 {
            return Err("Snapshot DAT claims are missing or conflicting.".into());
        }
        claims.sort();
        let evidence = Self {
            path: path.into(),
            database: database.into(),
            application_config: application_config.into(),
            dat_config: dat_config.into(),
            observation,
            claims,
            roots: config.source_folders,
            guards,
        };
        evidence.verify_read_guards()?;
        Ok(evidence)
    }
    pub fn identity(&self) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "ZX Spectrum".into(),
            game_key: self.observation.sha256.clone(),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn verify_read_guards(&self) -> Result<(), String> {
        for guard in &self.guards {
            guard.check()?;
        }
        let roots = TrustedRoots::from_paths(&self.roots);
        let parent = self.path.parent().ok_or("Snapshot has no parent.")?;
        crate::patch_manager::validate_destination_root(parent).map_err(|e| e.to_string())?;
        if !roots.contains_canonical(parent) {
            return Err("Snapshot root identity changed.".into());
        }
        Ok(())
    }
    pub fn revalidate(&self) -> Result<Self, String> {
        let current = Self::load_at(
            &self.path,
            &self.database,
            &self.application_config,
            &self.dat_config,
        )?;
        if &current != self {
            return Err(
                "Snapshot identity or source authorization changed; refresh readiness.".into(),
            );
        }
        Ok(current)
    }
}

#[cfg(test)]
mod tests;
