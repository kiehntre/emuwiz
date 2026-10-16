//! Bounded, read-only catalogue binding. Selection/import stays in the shared
//! catalogue infrastructure. Only an explicit user trust decision authorizes
//! a local reference; registration never becomes publisher authentication.
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;

use archivefs_core::dat::catalogue_selection::{CatalogueRef, resolve_catalogue};
use archivefs_core::dat::mame_arcade_join::VerifiedMameDat;
use archivefs_core::dat::model::DatEcosystem;
use archivefs_core::dat::parsers::{mame_listxml::parse_mame_listxml, parse_dat_file};
use archivefs_core::diagnostics::arcade_dat_version::MameVersion;
use archivefs_core::mame_collection_analyser::inspection::SourceStamp;
use archivefs_core::safe_read::{TrustedRoots, open_bounded_read};
use sha2::{Digest, Sha256};

use super::{MameEnvironment, MameInputs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CatalogueBinding {
    pub reference: CatalogueRef,
    pub source_id: String,
    pub display_name: String,
    pub provenance: String,
    pub version: String,
    pub sha256: String,
    pub source: SourceStamp,
}

pub(super) struct SelectedCatalogue {
    pub dat: VerifiedMameDat,
    pub binding: CatalogueBinding,
}

fn pinned_matches(source: &SourceStamp, file: &std::fs::File) -> Result<bool, String> {
    let m = file.metadata().map_err(|e| e.to_string())?;
    Ok(m.is_file()
        && m.dev() == source.device
        && m.ino() == source.inode
        && m.len() == source.bytes
        && m.modified().ok() == source.modified
        && m.ctime() == source.changed_seconds
        && m.ctime_nsec() == source.changed_nanoseconds)
}

pub(super) fn load_selected(
    inputs: &MameInputs,
    environment: &MameEnvironment,
) -> Result<SelectedCatalogue, String> {
    if !inputs.trusted {
        return Err("Choose an installed MAME DAT and explicitly trust it for this reconstruction. Registration alone does not authenticate its publisher.".into());
    }
    #[cfg(test)]
    let inventory = match &environment.catalogue_inventory {
        Some(inventory) => inventory.clone(),
        None => {
            crate::dat_catalogue_picker::load_inventory_snapshot().map_err(|e| e.to_string())?
        }
    };
    #[cfg(not(test))]
    let inventory = {
        let _ = environment;
        crate::dat_catalogue_picker::load_inventory_snapshot().map_err(|e| e.to_string())?
    };
    let mut limits = inventory.inputs().limits;
    limits.max_entries = 100_000;
    limits.max_roms_per_entry = 16_384;
    let mut resolution = inventory.inputs();
    resolution.limits = limits;
    let resolved = resolve_catalogue(&inputs.reference, resolution).map_err(|e| format!("Selected catalogue unavailable: {e}. Import/validate it in DAT Sources, select it and Preview again."))?;
    if resolved.backing_path != inputs.dat {
        return Err("Selected catalogue location changed; select it and Preview again.".into());
    }
    let source = SourceStamp::observe(&resolved.backing_path)?;
    if source.directory || source.bytes > limits.max_file_size || source.modified.is_none() {
        return Err("Choose one readable, bounded MAME arcade catalogue file.".into());
    }
    let opened =
        open_bounded_read(&resolved.backing_path, &TrustedRoots::none()).map_err(|e| e.detail())?;
    let mut file = opened.into_file();
    if !pinned_matches(&source, &file)? {
        return Err("Catalogue changed while opening; Preview again.".into());
    }
    // Parse exactly the bytes whose digest we show, never a separately reopened
    // contested pathname. This private disposable snapshot is not a journal.
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let snapshot_path = directory.path().join("catalogue.xml");
    let mut snapshot = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&snapshot_path)
        .map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    let mut reader = (&mut file).take(source.bytes.saturating_add(1));
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > source.bytes {
            return Err("Catalogue grew while reading; Preview again.".into());
        }
        snapshot
            .write_all(&buffer[..read])
            .map_err(|e| e.to_string())?;
        digest.update(&buffer[..read]);
    }
    snapshot.flush().map_err(|e| e.to_string())?;
    if bytes != source.bytes || !pinned_matches(&source, &file)? || !source.still_matches() {
        return Err("Catalogue changed while reading; Preview again.".into());
    }
    let sha256 = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if let CatalogueRef::ManagedCurrent {
        snapshot_sha256, ..
    } = &inputs.reference
        && snapshot_sha256 != &sha256
    {
        return Err("Managed catalogue digest changed; select it and Preview again.".into());
    }
    let header = parse_dat_file(&snapshot_path, limits)
        .map_err(|e| e.to_string())?
        .dat;
    if header.source.ecosystem != DatEcosystem::MAMEArcade {
        return Err("Reconstruction needs a MAME arcade machine DAT; software lists and other systems are not eligible.".into());
    }
    // Reuse the machine parser so BIOS/device/CHD/sample and clone/ROM-owner
    // distinctions are preserved. Only the supported <mame> listxml shape is
    // accepted; Logiqx <datafile> shapes refuse instead of losing machine metadata.
    let mut parsed = parse_mame_listxml(&snapshot_path, limits)
        .map_err(|e| format!("Use a supported MAME -listxml machine catalogue: {e}"))?
        .dat;
    if parsed.games.is_empty()
        || !parsed.source.parse_warnings.is_empty()
        || !header.source.parse_warnings.is_empty()
    {
        return Err("Catalogue is empty or has parser warnings; validate a complete MAME machine DAT before reconstruction.".into());
    }
    if let (Some(a), Some(b)) = (&parsed.source.version, &header.source.version)
        && a.trim() != b.trim()
    {
        return Err("Catalogue declares conflicting versions; Preview is refused.".into());
    }
    let version = parsed.source.version.as_deref().or(header.source.version.as_deref()).map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 128 && !v.chars().any(char::is_control) && MameVersion::parse(v).is_some())
        .ok_or("Catalogue has no supported declared MAME version. Import a versioned MAME -listxml DAT; no version is guessed.")?.to_string();
    parsed.source.version = Some(version.clone());
    parsed.source.file_path = resolved.backing_path.display().to_string();
    let binding = CatalogueBinding {
        reference: inputs.reference.clone(),
        source_id: resolved.source_id,
        display_name: resolved.display_name,
        provenance: format!("{:?}", resolved.provenance),
        version: version.clone(),
        sha256: sha256.clone(),
        source,
    };
    Ok(SelectedCatalogue {
        dat: VerifiedMameDat {
            parsed,
            path: resolved.backing_path,
            sha256,
            version,
        },
        binding,
    })
}
