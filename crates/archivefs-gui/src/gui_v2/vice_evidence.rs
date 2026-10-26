//! Current DAT evidence for the bounded Blue VICE Play lane; no trainer parsing.
use archivefs_core::dat::{
    configured_catalogues::{ConfiguredCatalogue, load_configured_catalogues},
    library_identity_summary::LibraryItemHashes,
};
use archivefs_core::launch::process_spawn::{CapturedFileIdentity, open_regular_file_nonblocking};
use sha2::Digest;
use std::{collections::HashMap, io::Read, path::Path};

pub(crate) const MAX_CARTRIDGE_BYTES: u64 = 1024 * 1024;

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Evidence {
    pub hashes: LibraryItemHashes,
    pub sha256: String,
    pub file: CapturedFileIdentity,
    pub catalogues: Vec<(String, Option<String>)>,
}

pub(crate) fn current(path: &Path) -> Result<Evidence, String> {
    let database = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
    let config = archivefs_core::dat::sources::default_dat_sources_config_path()
        .map_err(|e| e.to_string())?;
    inspect(path, &database, &load_configured_catalogues(&config))
}

/// Reuses Blue's source-aware Gold projection. A saved filename/platform is
/// insufficient: exact current DAT hashes must also agree with the actual file.
pub(crate) fn inspect(
    path: &Path,
    database: &Path,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
) -> Result<Evidence, String> {
    if !path.is_absolute()
        || !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("crt"))
    {
        return Err("VICE Play currently requires a direct .crt cartridge.".into());
    }
    let library = super::backend::load_library_with_catalogues(database, catalogues)?;
    let mut games = library
        .games
        .iter()
        .filter(|g| g.archive.absolute_path == path);
    let game = games
        .next()
        .ok_or("The cartridge is no longer in the library.")?;
    if games.next().is_some()
        || game.platform != "Commodore 64"
        || game.inspection_incomplete
        || game.dat_exact.is_none()
        || game.dat_state != Some(archivefs_core::LibraryDatState::Verified)
    {
        return Err(
            "A current, unambiguous exact C64 DAT match is required; recheck the library.".into(),
        );
    }
    let hashes = game
        .dat_exact_hashes
        .clone()
        .ok_or("Exact DAT hashes are unavailable.")?;
    let db = archivefs_core::Database::open_read_only(database).map_err(|e| e.to_string())?;
    let mut claims = 0;
    for saved in db
        .library_dat_identities_for_item(game.archive.id)
        .map_err(|e| e.to_string())?
    {
        if !saved.carries_identity() {
            continue;
        }
        let source = catalogues
            .get(&saved.source.source_id)
            .ok_or("A saved identity source is unavailable; recheck the DAT evidence.")?;
        let mut summary = db
            .library_dat_identity_summary_for_item(
                game.archive.id,
                &saved.source.source_id,
                Some(&hashes),
                source.revision.as_deref(),
                true,
            )
            .map_err(|e| e.to_string())?
            .ok_or("Saved DAT identity is unavailable.")?;
        source.check_saved_identity(&mut summary);
        if summary.provenance_freshness
            != archivefs_core::dat::library_identity_summary::DatProvenanceFreshness::Current
            || !summary.is_verified()
            || saved.archive_member.is_some()
            || saved.audited_hashes != hashes
            || archivefs_core::dat::identity::dat_platform_from_catalogue_names(
                &saved.source.catalogue_names,
                saved.source.ecosystem,
            )
            .platform()
                != Some("Commodore 64")
        {
            return Err(
                "DAT identities are stale, ambiguous, or do not establish a direct C64 cartridge."
                    .into(),
            );
        }
        claims += 1;
    }
    if claims == 0 {
        return Err("No current C64 DAT identity remains.".into());
    }
    let (bytes, file) = read_regular(path, MAX_CARTRIDGE_BYTES)?;
    archivefs_core::disk_format::crt::verify_standard_8k_rom(path, &bytes)?;
    let sha256 = crate::gui_v2::vice_evidence::hex(&sha2::Sha256::digest(&bytes));
    let actual = [
        (hashes.sha256.as_deref(), sha256.clone()),
        (
            hashes.sha1.as_deref(),
            crate::gui_v2::vice_evidence::hex(&sha1::Sha1::digest(&bytes)),
        ),
        (
            hashes.md5.as_deref(),
            crate::gui_v2::vice_evidence::hex(&md5::Md5::digest(&bytes)),
        ),
    ];
    if hashes.size_bytes != Some(bytes.len() as u64)
        || !actual.iter().any(|(expected, _)| expected.is_some())
        || actual
            .iter()
            .any(|(expected, found)| expected.is_some_and(|e| !e.eq_ignore_ascii_case(found)))
    {
        return Err("Cartridge bytes differ from the current exact DAT evidence.".into());
    }
    let mut catalogues: Vec<_> = catalogues
        .values()
        .map(|c| (c.source_id.clone(), c.sha256.clone()))
        .collect();
    catalogues.sort();
    Ok(Evidence {
        hashes,
        sha256,
        file,
        catalogues,
    })
}

/// Nonblocking, no-follow bounded read; never opens a FIFO or imported command.
pub(crate) fn read_regular(
    path: &Path,
    maximum: u64,
) -> Result<(Vec<u8>, CapturedFileIdentity), String> {
    let before = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    use std::os::unix::fs::MetadataExt;
    if !before.is_file() || before.nlink() != 1 || before.len() > maximum {
        return Err("Expected a bounded regular file with one link.".into());
    }
    let mut file = open_regular_file_nonblocking(path).map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    let stamp =
        |m: &std::fs::Metadata| (CapturedFileIdentity::capture(m), m.ctime(), m.ctime_nsec());
    let mut bytes = Vec::new();
    (&mut file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = file.metadata().map_err(|e| e.to_string())?;
    let named = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > maximum
        || stamp(&before) != stamp(&opened)
        || stamp(&opened) != stamp(&after)
        || stamp(&after) != stamp(&named)
    {
        return Err("File changed while checking; refresh VICE readiness.".into());
    }
    Ok((bytes, CapturedFileIdentity::capture(&after)))
}

#[cfg(test)]
pub(crate) mod tests;
